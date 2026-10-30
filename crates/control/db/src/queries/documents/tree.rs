//! Tree stamps for the node a document stands beside when its placement or visibility changes.

use crate::{Conn, DbError};
use orch_types::Id;

/// Finds the node beside which a document stands, including a deleted document.
pub async fn node_of(conn: &mut Conn, document: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT node_id FROM documents WHERE id = $1")
            .bind(document)
            .fetch_optional(conn)
            .await?,
    )
}

/// Locks a document before remembering its old node, so concurrent moves stamp the actual place left.
pub async fn lock_node_of(conn: &mut Conn, document: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT node_id FROM documents WHERE id = $1 FOR UPDATE")
            .bind(document)
            .fetch_optional(conn)
            .await?,
    )
}

/// Advances and announces that node's tree summary in the caller's transaction.
pub async fn stamp(conn: &mut Conn, document: Id) -> Result<(), DbError> {
    let changed: Option<(Id, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT node_id,GREATEST(updated_at,deleted_at) FROM documents WHERE id=$1")
            .bind(document)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some((node, at)) = changed {
        crate::queries::hierarchy::stamp(conn, node).await?;
        crate::queries::hierarchy::touch_activity(conn, node, at).await?;
    }
    Ok(())
}
