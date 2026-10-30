//! Hourly system audit rows for refused machine reports, without report content.

use crate::audit::{self, Actor, Entry, Origin};
use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};
use orch_access_contract::audit::AuditAction;
use orch_types::Id;
use serde_json::json;

/// A machine and stream kind produce at most one refusal audit row in any hour.
pub const AUDIT_INTERVAL: Duration = Duration::hours(1);
// Every API process uses the same hash seed to serialize this audit bucket.
const LOCK_HASH_SEED: i64 = 0;

/// Appends the count since the previous row, or returns that row's time when still limited.
pub async fn record(
    conn: &mut Conn,
    machine: Id,
    stream_kind: &str,
    count: i64,
    at: DateTime<Utc>,
) -> Result<(bool, DateTime<Utc>), DbError> {
    if !super::transaction_guard::in_transaction(conn) {
        return Err(DbError::Rule {
            constraint: "report_refusal_requires_transaction".to_owned(),
        });
    }
    let action = crate::audit::action!(AuditAction::MachineReportRefused).as_str();
    let key = format!("{action}:{machine}:{stream_kind}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,$2))")
        .bind(key)
        .bind(LOCK_HASH_SEED)
        .execute(&mut *conn)
        .await?;
    let latest: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT at FROM audit_log WHERE action=$1 AND object_kind='machine' AND object_id=$2
         AND details->>'stream_kind'=$3 ORDER BY at DESC LIMIT 1",
    )
    .bind(action)
    .bind(machine)
    .bind(stream_kind)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(latest) = latest
        && at - latest < AUDIT_INTERVAL
    {
        return Ok((false, latest));
    }
    let mut entry = Entry::new(
        Actor::System,
        action,
        "machine",
        Some(machine),
        "Reports from a machine about something it does not hold were refused",
    );
    entry.details = json!({"machine_id":machine,"stream_kind":stream_kind,"count":count});
    audit::append(conn, &entry, &Origin::default(), at).await?;
    Ok((true, at))
}
