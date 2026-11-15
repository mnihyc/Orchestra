//! Immutable workspace destinations and real native receipts of the shared upload sessions.

use super::super::storage::SessionRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id};
use serde::{Deserialize, Serialize};

/// The actual location selected at creation, never rebound to a later machine or incarnation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub environment_id: Id,
    pub machine_id: Option<Id>,
    pub generation: i32,
    pub workspace_root: String,
    pub path: String,
    pub thread_id: Option<Id>,
    pub relayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Target {
    pub session_id: Id,
    #[sqlx(flatten)]
    pub destination: Destination,
    pub receiving_command: Option<Id>,
    pub verifying_command: Option<Id>,
    pub aborting_command: Option<Id>,
    pub verified_hash: Option<Vec<u8>>,
    pub written_job: Option<Id>,
}
impl Target {
    pub fn verified(&self) -> Result<Option<Hash>, DbError> {
        self.verified_hash
            .as_deref()
            .map(|raw| {
                Hash::from_slice(raw).map_err(|_| {
                    DbError::Other(sqlx::Error::Decode(
                        "an upload receipt hash is not 32 bytes".into(),
                    ))
                })
            })
            .transpose()
    }
}

/// Cancellation retains its actual owner and accepted stamp for narrow native cleanup/retry.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct State {
    #[sqlx(flatten)]
    pub session: SessionRow,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}
const COLUMNS: &str = "session_id,environment_id,machine_id,generation,workspace_root,path,thread_id,relayed,receiving_command,verifying_command,aborting_command,verified_hash,written_job";

pub async fn insert(conn: &mut Conn, session: Id, to: &Destination) -> Result<(), DbError> {
    sqlx::query("INSERT INTO upload_session_targets (session_id,environment_id,machine_id,generation,workspace_root,path,thread_id,relayed) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(session).bind(to.environment_id).bind(to.machine_id).bind(to.generation)
        .bind(&to.workspace_root).bind(&to.path).bind(to.thread_id).bind(to.relayed)
        .execute(conn).await?;
    Ok(())
}

pub async fn of(conn: &mut Conn, session: Id) -> Result<Option<Target>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM upload_session_targets WHERE session_id=$1"
    )))
    .bind(session)
    .fetch_optional(conn)
    .await?)
}

/// Always acquire the session row before this target's writer lock.
pub async fn lock(conn: &mut Conn, session: Id) -> Result<Option<Target>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM upload_session_targets WHERE session_id=$1 FOR UPDATE"
    )))
    .bind(session)
    .fetch_optional(conn)
    .await?)
}

pub async fn state(conn: &mut Conn, id: Id, locked: bool) -> Result<Option<State>, DbError> {
    let tail = if locked { " FOR UPDATE" } else { "" };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT id,user_id,name,media_type,size,idle_secs,received,created_at,updated_at,expires_at,cancelled_at,completed_at FROM upload_sessions WHERE id=$1{tail}")))
        .bind(id).fetch_optional(conn).await?)
}

/// Records only the native destination's acknowledged prefix and renews its idle lease.
pub async fn progress(
    conn: &mut Conn,
    id: Id,
    received: i64,
    at: DateTime<Utc>,
    expires: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE upload_sessions SET received=$2,updated_at=$3,expires_at=$4 WHERE id=$1 AND cancelled_at IS NULL AND completed_at IS NULL")
        .bind(id).bind(received).bind(at).bind(expires).execute(conn).await?;
    Ok(())
}

pub async fn receiving(conn: &mut Conn, id: Id, command: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE upload_session_targets SET receiving_command=$2 WHERE session_id=$1")
        .bind(id)
        .bind(command)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn verifying(conn: &mut Conn, id: Id, command: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE upload_session_targets SET verifying_command=$2 WHERE session_id=$1")
        .bind(id)
        .bind(command)
        .execute(conn)
        .await?;
    Ok(())
}

/// A verified native receipt is metadata; it never creates a blob row or attachment grant.
pub async fn verified(conn: &mut Conn, id: Id, command: Id, hash: &Hash) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE upload_session_targets SET verified_hash=$3 WHERE session_id=$1 AND verifying_command=$2 AND verified_hash IS NULL")
        .bind(id).bind(command).bind(hash.as_bytes().as_slice()).execute(conn).await?.rows_affected() > 0)
}

/// A native upload is consumed by one actual accepted write, even across lost responses.
pub async fn written(conn: &mut Conn, id: Id, job: Id) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE upload_session_targets SET written_job=$2 WHERE session_id=$1 AND (written_job IS NULL OR written_job=$2)")
        .bind(id).bind(job).execute(conn).await?.rows_affected() > 0)
}

/// One actual cleanup command for a cancellation accepted by this session's owner.
pub async fn aborting(conn: &mut Conn, id: Id, command: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE upload_session_targets SET aborting_command=$2 WHERE session_id=$1")
        .bind(id)
        .bind(command)
        .execute(conn)
        .await?;
    Ok(())
}
