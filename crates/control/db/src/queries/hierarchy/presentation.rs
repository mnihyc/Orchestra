//! Atomic node text and icon colour changes, initial colours and child lists narrowed by kind.

use super::{NodeRow, TextChange};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// Changes text and colour together at the revision the caller read.
pub async fn change(
    conn: &mut Conn,
    node: Id,
    text: TextChange<'_>,
    color: Option<Option<i16>>,
    expected: i64,
    now: DateTime<Utc>,
) -> Result<Option<i64>, DbError> {
    change_with_own(conn, node, text, color, None, expected, now).await
}

/// Changes text and colour together at the revision the caller read, a colour of the icon's own included: one of
/// the twelve takes the place of a colour of its own, and a colour of its own the place of the twelve's.
pub async fn change_with_own(
    conn: &mut Conn,
    node: Id,
    text: TextChange<'_>,
    color: Option<Option<i16>>,
    own: Option<Option<&str>>,
    expected: i64,
    now: DateTime<Utc>,
) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE nodes SET name = COALESCE($2, name), description = COALESCE($3, description),
            title_source = CASE WHEN $2::text IS NULL THEN title_source ELSE 'user' END,
            description_source = CASE WHEN $3::text IS NULL THEN description_source ELSE 'user' END,
            color = CASE WHEN $4 THEN $5::smallint WHEN $8 THEN NULL ELSE color END,
            custom_color = CASE WHEN $4 THEN NULL WHEN $8 THEN $9::text ELSE custom_color END,
            revision = revision + 1, updated_at = $7
          WHERE id = $1 AND revision = $6 AND deleted_at IS NULL RETURNING revision",
    )
    .bind(node)
    .bind(text.name)
    .bind(text.description)
    .bind(color.is_some())
    .bind(color.flatten())
    .bind(expected)
    .bind(now)
    .bind(own.is_some())
    .bind(own.flatten())
    .fetch_optional(conn)
    .await?)
}

/// Sets the colour while a new node is still being made, without advancing its revision.
pub async fn initial_color(conn: &mut Conn, node: Id, color: i16) -> Result<(), DbError> {
    sqlx::query("UPDATE nodes SET color = $2 WHERE id = $1")
        .bind(node)
        .bind(color)
        .execute(conn)
        .await?;
    Ok(())
}

/// Lists children of one kind before paging, so other kinds consume no page slots.
pub async fn children_of_kind(
    conn: &mut Conn,
    parent: Id,
    kind: &str,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<NodeRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.owner_id, n.archived_at,
                n.deleted_at, n.revision, e.placement, t.environment_id
           FROM nodes n LEFT JOIN threads t ON t.node_id = n.id
           LEFT JOIN environments e ON e.id = t.environment_id
          WHERE n.parent_id = $1 AND n.kind = $2 AND n.deleted_at IS NULL
            AND ($3::uuid IS NULL OR n.id > $3) ORDER BY n.id LIMIT $4",
    )
    .bind(parent)
    .bind(kind)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}
