//! Inherited sub-agent limits reach the native session before the next turn, without changing a running turn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
use common::{World, queued, steering, turn_ended};
use orch_authz::Caller;
use orch_link_contract::commands::{Command, CommandKind};
use orch_link_contract::events::TurnOutcome;
use serde_json::json;

#[tokio::test]
async fn a_changed_place_limit_is_sent_before_the_next_turn_and_a_steer_keeps_the_old_session() {
    let world = World::new().await;
    let thread = world.thread("Sub-agent limits").await;
    let (running, _) = world.running_turn(thread, "Begin").await;
    let opens = world.seen.of_kind(CommandKind::SessionOpen);
    let first = opens
        .iter()
        .find_map(|command| match command {
            Command::SessionOpen(open) => Some(open),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (first.subagent_limits.at_once, first.subagent_limits.depth),
        (8, 1)
    );
    world
        .set(world.topic, "agents.subagents_at_once", json!(2))
        .await;
    world
        .set(world.topic, "agents.subagent_depth", json!(3))
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            steering("Keep working"),
        )
        .await
        .unwrap();
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    assert_eq!(
        world.seen.of_kind(CommandKind::SessionOpen).len(),
        opens.len(),
        "an inherited limit never reopens a running session behind a steer"
    );
    world
        .say(thread, turn_ended(running.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go on"))
        .await
        .unwrap();
    world.turn_start(thread, 1).await;
    let opens = world.seen.of_kind(CommandKind::SessionOpen);
    let last = opens
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::SessionOpen(open) => Some(open),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (last.subagent_limits.at_once, last.subagent_limits.depth),
        (2, 3)
    );
}
