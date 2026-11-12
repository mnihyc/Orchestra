//! The environment page has the same native sandbox explanation as the machine page.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use orch_api_contract::{environment::EnvironmentGet, resources::MachineGet};
use orch_db::queries::resources::access_fixtures as fixtures;
use serde_json::json;

#[tokio::test]
async fn an_environment_reports_its_actual_sandbox_limits_before_any_thread_exists() {
    let world = support::World::new().await;
    let owner = places::person(&world, "owner").await;
    let mut conn = world.conn().await;
    let machine = fixtures::insert_persons_machine(&mut conn, owner.id(), "Machine")
        .await
        .unwrap();
    let environment = fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(owner.id()),
        Some(owner.person.personal_org),
        "Workspace",
    )
    .await
    .unwrap();
    fixtures::register_on(&mut conn, "environment", environment, machine)
        .await
        .unwrap();
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: None,
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts || $2,'{policy}',COALESCE(facts->'policy','{}') || $3) WHERE id=$1")
        .bind(machine).bind(json!({"environment_descriptor":descriptor}))
        .bind(json!({"agent.max_sandbox":"workspace"})).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE environments SET placement='connected',template_id=NULL,config=$2,facts=$3 WHERE id=$1")
        .bind(environment).bind(json!({"exec.offers":["plain"]}))
        .bind(json!({"executors":[{"kind":"orch-exec","available":true,"sandbox_modes":["open"],"tool_sets":["plain"]}]}))
        .execute(&mut *conn).await.unwrap();
    orch_db::queries::settings::put(
        &mut conn,
        Some(owner.person.personal_org),
        "sandbox.mode",
        &orch_db::queries::settings::Stored {
            value: &json!("open"),
            locked: false,
            by: Some(owner.id()),
            at: world.now(),
        },
        None,
    )
    .await
    .unwrap();
    drop(conn);
    let before = world
        .app
        .call(EnvironmentGet, &[&environment])
        .token(&owner.token)
        .send()
        .await
        .ok();
    let report = before
        .sandbox_readiness
        .expect("full environment page readiness");
    assert_eq!(report.environment.id, environment);
    assert_eq!(
        report.machine.as_ref().map(|machine| machine.id),
        Some(machine)
    );
    assert!(!report.sandbox.ready);
    let machine_view = world
        .app
        .call(MachineGet, &[&machine])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(
        machine_view
            .environment_sandboxes
            .iter()
            .find(|entry| entry.environment.id == environment)
            .unwrap(),
        &report
    );
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,agent.max_sandbox}','\"open\"') WHERE id=$1")
        .bind(machine).execute(&world.db.pool).await.unwrap();
    let after = world
        .app
        .call(EnvironmentGet, &[&environment])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(after.sandbox_readiness.unwrap().sandbox.ready);
    let threads: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(threads, 0);
}
