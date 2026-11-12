//! Retry phases preserve native counts independently of the later persistent error notice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
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
    mapper.begin_turn(new_id());
    mapper
}
fn retry(attempt: Json) -> Frame {
    Frame::from_value(
        json!({"type":"system","subtype":"api_retry","attempt":attempt,
        "max_retries":10,"error_status":529,"error":"overloaded"}),
    )
}
fn phase(events: &[SessionEvent]) -> &TurnPhase {
    events
        .iter()
        .find_map(|e| {
            if let SessionEvent::TurnPhase(p) = e {
                Some(p)
            } else {
                None
            }
        })
        .expect("a native phase")
}
#[test]
fn the_first_four_retries_report_the_exact_native_number_without_an_error_item() {
    let mut mapper = mapper();
    for attempt in 1..=4 {
        let events = mapper.on_frame(&retry(json!(attempt)));
        assert_eq!(events.len(), 1);
        assert_eq!(phase(&events).kind, TurnPhaseKind::Retrying);
        assert_eq!(phase(&events).retry_attempt, Some(attempt));
        assert_eq!(Some(phase(&events).turn_id), mapper.turn);
        assert!(
            mapper.on_frame(&retry(json!(attempt))).is_empty(),
            "replayed native status cannot reset the timer"
        );
    }
    // The count is native, not the number of frames received: an observed jump stays a jump.
    let events = mapper.on_frame(&retry(json!(9)));
    assert_eq!(phase(&events).retry_attempt, Some(9));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, SessionEvent::ItemStarted(_)))
            .count(),
        1
    );
    assert!(mapper.on_frame(&retry(json!(9))).is_empty());
}
#[test]
fn only_real_main_output_ends_the_retry_wait_and_a_new_turn_keeps_no_old_phase() {
    let mut mapper = mapper();
    assert_eq!(
        phase(&mapper.on_frame(&retry(json!(1)))).kind,
        TurnPhaseKind::Retrying
    );
    for value in [
        json!({"type":"keep_alive"}),
        json!({"type":"system","subtype":"status","status":null}),
        json!({"type":"stream_event","parent_tool_use_id":"child","event":{"type":"message_start","message":{"id":"child-message"}}}),
    ] {
        assert!(
            !mapper
                .on_frame(&Frame::from_value(value))
                .iter()
                .any(|e| matches!(e, SessionEvent::TurnPhase(_)))
        );
    }
    let events=mapper.on_frame(&Frame::from_value(json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"main-message"}}})));
    assert_eq!(phase(&events).kind, TurnPhaseKind::Working);
    assert_eq!(phase(&events).retry_attempt, None);
    assert!(mapper.on_frame(&Frame::from_value(json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}}))).iter().all(|e|!matches!(e,SessionEvent::TurnPhase(_))));
    mapper.begin_turn(new_id());
    let events = mapper.on_frame(&retry(json!(1)));
    assert_eq!(phase(&events).kind, TurnPhaseKind::Retrying);
    assert_eq!(phase(&events).retry_attempt, Some(1));
}
#[test]
fn unknown_attempts_stay_unknown_and_a_child_retry_cannot_change_the_main_phase() {
    let mut mapper = mapper();
    for value in [Json::Null, json!(0), json!(-1), json!("2")] {
        mapper.begin_turn(new_id());
        let events = mapper.on_frame(&retry(value));
        assert_eq!(phase(&events).retry_attempt, None);
        assert_eq!(events.len(), 1, "no invented threshold notice");
    }
    let mut child = retry(json!(7)).raw().clone();
    child["parent_tool_use_id"] = json!("child");
    assert!(mapper.on_frame(&Frame::from_value(child)).is_empty());
    let _ = mapper.on_frame(&Frame::from_value(
        json!({"type":"result","subtype":"success"}),
    ));
    assert!(
        !mapper
            .on_frame(&retry(json!(1)))
            .iter()
            .any(|e| matches!(e, SessionEvent::TurnPhase(_))),
        "idle frames are not assigned to the last turn"
    );
}
#[test]
fn the_notice_threshold_is_a_profile_default_independent_of_the_live_phase() {
    let mut mapper = mapper();
    mapper.set_retry_notice_attempt(2);
    assert_eq!(mapper.on_frame(&retry(json!(1))).len(), 1);
    let events = mapper.on_frame(&retry(json!(2)));
    assert_eq!(phase(&events).retry_attempt, Some(2));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::ItemStarted(_)))
    );
    mapper.begin_turn(new_id());
    mapper.set_retry_notice_attempt(100);
    assert_eq!(
        mapper.on_frame(&retry(json!(5))).len(),
        1,
        "a default is never a cap"
    );
    mapper.begin_turn(new_id());
    mapper.set_retry_notice_attempt(0);
    assert!(
        mapper
            .on_frame(&retry(json!(1)))
            .iter()
            .any(|e| matches!(e, SessionEvent::ItemStarted(_)))
    );
}
