//! Commands to machines removed a day after they ended, a bounded number at each run.
//!
//! Every command the system sends a machine is a row, with what the machine was asked and what it answered,
//! so that it can be sent again until it is answered. Once a command is over its row stays for [`KEPT_FOR`],
//! for whoever looks at what a machine was asked when a fault is reported and for a job that resumes and reads
//! again how its commands ended; then this job removes it. It removes them in batches of [`BATCH`] rows, each
//! in a short transaction of its own, and at most [`BATCHES_PER_RUN`] batches a run, under the job's lease (no
//! transaction stays open between batches); what is left is taken by the next run. Two kinds of row stay: the kinds whose rows are read back as what a unit or an environment was
//! last given (`RECORD_KINDS` of the database's link queries), which go with their machine, and a command that
//! names a job that is not over.
//!
//! The answer of a read a person makes of a workspace (a listing, a search, the state of a repository) does
//! not wait for this: the API clears it from the row as soon as the one who asked has it.

use crate::schedule::{self, Job};
use chrono::Duration;
use orch_db::queries::{link, transaction_guard};
use orch_db::{Db, DbError};
use orch_types::Clock;

/// How long the row of a command stays after the command ended: a day. Long enough to look at what a machine
/// was asked when somebody reports a fault, and for a job that resumes to read how its commands ended; short
/// enough that the table holds about a day of what machines were asked.
pub const KEPT_FOR: Duration = Duration::hours(24);

/// How many rows one transaction removes: few enough that each holds its locks for a moment.
pub const BATCH: u32 = 1_000;

/// How many batches one run removes at most, so that no run holds the worker for long. At the hourly interval
/// that is 1.2 million commands a day, far above what people and agents ask of machines; a backlog is taken
/// over the runs that follow.
pub const BATCHES_PER_RUN: u32 = 50;

/// Removes the commands that ended more than [`KEPT_FOR`] ago, under the lease of the job, and returns how
/// many. None where another run holds the lease. No transaction stays open between batches.
pub async fn purge(db: &Db, clock: &dyn Clock) -> Result<Option<u64>, DbError> {
    let Some(lease) = schedule::take_lease(db, Job::CommandPurge, clock).await? else {
        return Ok(None);
    };
    let before = clock.now() - KEPT_FOR;
    let mut removed = 0;
    for _ in 0..BATCHES_PER_RUN {
        let mut tx = transaction_guard::scope(false, db.begin()).await?;
        transaction_guard::lock_action(&mut tx).await?;
        let batch = link::purge_finished(&mut tx, before, i64::from(BATCH)).await?;
        tx.commit().await.map_err(DbError::from)?;
        removed += batch;
        if batch < u64::from(BATCH) || !lease.hold(db, clock.now()).await? {
            break;
        }
    }
    lease.release(db).await?;
    Ok(Some(removed))
}
