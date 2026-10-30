//! Side threads do not become implicit shared-tree siblings when addressed by identifier.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_db::queries::hierarchy::testing::insert_node;
use orch_link_contract::commands::OrchTool;
use orch_types::new_id;
use serde_json::json;
use shared::*;

#[tokio::test]
async fn another_agents_side_thread_is_not_listed_or_searched_by_identifier() {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let parent = world.thread("Main review").await;
    let side = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    insert_node(
        &mut conn,
        side,
        "thread",
        Some(parent),
        "Private aside title",
        world.ada,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET topic_id=$2,side_of=$3 WHERE node_id=$1")
        .bind(side)
        .bind(world.topic)
        .bind(parent)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE nodes SET description='Private aside description' WHERE id=$1")
        .bind(side)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let listing = ok(call(
        &world,
        reader,
        OrchTool::ListShared,
        json!({"path":"/","depth":4}),
    )
    .await);
    assert!(!listing.to_string().contains("Private aside"));
    let path = format!("id:{side}");
    let direct = call(
        &world,
        reader,
        OrchTool::ListShared,
        json!({"path":path.clone()}),
    )
    .await;
    assert_eq!(refused(&direct).0, "not_found");
    assert!(!refused(&direct).1.contains("Private aside"));
    let search = call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"path":path,"query":"Private aside","regex":true,"snippets":true}),
    )
    .await;
    assert_eq!(refused(&search).0, "not_found");
    assert!(!refused(&search).1.contains("Private aside"));
}
