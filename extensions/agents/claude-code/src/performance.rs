//! Claude message boundaries and counts, with untimed sub-agents and explicit hidden thinking.

use crate::stream::Frame;
use orch_agent_contract::events::{TokenUsage, TokensReportedApart, UsageRequest};
use orch_agent_contract::requests::RequestTokens;
use orch_types::{Json, Time};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
struct Open {
    request: UsageRequest,
    counts: RequestTokens,
    thinking: bool,
    streamed_thinking: bool,
    stopped: bool,
}

/// The documented stream-json events, sampled without calculating a duration or speed.
#[derive(Debug, Default)]
pub struct Requests {
    pending: Option<UsageRequest>,
    open: HashMap<String, Open>,
    finished: HashSet<String>,
    serial: u64,
}

fn text<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or_default()
}
fn parts(usage: &Json) -> TokensReportedApart {
    let n = |key: &str| {
        usage
            .get(key)
            .and_then(Json::as_i64)
            .unwrap_or_default()
            .max(0)
    };
    TokensReportedApart {
        input: n("input_tokens"),
        cache_read: n("cache_read_input_tokens"),
        cache_write: n("cache_creation_input_tokens"),
        output: n("output_tokens"),
        reasoning: 0,
    }
}

fn search_count(usage: &Json) -> Option<i64> {
    usage
        .pointer("/server_tool_use/web_search_requests")
        .and_then(Json::as_i64)
        .filter(|count| *count >= 0)
}

fn counts(usage: &Json) -> RequestTokens {
    // Reuse the existing shared normalization of Claude's uncached-input parts,
    // retaining absence instead of publishing that normalizer's numeric defaults.
    let mut counts: RequestTokens = TokenUsage::from_parts(parts(usage)).into();
    let known = |key: &str| usage.get(key).and_then(Json::as_i64);
    if ![
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .all(|key| known(key).is_some())
    {
        counts.input_tokens = None;
    }
    counts.cached_input_tokens = known("cache_read_input_tokens");
    counts.cache_write_tokens = known("cache_creation_input_tokens");
    counts.output_tokens = known("output_tokens");
    counts.reasoning_tokens = usage
        .pointer("/output_tokens_details/thinking_tokens")
        .and_then(Json::as_i64);
    counts.total_tokens = None;
    counts
}

impl Requests {
    /// The caller has given the input to the harness.
    pub fn asked(&mut self, at: Time) -> UsageRequest {
        if let Some(pending) = &self.pending {
            return pending.clone();
        }
        self.serial += 1;
        let request = UsageRequest {
            id: format!("main:{}", self.serial),
            asked: Some(at),
            attempt: Some(1),
            tool_refs: Some(Vec::new()),
            ..UsageRequest::default()
        };
        self.pending = Some(request.clone());
        request
    }

    /// A process loss closes observations honestly, without inventing message_stop or final counts.
    pub fn abort(
        &mut self,
        code: &str,
        message: Option<&str>,
        said: Option<&str>,
    ) -> Vec<UsageRequest> {
        let mut requests: Vec<_> = self.open.drain().map(|(_, open)| open.request).collect();
        requests.extend(self.pending.take());
        for request in &mut requests {
            request.finish(code, message, said);
            self.finished.insert(request.id.clone());
        }
        requests
    }

    /// Observes a frame once; only the main stream has timed boundaries.
    pub fn observe(
        &mut self,
        frame: &Frame,
        at: Time,
        outcome: Option<(&str, Option<&str>)>,
    ) -> Vec<UsageRequest> {
        let parent = frame.parent_tool_use_id();
        let stream = parent.unwrap_or("main").to_owned();
        let event = frame.get("event").unwrap_or(&Json::Null);
        match frame.text("type") {
            Some("system")
                if frame.text("subtype") == Some("api_retry")
                    && frame.get("parent_tool_use_id").is_none_or(Json::is_null) =>
            {
                let native_next = frame
                    .get("attempt")
                    .and_then(Json::as_i64)
                    .filter(|n| *n > 0)
                    .and_then(|n| n.checked_add(1));
                if native_next.is_some()
                    && !self.open.contains_key("main")
                    && self
                        .pending
                        .as_ref()
                        .is_some_and(|request| request.attempt == native_next)
                {
                    return Vec::new();
                }
                let retry = Frame::from_value(
                    serde_json::json!({"type":"result","api_error_status":frame.get("error_status"),"api_error":frame.text("error"),"result":frame.text("message").unwrap_or("The provider request is being retried.")}),
                );
                let error = crate::failure::classify(
                    &retry,
                    &crate::failure::Notes::default(),
                    at.get(),
                    &chrono::Utc,
                );
                let mut reports = Vec::new();
                let mut next_attempt = None;
                let mut retry_of = None;
                if let Some(mut failed) = self
                    .open
                    .remove("main")
                    .map(|open| open.request)
                    .or_else(|| self.pending.take())
                {
                    next_attempt = failed.attempt.and_then(|n| n.checked_add(1));
                    retry_of = Some(failed.id.clone());
                    failed.error_reason = error.reason.clone();
                    failed.upstream_status = error.status;
                    failed.finish(&error.code, Some(&error.message), error.said.as_deref());
                    self.finished.insert(failed.id.clone());
                    reports.push(failed);
                }
                let mut next = self.asked(at);
                next.attempt = native_next.or(next_attempt);
                next.retry_of = retry_of;
                self.pending = Some(next.clone());
                reports.push(next);
                return reports;
            }
            Some("user")
                if parent.is_none()
                    && frame
                        .blocks()
                        .iter()
                        .any(|b| text(b, "type") == "tool_result") =>
            {
                return if self.pending.is_none() {
                    vec![self.asked(at)]
                } else {
                    Vec::new()
                };
            }
            Some("stream_event") => match text(event, "type") {
                "message_start" => {
                    let mut reports = Vec::new();
                    if let Some(mut replaced) = self.open.remove(&stream) {
                        replaced.request.finish(
                            "provider_unreachable",
                            Some("The previous response ended before its closing event."),
                            None,
                        );
                        self.finished.insert(replaced.request.id.clone());
                        reports.push(replaced.request);
                    }
                    let message = event.get("message").unwrap_or(&Json::Null);
                    self.serial += 1;
                    let id = text(message, "id");
                    let mut raw = if parent.is_none() {
                        self.pending.take().unwrap_or_else(|| UsageRequest {
                            id: format!("main:{}", self.serial),
                            ..UsageRequest::default()
                        })
                    } else {
                        UsageRequest {
                            id: if id.is_empty() {
                                format!("{stream}:{}", self.serial)
                            } else {
                                format!("{stream}:{id}")
                            },
                            ..UsageRequest::default()
                        }
                    };
                    raw.tool_refs = Some(Vec::new());
                    raw.provider_request_id = (!id.is_empty()).then(|| id.to_owned());
                    raw.model = message
                        .get("model")
                        .and_then(Json::as_str)
                        .map(str::to_owned);
                    raw.web_searches = search_count(message.get("usage").unwrap_or(&Json::Null));
                    raw.subagent = parent.is_some();
                    raw.first_output = parent.is_none().then_some(at);
                    raw.output_streamed = Some(parent.is_none());
                    let mut initial = counts(message.get("usage").unwrap_or(&Json::Null));
                    initial.output_tokens = None;
                    initial.reasoning_tokens = None; // message_start's output count is a placeholder, not final usage.
                    self.open.insert(
                        stream.clone(),
                        Open {
                            request: raw.clone(),
                            counts: initial,
                            thinking: false,
                            streamed_thinking: false,
                            stopped: false,
                        },
                    );
                    reports.push(raw);
                    return reports;
                }
                "content_block_start" => {
                    let block = event.get("content_block").unwrap_or(&Json::Null);
                    if text(block, "type") == "tool_use"
                        && let Some(open) = self.open.get_mut(&stream)
                    {
                        let id = text(block, "id");
                        if !id.is_empty() {
                            let refs = open.request.tool_refs.get_or_insert_with(Vec::new);
                            if !refs.iter().any(|known| known == id) {
                                refs.push(id.to_owned());
                            }
                        }
                    }
                    if text(event.get("content_block").unwrap_or(&Json::Null), "type") == "thinking"
                        && let Some(open) = self.open.get_mut(&stream)
                    {
                        open.thinking = true;
                    }
                }
                "content_block_delta" => {
                    let delta = event.get("delta").unwrap_or(&Json::Null);
                    if let Some(open) = self.open.get_mut(&stream) {
                        if text(delta, "type") == "thinking_delta"
                            && !text(delta, "thinking").is_empty()
                        {
                            open.thinking = true;
                            open.streamed_thinking = true;
                        }
                        if text(delta, "type") == "signature_delta" {
                            open.thinking = true;
                        }
                    }
                }
                "message_delta" => {
                    if let Some(open) = self.open.get_mut(&stream) {
                        if let Some(searches) =
                            search_count(event.get("usage").unwrap_or(&Json::Null))
                        {
                            open.request.web_searches = Some(searches);
                        }
                        if let Some(output) =
                            event.pointer("/usage/output_tokens").and_then(Json::as_i64)
                        {
                            open.counts.output_tokens = Some(output);
                        }
                        if let Some(thinking) = event
                            .pointer("/usage/output_tokens_details/thinking_tokens")
                            .and_then(Json::as_i64)
                        {
                            open.counts.reasoning_tokens = Some(thinking);
                        }
                        open.stopped = event
                            .pointer("/delta/stop_reason")
                            .and_then(Json::as_str)
                            .is_some();
                    }
                }
                "message_stop" => {
                    if let Some(mut open) = self.open.remove(&stream) {
                        open.request.tokens = open.counts;
                        open.request.end = parent.is_none().then_some(at);
                        open.request.reasoning_hidden = (open.thinking
                            || open.request.tokens.reasoning_tokens.is_some_and(|n| n > 0))
                            && !open.streamed_thinking;
                        if open.stopped {
                            open.request.finish("done", None, None);
                        } else {
                            open.request.finish(
                                "provider_unreachable",
                                Some("The response stream ended before its stop reason."),
                                None,
                            );
                        }
                        self.finished.insert(open.request.id.clone());
                        return vec![open.request];
                    }
                }
                _ => {}
            },
            Some("assistant") if parent.is_some() => {
                let message = frame.get("message").unwrap_or(&Json::Null);
                let message_id = text(message, "id");
                if !message_id.is_empty()
                    && let Some(open) = self.open.get_mut(&stream).filter(|open| {
                        open.request.provider_request_id.as_deref() == Some(message_id)
                    })
                {
                    // The CLI also emits the whole assistant block before the stream's
                    // final usage delta. Its start-usage output count is not a completion.
                    let usage = message.get("usage").unwrap_or(&Json::Null);
                    let observed = counts(usage);
                    if open.request.model.is_none() {
                        open.request.model = message
                            .get("model")
                            .and_then(Json::as_str)
                            .map(str::to_owned);
                    }
                    open.counts.input_tokens = open.counts.input_tokens.or(observed.input_tokens);
                    open.counts.cached_input_tokens = open
                        .counts
                        .cached_input_tokens
                        .or(observed.cached_input_tokens);
                    open.counts.cache_write_tokens = open
                        .counts
                        .cache_write_tokens
                        .or(observed.cache_write_tokens);
                    open.request.web_searches = search_count(usage).or(open.request.web_searches);
                    for block in message
                        .get("content")
                        .and_then(Json::as_array)
                        .into_iter()
                        .flatten()
                    {
                        open.thinking |= text(block, "type") == "thinking";
                        if text(block, "type") == "tool_use" {
                            let id = text(block, "id");
                            let refs = open.request.tool_refs.get_or_insert_with(Vec::new);
                            if !id.is_empty() && !refs.iter().any(|known| known == id) {
                                refs.push(id.to_owned());
                            }
                        }
                    }
                    return Vec::new();
                }
                let id = format!("{stream}:{message_id}");
                if self.finished.insert(id.clone()) {
                    self.open.remove(&stream);
                    let mut raw = UsageRequest {
                        id,
                        subagent: true,
                        model: message
                            .get("model")
                            .and_then(Json::as_str)
                            .map(str::to_owned),
                        provider_request_id: (!text(message, "id").is_empty())
                            .then(|| text(message, "id").to_owned()),
                        tool_refs: Some(
                            message
                                .get("content")
                                .and_then(Json::as_array)
                                .into_iter()
                                .flatten()
                                .filter(|block| text(block, "type") == "tool_use")
                                .filter_map(|block| block.get("id").and_then(Json::as_str))
                                .filter(|id| !id.is_empty())
                                .map(str::to_owned)
                                .collect(),
                        ),
                        web_searches: search_count(message.get("usage").unwrap_or(&Json::Null)),
                        tokens: counts(message.get("usage").unwrap_or(&Json::Null)),
                        output_streamed: Some(false),
                        ..UsageRequest::default()
                    };
                    raw.finish("done", None, None);
                    return vec![raw];
                }
            }
            Some("result") => {
                let (code, message) = outcome.unwrap_or(("done", None));
                let mut ended = Vec::new();
                let closing: Vec<_> = self
                    .open
                    .keys()
                    .filter(|stream| stream.as_str() == "main")
                    .cloned()
                    .collect();
                for stream in closing {
                    let Some(mut open) = self.open.remove(&stream) else {
                        continue;
                    };
                    // A result without message_stop has no proven last-output boundary.
                    open.request.tokens = open.counts;
                    open.request.reasoning_hidden = (open.thinking
                        || open.request.tokens.reasoning_tokens.is_some_and(|n| n > 0))
                        && !open.streamed_thinking;
                    open.request.finish(code, message, None);
                    ended.push(open.request);
                }
                let prior = self.finished.iter().any(|id| id.starts_with("main:"));
                if let Some(mut raw) = self.pending.take() {
                    if !prior {
                        raw.web_searches = search_count(frame.get("usage").unwrap_or(&Json::Null));
                        raw.tokens = counts(frame.get("usage").unwrap_or(&Json::Null));
                    }
                    raw.finish(code, message, None);
                    ended.push(raw);
                } else if ended.is_empty() && !prior {
                    let mut raw = UsageRequest {
                        id: format!("turn:{}", self.serial),
                        web_searches: search_count(frame.get("usage").unwrap_or(&Json::Null)),
                        tokens: if prior {
                            RequestTokens::default()
                        } else {
                            counts(frame.get("usage").unwrap_or(&Json::Null))
                        },
                        ..UsageRequest::default()
                    };
                    raw.finish(code, message, None);
                    ended.push(raw);
                }
                return ended;
            }
            _ => {}
        }
        Vec::new()
    }
}

#[cfg(test)]
mod native_child_usage_tests {
    #![allow(
        clippy::unwrap_used,
        reason = "a protocol assertion fails by panicking"
    )]
    use super::*;
    use serde_json::json;

    #[test]
    fn whole_child_echoes_do_not_finish_the_stream_before_its_final_usage() {
        let at: Time = "2026-11-08T03:00:00Z".parse().unwrap();
        let mut requests = Requests::default();
        let mut total = 0;
        for (id, input) in [("child-first", 70), ("child-after-barrier", 90)] {
            let frame = |event: Json| {
                Frame::from_value(
                    json!({"type":"stream_event","parent_tool_use_id":"owned-child","event":event}),
                )
            };
            let _=requests.observe(&frame(json!({"type":"message_start","message":{"id":id,"model":"actual-child-model",
                "usage":{"input_tokens":input,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":1}}})),at,None);
            let whole = Frame::from_value(
                json!({"type":"assistant","parent_tool_use_id":"owned-child","message":{"id":id,
                "model":"actual-child-model","content":[{"type":"tool_use","id":"child-command","name":"Bash","input":{}}],
                "usage":{"input_tokens":input,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":1}}}),
            );
            assert!(
                requests.observe(&whole, at, None).is_empty(),
                "the whole block is not the final usage boundary"
            );
            // The foreground result does not end this separately observed child request.
            let _ = requests.observe(
                &Frame::from_value(json!({"type":"result","subtype":"success"})),
                at,
                Some(("done", None)),
            );
            let _=requests.observe(&frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":10}})),at,None);
            let done = requests.observe(&frame(json!({"type":"message_stop"})), at, None);
            assert_eq!(done.len(), 1);
            let measured = &done[0];
            assert!(measured.subagent);
            assert_eq!(measured.tokens.input_tokens, Some(input));
            assert_eq!(measured.tokens.output_tokens, Some(10));
            assert_eq!(
                measured.tool_refs.as_deref(),
                Some(["child-command".to_owned()].as_slice())
            );
            assert!(
                measured.asked.is_none()
                    && measured.first_output.is_none()
                    && measured.end.is_none(),
                "untold child times stay unsupported"
            );
            total += measured.tokens.input_tokens.unwrap() + measured.tokens.output_tokens.unwrap();
            assert!(
                requests.observe(&whole, at, None).is_empty(),
                "the same native request is not counted again"
            );
        }
        assert_eq!(total, 180);
    }
}

#[cfg(test)]
mod native_search_tests {
    #![allow(clippy::expect_used, reason = "a test fails by panicking")]
    use super::*;
    use serde_json::json;

    #[test]
    fn native_search_reports_keep_the_vendor_cumulative_count() {
        let at = "2026-10-01T00:00:00Z".parse().expect("a fixed timestamp");
        let mut requests = Requests::default();
        let _ = requests.asked(at);
        let start = Frame::from_value(json!({"type":"stream_event","event":{
            "type":"message_start","message":{"id":"search-message","model":"chosen","usage":{
                "input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":1
            }}
        }}));
        let _ = requests.observe(&start, at, None);
        let delta = Frame::from_value(json!({"type":"stream_event","event":{
            "type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{
                "output_tokens":5,"server_tool_use":{"web_search_requests":2}
            }
        }}));
        let _ = requests.observe(&delta, at, None);
        let _ = requests.observe(&delta, at, None);
        let stop =
            Frame::from_value(json!({"type":"stream_event","event":{"type":"message_stop"}}));
        let ended = requests.observe(&stop, at, None);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].web_searches, Some(2));
        assert_eq!(ended[0].measured().web_searches.value(), Some(&2));
    }
}
