//! Native cancellation, late idle output and a new process must retain their actual request/turn owner.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use common::corpus::{
    Actor, SESSION, Scripts, WAIT, barrier, from_native, handshake, input, to_native, turn,
};
use orch_adapter_claude_code::{
    conversation::{self, Command},
    sessions::Origin,
};
use orch_agent_contract::{
    descriptor::{ApprovalMode, SandboxMode},
    session::{ApprovalDecision, TurnStart},
};
use orch_testkit::harness_corpus::{Bindings, Frame, assert_unrecognized, collect_through};
use orch_types::Id;
use serde_json::json;

fn permission() -> Frame {
    from_native(
        json!({"type":"control_request","request_id":"permission","request":{
        "subtype":"can_use_tool","tool_name":"FutureTool","tool_use_id":"tool","input":{"operation":"fixture"}}}),
    )
}
fn context() -> Vec<Frame> {
    vec![
        to_native(
            json!({"type":"control_request","request_id":{"$replay_capture":"context"},"request":{"subtype":"get_context_usage","detail":"summary"}}),
        ),
        from_native(
            json!({"type":"control_response","response":{"subtype":"error","request_id":{"$replay_ref":"context"},"error":"No breakdown in this fixture"}}),
        ),
    ]
}

#[tokio::test]
async fn native_cancellation_closes_the_exact_unknown_card_and_a_late_answer_sends_no_reply() {
    let mut frames = handshake();
    frames.push(input());
    frames.push(permission());
    frames.push(from_native(json!({"type":"control_request","request_id":"pending-observed","request":{"subtype":"hook_callback","callback_id":"orch-instructions-loaded","input":{}}})));
    frames.push(to_native(json!({"type":"control_response","response":{"subtype":"success","request_id":"pending-observed","response":{}}})));
    let after_pending = frames.len();
    frames.push(from_native(
        json!({"type":"control_cancel_request","request_id":"permission"}),
    ));
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after_pending, orch_testkit::clock::manual());
    let mut actor = Actor::start_with_scripts(scripts.clone(), Origin::New, false, false).await;
    actor.opened.as_ref().unwrap();
    let mut rows = collect_through(&mut actor.events, "approval.requested", WAIT)
        .await
        .unwrap();
    assert_eq!(
        rows.last().unwrap().body["request"]["request_kind"],
        "other"
    );
    gate.reached().await;
    gate.release();
    rows.extend(
        collect_through(&mut actor.events, "approval.closed", WAIT)
            .await
            .unwrap(),
    );
    rows.extend(actor.collect().await);
    let late = actor
        .handle
        .ask(|reply| Command::Resolve {
            request_ref: "permission".into(),
            decision: ApprovalDecision::Option {
                option_id: "allow_once".into(),
                note: None,
            },
            reply,
        })
        .await
        .unwrap();
    assert_eq!(late.unwrap_err().code(), "not_found");
    let closed: Vec<_> = rows
        .iter()
        .filter(|row| row.name == "approval.closed")
        .collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].body["request_ref"], "permission");
    assert!(
        !rows
            .iter()
            .any(|row| row.name == "turn.completed" || row.name == "session.lost")
    );
    actor.close().await;
    assert!(!scripts.transcripts()[0].frames.iter().any(|step|matches!(step,Frame::ToHarness{frame,..}
        if frame["type"]=="control_response" && frame.pointer("/response/request_id")==Some(&json!("permission")))),"a cancelled request has no late native response");
}

#[tokio::test]
async fn an_invalid_choice_keeps_the_real_request_pending_until_its_actual_refusal() {
    let mut frames = handshake();
    frames.push(input());
    frames.push(permission());
    frames.push(to_native(json!({"type":"control_response","response":{"subtype":"success","request_id":"permission","response":{"behavior":"deny","message":"Corpus refusal"}}})));
    frames.extend(barrier());
    let mut actor = Actor::start(frames, Origin::New, false).await;
    actor.opened.as_ref().unwrap();
    let mut rows = collect_through(&mut actor.events, "approval.requested", WAIT)
        .await
        .unwrap();
    let invalid = actor
        .handle
        .ask(|reply| Command::Resolve {
            request_ref: "permission".into(),
            decision: ApprovalDecision::Option {
                option_id: "future_unoffered_choice".into(),
                note: None,
            },
            reply,
        })
        .await
        .unwrap();
    assert_eq!(invalid.unwrap_err().code(), "not_found");
    actor
        .handle
        .ask(|reply| Command::Resolve {
            request_ref: "permission".into(),
            decision: ApprovalDecision::Option {
                option_id: "deny".into(),
                note: Some("Corpus refusal".into()),
            },
            reply,
        })
        .await
        .unwrap()
        .unwrap();
    rows.extend(actor.collect().await);
    assert_eq!(
        rows.iter()
            .filter(|row| row.name == "approval.requested")
            .count(),
        1
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.name == "turn.completed" || row.name == "session.lost")
    );
    actor.close().await;
}

#[tokio::test]
async fn malformed_and_unknown_output_after_completion_cannot_borrow_the_last_turn() {
    let mut frames = handshake();
    frames.push(input());
    frames.push(from_native(
        json!({"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":""}),
    ));
    frames.extend(context());
    frames.push(Frame::Raw {
        line: "{broken after turn".into(),
        newline: true,
    });
    frames.push(from_native(
        json!({"type":"future_after_turn","uuid":"idle-future"}),
    ));
    frames.extend(barrier());
    let mut actor = Actor::start(frames, Origin::New, false).await;
    actor.opened.as_ref().unwrap();
    let rows = actor.collect().await;
    let end = rows
        .iter()
        .position(|row| row.name == "turn.completed")
        .expect("the issued turn completes before the idle facts");
    let late = &rows[end + 1..];
    assert!(
        !late
            .iter()
            .any(|row| row.name == "item.started" || row.name == "turn.started")
    );
    let errors: Vec<_> = late
        .iter()
        .filter(|row| {
            row.name == "item.completed" && row.body["payload"]["code"] == "harness_error"
        })
        .collect();
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].body["payload"]["message"]
            .as_str()
            .unwrap()
            .contains("{broken after turn")
    );
    assert_unrecognized(late, "Claude Code", "future_after_turn", 1).unwrap();
    let marker = late
        .iter()
        .find(|row| {
            row.name == "item.completed" && row.body["payload"]["marker"] == "harness_changed"
        })
        .unwrap();
    assert_eq!(marker.body["payload"]["data"]["turn_id"], json!(null));
    actor.close().await;
}

#[tokio::test]
async fn a_resumed_process_can_reuse_a_native_uuid_without_reusing_its_previous_turns_marker() {
    let future = || from_native(json!({"type":"future_update","uuid":"same-native-uuid"}));
    let mut first = handshake();
    first.push(input());
    first.push(future());
    first.push(from_native(
        json!({"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":""}),
    ));
    first.extend(context());
    first.extend(barrier());
    let next = Id::from_u128(5);
    let mut second = handshake();
    second.push(to_native(json!({"type":"user","uuid":next,"message":{"role":"user","content":[{"type":"text","text":"Next corpus input"}]}})));
    second.push(future());
    second.extend(barrier());
    let scripts = Scripts::new(
        vec![first, second],
        orch_testkit::clock::manual(),
        Bindings::new(),
    );
    let mut actor = Actor::start_with_scripts(scripts.clone(), Origin::New, false, false).await;
    actor.opened.as_ref().unwrap();
    let before = actor.collect().await;
    assert_unrecognized(&before, "Claude Code", "future_update", 1).unwrap();
    actor
        .handle
        .ask(|reply| Command::StartTurn {
            turn: Box::new(TurnStart {
                native_model: None,
                thread_id: common::corpus::thread(),
                turn_id: next,
                text: "Next corpus input".into(),
                images: vec![],
                native_wake: None,
                plan: false,
                model: "mock-large".into(),
                effort: None,
                params: json!({}),
                approval_mode: ApprovalMode::Ask,
                sandbox_mode: SandboxMode::Open,
            }),
            blocks: conversation::message_blocks("Next corpus input", vec![]),
            reply,
        })
        .await
        .unwrap()
        .unwrap();
    let after = actor.collect().await;
    assert_unrecognized(&after, "Claude Code", "future_update", 1).unwrap();
    let observed = |rows: &[orch_testkit::harness_corpus::Observation]| {
        rows.iter()
            .find(|row| {
                row.name == "item.completed"
                    && row.body["payload"]["data"]["change"] == "unrecognized_event"
            })
            .unwrap()
            .body
            .clone()
    };
    let original = observed(&before);
    let resumed = observed(&after);
    assert_ne!(original["native_ref"], resumed["native_ref"]);
    assert_eq!(original["payload"]["data"]["turn_id"], json!(turn()));
    assert_eq!(resumed["payload"]["data"]["turn_id"], json!(next));
    assert!(!after.iter().any(|row| row.name == "session.lost"));
    let starts = scripts.launched.lock().unwrap().clone();
    assert_eq!(starts.len(), 2);
    assert!(
        starts[1]
            .args
            .windows(2)
            .any(|args| args[0] == "--resume" && args[1] == SESSION)
    );
    actor.close().await;
}
