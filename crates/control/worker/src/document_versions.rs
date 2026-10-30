//! Cutting idle document versions through the document service's existing locked history path.

use crate::schedule::{self, Job};
use orch_config::ServerConfig;
use orch_db::queries::documents::{self, history};
use orch_db::queries::transaction_guard;
use orch_db::{Db, DbError};
use orch_types::Clock;

/// A document-sized batch keeps the scheduler transaction short; another run picks up the next.
const BATCH: i64 = 200;

/// Cuts exactly one version for each changed document that remains idle under its lock.
pub async fn run(
    db: &Db,
    clock: &dyn Clock,
    config: &ServerConfig,
) -> Result<Option<u64>, DbError> {
    // The lease the API's run of this job takes too, so that one process cuts at a time.
    let Some(lease) = schedule::take_lease(db, Job::DocumentVersions, clock).await? else {
        return Ok(None);
    };
    let cuts = cut(db, clock, config).await;
    lease.release(db).await?;
    cuts.map(Some)
}

/// The cut, in one short transaction under the gate every action takes.
async fn cut(db: &Db, clock: &dyn Clock, config: &ServerConfig) -> Result<u64, DbError> {
    let mut tx = transaction_guard::scope(false, db.begin()).await?;
    transaction_guard::lock_action(&mut tx).await?;
    let now = clock.now();
    let idle = i64::try_from(config.documents.version_idle_secs).unwrap_or(i64::MAX);
    let candidates = history::left_alone(&mut tx, now, idle, BATCH).await?;
    let mut cuts = 0;
    for document in candidates {
        if history::cut_if_left_alone(&mut tx, document, now, idle)
            .await?
            .is_some()
        {
            documents::touch(&mut tx, document).await?;
            cuts += 1;
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(cuts)
}
