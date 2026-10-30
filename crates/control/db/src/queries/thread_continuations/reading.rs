//! Page identities first; decode frozen bodies only for the bounded returned window.

use crate::queries::threads::{
    HistorySegmentRow, INPUT_SELECT, InputRow, ItemRow, SEGMENT_SELECT, TURN_SELECT, TurnRow,
};
use crate::{Conn, DbError};
use orch_types::{Id, Seq};

/// Each owner occurs once in a flattened prefix; no recursive ancestor lookup is needed per page.
pub(crate) const OWNERS:&str="WITH owners AS (
    SELECT c.id AS owner,1 AS first_turn,2147483647 AS last_turn,0::bigint AS first_seq,
      9223372036854775807::bigint AS last_seq,NULL::bigint AS frozen_at
    FROM thread_continuations c WHERE c.thread_id=$1 AND c.id=$2
    UNION ALL SELECT s.owner_continuation_id,s.first_turn,s.last_turn,s.first_seq,s.last_seq,s.frozen_at
    FROM thread_continuation_spans s JOIN thread_continuations c ON c.id=s.continuation_id
    WHERE c.thread_id=$1 AND c.id=$2)";

/// One indexed ownership batch for the selected histories of the requested threads.
pub(crate) const SELECTED_OWNERS:&str="WITH owners AS (
    SELECT t.node_id AS thread_id,t.active_continuation_id AS owner,1 AS first_turn,
      2147483647 AS last_turn,0::bigint AS first_seq,9223372036854775807::bigint AS last_seq,
      NULL::bigint AS frozen_at FROM threads t WHERE t.node_id=ANY($1)
    UNION ALL SELECT t.node_id,s.owner_continuation_id,s.first_turn,s.last_turn,s.first_seq,s.last_seq,s.frozen_at
      FROM threads t JOIN thread_continuation_spans s ON s.continuation_id=t.active_continuation_id
      WHERE t.node_id=ANY($1))";

/// Structural identity and stream are immutable once referenced. Pin/quote metadata stays shared. The record is
/// made once for each row: `(f(...)).*` would make it again for every column of an item, two dozen times a row.
pub(crate) const DECODE: &str = "SELECT d.* FROM candidates c JOIN items i ON i.id=c.id
    CROSS JOIN LATERAL jsonb_populate_record(NULL::items,
    CASE WHEN c.frozen_at IS NULL OR i.seq_updated<=c.frozen_at THEN to_jsonb(i)
      ELSE (SELECT r.body FROM thread_item_revisions r WHERE r.item_id=i.id
        AND r.seq_updated<=c.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END
    || jsonb_build_object('pinned_at',i.pinned_at,'pinned_by',i.pinned_by,'chat_refs',i.chat_refs,
        'seq_updated',i.seq_updated,'updated_at',i.updated_at)) d
    ORDER BY i.seq_created";

/// before/after use permanent sequence positions; sparse global order is not renumbered.
///
/// Whether an item's turn belongs to the owner's turns is asked of the turn by its key, for each item of the page, and
/// not as `EXISTS (...)`: the planner turns that into a membership test against the identifiers of all the turns in the
/// owner's range, hashed afresh for every page, which reads every turn of a long thread to give 500 items.
pub async fn items(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    boundary: Option<Seq>,
    forward: bool,
    stream: Option<&str>,
    limit: i64,
) -> Result<Vec<ItemRow>, DbError> {
    let order = if forward { "ASC" } else { "DESC" };
    let comparison = if forward { ">" } else { "<" };
    let sql=format!("{OWNERS},candidates AS (
      SELECT selected.id,o.frozen_at,selected.seq_created FROM owners o JOIN LATERAL (
        SELECT i.id,i.seq_created FROM items i WHERE i.thread_id=$1 AND i.continuation_id=o.owner
          AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND ($3::bigint IS NULL OR i.seq_created {comparison} $3) AND ($4::text IS NULL OR i.stream=$4)
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn
            FROM turns t WHERE t.id=i.turn_id))
        ORDER BY i.seq_created {order} LIMIT $5) selected ON true
      ORDER BY selected.seq_created {order} LIMIT $5) {DECODE}");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(boundary)
        .bind(stream)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Membership is checked before accepting a page anchor or returning its frozen body.
pub async fn item(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    item: Id,
) -> Result<Option<ItemRow>, DbError> {
    let sql = format!(
        "{OWNERS},candidates AS (SELECT i.id,o.frozen_at FROM owners o JOIN items i
      ON i.continuation_id=o.owner AND i.thread_id=$1 WHERE i.id=$3
      AND i.seq_created BETWEEN o.first_seq AND o.last_seq
      AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn))) {DECODE}"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(item)
        .fetch_optional(conn)
        .await?)
}

/// Explicit deep links can reveal their real owning version under ordinary thread authority.
pub async fn item_owner(conn: &mut Conn, thread: Id, item: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT continuation_id FROM items WHERE thread_id=$1 AND id=$2")
            .bind(thread)
            .bind(item)
            .fetch_optional(conn)
            .await?,
    )
}

/// Frozen inherited bodies never accept live append deltas from their former source.
pub async fn mutable_items_among(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    ids: &[Id],
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM items WHERE thread_id=$1 AND continuation_id=$2 AND id=ANY($3)",
    )
    .bind(thread)
    .bind(view)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

pub async fn contains_input(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    input: Id,
) -> Result<bool, DbError> {
    let sql=format!("{OWNERS} SELECT EXISTS(SELECT 1 FROM thread_inputs i JOIN owners o ON o.owner=i.continuation_id
        WHERE i.thread_id=$1 AND i.id=$3 AND (i.continuation_id=$2 OR EXISTS(SELECT 1 FROM turns t
          WHERE t.id=i.turn_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn)))");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(input)
        .fetch_one(conn)
        .await?)
}

/// Pending work belongs only to its own suffix; inherited history never imports another queue.
pub async fn open_inputs(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<InputRow>, DbError> {
    let sql=format!("{INPUT_SELECT} WHERE thread_id=$1 AND continuation_id=$2
        AND state IN ('pending','delivering','failed') AND ($3::uuid IS NULL OR (position,created_at,id)>
          (SELECT position,created_at,id FROM thread_inputs WHERE id=$3 AND thread_id=$1 AND continuation_id=$2))
        ORDER BY position,created_at,id LIMIT $4");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Queue-page metadata reads only bounded indexed identities, never another copy of its bodies.
pub async fn queue_next_after(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    limit: u32,
) -> Result<Option<Id>, DbError> {
    let ids: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM thread_inputs WHERE thread_id=$1 AND continuation_id=$2
      AND state IN ('pending','delivering','failed') ORDER BY position,created_at,id LIMIT $3",
    )
    .bind(thread)
    .bind(view)
    .bind(i64::from(limit) + 1)
    .fetch_all(conn)
    .await?;
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    Ok(if ids.len() > limit {
        ids.get(limit.saturating_sub(1)).copied()
    } else {
        None
    })
}

/// Caller first bounds the whole changed-row set; this limit never silently acknowledges omitted rows.
pub async fn changed_items(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    since: Seq,
    limit: i64,
) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "{OWNERS},candidates AS (
      SELECT selected.id,o.frozen_at,selected.seq_updated FROM owners o JOIN LATERAL (
        SELECT i.id,i.seq_updated FROM items i WHERE i.thread_id=$1 AND i.continuation_id=o.owner
          AND i.seq_created BETWEEN o.first_seq AND o.last_seq AND i.seq_updated>$3
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn
            FROM turns t WHERE t.id=i.turn_id))
        ORDER BY i.seq_updated,i.id LIMIT $4) selected ON true
      ORDER BY selected.seq_updated,selected.id LIMIT $4) {DECODE}"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(since)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Version counts may change without changing immutable prompt bodies; return only bounded visible IDs.
pub async fn changed_prompt_inputs(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    since: Seq,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    let sql = format!(
        "{OWNERS} SELECT DISTINCT i.id FROM thread_prompt_groups g
      JOIN thread_prompt_versions v ON v.group_id=g.id JOIN thread_inputs i ON i.id=v.input_id
      JOIN owners o ON o.owner=i.continuation_id WHERE g.thread_id=$1 AND g.seq_updated>$3
      AND (i.continuation_id=$2 OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn)) ORDER BY i.id LIMIT $4"
    );
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(since)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Indexed permanent-ordinal pages, including turns with no visible item.
pub async fn turns(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    before: Option<i32>,
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    let sql = format!(
        "{OWNERS}, chosen AS (SELECT p.id,p.ordinal FROM owners o JOIN LATERAL (
      SELECT t.id,t.ordinal FROM turns t WHERE t.thread_id=$1 AND t.continuation_id=o.owner
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn AND ($3::int IS NULL OR t.ordinal<$3)
      ORDER BY t.ordinal DESC LIMIT $4) p ON true ORDER BY p.ordinal DESC LIMIT $4)
      {TURN_SELECT} WHERE id IN(SELECT id FROM chosen) ORDER BY ordinal"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

pub async fn turns_among(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    ids: &[Id],
) -> Result<Vec<TurnRow>, DbError> {
    let sql=format!("{OWNERS} {TURN_SELECT} WHERE thread_id=$1 AND id=ANY($3) AND EXISTS(
      SELECT 1 FROM owners o WHERE o.owner=turns.continuation_id AND turns.ordinal BETWEEN o.first_turn AND o.last_turn)
      ORDER BY ordinal");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

pub async fn window_turns(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    ids: &[Id],
    current: Option<Id>,
    ended: i64,
) -> Result<Vec<TurnRow>, DbError> {
    let mut rows = turns(conn, thread, view, None, ended).await?;
    let wanted: Vec<_> = ids
        .iter()
        .copied()
        .chain(current)
        .filter(|id| !rows.iter().any(|row| row.id == *id))
        .collect();
    rows.extend(turns_among(conn, thread, view, &wanted).await?);
    rows.sort_by_key(|row| row.ordinal);
    Ok(rows)
}

pub async fn segments_among(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    ids: &[Id],
) -> Result<Vec<HistorySegmentRow>, DbError> {
    let sql=format!("{OWNERS} {SEGMENT_SELECT} WHERE thread_id=$1 AND id=ANY($3) AND EXISTS(
      SELECT 1 FROM owners o WHERE o.owner=segments.continuation_id AND (o.frozen_at IS NULL
        OR EXISTS(SELECT 1 FROM turns t WHERE t.segment_id=segments.id AND t.ordinal BETWEEN o.first_turn AND o.last_turn)
        OR EXISTS(SELECT 1 FROM items i WHERE i.segment_id=segments.id AND i.turn_id IS NULL
          AND i.seq_created BETWEEN o.first_seq AND o.last_seq))) ORDER BY ordinal");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

pub async fn segments(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    before: Option<i32>,
    limit: i64,
) -> Result<Vec<HistorySegmentRow>, DbError> {
    let sql=format!("{OWNERS},chosen AS(SELECT p.id,p.ordinal FROM owners o JOIN LATERAL (
      SELECT s.id,s.ordinal FROM segments s WHERE s.thread_id=$1 AND s.continuation_id=o.owner
        AND ($3::int IS NULL OR s.ordinal<$3) AND (o.frozen_at IS NULL OR EXISTS(
          SELECT 1 FROM turns t WHERE t.segment_id=s.id AND t.ordinal BETWEEN o.first_turn AND o.last_turn)
          OR EXISTS(SELECT 1 FROM items i WHERE i.segment_id=s.id AND i.turn_id IS NULL
            AND i.seq_created BETWEEN o.first_seq AND o.last_seq))
      ORDER BY s.ordinal DESC LIMIT $4) p ON true ORDER BY p.ordinal DESC LIMIT $4)
      {SEGMENT_SELECT} WHERE id IN(SELECT id FROM chosen) ORDER BY ordinal");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Read IDs for changes first. A limit+1 result forces reset rather than acknowledging a truncated cut.
pub async fn changed_turns(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    since: Seq,
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    let sql=format!("{OWNERS},chosen AS(SELECT p.id,p.seq_updated FROM owners o JOIN LATERAL (
      SELECT t.id,t.seq_updated FROM turns t WHERE t.thread_id=$1 AND t.continuation_id=o.owner
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn AND t.seq_updated>$3 ORDER BY t.seq_updated,t.id LIMIT $4
      ) p ON true ORDER BY p.seq_updated,p.id LIMIT $4)
      {TURN_SELECT} WHERE id IN(SELECT id FROM chosen) ORDER BY ordinal");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(since)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

pub async fn changed_segments(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    since: Seq,
    limit: i64,
) -> Result<Vec<HistorySegmentRow>, DbError> {
    let sql=format!("{OWNERS} SELECT s.id FROM owners o JOIN segments s ON s.continuation_id=o.owner
      WHERE s.thread_id=$1 AND s.seq_updated>$3 AND (o.frozen_at IS NULL OR EXISTS(
        SELECT 1 FROM turns t WHERE t.segment_id=s.id AND t.ordinal BETWEEN o.first_turn AND o.last_turn)
        OR EXISTS(SELECT 1 FROM items i WHERE i.segment_id=s.id AND i.turn_id IS NULL
          AND i.seq_created BETWEEN o.first_seq AND o.last_seq))
      ORDER BY s.seq_updated,s.id LIMIT $4");
    let ids: Vec<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(since)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await?;
    segments_among(conn, thread, view, &ids).await
}

pub async fn changed_inputs(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    since: Seq,
    limit: i64,
) -> Result<Vec<InputRow>, DbError> {
    // Delivered prefix inputs are represented by item summaries, never loaded wholesale on opening.
    let sql = format!(
        "{INPUT_SELECT} WHERE thread_id=$1 AND continuation_id=$2 AND seq_updated>$3
      ORDER BY seq_updated,id LIMIT $4"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(since)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

pub async fn first_item_of_turn(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    ordinal: i64,
) -> Result<Option<(Id, Seq)>, DbError> {
    let sql=format!("{OWNERS} SELECT i.id,i.seq_created FROM owners o JOIN turns t ON t.continuation_id=o.owner
      JOIN items i ON i.turn_id=t.id AND i.stream='main' WHERE t.thread_id=$1 AND t.ordinal::bigint=$3
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn AND i.seq_created BETWEEN o.first_seq AND o.last_seq
      ORDER BY i.seq_created LIMIT 1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(ordinal)
        .fetch_optional(conn)
        .await?)
}

pub async fn last_read(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    view: Id,
) -> Result<Option<Seq>, DbError> {
    Ok(sqlx::query_scalar("SELECT COALESCE(r.last_read_seq,0)::bigint FROM thread_continuations c
      LEFT JOIN thread_reads r ON r.thread_id=c.thread_id AND r.continuation_id=c.id AND r.user_id=$1
      WHERE c.thread_id=$2 AND c.id=$3").bind(user).bind(thread).bind(view).fetch_optional(conn).await?)
}
