//! A person's manual level exceptions. The existing (user,node) primary key bounds both the page
//! and each ancestor lookup; no organization tree or another person's subscriptions are scanned.
use crate::{Conn, DbError};
use orch_types::Id;

#[derive(Debug, sqlx::FromRow)]
pub struct ExceptionRow {
    pub node_id: Id,
    pub level: String,
    pub default_level: String,
    pub default_from: Option<Id>,
}

/// Candidate rows only; the API applies the ordinary current node View decision to this page.
pub async fn page(
    conn: &mut Conn,
    user: Id,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<ExceptionRow>, DbError> {
    Ok(sqlx::query_as("SELECT s.node_id,s.level,COALESCE(inherited.level,'off') AS default_level,inherited.node_id AS default_from
        FROM subscriptions s JOIN nodes n ON n.id=s.node_id AND n.deleted_at IS NULL
        LEFT JOIN LATERAL (
            SELECT parent.node_id,parent.level FROM unnest(n.path) WITH ORDINALITY ancestor(id,position)
            JOIN subscriptions parent ON parent.node_id=ancestor.id AND parent.user_id=s.user_id
            WHERE ancestor.id<>n.id ORDER BY ancestor.position DESC LIMIT 1
        ) inherited ON true
        WHERE s.user_id=$1 AND s.source='manual' AND ($2::uuid IS NULL OR s.node_id>$2)
          AND s.level<>COALESCE(inherited.level,'off')
        ORDER BY s.node_id LIMIT $3")
        .bind(user).bind(after).bind(limit).fetch_all(conn).await?)
}
