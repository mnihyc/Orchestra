//! Physical machine/node update permissions, truthful running versions, audit and local refusal.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod places;
mod support;

use http::StatusCode;
use orch_api_contract::resources::{
    ComponentUpdate, HostGet, HostUpdateNow, MachineGet, MachineList, MachineUpdateNow,
};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_link_contract::policy::{LocalPolicy, PolicyKey};
use orch_types::{Id, Time};
use places::{Who, person};
use serde_json::json;
use support::World;

async fn grant(world: &World, kind: &str, id: Id, permission: &str, person: &Who) {
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new(kind, id, "live"),
        "any",
        permission,
        ("user", Some(person.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_contended_physical_update_fence_returns_retry_without_waiting_in_a_transaction() {
    let world = World::new().await;
    let machine = orch_types::new_id();
    // A session lock on a bare connection represents another task's owner of this exact fence.
    // This test never keeps a transaction while awaiting a timer, channel or second connection.
    let mut owner = world.conn().await;
    sqlx::query("SELECT pg_advisory_lock(hashtextextended('orch.component-update:' || ($1::uuid)::text, 0))")
        .bind(machine).execute(&mut *owner).await.unwrap();
    let mut tx = world.app.app.db.begin().await.unwrap();
    // A database statement bound prevents a regression to a blocking lock from hanging the test.
    sqlx::query("SET LOCAL statement_timeout='100ms'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let attempted = orch_db::queries::component_updates::lock(&mut tx, machine).await;
    tx.rollback().await.unwrap();
    sqlx::query("SELECT pg_advisory_unlock(hashtextextended('orch.component-update:' || ($1::uuid)::text, 0))")
        .bind(machine).execute(&mut *owner).await.unwrap();
    assert!(matches!(attempted, Err(orch_db::DbError::Retry)));
}

async fn physical(world: &World, owner: &Who, enabled: bool) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_persons_machine(&mut conn, owner.id(), "workstation")
        .await
        .unwrap();
    let mut policy = LocalPolicy::default();
    policy.set(PolicyKey::UpdateAuto, &json!(enabled)).unwrap();
    sqlx::query("UPDATE machines SET connector_version='0.0.0',online=true,roles=ARRAY['environment','agent'],facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(id).bind(serde_json::to_value(policy).unwrap()).execute(&mut *conn).await.unwrap();
    id
}

#[tokio::test]
async fn machine_manage_requests_an_audited_update_and_views_keep_the_running_version_until_reconnect()
 {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let stranger = person(&world, "stranger").await;
    let machine = physical(&world, &owner, true).await;
    let manager = person(&world, "manager").await;
    let viewer = person(&world, "viewer").await;
    grant(&world, "machine", machine, "machine.manage", &manager).await;
    grant(&world, "machine", machine, "machine.view", &viewer).await;
    let denied = world
        .app
        .call(MachineUpdateNow, &[&machine])
        .token(&stranger.token)
        .send()
        .await;
    assert_eq!(denied.status, 404);
    let viewed = world
        .app
        .call(MachineGet, &[&machine])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    assert!(viewed.software_update.unwrap().behind);
    assert_eq!(
        world
            .app
            .call(MachineUpdateNow, &[&machine])
            .token(&viewer.token)
            .send()
            .await
            .status,
        404
    );
    let listed = world
        .app
        .call(MachineList, &[])
        .token(&owner.token)
        .send()
        .await
        .ok();
    let before = listed
        .iter()
        .find(|row| row.base.id == machine)
        .unwrap()
        .software_update
        .as_ref()
        .unwrap();
    assert!(before.behind);
    assert_eq!(before.running_version, "0.0.0");
    let managed = world
        .app
        .call(MachineUpdateNow, &[&machine])
        .token(&manager.token)
        .send()
        .await
        .ok();
    assert_eq!(
        managed.status, "queued",
        "a shared manager need not be the machine's owner"
    );
    let requested: ComponentUpdate = world
        .app
        .call(MachineUpdateNow, &[&machine])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(requested.status, "queued");
    assert_eq!(requested.running_version, "0.0.0");
    let mut conn = world.conn().await;
    let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='machine.update_requested' AND actor_id=$2")
        .bind(machine).bind(owner.id()).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(audits, 1);
    // The authenticated hello records this version in production. A read never claims it from a download alone.
    orch_db::queries::resources::record_session(
        &mut conn,
        machine,
        &requested.recommended_version,
        &["environment".into(), "agent".into()],
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let after = world
        .app
        .call(MachineGet, &[&machine])
        .token(&owner.token)
        .send()
        .await
        .ok()
        .software_update
        .unwrap();
    assert!(!after.behind);
    assert_eq!(after.status, "current");
    assert_eq!(after.running_version, requested.recommended_version);
}

#[tokio::test]
async fn local_update_auto_off_is_a_visible_audited_refusal_without_a_drain_fence() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let machine = physical(&world, &owner, false).await;
    let refused = world
        .app
        .call(MachineUpdateNow, &[&machine])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(refused.status, "refused");
    assert!(refused.behind);
    let detail = refused.detail.unwrap();
    assert!(detail.contains("update.auto"));
    assert!(detail.contains("orch-connector update"));
    let mut conn = world.conn().await;
    assert!(
        orch_db::queries::component_updates::pending(&mut conn)
            .await
            .unwrap()
            .is_empty()
    );
    let shown = world
        .app
        .call(MachineGet, &[&machine])
        .token(&owner.token)
        .send()
        .await
        .ok()
        .software_update
        .unwrap();
    assert_eq!(shown.status, "refused");
    assert_eq!(shown.detail.as_deref(), Some(detail.as_str()));
}

#[tokio::test]
async fn a_node_uses_its_host_manage_page_and_reports_its_own_version_and_command() {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ObjectKind;
    use orch_authz::ownership::{NewAccess, start_resource};
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let stranger = person(&world, "stranger").await;
    let manager = person(&world, "manager").await;
    let viewer = person(&world, "viewer").await;
    // Owning is counted inside an org: the host is at home in one of its owner's.
    let home = places::make_org(&world, &owner, "Office", "office")
        .await
        .base
        .id;
    let mut conn = world.conn().await;
    let host = fixtures::insert_resource(&mut conn, "host", Some(owner.id()), Some(home), "node")
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        owner.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let policy = LocalPolicy::default();
    sqlx::query("UPDATE machines SET connector_version='0.0.0',online=true,roles=ARRAY['node'],facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(machine).bind(serde_json::to_value(policy).unwrap()).execute(&mut *conn).await.unwrap();
    drop(conn);
    grant(&world, "host", host, "host.manage", &manager).await;
    grant(&world, "host", host, "host.view", &viewer).await;
    assert!(matches!(
        world
            .app
            .call(HostUpdateNow, &[&host])
            .token(&stranger.token)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    let viewed = world
        .app
        .call(HostGet, &[&host])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    assert!(viewed.software_update.unwrap().behind);
    assert!(matches!(
        world
            .app
            .call(HostUpdateNow, &[&host])
            .token(&viewer.token)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    let before = world
        .app
        .call(HostGet, &[&host])
        .token(&owner.token)
        .send()
        .await
        .ok()
        .software_update
        .unwrap();
    assert!(before.behind);
    assert_eq!(before.command, "orch-node update");
    let shared = world
        .app
        .call(HostUpdateNow, &[&host])
        .token(&manager.token)
        .send()
        .await
        .ok();
    assert_eq!(
        shared.status, "queued",
        "Host Manage is sufficient without ownership"
    );
    let accepted = world
        .app
        .call(HostUpdateNow, &[&host])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(accepted.status, "queued");
    assert_eq!(accepted.running_version, "0.0.0");
    let mut conn = world.conn().await;
    let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='machine.update_requested' AND actor_id=$2")
        .bind(machine).bind(owner.id()).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(audits, 1);
}
