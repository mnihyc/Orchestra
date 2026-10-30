//! Canonical environment address segments, shared by displayed references and finder candidates.

use crate::{Conn, DbError};
use orch_types::Id;

/// A short segment from the environment row aliased `r`; punctuation never becomes address syntax.
/// The full immutable id is a stable fallback when the label contains no usable name characters.
pub const ENVIRONMENT_SEGMENT_SQL: &str = "COALESCE(NULLIF(btrim(regexp_replace(lower(r.name), '[^[:alnum:]_-]+', '-', 'g'), '-'), ''), 'environment-' || r.id::text)";

/// Environment segments in one query, with the very expression used before finder matching.
pub async fn environment_segments(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<Vec<(Id, String)>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT r.id,{ENVIRONMENT_SEGMENT_SQL} FROM environments r WHERE r.id=ANY($1)"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}
