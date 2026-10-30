//! The harness through a forwarder that adds a key: the plumbing against the scripted server, and one check against a real model service for a person who has a key.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;

use common::forwarder::Forwarder;
use common::{Rig, outcome, said, tool_calls};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::session::SessionMode;

/// The service of the real check and the model on it.
const REAL_UPSTREAM: &str = "https://openrouter.ai/api";
const REAL_MODEL: &str = "anthropic/claude-haiku-5.5";

/// The harness never sees the key; this is what it is given instead of an upstream's address.
fn quick() -> BTreeMap<String, String> {
    BTreeMap::from([("CLAUDE_CODE_MAX_RETRIES".to_owned(), "1".to_owned())])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forwarder_that_adds_the_key_passes_the_streamed_replies_and_the_harness_holds_no_key() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    // The scripted server insists on the key, which only the forwarder presents.
    let forwarder = Forwarder::start(&rig.model.url(), common::TOKEN).await;
    rig.start_through(quick(), forwarder.loopback()).await;
    rig.open().await;
    let script = "#!mock\nsay Let me run a command.\ntool Bash {\"command\":\"echo hello > out.txt && cat out.txt\",\"description\":\"Write a file\"}\nthen\nsay It said: {{result}}\n";
    let (_, events) = rig.run_deciding(script, common::allow).await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert!(said(&events).contains("hello"));
    assert!(forwarder.requests() >= 2, "both rounds went through it");
    assert_eq!(forwarder.refused(), 0);
    let seen = rig.model.requests();
    assert!(!seen.is_empty());
    assert!(
        seen.iter()
            .all(|request| request.credential.verdict
                == orch_testkit::doubles::model::Verdict::Accepted),
        "the server saw the key the forwarder added"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "talks to a real model service and costs a few cents; needs OPENROUTER_API_KEY in the environment"]
async fn a_real_model_works_a_tool_and_remembers_it_in_a_new_process() {
    let Ok(key) = std::env::var("OPENROUTER_API_KEY") else {
        eprintln!("skipped: OPENROUTER_API_KEY is not set");
        return;
    };
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    let forwarder = Forwarder::start(REAL_UPSTREAM, &key).await;
    drop(key);
    rig.start_through(quick(), forwarder.loopback()).await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.model = REAL_MODEL.into();
    let opened = rig.open_with(spec.clone()).await;

    let mut first = rig.turn(
        "Use the Bash tool to run exactly this command: echo orch-real-check-$((6*7)) \
         and then answer with the single word done.",
    );
    first.model = REAL_MODEL.into();
    let id = first.turn_id;
    rig.adapter.start_turn(first).await.unwrap();
    let events = rig.deciding_to_the_end(&mut common::allow).await;
    let end = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::TurnCompleted(end) if end.turn_id == id => Some(end),
            _ => None,
        })
        .expect("the turn ended");
    assert_eq!(end.status, TurnOutcome::Completed, "{:?}", end.error);
    let commands: Vec<_> = tool_calls(&events).into_iter().collect();
    assert!(!commands.is_empty(), "the model used its tool");
    let output = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(done) => done.payload["tool"]["output"]["inline"].as_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        output.contains("orch-real-check-42"),
        "the command ran: {output:?}"
    );
    let usage = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::UsageUpdated(usage) => Some(usage.clone()),
            _ => None,
        })
        .expect("the use of the turn is reported");
    assert!(usage.turn.total_tokens > 0 && usage.turn.is_consistent());
    eprintln!(
        "first turn: {} events, answer {:?}, {} tokens, {} requests so far",
        events.len(),
        said(&events),
        usage.turn.total_tokens,
        forwarder.requests()
    );

    // A new process, the same conversation.
    rig.adapter.close_session(rig.thread).await.unwrap();
    let mut again = rig.spec(SessionMode::Resume, Some(&opened.native_session_ref));
    again.settings.model = REAL_MODEL.into();
    rig.open_with(again).await;
    let mut second =
        rig.turn("What number did the command print after the dash? Answer with the number only.");
    second.model = REAL_MODEL.into();
    rig.adapter.start_turn(second).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let answer = said(&events);
    eprintln!(
        "second turn answer: {answer:?}; {} requests in all",
        forwarder.requests()
    );
    assert!(
        answer.contains("42"),
        "it remembers what the command printed: {answer:?}"
    );
    assert_eq!(forwarder.refused(), 0);
    rig.adapter.stop().await.unwrap();
}
