//! What a turn's tokens are made of and what fills a thread's context: each harness's report carried whole, and Orch's own estimate by part where a harness reports none.
//!
//! A harness reports a turn's tokens as it counts them (input with its cached and cache-write parts, output
//! with its reasoning part), and some report more: each model's share, their sub-agents' share, the size of
//! the last prompt against the window of the model, and where they shorten the conversation. A turn keeps all
//! of it ([`TurnUsageDetail`]) beside its totals.
//!
//! What fills the context is a [`ContextFill`]: the prompt of the last request, split into parts in Orch's
//! words. Where the harness breaks it down itself (Claude Code, on request), its parts are carried and each is
//! marked as counted or as estimated by the harness. Where it does not (Codex), Orch estimates the parts it can
//! see (its own instructions and tools, what people and the agent said, the tool calls and their results, the
//! files and images) from what it recorded sending and the items it observed, and names what the reported
//! total holds beyond them as the harness's own: Orch never invents a share for something it cannot see. The
//! parts of a fill add up to its `used`.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::routes::Access::Permission;
use crate::threads::TokenUsage;

/// How the tokens of a part were counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextCount {
    /// Counted by the harness, with its vendor's token counter.
    Counted,
    /// Estimated by the harness, from its last response and its own estimates.
    Estimated,
    /// Estimated by Orch, from what it sent and what it saw, with one estimate for each family of models.
    Orch,
}

/// A part of what fills a context, in Orch's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextPartKind {
    /// The harness's own system prompt and tools, and whatever else it adds that Orch neither sent nor saw.
    Harness,
    /// Orch's instructions: its introduction, the standing instructions of the thread's places, where the agent
    /// is.
    OrchInstructions,
    /// Orch's own tools.
    OrchTools,
    /// The tools of other MCP servers.
    McpTools,
    /// The instruction and memory files the harness reads by itself.
    MemoryFiles,
    /// The skills and sub-agents the harness offers its model.
    SkillsAgents,
    /// What people said.
    PeopleMessages,
    /// What the agent said.
    AgentMessages,
    /// Tool calls and their results.
    ToolCalls,
    /// Files and images.
    Files,
    /// Conversation the harness keeps without saying what it is.
    Other,
}

/// What a row inside a part is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextRowKind {
    /// A tool, with its calls apart from its results where that is known.
    Tool,
    /// A file the harness reads as instructions or memory.
    File,
    /// A tool of an MCP server.
    McpTool,
    /// A skill or a sub-agent the harness offers.
    Agent,
    /// The attachments of one kind.
    Attachment,
    /// Orch's own share of a part that the harness counts as its own: Orch's instructions inside the
    /// harness's system prompt.
    OrchShare,
}

/// A row inside a part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextRow {
    /// What it is.
    pub kind: ContextRowKind,
    /// Its name: a tool's, a file's path, `server/tool` for a tool of an MCP server.
    pub name: String,
    /// Its tokens.
    pub tokens: i64,
    /// For a tool: how many of its tokens are its calls, the rest being their results; `null` where that is
    /// not known.
    pub calls: Option<i64>,
    /// How they were counted.
    pub count: ContextCount,
}

/// One part of what fills a context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextPart {
    /// Which part.
    pub kind: ContextPartKind,
    /// Its tokens.
    pub tokens: i64,
    /// How they were counted.
    pub count: ContextCount,
    /// What it is made of, the largest first; empty where nothing finer is known.
    pub rows: Vec<ContextRow>,
}

/// What fills a thread's context, by part, as it stood at the end of a turn (or when it was last counted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextFill {
    /// How the whole was counted; a part or a row may say otherwise.
    pub count: ContextCount,
    /// The tokens that fill it: the prompt of the last request, its cached and cache-write parts included.
    pub used: i64,
    /// The window `used` is measured against; `null` where nothing reported one.
    pub window: Option<i64>,
    /// Where the harness shortens the conversation; `null` where that is not known.
    pub compact_at: Option<i64>,
    /// The parts, in the order of the bar; their tokens add up to `used`.
    pub parts: Vec<ContextPart>,
    /// Whether Orch's estimates came to more than the harness reported, and were scaled down to fit it.
    pub scaled: bool,
    /// When it was taken.
    pub at: Time,
}

impl ContextFill {
    /// The tokens of all parts together.
    #[must_use]
    pub fn parts_total(&self) -> i64 {
        self.parts.iter().map(|part| part.tokens).sum()
    }
}

/// One stream's share of a turn's tokens: a sub-agent's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StreamTokens {
    /// The stream of its items, `sub:<id>`.
    pub stream: String,
    /// What it used.
    pub tokens: TokenUsage,
}

/// One model's share of a turn's tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelTokens {
    /// The model, as the harness named it.
    pub model: String,
    /// What it used.
    pub tokens: TokenUsage,
}

/// A time the harness shortened its conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextShortening {
    /// When.
    pub at: Time,
    /// The size of the conversation before it, where the harness said or a prompt showed it.
    pub before: Option<i64>,
    /// Its size after it: the first prompt after it.
    pub after: Option<i64>,
    /// Whether the harness did it by itself (`true`) or was asked to (`false`); `null` where it did not say.
    pub automatic: Option<bool>,
}

/// What a turn used beyond its totals, as its harness reported it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnUsageDetail {
    /// API time reported for the whole turn; excludes tool and human waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_time_ms: Option<i64>,
    /// Its sub-agents' share of the turn's tokens, which the totals include; `null` where the adapter does not
    /// know it.
    pub subagents: Option<TokenUsage>,
    /// Each sub-agent's share, by the stream of its items.
    pub by_subagent: Vec<StreamTokens>,
    /// Each model's share, where the harness reports its use by model.
    pub models: Vec<ModelTokens>,
    /// The window of the model's context, as the harness reported it.
    pub context_window: Option<i64>,
    /// The size of the turn's last prompt: its input with the cached and cache-write parts.
    pub context_used: Option<i64>,
    /// Where the harness shortens the conversation.
    pub compact_at: Option<i64>,
    /// The times the harness shortened its conversation during the turn.
    pub shortenings: Vec<ContextShortening>,
    /// Whether the harness breaks down what fills the context itself; where it does not, Orch estimates it.
    #[serde(default)]
    pub breakdown: bool,
}

impl TurnUsageDetail {
    /// Whether it holds nothing more than an empty one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What an adapter reports of a turn's use beyond its totals, beside a report of the totals: carried from the
/// harness to the stored turn as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnUsageReport {
    /// Whole-turn API time, where the harness reports only a turn total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_time_ms: Option<i64>,
    /// The sub-agents' share of the turn's tokens, which they include.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<TokenUsage>,
    /// Each sub-agent's share, by the stream of its items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_subagent: Vec<StreamTokens>,
    /// Each model's share, where the harness reports its use by model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelTokens>,
    /// Where the harness shortens the conversation for the window in force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_at: Option<i64>,
    /// A shortening of the conversation since the report before, with its sizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortened: Option<ContextShortening>,
    /// What fills the context, by part, where the harness breaks it down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextFill>,
    /// Whether the harness breaks down what fills the context itself, so that a breakdown follows the end of
    /// a turn; where it does not, Orch estimates it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub breakdown: bool,
}

/// What a harness reports of its use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextReports {
    /// Its tokens, as it works.
    pub tokens: bool,
    /// The size of its prompt against the window of its model.
    pub context: bool,
    /// What fills the context, by part.
    pub breakdown: bool,
}

/// How a fill is asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextDetail {
    /// As it was kept at the end of the last turn.
    #[default]
    Kept,
    /// Counted by the harness now, where it counts on request and its session is open between turns: once for
    /// each turn, kept after that.
    Count,
}

/// The query of [`ThreadContextGet`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadContextGetQuery {
    /// `count` asks the harness to count; `kept` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ContextDetail>,
}

/// What fills a thread's context now, and when it was last shortened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadContext {
    /// The harness the thread's agent runs (`codex`, `claude-code`, …); `null` without a binding.
    pub harness_kind: Option<String>,
    /// The same harness, named.
    pub harness: Option<crate::resources::HarnessKindName>,
    /// What that harness reports.
    pub reports: ContextReports,
    /// What filled the context at the end of the latest turn that has it; `null` where nothing reported it.
    pub fill: Option<ContextFill>,
    /// The turn it was taken at the end of.
    pub turn_id: Option<Id>,
    /// The last time the conversation was shortened, and how much that freed.
    pub shortened: Option<ContextShortening>,
    /// Whether the harness can be asked to count it now: it counts on request, the thread is idle with its
    /// session open, and the fill was not counted since the last turn.
    pub can_count: bool,
}

/// One turn's use, for the thread's Usage view over time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnUsageRow {
    /// The turn.
    pub turn_id: Id,
    /// Its number in the thread.
    pub ordinal: i64,
    /// When it began.
    pub started_at: Option<Time>,
    /// When it ended; `null` while it runs.
    pub completed_at: Option<Time>,
    /// The model's routing name.
    pub model: String,
    /// The model's display name.
    pub model_display: String,
    /// Its tokens; `null` where nothing reported any.
    pub usage: Option<TokenUsage>,
    /// What its harness reported beyond them.
    pub detail: Option<TurnUsageDetail>,
}

/// The query of [`ThreadTurnUsageList`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadTurnUsageListQuery {
    /// Only turns numbered below this one: the `next_before` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    /// How many turns, at most [`TURN_USAGE_PAGE_MAX`]; [`TURN_USAGE_PAGE_DEFAULT`] unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// How many turns a page of a thread's use holds unless asked: enough for a chart of a long working day.
pub const TURN_USAGE_PAGE_DEFAULT: u32 = 200;
/// The most turns one page of a thread's use holds: a chart of more is not read turn by turn.
pub const TURN_USAGE_PAGE_MAX: u32 = 1000;

/// A page of a thread's turns with their use, the latest last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadTurnUsage {
    /// The harness the thread's agent runs; `null` without a binding.
    pub harness_kind: Option<String>,
    /// The same harness, named.
    pub harness: Option<crate::resources::HarnessKindName>,
    /// What that harness reports.
    pub reports: ContextReports,
    /// The turns, in their order.
    pub turns: Vec<TurnUsageRow>,
    /// The `before` of the next, older page; `null` at the first turn.
    pub next_before: Option<i64>,
}

/// What one model of a turn was counted at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnCostRow {
    /// The model's routing name, as recorded.
    pub model: String,
    /// The provider's declared display name, chosen from the newest counted row that still has one.
    /// Absent where the model no longer has a declared name; the routing identity remains separate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_display: Option<String>,
    /// Its tokens, as counted.
    pub tokens: TokenUsage,
    /// Its credits, as a decimal string; `null` where there was no rate.
    pub credits: Option<String>,
    /// Where its rate came from: `provider`, `rate_card` or `catalogue`; `null` where there was none.
    pub rate_source: Option<String>,
}

/// What a turn cost: the counted rows of the ledger for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnCost {
    /// Model/tool time and request performance, where this turn has request observations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance: Option<TurnPerformance>,
    /// The turn.
    pub turn_id: Id,
    /// The rows, by model.
    pub rows: Vec<TurnCostRow>,
    /// Their credits together, as a decimal string; `null` where no row had a rate.
    pub credits: Option<String>,
    /// The part of `credits` the shipped catalogue estimated, as a decimal string.
    pub estimated_credits: String,
    /// The tokens of the rows that had no rate.
    pub unrated_tokens: i64,
}

/// Timed main-stream requests and the remaining turn time, including sub-agent/tool work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnPerformance {
    /// Sum of timed main-stream requests; null if none were measured.
    pub model_ms: Option<i64>,
    /// Turn time less model time, never inferred when model time is unknown.
    pub tool_ms: Option<i64>,
    /// First request's time to first output, where known.
    pub first_output_ms: Option<i64>,
    /// Median eligible request speed in thousandths of a token per second.
    pub speed_milli: Option<i64>,
    /// Request observations contributing to these details.
    pub requests: i64,
    /// The first request's first-output time includes hidden thinking.
    pub reasoning_hidden: bool,
    /// Tool time includes work delegated to sub-agents.
    #[serde(default)]
    pub tool_includes_subagents: bool,
    /// Tool time includes intervals without a measured main-stream request time.
    #[serde(default)]
    pub tool_includes_unmeasured: bool,
}

crate::endpoints! {
    /// Reads what fills a thread's context, by part, and when it was last shortened; with `detail=count`, has the harness count it first where it can.
    ThreadContextGet: GET "/threads/{id}/context", Permission("thread.view"), query ThreadContextGetQuery, none, json ThreadContext, 200;
    /// Lists a thread's turns with what each used, the latest last, a page at a time.
    ThreadTurnUsageList: GET "/threads/{id}/usage/turns", Permission("thread.view"), query ThreadTurnUsageListQuery, none, json ThreadTurnUsage, 200;
    /// Reads what one turn of a thread was counted at, by model.
    ThreadTurnCostGet: GET "/threads/{id}/turns/{turn_id}/cost", Permission("thread.view"), none, none, json TurnCost, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_fill_reads_and_writes_as_the_wire_has_it() {
        let wire = json!({
            "count": "estimated", "used": 142_000, "window": 200_000, "compact_at": 167_000, "scaled": false,
            "at": "2026-10-14T10:00:00.000Z",
            "parts": [
                { "kind": "harness", "tokens": 20_000, "count": "estimated", "rows": [
                    { "kind": "orch_share", "name": "orch", "tokens": 3_000, "calls": null, "count": "orch" } ] },
                { "kind": "tool_calls", "tokens": 122_000, "count": "estimated", "rows": [
                    { "kind": "tool", "name": "Bash", "tokens": 100_000, "calls": 4_000, "count": "estimated" } ] }
            ]
        });
        let fill: ContextFill = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(fill.parts_total(), fill.used);
        assert_eq!(fill.parts[0].rows[0].kind, ContextRowKind::OrchShare);
        assert_eq!(serde_json::to_value(&fill).unwrap(), wire);
    }

    #[test]
    fn a_turn_detail_keeps_the_sub_agents_models_and_shortenings_apart() {
        let tokens = json!({ "input_tokens": 10, "cached_input_tokens": 4, "output_tokens": 2, "reasoning_tokens": 1, "total_tokens": 12 });
        let wire = json!({
            "subagents": tokens, "by_subagent": [{ "stream": "sub:a", "tokens": tokens }],
            "models": [{ "model": "claude-sonnet-5", "tokens": tokens }],
            "context_window": 200_000, "context_used": 9_000, "compact_at": 167_000,
            "shortenings": [{ "at": "2026-10-14T10:00:00.000Z", "before": 160_000, "after": 30_000, "automatic": true }],
            "breakdown": true
        });
        let detail: TurnUsageDetail = serde_json::from_value(wire.clone()).unwrap();
        assert!(!detail.is_empty());
        assert_eq!(detail.by_subagent[0].tokens.total_tokens, 12);
        assert_eq!(serde_json::to_value(&detail).unwrap(), wire);
        assert!(TurnUsageDetail::default().is_empty());
    }

    #[test]
    fn the_context_query_asks_for_a_count_only_by_name() {
        let query: ThreadContextGetQuery =
            serde_json::from_value(json!({ "detail": "count" })).unwrap();
        assert_eq!(query.detail, Some(ContextDetail::Count));
        assert!(serde_json::from_value::<ThreadContextGetQuery>(json!({ "full": true })).is_err());
    }
}
