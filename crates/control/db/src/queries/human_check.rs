//! What the audit holds of the human check's service not being reached, and the lock that keeps its rows to one an
//! hour for each act.

use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};
use orch_types::Json;

/// An act and an outcome write at most one row in any hour.
pub const AUDIT_INTERVAL: Duration = Duration::hours(1);
/// Every API process uses the same hash seed to serialize one bucket.
const LOCK_HASH_SEED: i64 = 0;

/// A row of the audit about the service not being reached.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Unreached {
    /// When it was written.
    pub at: DateTime<Utc>,
    /// `auth.human_check_admitted` or `auth.human_check_refused`.
    pub action: String,
    /// The act, the word for what went wrong, and how many times since the row before.
    pub details: Json,
}

/// Serializes the writers of one act's row in the caller's transaction, and answers when the last row of `action`
/// for `act` was written.
pub async fn lock_and_latest(
    conn: &mut Conn,
    action: &str,
    act: &str,
) -> Result<Option<DateTime<Utc>>, DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,$2))")
        .bind(format!("{action}:{act}"))
        .bind(LOCK_HASH_SEED)
        .execute(&mut *conn)
        .await?;
    Ok(sqlx::query_scalar(
        "SELECT at FROM audit_log WHERE action=$1 AND details->>'act'=$2 ORDER BY at DESC LIMIT 1",
    )
    .bind(action)
    .bind(act)
    .fetch_optional(conn)
    .await?)
}

/// The rows of `actions` written since `since`, the newest first.
pub async fn since(
    conn: &mut Conn,
    actions: &[&str],
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Unreached>, DbError> {
    let actions: Vec<String> = actions.iter().map(|action| (*action).to_owned()).collect();
    Ok(sqlx::query_as(
        "SELECT at, action, details FROM audit_log WHERE action = ANY($1) AND at >= $2 ORDER BY at DESC LIMIT $3",
    )
    .bind(actions)
    .bind(since)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}
