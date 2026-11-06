//! The real pinned CLI's completion follow-up waits for controller turn ownership.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native regression assertions"
)]
mod common;
use common::Rig;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::SessionEvent;

async fn completion(script: &str) {
    let mut rig = Rig::new()
        .await
        .expect("the pinned harness image is required");
    rig.start_quick().await;
    rig.open().await;
    let mut first = rig.turn(script);
    first.approval_mode = ApprovalMode::Bypass;
    let origin = first.turn_id;
    rig.adapter.start_turn(first).await.unwrap();
    let events = rig
        .until(
            |event| {
                matches!(event, SessionEvent::BackgroundTasks(report)
        if report.native_wake.is_some())
            },
            90,
        )
        .await;
    let report = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::BackgroundTasks(report) if report.native_wake.is_some() => Some(report),
            _ => None,
        })
        .expect("the native CLI started a completion follow-up without a second prompt");
    assert!(report.tasks.iter().all(|task| task.origin_turn == origin));
    assert!(
        events.iter().any(
            |event| matches!(event, SessionEvent::TurnCompleted(done) if done.turn_id == origin)
        )
    );
    let mut admitted = rig.turn("This text must not be submitted as another user prompt");
    admitted.approval_mode = ApprovalMode::Bypass;
    admitted.native_wake = Some(report.native_wake.as_ref().unwrap().token.clone());
    let continuation = admitted.turn_id;
    rig.adapter.start_turn(admitted).await.unwrap();
    let released = rig.until_the_turn_ends().await;
    assert!(
        matches!(released.first(), Some(SessionEvent::TurnStarted(start)) if start.turn_id == continuation)
    );
    assert!(released.iter().any(
        |event| matches!(event, SessionEvent::TurnCompleted(done) if done.turn_id == continuation)
    ));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_background_command_followup_waits_for_an_admitted_continuation() {
    completion("#!mock\ntool Bash {\"command\":\"sleep 2\",\"description\":\"Build\",\"run_in_background\":true}\nthen\nsay Waiting for Build.\n").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_background_agent_followup_waits_for_an_admitted_continuation() {
    completion("#!mock\ntool Agent {\"subagent_type\":\"general-purpose\",\"description\":\"Review\",\"run_in_background\":true,\"prompt\":\"#!mock\\ntool Bash {\\\"command\\\":\\\"sleep 2\\\"}\\nthen\\nsay Review complete.\\n\"}\nthen\nsay Waiting for Review.\n").await;
}
