//! Neutral conversion of a carried conversation into the Responses items a new thread is told.
//!
//! **A thread's own history stays the harness's.** Codex keeps it in its home, a page at a time, and Orch
//! never copies or parses it: the same home resumes and forks a thread through the harness's own protocol,
//! at any size, and a home that moves with its environment reopens its own store. When one thread goes into
//! a different home there is no lossless route, because the protocol lists a history only as display items
//! and cannot give back what the model was told.
//!
//! **A neutral history** is the main stream of a conversation as items, one to a line. It is how such a
//! thread arrives: the engine picks the newest turns that fit the share of the new model's window, and
//! each turn becomes Responses items that `thread/inject_items` appends to the new thread: what the person
//! sent, what the agent said, and the steps of the turn (what it ran, changed and read, with what came
//! back, within bounds) as one message of the assistant before its final answer. The whole conversation
//! is written beside them as a file the new agent can read.

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::history::{Fidelity, HistoryReport, ReportBuilder};
use orch_types::Json;
use serde_json::json;

/// The directory of the adapter's own files in a home, for what it writes for the connector to take.
pub const EXPORTS_DIRECTORY: &str = "orch/exports";

/// The most of a text from one source that a step shows in the written form of a turn: 2 000 characters.
pub const STEP_TEXT_MAX_CHARS: usize = 2_000;

/// The most of a difference that a step shows in the written form of a turn: 4 000 characters.
pub const STEP_DIFF_MAX_CHARS: usize = 4_000;

/// How many matches of a search a step shows.
pub const STEP_MATCHES_SHOWN: usize = 20;

/// How many results of a search of the web a step shows.
pub const STEP_RESULTS_SHOWN: usize = 10;

// ---------------------------------------------------------------------------------------------------------
// Neutral history to Responses items

fn clip_start(text: &str, most: usize) -> (String, bool) {
    if text.chars().count() <= most {
        return (text.to_owned(), false);
    }
    (text.chars().take(most).collect(), true)
}

fn clip_end(text: &str, most: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= most {
        return (text.to_owned(), false);
    }
    (text.chars().skip(count - most).collect(), true)
}

fn fenced(language: &str, body: &str) -> String {
    format!("```{language}\n{}\n```", body.trim_end_matches('\n'))
}

fn indent(block: &str) -> String {
    block
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// How a step ended, in the words the written form has: ` · exit 1`, ` · failed`, ` · still running`.
fn ending(item: &Json) -> String {
    let status = item
        .get("status")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let tool = item.get("payload").and_then(|payload| payload.get("tool"));
    if let Some(code) = tool
        .and_then(|tool| tool.get("exit_code"))
        .and_then(Json::as_i64)
    {
        return format!(" · exit {code}");
    }
    match status {
        "failed" => " · failed".to_owned(),
        "in_progress" => " · still running".to_owned(),
        _ => String::new(),
    }
}

/// One step of a turn in the written form at `full`: a line, and under it what it was given and what came
/// back, within the bounds of that form.
fn step(item: &Json) -> Option<String> {
    let payload = item.get("payload")?;
    let title = payload
        .get("title")
        .and_then(Json::as_str)
        .unwrap_or("A step");
    let mut text = format!("- {title}{}", ending(item));
    let tool = payload.get("tool").unwrap_or(&Json::Null);
    let mut detail = Vec::new();
    let note_left_out = |detail: &mut Vec<String>, what: &str| {
        detail.push(format!("({what} was left out here)"));
    };
    match tool.get("kind").and_then(Json::as_str).unwrap_or_default() {
        "command" => {
            if let Some(cwd) = tool
                .get("cwd")
                .and_then(Json::as_str)
                .filter(|cwd| !cwd.is_empty())
            {
                detail.push(format!("Directory: {cwd}"));
            }
            let output = tool
                .get("output")
                .and_then(|output| output.get("inline"))
                .and_then(Json::as_str)
                .unwrap_or_default();
            if !output.trim().is_empty() {
                let (shown, cut) = clip_end(output, STEP_TEXT_MAX_CHARS);
                detail.push(fenced("", &shown));
                if cut {
                    note_left_out(&mut detail, "the beginning of the output");
                }
            }
        }
        "file_change" => {
            for change in tool
                .get("changes")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(diff) = change
                    .get("diff")
                    .and_then(Json::as_str)
                    .filter(|diff| !diff.is_empty())
                {
                    let (shown, cut) = clip_start(diff, STEP_DIFF_MAX_CHARS);
                    detail.push(fenced("diff", &shown));
                    if cut {
                        note_left_out(&mut detail, "the rest of the difference");
                    }
                }
            }
        }
        "file_search" => {
            detail.push(format!(
                "Looked for {:?}{}",
                tool.get("query").and_then(Json::as_str).unwrap_or_default(),
                tool.get("path")
                    .and_then(Json::as_str)
                    .map(|path| format!(" in {path}"))
                    .unwrap_or_default()
            ));
            for hit in tool
                .get("matches")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .take(STEP_MATCHES_SHOWN)
            {
                detail.push(
                    hit.get("path")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                );
            }
        }
        "web_search" => {
            detail.push(format!(
                "Searched for {:?}",
                tool.get("query").and_then(Json::as_str).unwrap_or_default()
            ));
            for result in tool
                .get("results")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .take(STEP_RESULTS_SHOWN)
            {
                detail.push(format!(
                    "{} {}",
                    result
                        .get("title")
                        .and_then(Json::as_str)
                        .unwrap_or_default(),
                    result.get("url").and_then(Json::as_str).unwrap_or_default()
                ));
            }
        }
        "mcp" | "orch" | "other" => {
            for (label, member) in [("Arguments", "arguments"), ("Result", "result")] {
                if let Some(value) = tool.get(member).filter(|value| !value.is_null()) {
                    let (shown, cut) = clip_start(&value.to_string(), STEP_TEXT_MAX_CHARS);
                    detail.push(format!("{label}: {shown}"));
                    if cut {
                        note_left_out(&mut detail, "the rest of it");
                    }
                }
            }
        }
        "subagent" => {
            if let Some(prompt) = tool.get("prompt").and_then(Json::as_str) {
                let (shown, _) = clip_start(prompt, STEP_TEXT_MAX_CHARS);
                detail.push(format!("Asked: {shown}"));
            }
        }
        _ => {}
    }
    if !detail.is_empty() {
        text.push('\n');
        text.push_str(&indent(&detail.join("\n")));
    }
    Some(text)
}

fn plan_lines(item: &Json) -> Option<String> {
    let payload = item.get("payload")?;
    let steps = payload.get("steps").and_then(Json::as_array)?;
    if steps.is_empty() {
        return None;
    }
    let mut lines = vec!["Plan:".to_owned()];
    for plan_step in steps {
        let mark = match plan_step.get("status").and_then(Json::as_str) {
            Some("completed") => "[x]",
            Some("in_progress") => "[~]",
            _ => "[ ]",
        };
        lines.push(format!(
            "- {mark} {}",
            plan_step
                .get("text")
                .and_then(Json::as_str)
                .unwrap_or_default()
        ));
    }
    Some(lines.join("\n"))
}

fn user_text(item: &Json) -> String {
    let parts = item
        .get("payload")
        .and_then(|payload| payload.get("parts"))
        .and_then(Json::as_array);
    let Some(parts) = parts else {
        return String::new();
    };
    parts
        .iter()
        .filter_map(|part| match part.get("type").and_then(Json::as_str)? {
            "text" => part.get("text").and_then(Json::as_str).map(str::to_owned),
            "image" => Some("[image omitted]".to_owned()),
            "file" => Some(format!(
                "Attached file: {}",
                part.get("path").and_then(Json::as_str).unwrap_or_default()
            )),
            "forwarded" => part
                .get("text")
                .and_then(Json::as_str)
                .map(|text| format!("[Forwarded]\n{text}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn message(role: &str, kind: &str, text: &str) -> Json {
    json!({ "type": "message", "role": role, "content": [ { "type": kind, "text": text } ] })
}

/// How many bytes of Responses items one `thread/inject_items` request carries at most: 256 KiB. A request is a
/// single line on the standard input of the harness; this keeps a line small enough to be read at once and a
/// failed request cheap to repeat, while a share of a window of a million tokens (about 2.4 MB) still goes in ten
/// requests. A single item larger than this goes alone.
pub const INJECT_PAGE_BYTES: usize = 256 * 1024;

/// The Responses items one turn of a neutral history comes to, and what became of each item of it.
fn turn_items(turn: &[Json], report: &mut ReportBuilder) -> Vec<Json> {
    let mut items = Vec::new();
    let mut steps = Vec::new();
    let mut plan = None;
    let mut said = Vec::new();
    for item in turn {
        let kind = item.get("kind").and_then(Json::as_str).unwrap_or("item");
        match kind {
            "user_message" => {
                let text = user_text(item);
                if text.is_empty() {
                    report.drop_item(kind, "it carried nothing the model can be told");
                } else {
                    report.keep(kind);
                    items.push(message("user", "input_text", &text));
                }
            }
            "agent_message" => {
                let text = item
                    .get("payload")
                    .and_then(|payload| payload.get("text"))
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                if text.is_empty() {
                    report.drop_item(kind, "it said nothing");
                } else {
                    report.keep(kind);
                    said.push(text.to_owned());
                }
            }
            "tool_call" => {
                if let Some(written) = step(item) {
                    report.convert(kind);
                    steps.push(written);
                }
            }
            "plan" => {
                if let Some(written) = plan_lines(item) {
                    report.convert(kind);
                    plan = Some(written);
                } else {
                    report.drop_item(kind, "it had no steps");
                }
            }
            other => report.drop_item(other, "the model is not told of it"),
        }
    }
    let final_answer = said.pop();
    for text in said {
        items.push(message("assistant", "output_text", &text));
    }
    let mut work = Vec::new();
    if !steps.is_empty() {
        work.push(format!("Steps of this turn:\n{}", steps.join("\n")));
    }
    if let Some(plan) = plan {
        work.push(plan);
    }
    if !work.is_empty() {
        items.push(message("assistant", "output_text", &work.join("\n\n")));
    }
    if let Some(text) = final_answer {
        items.push(message("assistant", "output_text", &text));
    }
    items
}

/// A neutral history names turns by a legacy identity or by the engine's ordinal. Keep those namespaces distinct.
#[derive(PartialEq, Eq)]
enum NeutralTurnId {
    Id(String),
    Ordinal(i64),
}

/// A neutral history read a turn at a time: only the turn being read is held, whatever the length of the file.
/// A turn is the run of items of one `turn_id` or positive `turn_ordinal`, in the order they appear. The unnumbered
/// omission notice prefixes the first actual turn. `through` stops after the first turns, with one next item held.
pub struct NeutralTurns<R> {
    lines: std::io::Lines<R>,
    held: Option<(Option<NeutralTurnId>, Vec<Json>)>,
    through: Option<u32>,
    given: u32,
    report: ReportBuilder,
    finished: bool,
}

impl NeutralTurns<std::io::BufReader<std::fs::File>> {
    /// Opens the history at `path`.
    pub fn open(path: &std::path::Path, through: Option<u32>) -> Result<Self, AdapterError> {
        let file = std::fs::File::open(path).map_err(|error| {
            AdapterError::HarnessError(format!("the history could not be read: {error}"))
        })?;
        Ok(Self::new(std::io::BufReader::new(file), through))
    }
}

impl<R: std::io::BufRead> NeutralTurns<R> {
    /// Reads the history from `reader`.
    #[must_use]
    pub fn new(reader: R, through: Option<u32>) -> Self {
        Self {
            lines: reader.lines(),
            held: None,
            through,
            given: 0,
            report: ReportBuilder::new(),
            finished: false,
        }
    }

    /// The Responses items of the next turn; `None` once the history, or the part of it asked for, is told.
    pub fn next_turn(&mut self) -> Result<Option<Vec<Json>>, AdapterError> {
        if self.finished || self.through.is_some_and(|through| self.given >= through) {
            self.finished = true;
            return Ok(None);
        }
        loop {
            let Some(line) = self.lines.next() else {
                self.finished = true;
                return Ok(self.held.take().map(|(_, turn)| self.give(&turn)));
            };
            let line = line.map_err(|error| {
                AdapterError::HarnessError(format!("the history could not be read: {error}"))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let item: Json = serde_json::from_str(&line).map_err(|error| {
                AdapterError::HarnessError(format!("a line of the history is not JSON: {error}"))
            })?;
            let kind = item
                .get("kind")
                .and_then(Json::as_str)
                .unwrap_or("item")
                .to_owned();
            let stream = item.get("stream").and_then(Json::as_str).unwrap_or("main");
            if stream != "main" {
                self.report
                    .drop_item(&kind, "it belongs to a sub-agent's own stream");
                continue;
            }
            let turn = item
                .get("turn_id")
                .and_then(Json::as_str)
                .map(|id| NeutralTurnId::Id(id.to_owned()))
                .or_else(|| {
                    item.get("turn_ordinal")
                        .and_then(Json::as_i64)
                        .filter(|ordinal| *ordinal > 0)
                        .map(NeutralTurnId::Ordinal)
                });
            match self.held.as_mut() {
                Some((held, items)) if *held == turn => items.push(item),
                Some((held, items)) if held.is_none() && turn.is_some() => {
                    *held = turn;
                    items.push(item);
                }
                Some(_) => {
                    let before = self.held.replace((turn, vec![item]));
                    if let Some((_, finished)) = before {
                        return Ok(Some(self.give(&finished)));
                    }
                }
                None => self.held = Some((turn, vec![item])),
            }
        }
    }

    fn give(&mut self, turn: &[Json]) -> Vec<Json> {
        self.given += 1;
        turn_items(turn, &mut self.report)
    }

    /// What became of the items that were read.
    #[must_use]
    pub fn finish(self) -> HistoryReport {
        self.report.finish(Fidelity::Structured)
    }
}

/// The Responses items a neutral history comes to, and what became of each item. `through` limits it to the
/// first turns.
pub fn neutral_to_responses(
    history: &str,
    through: Option<u32>,
) -> Result<(Vec<Json>, HistoryReport), AdapterError> {
    let mut turns = NeutralTurns::new(history.as_bytes(), through);
    let mut items = Vec::new();
    while let Some(turn) = turns.next_turn()? {
        items.extend(turn);
    }
    Ok((items, turns.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, turn: &str, payload: Json) -> String {
        json!({ "kind": kind, "turn_id": turn, "stream": "main", "status": "completed", "payload": payload }).to_string()
    }

    fn history() -> String {
        [
            item("user_message", "t1", json!({ "parts": [ { "type": "text", "text": "Fix the login redirect" },
                    { "type": "file", "path": ".orch/uploads/trace.har", "name": "trace.har", "size": 3, "blob": null } ] })),
            item("agent_message", "t1", json!({ "text": "I will look at the handler first.", "text_len": 33, "phase": "commentary" })),
            item("tool_call", "t1", json!({ "tool": { "kind": "command", "command": "pytest", "cwd": "/w", "exit_code": 1,
                    "output": { "inline": "1 failed", "total_len": 8, "truncated": false, "blob": null } },
                    "title": "pytest tests/login", "started_at": "2026-10-11T10:00:00.000Z", "duration_ms": 5, "raw": {} })),
            item("tool_call", "t1", json!({ "tool": { "kind": "file_change", "changes": [ { "path": "a.rs", "change": "modify", "diff": "@@\n-a\n+b\n" } ] },
                    "title": "Edit a.rs", "started_at": "2026-10-11T10:00:01.000Z", "duration_ms": 5, "raw": {} })),
            item("plan", "t1", json!({ "explanation": null, "steps": [ { "text": "read", "status": "completed" }, { "text": "fix", "status": "pending" } ] })),
            item("reasoning", "t1", json!({ "summary": "thinking", "text_len": 8, "hidden": false })),
            item("agent_message", "t1", json!({ "text": "Fixed.", "text_len": 6, "phase": "final" })),
            item("user_message", "t2", json!({ "parts": [ { "type": "text", "text": "Thanks" } ] })),
            json!({ "kind": "agent_message", "turn_id": "t1", "stream": "sub:abc", "payload": { "text": "inside" } }).to_string(),
        ]
        .join("\n")
    }

    #[test]
    fn a_neutral_history_becomes_the_messages_of_each_turn_with_its_steps_before_the_final_answer()
    {
        let (items, report) = neutral_to_responses(&history(), None).unwrap();
        let roles: Vec<(String, String)> = items
            .iter()
            .map(|item| {
                (
                    item["role"].as_str().unwrap().to_owned(),
                    item["content"][0]["type"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            roles,
            [
                ("user".into(), "input_text".into()),
                ("assistant".into(), "output_text".into()),
                ("assistant".into(), "output_text".into()),
                ("assistant".into(), "output_text".into()),
                ("user".into(), "input_text".into()),
            ]
        );
        assert_eq!(
            items[0]["content"][0]["text"],
            "Fix the login redirect\n\nAttached file: .orch/uploads/trace.har"
        );
        assert_eq!(
            items[1]["content"][0]["text"],
            "I will look at the handler first."
        );
        let steps = items[2]["content"][0]["text"].as_str().unwrap();
        assert!(steps.starts_with("Steps of this turn:\n- pytest tests/login · exit 1\n  Directory: /w\n  ```\n  1 failed\n  ```"), "{steps}");
        assert!(
            steps.contains("- Edit a.rs\n  ```diff\n  @@\n  -a\n  +b\n  ```"),
            "{steps}"
        );
        assert!(steps.contains("Plan:\n- [x] read\n- [ ] fix"), "{steps}");
        assert_eq!(items[3]["content"][0]["text"], "Fixed.");
        assert_eq!(report.fidelity, Fidelity::Structured);
        assert_eq!(report.by_kind["tool_call"].converted, 2);
        assert_eq!(report.by_kind["reasoning"].dropped, 1);
        assert_eq!(
            report.by_kind["agent_message"].dropped, 1,
            "the sub-agent's own stream is not included"
        );
    }

    #[test]
    fn engine_ordinals_keep_turn_order_and_the_omission_notice_does_not_take_a_turn() {
        let ordinal_item = |kind: &str, ordinal: i64, payload: Json| {
            json!({"kind":kind,"turn_ordinal":ordinal,"stream":"main","status":"completed","payload":payload}).to_string()
        };
        let text = [
            ordinal_item(
                "user_message",
                0,
                json!({"parts":[{"type":"text","text":"Earlier turns are in the transcript."}]}),
            ),
            ordinal_item(
                "user_message",
                7,
                json!({"parts":[{"type":"text","text":"ASK_7"}]}),
            ),
            ordinal_item(
                "tool_call",
                7,
                json!({"title":"Check turn 7","tool":{"kind":"other"}}),
            ),
            ordinal_item("agent_message", 7, json!({"text":"ANSWER_7"})),
            ordinal_item(
                "user_message",
                8,
                json!({"parts":[{"type":"text","text":"ASK_8"}]}),
            ),
            ordinal_item("agent_message", 8, json!({"text":"ANSWER_8"})),
        ]
        .join("\n");
        let expected = [
            message("user", "input_text", "Earlier turns are in the transcript."),
            message("user", "input_text", "ASK_7"),
            message(
                "assistant",
                "output_text",
                "Steps of this turn:\n- Check turn 7",
            ),
            message("assistant", "output_text", "ANSWER_7"),
            message("user", "input_text", "ASK_8"),
            message("assistant", "output_text", "ANSWER_8"),
        ];
        let (items, report) = neutral_to_responses(&text, None).unwrap();
        assert_eq!(items, expected);
        assert_eq!(report.items_total, 6);
        assert_eq!(report.by_kind["user_message"].kept, 3);
        assert_eq!(report.by_kind["agent_message"].kept, 2);
        assert_eq!(report.by_kind["tool_call"].converted, 1);
        let (first, first_report) = neutral_to_responses(&text, Some(1)).unwrap();
        assert_eq!(first, expected[..4]);
        assert_eq!(first_report.items_total, 4);
        let (none, none_report) = neutral_to_responses(&text, Some(0)).unwrap();
        assert!(none.is_empty());
        assert_eq!(none_report.items_total, 0);
    }

    #[test]
    fn explicit_turn_ids_keep_precedence_over_ordinal_hints() {
        let text = [
            json!({"kind":"user_message","turn_id":"first","turn_ordinal":1,"payload":{"parts":[{"type":"text","text":"ASK_1"}]}}),
            json!({"kind":"agent_message","turn_id":"first","turn_ordinal":1,"payload":{"text":"ANSWER_1"}}),
            json!({"kind":"user_message","turn_id":"second","turn_ordinal":1,"payload":{"parts":[{"type":"text","text":"ASK_2"}]}}),
            json!({"kind":"agent_message","turn_id":"second","turn_ordinal":1,"payload":{"text":"ANSWER_2"}}),
        ].map(|line|line.to_string()).join("\n");
        let (items, _) = neutral_to_responses(&text, None).unwrap();
        assert_eq!(
            items,
            [
                message("user", "input_text", "ASK_1"),
                message("assistant", "output_text", "ANSWER_1"),
                message("user", "input_text", "ASK_2"),
                message("assistant", "output_text", "ANSWER_2"),
            ]
        );
    }

    #[test]
    fn a_history_can_be_taken_through_a_turn() {
        let (items, _) = neutral_to_responses(&history(), Some(1)).unwrap();
        assert_eq!(items.len(), 4);
        assert!(
            items
                .iter()
                .all(|item| !item.to_string().contains("Thanks"))
        );
    }

    #[test]
    fn the_steps_of_a_turn_are_bounded_as_the_written_form_says() {
        let long_output = "q".repeat(STEP_TEXT_MAX_CHARS + 500);
        let long_diff = format!("@@\n{}", "+y\n".repeat(STEP_DIFF_MAX_CHARS));
        let text = [
            item("user_message", "t", json!({ "parts": [ { "type": "text", "text": "go" } ] })),
            item("tool_call", "t", json!({ "tool": { "kind": "command", "cwd": "/w", "exit_code": 0,
                    "output": { "inline": long_output, "total_len": 9000, "truncated": true, "blob": null } },
                    "title": "make", "started_at": "2026-10-11T10:00:00.000Z", "duration_ms": 1, "raw": {} })),
            item("tool_call", "t", json!({ "tool": { "kind": "file_change", "changes": [ { "path": "big", "change": "add", "diff": long_diff } ] },
                    "title": "Edit big", "started_at": "2026-10-11T10:00:00.000Z", "duration_ms": 1, "raw": {} })),
            item("agent_message", "t", json!({ "text": "done", "text_len": 4, "phase": null })),
        ]
        .join("\n");
        let (items, _) = neutral_to_responses(&text, None).unwrap();
        let steps = items[1]["content"][0]["text"].as_str().unwrap();
        assert!(steps.contains("(the beginning of the output was left out here)"));
        assert!(steps.contains("(the rest of the difference was left out here)"));
        let shown_diff_lines = steps.matches("+y").count();
        assert!(
            shown_diff_lines * 3 <= STEP_DIFF_MAX_CHARS + 3,
            "{shown_diff_lines} lines of the difference are shown"
        );
        assert_eq!(
            steps.matches('q').count(),
            STEP_TEXT_MAX_CHARS,
            "the end of the output is what is shown"
        );
    }

    #[test]
    fn a_line_that_is_no_item_is_a_fault_of_the_history_and_an_empty_history_is_nothing() {
        assert!(neutral_to_responses("not json\n", None).is_err());
        let (items, report) = neutral_to_responses("", None).unwrap();
        assert!(items.is_empty());
        assert_eq!(report.items_total, 0);
    }
}
