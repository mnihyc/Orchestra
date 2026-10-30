//! Accepted live resource limits reach the existing unit and become its recorded current definition.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
mod hosting;

use common::World;
use orch_link_contract::commands::CommandKind;
use serde_json::json;

#[tokio::test]
async fn live_limits_are_acknowledged_on_the_same_runtime_without_a_replacement_or_stop() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "live-resources", 16_384).await;
    let template = hosting::template(&world).await;
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,live_changes}',$2) WHERE id=$1",
    )
    .bind(host.id)
    .bind(json!({"resources":true,"mounts":false,"capabilities":[],"gateway_given":[]}))
    .execute(world.pool.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE env_templates SET spec=jsonb_set(spec,'{resources,memory_mb}','256') WHERE id=$1",
    )
    .bind(template)
    .execute(world.pool.pool())
    .await
    .unwrap();
    let (environment, runtime, _) = hosting::running(&world, &host, template, "Live limits").await;
    sqlx::query(
        "UPDATE environments SET config=jsonb_set(config,'{env.resources}',$2) WHERE id=$1",
    )
    .bind(environment)
    .bind(json!({"memory_mb":512,"pids":128,"memory_swap_mb":512,"blkio_weight":600}))
    .execute(world.pool.pool())
    .await
    .unwrap();
    assert!(
        world
            .engine()
            .apply_environment_resources(environment)
            .await
            .unwrap()
    );
    assert_eq!(host.count(CommandKind::NodeResourcesApply), 1);
    // This world's command double keeps its journal in memory. Persist the actual successful
    // delivery before asking the production read model, which uses only durable acknowledgments.
    let kind = CommandKind::NodeResourcesApply.as_str();
    let command = world
        .commands
        .kinds()
        .into_iter()
        .find(|(_, name)| name == kind)
        .unwrap()
        .0;
    let (machine, body) = world.commands.issued_bodies(kind).pop().unwrap();
    assert_eq!(
        body["resources"]["memory_mb"], 512,
        "the existing unit was actually asked for the new limit"
    );
    sqlx::query("INSERT INTO commands(id,machine_id,kind,body,state,result,deadline_at) VALUES($1,$2,$3,$4,'succeeded','{}',$5)")
        .bind(command).bind(machine).bind(kind).bind(body).bind(world.clock.now()+chrono::Duration::seconds(i64::from(CommandKind::NodeResourcesApply.deadline_secs()))).execute(world.pool.pool()).await.unwrap();
    for kind in [
        CommandKind::NodeRuntimeCreate,
        CommandKind::NodeRuntimeStop,
        CommandKind::NodeRuntimeStart,
        CommandKind::NodeRuntimeRemove,
    ] {
        assert_eq!(host.count(kind), 0);
    }
    let mut conn = world.pool.acquire().await.unwrap();
    let held = orch_db::queries::environments::hosting::live_runtime(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.id, runtime);
    let one = orch_db::queries::jobs::changes::definition(&mut conn, runtime)
        .await
        .unwrap()
        .unwrap();
    let bulk = orch_db::queries::jobs::changes::definitions(&mut conn, &[runtime])
        .await
        .unwrap();
    assert_eq!(bulk[0].1.as_ref(), Some(&one));
    assert_eq!(one["resources"]["memory_mb"], 512);
    assert_eq!(one["resources"]["blkio_weight"], 600);
    drop(conn);
    let form = world
        .engine()
        .runtime_changes(orch_engine::updates::Target::Environment(environment))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(form.memory_used_bytes, Some(64 << 20));
    assert!(form.live_changes.resources);
    sqlx::query("UPDATE environments SET config=jsonb_set(config,'{env.resources,disk_gb}','40') WHERE id=$1").bind(environment).execute(world.pool.pool()).await.unwrap();
    assert!(
        !world
            .engine()
            .apply_environment_resources(environment)
            .await
            .unwrap()
    );
    assert_eq!(
        host.count(CommandKind::NodeResourcesApply),
        1,
        "a creation-fixed disk change is left for a reviewed recreate"
    );
    host.refuse(
        CommandKind::NodeResourcesApply,
        orch_link_contract::commands::CommandError::new(
            orch_link_contract::commands::ErrorCode::Unsupported,
            "this clear requires recreation",
        ),
    );
    sqlx::query(
        "UPDATE environments SET config=jsonb_set(config,'{env.resources}',$2) WHERE id=$1",
    )
    .bind(environment)
    .bind(json!({"memory_mb":640}))
    .execute(world.pool.pool())
    .await
    .unwrap();
    assert!(
        !world
            .engine()
            .apply_environment_resources(environment)
            .await
            .unwrap()
    );
    let form = world
        .engine()
        .runtime_changes(orch_engine::updates::Target::Environment(environment))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        form.pending_reason.as_deref(),
        Some("this clear requires recreation")
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let held = orch_db::queries::jobs::changes::definition(&mut conn, runtime)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        held["resources"]["memory_mb"], 512,
        "a refused desired limit never becomes the current acknowledged limit"
    );
    world.engine().stop();
}
