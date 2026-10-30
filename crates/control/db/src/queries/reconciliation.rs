//! Startup work already held by live threads, read in exact bounded pages without creating another turn, job or lease.

use crate::{Conn, DbError};
use orch_types::Id;

/// Pending and active work, including a turn left running while its durable machine stream catches up.
pub async fn threads(conn: &mut Conn, after: Option<Id>, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id
        WHERE n.deleted_at IS NULL AND n.archived_at IS NULL
          AND ($1::uuid IS NULL OR t.node_id > $1)
          AND (t.state IN ('starting','running','waiting_approval','blocked')
               OR EXISTS(SELECT 1 FROM thread_inputs i WHERE i.thread_id=t.node_id AND i.state='pending'))
        ORDER BY t.node_id LIMIT $2")
        .bind(after).bind(limit).fetch_all(conn).await?)
}
