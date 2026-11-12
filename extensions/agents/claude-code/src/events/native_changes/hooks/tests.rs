//! Vendor SDK shapes; real native capture evidence is qualified separately by conformance.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use super::*;
use orch_types::{ManualClock, new_id};
fn mapper() -> (EventMapper, ManualClock) {
    let clock = ManualClock::new("2026-11-07T00:00:00Z".parse().unwrap());
    let mut mapper = EventMapper::new(
        MapperSettings {
            directory: "/work".into(),
            cwd: "/work".into(),
            raw_events: false,
        },
        Arc::new(clock.clone()),
    );
    mapper.set_native_session("session");
    (mapper, clock)
}
fn frame(kind: &str, extra: Json) -> Frame {
    let mut value = json!({"type":"system","subtype":kind,"hook_id":"hook-one","hook_name":"Check workspace","hook_event":"PreToolUse","uuid":format!("{kind}-uuid"),"session_id":"session"});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    Frame::from_value(value)
}
fn completed(events: &[SessionEvent]) -> &ItemCompleted {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) => Some(item),
            _ => None,
        })
        .unwrap()
}
#[test]
fn a_hook_run_has_one_identity_and_safe_terminal_facts_on_its_actual_source_turn() {
    let (mut mapper, clock) = mapper();
    let origin = new_id();
    mapper.begin_turn(origin);
    let first = mapper.on_frame(&frame("hook_started", json!({})));
    let SessionEvent::ItemStarted(start) = &first[0] else {
        panic!("the hook begins on its actual source turn")
    };
    assert_eq!(first.len(), 1);
    assert_eq!(start.turn_id, origin);
    let native_ref = start.native_ref.clone();
    clock.advance(chrono::Duration::milliseconds(125));
    assert!(
        mapper
            .on_frame(&frame(
                "hook_progress",
                json!({"stdout":"PRIVATE-OUTPUT","stderr":"PRIVATE-ERROR","output":"PRIVATE-BODY"})
            ))
            .is_empty()
    );
    let _ = mapper.on_frame(&Frame::from_value(
        json!({"type":"result","subtype":"success"}),
    ));
    mapper.begin_turn(new_id());
    clock.advance(chrono::Duration::milliseconds(175));
    let end=mapper.on_frame(&frame("hook_response",json!({"outcome":"success","exit_code":0,"output":"PRIVATE-BODY","config":{"secret":"PRIVATE-CONFIG"}})));
    let item = completed(&end);
    assert_eq!(item.native_ref, native_ref);
    assert_eq!(item.status, ItemStatus::Completed);
    assert_eq!(item.payload["data"]["subject"], "Check workspace");
    assert_eq!(item.payload["data"]["turn_id"], json!(origin));
    assert_eq!(item.payload["data"]["value"]["duration_ms"], 300);
    assert_eq!(item.payload["data"]["value"]["event_name"], "PreToolUse");
    assert_eq!(item.payload["data"]["value"]["status"], "completed");
    assert!(!serde_json::to_string(&end).unwrap().contains("PRIVATE-"));
    clock.advance(chrono::Duration::milliseconds(500));
    assert!(
        mapper
            .on_frame(&frame(
                "hook_response",
                json!({"outcome":"success","exit_code":0})
            ))
            .is_empty(),
        "native replay must not retime the execution"
    );
    assert!(
        mapper
            .on_frame(&frame("hook_progress", json!({})))
            .is_empty(),
        "late progress cannot reopen a terminal hook"
    );
}
#[test]
fn standalone_idle_and_unnamed_reports_do_not_invent_a_turn_name_start_or_duration() {
    let (mut mapper, _) = mapper();
    let events = mapper.on_frame(&frame(
        "hook_response",
        json!({"hook_name":null,"outcome":"error","exit_code":2}),
    ));
    assert_eq!(events.len(), 1);
    let item = completed(&events);
    assert_eq!(item.status, ItemStatus::Failed);
    assert!(item.payload["data"]["turn_id"].is_null());
    assert!(item.payload["data"]["subject"].is_null());
    assert!(item.payload["data"]["value"]["duration_ms"].is_null());
    assert_eq!(item.payload["data"]["value"]["status"], "failed");
}
#[test]
fn unknown_or_cancelled_outcomes_are_not_claimed_successful_and_foreign_sessions_are_ignored() {
    for (outcome, status, item_status) in [
        ("cancelled", "stopped", ItemStatus::Cancelled),
        ("future", "unknown", ItemStatus::Completed),
    ] {
        let (mut mapper, _) = mapper();
        let turn = new_id();
        mapper.begin_turn(turn);
        assert!(
            mapper
                .on_frame(&frame("hook_started", json!({"session_id":"foreign"})))
                .is_empty()
        );
        let events = mapper.on_frame(&frame("hook_response", json!({"outcome":outcome})));
        let item = completed(&events);
        assert_eq!(item.status, item_status);
        assert_eq!(item.payload["data"]["value"]["status"], status);
        assert!(item.payload["data"]["previous"]["state"] == "unsupported");
    }
}
#[test]
fn a_running_hook_survives_a_live_mapper_refresh_but_new_native_process_ids_are_distinct() {
    let (mut original, _) = mapper();
    original.begin_turn(new_id());
    let start = original.on_frame(&frame("hook_started", json!({})));
    let SessionEvent::ItemStarted(start) = &start[0] else {
        panic!("started")
    };
    let (mut refreshed, _) = mapper();
    refreshed.inherit_process_report(&original);
    refreshed.begin_turn(new_id());
    let end = refreshed.on_frame(&frame("hook_response", json!({"outcome":"success"})));
    assert_eq!(completed(&end).native_ref, start.native_ref);
    assert_eq!(
        completed(&end).payload["data"]["turn_id"],
        json!(start.turn_id)
    );
    refreshed.process_started(false);
    let next = refreshed.on_frame(&frame("hook_response", json!({"outcome":"success"})));
    assert_ne!(completed(&next).native_ref, start.native_ref);
}
#[test]
fn child_hook_keeps_the_known_parent_when_the_native_terminal_frame_omits_it() {
    let (mut mapper, _) = mapper();
    let origin = new_id();
    mapper.begin_turn(origin);
    let _=mapper.on_frame(&Frame::from_value(json!({"type":"assistant","session_id":"session","message":{"id":"agent-message","content":[{"type":"tool_use","id":"agent-tool","name":"Agent","input":{"prompt":"child"}}]}})));
    let _ = mapper.on_frame(&Frame::from_value(
        json!({"type":"result","subtype":"success"}),
    ));
    mapper.begin_turn(new_id());
    let first = mapper.on_frame(&frame(
        "hook_started",
        json!({"parent_tool_use_id":"agent-tool"}),
    ));
    let SessionEvent::ItemStarted(start) = &first[0] else {
        panic!("known child source")
    };
    assert_eq!(start.turn_id, origin);
    assert_eq!(start.parent_native_ref.as_deref(), Some("agent-tool"));
    let end = mapper.on_frame(&frame("hook_response", json!({"outcome":"success"})));
    assert_eq!(completed(&end).native_ref, start.native_ref);
    assert_eq!(completed(&end).payload["data"]["turn_id"], json!(origin));
}
