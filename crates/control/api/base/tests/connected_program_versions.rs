//! Confirmed connected programs are visible before a thread, without invented build artifacts.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

mod places;
mod resources;
mod rig;
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

#[tokio::test]
async fn an_installed_program_no_catalogue_build_matches_shows_its_own_check_while_it_is_the_same_file()
 {
    install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    orch_api_core::routes::settings::install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    let world = World::new().await;
    let owner = places::person(&world, "ada").await;
    let reader = places::person(&world, "reader").await;
    let manager = places::person(&world, "manager").await;
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
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("machine", machine, "live"),
        "any",
        "machine.manage",
        ("user", Some(manager.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    let at = Time::new(world.now());
    let facts = |current: String| {
        json!({
            "policy":{"agent.harnesses":[{"kind":"codex","path":"/controlled/codex"},{"kind":"fake","path":"/controlled/orch-connector"}]},
            "harnesses":[
                {"kind":"codex","path":"/controlled/codex","version":"0.165.0","sha256":current,"daemon":false},
                {"kind":"fake","path":"/controlled/orch-connector","version":"unknown","daemon":false}
            ],
            "program_checks":[
                {"kind":"codex","path":"/controlled/codex","sha256":"e".repeat(64),"builtin":false,"at":at,
                 "status":"failed","version":"0.165.0","missing":["turn/steer"],"off":[],"error":null},
                {"kind":"fake","path":"/controlled/orch-connector","sha256":null,"builtin":true,"at":at,
                 "status":"checked","version":null,"missing":[],"off":[],"error":null}
            ]
        })
    };
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(machine)
        .bind(facts("e".repeat(64)))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    for person in [&reader, &manager] {
        resources::name_person(&world, "environment", id, "environment.view", person.id()).await;
    }
    let rows = |by: &places::Who| {
        let token = by.token.clone();
        let world = &world;
        async move {
            let rows = world
                .app
                .call(EnvironmentHarnessBuildList, &[&id])
                .token(&token)
                .send()
                .await
                .ok();
            serde_json::to_value(rows).unwrap()
        }
    };
    let of = |rows: &serde_json::Value, kind: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == kind)
            .cloned()
            .unwrap()
    };
    let seen = rows(&reader).await;
    let codex = of(&seen, "codex");
    assert_eq!(codex["build"], serde_json::Value::Null);
    assert_eq!(codex["installed"]["machine_id"], json!(machine));
    assert_eq!(codex["installed"]["path"], "/controlled/codex");
    assert_eq!(codex["installed"]["can_check"], false);
    assert_eq!(codex["installed"]["check"]["status"], "failed");
    assert_eq!(
        codex["installed"]["check"]["missing"],
        json!(["turn/steer"])
    );
    let example = of(&seen, "fake");
    assert_eq!(example["installed"]["check"]["builtin"], true);
    assert_eq!(example["installed"]["check"]["status"], "checked");
    assert_eq!(
        of(&rows(&manager).await, "codex")["installed"]["can_check"],
        true
    );
    // Different bytes at the same path are another program: what was found of the earlier ones does not stand for it.
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(machine)
        .bind(facts("f".repeat(64)))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let changed = of(&rows(&reader).await, "codex");
    assert_eq!(changed["installed"]["path"], "/controlled/codex");
    assert_eq!(changed["installed"]["check"], serde_json::Value::Null);
}

#[tokio::test]
async fn a_program_a_thread_reported_the_version_of_keeps_one_row_with_its_own_check() {
    let s = rig::Scene::new().await;
    s.thread().await;
    let environment = s.on.environment;
    let pool = s.rig.db.pool();
    sqlx::query("UPDATE environments SET placement='connected',template_id=NULL WHERE id=$1")
        .bind(environment)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE harnesses SET harness_version='1.0.0' WHERE agent_id=$1")
        .bind(s.on.agent)
        .execute(pool)
        .await
        .unwrap();
    // The machine lists the Example agent built into its connector with no version of its own; a thread's report says
    // which version it is.
    sqlx::query("UPDATE machines SET facts=facts || $2 WHERE id=$1")
        .bind(s.rig.machine_id)
        .bind(json!({
            "policy":{"agent.harnesses":[{"kind":"fake","path":"/usr/local/bin/orch-connector"}]},
            "harnesses":[{"kind":"fake","path":"/usr/local/bin/orch-connector","version":"unknown","daemon":false}],
            "program_checks":[{"kind":"fake","path":"/usr/local/bin/orch-connector","sha256":null,"builtin":true,
                "at":Time::new(s.rig.world.now()),"status":"checked","version":null,"missing":[],"off":[],"error":null}]
        }))
        .execute(pool)
        .await
        .unwrap();
    let rows = s
        .rig
        .world
        .app
        .call(EnvironmentHarnessBuildList, &[&environment])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let rows = serde_json::to_value(rows).unwrap();
    let example: Vec<_> = rows
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "fake")
        .collect();
    assert_eq!(example.len(), 1, "{rows}");
    assert_eq!(example[0]["version"], "1.0.0");
    assert_eq!(example[0]["installed"]["check"]["builtin"], true);
    assert_eq!(example[0]["installed"]["check"]["status"], "checked");
}
