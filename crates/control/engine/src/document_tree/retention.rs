//! A deletion's recorded promise, shared by restoration and the complete agent preview.

use super::{Entry, Error, Item, Options, Receipt, prepare};
use chrono::{DateTime, Utc};
use orch_db::{Conn, queries::documents::containers};
use orch_types::Time;

/// Reads the exact authorized entry's retained deadline. Live entries have no existing promise.
/// Callers authorize and pin the entry first; this metadata read grants no access itself.
/// Current deployment retention is used only for a legacy deletion with no saved promise.
pub async fn restore_deadline_in(
    conn: &mut Conn,
    entry: &Entry,
    options: Options,
) -> Result<Option<DateTime<Utc>>, Error> {
    if entry.deleted_at.is_none() {
        return Ok(None);
    }
    let kind = match entry.item {
        Item::Container(_) => "document_container",
        Item::Document(_) => "document",
    };
    if let Some(until) = orch_db::queries::trace::life(conn, kind, entry.item.id())
        .await?
        .and_then(|life| life.purge_at)
    {
        return Ok(Some(until));
    }
    if let Some(cohort) = entry.deletion
        && let Some(saved) = containers::operation_by_id(conn, cohort).await?
        && let Some(until) = serde_json::from_value::<Receipt>(saved.result)?.restore_until
    {
        return Ok(Some(until));
    }
    restore_deadline_from(entry, None, None, options)
}

/// Resolves already loaded retained metadata with the single-entry reader's exact precedence.
/// This arithmetic grants no access; callers must authorize the actual entry first.
pub fn restore_deadline_from(
    entry: &Entry,
    purge_at: Option<DateTime<Utc>>,
    cohort_until: Option<DateTime<Utc>>,
    options: Options,
) -> Result<Option<DateTime<Utc>>, Error> {
    let Some(deleted) = entry.deleted_at else {
        return Ok(None);
    };
    Ok(Some(match purge_at.or(cohort_until) {
        Some(until) => until,
        None => prepare::deadline(Time::new(deleted), options)?,
    }))
}
