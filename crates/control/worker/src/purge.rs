//! Expired tokens, sign-in codes, browsers nobody signed in from for their time, counts of wrong tries whose window has passed, idempotency keys and soft-deleted database rows; an org's, a provider's and an environment's only once nothing of it is left on a host; a document's removed blocks and the versions past those it keeps.
//!
//! Two jobs. `purge_expired` removes what has run out: tokens and codes past
//! their end, the browsers a person got in from that nobody has signed in from
//! for the days they are known, the counts of wrong tries whose window has
//! passed (and whose person has been told), the answers kept for repeating a
//! request for as long as they are kept, and the codes of a link that was never
//! followed. It also removes the deliveries that are over, a week after the
//! last try of one that went or was given up and a month after one that failed
//! for good, and the notifications of people's Inboxes older than the days of
//! `retention.inbox_days`, read or not; those go in a transaction of their own
//! that holds up nobody's action, a bounded number at each run.
//! `soft_delete_purge` hard-deletes what was soft-deleted longer ago
//! than `[retention] soft_delete_days`, and what something still refers to is
//! left: an org, a provider or an environment goes only once nothing of it is
//! left, which the references of the schema decide and the engine's reaper
//! brings about by removing what is on hosts first.
//!
//! Each runs under the lock of its job. Both are also asked of the clock, never
//! of the system, and remove what is due at that moment and nothing else.

use crate::schedule::{self, Job};
use crate::spans;
use chrono::{DateTime, Duration, Utc};
use orch_config::ServerConfig;
use orch_db::queries::identity;
use orch_db::queries::notifications::routes;
use orch_db::queries::notifications::upkeep::{self, SignInPurged, SoftDeletePurged};
use orch_db::queries::transaction_guard;
use orch_db::{Db, DbError, idempotency};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::Clock;

/// How long a person who was told that strangers' wrong tries filled a count at
/// their name is not told again, and so how long a count is kept after its
/// tries have aged out: a day.
const NOTICE_KEPT: Duration = Duration::days(1);

/// How long a delivery that went, or was given up, is kept after its last try:
/// a week, for whoever looks at what was sent lately or why nothing was.
const SETTLED_KEPT: Duration = Duration::days(7);

/// How long a delivery that failed for good is kept after its last try: a
/// month, so that its person can still see why and send it again.
const FAILED_KEPT: Duration = Duration::days(30);

/// How many deliveries, and how many Inbox rows, one run removes at most. The
/// job runs every ten minutes, so that is over a million of each a day, and a
/// backlog goes over the runs that follow without one run holding the tables.
const ROWS_PER_RUN: i64 = 10_000;

/// For how many days an Inbox keeps a notification where the setting has no
/// value: the setting's own default.
const DEFAULT_INBOX_DAYS: i64 = 90;

/// The fewest days an Inbox keeps a notification, whatever a row of the
/// database says: a value that is not a number of days never empties it now.
const LEAST_INBOX_DAYS: i64 = 1;

/// Why a run of `purge_expired` failed.
#[derive(Debug, thiserror::Error)]
pub enum PurgeError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The setting of how long an Inbox keeps its rows could not be read.
    #[error(transparent)]
    Setting(#[from] orch_settings::Error),
}

/// What a run of `purge_expired` removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Expired {
    /// Tokens, codes, browsers and attempts that had run out.
    pub sign_in: SignInPurged,
    /// Counts of wrong tries whose window has passed.
    pub throttle: u64,
    /// Answers kept for repeating a request, past their time.
    pub idempotency_keys: u64,
    /// Codes for linking a bot that nobody followed in time.
    pub link_codes: u64,
    /// Deliveries that were over long enough ago.
    pub deliveries: u64,
    /// Notifications of people's Inboxes past the days they are kept.
    pub inbox: u64,
}

/// Removes what has run out, under the lock of the job. None where another
/// run holds the lock.
pub async fn expired(
    db: &Db,
    clock: &dyn Clock,
    config: &ServerConfig,
) -> Result<Option<Expired>, PurgeError> {
    let Some(mut tx) = schedule::take(db, Job::PurgeExpired).await? else {
        return Ok(None);
    };
    let now = clock.now();
    let known_device_days = spans::days(config.auth.known_device_days).num_days();
    let sign_in = upkeep::purge_expired_sign_in(&mut tx, now, known_device_days).await?;
    // A count lives as long as the longest window a try is counted in.
    let window = config
        .auth
        .login_failure_window_secs
        .max(config.auth.address_failure_window_secs);
    let throttle =
        identity::prune_throttle(&mut tx, now, spans::seconds(window), NOTICE_KEPT).await?;
    let idempotency_keys = idempotency::purge(&mut tx, now - ttl_of_keys(config)).await?;
    let link_codes = routes::drop_expired_codes(&mut tx, now).await?;
    tx.commit().await.map_err(DbError::from)?;
    let (deliveries, inbox) = deliveries_and_inbox(db, now).await?;
    Ok(Some(Expired {
        sign_in,
        throttle,
        idempotency_keys,
        link_codes,
        deliveries,
        inbox,
    }))
}

/// Removes the deliveries that were over long enough ago and the Inbox rows past the days they are kept, at
/// most [`ROWS_PER_RUN`] of each. They decide nothing of who may do what, so they go under the shared gate
/// that every action takes, in a transaction of their own, and not under the exclusive one of the purge of
/// sign-in material, which would hold up every action while they are deleted.
async fn deliveries_and_inbox(db: &Db, now: DateTime<Utc>) -> Result<(u64, u64), PurgeError> {
    let mut tx = transaction_guard::scope(false, db.begin()).await?;
    transaction_guard::lock_action(&mut tx).await?;
    let days = orch_settings::get::<i64>(&mut tx, Place::System, keys::RETENTION_INBOX_DAYS)
        .await?
        .unwrap_or(DEFAULT_INBOX_DAYS)
        .max(LEAST_INBOX_DAYS);
    let deliveries =
        upkeep::purge_outbox(&mut tx, now - SETTLED_KEPT, now - FAILED_KEPT, ROWS_PER_RUN).await?;
    let inbox = upkeep::purge_inbox(
        &mut tx,
        now - spans::days(days.unsigned_abs()),
        ROWS_PER_RUN,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok((deliveries, inbox))
}

/// How long an answer is kept for repeating a request.
fn ttl_of_keys(config: &ServerConfig) -> Duration {
    spans::hours(config.api.idempotency_ttl_hours)
}

/// The moment before which a row that was soft-deleted is due to be purged.
#[must_use]
pub fn soft_delete_cutoff(now: DateTime<Utc>, config: &ServerConfig) -> DateTime<Utc> {
    now - spans::days(config.retention.soft_delete_days)
}

/// Hard-deletes the rows soft-deleted longer ago than the days they are kept,
/// under the lock of the job. None where another run holds the lock.
pub async fn soft_deleted(
    db: &Db,
    clock: &dyn Clock,
    config: &ServerConfig,
) -> Result<Option<Vec<SoftDeletePurged>>, DbError> {
    let Some(mut tx) =
        orch_db::queries::transaction_guard::scope(true, schedule::take(db, Job::SoftDeletePurge))
            .await?
    else {
        return Ok(None);
    };
    let now = clock.now();
    let purged = upkeep::purge_soft_deleted(&mut tx, soft_delete_cutoff(now, config), now).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(purged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_row_is_due_after_the_days_it_is_kept_and_not_a_moment_before() {
        let mut config = ServerConfig::default();
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
        assert_eq!(
            soft_delete_cutoff(now, &config),
            Utc.with_ymd_and_hms(2026, 9, 11, 12, 0, 0).unwrap()
        );
        config.retention.soft_delete_days = 7;
        assert_eq!(
            soft_delete_cutoff(now, &config),
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
        );
        assert_eq!(ttl_of_keys(&config), Duration::hours(24));
    }
}
