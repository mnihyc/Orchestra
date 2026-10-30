//! Temporary environment deadlines, their creating thread, and the owner's warning window.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// Durable timing, with the current running-turn fact projected on reads.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Lifetime {
    /// The environment.
    pub environment_id: Id,
    /// The first thread made for a thread-scoped temporary environment.
    pub thread_id: Option<Id>,
    /// The currently resolved idle limit.
    pub idle_hours: i64,
    /// Activity used to compute this deadline.
    pub activity_at: DateTime<Utc>,
    /// The archive transition used to compute this deadline.
    pub archive_at: Option<DateTime<Utc>>,
    /// Actual earliest removal time, including the warning opportunity.
    pub expires_at: DateTime<Utc>,
    /// When the current owner received the warning.
    pub warned_at: Option<DateTime<Utc>>,
    /// Whom the warning was for; a new owner needs a new warning.
    pub warned_owner_id: Option<Id>,
    /// A running turn keeps cleanup from starting.
    pub active: bool,
}

const COLUMNS: &str = "l.environment_id,l.thread_id,l.idle_hours,l.activity_at,l.archive_at,l.expires_at,l.warned_at,l.warned_owner_id,
    (EXISTS(SELECT 1 FROM threads t JOIN turns u ON u.thread_id=t.node_id WHERE t.environment_id=l.environment_id AND u.state IN ('starting','running'))
     OR EXISTS(SELECT 1 FROM jobs j WHERE j.environment_id=l.environment_id AND j.state IN ('queued','running'))) AS active";

/// An explicit choice; true also follows the creating thread, false only idle time.
/// Malformed or unrelated configuration never becomes destructive.
#[must_use]
pub fn mode(config: &orch_types::Json) -> Option<bool> {
    match config
        .get("env.lifetime")
        .and_then(orch_types::Json::as_str)
    {
        Some("temporary") => Some(false),
        Some("thread") => Some(true),
        _ => None,
    }
}

/// One record whose live environment still opts into temporary cleanup.
pub async fn read(conn: &mut Conn, id: Id) -> Result<Option<Lifetime>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM environment_lifetimes l JOIN environments e ON e.id=l.environment_id
        WHERE e.id=$1 AND e.deleted_at IS NULL AND e.config->>'env.lifetime' IN ('temporary','thread')")))
        .bind(id).fetch_optional(conn).await?)
}

/// A resource projection reads all of its lifecycle records together.
pub async fn among(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Lifetime>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM environment_lifetimes l JOIN environments e ON e.id=l.environment_id
        WHERE e.id=ANY($1) AND e.deleted_at IS NULL AND e.config->>'env.lifetime' IN ('temporary','thread')")))
        .bind(ids).fetch_all(conn).await?)
}

/// Opt-ins made by an ordinary setting edit also receive a lifecycle record.
pub async fn untracked(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT e.id FROM environments e WHERE e.deleted_at IS NULL AND e.remove_at IS NULL
        AND e.config->>'env.lifetime' IN ('temporary','thread') AND NOT EXISTS(SELECT 1 FROM environment_lifetimes l WHERE l.environment_id=e.id)
        ORDER BY e.id LIMIT $1").bind(limit).fetch_all(conn).await?)
}

/// Oldest checked first prevents active environments starving the rest of a batch.
pub async fn candidates(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT l.environment_id FROM environment_lifetimes l JOIN environments e ON e.id=l.environment_id
        WHERE e.deleted_at IS NULL AND e.remove_at IS NULL ORDER BY l.checked_at,l.environment_id LIMIT $1")
        .bind(limit).fetch_all(conn).await?)
}

/// A busy resource is retried on a later sweep rather than waiting with a transaction open.
pub async fn try_lock(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar::<_,Id>("SELECT id FROM environments WHERE id=$1 AND deleted_at IS NULL AND remove_at IS NULL FOR UPDATE SKIP LOCKED")
        .bind(id).fetch_optional(conn).await?.is_some())
}

/// Saves metadata; only a visible change advances the environment's revision.
pub async fn save(
    conn: &mut Conn,
    row: &Lifetime,
    now: DateTime<Utc>,
    visible_change: bool,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO environment_lifetimes(environment_id,thread_id,idle_hours,activity_at,archive_at,expires_at,warned_at,warned_owner_id,checked_at)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(environment_id) DO UPDATE SET thread_id=EXCLUDED.thread_id,idle_hours=EXCLUDED.idle_hours,
        activity_at=EXCLUDED.activity_at,archive_at=EXCLUDED.archive_at,expires_at=EXCLUDED.expires_at,warned_at=EXCLUDED.warned_at,
        warned_owner_id=EXCLUDED.warned_owner_id,checked_at=EXCLUDED.checked_at")
        .bind(row.environment_id).bind(row.thread_id).bind(row.idle_hours).bind(row.activity_at).bind(row.archive_at)
        .bind(row.expires_at).bind(row.warned_at).bind(row.warned_owner_id).bind(now).execute(&mut *conn).await?;
    if visible_change {
        sqlx::query("UPDATE environments SET revision=revision+1,updated_at=$2 WHERE id=$1 AND deleted_at IS NULL")
            .bind(row.environment_id).bind(now).execute(conn).await?;
    }
    Ok(())
}

/// Only the owner's first thread can claim a thread-scoped environment's creating-thread link.
pub async fn associate(
    conn: &mut Conn,
    environment: Id,
    thread: Id,
    owner: Id,
) -> Result<(), DbError> {
    sqlx::query("UPDATE environment_lifetimes l SET thread_id=$2 FROM environments e
        WHERE l.environment_id=$1 AND e.id=l.environment_id AND e.owner_id=$3 AND e.config->>'env.lifetime'='thread' AND l.thread_id IS NULL")
        .bind(environment).bind(thread).bind(owner).execute(conn).await?;
    Ok(())
}

/// The creating thread's archive or deletion, never another thread's transition.
pub async fn archived_at(conn: &mut Conn, thread: Id) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT COALESCE(archived_at,deleted_at) FROM nodes WHERE id=$1",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// Keep changes the lifetime and drops its metadata inside the caller's transaction.
pub async fn keep(conn: &mut Conn, id: Id, now: DateTime<Utc>) -> Result<bool, DbError> {
    let changed=sqlx::query("UPDATE environments SET config=jsonb_set(config,'{env.lifetime}','\"lasting\"'::jsonb),revision=revision+1,updated_at=$2
        WHERE id=$1 AND deleted_at IS NULL AND config->>'env.lifetime' IN ('temporary','thread')")
        .bind(id).bind(now).execute(&mut *conn).await?.rows_affected()>0;
    if changed {
        forget(conn, id).await?;
    }
    Ok(changed)
}

/// Forgetting metadata never schedules or removes files.
pub async fn forget(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM environment_lifetimes WHERE environment_id=$1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}
