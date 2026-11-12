//! Configured outages use actual turn/presence/stop state and retain the existing uncertainty boundary.
use super::*;
use orch_db::queries::threads::{self, offline};
use orch_settings_contract::registry::keys::THREADS_OFFLINE_DEADLINE as DEADLINE;
use orch_types::{Clock, Id, Time};
use serde_json::json;
use std::sync::Arc;

async fn away(world: &World, thread: Id) {
    world.set_machine_online(world.machine_id, false).await;
    world
        .engine()
        .machine_offline(world.machine_id)
        .await
        .unwrap();
    world.wait_blocked(thread, "agent_offline").await;
}
async fn episode(world: &World, turn: Id) -> offline::Episode {
    offline::episode(&mut world.pool.acquire().await.unwrap(), turn)
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_keeps_the_native_turn_waiting_without_an_implicit_deadline() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Indefinite offline wait").await;
    let (start, _) = world.running_turn(thread, "Keep the current work").await;
    away(&world, thread).await;
    let kept = episode(&world, start.turn_id).await;
    assert_eq!(kept.after_secs, None);
    assert!(!kept.expired(clock.now() + chrono::Duration::days(365_000)));
    clock.advance(chrono::Duration::seconds(20));
    world.engine().kick();
    meanwhile(Duration::from_millis(200)).await;
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    assert!(
        world
            .sync(thread)
            .await
            .thread
            .blocked
            .unwrap()
            .offline_deadline
            .is_none()
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retained_offline_deadline_ends_the_same_turn_once_through_the_native_stop_path() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Finite offline wait").await;
    world.set(world.root, DEADLINE, json!(10)).await;
    let (start, _) = world.running_turn(thread, "Keep this partial answer").await;
    world.machine.handler.hold(CommandKind::TurnInterrupt);
    away(&world, thread).await;
    let kept = episode(&world, start.turn_id).await;
    let shown = world
        .sync(thread)
        .await
        .thread
        .blocked
        .unwrap()
        .offline_deadline
        .unwrap();
    assert_eq!(shown.turn_id, start.turn_id);
    assert_eq!(shown.after_seconds, 10);
    assert_eq!(shown.since, Time::new(kept.since));
    assert_eq!(shown.at, kept.at().map(Time::new));
    clock.advance(chrono::Duration::seconds(5));
    // A changed setting affects the next outage; duplicate delivery does not reset this promise.
    world.set(thread, DEADLINE, json!(0)).await;
    world
        .engine()
        .machine_offline(world.machine_id)
        .await
        .unwrap();
    assert_eq!(episode(&world, start.turn_id).await, kept);
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    clock.advance(chrono::Duration::seconds(5));
    world.engine().kick();
    world.wait_turn(start.turn_id, "interrupted").await;
    let code: Option<String> = world
        .scalar(
            "SELECT error->>'code' FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert_eq!(code.as_deref(), Some("machine_offline"));
    let cause: Option<String> = world
        .scalar(
            "SELECT interrupt_cause FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert_eq!(cause.as_deref(), Some("machine_offline"));
    let confirmed: bool = world
        .scalar(
            "SELECT native_end_confirmed FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert!(
        !confirmed,
        "a deadline is not proof that offline native work stopped"
    );
    assert!(
        offline::episode(&mut world.pool.acquire().await.unwrap(), start.turn_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        threads::active_turn(&mut world.pool.acquire().await.unwrap(), thread)
            .await
            .unwrap()
            .is_none()
    );
    world
        .engine()
        .machine_offline(world.machine_id)
        .await
        .unwrap();
    world.engine().kick();
    let errors=world.count("SELECT count(*) FROM items WHERE turn_id=$1 AND kind='error' AND payload->>'code'='machine_offline'",start.turn_id).await;
    assert_eq!(errors, 1, "the existing terminal transition is idempotent");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_returned_machine_clears_the_old_outage_and_a_new_episode_uses_the_current_setting() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Reconnect cancels deadline").await;
    world.set(thread, DEADLINE, json!(10)).await;
    let (start, _) = world
        .running_turn(thread, "Keep running after reconnect")
        .await;
    away(&world, thread).await;
    let old = episode(&world, start.turn_id).await;
    clock.advance(chrono::Duration::seconds(5));
    world.set_machine_online(world.machine_id, true).await;
    world
        .engine()
        .machine_online(world.machine_id)
        .await
        .unwrap();
    world.wait_state(thread, "running").await;
    assert!(
        offline::episode(&mut world.pool.acquire().await.unwrap(), start.turn_id)
            .await
            .unwrap()
            .is_none()
    );
    clock.advance(chrono::Duration::seconds(6));
    world.engine().kick();
    meanwhile(Duration::from_millis(100)).await;
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    assert!(world.sync(thread).await.thread.blocked.is_none());
    world.set(thread, DEADLINE, json!(20)).await;
    away(&world, thread).await;
    let fresh = episode(&world, start.turn_id).await;
    assert!(fresh.since > old.since);
    assert_eq!(fresh.after_secs, Some(20));
    assert!(!fresh.expired(clock.now()));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_controller_uses_the_persisted_episode_instead_of_extending_its_deadline() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Restart retains outage").await;
    world.set(thread, DEADLINE, json!(10)).await;
    let (start, _) = world.running_turn(thread, "Retain the deadline").await;
    away(&world, thread).await;
    let kept = episode(&world, start.turn_id).await;
    let original = world.engine();
    let config = original.config().clone();
    original.stop();
    // Same durable native command/presence suppliers; the controller and its in-memory sessions are new.
    let restarted = orch_engine::Engine::new(orch_engine::EngineParts {
        db: world.pool.clone(),
        clock: world.clock.clone(),
        config,
        commands: world.commands.as_engine(),
        access: world.access.clone(),
        live: world.live.clone(),
    });
    restarted.start();
    assert_eq!(episode(&world, start.turn_id).await, kept);
    clock.advance(chrono::Duration::seconds(10));
    restarted.kick();
    world.wait_turn(start.turn_id, "interrupted").await;
    restarted.stop();
    let code: Option<String> = world
        .scalar(
            "SELECT error->>'code' FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert_eq!(code.as_deref(), Some("machine_offline"));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_locked_inherited_deadline_keeps_its_configured_value_over_a_lower_override() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Inherited offline deadline").await;
    world.set(thread, DEADLINE, json!(0)).await;
    orch_settings::write::write(
        &mut world.pool.acquire().await.unwrap(),
        &orch_settings::write::Write {
            actor: orch_db::audit::Actor::User(world.ada),
            target: orch_settings::Target::Node(world.root),
            key: DEADLINE,
            value: json!(30),
            locked: Some(true),
            expected_revision: None,
            may_lock: true,
            origin: &orch_db::audit::Origin::default(),
        },
        clock.now(),
    )
    .await
    .unwrap();
    let (start, _) = world
        .running_turn(thread, "Wait within the inherited deadline")
        .await;
    away(&world, thread).await;
    assert_eq!(episode(&world, start.turn_id).await.after_secs, Some(30));
    assert_eq!(world.turn_state(start.turn_id).await, "running");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_finished_turns_old_outage_cannot_end_the_next_turn() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Old deadline boundary").await;
    world.set(thread, DEADLINE, json!(10)).await;
    let (first, _) = world.running_turn(thread, "First turn").await;
    away(&world, thread).await;
    clock.advance(chrono::Duration::seconds(5));
    world.set_machine_online(world.machine_id, true).await;
    world
        .engine()
        .machine_online(world.machine_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let (second, _) = world.running_turn(thread, "A distinct new turn").await;
    clock.advance(chrono::Duration::seconds(6));
    world.engine().kick();
    meanwhile(Duration::from_millis(150)).await;
    assert_ne!(first.turn_id, second.turn_id);
    assert_eq!(world.turn_state(second.turn_id).await, "running");
    assert!(
        offline::episode(&mut world.pool.acquire().await.unwrap(), first.turn_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        offline::episode(&mut world.pool.acquire().await.unwrap(), second.turn_id)
            .await
            .unwrap()
            .is_none()
    );
}
