//! Immutable harness machines for segment streams.
//!
//! A segment can exist before a session opens. Its source is assigned when
//! the first open is sent, and later opens cannot give it another source.

use crate::{Conn, DbError};
use orch_types::Id;

/// Assigns the source of a segment once, accepting later opens only on that machine.
pub async fn bind_segment(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
    machine: Id,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE segments SET machine_id = $3
         WHERE id = $2 AND thread_id = $1 AND (machine_id IS NULL OR machine_id = $3)",
    )
    .bind(thread)
    .bind(segment)
    .bind(machine)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Whether this machine is the recorded source of this segment of this thread.
pub async fn owns_segment(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
    machine: Id,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM segments WHERE id = $2 AND thread_id = $1 AND machine_id = $3)",
    )
    .bind(thread)
    .bind(segment)
    .bind(machine)
    .fetch_one(conn)
    .await?)
}
