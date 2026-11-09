//! Small fixture inputs shared by creator deep-branch HTTP controls.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test setup fails by panicking"
)]
use crate::places::Who;
use crate::rig::{MODEL, Scene};
use crate::support::World;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::nodes::{
    CreateThread, CreateThreadBinding, ThreadCreate, TopicCreate, TopicCreateBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::Time;
use orch_types::{Id, Json, new_id};
use serde_json::json;

pub(crate) fn rule(
    op: &str,
    kind: &str,
    id: Id,
    reach: &str,
    resources: &[&str],
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,"allow":[{"operation":op,"actions":actions}]})
}
pub(crate) async fn key(s: &Scene, actor: Id, rules: Vec<Json>) -> String {
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let scope = serde_json::to_value(scope).unwrap();
    create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Creator control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    secret
}
pub(crate) fn body(s: &Scene) -> CreateThread {
    CreateThread {
        name: Some("Scoped creator".into()),
        binding: Some(CreateThreadBinding {
            agent_id: s.on.agent,
            provider_id: s.on.provider,
            arrangement: None,
        }),
        model: Some(MODEL.into()),
        environment: Some(EnvironmentChoice::Use(s.on.environment)),
        prepare_source: Some(false),
        agent_access: Some(orch_api_contract::nodes::AgentAccessChoice {
            where_: orch_api_contract::nodes::AgentAccessWhere::Nothing,
            parts: Default::default(),
        }),
        ..Default::default()
    }
}
pub(crate) async fn thread(s: &Scene, token: &str, parent: Id, body: &CreateThread) -> (u16, Json) {
    let request = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&parent])
        .token(token)
        .body(body);
    let response = request.send_bytes().await;
    (
        response.status.as_u16(),
        serde_json::from_slice(&response.raw).unwrap(),
    )
}
pub(crate) async fn topic(s: &Scene, token: &str, body: &CreateThread) -> (u16, Json) {
    let request = s
        .rig
        .world
        .app
        .call(TopicCreate, &[&s.tree.project.base.id])
        .token(token)
        .body(&TopicCreateBody {
            name: Some("Compound creator".into()),
            first_thread: Some(body.clone()),
            ..Default::default()
        });
    let response = request.send_bytes().await;
    (
        response.status.as_u16(),
        serde_json::from_slice(&response.raw).unwrap(),
    )
}
pub(crate) async fn count(s: &Scene, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind=$1")
        .bind(kind)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

async fn a_host(world: &World, owner: &Who, home: Id, name: &str, memory_mb: i64) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(&mut conn, "host", Some(owner.id()), Some(home), name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id,
        },
        owner.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE hosts SET drivers = $2, capacity = $3, storages = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(json!({ "docker": {
        "name": "docker", "title": "Docker",
        "spec_schemas": { "environment": {}, "provider": {} },
        "runtime_classes": ["runc"], "sources": ["image", "build", "archive"]
    }}))
    .bind(json!({ "cpus": 8, "memory_mb": memory_mb, "disk_gb": 500 }))
    .bind(json!([{ "name": "fast", "default": true, "capacity_gb": 500, "free_gb": 300, "private_views": false }]))
    .execute(&mut *conn)
    .await
    .unwrap();
    // Its node holds a session, and has reported the drivers its operator lets it offer.
    sqlx::query(
        "UPDATE machines SET online = true,
                facts = jsonb_set(facts, '{policy}', COALESCE(facts -> 'policy', '{}') || $2)
          WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)",
    )
    .bind(id)
    .bind(json!({
        "drivers": serde_json::to_value(orch_link_contract::policy::MachineDrivers::managed())
            .unwrap()
    }))
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

/// A template of environments that a Docker host makes from a small image.
async fn a_template(world: &World, owner: &Who, home: Id, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(&mut conn, "template", Some(owner.id()), Some(home), name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Template.into(),
            id,
        },
        owner.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE env_templates SET driver = 'docker', spec = $2 WHERE id = $1")
        .bind(id)
        .bind(json!({
            "image": "busybox:latest", "runtime": "runc",
            "user": { "name": "agent", "uid": 1000, "gid": 1000 },
            "resources": { "cpu_millis": 2000, "memory_mb": 2048, "pids": 512, "disk_gb": 20 },
            "needs": { "cpu_millis": 500, "memory_mb": 1024, "disk_gb": 5 },
            "exec": { "offers": ["orch-exec"], "orch_tools": true },
            "colocated_harnesses": [], "host_labels": [], "init": null
        }))
        .execute(&mut *conn)
        .await
        .unwrap();
    id
}

pub(crate) async fn managed(s: &crate::rig::Scene) -> (Id, Id) {
    let home = s.tree.project.base.org_id;
    let host = a_host(&s.rig.world, &s.ada, home, "Creator host", 8192).await;
    let template = a_template(&s.rig.world, &s.ada, home, "Creator template").await;
    (host, template)
}
