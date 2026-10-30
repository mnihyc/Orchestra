//! A turn whose environment is stopped starts it and waits, the thread saying what it waits for, and goes on when the environment is up; one whose environment cannot be made keeps its input and says why.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use chrono::Utc;
use common::{World, eventually, queued};
use hosting::{ScriptedHost, serve, template};
use orch_authz::Caller;
use orch_link_contract::commands::{CommandError, CommandKind as K, ErrorCode};
use orch_types::Id;

/// The world's environment, stopped on `host`: its unit's connector is away, as a stopped unit's is.
async fn stopped(world: &World, host: &ScriptedHost) -> (Id, hosting::Unit) {
    let template = template(world).await;
    let (environment, runtime, unit) = hosting::running(world, host, template, "Stopped").await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::environments::hosting::set_runtime_state(
        &mut conn,
        runtime,
        &["running"],
        "stopped",
        None,
        Utc::now(),
    )
    .await
    .unwrap();
    orch_db::queries::environments::hosting::set_status(
        &mut conn,
        environment,
        &["running"],
        "stopped",
        None,
        Utc::now(),
    )
    .await
    .unwrap();
    world.set_machine_online(unit.id, false).await;
    (environment, unit)
}

#[tokio::test]
async fn a_turn_waits_for_a_stopped_environment_and_runs_when_it_is_up() {
    // The subscription's first scheduled read would start its harness on the machine at any moment, with no turn
    // asking: the world begins with it observed, so that every harness start counted here is the turn's.
    let world = World::observed().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let (environment, unit) = stopped(&world, &host).await;
    let thread = world.thread("work").await;
    hosting::bind(&world, thread, environment).await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    // The thread says what it waits for, and the host is told to start the unit.
    eventually("the thread to wait for its environment", || async {
        world.state(thread).await.2.as_deref() == Some("environment")
    })
    .await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    assert_eq!(world.state(thread).await.0, "starting");
    assert_eq!(
        world.ran(K::TurnStart),
        0,
        "nothing is started in an environment that is not up"
    );
    assert_eq!(
        world.ran(K::AgentStart),
        0,
        "no harness is started for a turn whose environment is not up: {:?}",
        world.seen.of_kind(K::AgentStart)
    );
    // The unit's connector is back; the environment is attached again and the turn goes on.
    world.set_machine_online(unit.id, true).await;
    eventually("the turn to be handed to the harness", || async {
        world.ran(K::TurnStart) == 1
    })
    .await;
    let (status, generation): (String, i32) =
        sqlx::query_as("SELECT status, generation FROM environments WHERE id = $1")
            .bind(environment)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), generation),
        ("running", 1),
        "a start is not a new unit"
    );
    assert_eq!(
        unit.kinds(),
        [K::EnvAttach, K::ExecStart, K::FsMkdir],
        "the environment is attached to the connector again, the executor its thread works through is started, and the directory of the thread is made"
    );
    world.run_turn(thread, 0, "Hello!").await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test]
async fn a_turn_makes_the_directory_of_its_thread_in_the_environment_before_anything_starts_in_it()
{
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _runtime, unit) = hosting::running(&world, &host, template, "Up").await;
    let thread = world.thread("work").await;
    hosting::bind(&world, thread, environment).await;
    // The thread works in `web`, and its agent starts in `src` below it.
    sqlx::query("UPDATE threads SET workspace_dir = 'web' WHERE node_id = $1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(thread, "thread.cwd", serde_json::json!("src"))
        .await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    eventually("the turn to be handed to the harness", || async {
        world.ran(K::TurnStart) == 1
    })
    .await;
    let made = unit.calls_of(K::FsMkdir);
    assert_eq!(made.len(), 1, "once for the turn");
    assert_eq!(
        made[0].body["path"], "web/src",
        "the directory the agent starts in, which has the one above it with it"
    );
    world.run_turn(thread, 0, "Hello!").await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test]
async fn a_thread_that_works_at_the_root_of_the_workspace_has_nothing_to_make() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _runtime, unit) = hosting::running(&world, &host, template, "Up").await;
    let thread = world.thread("work").await;
    hosting::bind(&world, thread, environment).await;
    sqlx::query("UPDATE threads SET workspace_dir = '' WHERE node_id = $1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    eventually("the turn to be handed to the harness", || async {
        world.ran(K::TurnStart) == 1
    })
    .await;
    assert_eq!(unit.count(K::FsMkdir), 0);
}

#[tokio::test]
async fn a_turn_whose_environment_cannot_be_started_keeps_its_input_and_the_thread_says_why() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let (environment, _unit) = stopped(&world, &host).await;
    let thread = world.thread("work").await;
    hosting::bind(&world, thread, environment).await;
    host.refuse(
        K::NodeRuntimeStart,
        CommandError::new(ErrorCode::Unsupported, "the unit is gone"),
    );
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
    let (state, detail): (String, Option<String>) =
        sqlx::query_as("SELECT state, blocked_detail FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(state, "blocked");
    let detail = detail.unwrap();
    assert!(detail.contains("the unit is gone"), "{detail}");
    let (input, _) = world.input_state(submitted.input.id).await;
    assert_eq!(input, "pending", "the message waits where it stood");
    assert_eq!(world.ran(K::TurnStart), 0);
}
