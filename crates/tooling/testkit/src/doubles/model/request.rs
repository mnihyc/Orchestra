//! What a request to the scripted model server holds: its model, whether it streams, the message that drives the reply, and how many rounds of tool results follow that message.
//!
//! The message that drives the reply is the last message of the person: in the Responses format the last item
//! that is a message of role `user` and holds text, in the Messages format the last message of role `user` that
//! holds text and no tool result. Messages that hold tool results are not the person's, even when the harness
//! adds a line of its own to them. A *round* is a run of tool results after the driving message: the request
//! that carries the results of the first tool calls is in round 1. The script is read from the driving message,
//! the round says which of its responses is due, and the texts of the last round's results fill the place of
//! `{{result}}`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which of the three formats a request is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// The Responses format of OpenAI.
    Responses,
    /// The Messages format of Anthropic.
    Messages,
    /// The Chat Completions format of OpenAI, which many gateways and DeepSeek's own API speak.
    Chat,
}

impl Format {
    /// The name it is recorded and matched by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::Messages => "messages",
            Self::Chat => "chat",
        }
    }

    /// The format a name stands for.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "responses" => Some(Self::Responses),
            "messages" => Some(Self::Messages),
            "chat" => Some(Self::Chat),
            _ => None,
        }
    }
}

/// What the server makes of a request to create a response.
#[derive(Debug, Clone, Default)]
pub struct Inbound {
    /// The model it names.
    pub model: Option<String>,
    /// Whether it asks for a stream.
    pub stream: bool,
    /// The text blocks of the message that drives the reply.
    pub driving: Vec<String>,
    /// The rounds of tool results after that message.
    pub rounds: usize,
    /// The texts of the results of the last round.
    pub results: Vec<String>,
    /// The characters of text the whole request holds.
    pub chars: usize,
    /// The response it continues, in the Responses format.
    pub previous_response_id: Option<String>,
    /// Whether it asks for the encrypted form of the reasoning.
    pub encrypted_reasoning: bool,
}

/// The characters of every string a document holds.
fn characters(value: &Value) -> usize {
    match value {
        Value::String(text) => text.chars().count(),
        Value::Array(items) => items.iter().map(characters).sum(),
        Value::Object(members) => members.values().map(characters).sum(),
        _ => 0,
    }
}

/// The text a content member holds: a string, or the blocks of the given types.
fn texts(content: Option<&Value>, kinds: &[&str]) -> Vec<String> {
    match content {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| {
                block
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kinds.contains(&kind))
            })
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// The text of a tool's output: a string, or the parts that hold text.
fn output_texts(output: Option<&Value>) -> Vec<String> {
    texts(output, &["input_text", "output_text", "text"])
}

/// Counts the runs of tool results in `items` (those for which `is_result`) and gives the texts of the last.
fn rounds_of<'a>(
    items: impl Iterator<Item = &'a Value>,
    is_result: impl Fn(&Value) -> bool,
    result_texts: impl Fn(&Value) -> Vec<String>,
) -> (usize, Vec<String>) {
    let mut rounds = 0;
    let mut in_run = false;
    let mut last = Vec::new();
    for item in items {
        if is_result(item) {
            if !in_run {
                rounds += 1;
                last.clear();
                in_run = true;
            }
            last.extend(result_texts(item));
        } else {
            in_run = false;
        }
    }
    (rounds, last)
}

fn responses(body: &Value) -> Inbound {
    let mut inbound = Inbound {
        previous_response_id: body
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        encrypted_reasoning: body
            .get("include")
            .and_then(Value::as_array)
            .is_some_and(|include| {
                include
                    .iter()
                    .any(|entry| entry.as_str() == Some("reasoning.encrypted_content"))
            }),
        ..Inbound::default()
    };
    match body.get("input") {
        Some(Value::String(text)) => inbound.driving = vec![text.clone()],
        Some(Value::Array(items)) => {
            let user_texts = |item: &Value| {
                let is_message = item
                    .get("type")
                    .and_then(Value::as_str)
                    .is_none_or(|kind| kind == "message");
                if is_message && item.get("role").and_then(Value::as_str) == Some("user") {
                    texts(item.get("content"), &["input_text", "text"])
                } else {
                    Vec::new()
                }
            };
            let driving = items.iter().rposition(|item| !user_texts(item).is_empty());
            if let Some(at) = driving {
                inbound.driving = user_texts(&items[at]);
            }
            let after = driving.map_or(&items[..], |at| &items[at + 1..]);
            let (rounds, results) = rounds_of(
                after.iter(),
                |item| {
                    item.get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.ends_with("_call_output"))
                },
                |item| output_texts(item.get("output")),
            );
            inbound.rounds = rounds;
            inbound.results = results;
        }
        _ => {}
    }
    inbound
}

fn messages(body: &Value) -> Inbound {
    let mut inbound = Inbound::default();
    let Some(Value::Array(all)) = body.get("messages") else {
        return inbound;
    };
    let blocks_of = |message: &Value| -> Vec<Value> {
        match message.get("content") {
            Some(Value::Array(blocks)) => blocks.clone(),
            _ => Vec::new(),
        }
    };
    let has_result = |message: &Value| {
        blocks_of(message)
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
    };
    let is_user = |message: &Value| message.get("role").and_then(Value::as_str) == Some("user");
    let person = |message: &Value| {
        if is_user(message) && !has_result(message) {
            texts(message.get("content"), &["text"])
        } else {
            Vec::new()
        }
    };
    let driving = all.iter().rposition(|message| !person(message).is_empty());
    if let Some(at) = driving {
        inbound.driving = person(&all[at]);
    }
    let after = driving.map_or(&all[..], |at| &all[at + 1..]);
    let mut rounds = 0;
    let mut results = Vec::new();
    for message in after.iter().filter(|m| is_user(m) && has_result(m)) {
        rounds += 1;
        results = blocks_of(message)
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            .flat_map(|block| output_texts(block.get("content")))
            .collect();
    }
    inbound.rounds = rounds;
    inbound.results = results;
    inbound
}

/// A request of the Chat Completions format: the person's message is the last of role `user` that does not
/// follow a tool's result straight on (a harness that adds a line of its own after the results has not
/// spoken for the person), and the messages of role `tool` after it are the rounds.
fn chat(body: &Value) -> Inbound {
    let mut inbound = Inbound::default();
    let Some(Value::Array(all)) = body.get("messages") else {
        return inbound;
    };
    let role = |message: &Value| {
        message
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let person = |at: usize| {
        let message = &all[at];
        let after_result = at > 0 && role(&all[at - 1]).as_deref() == Some("tool");
        if role(message).as_deref() == Some("user") && !after_result {
            texts(message.get("content"), &["text"])
        } else {
            Vec::new()
        }
    };
    let driving = (0..all.len()).rev().find(|at| !person(*at).is_empty());
    if let Some(at) = driving {
        inbound.driving = person(at);
    }
    let after = driving.map_or(&all[..], |at| &all[at + 1..]);
    let (rounds, results) = rounds_of(
        after.iter(),
        |message| role(message).as_deref() == Some("tool"),
        |message| texts(message.get("content"), &["text"]),
    );
    inbound.rounds = rounds;
    inbound.results = results;
    inbound
}

/// Reads a request to create a response.
#[must_use]
pub fn analyze(format: Format, body: &Value) -> Inbound {
    let mut inbound = match format {
        Format::Responses => responses(body),
        Format::Messages => messages(body),
        Format::Chat => chat(body),
    };
    inbound.model = body.get("model").and_then(Value::as_str).map(str::to_owned);
    inbound.stream = body.get("stream").and_then(Value::as_bool) == Some(true);
    inbound.chars = characters(body);
    inbound
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_plain_input_string_is_the_message_that_drives_the_reply() {
        let found = analyze(
            Format::Responses,
            &json!({ "model": "m", "input": "hello there", "stream": true }),
        );
        assert_eq!(found.driving, ["hello there"]);
        assert_eq!(found.model.as_deref(), Some("m"));
        assert!(found.stream);
        assert_eq!(found.rounds, 0);
    }

    #[test]
    fn the_last_user_message_of_the_items_drives_the_reply_and_the_context_before_it_does_not() {
        let found = analyze(
            Format::Responses,
            &json!({ "input": [
                { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "rules" }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "<environment_context/>" }] },
                { "role": "user", "content": "first question" },
                { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "answer" }] },
                { "type": "message", "role": "user", "content": [
                    { "type": "input_text", "text": "#!mock" }, { "type": "input_text", "text": "say hi" }] }
            ] }),
        );
        assert_eq!(found.driving, ["#!mock", "say hi"]);
        assert_eq!(found.rounds, 0);
    }

    #[test]
    fn tool_outputs_after_the_driving_message_are_rounds_and_the_last_run_is_the_result() {
        let found = analyze(
            Format::Responses,
            &json!({ "input": [
                { "type": "message", "role": "user", "content": "go" },
                { "type": "reasoning", "summary": [] },
                { "type": "function_call", "call_id": "c1", "name": "a", "arguments": "{}" },
                { "type": "function_call", "call_id": "c2", "name": "b", "arguments": "{}" },
                { "type": "function_call_output", "call_id": "c1", "output": "one" },
                { "type": "function_call_output", "call_id": "c2", "output": "two" },
                { "type": "function_call", "call_id": "c3", "name": "c", "arguments": "{}" },
                { "type": "custom_tool_call_output", "call_id": "c3", "output": [{ "type": "input_text", "text": "three" }] }
            ] }),
        );
        assert_eq!(found.driving, ["go"]);
        assert_eq!(found.rounds, 2);
        assert_eq!(found.results, ["three"]);
        let first = analyze(
            Format::Responses,
            &json!({ "input": [
                { "role": "user", "content": "go" },
                { "type": "function_call_output", "call_id": "c1", "output": "one" },
                { "type": "function_call_output", "call_id": "c2", "output": "two" }
            ] }),
        );
        assert_eq!(
            (first.rounds, first.results),
            (1, vec!["one".to_owned(), "two".to_owned()])
        );
    }

    #[test]
    fn a_request_that_continues_a_stored_response_may_hold_only_the_results() {
        let found = analyze(
            Format::Responses,
            &json!({ "previous_response_id": "resp_7", "input": [
                { "type": "function_call_output", "call_id": "c1", "output": "ok" }] }),
        );
        assert!(found.driving.is_empty());
        assert_eq!(found.previous_response_id.as_deref(), Some("resp_7"));
        assert_eq!(found.results, ["ok"]);
    }

    #[test]
    fn the_encrypted_reasoning_is_asked_for_by_include() {
        let asked = analyze(
            Format::Responses,
            &json!({ "input": "x", "include": ["reasoning.encrypted_content"] }),
        );
        assert!(asked.encrypted_reasoning);
        assert!(!analyze(Format::Responses, &json!({ "input": "x" })).encrypted_reasoning);
    }

    #[test]
    fn a_messages_request_is_driven_by_the_last_message_of_the_person() {
        let found = analyze(
            Format::Messages,
            &json!({ "model": "m", "max_tokens": 16, "messages": [
                { "role": "user", "content": "earlier" },
                { "role": "assistant", "content": [{ "type": "text", "text": "ok" }] },
                { "role": "user", "content": [
                    { "type": "text", "text": "<system-reminder>x</system-reminder>" },
                    { "type": "text", "text": "#!mock\nsay hi" }] }
            ] }),
        );
        assert_eq!(found.driving.len(), 2);
        assert_eq!(found.driving[1], "#!mock\nsay hi");
        assert_eq!(found.rounds, 0);
        assert!(!found.stream);
    }

    #[test]
    fn a_message_of_tool_results_is_a_round_even_when_the_harness_adds_a_line_of_its_own() {
        let found = analyze(
            Format::Messages,
            &json!({ "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "#!mock\ntool t\nthen\nsay done" }] },
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "toolu_1", "name": "t", "input": {} },
                    { "type": "tool_use", "id": "toolu_2", "name": "t", "input": {} }] },
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "alpha" },
                    { "type": "tool_result", "tool_use_id": "toolu_2", "content": [{ "type": "text", "text": "beta" }] },
                    { "type": "text", "text": "<system-reminder>carry on</system-reminder>" }] }
            ] }),
        );
        assert_eq!(found.driving, ["#!mock\ntool t\nthen\nsay done"]);
        assert_eq!(found.rounds, 1);
        assert_eq!(found.results, ["alpha", "beta"]);
    }

    #[test]
    fn a_request_with_no_message_of_a_person_has_nothing_to_drive_it() {
        assert!(
            analyze(Format::Messages, &json!({ "messages": [] }))
                .driving
                .is_empty()
        );
        assert!(analyze(Format::Messages, &json!({})).driving.is_empty());
        assert!(
            analyze(Format::Responses, &json!({ "input": [] }))
                .driving
                .is_empty()
        );
        assert!(analyze(Format::Responses, &json!(null)).driving.is_empty());
    }

    #[test]
    fn a_chat_request_is_driven_by_the_last_user_message_that_does_not_follow_a_result() {
        let found = analyze(
            Format::Chat,
            &json!({ "model": "m", "stream": true, "messages": [
                { "role": "system", "content": "rules" },
                { "role": "user", "content": "context" },
                { "role": "user", "content": [{ "type": "text", "text": "#!mock\ntool bash" }] },
                { "role": "assistant", "content": null, "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "bash", "arguments": "{}" } }] },
                { "role": "tool", "tool_call_id": "c1", "content": "alpha" },
                { "role": "user", "content": "<reminder>carry on</reminder>" }
            ] }),
        );
        assert_eq!(found.driving, ["#!mock\ntool bash"]);
        assert_eq!((found.rounds, found.results), (1, vec!["alpha".to_owned()]));
        assert!(found.stream);
        let plain = analyze(
            Format::Chat,
            &json!({ "messages": [{ "role": "user", "content": "hello" }] }),
        );
        assert_eq!((plain.driving, plain.rounds), (vec!["hello".to_owned()], 0));
    }

    #[test]
    fn the_characters_are_those_of_every_string_the_request_holds() {
        let found = analyze(
            Format::Messages,
            &json!({ "model": "abc", "messages": [{ "role": "user", "content": "größe" }], "n": 12345 }),
        );
        // "abc", "user" and "größe" (five characters, not six bytes); names and numbers are not text.
        assert_eq!(found.chars, 3 + 4 + 5);
    }
}
