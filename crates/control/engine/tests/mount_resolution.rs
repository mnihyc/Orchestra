//! Mount modes and machine locations are resolved once for connected attaches and managed private views.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::mounts::{EnvMountReason, MountMode};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::mounts::{LocationRow, MountRow};
use orch_engine::mounts::{Target, resolve_for};
use orch_types::{Id, Time};
use serde_json::json;

async fn add(world: &World, machine: Id, read_only: bool) -> Id {
    let id = orch_types::new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::resources::mounts::insert(
        &mut conn,
        &MountRow {
            id,
            name: if read_only { "readonly" } else { "shared" }.into(),
            description: String::new(),
            org_id: Some(world.root),
            home_id: Some(world.root),
            owner_id: Some(world.ada),
            target: if read_only {
                "/mnt/readonly"
            } else {
                "/mnt/shared"
            }
            .into(),
            read_only,
            enabled: true,
            default_fate: "as_is".into(),
            revision: 1,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Mount.into(),
            id,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    orch_db::queries::resources::mounts::set_location(
        &mut conn,
        &LocationRow {
            mount_id: id,
            machine_id: machine,
            host_id: None,
            host_name: None,
            kind: "path".into(),
            source: format!("/srv/{id}"),
            storage: None,
            data_kind: None,
            created_by: world.ada,
            created_at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    id
}

/// The world's machine says what a connector with the environment role says of itself: it attaches registered
/// directories, and a mount there is used where its data already is.
async fn declare_directories(world: &World) {
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: None,
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET facts = facts || $2 WHERE id = $1")
        .bind(world.machine_id)
        .bind(json!({ "environment_descriptor": descriptor }))
        .execute(world.pool.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn connected_mounts_have_the_machine_path_and_private_requests_fall_back_to_read_only() {
    let world = World::new().await;
    declare_directories(&world).await;
    let ro = add(&world, world.machine_id, true).await;
    let shared = add(&world, world.machine_id, false).await;
    world
        .set(
            world.root,
            "env.mounts",
            json!({ro.to_string(): true, shared.to_string(): "private"}),
        )
        .await;
    let own = json!({});
    let mut conn = world.pool.acquire().await.unwrap();
    let result = resolve_for(
        &mut conn,
        &Target {
            environment: world.environment,
            home: world.topic,
            own: &own,
            owner: world.ada,
            machine: Some(world.machine_id),
            driver: None,
            workspace: "/workspace",
            home_path: None,
            storage: None,
            planned: &[],
            detached: &[],
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    let ro = result
        .iter()
        .find(|mount| mount.view.mount.id == ro)
        .unwrap();
    assert_eq!(ro.view.mode, MountMode::ReadOnly);
    assert_eq!(
        ro.attach.as_ref().unwrap().source,
        ro.view.path.as_ref().unwrap().as_str()
    );
    let private = result
        .iter()
        .find(|mount| mount.view.mount.id == shared)
        .unwrap();
    assert_eq!(private.view.asked, MountMode::Private);
    assert_eq!(private.view.mode, MountMode::ReadOnly);
    assert!(private.layer.is_none() && private.spec.is_none());
    assert!(!private.view.enforced);
}

#[tokio::test]
async fn a_managed_private_layer_belongs_to_the_environment_and_a_missing_location_is_explained() {
    let world = World::new().await;
    declare_directories(&world).await;
    let host = hosting::ScriptedHost::on(&world, "build", 16_384).await;
    let mount = add(&world, host.machine, true).await;
    world
        .set(
            world.root,
            "env.mounts",
            json!({mount.to_string(): "private"}),
        )
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,mounts}', '\"placed\"'), storages=$2 WHERE id=$1")
        .bind(host.id).bind(json!([{"name":"fast","default":true,"private_views":true}])).execute(&mut *conn).await.unwrap();
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,private_views}', 'true') WHERE id=$1",
    )
    .bind(host.id)
    .execute(&mut *conn)
    .await
    .unwrap();
    let own = json!({});
    let result = resolve_for(
        &mut conn,
        &Target {
            environment: world.environment,
            home: world.topic,
            own: &own,
            owner: world.ada,
            machine: Some(host.machine),
            driver: Some("docker"),
            workspace: "/workspace",
            home_path: Some("/home/agent"),
            storage: Some("fast".into()),
            planned: &[],
            detached: &[],
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    assert_eq!(result[0].view.mode, MountMode::Private);
    assert_eq!(
        result[0].spec.as_ref().unwrap()["source"],
        format!("/srv/{mount}")
    );
    assert_eq!(
        result[0].layer.as_ref().unwrap()["name"],
        format!("orch-over-{}-{mount}", world.environment)
    );
    let missing = resolve_for(
        &mut conn,
        &Target {
            environment: world.environment,
            home: world.topic,
            own: &own,
            owner: world.ada,
            machine: Some(world.machine_id),
            driver: None,
            workspace: "/workspace",
            home_path: None,
            storage: None,
            planned: &[],
            detached: &[],
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    assert_eq!(missing[0].view.reason, Some(EnvMountReason::NoLocation));
    assert!(missing[0].attach.is_none() && missing[0].view.path.is_none());
}
