//! Per-chat human posting stops and the durable rolling-hour count of successful agent posts.

use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};
use orch_types::Id;

/// The rolling window promised for agent chat broadcasts.
pub const WINDOW_SECONDS: i64 = 60 * 60;

/// The chats stopped for one origin, each with its node when it has one.
pub async fn stopped(conn: &mut Conn, thread: Id) -> Result<Vec<(Id, Option<Id>)>, DbError> {
    Ok(sqlx::query_as("SELECT p.chat_id,c.node_id FROM chat_agent_posting p JOIN chats c ON c.id=p.chat_id WHERE p.thread_id=$1 AND p.stopped")
        .bind(thread).fetch_all(conn).await?)
}

/// Whether a human currently stopped this origin in this chat.
pub async fn is_stopped(conn: &mut Conn, chat: Id, thread: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM chat_agent_posting WHERE chat_id=$1 AND thread_id=$2 AND stopped)")
        .bind(chat).bind(thread).fetch_one(conn).await?)
}

/// Candidate origins from live agent shares, successful messages and deliberate local stops.
/// The API cuts these by current thread View and resolves actual reach before naming any of them.
pub async fn origins(conn: &mut Conn, chat: Id, at: DateTime<Utc>) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT origin.id FROM chats c LEFT JOIN nodes place ON place.id=c.node_id
           JOIN permission_entries e ON e.holder_kind='agents' AND e.removed_at IS NULL
             AND (e.expires_at IS NULL OR e.expires_at>$2) AND e.permission IN ('chat.view','chat.interact')
             AND (e.object_kind='chat' AND e.object_id=c.id OR e.object_kind='node' AND e.object_id=ANY(place.path))
           JOIN nodes origin ON origin.kind='thread' AND origin.deleted_at IS NULL AND e.holder_id=ANY(origin.path)
          WHERE c.id=$1
         UNION SELECT author_thread_id FROM chat_messages WHERE chat_id=$1 AND author_thread_id IS NOT NULL
         UNION SELECT thread_id FROM chat_agent_posting WHERE chat_id=$1 ORDER BY 1",
    ).bind(chat).bind(at).fetch_all(conn).await?)
}

/// The main thread in its own chat, or the main threads of a topic's chat.
pub async fn local_mention_origins(conn: &mut Conn, chat: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT t.node_id FROM chats c JOIN threads t ON t.node_id=c.node_id OR t.topic_id=c.node_id JOIN nodes n ON n.id=t.node_id WHERE c.id=$1 AND t.side_of IS NULL AND n.deleted_at IS NULL ORDER BY t.node_id")
        .bind(chat).fetch_all(conn).await?)
}

/// Creation times of already-visible candidate threads, for distinguishing equal names.
pub async fn origin_starts(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<Vec<(Id, DateTime<Utc>)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id,created_at FROM nodes WHERE id=ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Sets the local stop while the caller holds authority and the origin thread's row lock.
pub async fn set(
    conn: &mut Conn,
    chat: Id,
    thread: Id,
    stopped: bool,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("INSERT INTO chat_agent_posting (chat_id,thread_id,stopped,changed_by,changed_at) VALUES ($1,$2,$3,$4,$5)
        ON CONFLICT (chat_id,thread_id) DO UPDATE SET stopped=$3,changed_by=$4,changed_at=$5
        WHERE chat_agent_posting.stopped IS DISTINCT FROM $3")
        .bind(chat).bind(thread).bind(stopped).bind(by).bind(at).execute(conn).await?.rows_affected() > 0)
}

/// The first instant another post fits, or none while below the positive limit.
/// The origin row lock serializes this read with every post and its audit across all destinations.
pub async fn next_allowed(
    conn: &mut Conn,
    thread: Id,
    limit: i64,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let from = now - Duration::seconds(WINDOW_SECONDS);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='chat.posted_by_agent' AND actor_kind='agent' AND actor_id=$1 AND at>$2 AND at<=$3")
        .bind(thread).bind(from).bind(now).fetch_one(&mut *conn).await?;
    if limit <= 0 || count < limit {
        return Ok(None);
    }
    // If policy was lowered, enough earlier posts must expire to get below the new bound.
    let release: DateTime<Utc> = sqlx::query_scalar("SELECT at FROM audit_log WHERE action='chat.posted_by_agent' AND actor_kind='agent' AND actor_id=$1 AND at>$2 AND at<=$3 ORDER BY at,id OFFSET $4 LIMIT 1")
        .bind(thread).bind(from).bind(now).bind(count - limit).fetch_one(conn).await?;
    Ok(Some(release + Duration::seconds(WINDOW_SECONDS)))
}
