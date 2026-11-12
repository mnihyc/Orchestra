//! Protocol controls preserve native words and source ownership without raw configuration bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
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
fn system(kind: &str, uuid: &str, extra: Json) -> Frame {
    let mut value =
        json!({"type":"system","subtype":kind,"uuid":uuid,"session_id":"native-session"});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    Frame::from_value(value)
}
fn markers(events: &[SessionEvent]) -> Vec<&Json> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.payload["marker"] == "harness_changed" => {
                Some(&item.payload)
            }
            _ => None,
        })
        .collect()
}
#[test]
fn post_init_mcp_states_keep_the_server_words_and_never_copy_its_private_definition() {
    let mut mapper = mapper();
    let turn = new_id();
    mapper.begin_turn(turn);
    let _ = mapper.on_frame(&system(
        "init",
        "init",
        json!({"mcp_servers":[{"name":"catalog","status":"connected"}]}),
    ));
    let frame = system(
        "mcp_status",
        "failed",
        json!({"mcp_servers":[{"name":"catalog","status":"failed","error":"Native connection failed","config":{"headers":{"Authorization":"credential-must-stay-private"}}}]}),
    );
    let events = mapper.on_frame(&frame);
    let marker = markers(&events)[0];
    assert_eq!(marker["data"]["turn_id"], json!(turn));
    assert_eq!(marker["data"]["change"], "mcp_startup");
    assert_eq!(marker["data"]["subject"], "catalog");
    assert_eq!(marker["data"]["previous"]["value"]["status"], "ready");
    assert_eq!(
        marker["data"]["value"]["message"],
        "Native connection failed"
    );
    assert_eq!(marker["data"]["changed"], true);
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("credential-must-stay-private")
    );
    let failed = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.payload["code"] == "mcp_server_failed" => {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(failed.status, ItemStatus::Completed);
    assert_eq!(failed.payload["details"]["server"], "catalog");
    assert_eq!(
        failed.payload["details"]["message"],
        "Native connection failed"
    );
    assert_eq!(failed.payload["details"]["turn_id"], json!(turn));
    assert!(mapper.turn_active());
    assert!(
        mapper.on_frame(&frame).is_empty(),
        "one native frame is not two reports"
    );
    let events = mapper.on_frame(&system(
        "mcp_server_status",
        "auth",
        json!({"mcp_server_name":"catalog","status":"needs-auth"}),
    ));
    assert_eq!(
        markers(&events)[0]["data"]["value"]["failure_reason"],
        "reauthenticationRequired"
    );
}
#[test]
fn an_unknown_system_kind_is_visible_with_its_words_but_no_instruction_input_or_output_body() {
    let mut mapper = mapper();
    mapper.begin_turn(new_id());
    let events=mapper.on_frame(&system("future_native_kind","future",json!({"title":"Native title","message":"Native reported words","reason":"Native reason",
        "input":"private-input","output":"private-output","instructions":"private-instructions","config":{"secret":"private-configuration"}})));
    let marker = markers(&events)[0];
    assert_eq!(marker["data"]["change"], "system_event");
    assert_eq!(marker["data"]["value"]["kind"], "future_native_kind");
    assert_eq!(marker["data"]["value"]["message"], "Native reported words");
    let shown = serde_json::to_string(&events).unwrap();
    for secret in [
        "private-input",
        "private-output",
        "private-instructions",
        "private-configuration",
    ] {
        assert!(!shown.contains(secret));
    }
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::TurnCompleted(_)))
    );
    let malformed = mapper.on_frame(&system(
        "mcp_status",
        "incomplete",
        json!({"message":"No structured state was supplied"}),
    ));
    assert_eq!(
        markers(&malformed)[0]["data"]["value"]["message"],
        "No structured state was supplied"
    );
}
#[test]
fn idle_unknown_and_foreign_reports_cannot_borrow_the_last_or_current_turn() {
    let mut mapper = mapper();
    let first = new_id();
    mapper.begin_turn(first);
    let _ = mapper.on_frame(&Frame::from_value(
        json!({"type":"result","subtype":"success"}),
    ));
    let idle = mapper.on_frame(&system(
        "notification",
        "idle",
        json!({"message":"Idle native notice"}),
    ));
    assert_eq!(idle.len(), 1);
    assert!(markers(&idle)[0]["data"]["turn_id"].is_null());
    mapper.begin_turn(new_id());
    let unknown = mapper.on_frame(&system(
        "notification",
        "unknown-child",
        json!({"parent_tool_use_id":"unowned-child","message":"Child native notice"}),
    ));
    assert_eq!(unknown.len(), 1);
    assert!(markers(&unknown)[0]["data"]["turn_id"].is_null());
    let foreign=mapper.on_frame(&system("mcp_status","foreign",json!({"session_id":"another-session","mcp_servers":[{"name":"catalog","status":"failed"}]})));
    assert!(foreign.is_empty());
}
#[test]
fn a_known_child_keeps_its_real_origin_even_when_the_main_turn_has_advanced() {
    let mut mapper = mapper();
    let first = new_id();
    mapper.begin_turn(first);
    let _=mapper.on_frame(&Frame::from_value(json!({"type":"assistant","message":{"id":"parent-message","content":[{"type":"tool_use","id":"child","name":"Agent","input":{"description":"Child work","prompt":"Work"}}]}})));
    let _ = mapper.on_frame(&Frame::from_value(
        json!({"type":"result","subtype":"success"}),
    ));
    mapper.begin_turn(new_id());
    let events = mapper.on_frame(&system(
        "notification",
        "child-event",
        json!({"parent_tool_use_id":"child","message":"Child native notice"}),
    ));
    let started = events
        .iter()
        .find_map(|event| {
            if let SessionEvent::ItemStarted(item) = event {
                Some(item)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(started.turn_id, first);
    assert_eq!(started.stream, "sub:child");
    assert_eq!(started.parent_native_ref.as_deref(), Some("child"));
    assert_eq!(markers(&events)[0]["data"]["turn_id"], json!(first));
}

#[test]
fn a_closed_child_origin_survives_mapper_refresh_but_not_a_native_process_restart() {
    let mut original = mapper();
    let origin = new_id();
    original.begin_turn(origin);
    let _=original.on_frame(&Frame::from_value(json!({"type":"assistant","message":{"id":"parent","content":[{"type":"tool_use","id":"child","name":"Agent","input":{"prompt":"Work"}}]}})));
    let _ = original.on_frame(&Frame::from_value(
        json!({"type":"result","subtype":"success"}),
    ));
    let mut refreshed = mapper();
    refreshed.inherit_process_report(&original);
    refreshed.begin_turn(new_id());
    let frame = system(
        "notification",
        "closed-child",
        json!({"parent_tool_use_id":"child","message":"Child notice"}),
    );
    let owned = refreshed.on_frame(&frame);
    assert!(
        owned
            .iter()
            .any(|event| matches!(event,SessionEvent::ItemStarted(item) if item.turn_id==origin))
    );
    refreshed.process_started(false);
    let unowned = refreshed.on_frame(&frame);
    assert!(
        !unowned
            .iter()
            .any(|event| matches!(event, SessionEvent::ItemStarted(_)))
    );
    assert!(markers(&unowned)[0]["data"]["turn_id"].is_null());
}

#[test]
fn a_reused_child_reference_never_guesses_between_two_actual_turns() {
    let mut mapper = mapper();
    for turn in [new_id(), new_id()] {
        mapper.begin_turn(turn);
        let _=mapper.on_frame(&Frame::from_value(json!({"type":"assistant","message":{"id":turn,"content":[{"type":"tool_use","id":"reused-child","name":"Agent","input":{"prompt":"Work"}}]}})));
        let _ = mapper.on_frame(&Frame::from_value(
            json!({"type":"result","subtype":"success"}),
        ));
    }
    mapper.begin_turn(new_id());
    let events = mapper.on_frame(&system(
        "notification",
        "ambiguous-child",
        json!({"parent_tool_use_id":"reused-child","message":"Child notice"}),
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ItemStarted(_)))
    );
    assert!(markers(&events)[0]["data"]["turn_id"].is_null());
}
#[test]
fn a_real_process_restart_needs_its_own_observations_and_has_a_new_reference_namespace() {
    let mut mapper = mapper();
    mapper.begin_turn(new_id());
    let frame = system(
        "notification",
        "same-native-id",
        json!({"message":"Native notice"}),
    );
    let first = mapper.on_frame(&frame);
    assert!(mapper.on_frame(&frame).is_empty());
    mapper.process_started(false);
    let second = mapper.on_frame(&frame);
    let reference = |events: &[SessionEvent]| {
        events
            .iter()
            .find_map(|event| {
                if let SessionEvent::ItemCompleted(item) = event {
                    Some(item.native_ref.clone())
                } else {
                    None
                }
            })
            .unwrap()
    };
    assert_ne!(reference(&first), reference(&second));
}
