//! Current thread-owner and whole-follow reasons for a chat, and revision-fenced read positions.

use super::StateRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Seq};

/// A current reason a person follows a thread's chat without engagement.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Derived {
    /// The person.
    pub user_id: Id,
    /// The thread's chat.
    pub chat_id: Id,
    /// Whether an active Owners entry contains them through the shared holder graph.
    pub owner: bool,
    /// Whether they follow the thread wholly.
    pub following: bool,
    /// Their own direct-chat preference, before an explicit chat bell overrides it.
    pub default_level: String,
    /// The effective level after an explicit chat bell.
    pub level: String,
}

/// Finds current derived reasons, restricted to one person or a known set of chats.
/// These are notification candidates, not permission decisions; every caller still checks chat View.
pub async fn derived(
    conn: &mut Conn,
    user: Option<Id>,
    chats: Option<&[Id]>,
    at: DateTime<Utc>,
) -> Result<Vec<Derived>, DbError> {
    if user.is_none() && chats.is_none() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "WITH wanted AS (
             SELECT c.id,c.node_id,n.org_id FROM chats c JOIN nodes n ON n.id=c.node_id
              WHERE n.kind='thread' AND n.deleted_at IS NULL AND ($2::uuid[] IS NULL OR c.id=ANY($2))
                AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL)
         ), reasons AS (
             SELECT s.user_id,c.id AS chat_id,false AS owner,true AS following
              FROM wanted c JOIN subscriptions s ON s.node_id=c.node_id AND s.level='all'
              JOIN users u ON u.id=s.user_id AND u.status='active'
              WHERE $1::uuid IS NULL OR s.user_id=$1
             UNION ALL
             SELECT u.id,c.id,true,false FROM wanted c
              JOIN permission_entries e ON e.object_kind='node' AND e.object_id=c.node_id
                AND e.page='live' AND e.permission='access.own' AND e.removed_at IS NULL
                AND (e.expires_at IS NULL OR e.expires_at > $3)
              CROSS JOIN LATERAL (
                SELECT e.holder_id AS user_id WHERE e.holder_kind='user'
                UNION SELECT user_id FROM access_holder_people('group',e.holder_id,'',$3,false)
                  WHERE e.holder_kind='group'
              ) holders
              JOIN org_members m ON m.org_id=c.org_id AND m.user_id=holders.user_id
              JOIN users u ON u.id=holders.user_id AND u.status='active'
              WHERE $1::uuid IS NULL OR u.id=$1
         )
         SELECT r.user_id,r.chat_id,bool_or(r.owner) AS owner,bool_or(r.following) AS following,
                COALESCE(p.chat_direct_level,$4) AS default_level,
                COALESCE(s.level,p.chat_direct_level,$4) AS level
          FROM reasons r LEFT JOIN chat_states s ON s.user_id=r.user_id AND s.chat_id=r.chat_id
          LEFT JOIN notification_prefs p ON p.user_id=r.user_id
          GROUP BY r.user_id,r.chat_id,s.level,p.chat_direct_level ORDER BY r.chat_id,r.user_id",
    ).bind(user).bind(chats).bind(at).bind(super::DEFAULT_PEOPLE_LEVEL).fetch_all(conn).await?)
}

/// Creates and locks the ordinary per-person state row; every competing update takes this row lock.
pub async fn locked(conn: &mut Conn, user: Id, chat: Id) -> Result<StateRow, DbError> {
    sqlx::query("INSERT INTO chat_states (user_id,chat_id) VALUES ($1,$2) ON CONFLICT DO NOTHING")
        .bind(user)
        .bind(chat)
        .execute(&mut *conn)
        .await?;
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        state_columns!(),
        " FROM chat_states WHERE user_id=$1 AND chat_id=$2 FOR UPDATE"
    ))
    .bind(user)
    .bind(chat)
    .fetch_one(conn)
    .await?)
}

/// Stores a checked read position and recomputes mentions after that exact position.
pub async fn put_read(
    conn: &mut Conn,
    user: Id,
    chat: Id,
    position: Seq,
    revision: i64,
) -> Result<StateRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE chat_states SET last_read_seq=$3,read_revision=$4,mentions_unread=(",
        "SELECT count(*)::int FROM chat_messages m WHERE m.chat_id=$2 AND m.seq_created>$3 ",
        "AND m.deleted_at IS NULL AND $1=ANY(m.mentions)) WHERE user_id=$1 AND chat_id=$2 RETURNING ",state_columns!()
    )).bind(user).bind(chat).bind(position).bind(revision).fetch_one(conn).await?)
}

/// Whether a requested unread anchor is an actual live message, rather than a reaction sequence.
pub async fn has_anchor(conn: &mut Conn, chat: Id, seq: Seq) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM chat_messages WHERE chat_id=$1 AND seq_created=$2 AND kind='message' AND deleted_at IS NULL)")
        .bind(chat).bind(seq).fetch_one(conn).await?)
}
