//! Side threads closed when nobody used them for as long as their place says.
//!
//! A side thread is an aside: it is not meant to last. Every few minutes the side threads that have no turn
//! under way, nothing waiting to be said to them and whose last turn ended (or that were made, if they never
//! had one) longer ago than `thread.side_idle_hours` of their parent's place are closed, as closing one by hand
//! does: the thread is deleted, so that it can be brought back until the days of the soft delete pass, and its
//! parent's history gets one line that says how many turns it had and that it was left alone, never what was
//! said in it. The harness's session of a closed side thread goes with the purge of its items.

use crate::schedule::{self, Job};
use orch_db::queries::threads::forking;
use orch_db::{Db, DbError};
use orch_settings::Place;
use orch_types::Clock;

/// How many hours a side thread may stand idle when the setting has no value: what it ships with.
const DEFAULT_IDLE_HOURS: i64 = 24;

/// How many idle side threads one run looks at; the rest are looked at by the next.
const BATCH: i64 = 200;

/// Why closing the idle side threads failed.
#[derive(Debug, thiserror::Error)]
pub enum SideThreadError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A setting could not be read.
    #[error(transparent)]
    Setting(#[from] orch_settings::Error),
}

/// Closes the side threads that stood idle for as long as their place allows, under the lock of the job.
/// None where another run holds the lock; else how many were closed.
pub async fn close_idle(db: &Db, clock: &dyn Clock) -> Result<Option<usize>, SideThreadError> {
    let Some(mut tx) = schedule::take(db, Job::SideThreads).await? else {
        return Ok(None);
    };
    let now = clock.now();
    let mut closed = 0;
    for side in forking::idle_side_threads(&mut tx, BATCH).await? {
        let hours =
            orch_settings::get::<i64>(&mut tx, Place::Node(side.parent), "thread.side_idle_hours")
                .await?
                .unwrap_or(DEFAULT_IDLE_HOURS);
        if side.idle_since + chrono::Duration::hours(hours.max(1)) <= now
            && forking::close_with_marker(&mut tx, side.thread, "idle", None, None, now)
                .await?
                .is_some()
        {
            closed += 1;
        }
    }
    // Ask uses the same small periodic lifetime pass, with immediate hard deletion
    // at its retention boundary and no soft-delete waiting period.
    for root in orch_db::queries::ask::roots(&mut tx).await? {
        // A recoverably deleted governing root still owns these personal-home
        // conversations. Resolve its retained policy for this cleanup only.
        let (minimum, maximum) = orch_db::queries::trace::read_scope(&[root], async {
            let minimum =
                orch_settings::get::<i64>(&mut tx, Place::Node(root), "ask.retention_min_days")
                    .await?
                    .unwrap_or(1);
            let maximum =
                orch_settings::get::<i64>(&mut tx, Place::Node(root), "ask.retention_max_days")
                    .await?
                    .unwrap_or(3650);
            Ok::<_, orch_settings::Error>((minimum, maximum))
        })
        .await?;
        if minimum < 1 || minimum > maximum {
            continue;
        }
        let minimum = i32::try_from(minimum).map_err(|_| orch_settings::Error::Unreadable {
            what: "Ask retention minimum",
        })?;
        let maximum = i32::try_from(maximum).map_err(|_| orch_settings::Error::Unreadable {
            what: "Ask retention maximum",
        })?;
        let _purged =
            orch_db::queries::ask::purge_root(&mut tx, root, minimum, maximum, now, BATCH).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(closed))
}
