//! Helpers for the tests of what a thread may be put on: more providers, profiles and environments than the scene has, what an environment offers, and what a person may be named for.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::Who;
use crate::rig::{MODEL, Scene, descriptor};
use orch_db::queries::access::{NewEntry, PageKey, insert_entry};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

/// Makes `who` an owner of a thing, as its maker is.
async fn owned_by(s: &Scene, kind: &str, id: Id, who: &Who) {
    let mut conn = s.rig.db.acquire().await.unwrap();
    insert_entry(
        &mut conn,
        &NewEntry {
            id: new_id(),
            page: &PageKey::new(kind, id, "live"),
            profile: "any",
            permission: "access.own",
            holder_kind: "user",
            holder_id: Some(who.id()),
            holder_role: None,
            options: &json!({}),
            expires_at: None,
            note: "",
            source: "creation",
            created_by: Some(who.id()),
            at: s.rig.world.now(),
        },
    )
    .await
    .unwrap();
}

/// A second agent profile and a second provider of Ada's, homed in the org like the scene's own: a
/// subscription of the same harness on the scene's machine, with one model, allowing the
/// arrangement types `arrangements`. The profile reported the descriptor the scene's does.
pub async fn another_provider(s: &Scene, name: &str, arrangements: &[&str]) -> (Id, Id) {
    let root = s.tree.org.base.id;
    let (agent, provider) = (new_id(), new_id());
    let arrangements: Vec<String> = arrangements.iter().map(|each| (*each).to_owned()).collect();
    let mut conn = s.rig.db.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
         VALUES ($1, $2, $3, $4, $4, 'fake', '{}', $5)",
    )
    .bind(agent)
    .bind(format!("{name} agent"))
    .bind(s.ada.id())
    .bind(root)
    .bind(descriptor())
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                machine_id, arrangements, status)
         VALUES ($1, $2, $3, $4, $4, 'subscription', 'fake', $5, $6, $7, 'ready')",
    )
    .bind(provider)
    .bind(name)
    .bind(s.ada.id())
    .bind(root)
    .bind(agent)
    .bind(s.rig.machine_id)
    .bind(arrangements)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
         VALUES ($1, $2, $3, 'Fake One', $4, true, 'manual', $5)",
    )
    .bind(new_id())
    .bind(provider)
    .bind(MODEL)
    .bind(json!({ "efforts": ["low", "high"], "default_effort": "low" }))
    .bind(s.ada.id())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    owned_by(s, "agent", agent, &s.ada).await;
    owned_by(s, "provider", provider, &s.ada).await;
    (agent, provider)
}

/// A second environment of Ada's, made from a template of its own, running on `machine` (none:
/// on no machine), whose home is the topic of the scene.
pub async fn another_environment(s: &Scene, name: &str, machine: Option<Id>) -> Id {
    let (environment, template) = (new_id(), new_id());
    let mut conn = s.rig.db.acquire().await.unwrap();
    sqlx::query("INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, $2, 'test', '{}')")
        .bind(template)
        .bind(format!("{name} template"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root,
                                   machine_id, template_id, generation, status)
         VALUES ($1, $2, $3, $4, 'managed', $5, '/workspace', $6, $7, 1, 'running')",
    )
    .bind(environment)
    .bind(name)
    .bind(s.ada.id())
    .bind(s.tree.org.base.id)
    .bind(s.tree.topic.base.id)
    .bind(machine)
    .bind(template)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    owned_by(s, "environment", environment, &s.ada).await;
    environment
}

/// A directory of the scene's machine registered as an environment of Ada's: on a person's machine
/// its Manage row names nobody as shipped, so it is hers alone.
pub async fn a_directory_on_a_machine(s: &Scene, name: &str) -> Id {
    let environment = new_id();
    let mut conn = s.rig.db.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root,
                                   machine_id, generation, status)
         VALUES ($1, $2, $3, $4, 'connected', $5, '/work/ada', $6, 1, 'running')",
    )
    .bind(environment)
    .bind(name)
    .bind(s.ada.id())
    .bind(s.tree.org.base.id)
    .bind(s.tree.topic.base.id)
    .bind(s.rig.machine_id)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    owned_by(s, "environment", environment, &s.ada).await;
    environment
}

/// Makes an environment list the tool set `plain` of Orch's own executor among what its exec node
/// can offer, and offer it.
pub async fn offering_plain(s: &Scene, environment: Id) {
    let facts: Value = json!({
        "os": "linux", "arch": "x86_64", "shell": { "name": "bash", "path": "/bin/bash" },
        "user": "agent", "home": "/srv/agent", "workspace_root": "/workspace", "path_style": "posix",
        "case_sensitive": true,
        "executors": [{ "kind": "orch-exec", "version": "0.1.0", "available": true, "reason": null,
                        "sandbox_modes": ["read_only", "workspace", "open"], "tool_sets": ["plain"] }],
        "mcp_found": [], "index": "full", "mounts": [], "private_views": false, "suspend": false,
        "git": true, "generation": 1
    });
    sqlx::query("UPDATE environments SET facts = $2, config = $3 WHERE id = $1")
        .bind(environment)
        .bind(facts)
        .bind(json!({ "exec.offers": ["plain"] }))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

/// Makes a person's Use of a provider (or any other permission on any page) narrower: names them
/// with options.
pub async fn names(s: &Scene, kind: &str, id: Id, permission: &str, who: &Who, options: Value) {
    s.rig.lets_use(kind, id, permission, who, options).await;
}
