//! Hosting tests keep deployed waits under a held domain clock and identify the event that times out.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use chrono::Duration as ClockStep;
use common::World;
use hosting::{
    ScriptedHost, await_event, enroll_holding, job, make, running, serve, status, template,
    wait_over,
};
use orch_authz::Caller;
use orch_engine::config::EngineConfig;
use orch_engine::environments::{StopError, StopReason};
use orch_engine::hosting::{ATTACH_WAIT, HostingConfig, JOB_LEASE, STOP_WAIT, WATCH_EVERY};
use orch_link_contract::commands::CommandKind as K;
use orch_testkit::clock;
use orch_types::{Clock, Json};
use std::sync::Arc;
use std::time::Duration;

/// Hold longer than the former world's ten-second enrollment and attach waits,
/// then release inside the actual deployment bounds. Four watcher passes cross
/// the old boundary without relying on a narrow polling window.
const FORMER_SHORT_WAIT: Duration = Duration::from_secs(10);
const HELD_REPLY: Duration = FORMER_SHORT_WAIT.saturating_add(WATCH_EVERY.saturating_mul(4));
/// The scripted host's declared memory fits the small environment template.
const HOST_MEMORY_MB: i64 = 16_384;
/// Wire timestamps distinguish milliseconds, so this is the last observable
/// instant before the configured interruption deadline.
const BEFORE_DEADLINE: ClockStep = ClockStep::milliseconds(1);

fn running_phase(progress: &Json, name: &str) -> bool {
    progress["phases"].as_array().is_some_and(|phases| {
        phases
            .iter()
            .any(|phase| phase["name"] == name && phase["state"] == "running")
    })
}

#[tokio::test]
async fn enrollment_and_attach_keep_the_deployed_bounds_while_the_domain_clock_is_held() {
    let clock = Arc::new(clock::manual());
    let at = clock.now();
    let world = World::with_clock(clock.clone()).await;
    serve(&world);
    let host = ScriptedHost::on(&world, "test-host", HOST_MEMORY_MB).await;
    let made = make(&world, template(&world).await, "Held connector").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let enrollment = HostingConfig::default().enroll_timeout;
    await_event(
        "the provisioning job to enter enrollment",
        enrollment,
        || async { running_phase(&job(&world, made.job.id).await.1, "enroll") },
    )
    .await;
    tokio::time::sleep(HELD_REPLY).await;
    let (state, progress, error) = job(&world, made.job.id).await;
    assert_eq!(state, "running", "{progress}; {error:?}");
    assert!(running_phase(&progress, "enroll"));
    let unit = enroll_holding(&world, host.started().unwrap(), K::EnvAttach).await;
    await_event(
        "the enrolled connector to begin env.attach",
        ATTACH_WAIT,
        || async { unit.began(K::EnvAttach) == 1 },
    )
    .await;
    tokio::time::sleep(HELD_REPLY).await;
    assert_eq!(status(&world, made.id).await.0, "provisioning");
    assert_eq!(job(&world, made.job.id).await.0, "running");
    assert_eq!(
        clock.now(),
        at,
        "domain time did not pass during monotonic waits"
    );
    unit.let_go();
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(status(&world, made.id).await.0, "running");
}

#[tokio::test]
async fn enrollment_expires_at_its_configured_bound_even_when_the_domain_clock_is_held() {
    let clock = Arc::new(clock::manual());
    let at = clock.now();
    let world = World::with_clock(clock.clone()).await;
    serve(&world);
    let host = ScriptedHost::on(&world, "test-host", HOST_MEMORY_MB).await;
    let began = tokio::time::Instant::now();
    let made = make(&world, template(&world).await, "No connector").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    await_event(
        "the provisioning job to enter enrollment",
        HostingConfig::default().enroll_timeout,
        || async { running_phase(&job(&world, made.job.id).await.1, "enroll") },
    )
    .await;
    let allowed = world.engine().config().hosting.enroll_timeout;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "enroll");
    assert!(
        error["message"].as_str().unwrap().contains(&format!(
            "did not come online within {} seconds",
            allowed.as_secs()
        )),
        "{error}"
    );
    // Start measuring before scheduling, because observing a phase through a
    // database read may be delayed on a busy machine. It cannot expire earlier
    // than this lower bound, regardless of when the observer was scheduled.
    assert!(began.elapsed() >= allowed);
    assert!(
        host.count(K::NodeRuntimeRemove) >= 1,
        "the timed-out unit is taken away"
    );
    assert_eq!(clock.now(), at);
}

#[tokio::test]
async fn a_stop_waits_for_the_host_for_the_deployed_bound_with_the_domain_clock_held() {
    let clock = Arc::new(clock::manual());
    let at = clock.now();
    let world = World::with_clock(clock.clone()).await;
    serve(&world);
    let host = ScriptedHost::on(&world, "test-host", HOST_MEMORY_MB).await;
    let (environment, _, _unit) = running(&world, &host, template(&world).await, "Held stop").await;
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    let engine = world.engine();
    let maker = world.ada;
    let began = tokio::time::Instant::now();
    let stopping = tokio::spawn(async move {
        engine
            .stop_environment(environment, false, StopReason::Person(maker))
            .await
    });
    host.wait_began(K::NodeRuntimeStop, 1).await;
    // The SQL write after the host wait gets the same bound as one job lease.
    let result = tokio::time::timeout(STOP_WAIT + JOB_LEASE, stopping)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(StopError::Host(ref detail)) if detail.contains("the host did not answer in time")),
        "{result:?}"
    );
    assert!(
        began.elapsed() >= STOP_WAIT,
        "the former five-second wait is not used"
    );
    assert_eq!(clock.now(), at);
    host.added.machine.handler.release();
}

#[tokio::test]
async fn an_observer_timeout_identifies_the_awaited_event_and_its_bound() {
    // The predicate never answers, so its own work is inside the observer's bound.
    let observer = tokio::spawn(async {
        await_event(
            "runtime connector to answer env.attach",
            WATCH_EVERY,
            || async { std::future::pending::<bool>().await },
        )
        .await;
    });
    let panic = observer.await.unwrap_err().into_panic();
    let message = panic.downcast_ref::<String>().unwrap();
    assert!(
        message.contains("runtime connector to answer env.attach"),
        "{message}"
    );
    assert!(message.contains("configured bound 250ms"), "{message}");
}

#[tokio::test]
async fn an_unanswered_interrupt_is_forced_only_at_the_deployed_domain_clock_deadline() {
    let clock = Arc::new(clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let deployed = EngineConfig::default();
    world.set_config(|config| {
        config.interrupt_force = deployed.interrupt_force;
        config.sweep_every = deployed.sweep_every;
    });
    world.restart_api().await;
    let thread = world.thread("Unanswered interruption").await;
    let (turn, _) = world.running_turn(thread, "Go").await;
    let requested_at = clock.now();
    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, turn.turn_id)
        .await
        .unwrap();
    world.wait_began(K::TurnInterrupt, 1).await;
    // The scripted harness accepts the command and never emits turn.completed.
    // Domain time, rather than a wall sleep or the fixture's old short wait,
    // decides when the engine may end it itself.
    let force = ClockStep::from_std(deployed.interrupt_force).unwrap();
    clock.advance(force - BEFORE_DEADLINE);
    tokio::time::sleep(deployed.sweep_every).await;
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    clock.advance(BEFORE_DEADLINE);
    world.wait_turn(turn.turn_id, "interrupted").await;
    let error: Json = world
        .scalar("SELECT error FROM turns WHERE id = $1", turn.turn_id)
        .await;
    assert_eq!(error["code"], "forced");
    assert_eq!(clock.now(), requested_at + force);
}
