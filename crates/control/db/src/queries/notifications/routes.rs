//! A person's routes through channels: switched on and off, linked by a code, what still waits to be told through them, and how far each has told.
//!
//! A route is one person's switch for one channel, and it is off until they
//! say otherwise. Which channel covers a notification is decided here, once,
//! so that the rows the engine writes at once and the rows the `routes` job
//! writes later agree: a root org's channel covers what happens in its tree
//! and in its chats; the system's and a person's own cover everything, except
//! that the system's mail server does not carry the mail of a root org that
//! has a mail server of its own, or that may not share the system's.

use super::outbox::{self, NewRow, Payload, Snapshot};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// Which of the notifications at a place a channel covers, as a condition on
/// the channel `c`. `$place` is an SQL expression for the root org the place
/// lies in (null for a notification that lies in none), `$shared` one for
/// whether the system's mail server may carry what lies in an org.
macro_rules! covers {
    ($place:literal, $shared:literal) => {
        concat!(
            "(c.org_id IS NULL OR c.org_id = ",
            $place,
            ") AND NOT (c.org_id IS NULL AND c.user_id IS NULL AND c.kind = 'email' AND ",
            $place,
            " IS NOT NULL AND (NOT ",
            $shared,
            " OR EXISTS (SELECT 1 FROM notification_channels o WHERE o.org_id = ",
            $place,
            " AND o.user_id IS NULL AND o.kind = 'email' AND o.enabled)))"
        )
    };
}

macro_rules! route_columns {
    () => {
        "id, user_id, channel_id, channel_snapshot, enabled, address, link_hash, link_expires_at, linked_at, kinds, unread_delay_minutes, min_interval_minutes, sent_seq, last_sent_at, last_ok_at, last_error, failure_episode, failed_at, health, held, created_at, updated_at"
    };
}

macro_rules! route_columns_of_r {
    () => {
        "r.id, r.user_id, r.channel_id, r.channel_snapshot, r.enabled, r.address, r.link_hash, r.link_expires_at, r.linked_at, r.kinds, r.unread_delay_minutes, r.min_interval_minutes, r.sent_seq, r.last_sent_at, r.last_ok_at, r.last_error, r.failure_episode, r.failed_at, r.health, r.held, r.created_at, r.updated_at"
    };
}

/// The columns a route is read with, for the other readers of routes.
pub const ROUTE_COLUMNS: &str = route_columns!();

/// The words a route's health is kept as.
pub mod health {
    /// Nothing went through it yet.
    pub const NEVER: &str = "never";
    /// Its notifier accepted the last message.
    pub const OK: &str = "ok";
    /// The last message failed after its tries, or was refused for good.
    pub const FAILED: &str = "failed";
    /// Push reached some browsers and not others.
    pub const PARTIAL: &str = "partial";
    /// What it sent to is gone.
    pub const TARGET_GONE: &str = "target_gone";
    /// Its channel was removed.
    pub const CHANNEL_REMOVED: &str = "channel_removed";
}

/// The safe facts of a channel that a route keeps: its identifier, notifier, name and root org,
/// and nothing of its fields, secrets, addresses or tokens. Made in SQL from the channel `c`.
const SNAPSHOT_OF_C: &str =
    "jsonb_build_object('id', c.id, 'kind', c.kind, 'name', c.name, 'org_id', c.org_id)";

/// One person's choice to be told through one channel.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct RouteRow {
    /// Its identifier.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// The channel it goes through; none once the channel was removed.
    pub channel_id: Option<Id>,
    /// The channel as the route keeps it: `id`, `kind`, `name` and `org_id`, also after it is gone.
    pub channel_snapshot: Json,
    /// Whether it is on; every route starts off.
    pub enabled: bool,
    /// Where the person is on that channel, where it has to be told.
    pub address: Json,
    /// The hash of the code the person sends a bot to say which chat is theirs.
    pub link_hash: Option<Vec<u8>>,
    /// Until when that code counts.
    pub link_expires_at: Option<DateTime<Utc>>,
    /// When the address was confirmed.
    pub linked_at: Option<DateTime<Utc>>,
    /// The kinds it tells; empty for everything the channel covers.
    pub kinds: Vec<String>,
    /// Tell only what is still unread after this many minutes.
    pub unread_delay_minutes: Option<i32>,
    /// Tell at most once in this many minutes.
    pub min_interval_minutes: Option<i32>,
    /// The person's inbox sequence up to which this route has told them.
    pub sent_seq: i64,
    /// When a message last went through it.
    pub last_sent_at: Option<DateTime<Utc>>,
    /// When its notifier last accepted a message for the person: no proof that a device showed it.
    pub last_ok_at: Option<DateTime<Utc>>,
    /// Why it last failed: `{ code, message, affected_devices }`, with no endpoint or token.
    pub last_error: Option<Json>,
    /// The failure it is in, from its first terminal outcome until it fully recovers.
    pub failure_episode: Option<Id>,
    /// When that failure began.
    pub failed_at: Option<DateTime<Utc>>,
    /// How it fared when it last sent: a word of [`health`].
    pub health: String,
    /// Whether quiet hours or a pause hold its unsent work, which goes as one digest after.
    pub held: bool,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it was last changed.
    pub updated_at: DateTime<Utc>,
}

impl RouteRow {
    /// Whether the person set a delay or a limit: such a route is served by
    /// the `routes` job and not in the transaction of the event.
    #[must_use]
    pub fn waits(&self) -> bool {
        self.unread_delay_minutes.is_some() || self.min_interval_minutes.is_some()
    }

    /// The identifier of its channel as the route keeps it, also after the channel is gone.
    #[must_use]
    pub fn kept_channel_id(&self) -> Option<Id> {
        self.channel_id.or_else(|| {
            self.channel_snapshot
                .get("id")
                .and_then(Json::as_str)
                .and_then(|text| text.parse().ok())
        })
    }

    /// A member of the kept channel, as text.
    #[must_use]
    pub fn kept_channel_text(&self, member: &str) -> Option<&str> {
        self.channel_snapshot.get(member).and_then(Json::as_str)
    }
}

/// The route `id` under its lock, the third a delivery takes, after its channel and its person's
/// choices.
pub async fn lock_route(conn: &mut Conn, id: Id) -> Result<Option<RouteRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        route_columns!(),
        " FROM notification_routes WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// How many unread notifications of its person wait in a route's scope and kinds: the root org
/// its channel belonged to, or everything for the system's and a person's own. What its person
/// was told of a route itself is not counted.
pub async fn waiting_count(conn: &mut Conn, route: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM notification_routes r
           JOIN notifications n ON n.user_id = r.user_id
           LEFT JOIN nodes nd ON nd.id = n.node_id
           LEFT JOIN chats ch ON ch.id = n.chat_id
          WHERE r.id = $1 AND n.read_at IS NULL AND n.kind <> 'notification.route_attention'
            AND (cardinality(r.kinds) = 0 OR n.kind = ANY(r.kinds))
            AND (r.channel_snapshot ->> 'org_id' IS NULL
                 OR (r.channel_snapshot ->> 'org_id')::uuid = COALESCE(ch.org_id, nd.org_id))",
    )
    .bind(route)
    .fetch_one(conn)
    .await?)
}

/// The route `id`.
pub async fn route(conn: &mut Conn, id: Id) -> Result<Option<RouteRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        route_columns!(),
        " FROM notification_routes WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The route of a person through a channel, if they have one.
pub async fn route_for(
    conn: &mut Conn,
    user: Id,
    channel: Id,
) -> Result<Option<RouteRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        route_columns!(),
        " FROM notification_routes WHERE user_id = $1 AND channel_id = $2"
    ))
    .bind(user)
    .bind(channel)
    .fetch_optional(conn)
    .await?)
}

/// What is changed of a route; what is `None` stays. A delay or a limit that
/// is `Some(None)` is taken away.
#[derive(Debug, Clone, Copy, Default)]
pub struct RouteChange<'a> {
    /// Switched on or off.
    pub enabled: Option<bool>,
    /// The kinds it tells.
    pub kinds: Option<&'a [String]>,
    /// How long a notification stays unread before it is told.
    pub unread_delay_minutes: Option<Option<i32>>,
    /// How seldom a message goes.
    pub min_interval_minutes: Option<Option<i32>>,
}

/// Makes the route of a person through a channel, or changes the one there
/// is. A new route is off, tells every kind and waits for nothing, except
/// where the change says otherwise.
pub async fn put_route(
    conn: &mut Conn,
    new_id: Id,
    user: Id,
    channel: Id,
    change: &RouteChange<'_>,
    at: DateTime<Utc>,
) -> Result<RouteRow, DbError> {
    let (set_delay, delay) = match change.unread_delay_minutes {
        Some(value) => (true, value),
        None => (false, None),
    };
    let (set_interval, interval) = match change.min_interval_minutes {
        Some(value) => (true, value),
        None => (false, None),
    };
    // The channel's safe facts are kept with the route, and refreshed by every change of it, so
    // that a route whose channel is removed later still says what it was.
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO notification_routes (id, user_id, channel_id, channel_snapshot, enabled, kinds, unread_delay_minutes, min_interval_minutes, created_at, updated_at) \
         SELECT $1, $2, c.id, {SNAPSHOT_OF_C}, COALESCE($4::bool, false), COALESCE($5::text[], '{{}}'), $7::int, $9::int, $10, $10 \
           FROM notification_channels c WHERE c.id = $3 \
         ON CONFLICT (user_id, channel_id) DO UPDATE SET \
         channel_snapshot = EXCLUDED.channel_snapshot, \
         enabled = COALESCE($4::bool, notification_routes.enabled), \
         kinds = COALESCE($5::text[], notification_routes.kinds), \
         unread_delay_minutes = CASE WHEN $6::bool THEN $7::int ELSE notification_routes.unread_delay_minutes END, \
         min_interval_minutes = CASE WHEN $8::bool THEN $9::int ELSE notification_routes.min_interval_minutes END, \
         updated_at = $10 RETURNING {}",
        route_columns!()
    )))
    .bind(new_id)
    .bind(user)
    .bind(channel)
    .bind(change.enabled)
    .bind(change.kinds)
    .bind(set_delay)
    .bind(delay)
    .bind(set_interval)
    .bind(interval)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Gives a route a code to be linked by: the hash of what the person sends the
/// bot, good until `expires_at`. A code asked for before replaces the old.
pub async fn start_link(
    conn: &mut Conn,
    route: Id,
    hash: &[u8],
    expires_at: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE notification_routes SET link_hash = $2, link_expires_at = $3, updated_at = $4 WHERE id = $1",
    )
    .bind(route)
    .bind(hash)
    .bind(expires_at)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A route with a code that is still good.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct WaitingCode {
    /// The route.
    pub route_id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// The hash of its code.
    pub link_hash: Vec<u8>,
}

/// The routes of a channel that wait for their code to come back at `now`.
pub async fn waiting_codes(
    conn: &mut Conn,
    channel: Id,
    now: DateTime<Utc>,
) -> Result<Vec<WaitingCode>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id AS route_id, user_id, link_hash FROM notification_routes
          WHERE channel_id = $1 AND link_hash IS NOT NULL AND link_expires_at > $2
          ORDER BY link_expires_at",
    )
    .bind(channel)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// Links a route: its address is what the notifier learned, and the code is
/// spent. A code is good once and until its time: the answer says whether it
/// was.
pub async fn complete_link(
    conn: &mut Conn,
    route: Id,
    hash: &[u8],
    address: &Json,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let linked: Option<(Id, Option<Id>)> = sqlx::query_as(
        "UPDATE notification_routes
            SET address = $3, linked_at = $4, link_hash = NULL, link_expires_at = NULL, updated_at = $4
          WHERE id = $1 AND link_hash = $2 AND link_expires_at > $4
      RETURNING user_id, channel_id",
    )
    .bind(route)
    .bind(hash)
    .bind(address)
    .bind(at)
    .fetch_optional(&mut *conn)
    .await?;
    match linked {
        Some((user, channel)) => {
            forget_kept_code(conn, route).await?;
            tell_route_moved(conn, user, channel).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Forgets the sealed code a route's link was kept with, once it was pressed or lapsed.
async fn forget_kept_code(conn: &mut Conn, route: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM secrets WHERE owner_kind = 'notification_channel' AND name = $1")
        .bind(format!("route_link:{route}"))
        .execute(conn)
        .await?;
    Ok(())
}

/// The same news for a route whose channel may be gone.
async fn tell_route_moved(conn: &mut Conn, user: Id, channel: Option<Id>) -> Result<(), DbError> {
    crate::sequences::notify_user_event(
        conn,
        &[user],
        &serde_json::json!({ "what": "route", "channel_id": channel }),
    )
    .await?;
    Ok(())
}

/// Tells the person's open screens that one of their routes changed without
/// them: a bot was started and the route is linked, or a delivery found its
/// target gone and switched it off. They load their channels again. The news
/// is delivered when the transaction commits.
pub async fn tell_route_changed(conn: &mut Conn, user: Id, channel: Id) -> Result<(), DbError> {
    crate::sequences::notify_user_event(
        conn,
        &[user],
        &serde_json::json!({ "what": "route", "channel_id": channel }),
    )
    .await?;
    Ok(())
}

/// Forgets the codes whose time has passed: one that comes back later
/// matches nothing. Says how many routes had one.
pub async fn drop_expired_codes(conn: &mut Conn, now: DateTime<Utc>) -> Result<u64, DbError> {
    let done = sqlx::query(
        "UPDATE notification_routes SET link_hash = NULL, link_expires_at = NULL
          WHERE link_hash IS NOT NULL AND link_expires_at <= $1",
    )
    .bind(now)
    .execute(&mut *conn)
    .await?;
    // The sealed codes kept for links that lapsed, or whose route went another way, go with them.
    sqlx::query(
        "DELETE FROM secrets s
          WHERE s.owner_kind = 'notification_channel' AND s.name LIKE 'route_link:%'
            AND NOT EXISTS (SELECT 1 FROM notification_routes r
                             WHERE 'route_link:' || r.id::text = s.name
                               AND r.link_hash IS NOT NULL AND r.link_expires_at > $1)",
    )
    .bind(now)
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// Switches a route off, and with `unlink` forgets where it was linked and any
/// code that waits, as a target that is gone asks for. Says whether the route
/// was there.
pub async fn switch_off(
    conn: &mut Conn,
    route: Id,
    unlink: bool,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let off: Option<(Id, Option<Id>)> = sqlx::query_as(
        "UPDATE notification_routes
            SET enabled = false, updated_at = $3,
                address = CASE WHEN $2 THEN '{}'::jsonb ELSE address END,
                linked_at = CASE WHEN $2 THEN NULL ELSE linked_at END,
                link_hash = CASE WHEN $2 THEN NULL ELSE link_hash END,
                link_expires_at = CASE WHEN $2 THEN NULL ELSE link_expires_at END
          WHERE id = $1
      RETURNING user_id, channel_id",
    )
    .bind(route)
    .bind(unlink)
    .bind(at)
    .fetch_optional(&mut *conn)
    .await?;
    match off {
        Some((user, channel)) => {
            tell_route_moved(conn, user, channel).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// The route's failure as it is kept and shown: a word for the reason, the safe words, and the
/// names of the browsers it concerns.
#[must_use]
pub fn failure_json(code: &str, message: &str, affected_devices: &[String]) -> Json {
    serde_json::json!({ "code": code, "message": message, "affected_devices": affected_devices })
}

/// Notes that a route's notifier accepted a message. Where push reached some of the person's
/// browsers and not others, `partial` names those it did not: the route is `partial`, in a
/// failure episode until it is repaired (the episode is returned, for the one notice of it),
/// since success on one browser does not hide the loss of another. A whole acceptance ends any
/// failure. For a route whose channel was removed meanwhile, only when it last accepted moves.
pub async fn record_accepted(
    conn: &mut Conn,
    route: Id,
    partial: Option<&Json>,
    at: DateTime<Utc>,
) -> Result<Option<Id>, DbError> {
    let episode: Option<Option<Id>> = sqlx::query_scalar(
        "UPDATE notification_routes SET last_ok_at = $3, last_sent_at = $3, updated_at = $3,
                health = CASE WHEN channel_id IS NULL THEN health WHEN $2::jsonb IS NULL THEN 'ok' ELSE 'partial' END,
                last_error = CASE WHEN channel_id IS NULL THEN last_error ELSE $2::jsonb END,
                failure_episode = CASE WHEN channel_id IS NULL THEN failure_episode
                                       WHEN $2::jsonb IS NULL THEN NULL
                                       ELSE COALESCE(failure_episode, $4) END,
                failed_at = CASE WHEN channel_id IS NULL THEN failed_at
                                 WHEN $2::jsonb IS NULL THEN NULL ELSE COALESCE(failed_at, $3) END
          WHERE id = $1
      RETURNING CASE WHEN channel_id IS NOT NULL AND $2::jsonb IS NOT NULL THEN failure_episode END",
    )
    .bind(route)
    .bind(partial)
    .bind(at)
    .bind(orch_types::new_id())
    .fetch_optional(conn)
    .await?;
    Ok(episode.flatten())
}

/// Notes a failure of a route: its reason is kept at every try, and an outcome that is terminal
/// (the tries ran out, a refusal for good, the target gone) moves its health and begins a
/// failure episode unless one is under way. Returns the episode a terminal outcome is in, for
/// the one notice of it; none for a try that will be made again, or a route that is gone.
pub async fn record_failure(
    conn: &mut Conn,
    route: Id,
    terminal: Option<&str>,
    failure: &Json,
    at: DateTime<Utc>,
) -> Result<Option<Id>, DbError> {
    let episode: Option<Option<Id>> = sqlx::query_scalar(
        "UPDATE notification_routes SET last_error = $3, updated_at = $4,
                health = COALESCE($2::text, health),
                failure_episode = CASE WHEN $2::text IS NULL THEN failure_episode
                                       ELSE COALESCE(failure_episode, $5) END,
                failed_at = CASE WHEN $2::text IS NULL THEN failed_at ELSE COALESCE(failed_at, $4) END
          WHERE id = $1
      RETURNING CASE WHEN $2::text IS NULL THEN NULL ELSE failure_episode END",
    )
    .bind(route)
    .bind(terminal)
    .bind(failure)
    .bind(at)
    .bind(orch_types::new_id())
    .fetch_optional(conn)
    .await?;
    Ok(episode.flatten())
}

/// Notes that a sample message reached a route's person: when it last accepted moves, and
/// nothing else, since a test sends no work that waited and repairs no failure by itself.
pub async fn record_test_taken(
    conn: &mut Conn,
    route: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE notification_routes SET last_ok_at = $2 WHERE id = $1")
        .bind(route)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Marks whether quiet hours or a pause hold a route's unsent work.
pub async fn set_held(conn: &mut Conn, route: Id, held: bool) -> Result<(), DbError> {
    sqlx::query("UPDATE notification_routes SET held = $2 WHERE id = $1 AND held <> $2")
        .bind(route)
        .bind(held)
        .execute(conn)
        .await?;
    Ok(())
}

/// A route that a channel's removal ended, for the one notice its person is given.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct RemovedRoute {
    /// The route.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// The failure it is in now.
    pub failure_episode: Option<Id>,
    /// Whether it was on, or had work that had not gone: only those are told of the removal.
    pub mattered: bool,
}

/// Keeps every route of a channel that is about to be removed as a record, off, with the
/// channel's safe facts as they are now and the reason, and cancels the work of those routes
/// that has not gone. Run under the channel's lock, before the channel row is deleted, so that
/// no removal, an org's purge among them, loses a route's notice. Returns the routes.
pub async fn keep_routes_of_removed_channel(
    conn: &mut Conn,
    channel: Id,
    at: DateTime<Utc>,
) -> Result<Vec<RemovedRoute>, DbError> {
    let failure = failure_json(
        health::CHANNEL_REMOVED,
        "The channel was removed by whoever looked after it.",
        &[],
    );
    let kept: Vec<RemovedRoute> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH before AS (
            SELECT id, enabled FROM notification_routes WHERE channel_id = $1 ORDER BY user_id, id FOR UPDATE
         ), unsent AS (
            SELECT DISTINCT route_id FROM outbox
             WHERE route_id IN (SELECT id FROM before) AND state IN ('pending', 'failed')
         )
         UPDATE notification_routes r
            SET channel_snapshot = (SELECT {SNAPSHOT_OF_C} FROM notification_channels c WHERE c.id = $1),
                enabled = false, held = false, health = 'channel_removed', last_error = $2,
                failure_episode = COALESCE(r.failure_episode, gen_random_uuid()),
                failed_at = COALESCE(r.failed_at, $3), updated_at = $3,
                link_hash = NULL, link_expires_at = NULL
           FROM before b
          WHERE r.id = b.id
      RETURNING r.id, r.user_id, r.failure_episode,
                (b.enabled OR r.id IN (SELECT route_id FROM unsent)) AS mattered"
    )))
    .bind(channel)
    .bind(&failure)
    .bind(at)
    .fetch_all(&mut *conn)
    .await?;
    // A delivery reserved before the removal may still finish; what has not been reserved never goes.
    sqlx::query(
        "UPDATE outbox SET state = 'cancelled', cancel_reason = 'channel_removed'
          WHERE route_id IN (SELECT id FROM notification_routes WHERE channel_id = $1) AND state = 'pending'",
    )
    .bind(channel)
    .execute(&mut *conn)
    .await?;
    Ok(kept)
}

/// Removes a person's own route by its identifier: one that is off, or kept after its channel
/// was removed. A deliberate removal tells nobody of a failure. Says whether there was one.
pub async fn remove_own(conn: &mut Conn, user: Id, route: Id) -> Result<bool, DbError> {
    sqlx::query(
        "UPDATE outbox SET state = 'cancelled', cancel_reason = 'route_off'
          WHERE route_id = $1 AND state = 'pending'",
    )
    .bind(route)
    .execute(&mut *conn)
    .await?;
    let done = sqlx::query(
        "DELETE FROM notification_routes WHERE id = $1 AND user_id = $2 AND NOT enabled",
    )
    .bind(route)
    .bind(user)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Notes that a message went through a route.
pub async fn record_sent(conn: &mut Conn, route: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE notification_routes SET last_sent_at = $2 WHERE id = $1")
        .bind(route)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Moves a route up to the present: its sequence goes to the person's latest,
/// so that what came before they switched it on is never told to them as if
/// it were new.
pub async fn catch_up(conn: &mut Conn, route: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE notification_routes r SET sent_seq = u.inbox_seq, updated_at = $2
           FROM users u WHERE r.id = $1 AND u.id = r.user_id AND r.sent_seq < u.inbox_seq",
    )
    .bind(route)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The routes that are on, through a channel that is in use, with a delay or a
/// limit, and for which something unread is past their sequence: the only ones
/// the `routes` job has to look at.
pub async fn due_routes(conn: &mut Conn) -> Result<Vec<RouteRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        route_columns_of_r!(),
        " FROM notification_routes r JOIN notification_channels c ON c.id = r.channel_id ",
        "WHERE r.enabled AND c.enabled AND (r.unread_delay_minutes IS NOT NULL OR r.min_interval_minutes IS NOT NULL) ",
        "AND EXISTS (SELECT 1 FROM notifications n WHERE n.user_id = r.user_id AND n.inbox_seq > r.sent_seq AND n.read_at IS NULL) ",
        "ORDER BY r.created_at, r.id"
    ))
    .fetch_all(conn)
    .await?)
}

/// A notification that waits to be told through a route.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct WaitingRow {
    /// Its identifier.
    pub id: Id,
    /// Its kind.
    pub kind: String,
    /// The node it happened at.
    pub node_id: Option<Id>,
    /// The thread it is about.
    pub thread_id: Option<Id>,
    /// The approval it is about.
    pub approval_id: Option<Id>,
    /// The chat it is about.
    pub chat_id: Option<Id>,
    /// Its title.
    pub title: String,
    /// Its first line.
    pub body: String,
    /// Whatever else its kind carries.
    pub data: Json,
    /// Its number in the person's feed.
    pub inbox_seq: i64,
    /// When it happened.
    pub created_at: DateTime<Utc>,
}

impl WaitingRow {
    /// The notification as an outbox row carries it.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            id: Some(self.id),
            kind: self.kind.clone(),
            at: self.created_at.into(),
            title: self.title.clone(),
            body: self.body.clone(),
            node_id: self.node_id,
            thread_id: self.thread_id,
            approval_id: self.approval_id,
            chat_id: self.chat_id,
            data: self.data.clone(),
        }
    }
}

/// What waits to be told through a route: the person's notifications that the
/// channel covers and the route's kinds take, past the sequence the route has
/// told up to, still unread, and, where `older_than` is given, not newer than
/// it. `mail_shared` says whether the system's mail server may carry what lies
/// in an org of its own. In the order of the feed.
pub async fn waiting_inbox(
    conn: &mut Conn,
    route: Id,
    older_than: Option<DateTime<Utc>>,
    mail_shared: bool,
) -> Result<Vec<WaitingRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT n.id, n.kind, n.node_id, n.thread_id, n.approval_id, n.chat_id, n.title, n.body, n.data, n.inbox_seq, n.created_at ",
        "FROM notification_routes r ",
        "JOIN notification_channels c ON c.id = r.channel_id ",
        "JOIN notifications n ON n.user_id = r.user_id ",
        "LEFT JOIN nodes nd ON nd.id = n.node_id ",
        "LEFT JOIN chats ch ON ch.id = n.chat_id ",
        "WHERE r.id = $1 AND n.inbox_seq > r.sent_seq AND n.read_at IS NULL ",
        "AND ($2::timestamptz IS NULL OR n.created_at <= $2) ",
        "AND (cardinality(r.kinds) = 0 OR n.kind = ANY(r.kinds)) AND ",
        covers!("COALESCE(ch.org_id, nd.org_id)", "$3::bool"),
        " ORDER BY n.inbox_seq"
    ))
    .bind(route)
    .bind(older_than)
    .bind(mail_shared)
    .fetch_all(conn)
    .await?)
}

/// Those of these notifications of a person that are still unread, as a delivery carries them,
/// in the order of the feed.
pub async fn unread_rows_of(
    conn: &mut Conn,
    user: Id,
    ids: &[Id],
) -> Result<Vec<WaitingRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, node_id, thread_id, approval_id, chat_id, title, body, data, inbox_seq, created_at
           FROM notifications WHERE user_id = $1 AND id = ANY($2) AND read_at IS NULL ORDER BY inbox_seq",
    )
    .bind(user)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Moves a route past what it has told: its sequence goes from `from` to `to`
/// and a message is noted as having left at `at`. Says whether the route was
/// still at `from`; two runs of the job never both move it.
pub async fn move_on(
    conn: &mut Conn,
    route: Id,
    from: i64,
    to: i64,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE notification_routes SET sent_seq = $3, last_sent_at = $4, updated_at = $4
          WHERE id = $1 AND sent_seq = $2",
    )
    .bind(route)
    .bind(from)
    .bind(to)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A route that tells a notification at once.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ImmediateRoute {
    /// The route.
    pub route_id: Id,
    /// The channel it goes through.
    pub channel_id: Id,
    /// That channel's notifier.
    pub channel_kind: String,
}

/// The routes of a person that tell a notification of `kind` at once, in the
/// transaction of the event: on, through a channel in use that covers the
/// place (`place_org` is the root org it lies in, none for one that lies in
/// none), taking the kind, and waiting for nothing.
pub async fn immediate_routes(
    conn: &mut Conn,
    user: Id,
    kind: &str,
    place_org: Option<Id>,
    mail_shared: bool,
) -> Result<Vec<ImmediateRoute>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT r.id AS route_id, c.id AS channel_id, c.kind AS channel_kind ",
        "FROM notification_routes r JOIN notification_channels c ON c.id = r.channel_id ",
        "WHERE r.user_id = $1 AND r.enabled AND c.enabled ",
        "AND r.unread_delay_minutes IS NULL AND r.min_interval_minutes IS NULL ",
        "AND (cardinality(r.kinds) = 0 OR $2 = ANY(r.kinds)) AND ",
        covers!("$3::uuid", "$4::bool"),
        " ORDER BY r.created_at, r.id"
    ))
    .bind(user)
    .bind(kind)
    .bind(place_org)
    .bind(mail_shared)
    .fetch_all(conn)
    .await?)
}

/// Writes the outbox rows that tell a notification at once: one for each
/// route that tells it. Says how many.
pub async fn tell_routes(
    conn: &mut Conn,
    user: Id,
    snapshot: &Snapshot,
    place_org: Option<Id>,
    mail_shared: bool,
    at: DateTime<Utc>,
) -> Result<usize, DbError> {
    let routes = immediate_routes(conn, user, &snapshot.kind, place_org, mail_shared).await?;
    let payload = Payload::of(std::slice::from_ref(snapshot)).to_json();
    for route in &routes {
        outbox::insert(
            conn,
            &NewRow {
                id: orch_types::new_id(),
                channel: &route.channel_kind,
                route_id: Some(route.route_id),
                destination_id: None,
                channel_id: None,
                payload: &payload,
                at,
            },
        )
        .await?;
    }
    Ok(routes.len())
}

/// How many routes there are of each kind of channel, by whether they are on
/// and whether they are linked.
pub async fn route_counts(conn: &mut Conn) -> Result<Vec<RouteCount>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.kind AS channel_kind, r.enabled, (r.linked_at IS NOT NULL) AS linked, count(*) AS routes
           FROM notification_routes r JOIN notification_channels c ON c.id = r.channel_id
          GROUP BY 1, 2, 3 ORDER BY 1, 2, 3",
    )
    .fetch_all(conn)
    .await?)
}

/// A number of routes alike.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RouteCount {
    /// The notifier of their channel.
    pub channel_kind: String,
    /// Whether they are on.
    pub enabled: bool,
    /// Whether they were linked.
    pub linked: bool,
    /// How many.
    pub routes: i64,
}
