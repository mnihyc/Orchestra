//! Source controls, distinct from the candidate's separately recorded native settings proof.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use super::*;
use orch_types::{ManualClock, new_id};
fn new_mapper() -> EventMapper {
    let mut mapper = EventMapper::new(
        MapperSettings {
            directory: "/work".into(),
            cwd: "/work".into(),
            raw_events: false,
        },
        Arc::new(ManualClock::new("2026-11-07T00:00:00Z".parse().unwrap())),
    );
    mapper.expect_managed_pins(BTreeMap::from([
        ("autoContinueAtUsageLimit".into(), json!(false)),
        ("idleCompaction".into(), json!(false)),
        ("permissions.denyInsteadOfPrompting".into(), json!(false)),
    ]));
    mapper
}
fn frame() -> Frame {
    Frame::from_value(
        json!({"type":"control_response","response":{"subtype":"success","request_id":"read-pins"}}),
    )
}
fn markers(events: &[SessionEvent]) -> Vec<&Json> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.payload["marker"] == "harness_changed" => {
                Some(&item.payload["data"])
            }
            _ => None,
        })
        .collect()
}
#[test]
fn server_policy_override_warns_with_the_pin_name_and_no_private_settings() {
    let mut mapper = new_mapper();
    let turn = new_id();
    mapper.begin_turn(turn);
    let reply = ControlOutcome::Success(
        json!({"effective":{"autoContinueAtUsageLimit":true,"idleCompaction":false,
        "permissions":{"denyInsteadOfPrompting":true,"deny":["PRIVATE-RULE"]},"env":{"TOKEN":"PRIVATE-TOKEN"}},
        "sources":[{"settings":{"instructions":"PRIVATE-INSTRUCTION"}}]}),
    );
    let events = mapper.native_settings(&frame(), &reply, Some(turn));
    let changed: Vec<_> = markers(&events)
        .into_iter()
        .filter(|data| data["change"] == "managed_setting")
        .collect();
    assert_eq!(changed.len(), 2);
    for data in changed {
        assert_eq!(data["expected"]["value"], false);
        assert_eq!(data["diverged"], true);
        assert_eq!(data["turn_id"], json!(turn));
    }
    assert_eq!(events.iter().filter(|event|matches!(event,SessionEvent::ItemCompleted(item) if item.payload["code"]=="harness_diverged" && item.status==ItemStatus::Completed)).count(),2);
    assert!(!serde_json::to_string(&events).unwrap().contains("PRIVATE-"));
    assert!(
        mapper.turn_active(),
        "a native settings observation never ends the person's turn"
    );
    assert!(
        mapper
            .native_settings(&frame(), &reply, Some(turn))
            .is_empty(),
        "the same unchanged policy produces no second warning"
    );
}
#[test]
fn an_idle_read_back_does_not_borrow_a_turn_started_while_the_request_was_in_flight() {
    let mut mapper = new_mapper();
    mapper.begin_turn(new_id());
    let events = mapper.native_settings(
        &frame(),
        &ControlOutcome::Success(json!({"effective":{"autoContinueAtUsageLimit":true}})),
        None,
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ItemStarted(_)))
    );
    assert!(
        markers(&events)
            .iter()
            .all(|data| data["turn_id"].is_null())
    );
    assert!(events.iter().any(|event|matches!(event,SessionEvent::ItemCompleted(item) if item.payload["code"]=="harness_diverged" && item.payload["details"]["turn_id"].is_null())));
}
#[test]
fn missing_or_invalid_native_values_never_prove_that_a_pin_is_installed() {
    let mut mapper = new_mapper();
    let events = mapper.native_settings(
        &frame(),
        &ControlOutcome::Success(json!({"effective":{"autoContinueAtUsageLimit":"PRIVATE-BODY"}})),
        None,
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("PRIVATE-BODY")
    );
    let data = markers(&events);
    assert_eq!(data[0]["value"], json!({"reported":true,"valid":false}));
    assert_eq!(data.last().unwrap()["value"]["status"], "incomplete");
    let mut mapper = new_mapper();
    let events = mapper.native_settings(
        &frame(),
        &ControlOutcome::Error("PRIVATE-DIAGNOSTIC".into()),
        None,
    );
    assert_eq!(markers(&events)[0]["value"]["reported"], json!([]));
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("PRIVATE-DIAGNOSTIC")
    );
}
#[test]
fn a_restored_pin_preserves_its_previous_value_across_a_live_mapper_refresh() {
    let mut mapper = new_mapper();
    let _ = mapper.native_settings(
        &frame(),
        &ControlOutcome::Success(json!({"effective":{"autoContinueAtUsageLimit":true}})),
        None,
    );
    let mut refreshed = new_mapper();
    refreshed.inherit_process_report(&mapper);
    let events = refreshed.native_settings(
        &frame(),
        &ControlOutcome::Success(json!({"effective":{"autoContinueAtUsageLimit":false}})),
        None,
    );
    let data = markers(&events);
    assert_eq!(data[0]["previous"]["value"], true);
    assert_eq!(data[0]["diverged"], false);
    assert_eq!(data[0]["changed"], true);
}

#[test]
fn a_real_readback_timeout_is_visible_without_inventing_a_native_value_or_turn() {
    let mut mapper = new_mapper();
    mapper.begin_turn(new_id());
    let events = mapper.native_settings_unavailable("request-one", None);
    let data = markers(&events);
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["value"]["status"], "unavailable");
    assert_eq!(data[0]["value"]["reason"], "timeout");
    assert_eq!(data[0]["expected"]["state"], "unsupported");
    assert!(data[0]["turn_id"].is_null());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ItemStarted(_)))
    );
}
