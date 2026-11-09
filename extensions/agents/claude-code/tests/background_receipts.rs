//! A native End acknowledgement is neither proof of a terminal outcome nor authority over another run.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use common::corpus::{
    Actor, SESSION, Scripts, WAIT, barrier, from_native, handshake, input, to_native, turn,
};
use futures::poll;
use orch_adapter_claude_code::{conversation::Command, sessions::Origin};
use orch_agent_contract::background::BackgroundEndState;
use orch_testkit::harness_corpus::{Frame, collect_through};
use serde_json::json;

fn notification(run: &str, session: &str, status: &str) -> Frame {
    from_native(
        json!({"type":"system","subtype":"task_notification","session_id":session,
        "task_id":"task","run_id":run,"tool_use_id":"tool","status":status,"summary":"Fixture terminal",
        "exit_code":if status=="completed" {Some(0)} else if status=="failed" {Some(2)} else {None}}),
    )
}

async fn receipt(native_status: &str, expected: BackgroundEndState) {
    let mut frames = handshake();
    frames.push(input());
    frames.push(from_native(json!({"type":"assistant","message":{"id":"message","role":"assistant","content":[{
        "type":"tool_use","id":"tool","name":"Bash","input":{"command":"fixture background","run_in_background":true}}]}})));
    frames.push(from_native(json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool","content":"Launched"}]},"tool_use_result":{"backgroundTaskId":"task"}})));
    frames.push(from_native(json!({"type":"system","subtype":"task_started","session_id":SESSION,
        "task_id":"task","run_id":"run-1","tool_use_id":"tool","task_type":"local_bash","is_backgrounded":true,"description":"Fixture task"})));
    frames.push(to_native(json!({"type":"control_request","request_id":{"$replay_capture":"stop"},"request":{"subtype":"stop_task","task_id":"task"}})));
    frames.push(from_native(json!({"type":"control_response","response":{"subtype":"success","request_id":{"$replay_ref":"stop"},"response":{}}})));
    frames.push(from_native(
        json!({"type":"system","subtype":"task_progress","session_id":SESSION,
        "task_id":"task","run_id":"run-1","tool_use_id":"tool","summary":"Acknowledged only"}),
    ));
    // Neither another run nor another native session may settle this End request.
    frames.push(notification("run-0", SESSION, "stopped"));
    frames.push(notification(
        "run-1",
        "00000000-0000-0000-0000-000000000005",
        "stopped",
    ));
    frames.push(from_native(
        json!({"type":"control_request","request_id":"end-ack-observed","request":{
        "subtype":"hook_callback","callback_id":"orch-instructions-loaded","input":{}}}),
    ));
    frames.push(to_native(json!({"type":"control_response","response":{"subtype":"success","request_id":"end-ack-observed","response":{}}})));
    let after_ack = frames.len();
    frames.push(notification("run-1", SESSION, native_status));
    frames.extend(barrier());

    let (scripts, gate) = Scripts::paused(frames, after_ack, orch_testkit::clock::manual());
    let mut actor = Actor::start_with_scripts(scripts.clone(), Origin::New, false, false).await;
    actor.opened.as_ref().unwrap();
    let mut observed = collect_through(&mut actor.events, "session.background_tasks", WAIT)
        .await
        .unwrap();
    let roster = actor
        .handle
        .ask(|reply| Command::Background { reply })
        .await
        .unwrap();
    let id = roster
        .items
        .first()
        .expect("the exact observed native run is offered")
        .id
        .clone();
    let ended = {
        let selected = id.clone();
        let pending = actor.handle.ask(|reply| Command::EndBackground {
            id: selected,
            reply,
        });
        tokio::pin!(pending);
        tokio::time::timeout(WAIT,async {
            tokio::select! {
                answer=&mut pending=>panic!("an acknowledgement or foreign terminal cannot settle End: {answer:?}"),
                ()=gate.reached()=>{}
            }
            assert!(poll!(pending.as_mut()).is_pending(),"the actual terminal event is still withheld");
        }).await.expect("the actor must acknowledge the native checkpoint while End waits");
        gate.release();
        tokio::time::timeout(WAIT, pending).await.unwrap().unwrap()
    };
    assert_eq!(ended.id, id);
    assert_eq!(ended.state, expected);
    observed.extend(actor.collect().await);
    let completions: Vec<_> = observed
        .iter()
        .filter(|row| row.name == "item.completed" && row.body["native_ref"] == "tool")
        .collect();
    assert_eq!(
        completions.len(),
        1,
        "only this run's genuine terminal closes the original item"
    );
    assert_eq!(
        completions[0].body["status"],
        match native_status {
            "stopped" => "cancelled",
            "failed" => "failed",
            _ => "completed",
        }
    );
    let terminal: Vec<_> = observed
        .iter()
        .filter(|row| {
            row.name == "session.background_tasks" && row.body["tasks"][0]["state"] != "running"
        })
        .collect();
    assert_eq!(
        terminal.len(),
        1,
        "stale or foreign terminals cannot publish a second outcome"
    );
    assert_eq!(terminal[0].body["tasks"][0]["origin_turn"], json!(turn()));
    assert_eq!(terminal[0].body["tasks"][0]["run_id"], "run-1");
    assert!(
        !observed
            .iter()
            .any(|row| row.name == "turn.completed" || row.name == "session.lost")
    );
    actor.close().await;
    let transcript = scripts.transcripts();
    let sent: Vec<_> = transcript[0]
        .frames
        .iter()
        .filter_map(|step| match step {
            Frame::ToHarness { frame, .. } => Some(frame),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent.iter().filter(|frame| frame["type"] == "user").count(),
        1,
        "End adds no model input"
    );
    assert_eq!(
        sent.iter()
            .filter(|frame| frame.pointer("/request/subtype") == Some(&json!("stop_task")))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_real_stopped_event_settles_end_after_acknowledgement_and_rejects_other_runs() {
    receipt("stopped", BackgroundEndState::Ended).await;
}
#[tokio::test]
async fn a_command_that_completes_after_the_acknowledgement_is_already_ended() {
    receipt("completed", BackgroundEndState::AlreadyEnded).await;
}
#[tokio::test]
async fn a_command_that_fails_after_the_acknowledgement_is_already_ended() {
    receipt("failed", BackgroundEndState::AlreadyEnded).await;
}
