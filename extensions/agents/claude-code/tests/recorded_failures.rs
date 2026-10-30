//! What the real Claude Code wrote when it failed, replayed through the mapper: one fixture and one test for each kind of failure.
//!
//! Each file of `tests/fixtures/failures` is what the harness wrote to its standard output (and error) against
//! a model server that failed on purpose, as it came, with the fault the server was told to give, how the
//! harness was signed in, and what the adapter has to make of it. They are recorded by `tests/failures.rs`
//! against the real harness in its image (the canary of a new release runs that again and shows the rows
//! that changed); these tests need no container.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use orch_adapter_claude_code::events::{EventMapper, MapperSettings};
use orch_adapter_claude_code::stream::Frame;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_types::{ManualClock, new_id};
use serde_json::Value;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/failures");

/// Replays the frames of a fixture as one turn, and checks what the fixture says the adapter has to make of it.
fn row(name: &str) {
    let path = format!("{FIXTURES}/{name}.json");
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let clock = ManualClock::new(Utc.with_ymd_and_hms(2026, 10, 12, 10, 0, 0).unwrap());
    let mut mapper = EventMapper::new(
        MapperSettings {
            directory: "/workspace".into(),
            cwd: "/workspace".into(),
            raw_events: false,
        },
        Arc::new(clock),
    );
    mapper.begin_turn(new_id());
    let mut events = Vec::new();
    for frame in fixture["frames"].as_array().unwrap() {
        events.extend(mapper.on_frame(&Frame::from_value(frame.clone())));
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
    // Nothing the failed attempts started is left open for the person to watch forever.
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
fn a_weekly_limit_of_the_plan_is_exhausted_until_the_moment_the_vendor_names_in_its_zone() {
    row("plan_limit_weekly_dated");
}

#[test]
fn a_session_limit_ends_at_the_moment_the_event_gives_and_not_at_the_one_in_the_sentence() {
    row("plan_limit_session_today");
}

#[test]
fn a_limit_that_names_no_time_is_exhausted_with_no_time() {
    row("plan_limit_without_a_time");
}

#[test]
fn a_limit_worded_in_another_zone_ends_at_the_same_moment() {
    row("plan_limit_weekly_in_new_york");
}

#[test]
fn a_limit_of_one_family_of_models_is_said_as_that() {
    row("plan_limit_of_one_family_of_models");
}

#[test]
fn a_limit_of_spending_is_said_as_that() {
    row("plan_limit_of_spending");
}

#[test]
fn a_throttling_that_is_not_the_plans_is_busy_and_says_how_often_the_harness_tried() {
    row("throttled_though_not_the_plans_limit");
}

#[test]
fn a_rate_limit_on_a_key_is_busy() {
    row("rate_limited_with_a_key");
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
fn a_gateway_that_timed_out_is_unreachable() {
    row("gateway_timeout_504");
}

#[test]
fn the_page_of_a_gateway_in_the_way_is_busy_and_kept_in_words() {
    row("page_of_a_gateway_502");
}

#[test]
fn a_sign_in_the_vendor_refused_is_unauthorized() {
    row("sign_in_refused_401");
}

#[test]
fn a_request_refusal_does_not_reject_the_provider_credential() {
    row("forbidden_403");
}

#[test]
fn a_balance_that_ran_out_is_exhausted_without_rejecting_the_credential() {
    row("balance_ran_out_402");
}

#[test]
fn a_model_the_vendor_does_not_know_is_unavailable() {
    row("unknown_model_404");
}

#[test]
fn a_prompt_that_is_too_long_overflows_the_context() {
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
fn a_first_byte_that_never_comes_leaves_the_vendor_out_of_reach() {
    row("first_byte_never_comes");
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
fn a_stream_that_ends_without_its_closing_events_is_out_of_reach_and_incomplete() {
    row("stream_ended_without_its_closing_events");
}

#[test]
fn an_error_event_inside_a_stream_is_read_by_its_status_and_leaves_the_answer_incomplete() {
    row("error_event_inside_a_stream");
}

#[test]
fn an_answer_that_is_no_json_leaves_the_vendor_out_of_reach() {
    row("answer_that_is_no_json");
}

#[test]
fn a_refused_connection_leaves_the_vendor_out_of_reach_after_the_harness_tried_again() {
    row("connection_refused");
}

#[test]
fn a_certificate_nobody_trusts_leaves_the_vendor_out_of_reach() {
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
fn a_silent_stream_the_harness_resumes_is_no_failure() {
    row("silent_stream_resumed_by_the_harness");
}

#[test]
fn synthetic_refusals_and_ungranted_access_use_the_recorded_protocol_shapes() {
    for name in ["filter_cyber", "filter_advice", "programme_access"] {
        row(name);
    }
}
