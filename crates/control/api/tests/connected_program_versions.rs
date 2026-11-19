//! Confirmed connected programs are visible before a thread, without invented build artifacts.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

mod places;
mod resources;
mod support;

use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_api_contract::admin::EnvironmentHarnessBuildList;
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::{Id, Time};
use serde_json::json;
use support::World;

#[tokio::test]
async fn confirmed_program_versions_are_listed_without_threads_or_unconfirmed_discoveries() {
    install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    orch_api_core::routes::settings::install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    let world = World::new().await;
    let owner = places::person(&world, "ada").await;
    let reader = places::person(&world, "reader").await;
    let org = places::make_org(&world, &owner, "Workshop", "workshop").await;
    let mut conn = world.conn().await;
    let id = fixtures::insert_environment(
        &mut conn,
        owner.id(),
        Some(org.base.id),
        "Programs",
        "connected",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        owner.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM environments WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let codex =
        json!({"kind":"codex","path":"/controlled/codex","version":"0.165.0","daemon":false});
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1").bind(machine).bind(json!({
        "policy":{"agent.harnesses":[{"kind":"codex","path":"/controlled/codex"},{"kind":"claude-code","path":"/controlled/claude"}]},
        "harnesses":[codex.clone(),codex,{"kind":"claude-code","path":"/controlled/claude","version":"unknown","daemon":false}],
        "discovered_harnesses":[{"kind":"dsh","path":"/controlled/dsh","version":"0.1.0","daemon":false}]
    })).execute(&mut *conn).await.unwrap();
    let threads: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE environment_id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(threads, 0);
    drop(conn);
    resources::name_person(&world, "environment", id, "environment.view", reader.id()).await;
    let builds = world
        .app
        .call(EnvironmentHarnessBuildList, &[&id])
        .token(&reader.token)
        .send()
        .await
        .ok();
    let mut versions = builds
        .iter()
        .map(|build| (build.kind.as_str(), build.version.as_str()))
        .collect::<Vec<_>>();
    versions.sort_unstable();
    assert_eq!(versions, [("claude-code", "unknown"), ("codex", "0.165.0")]);
    assert!(builds.iter().all(|build| build.build.is_none()));
    assert!(
        builds.iter().all(|build| build.harness.is_some()),
        "the existing registry supplies localized harness names"
    );
}
