//! A managed schedule is an explicit inherited choice, never an implicitly offered harness tool.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod common;
use common::World;
use orch_agent_contract::events::OrchTool;
use orch_db::queries::threads;
use orch_engine::tools::offered_for_session;
use orch_types::new_id;
use serde_json::json;

#[tokio::test]
async fn the_schedule_tool_is_absent_by_default_and_never_given_to_a_reading_side() {
    let world = World::new().await;
    let thread = world.thread("Managed schedules").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let mut row = threads::thread(&mut conn, thread).await.unwrap().unwrap();
    assert!(
        !offered_for_session(&mut conn, &row, true, world.clock.time())
            .await
            .unwrap()
            .contains(&OrchTool::Schedule)
    );
    drop(conn);
    world
        .set(thread, "threads.agent_schedules", json!("on"))
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        offered_for_session(&mut conn, &row, true, world.clock.time())
            .await
            .unwrap()
            .contains(&OrchTool::Schedule)
    );
    assert!(
        offered_for_session(&mut conn, &row, false, world.clock.time())
            .await
            .unwrap()
            .is_empty()
    );
    row.side_of = Some(new_id());
    row.side_reading = Some("read_only".into());
    assert!(
        !offered_for_session(&mut conn, &row, true, world.clock.time())
            .await
            .unwrap()
            .contains(&OrchTool::Schedule)
    );
}
