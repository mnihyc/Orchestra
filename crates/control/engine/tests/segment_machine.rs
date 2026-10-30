//! A segment keeps the machine that was asked to open its harness session.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::World;
use orch_db::queries::machine_streams;
use orch_types::Id;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_segment_waits_for_its_first_open_and_keeps_that_machine_after_its_environment_moves() {
    let world = World::new().await;
    let thread = world.thread("A session's source").await;
    let segment = world.segment_of(thread).await;
    let source: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM segments WHERE id = $1")
        .bind(segment)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(source, None);
    world.running_turn(thread, "Begin the session").await;
    let source: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM segments WHERE id = $1")
        .bind(segment)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(source, Some(world.machine_id));

    let other = world
        .add_machine("new-place", &["agent", "environment"])
        .await;
    sqlx::query("UPDATE environments SET machine_id = $2 WHERE id = $1")
        .bind(world.environment)
        .bind(other.id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        machine_streams::owns_segment(&mut conn, thread, segment, world.machine_id)
            .await
            .unwrap()
    );
    assert!(
        !machine_streams::bind_segment(&mut conn, thread, segment, other.id)
            .await
            .unwrap()
    );
    assert!(
        !machine_streams::owns_segment(&mut conn, thread, segment, other.id)
            .await
            .unwrap()
    );
}
