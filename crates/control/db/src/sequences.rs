//! Transactional feed sequences and database notifications.
//!
//! A thread's changes are numbered in the order they commit: the writer
//! locks the thread's row by asking for the next number, stamps every row it
//! changes with a number of its own, and the notification that tells every
//! process there is more to read is queued in the same transaction, so that
//! PostgreSQL delivers it when, and only when, the transaction commits. A
//! notification is a wake-up and carries no data: whoever receives one reads
//! the rows stamped after the last number it sent.
//!
//! The other feeds of the realtime socket work the same way, each with the
//! counter of its own row: a topic's, a root org's tree (stamped by
//! `queries::hierarchy::stamp`) and a person's inbox. What a writer calls inside
//! its transaction is the stamp of its feed and the matching `notify_*`; the
//! sweep of every API process finds what an announcement did not.
//!
//! Three more announcements are not about a sequence. The authorization epoch
//! moved ([`notify_epoch`]): every open subscription is asked again. A person
//! has news that is no row ([`notify_user_event`]): something changed on one
//! of their connections, or an approval waits for them; it reaches every API
//! process, and so every connection of theirs, and is not kept. And a process
//! says who reads what on its connections ([`PRESENCE_CHANNEL`]).

use crate::{Conn, Db, DbError};
use orch_types::{Id, Json, Seq};
use sqlx::postgres::{PgListener, PgPoolOptions};

/// The channel on which a thread's changes are announced; its payload is
/// `<thread id>:<sequence>`.
pub const THREAD_CHANNEL: &str = "orch_thread";

/// The next number of a thread's sequence, taken while the thread's row is
/// locked until the transaction ends, so that numbers are given out in the
/// order the transactions commit. None where there is no such thread.
pub async fn next(conn: &mut Conn, thread: Id) -> Result<Option<Seq>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE threads SET next_seq = next_seq + 1 WHERE node_id = $1 RETURNING next_seq - 1",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// The highest number given out for a thread; none where there is no such
/// thread. A thread that has had no change has given out none, which is `0`.
pub async fn current(conn: &mut Conn, thread: Id) -> Result<Option<Seq>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT next_seq - 1 FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_optional(conn)
            .await?,
    )
}

/// The payload that announces number `seq` of `thread`.
#[must_use]
pub fn payload(thread: Id, seq: Seq) -> String {
    format!("{thread}:{seq}")
}

/// The thread and the number an announcement names; none for a payload that
/// is not one.
#[must_use]
pub fn parse_payload(payload: &str) -> Option<(Id, Seq)> {
    let (thread, seq) = payload.split_once(':')?;
    Some((thread.parse().ok()?, seq.parse().ok()?))
}

/// Queues the announcement that `thread` has changes up to `seq`. It is
/// delivered when the transaction commits, and not at all if it does not.
pub async fn notify(conn: &mut Conn, thread: Id, seq: Seq) -> Result<(), DbError> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(THREAD_CHANNEL)
        .bind(payload(thread, seq))
        .execute(conn)
        .await?;
    Ok(())
}

/// The channel on which a topic's changes are announced; its payload is
/// `<topic id>:<sequence>`.
pub const TOPIC_CHANNEL: &str = "orch_topic";
/// The channel on which the tree of a root org is announced; its payload is
/// `<org id>:<sequence>`. `queries::hierarchy::stamp` sends it.
pub const TREE_CHANNEL: &str = "orch_tree";
/// The channel of a person's inbox. Its payload is `<user id>:<sequence>`
/// when a row was added, or a JSON object when it is news that is no row
/// ([`notify_user_event`]); [`parse_user_payload`] tells them apart.
pub const USER_CHANNEL: &str = "orch_user";
/// The channel on which a move of the authorization epoch is announced; its
/// payload is the new epoch.
pub const AUTHZ_CHANNEL: &str = "orch_authz";
/// Personal credential changes, announced at commit without writing the global epoch.
/// Its payload is the person whose live connections must be checked again.
pub const IDENTITY_CHANNEL: &str = "orch_identity";
/// The channel on which a process of the API says who reads what on its
/// connections; its payload is whatever the realtime server writes.
pub const PRESENCE_CHANNEL: &str = "orch_presence";

/// The most bytes of a payload that is sent. PostgreSQL refuses a payload of
/// 8000 bytes or more; this leaves room under it for the arithmetic of
/// whoever counts the bytes of a longer text into pieces.
pub const MAX_PAYLOAD_BYTES: usize = 7_900;

async fn announce(conn: &mut Conn, channel: &str, payload: &str) -> Result<(), DbError> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(channel)
        .bind(payload)
        .execute(conn)
        .await?;
    Ok(())
}

/// Queues the announcement that `topic` has changes up to `seq`. It is
/// delivered when the transaction commits, and not at all if it does not.
pub async fn notify_topic(conn: &mut Conn, topic: Id, seq: Seq) -> Result<(), DbError> {
    announce(conn, TOPIC_CHANNEL, &payload(topic, seq)).await
}

/// Queues the announcement that the tree of the root org `org` has changes
/// up to `seq`, for a writer that stamps nodes by itself and not through
/// `queries::hierarchy::stamp`, which announces for its caller.
pub async fn notify_tree(conn: &mut Conn, org: Id, seq: Seq) -> Result<(), DbError> {
    announce(conn, TREE_CHANNEL, &payload(org, seq)).await
}

/// Queues the announcement that the inbox of `user` has rows up to `seq`.
pub async fn notify_user(conn: &mut Conn, user: Id, seq: Seq) -> Result<(), DbError> {
    announce(conn, USER_CHANNEL, &payload(user, seq)).await
}

/// Queues the announcement that the authorization epoch is now `epoch`: whoever
/// holds something open on the strength of what was decided before asks again.
pub async fn notify_epoch(conn: &mut Conn, epoch: i64) -> Result<(), DbError> {
    announce(conn, AUTHZ_CHANNEL, &epoch.to_string()).await
}

/// Rechecks only this person's live credentials/second-step holds after commit.
pub async fn notify_identity(conn: &mut Conn, user: Id) -> Result<(), DbError> {
    announce(conn, IDENTITY_CHANNEL, &user.to_string()).await
}

/// Announces changed public metadata inputs after commit, without enumerating people.
/// Each API process tells only its connected personal-feed readers to reread their own Meta.
pub async fn notify_meta_changed(conn: &mut Conn, request_id: Option<&str>) -> Result<(), DbError> {
    let payload = match request_id.filter(|id| !id.is_empty()) {
        Some(id) => serde_json::json!({ "meta_changed": true, "request_id": id }),
        None => serde_json::json!({ "meta_changed": true }),
    };
    announce(conn, USER_CHANNEL, &payload.to_string()).await
}

/// Queues news for `users` that is no row of any feed: `event` is what the
/// `user` feed's `event` frame carries (`{ "what": "favorites" }`). It reaches
/// every connection of theirs on every API process when the transaction
/// commits, and is not kept. Returns false, and sends nothing, when the news
/// does not fit in one announcement.
pub async fn notify_user_event(
    conn: &mut Conn,
    users: &[Id],
    event: &Json,
) -> Result<bool, DbError> {
    // A notice names its people in pieces that each fit, with the same event.
    let event_bytes = event.to_string().len();
    // The keys, braces and one comma around the people and the event, and the
    // two quotes, a comma and a space around each identifier of 36 characters.
    const FRAME_BYTES: usize = 32;
    const PERSON_BYTES: usize = 36 + 4;
    let room = MAX_PAYLOAD_BYTES.saturating_sub(FRAME_BYTES + event_bytes);
    let per_notice = room / PERSON_BYTES;
    if per_notice == 0 {
        return Ok(false);
    }
    for piece in users.chunks(per_notice) {
        let text = serde_json::json!({ "users": piece, "event": event }).to_string();
        announce(conn, USER_CHANNEL, &text).await?;
    }
    Ok(true)
}

/// What an announcement on [`USER_CHANNEL`] says.
#[derive(Debug, Clone, PartialEq)]
pub enum UserNotice {
    /// The inbox of a person has rows up to a number.
    Inbox {
        /// The person.
        user: Id,
        /// The number.
        seq: Seq,
    },
    /// A payload-free hint to current personal-feed readers, never a directory of users.
    MetaChanged {
        /// Native mutation causation; absent for recovery or a writer without an HTTP receipt.
        request_id: Option<String>,
    },
    /// News for some people that is no row.
    Event {
        /// The people.
        users: Vec<Id>,
        /// The news, as the `event` frame carries it.
        event: Json,
    },
}

/// What a payload on [`USER_CHANNEL`] says; none for one that says nothing
/// this reads.
#[must_use]
pub fn parse_user_payload(payload: &str) -> Option<UserNotice> {
    if payload.starts_with('{') {
        let value: Json = serde_json::from_str(payload).ok()?;
        if value.get("meta_changed") == Some(&Json::Bool(true)) {
            let request_id = match value.get("request_id") {
                None | Some(Json::Null) => None,
                Some(Json::String(id)) if !id.is_empty() => Some(id.clone()),
                _ => return None,
            };
            return Some(UserNotice::MetaChanged { request_id });
        }
        let users = value
            .get("users")?
            .as_array()?
            .iter()
            .filter_map(|id| id.as_str()?.parse().ok())
            .collect();
        return Some(UserNotice::Event {
            users,
            event: value.get("event")?.clone(),
        });
    }
    let (user, seq) = parse_payload(payload)?;
    Some(UserNotice::Inbox { user, seq })
}

/// Queues what a process of the API says of who reads what, for the others;
/// false, with nothing sent, when it does not fit in one announcement.
pub async fn notify_presence(conn: &mut Conn, text: &str) -> Result<bool, DbError> {
    if text.len() > MAX_PAYLOAD_BYTES {
        return Ok(false);
    }
    announce(conn, PRESENCE_CHANNEL, text).await?;
    Ok(true)
}

/// What a listener heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// An announcement.
    Heard {
        /// The channel it came on.
        channel: String,
        /// What it said.
        payload: String,
    },
    /// The connection was lost and made again. Announcements sent while it
    /// was gone are lost: whoever acts on them reads everything again.
    Reconnected,
}

/// A connection of its own that listens on some channels, and does not take
/// a place from the pool of the requests.
#[derive(Debug)]
pub struct Notices {
    listener: PgListener,
}

/// Starts listening on `channels`. The connection is made like the pool's
/// and is held for as long as the listener lives.
pub async fn listen(db: &Db, channels: &[&str]) -> Result<Notices, DbError> {
    let options = (*db.pool().connect_options()).clone();
    // The listener's pool only ever makes this one connection again when it
    // breaks, and keeps it for good: nothing else is asked of it.
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_with(options)
        .await?;
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.ignore_pool_close_event(true);
    listener.listen_all(channels.iter().copied()).await?;
    Ok(Notices { listener })
}

impl Notices {
    /// The next announcement, waiting for it. A connection that broke and was
    /// made again is told as [`Notice::Reconnected`]. An error is a
    /// connection that cannot be made again for now: the caller waits a
    /// while and asks again, and the listener tries again then.
    pub async fn next(&mut self) -> Result<Notice, DbError> {
        Ok(match self.listener.try_recv().await? {
            Some(heard) => Notice::Heard {
                channel: heard.channel().to_owned(),
                payload: heard.payload().to_owned(),
            },
            None => Notice::Reconnected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::hierarchy::testing::insert_node;
    use crate::queries::identity::testing::insert_user;
    use orch_testkit::database::TestDb;
    use orch_types::new_id;

    async fn thread(conn: &mut Conn) -> Id {
        let (user, org, topic, thread) = (new_id(), new_id(), new_id(), new_id());
        insert_user(conn, user, "ada").await.unwrap();
        insert_node(conn, org, "org", None, "Acme", user)
            .await
            .unwrap();
        let project = new_id();
        insert_node(conn, project, "project", Some(org), "Web", user)
            .await
            .unwrap();
        insert_node(conn, topic, "topic", Some(project), "Login", user)
            .await
            .unwrap();
        insert_node(conn, thread, "thread", Some(topic), "Fix", user)
            .await
            .unwrap();
        thread
    }

    #[tokio::test]
    async fn numbers_are_given_out_one_after_the_other_from_one() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let thread = thread(&mut conn).await;
        assert_eq!(current(&mut conn, thread).await.unwrap(), Some(0));
        assert_eq!(next(&mut conn, thread).await.unwrap(), Some(1));
        assert_eq!(next(&mut conn, thread).await.unwrap(), Some(2));
        assert_eq!(current(&mut conn, thread).await.unwrap(), Some(2));
        assert_eq!(next(&mut conn, new_id()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_second_writer_waits_for_the_first_to_commit_and_gets_the_next_number() {
        let test = TestDb::fresh().await;
        let thread = {
            let mut conn = test.pool.acquire().await.unwrap();
            thread(&mut conn).await
        };
        let mut first = test.pool.begin().await.unwrap();
        assert_eq!(next(&mut first, thread).await.unwrap(), Some(1));
        let pool = test.pool.clone();
        let second = tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            let seq = next(&mut tx, thread).await.unwrap();
            tx.commit().await.unwrap();
            seq
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!second.is_finished(), "the thread's row is locked");
        first.commit().await.unwrap();
        assert_eq!(second.await.unwrap(), Some(2));
    }

    #[tokio::test]
    async fn the_announcement_reaches_a_listener_after_the_commit_and_not_before() {
        let test = TestDb::fresh().await;
        let thread = {
            let mut conn = test.pool.acquire().await.unwrap();
            thread(&mut conn).await
        };
        let mut listener = sqlx::postgres::PgListener::connect(&test.url)
            .await
            .unwrap();
        listener.listen(THREAD_CHANNEL).await.unwrap();
        let mut tx = test.pool.begin().await.unwrap();
        let seq = next(&mut tx, thread).await.unwrap().unwrap();
        notify(&mut tx, thread, seq).await.unwrap();
        let early =
            tokio::time::timeout(std::time::Duration::from_millis(200), listener.recv()).await;
        assert!(early.is_err(), "nothing is announced before the commit");
        tx.commit().await.unwrap();
        let heard = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parse_payload(heard.payload()), Some((thread, seq)));
    }

    #[test]
    fn a_payload_reads_back_and_a_stranger_is_refused() {
        let thread = new_id();
        assert_eq!(parse_payload(&payload(thread, 41)), Some((thread, 41)));
        assert_eq!(parse_payload("nothing"), None);
        assert_eq!(parse_payload(&format!("{thread}:x")), None);
    }

    #[test]
    fn a_payload_of_the_inbox_and_one_of_news_are_told_apart() {
        let (first, second) = (new_id(), new_id());
        assert_eq!(
            parse_user_payload(&payload(first, 9)),
            Some(UserNotice::Inbox {
                user: first,
                seq: 9
            })
        );
        let news =
            serde_json::json!({ "users": [first, second], "event": { "what": "favorites" } });
        assert_eq!(
            parse_user_payload(&news.to_string()),
            Some(UserNotice::Event {
                users: vec![first, second],
                event: serde_json::json!({ "what": "favorites" }),
            })
        );
        assert_eq!(parse_user_payload("{not json"), None);
        assert_eq!(parse_user_payload("{\"users\": []}"), None);
    }

    #[tokio::test]
    async fn a_personal_credential_change_notifies_only_that_person_after_commit_without_moving_the_epoch()
     {
        use crate::queries::{
            access, identity,
            transaction_guard::{self as locks, Scope},
        };
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let user = new_id();
        identity::testing::insert_user(&mut db.acquire().await.unwrap(), user, "personal")
            .await
            .unwrap();
        let mut notices = listen(&db, &[IDENTITY_CHANNEL, AUTHZ_CHANNEL])
            .await
            .unwrap();
        locks::request_scope(Scope::Personal { user: Some(user) }, async {
            let mut tx = db.begin().await.unwrap();
            let before = access::epoch(&mut tx).await.unwrap();
            assert_eq!(access::bump_epoch(&mut tx).await.unwrap(), before);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), notices.next())
                    .await
                    .is_err()
            );
            tx.commit().await.unwrap();
        })
        .await;
        let notice = tokio::time::timeout(std::time::Duration::from_secs(5), notices.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            notice,
            Notice::Heard {
                channel: IDENTITY_CHANNEL.to_owned(),
                payload: user.to_string()
            }
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), notices.next())
                .await
                .is_err(),
            "no global epoch notification"
        );
    }

    #[tokio::test]
    async fn metadata_hints_commit_once_without_people_or_authority_changes() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let mut notices = listen(&db, &[USER_CHANNEL, AUTHZ_CHANNEL]).await.unwrap();
        let mut tx = test.pool.begin().await.unwrap();
        notify_meta_changed(&mut tx, Some("committed-request"))
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), notices.next())
                .await
                .is_err()
        );
        let mut tx = test.pool.begin().await.unwrap();
        notify_meta_changed(&mut tx, Some("committed-request"))
            .await
            .unwrap();
        notify_meta_changed(&mut tx, Some("committed-request"))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let Notice::Heard { channel, payload } =
            tokio::time::timeout(std::time::Duration::from_secs(5), notices.next())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("the committed metadata hint was not delivered");
        };
        assert_eq!(channel, USER_CHANNEL);
        assert_eq!(
            parse_user_payload(&payload),
            Some(UserNotice::MetaChanged {
                request_id: Some("committed-request".to_owned())
            })
        );
        assert_eq!(
            serde_json::from_str::<Json>(&payload).unwrap(),
            serde_json::json!({"meta_changed":true,"request_id":"committed-request"})
        );
        assert_eq!(
            parse_user_payload(r#"{"meta_changed":true}"#),
            Some(UserNotice::MetaChanged { request_id: None })
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), notices.next())
                .await
                .is_err(),
            "PostgreSQL folds equal hints within one transaction; no authority notice is emitted"
        );
    }

    #[tokio::test]
    async fn news_for_a_person_reaches_a_listener_after_the_commit_and_names_them_all() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let mut notices = listen(&db, &[USER_CHANNEL, AUTHZ_CHANNEL]).await.unwrap();
        let people: Vec<Id> = (0..3).map(|_| new_id()).collect();
        let mut tx = test.pool.begin().await.unwrap();
        let event = serde_json::json!({ "what": "approvals" });
        assert!(notify_user_event(&mut tx, &people, &event).await.unwrap());
        notify_epoch(&mut tx, 7).await.unwrap();
        tx.commit().await.unwrap();
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), notices.next())
            .await
            .unwrap()
            .unwrap();
        let Notice::Heard { channel, payload } = first else {
            panic!("an announcement was expected");
        };
        assert_eq!(channel, USER_CHANNEL);
        assert_eq!(
            parse_user_payload(&payload),
            Some(UserNotice::Event {
                users: people,
                event
            })
        );
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), notices.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            second,
            Notice::Heard {
                channel: AUTHZ_CHANNEL.to_owned(),
                payload: "7".to_owned()
            }
        );
    }

    #[tokio::test]
    async fn news_for_more_people_than_one_announcement_holds_goes_in_pieces_and_names_each_once() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let mut notices = listen(&db, &[USER_CHANNEL]).await.unwrap();
        let people: Vec<Id> = (0..400).map(|_| new_id()).collect();
        let mut conn = test.pool.acquire().await.unwrap();
        let event = serde_json::json!({ "what": "approvals" });
        assert!(notify_user_event(&mut conn, &people, &event).await.unwrap());
        let mut named: Vec<Id> = Vec::new();
        while named.len() < people.len() {
            let heard = tokio::time::timeout(std::time::Duration::from_secs(5), notices.next())
                .await
                .unwrap()
                .unwrap();
            let Notice::Heard { payload, .. } = heard else {
                panic!("an announcement was expected");
            };
            assert!(payload.len() <= MAX_PAYLOAD_BYTES);
            let Some(UserNotice::Event { users, .. }) = parse_user_payload(&payload) else {
                panic!("news was expected");
            };
            named.extend(users);
        }
        assert_eq!(named, people);
    }

    #[tokio::test]
    async fn news_that_cannot_fit_in_an_announcement_is_not_sent() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let big = serde_json::json!({ "what": "draft", "text": "x".repeat(MAX_PAYLOAD_BYTES) });
        assert!(
            !notify_user_event(&mut conn, &[new_id()], &big)
                .await
                .unwrap()
        );
        assert!(
            !notify_presence(&mut conn, &"y".repeat(MAX_PAYLOAD_BYTES + 1))
                .await
                .unwrap()
        );
    }
}
