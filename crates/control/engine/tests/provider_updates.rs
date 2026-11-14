//! Provider replacements keep their state and sign-in, hold every bound thread and undo a lost sign-in.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod runtimes;

use common::{World, eventually};
use hosting::{ScriptedHost, serve, wait_over};
use orch_api_contract::environment::UpdatePreflightReason;
use orch_authz::{Caller, Principal};
use orch_db::queries::{environments::hosting as envs, resources::provider_runtime as rows};
use orch_engine::provider_runtime::{RuntimeError, StopWhy};
use orch_engine::updates::Target;
use orch_link_contract::commands::{
    AgentStartResult, CommandKind, CommandResult, ProviderRefreshResult,
};
use orch_types::{Id, Time};
use serde_json::json;

async fn replacement(world: &World, provider: Id) -> orch_db::queries::jobs::JobRow {
    replacement_queued(world, provider, || {}).await
}

async fn replacement_queued(
    world: &World,
    provider: Id,
    queued: impl FnOnce(),
) -> orch_db::queries::jobs::JobRow {
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let draft = world
        .engine()
        .draft_update(
            &mut tx,
            &principal,
            Target::Provider(provider),
            UpdatePreflightReason::Update,
            None,
        )
        .await
        .unwrap();
    let job = orch_engine::jobs::recreate::queue(&mut tx, draft, None, world.clock.now())
        .await
        .unwrap();
    queued();
    tx.commit().await.unwrap();
    world.engine().kick();
    job
}

async fn fixture() -> (World, ScriptedHost, Id, Id, hosting::Unit) {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    runtimes::provider_template(&world).await;
    let provider = runtimes::managed_subscription(&world, "Service").await;
    let (runtime, unit) = runtimes::made(&world, &host, provider).await;
    sqlx::query("UPDATE providers SET status = 'ready' WHERE id = $1")
        .bind(provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    unit.added.machine.handler.on(CommandKind::AgentStart, |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: false,
            harness_version: "1.0.0".into(),
            descriptor: serde_json::from_value(common::descriptor()).unwrap(),
        }))
    });
    unit.added
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, |_| {
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: "ready".into(),
                facts: serde_json::from_value(json!({})).unwrap(),
            }))
        });
    (world, host, provider, runtime, unit)
}

#[tokio::test]
async fn updating_a_provider_retains_the_state_volume_and_the_same_provider_and_sign_in() {
    let (world, _host, provider, old, _unit) = fixture().await;
    let before = envs::runtime(&mut world.pool.acquire().await.unwrap(), old)
        .await
        .unwrap()
        .unwrap();
    let job = replacement(&world, provider).await;
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let mut conn = world.pool.acquire().await.unwrap();
    let after = rows::live_runtime(&mut conn, provider)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(after.id, old);
    assert_eq!(before.volumes, after.volumes);
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(after.provider_id, Some(provider));
    assert_eq!(
        rows::state(&mut conn, provider)
            .await
            .unwrap()
            .unwrap()
            .status,
        "ready"
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_provider_whose_successor_lost_its_sign_in_goes_back_to_its_old_unit() {
    let (world, host, provider, old, unit) = fixture().await;
    // Capture the old signed-in state before the successor loses it. The queued job becomes
    // visible only after this handler is installed and its transaction commits.
    let job = replacement_queued(&world, provider, || {
        unit.added
            .machine
            .handler
            .on(CommandKind::ProviderRefresh, |_| {
                Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                    source: None,
                    status: "needs_setup".into(),
                    facts: serde_json::from_value(json!({})).unwrap(),
                }))
            });
    })
    .await;
    let (state, progress, error) = wait_over(&world, job.id).await;
    assert_eq!(
        state,
        "failed",
        "job={} progress={progress} error={error:?}; native_commands={:?}",
        job.id,
        unit.added
            .machine
            .handler
            .executions()
            .iter()
            .map(|run| run.kind)
            .collect::<Vec<_>>()
    );
    assert_eq!(error.as_ref().unwrap()["code"], "sign_in_lost");
    assert_eq!(error.unwrap()["undone"], true);
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        rows::live_runtime(&mut conn, provider)
            .await
            .unwrap()
            .unwrap()
            .id,
        old
    );
    assert_eq!(host.calls().last().unwrap().body["runtime_id"], json!(old));
    world.engine().stop();
}

#[tokio::test]
async fn a_provider_can_choose_to_update_when_it_is_woken_and_a_failed_update_wakes_the_old_unit() {
    let (world, host, provider, old, _unit) = fixture().await;
    // A newly made unit is current; changing its template supplies the update this wake applies.
    sqlx::query("UPDATE env_templates SET spec = jsonb_set(spec, '{image}', to_jsonb('busybox:newer'::text)) WHERE id = (SELECT template_id FROM runtimes WHERE id = $1)")
        .bind(old).execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE providers SET runtime_config = '{\"runtime.update\":\"on_start\"}', machine_id = NULL WHERE id = $1").bind(provider).execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE runtimes SET state = 'stopped' WHERE id = $1")
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    host.refuse(
        CommandKind::NodeRuntimeCreate,
        orch_link_contract::commands::CommandError::new(
            orch_link_contract::commands::ErrorCode::Conflict,
            "the newer image cannot run",
        ),
    );
    let started = world
        .engine()
        .start_provider_runtime(provider, None)
        .await
        .unwrap();
    let orch_engine::provider_runtime::RuntimeStart::Job(job) = started else {
        panic!("an update was queued")
    };
    assert_eq!(job.kind, "provider_recreate");
    assert_eq!(wait_over(&world, job.id).await.0, "failed");
    let mut conn = world.pool.acquire().await.unwrap();
    let runtime = rows::live_runtime(&mut conn, provider)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runtime.id, old);
    assert_eq!(runtime.state, "running");
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'",
    )
    .bind(world.ada)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(told, 1);
    assert_eq!(
        rows::state(&mut conn, provider)
            .await
            .unwrap()
            .unwrap()
            .machine_id,
        runtime.machine_id
    );
    world.engine().stop();
}

async fn state_of(world: &World, runtime: Id) -> String {
    sqlx::query_scalar("SELECT state FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

/// The unit an update made to replace `old`, which stands beside it on the same volume until the update commits.
async fn successor_of(world: &World, old: Id) -> Id {
    sqlx::query_scalar("SELECT id FROM runtimes WHERE successor_of = $1")
        .bind(old)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

/// Deletes the unit as a host manager does, in a transaction of its own.
async fn remove_by_force(world: &World, host: Id, runtime: Id) -> Result<(), RuntimeError> {
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    world
        .engine()
        .remove_host_provider_unit_in(
            tx,
            &principal,
            &orch_db::audit::Origin::default(),
            host,
            runtime,
            false,
            true,
        )
        .await
}

#[tokio::test]
async fn a_forced_stop_while_an_update_waits_for_its_new_unit_puts_the_update_back_and_leaves_no_second_unit_on_the_volume()
 {
    let (world, host, provider, old, _unit) = fixture().await;
    world.set_system("jobs.stop_wait_secs", json!(5)).await;
    // The update has stopped the first unit, made the second and asked the host to start it; the host
    // does not answer.
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeRuntimeStart);
    let job = replacement(&world, provider).await;
    host.wait_began(CommandKind::NodeRuntimeStart, 2).await;
    let new = successor_of(&world, old).await;
    world
        .engine()
        .stop_provider_runtime_with(provider, false, StopWhy::Person(world.ada), true)
        .await
        .unwrap();
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "cancelled", "{error:?}");
    assert_eq!(
        error.as_ref().unwrap()["undone"],
        true,
        "the update was undone, not only ended: {error:?}"
    );
    assert_eq!(
        state_of(&world, new).await,
        "removed",
        "the second unit is taken away, so nothing runs on the volume of the first"
    );
    assert!(
        host.calls()
            .iter()
            .any(|call| call.kind == CommandKind::NodeRuntimeRemove
                && call.body["runtime_id"] == json!(new)),
        "the host was told to remove the second unit"
    );
    assert_eq!(state_of(&world, old).await, "stopped");
    assert_eq!(
        host.began(CommandKind::NodeRuntimeStart),
        2,
        "the first unit was not started again against the person's stop"
    );
    let machine: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM providers WHERE id = $1")
        .bind(provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert!(
        machine.is_none(),
        "a provider whose unit is stopped has no machine"
    );
    host.added.machine.handler.release();
    world.engine().stop();
}

#[tokio::test]
async fn a_forced_stop_that_cannot_undo_the_update_in_time_leaves_the_new_unit_to_be_removed_and_keeps_the_volume()
 {
    let (world, host, provider, old, _unit) = fixture().await;
    world.set_system("jobs.stop_wait_secs", json!(1)).await;
    // The host answers neither the start of the second unit nor, later, its removal.
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeRuntimeStart);
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeRuntimeRemove);
    let job = replacement(&world, provider).await;
    host.wait_began(CommandKind::NodeRuntimeStart, 2).await;
    let new = successor_of(&world, old).await;
    world
        .engine()
        .stop_provider_runtime_with(provider, false, StopWhy::Person(world.ada), true)
        .await
        .unwrap();
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "cancelled", "{error:?}");
    assert!(
        matches!(state_of(&world, new).await.as_str(), "retired" | "removed"),
        "the second unit is not left as a live unit on the shared volume: {}",
        state_of(&world, new).await
    );
    assert_eq!(state_of(&world, old).await, "stopped");
    // When the host answers again the second unit is stopped and removed, and the volume it shared stays.
    host.added.machine.handler.release();
    eventually(
        "the second unit to be removed once its host answers",
        || async { state_of(&world, new).await == "removed" },
    )
    .await;
    assert_eq!(
        host.count(CommandKind::NodeVolumeRemove),
        0,
        "the state volume the first unit stands on was not removed with the second unit"
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_host_managers_forced_removal_of_the_unit_an_update_replaces_puts_the_update_back_and_removes_that_unit()
 {
    let (world, host, provider, old, _unit) = fixture().await;
    world.set_system("jobs.stop_wait_secs", json!(5)).await;
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeRuntimeStart);
    let job = replacement(&world, provider).await;
    host.wait_began(CommandKind::NodeRuntimeStart, 2).await;
    let new = successor_of(&world, old).await;
    remove_by_force(&world, host.id, old).await.unwrap();
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "cancelled", "{error:?}");
    assert_eq!(error.as_ref().unwrap()["undone"], true, "{error:?}");
    eventually(
        "the unit that was removed to be gone from its host",
        || async { state_of(&world, old).await == "removed" },
    )
    .await;
    assert_eq!(
        state_of(&world, new).await,
        "removed",
        "the update's new unit does not outlive the unit it was to replace"
    );
    assert_eq!(
        host.count(CommandKind::NodeVolumeRemove),
        0,
        "the manager kept the volumes"
    );
    host.added.machine.handler.release();
    world.engine().stop();
}
