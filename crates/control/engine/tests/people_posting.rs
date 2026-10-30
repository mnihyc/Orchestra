//! Successful agent posts share a durable origin-wide rolling-hour bound, and human local stops narrow every share.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use orch_types::{Clock, Id, ManualClock};
use serde_json::json;
use shared::{call, call_as, chat_of, entry, ok, refused, share, the_agent_takes_tools_again};
use std::sync::Arc;

struct Setup {
    world: common::World,
    clock: Arc<ManualClock>,
    origin: Id,
    topic: Id,
    chats: [Id; 2],
}

async fn setup() -> Setup {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = common::World::with_clock(clock.clone()).await;
    the_agent_takes_tools_again(&world).await;
    let origin = world.thread("Writer").await;
    let topic = world.topic("First room").await;
    let other = world.topic("Second room").await;
    let chats = [chat_of(&world, topic).await, chat_of(&world, other).await];
    for node in [topic, other] {
        share(&world, "node", node, "live", "chat.interact", "any", origin).await;
    }
    Setup {
        world,
        clock,
        origin,
        topic,
        chats,
    }
}

async fn posts(s: &Setup) -> i64 {
    s.world
        .count(
            "SELECT count(*) FROM audit_log WHERE action='chat.posted_by_agent' AND actor_id=$1",
            s.origin,
        )
        .await
}

#[tokio::test]
async fn a_repeat_and_a_refusal_count_zero_and_the_hourly_bound_survives_a_process_restart() {
    let s = setup().await;
    s.world
        .set(s.origin, "broadcast.chat_messages_per_hour", json!(1))
        .await;
    let invalid = call(
        &s.world,
        s.origin,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[0]) }, "text": "" }),
    )
    .await;
    assert!(invalid.error.is_some());
    assert_eq!(posts(&s).await, 0);
    let args = json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[0]) }, "text": "One accepted post." });
    ok(call_as(
        &s.world,
        s.origin,
        OrchTool::SendMessage,
        args.clone(),
        "same-post",
    )
    .await);
    ok(call_as(&s.world, s.origin, OrchTool::SendMessage, args, "same-post").await);
    assert_eq!(posts(&s).await, 1);
    s.world.restart_api().await;
    let denied = call(&s.world, s.origin, OrchTool::SendMessage, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[1]) }, "text": "Different destination." })).await;
    let (code, message) = refused(&denied);
    assert_eq!(code, "rate_limited");
    assert!(message.contains(&(s.clock.now() + chrono::Duration::hours(1)).to_rfc3339()));
    assert_eq!(posts(&s).await, 1);
    s.clock.advance(chrono::Duration::hours(1));
    ok(call(&s.world, s.origin, OrchTool::SendMessage, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[1]) }, "text": "At the rolling boundary." })).await);
    assert_eq!(posts(&s).await, 2);
}

#[tokio::test]
async fn concurrent_destinations_cannot_both_take_the_last_origin_slot() {
    let s = setup().await;
    s.world
        .set(s.origin, "broadcast.chat_messages_per_hour", json!(1))
        .await;
    let first = json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[0]) }, "text": "First" });
    let second = json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[1]) }, "text": "Second" });
    let (one, two) = tokio::join!(
        call_as(
            &s.world,
            s.origin,
            OrchTool::SendMessage,
            first,
            "first-room"
        ),
        call_as(
            &s.world,
            s.origin,
            OrchTool::SendMessage,
            second,
            "second-room"
        )
    );
    assert_eq!(
        [&one, &two]
            .into_iter()
            .filter(|reply| reply.error.is_none())
            .count(),
        1
    );
    assert_eq!(posts(&s).await, 1);
    let failed = if one.error.is_some() { &one } else { &two };
    assert_eq!(refused(failed).0, "rate_limited");
}

#[tokio::test]
async fn a_local_stop_preserves_reading_and_defeats_overlapping_shares_but_allow_grants_nothing() {
    let s = setup().await;
    // A second independent source would still write without a central local stop.
    let second = share(
        &s.world,
        "node",
        s.topic,
        "live",
        "chat.interact",
        "any",
        s.world.root,
    )
    .await;
    {
        let mut tx = s.world.pool.begin().await.unwrap();
        orch_db::queries::threads::lock(&mut tx, s.origin)
            .await
            .unwrap()
            .unwrap();
        orch_db::queries::chats::posting::set(
            &mut tx,
            s.chats[0],
            s.origin,
            true,
            s.world.ada,
            s.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let listing = ok(call(
        &s.world,
        s.origin,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", s.topic) }),
    )
    .await);
    assert_eq!(entry(&listing, "chat.md")["readable"], true);
    assert_eq!(entry(&listing, "chat.md")["writable"], false);
    let denied = call(&s.world, s.origin, OrchTool::SendMessage, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[0]) }, "text": "Stopped" })).await;
    assert_eq!(refused(&denied).0, "closed");
    assert_eq!(posts(&s).await, 0);
    ok(call(&s.world, s.origin, OrchTool::SendMessage, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[1]) }, "text": "Other room still works" })).await);
    {
        let mut tx = s.world.pool.begin().await.unwrap();
        orch_db::queries::threads::lock(&mut tx, s.origin)
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE object_kind='node' AND object_id=$1 AND holder_kind='agents' AND permission='chat.interact'")
            .bind(s.topic).bind(s.clock.now()).execute(&mut *tx).await.unwrap();
        orch_db::queries::chats::posting::set(
            &mut tx,
            s.chats[0],
            s.origin,
            false,
            s.world.ada,
            s.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let _ = second;
    let denied = call(&s.world, s.origin, OrchTool::SendMessage, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[0]) }, "text": "Allow alone gives no share" })).await;
    assert!(denied.error.is_some());
    assert_eq!(posts(&s).await, 1);
}

#[tokio::test]
async fn a_lowered_limit_waits_for_enough_old_posts_and_zero_disables_posting() {
    let s = setup().await;
    s.world
        .set(s.origin, "broadcast.chat_messages_per_hour", json!(3))
        .await;
    for index in 0..3 {
        ok(call(&s.world, s.origin, OrchTool::SendMessage, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[0]) }, "text": format!("Post {index}") })).await);
        s.clock.advance(chrono::Duration::minutes(1));
    }
    s.world
        .set(s.origin, "broadcast.chat_messages_per_hour", json!(1))
        .await;
    let denied = call(&s.world, s.origin, OrchTool::SendMessage, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[0]) }, "text": "Wait" })).await;
    let release =
        orch_testkit::clock::start() + chrono::Duration::hours(1) + chrono::Duration::minutes(2);
    assert!(refused(&denied).1.contains(&release.to_rfc3339()));
    s.world
        .set(s.origin, "broadcast.chat_messages_per_hour", json!(0))
        .await;
    s.clock.advance(chrono::Duration::hours(2));
    let denied = call(&s.world, s.origin, OrchTool::SendMessage, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chats[0]) }, "text": "Disabled" })).await;
    assert_eq!(refused(&denied).0, "rate_limited");
    assert_eq!(posts(&s).await, 3);
}
