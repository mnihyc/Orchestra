//! Selected history aggregates and export pages share immutable-prefix membership and frozen item bodies.

use crate::queries::thread_continuations::reading::{self, OWNERS};
use crate::queries::threads::{
    CatchupCounts, HistorySegmentRow, ItemRow, SEGMENT_SELECT, TURN_SELECT, TurnRow,
};
use crate::{Conn, DbError};
use orch_types::{Id, Json, Seq};
use std::collections::HashMap;

/// Decode no more than the ordinary largest timeline page at once, including full exports.
pub const PAGE: i64 = 500;

/// The current default read view. Explicit retained readers pass their captured view instead.
pub async fn active_view(conn: &mut Conn, thread: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT active_continuation_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_optional(conn)
            .await?,
    )
}

/// A late report may update its own history, but only this lane drives thread-wide state.
pub async fn selected_segment(conn: &mut Conn, thread: Id, segment: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads t JOIN segments s ON s.id=$2 AND s.thread_id=t.node_id
      WHERE t.node_id=$1 AND t.current_segment_id=s.id AND t.active_continuation_id=s.continuation_id)")
        .bind(thread).bind(segment).fetch_one(conn).await?)
}

/// Only the latest actual turn of the selected lane supplies its current context totals.
pub async fn selected_turn(conn: &mut Conn, thread: Id, turn: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads h JOIN turns t ON t.id=$2 AND t.thread_id=h.node_id
      WHERE h.node_id=$1 AND h.active_continuation_id=t.continuation_id AND h.current_segment_id=t.segment_id
        AND (h.current_turn_id=t.id OR (h.current_turn_id IS NULL AND NOT EXISTS(SELECT 1 FROM turns newer
          WHERE newer.thread_id=h.node_id AND newer.continuation_id=h.active_continuation_id AND newer.ordinal>t.ordinal))))")
        .bind(thread).bind(turn).fetch_one(conn).await?)
}

/// The last actual turn owned by the selected suffix, independently of newer inactive ordinals.
pub async fn latest_selected_turn(conn: &mut Conn, thread: Id) -> Result<Option<TurnRow>, DbError> {
    let sql = format!(
        "{TURN_SELECT} WHERE thread_id=$1 AND continuation_id=(SELECT active_continuation_id
      FROM threads WHERE node_id=$1) ORDER BY ordinal DESC LIMIT 1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_optional(conn)
        .await?)
}

/// Retained context observations of one selected conversation, distinct from lifetime accounting.
#[derive(sqlx::FromRow)]
pub struct ContextHistory {
    /// Actual turn that supplied the most recent selected context report.
    pub turn_id: Option<Id>,
    /// Unmodified report kept with that actual turn.
    pub context: Option<Json>,
    /// Latest selected native shortening report, if present.
    pub shortenings: Option<Json>,
    /// Whether the most recent selected detailed usage report supplied a breakdown.
    pub breakdown: bool,
}

/// One metadata batch; no excluded suffix can become the current context because its ordinal is newer.
pub async fn context_history(
    conn: &mut Conn,
    thread: Id,
    view: Id,
) -> Result<ContextHistory, DbError> {
    let sql=format!("{OWNERS},chosen AS NOT MATERIALIZED (
      SELECT t.* FROM turns t WHERE t.thread_id=$1 AND EXISTS(SELECT 1 FROM owners o
        WHERE o.owner=t.continuation_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn))
      SELECT fill.id AS turn_id,fill.context,shortening.value AS shortenings,COALESCE(detail.breakdown,false) AS breakdown
      FROM (VALUES(1)) seed(n)
      LEFT JOIN LATERAL(SELECT id,context FROM chosen WHERE context IS NOT NULL ORDER BY ordinal DESC LIMIT 1) fill ON true
      LEFT JOIN LATERAL(SELECT usage_detail->'shortenings' AS value FROM chosen
        WHERE jsonb_typeof(usage_detail->'shortenings')='array' AND jsonb_array_length(usage_detail->'shortenings')>0
        ORDER BY ordinal DESC LIMIT 1) shortening ON true
      LEFT JOIN LATERAL(SELECT COALESCE((usage_detail->>'breakdown')::boolean,false) AS breakdown FROM chosen
        WHERE usage_detail IS NOT NULL ORDER BY ordinal DESC LIMIT 1) detail ON true");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .fetch_one(conn)
        .await?)
}

/// Summarizers consume a bounded window of the same frozen bodies a current reader sees.
pub type SummaryActivity = (String, Json, chrono::DateTime<chrono::Utc>, Seq);

/// The newest matching activity rows; callers reverse them for chronological presentation.
pub async fn summary_activity(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    since: Seq,
    limit: i64,
) -> Result<Vec<SummaryActivity>, DbError> {
    let sql = format!(
        "{OWNERS}{FROZEN} SELECT kind,body->'payload',(body->>'created_at')::timestamptz,
        (body->>'seq_updated')::bigint FROM visible_items WHERE stream='main'
        AND (body IS NULL OR ((body->>'seq_updated')::bigint>$3
          AND kind IN ('user_message','agent_message','reasoning','plan','tool_call','error')))
        ORDER BY seq_created DESC LIMIT $4"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(since)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The initial selected user message, including its retained body when inherited.
pub async fn first_prompt(conn: &mut Conn, thread: Id, view: Id) -> Result<Option<Json>, DbError> {
    let sql = format!(
        "{OWNERS}{FROZEN} SELECT body->'payload' FROM visible_items
      WHERE kind='user_message' AND stream='main' ORDER BY seq_created LIMIT 1"
    );
    // A NULL frozen body is damage rather than an empty prompt.
    let row: Option<(Option<Json>,)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .fetch_optional(conn)
        .await?;
    row.map(|(payload,)| payload.ok_or_else(damaged))
        .transpose()
}

/// Aggregates and title lookups need only the frozen JSON, not a second item decoder.
/// A retained revision is required; a damaged prefix never falls back to its newer source body.
const FROZEN: &str = ",visible_items AS NOT MATERIALIZED (
    SELECT i.id,i.kind,i.stream,i.seq_created,
      CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN to_jsonb(i)
        ELSE (SELECT r.body FROM thread_item_revisions r WHERE r.item_id=i.id
          AND r.seq_updated<=o.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END AS body
    FROM owners o JOIN items i ON i.thread_id=$1 AND i.continuation_id=o.owner
    WHERE i.seq_created BETWEEN o.first_seq AND o.last_seq
      AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn)))";

pub(crate) fn damaged() -> DbError {
    sqlx::Error::Decode(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "A retained item revision is missing from this history.",
    )))
    .into()
}

/// Aggregate readers share the same selected membership and frozen payload as returned items.
pub(crate) const SELECTED_ITEMS: &str = ",visible_items AS NOT MATERIALIZED (
    SELECT i.thread_id,i.kind,i.stream,i.created_at,i.seq_created,
      CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN to_jsonb(i)
        ELSE (SELECT r.body FROM thread_item_revisions r WHERE r.item_id=i.id
          AND r.seq_updated<=o.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END AS body
    FROM owners o JOIN items i ON i.thread_id=o.thread_id AND i.continuation_id=o.owner
    WHERE i.seq_created BETWEEN o.first_seq AND o.last_seq
      AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn)))";

/// Default shared readers measure only this frozen history's actual item timestamps.
pub async fn last_item_at(
    conn: &mut Conn,
    thread: Id,
    view: Id,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, DbError> {
    let sql = format!(
        "{OWNERS}{FROZEN} SELECT max((body->>'updated_at')::timestamptz),
      count(*) FILTER(WHERE body IS NULL)::bigint FROM visible_items"
    );
    let (at, missing): (Option<chrono::DateTime<chrono::Utc>>, i64) =
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(thread)
            .bind(view)
            .fetch_one(conn)
            .await?;
    if missing != 0 {
        return Err(damaged());
    }
    Ok(at)
}

/// Permanent ordinals are not renumbered when another suffix is selected.
pub async fn last_turn(conn: &mut Conn, thread: Id, view: Id) -> Result<i64, DbError> {
    let sql=format!("{OWNERS} SELECT COALESCE(max(p.ordinal),0)::bigint FROM owners o
      LEFT JOIN LATERAL(SELECT t.ordinal FROM turns t WHERE t.thread_id=$1 AND t.continuation_id=o.owner
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn ORDER BY t.ordinal DESC LIMIT 1) p ON true");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .fetch_one(conn)
        .await?)
}

/// Only actual selected turns in this permanent-ordinal interval, oldest first.
pub async fn turns_between(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    low: i64,
    high: i64,
) -> Result<Vec<TurnRow>, DbError> {
    let sql = format!(
        "{OWNERS} {TURN_SELECT} WHERE thread_id=$1 AND ordinal::bigint BETWEEN $3 AND $4
      AND EXISTS(SELECT 1 FROM owners o WHERE o.owner=turns.continuation_id
        AND turns.ordinal BETWEEN o.first_turn AND o.last_turn) ORDER BY ordinal"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(low)
        .bind(high)
        .fetch_all(conn)
        .await?)
}

/// A forward export page bounds segment identities before loading their records.
pub async fn segments_after(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    after: i32,
    limit: i64,
) -> Result<Vec<HistorySegmentRow>, DbError> {
    let sql=format!("{OWNERS},chosen AS(SELECT s.id,s.ordinal FROM owners o JOIN segments s ON s.continuation_id=o.owner
      WHERE s.thread_id=$1 AND s.ordinal>$3 AND (o.frozen_at IS NULL OR EXISTS(
        SELECT 1 FROM turns t WHERE t.segment_id=s.id AND t.ordinal BETWEEN o.first_turn AND o.last_turn)
        OR EXISTS(SELECT 1 FROM items i WHERE i.segment_id=s.id AND i.turn_id IS NULL
          AND i.seq_created BETWEEN o.first_seq AND o.last_seq)) ORDER BY s.ordinal LIMIT $4)
      {SEGMENT_SELECT} WHERE id IN(SELECT id FROM chosen) ORDER BY ordinal");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Day filters only narrow turns already present in the captured history.
pub async fn turn_numbers_in(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    span: crate::queries::exports::Span,
) -> Result<Option<(i64, i64)>, DbError> {
    let sql=format!("{OWNERS} SELECT min(t.ordinal),max(t.ordinal) FROM owners o JOIN turns t
      ON t.thread_id=$1 AND t.continuation_id=o.owner AND t.ordinal BETWEEN o.first_turn AND o.last_turn
      WHERE ($3::timestamptz IS NULL OR t.created_at>=$3) AND ($4::timestamptz IS NULL OR t.created_at<$4)");
    let (first, last): (Option<i32>, Option<i32>) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(span.from)
        .bind(span.until)
        .fetch_one(conn)
        .await?;
    Ok(first
        .zip(last)
        .map(|(first, last)| (i64::from(first), i64::from(last))))
}

/// The last visible main-stream item through this ordinal, without decoding any bodies.
pub async fn turn_high(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    ordinal: i64,
) -> Result<Option<Seq>, DbError> {
    let sql=format!("{OWNERS} SELECT max(i.seq_created) FROM owners o JOIN turns t
      ON t.thread_id=$1 AND t.continuation_id=o.owner AND t.ordinal BETWEEN o.first_turn AND o.last_turn
      JOIN items i ON i.turn_id=t.id AND i.continuation_id=o.owner WHERE t.ordinal::bigint<=$3
        AND i.seq_created BETWEEN o.first_seq AND o.last_seq AND i.stream='main' AND i.kind<>'reasoning'");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(ordinal)
        .fetch_one(conn)
        .await?)
}

/// Read through the common bounded decoder, stopping before the next written window.
pub async fn items_between(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    after: Seq,
    through: Seq,
    stream: Option<&str>,
) -> Result<Vec<ItemRow>, DbError> {
    let mut rows = Vec::new();
    let mut cursor = after;
    loop {
        let page = reading::items(conn, thread, view, Some(cursor), true, stream, PAGE).await?;
        let Some(last) = page.last().map(|row| row.seq_created) else {
            break;
        };
        rows.extend(
            page.into_iter()
                .take_while(|row| row.seq_created <= through),
        );
        if last >= through {
            break;
        }
        cursor = last;
    }
    Ok(rows)
}

/// Main-stream turn items retain their source revision, including attachments and tool output.
pub async fn items_of_turns(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    turns: &[Id],
) -> Result<Vec<ItemRow>, DbError> {
    if turns.is_empty() {
        return Ok(Vec::new());
    }
    let sql=format!("{OWNERS} SELECT min(i.seq_created),max(i.seq_created) FROM owners o JOIN items i
      ON i.thread_id=$1 AND i.continuation_id=o.owner WHERE i.turn_id=ANY($3) AND i.stream='main'
      AND i.seq_created BETWEEN o.first_seq AND o.last_seq AND EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn)");
    let (first, last): (Option<Seq>, Option<Seq>) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(turns)
        .fetch_one(&mut *conn)
        .await?;
    let (Some(first), Some(last)) = (first, last) else {
        return Ok(Vec::new());
    };
    let mut rows = items_between(
        conn,
        thread,
        view,
        first.saturating_sub(1),
        last,
        Some("main"),
    )
    .await?;
    rows.retain(|row| {
        row.kind != "reasoning" && row.turn_id.is_some_and(|turn| turns.contains(&turn))
    });
    Ok(rows)
}

/// What a range of turns of the selected history holds, counted in SQL: nothing is read into memory, whatever the
/// length of the history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RangeCounts {
    /// The turns in the range.
    pub turns: i64,
    /// The main-stream items by kind: those made in the turns (the agent's reasoning is not among them) and, when the
    /// range begins at the first turn, those from before Orch.
    pub items: std::collections::BTreeMap<String, i64>,
    /// How much room the payloads of those items take, as the database keeps them (a large one is compressed there, so
    /// this is not more than their length): what a reader that would load them all is measured against.
    pub bytes: i64,
}

/// Counts what the turns `low` to `high` of the history seen through `view` hold. Both come from indexes: the turns by
/// their numbers, the items by the position the history keeps them in.
pub async fn range_counts(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    low: i64,
    high: i64,
) -> Result<RangeCounts, DbError> {
    let sql = format!(
        "{OWNERS} SELECT count(*)::bigint FROM turns t WHERE t.thread_id=$1 AND t.ordinal BETWEEN $3 AND $4
      AND EXISTS(SELECT 1 FROM owners o WHERE o.owner=t.continuation_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn)"
    );
    let turns: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(low)
        .bind(high)
        .fetch_one(&mut *conn)
        .await?;
    let mut items = std::collections::BTreeMap::new();
    let mut bytes = 0_i64;
    if turns > 0 {
        let sql = format!(
            "{OWNERS} SELECT i.kind,count(*)::bigint,COALESCE(sum(i.payload_bytes),0)::bigint FROM owners o
          JOIN items i ON i.thread_id=$1 AND i.continuation_id=o.owner AND i.stream='main' AND i.kind<>'reasoning'
            AND i.seq_created BETWEEN o.first_seq AND o.last_seq
          JOIN turns t ON t.id=i.turn_id AND t.ordinal BETWEEN $3 AND $4
            AND t.ordinal BETWEEN o.first_turn AND o.last_turn
          GROUP BY i.kind"
        );
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(thread)
            .bind(view)
            .bind(low)
            .bind(high)
            .fetch_all(&mut *conn)
            .await?;
        for (kind, count, size) in rows {
            items.insert(kind, count);
            bytes = bytes.saturating_add(size);
        }
    }
    if low <= 1 {
        // The lines from before Orch lie before the first turn, so the scan stops at it.
        let sql = format!(
            "{OWNERS} SELECT i.kind,count(*)::bigint,COALESCE(sum(i.payload_bytes),0)::bigint FROM owners o
          JOIN items i ON i.thread_id=$1 AND i.continuation_id=o.owner AND i.before_orch AND i.turn_id IS NULL
            AND i.stream='main' AND i.seq_created BETWEEN o.first_seq AND o.last_seq
            AND i.seq_created<COALESCE((SELECT x.seq_created FROM items x WHERE x.thread_id=$1
              AND x.continuation_id=o.owner AND x.turn_id IS NOT NULL ORDER BY x.seq_created LIMIT 1),9223372036854775807)
          GROUP BY i.kind"
        );
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(thread)
            .bind(view)
            .fetch_all(&mut *conn)
            .await?;
        for (kind, count, size) in rows {
            *items.entry(kind).or_default() += count;
            bytes = bytes.saturating_add(size);
        }
    }
    Ok(RangeCounts {
        turns,
        items,
        bytes,
    })
}

/// The turns after the number `after`, up to `high`, oldest first, at most `limit` of them: a page of the selected history
/// by the numbers the turns keep.
pub async fn turns_after(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    after: i64,
    high: i64,
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    let sql = format!(
        "{OWNERS} {TURN_SELECT} WHERE thread_id=$1 AND ordinal>$3 AND ordinal<=$4
      AND EXISTS(SELECT 1 FROM owners o WHERE o.owner=turns.continuation_id
        AND turns.ordinal BETWEEN o.first_turn AND o.last_turn) ORDER BY ordinal LIMIT $5"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(after)
        .bind(high)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// A bounded returned search page is decoded together, under each thread's selected or explicit view.
pub async fn search_items(
    conn: &mut Conn,
    ids: &[Id],
    view: Option<Id>,
) -> Result<Vec<ItemRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let decode = reading::DECODE.replace(
        "JOIN items i ON i.id=c.id",
        "JOIN wanted_items i ON i.id=c.id",
    );
    let sql=format!("WITH wanted_items AS MATERIALIZED (SELECT * FROM items WHERE id=ANY($1)),selected AS (SELECT t.node_id AS thread_id,c.id AS view FROM threads t
        JOIN thread_continuations c ON c.thread_id=t.node_id AND c.id=COALESCE($2,t.active_continuation_id)
        WHERE t.node_id IN(SELECT thread_id FROM wanted_items)),owners AS (
      SELECT s.thread_id,s.view AS owner,1 AS first_turn,2147483647 AS last_turn,
        0::bigint AS first_seq,9223372036854775807::bigint AS last_seq,NULL::bigint AS frozen_at FROM selected s
      UNION ALL SELECT s.thread_id,p.owner_continuation_id,p.first_turn,p.last_turn,p.first_seq,p.last_seq,p.frozen_at
        FROM selected s JOIN thread_continuation_spans p ON p.continuation_id=s.view),candidates AS (
      SELECT i.id,o.frozen_at FROM owners o JOIN wanted_items i ON i.thread_id=o.thread_id AND i.continuation_id=o.owner
        WHERE i.seq_created BETWEEN o.first_seq AND o.last_seq
          AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id
            AND t.ordinal BETWEEN o.first_turn AND o.last_turn))) {}",decode);
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .bind(view)
        .fetch_all(conn)
        .await?)
}

/// One fixed metadata batch supplies exact deep-link views for every returned item, with no native probe. A search
/// that names no history follows each thread's active one, and says so as `thread_continuations::projection` does:
/// a reader that follows the active history otherwise takes every hit for a changed history and reads it again.
pub async fn search_projections(
    conn: &mut Conn,
    threads: &[Id],
    view: Option<Id>,
) -> Result<Vec<crate::queries::thread_continuations::Projection>, DbError> {
    Ok(sqlx::query_as("SELECT t.node_id AS thread_id,c.id AS viewed_continuation_id,
      t.active_continuation_id,t.execution_revision,c.projection_revision,($2::uuid IS NULL) AS follows_active,
      c.current_segment_id AS viewed_segment_id FROM threads t JOIN thread_continuations c ON c.thread_id=t.node_id
        AND c.id=COALESCE($2,t.active_continuation_id) WHERE t.node_id=ANY($1)")
        .bind(threads).bind(view).fetch_all(conn).await?)
}

/// Sub-agent answers use the same frozen owner/range as the step that refers to them.
pub async fn last_sub_messages(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    streams: &[String],
) -> Result<HashMap<String, String>, DbError> {
    if streams.is_empty() {
        return Ok(HashMap::new());
    }
    let sql=format!("{OWNERS}{FROZEN} SELECT DISTINCT ON(stream) stream,body->'payload' AS payload
      FROM visible_items WHERE stream=ANY($3) AND kind='agent_message' ORDER BY stream,seq_created DESC");
    let rows: Vec<(String, Option<Json>)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(streams)
        .fetch_all(conn)
        .await?;
    let mut found = HashMap::new();
    for (stream, payload) in rows {
        let payload = payload.ok_or_else(damaged)?;
        if let Some(text) = payload.get("text").and_then(Json::as_str) {
            found.insert(stream, text.to_owned());
        }
    }
    Ok(found)
}

#[derive(sqlx::FromRow)]
struct Counted {
    #[sqlx(flatten)]
    counts: CatchupCounts,
    damaged: i64,
}

/// Counts include only this history; payload-dependent totals use its frozen revision in SQL.
pub async fn catchup_counts(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    since: Seq,
) -> Result<CatchupCounts, DbError> {
    let sql=format!("{OWNERS}{FROZEN},selected_turns AS NOT MATERIALIZED (
      SELECT t.state,t.seq_updated FROM owners o JOIN turns t ON t.thread_id=$1 AND t.continuation_id=o.owner
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn)
      SELECT count(*)::bigint AS items,
        (SELECT count(*) FROM selected_turns WHERE state='completed' AND seq_updated>$3) AS turns_completed,
        (SELECT count(*) FROM selected_turns WHERE state='failed' AND seq_updated>$3) AS turns_failed,
        count(*) FILTER(WHERE kind='tool_call' AND body#>>'{{payload,tool,kind}}'='command')::bigint AS commands,
        COALESCE(sum(CASE WHEN kind='tool_call' AND body#>>'{{payload,tool,kind}}'='file_change'
          THEN jsonb_array_length(body#>'{{payload,tool,changes}}') ELSE 0 END),0)::bigint AS files_changed,
        count(*) FILTER(WHERE kind='error')::bigint AS errors,
        count(*) FILTER(WHERE body IS NULL)::bigint AS damaged FROM visible_items WHERE seq_created>$3");
    let row: Counted = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(since)
        .fetch_one(conn)
        .await?;
    if row.damaged != 0 {
        return Err(damaged());
    }
    Ok(row.counts)
}

/// Harness approvals belong to the actual selected turns, not every retained suffix.
pub async fn approvals_pending(conn: &mut Conn, thread: Id, view: Id) -> Result<i64, DbError> {
    let sql=format!("{OWNERS} SELECT count(*) FROM approvals a JOIN turns t ON t.id=a.turn_id
      WHERE a.thread_id=$1 AND a.kind='harness' AND a.state='pending' AND EXISTS(SELECT 1 FROM owners o
        WHERE o.owner=t.continuation_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn)");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .fetch_one(conn)
        .await?)
}

/// Two independent title cursors never acknowledge omitted rows in the other list.
#[derive(Clone, Copy)]
pub enum Titles {
    /// Milestone announcements made by an agent's shared tool.
    Milestone,
    /// Announcements delivered into the conversation.
    Announcement,
}

/// A title's permanent sequence also supplies its exclusive continuation cursor.
#[derive(sqlx::FromRow)]
pub struct Title {
    /// Actual retained item identity.
    pub item_id: Id,
    /// Permanent creation position.
    pub seq: Seq,
    /// Frozen visible title, including an empty title as recorded.
    pub title: String,
}

/// Bound IDs in SQL before decoding the title from its retained body.
pub async fn titles(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    after: Seq,
    kind: Titles,
    limit: i64,
) -> Result<Vec<Title>, DbError> {
    let (predicate, field) = match kind {
        Titles::Milestone => (
            "kind='tool_call' AND body#>>'{payload,tool,kind}'='orch' AND body#>>'{payload,tool,tool}'='announce' AND body#>>'{payload,tool,arguments,category}'='milestone'",
            "body#>>'{payload,tool,arguments,title}'",
        ),
        Titles::Announcement => ("kind='announcement'", "body#>>'{payload,title}'"),
    };
    let sql = format!(
        "{OWNERS}{FROZEN} SELECT id AS item_id,seq_created AS seq,COALESCE({field},'') AS title
      FROM visible_items WHERE seq_created>$3 AND {predicate} ORDER BY seq_created LIMIT $4"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Opaque raw protocol frames cannot be clipped safely to a partially inherited native segment.
/// A referenced prefix has no captured raw-frame boundary; only its actual owner may read the whole segment.
pub async fn complete_segment(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    segment: Id,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM segments WHERE thread_id=$1 AND continuation_id=$2 AND id=$3)",
    )
    .bind(thread)
    .bind(view)
    .bind(segment)
    .fetch_one(conn)
    .await?)
}
