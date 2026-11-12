//! All native full-thread readers carry the real reader-specific presentation, including empty deltas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, message_began, message_done, queued, turn_began, turn_ended};
use orch_api_contract::threads::{
    THREAD_SYNC_DEFAULT_LIMIT, ThreadDisplay, ThreadDisplayState, continuations::ThreadReadSelector,
};
use orch_authz::Caller;
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, Time};

type Proof = Result<(), Box<dyn std::error::Error>>;

fn carries(display: &ThreadDisplay, expected: ThreadDisplayState) -> Proof {
    assert_eq!(
        display.state, expected,
        "actual work cannot fall back to Idle"
    );
    let wire = serde_json::to_value(display)?;
    for field in [
        "state",
        "seq",
        "read_seq",
        "queued_inputs",
        "held_inputs",
        "waiting_inputs",
        "waiting_for_answer",
        "blocked_reason",
        "finished_at",
    ] {
        assert!(
            wire.get(field).is_some(),
            "the native presentation carries {field}"
        );
    }
    Ok(())
}

async fn every_reader(world: &World, thread: Id, expected: ThreadDisplayState) -> Proof {
    let engine = world.engine();
    let first = engine
        .sync(thread, Some(world.ada), None, THREAD_SYNC_DEFAULT_LIMIT)
        .await?;
    carries(&first.thread.display, expected)?;
    let mut tx = orch_db::queries::feeds::begin_reading(&world.pool).await?;
    for since in [None, Some(first.seq)] {
        let implicit = engine
            .sync_in(
                &mut tx,
                thread,
                Some(world.ada),
                since,
                THREAD_SYNC_DEFAULT_LIMIT,
            )
            .await?;
        carries(&implicit.thread.display, expected)?;
        let selected = engine
            .sync_view_in(
                &mut tx,
                thread,
                Some(world.ada),
                since,
                THREAD_SYNC_DEFAULT_LIMIT,
                ThreadReadSelector {
                    continuation_id: Some(first.projection.viewed_continuation_id),
                    projection_revision: Some(first.projection.projection_revision),
                    ..Default::default()
                },
            )
            .await?;
        carries(&selected.thread.display, expected)?;
    }
    let batch = orch_engine::views::thread_displays(
        &mut tx,
        &[thread],
        Some(world.ada),
        Time::new(world.clock.now()),
    )
    .await?;
    carries(
        batch
            .get(&thread)
            .expect("the actual readable thread has presentation facts"),
        expected,
    )?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_native_thread_read_carries_display_instead_of_inferring_idle_from_execution() -> Proof
{
    let world = World::new().await;
    let thread = world.thread("A presentation on every native read").await;
    every_reader(&world, thread, ThreadDisplayState::Idle).await?;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Work once"))
        .await?;
    let turn = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(turn.turn_id)).await;
    world.wait_state(thread, "running").await;
    every_reader(&world, thread, ThreadDisplayState::Running).await?;
    world
        .say(thread, message_began(turn.turn_id, "answer"))
        .await;
    world
        .say(thread, message_done("answer", "Finished work"))
        .await;
    world
        .say(thread, turn_ended(turn.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world.wait_followed_up(thread).await;
    // Execution is idle again, but its unread result has a different reader presentation.
    every_reader(&world, thread, ThreadDisplayState::Finished).await?;
    Ok(())
}
