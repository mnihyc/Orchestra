//! What the tests of a provider's runtime stand on: a template of the pool of providers' runtimes, a managed subscription, the job that makes its runtime, and the connector of a unit that comes online with the roles of a runtime of that pool.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::common::World;
use crate::hosting::{ScriptedHost, Unit, connector, wait_over};
use chrono::Utc;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_authz::ownership::{self, NewAccess};
use orch_db::queries::jobs::provider_jobs;
use orch_db::queries::resources::hosting as host_rows;
use orch_db::queries::resources::provider_runtime as rows;
use orch_engine::jobs::provider_provision::{self, Mode};
use orch_link_contract::commands::CommandKind as K;
use orch_types::{Id, Time, new_id};
use serde_json::json;

/// A template of the pool of providers' runtimes that a Docker host makes from a small image, the one
/// the provider's values name.
pub async fn provider_template(world: &World) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO env_templates (id, org_id, home_id, owner_id, name, kind, driver, spec)
         VALUES ($1, $2, $2, $3, 'Provider runtime', 'provider', 'docker', $4)",
    )
    .bind(id)
    .bind(world.root)
    .bind(world.ada)
    .bind(json!({
        "image": "busybox:latest", "runtime": "runc",
        "user": { "name": "nonroot", "uid": 65532, "gid": 65532 },
        "resources": { "cpu_millis": 2000, "memory_mb": 2048, "pids": 512 },
        "host_labels": []
    }))
    .execute(world.pool.pool())
    .await
    .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    ownership::start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Template.into(),
            id,
        },
        world.ada,
        NewAccess::Defaults,
        Time::new(Utc::now()),
    )
    .await
    .unwrap();
    drop(conn);
    world
        .set(world.root, "runtime.template_id", json!(id))
        .await;
    id
}

/// A managed subscription of the fake harness, the world's maker's, whose runtime is not made yet.
pub async fn managed_subscription(world: &World, name: &str) -> Id {
    let id = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    let made = rows::insert_subscription(
        &mut conn,
        &rows::NewSubscription {
            id,
            name,
            owner_id: world.ada,
            home_id: world.root,
            harness_kind: "fake",
            agent_id: world.agent,
            config: &json!({}),
            arrangements: &["separated".to_owned()],
            runtime_config: &json!({}),
        },
        Utc::now(),
    )
    .await
    .unwrap();
    assert!(made, "the home is there");
    ownership::start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id,
        },
        world.ada,
        NewAccess::Defaults,
        Time::new(Utc::now()),
    )
    .await
    .unwrap();
    id
}

/// Queues the job that makes the runtime of a provider, as `POST /providers` does, and has the engine
/// take it up.
pub async fn queue(world: &World, provider: Id, mode: Mode) -> Id {
    let mut tx = world.pool.begin().await.unwrap();
    let queued = provider_provision::queue(&mut tx, Utc::now(), provider, mode, Some(world.ada))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    match queued {
        provider_jobs::Queued::Made(job) | provider_jobs::Queued::Running(job) => job.id,
        provider_jobs::Queued::NoProvider => panic!("the provider is there"),
    }
}

/// The connector of the unit of a provider's runtime comes online with the roles of a runtime of that
/// pool, which the runtime then knows as its own.
pub async fn enrol(world: &World, runtime: Id) -> Unit {
    let unit = connector(world, "unit", &["provider", "agent"]).await;
    let mut conn = world.pool.acquire().await.unwrap();
    host_rows::adopt_machine(&mut conn, unit.id, runtime, Utc::now())
        .await
        .unwrap();
    unit
}

/// A provider whose runtime was made, as a job would have left it: the unit's connector is online and
/// the runtime and the provider say so.
pub async fn made(world: &World, host: &ScriptedHost, provider: Id) -> (Id, Unit) {
    let before = host.count(K::NodeRuntimeStart);
    let job = queue(world, provider, Mode::Make).await;
    host.wait_for(K::NodeRuntimeStart, before + 1).await;
    let runtime = host.started().unwrap();
    let unit = enrol(world, runtime).await;
    let (state, _, error) = wait_over(world, job).await;
    assert_eq!(state, "succeeded", "{error:?}");
    (runtime, unit)
}

/// The world's own provider made a managed one whose runtime is up on the world's machine, which is the
/// unit's connector. The runtime's identifier.
pub async fn managed_world_provider(world: &World, host: &ScriptedHost, template: Id) -> Id {
    let runtime = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    rows::insert_runtime(
        &mut conn,
        &rows::NewRuntime {
            id: runtime,
            host_id: host.id,
            unit_name: &provider_provision::unit_name(&runtime),
            provider_id: world.provider,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({ "state": { "name": "orch-state-x", "storage": null } }),
        },
        Utc::now(),
    )
    .await
    .unwrap();
    host_rows::adopt_machine(&mut conn, world.machine_id, runtime, Utc::now())
        .await
        .unwrap();
    orch_db::queries::environments::hosting::set_runtime_state(
        &mut conn,
        runtime,
        &["creating"],
        "running",
        None,
        Utc::now(),
    )
    .await
    .unwrap();
    drop(conn);
    runtime
}
