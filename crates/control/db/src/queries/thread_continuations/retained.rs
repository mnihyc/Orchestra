//! Native-independent authored history, read by immutable submission order and ordinary item anchors.

use super::reading::OWNERS;
use crate::queries::threads::{INPUT_SELECT, InputRow};
use crate::{Conn, DbError};
use orch_types::{Id, Seq};

/// The selected queue revision and whether a full sync needs to inspect retained history.
/// A positive probe still needs the selected continuation's visibility and frozen sequence bounds.
pub async fn sync_queue(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    check_retained: bool,
) -> Result<Option<(i64, bool)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.queue_revision, $3::boolean AND EXISTS (
            SELECT 1 FROM thread_inputs i WHERE i.thread_id=c.thread_id
              AND i.source='user' AND i.state='cancelled'
         ) FROM thread_continuations c WHERE c.thread_id=$1 AND c.id=$2",
    )
    .bind(thread)
    .bind(view)
    .bind(check_retained)
    .fetch_optional(conn)
    .await?)
}

/// Only keys are read to establish the next anchor; a large native item body is never decoded for this.
pub async fn next_item_seq(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    after: Option<Seq>,
) -> Result<Option<Seq>, DbError> {
    let sql = format!("{OWNERS} SELECT MIN(p.seq_created) FROM owners o JOIN LATERAL (
        SELECT i.seq_created FROM items i WHERE i.thread_id=$1 AND i.continuation_id=o.owner
          AND i.stream='main' AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND ($3::bigint IS NULL OR i.seq_created>$3)
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn FROM turns t WHERE t.id=i.turn_id))
        ORDER BY i.seq_created LIMIT 1) p ON true");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(after)
        .fetch_one(conn)
        .await?)
}

/// Validate an anchor against the current selected/frozen history, without reading its payload.
pub async fn item_seq(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    item: Id,
) -> Result<Option<Seq>, DbError> {
    let sql = format!("{OWNERS} SELECT i.seq_created FROM items i JOIN owners o ON o.owner=i.continuation_id
        WHERE i.thread_id=$1 AND i.id=$3 AND i.stream='main' AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn FROM turns t WHERE t.id=i.turn_id))");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(item)
        .fetch_optional(conn)
        .await?)
}

/// The exact preceding item for a changed input; clients never compare submission and item clocks.
pub async fn anchor(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    input: Id,
) -> Result<Option<Id>, DbError> {
    let sql = format!("{OWNERS} SELECT p.id FROM owners o JOIN LATERAL (
        SELECT i.id,i.seq_created FROM items i WHERE i.thread_id=$1 AND i.continuation_id=o.owner
          AND i.stream='main' AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND i.seq_created<(SELECT seq_created FROM thread_inputs WHERE id=$3 AND thread_id=$1)
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn FROM turns t WHERE t.id=i.turn_id))
        ORDER BY i.seq_created DESC LIMIT 1) p ON true ORDER BY p.seq_created DESC LIMIT 1");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(input)
        .fetch_optional(conn)
        .await?)
}

/// A bounded key page belongs to exactly one anchor. Cancellation after the observed cut cannot enter its walk.
#[allow(
    clippy::too_many_arguments,
    reason = "one indexed retained-input page keeps its exact history, anchor and observed cut"
)]
pub async fn group_ids(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    lower: Option<Seq>,
    upper: Option<Seq>,
    after: Option<Id>,
    at: Seq,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    let sql = format!("{OWNERS} SELECT p.id,p.seq_created FROM owners o JOIN LATERAL (
        SELECT i.id,i.seq_created FROM thread_inputs i WHERE i.thread_id=$1 AND i.continuation_id=o.owner
          AND i.source='user' AND i.state='cancelled' AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND i.seq_updated<=LEAST($7,COALESCE(o.frozen_at,$7))
          AND ($3::bigint IS NULL OR i.seq_created>$3) AND ($4::bigint IS NULL OR i.seq_created<$4)
          AND ($5::uuid IS NULL OR (i.seq_created,i.id)>(SELECT seq_created,id FROM thread_inputs WHERE id=$5 AND thread_id=$1))
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn FROM turns t WHERE t.id=i.turn_id))
        ORDER BY i.seq_created,i.id LIMIT $6) p ON true ORDER BY p.seq_created,p.id LIMIT $6");
    let rows: Vec<(Id, Seq)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(lower)
        .bind(upper)
        .bind(after)
        .bind(limit)
        .bind(at)
        .fetch_all(conn)
        .await?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// Decode one whole candidate without materializing the remainder of its anchor group.
pub async fn input(conn: &mut Conn, thread: Id, input: Id) -> Result<Option<InputRow>, DbError> {
    let sql = format!(
        "{INPUT_SELECT} WHERE thread_id=$1 AND id=$2 AND source='user' AND state='cancelled'"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(input)
        .fetch_optional(conn)
        .await?)
}
