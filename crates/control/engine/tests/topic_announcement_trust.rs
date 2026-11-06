//! Every announcement recipient requires the same current agent grant as a direct send.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use common::World;
use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::{Value, json};
use shared::{
    a_world_whose_agent_takes_tools, call, ok, prepare_call_turn, share, thread_in_topic,
};

/// Observe every persisted item/input and pending notice, including the thread's creation marker.
async fn delivery_counts(world: &World, thread: Id) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM items WHERE thread_id=$1),
                (SELECT count(*) FROM thread_inputs WHERE thread_id=$1),
                jsonb_array_length(pending_notices)::bigint
           FROM threads WHERE node_id=$1",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap()
}

async fn announce(world: &World, sender: Id, title: &str, audience: &str) -> Value {
    ok(call(
        world,
        sender,
        OrchTool::Announce,
        json!({"title":title,"audience":audience}),
    )
    .await)
}

#[tokio::test]
async fn a_topic_notice_requires_a_current_agent_grant() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Sender").await;
    let receiver = world.thread("Receiver").await;
    let side = world.thread("Aside").await;
    sqlx::query("UPDATE threads SET side_of=$2 WHERE node_id=$1")
        .bind(side)
        .bind(receiver)
        .execute(world.pool.pool())
        .await
        .unwrap();
    prepare_call_turn(&world, sender).await;
    let before = delivery_counts(&world, receiver).await;
    let side_before = delivery_counts(&world, side).await;

    let off = announce(&world, sender, "Default messages are off", "topic").await;
    assert_eq!(
        delivery_counts(&world, receiver).await,
        before,
        "default-off topic messaging must leave every recipient item, input and notice unchanged"
    );
    assert_eq!(off["delivered"]["deferred"], 0);
    assert_eq!(delivery_counts(&world, side).await, side_before);

    let grant = share(
        &world,
        "node",
        receiver,
        "live",
        "thread.interact",
        "any",
        sender,
    )
    .await;
    let shared = announce(&world, sender, "Explicitly shared collaboration", "topic").await;
    assert_eq!(shared["delivered"]["deferred"], 1);
    let twice = (before.0 + 1, before.1, before.2 + 1);
    assert_eq!(delivery_counts(&world, receiver).await, twice);
    assert_eq!(delivery_counts(&world, side).await, side_before);

    orch_db::queries::access::remove_entry(
        &mut world.pool.acquire().await.unwrap(),
        grant,
        world.ada,
        world.clock.now(),
    )
    .await
    .unwrap();
    let revoked = announce(&world, sender, "The agent grant was revoked", "topic").await;
    assert_eq!(revoked["delivered"]["deferred"], 0);
    assert_eq!(delivery_counts(&world, receiver).await, twice);
}

#[tokio::test]
async fn every_project_recipient_needs_its_own_current_agent_right() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Sender").await;
    let sibling = world.thread("Sibling").await;
    let other_topic = world.topic("Related work").await;
    let other = thread_in_topic(&world, other_topic, "Project recipient").await;
    prepare_call_turn(&world, sender).await;
    let sibling_before = delivery_counts(&world, sibling).await;
    let other_before = delivery_counts(&world, other).await;
    let off = announce(&world, sender, "No recipients lent Send", "project").await;
    assert_eq!(off["delivered"]["deferred"], 0);
    assert_eq!(delivery_counts(&world, sibling).await, sibling_before);
    assert_eq!(delivery_counts(&world, other).await, other_before);
    let foreign_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM announcements WHERE topic_id=$1")
            .bind(other_topic)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        foreign_rows, 0,
        "an audience alone must not post into an unauthorized topic"
    );

    // View alone still permits no direct or broadcast control.
    share(&world, "node", other, "live", "thread.view", "any", sender).await;
    let view = announce(&world, sender, "View is not Send", "project").await;
    assert_eq!(view["delivered"]["deferred"], 0);
    assert_eq!(delivery_counts(&world, other).await, other_before);

    let grant = share(
        &world,
        "node",
        other,
        "live",
        "thread.interact",
        "any",
        sender,
    )
    .await;
    let sent = announce(&world, sender, "Only the authorized recipient", "project").await;
    assert_eq!(sent["delivered"]["deferred"], 1);
    let once = (other_before.0 + 1, other_before.1, other_before.2 + 1);
    assert_eq!(delivery_counts(&world, other).await, once);
    assert_eq!(delivery_counts(&world, sibling).await, sibling_before);

    orch_db::queries::access::remove_entry(
        &mut world.pool.acquire().await.unwrap(),
        grant,
        world.ada,
        world.clock.now(),
    )
    .await
    .unwrap();
    let revoked = announce(&world, sender, "Revoked Send stops broadcasts", "project").await;
    assert_eq!(revoked["delivered"]["deferred"], 0);
    assert_eq!(delivery_counts(&world, other).await, once);
}
