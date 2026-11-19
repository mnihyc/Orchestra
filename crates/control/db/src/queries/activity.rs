//! A place's activity at two depths, with repeated meaningful acts grouped before paging.

use super::hierarchy::{AuditCursor, AuditRow};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// One activity entry before the caller's permission checks.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Row {
    /// The newest act represented by this entry.
    #[sqlx(flatten)]
    pub base: AuditRow,
    /// The number of acts represented.
    pub count: i64,
    /// The place a removed node used to lie in.
    pub parent_id: Option<Id>,
    /// The current kind of the node, or its recorded kind after removal.
    pub node_kind: String,
    /// Whether the node no longer has a page to open.
    pub node_removed: bool,
}

/// Reads representative acts, newest first, with filters and grouping applied before the cursor.
///
/// Summary uses fixed UTC hours so a group cannot reappear on the next page. Each group contains
/// the same actor, action and object, and spans less than one hour. Every change keeps each act.
pub async fn read(
    conn: &mut Conn,
    place: Id,
    since: Option<DateTime<Utc>>,
    after: Option<AuditCursor>,
    limit: i64,
    summary: bool,
    actions: &[String],
) -> Result<Vec<Row>, DbError> {
    Ok(sqlx::query_as(
        "WITH scoped AS (
            SELECT a.*, COALESCE(n.parent_id, s.home_node_id) AS parent_id,
                   COALESCE(n.kind, NULLIF(a.object_kind, 'node'), 'thread') AS node_kind,
                   (n.id IS NULL OR EXISTS (SELECT 1 FROM nodes d WHERE d.id = ANY(n.path) AND d.deleted_at IS NOT NULL)) AS node_removed
              FROM audit_log a
              LEFT JOIN nodes n ON n.id = a.node_id
              LEFT JOIN audit_object_state s ON s.object_kind = 'node' AND s.object_id = a.node_id
             WHERE (a.node_id = $1 OR COALESCE(n.parent_id, s.home_node_id) = $1
                    OR (a.action='thread.moved' AND (a.details->>'from'=$1::text OR a.details->>'to'=$1::text))
                    OR n.parent_id IN (SELECT f.id FROM nodes f JOIN nodes p ON p.id = $1 AND p.kind = 'project'
                                        WHERE f.kind = 'project' AND f.deleted_at IS NULL AND f.path @> ARRAY[$1]::uuid[]))
               AND ($2::timestamptz IS NULL OR a.at > $2)
               AND a.action = ANY($6)
               AND (NOT $5 OR COALESCE(a.details->>'page', '') <> 'defaults')
         ), grouped AS (
            SELECT scoped.*,
                   row_number() OVER acts AS ordinal,
                   count(*) OVER acts AS count
              FROM scoped
              WINDOW acts AS (PARTITION BY
                CASE WHEN $5 THEN actor_kind ELSE id::text END,
                CASE WHEN $5 THEN actor_id END,
                CASE WHEN $5 THEN action END,
                CASE WHEN $5 THEN COALESCE(CASE WHEN action LIKE 'permission.%' THEN node_id ELSE object_id END, node_id) END,
                CASE WHEN $5 THEN date_trunc('hour', at AT TIME ZONE 'UTC') END
                ORDER BY at DESC, id DESC ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)
         )
         SELECT id, at, actor_kind, actor_id, action, history, override, object_kind, object_id,
                node_id, summary, details, host(ip) AS ip, count, parent_id, node_kind, node_removed
           FROM grouped
          WHERE ordinal = 1 AND ($3::uuid IS NULL OR (at, id) < ($4, $3))
          ORDER BY at DESC, id DESC LIMIT $7",
    )
    .bind(place)
    .bind(since)
    .bind(after.map(|cursor| cursor.id))
    .bind(after.map(|cursor| cursor.at))
    .bind(summary)
    .bind(actions)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}
