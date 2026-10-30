//! The context a session is given reaches the real Codex in Docker as the two members of its configuration, and the window the harness
//! reports with its usage is the one in force.
//!
//! The harness is run refusing settings it does not know (`--strict-config`), so that a session opens at all only when both members
//! have the names it reads. It reports 95 % of the window it works with, holds any threshold it is given to nine tenths of that window,
//! and holds a window to the largest its catalogue allows for the model: for a model it does not know, what it would work with had it
//! been told nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::Rig;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{SessionEvent, TurnPhaseKind};
use orch_agent_contract::session::{SessionMode, ThreadId};
use orch_types::new_id;
use serde_json::{Value, json};

/// The window a model the harness does not know is given when it is told none.
const UNKNOWN_MODELS_OWN_WINDOW: i64 = 272_000;
/// What the harness reports of a window: nine tenths and a half of a tenth, which is all it works with.
const fn reported(window: i64) -> i64 {
    window * 95 / 100
}

/// Opens a session of `thread` on `model` with `params` (a new one, or the one the harness holds under `native`), runs a turn, and
/// returns the window the harness reported with the usage of the turn.
async fn window_reported(
    rig: &mut Rig,
    thread: ThreadId,
    model: &str,
    params: &Value,
    native: Option<&str>,
) -> (i64, String) {
    let mode = if native.is_some() {
        SessionMode::Resume
    } else {
        SessionMode::New
    };
    let mut spec = rig.spec(thread, mode, native);
    spec.settings.model = model.to_owned();
    spec.settings.params = params.clone();
    let opened = rig.adapter.open_session(spec).await.unwrap();
    let mut turn = rig.turn(thread, "how large is your window?");
    turn.model = model.to_owned();
    turn.params = params.clone();
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_turn_ends().await;
    let window = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::UsageUpdated(usage) => usage.context_window,
            _ => None,
        })
        .expect("the harness reports the window with the usage");
    (window, opened.native_session_ref)
}

#[tokio::test]
async fn a_context_below_the_window_a_model_would_have_is_the_window_the_harness_works_with() {
    let Some(mut rig) = Rig::start("context-below").await else {
        return;
    };
    rig.boot().await;
    let none = json!({});
    let (told_nothing, _) = window_reported(&mut rig, new_id(), common::MODEL, &none, None).await;
    assert_eq!(
        told_nothing,
        reported(UNKNOWN_MODELS_OWN_WINDOW),
        "a model it does not know, told nothing, has the window it has for those"
    );
    let smaller = json!({ "context": 100_000, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    let (window, _) = window_reported(&mut rig, new_id(), common::MODEL, &smaller, None).await;
    assert_eq!(
        window,
        reported(100_000),
        "both members were accepted, and the window is the one that was asked for"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_context_above_what_the_harness_takes_for_a_model_it_does_not_know_is_held_to_that() {
    let Some(mut rig) = Rig::start("context-above").await else {
        return;
    };
    rig.boot().await;
    let larger = json!({ "context": 1_000_000, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    let (window, _) = window_reported(&mut rig, new_id(), common::MODEL, &larger, None).await;
    assert_eq!(
        window,
        reported(UNKNOWN_MODELS_OWN_WINDOW),
        "a million asked for a model the harness does not know: no more than it takes"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_model_the_harness_knows_takes_a_larger_window_than_its_default_up_to_the_largest_its_catalogue_allows()
 {
    let Some(mut rig) = Rig::start("context-known").await else {
        return;
    };
    rig.boot().await;
    let model = "gpt-6-sol";
    let (own, _) = window_reported(&mut rig, new_id(), model, &json!({}), None).await;
    assert_eq!(
        own,
        reported(UNKNOWN_MODELS_OWN_WINDOW),
        "told nothing: its own window"
    );
    let asked = json!({ "context": 600_000, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    let (window, _) = window_reported(&mut rig, new_id(), model, &asked, None).await;
    assert_eq!(
        window,
        reported(600_000),
        "a larger window than its own is taken"
    );
    // The largest the catalogue allows the model is the window the harness holds an override to.
    let largest = rig
        .adapter
        .discover_models()
        .await
        .unwrap()
        .into_iter()
        .find(|found| found.model_name == model)
        .and_then(|found| found.parameters.offered)
        .and_then(|offered| offered.context_max)
        .expect("the catalogue allows the model a window larger than its own");
    assert_eq!(largest, 872_000);
    // A million, with the threshold of 900 000 the adapter writes out for it (which the harness holds to nine tenths of the window
    // it works with), is held to that largest: the harness reports 95 % of it, which is 828 400.
    let million = json!({ "context": 1_000_000, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    let (window, _) = window_reported(&mut rig, new_id(), model, &million, None).await;
    assert_eq!(
        window,
        reported(largest),
        "a million is held to the largest the catalogue gives the model, and the harness reports 95 % of that"
    );
    assert_eq!(window, 828_400);
    rig.adapter.stop().await.unwrap();
}

/// Runs two turns on a new session of `model` with `params`, the first of which reports `tokens` of prompt, and says whether
/// the harness shortened the conversation on the way.
async fn shortened_after(rig: &mut Rig, model: &str, params: &Value, tokens: i64) -> bool {
    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.settings.model = model.to_owned();
    spec.settings.params = params.clone();
    rig.adapter.open_session(spec).await.unwrap();
    let mut events = Vec::new();
    for text in [
        common::script("go", &["say hello", &format!("usage {tokens} 10")]),
        common::script("again", &["say again", "usage 5000 10"]),
    ] {
        let mut turn = rig.turn(thread, &text);
        turn.model = model.to_owned();
        turn.params = params.clone();
        rig.adapter.start_turn(turn).await.unwrap();
        events.extend(rig.until_turn_ends().await);
    }
    events.iter().any(|event| {
        matches!(event, SessionEvent::TurnPhase(phase) if phase.kind == TurnPhaseKind::Compacting)
    })
}

#[tokio::test]
async fn the_threshold_written_out_for_a_raised_window_is_held_to_nine_tenths_of_the_window_the_harness_works_with()
 {
    let Some(mut rig) = Rig::start("context-threshold").await else {
        return;
    };
    rig.boot().await;
    // A million is held to the 872 000 the catalogue allows the model, and the 900 000 written out for it to nine tenths
    // of that, 784 800: a conversation of 700 000 is let be, and one of 800 000 is shortened before the next turn.
    let model = "gpt-6-sol";
    let million = json!({ "context": 1_000_000, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    assert!(!shortened_after(&mut rig, model, &million, 700_000).await);
    assert!(shortened_after(&mut rig, model, &million, 800_000).await);
    // A model the harness does not know is held to its 272 000, and the threshold to nine tenths of that, 244 800.
    assert!(!shortened_after(&mut rig, common::MODEL, &million, 200_000).await);
    assert!(shortened_after(&mut rig, common::MODEL, &million, 250_000).await);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_session_resumed_takes_the_context_it_is_given_then() {
    let Some(mut rig) = Rig::start("context-resume").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    let first = json!({ "context": 100_000, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    let (window, native) = window_reported(&mut rig, thread, common::MODEL, &first, None).await;
    assert_eq!(window, reported(100_000));
    rig.adapter.close_session(thread).await.unwrap();
    rig.adapter.stop().await.unwrap();
    rig.drain();

    // The same conversation in a new process, opened with another context: `thread/resume` takes the configuration too.
    rig.boot().await;
    let second = json!({ "context": 200_000, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    let (window, again) =
        window_reported(&mut rig, thread, common::MODEL, &second, Some(&native)).await;
    assert_eq!(again, native, "the conversation is the same one");
    assert_eq!(
        window,
        reported(200_000),
        "the window of the new configuration"
    );
    rig.adapter.stop().await.unwrap();
}
