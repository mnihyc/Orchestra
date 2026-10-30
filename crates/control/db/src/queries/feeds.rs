//! What the realtime socket reads of the database beside the rows of a thread: where each feed stands, the Inbox rows of a person after a number, what is gone, and how long the token of a socket is good.
//!
//! The socket sends a subscriber the rows stamped after the number it holds.
//! The rows of a thread are read by the queries of `threads` and those of a
//! tree by `hierarchy`; the rest is here. A feed's number only grows, and a
//! number that is announced is one whose rows are already committed, so a
//! reader that takes the rows stamped in `(after, upto]` for an `upto` it was
//! told or read from the feed's counter misses none.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

/// Where the feeds of some threads stand: for each of `ids` that is a thread,
/// the highest number given out.
pub async fn thread_seqs(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Seq)>, DbError> {
    Ok(
        sqlx::query_as("SELECT node_id, next_seq - 1 FROM threads WHERE node_id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Where the feeds of the trees of some root orgs stand.
pub async fn tree_seqs(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Seq)>, DbError> {
    Ok(
        sqlx::query_as("SELECT node_id, tree_seq FROM orgs WHERE node_id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// How many nodes of the tree of the root org `org` are stamped above `after`.
pub async fn tree_changed_after(conn: &mut Conn, org: Id, after: Seq) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE org_id = $1 AND tree_seq > $2")
            .bind(org)
            .bind(after)
            .fetch_one(conn)
            .await?,
    )
}

/// Where the feeds of some topics stand.
pub async fn topic_seqs(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Seq)>, DbError> {
    Ok(
        sqlx::query_as("SELECT node_id, topic_seq FROM topics WHERE node_id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Begins a transaction that reads the database as it was when it began, whatever commits meanwhile:
/// what the state behind a feed is read in, so that the number it ends with and the rows it holds
/// are of one moment.
pub async fn begin_reading(db: &crate::Db) -> Result<crate::Tx, DbError> {
    db.begin_isolated("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
}

/// Where the feeds of some environments stand.
pub async fn environment_seqs(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Seq)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, env_seq FROM environments WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Which of the environments `ids` are gone: there is no such environment, or it is deleted. One
/// that is only being removed is still there, and its feed says how the removal goes.
pub async fn gone_environments(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT i.id FROM unnest($1::uuid[]) AS i(id)
           LEFT JOIN environments e ON e.id = i.id
          WHERE e.id IS NULL OR e.deleted_at IS NOT NULL",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Where the inboxes of some people stand.
pub async fn inbox_seqs(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Seq)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, inbox_seq FROM users WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Which of the nodes `ids` are gone: there is no such node, or it is deleted,
/// or a node above it is. What was deleted is kept for a while and can come
/// back, so a feed of it ends with the reason `deleted` and is made anew by
/// whoever subscribes after.
pub async fn gone_nodes(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT i.id FROM unnest($1::uuid[]) AS i(id)
           LEFT JOIN nodes n ON n.id = i.id
          WHERE n.id IS NULL
             OR n.deleted_at IS NOT NULL
             OR EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// When the access token whose hash is `token_hash` stops being good, where it
/// is good at `now`: it has not expired, and the session it belongs to is
/// neither expired nor revoked. None for a token that is not good.
pub async fn access_token_expiry(
    conn: &mut Conn,
    token_hash: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT a.expires_at
           FROM access_tokens a JOIN sessions s ON s.id = a.session_id
          WHERE a.token_hash = $1 AND a.expires_at > $2 AND s.expires_at > $2 AND s.revoked_at IS NULL",
    )
    .bind(token_hash)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// One row of a person's Inbox, as it is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct InboxRow {
    /// Its identifier.
    pub id: Id,
    /// What it is about, as the word of the kind.
    pub kind: String,
    /// The place it happened in.
    pub node_id: Option<Id>,
    /// The thread it is about.
    pub thread_id: Option<Id>,
    /// The approval it is about.
    pub approval_id: Option<Id>,
    /// The chat it is about.
    pub chat_id: Option<Id>,
    /// Its title.
    pub title: String,
    /// More about it.
    pub body: String,
    /// What else its kind carries.
    pub data: Json,
    /// Its number in the person's feed.
    pub inbox_seq: Seq,
    /// When it was written.
    pub created_at: DateTime<Utc>,
    /// When the person read it.
    pub read_at: Option<DateTime<Utc>>,
}

/// The rows of a person's Inbox numbered in `(after, upto]`, in their order,
/// at most `limit`.
pub async fn inbox_between(
    conn: &mut Conn,
    user: Id,
    after: Seq,
    upto: Seq,
    limit: i64,
) -> Result<Vec<InboxRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, node_id, thread_id, approval_id, chat_id, title, body, data, inbox_seq, created_at, read_at
           FROM notifications
          WHERE user_id = $1 AND inbox_seq > $2 AND inbox_seq <= $3
          ORDER BY inbox_seq
          LIMIT $4",
    )
    .bind(user)
    .bind(after)
    .bind(upto)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// How many rows of a person's Inbox are numbered above `after`.
pub async fn inbox_after_count(conn: &mut Conn, user: Id, after: Seq) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND inbox_seq > $2",
    )
    .bind(user)
    .bind(after)
    .fetch_one(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::hierarchy::testing::insert_node;
    use crate::queries::identity::testing::{insert_session, insert_user};
    use chrono::Duration;
    use orch_testkit::database::TestDb;
    use orch_types::new_id;

    struct Tree {
        user: Id,
        org: Id,
        project: Id,
        topic: Id,
        thread: Id,
    }

    async fn tree(conn: &mut Conn) -> Tree {
        let (user, org, project, topic, thread) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        insert_user(conn, user, "ada").await.unwrap();
        insert_node(conn, org, "org", None, "Acme", user)
            .await
            .unwrap();
        insert_node(conn, project, "project", Some(org), "Web", user)
            .await
            .unwrap();
        insert_node(conn, topic, "topic", Some(project), "Login", user)
            .await
            .unwrap();
        insert_node(conn, thread, "thread", Some(topic), "Fix", user)
            .await
            .unwrap();
        Tree {
            user,
            org,
            project,
            topic,
            thread,
        }
    }

    #[tokio::test]
    async fn where_a_feed_stands_is_what_its_counter_holds_for_the_things_there_are() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        crate::sequences::next(&mut conn, t.thread).await.unwrap();
        crate::sequences::next(&mut conn, t.thread).await.unwrap();
        crate::queries::hierarchy::stamp(&mut conn, t.topic)
            .await
            .unwrap();
        let stranger = new_id();
        assert_eq!(
            thread_seqs(&mut conn, &[t.thread, stranger]).await.unwrap(),
            vec![(t.thread, 2)]
        );
        assert_eq!(
            topic_seqs(&mut conn, &[t.topic]).await.unwrap(),
            vec![(t.topic, 1)]
        );
        assert_eq!(
            tree_seqs(&mut conn, &[t.org]).await.unwrap(),
            vec![(t.org, 1)]
        );
        assert_eq!(
            inbox_seqs(&mut conn, &[t.user]).await.unwrap(),
            vec![(t.user, 0)]
        );
    }

    #[tokio::test]
    async fn a_node_is_gone_when_it_or_a_node_above_it_was_deleted_or_it_never_was() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let stranger = new_id();
        let all = [t.org, t.project, t.topic, t.thread, stranger];
        assert_eq!(gone_nodes(&mut conn, &all).await.unwrap(), vec![stranger]);
        sqlx::query("UPDATE nodes SET deleted_at = now() WHERE id = $1")
            .bind(t.topic)
            .execute(&mut *conn)
            .await
            .unwrap();
        let mut gone = gone_nodes(&mut conn, &all).await.unwrap();
        gone.sort_unstable();
        let mut expected = vec![t.topic, t.thread, stranger];
        expected.sort_unstable();
        assert_eq!(gone, expected);
    }

    #[tokio::test]
    async fn a_token_is_good_until_it_expires_or_its_session_is_revoked() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let now = orch_testkit::clock::start();
        let (session, hash) = (new_id(), [7u8; 32]);
        let until = now + Duration::minutes(15);
        insert_session(&mut conn, session, t.user, &hash, until)
            .await
            .unwrap();
        let found = access_token_expiry(&mut conn, &hash, now).await.unwrap();
        assert_eq!(found.map(|at| at.timestamp()), Some(until.timestamp()));
        assert_eq!(
            access_token_expiry(&mut conn, &hash, until + Duration::seconds(1))
                .await
                .unwrap(),
            None,
            "it ran out"
        );
        assert_eq!(
            access_token_expiry(&mut conn, &[8u8; 32], now)
                .await
                .unwrap(),
            None,
            "nobody knows it"
        );
        sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1")
            .bind(session)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            access_token_expiry(&mut conn, &hash, now).await.unwrap(),
            None,
            "its session was revoked"
        );
    }

    #[tokio::test]
    async fn the_inbox_after_a_number_comes_in_order_and_stops_at_the_number_asked() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        for number in 1..=5i64 {
            sqlx::query("UPDATE users SET inbox_seq = $2 WHERE id = $1")
                .bind(t.user)
                .bind(number)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO notifications (id, user_id, kind, title, dedupe_key, inbox_seq)
                 VALUES ($1, $2, 'milestone', $3, $4, $5)",
            )
            .bind(new_id())
            .bind(t.user)
            .bind(format!("row {number}"))
            .bind(format!("milestone:{number}"))
            .bind(number)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        let rows = inbox_between(&mut conn, t.user, 1, 4, 100).await.unwrap();
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["row 2", "row 3", "row 4"]);
        assert_eq!(inbox_after_count(&mut conn, t.user, 3).await.unwrap(), 2);
        assert_eq!(
            inbox_between(&mut conn, t.user, 0, 5, 2)
                .await
                .unwrap()
                .len(),
            2,
            "at most the limit"
        );
    }
}
