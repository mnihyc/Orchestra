//! The same effective Inbox retention and cutoff serve cleanup and its own-reader facts.
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
