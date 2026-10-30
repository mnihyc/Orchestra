//! The outbox: rows claimed under a lock so that two workers never take one, delivered, retried and finished, and what the engine and the worker write into it.
//!
//! A row is a delivery a notifier is to make: for a person through a route,
//! for a node through a destination, or, for mail that is no notification,
//! through a channel. It is `pending` until it is taken (`sent`) or given up
//! (`failed`). A worker claims the row that is due with `FOR UPDATE SKIP
//! LOCKED` and holds the lock while it delivers, so that another worker skips
//! it, and a worker that dies loses its claim with its connection. The
//! transaction that holds claims is made to end by itself when it sits idle
//! for far longer than any delivery takes ([`begin_claim`]), and
//! [`stale_claims`] finds the ones that outlived such a worker.

use crate::pool::{Db, Tx};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::time::Duration;

macro_rules! outbox_columns {
    () => {
        "id, channel, route_id, destination_id, channel_id, payload, state, attempts, next_attempt_at, last_error, created_at"
    };
}

/// A delivery a notifier is to make.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct OutboxRow {
    /// Its identifier; the same across the tries.
    pub id: Id,
    /// The notifier it goes through.
    pub channel: String,
    /// The route it is for, for a delivery to a person.
    pub route_id: Option<Id>,
    /// The destination it is for, for one to a node's place.
    pub destination_id: Option<Id>,
    /// The channel it goes by, for mail that is no notification.
    pub channel_id: Option<Id>,
    /// What is delivered.
    pub payload: Json,
    /// `pending`, `sent` or `failed`.
    pub state: String,
    /// How many tries were made.
    pub attempts: i32,
    /// When it is next due.
    pub next_attempt_at: DateTime<Utc>,
    /// Why the last try failed.
    pub last_error: Option<String>,
    /// When it was written.
    pub created_at: DateTime<Utc>,
}

impl OutboxRow {
    /// Whether quiet hours held it back: such rows of one route go out as one
    /// message when the window ends.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.payload
            .get("held")
            .and_then(Json::as_bool)
            .unwrap_or(false)
    }
}

/// One notification as an outbox row carries it: as it was when the row was
/// written, so that a delivery never depends on rows that changed since.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Its identifier in the person's Inbox; none for what happens at a node
    /// and is told for nobody in particular.
    #[serde(default)]
    pub id: Option<Id>,
    /// Its kind, as `turn.completed`.
    pub kind: String,
    /// When it happened.
    pub at: Time,
    /// Its title.
    pub title: String,
    /// Its first line.
    pub body: String,
    /// The node it happened at.
    #[serde(default)]
    pub node_id: Option<Id>,
    /// The thread it is about.
    #[serde(default)]
    pub thread_id: Option<Id>,
    /// The approval it is about.
    #[serde(default)]
    pub approval_id: Option<Id>,
    /// The chat it is about.
    #[serde(default)]
    pub chat_id: Option<Id>,
    /// Whatever else its kind carries. A member `actor` names who did it, and
    /// a member `path` is the in-app path that opens it.
    #[serde(default)]
    pub data: Json,
}

/// What the outbox row of a delivery to a person or to a destination holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    /// The notifications the message tells, oldest first. One is a message of
    /// its own; several are told together.
    #[serde(default)]
    pub notifications: Vec<Snapshot>,
    /// Whether quiet hours held the row back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    /// The browsers a try already reached, so that a retry leaves them alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delivered_to: Vec<Id>,
}

impl Payload {
    /// A payload that tells these notifications.
    #[must_use]
    pub fn of(notifications: &[Snapshot]) -> Self {
        Self {
            notifications: notifications.to_vec(),
            ..Self::default()
        }
    }

    /// Reads the payload of a row; none where it is not one of this shape.
    #[must_use]
    pub fn read(payload: &Json) -> Option<Self> {
        serde_json::from_value(payload.clone()).ok()
    }

    /// The payload as it is stored.
    #[must_use]
    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or(Json::Null)
    }
}

/// What a new row is made from.
#[derive(Debug, Clone, Copy)]
pub struct NewRow<'a> {
    /// Its identifier.
    pub id: Id,
    /// The notifier it goes through.
    pub channel: &'a str,
    /// The route it is for.
    pub route_id: Option<Id>,
    /// The destination it is for.
    pub destination_id: Option<Id>,
    /// The channel it goes by.
    pub channel_id: Option<Id>,
    /// What is delivered.
    pub payload: &'a Json,
    /// When it is written and due.
    pub at: DateTime<Utc>,
}

/// Writes a delivery to the outbox, due at once. A delivery to a person keeps the Inbox rows it
/// tells, which every try, every recovery and a digest look at again.
pub async fn insert(conn: &mut Conn, new: &NewRow<'_>) -> Result<(), DbError> {
    let sources: Vec<Id> = if new.route_id.is_some() {
        Payload::read(new.payload)
            .map(|payload| {
                payload
                    .notifications
                    .iter()
                    .filter_map(|each| each.id)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    sqlx::query(
        "INSERT INTO outbox (id, channel, route_id, destination_id, channel_id, payload, notification_ids, state, attempts, next_attempt_at, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $8, 'pending', 0, $7, $7)",
    )
    .bind(new.id)
    .bind(new.channel)
    .bind(new.route_id)
    .bind(new.destination_id)
    .bind(new.channel_id)
    .bind(new.payload)
    .bind(new.at)
    .bind(&sources)
    .execute(conn)
    .await?;
    Ok(())
}

// ------------------------------------------------------------- one attempt, reserved

/// A delivery that is due, as it is found before anything is locked: which route, destination or
/// channel it is for, so that the locks are taken in their order (the channel, the person's
/// choices, the route, then the delivery).
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct Due {
    /// The delivery.
    pub id: Id,
    /// The route it is for.
    pub route_id: Option<Id>,
}

/// The delivery that has waited longest among those due and not reserved, unless none is.
/// Nothing is locked: the caller takes the locks in their order and then the row itself.
pub async fn next_due(
    conn: &mut Conn,
    now: DateTime<Utc>,
    skip: &[Id],
) -> Result<Option<Due>, DbError> {
    // The deliveries a hold kept back are left to their route's one message ([`next_held_route`]).
    Ok(sqlx::query_as(
        "SELECT o.id, o.route_id FROM outbox o
          WHERE o.state = 'pending' AND o.next_attempt_at <= $1 AND NOT (o.id = ANY($2))
            AND COALESCE(o.payload ->> 'held', 'false') <> 'true'
          ORDER BY o.next_attempt_at, o.created_at, o.id LIMIT 1",
    )
    .bind(now)
    .bind(skip)
    .fetch_optional(conn)
    .await?)
}

/// The route whose deliveries a hold kept back are due and have waited longest, unless none is:
/// they go together, as one message. Nothing is locked.
pub async fn next_held_route(
    conn: &mut Conn,
    now: DateTime<Utc>,
    skip: &[Id],
) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT route_id FROM outbox
          WHERE state = 'pending' AND next_attempt_at <= $1 AND route_id IS NOT NULL
            AND payload ->> 'held' = 'true' AND NOT (route_id = ANY($2))
          GROUP BY route_id ORDER BY min(next_attempt_at), route_id LIMIT 1",
    )
    .bind(now)
    .bind(skip)
    .fetch_optional(conn)
    .await?)
}

/// The deliveries a hold kept back for a route that are due, under their locks: none where
/// another worker has them.
pub async fn take_held_of_route(
    conn: &mut Conn,
    route: Id,
    now: DateTime<Utc>,
) -> Result<Vec<OutboxRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        outbox_columns!(),
        " FROM outbox WHERE state = 'pending' AND next_attempt_at <= $1 AND route_id = $2 ",
        "AND payload ->> 'held' = 'true' ORDER BY created_at, id FOR UPDATE SKIP LOCKED"
    ))
    .bind(now)
    .bind(route)
    .fetch_all(conn)
    .await?)
}

/// Makes a person's held deliveries due now, so that a pause they ended, or that ran out, is
/// looked at again at once rather than at the moment it was set to end.
pub async fn rearm_held_of(conn: &mut Conn, user: Id, now: DateTime<Utc>) -> Result<u64, DbError> {
    let done = sqlx::query(
        "UPDATE outbox SET next_attempt_at = $2
          WHERE state = 'pending' AND payload ->> 'held' = 'true' AND next_attempt_at > $2
            AND route_id IN (SELECT id FROM notification_routes WHERE user_id = $1)",
    )
    .bind(user)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// The delivery under its lock, while it is still due and not reserved: none where another
/// worker has it or it changed meanwhile.
pub async fn take_due(
    conn: &mut Conn,
    id: Id,
    now: DateTime<Utc>,
) -> Result<Option<OutboxRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        outbox_columns!(),
        " FROM outbox WHERE id = $1 AND state = 'pending' AND next_attempt_at <= $2 FOR UPDATE SKIP LOCKED"
    ))
    .bind(id)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Reserves a delivery for one attempt by one worker: it is `sending`, with an attempt of its
/// own, the worker's identity and a deadline, and no claim of the due ones takes it. Nothing is
/// held across the remote call: the transaction that reserves it ends first. Returns the
/// attempt's identifier, which recording its result compares.
pub async fn reserve(
    conn: &mut Conn,
    id: Id,
    owner: Id,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<Id>, DbError> {
    let dispatch = orch_types::new_id();
    Ok(reserve_as(conn, id, dispatch, owner, expires_at, now)
        .await?
        .then_some(dispatch))
}

/// Reserves a delivery for an attempt that is given: the deliveries that go together as one
/// message share it. Says whether it was reserved.
pub async fn reserve_as(
    conn: &mut Conn,
    id: Id,
    dispatch: Id,
    owner: Id,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE outbox SET state = 'sending', dispatch_id = $2, dispatch_owner = $3,
                dispatch_expires_at = $4, dispatch_started_at = $5
          WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(dispatch)
    .bind(owner)
    .bind(expires_at)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Locks the rows still reserved for this attempt before applying its answer's side effects.
pub async fn lock_attempt(conn: &mut Conn, ids: &[Id], dispatch: Id) -> Result<bool, DbError> {
    let current: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM outbox WHERE id = ANY($1) AND state = 'sending' AND dispatch_id = $2
         ORDER BY id FOR UPDATE",
    )
    .bind(ids)
    .bind(dispatch)
    .fetch_all(conn)
    .await?;
    Ok(!current.is_empty())
}

/// Records what an attempt came to, only where the delivery is still `sending` under that very
/// attempt: a result that comes back after the attempt was given up, retried or cancelled
/// changes nothing. A try that is to be made again is `pending` again; a try is counted. Says
/// whether it was recorded.
pub async fn record(
    conn: &mut Conn,
    id: Id,
    dispatch: Id,
    how: &Settle<'_>,
) -> Result<bool, DbError> {
    let (state, next, error, payload): (&str, Option<DateTime<Utc>>, Option<&str>, Option<&Json>) =
        match how {
            Settle::Sent { payload } => ("sent", None, None, *payload),
            Settle::Failed { error, payload } => ("failed", None, Some(*error), *payload),
            Settle::Retry { at, error, payload } => ("pending", Some(*at), Some(*error), *payload),
            Settle::Hold { until, payload } => ("pending", Some(*until), None, Some(*payload)),
        };
    let counted = !matches!(how, Settle::Hold { .. });
    let done = sqlx::query(
        "UPDATE outbox SET state = $3, attempts = attempts + CASE WHEN $7 THEN 1 ELSE 0 END,
                next_attempt_at = COALESCE($4, next_attempt_at), last_error = $5,
                payload = COALESCE($6, payload), dispatch_owner = NULL, dispatch_expires_at = NULL
          WHERE id = $1 AND state = 'sending' AND dispatch_id = $2",
    )
    .bind(id)
    .bind(dispatch)
    .bind(state)
    .bind(next)
    .bind(error)
    .bind(payload)
    .bind(counted)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Leaves deliveries that quiet hours or a pause hold where they are, unreserved, marked as held
/// and with no try counted, until `until`: they keep their Inbox rows, and go together as one
/// message when the hold is over.
pub async fn hold_unreserved(
    conn: &mut Conn,
    ids: &[Id],
    until: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE outbox SET next_attempt_at = $2, payload = jsonb_set(payload, '{held}', 'true'::jsonb)
          WHERE id = ANY($1) AND state = 'pending'",
    )
    .bind(ids)
    .bind(until)
    .execute(conn)
    .await?;
    Ok(())
}

/// Gives up a delivery that is no longer to go, for a reason that is no failure of it: `read`,
/// `coalesced`, `route_off`, `channel_removed` or `lost_access`. A reservation it had is void.
pub async fn cancel(conn: &mut Conn, id: Id, reason: &str) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE outbox SET state = 'cancelled', cancel_reason = $2, dispatch_owner = NULL, dispatch_expires_at = NULL
          WHERE id = $1 AND state IN ('pending', 'sending')",
    )
    .bind(id)
    .bind(reason)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// What the last error of an attempt that nobody confirmed reads: it may have been delivered.
pub const UNCONFIRMED: &str =
    "the delivery was not confirmed before its deadline; it may have gone";

/// Returns to the retry path every reservation whose deadline has passed, a worker's that ended
/// among them: the attempt counts, and it is tried again now, since a send that was accepted but
/// not confirmed may go twice rather than never. A live worker's reservation is left alone until
/// its deadline. Returns how many.
pub async fn recover_abandoned(conn: &mut Conn, now: DateTime<Utc>) -> Result<u64, DbError> {
    let done = sqlx::query(
        "UPDATE outbox SET state = 'pending', attempts = attempts + 1, next_attempt_at = $1, last_error = $2,
                dispatch_owner = NULL, dispatch_expires_at = NULL
          WHERE state = 'sending' AND dispatch_expires_at <= $1",
    )
    .bind(now)
    .bind(UNCONFIRMED)
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// The deliveries of a route that wait and have not gone: those a hold kept back, which a digest
/// takes together.
pub async fn pending_of_route(conn: &mut Conn, route: Id) -> Result<Vec<OutboxRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        outbox_columns!(),
        " FROM outbox WHERE route_id = $1 AND state = 'pending' ORDER BY created_at, id FOR UPDATE"
    ))
    .bind(route)
    .fetch_all(conn)
    .await?)
}

/// The Inbox rows the failed deliveries of a route told, for a person's explicit retry: the
/// failed deliveries are then given up as coalesced into the new one.
pub async fn failed_sources_of_route(conn: &mut Conn, route: Id) -> Result<Vec<Id>, DbError> {
    let ids: Vec<Vec<Id>> = sqlx::query_scalar(
        "SELECT notification_ids FROM outbox WHERE route_id = $1 AND state = 'failed' ORDER BY created_at, id",
    )
    .bind(route)
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE outbox SET state = 'cancelled', cancel_reason = 'coalesced' WHERE route_id = $1 AND state = 'failed'",
    )
    .bind(route)
    .execute(conn)
    .await?;
    let mut all: Vec<Id> = ids.into_iter().flatten().collect();
    all.sort_unstable();
    all.dedup();
    Ok(all)
}

/// Begins the transaction that holds claims. It ends by itself, and its locks
/// with it, when it sits idle for `idle_limit`: far longer than any delivery
/// is allowed to take, so that a worker that went quiet without closing its
/// connection cannot keep rows from the others.
pub async fn begin_claim(db: &Db, idle_limit: Duration) -> Result<Tx, DbError> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT set_config('idle_in_transaction_session_timeout', $1, true)")
        .bind(format!("{}s", idle_limit.as_secs().max(1)))
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

/// Claims the row that is due and has waited longest, unless another worker
/// holds it. The claim lasts as long as the transaction. Rows that quiet hours
/// held are left to [`claim_held_group`], which takes them together.
pub async fn claim_due(conn: &mut Conn, now: DateTime<Utc>) -> Result<Option<OutboxRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        outbox_columns!(),
        " FROM outbox WHERE state = 'pending' AND next_attempt_at <= $1 ",
        "AND COALESCE(payload ->> 'held', 'false') <> 'true' ",
        "ORDER BY next_attempt_at, created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED"
    ))
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Claims all the rows that quiet hours held for one route and that are due, to
/// go out as one message. The route is taken under a lock of its own first, in
/// the transaction, so that of the workers that find its rows due one takes
/// them all and the others leave the route alone. Empty where no route has
/// held rows that are due, or every such route is being served.
pub async fn claim_held_group(
    conn: &mut Conn,
    now: DateTime<Utc>,
) -> Result<Vec<OutboxRow>, DbError> {
    let routes: Vec<Id> = sqlx::query_scalar(
        "SELECT DISTINCT route_id FROM outbox
          WHERE state = 'pending' AND next_attempt_at <= $1 AND route_id IS NOT NULL
            AND payload ->> 'held' = 'true'
          ORDER BY route_id",
    )
    .bind(now)
    .fetch_all(&mut *conn)
    .await?;
    for route in routes {
        let taken: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(format!("orch.outbox.route.{route}"))
                .fetch_one(&mut *conn)
                .await?;
        if !taken {
            continue;
        }
        let group: Vec<OutboxRow> = sqlx::query_as(concat!(
            "SELECT ",
            outbox_columns!(),
            " FROM outbox WHERE state = 'pending' AND next_attempt_at <= $1 AND route_id = $2 ",
            "AND payload ->> 'held' = 'true' ORDER BY created_at, id FOR UPDATE SKIP LOCKED"
        ))
        .bind(now)
        .bind(route)
        .fetch_all(&mut *conn)
        .await?;
        if !group.is_empty() {
            return Ok(group);
        }
    }
    Ok(Vec::new())
}

/// What a try of a row came to.
#[derive(Debug, Clone, Copy)]
pub enum Settle<'a> {
    /// The far side took it.
    Sent {
        /// The row's payload as it is kept from now on, where it changes.
        payload: Option<&'a Json>,
    },
    /// It failed for good.
    Failed {
        /// Why.
        error: &'a str,
        /// The row's payload as it is kept from now on, where it changes.
        payload: Option<&'a Json>,
    },
    /// It is tried again.
    Retry {
        /// When.
        at: DateTime<Utc>,
        /// Why this try failed.
        error: &'a str,
        /// The row's payload as it is kept from now on, where it changes.
        payload: Option<&'a Json>,
    },
    /// Quiet hours hold it until they end. No try is counted.
    Hold {
        /// When it is due again.
        until: DateTime<Utc>,
        /// The row's payload, marked as held.
        payload: &'a Json,
    },
}

/// Records what a try came to for a row that is still pending. A try is
/// counted, except for a row that is only held. Says whether the row was
/// still pending, which it is for whoever holds its claim.
pub async fn settle(conn: &mut Conn, id: Id, how: &Settle<'_>) -> Result<bool, DbError> {
    let done =
        match how {
            Settle::Sent { payload } => {
                sqlx::query(
                    "UPDATE outbox SET state = 'sent', attempts = attempts + 1, last_error = NULL,
                        payload = COALESCE($2, payload)
                  WHERE id = $1 AND state = 'pending'",
                )
                .bind(id)
                .bind(*payload)
                .execute(conn)
                .await?
            }
            Settle::Failed { error, payload } => {
                sqlx::query(
                    "UPDATE outbox SET state = 'failed', attempts = attempts + 1, last_error = $2,
                        payload = COALESCE($3, payload)
                  WHERE id = $1 AND state = 'pending'",
                )
                .bind(id)
                .bind(*error)
                .bind(*payload)
                .execute(conn)
                .await?
            }
            Settle::Retry { at, error, payload } => sqlx::query(
                "UPDATE outbox SET attempts = attempts + 1, next_attempt_at = $2, last_error = $3,
                        payload = COALESCE($4, payload)
                  WHERE id = $1 AND state = 'pending'",
            )
            .bind(id)
            .bind(*at)
            .bind(*error)
            .bind(*payload)
            .execute(conn)
            .await?,
            Settle::Hold { until, payload } => {
                sqlx::query(
                    "UPDATE outbox SET next_attempt_at = $2, payload = $3
                  WHERE id = $1 AND state = 'pending'",
                )
                .bind(id)
                .bind(*until)
                .bind(*payload)
                .execute(conn)
                .await?
            }
        };
    Ok(done.rows_affected() == 1)
}

/// How many deliveries wait, by notifier.
pub async fn pending_by_channel(conn: &mut Conn) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT channel, count(*) FROM outbox WHERE state = 'pending' GROUP BY channel ORDER BY channel",
    )
    .fetch_all(conn)
    .await?)
}

/// How many deliveries were due more than `late` before `now` and are still
/// pending: what a worker finds that nobody has been taking.
pub async fn overdue(conn: &mut Conn, now: DateTime<Utc>, late: Duration) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM outbox
          WHERE state = 'pending' AND next_attempt_at <= $1 - make_interval(secs => $2)",
    )
    .bind(now)
    .bind(late.as_secs_f64())
    .fetch_one(conn)
    .await?)
}

/// A session that holds claims and has done nothing for a long time.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct StaleClaim {
    /// The session's process in the database.
    pub pid: i32,
    /// How long it has been idle, in seconds.
    pub idle_secs: f64,
}

/// The sessions of this database that hold claims on the outbox and have sat
/// idle in their transaction for longer than `idle_for`, as the database
/// measures time: no live worker sits idle in a claim for longer than a
/// delivery takes, so these belong to workers that are gone.
pub async fn stale_claims(conn: &mut Conn, idle_for: Duration) -> Result<Vec<StaleClaim>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.pid, EXTRACT(EPOCH FROM (now() - a.state_change))::float8 AS idle_secs
           FROM pg_stat_activity a
          WHERE a.datname = current_database() AND a.pid <> pg_backend_pid()
            AND a.state = 'idle in transaction'
            AND a.state_change < now() - make_interval(secs => $1)
            AND EXISTS (SELECT 1 FROM pg_locks l
                         WHERE l.pid = a.pid AND l.locktype = 'relation' AND l.relation = 'outbox'::regclass
                           AND l.mode IN ('RowShareLock', 'RowExclusiveLock'))
          ORDER BY a.state_change",
    )
    .bind(idle_for.as_secs_f64())
    .fetch_all(conn)
    .await?)
}

/// How long ending a session waits for it to be gone, in milliseconds: the
/// claims it held are free when this returns, and a session that cannot be
/// ended in this time is an error to look into and not something to wait for.
const END_SESSION_WAIT_MS: i64 = 5_000;

/// Ends a session of the database and waits until it is gone, which releases
/// the claims it held. Says whether it was.
pub async fn end_session(conn: &mut Conn, pid: i32) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT pg_terminate_backend($1, $2)")
        .bind(pid)
        .bind(END_SESSION_WAIT_MS)
        .fetch_one(conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::identity::testing::insert_user;
    use crate::queries::notifications::channels::{self, NewChannel};
    use crate::queries::notifications::routes::{self, RouteChange};
    use chrono::Duration as Span;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    async fn route(conn: &mut Conn) -> Id {
        let (user, channel) = (new_id(), new_id());
        insert_user(conn, user, "ada").await.unwrap();
        channels::insert_channel(
            conn,
            &NewChannel {
                id: channel,
                org_id: None,
                user_id: None,
                kind: "email",
                name: "Mail",
                config: &json!({}),
                templates: &json!({}),
                enabled: true,
                created_by: user,
                at: clock::start(),
            },
        )
        .await
        .unwrap();
        routes::put_route(
            conn,
            new_id(),
            user,
            channel,
            &RouteChange::default(),
            clock::start(),
        )
        .await
        .unwrap()
        .id
    }

    async fn put(conn: &mut Conn, route: Id, at: DateTime<Utc>, payload: &Json) -> Id {
        let id = new_id();
        insert(
            conn,
            &NewRow {
                id,
                channel: "email",
                route_id: Some(route),
                destination_id: None,
                channel_id: None,
                payload,
                at,
            },
        )
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn a_row_that_one_worker_holds_is_skipped_by_another_until_it_is_settled() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let now = clock::start();
        let mut conn = test.pool.acquire().await.unwrap();
        let route = route(&mut conn).await;
        let first = put(&mut conn, route, now, &json!({})).await;
        let second = put(&mut conn, route, now + Span::seconds(1), &json!({})).await;

        let mut one = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        let mut two = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        let later = now + Span::minutes(1);
        let taken = claim_due(&mut one, later).await.unwrap().unwrap();
        assert_eq!(
            taken.id, first,
            "the row that waited longest is claimed first"
        );
        let other = claim_due(&mut two, later).await.unwrap().unwrap();
        assert_eq!(
            other.id, second,
            "the second worker is given the next, not the same"
        );
        assert!(
            settle(&mut one, first, &Settle::Sent { payload: None })
                .await
                .unwrap()
        );
        assert!(
            !settle(&mut one, first, &Settle::Sent { payload: None })
                .await
                .unwrap(),
            "a row that is sent is not settled twice"
        );
        one.commit().await.unwrap();
        two.rollback().await.unwrap();
        let mut three = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        let again = claim_due(&mut three, later).await.unwrap().unwrap();
        assert_eq!(
            again.id, second,
            "a row that was only held, and let go, is due again"
        );
    }

    #[tokio::test]
    async fn a_row_is_not_due_before_its_time_and_a_retry_moves_it_on() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let now = clock::start();
        let mut conn = test.pool.acquire().await.unwrap();
        let route = route(&mut conn).await;
        let id = put(&mut conn, route, now + Span::minutes(5), &json!({})).await;
        let mut tx = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        assert!(claim_due(&mut tx, now).await.unwrap().is_none());
        let row = claim_due(&mut tx, now + Span::minutes(5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((row.id, row.attempts), (id, 0));
        let retry = now + Span::minutes(10);
        settle(
            &mut tx,
            id,
            &Settle::Retry {
                at: retry,
                error: "unreachable",
                payload: None,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let after: (i32, String, Option<String>, DateTime<Utc>) = sqlx::query_as(
            "SELECT attempts, state, last_error, next_attempt_at FROM outbox WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            after,
            (1, "pending".into(), Some("unreachable".into()), retry),
            "a retry is a try that is counted, and the row stays pending"
        );
    }

    #[tokio::test]
    async fn rows_that_quiet_hours_held_for_a_route_are_claimed_together_and_by_one_worker_only() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let now = clock::start();
        let mut conn = test.pool.acquire().await.unwrap();
        let route = route(&mut conn).await;
        let held = json!({ "notifications": [], "held": true });
        let a = put(&mut conn, route, now, &held).await;
        let b = put(&mut conn, route, now + Span::seconds(1), &held).await;
        let plain = put(&mut conn, route, now + Span::seconds(2), &json!({})).await;
        let later = now + Span::hours(1);
        let mut one = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        let mut two = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        let first = claim_due(&mut one, later).await.unwrap().unwrap();
        assert_eq!(
            first.id, plain,
            "a row that was not held is claimed on its own"
        );
        assert!(!first.is_held());
        let group = claim_held_group(&mut one, later).await.unwrap();
        assert_eq!(group.iter().map(|row| row.id).collect::<Vec<_>>(), [a, b]);
        assert!(group.iter().all(OutboxRow::is_held));
        assert!(
            claim_held_group(&mut two, later).await.unwrap().is_empty(),
            "while one worker has the route the others leave it alone"
        );
        one.commit().await.unwrap();
        assert!(
            claim_held_group(&mut two, later).await.unwrap().len() == 2,
            "a route whose rows were let go is taken again as a whole"
        );
    }

    #[tokio::test]
    async fn a_session_that_sits_idle_in_a_claim_is_found_and_ending_it_frees_the_row() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let now = clock::start();
        let mut conn = test.pool.acquire().await.unwrap();
        let route = route(&mut conn).await;
        let id = put(&mut conn, route, now, &json!({})).await;

        let mut dead = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        assert!(claim_due(&mut dead, now).await.unwrap().is_some());
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *dead)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;

        let mut other = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        assert!(
            claim_due(&mut other, now).await.unwrap().is_none(),
            "while its worker lives the claim holds"
        );
        let stale = stale_claims(&mut conn, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(stale.iter().map(|each| each.pid).collect::<Vec<_>>(), [pid]);
        assert!(
            stale_claims(&mut conn, Duration::from_secs(600))
                .await
                .unwrap()
                .is_empty(),
            "a claim that is not old is not stale"
        );
        assert!(end_session(&mut conn, pid).await.unwrap());
        let taken = claim_due(&mut other, now).await.unwrap().unwrap();
        assert_eq!(taken.id, id, "the row a dead worker held is taken up");
    }

    #[tokio::test]
    async fn a_claiming_transaction_that_goes_quiet_is_ended_by_the_database_itself() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let now = clock::start();
        let mut conn = test.pool.acquire().await.unwrap();
        let route = route(&mut conn).await;
        let id = put(&mut conn, route, now, &json!({})).await;
        let mut quiet = begin_claim(&db, Duration::from_secs(1)).await.unwrap();
        assert!(claim_due(&mut quiet, now).await.unwrap().is_some());
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let mut other = begin_claim(&db, Duration::from_secs(600)).await.unwrap();
        let taken = claim_due(&mut other, now).await.unwrap().unwrap();
        assert_eq!(taken.id, id);
        assert!(
            claim_due(&mut quiet, now).await.is_err(),
            "the session that went quiet was ended"
        );
    }

    #[tokio::test]
    async fn the_pending_rows_are_counted_by_the_notifier_they_go_through() {
        let test = TestDb::fresh().await;
        let now = clock::start();
        let mut conn = test.pool.acquire().await.unwrap();
        let route = route(&mut conn).await;
        let sent = put(&mut conn, route, now, &json!({})).await;
        put(&mut conn, route, now, &json!({})).await;
        put(&mut conn, route, now - Span::hours(2), &json!({})).await;
        settle(&mut conn, sent, &Settle::Sent { payload: None })
            .await
            .unwrap();
        assert_eq!(
            pending_by_channel(&mut conn).await.unwrap(),
            [("email".to_owned(), 2)]
        );
        assert_eq!(
            overdue(&mut conn, now, Duration::from_secs(3600))
                .await
                .unwrap(),
            1
        );
    }
}
