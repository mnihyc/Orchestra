//! A machine goes away and comes back while threads depend on it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, eventually, meanwhile, message_done, message_grew, queued, said, turn_ended};
use orch_authz::Caller;
use orch_link_contract::commands::CommandKind;
use orch_link_contract::events::TurnOutcome;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_that_drops_in_the_middle_of_a_turn_blocks_the_thread_and_the_turn_goes_on_when_it_returns()
 {
    let world = World::new().await;
    let thread = world.thread("Dropped").await;
    let ada = Caller::session(world.ada);
    let (start, _) = world.running_turn(thread, "A long job").await;

    world.link_drops();
    world.wait_blocked(thread, "agent_offline").await;
    assert_eq!(
        world.turn_state(start.turn_id).await,
        "running",
        "the turn is left as it is"
    );
    let online: bool = world
        .scalar(
            "SELECT online FROM machines WHERE id = $1",
            world.machine_id,
        )
        .await;
    assert!(!online);

    // The harness goes on while the link is down; what it says waits in its spool. Somebody sends
    // another input meanwhile, which waits behind the turn.
    world.say(thread, message_grew("m", 7, " and on")).await;
    world.say(thread, message_done("m", "Working and on")).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    let later = world
        .engine()
        .submit_input(&ada, thread, queued("After the job"))
        .await
        .unwrap();
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.turn_state(start.turn_id).await,
        "running",
        "nothing arrives while the link is down"
    );
    assert_eq!(world.input_state(later.input.id).await.0, "pending");

    world.link_returns();
    world.wait_turn(start.turn_id, "completed").await;
    // What was said is there exactly once, and the input that waited starts its turn.
    let second = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&second.text), "After the job");
    let sync = world.sync(thread).await;
    assert_eq!(
        said(&sync)[..2],
        ["user: A long job", "agent: Working and on"]
    );
    world.answer(thread, &second, "m2", "Done.").await;
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let online: bool = world
        .scalar(
            "SELECT online FROM machines WHERE id = $1",
            world.machine_id,
        )
        .await;
    assert!(online);
    assert_eq!(world.ran(CommandKind::TurnStart), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_blocked_for_want_of_a_machine_that_returns_while_the_turn_ran_goes_back_to_running()
 {
    let world = World::new().await;
    let thread = world.thread("Back").await;
    let (start, _) = world.running_turn(thread, "Job").await;
    world.link_drops();
    world.wait_blocked(thread, "agent_offline").await;
    world.link_returns();
    // The harness has nothing to say; the thread is running again all the same.
    world.wait_state(thread, "running").await;
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_sent_while_the_machine_is_away_waits_and_starts_when_it_returns() {
    let world = World::new().await;
    let thread = world.thread("Away").await;
    let ada = Caller::session(world.ada);
    world.link_drops();
    eventually("the system sees the machine gone", || async {
        let online: bool = world
            .scalar(
                "SELECT online FROM machines WHERE id = $1",
                world.machine_id,
            )
            .await;
        !online
    })
    .await;

    let submitted = world
        .engine()
        .submit_input(&ada, thread, queued("While away"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.input_state(submitted.input.id).await.0, "pending");
    assert_eq!(world.turn_starts(thread).len(), 0);

    world.link_returns();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "While away");
    world.answer(thread, &start, "m1", "Back again.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}
