//! Chats, members, messages, quoted items and per-person states.
//!
//! A chat has a sequence of its own, used as a thread's is: every change to a
//! row that belongs to the chat (the chat, its members, its messages) takes
//! the next number of `chats.next_seq` in the transaction that makes it and is
//! stamped with it, and [`notify`] queues the announcement that is delivered
//! when that transaction commits. Whoever takes a number first locks the chat
//! row until the end of its transaction, so numbers are given out in the order
//! the transactions commit. Nothing here decides who may do what: these are
//! the statements that the rules of the chat part are made of.

use crate::queries::notifications::people::Announced;
use crate::{Conn, DbError, sequences};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};
use std::collections::HashMap;
pub mod people;
pub mod posting;

/// The channel on which a chat's changes are announced; its payload is
/// `<chat id>:<sequence>`.
pub const CHAT_CHANNEL: &str = "orch_chat";

/// The kind of a chat that belongs to a node.
pub const KIND_NODE: &str = "node";
/// The kind of a chat between two people.
pub const KIND_DIRECT: &str = "direct";
/// The kind of a chat among several people.
pub const KIND_GROUP: &str = "group";

/// The kind of a message somebody wrote.
pub const MESSAGE: &str = "message";
/// The kind of a message the system wrote about the chat.
pub const SYSTEM: &str = "system";

/// The kind of notification that tells of a new message in a chat.
pub const NOTICE_MESSAGE: &str = "chat.message";
/// The kind of notification that tells a person they were named.
pub const NOTICE_MENTION: &str = "mention";

// ------------------------------------------------------------------ chats

macro_rules! chat_columns {
    () => {
        "id, kind, node_id, org_id, direct_key, name, created_by, next_seq, seq_state, last_message_seq, last_message_at, created_at"
    };
}

/// A chat as it is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ChatRow {
    /// Its identifier.
    pub id: Id,
    /// `node`, `direct` or `group`.
    pub kind: String,
    /// The node it belongs to, for a node's chat.
    pub node_id: Option<Id>,
    /// The root org it belongs to; for a node's chat, its node's.
    pub org_id: Id,
    /// For a direct chat, the two people's identifiers, the smaller first.
    pub direct_key: Option<String>,
    /// The name of a group; none names it after its members.
    pub name: Option<String>,
    /// Who made it.
    pub created_by: Option<Id>,
    /// The next number of its sequence.
    pub next_seq: i64,
    /// The number of the last change to the chat row or to its members.
    pub seq_state: i64,
    /// The number of its newest message: what unread is measured against.
    pub last_message_seq: i64,
    /// When that message was written.
    pub last_message_at: Option<DateTime<Utc>>,
    /// When the chat was made.
    pub created_at: DateTime<Utc>,
}

impl ChatRow {
    /// The highest number of the chat's sequence that was given out.
    #[must_use]
    pub fn seq(&self) -> Seq {
        self.next_seq - 1
    }

    /// Whether it is a direct or a group chat.
    #[must_use]
    pub fn is_people_chat(&self) -> bool {
        self.kind != KIND_NODE
    }
}

/// The chat `id`.
pub async fn chat(conn: &mut Conn, id: Id) -> Result<Option<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        chat_columns!(),
        " FROM chats WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The chats `ids`; one that is not there is not among them.
pub async fn chats(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        chat_columns!(),
        " FROM chats WHERE id = ANY($1)"
    ))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The chat of the node `node`.
pub async fn chat_of_node(conn: &mut Conn, node: Id) -> Result<Option<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        chat_columns!(),
        " FROM chats WHERE node_id = $1"
    ))
    .bind(node)
    .fetch_optional(conn)
    .await?)
}

/// The chat `id`, locked until the end of the transaction: whoever changes
/// what a chat holds reads it this way first, so that two changes of one chat
/// are made one after the other.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        chat_columns!(),
        " FROM chats WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The next number of a chat's sequence, taken while the chat's row is
/// locked until the transaction ends. None where there is no such chat.
pub async fn next_seq(conn: &mut Conn, chat: Id) -> Result<Option<Seq>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE chats SET next_seq = next_seq + 1 WHERE id = $1 RETURNING next_seq - 1",
    )
    .bind(chat)
    .fetch_optional(conn)
    .await?)
}

/// Where each of the chats `ids` stands: the highest number given out. A chat
/// that is not there is not among them.
pub async fn seqs(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Seq)>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_as("SELECT id, next_seq - 1 FROM chats WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Queues the announcement that `chat` has changes up to `seq`. It is
/// delivered when the transaction commits, and not at all if it does not.
pub async fn notify(conn: &mut Conn, chat: Id, seq: Seq) -> Result<(), DbError> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(CHAT_CHANNEL)
        .bind(sequences::payload(chat, seq))
        .execute(conn)
        .await?;
    Ok(())
}

/// The channel on which a person typing in a chat is passed on to every API
/// process, which shows it to the subscribers of the chat it has; its payload
/// is `<chat id>:<user id>`. Typing is not stored and not replayed.
pub const TYPING_CHANNEL: &str = "orch_chat_typing";

/// Queues the word that `user` is typing in `chat`, for every API process.
pub async fn notify_typing(conn: &mut Conn, chat: Id, user: Id) -> Result<(), DbError> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(TYPING_CHANNEL)
        .bind(format!("{chat}:{user}"))
        .execute(conn)
        .await?;
    Ok(())
}

/// The chat and the person an announcement on [`TYPING_CHANNEL`] names.
#[must_use]
pub fn parse_typing(payload: &str) -> Option<(Id, Id)> {
    let (chat, user) = payload.split_once(':')?;
    Some((chat.parse().ok()?, user.parse().ok()?))
}

/// Stamps the chat row, or its members, as changed at `seq`.
pub async fn stamp_state(conn: &mut Conn, chat: Id, seq: Seq) -> Result<(), DbError> {
    sqlx::query("UPDATE chats SET seq_state = $2 WHERE id = $1")
        .bind(chat)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
}

/// A chat that is made.
#[derive(Debug, Clone, Copy)]
pub struct NewChat<'a> {
    /// Its identifier.
    pub id: Id,
    /// `direct` or `group`.
    pub kind: &'a str,
    /// The root org it belongs to.
    pub org_id: Id,
    /// For a direct chat, the two people's identifiers, the smaller first.
    pub direct_key: Option<&'a str>,
    /// The name of a group.
    pub name: Option<&'a str>,
    /// Who makes it.
    pub created_by: Id,
    /// When.
    pub at: DateTime<Utc>,
}

/// Makes a direct or a group chat. None where a direct chat of those two
/// people in that org exists already.
pub async fn insert_chat(conn: &mut Conn, new: &NewChat<'_>) -> Result<Option<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO chats (id, kind, org_id, direct_key, name, created_by, created_at) ",
        "VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (org_id, direct_key) DO NOTHING ",
        "RETURNING ",
        chat_columns!()
    ))
    .bind(new.id)
    .bind(new.kind)
    .bind(new.org_id)
    .bind(new.direct_key)
    .bind(new.name)
    .bind(new.created_by)
    .bind(new.at)
    .fetch_optional(conn)
    .await?)
}

/// The direct chat of two people in a root org, by their key.
pub async fn direct_chat(conn: &mut Conn, org: Id, key: &str) -> Result<Option<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        chat_columns!(),
        " FROM chats WHERE org_id = $1 AND direct_key = $2"
    ))
    .bind(org)
    .bind(key)
    .fetch_optional(conn)
    .await?)
}

/// Gives a group a name, or names it after its members again.
pub async fn rename(conn: &mut Conn, chat: Id, name: Option<&str>) -> Result<(), DbError> {
    sqlx::query("UPDATE chats SET name = $2 WHERE id = $1 AND kind = 'group'")
        .bind(chat)
        .bind(name)
        .execute(conn)
        .await?;
    Ok(())
}

/// Notes that a message was written: the chat's newest message moves to it.
pub async fn note_message(
    conn: &mut Conn,
    chat: Id,
    seq: Seq,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE chats SET last_message_seq = $2, last_message_at = $3 WHERE id = $1")
        .bind(chat)
        .bind(seq)
        .bind(at)
        .execute(&mut *conn)
        .await?;
    touch_place_activity(conn, chat, at).await?;
    Ok(())
}

async fn touch_place_activity(conn: &mut Conn, chat: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    let place: Option<Id> =
        sqlx::query_scalar("SELECT COALESCE(node_id,org_id) FROM chats WHERE id=$1")
            .bind(chat)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(place) = place {
        crate::queries::hierarchy::touch_activity(conn, place, at).await?;
    }
    Ok(())
}

// ------------------------------------------------------------------ members

/// A person in a direct or a group chat, as it is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct MemberRow {
    /// The chat.
    pub chat_id: Id,
    /// The person.
    pub user_id: Id,
    /// Who added them.
    pub added_by: Option<Id>,
    /// When they joined.
    pub joined_at: DateTime<Utc>,
    /// When they left or were removed; none while they are in.
    pub left_at: Option<DateTime<Utc>>,
    /// Whether they came in through Share without being a member of the org.
    pub guest: bool,
    /// The number of the chat's sequence at which this row last changed.
    pub seq_updated: i64,
    /// When a timed membership ends; null is the ordinary permanent membership.
    #[sqlx(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

impl MemberRow {
    /// Whether they are in the chat now.
    #[must_use]
    pub fn is_in(&self) -> bool {
        self.left_at.is_none()
    }

    /// Whether the membership authorizes participation at the caller's current clock time.
    #[must_use]
    pub fn active_at(&self, at: DateTime<Utc>) -> bool {
        self.left_at.is_none() && self.expires_at.is_none_or(|end| end > at)
    }
}

/// A current participation snapshot, even before the expiry reaper has recorded each removal.
#[must_use]
pub fn effective_members(members: &[MemberRow], at: DateTime<Utc>) -> Vec<MemberRow> {
    members
        .iter()
        .cloned()
        .map(|mut member| {
            if member.left_at.is_none() && member.expires_at.is_some_and(|end| end <= at) {
                member.left_at = member.expires_at;
            }
            member
        })
        .collect()
}

/// Everyone who ever was in the chat, the first to join first.
pub async fn members(conn: &mut Conn, chat: Id) -> Result<Vec<MemberRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT chat_id, user_id, added_by, joined_at, left_at, guest, seq_updated, expires_at
           FROM chat_members WHERE chat_id = $1 ORDER BY joined_at, user_id",
    )
    .bind(chat)
    .fetch_all(conn)
    .await?)
}

/// A compact label's active peers; expiry and non-expiring entrances each stop before the final merge.
pub async fn members_for_label(
    conn: &mut Conn,
    chat: Id,
    except: Id,
    at: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<MemberRow>, DbError> {
    Ok(sqlx::query_as(r#"
      SELECT * FROM (
       (SELECT chat_id,user_id,added_by,joined_at,left_at,guest,seq_updated,expires_at
        FROM chat_members WHERE chat_id=$1 AND user_id<>$2 AND left_at IS NULL AND expires_at IS NULL
        ORDER BY expires_at,user_id LIMIT $4)
       UNION ALL
       (SELECT chat_id,user_id,added_by,joined_at,left_at,guest,seq_updated,expires_at
        FROM chat_members WHERE chat_id=$1 AND user_id<>$2 AND left_at IS NULL AND expires_at>$3
        ORDER BY expires_at,user_id LIMIT $4)
      ) peers ORDER BY joined_at,user_id LIMIT $4
    "#).bind(chat).bind(except).bind(at).bind(limit).fetch_all(conn).await?)
}

/// A bounded batch of overdue group memberships, oldest first; callers lock and recheck each chat.
pub async fn expired_group_memberships(
    conn: &mut Conn,
    at: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.chat_id, m.user_id FROM chat_members m JOIN chats c ON c.id = m.chat_id
         WHERE c.kind = 'group' AND m.left_at IS NULL AND m.expires_at <= $1
         ORDER BY m.expires_at, m.chat_id, m.user_id LIMIT $2",
    )
    .bind(at)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Everyone who ever was in any of the chats `chats`.
pub async fn members_in(conn: &mut Conn, chats: &[Id]) -> Result<Vec<MemberRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT chat_id, user_id, added_by, joined_at, left_at, guest, seq_updated, expires_at
           FROM chat_members WHERE chat_id = ANY($1) ORDER BY chat_id, joined_at, user_id",
    )
    .bind(chats)
    .fetch_all(conn)
    .await?)
}

/// A person who comes into a chat, or comes back.
#[derive(Debug, Clone, Copy)]
pub struct NewMember {
    /// An optional end for a group membership.
    pub expires_at: Option<DateTime<Utc>>,
    /// The chat.
    pub chat_id: Id,
    /// The person.
    pub user_id: Id,
    /// Who adds them.
    pub added_by: Option<Id>,
    /// Whether they are not a member of the org.
    pub guest: bool,
    /// The number of the chat's sequence of this change.
    pub seq: Seq,
    /// When.
    pub at: DateTime<Utc>,
}

/// Puts a person in a chat. One who was in and left comes back with the
/// moment they came back.
pub async fn add_member(conn: &mut Conn, new: &NewMember) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO chat_members (chat_id, user_id, added_by, joined_at, guest, seq_updated, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (chat_id, user_id) DO UPDATE
            SET left_at = NULL, joined_at = EXCLUDED.joined_at, added_by = EXCLUDED.added_by,
                guest = EXCLUDED.guest, seq_updated = EXCLUDED.seq_updated, expires_at=EXCLUDED.expires_at",
    )
    .bind(new.chat_id)
    .bind(new.user_id)
    .bind(new.added_by)
    .bind(new.at)
    .bind(new.guest)
    .bind(new.seq)
    .bind(new.expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Takes a person out of a chat. Says whether they were in it.
pub async fn remove_member(
    conn: &mut Conn,
    chat: Id,
    user: Id,
    seq: Seq,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE chat_members SET left_at = $3, seq_updated = $4
          WHERE chat_id = $1 AND user_id = $2 AND left_at IS NULL",
    )
    .bind(chat)
    .bind(user)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The direct and group chats a person is in now.
pub async fn chats_of_member(conn: &mut Conn, user: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT chat_id FROM chat_members WHERE user_id = $1 AND left_at IS NULL",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

// ------------------------------------------------------------------ people

/// What the chat part needs to know of a person to say who may be in a
/// chat with whom.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Standing {
    /// The person.
    pub id: Id,
    /// `human` or `service`.
    pub kind: String,
    /// The name they are mentioned by.
    pub username: String,
    /// Whether the system suspended them altogether.
    pub suspended: bool,
    /// Whether they are a member of the root org.
    pub member: bool,
    /// Whether the root org suspended them.
    pub suspended_in_org: bool,
    /// The org they belong to in that tree, with the orgs it lies in: the
    /// nodes from the root org down to their domain. Empty for somebody who
    /// is no member.
    pub domain_path: Vec<Id>,
}

impl Standing {
    /// Whether they are here to be written to: a member, not suspended.
    #[must_use]
    pub fn is_present(&self) -> bool {
        self.member && !self.suspended && !self.suspended_in_org
    }
}

/// The people `users` as they stand towards the root org `root`; one that is
/// not there is not among them.
pub async fn standing(conn: &mut Conn, root: Id, users: &[Id]) -> Result<Vec<Standing>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.kind, u.username, (u.status = 'suspended') AS suspended,
                (m.user_id IS NOT NULL) AS member,
                EXISTS (SELECT 1 FROM org_suspensions s WHERE s.org_id = $1 AND s.user_id = u.id) AS suspended_in_org,
                COALESCE(d.path, ARRAY[]::uuid[]) AS domain_path
           FROM users u
           LEFT JOIN org_members m ON m.org_id = $1 AND m.user_id = u.id
           LEFT JOIN nodes d ON d.id = m.domain_id
          WHERE u.id = ANY($2)",
    )
    .bind(root)
    .bind(users)
    .fetch_all(conn)
    .await?)
}

/// The people whose username is among `names`, as they stand towards the
/// root org `root`. Names are matched without regard to case.
pub async fn standing_by_username(
    conn: &mut Conn,
    root: Id,
    names: &[String],
) -> Result<Vec<Standing>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.kind, u.username, (u.status = 'suspended') AS suspended,
                (m.user_id IS NOT NULL) AS member,
                EXISTS (SELECT 1 FROM org_suspensions s WHERE s.org_id = $1 AND s.user_id = u.id) AS suspended_in_org,
                COALESCE(d.path, ARRAY[]::uuid[]) AS domain_path
           FROM users u
           LEFT JOIN org_members m ON m.org_id = $1 AND m.user_id = u.id
           LEFT JOIN nodes d ON d.id = m.domain_id
          WHERE lower(u.username) = ANY($2)",
    )
    .bind(root)
    .bind(
        names
            .iter()
            .map(|name| name.to_lowercase())
            .collect::<Vec<_>>(),
    )
    .fetch_all(conn)
    .await?)
}

// ------------------------------------------------------------------ messages

macro_rules! message_columns {
    () => {
        "id, chat_id, kind, author_id, author_thread_id, author_agent, content, reply_to_id, mentions, mentions_all, mention_targets, asked_agents, reactions, client_id, search_text, edited_at, pinned_at, pinned_by, deleted_at, deleted_by, seq_created, seq_updated, created_at"
    };
}

/// A message as it is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct MessageRow {
    /// The stable explicit targets, with actual accepted recipients kept in mentions.
    pub mention_targets: Json,
    /// The ordinary thread/input identities accepted with this post.
    pub asked_agents: Json,
    /// Its identifier.
    pub id: Id,
    /// The chat.
    pub chat_id: Id,
    /// `message` or `system`.
    pub kind: String,
    /// The person who wrote it; none for a system message and for an agent's.
    pub author_id: Option<Id>,
    /// The thread of the agent that wrote it.
    pub author_thread_id: Option<Id>,
    /// That agent's name, kept when the thread is gone.
    pub author_agent: Option<String>,
    /// The parts it holds; for a system message, its event.
    pub content: Json,
    /// The message it replies to.
    pub reply_to_id: Option<Id>,
    /// The people it names with `@`.
    pub mentions: Vec<Id>,
    /// Whether it says `@all`.
    pub mentions_all: bool,
    /// Emoji to the people who reacted with it, in the order they did.
    pub reactions: Json,
    /// The value its author sent to make sending it again harmless.
    pub client_id: Option<String>,
    /// Its text and the names of what it carries; none when it is not searched.
    pub search_text: Option<String>,
    /// When it was last edited.
    pub edited_at: Option<DateTime<Utc>>,
    /// When it was pinned.
    pub pinned_at: Option<DateTime<Utc>>,
    /// Who pinned it.
    pub pinned_by: Option<Id>,
    /// When it was deleted: the row stays, with its content emptied.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Who deleted it.
    pub deleted_by: Option<Id>,
    /// The number of the chat's sequence at which it was written.
    pub seq_created: i64,
    /// The number at which it last changed.
    pub seq_updated: i64,
    /// When it was written.
    pub created_at: DateTime<Utc>,
}

impl MessageRow {
    /// Whether it was deleted.
    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// A message that is written.
#[derive(Debug, Clone, Copy)]
pub struct NewMessage<'a> {
    /// Its identifier.
    pub id: Id,
    /// The chat.
    pub chat_id: Id,
    /// `message` or `system`.
    pub kind: &'a str,
    /// The person who writes it.
    pub author_id: Option<Id>,
    /// The thread of the agent that writes it.
    pub author_thread_id: Option<Id>,
    /// That agent's name.
    pub author_agent: Option<&'a str>,
    /// Its parts, or the event of a system message.
    pub content: &'a Json,
    /// The message it replies to.
    pub reply_to_id: Option<Id>,
    /// The people it names.
    pub mentions: &'a [Id],
    /// Whether it says `@all`.
    pub mentions_all: bool,
    /// The author's value that makes sending it again harmless.
    pub client_id: Option<&'a str>,
    /// What a search finds it by.
    pub search_text: Option<&'a str>,
    /// The number of the chat's sequence of this write.
    pub seq: Seq,
    /// When.
    pub at: DateTime<Utc>,
}

/// Writes a message.
pub async fn insert_message(conn: &mut Conn, new: &NewMessage<'_>) -> Result<MessageRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO chat_messages (id, chat_id, kind, author_id, author_thread_id, author_agent, content, ",
        "reply_to_id, mentions, mentions_all, client_id, search_text, seq_created, seq_updated, created_at) ",
        "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13, $14) RETURNING ",
        message_columns!()
    ))
    .bind(new.id)
    .bind(new.chat_id)
    .bind(new.kind)
    .bind(new.author_id)
    .bind(new.author_thread_id)
    .bind(new.author_agent)
    .bind(new.content)
    .bind(new.reply_to_id)
    .bind(new.mentions)
    .bind(new.mentions_all)
    .bind(new.client_id)
    .bind(new.search_text)
    .bind(new.seq)
    .bind(new.at)
    .fetch_one(conn)
    .await?)
}

/// The message `id` of the chat `chat`.
pub async fn message(conn: &mut Conn, chat: Id, id: Id) -> Result<Option<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 AND id = $2"
    ))
    .bind(chat)
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The newest ordinary message of each already authorized chat, in one query.
/// Deleted messages stay the newest; their projection carries an empty excerpt.
pub async fn last_messages(conn: &mut Conn, chats: &[Id]) -> Result<Vec<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT DISTINCT ON (chat_id) ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = ANY($1) AND kind = 'message'
          ORDER BY chat_id, seq_created DESC, id DESC"
    ))
    .bind(chats)
    .fetch_all(conn)
    .await?)
}

/// The messages `ids` of the chat `chat`; one that is not there is not among
/// them.
pub async fn messages_by_ids(
    conn: &mut Conn,
    chat: Id,
    ids: &[Id],
) -> Result<Vec<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 AND id = ANY($2) ORDER BY seq_created"
    ))
    .bind(chat)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The message an author already sent under a value of `client_id`.
pub async fn message_by_client(
    conn: &mut Conn,
    chat: Id,
    author: Id,
    client_id: &str,
) -> Result<Option<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 AND author_id = $2 AND client_id = $3"
    ))
    .bind(chat)
    .bind(author)
    .bind(client_id)
    .fetch_optional(conn)
    .await?)
}

/// What an edit writes.
#[derive(Debug, Clone, Copy)]
pub struct Edit<'a> {
    /// The message.
    pub id: Id,
    /// Its new parts.
    pub content: &'a Json,
    /// The people it names now.
    pub mentions: &'a [Id],
    /// Whether it says `@all` now.
    pub mentions_all: bool,
    /// What a search finds it by now.
    pub search_text: Option<&'a str>,
    /// The number of the chat's sequence of this change.
    pub seq: Seq,
    /// When.
    pub at: DateTime<Utc>,
}

/// Replaces what a message says.
pub async fn edit_message(conn: &mut Conn, edit: &Edit<'_>) -> Result<MessageRow, DbError> {
    let message: MessageRow = sqlx::query_as(concat!(
        "UPDATE chat_messages SET content = $2, mentions = $3, mentions_all = $4, search_text = $5, ",
        "edited_at = $6, seq_updated = $7 WHERE id = $1 RETURNING ",
        message_columns!()
    ))
    .bind(edit.id)
    .bind(edit.content)
    .bind(edit.mentions)
    .bind(edit.mentions_all)
    .bind(edit.search_text)
    .bind(edit.at)
    .bind(edit.seq)
    .fetch_one(&mut *conn)
    .await?;
    touch_place_activity(conn, message.chat_id, edit.at).await?;
    Ok(message)
}

/// Deletes a message the way a chat does: the row stays, with what it said
/// emptied, so that everything changed after a number still describes every
/// change.
pub async fn delete_message(
    conn: &mut Conn,
    id: Id,
    by: Id,
    seq: Seq,
    at: DateTime<Utc>,
) -> Result<MessageRow, DbError> {
    let message: MessageRow = sqlx::query_as(concat!(
        "UPDATE chat_messages SET content = '[]'::jsonb, mentions = '{}', mentions_all = false, ",
        "mention_targets = '[]'::jsonb, asked_agents = '[]'::jsonb, reactions = '{}'::jsonb, search_text = NULL, pinned_at = NULL, pinned_by = NULL, ",
        "deleted_at = $3, deleted_by = $2, seq_updated = $4 WHERE id = $1 RETURNING ",
        message_columns!()
    ))
    .bind(id)
    .bind(by)
    .bind(at)
    .bind(seq)
    .fetch_one(&mut *conn)
    .await?;
    touch_place_activity(conn, message.chat_id, at).await?;
    Ok(message)
}

/// Replaces the reactions of a message.
pub async fn set_reactions(
    conn: &mut Conn,
    id: Id,
    reactions: &Json,
    seq: Seq,
) -> Result<MessageRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE chat_messages SET reactions = $2, seq_updated = $3 WHERE id = $1 RETURNING ",
        message_columns!()
    ))
    .bind(id)
    .bind(reactions)
    .bind(seq)
    .fetch_one(conn)
    .await?)
}

/// Pins a message, or takes the pin off (`by` is none).
pub async fn set_pin(
    conn: &mut Conn,
    id: Id,
    by: Option<Id>,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<MessageRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE chat_messages SET pinned_at = CASE WHEN $2::uuid IS NULL THEN NULL ELSE $3 END, ",
        "pinned_by = $2, seq_updated = $4 WHERE id = $1 RETURNING ",
        message_columns!()
    ))
    .bind(id)
    .bind(by)
    .bind(at)
    .bind(seq)
    .fetch_one(conn)
    .await?)
}

/// The newest `limit` messages, oldest first.
pub async fn latest(conn: &mut Conn, chat: Id, limit: i64) -> Result<Vec<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT * FROM (SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 ORDER BY seq_created DESC LIMIT $2) newest ORDER BY seq_created"
    ))
    .bind(chat)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The newest `limit` messages written before the number `before`, oldest
/// first.
pub async fn before(
    conn: &mut Conn,
    chat: Id,
    before: Seq,
    limit: i64,
) -> Result<Vec<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT * FROM (SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 AND seq_created < $2 ORDER BY seq_created DESC LIMIT $3) older ORDER BY seq_created"
    ))
    .bind(chat)
    .bind(before)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The first `limit` messages written after the number `after`, oldest first.
pub async fn after(
    conn: &mut Conn,
    chat: Id,
    after: Seq,
    limit: i64,
) -> Result<Vec<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 AND seq_created > $2 ORDER BY seq_created LIMIT $3"
    ))
    .bind(chat)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Whether the chat has a message written before the number `before`.
pub async fn any_before(conn: &mut Conn, chat: Id, before: Seq) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM chat_messages WHERE chat_id = $1 AND seq_created < $2)",
    )
    .bind(chat)
    .bind(before)
    .fetch_one(conn)
    .await?)
}

/// Whether the chat has a message written after the number `after`.
pub async fn any_after(conn: &mut Conn, chat: Id, after: Seq) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM chat_messages WHERE chat_id = $1 AND seq_created > $2)",
    )
    .bind(chat)
    .bind(after)
    .fetch_one(conn)
    .await?)
}

/// The messages that are pinned, the newest `limit` of them, oldest first.
pub async fn pinned(conn: &mut Conn, chat: Id, limit: i64) -> Result<Vec<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT * FROM (SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 AND pinned_at IS NOT NULL AND deleted_at IS NULL ",
        "ORDER BY seq_created DESC LIMIT $2) pins ORDER BY seq_created"
    ))
    .bind(chat)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The messages that quote an item, the newest `limit` of them, oldest first.
pub async fn quoting(
    conn: &mut Conn,
    chat: Id,
    item: Id,
    limit: i64,
) -> Result<Vec<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT * FROM (SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 AND deleted_at IS NULL ",
        "AND id IN (SELECT message_id FROM chat_item_refs WHERE item_id = $2) ",
        "ORDER BY seq_created DESC LIMIT $3) quotes ORDER BY seq_created"
    ))
    .bind(chat)
    .bind(item)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The messages that changed after the number `after`, up to the number `upto`
/// (or all of them, with none), in the order they changed, at most `limit`.
pub async fn changed(
    conn: &mut Conn,
    chat: Id,
    after: Seq,
    upto: Option<Seq>,
    limit: i64,
) -> Result<Vec<MessageRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        message_columns!(),
        " FROM chat_messages WHERE chat_id = $1 AND seq_updated > $2 AND ($3::bigint IS NULL OR seq_updated <= $3) ",
        "ORDER BY seq_updated, id LIMIT $4"
    ))
    .bind(chat)
    .bind(after)
    .bind(upto)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// How many messages changed after the number `after`.
pub async fn count_changed(conn: &mut Conn, chat: Id, after: Seq) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE chat_id = $1 AND seq_updated > $2",
    )
    .bind(chat)
    .bind(after)
    .fetch_one(conn)
    .await?)
}

// ------------------------------------------------------------------ quoted items

/// Which of the items `items` belong to the thread `thread`.
pub async fn items_of_thread(
    conn: &mut Conn,
    thread: Id,
    items: &[Id],
) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM items WHERE thread_id = $1 AND id = ANY($2)")
            .bind(thread)
            .bind(items)
            .fetch_all(conn)
            .await?,
    )
}

/// The kind of each of the items `items` that belongs to the thread `thread`.
pub async fn items_with_kinds(
    conn: &mut Conn,
    thread: Id,
    items: &[Id],
) -> Result<Vec<(Id, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, kind FROM items WHERE thread_id = $1 AND id = ANY($2)")
            .bind(thread)
            .bind(items)
            .fetch_all(conn)
            .await?,
    )
}

/// Records that a message quotes items.
pub async fn add_item_refs(conn: &mut Conn, message: Id, items: &[Id]) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO chat_item_refs (message_id, item_id)
         SELECT $1, unnest($2::uuid[]) ON CONFLICT DO NOTHING",
    )
    .bind(message)
    .bind(items)
    .execute(conn)
    .await?;
    Ok(())
}

/// The items a message quotes.
pub async fn item_refs(conn: &mut Conn, message: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT item_id FROM chat_item_refs WHERE message_id = $1 ORDER BY item_id",
    )
    .bind(message)
    .fetch_all(conn)
    .await?)
}

/// Records that a message quotes some items no longer.
pub async fn drop_item_refs(conn: &mut Conn, message: Id, items: &[Id]) -> Result<(), DbError> {
    sqlx::query("DELETE FROM chat_item_refs WHERE message_id = $1 AND item_id = ANY($2)")
        .bind(message)
        .bind(items)
        .execute(conn)
        .await?;
    Ok(())
}

/// Counts one more (`delta` 1) or one fewer (`-1`) quote on each of the items
/// and stamps them as changed at `seq` of their thread's sequence, at `at`.
pub async fn count_item_quotes(
    conn: &mut Conn,
    items: &[Id],
    delta: i32,
    seq: Seq,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE items SET chat_refs = GREATEST(chat_refs + $2, 0), seq_updated = $3, updated_at = $4
          WHERE id = ANY($1)",
    )
    .bind(items)
    .bind(delta)
    .bind(seq)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

// ------------------------------------------------------------------ one person's state

macro_rules! state_columns {
    () => {
        "user_id, chat_id, level, engaged, last_read_seq, read_revision, mentions_unread, draft, draft_reply_to, hidden, updated_at"
    };
}

pub mod following;

/// One person's own state in one chat, as it is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct StateRow {
    /// The fence advanced by deliberate read and unread choices.
    pub read_revision: i64,
    /// The person.
    pub user_id: Id,
    /// The chat.
    pub chat_id: Id,
    /// What they set with the bell; none when they have not.
    pub level: Option<String>,
    /// Whether they posted or were mentioned here.
    pub engaged: bool,
    /// How far they have read; it only increases.
    pub last_read_seq: i64,
    /// How many mentions of them came after that.
    pub mentions_unread: i32,
    /// What they began to write and did not send.
    pub draft: Option<Json>,
    /// The message that draft replies to.
    pub draft_reply_to: Option<Id>,
    /// Whether the chat is off their list until its next message.
    pub hidden: bool,
    /// When they last changed it themselves.
    pub updated_at: DateTime<Utc>,
}

/// The person's state in one chat.
pub async fn state(conn: &mut Conn, user: Id, chat: Id) -> Result<Option<StateRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        state_columns!(),
        " FROM chat_states WHERE user_id = $1 AND chat_id = $2"
    ))
    .bind(user)
    .bind(chat)
    .fetch_optional(conn)
    .await?)
}

/// The person's state in some chats.
pub async fn states_in(conn: &mut Conn, user: Id, chats: &[Id]) -> Result<Vec<StateRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        state_columns!(),
        " FROM chat_states WHERE user_id = $1 AND chat_id = ANY($2)"
    ))
    .bind(user)
    .bind(chats)
    .fetch_all(conn)
    .await?)
}

/// The person's state in every chat they have one in.
pub async fn states_of(conn: &mut Conn, user: Id) -> Result<Vec<StateRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        state_columns!(),
        " FROM chat_states WHERE user_id = $1 ORDER BY chat_id"
    ))
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// Marks people as having taken part in a chat: they follow it from now on,
/// at their default level.
pub async fn engage(
    conn: &mut Conn,
    chat: Id,
    users: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO chat_states (user_id, chat_id, engaged, updated_at)
         SELECT u, $1, true, $3 FROM unnest($2::uuid[]) AS u
         ON CONFLICT (user_id, chat_id) DO UPDATE SET engaged = true",
    )
    .bind(chat)
    .bind(users)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Counts one more mention for each of the people, and marks them as having
/// taken part.
pub async fn raise_mentions(
    conn: &mut Conn,
    chat: Id,
    users: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO chat_states (user_id, chat_id, engaged, mentions_unread, updated_at)
         SELECT u, $1, true, 1, $3 FROM unnest($2::uuid[]) AS u
         ON CONFLICT (user_id, chat_id) DO UPDATE
            SET engaged = true, mentions_unread = chat_states.mentions_unread + 1",
    )
    .bind(chat)
    .bind(users)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Sets what the person chose with the bell, or takes the choice back.
pub async fn set_level(
    conn: &mut Conn,
    user: Id,
    chat: Id,
    level: Option<&str>,
    at: DateTime<Utc>,
) -> Result<StateRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO chat_states (user_id, chat_id, level, updated_at) VALUES ($1, $2, $3, $4) ",
        "ON CONFLICT (user_id, chat_id) DO UPDATE SET level = EXCLUDED.level, updated_at = EXCLUDED.updated_at ",
        "RETURNING ",
        state_columns!()
    ))
    .bind(user)
    .bind(chat)
    .bind(level)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Takes a chat off the person's list, or puts it back.
pub async fn set_hidden(
    conn: &mut Conn,
    user: Id,
    chat: Id,
    hidden: bool,
    at: DateTime<Utc>,
) -> Result<StateRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO chat_states (user_id, chat_id, hidden, updated_at) VALUES ($1, $2, $3, $4) ",
        "ON CONFLICT (user_id, chat_id) DO UPDATE SET hidden = EXCLUDED.hidden, updated_at = EXCLUDED.updated_at ",
        "RETURNING ",
        state_columns!()
    ))
    .bind(user)
    .bind(chat)
    .bind(hidden)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Puts back on the lists of everyone who took a chat off theirs: a new
/// message brings it back. The people it was put back for.
pub async fn unhide_all(conn: &mut Conn, chat: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE chat_states SET hidden = false WHERE chat_id = $1 AND hidden RETURNING user_id",
    )
    .bind(chat)
    .fetch_all(conn)
    .await?)
}

/// Moves the person's read position forward to `seq` and clears the count of
/// their unread mentions. It never moves back.
pub async fn mark_read(conn: &mut Conn, user: Id, chat: Id, seq: Seq) -> Result<StateRow, DbError> {
    let current = following::locked(conn, user, chat).await?;
    following::put_read(
        conn,
        user,
        chat,
        seq.max(current.last_read_seq),
        current.read_revision.saturating_add(1),
    )
    .await
}

/// The draft a person has in a chat, with when it was saved.
pub async fn draft(
    conn: &mut Conn,
    user: Id,
    chat: Id,
) -> Result<Option<(Json, Option<Id>, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT draft, draft_reply_to, updated_at FROM chat_states
          WHERE user_id = $1 AND chat_id = $2 AND draft IS NOT NULL",
    )
    .bind(user)
    .bind(chat)
    .fetch_optional(conn)
    .await?)
}

/// Keeps what a person has begun to write in a chat.
pub async fn save_draft(
    conn: &mut Conn,
    user: Id,
    chat: Id,
    parts: &Json,
    reply_to: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO chat_states (user_id, chat_id, draft, draft_reply_to, updated_at) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (user_id, chat_id) DO UPDATE
            SET draft = EXCLUDED.draft, draft_reply_to = EXCLUDED.draft_reply_to, updated_at = EXCLUDED.updated_at",
    )
    .bind(user)
    .bind(chat)
    .bind(parts)
    .bind(reply_to)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Throws a person's draft away. Says whether they had one.
pub async fn delete_draft(conn: &mut Conn, user: Id, chat: Id) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE chat_states SET draft = NULL, draft_reply_to = NULL
          WHERE user_id = $1 AND chat_id = $2 AND draft IS NOT NULL",
    )
    .bind(user)
    .bind(chat)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

// ------------------------------------------------------------------ who follows

/// The level a person has in a chat, in the order the rules give: what they
/// chose with the bell; else, when they took part in it (or are in the direct
/// or group chat), their default for that kind of chat; else `off`.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Follower {
    /// The person.
    pub user_id: Id,
    /// Their level: `all`, `mentions` or `off`.
    pub level: String,
}

/// What a person who has no notification preferences of their own gets for
/// the chat of a node.
pub const DEFAULT_NODE_LEVEL: &str = "mentions";
/// What a person who has no notification preferences of their own gets for a
/// direct or group chat.
pub const DEFAULT_PEOPLE_LEVEL: &str = "all";

/// The stored node-chat follow choice used by both follower lists and a frame's participation facts.
/// Aliases are source constants; no user text becomes SQL.
pub(crate) fn node_follow_level_sql(state: &str, prefs: &str) -> String {
    format!(
        "COALESCE({state}.level, CASE WHEN {state}.engaged THEN COALESCE({prefs}.chat_node_level, '{DEFAULT_NODE_LEVEL}') ELSE 'off' END)"
    )
}

/// The corresponding direct/group follow choice, after checking active membership.
pub(crate) fn people_follow_level_sql(state: &str, prefs: &str) -> String {
    format!("COALESCE({state}.level, {prefs}.chat_direct_level, '{DEFAULT_PEOPLE_LEVEL}')")
}

/// Everyone whose level in a node's chat is not `off`.
pub async fn node_chat_followers(conn: &mut Conn, chat: Id) -> Result<Vec<Follower>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT user_id, level FROM (
            SELECT s.user_id,
                   {} AS level
              FROM chat_states s LEFT JOIN notification_prefs p ON p.user_id = s.user_id
             WHERE s.chat_id = $1
         ) levels WHERE level <> 'off'",
        node_follow_level_sql("s", "p"),
    )))
    .bind(chat)
    .fetch_all(conn)
    .await?)
}

/// Everyone in a direct or group chat whose level is not `off`.
pub async fn people_chat_followers(conn: &mut Conn, chat: Id) -> Result<Vec<Follower>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT user_id, level FROM (
            SELECT m.user_id, {} AS level
              FROM chat_members m
              LEFT JOIN chat_states s ON s.chat_id = m.chat_id AND s.user_id = m.user_id
              LEFT JOIN notification_prefs p ON p.user_id = m.user_id
             WHERE m.chat_id = $1 AND m.left_at IS NULL
         ) levels WHERE level <> 'off'",
        people_follow_level_sql("s", "p"),
    )))
    .bind(chat)
    .fetch_all(conn)
    .await?)
}

/// The defaults the people `users` chose for the chats of nodes and for
/// direct and group chats, by person. A person who chose none is not among
/// them.
pub async fn default_levels(
    conn: &mut Conn,
    users: &[Id],
) -> Result<HashMap<Id, (String, String)>, DbError> {
    let rows: Vec<(Id, String, String)> = sqlx::query_as(
        "SELECT user_id, chat_node_level, chat_direct_level FROM notification_prefs WHERE user_id = ANY($1)",
    )
    .bind(users)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user, node, direct)| (user, (node, direct)))
        .collect())
}

// ------------------------------------------------------------------ lists of a root org

/// Where a page of a list of chats begins: after the chat with this moment of
/// activity and identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct After {
    /// The moment of the activity of the last chat of the page before.
    pub at: DateTime<Utc>,
    /// Its identifier.
    pub id: Id,
}

impl ChatRow {
    /// The moment a chat was last active: its newest message, else its making.
    #[must_use]
    pub fn active_at(&self) -> DateTime<Utc> {
        self.last_message_at.unwrap_or(self.created_at)
    }
}

/// The direct and group chats a person is in, in a root org, that are not off
/// their list, the most recently active first. A person who is no member of
/// the org (`guest_only`) is a guest of those chats alone where they are one.
pub async fn mine(
    conn: &mut Conn,
    user: Id,
    org: Id,
    guest_only: bool,
    after: Option<After>,
    limit: i64,
) -> Result<Vec<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT c.id, c.kind, c.node_id, c.org_id, c.direct_key, c.name, c.created_by, c.next_seq, c.seq_state, ",
        "c.last_message_seq, c.last_message_at, c.created_at ",
        "FROM chats c ",
        "JOIN chat_members m ON m.chat_id = c.id AND m.user_id = $1 AND m.left_at IS NULL ",
        "LEFT JOIN chat_states s ON s.chat_id = c.id AND s.user_id = $1 ",
        "WHERE c.org_id = $2 AND c.kind <> 'node' AND NOT COALESCE(s.hidden, false) ",
        "AND (NOT $3 OR m.guest) ",
        "AND ($4::timestamptz IS NULL OR (COALESCE(c.last_message_at, c.created_at), c.id) < ($4, $5)) ",
        "ORDER BY COALESCE(c.last_message_at, c.created_at) DESC, c.id DESC LIMIT $6"
    ))
    .bind(user)
    .bind(org)
    .bind(guest_only)
    .bind(after.map(|after| after.at))
    .bind(after.map(|after| after.id))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The direct and group chats of a root org that the person is not in and that
/// all of whose people belong to the org `of` (every one of them, for the root
/// org itself), the most recently active first. Groups with nobody in them
/// belong to the root org alone.
pub async fn others(
    conn: &mut Conn,
    user: Id,
    org: Id,
    of: Id,
    after: Option<After>,
    limit: i64,
) -> Result<Vec<ChatRow>, DbError> {
    others_among(conn, user, org, std::slice::from_ref(&of), after, limit).await
}

/// The union of readable departments, filtered once before paging. An empty scope
/// yields an empty list; a root scope includes every people's chat of that root.
pub async fn others_among(
    conn: &mut Conn,
    user: Id,
    org: Id,
    of: &[Id],
    after: Option<After>,
    limit: i64,
) -> Result<Vec<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT c.id, c.kind, c.node_id, c.org_id, c.direct_key, c.name, c.created_by, c.next_seq, c.seq_state, ",
        "c.last_message_seq, c.last_message_at, c.created_at ",
        "FROM chats c ",
        "WHERE c.org_id = $2 AND c.kind <> 'node' ",
        "AND NOT EXISTS (SELECT 1 FROM chat_members me WHERE me.chat_id = c.id AND me.user_id = $1 AND me.left_at IS NULL) ",
        "AND ($2 = ANY($3::uuid[]) OR EXISTS (SELECT 1 FROM unnest($3::uuid[]) scope(org) WHERE (",
        "  EXISTS (SELECT 1 FROM chat_members a WHERE a.chat_id = c.id AND a.left_at IS NULL) ",
        "  AND NOT EXISTS (SELECT 1 FROM chat_members m WHERE m.chat_id = c.id AND m.left_at IS NULL ",
        "    AND NOT EXISTS (SELECT 1 FROM org_members om JOIN nodes d ON d.id = om.domain_id ",
        "                     WHERE om.org_id = $2 AND om.user_id = m.user_id AND scope.org = ANY(d.path)))))) ",
        "AND ($4::timestamptz IS NULL OR (COALESCE(c.last_message_at, c.created_at), c.id) < ($4, $5)) ",
        "ORDER BY COALESCE(c.last_message_at, c.created_at) DESC, c.id DESC LIMIT $6"
    ))
    .bind(user)
    .bind(org)
    .bind(of)
    .bind(after.map(|after| after.at))
    .bind(after.map(|after| after.id))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The chats of the nodes of a root org that have messages, the most recently
/// active first. Whether the caller may read each is for the caller to ask.
pub async fn node_chats_with_messages(
    conn: &mut Conn,
    org: Id,
    after: Option<After>,
    limit: i64,
) -> Result<Vec<ChatRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT c.id, c.kind, c.node_id, c.org_id, c.direct_key, c.name, c.created_by, c.next_seq, c.seq_state, ",
        "c.last_message_seq, c.last_message_at, c.created_at ",
        "FROM chats c JOIN nodes n ON n.id = c.node_id ",
        "WHERE c.org_id = $1 AND c.kind = 'node' AND c.last_message_seq > 0 AND n.deleted_at IS NULL ",
        "AND ($2::timestamptz IS NULL OR (c.last_message_at, c.id) < ($2, $3)) ",
        "ORDER BY c.last_message_at DESC, c.id DESC LIMIT $4"
    ))
    .bind(org)
    .bind(after.map(|after| after.at))
    .bind(after.map(|after| after.id))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The chats of a root org that the person has read less of than has been
/// written, with how far they have read: the chats that are unread for them
/// if they follow them. Direct and group chats where they are a member and
/// have no state yet are among them.
pub async fn unread_in_org(
    conn: &mut Conn,
    user: Id,
    org: Id,
) -> Result<Vec<(ChatRow, Option<StateRow>)>, DbError> {
    let chats: Vec<ChatRow> = sqlx::query_as(concat!(
        "SELECT c.id, c.kind, c.node_id, c.org_id, c.direct_key, c.name, c.created_by, c.next_seq, c.seq_state, ",
        "c.last_message_seq, c.last_message_at, c.created_at ",
        "FROM chats c ",
        "LEFT JOIN chat_states s ON s.chat_id = c.id AND s.user_id = $1 ",
        "WHERE c.org_id = $2 AND c.last_message_seq > COALESCE(s.last_read_seq, 0) ",
        "AND (c.kind = 'node' ",
        "     OR c.kind <> 'node' AND EXISTS (SELECT 1 FROM chat_members m WHERE m.chat_id = c.id AND m.user_id = $1 AND m.left_at IS NULL))"
    ))
    .bind(user)
    .bind(org)
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Id> = chats.iter().map(|chat| chat.id).collect();
    let mut states: HashMap<Id, StateRow> = states_in(conn, user, &ids)
        .await?
        .into_iter()
        .map(|state| (state.chat_id, state))
        .collect();
    Ok(chats
        .into_iter()
        .map(|chat| {
            let state = states.remove(&chat.id);
            (chat, state)
        })
        .collect())
}

// ------------------------------------------------------------------ notifications

/// A notice that is renewed in a person's Inbox.
#[derive(Debug, Clone, Copy)]
pub struct Notice<'a> {
    /// Whose Inbox.
    pub user_id: Id,
    /// `chat.message` or `mention`.
    pub kind: &'a str,
    /// The node the chat belongs to, for a node's chat.
    pub node_id: Option<Id>,
    /// The chat.
    pub chat_id: Id,
    /// Its title.
    pub title: &'a str,
    /// Its first line.
    pub body: &'a str,
    /// Whatever else it carries.
    pub data: &'a Json,
    /// `<kind>:<source>`; the same key is the same row.
    pub dedupe_key: &'a str,
    /// When.
    pub at: DateTime<Utc>,
}

/// Writes a notice to a person's Inbox, or renews the one they have under its
/// key: it is unread again, newer, and says what the latest message says. The
/// database is told that their feed moved when the transaction ends.
pub async fn renew_notice(
    conn: &mut Conn,
    notice: &Notice<'_>,
) -> Result<Option<Announced>, DbError> {
    let written: Option<(Id, i64)> = sqlx::query_as(
        "WITH bumped AS (
            UPDATE users SET inbox_seq = inbox_seq + 1 WHERE id = $1 RETURNING inbox_seq
         )
         INSERT INTO notifications (id, user_id, kind, node_id, chat_id, title, body, data, dedupe_key, inbox_seq, created_at)
         SELECT $2, $1, $3, $4, $5, $6, $7, $8, $9, bumped.inbox_seq, $10 FROM bumped
         ON CONFLICT (user_id, dedupe_key) DO UPDATE
            SET title = EXCLUDED.title, body = EXCLUDED.body, data = EXCLUDED.data, node_id = EXCLUDED.node_id,
                inbox_seq = EXCLUDED.inbox_seq, created_at = EXCLUDED.created_at, read_at = NULL
         RETURNING id, inbox_seq",
    )
    .bind(notice.user_id)
    .bind(orch_types::new_id())
    .bind(notice.kind)
    .bind(notice.node_id)
    .bind(notice.chat_id)
    .bind(notice.title)
    .bind(notice.body)
    .bind(notice.data)
    .bind(notice.dedupe_key)
    .bind(notice.at)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, inbox_seq)) = written else {
        return Ok(None);
    };
    sequences::notify_user(conn, notice.user_id, inbox_seq).await?;
    Ok(Some(Announced { id, inbox_seq }))
}

/// Marks the person's notifications about a chat as read, and moves their
/// Inbox on once if any of them changed, so that their other screens learn of
/// it. The new number of their Inbox, if it moved.
pub async fn read_notices(
    conn: &mut Conn,
    user: Id,
    chat: Id,
    at: DateTime<Utc>,
) -> Result<Option<i64>, DbError> {
    read_notices_through(conn, user, chat, None, at).await
}

/// Marks only notifications whose message is at or before the captured read position.
pub async fn read_notices_through(
    conn: &mut Conn,
    user: Id,
    chat: Id,
    through: Option<Seq>,
    at: DateTime<Utc>,
) -> Result<Option<i64>, DbError> {
    let seq: Option<i64> = sqlx::query_scalar(
        "UPDATE users SET inbox_seq = inbox_seq + 1
          WHERE id = $1 AND EXISTS (SELECT 1 FROM notifications n WHERE user_id = $1 AND chat_id = $2 AND read_at IS NULL
             AND ($3::bigint IS NULL OR EXISTS (SELECT 1 FROM chat_messages m WHERE m.chat_id=$2 AND m.id::text=n.data->>'message_id' AND m.seq_created <= $3)))
      RETURNING inbox_seq",
    )
    .bind(user)
    .bind(chat)
    .bind(through)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(seq) = seq else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE notifications n SET read_at = $3, inbox_seq = $4
          WHERE user_id = $1 AND chat_id = $2 AND read_at IS NULL
            AND ($5::bigint IS NULL OR EXISTS (SELECT 1 FROM chat_messages m WHERE m.chat_id=$2 AND m.id::text=n.data->>'message_id' AND m.seq_created <= $5))",
    )
    .bind(user)
    .bind(chat)
    .bind(at)
    .bind(seq)
    .bind(through)
    .execute(&mut *conn)
    .await?;
    sequences::notify_user(conn, user, seq).await?;
    Ok(Some(seq))
}

/// Whether `org` is a root org that is not deleted.
pub async fn is_root_org(conn: &mut Conn, org: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM nodes WHERE id = $1 AND kind = 'org' AND parent_id IS NULL AND deleted_at IS NULL)",
    )
    .bind(org)
    .fetch_one(conn)
    .await?)
}

/// The orgs of a root org's tree, the root first and the rest by how deep they
/// lie.
pub async fn orgs_of_tree(conn: &mut Conn, root: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM nodes WHERE org_id = $1 AND kind = 'org' AND deleted_at IS NULL
          ORDER BY array_length(path, 1), id",
    )
    .bind(root)
    .fetch_all(conn)
    .await?)
}

/// All orgs of several chat roots, in one read for a batched authority frame.
pub async fn orgs_of_trees(conn: &mut Conn, roots: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT id FROM nodes WHERE org_id=ANY($1) AND kind='org' AND deleted_at IS NULL ORDER BY array_length(path,1),id")
        .bind(roots).fetch_all(conn).await?)
}

/// People's chats wholly inside any of these authorized orgs. The join is filtered
/// before callers page results; it never runs a holder function for each person.
pub async fn people_chats_in(
    conn: &mut Conn,
    chats: &[Id],
    places: &[Id],
) -> Result<Vec<(Id, Id)>, DbError> {
    if chats.is_empty() || places.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT c.id,n.id FROM chats c JOIN nodes n ON n.org_id=c.org_id AND n.id=ANY($2)
          WHERE c.id=ANY($1) AND c.node_id IS NULL AND (n.id=c.org_id OR (
            EXISTS (SELECT 1 FROM chat_members cm WHERE cm.chat_id=c.id AND cm.left_at IS NULL)
            AND NOT EXISTS (SELECT 1 FROM chat_members cm WHERE cm.chat_id=c.id AND cm.left_at IS NULL
              AND NOT EXISTS (SELECT 1 FROM org_members m JOIN nodes d ON d.id=m.domain_id
                               WHERE m.org_id=c.org_id AND m.user_id=cm.user_id AND n.id=ANY(d.path)))))
          ORDER BY c.id,array_length(n.path,1) DESC,n.id",
    ).bind(chats).bind(places).fetch_all(conn).await?)
}

/// The kind of a node: `org`, `project`, `topic` or `thread`.
pub async fn node_kind(conn: &mut Conn, node: Id) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar("SELECT kind FROM nodes WHERE id = $1")
        .bind(node)
        .fetch_optional(conn)
        .await?)
}

/// The name of a node.
pub async fn node_name(conn: &mut Conn, node: Id) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar("SELECT name FROM nodes WHERE id = $1")
        .bind(node)
        .fetch_optional(conn)
        .await?)
}

/// The topic the chat of a node belongs to: the node itself for a topic's, the
/// topic a thread lies in for a thread's. None for an org's and a project's.
pub async fn topic_of(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar::<_, Option<Id>>(
        "SELECT CASE n.kind WHEN 'topic' THEN n.id ELSE t.topic_id END
           FROM nodes n LEFT JOIN threads t ON t.node_id = n.id
          WHERE n.id = $1 AND n.kind IN ('topic', 'thread')",
    )
    .bind(node)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// The slug of a root org: what the path that opens a chat of its people is
/// made of.
pub async fn org_slug(conn: &mut Conn, org: Id) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT slug FROM orgs WHERE node_id = $1")
            .bind(org)
            .fetch_optional(conn)
            .await?,
    )
}

// ------------------------------------------------------------------ inspection

/// Whether the person has an audit row of opening this chat as an outsider
/// from the start of the day of `at` (UTC) on. Written rows are never changed,
/// so this is also what makes the row once a day.
pub async fn inspected_today(
    conn: &mut Conn,
    user: Id,
    chat: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM audit_log
             WHERE action = 'chat.inspected' AND actor_kind = 'user' AND actor_id = $1 AND object_id = $2
               AND at >= date_trunc('day', $3::timestamptz AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
         )",
    )
    .bind(user)
    .bind(chat)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Takes a lock that lasts to the end of the transaction, named by some text:
/// two requests that ask for the same text wait for each other.
pub async fn lock_named(conn: &mut Conn, name: &str) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(name)
        .execute(conn)
        .await?;
    Ok(())
}

// ------------------------------------------------------------------ blobs

/// Which of the blobs `hashes` the store knows, with their sizes.
pub async fn blobs_known(
    conn: &mut Conn,
    hashes: &[Vec<u8>],
) -> Result<Vec<(Vec<u8>, i64)>, DbError> {
    Ok(
        sqlx::query_as("SELECT hash, size FROM blobs WHERE hash = ANY($1)")
            .bind(hashes)
            .fetch_all(conn)
            .await?,
    )
}

/// The parts of the oldest message of the chat, not deleted, that holds the blob
/// `hash` as an image or a file, or in a copy of another message it holds. None
/// when no message of the chat does: a message that was edited or deleted since
/// holds it no more. This is what lets a chat serve a blob to those who read
/// it, and nothing else the store has; a copy made by forwarding is a part of
/// the chat it was forwarded into, so it is served there.
pub async fn message_carrying(
    conn: &mut Conn,
    chat: Id,
    hash: &orch_types::Hash,
) -> Result<Option<Json>, DbError> {
    let blob = hash.to_string();
    let direct = serde_json::json!([{ "blob": blob }]);
    let images = serde_json::json!([{ "images": [{ "blob": blob }] }]);
    let attachments = serde_json::json!([{ "attachments": [{ "blob": blob }] }]);
    Ok(sqlx::query_scalar(
        "SELECT content FROM chat_messages
          WHERE chat_id = $1 AND deleted_at IS NULL AND kind = 'message'
            AND (content @> $2 OR content @> $3 OR content @> $4)
          ORDER BY seq_created LIMIT 1",
    )
    .bind(chat)
    .bind(&direct)
    .bind(&images)
    .bind(&attachments)
    .fetch_optional(conn)
    .await?)
}

/// Pins each of the blobs once more (`delta` 1), or once fewer (`-1`): a
/// message holds the images and files it carries for as long as it exists.
pub async fn pin_blobs(conn: &mut Conn, hashes: &[Vec<u8>], delta: i32) -> Result<(), DbError> {
    sqlx::query("UPDATE blobs SET pin_count = GREATEST(pin_count + $2, 0) WHERE hash = ANY($1)")
        .bind(hashes)
        .bind(delta)
        .execute(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::access::testing::insert_member;
    use crate::queries::hierarchy::testing::insert_node;
    use crate::queries::identity::testing::insert_user;
    use orch_testkit::database::TestDb;
    use orch_types::new_id;

    struct World {
        test: TestDb,
        ada: Id,
        bo: Id,
        cy: Id,
        root: Id,
        sales: Id,
        eng: Id,
        topic: Id,
    }

    async fn world() -> World {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bo, cy) = (new_id(), new_id(), new_id());
        for (id, name) in [(ada, "ada"), (bo, "bob"), (cy, "cyd")] {
            insert_user(&mut conn, id, name).await.unwrap();
        }
        let (root, sales, eng, project, topic) = (new_id(), new_id(), new_id(), new_id(), new_id());
        insert_node(&mut conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_node(&mut conn, sales, "org", Some(root), "Sales", ada)
            .await
            .unwrap();
        insert_node(&mut conn, eng, "org", Some(root), "Engineering", ada)
            .await
            .unwrap();
        insert_node(&mut conn, project, "project", Some(root), "Web", ada)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        insert_member(&mut conn, root, ada, root).await.unwrap();
        insert_member(&mut conn, root, bo, sales).await.unwrap();
        insert_member(&mut conn, root, cy, eng).await.unwrap();
        drop(conn);
        World {
            test,
            ada,
            bo,
            cy,
            root,
            sales,
            eng,
            topic,
        }
    }

    async fn group(conn: &mut Conn, world: &World, people: &[Id]) -> ChatRow {
        let at = chrono::Utc::now();
        let chat = insert_chat(
            conn,
            &NewChat {
                id: new_id(),
                kind: KIND_GROUP,
                org_id: world.root,
                direct_key: None,
                name: None,
                created_by: world.ada,
                at,
            },
        )
        .await
        .unwrap()
        .unwrap();
        for person in people {
            add_member(
                conn,
                &NewMember {
                    expires_at: None,
                    chat_id: chat.id,
                    user_id: *person,
                    added_by: Some(world.ada),
                    guest: false,
                    seq: 0,
                    at,
                },
            )
            .await
            .unwrap();
        }
        chat
    }

    #[tokio::test]
    async fn a_chat_numbers_its_changes_one_after_the_other_from_one() {
        let w = world().await;
        let mut conn = w.test.pool.acquire().await.unwrap();
        let chat = chat_of_node(&mut conn, w.topic).await.unwrap().unwrap();
        assert_eq!((chat.seq(), chat.kind.as_str()), (0, KIND_NODE));
        assert_eq!(next_seq(&mut conn, chat.id).await.unwrap(), Some(1));
        assert_eq!(next_seq(&mut conn, chat.id).await.unwrap(), Some(2));
        assert_eq!(
            seqs(&mut conn, &[chat.id]).await.unwrap(),
            vec![(chat.id, 2)]
        );
        assert_eq!(next_seq(&mut conn, new_id()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn two_people_have_one_direct_chat_in_an_org() {
        let w = world().await;
        let mut conn = w.test.pool.acquire().await.unwrap();
        let key = format!("{}:{}", w.ada.min(w.bo), w.ada.max(w.bo));
        let new = |id| NewChat {
            id,
            kind: KIND_DIRECT,
            org_id: w.root,
            direct_key: Some(&key),
            name: None,
            created_by: w.ada,
            at: chrono::Utc::now(),
        };
        let first = insert_chat(&mut conn, &new(new_id()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            insert_chat(&mut conn, &new(new_id()))
                .await
                .unwrap()
                .is_none()
        );
        let found = direct_chat(&mut conn, w.root, &key).await.unwrap().unwrap();
        assert_eq!(found.id, first.id);
    }

    #[tokio::test]
    async fn the_read_position_only_moves_forward_and_clears_the_mentions() {
        let w = world().await;
        let mut conn = w.test.pool.acquire().await.unwrap();
        let chat = group(&mut conn, &w, &[w.ada, w.bo]).await;
        raise_mentions(&mut conn, chat.id, &[w.bo], chrono::Utc::now())
            .await
            .unwrap();
        let state = state(&mut conn, w.bo, chat.id).await.unwrap().unwrap();
        assert_eq!((state.mentions_unread, state.engaged), (1, true));
        assert_eq!(
            mark_read(&mut conn, w.bo, chat.id, 9)
                .await
                .unwrap()
                .last_read_seq,
            9
        );
        let behind = mark_read(&mut conn, w.bo, chat.id, 4).await.unwrap();
        assert_eq!((behind.last_read_seq, behind.mentions_unread), (9, 0));
    }

    #[tokio::test]
    async fn a_person_follows_what_they_chose_else_what_taking_part_gives_them_else_nothing() {
        let w = world().await;
        let mut conn = w.test.pool.acquire().await.unwrap();
        let chat = chat_of_node(&mut conn, w.topic).await.unwrap().unwrap();
        let at = chrono::Utc::now();
        // Reading alone makes nobody follow; taking part does, at the default for a node's chat.
        mark_read(&mut conn, w.ada, chat.id, 0).await.unwrap();
        engage(&mut conn, chat.id, &[w.bo], at).await.unwrap();
        // The bell: a value follows whatever happened before; `off` mutes.
        set_level(&mut conn, w.cy, chat.id, Some("all"), at)
            .await
            .unwrap();
        set_level(&mut conn, w.ada, chat.id, Some("off"), at)
            .await
            .unwrap();
        let mut levels: Vec<(Id, String)> = node_chat_followers(&mut conn, chat.id)
            .await
            .unwrap()
            .into_iter()
            .map(|each| (each.user_id, each.level))
            .collect();
        levels.sort();
        let mut expected = vec![
            (w.bo, DEFAULT_NODE_LEVEL.to_owned()),
            (w.cy, "all".to_owned()),
        ];
        expected.sort();
        assert_eq!(levels, expected);
        // Taking the choice back returns to the default.
        set_level(&mut conn, w.cy, chat.id, None, at).await.unwrap();
        let followers = node_chat_followers(&mut conn, chat.id).await.unwrap();
        assert!(followers.iter().all(|each| each.user_id != w.cy));
    }

    #[tokio::test]
    async fn the_members_of_a_group_follow_it_at_the_default_for_such_chats_unless_they_chose() {
        let w = world().await;
        let mut conn = w.test.pool.acquire().await.unwrap();
        let chat = group(&mut conn, &w, &[w.ada, w.bo, w.cy]).await;
        set_level(
            &mut conn,
            w.bo,
            chat.id,
            Some("mentions"),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO notification_prefs (user_id, chat_direct_level) VALUES ($1, 'off')",
        )
        .bind(w.cy)
        .execute(&mut *conn)
        .await
        .unwrap();
        let mut levels: Vec<(Id, String)> = people_chat_followers(&mut conn, chat.id)
            .await
            .unwrap()
            .into_iter()
            .map(|each| (each.user_id, each.level))
            .collect();
        levels.sort();
        let mut expected = vec![
            (w.ada, DEFAULT_PEOPLE_LEVEL.to_owned()),
            (w.bo, "mentions".to_owned()),
        ];
        expected.sort();
        assert_eq!(levels, expected, "cy turned such chats off");
    }

    #[tokio::test]
    async fn an_admin_of_a_department_is_offered_the_chats_among_its_own_people_only() {
        let w = world().await;
        let mut conn = w.test.pool.acquire().await.unwrap();
        let second_in_sales = new_id();
        insert_user(&mut conn, second_in_sales, "eve")
            .await
            .unwrap();
        insert_member(&mut conn, w.root, second_in_sales, w.sales)
            .await
            .unwrap();
        let within = group(&mut conn, &w, &[w.bo, second_in_sales]).await;
        let across = group(&mut conn, &w, &[w.bo, w.cy]).await;
        let empty = group(&mut conn, &w, &[]).await;
        let seen = |rows: Vec<ChatRow>| {
            let mut ids: Vec<Id> = rows.into_iter().map(|row| row.id).collect();
            ids.sort();
            ids
        };
        let stranger = new_id();
        let sales_sees = seen(
            others(&mut conn, stranger, w.root, w.sales, None, 50)
                .await
                .unwrap(),
        );
        assert_eq!(sales_sees, vec![within.id]);
        let eng_sees = others(&mut conn, stranger, w.root, w.eng, None, 50)
            .await
            .unwrap();
        assert!(
            eng_sees.is_empty(),
            "nobody of Engineering is alone in a chat"
        );
        let root_sees = seen(
            others(&mut conn, stranger, w.root, w.root, None, 50)
                .await
                .unwrap(),
        );
        let mut every = vec![within.id, across.id, empty.id];
        every.sort();
        assert_eq!(
            root_sees, every,
            "the root org sees every chat of its tree, even one nobody is left in"
        );
    }

    #[tokio::test]
    async fn deleting_a_message_empties_it_and_keeps_the_row() {
        let w = world().await;
        let mut conn = w.test.pool.acquire().await.unwrap();
        let chat = chat_of_node(&mut conn, w.topic).await.unwrap().unwrap();
        let at = chrono::Utc::now();
        let content = serde_json::json!([{ "type": "text", "text": "hello @bob" }]);
        let seq = next_seq(&mut conn, chat.id).await.unwrap().unwrap();
        let written = insert_message(
            &mut conn,
            &NewMessage {
                id: new_id(),
                chat_id: chat.id,
                kind: MESSAGE,
                author_id: Some(w.ada),
                author_thread_id: None,
                author_agent: None,
                content: &content,
                reply_to_id: None,
                mentions: &[w.bo],
                mentions_all: false,
                client_id: Some("c1"),
                search_text: Some("hello @bob"),
                seq,
                at,
            },
        )
        .await
        .unwrap();
        let seq = next_seq(&mut conn, chat.id).await.unwrap().unwrap();
        set_reactions(
            &mut conn,
            written.id,
            &serde_json::json!({ "x": [w.bo] }),
            seq,
        )
        .await
        .unwrap();
        let seq = next_seq(&mut conn, chat.id).await.unwrap().unwrap();
        let gone = delete_message(&mut conn, written.id, w.ada, seq, at)
            .await
            .unwrap();
        assert_eq!(gone.content, serde_json::json!([]));
        assert!(gone.mentions.is_empty() && !gone.mentions_all);
        assert_eq!(gone.reactions, serde_json::json!({}));
        assert!(gone.search_text.is_none() && gone.is_deleted());
        assert_eq!((gone.seq_created, gone.seq_updated), (1, 3));
        let changed = changed(&mut conn, chat.id, 1, None, 10).await.unwrap();
        assert_eq!(
            changed.len(),
            1,
            "everything changed after a number still describes every change"
        );
        let again = message_by_client(&mut conn, chat.id, w.ada, "c1")
            .await
            .unwrap();
        assert_eq!(again.map(|row| row.id), Some(written.id));
    }
}
