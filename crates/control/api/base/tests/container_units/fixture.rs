//! The units that a host runs, as the system leaves them once the container of a provider or of an environment has enrolled: a runtime on the host, and a machine with no owner, home or page of its own whose `runtime_id` and `host_id` are set.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test program uses some of what is here")]

use chrono::{DateTime, Utc};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::Conn;
use orch_db::queries::resources::{access_fixtures, hosting, provider_runtime, testing};
use orch_types::{Id, Time, new_id};

/// A host that `owner` made and holds alone, at home in `home`, and a template for the runtimes
/// made on it.
pub async fn host_and_template(
    conn: &mut Conn,
    owner: Id,
    home: Id,
    name: &str,
    now: DateTime<Utc>,
) -> (Id, Id) {
    let host = access_fixtures::insert_resource(
        &mut *conn,
        "host",
        Some(owner),
        Some(home),
        &format!("{name} host"),
    )
    .await
    .unwrap();
    let template = access_fixtures::insert_resource(
        &mut *conn,
        "template",
        Some(owner),
        Some(home),
        &format!("{name} template"),
    )
    .await
    .unwrap();
    start_resource(
        &mut *conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        owner,
        NewAccess::OnlyMe,
        Time::new(now),
    )
    .await
    .unwrap();
    (host, template)
}

/// Runs `provider` in a container of `host`, as the system leaves it once the container has
/// enrolled: its runtime, the unit's machine, and the provider naming that machine as the one it
/// runs on and the one its key is on. Returns the unit's machine.
pub async fn provider_unit(
    conn: &mut Conn,
    host: Id,
    template: Id,
    provider: Id,
    now: DateTime<Utc>,
) -> Id {
    let unit = testing::insert_machine(&mut *conn, "orch-provider-unit", "managed")
        .await
        .unwrap();
    let runtime = new_id();
    provider_runtime::insert_runtime(
        &mut *conn,
        &provider_runtime::NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: &format!("orch-provider-{}", &runtime.simple().to_string()[20..]),
            provider_id: provider,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &serde_json::json!({}),
        },
        now,
    )
    .await
    .unwrap();
    hosting::adopt_machine(&mut *conn, unit, runtime, now)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET roles = ARRAY['provider','agent'] WHERE id = $1")
        .bind(unit)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET machine_id = $1, key_machine_id = $1 WHERE id = $2")
        .bind(unit)
        .bind(provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    unit
}

/// Runs `environment` in a container of `host`, as the system leaves it once the container has
/// enrolled: its runtime, the unit's machine, and the environment naming that machine as the one it
/// runs on. Returns the unit's machine.
pub async fn environment_unit(
    conn: &mut Conn,
    host: Id,
    template: Id,
    environment: Id,
    now: DateTime<Utc>,
) -> Id {
    let unit = testing::insert_machine(&mut *conn, "orch-environment-unit", "managed")
        .await
        .unwrap();
    let runtime = new_id();
    sqlx::query(
        "INSERT INTO runtimes (id, kind, host_id, environment_id, unit_name, driver, template_id,
                               spec_hash, state)
         VALUES ($1, 'environment', $2, $3, $4, 'docker', $5, decode(repeat('00', 32), 'hex'), 'running')",
    )
    .bind(runtime)
    .bind(host)
    .bind(environment)
    .bind(format!(
        "orch-environment-{}",
        &runtime.simple().to_string()[20..]
    ))
    .bind(template)
    .execute(&mut *conn)
    .await
    .unwrap();
    hosting::adopt_machine(&mut *conn, unit, runtime, now)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET roles = ARRAY['environment','agent'] WHERE id = $1")
        .bind(unit)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET machine_id = $1 WHERE id = $2")
        .bind(unit)
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    unit
}
