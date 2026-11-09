//! Immutable work attribution from a native helper's actual initial input and tool journal.
//! These associations are billing facts, never permission or an executable source proof.
use crate::{Conn, DbError};
use orch_types::Id;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Work {
    /// The requesting place, independently of the helper's own thread/turn membership.
    pub node: Id,
    /// The requested common capability.
    pub purpose: String,
}

const SOURCE: &str = "SELECT node_id AS node,purpose FROM usage_work_scope($1)";

/// One retained input; later independent inputs in the same ordinary helper have no such association.
pub async fn for_input(conn: &mut Conn, input: Id) -> Result<Option<Work>, DbError> {
    Ok(sqlx::query_as(SOURCE)
        .bind(input)
        .fetch_optional(conn)
        .await?)
}

/// Preserve actual native turn identity while supplying its separate requesting work scope.
pub async fn for_turn(conn: &mut Conn, turn: Id) -> Result<Option<Work>, DbError> {
    let input: Option<Id> = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(turn)
        .fetch_optional(&mut *conn)
        .await?;
    match input {
        Some(input) => for_input(conn, input).await,
        None => Ok(None),
    }
}
