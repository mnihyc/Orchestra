//! What fills a thread's context and what its turns used beyond their totals: each harness's report kept with its turn, Orch's own estimate by part where a harness breaks nothing down, and a count asked of the harness on request.
//!
//! **What is kept.** Every report of use an adapter sends may carry more than the totals: the sub-agents' and
//! each model's share of the turn, the size of the last prompt against the window, where the harness shortens
//! the conversation, a shortening with its sizes, and what fills the context by part. The turn keeps it all
//! (`turns.usage_detail`, `turns.context`); the thread's own usage keeps where the harness shortens.
//!
//! **Orch's estimate.** A harness that breaks nothing down still reports the size of its last prompt.
//! At the end of each of its turns Orch splits that size into the parts it can see, from what it recorded
//! sending (the session's opening text and tool definitions, each input's text) and the items it observed
//! since the conversation was last shortened (what the agent said, each tool call with its result, the
//! images), and names the rest the harness's own: its system prompt, its tools, the summary it keeps of what
//! it shortened, the reasoning it keeps. It never invents a share for what it cannot see. Each part is turned
//! into tokens with the measures of the model's family, which the catalogue of `token_density` holds as data:
//! bytes of ASCII text per token, and characters beyond ASCII per token. Structured text (code, JSON, what a
//! command printed) is counted at the middle of code and JSON. So the estimate of one part errs by about a
//! tenth for a family that was measured and about a quarter for one that was not, more where a part is
//! unusual (a file of a language unlike the samples); the harness's own part takes up the difference, since
//! the whole is what the harness reported. Where the estimates come to more than that whole, they are scaled
//! down to it, and the fill says so. A text known only by its length (a long result, kept as a blob) is
//! counted as ASCII of its kind; an image at the size of a screenshot of 1024 by 768 pixels.

use std::collections::BTreeMap;

use orch_api_contract::context::{
    ContextCount, ContextFill, ContextPart, ContextPartKind, ContextReports, ContextRow,
    ContextRowKind, ContextShortening, ThreadContext, ThreadTurnUsage, TurnCost, TurnCostRow,
    TurnUsageDetail, TurnUsageRow,
};
use orch_api_contract::resources::HarnessKindName;
use orch_api_contract::threads::TokenUsage;
use orch_db::Conn;
use orch_db::queries::threads::TurnRow;
use orch_db::usage::ledger;
use orch_link_contract::commands::{SerialKey, SessionContext, SessionContextResult};
use orch_link_contract::events::UsageUpdated;
use orch_types::{Id, Json, Time, new_id};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::unit::Unit;

/// A stored value read as `T`, as the wire has it; `null` where it is none or of another shape, so that an
/// older record never makes what holds it unreadable.
#[must_use]
pub fn readable<T: DeserializeOwned + Serialize>(value: Option<&Json>) -> Json {
    value
        .and_then(|value| serde_json::from_value::<T>(value.clone()).ok())
        .and_then(|read| serde_json::to_value(read).ok())
        .unwrap_or(Json::Null)
}

fn read<T: DeserializeOwned>(value: Option<&Json>) -> Option<T> {
    value.and_then(|value| serde_json::from_value(value.clone()).ok())
}

// ---------------------------------------------------------------------------------------------------------
// The estimate

/// The measures of a model's tokenizer: the catalogue's row for its family.
pub use orch_api_contract::programs::TokenEstimate as Family;

/// What kind of text, for how densely it packs into tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// Words: instructions, what people and the agent said.
    Prose,
    /// Code, JSON, what a command printed: tool definitions, calls and their results.
    Structured,
}

/// Bytes of ASCII text per token, times a hundred, as measured: the structured kind at the middle of code and JSON.
fn ascii_bytes_per_token_x100(family: Family, kind: TextKind) -> i64 {
    match kind {
        TextKind::Prose => family.ascii_prose_x100,
        TextKind::Structured => family.ascii_structured_x100,
    }
}

/// Characters beyond ASCII per token, times a hundred, as measured on Chinese text once its ASCII share was
/// taken out.
fn other_chars_per_token_x100(family: Family) -> i64 {
    family.other_x100
}

/// What an image takes, without its size: a screenshot of 1024 by 768 pixels.
fn image_tokens(family: Family) -> i64 {
    family.image_tokens
}

/// The most of one tool call and of its result counted as being in the context, in tokens: the harness hands
/// the model about ten thousand tokens of what a command printed at most and drops the middle of the rest, so
/// a longer output, which may run to megabytes, is counted at that.
const TOOL_EXCHANGE_MOST_TOKENS: i64 = 10_000;

/// Percent: the measures are kept times a hundred so that they stay integers.
const HUNDRED: i64 = 100;

fn ceil_div(numerator: i64, denominator: i64) -> i64 {
    if numerator <= 0 || denominator <= 0 {
        0
    } else {
        (numerator + denominator - 1) / denominator
    }
}

fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The tokens of a text, estimated: its ASCII bytes and its other characters, each at the family's measure.
#[must_use]
pub fn estimate_text(text: &str, family: Family, kind: TextKind) -> i64 {
    let ascii = count(text.bytes().filter(u8::is_ascii).count());
    let other = count(text.chars().filter(|c| !c.is_ascii()).count());
    ceil_div(ascii * HUNDRED, ascii_bytes_per_token_x100(family, kind))
        + ceil_div(other * HUNDRED, other_chars_per_token_x100(family))
}

/// The tokens of a text known only by its length in bytes, counted as ASCII of its kind.
#[must_use]
pub fn estimate_bytes(bytes: i64, family: Family, kind: TextKind) -> i64 {
    ceil_div(
        bytes.max(0) * HUNDRED,
        ascii_bytes_per_token_x100(family, kind),
    )
}

/// The tokens of `images` images.
#[must_use]
pub fn estimate_images(images: i64, family: Family) -> i64 {
    images.max(0) * image_tokens(family)
}

/// Where a part stands in the bar: what every request carries first, the conversation after.
fn bar_order(kind: ContextPartKind) -> u8 {
    match kind {
        ContextPartKind::Harness => 0,
        ContextPartKind::OrchInstructions => 1,
        ContextPartKind::OrchTools => 2,
        ContextPartKind::McpTools => 3,
        ContextPartKind::MemoryFiles => 4,
        ContextPartKind::SkillsAgents => 5,
        ContextPartKind::PeopleMessages => 6,
        ContextPartKind::AgentMessages => 7,
        ContextPartKind::ToolCalls => 8,
        ContextPartKind::Files => 9,
        ContextPartKind::Other => 10,
    }
}

fn scale(tokens: i64, by: i64, of: i64) -> i64 {
    if of <= 0 {
        return 0;
    }
    i64::try_from(i128::from(tokens) * i128::from(by) / i128::from(of)).unwrap_or(i64::MAX)
}

/// The fill a harness reported the size of, made of Orch's estimates of the parts it can see and, for the
/// rest, the harness's own part. Where the estimates come to more than the size, they are scaled down to it,
/// the largest part taking what rounding leaves, and the fill says so. Its parts always add up to `used`.
#[must_use]
pub fn compose(
    used: i64,
    window: Option<i64>,
    compact_at: Option<i64>,
    estimates: Vec<ContextPart>,
    at: Time,
) -> ContextFill {
    let used = used.max(0);
    let mut parts: Vec<ContextPart> = estimates
        .into_iter()
        .filter(|part| part.tokens > 0)
        .collect();
    let estimated: i64 = parts.iter().map(|part| part.tokens).sum();
    let scaled = estimated > used;
    if scaled {
        for part in &mut parts {
            part.tokens = scale(part.tokens, used, estimated);
            for row in &mut part.rows {
                row.tokens = scale(row.tokens, used, estimated);
                row.calls = row.calls.map(|calls| scale(calls, used, estimated));
            }
        }
        let rounded: i64 = parts.iter().map(|part| part.tokens).sum();
        if let Some(largest) = parts.iter_mut().max_by_key(|part| part.tokens) {
            largest.tokens += used - rounded;
        }
        parts.retain(|part| part.tokens > 0);
    } else if used > estimated {
        parts.push(ContextPart {
            kind: ContextPartKind::Harness,
            tokens: used - estimated,
            count: ContextCount::Orch,
            rows: Vec::new(),
        });
    }
    parts.sort_by_key(|part| bar_order(part.kind));
    ContextFill {
        count: ContextCount::Orch,
        used,
        window,
        compact_at,
        parts,
        scaled,
        at,
    }
}

/// Rows the largest first, and the part that holds them; `None` for a part of nothing.
fn estimated_part(kind: ContextPartKind, mut rows: Vec<ContextRow>) -> Option<ContextPart> {
    rows.retain(|row| row.tokens > 0);
    rows.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    let tokens: i64 = rows.iter().map(|row| row.tokens).sum();
    (tokens > 0).then_some(ContextPart {
        kind,
        tokens,
        count: ContextCount::Orch,
        rows,
    })
}

fn orch_row(kind: ContextRowKind, name: &str, tokens: i64, calls: Option<i64>) -> ContextRow {
    ContextRow {
        kind,
        name: name.to_owned(),
        tokens,
        calls,
        count: ContextCount::Orch,
    }
}

/// A block of a record of what Orch sent: its key, where it comes from, and its text or, for a long one kept
/// apart, its length.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct SentBlock {
    key: String,
    source: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    total_len: Option<i64>,
}

impl SentBlock {
    fn tokens(&self, family: Family, kind: TextKind) -> i64 {
        match self.total_len {
            Some(bytes) if bytes > i64::try_from(self.text.len()).unwrap_or(i64::MAX) => {
                estimate_bytes(bytes, family, kind)
            }
            _ => estimate_text(&self.text, family, kind),
        }
    }
}

/// What Orch sent and saw of one segment since its conversation was last shortened.
#[derive(Debug, Default)]
struct Seen {
    /// The blocks the session was opened with.
    session: Vec<SentBlock>,
    /// The blocks of each input since.
    inputs: Vec<SentBlock>,
    /// What the agent said, in bytes.
    said: i64,
    /// Tool calls by the name the harness gave them: how many, the bytes of their calls and of their results.
    tools: BTreeMap<String, (i64, i64, i64)>,
    /// Images, given by people or viewed by a tool.
    images: i64,
}

/// The parts Orch can see, estimated: its instructions, its tools, people's messages, the agent's messages,
/// tool calls and their results, files and images.
fn estimates_of(seen: &Seen, family: Family) -> Vec<ContextPart> {
    let mut instructions: BTreeMap<String, i64> = BTreeMap::new();
    let mut tools = Vec::new();
    for block in &seen.session {
        if block.source == "tool" {
            tools.push(orch_row(
                ContextRowKind::McpTool,
                &block.key,
                block.tokens(family, TextKind::Structured),
                None,
            ));
        } else {
            *instructions.entry(block.key.clone()).or_default() +=
                block.tokens(family, TextKind::Prose);
        }
    }
    let mut people = 0;
    for block in &seen.inputs {
        if block.key == "input" {
            people += block.tokens(family, TextKind::Prose);
        } else {
            *instructions.entry(block.key.clone()).or_default() +=
                block.tokens(family, TextKind::Prose);
        }
    }
    let calls: Vec<ContextRow> = seen
        .tools
        .iter()
        .map(|(name, (_, call, result))| {
            let call = estimate_bytes(*call, family, TextKind::Structured);
            let result = estimate_bytes(*result, family, TextKind::Structured);
            orch_row(ContextRowKind::Tool, name, call + result, Some(call))
        })
        .collect();
    let images = estimate_images(seen.images, family);
    [
        estimated_part(
            ContextPartKind::OrchInstructions,
            instructions
                .iter()
                .map(|(key, tokens)| orch_row(ContextRowKind::OrchShare, key, *tokens, None))
                .collect(),
        ),
        estimated_part(ContextPartKind::OrchTools, tools),
        estimated_part(
            ContextPartKind::PeopleMessages,
            vec![orch_row(ContextRowKind::OrchShare, "people", people, None)],
        )
        .map(|mut part| {
            part.rows.clear();
            part
        }),
        estimated_part(
            ContextPartKind::AgentMessages,
            vec![orch_row(
                ContextRowKind::OrchShare,
                "agent",
                estimate_bytes(seen.said, family, TextKind::Prose),
                None,
            )],
        )
        .map(|mut part| {
            part.rows.clear();
            part
        }),
        estimated_part(ContextPartKind::ToolCalls, calls),
        estimated_part(
            ContextPartKind::Files,
            vec![orch_row(ContextRowKind::Attachment, "image", images, None)],
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The bytes one call or one result is counted at, at most, for a family.
fn exchange_most_bytes(family: Family) -> i64 {
    TOOL_EXCHANGE_MOST_TOKENS * ascii_bytes_per_token_x100(family, TextKind::Structured) / HUNDRED
}

/// Items of one kind (and, for tool calls, of one name and kind of tool), summed.
#[derive(Debug, sqlx::FromRow)]
struct SeenRow {
    kind: String,
    name: String,
    tool: String,
    items: i64,
    said: i64,
    call_bytes: i64,
    result_bytes: i64,
    images: i64,
}

/// A turn with what it used, as it is read for a page.
#[derive(Debug, sqlx::FromRow)]
struct UsageRow {
    id: Id,
    ordinal: i32,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    model: String,
    model_display: String,
    usage: Option<Json>,
    usage_detail: Option<Json>,
}

/// The counted ledger rows of one model of a turn, summed.
#[derive(Debug, sqlx::FromRow)]
struct CostRow {
    model: String,
    model_display: Option<String>,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    total_tokens: i64,
    credits: Option<String>,
    rate_source: Option<String>,
}

/// What Orch sent and saw of a segment since its conversation was last shortened: the session's opening, the
/// inputs since, and the items of its main stream since (a sub-agent's work is in a context of its own).
async fn seen_of(conn: &mut Conn, segment: Id, family: Family) -> Result<Seen, EngineError> {
    let since: i64 = sqlx::query_scalar(
        "SELECT COALESCE(max(seq_created), 0)::bigint FROM items
          WHERE segment_id = $1 AND kind = 'marker' AND payload->>'marker' = 'compacted'",
    )
    .bind(segment)
    .fetch_one(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let mut seen = Seen::default();
    let session: Option<Json> = sqlx::query_scalar(
        "SELECT blocks FROM model_inputs WHERE segment_id = $1 AND purpose = 'session'
          ORDER BY seq_created DESC LIMIT 1",
    )
    .bind(segment)
    .fetch_optional(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    seen.session = session
        .and_then(|blocks| serde_json::from_value(blocks).ok())
        .unwrap_or_default();
    let inputs: Vec<Json> = sqlx::query_scalar(
        "SELECT blocks FROM model_inputs WHERE segment_id = $1 AND purpose = 'input' AND seq_created > $2
          AND state <> 'failed' ORDER BY seq_created",
    )
    .bind(segment)
    .bind(since)
    .fetch_all(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    for blocks in inputs {
        let blocks: Vec<SentBlock> = serde_json::from_value(blocks).unwrap_or_default();
        seen.inputs.extend(blocks);
    }
    let rows: Vec<SeenRow> = sqlx::query_as(
        "SELECT kind, COALESCE(payload->'raw'->>'name', '') AS name, COALESCE(payload->'tool'->>'kind', '') AS tool,
                count(*)::bigint AS items,
                COALESCE(sum(CASE WHEN kind = 'agent_message' THEN octet_length(COALESCE(payload->>'text', '')) ELSE 0 END), 0)::bigint AS said,
                COALESCE(sum(CASE WHEN kind = 'tool_call' AND jsonb_typeof(payload->'raw'->'input'->'total_len') = 'number'
                                  THEN LEAST((payload->'raw'->'input'->>'total_len')::bigint, $3) ELSE 0 END), 0)::bigint AS call_bytes,
                COALESCE(sum(CASE WHEN kind = 'tool_call' AND jsonb_typeof(payload->'raw'->'output'->'total_len') = 'number'
                                  THEN LEAST((payload->'raw'->'output'->>'total_len')::bigint, $3) ELSE 0 END), 0)::bigint AS result_bytes,
                COALESCE(sum(CASE WHEN kind = 'user_message' AND jsonb_typeof(payload->'parts') = 'array'
                                  THEN (SELECT count(*) FROM jsonb_array_elements(payload->'parts') AS part
                                         WHERE part->>'type' = 'image')
                                  ELSE 0 END), 0)::bigint AS images
           FROM items
          WHERE segment_id = $1 AND stream = 'main' AND NOT before_orch AND seq_created > $2
            AND kind IN ('agent_message', 'tool_call', 'user_message')
          GROUP BY 1, 2, 3",
    )
    .bind(segment)
    .bind(since)
    .bind(exchange_most_bytes(family))
    .fetch_all(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    for row in rows {
        seen.said += row.said;
        seen.images += row.images;
        if row.kind == "tool_call" {
            if row.tool == "image" {
                // A picture the agent viewed is in the context as an image, not as text.
                seen.images += row.items;
            } else {
                let entry = seen.tools.entry(row.name).or_default();
                entry.0 += row.items;
                entry.1 += row.call_bytes;
                entry.2 += row.result_bytes;
            }
        }
    }
    Ok(seen)
}

/// What Orch sent and saw of a segment, read inside a savepoint of its own, so that a fault in reading it leaves
/// the transaction that asked whole: `None` then, logged.
async fn seen_safely(
    conn: &mut Conn,
    segment: Id,
    family: Family,
) -> Result<Option<Seen>, EngineError> {
    let mut savepoint = sqlx::Connection::begin(conn)
        .await
        .map_err(orch_db::DbError::from)?;
    match seen_of(&mut savepoint, segment, family).await {
        Ok(seen) => {
            savepoint.commit().await.map_err(orch_db::DbError::from)?;
            Ok(Some(seen))
        }
        Err(error) => {
            savepoint.rollback().await.map_err(orch_db::DbError::from)?;
            tracing::warn!(%error, %segment, "what Orch sent to a session could not be read");
            Ok(None)
        }
    }
}

/// Orch's share of a part that the harness counts as its own: its instructions inside the harness's system
/// prompt, and the lines it adds to each input inside people's messages, estimated.
fn with_orch_share(mut fill: ContextFill, seen: &Seen, family: Family) -> ContextFill {
    let opening: i64 = seen
        .session
        .iter()
        .filter(|block| block.source != "tool")
        .map(|block| block.tokens(family, TextKind::Prose))
        .sum();
    let added: i64 = seen
        .inputs
        .iter()
        .filter(|block| block.key != "input")
        .map(|block| block.tokens(family, TextKind::Prose))
        .sum();
    for part in &mut fill.parts {
        let share = match part.kind {
            ContextPartKind::Harness => opening,
            ContextPartKind::PeopleMessages => added,
            _ => continue,
        };
        part.rows
            .retain(|row| row.kind != ContextRowKind::OrchShare);
        let share = share.min(part.tokens);
        if share > 0 {
            part.rows
                .insert(0, orch_row(ContextRowKind::OrchShare, "orch", share, None));
        }
    }
    fill
}

// ---------------------------------------------------------------------------------------------------------
// Keeping what was reported, and the estimate at the end of a turn

/// The thread's usage as it is kept, with where the harness shortens the conversation from `usage`'s report,
/// or as it was.
pub(crate) fn with_compact_at(whole: &mut Json, usage: &UsageUpdated, previous: Option<&Json>) {
    let reported = usage.report.as_ref().and_then(|report| report.compact_at);
    if let Some(object) = whole.as_object_mut() {
        let value = match reported {
            Some(at) => Json::from(at),
            None => previous
                .and_then(|previous| previous.get("compact_at"))
                .cloned()
                .unwrap_or(Json::Null),
        };
        object.insert("compact_at".to_owned(), value);
    }
}

impl Engine {
    /// Keeps with its turn what a report of use carried beyond the totals: the sub-agents' and each model's
    /// share, the context, a shortening, and a breakdown of the context, which takes Orch's share of the
    /// harness's parts with it.
    pub(crate) async fn keep_usage_report(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        turn: &TurnRow,
        usage: &UsageUpdated,
    ) -> Result<(), EngineError> {
        let report = usage.report.clone().unwrap_or_default();
        let mut detail: TurnUsageDetail = read(turn.usage_detail.as_ref()).unwrap_or_default();
        let before = detail.clone();
        if report.api_time_ms.is_some() {
            detail.api_time_ms = report.api_time_ms;
        }
        if report.subagents.is_some() {
            detail.subagents = report.subagents;
        }
        if !report.by_subagent.is_empty() {
            detail.by_subagent = report.by_subagent;
        }
        if !report.models.is_empty() {
            detail.models = report.models;
        }
        if usage.context_window.is_some() {
            detail.context_window = usage.context_window;
        }
        if usage.context_used.is_some() {
            detail.context_used = usage.context_used;
        }
        if report.compact_at.is_some() {
            detail.compact_at = report.compact_at;
        }
        if let Some(shortened) = report.shortened
            && !detail
                .shortenings
                .iter()
                .any(|known| known.at == shortened.at)
        {
            detail.shortenings.push(shortened);
        }
        if usage.report.is_some() {
            detail.breakdown = report.breakdown;
        }
        let fill = match report.context {
            Some(fill) => {
                let family = crate::token_density::of(&turn.model);
                // Orch's share is an addition: where what it sent cannot be read, the harness's breakdown stands alone.
                match seen_safely(&mut *unit.conn, turn.segment_id, family).await? {
                    Some(seen) => Some(with_orch_share(fill, &seen, family)),
                    None => Some(fill),
                }
            }
            None => None,
        };
        if detail == before && fill.is_none() {
            return Ok(());
        }
        let seq = unit.stamp(thread).await?;
        let fill = fill.map(serde_json::to_value).transpose()?;
        sqlx::query(
            "UPDATE turns SET usage_detail = $2, context = COALESCE($3, context), seq_updated = $4 WHERE id = $1",
        )
        .bind(turn.id)
        .bind(serde_json::to_value(&detail)?)
        .bind(fill)
        .bind(seq)
        .execute(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// At the end of a turn: what filled the context, estimated by Orch where the harness breaks nothing down
    /// itself and reported the size of its prompt. The estimate runs inside a savepoint of its own: a fault in
    /// it is logged, and the turn ends all the same.
    pub(crate) async fn estimate_context_at_end(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        turn: Id,
    ) -> Result<(), EngineError> {
        let Some(row) = orch_db::queries::threads::turn(&mut *unit.conn, turn).await? else {
            return Ok(());
        };
        let detail: TurnUsageDetail = read(row.usage_detail.as_ref()).unwrap_or_default();
        let kept: Option<ContextFill> = read(row.context.as_ref());
        let Some(used) = detail.context_used.filter(|_| !detail.breakdown) else {
            return Ok(());
        };
        if kept.is_some_and(|fill| fill.count != ContextCount::Orch) {
            return Ok(());
        }
        let at = unit.now();
        let mut savepoint = sqlx::Connection::begin(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let family = crate::token_density::of(&row.model);
        let estimated = async {
            let seen = seen_of(&mut savepoint, row.segment_id, family).await?;
            let fill = compose(
                used,
                detail.context_window,
                detail.compact_at,
                estimates_of(&seen, family),
                Time::new(at),
            );
            sqlx::query("UPDATE turns SET context = $2 WHERE id = $1")
                .bind(row.id)
                .bind(serde_json::to_value(&fill)?)
                .execute(&mut *savepoint)
                .await
                .map_err(orch_db::DbError::from)?;
            Ok::<(), EngineError>(())
        }
        .await;
        match estimated {
            Ok(()) => {
                savepoint.commit().await.map_err(orch_db::DbError::from)?;
                // The turn's row changed: it is stamped, so that whoever has the thread open reads it.
                let seq = unit.stamp(thread).await?;
                sqlx::query("UPDATE turns SET seq_updated = $2 WHERE id = $1")
                    .bind(row.id)
                    .bind(seq)
                    .execute(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            Err(error) => {
                savepoint.rollback().await.map_err(orch_db::DbError::from)?;
                tracing::warn!(%error, %thread, %turn, "what filled the context could not be estimated");
            }
        }
        Ok(())
    }

    /// Has the harness count what fills a thread's context now, where it breaks its context down, the thread is
    /// idle and its session is open; keeps the count with the latest turn of that session. `None` where it
    /// cannot.
    pub async fn count_context(&self, thread: Id) -> Result<Option<ContextFill>, EngineError> {
        let target: Option<(Id, Option<Id>)> = {
            let mut conn = self.inner.db.acquire().await?;
            sqlx::query_as(
                "SELECT s.id, s.machine_id FROM threads t JOIN segments s ON s.id = t.current_segment_id
                  WHERE t.node_id = $1 AND t.state = 'idle' AND s.ended_at IS NULL
                    AND s.native_session_ref IS NOT NULL",
            )
            .bind(thread)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?
        };
        let Some((segment, Some(machine))) = target else {
            return Ok(None);
        };
        let command = Outgoing::new(
            new_id(),
            machine,
            SerialKey::Thread(thread),
            &SessionContext {
                thread_id: thread,
                segment_id: segment,
            },
        )?;
        let fill = match self.inner.commands.observe(command).await {
            Ok(Outcome::Succeeded(value)) => serde_json::from_value::<SessionContextResult>(value)
                .ok()
                .and_then(|result| result.fill),
            _ => None,
        };
        let Some(fill) = fill else {
            return Ok(None);
        };
        let mut tx = self.inner.db.begin().await?;
        let selected = orch_db::queries::threads::lock(&mut tx, thread).await?;
        if selected
            .as_ref()
            .is_none_or(|head| head.current_segment_id != Some(segment))
            || !orch_db::queries::thread_history::selected_segment(&mut tx, thread, segment).await?
        {
            return Ok(None);
        }
        let latest: Option<TurnRow> =
            orch_db::queries::thread_history::latest_selected_turn(&mut tx, thread)
                .await?
                .filter(|turn| turn.segment_id == segment && !turn.is_active());
        let Some(turn) = latest else {
            return Ok(Some(fill));
        };
        let family = crate::token_density::of(&turn.model);
        let fill = match seen_safely(&mut tx, segment, family).await? {
            Some(seen) => with_orch_share(fill, &seen, family),
            None => fill,
        };
        if let Some(seq) = orch_db::sequences::next(&mut tx, thread).await? {
            sqlx::query("UPDATE turns SET context = $2, seq_updated = $3 WHERE id = $1")
                .bind(turn.id)
                .bind(serde_json::to_value(&fill)?)
                .bind(seq)
                .execute(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
            orch_db::sequences::notify(&mut tx, thread, seq).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Some(fill))
    }
}

// ---------------------------------------------------------------------------------------------------------
// Reading

/// What the harness of a thread reports, from the descriptor its agent profile last reported and its turns.
async fn reports_of(
    conn: &mut Conn,
    thread: Id,
    selected_breakdown: Option<bool>,
) -> Result<(Option<String>, Option<HarnessKindName>, ContextReports), EngineError> {
    let agent: Option<(String, Json)> = sqlx::query_as(
        "SELECT a.harness_kind, a.descriptor FROM threads t JOIN agents a ON a.id = t.agent_id WHERE t.node_id = $1",
    )
    .bind(thread)
    .fetch_optional(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let breakdown: Option<bool> = if selected_breakdown.is_some() {
        selected_breakdown
    } else {
        sqlx::query_scalar(
            "SELECT COALESCE((usage_detail->>'breakdown')::boolean, false) FROM turns
          WHERE thread_id = $1 AND usage_detail IS NOT NULL ORDER BY ordinal DESC LIMIT 1",
        )
        .bind(thread)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?
    };
    let Some((kind, descriptor)) = agent else {
        return Ok((None, None, ContextReports::default()));
    };
    let capability = |name: &str| {
        descriptor
            .pointer(&format!("/capabilities/{name}"))
            .and_then(Json::as_bool)
            .unwrap_or(false)
    };
    let harness = crate::programs::harness_name(
        orch_agent_programs::Programs::builtin(),
        &kind,
        Some(&descriptor),
    );
    Ok((
        Some(kind),
        Some(harness),
        ContextReports {
            tokens: capability("usage_tokens"),
            // A descriptor written before the report had words says it with a boolean.
            context: match descriptor.pointer("/capabilities/usage_context") {
                Some(Json::Bool(reported)) => *reported,
                Some(Json::String(kind)) => kind != "none",
                _ => false,
            },
            breakdown: breakdown.unwrap_or(false),
        },
    ))
}

/// What fills a thread's context as it was kept: at the end of its latest turn that has a fill, with the last
/// shortening and whether the harness can be asked to count it now.
pub async fn thread_context(conn: &mut Conn, thread: Id) -> Result<ThreadContext, EngineError> {
    let scope = orch_db::queries::thread_continuations::projection(conn, thread, None)
        .await?
        .ok_or_else(|| EngineError::unreadable("The selected thread history is unavailable."))?;
    let history = orch_db::queries::thread_history::context_history(
        conn,
        thread,
        scope.viewed_continuation_id,
    )
    .await?;
    let (harness_kind, harness, reports) =
        reports_of(conn, thread, Some(history.breakdown)).await?;
    let turn_id = history.turn_id;
    let fill = history
        .context
        .and_then(|fill| serde_json::from_value::<ContextFill>(fill).ok());
    let shortened = history
        .shortenings
        .into_iter()
        .filter_map(|list| serde_json::from_value::<Vec<ContextShortening>>(list).ok())
        .flatten()
        .max_by_key(|shortening| shortening.at);
    let idle: bool = sqlx::query_scalar(
        "SELECT t.state = 'idle' AND s.ended_at IS NULL AND s.native_session_ref IS NOT NULL AND s.machine_id IS NOT NULL
           FROM threads t JOIN segments s ON s.id = t.current_segment_id WHERE t.node_id = $1",
    )
    .bind(thread)
    .fetch_optional(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?
    .unwrap_or(false);
    let counted = fill
        .as_ref()
        .is_some_and(|fill| fill.count == ContextCount::Counted);
    Ok(ThreadContext {
        harness_kind,
        harness,
        reports,
        can_count: reports.breakdown && idle && !counted,
        fill,
        turn_id,
        shortened,
    })
}

/// A thread's turns with what each used, the latest last: at most `limit`, numbered below `before`.
pub async fn turn_usage_page(
    conn: &mut Conn,
    thread: Id,
    before: Option<i64>,
    limit: u32,
) -> Result<ThreadTurnUsage, EngineError> {
    // The Usage page is a lifetime record of actual turns, including inactive histories.
    let (harness_kind, harness, reports) = reports_of(conn, thread, None).await?;
    let before = before.and_then(|before| i32::try_from(before).ok());
    let rows: Vec<UsageRow> = sqlx::query_as(
        "SELECT t.id, t.ordinal, t.started_at, t.completed_at, t.model,
                COALESCE(pm.display_name, '') AS model_display, t.usage, t.usage_detail
           FROM turns t JOIN segments s ON s.id = t.segment_id
           LEFT JOIN provider_models pm ON pm.provider_id = s.provider_id AND pm.model_name = t.model
          WHERE t.thread_id = $1 AND ($2::int IS NULL OR t.ordinal < $2)
          ORDER BY t.ordinal DESC LIMIT $3",
    )
    .bind(thread)
    .bind(before)
    .bind(i64::from(limit) + 1)
    .fetch_all(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let most = usize::try_from(limit).unwrap_or(usize::MAX);
    let more = rows.len() > most;
    let mut turns: Vec<TurnUsageRow> = rows
        .into_iter()
        .take(most)
        .map(|row| TurnUsageRow {
            turn_id: row.id,
            ordinal: i64::from(row.ordinal),
            started_at: row.started_at.map(Time::new),
            completed_at: row.completed_at.map(Time::new),
            model: row.model,
            model_display: row.model_display,
            usage: read::<TokenUsage>(row.usage.as_ref()),
            detail: read::<TurnUsageDetail>(row.usage_detail.as_ref()),
        })
        .collect();
    turns.reverse();
    let next_before = if more {
        turns.first().map(|turn| turn.ordinal)
    } else {
        None
    };
    Ok(ThreadTurnUsage {
        harness_kind,
        harness,
        reports,
        turns,
        next_before,
    })
}

/// What one turn of a thread was counted at in the ledger, by model; `None` for a turn the thread does not
/// have.
pub async fn turn_cost(
    conn: &mut Conn,
    thread: Id,
    turn: Id,
) -> Result<Option<TurnCost>, EngineError> {
    let known: Option<Id> =
        sqlx::query_scalar("SELECT id FROM turns WHERE id = $1 AND thread_id = $2")
            .bind(turn)
            .bind(thread)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    if known.is_none() {
        return Ok(None);
    }
    let mut observations = ledger::events_of_turn(conn, turn).await?;
    let per_request = observations
        .iter()
        .any(|row| row.origin == ledger::origin::HARNESS && row.scope == ledger::scope::REQUEST);
    observations.retain(|row| {
        row.origin != ledger::origin::FORWARDER
            && row.purpose == ledger::purpose::TURN
            && (!per_request || row.scope == ledger::scope::REQUEST)
    });
    observations.sort_by_key(|row| row.timing().asked.map(|at| at.get()).unwrap_or(row.at));
    let measured: Vec<_> = observations.iter().map(|row| row.timing()).collect();
    let performance = if measured.iter().any(|timing| !timing.is_unsupported()) {
        type TurnBounds = (
            Option<chrono::DateTime<chrono::Utc>>,
            Option<chrono::DateTime<chrono::Utc>>,
        );
        let times: Option<TurnBounds> =
            sqlx::query_as("SELECT started_at,completed_at FROM turns WHERE id=$1")
                .bind(turn)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        let (start, end) = times.unwrap_or_default();
        let (model_ms, tool_ms) = orch_api_contract::performance::turn_times(
            start.map(Into::into),
            end.map(Into::into),
            &measured,
        );
        let main: Vec<_> = measured.iter().filter(|timing| !timing.subagent).collect();
        let first = main.first().copied();
        Some(orch_api_contract::context::TurnPerformance {
            model_ms,
            tool_ms,
            first_output_ms: first.and_then(|timing| timing.first_token_ms.value().copied()),
            speed_milli: orch_api_contract::performance::median(
                main.iter()
                    .filter_map(|timing| timing.speed_milli.value().copied())
                    .collect(),
            ),
            requests: measured.len() as i64,
            reasoning_hidden: first.is_some_and(|timing| timing.reasoning_hidden),
            tool_includes_subagents: measured.iter().any(|timing| timing.subagent),
            tool_includes_unmeasured: main
                .iter()
                .any(|timing| timing.request_ms.value().is_none()),
        })
    } else {
        None
    };
    let rows: Vec<CostRow> = sqlx::query_as(
        "SELECT e.model,
                (array_agg(pm.display_name ORDER BY e.at DESC, e.id DESC) FILTER (WHERE btrim(pm.display_name) <> ''))[1] AS model_display,
                sum(e.input_tokens)::bigint AS input_tokens, sum(e.cached_input_tokens)::bigint AS cached_input_tokens,
                sum(e.cache_write_tokens)::bigint AS cache_write_tokens, sum(e.output_tokens)::bigint AS output_tokens,
                sum(e.reasoning_tokens)::bigint AS reasoning_tokens, sum(e.total_tokens)::bigint AS total_tokens,
                (sum(e.credits) FILTER (WHERE e.credits IS NOT NULL))::text AS credits, max(e.rate_source) AS rate_source
           FROM usage_events e LEFT JOIN provider_models pm ON pm.provider_id = e.provider_id AND pm.model_name = e.model
          WHERE e.turn_id = $1 AND e.counted GROUP BY e.model ORDER BY e.model",
    )
    .bind(turn)
    .fetch_all(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let (credits, estimated_credits, unrated_tokens): (Option<String>, String, i64) =
        sqlx::query_as(
            "SELECT (sum(credits) FILTER (WHERE credits IS NOT NULL))::text,
                COALESCE(sum(credits) FILTER (WHERE rate_source = 'catalogue'), 0)::text,
                COALESCE(sum(total_tokens) FILTER (WHERE credits IS NULL), 0)::bigint
           FROM usage_events WHERE turn_id = $1 AND counted",
        )
        .bind(turn)
        .fetch_one(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(Some(TurnCost {
        performance,
        turn_id: turn,
        rows: rows
            .into_iter()
            .map(|row| TurnCostRow {
                model: row.model,
                model_display: row.model_display,
                tokens: TokenUsage {
                    input_tokens: row.input_tokens,
                    cached_input_tokens: row.cached_input_tokens,
                    cache_write_tokens: row.cache_write_tokens,
                    output_tokens: row.output_tokens,
                    reasoning_tokens: row.reasoning_tokens,
                    total_tokens: row.total_tokens,
                },
                credits: row.credits,
                rate_source: row.rate_source,
            })
            .collect(),
        credits,
        estimated_credits,
        unrated_tokens,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Time {
        Time::new(chrono::DateTime::from_timestamp(1_791_700_000, 0).unwrap())
    }

    fn part(kind: ContextPartKind, tokens: i64) -> ContextPart {
        ContextPart {
            kind,
            tokens,
            count: ContextCount::Orch,
            rows: Vec::new(),
        }
    }

    #[test]
    fn the_estimate_follows_the_measures_of_each_family() {
        // 4 820 bytes of prose are a thousand tokens of a GPT model; the same are about 1 456 of a Claude model.
        let prose = "a".repeat(4_820);
        assert_eq!(
            estimate_text(
                &prose,
                crate::token_density::of("gpt-6-sol"),
                TextKind::Prose
            ),
            1_000
        );
        assert_eq!(
            estimate_text(
                &prose,
                crate::token_density::of("claude-sonnet-5"),
                TextKind::Prose
            ),
            1_457
        );
        // Characters beyond ASCII count by themselves: 132 Chinese characters are about a hundred GPT tokens.
        let chinese = "字".repeat(132);
        assert_eq!(
            estimate_text(
                &chinese,
                crate::token_density::of("gpt-6-sol"),
                TextKind::Prose
            ),
            100
        );
        assert_eq!(
            estimate_bytes(
                4_370,
                crate::token_density::of("gpt-6-sol"),
                TextKind::Structured
            ),
            1_000
        );
        assert_eq!(
            estimate_images(2, crate::token_density::of("claude-sonnet-5")),
            2_098
        );
        assert_eq!(
            estimate_text("", crate::token_density::of("ollama"), TextKind::Prose),
            0
        );
    }

    #[test]
    fn the_harness_s_own_part_is_what_the_reported_size_holds_beyond_orch_s_estimates() {
        let fill = compose(
            10_000,
            Some(258_400),
            Some(244_800),
            vec![
                part(ContextPartKind::ToolCalls, 6_000),
                part(ContextPartKind::OrchInstructions, 1_500),
            ],
            at(),
        );
        assert_eq!(fill.parts_total(), 10_000);
        assert!(!fill.scaled);
        assert_eq!(fill.parts[0].kind, ContextPartKind::Harness);
        assert_eq!(fill.parts[0].tokens, 2_500);
        assert_eq!(
            fill.parts[1].kind,
            ContextPartKind::OrchInstructions,
            "in the order of the bar"
        );
        assert!(
            fill.parts
                .iter()
                .all(|part| part.count == ContextCount::Orch)
        );
    }

    #[test]
    fn estimates_above_the_reported_size_are_scaled_down_to_it_and_say_so() {
        let fill = compose(
            1_000,
            None,
            None,
            vec![
                part(ContextPartKind::ToolCalls, 1_500),
                part(ContextPartKind::AgentMessages, 499),
                part(ContextPartKind::Files, 1),
            ],
            at(),
        );
        assert!(fill.scaled);
        assert_eq!(
            fill.parts_total(),
            1_000,
            "the parts add up to what the harness reported, rounding and all"
        );
        assert!(
            fill.parts
                .iter()
                .all(|part| part.kind != ContextPartKind::Harness)
        );
        let tools = fill
            .parts
            .iter()
            .find(|part| part.kind == ContextPartKind::ToolCalls)
            .unwrap();
        assert_eq!(
            tools.tokens,
            750 + 1,
            "the largest part takes what rounding left"
        );
    }

    #[test]
    fn what_orch_sent_and_saw_becomes_its_parts() {
        let block = |key: &str, source: &str, text: &str| SentBlock {
            key: key.to_owned(),
            source: source.to_owned(),
            text: text.to_owned(),
            total_len: None,
        };
        let mut tools = BTreeMap::new();
        tools.insert("exec_command".to_owned(), (3, 437, 4_370));
        let seen = Seen {
            session: vec![
                block("preamble", "orch", &"a".repeat(482)),
                block("send_message", "tool", &"{".repeat(437)),
            ],
            inputs: vec![
                block("where_you_are", "orch", &"b".repeat(964)),
                block("input", "input", &"c".repeat(482)),
            ],
            said: 4_820,
            tools,
            images: 1,
        };
        let parts = estimates_of(&seen, crate::token_density::of("gpt-6-sol"));
        let tokens = |kind: ContextPartKind| {
            parts
                .iter()
                .find(|part| part.kind == kind)
                .map_or(0, |part| part.tokens)
        };
        assert_eq!(tokens(ContextPartKind::OrchInstructions), 100 + 200);
        assert_eq!(tokens(ContextPartKind::OrchTools), 100);
        assert_eq!(tokens(ContextPartKind::PeopleMessages), 100);
        assert_eq!(tokens(ContextPartKind::AgentMessages), 1_000);
        assert_eq!(tokens(ContextPartKind::ToolCalls), 100 + 1_000);
        assert_eq!(tokens(ContextPartKind::Files), 765);
        let calls = parts
            .iter()
            .find(|part| part.kind == ContextPartKind::ToolCalls)
            .unwrap();
        assert_eq!(
            calls.rows[0].calls,
            Some(100),
            "a tool's calls apart from its results"
        );
    }

    #[test]
    fn orch_s_share_of_a_harness_breakdown_is_a_row_of_the_part_that_holds_it() {
        let seen = Seen {
            session: vec![SentBlock {
                key: "preamble".to_owned(),
                source: "orch".to_owned(),
                text: "a".repeat(3_310),
                total_len: None,
            }],
            ..Seen::default()
        };
        let fill = ContextFill {
            count: ContextCount::Estimated,
            used: 20_000,
            window: Some(200_000),
            compact_at: Some(167_000),
            parts: vec![ContextPart {
                kind: ContextPartKind::Harness,
                tokens: 15_000,
                count: ContextCount::Estimated,
                rows: Vec::new(),
            }],
            scaled: false,
            at: at(),
        };
        let fill = with_orch_share(fill, &seen, crate::token_density::of("claude-sonnet-5"));
        let row = &fill.parts[0].rows[0];
        assert_eq!(
            (row.kind, row.tokens, row.count),
            (ContextRowKind::OrchShare, 1_000, ContextCount::Orch)
        );
        assert_eq!(
            fill.parts[0].tokens, 15_000,
            "the harness's count of the part stays as it is"
        );
    }
}
