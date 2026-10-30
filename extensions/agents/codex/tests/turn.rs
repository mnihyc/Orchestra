//! A session is opened on the real Codex in Docker and a turn is run through the scripted model server.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, kinds, session_events};
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::descriptor::HarnessSupport;
use orch_agent_contract::events::{
    AgentMessagePayload, DeltaField, ItemContent, ItemStatus, ReasoningPayload, SessionEvent,
    TurnOutcome,
};
use orch_agent_contract::facts::HarnessState;
use orch_types::new_id;

#[tokio::test]
async fn a_turn_of_plain_text_streams_from_the_real_harness_into_neutral_events() {
    let Some(mut rig) = Rig::start("plain").await else {
        return;
    };
    let started = rig.boot().await;
    assert_eq!(
        started.descriptor.harness.status,
        HarnessSupport::Supported,
        "{}",
        started.harness_version
    );
    assert_eq!(started.descriptor.kind, "codex");
    assert!(matches!(
        rig.next_event().await,
        AdapterEvent::Status(status) if status.status == HarnessState::Online
            && status.harness_version.as_deref() == Some(started.harness_version.as_str())
    ));

    let thread = new_id();
    let opened = rig.open_new(thread).await;
    assert!(!opened.native_session_ref.is_empty() && !opened.resumed);
    assert!(matches!(
        rig.next_event().await,
        AdapterEvent::Session { event: SessionEvent::SessionOpened(event), thread_id, .. }
            if event.native_session_ref == opened.native_session_ref && !event.resumed && thread_id == thread
    ));

    let turn = rig.turn(thread, "hello there");
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_turn_ends().await;

    assert_eq!(
        events.first().map(SessionEvent::kind),
        Some("turn.started"),
        "{:?}",
        kinds(&events)
    );
    let SessionEvent::TurnStarted(began) = &events[0] else {
        panic!("the turn began");
    };
    assert_eq!(began.turn_id, turn_id);
    assert!(!began.native_turn_ref.is_empty());

    // What the person said is the engine's to write: the harness's echo of it is not reported.
    let started_items: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(item) => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(started_items.len(), 1, "{:?}", kinds(&events));
    assert!(
        matches!(&started_items[0].content, ItemContent::AgentMessage(message) if message.text.is_empty())
    );
    assert_eq!(started_items[0].stream, "main");
    assert_eq!(started_items[0].turn_id, turn_id);
    let reference = started_items[0].native_ref.clone();

    // The text arrives in pieces whose offsets continue each other, and the pieces make the final text.
    let mut said = String::new();
    let mut offset = 0;
    for event in &events {
        if let SessionEvent::ItemDelta(delta) = event {
            assert_eq!(
                (delta.native_ref.as_str(), delta.field),
                (reference.as_str(), DeltaField::Text)
            );
            assert_eq!(delta.offset, offset);
            offset += delta.len;
            said.push_str(&delta.data);
        }
    }
    assert_eq!(said, "You said: hello there");
    let completed = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) => Some(item),
            _ => None,
        })
        .expect("the message completes");
    assert_eq!(
        (completed.native_ref.as_str(), completed.status),
        (reference.as_str(), ItemStatus::Completed)
    );
    let message: AgentMessagePayload = serde_json::from_value(completed.payload.clone()).unwrap();
    assert_eq!(
        (message.text.as_str(), message.text_len),
        ("You said: hello there", 21)
    );

    let usage = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::UsageUpdated(usage) => Some(usage),
            _ => None,
        })
        .expect("the use is reported");
    assert!(usage.turn.is_consistent() && usage.turn.total_tokens > 0);
    assert_eq!(usage.turn, usage.thread);
    assert!(usage.context_window.is_some_and(|window| window > 0) && usage.context_used.is_some());

    let Some(SessionEvent::TurnCompleted(ended)) = events.last() else {
        panic!("the turn ends the events");
    };
    assert_eq!(
        (ended.turn_id, ended.status),
        (turn_id, TurnOutcome::Completed)
    );
    assert!(ended.error.is_none());

    // The model was reached under the session's key, with the token from the environment and not from an
    // argument, and was told what Orch tells the agent.
    let requests = rig.model_requests();
    assert_eq!(requests.len(), 1);
    let everything: Vec<_> = rig
        .mock
        .requests()
        .into_iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect();
    assert_eq!(
        everything.len(),
        1,
        "the harness asks the forwarder for nothing but the response: {everything:?}"
    );
    let request = &requests[0];
    // What the forwarder is asked for: a streamed response, as plain JSON, for a session and a thread.
    assert_eq!(request.method, "POST");
    assert_eq!(request.headers["accept"], "text/event-stream");
    assert_eq!(request.headers["content-type"], "application/json");
    assert!(!request.headers.contains_key("content-encoding"));
    assert_eq!(request.headers["originator"], "orch");
    assert!(
        request.headers.contains_key("session-id") && request.headers.contains_key("thread-id")
    );
    assert!(request.stream);
    assert!(
        request.path.starts_with("/llm/") && request.path.ends_with("/v1/responses"),
        "{}",
        request.path
    );
    assert_eq!(
        request.credential.verdict,
        orch_testkit::doubles::model::Verdict::Accepted
    );
    assert_eq!(request.model.as_deref(), Some("mock-large"));
    assert!(
        request
            .body
            .to_string()
            .contains("You are working inside Orch.")
    );
    let launched = rig.launcher.launched.lock().unwrap().clone();
    assert!(
        launched
            .iter()
            .all(|launch| launch.args.iter().all(|arg| !arg.contains(&rig.token))),
        "the token is in no argument"
    );
    assert_eq!(
        common::env_of(&launched[0]).get("ORCH_LLM_TOKEN"),
        Some(&rig.token)
    );

    rig.adapter.close_session(thread).await.unwrap();
    rig.adapter.stop().await.unwrap();
    let rest = session_events(&rig.drain());
    assert!(
        rest.iter().all(|event| event.kind() != "session.lost"),
        "{:?}",
        kinds(&rest)
    );
}

#[tokio::test]
async fn the_reasoning_of_a_model_is_streamed_as_a_summary_before_what_it_says() {
    let Some(mut rig) = Rig::start("think").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    let text = common::script(
        "work it out",
        &["think weighing the options", "say The answer is 42"],
    );
    let events = rig.run_turn(thread, &text).await;
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(item) => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 2, "{:?}", kinds(&events));
    assert!(matches!(started[0].content, ItemContent::Reasoning(_)));
    assert!(matches!(started[1].content, ItemContent::AgentMessage(_)));

    // The summary arrives in pieces that continue each other, and the pieces make the summary.
    let reasoning = started[0].native_ref.clone();
    let mut summary = String::new();
    let mut offset = 0;
    for event in &events {
        if let SessionEvent::ItemDelta(delta) = event
            && delta.native_ref == reasoning
        {
            assert_eq!(delta.field, DeltaField::Summary);
            assert_eq!(delta.offset, offset);
            offset += delta.len;
            summary.push_str(&delta.data);
        }
    }
    assert_eq!(summary, "weighing the options");
    let done = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.native_ref == reasoning => Some(item),
            _ => None,
        })
        .expect("the reasoning completes");
    let payload: ReasoningPayload = serde_json::from_value(done.payload.clone()).unwrap();
    assert_eq!(payload.summary, "weighing the options");
    assert_eq!(payload.text_len, 20);
    assert!(!payload.hidden);
    let spoken = events.iter().find_map(|event| match event {
        SessionEvent::ItemCompleted(item) if item.native_ref == started[1].native_ref => Some(item),
        _ => None,
    });
    assert_eq!(spoken.unwrap().payload["text"], "The answer is 42");
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );
    rig.adapter.stop().await.unwrap();
}
