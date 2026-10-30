//! What the real Codex app-server sent when a turn failed, replayed through the mapper: one fixture and one test for each kind of failure.
//!
//! Each file of `tests/fixtures/failures` is the notifications the app-server sent for a turn against a model
//! server that failed on purpose, as they came, with the fault the server was told to give and what the
//! adapter has to make of it. They are recorded by `tests/failures.rs` against the real harness in its image
//! (the canary of a new release runs that again and shows the rows that changed); these tests need no
//! container. As the runtime does, the replay keeps the windows of the last `account/rateLimits/updated` and
//! gives them to the mapper.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeSet;

use chrono::{TimeZone, Utc};
use orch_adapter_codex::events::{Context, Mapper, MapperConfig, OutputStore};
use orch_adapter_codex::failure::Windows;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_types::{ManualClock, new_id};
use serde_json::Value;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/failures");

/// Replays the notifications of a fixture as one turn, and checks what the fixture says the adapter has to
/// make of it.
fn row(name: &str) {
    let path = format!("{FIXTURES}/{name}.json");
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let frames = fixture["frames"].as_array().unwrap();
    let thread = frames
        .iter()
        .find_map(|frame| frame["params"]["threadId"].as_str())
        .expect("the turn has a thread")
        .to_owned();
    let clock = ManualClock::new(Utc.with_ymd_and_hms(2026, 10, 12, 10, 0, 0).unwrap());
    let directory = tempfile::tempdir().unwrap();
    let outputs = OutputStore::new(directory.path().join("outputs"));
    let mut mapper = Mapper::new(MapperConfig {
        thread_id: new_id(),
        segment_id: new_id(),
        native_thread: thread,
        directory: "/workspace".into(),
    });
    mapper.expect_turn(new_id());
    let mut windows: Option<Windows> = None;
    let mut events = Vec::new();
    let mut retries = 0;
    for frame in frames {
        let method = frame["method"].as_str().unwrap_or_default();
        let params = &frame["params"];
        if method == "account/rateLimits/updated" {
            windows = Some(Windows::read(&params["rateLimits"]));
        }
        if method == "error" && params["willRetry"] == true {
            retries += 1;
        }
        let ctx = Context {
            clock: &clock,
            outputs: &outputs,
            windows: windows.as_ref(),
        };
        events.extend(mapper.handle(method, params, &ctx));
    }
    let expect = &fixture["expect"];
    let ended = events.iter().find_map(|event| match event {
        SessionEvent::TurnCompleted(ended) => Some(ended),
        _ => None,
    });
    match expect["outcome"].as_str().unwrap() {
        "completed" => {
            let ended = ended.expect("the turn ends");
            assert_eq!(ended.status, TurnOutcome::Completed, "{name}");
            assert!(ended.error.is_none(), "{name}");
            return;
        }
        "none" => {
            assert!(
                ended.is_none(),
                "{name}: the harness has not ended the turn"
            );
            assert!(
                retries >= expect["retries_at_least"].as_u64().unwrap(),
                "{name}: {retries} retries"
            );
            return;
        }
        _ => {}
    }
    let ended = ended.unwrap_or_else(|| panic!("{name}: the turn ends"));
    assert_eq!(ended.status, TurnOutcome::Failed, "{name}");
    let error = ended.error.as_ref().expect("a failed turn says why");
    assert_eq!(
        error.code,
        expect["code"].as_str().unwrap(),
        "{name}: {error:?}"
    );
    assert_eq!(
        error.retryable,
        expect["retryable"].as_bool().unwrap(),
        "{name}"
    );
    assert_eq!(
        error.incomplete,
        expect["incomplete"].as_bool().unwrap(),
        "{name}"
    );
    assert_eq!(
        error.resets_at.map(|time| time.to_string()),
        expect["resets_at"].as_str().map(str::to_owned),
        "{name}"
    );
    assert_eq!(mapper.last_failure_row(), expect["row"].as_str(), "{name}");
    if let Some(reason) = expect["reason"].as_str() {
        assert_eq!(error.reason.as_deref(), Some(reason), "{name}");
    }
    let said = error.said.as_deref().unwrap_or_default();
    if let Some(exactly) = expect["said_exactly"].as_str() {
        assert_eq!(said, exactly, "{name}");
    }
    for member in ["said_contains", "said_also"] {
        if let Some(part) = expect[member].as_str() {
            assert!(said.contains(part), "{name}: {part:?} is not in {said:?}");
        }
    }
    assert!(
        !error.message.is_empty(),
        "{name}: the adapter says it in its own words"
    );
    // What the failed attempts started is not left open.
    let started: BTreeSet<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(item) => Some(item.native_ref.clone()),
            _ => None,
        })
        .collect();
    let completed: BTreeSet<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item) => Some(item.native_ref.clone()),
            _ => None,
        })
        .collect();
    assert!(
        started.is_subset(&completed),
        "{name}: left open: {:?}",
        started.difference(&completed)
    );
}

#[test]
fn a_session_limit_ends_at_the_moment_of_the_window_that_is_full() {
    row("plan_limit_session");
}

#[test]
fn a_weekly_limit_ends_at_the_moment_of_its_window_whatever_the_zone_of_the_sentence() {
    row("plan_limit_weekly_dated");
}

#[test]
fn a_limit_that_names_no_time_is_exhausted_with_no_time() {
    row("plan_limit_without_a_time");
}

#[test]
fn a_limit_worded_for_a_team_is_the_same_limit() {
    row("plan_limit_of_a_team");
}

#[test]
fn a_quota_that_ran_out_is_exhausted_without_rejecting_the_credential() {
    row("quota_ran_out");
}

#[test]
fn a_rate_limit_the_harness_made_again_is_busy_and_says_how_often() {
    row("rate_limited_and_made_again");
}

#[test]
fn a_rate_limit_without_a_hint_to_wait_is_busy_at_once() {
    row("rate_limited_and_not_made_again");
}

#[test]
fn a_model_at_capacity_is_busy_and_says_how_often_the_harness_tried() {
    row("model_at_capacity_503");
}

#[test]
fn an_overloaded_vendor_is_busy() {
    row("overloaded_529");
}

#[test]
fn a_vendor_that_fails_is_busy() {
    row("server_error_500");
}

#[test]
fn a_bad_gateway_is_busy() {
    row("bad_gateway_502");
}

#[test]
fn a_gateway_that_timed_out_is_unreachable() {
    row("gateway_timeout_504");
}

#[test]
fn the_page_of_a_gateway_in_the_way_is_busy_and_kept_in_words() {
    row("page_of_a_gateway_502");
}

#[test]
fn a_key_the_vendor_refused_is_unauthorized_though_the_harness_does_not_say_so() {
    row("key_refused_401");
}

#[test]
fn a_request_refusal_does_not_reject_the_provider_credential() {
    row("forbidden_403");
}

#[test]
fn a_model_the_vendor_does_not_know_is_unavailable() {
    row("unknown_model_404");
}

#[test]
fn a_prompt_that_is_too_long_is_read_from_the_code_in_the_vendors_body() {
    row("prompt_too_long_400");
}

#[test]
fn a_request_that_is_too_large_overflows_the_context() {
    row("request_too_large_413");
}

#[test]
fn a_refusal_for_no_known_reason_is_a_harness_error_with_the_vendors_words() {
    row("invalid_request_400");
}

#[test]
fn a_context_window_that_ran_out_in_the_stream_overflows_the_context() {
    row("context_window_failed_in_the_stream");
}

#[test]
fn a_quota_that_ran_out_in_the_stream_is_exhausted() {
    row("quota_failed_in_the_stream");
}

#[test]
fn a_rate_limit_that_failed_the_response_is_busy() {
    row("rate_limit_failed_in_the_stream");
}

#[test]
fn a_server_error_that_failed_the_response_leaves_the_stream_broken() {
    row("server_error_failed_in_the_stream");
}

#[test]
fn a_stream_cut_in_the_middle_of_a_sentence_is_out_of_reach_and_incomplete() {
    row("stream_cut_in_the_middle_of_a_sentence");
}

#[test]
fn a_stream_cut_in_the_middle_of_a_tool_call_is_out_of_reach_and_incomplete() {
    row("stream_cut_in_the_middle_of_a_tool_call");
}

#[test]
fn a_stream_that_ends_without_its_closing_event_is_out_of_reach_and_incomplete() {
    row("stream_ended_without_its_closing_event");
}

#[test]
fn an_error_event_inside_a_stream_that_the_harness_does_not_read_is_a_broken_stream() {
    row("error_event_inside_a_stream");
}

#[test]
fn an_answer_that_is_no_event_stream_leaves_the_vendor_out_of_reach() {
    row("answer_that_is_no_event_stream");
}

#[test]
fn an_answer_cut_by_the_models_output_limit_is_incomplete_and_no_outage() {
    row("answer_cut_by_the_output_limit");
}

#[test]
fn a_stream_that_goes_silent_is_out_of_reach_and_incomplete() {
    row("stream_silent_after_it_began");
}

#[test]
fn a_refused_connection_makes_the_harness_wait_for_the_network_without_end() {
    row("connection_refused");
}

#[test]
fn a_certificate_nobody_trusts_makes_the_harness_wait_for_the_network_without_end() {
    row("certificate_nobody_trusts");
}

#[test]
fn an_event_nobody_knows_changes_nothing_in_the_turn() {
    row("event_nobody_knows_is_ignored");
}

#[test]
fn a_line_that_is_no_json_changes_nothing_in_the_turn() {
    row("line_that_is_no_json_is_ignored");
}

#[test]
fn a_throttling_that_passes_is_no_failure() {
    row("throttled_once_and_then_answered");
}

#[test]
fn synthetic_refusals_and_ungranted_access_use_the_recorded_protocol_shapes() {
    for name in ["filter_cyber", "filter_advice", "programme_access"] {
        row(name);
    }
}
