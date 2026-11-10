//! Literal protocol frames exercise reported modes, provenance and requested-mode comparisons.
#![allow(
    clippy::unwrap_used,
    reason = "a protocol assertion fails by panicking"
)]
use super::*;
use orch_types::{ManualClock, new_id};

fn mapper() -> EventMapper {
    let mut mapper = EventMapper::new(
        MapperSettings {
            directory: "/work".into(),
            cwd: "/work".into(),
            raw_events: false,
        },
        Arc::new(ManualClock::new("2026-11-07T00:00:00Z".parse().unwrap())),
    );
    mapper.set_native_session("native-session");
    mapper
}
fn report(mode: Json) -> Frame {
    Frame::from_value(json!({"type":"system","subtype":"init","uuid":"init",
        "session_id":"native-session","permissionMode":mode,"config":{"secret":"private-setting"}}))
}
fn modes(events: &[SessionEvent]) -> Vec<(&str, &Json)> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item)
                if item.payload["marker"] == "harness_changed"
                    && item.payload["data"]["change"] == "approval_mode" =>
            {
                Some((item.native_ref.as_str(), &item.payload["data"]))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn native_modes_are_compared_with_the_exact_requested_mode_including_plan() {
    for mode in [
        "default",
        "acceptEdits",
        "auto",
        "bypassPermissions",
        "plan",
    ] {
        let mut mapper = mapper();
        let turn = new_id();
        mapper.expect_permission_mode(mode);
        mapper.begin_turn(turn);
        let frame = report(json!(mode));
        let events = mapper.on_frame(&frame);
        let data = modes(&events)[0].1;
        assert_eq!(data["turn_id"], json!(turn));
        assert_eq!(data["value"], value(mode));
        assert_eq!(data["expected"]["value"], value(mode));
        assert_eq!(data["diverged"], false);
        assert_eq!(data["next_turn"], false);
        assert!(!events.iter().any(|event|matches!(event,SessionEvent::ItemCompleted(item) if item.payload["code"]=="harness_diverged")));
        assert!(mapper.on_frame(&frame).is_empty());
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("private-setting")
        );
    }
}

#[test]
fn a_different_native_mode_is_owned_visible_and_does_not_replace_the_requested_policy() {
    let mut mapper = mapper();
    let turn = new_id();
    mapper.expect_permission_mode("default");
    mapper.begin_turn(turn);
    let events = mapper.on_frame(&report(json!("bypassPermissions")));
    let data = modes(&events)[0].1;
    assert_eq!(data["expected"]["value"], value("default"));
    assert_eq!(data["value"], value("bypassPermissions"));
    assert_eq!(data["diverged"], true);
    assert!(events.iter().any(|event| matches!(event,SessionEvent::ItemCompleted(item)
        if item.payload["code"]=="harness_diverged" && item.payload["details"]["turn_id"]==json!(turn))));
    assert_eq!(mapper.requested_permission_mode.as_deref(), Some("default"));
    assert!(mapper.turn_active());
}

#[test]
fn init_before_the_turn_and_a_still_running_process_keep_each_turns_actual_mode() {
    let mut mapper = mapper();
    mapper.expect_permission_mode("default");
    let idle = mapper.on_frame(&report(json!("acceptEdits")));
    assert!(modes(&idle)[0].1["turn_id"].is_null());
    assert!(
        idle.iter()
            .all(|event| !matches!(event, SessionEvent::ItemStarted(_)))
    );
    let first = new_id();
    let active = mapper.begin_turn_with_model(first, "model", None);
    assert_eq!(modes(&active)[0].1["turn_id"], json!(first));
    assert_ne!(modes(&idle)[0].0, modes(&active)[0].0);
    assert!(mapper.on_frame(&report(json!("acceptEdits"))).is_empty());
    let _ = mapper.on_frame(&Frame::from_value(
        json!({"type":"result","subtype":"success"}),
    ));
    mapper.expect_permission_mode("acceptEdits");
    let second = new_id();
    let next = mapper.begin_turn_with_model(second, "model", None);
    assert_eq!(modes(&next)[0].1["turn_id"], json!(second));
    assert_eq!(modes(&next)[0].1["diverged"], false);
    assert_eq!(modes(&next)[0].1["changed"], false);
    assert_ne!(modes(&active)[0].0, modes(&next)[0].0);
    mapper.process_started(false);
    assert!(
        mapper
            .begin_turn_with_model(new_id(), "model", None)
            .is_empty()
    );
}

#[test]
fn foreign_child_unknown_and_missing_modes_never_change_the_main_process_report() {
    let mut mapper = mapper();
    mapper.expect_permission_mode("default");
    mapper.begin_turn(new_id());
    let mut foreign = report(json!("bypassPermissions")).raw().clone();
    foreign["session_id"] = json!("another-session");
    assert!(mapper.on_frame(&Frame::from_value(foreign)).is_empty());
    assert!(mapper.on_frame(&report(Json::Null)).is_empty());
    assert!(mapper.reported_permission_mode.is_none());
    let mut child = report(json!("future-mode")).raw().clone();
    child["parent_tool_use_id"] = json!("unknown-child");
    let events = mapper.on_frame(&Frame::from_value(child));
    assert_eq!(modes(&events)[0].1["value"], value("future-mode"));
    assert!(modes(&events)[0].1["turn_id"].is_null());
    assert_eq!(modes(&events)[0].1["expected"]["state"], "unsupported");
    assert!(mapper.reported_permission_mode.is_none());
    let actual = mapper.on_frame(&report(json!("future-main-mode")));
    assert_eq!(modes(&actual)[0].1["value"], value("future-main-mode"));
    assert_eq!(modes(&actual)[0].1["diverged"], true);
}

#[test]
fn a_permission_control_acknowledges_only_a_mode_the_program_actually_reports() {
    let mut mapper = mapper();
    mapper.expect_permission_mode("plan");
    mapper.begin_turn(new_id());
    let _ = mapper.on_frame(&report(json!("plan")));
    mapper.expect_permission_mode("default");
    mapper.forget_permission_report();
    let answer = |response: Json| {
        Frame::from_value(json!({"type":"control_response",
        "response":{"subtype":"success","request_id":"mode-change","response":response}}))
    };
    assert!(mapper.permission_mode_answer(&answer(json!({}))).is_empty());
    assert!(mapper.reported_permission_mode.is_none());
    let events =
        mapper.permission_mode_answer(&answer(json!({"current_permission_mode":"default"})));
    assert_eq!(modes(&events)[0].1["value"], value("default"));
    assert_eq!(modes(&events)[0].1["diverged"], false);
    assert_eq!(modes(&events)[0].1["changed"], true);
}
