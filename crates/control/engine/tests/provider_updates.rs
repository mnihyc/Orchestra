//! Provider replacements keep their state and sign-in, hold every bound thread and undo a lost sign-in.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod runtimes;

use common::World;
use hosting::{ScriptedHost, serve, wait_over};
use orch_api_contract::environment::UpdatePreflightReason;
use orch_authz::{Caller, Principal};
use orch_db::queries::{environments::hosting as envs, resources::provider_runtime as rows};
use orch_engine::updates::Target;
use orch_link_contract::commands::{
    AgentStartResult, CommandKind, CommandResult, ProviderRefreshResult,
};
use orch_types::{Id, Time};
use serde_json::json;

async fn replacement(world: &World, provider: Id) -> orch_db::queries::jobs::JobRow {
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
    let job = replacement(&world, provider).await;
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "failed");
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
