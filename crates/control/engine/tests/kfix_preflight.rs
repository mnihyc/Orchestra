//! A policy refusal precedes a lost execution role, and the waiting input runs when both causes clear.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, queued};
use orch_authz::Caller;
use orch_settings_contract::registry::keys;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn policy_refusal_precedes_a_lost_agent_role_and_the_same_input_runs_when_both_clear() {
    let world = World::new().await;
    let thread = world.thread("Waiting for policy and machine").await;
    world
        .set(world.topic, keys::BINDING_ALLOWED, json!(["key_separated"]))
        .await;
    world
        .exec("UPDATE machines SET roles = ARRAY['environment','provider']")
        .await;
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Keep my place"))
        .await
        .unwrap();
    world.wait_blocked(thread, "binding_invalid").await;
    assert_eq!(world.input_state(submitted.input.id).await.0, "pending");
    assert!(world.turn_starts(thread).is_empty());

    world.unset(world.topic, keys::BINDING_ALLOWED).await;
    world.wait_blocked(thread, "agent_offline").await;
    assert_eq!(world.input_state(submitted.input.id).await.0, "pending");
    assert!(world.turn_starts(thread).is_empty());

    world
        .exec("UPDATE machines SET roles = ARRAY['agent','environment','provider']")
        .await;
    world.run_turn(thread, 0, "Ready.").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.input_state(submitted.input.id).await.0, "delivered");
}
