//! Current confirmation is required even while an older executor report is still available.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

mod common;
mod hosting;

use common::World;
use orch_agent_contract::descriptor::ArrangementType;
use orch_db::queries::{
    environments::thread_engine as environments,
    resources::{access_fixtures as fixtures, thread_engine as resources},
};
use orch_engine::{binding, resolved};
use orch_types::{Id, Json};
use serde_json::json;

fn descriptor(build: &str, other: bool) -> Json {
    let mut descriptor = common::descriptor();
    descriptor["arrangements"] =
        json!([{"type":"separated","executor":"codex-exec-server","executor_version":build}]);
    if other {
        descriptor["arrangements"].as_array_mut().unwrap().push(json!({
            "type":"separated","executor":"orch-exec","executor_version":"*","tool_sets":["plain"]
        }));
    }
    descriptor
}

fn executors() -> Json {
    let mut facts = hosting::facts();
    facts["executors"] = json!([
        {"kind":"codex-exec-server","version":"1.0.0","available":true,"sandbox_modes":["open"]},
        {"kind":"orch-exec","version":"1.0.0","available":true,"sandbox_modes":["open"],"tool_sets":["plain"]}
    ]);
    facts
}

async fn choices(
    world: &World,
    settings_node: Id,
    environment: Id,
    build: &str,
    other: bool,
) -> Vec<binding::ArrangementOption> {
    // Preparation must see the same current declaration as the mode being compared.
    world.declares(descriptor(build, other)).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let agent = resources::agent(&mut conn, world.agent)
        .await
        .unwrap()
        .unwrap();
    let provider = resources::provider(&mut conn, world.provider)
        .await
        .unwrap()
        .unwrap();
    let environment = environments::environment(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    let descriptor = serde_json::from_value(agent.descriptor.clone()).unwrap();
    let settings = resolved::load(&mut conn, settings_node).await.unwrap();
    let runtime = binding::runtime_facts(&mut conn, &agent, &provider, &environment)
        .await
        .unwrap();
    binding::options(
        &binding::Facts {
            descriptor: &descriptor,
            provider: &provider,
            environment: Some(&environment),
            offers: if other {
                vec!["codex-exec-server".into(), "plain".into()]
            } else {
                vec!["codex-exec-server".into()]
            },
            settings: &settings,
            entry: None,
            colocated: Vec::new(),
            runtime,
        },
        None,
    )
}

#[tokio::test]
async fn removing_confirmation_blocks_stale_codex_facts_for_same_any_and_range_with_a_distinct_runtime_host()
 {
    let world = World::observed().await;
    // Start the real fixture harness through its ordinary separated tools before changing the declaration.
    sqlx::query("UPDATE environments SET facts=$2 WHERE id=$1")
        .bind(world.environment)
        .bind(hosting::facts())
        .execute(world.pool.pool())
        .await
        .unwrap();
    let thread = world.thread("Real reported provider build").await;
    world
        .running_turn(thread, "Report the current runtime")
        .await;
    world.run_turn(thread, 0, "Done").await;
    world.declares(descriptor("same", false)).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let environment = fixtures::insert_environment(
        &mut conn,
        world.ada,
        Some(world.root),
        "Environment machine",
        "connected",
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_ne!(
        machine, world.machine_id,
        "the provider's runtime is a different machine"
    );
    sqlx::query("UPDATE machines SET roles=ARRAY['environment'],online=true,facts=$2 WHERE id=$1")
        .bind(machine).bind(json!({"policy":{"agent.harnesses":[{"kind":"codex","path":"/controlled/codex"}]},
            "harnesses":[{"kind":"codex","path":"/controlled/codex","version":"1.0.0","daemon":false}]}))
        .execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE environments SET status='running',facts=$2 WHERE id=$1")
        .bind(environment)
        .bind(executors())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    for build in ["same", "*", ">=1.0.0, <2.0.0"] {
        assert!(
            choices(&world, thread, environment, build, false)
                .await
                .iter()
                .any(|choice| choice.kind == ArrangementType::Separated && choice.is_open())
        );
    }
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(machine)
        .bind(json!({"policy":{"agent.harnesses":[]},"harnesses":[]}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let stale: Json = sqlx::query_scalar("SELECT facts FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        stale,
        executors(),
        "the environment has deliberately not published its withdrawal yet"
    );
    for build in ["same", "*", ">=1.0.0, <2.0.0"] {
        let choices = choices(&world, thread, environment, build, false).await;
        let choice = choices
            .iter()
            .find(|choice| choice.kind == ArrangementType::Separated)
            .unwrap();
        assert!(
            !choice.available,
            "removed confirmation cannot reuse a stale executor fact ({build})"
        );
        assert!(
            !choice
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("allow it"))
        );
    }
    assert!(
        choices(&world, thread, environment, "same", true)
            .await
            .iter()
            .any(|choice| choice.is_open()
                && choice
                    .arrangement
                    .as_ref()
                    .and_then(|found| found.executor.as_deref())
                    == Some("orch-exec"))
    );
    sqlx::query("UPDATE environments SET facts=$2 WHERE id=$1")
        .bind(world.environment)
        .bind(executors())
        .execute(world.pool.pool())
        .await
        .unwrap();
    for build in ["same", "*", ">=1.0.0, <2.0.0"] {
        assert!(
            choices(&world, thread, world.environment, build, false)
                .await
                .iter()
                .any(|choice| choice.is_open()),
            "a managed cache does not acquire a local confirmation requirement"
        );
    }
}
