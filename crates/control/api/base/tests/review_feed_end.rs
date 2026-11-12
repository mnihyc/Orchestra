//! The end of an entry closes the feed it gave before further content is sent.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use chrono::Duration;
use orch_db::queries::access::{PageKey, testing};
use realtime_support::{Client, Rig};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entry_that_ends_closes_its_feed_without_waiting_for_another_page_change() {
    let rig = Rig::with_time_held().await;
    let thread = rig.thread("Reading").await;
    let turn = rig.send(thread, "Before the entry ended").await;
    let (reader, token) = rig.person("reader").await;
    {
        let mut conn = rig.pool.acquire().await.unwrap();
        testing::add_entry(
            &mut conn,
            &PageKey::new("node", thread, "live"),
            "any",
            "thread.view",
            ("user", Some(reader), None),
            Some(rig.server.app.clock.now() + Duration::seconds(1)),
            rig.server.app.clock.now(),
        )
        .await
        .unwrap();
    }
    rig.bump_epoch().await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.sub(1, &format!("thread:{thread}"), None).await;
    client.expect("subscribed").await;
    client.expect("presence").await;
    rig.let_time_pass(Duration::seconds(2));
    {
        let mut conn = rig.pool.acquire().await.unwrap();
        let person = orch_authz::Principal::load(
            &mut conn,
            orch_authz::Caller::session(reader),
            orch_types::Time::new(rig.server.app.clock.now()),
        )
        .await
        .unwrap();
        let decision = orch_authz::nodes::check(
            &mut conn,
            &person,
            thread,
            orch_access_contract::permissions::Permission::ThreadView,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        .unwrap();
        assert!(
            !decision.is_allowed(),
            "the entry has ended for an ordinary read too"
        );
    }
    rig.begin_message(thread, &turn, "after-end").await;
    rig.finish_turn(thread, &turn, "after-end", "After the entry ended")
        .await;
    loop {
        let frame = client.frame().await;
        if matches!(frame["op"].as_str(), Some("presence" | "pong")) {
            continue;
        }
        if frame["op"] == "closed" {
            assert_eq!(frame["reason"], "revoked");
            break;
        }
        // Frames already prepared while the entry counted may be in the socket. The text
        // below was written after its end, and must never be sent under the old decision.
        assert!(
            !serde_json::to_string(&frame)
                .unwrap()
                .contains("After the entry ended"),
            "an ended entry gives no content written after its end: {frame}",
        );
    }
    rig.server.stop().await;
}
