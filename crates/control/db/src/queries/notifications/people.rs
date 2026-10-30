//! The people a message is for and the places it is about: their language, time zone, confirmed address and quiet hours; their browsers that take push; the places notifications happened at; and the Inbox row a notification starts as.
//!
//! A message is written in the language of the person it is for, with its
//! times in their time zone, and waits while their quiet hours last. A
//! notification names the node it happened at; its path of names and the
//! root org it lies in are what a text shows of it.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// The private envelope within existing row data that preserves the action vocabulary version.
pub const NOTIFICATION_DETAILS_MEMBER: &str = "_orch_action";
/// The stable action vocabulary written by this release.
pub const NOTIFICATION_DETAILS_VERSION: u32 = 1;

/// Machine outage notices contain thread names without the identifiers needed to authorize them.
#[must_use]
pub fn machine_notice_has_unscoped_thread_text(kind: &str, data: &Json) -> bool {
    kind == "machine.offline"
        && data.get("reason").and_then(Json::as_str) != Some("certificate_expiring")
}

/// Current facts for an approval already named in the recipient's notification page.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NoticeApproval {
    /// Its source kind.
    pub kind: String,
    /// Its actual decision body.
    pub decision: Option<Json>,
    /// When it was settled.
    pub decided_at: Option<DateTime<Utc>>,
    /// The approval already named by the notice.
    pub id: Id,
    /// Who asked, for a request still waiting.
    pub requester_user_id: Option<Id>,
    /// Who decided, where a person did.
    pub decided_by: Option<Id>,
    /// The actual pending/approved/denied/cancelled/expired state.
    pub state: String,
}

/// One bounded batch supplies the request's present state and the person its row should name.
pub async fn approval_actors(conn: &mut Conn, ids: &[Id]) -> Result<Vec<NoticeApproval>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT id, kind, requester_user_id, decided_by, state, decision, decided_at FROM approvals WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Enriches a new object event with versioned action parameters without parsing a stored sentence or
/// overwriting any kind-specific data. Renewed rows replace this same envelope.
#[must_use]
pub fn notification_data(kind: &str, data: &Json) -> Json {
    let Some(mut parameters) = data.as_object().cloned() else {
        return data.clone();
    };
    parameters.remove(NOTIFICATION_DETAILS_MEMBER);
    let mut stored = parameters.clone();
    stored.insert(
        NOTIFICATION_DETAILS_MEMBER.to_owned(),
        serde_json::json!({
            "v":NOTIFICATION_DETAILS_VERSION, "action":kind, "parameters":parameters,
        }),
    );
    Json::Object(stored)
}

/// Active human accounts named by literal usernames in a newly authored thread input.
pub async fn mentioned_people(conn: &mut Conn, names: &[String]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT id FROM users WHERE kind = 'human' AND status = 'active' AND username = ANY($1) ORDER BY id")
        .bind(names).fetch_all(conn).await?)
}

/// A person as a message needs them.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Recipient {
    /// Their identifier.
    pub id: Id,
    /// `human` or `service`.
    pub kind: String,
    /// The name they sign in under.
    pub username: String,
    /// The name shown.
    pub display_name: String,
    /// The colour of their avatar.
    pub avatar_color: i16,
    /// Their language: `en` or `zh-CN`.
    pub locale: String,
    /// Their time zone, an IANA name.
    pub timezone: String,
    /// Their address, confirmed or not.
    pub email: Option<String>,
    /// When they confirmed it.
    pub email_verified_at: Option<DateTime<Utc>>,
    /// Where their quiet hours begin, in minutes after local midnight.
    pub quiet_start_minute: Option<i32>,
    /// Where their quiet hours end.
    pub quiet_end_minute: Option<i32>,
    /// Until when they paused every way out, as stored: a moment that has passed holds nothing.
    pub pause_until: Option<DateTime<Utc>>,
}

impl Recipient {
    /// Their address where they have confirmed one: the only one mail goes
    /// to.
    #[must_use]
    pub fn confirmed_email(&self) -> Option<&str> {
        self.email
            .as_deref()
            .filter(|_| self.email_verified_at.is_some())
    }

    /// Their quiet hours, as the minutes they begin and end at.
    #[must_use]
    pub fn quiet_window(&self) -> Option<(i32, i32)> {
        Some((self.quiet_start_minute?, self.quiet_end_minute?))
    }
}

/// A person, with their quiet hours.
pub async fn recipient(conn: &mut Conn, user: Id) -> Result<Option<Recipient>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.kind, u.username, u.display_name, u.avatar_color, u.locale, u.timezone, u.email,
                u.email_verified_at, p.quiet_start_minute, p.quiet_end_minute, p.pause_until
           FROM users u LEFT JOIN notification_prefs p ON p.user_id = u.id
          WHERE u.id = $1",
    )
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// Takes a person's choices under a shared lock, the second lock a delivery takes after its
/// channel: a pause or a resume, which takes the row alone, is ordered with it. Nothing is
/// locked for a person who chose nothing.
pub async fn lock_prefs_shared(conn: &mut Conn, user: Id) -> Result<(), DbError> {
    sqlx::query("SELECT 1 FROM notification_prefs WHERE user_id = $1 FOR SHARE")
        .bind(user)
        .execute(conn)
        .await?;
    Ok(())
}

/// Those of these Inbox rows of a person that are still unread: what a delivery still tells.
pub async fn unread_among(conn: &mut Conn, user: Id, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM notifications WHERE user_id = $1 AND id = ANY($2) AND read_at IS NULL",
    )
    .bind(user)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Those of these Inbox rows that a person's Inbox still has, read or not.
pub async fn known_among(conn: &mut Conn, user: Id, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM notifications WHERE user_id = $1 AND id = ANY($2)")
            .bind(user)
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// The kind of the one Inbox notice of a route that stopped reaching its person.
pub const ROUTE_ATTENTION: &str = "notification.route_attention";

/// Where the notice of a route opens: the person's ways out.
const ROUTE_ATTENTION_PATH: &str = "/settings/notifications";

/// Tells a person, in their Inbox only, that a route of theirs stopped reaching them: once for a
/// failure episode, the row brought up to date by later outcomes of the same episode. It names
/// the way, the reason, when it last accepted, the browsers concerned and how many unread
/// notifications wait in its scope, and nothing of any message, address or token. It is never
/// sent through a route, so that a broken route cannot fail again on its own notice.
pub async fn tell_route_attention(
    conn: &mut Conn,
    route: &super::routes::RouteRow,
    episode: Id,
    waiting: i64,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let name = route
        .kept_channel_text("name")
        .unwrap_or_default()
        .to_owned();
    let kind = route
        .kept_channel_text("kind")
        .unwrap_or_default()
        .to_owned();
    let failure = route.last_error.clone().unwrap_or(Json::Null);
    let reason = failure
        .get("code")
        .and_then(Json::as_str)
        .unwrap_or("failed")
        .to_owned();
    let devices = failure
        .get("affected_devices")
        .cloned()
        .unwrap_or_else(|| Json::Array(Vec::new()));
    let title = format!("{name} stopped reaching you");
    let body = match reason.as_str() {
        "channel_removed" => "The channel was removed. Choose another way, or remove this one.",
        "target_gone" => "Where it sent to is gone. Connect it again before switching it on.",
        "partial" => "Some of your browsers no longer take push.",
        _ => "What waited could not be delivered after every try.",
    };
    let data = serde_json::json!({
        "v": 1,
        "action": "route_attention",
        "route_id": route.id,
        "reason": reason,
        "channel": { "name": name, "kind": kind },
        "last_ok_at": route.last_ok_at.map(|accepted| accepted.to_rfc3339()),
        "affected_devices": devices,
        "waiting_count": waiting,
        "path": ROUTE_ATTENTION_PATH,
    });
    let key = format!("{ROUTE_ATTENTION}:{}:{episode}", route.id);
    let written = announce(
        conn,
        &NewNotification {
            user_id: route.user_id,
            kind: ROUTE_ATTENTION,
            node_id: None,
            thread_id: None,
            approval_id: None,
            chat_id: None,
            title: &title,
            body,
            data: &data,
            dedupe_key: &key,
            at,
        },
    )
    .await?;
    if written.is_none() {
        sqlx::query(
            "UPDATE notifications SET title = $3, body = $4, data = $5 WHERE user_id = $1 AND dedupe_key = $2",
        )
        .bind(route.user_id)
        .bind(&key)
        .bind(&title)
        .bind(body)
        .bind(&data)
        .execute(conn)
        .await?;
    }
    Ok(())
}

/// Keeps the routes of a channel that is about to be removed as records, cancels what had not
/// gone through them, and tells once each person whose route was on or had work waiting.
pub async fn keep_routes_and_tell(
    conn: &mut Conn,
    channel: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let kept = super::routes::keep_routes_of_removed_channel(conn, channel, at).await?;
    for removed in kept.into_iter().filter(|removed| removed.mattered) {
        let (Some(route), Some(episode)) = (
            super::routes::route(conn, removed.id).await?,
            removed.failure_episode,
        ) else {
            continue;
        };
        let waiting = super::routes::waiting_count(conn, route.id).await?;
        tell_route_attention(conn, &route, episode, waiting, at).await?;
    }
    Ok(())
}

/// A browser of a person that takes push.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PushSubscriptionRow {
    /// Its identifier.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// Where the browser's push service takes a message for it.
    pub endpoint: String,
    /// The browser's public key, base64url.
    pub p256dh: String,
    /// The browser's authentication secret, base64url.
    pub auth: String,
    /// The browser it is, as it named itself.
    pub user_agent: String,
    /// When it was added.
    pub created_at: DateTime<Utc>,
    /// When a message last reached it.
    pub last_ok_at: Option<DateTime<Utc>>,
    /// How many deliveries in a row failed.
    pub failed_count: i32,
}

/// The browsers of a person that take push, the oldest first.
pub async fn push_subscriptions_of(
    conn: &mut Conn,
    user: Id,
) -> Result<Vec<PushSubscriptionRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, user_id, endpoint, p256dh, auth, user_agent, created_at, last_ok_at, failed_count
           FROM push_subscriptions WHERE user_id = $1 ORDER BY created_at, id",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// A browser that takes push, as it is added.
#[derive(Debug, Clone, Copy)]
pub struct NewPushSubscription<'a> {
    /// Its identifier, used when the browser is new.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// Where the browser's push service takes a message for it.
    pub endpoint: &'a str,
    /// The browser's public key, base64url.
    pub p256dh: &'a str,
    /// The browser's authentication secret, base64url.
    pub auth: &'a str,
    /// The browser it is, as it named itself.
    pub user_agent: &'a str,
    /// When it is added.
    pub at: DateTime<Utc>,
}

/// Adds a browser that takes push, or renews the keys of one that is there.
/// The answer is the identifier the browser has.
pub async fn put_push_subscription(
    conn: &mut Conn,
    new: &NewPushSubscription<'_>,
) -> Result<Id, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO push_subscriptions (id, user_id, endpoint, p256dh, auth, user_agent, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (endpoint) DO UPDATE
            SET user_id = EXCLUDED.user_id, p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth,
                user_agent = EXCLUDED.user_agent, failed_count = 0
         RETURNING id",
    )
    .bind(new.id)
    .bind(new.user_id)
    .bind(new.endpoint)
    .bind(new.p256dh)
    .bind(new.auth)
    .bind(new.user_agent)
    .bind(new.at)
    .fetch_one(conn)
    .await?)
}

/// Notes that a message reached a browser.
pub async fn push_succeeded(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE push_subscriptions SET last_ok_at = $2, failed_count = 0 WHERE id = $1")
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Counts a delivery that failed for a browser, and says how many in a row
/// have now. None where the browser is no longer there.
pub async fn push_failed(conn: &mut Conn, id: Id) -> Result<Option<i32>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE push_subscriptions SET failed_count = failed_count + 1 WHERE id = $1 RETURNING failed_count",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Removes a browser. Says whether it was there.
pub async fn remove_push_subscription(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    let done = sqlx::query("DELETE FROM push_subscriptions WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// A place in the tree, as a text names it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PlaceRow {
    /// Its identifier.
    pub id: Id,
    /// `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// The names of the nodes from the root org down to it, itself last.
    pub path: Vec<String>,
    /// The root org it lies in.
    pub org_id: Id,
    /// That root org's name.
    pub org_name: String,
}

/// The places named, with the names on their paths. A place that is not
/// there is left out.
pub async fn places(conn: &mut Conn, ids: &[Id]) -> Result<Vec<PlaceRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.name, n.org_id, o.name AS org_name,
                ARRAY(SELECT p.name FROM unnest(n.path) WITH ORDINALITY AS a(node_id, ord)
                        JOIN nodes p ON p.id = a.node_id ORDER BY a.ord) AS path
           FROM nodes n JOIN nodes o ON o.id = n.org_id
          WHERE n.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The root org a chat belongs to, with its name.
pub async fn org_of_chat(conn: &mut Conn, chat: Id) -> Result<Option<(Id, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.org_id, o.name FROM chats c JOIN nodes o ON o.id = c.org_id WHERE c.id = $1",
    )
    .bind(chat)
    .fetch_optional(conn)
    .await?)
}

/// The name of the root org with this identifier.
pub async fn org_name(conn: &mut Conn, org: Id) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar("SELECT name FROM nodes WHERE id = $1")
        .bind(org)
        .fetch_optional(conn)
        .await?)
}

/// A notification as it starts: a row of a person's Inbox.
#[derive(Debug, Clone, Copy)]
pub struct NewNotification<'a> {
    /// Whose Inbox.
    pub user_id: Id,
    /// The kind of notification.
    pub kind: &'a str,
    /// The node it happened at.
    pub node_id: Option<Id>,
    /// The thread it is about.
    pub thread_id: Option<Id>,
    /// The approval it is about.
    pub approval_id: Option<Id>,
    /// The chat it is about.
    pub chat_id: Option<Id>,
    /// Its title.
    pub title: &'a str,
    /// Its first line.
    pub body: &'a str,
    /// Whatever else its kind carries.
    pub data: &'a Json,
    /// `<kind>:<source>`: the same key twice is one row.
    pub dedupe_key: &'a str,
    /// When it happened.
    pub at: DateTime<Utc>,
}

/// An Inbox row that was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Announced {
    /// The row's identifier.
    pub id: Id,
    /// The number it was given in the person's feed.
    pub inbox_seq: i64,
}

/// Writes a row to the Inbox of a person and moves their feed on, unless they
/// have a row with that key already. The database is told that their feed
/// moved when the transaction ends.
pub async fn announce(
    conn: &mut Conn,
    new: &NewNotification<'_>,
) -> Result<Option<Announced>, DbError> {
    if super::operations::receivers(conn, &[new.user_id], new.kind)
        .await?
        .is_empty()
    {
        return Ok(None);
    }
    let seq: Option<i64> = sqlx::query_scalar(
        "UPDATE users SET inbox_seq = inbox_seq + 1
          WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM notifications WHERE user_id = $1 AND dedupe_key = $2)
      RETURNING inbox_seq",
    )
    .bind(new.user_id)
    .bind(new.dedupe_key)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(inbox_seq) = seq else {
        return Ok(None);
    };
    let id = orch_types::new_id();
    sqlx::query(
        "INSERT INTO notifications
            (id, user_id, kind, node_id, thread_id, approval_id, chat_id, title, body, data, dedupe_key, inbox_seq, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(new.user_id)
    .bind(new.kind)
    .bind(new.node_id)
    .bind(new.thread_id)
    .bind(new.approval_id)
    .bind(new.chat_id)
    .bind(new.title)
    .bind(new.body)
    .bind(notification_data(new.kind, new.data))
    .bind(new.dedupe_key)
    .bind(inbox_seq)
    .bind(new.at)
    .execute(&mut *conn)
    .await?;
    sqlx::query("SELECT pg_notify('orch_user', $1)")
        .bind(format!("{}:{inbox_seq}", new.user_id))
        .execute(conn)
        .await?;
    Ok(Some(Announced { id, inbox_seq }))
}
