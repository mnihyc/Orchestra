//! The Responses format of OpenAI as the scripted server writes it: the events of a stream and the document of a plain answer.
//!
//! A stream is server-sent events, each with an `event:` line naming its type and a `data:` line holding a JSON
//! document that begins with the same type and a sequence number counted from 0. The response is announced
//! (`response.created`, `response.in_progress`); each item of the output is added, filled by deltas and done
//! (`response.output_item.added` … `response.output_item.done`, with a content part and a text for a message, a
//! summary for reasoning, arguments for a function call, an input for a free-text tool call); and the response
//! ends with `response.completed`, `response.incomplete` or `response.failed`, which carry the whole response,
//! its output and its `usage`. There is no `[DONE]` marker. The plain answer is that same response object.

use serde_json::{Value, json};

use super::errors;
use super::plan::{Ctx, Ending, Frame, Usage, Wire};
use super::request::Format;
use super::script::{Finish, Pace, Segment, Step, pieces};

/// The `usage` of a response.
fn usage_json(usage: &Usage) -> Value {
    let mut value = json!({
        "input_tokens": usage.input,
        "input_tokens_details": { "cached_tokens": usage.cached, "cache_write_tokens": usage.cache_write },
        "output_tokens": usage.output,
        "output_tokens_details": { "reasoning_tokens": usage.reasoning },
        "total_tokens": usage.input + usage.output
    });
    if let Some(cost) = usage
        .cost
        .as_deref()
        .and_then(|cost| cost.parse::<f64>().ok())
        .and_then(serde_json::Number::from_f64)
    {
        value["cost"] = Value::Number(cost);
    }
    value
}

/// How a response ends, for the object that says so.
struct Closing<'a> {
    status: &'a str,
    incomplete: Option<&'a str>,
    error: Option<Value>,
}

/// The response object, as the events that announce and close it carry it and as a plain answer is.
fn response_object(
    ctx: &Ctx,
    output: Vec<Value>,
    usage: Option<&Usage>,
    closing: &Closing<'_>,
) -> Value {
    let finished = closing.status == "completed";
    json!({
        "id": format!("resp_mock_{:06}", ctx.seq),
        "object": "response",
        "created_at": ctx.created_at,
        "status": closing.status,
        "completed_at": if finished { Value::from(ctx.created_at) } else { Value::Null },
        "error": closing.error.clone().unwrap_or(Value::Null),
        "incomplete_details": closing.incomplete.map_or(Value::Null, |reason| json!({ "reason": reason })),
        "instructions": null,
        "max_output_tokens": null,
        "model": ctx.model,
        "output": output,
        "parallel_tool_calls": true,
        "previous_response_id": ctx.previous_response_id,
        "reasoning": { "effort": null, "summary": null },
        "store": true,
        "temperature": 1,
        "text": { "format": { "type": "text" } },
        "tool_choice": "auto",
        "tools": [],
        "top_p": 1,
        "truncation": "disabled",
        "usage": usage.map_or(Value::Null, usage_json),
        "user": null,
        "metadata": {}
    })
}

/// An output item as it is when it is done.
fn done_item(ctx: &Ctx, index: usize, step: &Step) -> Option<Value> {
    let seq = ctx.seq;
    match step {
        Step::Text { text, .. } => Some(json!({
            "id": format!("msg_mock_{seq:06}_{index}"), "type": "message", "status": "completed",
            "content": [{ "type": "output_text", "annotations": [], "logprobs": [], "text": text }],
            "role": "assistant"
        })),
        Step::Reasoning { text, .. } => {
            let mut item = json!({
                "id": format!("rs_mock_{seq:06}_{index}"), "type": "reasoning",
                "summary": [{ "type": "summary_text", "text": text }]
            });
            if ctx.encrypted_reasoning {
                item["encrypted_content"] = Value::from(format!("mock-encrypted-{seq:06}-{index}"));
            }
            Some(item)
        }
        Step::Tool {
            name, arguments, ..
        } => Some(function_item(
            json!({
                "id": format!("fc_mock_{seq:06}_{index}"), "type": "function_call", "status": "completed",
                "arguments": arguments, "call_id": format!("call_mock_{seq:06}_{index}"), "name": name
            }),
            name,
        )),
        Step::Custom { name, input, .. } => Some(json!({
            "id": format!("ctc_mock_{seq:06}_{index}"), "type": "custom_tool_call", "status": "completed",
            "call_id": format!("call_mock_{seq:06}_{index}"), "name": name, "input": input
        })),
        _ => None,
    }
}

/// A dotted scripted name addresses the namespace advertised by the native Responses client.
fn function_item(mut item: Value, name: &str) -> Value {
    if let Some((namespace, function)) = name.rsplit_once('.') {
        item["namespace"] = json!(namespace);
        item["name"] = json!(function);
    }
    item
}

/// The deltas of a text, a wait before each.
fn deltas(wire: &mut Wire, text: &str, pace: Pace, mut one: impl FnMut(&mut Wire, &str)) {
    for piece in pieces(text, pace.chunk) {
        wire.wait(pace.delay);
        one(wire, &piece);
    }
}

/// Writes the events of one item: added, filled, and, unless the stream stops inside it, done.
fn item_events(wire: &mut Wire, ctx: &Ctx, index: usize, step: &Step, done: &Value, close: bool) {
    let seq = ctx.seq;
    let id = done["id"].clone();
    match step {
        Step::Text { text, pace } => {
            wire.numbered(
                "response.output_item.added",
                json!({ "output_index": index, "item": {
                    "id": id, "type": "message", "status": "in_progress", "content": [], "role": "assistant"
                } }),
            );
            wire.numbered(
                "response.content_part.added",
                json!({ "item_id": id, "output_index": index, "content_index": 0,
                        "part": { "type": "output_text", "annotations": [], "logprobs": [], "text": "" } }),
            );
            deltas(wire, text, *pace, |wire, piece| {
                wire.numbered(
                    "response.output_text.delta",
                    json!({ "item_id": id, "output_index": index, "content_index": 0, "delta": piece, "logprobs": [] }),
                );
            });
            if close {
                wire.numbered(
                    "response.output_text.done",
                    json!({ "item_id": id, "output_index": index, "content_index": 0, "text": text, "logprobs": [] }),
                );
                wire.numbered(
                    "response.content_part.done",
                    json!({ "item_id": id, "output_index": index, "content_index": 0,
                            "part": { "type": "output_text", "annotations": [], "logprobs": [], "text": text } }),
                );
            }
        }
        Step::Reasoning { text, pace } => {
            wire.numbered(
                "response.output_item.added",
                json!({ "output_index": index, "item": { "id": id, "type": "reasoning", "summary": [] } }),
            );
            wire.numbered(
                "response.reasoning_summary_part.added",
                json!({ "item_id": id, "output_index": index, "summary_index": 0,
                        "part": { "type": "summary_text", "text": "" } }),
            );
            deltas(wire, text, *pace, |wire, piece| {
                wire.numbered(
                    "response.reasoning_summary_text.delta",
                    json!({ "item_id": id, "output_index": index, "summary_index": 0, "delta": piece }),
                );
            });
            if close {
                wire.numbered(
                    "response.reasoning_summary_text.done",
                    json!({ "item_id": id, "output_index": index, "summary_index": 0, "text": text }),
                );
                wire.numbered(
                    "response.reasoning_summary_part.done",
                    json!({ "item_id": id, "output_index": index, "summary_index": 0,
                            "part": { "type": "summary_text", "text": text } }),
                );
            }
        }
        Step::Tool {
            name,
            arguments,
            pace,
        } => {
            wire.numbered(
                "response.output_item.added",
                json!({ "output_index": index, "item": function_item(json!({
                    "id": id, "type": "function_call", "status": "in_progress", "arguments": "",
                    "call_id": format!("call_mock_{seq:06}_{index}"), "name": name
                }),name) }),
            );
            deltas(wire, arguments, *pace, |wire, piece| {
                wire.numbered(
                    "response.function_call_arguments.delta",
                    json!({ "item_id": id, "output_index": index, "delta": piece }),
                );
            });
            if close {
                wire.numbered(
                    "response.function_call_arguments.done",
                    json!({ "item_id": id, "output_index": index, "arguments": arguments }),
                );
            }
        }
        Step::Custom { name, input, pace } => {
            wire.numbered(
                "response.output_item.added",
                json!({ "output_index": index, "item": {
                    "id": id, "type": "custom_tool_call", "status": "in_progress",
                    "call_id": format!("call_mock_{seq:06}_{index}"), "name": name, "input": ""
                } }),
            );
            deltas(wire, input, *pace, |wire, piece| {
                wire.numbered(
                    "response.custom_tool_call_input.delta",
                    json!({ "output_index": index, "item_id": id, "delta": piece }),
                );
            });
            if close {
                wire.numbered(
                    "response.custom_tool_call_input.done",
                    json!({ "output_index": index, "item_id": id, "input": input }),
                );
            }
        }
        _ => return,
    }
    if close {
        wire.numbered(
            "response.output_item.done",
            json!({ "output_index": index, "item": done }),
        );
    }
}

/// Whether the step after position `at` ends the stream where it stands, which leaves an item before it open.
fn ends_inside(steps: &[Step], at: usize) -> bool {
    matches!(
        steps.get(at + 1),
        Some(Step::Cut | Step::Truncate | Step::StreamError(_))
    )
}

/// The frames of a stream, and how it ends.
#[must_use]
pub fn stream(segment: &Segment, usage: &Usage, ctx: &Ctx) -> (Vec<Frame>, Ending) {
    let mut wire = if segment.gateway {
        Wire::bare()
    } else {
        Wire::new()
    };
    if segment.gateway {
        wire.comment("");
    }
    let open = Closing {
        status: "in_progress",
        incomplete: None,
        error: None,
    };
    wire.numbered(
        "response.created",
        json!({ "response": response_object(ctx, Vec::new(), None, &open) }),
    );
    wire.numbered(
        "response.in_progress",
        json!({ "response": response_object(ctx, Vec::new(), None, &open) }),
    );
    let mut output: Vec<Value> = Vec::new();
    for (at, step) in segment.steps.iter().enumerate() {
        match step {
            Step::Sleep(duration) => wire.wait(*duration),
            Step::Keepalive => wire.comment("keep-alive"),
            Step::Event { name, data } => wire.raw(Some(name), data),
            Step::Garbage(text) => wire.raw(None, text),
            Step::StreamError(wanted) => {
                let sequence = wire.next_sequence();
                let (name, data) = errors::stream_event(Format::Responses, wanted, sequence);
                wire.event(name, &data);
                return (wire.finish(), Ending::StreamError);
            }
            Step::Cut => {
                wire.abort();
                return (wire.finish(), Ending::Cut);
            }
            Step::Truncate => return (wire.finish(), Ending::Truncated),
            item => {
                let index = output.len();
                if let Some(done) = done_item(ctx, index, item) {
                    item_events(
                        &mut wire,
                        ctx,
                        index,
                        item,
                        &done,
                        !ends_inside(&segment.steps, at),
                    );
                    output.push(done);
                }
            }
        }
    }
    match &segment.finish {
        Finish::Complete => {
            let closing = Closing {
                status: "completed",
                incomplete: None,
                error: None,
            };
            wire.numbered(
                "response.completed",
                json!({ "response": response_object(ctx, output, Some(usage), &closing) }),
            );
        }
        Finish::Incomplete(reason) => {
            let closing = Closing {
                status: "incomplete",
                incomplete: Some(reason),
                error: None,
            };
            wire.numbered(
                "response.incomplete",
                json!({ "response": response_object(ctx, output, Some(usage), &closing) }),
            );
        }
        Finish::Fail(wanted) => {
            let closing = Closing {
                status: "failed",
                incomplete: None,
                error: Some(
                    json!({ "code": errors::response_code(wanted), "message": errors::message(Format::Responses, wanted) }),
                ),
            };
            wire.numbered(
                "response.failed",
                json!({ "response": response_object(ctx, output, None, &closing) }),
            );
        }
    }
    if segment.gateway {
        wire.done_marker();
    }
    let ending = if matches!(segment.finish, Finish::Fail(_)) {
        Ending::StreamError
    } else {
        Ending::Completed
    };
    (wire.finish(), ending)
}

/// The response object a plain answer is.
#[must_use]
pub fn plain(segment: &Segment, usage: &Usage, ctx: &Ctx) -> Value {
    let mut output: Vec<Value> = Vec::new();
    for step in &segment.steps {
        if let Some(item) = done_item(ctx, output.len(), step) {
            output.push(item);
        }
    }
    match &segment.finish {
        Finish::Complete => response_object(
            ctx,
            output,
            Some(usage),
            &Closing {
                status: "completed",
                incomplete: None,
                error: None,
            },
        ),
        Finish::Incomplete(reason) => response_object(
            ctx,
            output,
            Some(usage),
            &Closing {
                status: "incomplete",
                incomplete: Some(reason),
                error: None,
            },
        ),
        Finish::Fail(wanted) => response_object(
            ctx,
            output,
            None,
            &Closing {
                status: "failed",
                incomplete: None,
                error: Some(
                    json!({ "code": errors::response_code(wanted), "message": errors::message(Format::Responses, wanted) }),
                ),
            },
        ),
    }
}

/// The answer to a request to compact a conversation: the user's items it was given, and a compaction item.
#[must_use]
pub fn compaction(ctx: &Ctx, kept: Vec<Value>, usage: &Usage) -> Value {
    let mut output = kept;
    output.push(json!({
        "id": format!("cmp_mock_{:06}", ctx.seq), "type": "compaction",
        "encrypted_content": format!("mock-compacted-{:06}", ctx.seq)
    }));
    json!({
        "id": format!("resp_mock_{:06}", ctx.seq),
        "object": "response.compaction",
        "output": output,
        "created_at": ctx.created_at,
        "usage": usage_json(usage)
    })
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
    fn a_text_answer_is_announced_filled_by_deltas_closed_and_completed_with_its_usage() {
        let (events, ending) =
            render("chunk 6\nsay Hello there\nusage 120 30 cached=100 reasoning=10");
        assert_eq!(ending, Ending::Completed);
        assert_eq!(
            names(&events),
            [
                "response.created",
                "response.in_progress",
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
                "response.output_text.delta",
                "response.output_text.done",
                "response.content_part.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        for (name, data) in &events {
            assert_eq!(
                data["type"],
                name.as_str(),
                "the event repeats its name as its type"
            );
        }
        let sequence: Vec<_> = events
            .iter()
            .map(|(_, d)| d["sequence_number"].as_u64().unwrap())
            .collect();
        assert_eq!(sequence, (0..10).collect::<Vec<_>>());
        assert_eq!(events[4].1["delta"], "Hello ");
        assert_eq!(events[5].1["delta"], "there");
        assert_eq!(events[6].1["text"], "Hello there");
        let created = &events[0].1["response"];
        assert_eq!(created["id"], "resp_mock_000003");
        assert_eq!(created["status"], "in_progress");
        assert_eq!(created["usage"], Value::Null);
        assert_eq!(created["model"], "mock-large");
        let completed = &events[9].1["response"];
        assert_eq!(completed["status"], "completed");
        assert_eq!(completed["output"][0]["content"][0]["text"], "Hello there");
        assert_eq!(
            completed["usage"],
            json!({ "input_tokens": 120, "input_tokens_details": { "cached_tokens": 100, "cache_write_tokens": 0 },
                    "output_tokens": 30, "output_tokens_details": { "reasoning_tokens": 10 }, "total_tokens": 150 })
        );
    }

    #[test]
    fn reasoning_and_several_tool_calls_are_items_of_their_own_in_the_order_of_the_script() {
        let (events, _) = render(
            "chunk 100\nthink hmm\ntool read_file {\"path\":\"a\"}\ncustom apply_patch *** Begin\ntool list {}",
        );
        assert_eq!(
            names(&events),
            [
                "response.created",
                "response.in_progress",
                "response.output_item.added",
                "response.reasoning_summary_part.added",
                "response.reasoning_summary_text.delta",
                "response.reasoning_summary_text.done",
                "response.reasoning_summary_part.done",
                "response.output_item.done",
                "response.output_item.added",
                "response.function_call_arguments.delta",
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.output_item.added",
                "response.custom_tool_call_input.delta",
                "response.custom_tool_call_input.done",
                "response.output_item.done",
                "response.output_item.added",
                "response.function_call_arguments.delta",
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        let output = &events.last().unwrap().1["response"]["output"];
        assert_eq!(output.as_array().unwrap().len(), 4);
        assert_eq!(output[1]["type"], "function_call");
        assert_eq!(output[1]["name"], "read_file");
        assert_eq!(output[1]["arguments"], "{\"path\":\"a\"}");
        assert_eq!(output[1]["call_id"], "call_mock_000003_1");
        assert_eq!(output[2]["type"], "custom_tool_call");
        assert_eq!(output[2]["input"], "*** Begin");
        assert_eq!(output[3]["call_id"], "call_mock_000003_3");
        assert_eq!(output[0]["summary"][0]["text"], "hmm");
    }

    #[test]
    fn an_answer_that_stops_at_its_limit_is_incomplete_with_the_reason_and_still_has_usage() {
        let (events, _) = render("say cut sh\nincomplete");
        let last = &events.last().unwrap();
        assert_eq!(last.0, "response.incomplete");
        assert_eq!(last.1["response"]["status"], "incomplete");
        assert_eq!(
            last.1["response"]["incomplete_details"]["reason"],
            "max_output_tokens"
        );
        assert!(last.1["response"]["usage"].is_object());
    }

    #[test]
    fn a_failed_response_has_its_error_and_no_usage() {
        let (events, _) = render("say a\nfail overloaded");
        let last = &events.last().unwrap();
        assert_eq!(last.0, "response.failed");
        assert_eq!(last.1["response"]["status"], "failed");
        assert_eq!(last.1["response"]["error"]["code"], "server_is_overloaded");
        assert_eq!(last.1["response"]["usage"], Value::Null);
    }

    #[test]
    fn a_gateway_writes_no_event_names_opens_with_a_comment_and_ends_with_done() {
        let segment = Script::parse("gateway\nsay hi\nusage 5 2")
            .unwrap()
            .segment(0);
        let (frames, _) = stream(&segment, &usage_of(&segment, 4), &ctx());
        let text = String::from_utf8(body_of(&frames)).unwrap();
        assert!(
            text.starts_with(": \n\ndata: {\"type\":\"response.created\""),
            "{text}"
        );
        assert!(!text.contains("event:"));
        assert!(text.ends_with("data: [DONE]\n\n"));
        let mut parser = SseParser::new();
        let events = parser.push(text.as_bytes());
        assert_eq!(
            events.len(),
            9 + 1,
            "the comment is no event, the marker is one"
        );
        assert_eq!(events.last().unwrap().data, "[DONE]");
    }

    #[test]
    fn an_error_inside_the_stream_ends_it_with_an_error_event() {
        let (events, ending) = render("say abc\nstream_error rate_limit Slow\nsay never");
        assert_eq!(ending, Ending::StreamError);
        let last = events.last().unwrap();
        assert_eq!(last.0, "error");
        assert_eq!(last.1["code"], "rate_limit_exceeded");
        assert_eq!(last.1["message"], "Slow");
        assert!(!names(&events).contains(&"response.completed"));
    }

    #[test]
    fn a_cut_leaves_the_item_open_and_aborts_while_a_truncation_just_stops() {
        let (events, ending) = render("chunk 2\nsay abcdef\ncut\nsay never");
        assert_eq!(ending, Ending::Cut);
        assert_eq!(names(&events).last(), Some(&"response.output_text.delta"));
        assert!(
            !names(&events).contains(&"response.output_text.done"),
            "the item is left open"
        );
        let segment = Script::parse("say abc\ncut").unwrap().segment(0);
        let (frames, _) = stream(&segment, &usage_of(&segment, 4), &ctx());
        assert_eq!(frames.last(), Some(&Frame::Abort));
        let (_, ending) = render("say abc\ntruncate");
        assert_eq!(ending, Ending::Truncated);
    }

    #[test]
    fn waits_and_comments_are_where_the_script_put_them() {
        let segment = Script::parse("delay 7\nchunk 2\nsay abcd\nsleep 30\nkeepalive")
            .unwrap()
            .segment(0);
        let (frames, _) = stream(&segment, &usage_of(&segment, 4), &ctx());
        let waits: Vec<_> = frames
            .iter()
            .filter_map(|f| {
                if let Frame::Wait(d) = f {
                    Some(d.as_millis())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            waits,
            [7, 7, 30],
            "a wait before each of two pieces, then the sleep"
        );
        assert!(
            String::from_utf8(body_of(&frames))
                .unwrap()
                .contains(": keep-alive\n\n")
        );
    }

    #[test]
    fn encrypted_reasoning_is_given_only_to_a_request_that_asks_for_it() {
        let segment = Script::parse("think x").unwrap().segment(0);
        let usage = usage_of(&segment, 4);
        let plain_answer = plain(&segment, &usage, &ctx());
        assert!(plain_answer["output"][0].get("encrypted_content").is_none());
        let asked = Ctx {
            encrypted_reasoning: true,
            ..ctx()
        };
        assert!(plain(&segment, &usage, &asked)["output"][0]["encrypted_content"].is_string());
    }

    #[test]
    fn a_plain_answer_is_the_same_response_with_its_output_and_usage() {
        let segment = Script::parse("say Hello\ntool t {}\nusage 10 5")
            .unwrap()
            .segment(0);
        let usage = usage_of(&segment, 4);
        let answer = plain(&segment, &usage, &ctx());
        assert_eq!(answer["object"], "response");
        assert_eq!(answer["status"], "completed");
        assert_eq!(answer["output"][0]["content"][0]["text"], "Hello");
        assert_eq!(answer["output"][1]["call_id"], "call_mock_000003_1");
        assert_eq!(answer["usage"]["total_tokens"], 15);
        let (events, _) = render("say Hello\ntool t {}\nusage 10 5");
        assert_eq!(
            events.last().unwrap().1["response"]["output"],
            answer["output"]
        );
    }

    #[test]
    fn a_cost_the_script_states_is_in_the_usage_as_a_number() {
        let segment = Script::parse("usage 1 1 cost=0.0021").unwrap().segment(0);
        let answer = plain(&segment, &usage_of(&segment, 4), &ctx());
        assert_eq!(answer["usage"]["cost"], 0.0021);
    }

    #[test]
    fn the_answer_to_a_compaction_keeps_what_it_was_given_and_adds_a_compaction_item() {
        let kept = vec![json!({ "type": "message", "role": "user", "content": "hi" })];
        let usage = Usage {
            input: 9,
            output: 2,
            cached: 0,
            reasoning: 0,
            cache_write: 0,
            cost: None,
        };
        let answer = compaction(&ctx(), kept, &usage);
        assert_eq!(answer["object"], "response.compaction");
        assert_eq!(answer["output"].as_array().unwrap().len(), 2);
        assert_eq!(answer["output"][1]["type"], "compaction");
        assert_eq!(answer["usage"]["total_tokens"], 11);
        assert!(answer.get("model").is_none());
    }
}
