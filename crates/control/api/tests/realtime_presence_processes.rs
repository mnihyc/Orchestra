//! Presence merges one person's visible tabs across two API processes and removes them only after the last tab leaves.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use realtime_support::{Client, Rig, Server, quick};
use serde_json::json;

async fn readers(client: &mut Client, expected: usize) -> serde_json::Value {
    loop {
        let frame = client.frame().await;
        if frame["op"] == "presence"
            && frame["users"]
                .as_array()
                .is_some_and(|users| users.len() == expected)
        {
            return frame;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_person_in_two_api_processes_is_one_reader_and_leaves_after_the_last_visible_tab() {
    let rig = Rig::new().await;
    let thread = rig.thread("Seen together").await;
    let (bea, bea_token) = rig.person("bea").await;
    rig.give(bea, "viewer").await;
    let second = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    let feed = format!("thread:{thread}");
    let mut one = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut two = Client::signed_in(second.addr, &rig.token).await;
    let mut observer = Client::signed_in(second.addr, &bea_token).await;
    for client in [&mut one, &mut two, &mut observer] {
        client.sub(1, &feed, None).await;
        client.expect("subscribed").await;
        client.expect("presence").await;
    }
    one.send(json!({"op": "visible", "feed": feed, "visible": true}))
        .await;
    two.send(json!({"op": "visible", "feed": feed, "visible": true}))
        .await;
    for client in [&mut one, &mut two, &mut observer] {
        let frame = readers(client, 1).await;
        assert_eq!(frame["users"][0]["user"]["id"], rig.ada.to_string());
    }
    assert!(
        rig.server
            .realtime
            .sink()
            .thread_is_visible(thread, rig.ada)
    );
    assert!(
        second.realtime.sink().thread_is_visible(thread, rig.ada),
        "the notification sink shares the same cross-process presence"
    );
    one.send(json!({"op": "visible", "feed": feed, "visible": false}))
        .await;
    one.ping().await;
    realtime_support::eventually(
        "the remaining tab on the other process still counts as watching",
        || async {
            rig.server
                .realtime
                .sink()
                .thread_is_visible(thread, rig.ada)
        },
    )
    .await;
    two.send(json!({"op": "visible", "feed": feed, "visible": false}))
        .await;
    readers(&mut observer, 0).await;
    assert!(
        !second.realtime.sink().thread_is_visible(thread, rig.ada),
        "only the last visible tab leaving ends watching"
    );
    second.stop().await;
    rig.server.stop().await;
}
