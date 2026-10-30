//! What fills the context, as the harness breaks it down on request (`get_context_usage`), in Orch's words.
//!
//! The harness answers the control request `get_context_usage` with what its `/context` shows: categories,
//! each `used`, `free`, `buffer` (the reserve it keeps for shortening the conversation) or `deferred` (tools
//! loaded on demand, not in the context); the MCP tools, memory files, sub-agents and skills one by one; and,
//! where it has it, the messages split into what the user and the agent said, tool calls and their results by
//! tool, and attachments. With `detail: "summary"` it answers from its last response and its own estimates and
//! no request leaves the machine; with `"full"` it counts each part with the vendor's token counter.
//!
//! The parts are told apart by the structure of the answer, never by the display names of its categories: the
//! tools of Orch's own tool server are Orch's tools, those of the executor are the harness's tools carried by
//! Orch, the other servers' are MCP tools; and what the `used` categories hold beyond every part the answer
//! attributes is the harness's own system prompt and tools.

use orch_agent_contract::events::{
    ContextCount, ContextFill, ContextPart, ContextPartKind, ContextRow, ContextRowKind,
};
use orch_agent_contract::loopback::{EXEC_SERVER, ORCH_SERVER};
use orch_types::{Json, Time};

/// The control request that asks for the breakdown, at the detail given.
#[must_use]
pub fn request(count: ContextCount) -> Json {
    let detail = if count == ContextCount::Counted {
        "full"
    } else {
        "summary"
    };
    serde_json::json!({ "subtype": "get_context_usage", "detail": detail })
}

fn number(value: &Json, member: &str) -> i64 {
    value.get(member).and_then(Json::as_i64).unwrap_or(0).max(0)
}

fn list<'a>(value: &'a Json, member: &str) -> impl Iterator<Item = &'a Json> {
    value
        .get(member)
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
}

fn text<'a>(value: &'a Json, member: &str) -> &'a str {
    value.get(member).and_then(Json::as_str).unwrap_or_default()
}

/// A part with its rows, the largest row first; `None` when it holds nothing.
fn part(
    kind: ContextPartKind,
    tokens: i64,
    count: ContextCount,
    mut rows: Vec<ContextRow>,
) -> Option<ContextPart> {
    rows.retain(|row| row.tokens > 0);
    rows.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    (tokens > 0).then_some(ContextPart {
        kind,
        tokens,
        count,
        rows,
    })
}

fn row(
    kind: ContextRowKind,
    name: String,
    tokens: i64,
    calls: Option<i64>,
    count: ContextCount,
) -> ContextRow {
    ContextRow {
        kind,
        name,
        tokens,
        calls,
        count,
    }
}

/// What the answer to `get_context_usage` says fills the context; `None` for an answer that is not one.
#[must_use]
pub fn fill_of(answer: &Json, count: ContextCount, at: Time) -> Option<ContextFill> {
    let categories: Vec<&Json> = list(answer, "categories").collect();
    if categories.is_empty() && answer.get("totalTokens").is_none() {
        return None;
    }
    let of_kind = |kind: &str| -> i64 {
        categories
            .iter()
            .filter(|category| text(category, "kind") == kind)
            .map(|category| number(category, "tokens"))
            .sum()
    };
    let used_by_categories = of_kind("used");
    let total = answer
        .get("totalTokens")
        .and_then(Json::as_i64)
        .unwrap_or(used_by_categories)
        .max(0);
    let window = answer
        .get("maxTokens")
        .or_else(|| answer.get("rawMaxTokens"))
        .and_then(Json::as_i64)
        .filter(|window| *window > 0);
    let reserve = of_kind("buffer");
    let compact_at = if answer.get("isAutoCompactEnabled").and_then(Json::as_bool) == Some(false) {
        None
    } else {
        answer
            .get("autoCompactThreshold")
            .and_then(Json::as_i64)
            .filter(|at| *at > 0)
            .or_else(|| {
                window
                    .filter(|_| reserve > 0)
                    .map(|window| window - reserve)
            })
    };

    // Tools of MCP servers: Orch's own, the executor's (the harness's tools, carried by Orch), and the others.
    // A tool loaded on demand that is not loaded is not in the context.
    let mut orch_tools = Vec::new();
    let mut harness_rows = Vec::new();
    let mut mcp_rows = Vec::new();
    for tool in list(answer, "mcpTools") {
        if tool.get("isLoaded").and_then(Json::as_bool) == Some(false) {
            continue;
        }
        let tokens = number(tool, "tokens");
        let name = text(tool, "name").to_owned();
        match text(tool, "serverName") {
            ORCH_SERVER => orch_tools.push(row(ContextRowKind::McpTool, name, tokens, None, count)),
            EXEC_SERVER => harness_rows.push(row(ContextRowKind::Tool, name, tokens, None, count)),
            server => mcp_rows.push(row(
                ContextRowKind::McpTool,
                format!("{server}/{name}"),
                tokens,
                None,
                count,
            )),
        }
    }
    let memory_rows: Vec<ContextRow> = list(answer, "memoryFiles")
        .map(|file| {
            row(
                ContextRowKind::File,
                text(file, "path").to_owned(),
                number(file, "tokens"),
                None,
                count,
            )
        })
        .collect();
    let mut agent_rows: Vec<ContextRow> = list(answer, "agents")
        .map(|agent| {
            row(
                ContextRowKind::Agent,
                text(agent, "agentType").to_owned(),
                number(agent, "tokens"),
                None,
                count,
            )
        })
        .collect();
    let skills = answer.get("skills").unwrap_or(&Json::Null);
    agent_rows.extend(list(skills, "skillFrontmatter").map(|skill| {
        row(
            ContextRowKind::Agent,
            text(skill, "name").to_owned(),
            number(skill, "tokens"),
            None,
            count,
        )
    }));
    let commands = answer
        .get("slashCommands")
        .map_or(0, |commands| number(commands, "tokens"));
    let skills_agents = number(skills, "tokens")
        + list(answer, "agents")
            .map(|agent| number(agent, "tokens"))
            .sum::<i64>()
        + commands;

    let sum = |rows: &[ContextRow]| rows.iter().map(|row| row.tokens).sum::<i64>();
    let mut parts: Vec<ContextPart> = Vec::new();
    parts.extend(part(
        ContextPartKind::OrchTools,
        sum(&orch_tools),
        count,
        orch_tools,
    ));
    parts.extend(part(
        ContextPartKind::McpTools,
        sum(&mcp_rows),
        count,
        mcp_rows,
    ));
    parts.extend(part(
        ContextPartKind::MemoryFiles,
        sum(&memory_rows),
        count,
        memory_rows,
    ));
    parts.extend(part(
        ContextPartKind::SkillsAgents,
        skills_agents,
        count,
        agent_rows,
    ));
    let executor_tools = sum(&harness_rows);
    let Some(messages) = answer
        .get("messageBreakdown")
        .filter(|breakdown| breakdown.is_object())
    else {
        // Without the messages split, what is left holds the conversation with the harness's own system prompt
        // and tools, and they cannot be told apart: it is one part, said to be so.
        let rest = (total - total_of(&parts)).max(0);
        parts.extend(part(ContextPartKind::Other, rest, count, harness_rows));
        return Some(finish(parts, total, window, compact_at, count, at));
    };
    let tool_rows: Vec<ContextRow> = list(messages, "toolCallsByType")
        .map(|tool| {
            let calls = number(tool, "callTokens");
            let results = number(tool, "resultTokens");
            row(
                ContextRowKind::Tool,
                text(tool, "name").to_owned(),
                calls + results,
                Some(calls),
                count,
            )
        })
        .collect();
    let attachment_rows: Vec<ContextRow> = list(messages, "attachmentsByType")
        .map(|attachment| {
            row(
                ContextRowKind::Attachment,
                text(attachment, "name").to_owned(),
                number(attachment, "tokens"),
                None,
                count,
            )
        })
        .collect();
    parts.extend(part(
        ContextPartKind::PeopleMessages,
        number(messages, "userMessageTokens"),
        count,
        Vec::new(),
    ));
    parts.extend(part(
        ContextPartKind::AgentMessages,
        number(messages, "assistantMessageTokens"),
        count,
        Vec::new(),
    ));
    parts.extend(part(
        ContextPartKind::ToolCalls,
        number(messages, "toolCallTokens") + number(messages, "toolResultTokens"),
        count,
        tool_rows,
    ));
    parts.extend(part(
        ContextPartKind::Files,
        number(messages, "attachmentTokens"),
        count,
        attachment_rows,
    ));
    parts.extend(part(
        ContextPartKind::Other,
        number(messages, "redirectedContextTokens") + number(messages, "unattributedTokens"),
        count,
        Vec::new(),
    ));
    // What the `used` categories hold beyond every part attributed: the harness's system prompt and its tools,
    // the executor's among them.
    let harness = (total - total_of(&parts)).max(executor_tools);
    parts.extend(part(ContextPartKind::Harness, harness, count, harness_rows));
    Some(finish(parts, total, window, compact_at, count, at))
}

fn total_of(parts: &[ContextPart]) -> i64 {
    parts.iter().map(|part| part.tokens).sum()
}

/// The fill, its parts in the order of the bar. Where the parts came to more than the harness's total (its
/// estimates of the parts are its own), the fill says the larger, so that its parts add up to it.
fn finish(
    mut parts: Vec<ContextPart>,
    total: i64,
    window: Option<i64>,
    compact_at: Option<i64>,
    count: ContextCount,
    at: Time,
) -> ContextFill {
    parts.sort_by_key(|part| bar_order(part.kind));
    let used = total_of(&parts).max(total);
    ContextFill {
        count,
        used,
        window,
        compact_at,
        parts,
        scaled: false,
        at,
    }
}

/// Where a part stands in the bar: what the harness and Orch give every request first, the conversation after.
#[must_use]
pub fn bar_order(kind: ContextPartKind) -> u8 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at() -> Time {
        Time::new(chrono::DateTime::from_timestamp(1_791_700_000, 0).unwrap())
    }

    fn answer() -> Json {
        json!({
            "categories": [
                { "name": "System prompt", "tokens": 3_000, "color": "x", "kind": "used" },
                { "name": "System tools", "tokens": 12_000, "color": "x", "kind": "used" },
                { "name": "MCP tools", "tokens": 5_000, "color": "x", "kind": "used" },
                { "name": "MCP tools (loaded on demand)", "tokens": 9_000, "color": "x", "kind": "deferred", "isDeferred": true },
                { "name": "Memory files", "tokens": 1_000, "color": "x", "kind": "used" },
                { "name": "Messages", "tokens": 40_000, "color": "x", "kind": "used" },
                { "name": "Free space", "tokens": 106_000, "color": "x", "kind": "free" },
                { "name": "Autocompact buffer", "tokens": 33_000, "color": "x", "kind": "buffer" }
            ],
            "totalTokens": 61_000, "maxTokens": 200_000, "rawMaxTokens": 200_000, "percentage": 31,
            "isAutoCompactEnabled": true, "autoCompactThreshold": 167_000,
            "memoryFiles": [{ "path": "/w/CLAUDE.md", "type": "Project", "tokens": 1_000 }],
            "mcpTools": [
                { "name": "send_message", "serverName": "orch", "tokens": 2_500, "isLoaded": true },
                { "name": "list_threads", "serverName": "orch", "tokens": 1_500 },
                { "name": "browser_open", "serverName": "playwright", "tokens": 1_000 },
                { "name": "browser_click", "serverName": "playwright", "tokens": 9_000, "isLoaded": false }
            ],
            "agents": [],
            "messageBreakdown": {
                "toolCallTokens": 6_000, "toolResultTokens": 24_000, "attachmentTokens": 2_000,
                "assistantMessageTokens": 5_000, "userMessageTokens": 2_500, "redirectedContextTokens": 0,
                "unattributedTokens": 500,
                "toolCallsByType": [
                    { "name": "Bash", "callTokens": 4_000, "resultTokens": 20_000 },
                    { "name": "Read", "callTokens": 2_000, "resultTokens": 4_000 }
                ],
                "attachmentsByType": [{ "name": "image", "tokens": 2_000 }]
            },
            "apiUsage": { "input_tokens": 10, "output_tokens": 300, "cache_creation_input_tokens": 1_000, "cache_read_input_tokens": 59_990 }
        })
    }

    fn tokens_of(fill: &ContextFill, kind: ContextPartKind) -> i64 {
        fill.parts
            .iter()
            .find(|part| part.kind == kind)
            .map_or(0, |part| part.tokens)
    }

    #[test]
    fn the_parts_are_told_apart_by_the_answer_s_structure_and_add_up_to_its_total() {
        let fill = fill_of(&answer(), ContextCount::Estimated, at()).unwrap();
        assert_eq!(fill.used, 61_000);
        assert_eq!(fill.parts_total(), fill.used);
        assert_eq!(
            (fill.window, fill.compact_at),
            (Some(200_000), Some(167_000))
        );
        assert_eq!(
            tokens_of(&fill, ContextPartKind::OrchTools),
            4_000,
            "the tools of Orch's own server"
        );
        assert_eq!(
            tokens_of(&fill, ContextPartKind::McpTools),
            1_000,
            "a tool that is not loaded is not counted"
        );
        assert_eq!(tokens_of(&fill, ContextPartKind::MemoryFiles), 1_000);
        assert_eq!(tokens_of(&fill, ContextPartKind::PeopleMessages), 2_500);
        assert_eq!(tokens_of(&fill, ContextPartKind::AgentMessages), 5_000);
        assert_eq!(tokens_of(&fill, ContextPartKind::ToolCalls), 30_000);
        assert_eq!(tokens_of(&fill, ContextPartKind::Files), 2_000);
        assert_eq!(tokens_of(&fill, ContextPartKind::Other), 500);
        // The system prompt and the system tools: what the used categories hold beyond the parts above.
        assert_eq!(tokens_of(&fill, ContextPartKind::Harness), 15_000);
        assert_eq!(
            fill.parts[0].kind,
            ContextPartKind::Harness,
            "the bar begins with what every request carries"
        );
        let tools = fill
            .parts
            .iter()
            .find(|part| part.kind == ContextPartKind::ToolCalls)
            .unwrap();
        assert_eq!(
            (
                tools.rows[0].name.as_str(),
                tools.rows[0].tokens,
                tools.rows[0].calls
            ),
            ("Bash", 24_000, Some(4_000))
        );
        assert!(
            fill.parts
                .iter()
                .all(|part| part.count == ContextCount::Estimated)
        );
    }

    #[test]
    fn a_count_by_the_vendor_is_marked_counted_and_asked_for_in_full() {
        let fill = fill_of(&answer(), ContextCount::Counted, at()).unwrap();
        assert!(
            fill.parts
                .iter()
                .all(|part| part.count == ContextCount::Counted)
        );
        assert_eq!(request(ContextCount::Counted)["detail"], "full");
        assert_eq!(request(ContextCount::Estimated)["detail"], "summary");
    }

    #[test]
    fn without_the_messages_split_the_rest_is_one_part_that_says_so() {
        let mut bare = answer();
        bare.as_object_mut().unwrap().remove("messageBreakdown");
        let fill = fill_of(&bare, ContextCount::Estimated, at()).unwrap();
        assert_eq!(tokens_of(&fill, ContextPartKind::Harness), 0);
        assert_eq!(tokens_of(&fill, ContextPartKind::Other), 61_000 - 6_000);
        assert_eq!(fill.parts_total(), fill.used);
    }

    #[test]
    fn a_reserve_without_a_threshold_says_where_the_conversation_is_shortened() {
        let mut answer = answer();
        answer
            .as_object_mut()
            .unwrap()
            .remove("autoCompactThreshold");
        let fill = fill_of(&answer, ContextCount::Estimated, at()).unwrap();
        assert_eq!(fill.compact_at, Some(200_000 - 33_000));
        answer["isAutoCompactEnabled"] = json!(false);
        assert_eq!(
            fill_of(&answer, ContextCount::Estimated, at())
                .unwrap()
                .compact_at,
            None
        );
    }

    #[test]
    fn an_answer_that_is_none_is_no_fill() {
        assert!(
            fill_of(
                &json!({ "error": "not supported" }),
                ContextCount::Estimated,
                at()
            )
            .is_none()
        );
    }
}
