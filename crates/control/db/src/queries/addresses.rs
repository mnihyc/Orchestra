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

/// The address of a known environment. A bounded reader sees only admitted organization
/// names along its home path; the public owner and the environment segment identify the rest.
pub async fn environment_address(
    conn: &mut Conn,
    id: Id,
    allowed_orgs: Option<&[Id]>,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT concat_ws('.', (
             SELECT string_agg(o.slug, '.' ORDER BY p.ord)
               FROM unnest(n.path) WITH ORDINALITY p(id, ord)
               JOIN orgs o ON o.node_id=p.id
              WHERE $2::uuid[] IS NULL OR o.node_id=ANY($2)
         ), u.username, {ENVIRONMENT_SEGMENT_SQL})
         FROM environments r
         LEFT JOIN nodes n ON n.id=r.home_node_id
         LEFT JOIN users u ON u.id=r.owner_id
         WHERE r.id=$1"
    )))
    .bind(id)
    .bind(allowed_orgs)
    .fetch_optional(conn)
    .await?)
}
