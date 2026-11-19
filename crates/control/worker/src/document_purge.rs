//! Pruning document history and expired editing records while retaining every referenced file pin.

use crate::schedule::{self, Job};
use crate::spans;
use orch_config::ServerConfig;
use orch_db::queries::{
    documents::{self, history},
    transaction_guard,
};
use orch_db::{Db, DbError};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Clock, Id};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

/// The document maintenance read and its locks are bounded by the same ordinary history-page budget.
const BATCH: i64 = 200;

/// What committed maintenance pages collected, including a pass stopped between pages.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Done {
    /// Documents whose oldest versions were pruned.
    pub histories: u64,
    /// Documents whose removed-block rows were purged.
    pub block_documents: i64,
    /// Expired batch idempotency records.
    pub batches: u64,
    /// File associations no retained content names any more.
    pub files: u64,
}

/// Runs the existing history/removed-block paths, stopping between committed pages on shutdown,
/// then releases only unreferenced document-file pins.
pub async fn run(
    db: &Db,
    clock: &dyn Clock,
    config: &ServerConfig,
    stop: &CancellationToken,
) -> Result<Option<Done>, Error> {
    // A lease, not a transaction held open across the pages: each page commits on its own.
    let Some(lease) = schedule::take_lease(db, Job::DocumentPurge, clock).await? else {
        return Ok(None);
    };
    let now = clock.now();
    let mut done = Done::default();
    let defs = [
        registry::get(keys::DOCS_VERSIONS_KEPT).ok_or(Error::Registry)?,
        registry::get(keys::DOCS_HISTORY_MAX_BYTES).ok_or(Error::Registry)?,
    ];
    let mut after = None;
    while !stop.is_cancelled() {
        let mut tx = transaction_guard::scope(false, db.begin()).await?;
        transaction_guard::lock_action(&mut tx).await?;
        let candidates = documents::maintenance_page(&mut tx, after, BATCH).await?;
        if candidates.is_empty() {
            break;
        }
        let ids: Vec<_> = candidates.iter().map(|(id, _)| *id).collect();
        done.block_documents += documents::purge_removed_blocks_among(
            &mut tx,
            now - spans::days(config.retention.soft_delete_days),
            &ids,
        )
        .await?;
        // Batch ids are promised for a day, independently of the recovery window.
        done.batches +=
            documents::purge_batch_records_among(&mut tx, now - chrono::Duration::days(1), &ids)
                .await?;
        // Resolve a home once within this committed page. Nothing survives into the
        // next page, which must see any settings changes made between commits.
        let mut limits: HashMap<Id, Option<(i64, i64)>> = HashMap::new();
        for (document, node) in &candidates {
            let limits =
                match limits.entry(*node) {
                    std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let resolved =
                            match Snapshot::load(&mut tx, Place::Node(*node), Wanted::Keys(&defs))
                                .await
                            {
                                Ok(snapshot) => Some((
                                    snapshot
                                        .resolve(defs[0])
                                        .value
                                        .and_then(|value| value.as_i64())
                                        .unwrap_or(200),
                                    snapshot
                                        .resolve(defs[1])
                                        .value
                                        .and_then(|value| value.as_i64())
                                        .unwrap_or(16 * 1024 * 1024),
                                )),
                                // A deleted home retains its history until restoration or ordinary hard purge.
                                Err(orch_settings::Error::NotFound) => None,
                                Err(error) => return Err(error.into()),
                            };
                        *entry.insert(resolved)
                    }
                };
            let Some((kept, bytes)) = limits else {
                continue;
            };
            if !history::prune(&mut tx, *document, kept, bytes)
                .await?
                .is_empty()
            {
                done.histories += 1;
                documents::touch(&mut tx, *document).await?;
            }
        }
        after = candidates.last().map(|(id, _)| *id);
        tx.commit().await.map_err(DbError::from)?;
        if stop.is_cancelled() {
            break;
        }
        if !lease.hold(db, clock.now()).await? {
            return Ok(Some(done));
        }
    }
    // A day's grace protects an upload whose referencing edit has not been submitted yet.
    // The document service rechecks all live/removed blocks and kept versions under its row lock.
    while !stop.is_cancelled() {
        let mut tx = transaction_guard::scope(false, db.begin()).await?;
        transaction_guard::lock_action(&mut tx).await?;
        let mut files =
            documents::unnamed_files(&mut tx, now - chrono::Duration::days(1), BATCH).await?;
        if files.is_empty() {
            break;
        }
        files.sort_unstable();
        for (document, file) in files {
            if documents::release_unnamed_file(
                &mut tx,
                document,
                file,
                now - chrono::Duration::days(1),
                now,
            )
            .await?
            {
                done.files += 1;
            }
        }
        tx.commit().await.map_err(DbError::from)?;
        if stop.is_cancelled() {
            break;
        }
        if !lease.hold(db, clock.now()).await? {
            return Ok(Some(done));
        }
    }
    lease.release(db).await?;
    Ok(Some(done))
}

/// Why a maintenance run left its transaction uncommitted.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// An inherited history limit could not be read.
    #[error(transparent)]
    Setting(#[from] orch_settings::Error),
    /// A required registry entry was absent.
    #[error("a document history setting is not registered")]
    Registry,
}
