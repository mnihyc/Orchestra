//! Recorded Codex requests prove exact boundaries, late-count matching and provider-independent figures.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_adapter_codex::performance::Requests;
use orch_agent_contract::events::{UsageMeasurement, UsageRequest, request_timing};
use orch_types::{Json, Time};
use serde_json::json;

fn at(ms: i64) -> Time {
    let base: Time = "2026-10-15T00:00:00Z".parse().unwrap();
    Time::new(base.get() + chrono::Duration::milliseconds(ms))
}

fn replay(stream: &str) -> Vec<UsageRequest> {
    let rows: Vec<Json> = stream
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let thread = rows
        .iter()
        .find_map(|row| row.pointer("/event/params/threadId").and_then(Json::as_str))
        .unwrap();
    let mut observer = Requests::default();
    let mut complete = Vec::new();
    for row in &rows {
        let time = at(row["at_ms"].as_i64().unwrap());
        if row["sent"] == "turn/start" {
            observer.asked(thread, time);
            continue;
        }
        let Some(event) = row.get("event") else {
            continue;
        };
        let Some(method) = event["method"].as_str() else {
            continue;
        };
        let params = event.get("params").unwrap_or(&Json::Null);
        complete.extend(
            observer
                .observe(method, params, time, Some(("done", None)))
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
        assert_eq!(raw.tokens.output_tokens, row["output_tokens"].as_i64());
        assert_eq!(
            measured.reasoning_hidden,
            row["reasoning_hidden"].as_bool().unwrap()
        );
    }
}

#[test]
fn recorded_requests_match_the_pinned_tool_long_hidden_and_api_key_cases_to_the_millisecond() {
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
fn tokens_without_proven_output_boundaries_never_invent_a_latency_or_speed() {
    let mut observer = Requests::default();
    let raw=observer.observe("thread/tokenUsage/updated",&json!({"threadId":"unknown","tokenUsage":{"last":{"inputTokens":2,"outputTokens":100,"totalTokens":102}}}),at(1000),None);
    let measured = request_timing(&raw[0]);
    assert_eq!(measured.first_token_ms, UsageMeasurement::Unsupported);
    assert_eq!(measured.speed_milli, UsageMeasurement::Unsupported);
}

#[test]
fn late_counts_wait_for_the_turn_outcome_and_a_process_loss_closes_running_requests() {
    let mut observer = Requests::default();
    observer.asked("main", at(0));
    observer.observe(
        "item/agentMessage/delta",
        &json!({"threadId":"main","delta":"answer"}),
        at(1000),
        None,
    );
    let counts=observer.observe("thread/tokenUsage/updated",&json!({"threadId":"main","tokenUsage":{"last":{"inputTokens":10,"outputTokens":64,"totalTokens":74}}}),at(2000),None);
    assert!(
        counts.is_empty(),
        "counts do not prove that the request succeeded"
    );
    let end = observer.observe(
        "turn/completed",
        &json!({"threadId":"main"}),
        at(2100),
        Some(("provider_busy", Some("Try again later"))),
    );
    assert_eq!(end.len(), 1);
    end[0].check().unwrap();
    assert_eq!(
        request_timing(&end[0]).outcome.value().map(String::as_str),
        Some("provider_busy")
    );
    assert_eq!(
        end[0].end,
        Some(at(1000)),
        "the result boundary is the last output, not turn completion"
    );
    observer.asked("main", at(3000));
    let lost = observer.abort("main", "session_lost", Some("The harness exited"));
    assert_eq!(lost.len(), 1);
    lost[0].check().unwrap();
    assert_eq!(lost[0].tokens.output_tokens, None);
    assert_eq!(
        request_timing(&lost[0]).speed_milli,
        UsageMeasurement::Unsupported
    );
}

#[test]
fn a_parent_turn_closes_its_child_request_observations() {
    let mut observer = Requests::default();
    observer.asked("main", at(0));
    observer.asked("child", at(100));
    let ended = observer.abort_all("session_lost", Some("The session exited"));
    assert_eq!(ended.len(), 2);
    assert!(ended.iter().all(|request| request.outcome.is_some()));
    assert_eq!(ended.iter().filter(|request| request.subagent).count(), 1);
    assert!(observer.abort_all("session_lost", None).is_empty());
}

#[test]
fn a_retry_counts_the_failed_attempt_and_restarts_first_output_and_generation() {
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
    assert_eq!(requests[1].tokens.cache_write_tokens, Some(0));
    let retried = request_timing(&requests[1]);
    assert_eq!(retried.first_token_ms.value(), Some(&500));
    assert_eq!(retried.generation_ms.value(), Some(&1000));
    assert_eq!(retried.speed_milli.value(), Some(&64_000));
}

#[test]
fn provider_side_web_search_remains_inside_one_response_and_its_one_count_report() {
    let requests = replay(include_str!("fixtures/performance/web-search.jsonl"));
    assert_eq!(requests.len(), 1);
    let measured = request_timing(&requests[0]);
    assert_eq!(measured.first_token_ms.value(), Some(&500));
    assert_eq!(measured.generation_ms.value(), Some(&3000));
    assert_eq!(measured.request_ms.value(), Some(&3500));
    assert_eq!(measured.speed_milli.value(), Some(&21_333));
}

#[test]
fn parallel_tool_item_references_stay_with_their_actual_model_request() {
    let mut requests = Requests::default();
    requests.asked("main", at(0));
    let first = requests.running("main").unwrap();
    for (offset, id) in [(100, "call-a"), (110, "call-b")] {
        let reports = requests.observe(
            "item/started",
            &json!({"threadId":"main","item":{"type":"commandExecution","id":id}}),
            at(offset),
            None,
        );
        assert!(reports.iter().all(|request| request.id == first.id));
    }
    let completed=requests.observe("thread/tokenUsage/updated",&json!({"threadId":"main","tokenUsage":{"last":{"inputTokens":10,"outputTokens":2,"totalTokens":12}}}),at(120),None);
    assert_eq!(completed.len(), 1);
    assert_eq!(
        completed[0].tool_refs.as_deref(),
        Some(["call-a".to_owned(), "call-b".to_owned()].as_slice())
    );
    for id in ["call-a", "call-b"] {
        requests.observe(
            "item/completed",
            &json!({"threadId":"main","item":{"type":"commandExecution","id":id}}),
            at(300),
            None,
        );
    }
    let next = requests.running("main").unwrap();
    assert_ne!(next.id, first.id);
    assert_eq!(next.tool_refs, Some(Vec::new()));
}

#[test]
fn native_retry_names_its_predecessor_instead_of_resetting_a_recovery_ordinal() {
    let mut requests = Requests::default();
    requests.asked("main", at(0));
    let original = requests.running("main").unwrap();
    let reports=requests.observe("error",&json!({"threadId":"main","willRetry":true,"error":{"message":"rate limit exceeded","codexErrorInfo":"rateLimitExceeded"}}),at(50),None);
    let next = requests.running("main").unwrap();
    let failed = reports
        .iter()
        .find(|request| {
            matches!(
                request.outcome,
                Some(orch_agent_contract::requests::RequestOutcome::Error { .. })
            )
        })
        .unwrap();
    assert_eq!(failed.previous_ref.as_deref(), Some(original.id.as_str()));
    assert_eq!(next.retry_of.as_deref(), Some(failed.id.as_str()));
    assert_eq!(next.attempt, Some(2));
}

#[test]
fn anonymous_failure_events_have_session_turn_sequence_identity_without_inferred_boundaries() {
    let mut requests = Requests::default();
    let event = json!({"threadId":"native-session","turnId":"native-turn","willRetry":false,"error":{"message":"The request was rate limited","codexErrorInfo":"rateLimitExceeded"}});
    let first = requests.observe("error", &event, at(100), None);
    let second = requests.observe("error", &event, at(100), None);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].id, "native-session:native-turn:failure:1");
    assert_eq!(second[0].id, "native-session:native-turn:failure:2");
    for request in first.iter().chain(second.iter()) {
        assert!(request.provider_request_id.is_none());
        assert!(request.asked.is_none());
        assert!(request.first_output.is_none());
        assert!(request.end.is_none());
        assert!(request.tokens.input_tokens.is_none());
        assert!(request.tokens.output_tokens.is_none());
        request.check().unwrap();
    }
}
