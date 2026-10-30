//! Recover abandoned delivery claims.
//!
//! A delivery is claimed by a lock that the claiming worker holds in its
//! database session while the notifier works, so a worker that dies gives the
//! claim back with its connection, and nothing has to be reset by hand. Two
//! things can still leave a claim standing: a worker whose machine vanished
//! without its connection being closed, which the database notices only after
//! a long time; and a worker that is wedged. The database ends a claiming
//! transaction by itself when it sits idle for far longer than any delivery
//! may take, and this module ends the sessions that are found holding claims
//! for longer than that, so that their rows are taken up at once.
//!
//! At start, a worker that finds that no worker has reported for a while knows
//! that every claim there is belongs to one that is gone, and ends them after a
//! short grace; otherwise it ends only those that are stale.
//!
//! A worker does not apply the schema: the API does, when it starts, and the
//! worker may start first. It waits for the schema it was built for.

use chrono::Duration as Span;
use orch_db::queries::notifications::{outbox, upkeep};
use orch_db::{Db, DbError, heartbeat};
use orch_types::Clock;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How long a claim may sit idle before it is taken for a worker's that is
/// gone: more than a delivery may take (the longest is five minutes) with room
/// to spare, and well under the time after which the database ends the
/// session itself.
pub const STALE_CLAIM_IDLE: Duration = Duration::from_secs(600);

/// How long a claim is left alone at start when no worker is alive: a worker
/// that started a moment ago may have made it.
pub const RESTART_GRACE: Duration = Duration::from_secs(30);

/// How long a worker may be silent before it is taken for dead: three of the
/// pauses between its reports.
pub const DEAD_AFTER: Span = Span::seconds(45);

/// How long a row may have been due before it is counted as one nobody has been
/// taking.
const LATE: Duration = Duration::from_secs(60);

/// How long a worker that started before its schema is complete waits between
/// two looks: the API applies the migrations within seconds, and a look is one
/// query.
pub const SCHEMA_POLL: Duration = Duration::from_secs(3);

/// What a start brought back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Recovered {
    /// Sessions of the database that held claims and were ended.
    pub sessions_ended: u32,
    /// Rows that were due long ago and are waiting to be taken up.
    pub overdue: i64,
}

/// Ends the sessions that have held claims on the outbox, idle, for longer than
/// `idle_for`. Says how many.
pub async fn end_stale_claims(db: &Db, idle_for: Duration) -> Result<u32, DbError> {
    let mut conn = db.acquire().await?;
    let mut ended = 0;
    for claim in outbox::stale_claims(&mut conn, idle_for).await? {
        tracing::warn!(
            pid = claim.pid,
            idle_secs = claim.idle_secs,
            "ending a session that holds claims on the outbox and does nothing"
        );
        if outbox::end_session(&mut conn, claim.pid).await? {
            ended += 1;
        }
    }
    Ok(ended)
}

/// What a worker does when it starts, before it serves: brings back the claims
/// that a worker that is gone left, and counts the rows that wait. Called
/// before the worker reports itself, so that it sees whether another did lately.
pub async fn at_start(db: &Db, clock: &dyn Clock) -> Result<Recovered, DbError> {
    let now = clock.now();
    let reports = {
        let mut conn = db.acquire().await?;
        heartbeat::all(&mut conn).await?
    };
    let another_is_alive = reports.iter().any(|report| {
        report.component == crate::heartbeat::COMPONENT && now - report.seen_at < DEAD_AFTER
    });
    let idle_for = if another_is_alive {
        STALE_CLAIM_IDLE
    } else {
        RESTART_GRACE
    };
    let sessions_ended = end_stale_claims(db, idle_for).await?;
    let mut conn = db.acquire().await?;
    let overdue = outbox::overdue(&mut conn, now, LATE).await?;
    Ok(Recovered {
        sessions_ended,
        overdue,
    })
}

/// Waits until the database has applied the migration numbered `shipped`, the
/// newest the release carries. Says false where it was told to stop first.
pub async fn wait_for_schema(
    db: &Db,
    shipped: i64,
    poll: Duration,
    stop: &CancellationToken,
) -> Result<bool, DbError> {
    let mut told = false;
    loop {
        let applied = {
            let mut conn = db.acquire().await?;
            upkeep::applied_schema_version(&mut conn).await?
        };
        if applied.is_some_and(|version| version >= shipped) {
            return Ok(true);
        }
        if !told {
            tracing::info!(
                ?applied,
                shipped,
                "waiting for the API to bring the schema up to date"
            );
            told = true;
        }
        tokio::select! {
            () = stop.cancelled() => return Ok(false),
            () = tokio::time::sleep(poll) => {}
        }
    }
}
