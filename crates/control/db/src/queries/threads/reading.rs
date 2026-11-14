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
    /// The immutable author identity, resolved in one batch by the API.
    pub author_id: Option<Id>,
    /// Its source turn, independent of which turns the browser holds.
    pub turn_id: Option<Id>,
    /// Bounded answer/change/child facts; never raw command or output bodies.
    pub metadata: Json,
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
    outline_selected_points_view(
        conn,
        thread,
        view,
        OutlineWindow {
            boundary,
            forward,
            limit,
            title_chars,
            selection: "conversation",
            q: None,
            changes: false,
        },
    )
    .await
}

/// One bounded metadata neighborhood or page, with source-owned selection and literal search.
#[derive(Debug, Clone, Copy)]
pub struct OutlineWindow<'a> {
    /// The exclusive creation cursor, when supplied.
    pub boundary: Option<Seq>,
    /// Reads after rather than before that cursor.
    pub forward: bool,
    /// Caller-selected source-point count.
    pub limit: i64,
    /// The existing outline-title budget.
    pub title_chars: i64,
    /// Conversation points, human messages, or child agents.
    pub selection: &'a str,
    /// A literal substring; no item bodies are returned for search.
    pub q: Option<&'a str>,
    /// Project optional admitted-change counts only when the caller displays them.
    pub changes: bool,
}

/// Page within each immutable owner before merging, so old history and large outputs are not scanned to open a list.
pub async fn outline_selected_points_view(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    window: OutlineWindow<'_>,
) -> Result<Vec<OutlinePoint>, DbError> {
    let kind = match window.selection {
        "messages" => "i.kind='user_message' AND i.payload->>'source'='user' AND v.payload->>'source'='user'",
        "agents" => "i.kind='tool_call' AND i.payload #>> '{tool,kind}'='subagent' AND v.payload #>> '{tool,kind}'='subagent'",
        _ => "i.kind IN ('user_message','agent_message','approval')
            OR (i.kind='announcement' AND v.payload->>'category'='milestone')
            OR (i.kind='tool_call' AND v.payload #>> '{tool,kind}'='orch' AND v.payload #>> '{tool,tool}'='announce'
              AND v.payload #>> '{tool,arguments,category}'='milestone')
            OR (i.turn_id IS NOT NULL AND i.kind<>'marker' AND NOT EXISTS(SELECT 1 FROM items prior
              WHERE prior.thread_id=i.thread_id AND prior.turn_id=i.turn_id AND prior.stream='main'
                AND prior.kind<>'marker' AND prior.seq_created<i.seq_created))",
    };
    let query = include_str!("outline_points.sql")
        .replace(
            "__OWNERS__",
            crate::queries::thread_continuations::reading::OWNERS,
        )
        .replace("__ORDER__", if window.forward { "ASC" } else { "DESC" })
        .replace("__COMPARISON__", if window.forward { ">" } else { "<" })
        .replace("__KIND_FILTER__", kind)
        .replace(
            "__STREAM_FILTER__",
            if window.selection == "agents" {
                "true"
            } else {
                "i.stream='main'"
            },
        )
        .replace(
            "__MESSAGE_FACTS__",
            if window.selection == "agents" {
                "false"
            } else {
                "true"
            },
        )
        .replace(
            "__ANSWER_FACTS__",
            if window.selection == "conversation" {
                "true"
            } else {
                "false"
            },
        )
        .replace(
            "__COUNT_EDITS__",
            if window.selection == "messages" && window.changes {
                "true"
            } else {
                "false"
            },
        );
    let pattern = window.q.map(crate::queries::search::substring);
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(query))
        .bind(thread)
        .bind(view)
        .bind(window.boundary)
        .bind(window.limit)
        .bind(window.title_chars)
        .bind(pattern)
        .fetch_all(conn)
        .await?)
}

/// The complete child count uses the same owner and frozen-body membership, without reading child prompts.
pub async fn outline_agent_count_view(
    conn: &mut Conn,
    thread: Id,
    view: Id,
) -> Result<i64, DbError> {
    let owners = crate::queries::thread_continuations::reading::OWNERS;
    let query = format!("{owners} SELECT count(*) FROM owners o JOIN items i ON i.thread_id=$1 AND i.continuation_id=o.owner
        WHERE i.kind='tool_call' AND i.payload #>> '{{tool,kind}}'='subagent'
        AND i.seq_created BETWEEN o.first_seq AND o.last_seq
        AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn))
        AND (CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.payload ELSE
          (SELECT r.body->'payload' FROM thread_item_revisions r WHERE r.item_id=i.id AND r.seq_updated<=o.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END) #>> '{{tool,kind}}'='subagent'");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .bind(thread)
        .bind(view)
        .fetch_one(conn)
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
