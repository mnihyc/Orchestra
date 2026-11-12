//! Recorded Claude requests prove message boundaries, hidden thinking and untimed sub-agents.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_adapter_claude_code::{performance::Requests, stream::Frame};
use orch_agent_contract::events::{UsageMeasurement, UsageRequest, request_timing};
use orch_types::{Json, Time};
use serde_json::json;

#[test]
fn published_recordings_have_no_account_events_signatures_or_native_id_keys() {
    fn clean(value: &Json) {
        match value {
            Json::Object(fields) => {
                for (key, value) in fields {
                    assert_ne!(key, "signature");
                    assert!(!key.starts_with("toolu_"));
                    assert!(!key.starts_with("msg_"));
                    clean(value);
                }
            }
            Json::Array(values) => {
                for value in values {
                    clean(value);
                }
            }
            Json::String(text) => {
                assert!(!text.contains("toolu_"));
                assert!(!text.contains("msg_"));
            }
            _ => {}
        }
    }
    for source in [
        include_str!("fixtures/performance/tool-call.jsonl"),
        include_str!("fixtures/performance/long-answer.jsonl"),
        include_str!("fixtures/performance/hidden-reasoning.jsonl"),
        include_str!("fixtures/performance/api-key.jsonl"),
    ] {
        for line in source.lines() {
            let row: Json = serde_json::from_str(line).unwrap();
            let event = &row["event"];
            assert_ne!(event["type"], "rate_limit_event");
            assert!(
                !event["method"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("account/")
            );
            clean(&row);
        }
    }
}

fn at(ms: i64) -> Time {
    let base: Time = "2026-10-15T00:00:00Z".parse().unwrap();
    Time::new(base.get() + chrono::Duration::milliseconds(ms))
}

fn replay(stream: &str) -> Vec<UsageRequest> {
    let mut observer = Requests::default();
    let mut complete = Vec::new();
    for line in stream.lines() {
        let row: Json = serde_json::from_str(line).unwrap();
        let Some(event) = row.get("event") else {
            continue;
        };
        let time = at(row["at_ms"].as_i64().unwrap());
        // The process-launch capture marks its ready input at system/init.
        if event["type"] == "system" && event["subtype"] == "init" {
            observer.asked(time);
        }
        let frame = Frame::from_value(event.clone());
        complete.extend(
            observer
                .observe(&frame, time, Some(("done", None)))
                .into_iter()
                .filter(|request| request.outcome.is_some()),
        );
    }
    complete
}

fn check(stream: &str, expected: &str) {
    let expected: Json = serde_json::from_str(expected).unwrap();
    let requests = replay(stream);
    let rows = expected["requests"].as_array().unwrap();
    assert_eq!(requests.len(), rows.len());
    let mut whole = 0;
    for (raw, row) in requests.iter().zip(rows) {
        let measured = request_timing(raw);
        for (name, value) in [
            ("first_token_ms", &measured.first_token_ms),
            ("generation_ms", &measured.generation_ms),
            ("request_ms", &measured.request_ms),
            ("streamed_tokens", &measured.streamed_tokens),
            ("speed_milli", &measured.speed_milli),
        ] {
            assert_eq!(
                value.value().copied(),
                row[name].as_i64(),
                "{name}: {}",
                raw.id
            );
        }
        whole += measured.request_ms.value().copied().unwrap_or_default();
        assert_eq!(raw.tokens.output_tokens, row["output_tokens"].as_i64());
        assert_eq!(
            raw.tokens.reasoning_tokens,
            row["reasoning_tokens"].as_i64()
        );
        assert_eq!(
            measured.reasoning_hidden,
            row["reasoning_hidden"].as_bool().unwrap()
        );
    }
    let api = expected["duration_api_ms"].as_i64().unwrap();
    // Capture dispatch and the native API stopwatch differ by transport overhead,
    // bounded by the observed ten-percent agreement rather than an arbitrary delay.
    assert!((whole - api).abs() * 100 <= api * 10);
}

#[test]
fn recorded_tool_long_hidden_and_api_key_requests_have_the_exact_observed_figures() {
    for (stream, expected) in [
        (
            include_str!("fixtures/performance/tool-call.jsonl"),
            include_str!("fixtures/performance/tool-call.expected.json"),
        ),
        (
            include_str!("fixtures/performance/long-answer.jsonl"),
            include_str!("fixtures/performance/long-answer.expected.json"),
        ),
        (
            include_str!("fixtures/performance/hidden-reasoning.jsonl"),
            include_str!("fixtures/performance/hidden-reasoning.expected.json"),
        ),
        (
            include_str!("fixtures/performance/api-key.jsonl"),
            include_str!("fixtures/performance/api-key.expected.json"),
        ),
    ] {
        check(stream, expected);
    }
}

#[test]
fn complete_subagent_messages_and_streams_without_boundaries_have_no_fabricated_times() {
    let mut observer = Requests::default();
    let sub = Frame::from_value(
        json!({"type":"assistant","parent_tool_use_id":"tool-1","message":{"id":"message-1","model":"model-a","usage":{"input_tokens":10,"output_tokens":100}}}),
    );
    // A complete message is not the closing usage of its request: it is handed over to be settled from the
    // sub-agent's transcript, and nothing is reported from here.
    assert!(observer.observe(&sub, at(1000), None).is_empty());
    let printed = observer.take_printed();
    assert_eq!(printed.len(), 1);
    assert!(observer.take_printed().is_empty());
    let raw = [printed[0].request.clone()];
    assert!(raw[0].subagent);
    let measured = request_timing(&raw[0]);
    assert_eq!(measured.first_token_ms, UsageMeasurement::Unsupported);
    assert_eq!(measured.generation_ms, UsageMeasurement::Unsupported);
    assert_eq!(measured.speed_milli, UsageMeasurement::Unsupported);
    let mut observer = Requests::default();
    let raw = observer.observe(
        &Frame::from_value(
            json!({"type":"result","subtype":"success","usage":{"output_tokens":100}}),
        ),
        at(2000),
        None,
    );
    assert_eq!(
        request_timing(&raw[0]).speed_milli,
        UsageMeasurement::Unsupported
    );
}

#[test]
fn asked_and_first_output_keep_one_identity_and_a_result_cannot_invent_message_stop() {
    let mut observer = Requests::default();
    let asked = observer.asked(at(0));
    let first=observer.observe(&Frame::from_value(json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"native","model":"model-a","usage":{"input_tokens":10}}}})),at(1000),None);
    assert_eq!(first[0].id, asked.id);
    first[0].check().unwrap();
    let ended = observer.observe(
        &Frame::from_value(json!({"type":"result","subtype":"error"})),
        at(2000),
        Some(("provider_busy", Some("Try again later"))),
    );
    assert_eq!(ended.len(), 1);
    ended[0].check().unwrap();
    assert_eq!(ended[0].tokens.output_tokens, None);
    assert_eq!(ended[0].end, None);
    assert_eq!(
        request_timing(&ended[0]).generation_ms,
        UsageMeasurement::Unsupported
    );
    observer.asked(at(3000));
    let lost = observer.abort("session_lost", Some("The harness exited"), None);
    assert_eq!(lost.len(), 1);
    lost[0].check().unwrap();
    assert_eq!(
        request_timing(&lost[0]).outcome.value().map(String::as_str),
        Some("session_lost")
    );
}

#[test]
fn parallel_tool_results_share_one_asked_and_a_replaced_message_gets_a_terminal_outcome() {
    let mut observer = Requests::default();
    let tool = |id| {
        Frame::from_value(
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"content":"done"}]}}),
        )
    };
    let first = observer.observe(&tool("tool-a"), at(1000), None);
    assert_eq!(first.len(), 1);
    assert!(observer.observe(&tool("tool-b"), at(1100), None).is_empty());
    let start = |id| {
        Frame::from_value(
            json!({"type":"stream_event","event":{"type":"message_start","message":{"id":id,"model":"model-a"}}}),
        )
    };
    let begun = observer.observe(&start("one"), at(2000), None);
    assert_eq!(begun[0].id, first[0].id);
    assert_eq!(begun[0].asked, Some(at(1000)));
    let replaced = observer.observe(&start("two"), at(3000), None);
    assert_eq!(replaced.len(), 2);
    assert_eq!(replaced[0].id, first[0].id);
    assert!(replaced[0].outcome.is_some());
    assert!(replaced[1].outcome.is_none());
}

#[test]
fn a_retry_counts_rate_limiting_and_a_broken_stop_never_counts_placeholder_output() {
    let requests = replay(include_str!("fixtures/performance/retry.jsonl"));
    assert_eq!(requests.len(), 2);
    assert_eq!(
        request_timing(&requests[0])
            .outcome
            .value()
            .map(String::as_str),
        Some("rate_limited")
    );
    assert_eq!(requests[0].tokens.output_tokens, None);
    let retried = request_timing(&requests[1]);
    assert_eq!(retried.first_token_ms.value(), Some(&500));
    assert_eq!(retried.speed_milli.value(), Some(&58_182));
    let mut observer = Requests::default();
    observer.asked(at(0));
    observer.observe(&Frame::from_value(json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"ref-0001","usage":{"output_tokens":16}}}})),at(100),None);
    let ended = observer.observe(
        &Frame::from_value(json!({"type":"stream_event","event":{"type":"message_stop"}})),
        at(1000),
        None,
    );
    assert_eq!(ended[0].tokens.output_tokens, None);
    assert_eq!(
        request_timing(&ended[0])
            .outcome
            .value()
            .map(String::as_str),
        Some("stream_interrupted")
    );
    assert!(request_timing(&ended[0]).speed_milli.value().is_none());
}

#[test]
fn message_tool_references_and_upstream_identity_survive_the_terminal_report() {
    let mut requests = Requests::default();
    requests.asked(at(0));
    for (offset, event) in [
        (
            1,
            json!({"type":"message_start","message":{"id":"message-local","model":"model","usage":{"input_tokens":10,"output_tokens":0}}}),
        ),
        (
            2,
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-local","name":"Read"}}),
        ),
        (
            3,
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
        ),
    ] {
        requests.observe(
            &Frame::from_value(json!({"type":"stream_event","event":event})),
            at(offset),
            None,
        );
    }
    let completed = requests.observe(
        &Frame::from_value(json!({"type":"stream_event","event":{"type":"message_stop"}})),
        at(4),
        None,
    );
    assert_eq!(completed.len(), 1);
    assert_eq!(
        completed[0].provider_request_id.as_deref(),
        Some("message-local")
    );
    assert_eq!(completed[0].tool_refs, Some(vec!["tool-local".to_owned()]));
}

#[test]
fn native_retry_numbers_survive_missing_and_replayed_notifications() {
    let mut observer = Requests::default();
    let first = observer.asked(at(0));
    assert_eq!(first.attempt, Some(1));
    let frame = Frame::from_value(
        json!({"type":"system","subtype":"api_retry","attempt":4,"error_status":529,"error":"overloaded"}),
    );
    let reports = observer.observe(&frame, at(1000), None);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].id, first.id);
    assert_eq!(reports[1].retry_of.as_deref(), Some(first.id.as_str()));
    assert_eq!(
        reports[1].attempt,
        Some(5),
        "model-call numbers include the initial call, unlike native retry numbers"
    );
    assert!(
        observer.observe(&frame, at(1200), None).is_empty(),
        "duplicate retry notifications cannot create attempts or reset timing"
    );
    let mut child = frame.raw().clone();
    child["parent_tool_use_id"] = json!("child");
    assert!(
        observer
            .observe(&Frame::from_value(child), at(1400), None)
            .is_empty()
    );
    let main=observer.observe(&Frame::from_value(json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"main-message","model":"model-a"}}})),at(1500),None);
    assert_eq!(main[0].id, reports[1].id);
    assert_eq!(main[0].asked, Some(at(1000)));
    assert_eq!(main[0].attempt, Some(5));
}
