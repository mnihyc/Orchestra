//! Bounded conversation windows and only the historical metadata their references require.

use super::{HistorySegmentRow, SEGMENT_SELECT, TURN_SELECT, TurnRow};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

/// Exact turns of this thread; a supplied foreign identifier never broadens the read.
pub async fn turns_among(conn: &mut Conn, thread: Id, ids: &[Id]) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{TURN_SELECT} WHERE thread_id=$1 AND id=ANY($2) ORDER BY ordinal"
    )))
    .bind(thread)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Current work, the retained recent-ended allowance and exact turns referenced by the visible window.
pub async fn window_turns(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
    current: Option<Id>,
    ended: i64,
) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("{TURN_SELECT} WHERE thread_id=$1 AND
        (id=ANY($2) OR id=$3 OR id IN(SELECT id FROM turns WHERE thread_id=$1
            AND state NOT IN ('starting','running') ORDER BY ordinal DESC LIMIT $4)) ORDER BY ordinal")))
        .bind(thread).bind(ids).bind(current).bind(ended).fetch_all(conn).await?)
}

/// Exact segments needed for a window/changed row set; no full-history provider lookup.
pub async fn segments_among(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
) -> Result<Vec<HistorySegmentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SEGMENT_SELECT} WHERE thread_id=$1 AND id=ANY($2) ORDER BY ordinal"
    )))
    .bind(thread)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Explicit older segment page, ascending, with a limit+1 caller-owned continuation probe.
pub async fn older_segments(
    conn: &mut Conn,
    thread: Id,
    before: Option<i32>,
    limit: i64,
) -> Result<Vec<HistorySegmentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM ({SEGMENT_SELECT} WHERE thread_id=$1
        AND ($2::int IS NULL OR ordinal<$2) ORDER BY ordinal DESC LIMIT $3) page ORDER BY ordinal"
    )))
    .bind(thread)
    .bind(before)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Explicit older turn page, including turns with no visible item; nothing is silently unreachable.
pub async fn older_turns(
    conn: &mut Conn,
    thread: Id,
    before: Option<i32>,
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM ({TURN_SELECT} WHERE thread_id=$1
        AND ($2::int IS NULL OR ordinal<$2) ORDER BY ordinal DESC LIMIT $3) page ORDER BY ordinal"
    )))
    .bind(thread)
    .bind(before)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Only the fields needed to render one outline point; command/output/reasoning bodies are never read.
#[derive(Debug, sqlx::FromRow)]
pub struct OutlinePoint {
    /// Exact timeline anchor.
    pub id: Id,
    /// Permanent creation sequence.
    pub seq_created: Seq,
    /// Display time.
    pub created_at: DateTime<Utc>,
    /// Native item kind.
    pub kind: String,
    /// A title-only payload projection. User text retains its native context-stripping extractor.
    pub payload: Json,
    /// The actual turn's ordinal; never obtained from an all-turn scan.
    pub turn_ordinal: Option<i32>,
    /// This is the turn's first non-marker main item, independently of page boundaries.
    pub starts_turn: bool,
}

/// One bounded direction of outline points. A before cursor reads latest-first, then returns ascending.
pub async fn outline_points(
    conn: &mut Conn,
    thread: Id,
    boundary: Option<Seq>,
    forward: bool,
    limit: i64,
    title_chars: i64,
) -> Result<Vec<OutlinePoint>, DbError> {
    let view: Id =
        sqlx::query_scalar("SELECT active_continuation_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(&mut *conn)
            .await?;
    outline_points_view(conn, thread, view, boundary, forward, limit, title_chars).await
}

/// Title-only reads use the same immutable membership/body as the timeline, without materializing it.
pub async fn outline_points_view(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    boundary: Option<Seq>,
    forward: bool,
    limit: i64,
    title_chars: i64,
) -> Result<Vec<OutlinePoint>, DbError> {
    let order = if forward { "ASC" } else { "DESC" };
    let comparison = if forward { ">" } else { "<" };
    let owners = crate::queries::thread_continuations::reading::OWNERS;
    // The page is chosen in each owner's own order, as `thread_continuations::reading::items` chooses one, so that
    // a long history is read from its end until a page is found: ordered across the owners' join, every item of the
    // thread was read and tested to keep the newest fifty (a thread of 10,000 turns read 205,000 blocks to open).
    let query = format!("{owners},visible AS NOT MATERIALIZED (
      SELECT i.id,i.thread_id,i.turn_id,i.stream,i.kind,i.seq_created,i.created_at,
        CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.payload ELSE
          (SELECT r.body->'payload' FROM thread_item_revisions r WHERE r.item_id=i.id
            AND r.seq_updated<=o.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END AS payload
      FROM owners o JOIN items i ON i.continuation_id=o.owner AND i.thread_id=$1
      WHERE i.seq_created BETWEEN o.first_seq AND o.last_seq AND (i.turn_id IS NULL OR EXISTS(
        SELECT 1 FROM turns t WHERE t.id=i.turn_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn))),candidates AS (
      SELECT selected.* FROM owners o JOIN LATERAL (
        SELECT i.id,i.seq_created,i.created_at,i.kind,i.turn_id,
          (i.turn_id IS NOT NULL AND i.kind<>'marker' AND NOT EXISTS(SELECT 1 FROM items prior
            WHERE prior.thread_id=i.thread_id AND prior.turn_id=i.turn_id AND prior.stream='main'
              AND prior.kind<>'marker' AND prior.seq_created<i.seq_created)) AS starts_turn
        FROM items i CROSS JOIN LATERAL (SELECT CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.payload ELSE
            (SELECT r.body->'payload' FROM thread_item_revisions r WHERE r.item_id=i.id
              AND r.seq_updated<=o.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END AS payload) v
        WHERE i.thread_id=$1 AND i.continuation_id=o.owner AND i.stream='main'
          AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND ($3::bigint IS NULL OR i.seq_created {comparison} $3)
          AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn
            FROM turns t WHERE t.id=i.turn_id))
          AND (i.kind IN ('user_message','agent_message','approval')
            OR (i.kind='announcement' AND v.payload->>'category'='milestone')
            OR (i.kind='tool_call' AND v.payload #>> '{{tool,kind}}'='orch'
              AND v.payload #>> '{{tool,tool}}'='announce' AND v.payload #>> '{{tool,arguments,category}}'='milestone')
            OR (i.turn_id IS NOT NULL AND i.kind<>'marker' AND NOT EXISTS(SELECT 1 FROM items prior
              WHERE prior.thread_id=i.thread_id AND prior.turn_id=i.turn_id AND prior.stream='main'
                AND prior.kind<>'marker' AND prior.seq_created<i.seq_created)))
        ORDER BY i.seq_created {order} LIMIT $4) selected ON true
      ORDER BY selected.seq_created {order} LIMIT $4)
      SELECT c.id,c.seq_created,c.created_at,c.kind,c.starts_turn,t.ordinal AS turn_ordinal,
        CASE c.kind
          WHEN 'user_message' THEN jsonb_build_object('source',i.payload->'source',
            'goal_condition',left(i.payload #>> '{{goal,condition}}',$5::int),
            'parts',CASE WHEN i.payload->>'source'='goal' THEN '[]'::jsonb ELSE COALESCE((SELECT jsonb_agg(
              jsonb_build_object('type',p.part->'type','text',p.part->'text','name',p.part->'name') ORDER BY p.ordinal)
              FROM jsonb_array_elements(CASE WHEN jsonb_typeof(i.payload->'parts')='array' THEN i.payload->'parts' ELSE '[]'::jsonb END)
                WITH ORDINALITY p(part,ordinal)),'[]'::jsonb) END)
          WHEN 'agent_message' THEN jsonb_build_object('text',(SELECT left(regexp_replace(line,'^[[:space:]]+|[[:space:]]+$','','g'),$5::int)
            FROM regexp_split_to_table(COALESCE(i.payload->>'text',''),E'\\n') WITH ORDINALITY lines(line,ordinal)
            WHERE line ~ '[^[:space:]]' ORDER BY ordinal LIMIT 1))
          WHEN 'announcement' THEN jsonb_build_object('title',left(i.payload->>'title',$5::int),'category',i.payload->'category')
          WHEN 'approval' THEN jsonb_build_object('approval_id',i.payload->'approval_id')
          WHEN 'tool_call' THEN CASE WHEN i.payload #>> '{{tool,kind}}'='orch'
            AND i.payload #>> '{{tool,tool}}'='announce' AND i.payload #>> '{{tool,arguments,category}}'='milestone'
            THEN jsonb_build_object('tool',jsonb_build_object('tool','announce','arguments',
              jsonb_build_object('category','milestone','title',left(i.payload #>> '{{tool,arguments,title}}',$5::int))))
            ELSE '{{}}'::jsonb END
          ELSE '{{}}'::jsonb END AS payload
      FROM candidates c JOIN visible i ON i.id=c.id LEFT JOIN turns t ON t.id=c.turn_id AND t.thread_id=$1
      ORDER BY c.seq_created");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(query))
        .bind(thread)
        .bind(view)
        .bind(boundary)
        .bind(limit)
        .bind(title_chars)
        .fetch_all(conn)
        .await?)
}

/// An anchor's position without loading its potentially large payload.
pub async fn item_position(conn: &mut Conn, thread: Id, item: Id) -> Result<Option<Seq>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT seq_created FROM items WHERE thread_id=$1 AND id=$2")
            .bind(thread)
            .bind(item)
            .fetch_optional(conn)
            .await?,
    )
}
