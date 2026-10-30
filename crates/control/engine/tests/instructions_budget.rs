//! The standing instructions a thread is told, with the headings of their places, stay within their limit: a change
//! that would pass it is found before it is kept, and no thread is made where it is passed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{MODEL, World};
use orch_authz::Caller;
use orch_engine::binding::{CreateThreadError, NewThread};
use orch_engine::sessions::{STANDING_LIMIT, standing_characters, standing_past_limit};
use serde_json::json;

/// What each place says: a place may say 20,000 characters, so that three of these pass the limit together and two
/// do not.
const SAID: usize = 15_000;

#[tokio::test]
async fn instructions_budget_counts_each_place_with_its_heading_and_refuses_a_thread_past_the_limit()
 {
    let world = World::new().await;
    world
        .set(world.root, "agent.instructions", json!("o".repeat(SAID)))
        .await;
    world
        .set(world.project, "agent.instructions", json!("p".repeat(SAID)))
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let two = standing_characters(&mut conn, world.topic).await.unwrap();
    assert!(
        two > 2 * SAID && two < STANDING_LIMIT,
        "two places and their headings: {two}"
    );
    assert_eq!(
        standing_past_limit(&mut conn, world.root).await.unwrap(),
        None
    );
    drop(conn);

    // A third place passes it: the change is found wherever it was made above the topic, and the count says how far.
    world
        .set(world.topic, "agent.instructions", json!("t".repeat(SAID)))
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let three = standing_characters(&mut conn, world.topic).await.unwrap();
    assert!(three > STANDING_LIMIT, "{three}");
    assert_eq!(
        standing_past_limit(&mut conn, world.topic).await.unwrap(),
        Some(three)
    );
    assert_eq!(
        standing_past_limit(&mut conn, world.root).await.unwrap(),
        Some(three)
    );
    drop(conn);

    // No thread is made there: its first turn would meet what it may not be told.
    let refused = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            NewThread {
                topic: world.topic,
                name: "Too much to be told".to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: None,
                model: Some(MODEL.to_owned()),
                effort: None,
            },
        )
        .await;
    assert!(
        matches!(refused, Err(CreateThreadError::InstructionsTooLarge(characters)) if characters == three),
        "{refused:?}"
    );
}
