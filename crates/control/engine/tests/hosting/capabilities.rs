//! Existing prepared-live capability setup, shared by integration and owned native authority controls.
use super as hosting;
use crate::common::World;
use orch_types::{Id, Json};
use serde_json::json;

pub async fn declared_drivers(world: &World, host: &hosting::ScriptedHost) {
    let mut docker: Json = world
        .scalar("SELECT drivers->'docker' FROM hosts WHERE id=$1", host.id)
        .await;
    docker["version"] = json!("0.1.0");
    docker["confinement"] = json!("isolated");
    docker["process_scope"] = json!("all");
    docker["network_egress"] = json!(["open"]);
    docker["recreate"] = json!("replace");
    docker["capabilities"] =
        json!({"ptrace":{},"kvm":{},"gpu":{},"net_admin":{},"privileged":{},"unconfined":{}});
    let docker: orch_driver_contract::descriptor::DriverDescriptor =
        serde_json::from_value(docker).unwrap();
    let mut programs = json!(docker);
    programs["name"] = json!("host");
    programs["title"] = json!("Programs on the host");
    programs["confinement"] = json!("none");
    programs["capabilities"] = json!({});
    let programs: orch_driver_contract::descriptor::DriverDescriptor =
        serde_json::from_value(programs).unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(host.id)
        .bind(json!({"docker":docker,"host":programs}))
        .execute(world.pool.pool())
        .await
        .unwrap();
}

pub async fn prepared_live_environment(
    world: &World,
) -> (hosting::ScriptedHost, Id, Id, hosting::Unit) {
    hosting::serve(world);
    let host = hosting::ScriptedHost::on(world, "live-capabilities", 16_384).await;
    hosting::physical::report(world, host.machine, false, Some(true), "open").await;
    declared_drivers(world, &host).await;
    let template = hosting::template(world).await;
    sqlx::query("UPDATE env_templates SET spec=spec || $2 WHERE id=$1")
        .bind(template)
        .bind(json!({"allowed_capabilities":["kvm"],"base_capabilities":[]}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE capability_defs SET driver_patch=$2 WHERE key=$1")
        .bind("kvm")
        .bind(json!({"docker":{"devices":[{"path":"/dev/kvm","permissions":"rwm"}]}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,live_changes}',$2) WHERE id=$1").bind(host.id)
        .bind(json!({"resources":true,"mounts":true,"capabilities":["kvm","docker_socket"],"gateway_given":["port_place","port_host","port_link"]})).execute(world.pool.pool()).await.unwrap();
    let (environment, runtime, unit) =
        hosting::running(world, &host, template, "Prepared live unit").await;
    let capabilities = json!({"allowed":["kvm"],"base":[],"granted":[],"patches":{"kvm":{"devices":[{"path":"/dev/kvm","permissions":"rwm"}]}}});
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{created_spec,capabilities}',$2) WHERE environment_id=$1
        AND kind='environment_provision' AND state='succeeded' AND plan->>'runtime'=$3")
        .bind(environment).bind(capabilities).bind(runtime.to_string()).execute(world.pool.pool()).await.unwrap();
    (host, environment, runtime, unit)
}
