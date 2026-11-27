//! The same effective Inbox retention and cutoff serve cleanup and its own-reader facts; the system's windows for
//! what was deleted and for stored content nothing names serve every reader of them.
use crate::{Error, Place};
use chrono::{DateTime, Duration, Utc};
use orch_db::{Conn, DbError};
use orch_settings_contract::registry::{self, keys};
/// Current policy, measured without claiming that cleanup has already run.
pub struct InboxPolicy {
    /// Effective days, including the catalogue's lower bound.
    pub days: i64,
    /// Rows older than this are eligible for expiry, read or unread.
    pub expires_before: DateTime<Utc>,
    /// Whether the system wrote a value rather than using the catalogue default.
    pub configured: bool,
}
/// Resolve once, without reading any private configuration origin.
pub async fn inbox(conn: &mut Conn, now: DateTime<Utc>) -> Result<InboxPolicy, Error> {
    let view = crate::resolve(conn, Place::System, keys::RETENTION_INBOX_DAYS).await?;
    let configured = view.own.is_some();
    let fallback = registry::get(keys::RETENTION_INBOX_DAYS)
        .and_then(|def| def.default_value())
        .and_then(|value| value.as_i64())
        .ok_or_else(|| DbError::Rule {
            constraint: "inbox_retention_default_missing".into(),
        })?;
    let days = match view.resolved.value {
        Some(value) => {
            serde_json::from_value::<i64>(value).map_err(|error| Error::NotOfThatType {
                key: keys::RETENTION_INBOX_DAYS.into(),
                reason: error.to_string(),
            })?
        }
        None => fallback,
    }
    .max(1);
    let expires_before = Duration::try_days(days)
        .and_then(|duration| now.checked_sub_signed(duration))
        .unwrap_or(DateTime::<Utc>::MIN_UTC);
    Ok(InboxPolicy {
        days,
        expires_before,
        configured,
    })
}

/// For how many days what was deleted can be brought back before the purge takes it, as the system sets it.
pub async fn deleted_keep_days(conn: &mut Conn) -> Result<u64, DbError> {
    system_amount(conn, keys::RETENTION_DELETED_KEEP_DAYS).await
}

/// For how many hours stored content that nothing names is kept before it is collected, as the system sets it.
pub async fn blob_grace_hours(conn: &mut Conn) -> Result<u64, DbError> {
    system_amount(conn, keys::RETENTION_BLOB_GRACE_HOURS).await
}

/// A whole amount the system sets, with the catalogue's default where it sets none.
async fn system_amount(conn: &mut Conn, key: &'static str) -> Result<u64, DbError> {
    let value = orch_db::queries::settings::row(conn, None, key)
        .await?
        .map(|row| row.value)
        .or_else(|| registry::get(key)?.default_value());
    value
        .and_then(|value| value.as_u64())
        .ok_or_else(|| DbError::Rule {
            constraint: key.into(),
        })
}

/// End of the restoration window, including windows longer than a representable date.
pub fn purge_at(at: DateTime<Utc>, days: u64) -> DateTime<Utc> {
    i64::try_from(days)
        .ok()
        .and_then(Duration::try_days)
        .and_then(|span| at.checked_add_signed(span))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// Oldest restorable deletion for this window.
pub fn deleted_before(at: DateTime<Utc>, days: u64) -> DateTime<Utc> {
    i64::try_from(days)
        .ok()
        .and_then(Duration::try_days)
        .and_then(|span| at.checked_sub_signed(span))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// Oldest time still within the grace for unreferenced content.
pub fn unreferenced_before(at: DateTime<Utc>, hours: u64) -> DateTime<Utc> {
    i64::try_from(hours)
        .ok()
        .and_then(Duration::try_hours)
        .and_then(|span| at.checked_sub_signed(span))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}
