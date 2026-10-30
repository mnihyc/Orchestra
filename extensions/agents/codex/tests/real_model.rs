//! The one check that reaches a real model: the real Codex in Docker, a relay that stands for the connector's
//! forwarder and holds the key of a real interface, and a model that costs next to nothing.
//!
//! It runs only where the key is in the environment (`OPENROUTER_API_KEY`) and the check is asked for
//! (`ORCH_REAL_MODEL_CHECK=1`), so that a key that is there for another reason spends nothing here; anywhere
//! else it says so and passes. The key is read into memory, handed to the relay and nowhere else: it is not printed, not written, and the
//! harness never sees it. At most a handful of requests go upstream (the relay refuses the rest).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::relay::Relay;
use common::{Rig, kinds};
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{
    HarnessRequestKind, ItemStatus, SessionEvent, ToolCallPayload, ToolPayload, TurnOutcome,
};
use orch_agent_contract::session::SessionMode;
use orch_types::new_id;

/// The interface and the cheapest model of the family that answers there.
const UPSTREAM: &str = "https://openrouter.ai/api/v1/responses";
const MODEL: &str = "openai/gpt-5.6-luna";

/// The requests the check may send upstream: one turn of text, one of a command that asks to be allowed (two
/// requests), and room for one more.
const REQUEST_CAP: usize = 4;

#[tokio::test]
async fn a_real_model_answers_through_the_real_harness_and_runs_a_command_that_was_allowed() {
    let asked = std::env::var("ORCH_REAL_MODEL_CHECK").is_ok_and(|value| value == "1");
    let Some(key) = std::env::var("OPENROUTER_API_KEY").ok().filter(|_| asked) else {
        eprintln!(
            "SKIPPED real-model: needs OPENROUTER_API_KEY and ORCH_REAL_MODEL_CHECK=1 in the environment; \
             this check reaches a real model"
        );
        return;
    };
    let Some(mut rig) = Rig::start("real").await else {
        return;
    };
    let relay = Relay::start(UPSTREAM, key, rig.token.clone(), REQUEST_CAP).await;
    rig.loopback_port = Some(relay.port());
    rig.boot().await;

    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.settings.model = MODEL.to_owned();
    spec.settings.approval_mode = ApprovalMode::Ask;
    rig.adapter.open_session(spec).await.unwrap();

    // A turn of text.
    let mut turn = rig.turn(thread, "Reply with exactly the word: pong");
    turn.model = MODEL.to_owned();
    turn.effort = Some("low".to_owned());
    turn.approval_mode = ApprovalMode::Ask;
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_turn_ends().await;
    let Some(SessionEvent::TurnCompleted(ended)) = events.last() else {
        panic!("the turn ends: {:?}", kinds(&events));
    };
    assert_eq!(
        ended.status,
        TurnOutcome::Completed,
        "{:?} {:?} {:?}",
        ended.error,
        kinds(&events),
        relay.log()
    );
    let said: String = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item) => item.payload.get("text").and_then(|t| t.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("real-model: the model said {said:?}");
    assert!(said.to_lowercase().contains("pong"), "{said:?}");
    let usage = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::UsageUpdated(usage) => Some(usage),
            _ => None,
        })
        .expect("the use is reported");
    assert!(usage.turn.is_consistent() && usage.turn.total_tokens > 0);
    eprintln!("real-model: usage of the first turn {:?}", usage.turn);

    // A turn with a command: it asks, it is allowed, it runs, its output is on the timeline.
    let mut turn = rig.turn(
        thread,
        "Run this shell command and then tell me in one short sentence what it printed: \
         echo orch-real-check > real-check.txt && cat real-check.txt",
    );
    turn.model = MODEL.to_owned();
    turn.effort = Some("low".to_owned());
    turn.approval_mode = ApprovalMode::Ask;
    rig.adapter.start_turn(turn).await.unwrap();
    let mut asked = 0;
    let mut events = Vec::new();
    loop {
        let event = rig.next_event().await;
        match event {
            AdapterEvent::Session {
                event: SessionEvent::ApprovalRequested(request),
                ..
            } => {
                asked += 1;
                assert_eq!(request.request.request_kind, HarnessRequestKind::Command);
                rig.choose(thread, &request.request_ref, "once").await;
                events.push(SessionEvent::ApprovalRequested(request));
            }
            AdapterEvent::Session { event, .. } => {
                let done = matches!(event, SessionEvent::TurnCompleted(_));
                events.push(event);
                if done {
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(SessionEvent::TurnCompleted(ended)) = events.last() else {
        panic!("the turn ends");
    };
    assert_eq!(
        ended.status,
        TurnOutcome::Completed,
        "{:?} {:?} {:?}",
        ended.error,
        kinds(&events),
        relay.log()
    );
    assert!(
        asked >= 1,
        "the command asked to be allowed: {:?}",
        kinds(&events)
    );
    let ran = events.iter().any(|event| match event {
        SessionEvent::ItemCompleted(item) if item.status == ItemStatus::Completed => {
            serde_json::from_value::<ToolCallPayload>(item.payload.clone())
                .ok()
                .is_some_and(|call| {
                    matches!(&call.tool, ToolPayload::Command { exit_code: Some(0), output, .. }
                        if output.inline.contains("orch-real-check"))
                })
        }
        _ => false,
    });
    assert!(
        ran,
        "the command ran and its output arrived: {:?}",
        kinds(&events)
    );
    assert_eq!(
        std::fs::read_to_string(rig.workspace_path().join("real-check.txt")).unwrap(),
        "orch-real-check\n"
    );
    eprintln!(
        "real-model: {} requests went upstream: {:?}",
        relay.upstream_requests(),
        relay.log()
    );
    assert!(relay.upstream_requests() <= REQUEST_CAP);
    rig.adapter.stop().await.unwrap();
}
