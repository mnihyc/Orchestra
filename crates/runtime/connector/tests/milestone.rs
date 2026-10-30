//! The first thing the connector has to get right: a connector on the in-memory hub hosting the scripted adapter, one scripted turn streamed to the scripted controller.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::Rig;
use orch_link_contract::commands::SerialKey;
use orch_types::new_id;

#[tokio::test]
async fn a_scripted_turn_is_streamed_to_the_system_in_order_and_ends() {
    let mut rig = Rig::new();
    rig.start().await;

    // The harness process, then a session, then a turn that answers the input as the scripted adapter does.
    let started = rig.agent_start(serde_json::json!({})).await;
    assert_eq!(started["descriptor"]["kind"], "fake");
    let (thread, segment) = (new_id(), new_id());
    let opened = rig
        .open_session(rig.open_body(thread, segment, "/tmp/work"))
        .await;
    assert_eq!(opened["resumed"], false);
    assert!(
        opened["native_session_ref"]
            .as_str()
            .unwrap()
            .starts_with("fake_")
    );
    let turn = rig.start_turn(thread, "hello").await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;

    let applied = rig.applied(thread, segment);
    // Each request the turn made of its model is reported beside the items, in the same stream.
    assert!(applied.iter().any(|e| e.kind == "model.request"));
    let kinds: Vec<&str> = applied
        .iter()
        .map(|e| e.kind.as_str())
        .filter(|kind| *kind != "model.request")
        .collect();
    assert_eq!(
        kinds,
        [
            "session.opened",
            "turn.started",
            "item.started",
            "item.delta",
            "item.delta",
            "item.delta",
            "item.completed",
            "turn.completed"
        ]
    );
    // Every event arrived once, in the order it was made, without a hole.
    for (place, event) in applied.iter().enumerate() {
        assert_eq!(event.seq, place as i64 + 1);
    }
    assert!(
        rig.controller.violations().is_empty(),
        "{:?}",
        rig.controller.violations()
    );
    // What was said is what the input asked for, streamed in pieces that add up to it.
    assert_eq!(rig.messages(thread, segment), ["You said: hello"]);
    let pieces: String = applied
        .iter()
        .filter(|e| e.kind == "item.delta")
        .map(|e| e.body["data"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(pieces, "You said: hello");
    // The turn ended well, and it is the turn that was started.
    let ended = applied.last().unwrap();
    assert_eq!(ended.body["status"], "completed");
    assert_eq!(ended.body["turn_id"], serde_json::json!(turn));
    // The session closes when it is asked to.
    let closed = rig
        .succeed(
            &SerialKey::Thread(thread),
            &orch_link_contract::commands::SessionClose { thread_id: thread },
        )
        .await;
    assert_eq!(closed, serde_json::json!({}));
    rig.connector.take().unwrap().stop().await;
}
