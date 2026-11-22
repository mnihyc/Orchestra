//! Native-independent authored history, read by immutable submission order and ordinary item anchors.

use super::reading::OWNERS;
use crate::queries::threads::{INPUT_SELECT, InputRow};
use crate::{Conn, DbError};
use orch_types::{Id, Seq};

const NEXT_MAIN: &str = "SELECT MIN(p.seq_created) AS upper_seq FROM owners o JOIN LATERAL (
        SELECT i.seq_created FROM items i WHERE i.thread_id=$1 AND i.continuation_id=o.owner
          AND i.stream='main' AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND (a.lower_seq IS NULL OR i.seq_created>a.lower_seq)
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn FROM turns t WHERE t.id=i.turn_id))
        ORDER BY i.seq_created LIMIT 1) p ON true";

/// Only keys are read to establish the next anchor; a large native item body is never decoded for this.
pub async fn next_item_seq(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    after: Option<Seq>,
) -> Result<Option<Seq>, DbError> {
    let sql = format!(
        "{OWNERS} SELECT n.upper_seq FROM (SELECT $3::bigint AS lower_seq) a
        CROSS JOIN LATERAL ({NEXT_MAIN}) n"
    );
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

// Each selected owner supplies only its earliest eligible keys, including its frozen cut.
const GROUP_KEYS: &str = "SELECT p.id,p.seq_created FROM owners o JOIN LATERAL (
        SELECT i.id,i.seq_created FROM thread_inputs i WHERE i.thread_id=$1 AND i.continuation_id=o.owner
          AND i.source='user' AND i.state='cancelled' AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND i.seq_updated<=LEAST($3,COALESCE(o.frozen_at,$3))
          AND (a.lower_seq IS NULL OR i.seq_created>a.lower_seq) AND (n.upper_seq IS NULL OR i.seq_created<n.upper_seq)
          AND ($5::uuid IS NULL OR (i.seq_created,i.id)>(SELECT seq_created,id FROM thread_inputs WHERE id=$5 AND thread_id=$1))
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn FROM turns t WHERE t.id=i.turn_id))
        ORDER BY i.seq_created,i.id LIMIT $4) p ON true ORDER BY p.seq_created,p.id LIMIT $4";

/// Get a bounded prefix of each occupied group beside the main items the page already read.
/// The next main item can be outside this page. Its sequence still closes the interval.
pub async fn window_keys(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    at: Seq,
    lower: &[Option<Seq>],
    key_limit: i64,
) -> Result<std::collections::BTreeMap<Option<Seq>, GroupKeys>, DbError> {
    if super::opening_history(conn, thread, view)
        .is_some_and(|history| !history.has_retained_inputs)
    {
        return Ok(std::collections::BTreeMap::new());
    }
    let sql = format!("{OWNERS} SELECT a.lower_seq,n.upper_seq,p.id
        FROM unnest($6::bigint[]) a(lower_seq)
        CROSS JOIN LATERAL ({NEXT_MAIN}) n CROSS JOIN LATERAL ({GROUP_KEYS}) p
        WHERE EXISTS (SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND source='user' AND state='cancelled')
        ORDER BY a.lower_seq NULLS FIRST,p.seq_created,p.id");
    let rows: Vec<(Option<Seq>, Option<Seq>, Id)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(at)
        .bind(key_limit)
        .bind(None::<Id>)
        .bind(lower)
        .fetch_all(conn)
        .await?;
    let mut groups = std::collections::BTreeMap::new();
    for (lower, upper, id) in rows {
        groups
            .entry(lower)
            .or_insert_with(|| GroupKeys {
                upper,
                ids: Vec::new(),
            })
            .ids
            .push(id);
    }
    Ok(groups)
}

pub struct GroupKeys {
    pub upper: Option<Seq>,
    pub ids: Vec<Id>,
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
    let sql = format!(
        "{OWNERS} SELECT p.id FROM (SELECT $6::bigint AS lower_seq) a
        CROSS JOIN (SELECT $7::bigint AS upper_seq) n CROSS JOIN LATERAL ({GROUP_KEYS}) p
        ORDER BY p.seq_created,p.id"
    );
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(at)
        .bind(limit)
        .bind(after)
        .bind(lower)
        .bind(upper)
        .fetch_all(conn)
        .await?)
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
