//! The Messages format of Anthropic as the scripted server writes it: the events of a stream and the document of a plain answer.
//!
//! A stream is server-sent events, each with an `event:` line naming its type and a `data:` line holding a JSON
//! document that begins with the same type. It opens with `message_start`, which carries the message with no
//! content and a `usage` that says what the prompt took (`input_tokens` without what was read from or written
//! to a cache, which have members of their own) and a first count of output. Each content block follows as
//! `content_block_start`, deltas (`text_delta`, `thinking_delta` and then `signature_delta`, `input_json_delta`)
//! and `content_block_stop`; a `ping` comes after the first block starts. It closes with `message_delta`,
//! whose `delta` holds the stop reason and whose `usage` is cumulative, and `message_stop`. A failure inside a
//! stream is an `error` event. The plain answer is the message object, with its content and its `usage`.

use serde_json::{Value, json};

use super::errors;
use super::plan::{Ctx, Ending, Frame, Usage, Wire};
use super::request::Format;
use super::script::{Finish, Pace, Segment, Step, pieces};

/// What the first count of output in `message_start` is: the vendor's start event counts one token.
const START_OUTPUT_TOKENS: i64 = 1;

/// How many hexadecimal digits of the hash of a thinking text its stand-in signature holds.
const SIGNATURE_DIGITS: usize = 16;

/// The `usage` a message starts with.
fn start_usage(usage: &Usage) -> Value {
    json!({
        "input_tokens": usage.input - usage.cached - usage.cache_write,
        "cache_creation_input_tokens": usage.cache_write,
        "cache_read_input_tokens": usage.cached,
        "output_tokens": START_OUTPUT_TOKENS
    })
}

/// The members that count the output: its total, the thinking part of it where there is one, and a cost.
fn output_members(usage: &Usage, into: &mut Value) {
    into["output_tokens"] = Value::from(usage.output);
    if usage.reasoning > 0 {
        into["output_tokens_details"] = json!({ "thinking_tokens": usage.reasoning });
    }
    if let Some(cost) = usage
        .cost
        .as_deref()
        .and_then(|cost| cost.parse::<f64>().ok())
        .and_then(serde_json::Number::from_f64)
    {
        into["cost"] = Value::Number(cost);
    }
}

/// The cumulative `usage` of `message_delta`.
fn delta_usage(usage: &Usage) -> Value {
    let mut value = json!({});
    output_members(usage, &mut value);
    value
}

/// The `usage` of a plain message.
fn message_usage(usage: &Usage) -> Value {
    let mut value = json!({
        "input_tokens": usage.input - usage.cached - usage.cache_write,
        "cache_creation_input_tokens": usage.cache_write,
        "cache_read_input_tokens": usage.cached
    });
    output_members(usage, &mut value);
    value["service_tier"] = Value::from("standard");
    value
}

/// The stand-in for the signature of a thinking block: a function of what was thought.
fn signature(text: &str) -> String {
    let hash = blake3::hash(text.as_bytes()).to_hex();
    format!("mock-signature-{}", &hash.as_str()[..SIGNATURE_DIGITS])
}

/// Why the message stopped.
fn stop_reason(segment: &Segment) -> String {
    if let Some(stop) = &segment.stop {
        return stop.clone();
    }
    if matches!(segment.finish, Finish::Incomplete(_)) {
        return "max_tokens".to_owned();
    }
    let calls = segment
        .steps
        .iter()
        .any(|step| matches!(step, Step::Tool { .. } | Step::Custom { .. }));
    if calls { "tool_use" } else { "end_turn" }.to_owned()
}

/// The text a free-text call is sent as: this format has no such tool, so it is a call with one member.
fn custom_arguments(input: &str) -> String {
    json!({ "input": input }).to_string()
}

/// A content block as it is when it is done; `None` for a step that is no block.
fn done_block(ctx: &Ctx, index: usize, step: &Step) -> Option<Value> {
    let seq = ctx.seq;
    match step {
        Step::Text { text, .. } => Some(json!({ "type": "text", "text": text })),
        Step::Reasoning { text, .. } => {
            Some(json!({ "type": "thinking", "thinking": text, "signature": signature(text) }))
        }
        Step::Tool {
            name, arguments, ..
        } => Some(json!({
            "type": "tool_use", "id": format!("toolu_mock_{seq:06}_{index}"), "name": name,
            "input": serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}))
        })),
        Step::Custom { name, input, .. } => Some(json!({
            "type": "tool_use", "id": format!("toolu_mock_{seq:06}_{index}"), "name": name,
            "input": { "input": input }
        })),
        _ => None,
    }
}

/// The message object: with no content and no stop while it streams, with both as a plain answer.
fn message_object(ctx: &Ctx, content: Vec<Value>, stop: Option<&str>, usage: Value) -> Value {
    json!({
        "id": format!("msg_mock_{:06}", ctx.seq),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": ctx.model,
        "stop_reason": stop,
        "stop_sequence": null,
        "usage": usage
    })
}

fn block_delta(wire: &mut Wire, index: usize, delta: Value) {
    wire.event(
        "content_block_delta",
        &json!({ "type": "content_block_delta", "index": index, "delta": delta }),
    );
}

/// The deltas of a text, a wait before each.
fn deltas(wire: &mut Wire, text: &str, pace: Pace, mut one: impl FnMut(&mut Wire, &str)) {
    for piece in pieces(text, pace.chunk) {
        wire.wait(pace.delay);
        one(wire, &piece);
    }
}

/// Writes the events of one block: started, filled, and, unless the stream stops inside it, stopped.
fn block_events(wire: &mut Wire, index: usize, step: &Step, done: &Value, close: bool) {
    let start = |content_block: Value| json!({ "type": "content_block_start", "index": index, "content_block": content_block });
    match step {
        Step::Text { text, pace } => {
            wire.event(
                "content_block_start",
                &start(json!({ "type": "text", "text": "" })),
            );
            if index == 0 {
                wire.event("ping", &json!({ "type": "ping" }));
            }
            deltas(wire, text, *pace, |wire, piece| {
                block_delta(wire, index, json!({ "type": "text_delta", "text": piece }));
            });
        }
        Step::Reasoning { text, pace } => {
            wire.event(
                "content_block_start",
                &start(json!({ "type": "thinking", "thinking": "", "signature": "" })),
            );
            if index == 0 {
                wire.event("ping", &json!({ "type": "ping" }));
            }
            deltas(wire, text, *pace, |wire, piece| {
                block_delta(
                    wire,
                    index,
                    json!({ "type": "thinking_delta", "thinking": piece }),
                );
            });
            if close {
                block_delta(
                    wire,
                    index,
                    json!({ "type": "signature_delta", "signature": done["signature"] }),
                );
            }
        }
        Step::Tool {
            name,
            arguments,
            pace,
        } => {
            wire.event(
                "content_block_start",
                &start(json!({ "type": "tool_use", "id": done["id"], "name": name, "input": {} })),
            );
            if index == 0 {
                wire.event("ping", &json!({ "type": "ping" }));
            }
            block_delta(
                wire,
                index,
                json!({ "type": "input_json_delta", "partial_json": "" }),
            );
            deltas(wire, arguments, *pace, |wire, piece| {
                block_delta(
                    wire,
                    index,
                    json!({ "type": "input_json_delta", "partial_json": piece }),
                );
            });
        }
        Step::Custom { name, input, pace } => {
            wire.event(
                "content_block_start",
                &start(json!({ "type": "tool_use", "id": done["id"], "name": name, "input": {} })),
            );
            if index == 0 {
                wire.event("ping", &json!({ "type": "ping" }));
            }
            block_delta(
                wire,
                index,
                json!({ "type": "input_json_delta", "partial_json": "" }),
            );
            deltas(wire, &custom_arguments(input), *pace, |wire, piece| {
                block_delta(
                    wire,
                    index,
                    json!({ "type": "input_json_delta", "partial_json": piece }),
                );
            });
        }
        _ => return,
    }
    if close {
        wire.event(
            "content_block_stop",
            &json!({ "type": "content_block_stop", "index": index }),
        );
    }
}

/// Whether the step after position `at` ends the stream where it stands, which leaves a block before it open.
fn ends_inside(steps: &[Step], at: usize) -> bool {
    matches!(
        steps.get(at + 1),
        Some(Step::Cut | Step::Truncate | Step::StreamError(_))
    )
}

/// The frames of a stream, and how it ends.
#[must_use]
pub fn stream(segment: &Segment, usage: &Usage, ctx: &Ctx) -> (Vec<Frame>, Ending) {
    let mut wire = Wire::new();
    wire.event(
        "message_start",
        &json!({ "type": "message_start", "message": message_object(ctx, Vec::new(), None, start_usage(usage)) }),
    );
    let mut blocks = 0_usize;
    for (at, step) in segment.steps.iter().enumerate() {
        match step {
            Step::Sleep(duration) => wire.wait(*duration),
            Step::Keepalive => wire.comment("keep-alive"),
            Step::Event { name, data } => wire.raw(Some(name), data),
            Step::Garbage(text) => wire.raw(None, text),
            Step::StreamError(wanted) => {
                let (name, data) = errors::stream_event(Format::Messages, wanted, 0);
                wire.event(name, &data);
                return (wire.finish(), Ending::StreamError);
            }
            Step::Cut => {
                wire.abort();
                return (wire.finish(), Ending::Cut);
            }
            Step::Truncate => return (wire.finish(), Ending::Truncated),
            block => {
                if let Some(done) = done_block(ctx, blocks, block) {
                    block_events(
                        &mut wire,
                        blocks,
                        block,
                        &done,
                        !ends_inside(&segment.steps, at),
                    );
                    blocks += 1;
                }
            }
        }
    }
    if let Finish::Fail(wanted) = &segment.finish {
        let (name, data) = errors::stream_event(Format::Messages, wanted, 0);
        wire.event(name, &data);
        return (wire.finish(), Ending::StreamError);
    }
    wire.event(
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason(segment), "stop_sequence": null },
            "usage": delta_usage(usage)
        }),
    );
    wire.event("message_stop", &json!({ "type": "message_stop" }));
    if segment.gateway {
        wire.done_marker();
    }
    (wire.finish(), Ending::Completed)
}

/// The message object a plain answer is. A failure is not a message: it is the error of
/// [`errors::reply`], and the caller gives that.
#[must_use]
pub fn plain(segment: &Segment, usage: &Usage, ctx: &Ctx) -> Value {
    let mut content: Vec<Value> = Vec::new();
    for step in &segment.steps {
        if let Some(block) = done_block(ctx, content.len(), step) {
            content.push(block);
        }
    }
    message_object(
        ctx,
        content,
        Some(&stop_reason(segment)),
        message_usage(usage),
    )
}

/// The answer to a request to count the tokens of a message.
#[must_use]
pub fn token_count(tokens: i64) -> Value {
    json!({ "input_tokens": tokens })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doubles::model::plan::{body_of, usage_of};
    use crate::doubles::model::script::Script;
    use orch_model_contract::usage::SseParser;

    fn ctx() -> Ctx {
        Ctx {
            seq: 3,
            model: "mock-large".into(),
            created_at: 1_791_700_000,
            encrypted_reasoning: false,
            previous_response_id: None,
        }
    }

    fn render(script: &str) -> (Vec<(String, Value)>, Ending) {
        let segment = Script::parse(script).unwrap().segment(0);
        let usage = usage_of(&segment, 40);
        let (frames, ending) = stream(&segment, &usage, &ctx());
        let mut parser = SseParser::new();
        let events = parser
            .push(&body_of(&frames))
            .into_iter()
            .map(|event| {
                (
                    event.event.unwrap_or_default(),
                    serde_json::from_str(&event.data).unwrap(),
                )
            })
            .collect();
        (events, ending)
    }

    fn names(events: &[(String, Value)]) -> Vec<&str> {
        events.iter().map(|(name, _)| name.as_str()).collect()
    }

    #[test]
    fn a_text_answer_streams_as_the_vendor_documents_with_a_ping_after_the_first_block_starts() {
        let (events, ending) = render("chunk 6\nsay Hello there\nusage 25 15");
        assert_eq!(ending, Ending::Completed);
        assert_eq!(
            names(&events),
            [
                "message_start",
                "content_block_start",
                "ping",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        for (name, data) in &events {
            assert_eq!(data["type"], name.as_str());
        }
        assert_eq!(
            events[3].1["delta"],
            json!({ "type": "text_delta", "text": "Hello " })
        );
        assert_eq!(events[4].1["delta"]["text"], "there");
        let start = &events[0].1["message"];
        assert_eq!(start["id"], "msg_mock_000003");
        assert_eq!(start["content"], json!([]));
        assert_eq!(start["stop_reason"], Value::Null);
        assert_eq!(start["model"], "mock-large");
        assert_eq!(
            start["usage"],
            json!({ "input_tokens": 25, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 1 })
        );
        assert_eq!(
            events[6].1,
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                    "usage": { "output_tokens": 15 } })
        );
    }

    #[test]
    fn the_cache_is_apart_from_the_input_in_the_start_and_the_thinking_part_is_named_in_the_delta()
    {
        let (events, _) = render("say a\nusage 100 40 cached=60 cache_write=10 reasoning=25");
        assert_eq!(
            events[0].1["message"]["usage"]["input_tokens"], 30,
            "100 less 60 read and 10 written"
        );
        assert_eq!(
            events[0].1["message"]["usage"]["cache_read_input_tokens"],
            60
        );
        assert_eq!(
            events[0].1["message"]["usage"]["cache_creation_input_tokens"],
            10
        );
        let delta = events
            .iter()
            .find(|(name, _)| name == "message_delta")
            .unwrap();
        assert_eq!(delta.1["usage"]["output_tokens"], 40);
        assert_eq!(
            delta.1["usage"]["output_tokens_details"]["thinking_tokens"],
            25
        );
    }

    #[test]
    fn thinking_ends_with_a_signature_and_a_tool_call_streams_its_arguments_as_partial_json() {
        let (events, _) = render("chunk 100\nthink hmm\ntool read_file {\"path\":\"a\"}");
        assert_eq!(
            names(&events),
            [
                "message_start",
                "content_block_start",
                "ping",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert_eq!(
            events[1].1["content_block"],
            json!({ "type": "thinking", "thinking": "", "signature": "" })
        );
        assert_eq!(events[3].1["delta"]["type"], "thinking_delta");
        assert_eq!(events[4].1["delta"]["type"], "signature_delta");
        assert!(
            events[4].1["delta"]["signature"]
                .as_str()
                .unwrap()
                .starts_with("mock-signature-")
        );
        assert_eq!(events[6].1["index"], 1);
        assert_eq!(
            events[6].1["content_block"],
            json!({ "type": "tool_use", "id": "toolu_mock_000003_1", "name": "read_file", "input": {} })
        );
        assert_eq!(
            events[7].1["delta"],
            json!({ "type": "input_json_delta", "partial_json": "" })
        );
        assert_eq!(events[8].1["delta"]["partial_json"], "{\"path\":\"a\"}");
        assert_eq!(events[10].1["delta"]["stop_reason"], "tool_use");
    }

    #[test]
    fn the_partial_json_of_a_call_put_together_is_the_arguments() {
        let (events, _) = render("chunk 3\ntool t {\"alpha\":\"beta\",\"n\":12}");
        let joined: String = events
            .iter()
            .filter(|(_, d)| d["delta"]["type"] == "input_json_delta")
            .map(|(_, d)| d["delta"]["partial_json"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            serde_json::from_str::<Value>(&joined).unwrap(),
            json!({ "alpha": "beta", "n": 12 })
        );
    }

    #[test]
    fn a_free_text_call_is_a_call_with_one_member_in_this_format() {
        let (events, _) = render("custom apply_patch *** Begin");
        let joined: String = events
            .iter()
            .filter(|(_, d)| d["delta"]["type"] == "input_json_delta")
            .map(|(_, d)| d["delta"]["partial_json"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            serde_json::from_str::<Value>(&joined).unwrap(),
            json!({ "input": "*** Begin" })
        );
    }

    #[test]
    fn the_stop_reason_follows_the_script_and_otherwise_what_the_answer_holds() {
        let reason = |script: &str| {
            let (events, _) = render(script);
            events
                .iter()
                .find(|(name, _)| name == "message_delta")
                .map(|(_, d)| d["delta"]["stop_reason"].as_str().unwrap().to_owned())
                .unwrap()
        };
        assert_eq!(reason("say a"), "end_turn");
        assert_eq!(reason("tool t"), "tool_use");
        assert_eq!(reason("say a\nincomplete"), "max_tokens");
        assert_eq!(reason("say a\nstop refusal"), "refusal");
    }

    #[test]
    fn a_gateway_ends_the_stream_with_a_done_event_under_the_name_data() {
        let segment = Script::parse("gateway\nsay hi").unwrap().segment(0);
        let (frames, _) = stream(&segment, &usage_of(&segment, 4), &ctx());
        let text = String::from_utf8(body_of(&frames)).unwrap();
        assert!(text.starts_with("event: message_start\n"));
        assert!(text.ends_with("event: data\ndata: [DONE]\n\n"));
    }

    #[test]
    fn a_failure_is_an_error_event_with_no_end_of_message_after_it() {
        for script in ["say a\nstream_error overloaded", "say a\nfail overloaded"] {
            let (events, ending) = render(script);
            assert_eq!(ending, Ending::StreamError, "{script}");
            let last = events.last().unwrap();
            assert_eq!(last.0, "error");
            assert_eq!(last.1["error"]["type"], "overloaded_error");
            assert!(!names(&events).contains(&"message_stop"));
        }
    }

    #[test]
    fn a_cut_leaves_the_block_open_and_a_truncation_just_stops() {
        let (events, ending) = render("chunk 2\nsay abcdef\ncut\nsay never");
        assert_eq!(ending, Ending::Cut);
        assert_eq!(names(&events).last(), Some(&"content_block_delta"));
        assert!(!names(&events).contains(&"content_block_stop"));
        let (_, ending) = render("say abc\ntruncate");
        assert_eq!(ending, Ending::Truncated);
    }

    #[test]
    fn a_plain_answer_is_the_message_with_its_content_and_the_usage_of_the_whole() {
        let segment = Script::parse(
            "think hmm\nsay Hello\ntool t {\"a\":1}\nusage 100 40 cached=60 reasoning=25",
        )
        .unwrap()
        .segment(0);
        let usage = usage_of(&segment, 4);
        let answer = plain(&segment, &usage, &ctx());
        assert_eq!(answer["type"], "message");
        assert_eq!(answer["role"], "assistant");
        assert_eq!(answer["stop_reason"], "tool_use");
        assert_eq!(answer["content"][0]["type"], "thinking");
        assert_eq!(
            answer["content"][1],
            json!({ "type": "text", "text": "Hello" })
        );
        assert_eq!(answer["content"][2]["input"], json!({ "a": 1 }));
        assert_eq!(answer["usage"]["input_tokens"], 40);
        assert_eq!(answer["usage"]["cache_read_input_tokens"], 60);
        assert_eq!(answer["usage"]["output_tokens"], 40);
        assert_eq!(
            answer["usage"]["output_tokens_details"]["thinking_tokens"],
            25
        );
        assert_eq!(answer["usage"]["service_tier"], "standard");
        let (events, _) =
            render("think hmm\nsay Hello\ntool t {\"a\":1}\nusage 100 40 cached=60 reasoning=25");
        assert_eq!(events[0].1["message"]["model"], answer["model"]);
    }

    #[test]
    fn the_signature_of_a_thought_is_a_function_of_the_thought() {
        assert_eq!(signature("a"), signature("a"));
        assert_ne!(signature("a"), signature("b"));
    }

    #[test]
    fn a_count_of_tokens_is_a_document_of_one_member() {
        assert_eq!(token_count(42), json!({ "input_tokens": 42 }));
    }
}
