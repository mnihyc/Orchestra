//! Host selection answers with the same driver-policy refusal that the node enforces.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod common;
mod hosting;
use common::World;
use hosting::ScriptedHost;
use orch_authz::{Caller, Principal};
use orch_db::queries::resources::hosting as rows;
use orch_engine::places::{self, Ask};
use orch_link_contract::policy::{LocalPolicy, PolicyValues};
use orch_types::Time;
use serde_json::json;

#[tokio::test]
async fn a_nodes_kept_image_rule_bounds_a_custom_runtime_and_names_the_policy_when_refused() {
    let world = World::new().await;
    let host = ScriptedHost::on(&world, "Custom node", 8192).await;
    let template = hosting::template(&world).await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE env_templates SET spec=spec || $2::jsonb WHERE id=$1")
        .bind(template)
        .bind(json!({"image":"registry.example.test/team/agent:1","runtime":"gvisor"}))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,runtime_classes}',$2) WHERE id=$1",
    )
    .bind(host.id)
    .bind(json!(["gvisor"]))
    .execute(&mut *conn)
    .await
    .unwrap();
    let mut policy =
        LocalPolicy::resolve(&["*".into()], &PolicyValues::new(), &PolicyValues::new()).unwrap();
    policy.drivers.images.images = vec!["registry.example.test/team/agent:1".into()];
    policy.drivers.images.registries = vec!["registry.example.test".into()];
    policy.drivers.images.runtime_classes = vec!["gvisor".into()];
    policy
        .local_values
        .insert("drivers".into(), json!(policy.drivers));
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',$2) WHERE id=$1")
        .bind(host.machine)
        .bind(json!(policy))
        .execute(&mut *conn)
        .await
        .unwrap();
    let template = rows::template(&mut conn, template).await.unwrap().unwrap();
    let wanted = places::wanted(&mut conn, world.topic, template, &json!({}))
        .await
        .unwrap();
    let now = world.clock.now();
    let principal = Principal::load(&mut conn, Caller::session(world.ada), Time::new(now))
        .await
        .unwrap();
    let ask = Ask {
        home: world.topic,
        wanted: &wanted,
        current: None,
        minutes_idle: 60,
        now,
    };
    let first = places::host_places(&mut conn, &principal, &ask)
        .await
        .unwrap();
    assert!(
        first
            .iter()
            .find(|place| place.host.id == host.id)
            .unwrap()
            .place
            .usable
    );
    policy.drivers.images.images = vec!["registry.example.test/other:1".into()];
    policy
        .local_values
        .insert("drivers".into(), json!(policy.drivers));
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',$2) WHERE id=$1")
        .bind(host.machine)
        .bind(json!(policy))
        .execute(&mut *conn)
        .await
        .unwrap();
    let second = places::host_places(&mut conn, &principal, &ask)
        .await
        .unwrap();
    let refused = &second
        .iter()
        .find(|place| place.host.id == host.id)
        .unwrap()
        .place;
    assert!(!refused.usable);
    let reason = refused.policy_refusal.as_deref().unwrap();
    assert!(
        reason.contains("drivers (node role)")
            && reason.contains("image")
            && reason.contains("operator"),
        "{reason}"
    );
}

#[tokio::test]
async fn a_prospective_unit_uses_its_target_driver_without_borrowing_an_unselected_machines_policy()
{
    use orch_db::queries::environment_facts;
    let world = World::new().await;
    let host = ScriptedHost::on(&world, "Unit node", 8192).await;
    let template = hosting::template(&world).await;
    let mut conn = world.pool.acquire().await.unwrap();
    // The scripted host declares what the Docker driver declares about confinement: its units isolate the
    // agent, which is what the environment is to learn from the driver before any machine is selected.
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,confinement}','\"isolated\"') WHERE id=$1",
    )
    .bind(host.id)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET template_id=$2,config='{}' WHERE id=$1")
        .bind(world.environment)
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
    let before = environment_facts::of(&mut conn, world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.confinement(), "isolated");
    assert!(
        before.machine_id.is_none() && before.policy.is_none(),
        "the driver is known before any physical machine is selected"
    );
    sqlx::query(
        "UPDATE environments SET config=jsonb_build_object('env.host_id',$2::text) WHERE id=$1",
    )
    .bind(world.environment)
    .bind(host.id.to_string())
    .execute(&mut *conn)
    .await
    .unwrap();
    let selected = environment_facts::of(&mut conn, world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.machine_id, Some(host.machine));
    assert!(selected.policy.is_some());
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state)
        VALUES($1,'environment',$2,$3,$4,'docker',$5,$6,'creating')")
        .bind(orch_types::new_id()).bind(host.id).bind(format!("prospective-{}",world.environment))
        .bind(world.environment).bind(template).bind(vec![0_u8;32]).execute(&mut *conn).await.unwrap();
    let created = environment_facts::of(&mut conn, world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        created, selected,
        "the selected unit keeps the same facts and ceiling from planning through creation"
    );
}
