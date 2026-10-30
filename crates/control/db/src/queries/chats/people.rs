//! Frozen mention targets and visible asked-agent input references stored with a chat message.

use crate::{Conn, DbError};
use orch_types::{Id, Json};

/// Stores typed targets without issuing a second message sequence or notification.
pub async fn targets(conn: &mut Conn, message: Id, targets: &Json) -> Result<(), DbError> {
    sqlx::query("UPDATE chat_messages SET mention_targets = $2 WHERE id = $1")
        .bind(message)
        .bind(targets)
        .execute(conn)
        .await?;
    Ok(())
}

/// Stores the input references made by the message's ordinary forwarding transaction.
pub async fn asked(conn: &mut Conn, message: Id, references: &Json) -> Result<(), DbError> {
    sqlx::query("UPDATE chat_messages SET asked_agents = $2 WHERE id = $1")
        .bind(message)
        .bind(references)
        .execute(conn)
        .await?;
    Ok(())
}
