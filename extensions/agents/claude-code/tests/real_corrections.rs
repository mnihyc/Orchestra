//! A running tool says what it is, and the real harness's natural turn identifies its final answer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::Rig;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{ItemContent, SessionEvent, ToolPayload, TurnOutcome};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_real_program_shows_sleep_before_its_result_and_marks_its_last_answer_final() {
    let mut rig = Rig::new()
        .await
        .expect("the real harness image is required");
    rig.start_quick().await;
    rig.open().await;
    let mut turn = rig.turn("#!mock\nsay Working.\ntool Bash {\"command\":\"sleep 5\",\"description\":\"Wait\"}\nthen\nsay Done.\n");
    turn.approval_mode = ApprovalMode::Bypass;
    rig.adapter.start_turn(turn).await.unwrap();
    let mut events = rig
        .until(
            |event| match event {
                SessionEvent::ItemUpdated(update) => update.payload["tool"]["command"] == "sleep 5",
                SessionEvent::ItemStarted(start) => {
                    matches!(&start.content, ItemContent::ToolCall(call)
            if matches!(&call.tool, ToolPayload::Command { command, .. } if command == "sleep 5"))
                }
                _ => false,
            },
            90,
        )
        .await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ItemCompleted(done)
        if done.payload.get("tool").is_some())),
        "the command is visible while it runs"
    );
    events.extend(rig.until_the_turn_ends().await);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::ItemCompleted(done)
        if done.payload["tool"]["command"] == "sleep 5"))
    );
    let messages: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(done) if done.payload.get("text").is_some() => Some((
                done.payload["text"].as_str().unwrap(),
                done.payload["phase"].as_str().unwrap(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(messages, [("Working.", "commentary"), ("Done.", "final")]);
    assert!(
        matches!(events.last().unwrap(), SessionEvent::TurnCompleted(end) if end.status == TurnOutcome::Completed)
    );
    rig.adapter.stop().await.unwrap();
}
