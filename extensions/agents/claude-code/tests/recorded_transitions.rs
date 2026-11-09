//! Native recorded transitions replay through the public mapper without a process or account.
//!
//! These are versioned native captures, not constructed stream-json examples.
//! Kinds that lack a native capture are listed separately; this suite does not claim to cover them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use orch_adapter_claude_code::{
    events::{EventMapper, MapperSettings},
    stream::Frame,
};
use orch_agent_contract::events::{
    ItemContent, ItemStatus, SessionEvent, TurnOutcome, TurnPhaseKind,
};
use orch_types::{Json, ManualClock, new_id};
use serde_json::json;
use std::sync::Arc;

fn captured(source: &str) -> Json {
    let doc: Json = serde_json::from_str(source).unwrap();
    assert_eq!(doc["harness"], "claude-code");
    assert_eq!(
        doc["version"], "2.1.296",
        "native evidence names the version it actually observed"
    );
    assert_eq!(doc["recorded"], "2026-10-12");
    assert!(
        doc["frames"]
            .as_array()
            .is_some_and(|frames| !frames.is_empty())
    );
    doc
}
fn mapper() -> EventMapper {
    EventMapper::new(
        MapperSettings {
            directory: "/workspace".into(),
            cwd: "/workspace".into(),
            raw_events: false,
        },
        Arc::new(ManualClock::new("2026-10-12T10:00:00Z".parse().unwrap())),
    )
}
fn notices(events: &[SessionEvent], code: &str) -> usize {
    events.iter().filter(|event|matches!(event,SessionEvent::ItemStarted(item) if matches!(&item.content,ItemContent::Error(error) if error.code==code))).count()
}
fn initial(doc: &Json) -> Frame {
    Frame::from_value(
        doc["frames"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["type"] == "system" && row["subtype"] == "init")
            .unwrap()
            .clone(),
    )
}
const OVERLOADED: &str = include_str!("fixtures/failures/overloaded_529.json");

#[path = "recorded_transitions/native_2_1_319.rs"]
mod native_2_1_319;

#[test]
fn a_recorded_init_exposes_both_models_and_one_nonfatal_notice_on_the_actual_turn() {
    let recording = captured(OVERLOADED);
    let frame = initial(&recording);
    let actual = frame.text("model").unwrap();
    for init_before_turn in [true, false] {
        let mut mapper = mapper();
        let turn = new_id();
        if init_before_turn {
            let _ = mapper.on_frame(&frame);
        }
        let mut events = mapper.begin_turn_with_model(turn, "requested-other-model", None);
        if !init_before_turn {
            events.extend(mapper.on_frame(&frame));
        }
        assert_eq!(notices(&events, "model_rerouted"), 1);
        let notice = events
            .iter()
            .find_map(|e| {
                if let SessionEvent::ItemStarted(i) = e
                    && matches!(&i.content, ItemContent::Error(error) if error.code == "model_rerouted")
                {
                    Some(i)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(notice.turn_id, turn);
        let ItemContent::Error(error) = &notice.content else {
            panic!("the model notice is an error item")
        };
        assert!(error.message.contains(actual) && error.message.contains("requested-other-model"));
        assert!(!error.retryable);
        assert!(events.iter().any(
            |e| matches!(e,SessionEvent::ItemCompleted(i) if i.native_ref == notice.native_ref && i.status==ItemStatus::Completed)
        ));
        assert_eq!(notices(&mapper.on_frame(&frame), "model_rerouted"), 0);
    }
}
#[test]
fn native_529_500_and_429_recordings_show_every_retry_before_the_later_notice() {
    for source in [
        OVERLOADED,
        include_str!("fixtures/failures/server_error_500.json"),
        include_str!("fixtures/failures/throttled_though_not_the_plans_limit.json"),
    ] {
        let recording = captured(source);
        let mut mapper = mapper();
        let turn = new_id();
        mapper.begin_turn(turn);
        let mut attempts = Vec::new();
        let mut notice_count = 0;
        let mut ended = Vec::new();
        for row in recording["frames"].as_array().unwrap() {
            let events = mapper.on_frame(&Frame::from_value(row.clone()));
            if row["type"] == "system" && row["subtype"] == "api_retry" {
                let number = row["attempt"].as_i64().unwrap();
                attempts.push(number);
                let phases: Vec<_> = events
                    .iter()
                    .filter_map(|e| {
                        if let SessionEvent::TurnPhase(p) = e {
                            Some(p)
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(
                    phases.len(),
                    1,
                    "each native retry is immediately observable"
                );
                assert_eq!(phases[0].turn_id, turn);
                assert_eq!(phases[0].kind, TurnPhaseKind::Retrying);
                assert_eq!(phases[0].retry_attempt, Some(number));
                let notices = notices(&events, "upstream_retrying");
                assert_eq!(
                    notices,
                    usize::from(number == 5),
                    "first four have status, the fifth adds one notice"
                );
                notice_count += notices;
            }
            ended.extend(events.into_iter().filter_map(|e| {
                if let SessionEvent::TurnCompleted(t) = e {
                    Some(t)
                } else {
                    None
                }
            }));
        }
        assert_eq!(
            attempts,
            (1..=10).collect::<Vec<_>>(),
            "literal counts from these native captures"
        );
        assert_eq!(notice_count, 1);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].status, TurnOutcome::Failed);
        assert_eq!(
            ended[0].error.as_ref().unwrap().code,
            recording["expect"]["code"].as_str().unwrap()
        );
    }
}
#[test]
fn the_recorded_success_after_a_retry_resumes_work_and_has_one_successful_end() {
    let recording = captured(include_str!(
        "fixtures/failures/throttled_once_and_then_answered.json"
    ));
    let mut mapper = mapper();
    let turn = new_id();
    mapper.begin_turn(turn);
    let events: Vec<_> = recording["frames"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| mapper.on_frame(&Frame::from_value(row.clone())))
        .collect();
    let phases: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let SessionEvent::TurnPhase(p) = e {
                Some((p.kind, p.retry_attempt))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        phases,
        [
            (TurnPhaseKind::Retrying, Some(1)),
            (TurnPhaseKind::Working, None)
        ]
    );
    assert_eq!(notices(&events, "upstream_retrying"), 0);
    let ended: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let SessionEvent::TurnCompleted(t) = e {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].turn_id, turn);
    assert_eq!(ended[0].status, TurnOutcome::Completed);
    assert!(ended[0].error.is_none());
}
#[test]
fn replayed_native_retries_never_cross_the_process_or_turn_boundary() {
    let recording = captured(OVERLOADED);
    let retry = recording["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["subtype"] == "api_retry")
        .unwrap();
    let mut mapper = mapper();
    assert!(
        mapper
            .on_frame(&Frame::from_value(retry.clone()))
            .iter()
            .all(|e| !matches!(e, SessionEvent::TurnPhase(_)))
    );
    for turn in [new_id(), new_id()] {
        mapper.process_started(false);
        mapper.begin_turn(turn);
        let events = mapper.on_frame(&Frame::from_value(retry.clone()));
        assert!(events.iter().any(
            |e| matches!(e,SessionEvent::TurnPhase(p) if p.turn_id==turn&&p.retry_attempt==Some(1))
        ));
        assert!(
            mapper
                .on_frame(&Frame::from_value(retry.clone()))
                .is_empty()
        );
        let _ = mapper.on_frame(&Frame::from_value(
            json!({"type":"result","subtype":"success"}),
        ));
        assert!(
            mapper
                .on_frame(&Frame::from_value(retry.clone()))
                .iter()
                .all(|e| !matches!(e, SessionEvent::TurnPhase(_)))
        );
    }
}

fn recorded_requesting(recording: &Json) -> Frame {
    Frame::from_value(
        recording["frames"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| {
                row["type"] == "system"
                    && row["subtype"] == "status"
                    && row["status"] == "requesting"
            })
            .expect("this native capture has its requesting status")
            .clone(),
    )
}

#[test]
fn the_recorded_requesting_status_is_a_safe_owned_marker_even_without_raw_logging() {
    let recording = captured(OVERLOADED);
    let status = recorded_requesting(&recording);
    let mut mapper = mapper();
    let turn = new_id();
    mapper.begin_turn(turn);
    let events = mapper.on_frame(&status);
    assert_eq!(
        events.len(),
        2,
        "one started and one completed observation, with raw logging off"
    );
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(item) if matches!(&item.content, ItemContent::Marker(_)) => {
                Some(item)
            }
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].turn_id, turn);
    let ItemContent::Marker(orch_agent_contract::events::MarkerPayload::HarnessChanged(change)) =
        &started[0].content
    else {
        panic!("a reported native state is a harness observation")
    };
    assert_eq!(change.change, "session_status");
    assert_eq!(change.value, json!({"type":"working","active_flags":[]}));
    assert_eq!(change.turn_id, Some(turn));
    assert!(!change.diverged);
    let completed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.native_ref == started[0].native_ref => {
                Some(item)
            }
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].status, ItemStatus::Completed);
    assert_eq!(completed[0].payload["data"]["turn_id"], json!(turn));
    assert!(completed[0].native_ref.contains(status.uuid().unwrap()));
    assert!(
        mapper.on_frame(&status).is_empty(),
        "the same captured UUID is not published twice"
    );
}

#[test]
fn a_recorded_status_after_the_turn_ends_does_not_borrow_the_finished_turn() {
    let recording = captured(OVERLOADED);
    let status = recorded_requesting(&recording);
    let result = Frame::from_value(
        recording["frames"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["type"] == "result")
            .unwrap()
            .clone(),
    );
    let mut mapper = mapper();
    mapper.begin_turn(new_id());
    let _ = mapper.on_frame(&result);
    assert!(!mapper.turn_active());
    let events = mapper.on_frame(&status);
    assert_eq!(
        events.len(),
        1,
        "one durable idle observation without a borrowed turn"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::ItemStarted(_)))
    );
    let completed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.payload["marker"] == "harness_changed" => {
                Some(item)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        completed.len(),
        1,
        "the idle observation still has a durable publication"
    );
    assert!(completed[0].payload["data"]["turn_id"].is_null());
    assert_eq!(completed[0].payload["data"]["change"], "session_status");
    assert_eq!(
        completed[0].payload["data"]["value"],
        json!({"type":"working","active_flags":[]})
    );
}

#[test]
fn recorded_status_deduplication_does_not_suppress_a_new_process_generation() {
    let recording = captured(OVERLOADED);
    let status = recorded_requesting(&recording);
    let mut mapper = mapper();
    let mut refs = Vec::new();
    for turn in [new_id(), new_id()] {
        mapper.process_started(false);
        mapper.begin_turn(turn);
        let events = mapper.on_frame(&status);
        let completed: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemCompleted(item)
                    if item.payload["marker"] == "harness_changed" =>
                {
                    Some(item)
                }
                _ => None,
            })
            .collect();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].payload["data"]["turn_id"], json!(turn));
        refs.push(completed[0].native_ref.clone());
        assert!(mapper.on_frame(&status).is_empty());
    }
    assert_ne!(refs[0], refs[1]);
}

#[test]
fn the_recorded_init_permission_mode_is_observed_without_guessing_the_requested_policy() {
    let recording = captured(OVERLOADED);
    let frame = initial(&recording);
    let actual = frame
        .text("permissionMode")
        .expect("the native init records its effective mode");
    for active in [true, false] {
        let mut mapper = mapper();
        let turn = active.then(new_id);
        if let Some(turn) = turn {
            mapper.begin_turn(turn);
        }
        let events = mapper.on_frame(&frame);
        let modes: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemCompleted(item)
                    if item.payload["marker"] == "harness_changed"
                        && item.payload["data"]["change"] == "approval_mode" =>
                {
                    Some(item)
                }
                _ => None,
            })
            .collect();
        assert_eq!(modes.len(), 1);
        assert_eq!(
            modes[0].payload["data"]["value"],
            json!({"native_mode":actual})
        );
        let expected_turn = json!(turn);
        assert_eq!(
            modes[0].payload["data"].get("turn_id"),
            Some(&expected_turn)
        );
        assert_eq!(
            modes[0].payload["data"]["expected"],
            json!({"state":"unsupported"})
        );
        assert_eq!(modes[0].payload["data"]["diverged"], false);
        assert_eq!(modes[0].payload["data"]["next_turn"], false);
        let starts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemStarted(item) if item.native_ref == modes[0].native_ref => {
                    Some(item)
                }
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), usize::from(active));
        if let Some(started) = starts.first() {
            assert_eq!(Some(started.turn_id), turn);
        }
        assert_eq!(
            notices(&events, "model_rerouted"),
            0,
            "an unknown requested model is not a mismatch"
        );
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, SessionEvent::TurnCompleted(_)))
        );
        assert!(
            mapper.on_frame(&frame).is_empty(),
            "the same captured init is not reported twice"
        );
    }
}
