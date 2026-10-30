//! A person's Inbox as the pages read it: the list newest first, the counters of the badge, what is marked read, an approval that needs nobody any more, and what the person chose about being notified.
//!
//! The Inbox is state: a row is written in the transaction of the event it
//! reports ([`super::people::announce`]) and reaches open screens through the
//! person's feed. A row that changes after that (it is read) is given the
//! person's next number, so that the feed carries it again and every device
//! learns of it; the numbers only grow.

use crate::queries::feeds::InboxRow;
use crate::{Conn, DbError, sequences};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use std::collections::HashMap;

/// Where a page of the Inbox ends: the moment and the identifier of its last
/// row, which the next page begins after. Newest rows come first, so a page
/// continues with what is older.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct After {
    /// When the last row of the page was written.
    pub created_at: DateTime<Utc>,
    /// Its identifier, which orders rows written in the same moment.
    pub id: Id,
}

impl After {
    /// The position as text, for a client to hand back.
    #[must_use]
    pub fn to_cursor(self) -> String {
        format!("{}.{}", self.created_at.timestamp_micros(), self.id)
    }

    /// The position a cursor names; none for text this never wrote.
    #[must_use]
    pub fn from_cursor(text: &str) -> Option<Self> {
        let (micros, id) = text.split_once('.')?;
        Some(Self {
            created_at: DateTime::from_timestamp_micros(micros.parse().ok()?)?,
            id: id.parse().ok()?,
        })
    }
}

/// Which rows of an Inbox a list asks for.
#[derive(Debug, Clone, Copy, Default)]
pub struct ListFilter<'a> {
    /// Only those not read yet.
    pub unread_only: bool,
    /// Only these kinds; all where empty.
    pub kinds: &'a [String],
    /// Only those written at or after this moment.
    pub since: Option<DateTime<Utc>>,
}

/// The rows of a person's Inbox, newest first, as the filter asks, after the row a cursor names,
/// at most `limit`.
pub async fn list(
    conn: &mut Conn,
    user: Id,
    filter: &ListFilter<'_>,
    after: Option<After>,
    limit: i64,
) -> Result<Vec<InboxRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, node_id, thread_id, approval_id, chat_id, title, body, data, inbox_seq, created_at, read_at
           FROM notifications
          WHERE user_id = $1 AND (NOT $2 OR read_at IS NULL)
            AND ($3::timestamptz IS NULL OR (created_at, id) < ($3::timestamptz, $4::uuid))
            AND (cardinality($6::text[]) = 0 OR kind = ANY($6))
            AND ($7::timestamptz IS NULL OR created_at >= $7)
          ORDER BY created_at DESC, id DESC
          LIMIT $5",
    )
    .bind(user)
    .bind(filter.unread_only)
    .bind(after.map(|after| after.created_at))
    .bind(after.map(|after| after.id))
    .bind(limit)
    .bind(filter.kinds)
    .bind(filter.since)
    .fetch_all(conn)
    .await?)
}

/// The rows of a person's Inbox up to a cut of their sequence, as the filter asks, newest
/// first, at most `limit`: the set a list by place is paged through, which nothing newer
/// changes while it is paged.
pub async fn list_within_cut(
    conn: &mut Conn,
    user: Id,
    filter: &ListFilter<'_>,
    cut: i64,
    after: Option<After>,
    limit: i64,
) -> Result<Vec<InboxRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, node_id, thread_id, approval_id, chat_id, title, body, data, inbox_seq, created_at, read_at
           FROM notifications
          WHERE user_id = $1 AND (NOT $2 OR read_at IS NULL) AND inbox_seq <= $3
            AND (cardinality($4::text[]) = 0 OR kind = ANY($4))
            AND ($5::timestamptz IS NULL OR created_at >= $5)
            AND ($7::timestamptz IS NULL OR (created_at, id) < ($7, $8::uuid))
          ORDER BY created_at DESC, id DESC
          LIMIT $6",
    )
    .bind(user)
    .bind(filter.unread_only)
    .bind(cut)
    .bind(filter.kinds)
    .bind(filter.since)
    .bind(limit)
    .bind(after.map(|after| after.created_at))
    .bind(after.map(|after| after.id))
    .fetch_all(conn)
    .await?)
}

/// Marks these unread rows of a person read as one batch, which a ticket can undo: the person's
/// Inbox moves on one number, the rows take it, and the feed is told. Returns how many changed.
pub async fn mark_batch_read(
    conn: &mut Conn,
    user: Id,
    ids: &[Id],
    batch: Id,
    at: DateTime<Utc>,
) -> Result<i64, DbError> {
    let moved: Option<i64> = sqlx::query_scalar(
        "UPDATE users SET inbox_seq = inbox_seq + 1
          WHERE id = $1 AND EXISTS (
                SELECT 1 FROM notifications WHERE user_id = $1 AND read_at IS NULL AND id = ANY($2))
      RETURNING inbox_seq",
    )
    .bind(user)
    .bind(ids)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(seq) = moved else {
        return Ok(0);
    };
    let done = sqlx::query(
        "UPDATE notifications SET read_at = $3, read_batch_id = $4, inbox_seq = $5
          WHERE user_id = $1 AND read_at IS NULL AND id = ANY($2)",
    )
    .bind(user)
    .bind(ids)
    .bind(at)
    .bind(batch)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    sequences::notify_user(conn, user, seq).await?;
    Ok(i64::try_from(done.rows_affected()).unwrap_or(i64::MAX))
}

/// Undoes a batch read: its rows that still carry the batch are unread again, and nothing else
/// changes (a row read since by other means, or renewed, no longer carries it). Returns how many
/// changed.
pub async fn undo_batch(
    conn: &mut Conn,
    user: Id,
    batch: Id,
    readable: &[Id],
) -> Result<i64, DbError> {
    let moved: Option<i64> = sqlx::query_scalar(
        "UPDATE users SET inbox_seq = inbox_seq + 1
          WHERE id = $1 AND EXISTS (
                SELECT 1 FROM notifications WHERE user_id = $1 AND read_batch_id = $2 AND id = ANY($3))
      RETURNING inbox_seq",
    )
    .bind(user)
    .bind(batch)
    .bind(readable)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(seq) = moved else {
        return Ok(0);
    };
    let done = sqlx::query(
        "UPDATE notifications SET read_at = NULL, read_batch_id = NULL, inbox_seq = $4
          WHERE user_id = $1 AND read_batch_id = $2 AND id = ANY($3)",
    )
    .bind(user)
    .bind(batch)
    .bind(readable)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    sequences::notify_user(conn, user, seq).await?;
    Ok(i64::try_from(done.rows_affected()).unwrap_or(i64::MAX))
}

/// The rows a batch read changed that still carry it.
pub async fn rows_of_batch(conn: &mut Conn, user: Id, batch: Id) -> Result<Vec<InboxRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, node_id, thread_id, approval_id, chat_id, title, body, data, inbox_seq, created_at, read_at
           FROM notifications WHERE user_id = $1 AND read_batch_id = $2",
    )
    .bind(user)
    .bind(batch)
    .fetch_all(conn)
    .await?)
}

/// How many rows wait unread and how far the person's Inbox has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct Counts {
    /// Rows not read yet.
    pub unread: i64,
    /// The highest number their Inbox has given out.
    pub seq: i64,
}

/// Serializes a grouped read against announcements and renewals that allocate an Inbox sequence.
pub async fn lock_sequence(conn: &mut Conn, user: Id) -> Result<(), DbError> {
    sqlx::query("SELECT inbox_seq FROM users WHERE id = $1 FOR UPDATE")
        .bind(user)
        .execute(conn)
        .await?;
    Ok(())
}

/// The counters of a person's Inbox; none where there is no such person.
pub async fn counts(conn: &mut Conn, user: Id) -> Result<Option<Counts>, DbError> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM notifications WHERE user_id = $1 AND read_at IS NULL) AS unread,
                u.inbox_seq AS seq
           FROM users u WHERE u.id = $1",
    )
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// Which rows a request marks read. A row that any of the members names is
/// marked; a member left out names none.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadFilter<'a> {
    /// These rows.
    pub ids: &'a [Id],
    /// The rows that happened at this node.
    pub node: Option<Id>,
    /// The rows about this chat.
    pub chat: Option<Id>,
    /// The rows written at or before this moment.
    pub before: Option<DateTime<Utc>>,
}

/// Marks the person's rows read. When any was unread, the person's Inbox moves
/// on one number, the rows that changed take it and the feed is told; the
/// answer is that number. None where nothing changed.
pub async fn mark_read(
    conn: &mut Conn,
    user: Id,
    filter: &ReadFilter<'_>,
    at: DateTime<Utc>,
) -> Result<Option<i64>, DbError> {
    let moved: Option<i64> = sqlx::query_scalar(
        "UPDATE users SET inbox_seq = inbox_seq + 1
          WHERE id = $1 AND EXISTS (
                SELECT 1 FROM notifications
                 WHERE user_id = $1 AND read_at IS NULL
                   AND (id = ANY($2) OR node_id = $3::uuid OR chat_id = $4::uuid OR created_at <= $5::timestamptz))
      RETURNING inbox_seq",
    )
    .bind(user)
    .bind(filter.ids)
    .bind(filter.node)
    .bind(filter.chat)
    .bind(filter.before)
    .fetch_optional(&mut *conn)
    .await?;
    // A read by other means ends what a batch read can undo of these rows, even of those read
    // already, so that an older Undo never makes them unread again.
    sqlx::query(
        "UPDATE notifications SET read_batch_id = NULL
          WHERE user_id = $1 AND read_batch_id IS NOT NULL
            AND (id = ANY($2) OR node_id = $3::uuid OR chat_id = $4::uuid OR created_at <= $5::timestamptz)",
    )
    .bind(user)
    .bind(filter.ids)
    .bind(filter.node)
    .bind(filter.chat)
    .bind(filter.before)
    .execute(&mut *conn)
    .await?;
    let Some(seq) = moved else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE notifications SET read_at = $6, inbox_seq = $7
          WHERE user_id = $1 AND read_at IS NULL
            AND (id = ANY($2) OR node_id = $3::uuid OR chat_id = $4::uuid OR created_at <= $5::timestamptz)",
    )
    .bind(user)
    .bind(filter.ids)
    .bind(filter.node)
    .bind(filter.chat)
    .bind(filter.before)
    .bind(at)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    sequences::notify_user(conn, user, seq).await?;
    Ok(Some(seq))
}

/// Marks as read every row that is about an approval, for everybody it
/// was told to: once an approval has been decided or withdrawn nobody has
/// anything left to do about it, and a message that still asked would be
/// sent after the fact. Each person's Inbox moves on one number. Says whose
/// feeds moved. Already-read rows also move: their pending/actionable count changed, while their read time stays.
pub async fn settle_approval(
    conn: &mut Conn,
    approval: Id,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, DbError> {
    settle_request(conn, RequestSource::Approval(approval), at).await
}

/// The source identity of any request with several possible deciders.
#[derive(Debug, Clone, Copy)]
pub enum RequestSource {
    /// A harness, capability, port or access approval.
    Approval(Id),
    /// A plan kept on a completed planning turn.
    Plan(Id),
    /// A message held for someone who may use the model.
    Input(Id),
}

/// Settles every copy in one place, preserving history and publishing changed sequences without a notice.
pub async fn settle_request(
    conn: &mut Conn,
    source: RequestSource,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, DbError> {
    let approval: Option<Id> = match source {
        RequestSource::Approval(id) => Some(id),
        RequestSource::Plan(id) => {
            sqlx::query_scalar("SELECT (payload->>'approval_id')::uuid FROM items WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
                .flatten()
        }
        RequestSource::Input(id) => {
            sqlx::query_scalar("SELECT approval_id FROM thread_inputs WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
                .flatten()
        }
    };
    let Some(approval) = approval else {
        return Ok(Vec::new());
    };
    let moved: Vec<(Id, i64)> = sqlx::query_as(
        "WITH affected AS (
            SELECT DISTINCT user_id FROM notifications WHERE approval_id=$1
              AND data ->> '_orch_approval_settled' IS DISTINCT FROM 'true'
         ), bumped AS (
            UPDATE users u SET inbox_seq = u.inbox_seq + 1 FROM affected a WHERE u.id = a.user_id
            RETURNING u.id, u.inbox_seq
         )
         UPDATE notifications n SET read_at = COALESCE(n.read_at, $2), read_batch_id = NULL, inbox_seq = b.inbox_seq,
            data = (CASE WHEN jsonb_typeof(n.data) = 'object' THEN n.data ELSE jsonb_build_object('_orch_legacy', n.data) END)
                   || jsonb_build_object('_orch_approval_settled', true)
           FROM bumped b
          WHERE n.user_id = b.id AND n.approval_id=$1
            AND n.data ->> '_orch_approval_settled' IS DISTINCT FROM 'true'
      RETURNING n.user_id, n.inbox_seq",
    )
    .bind(approval)
    .bind(at)
    .fetch_all(&mut *conn)
    .await?;
    let mut people: Vec<Id> = Vec::new();
    for (user, seq) in moved {
        if !people.contains(&user) {
            sequences::notify_user(conn, user, seq).await?;
            people.push(user);
        }
    }
    Ok(people)
}

/// What a person chose about being notified, apart from their routes.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PrefsRow {
    /// Kinds this person has switched off, as an array of stable keys.
    pub disabled_kinds: Json,
    /// Whether to be told of approvals they can answer wherever they are not subscribed.
    pub approvals_always: bool,
    /// Their level in the chat of a project, topic or thread until they set one there.
    pub chat_node_level: String,
    /// Their level in direct and group chats until they set one there.
    pub chat_direct_level: String,
    /// The minute after local midnight at which quiet hours begin.
    pub quiet_start_minute: Option<i32>,
    /// The minute after local midnight at which they end.
    pub quiet_end_minute: Option<i32>,
    /// Until when every way out is paused, as stored: a moment that has passed holds nothing.
    pub pause_until: Option<DateTime<Utc>>,
    /// The revision a change is made from; every change moves it on.
    pub revision: i64,
}

/// The revision of a person's choices before they chose anything: the schema's default.
pub const FIRST_PREFS_REVISION: i64 = 1;

impl PrefsRow {
    /// What a person who has chosen nothing has: the defaults of the schema.
    #[must_use]
    pub fn unchosen() -> Self {
        Self {
            disabled_kinds: serde_json::json!([]),
            approvals_always: true,
            chat_node_level: "mentions".to_owned(),
            chat_direct_level: "all".to_owned(),
            quiet_start_minute: None,
            quiet_end_minute: None,
            pause_until: None,
            revision: FIRST_PREFS_REVISION,
        }
    }

    /// The pause that holds at `now`: the stored moment while it is still ahead.
    #[must_use]
    pub fn pause_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.pause_until.filter(|until| *until > now)
    }
}

/// The columns a person's choices are read with.
macro_rules! prefs_columns {
    () => {
        "disabled_kinds, approvals_always, chat_node_level, chat_direct_level, quiet_start_minute, quiet_end_minute, pause_until, revision"
    };
}

/// What a person chose, or the defaults where they chose nothing.
pub async fn prefs(conn: &mut Conn, user: Id) -> Result<PrefsRow, DbError> {
    let found: Option<PrefsRow> = sqlx::query_as(concat!(
        "SELECT ",
        prefs_columns!(),
        " FROM notification_prefs WHERE user_id = $1"
    ))
    .bind(user)
    .fetch_optional(conn)
    .await?;
    Ok(found.unwrap_or_else(PrefsRow::unchosen))
}

/// What is changed of a person's choices; what is `None` stays. `quiet` is
/// the minutes quiet hours begin and end at, or `Some(None)` to take them away;
/// `pause` is the moment every way out is paused until, or `Some(None)` to resume.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrefsChange<'a> {
    /// A replacement array of kinds the person has switched off.
    pub disabled_kinds: Option<&'a Json>,
    /// Whether to be told of approvals wherever they are not subscribed.
    pub approvals_always: Option<bool>,
    /// The level in the chat of a node.
    pub chat_node_level: Option<&'a str>,
    /// The level in direct and group chats.
    pub chat_direct_level: Option<&'a str>,
    /// Quiet hours.
    pub quiet: Option<Option<(i32, i32)>>,
    /// The pause.
    pub pause: Option<Option<DateTime<Utc>>>,
}

/// Changes a person's choices, making their row if they had none, and returns
/// them as they are now. Every change moves the revision on.
pub async fn put_prefs(
    conn: &mut Conn,
    user: Id,
    change: &PrefsChange<'_>,
) -> Result<PrefsRow, DbError> {
    let (set_quiet, start, end) = match change.quiet {
        Some(Some((start, end))) => (true, Some(start), Some(end)),
        Some(None) => (true, None, None),
        None => (false, None, None),
    };
    let (set_pause, pause) = match change.pause {
        Some(value) => (true, value),
        None => (false, None),
    };
    Ok(sqlx::query_as(concat!(
        "INSERT INTO notification_prefs (user_id, approvals_always, chat_node_level, chat_direct_level, quiet_start_minute, quiet_end_minute, pause_until, disabled_kinds)
         VALUES ($1, COALESCE($2::bool, true), COALESCE($3::text, 'mentions'), COALESCE($4::text, 'all'), $6::int, $7::int, $9::timestamptz, COALESCE($10::jsonb, '[]'::jsonb))
         ON CONFLICT (user_id) DO UPDATE SET
            approvals_always = COALESCE($2::bool, notification_prefs.approvals_always),
            chat_node_level = COALESCE($3::text, notification_prefs.chat_node_level),
            chat_direct_level = COALESCE($4::text, notification_prefs.chat_direct_level),
            quiet_start_minute = CASE WHEN $5::bool THEN $6::int ELSE notification_prefs.quiet_start_minute END,
            quiet_end_minute = CASE WHEN $5::bool THEN $7::int ELSE notification_prefs.quiet_end_minute END,
            pause_until = CASE WHEN $8::bool THEN $9::timestamptz ELSE notification_prefs.pause_until END,
            disabled_kinds = COALESCE($10::jsonb, notification_prefs.disabled_kinds),
            revision = notification_prefs.revision + 1
         RETURNING ",
        prefs_columns!()
    ))
    .bind(user)
    .bind(change.approvals_always)
    .bind(change.chat_node_level)
    .bind(change.chat_direct_level)
    .bind(set_quiet)
    .bind(start)
    .bind(end)
    .bind(set_pause)
    .bind(pause)
    .bind(change.disabled_kinds)
    .fetch_one(conn)
    .await?)
}

/// Changes a person's choices only when they were made from the revision there is: the row is
/// taken under its lock first (the one lock a pause takes), and a change made from another
/// revision is refused with the choices as they are (`Err`). Returns them as they are now.
pub async fn put_prefs_at(
    conn: &mut Conn,
    user: Id,
    expected_revision: i64,
    change: &PrefsChange<'_>,
) -> Result<Result<PrefsRow, PrefsRow>, DbError> {
    sqlx::query(
        "INSERT INTO notification_prefs (user_id) VALUES ($1) ON CONFLICT (user_id) DO NOTHING",
    )
    .bind(user)
    .execute(&mut *conn)
    .await?;
    let current: PrefsRow = sqlx::query_as(concat!(
        "SELECT ",
        prefs_columns!(),
        " FROM notification_prefs WHERE user_id = $1 FOR UPDATE"
    ))
    .bind(user)
    .fetch_one(&mut *conn)
    .await?;
    if current.revision != expected_revision {
        return Ok(Err(current));
    }
    Ok(Ok(put_prefs(conn, user, change).await?))
}

/// Ends the pauses that have run out by `now`, each under the lock of its row and only where the
/// stored moment is still the one that ran out, so that a pause set meanwhile is never cleared:
/// the revision moves on. Returns the people whose pause ended.
pub async fn expire_pauses(conn: &mut Conn, now: DateTime<Utc>) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE notification_prefs p SET pause_until = NULL, revision = p.revision + 1
           FROM (SELECT user_id, pause_until FROM notification_prefs
                  WHERE pause_until IS NOT NULL AND pause_until <= $1
                  ORDER BY user_id FOR UPDATE SKIP LOCKED) due
          WHERE p.user_id = due.user_id AND p.pause_until = due.pause_until
      RETURNING p.user_id",
    )
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// For each of these people, whether they want to be told of the approvals
/// they can answer wherever they are not subscribed. A person who chose
/// nothing wants to be.
pub async fn approvals_always_of(
    conn: &mut Conn,
    users: &[Id],
) -> Result<HashMap<Id, bool>, DbError> {
    let chosen: Vec<(Id, bool)> = sqlx::query_as(
        "SELECT user_id, approvals_always FROM notification_prefs WHERE user_id = ANY($1)",
    )
    .bind(users)
    .fetch_all(conn)
    .await?;
    let chosen: HashMap<Id, bool> = chosen.into_iter().collect();
    Ok(users
        .iter()
        .map(|user| (*user, chosen.get(user).copied().unwrap_or(true)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::identity::testing::insert_user;
    use crate::queries::notifications::people::{NewNotification, announce};
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    async fn person(conn: &mut Conn, name: &str) -> Id {
        let id = new_id();
        insert_user(conn, id, name).await.unwrap();
        id
    }

    async fn tell(conn: &mut Conn, user: Id, key: &str, approval: Option<Id>) -> Id {
        announce(
            conn,
            &NewNotification {
                user_id: user,
                kind: "milestone",
                node_id: None,
                thread_id: None,
                approval_id: approval,
                chat_id: None,
                title: key,
                body: "",
                data: &json!({}),
                dedupe_key: key,
                at: clock::start(),
            },
        )
        .await
        .unwrap()
        .expect("a row was written")
        .id
    }

    #[test]
    fn a_cursor_names_the_row_it_was_made_from_and_nothing_else() {
        let after = After {
            created_at: DateTime::from_timestamp_micros(1_760_000_000_123_456).unwrap(),
            id: orch_types::new_id(),
        };
        assert_eq!(After::from_cursor(&after.to_cursor()), Some(after));
        for text in [
            "",
            "x",
            "12.not-an-id",
            "nope.0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b",
        ] {
            assert_eq!(After::from_cursor(text), None, "{text}");
        }
    }

    #[tokio::test]
    async fn rows_that_are_read_take_the_next_number_once_and_rows_already_read_keep_theirs() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = person(&mut conn, "ada").await;
        let (first, second, third) = (
            tell(&mut conn, ada, "one", None).await,
            tell(&mut conn, ada, "two", None).await,
            tell(&mut conn, ada, "three", None).await,
        );
        let now = clock::start();
        let seq = mark_read(
            &mut conn,
            ada,
            &ReadFilter {
                ids: &[first, second],
                ..ReadFilter::default()
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(
            seq,
            Some(4),
            "one number for the call, after the three rows"
        );
        let after: Vec<(Id, i64, bool)> = sqlx::query_as(
            "SELECT id, inbox_seq, read_at IS NOT NULL FROM notifications WHERE user_id = $1 ORDER BY created_at, inbox_seq",
        )
        .bind(ada)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        let by_id = |id: Id| after.iter().find(|row| row.0 == id).unwrap();
        assert_eq!((by_id(first).1, by_id(first).2), (4, true));
        assert_eq!((by_id(second).1, by_id(second).2), (4, true));
        assert_eq!((by_id(third).1, by_id(third).2), (3, false));

        // Naming what is read already changes nothing and takes no number.
        let again = mark_read(
            &mut conn,
            ada,
            &ReadFilter {
                ids: &[first],
                ..ReadFilter::default()
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(again, None);
        let badge = counts(&mut conn, ada).await.unwrap().unwrap();
        assert_eq!((badge.unread, badge.seq), (1, 4));
    }

    #[tokio::test]
    async fn an_approval_that_needs_nobody_any_more_is_read_for_everyone_it_was_told_to_and_nobody_else()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bob, cyd) = (
            person(&mut conn, "ada").await,
            person(&mut conn, "bob").await,
            person(&mut conn, "cyd").await,
        );
        let (one, other) = (new_id(), new_id());
        tell(&mut conn, ada, "approval:one", Some(one)).await;
        tell(&mut conn, bob, "approval:one", Some(one)).await;
        tell(&mut conn, cyd, "approval:other", Some(other)).await;
        let dana = person(&mut conn, "dana").await;
        let opened = tell(&mut conn, dana, "approval:one", Some(one)).await;
        let opened_at = clock::start() + chrono::Duration::seconds(1);
        mark_read(
            &mut conn,
            dana,
            &ReadFilter {
                ids: &[opened],
                ..ReadFilter::default()
            },
            opened_at,
        )
        .await
        .unwrap();

        let settled_at = clock::start() + chrono::Duration::seconds(2);
        let mut moved = settle_approval(&mut conn, one, settled_at).await.unwrap();
        moved.sort();
        let mut told = vec![ada, bob, dana];
        told.sort();
        assert_eq!(moved, told);
        for (who, unread) in [(ada, 0), (bob, 0), (cyd, 1)] {
            assert_eq!(
                counts(&mut conn, who).await.unwrap().unwrap().unread,
                unread
            );
        }
        assert_eq!(counts(&mut conn, ada).await.unwrap().unwrap().seq, 2);
        assert_eq!(
            counts(&mut conn, dana).await.unwrap().unwrap().seq,
            3,
            "opening the notice does not suppress its decision/count update"
        );
        let read_at: DateTime<Utc> =
            sqlx::query_scalar("SELECT read_at FROM notifications WHERE id=$1")
                .bind(opened)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(read_at, opened_at, "the original read time is retained");
        assert_eq!(
            counts(&mut conn, cyd).await.unwrap().unwrap().seq,
            1,
            "hers did not move"
        );
        assert!(
            settle_approval(&mut conn, one, clock::start())
                .await
                .unwrap()
                .is_empty(),
            "settled once"
        );
    }

    #[tokio::test]
    async fn the_choices_of_a_person_start_as_shipped_and_change_a_member_at_a_time() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = person(&mut conn, "ada").await;
        assert_eq!(prefs(&mut conn, ada).await.unwrap(), PrefsRow::unchosen());
        assert!(
            approvals_always_of(&mut conn, &[ada]).await.unwrap()[&ada],
            "somebody who chose nothing wants to be told of approvals"
        );
        let changed = put_prefs(
            &mut conn,
            ada,
            &PrefsChange {
                approvals_always: Some(false),
                quiet: Some(Some((1320, 420))),
                ..PrefsChange::default()
            },
        )
        .await
        .unwrap();
        assert!(!changed.approvals_always);
        assert_eq!(changed.chat_node_level, "mentions");
        assert_eq!(
            (changed.quiet_start_minute, changed.quiet_end_minute),
            (Some(1320), Some(420))
        );
        let later = put_prefs(
            &mut conn,
            ada,
            &PrefsChange {
                chat_direct_level: Some("mentions"),
                ..PrefsChange::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(later.chat_direct_level, "mentions");
        assert!(!later.approvals_always, "what is left out stays");
        assert_eq!(later.quiet_start_minute, Some(1320));
        let cleared = put_prefs(
            &mut conn,
            ada,
            &PrefsChange {
                quiet: Some(None),
                ..PrefsChange::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (cleared.quiet_start_minute, cleared.quiet_end_minute),
            (None, None)
        );
        assert!(!approvals_always_of(&mut conn, &[ada]).await.unwrap()[&ada]);
    }
}
