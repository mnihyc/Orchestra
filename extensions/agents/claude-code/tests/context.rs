//! The context a thread is given reaches the real Claude Code in Docker as a variable of its environment and a suffix of the model's
//! name, the window the harness reports with its usage is the one in force, and the conversation is shortened within the context
//! given and not at the model's own point.
//!
//! A fast-mode cooldown result is presently a constructed-frame mapping case in events.rs.
//! It is fixture evidence until a provider manager captures a real plan's cooldown using the
//! procedure in scripts/harness-canary/README.md; these context tests do not certify that state.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, outcome};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{SessionEvent, TurnOutcome, TurnPhaseKind};
use serde_json::{Value, json};

/// The window the harness works with for a name it does not know, when it is told nothing.
const UNKNOWN_NAMES_OWN_WINDOW: i64 = 200_000;

/// Runs a turn with these parameters to its end, allowing what the harness asks to do.
async fn run(rig: &mut Rig, text: &str, params: &Value) -> Vec<SessionEvent> {
    let mut turn = rig.turn(text);
    turn.params = params.clone();
    rig.adapter.start_turn(turn).await.unwrap();
    let mut allow = common::allow;
    rig.deciding_to_the_end(&mut allow).await
}

/// A turn that reads a lot: the model reports `prompt_tokens` of prompt for the request that asks for a tool, so the harness
/// weighs the conversation against its limit before it makes the next request.
fn reading(prompt_tokens: i64) -> String {
    format!(
        "#!mock\nusage {prompt_tokens} 50\nsay Reading a lot.\ntool Bash {{\"command\":\"mkdir -p c && echo x > c/x\",\"description\":\"x\"}}\nthen\nusage 3000 20\nsay Finished.\n"
    )
}

/// The phases a turn went through, in order.
fn phases(events: &[SessionEvent]) -> Vec<TurnPhaseKind> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TurnPhase(phase) => Some(phase.kind),
            _ => None,
        })
        .collect()
}

/// The window the harness reported with the usage of a turn.
fn window_of(events: &[SessionEvent]) -> Option<i64> {
    events.iter().find_map(|event| match event {
        SessionEvent::UsageUpdated(usage) => usage.context_window,
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_window_the_harness_reports_is_the_one_the_context_and_the_models_own_window_come_to() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let cases = [
        (
            "nothing told: what it has for a name it does not know",
            json!({}),
            UNKNOWN_NAMES_OWN_WINDOW,
        ),
        (
            "a larger context than 200 000: the suffix gives the name a million",
            json!({ "context": 1_000_000, "context_window": 200_000 }),
            1_000_000,
        ),
        (
            "a context below the window: the window is the model's own, and only the point of shortening moves",
            json!({ "context": 150_000, "context_window": 200_000 }),
            UNKNOWN_NAMES_OWN_WINDOW,
        ),
        (
            "a model whose own window is told, which is not the harness's for a name it does not know",
            json!({ "context_window": 128_000 }),
            128_000,
        ),
    ];
    for (what, params, window) in cases {
        let events = run(&mut rig, "how large is your window?", &params).await;
        assert_eq!(outcome(&events), TurnOutcome::Completed, "{what}");
        assert_eq!(window_of(&events), Some(window), "{what}");
    }
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conversation_is_shortened_within_the_context_given_and_not_at_the_models_own_point() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    // 150 000 is well within the 200 000 the harness works with for this name, but past a context of 100 000.
    let params = json!({ "context": 100_000, "context_window": 200_000 });
    let events = run(&mut rig, &reading(150_000), &params).await;
    assert_eq!(
        phases(&events),
        [TurnPhaseKind::Compacting, TurnPhaseKind::Working],
        "the conversation was shortened, since it is larger than the context"
    );
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conversation_the_models_own_window_would_have_shortened_is_not_where_the_context_is_larger()
 {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    // 198 000 is where the harness shortens a conversation of this name when it is told nothing (the test of compaction
    // in `turns.rs`): with a context of a million it is not.
    let params = json!({ "context": 1_000_000, "context_window": 200_000 });
    let events = run(&mut rig, &reading(198_000), &params).await;
    assert_eq!(
        phases(&events),
        Vec::<TurnPhaseKind>::new(),
        "no shortening below the context given"
    );
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    rig.adapter.stop().await.unwrap();
}
