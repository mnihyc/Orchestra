//! The documents' scheduled jobs: a version of each document that was left alone, and the daily purge of what no document needs any more.
//!
//! Two jobs run here, each under a lease of its own in the database as the cache's jobs do, so that
//! of however many API processes and workers there are, one runs each at a time.
//!
//! - `document_versions`, every minute: a document that changed since its last version and that
//!   nobody has written in for `[documents] version_idle_secs` gets a version, so that the state
//!   somebody leaves behind is itself one, and not only the open end of its history.
//! - `document_purge`, every day: the rows of blocks removed longer ago than `[retention]
//!   soft_delete_days` go, and each document says up to where, so that a reader who would go on
//!   from below that loads it again; the records of accepted batches older than a day go; and so
//!   do the files that nothing names any more: no block of their document, live or removed and not
//!   yet purged (a removed block can come back), and no version the document keeps (a version can
//!   be put back). Such a file lets go of the pin it held on its bytes, as taking it out by hand
//!   does, and the collection of content removes the bytes once their grace has run out. A file put
//!   in less than `[retention] blob_grace_hours` ago is never taken: it may be the upload of a block
//!   that is about to name it.
//!
//! A document that is purged as a whole lets go of the pins of its files where soft-deleted rows
//! are purged, in the database's own upkeep.

use crate::cache::{interval_of, locked, run_every};
use crate::state::AppState;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use orch_config::ServerConfig;
use orch_db::queries::documents::{self as rows, history};
use orch_db::{Db, DbError};
use orch_types::Clock;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The key of the cut of versions in `[schedule]` and the name of its lease.
pub const VERSIONS_JOB: &str = "document_versions";

/// The key of the purge in `[schedule]` and the name of its lease.
pub const PURGE_JOB: &str = "document_purge";

/// How often versions are cut when nobody named another interval: every minute, so that a document
/// left alone gets its version within a minute of its idle time.
pub const VERSIONS_EVERY: Duration = Duration::from_secs(60);

/// How often the purge runs when nobody named another interval: once a day; what it removes is
/// counted in days.
pub const PURGE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// How long the record of an accepted batch is kept: a day, far longer than a writer waits before
/// it sends a batch again whose answer it lost.
const BATCH_RECORDS_KEPT: ChronoDuration = ChronoDuration::days(1);

/// How many documents or files a run takes up at most: one that is left is taken by the next run.
const BATCH: i64 = 500;

/// What a run of the purge removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Purged {
    /// The documents whose removed blocks went.
    pub documents: i64,
    /// The records of accepted batches that went.
    pub batch_records: u64,
    /// The files that nothing named and went.
    pub files: u64,
}

/// The moment `span` before `now`; the earliest moment there is where that lies before it, or
/// where the span is more than a moment can say.
fn before(now: DateTime<Utc>, span: Option<ChronoDuration>) -> DateTime<Utc> {
    span.and_then(|span| now.checked_sub_signed(span))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// Cuts a version of each document that changed since its last one and that was left alone for
/// `[documents] version_idle_secs`, each in a transaction of its own under the document's lock.
/// Returns how many were cut.
pub async fn cut_versions(
    db: &Db,
    clock: &dyn Clock,
    config: &ServerConfig,
) -> Result<u64, DbError> {
    let now = clock.now();
    let idle = i64::try_from(config.documents.version_idle_secs).unwrap_or(i64::MAX);
    let due = {
        let mut conn = db.acquire().await?;
        history::left_alone(&mut conn, now, idle, BATCH).await?
    };
    let mut cut = 0;
    for document in due {
        let mut tx = db.begin().await?;
        // Looked at again under the lock: somebody may have written in it meanwhile.
        if history::cut_if_left_alone(&mut tx, document, now, idle)
            .await?
            .is_some()
        {
            cut += 1;
        }
        tx.commit().await.map_err(DbError::from)?;
    }
    Ok(cut)
}

/// Removes what no document needs any more: the rows of blocks removed longer ago than
/// `[retention] soft_delete_days`, the records of batches older than a day, and the files that
/// nothing names and that were put in longer ago than `[retention] blob_grace_hours`, whose pins
/// on their bytes are let go.
pub async fn purge(db: &Db, clock: &dyn Clock, config: &ServerConfig) -> Result<Purged, DbError> {
    let now = clock.now();
    let blocks_before = before(
        now,
        i64::try_from(config.retention.soft_delete_days)
            .ok()
            .and_then(ChronoDuration::try_days),
    );
    let files_before = before(
        now,
        i64::try_from(config.retention.blob_grace_hours)
            .ok()
            .and_then(ChronoDuration::try_hours),
    );
    let mut done = Purged::default();
    {
        let mut tx = db.begin().await?;
        // Blocks first: a file that only a block purged now named is free to go below.
        done.documents = rows::purge_removed_blocks(&mut tx, blocks_before).await?;
        done.batch_records =
            rows::purge_batch_records(&mut tx, before(now, Some(BATCH_RECORDS_KEPT))).await?;
        tx.commit().await.map_err(DbError::from)?;
    }
    let unnamed = {
        let mut conn = db.acquire().await?;
        rows::unnamed_files(&mut conn, files_before, BATCH).await?
    };
    for (document, file) in unnamed {
        let mut tx = db.begin().await?;
        if rows::release_unnamed_file(&mut tx, document, file, files_before, now).await? {
            done.files += 1;
        }
        tx.commit().await.map_err(DbError::from)?;
    }
    Ok(done)
}

/// Starts the two jobs of this module on their schedules, until `stop` is asked for.
pub fn start(app: &Arc<AppState>, stop: &CancellationToken) -> Vec<tokio::task::JoinHandle<()>> {
    let versions = {
        let (app, stop) = (Arc::clone(app), stop.clone());
        tokio::spawn(async move {
            let every = interval_of(&app.config, VERSIONS_JOB, VERSIONS_EVERY);
            run_every(every, &stop, || async {
                locked(&app.db, app.clock.as_ref(), VERSIONS_JOB, || async {
                    let cut = cut_versions(&app.db, app.clock.as_ref(), &app.config).await?;
                    if cut > 0 {
                        tracing::debug!(cut, "documents left alone were given a version");
                    }
                    Ok(())
                })
                .await
            })
            .await;
        })
    };
    let purging = {
        let (app, stop) = (Arc::clone(app), stop.clone());
        tokio::spawn(async move {
            let every = interval_of(&app.config, PURGE_JOB, PURGE_EVERY);
            run_every(every, &stop, || async {
                locked(&app.db, app.clock.as_ref(), PURGE_JOB, || async {
                    let purged = purge(&app.db, app.clock.as_ref(), &app.config).await?;
                    if purged != Purged::default() {
                        tracing::info!(?purged, "what no document needed any more was removed");
                    }
                    Ok(())
                })
                .await
            })
            .await;
        })
    };
    vec![versions, purging]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_span_longer_than_time_can_say_reaches_back_to_the_beginning_and_never_fails() {
        let now = Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap();
        assert_eq!(
            before(now, ChronoDuration::try_days(1)),
            Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
        );
        assert_eq!(before(now, None), DateTime::<Utc>::MIN_UTC);
        assert_eq!(
            before(
                now,
                i64::try_from(u64::MAX)
                    .ok()
                    .and_then(ChronoDuration::try_days)
            ),
            DateTime::<Utc>::MIN_UTC
        );
    }
}
