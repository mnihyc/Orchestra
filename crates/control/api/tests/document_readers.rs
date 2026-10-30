//! The people who have a document open, as the engine asks for them before an agent deletes or moves it: those who said the document is in front of them, each counted once however many windows they have it in, and nobody for a document that only follows.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_core::registry::Registry;
use orch_api_core::testing::TestApp;
use orch_engine::live::LiveSink;
use orch_testkit::clock;
use orch_types::Id;
use realtime_support::{Client, Rig};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Waits for the next frame of `op` among those that come.
async fn next_of(client: &mut Client, op: &str) -> Value {
    loop {
        let frame = client.frame().await;
        if frame["op"] == op {
            return frame;
        }
    }
}

/// Follows a document and takes the answer and the list of who is looking.
async fn follow(client: &mut Client, document: Id) {
    client.sub(1, &format!("doc:{document}"), None).await;
    client.expect("subscribed").await;
    client.expect("presence").await;
}

/// Waits until the engine would be told `people`, and says what it is told then.
async fn readers_become(sink: &Arc<dyn LiveSink>, document: Id, people: usize) -> usize {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let now = sink.document_readers(document);
        if now == people || Instant::now() > until {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_is_told_how_many_people_have_a_document_open_each_counted_once_and_none_who_left()
 {
    let rig = Rig::new().await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(rig.server.app.clone(), clock::manual(), registry);
    let document = app
        .call(NodeDocumentCreate, &[&rig.topic])
        .token(&rig.token)
        .body(&NodeDocumentCreateBody {
            title: "Plan".to_owned(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id;
    let (vera, vera_token) = rig.person("vera").await;
    rig.give(vera, "viewer").await;
    let sink = rig.server.realtime.sink();
    let feed = format!("doc:{document}");
    assert_eq!(sink.document_readers(document), 0, "nobody has it open yet");

    // Ada has it in two windows and Vera in one; all three follow it.
    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut ada_again = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut vera = Client::signed_in(rig.server.addr, &vera_token).await;
    for client in [&mut ada, &mut ada_again, &mut vera] {
        follow(client, document).await;
    }
    assert_eq!(
        sink.document_readers(document),
        0,
        "following a document is not having it in front of one"
    );

    ada.send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    next_of(&mut ada, "presence").await;
    assert_eq!(readers_become(&sink, document, 1).await, 1);
    ada_again
        .send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    vera.send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    assert_eq!(readers_become(&sink, document, 2).await, 2);
    let _ = ada_again.ping().await;
    let _ = vera.ping().await;
    assert_eq!(
        sink.document_readers(document),
        2,
        "a person with it open in two windows is one person"
    );

    // One of Ada's windows turns away, and she still has it open in the other.
    ada.send(json!({ "op": "visible", "feed": feed, "visible": false }))
        .await;
    let _ = ada.ping().await;
    assert_eq!(sink.document_readers(document), 2);
    ada_again
        .send(json!({ "op": "visible", "feed": feed, "visible": false }))
        .await;
    assert_eq!(readers_become(&sink, document, 1).await, 1);

    // Vera closes the window: nobody has it open.
    drop(vera);
    assert_eq!(readers_become(&sink, document, 0).await, 0);
}
