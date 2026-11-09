//! Recorded init frames report the actual process model on the correct visible turn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]
use super::*;
use orch_types::{ManualClock, new_id};
fn new_mapper() -> EventMapper {
    EventMapper::new(
        MapperSettings {
            directory: "/work".into(),
            cwd: "/work".into(),
            raw_events: false,
        },
        Arc::new(ManualClock::new("2026-11-07T00:00:00Z".parse().unwrap())),
    )
}
fn recorded_init() -> Frame {
    let record: Json = serde_json::from_str(
        include_str!("../../tests/fixtures/performance/api-key.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    Frame::from_value(record["event"].clone())
}
fn notice(events: &[SessionEvent]) -> &ItemStarted {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemStarted(item) => Some(item),
            _ => None,
        })
        .expect("the current turn has a visible notice")
}
#[test]
fn a_recorded_init_before_the_turn_reports_both_models_once_on_that_turn() {
    let mut mapper = new_mapper();
    mapper.process_started(false);
    assert!(mapper.on_frame(&recorded_init()).is_empty());
    let turn = new_id();
    let events = mapper.begin_turn_with_model(turn, "requested-model", None);
    let started = notice(&events);
    assert_eq!(started.turn_id, turn);
    let ItemContent::Error(error) = &started.content else {
        panic!("reroute is the existing error item");
    };
    assert_eq!(
        error.code,
        orch_agent_contract::events::turn_error::MODEL_REROUTED
    );
    assert!(!error.retryable);
    assert!(
        error
            .message
            .contains("anthropic/claude-haiku-4.5 instead of requested-model")
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event,SessionEvent::ItemCompleted(item)
        if item.status==ItemStatus::Completed && item.payload["details"]["turn_id"]==json!(turn)
        && item.payload["details"]["from_model"]=="requested-model"
        && item.payload["details"]["to_model"]=="anthropic/claude-haiku-4.5"
        && item.payload["details"]["next_turn"]==false))
    );
    assert!(
        mapper.on_frame(&recorded_init()).is_empty(),
        "a repeated native init does not duplicate the notice"
    );
    let ended = mapper.on_frame(&Frame::from_value(
        json!({"type":"result","subtype":"success","is_error":false}),
    ));
    assert!(ended.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(turn) if turn.status==TurnOutcome::Completed && turn.error.is_none())));
}
#[test]
fn an_init_after_turn_start_uses_the_same_request_and_does_not_replace_missing_facts() {
    let mut mapper = new_mapper();
    let turn = new_id();
    assert!(
        mapper
            .begin_turn_with_model(turn, "requested-model", None)
            .is_empty()
    );
    let events = mapper.on_frame(&recorded_init());
    assert_eq!(notice(&events).turn_id, turn);
    let mut absent = new_mapper();
    assert!(
        absent
            .begin_turn_with_model(new_id(), "requested-model", None)
            .is_empty()
    );
    for model in [Json::Null, json!(""), json!(42)] {
        assert!(
            absent
                .on_frame(&Frame::from_value(
                    json!({"type":"system","subtype":"init","model":model})
                ))
                .is_empty()
        );
    }
}
#[test]
fn a_requested_model_or_its_exact_transport_context_suffix_is_not_a_reroute() {
    for (reported, context) in [
        ("requested-model", None),
        ("requested-model[1m]", Some(1_000_000)),
    ] {
        let mut mapper = new_mapper();
        assert!(
            mapper
                .begin_turn_with_model(new_id(), "requested-model", context)
                .is_empty()
        );
        assert!(
            mapper
                .on_frame(&Frame::from_value(
                    json!({"type":"system","subtype":"init","model":reported})
                ))
                .is_empty()
        );
    }
    let mut mapper = new_mapper();
    assert!(
        mapper
            .begin_turn_with_model(new_id(), "requested-model", Some(1_000_000))
            .is_empty()
    );
    assert!(
        !mapper
            .on_frame(&Frame::from_value(
                json!({"type":"system","subtype":"init","model":"another-model[1m]"})
            ))
            .is_empty()
    );
}
#[test]
fn the_report_follows_a_live_process_across_turns_and_mapper_refresh_but_not_a_new_process() {
    let mut original = new_mapper();
    assert!(original.on_frame(&recorded_init()).is_empty());
    let first = new_id();
    assert_eq!(
        notice(&original.begin_turn_with_model(first, "requested-model", None)).turn_id,
        first
    );
    let mut refreshed = new_mapper();
    refreshed.inherit_process_report(&original);
    let next = new_id();
    assert_eq!(
        notice(&refreshed.begin_turn_with_model(next, "requested-model", None)).turn_id,
        next
    );
    refreshed.process_started(false);
    assert!(
        refreshed
            .begin_turn_with_model(new_id(), "requested-model", None)
            .is_empty(),
        "a new process must report its own model"
    );
}
#[test]
fn a_subagents_init_never_changes_the_main_process_model() {
    let mut mapper = new_mapper();
    assert!(
        mapper
            .begin_turn_with_model(new_id(), "requested-model", None)
            .is_empty()
    );
    assert!(
        mapper
            .on_frame(&Frame::from_value(
                json!({"type":"system","subtype":"init","model":"requested-model"})
            ))
            .is_empty()
    );
    for parent in [json!("agent-tool"), json!(17)] {
        assert!(mapper.on_frame(&Frame::from_value(json!({"type":"system","subtype":"init","model":"subagent-model","parent_tool_use_id":parent}))).is_empty());
    }
    assert_eq!(mapper.running_model.as_deref(), Some("requested-model"));
}
