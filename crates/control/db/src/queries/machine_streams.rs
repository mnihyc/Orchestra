//! Immutable harness machines for segment streams.
//!
//! A segment can exist before a session opens. Its source is assigned when
//! the first open is sent, and later opens cannot give it another source.

use crate::{Conn, DbError};
use orch_types::Id;

/// Keeps only the safe identity and reason of an event whose representation the database refused.
pub async fn record_storage_gap(
    conn: &mut Conn,
    machine: Id,
    stream: &str,
    seq: i64,
    code: &str,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO machine_event_storage_gaps(machine_id,stream,event_seq,sqlstate,recorded_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
        .bind(machine).bind(stream).bind(seq).bind(code).bind(at).execute(conn).await?;
    Ok(())
}

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

/// The immutable harness source recorded for this actual thread and segment, when bound.
pub async fn segment_machine(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar::<_, Option<Id>>(
        "SELECT machine_id FROM segments WHERE thread_id=$1 AND id=$2",
    )
    .bind(thread)
    .bind(segment)
    .fetch_optional(conn)
    .await?
    .flatten())
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
