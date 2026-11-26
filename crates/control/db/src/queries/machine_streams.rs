//! Immutable harness machines for segment streams.
//!
//! History and native preparation can arrive in either order. The admitted
//! opening retains its actual source, which the first matching history report confirms.

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
         WHERE id = $2 AND thread_id = $1 AND (machine_id IS NULL OR machine_id = $3)
           AND NOT EXISTS(SELECT 1 FROM native_session_states p WHERE p.id=$2
             AND (p.thread_id<>$1 OR (p.machine_id IS NOT NULL AND p.machine_id<>$3)))",
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
        "SELECT COALESCE(s.machine_id,p.machine_id) FROM segments s LEFT JOIN native_session_states p ON p.id=s.id AND p.thread_id=s.thread_id AND p.delivered_resources->'tool_admission' IS NOT NULL WHERE s.thread_id=$1 AND s.id=$2 UNION ALL SELECT machine_id FROM native_session_states WHERE thread_id=$1 AND id=$2 AND delivered_resources->'tool_admission' IS NOT NULL AND NOT EXISTS(SELECT 1 FROM segments WHERE id=$2)",
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
        "SELECT EXISTS(SELECT 1 FROM segments WHERE id=$2 AND thread_id=$1 AND machine_id=$3 UNION ALL SELECT 1 FROM native_session_states p WHERE p.id=$2 AND p.thread_id=$1 AND p.machine_id=$3 AND p.delivered_resources->'tool_admission' IS NOT NULL AND NOT EXISTS(SELECT 1 FROM segments s WHERE s.id=$2 AND (s.thread_id<>$1 OR (s.machine_id IS NOT NULL AND s.machine_id<>$3))))",
    )
    .bind(thread)
    .bind(segment)
    .bind(machine)
    .fetch_one(conn)
    .await?)
}
