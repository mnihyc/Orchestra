//! Throwaway timing of the explicit-Stop case's steps (never committed to a branch).
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "a test fails by panicking")]

mod common;

use common::World;
use orch_authz::Caller;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_steps_of_an_explicit_stop_after_a_running_turn() {
    let t0 = Instant::now();
    let ms = |at: Instant| at.duration_since(t0).as_millis();
    let world = World::new().await;
    let world_at = Instant::now();
    let thread = world.thread("Timing of a Stop").await;
    let thread_at = Instant::now();
    let caller = Caller::session(world.ada);
    let next_turn = world.turn_starts(thread).len();
    let _submitted = world
        .engine()
        .submit_input(&caller, thread, common::queued("Begin"))
        .await
        .unwrap();
    let submit_at = Instant::now();
    let start = world.turn_start(thread, next_turn).await;
    let start_at = Instant::now();
    world.say(thread, common::turn_began(start.turn_id)).await;
    world.say(thread, common::message_began(start.turn_id, "m")).await;
    world.say(thread, common::message_grew("m", 0, "Working")).await;
    let said_at = Instant::now();
    world.wait_turn(start.turn_id, "running").await;
    let running_at = Instant::now();
    world.wait_state(thread, "running").await;
    let state_at = Instant::now();
    let _queued = world
        .engine()
        .submit_input(&caller, thread, common::queued("Keep waiting"))
        .await
        .unwrap();
    let queued_at = Instant::now();
    let engine = world.engine();
    engine.stop();
    let stopped = engine
        .stop_thread(&caller, thread, Some(start.turn_id))
        .await
        .unwrap();
    let stopped_at = Instant::now();
    let sync = world.sync(thread).await;
    let sync_at = Instant::now();
    eprintln!(
        "TIMING world {} thread {} submit {} turn.start {} said {} running {} thread-running {} queued {} stopped {} (applied {}) sync {} (held {})",
        ms(world_at),
        ms(thread_at),
        ms(submit_at),
        ms(start_at),
        ms(said_at),
        ms(running_at),
        ms(state_at),
        ms(queued_at),
        ms(stopped_at),
        stopped.applied,
        ms(sync_at),
        sync.thread.queue.held
    );
    drop(sync);
    drop(world);
    eprintln!("TIMING dropped {}", ms(Instant::now()));
}
