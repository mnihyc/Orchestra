//! The Chat Completions format of OpenAI as the scripted server writes it: the chunks of a stream and the document of a plain answer.
//!
//! A stream is server-sent events with `data:` lines alone, each a JSON document of the object
//! `chat.completion.chunk`: the first carries the role, then come the deltas of the answer (`reasoning_content`
//! for what the model thought, `content` for what it said, `tool_calls` for the calls, whose arguments arrive in
//! pieces under the call's index), a chunk with the reason it finished and no delta, and, since the client asked
//! for it, a last chunk with no choice and the use. The stream ends with `data: [DONE]`. The use counts the way
//! the vendors do: `prompt_tokens` holds the cached tokens, which `prompt_tokens_details.cached_tokens` names
//! (and `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`, the words of DeepSeek's API), and
//! `completion_tokens` holds the reasoning, which `completion_tokens_details.reasoning_tokens` names.

use serde_json::{Value, json};

use super::errors;
use super::plan::{Ctx, Ending, Frame, Usage, Wire};
use super::request::Format;
use super::script::{Finish, Pace, Segment, Step, pieces};

/// The `usage` of a chunk or of a plain answer.
fn usage_object(usage: &Usage) -> Value {
    let mut value = json!({
        "prompt_tokens": usage.input,
        "completion_tokens": usage.output,
        "total_tokens": usage.input + usage.output,
        "prompt_tokens_details": { "cached_tokens": usage.cached },
        "completion_tokens_details": { "reasoning_tokens": usage.reasoning },
        "prompt_cache_hit_tokens": usage.cached,
        "prompt_cache_miss_tokens": usage.input - usage.cached
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

/// Why the answer finished.
fn finish_reason(segment: &Segment) -> &'static str {
    if matches!(segment.finish, Finish::Incomplete(_)) {
        return "length";
    }
    if segment
        .steps
        .iter()
        .any(|step| matches!(step, Step::Tool { .. } | Step::Custom { .. }))
    {
        "tool_calls"
    } else {
        "stop"
    }
}

/// The arguments a free-text call is sent as: this format has no such tool, so it is a call with one member.
fn custom_arguments(input: &str) -> String {
    json!({ "input": input }).to_string()
}

/// The identifier of the call a step makes.
fn call_id(ctx: &Ctx, call: usize) -> String {
    format!("call_mock_{:06}_{call}", ctx.seq)
}

fn chunk(ctx: &Ctx, choices: Value) -> Value {
    json!({
        "id": format!("chatcmpl-mock-{:06}", ctx.seq),
        "object": "chat.completion.chunk",
        "created": ctx.created_at,
        "model": ctx.model,
        "choices": choices
    })
}

fn delta(ctx: &Ctx, delta: Value) -> Value {
    chunk(
        ctx,
        json!([{ "index": 0, "delta": delta, "finish_reason": null }]),
    )
}

/// The deltas of a text, a wait before each.
fn deltas(wire: &mut Wire, text: &str, pace: Pace, mut one: impl FnMut(&mut Wire, &str)) {
    for piece in pieces(text, pace.chunk) {
        wire.wait(pace.delay);
        one(wire, &piece);
    }
}

/// The frames of a stream, and how it ends.
#[must_use]
pub fn stream(segment: &Segment, usage: &Usage, ctx: &Ctx) -> (Vec<Frame>, Ending) {
    let mut wire = Wire::bare();
    wire.event(
        "chunk",
        &delta(ctx, json!({ "role": "assistant", "content": "" })),
    );
    let mut calls = 0_usize;
    for step in &segment.steps {
        match step {
            Step::Sleep(duration) => wire.wait(*duration),
            Step::Keepalive => wire.comment("keep-alive"),
            Step::Event { name, data } => wire.raw(Some(name), data),
            Step::Garbage(text) => wire.raw(None, text),
            Step::StreamError(wanted) => {
                let (name, data) = errors::stream_event(Format::Chat, wanted, 0);
                wire.event(name, &data);
                return (wire.finish(), Ending::StreamError);
            }
            Step::Cut => {
                wire.abort();
                return (wire.finish(), Ending::Cut);
            }
            Step::Truncate => return (wire.finish(), Ending::Truncated),
            Step::Reasoning { text, pace } => {
                deltas(&mut wire, text, *pace, |wire, piece| {
                    wire.event("chunk", &delta(ctx, json!({ "reasoning_content": piece })));
                });
            }
            Step::Text { text, pace } => {
                deltas(&mut wire, text, *pace, |wire, piece| {
                    wire.event("chunk", &delta(ctx, json!({ "content": piece })));
                });
            }
            Step::Tool {
                name,
                arguments,
                pace,
            } => {
                call_events(&mut wire, ctx, calls, name, arguments, *pace);
                calls += 1;
            }
            Step::Custom { name, input, pace } => {
                call_events(&mut wire, ctx, calls, name, &custom_arguments(input), *pace);
                calls += 1;
            }
        }
    }
    if let Finish::Fail(wanted) = &segment.finish {
        let (name, data) = errors::stream_event(Format::Chat, wanted, 0);
        wire.event(name, &data);
        return (wire.finish(), Ending::StreamError);
    }
    wire.event(
        "chunk",
        &chunk(
            ctx,
            json!([{ "index": 0, "delta": {}, "finish_reason": finish_reason(segment) }]),
        ),
    );
    let mut last = chunk(ctx, json!([]));
    last["usage"] = usage_object(usage);
    wire.event("chunk", &last);
    wire.raw(None, "[DONE]");
    (wire.finish(), Ending::Completed)
}

/// The chunks of one call: its start, with the name, and its arguments in pieces.
fn call_events(wire: &mut Wire, ctx: &Ctx, index: usize, name: &str, arguments: &str, pace: Pace) {
    wire.event(
        "chunk",
        &delta(
            ctx,
            json!({ "tool_calls": [{
                "index": index, "id": call_id(ctx, index), "type": "function",
                "function": { "name": name, "arguments": "" }
            }] }),
        ),
    );
    deltas(wire, arguments, pace, |wire, piece| {
        wire.event(
            "chunk",
            &delta(
                ctx,
                json!({ "tool_calls": [{ "index": index, "function": { "arguments": piece } }] }),
            ),
        );
    });
}

/// The document a plain answer is. A failure is not a completion: it is the error of [`errors::reply`], and the
/// caller gives that.
#[must_use]
pub fn plain(segment: &Segment, usage: &Usage, ctx: &Ctx) -> Value {
    let (mut said, mut thought) = (String::new(), String::new());
    let mut calls: Vec<Value> = Vec::new();
    for step in &segment.steps {
        match step {
            Step::Text { text, .. } => said.push_str(text),
            Step::Reasoning { text, .. } => thought.push_str(text),
            Step::Tool {
                name, arguments, ..
            } => calls.push(json!({
                "id": call_id(ctx, calls.len()), "type": "function",
                "function": { "name": name, "arguments": arguments }
            })),
            Step::Custom { name, input, .. } => calls.push(json!({
                "id": call_id(ctx, calls.len()), "type": "function",
                "function": { "name": name, "arguments": custom_arguments(input) }
            })),
            _ => {}
        }
    }
    let mut message = json!({ "role": "assistant", "content": if said.is_empty() && !calls.is_empty() { Value::Null } else { Value::from(said) } });
    if !thought.is_empty() {
        message["reasoning_content"] = Value::from(thought);
    }
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    json!({
        "id": format!("chatcmpl-mock-{:06}", ctx.seq),
        "object": "chat.completion",
        "created": ctx.created_at,
        "model": ctx.model,
        "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason(segment) }],
        "usage": usage_object(usage)
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

    fn render(script: &str) -> (Vec<Value>, Ending, String) {
        let segment = Script::parse(script).unwrap().segment(0);
        let usage = usage_of(&segment, 40);
        let (frames, ending) = stream(&segment, &usage, &ctx());
        let body = String::from_utf8(body_of(&frames)).unwrap();
        let mut parser = SseParser::new();
        let events = parser
            .push(body.as_bytes())
            .into_iter()
            .filter(|event| event.data != "[DONE]")
            .map(|event| serde_json::from_str(&event.data).unwrap())
            .collect();
        (events, ending, body)
    }

    #[test]
    fn a_text_answer_streams_as_chunks_with_a_finish_reason_and_a_last_chunk_of_use() {
        let (chunks, ending, body) = render("chunk 6\nsay Hello there\nusage 25 15 cached=10");
        assert_eq!(ending, Ending::Completed);
        assert!(body.ends_with("data: [DONE]\n\n"), "{body}");
        assert!(
            !body.contains("event:"),
            "a chat stream has data lines alone"
        );
        let text: String = chunks
            .iter()
            .filter_map(|chunk| chunk["choices"][0]["delta"]["content"].as_str())
            .collect();
        assert_eq!(text, "Hello there");
        let finish = chunks
            .iter()
            .find(|chunk| chunk["choices"][0]["finish_reason"] == "stop")
            .expect("a chunk that says why it finished");
        assert_eq!(finish["choices"][0]["delta"], json!({}));
        let last = chunks.last().unwrap();
        assert_eq!(last["choices"], json!([]));
        assert_eq!(last["usage"]["prompt_tokens"], 25);
        assert_eq!(last["usage"]["prompt_tokens_details"]["cached_tokens"], 10);
        assert_eq!(last["usage"]["prompt_cache_miss_tokens"], 15);
        assert_eq!(last["usage"]["total_tokens"], 40);
    }

    #[test]
    fn reasoning_and_calls_stream_in_their_own_members_and_a_call_has_one_id_and_its_arguments_in_pieces()
     {
        let (chunks, _, _) = render(
            "chunk 4\nthink pondering\nsay ok\ntool bash {\"command\":\"ls\"}\ntool read {\"file_path\":\"a\"}\nusage 10 20 reasoning=5",
        );
        let thought: String = chunks
            .iter()
            .filter_map(|chunk| chunk["choices"][0]["delta"]["reasoning_content"].as_str())
            .collect();
        assert_eq!(thought, "pondering");
        let calls: Vec<&Value> = chunks
            .iter()
            .filter_map(|chunk| chunk["choices"][0]["delta"]["tool_calls"].get(0))
            .collect();
        let first_ids: Vec<&str> = calls
            .iter()
            .filter_map(|call| call["id"].as_str())
            .collect();
        assert_eq!(first_ids, ["call_mock_000003_0", "call_mock_000003_1"]);
        let arguments: String = calls
            .iter()
            .filter(|call| call["index"] == 0)
            .filter_map(|call| call["function"]["arguments"].as_str())
            .collect();
        assert_eq!(arguments, "{\"command\":\"ls\"}");
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk["choices"][0]["finish_reason"] == "tool_calls")
        );
        assert_eq!(
            chunks.last().unwrap()["usage"]["completion_tokens_details"]["reasoning_tokens"],
            5
        );
    }

    #[test]
    fn a_plain_answer_is_a_completion_with_its_message_calls_and_use() {
        let segment =
            Script::parse("think hm\nsay done\ntool bash {\"command\":\"ls\"}\nusage 7 9")
                .unwrap()
                .segment(0);
        let usage = usage_of(&segment, 0);
        let answer = plain(&segment, &usage, &ctx());
        assert_eq!(answer["object"], "chat.completion");
        assert_eq!(answer["choices"][0]["message"]["content"], "done");
        assert_eq!(answer["choices"][0]["message"]["reasoning_content"], "hm");
        assert_eq!(
            answer["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "bash"
        );
        assert_eq!(answer["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(answer["usage"]["total_tokens"], 16);
    }

    #[test]
    fn a_cut_leaves_the_stream_broken_after_what_was_sent() {
        let (_, ending, _) = render("say half of it\ncut");
        assert_eq!(ending, Ending::Cut);
    }
}
