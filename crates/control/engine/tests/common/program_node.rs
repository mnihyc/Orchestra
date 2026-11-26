//! The real host, driver report and provider runtime used by isolated image cases.
use chrono::{DateTime, Utc};
use orch_db::queries::resources::{self, NewHost, provider_runtime};
use orch_types::{Hash, Id, new_id};
use serde_json::json;

pub async fn placement(
    conn: &mut orch_db::Conn,
    node: Id,
    peer: Id,
    provider: Id,
    owner: Id,
    org: Id,
    now: DateTime<Utc>,
) -> Result<Id, orch_db::DbError> {
    let host = new_id();
    let template = new_id();
    let runtime = new_id();
    resources::insert_host(
        conn,
        &NewHost {
            id: host,
            machine_id: node,
            owner_id: Some(owner),
            home_id: Some(org),
            name: "Actual program placement",
        },
        now,
    )
    .await?;
    let mut driver = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "1",
        Default::default(),
    );
    driver.name = "recorded-unit-driver".into();
    driver.lifecycle = orch_driver_contract::descriptor::Lifecycle::Unit;
    driver.program_units = orch_driver_contract::descriptor::OperationSupport::available(true, "");
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(host)
        .bind(json!({"recorded-unit-driver":driver}))
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO env_templates(id,owner_id,org_id,home_id,name,kind,driver,spec) VALUES($1,$2,$3,$3,'Recorded program source','provider','recorded-unit-driver',$4)")
        .bind(template).bind(owner).bind(org).bind(json!({"image":"original-source"})).execute(&mut *conn).await?;
    provider_runtime::insert_runtime(
        conn,
        &provider_runtime::NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: "recorded-provider-unit",
            provider_id: provider,
            driver: "recorded-unit-driver",
            template_id: template,
            spec_hash: Hash::of(b"recorded-source").as_bytes(),
            generation: 1,
            volumes: &json!({}),
        },
        now,
    )
    .await?;
    sqlx::query("UPDATE runtimes SET machine_id=$2,state='running' WHERE id=$1")
        .bind(runtime)
        .bind(peer)
        .execute(&mut *conn)
        .await?;
    Ok(host)
}
