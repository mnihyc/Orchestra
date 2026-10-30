//! Resource writes use the caller's current authority after a delayed body or a database lock wait.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use axum::body::{Body, to_bytes};
use bytes::Bytes;
use chrono::Duration;
use futures::stream;
use http::{Request, StatusCode};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_core::registry::Registry;
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::access::{self, PageKey, testing};
use orch_db::queries::environments::hosting::{self, NewRuntime};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_db::secrets::MasterKey;
use orch_settings::Target;
use orch_types::{Id, Json, Time, new_id};
use places::{Who, join, make_org, person};
use serde_json::json;
use std::future::Future;
use std::time::Duration as Wait;
use support::World;
use tower::ServiceExt;

// Scheduling headroom on the shared machine; PostgreSQL lock state decides when to continue.
const LOCK_WAIT: Wait = Wait::from_secs(30);
// These replies are ordinary resource objects or problem details, within the API's JSON limit.
const REPLY_LIMIT: usize = orch_api_contract::common::MAX_JSON_BYTES;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Authority {
    Stays,
    Expires,
    Revoked,
}

struct Scene {
    world: World,
    owner: Who,
    writer: Who,
    home: Id,
}

impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        install_master_key(MasterKey::new([7; 32]));
        let owner = person(&world, "owner").await;
        let writer = person(&world, "writer").await;
        let home = make_org(&world, &owner, "Office", "office").await.base.id;
        join(&world, home, home, &writer, "viewer", &owner).await;
        Self {
            world,
            owner,
            writer,
            home,
        }
    }

    async fn resource(&self, kind: ResourceKind) -> Id {
        let mut conn = self.world.conn().await;
        let id = fixtures::insert_resource(
            &mut conn,
            kind.as_str(),
            Some(self.owner.id()),
            Some(self.home),
            "Original",
        )
        .await
        .unwrap();
        if kind == ResourceKind::Provider {
            // HTTP replies describe the endpoint, so give the fixture a registered protocol.
            sqlx::query("UPDATE providers SET upstreams = $2, protocols = ARRAY['openai-responses'] WHERE id = $1")
                .bind(id)
                .bind(json!([{"protocol":"openai-responses","base_url":"https://api.example.test","options":{"auth":"none"}}]))
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.into(),
                id,
            },
            self.owner.id(),
            NewAccess::Defaults,
            Time::new(self.world.now()),
        )
        .await
        .unwrap();
        id
    }

    async fn grant(&self, kind: &str, id: Id, permission: &str, authority: Authority) -> Id {
        testing::add_entry(
            &mut *self.world.conn().await,
            &PageKey::new(kind, id, "live"),
            "any",
            permission,
            ("user", Some(self.writer.id()), None),
            (authority == Authority::Expires).then(|| self.world.now() + Duration::seconds(1)),
            self.world.now(),
        )
        .await
        .unwrap()
    }

    async fn row(&self, kind: ResourceKind, id: Id) -> Json {
        let query = match kind {
            ResourceKind::Provider => "SELECT to_jsonb(r) FROM providers r WHERE id = $1",
            ResourceKind::Agent => "SELECT to_jsonb(r) FROM agents r WHERE id = $1",
            ResourceKind::Template => "SELECT to_jsonb(r) FROM env_templates r WHERE id = $1",
            ResourceKind::Host => "SELECT to_jsonb(r) FROM hosts r WHERE id = $1",
            _ => panic!("the tested resource kind"),
        };
        sqlx::query_scalar(query)
            .bind(id)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }

    async fn audits(&self, id: Id) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id = $1")
            .bind(id)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }

    async fn delayed(
        &self,
        method: &str,
        path: &str,
        value: Json,
        entry: Id,
        authority: Authority,
    ) -> (StatusCode, Json) {
        let pool = self.world.db.pool.clone();
        let clock = self.world.app.clock.clone();
        let owner = self.owner.id();
        let now = self.world.now();
        after(&self.world, &self.writer, method, path, value, async move {
            if authority == Authority::Revoked {
                let mut tx = pool.begin().await.unwrap();
                access::lock_system_defaults(&mut tx).await.unwrap();
                access::remove_entry(&mut tx, entry, owner, now)
                    .await
                    .unwrap();
                access::bump_epoch(&mut tx).await.unwrap();
                tx.commit().await.unwrap();
            }
            clock.advance(Duration::seconds(2));
        })
        .await
    }
}

fn router(world: &World) -> axum::Router {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    registry.into_router().with_state(world.app.app.clone())
}

fn count_query(table: &str) -> &'static str {
    match table {
        "providers" => "SELECT count(*) FROM providers",
        "agents" => "SELECT count(*) FROM agents",
        "env_templates" => "SELECT count(*) FROM env_templates",
        "enrollment_tokens" => "SELECT count(*) FROM enrollment_tokens",
        _ => panic!("the tested resource table"),
    }
}

fn request(who: &Who, method: &str, path: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", who.token))
        .header("content-type", "application/json")
        .body(body)
        .unwrap()
}

async fn reply(response: axum::response::Response) -> (StatusCode, Json) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), REPLY_LIMIT).await.unwrap();
    let value = if bytes.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

async fn after(
    world: &World,
    who: &Who,
    method: &str,
    path: &str,
    value: Json,
    change: impl Future<Output = ()> + Send + 'static,
) -> (StatusCode, Json) {
    // Authentication and the route permission have succeeded before this stream is polled.
    let body = Body::from_stream(stream::once(async move {
        change.await;
        Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&value).unwrap()))
    }));
    reply(
        router(world)
            .oneshot(request(who, method, path, body))
            .await
            .unwrap(),
    )
    .await
}

fn denied(status: StatusCode, value: &Json, permission: &str) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
    assert_eq!(value["code"], "authz.denied", "{value}");
    assert_eq!(value["required"], permission, "{value}");
    assert!(
        value["missing"].as_array().unwrap().iter().any(|missing| {
            missing["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|held| held == permission)
        }),
        "the refusal names the missing permission: {value}"
    );
}

#[tokio::test]
async fn every_resource_update_uses_manage_as_it_stands_after_the_body() {
    for kind in [
        ResourceKind::Provider,
        ResourceKind::Agent,
        ResourceKind::Template,
        ResourceKind::Host,
    ] {
        for authority in [Authority::Expires, Authority::Revoked, Authority::Stays] {
            let scene = Scene::new().await;
            let id = scene.resource(kind).await;
            let permission = format!("{}.manage", kind.as_str());
            let entry = scene.grant(kind.as_str(), id, &permission, authority).await;
            let before = scene.row(kind, id).await;
            let audits = scene.audits(id).await;
            // A host has a revision of its own, as the others do.
            let revision = before["revision"].as_i64().unwrap();
            let (status, value) = scene
                .delayed(
                    "PATCH",
                    &format!("/{}s/{id}", kind.as_str()),
                    json!({"name": "Changed", "expected_revision": revision}),
                    entry,
                    authority,
                )
                .await;
            let current = scene.row(kind, id).await;
            if authority == Authority::Stays {
                assert_eq!(status, StatusCode::OK, "{kind:?}: {value}");
                assert_eq!(current["name"], "Changed");
                assert_eq!(scene.audits(id).await, audits + 1);
            } else {
                denied(status, &value, &permission);
                assert_eq!(current, before, "{kind:?} after {authority:?}");
                assert_eq!(scene.audits(id).await, audits);
            }
        }
    }
}

#[tokio::test]
async fn keys_secret_headers_and_fact_overrides_require_current_provider_manage() {
    for operation in ["key", "secrets/X-Gateway", "facts-overrides"] {
        for authority in [Authority::Expires, Authority::Revoked, Authority::Stays] {
            let scene = Scene::new().await;
            let id = scene.resource(ResourceKind::Provider).await;
            sqlx::query("UPDATE providers SET upstreams = $2 WHERE id = $1")
                .bind(id).bind(json!([{"protocol":"openai-responses","base_url":"https://api.example.test","options":{"secret_headers":["X-Gateway"]}}]))
                .execute(&scene.world.db.pool).await.unwrap();
            let before = scene.row(ResourceKind::Provider, id).await;
            let audits = scene.audits(id).await;
            let entry = scene
                .grant("provider", id, "provider.manage", authority)
                .await;
            let value = match operation {
                "key" => json!({"api_key":"a-test-key"}),
                // Overrides name the revision of the provider that was reviewed.
                "facts-overrides" => {
                    json!({"plan":"Changed","expected_revision":before["revision"]})
                }
                _ => json!({"value":"a-test-header"}),
            };
            let (status, value) = scene
                .delayed(
                    "PUT",
                    &format!("/providers/{id}/{operation}"),
                    value,
                    entry,
                    authority,
                )
                .await;
            let secrets: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1",
            )
            .bind(id)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
            if authority == Authority::Stays {
                assert_eq!(
                    status,
                    // Setting a key answers with the provider as it now is; a secret header answers nothing.
                    if operation == "facts-overrides" || operation == "key" {
                        StatusCode::OK
                    } else {
                        StatusCode::NO_CONTENT
                    },
                    "{value}"
                );
                assert_eq!(secrets, i64::from(operation != "facts-overrides"));
                assert_eq!(scene.audits(id).await, audits + 1);
                if operation == "facts-overrides" {
                    assert_eq!(
                        scene.row(ResourceKind::Provider, id).await["facts_overrides"]["plan"],
                        "Changed"
                    );
                }
            } else {
                denied(status, &value, "provider.manage");
                assert_eq!(scene.row(ResourceKind::Provider, id).await, before);
                assert_eq!(secrets, 0);
                assert_eq!(scene.audits(id).await, audits);
            }
        }
    }
}

fn api_provider(home: Id) -> Json {
    json!({"kind":"api","name":"Created","home_id":home,"upstreams":[{"protocol":"openai-responses","base_url":"https://api.example.test","options":{"auth":"none"}}]})
}

#[tokio::test]
async fn creating_resources_rechecks_the_rules_of_their_home_after_the_body() {
    for operation in ["provider", "subscription", "agent", "template", "host"] {
        for authority in [Authority::Expires, Authority::Revoked, Authority::Stays] {
            let scene = Scene::new().await;
            scene
                .world
                .set_at(
                    Target::Node(scene.home),
                    "resources.created_by_members",
                    json!([]),
                )
                .await;
            let entry = scene
                .grant("node", scene.home, "settings.manage", authority)
                .await;
            let (path, body, table) = match operation {
                "provider" => (
                    "/providers".to_owned(),
                    api_provider(scene.home),
                    "providers",
                ),
                "subscription" => {
                    let agent = scene.resource(ResourceKind::Agent).await;
                    scene
                        .grant("agent", agent, "agent.use", Authority::Stays)
                        .await;
                    (
                        "/providers".to_owned(),
                        json!({"kind":"subscription","name":"Created","home_id":scene.home,"agent_id":agent}),
                        "providers",
                    )
                }
                "agent" => (
                    format!("/orgs/{}/agents", scene.home),
                    json!({"name":"Created","harness_kind":"fake","profile":{}}),
                    "agents",
                ),
                "template" => {
                    // A template is checked against its driver's schema as a host reports it.
                    let host = scene.resource(ResourceKind::Host).await;
                    sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
                        .bind(host)
                        .bind(json!({"docker": {"name": "docker", "title": "Docker",
                            "spec_schemas": {"environment": {}, "provider": {}}}}))
                        .execute(&scene.world.db.pool)
                        .await
                        .unwrap();
                    (
                        "/templates".to_owned(),
                        json!({"name":"Created","home_id":scene.home,"driver":"docker","spec":{}}),
                        "env_templates",
                    )
                }
                _ => (
                    "/hosts/enrollment-tokens".to_owned(),
                    json!({"home_id":scene.home,"name_hint":"Created"}),
                    "enrollment_tokens",
                ),
            };
            let count = count_query(table);
            let before: i64 = sqlx::query_scalar(count)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            let (status, value) = scene.delayed("POST", &path, body, entry, authority).await;
            let current: i64 = sqlx::query_scalar(count)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            if authority == Authority::Stays {
                assert_eq!(status, StatusCode::OK, "{operation}: {value}");
                assert_eq!(current, before + 1);
            } else {
                denied(status, &value, "settings.manage");
                assert_eq!(current, before, "{operation} after {authority:?}");
            }
        }
    }
}

#[tokio::test]
async fn a_subscription_creator_needs_current_use_of_the_profile_that_holds_its_login() {
    for authority in [Authority::Expires, Authority::Revoked, Authority::Stays] {
        let scene = Scene::new().await;
        let agent = scene.resource(ResourceKind::Agent).await;
        let entry = scene.grant("agent", agent, "agent.use", authority).await;
        let (status, value) = scene.delayed("POST", "/providers",
            json!({"kind":"subscription","name":"Created","home_id":scene.home,"agent_id":agent}), entry, authority).await;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM providers")
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        if authority == Authority::Stays {
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_eq!(count, 1);
        } else {
            denied(status, &value, "agent.use");
            assert_eq!(count, 0);
        }
    }
}

#[tokio::test]
async fn system_hosts_system_templates_and_restricted_drivers_need_current_administration() {
    for operation in ["system-host", "system-template", "restricted-driver"] {
        for revoke in [true, false] {
            let scene = Scene::new().await;
            scene.world.make_admin(&scene.owner.person).await;
            scene.world.make_admin(&scene.writer.person).await;
            // A system admin acts as one with the session's As admin switch on.
            places::admin_mode(&scene.world, &scene.owner, true).await;
            places::admin_mode(&scene.world, &scene.writer, true).await;
            if operation != "system-host" {
                // A template is checked against its driver's schema as a host reports it.
                let host = scene.resource(ResourceKind::Host).await;
                sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
                    .bind(host)
                    .bind(json!({
                        "docker": {"name": "docker", "title": "Docker",
                            "spec_schemas": {"environment": {}, "provider": {}}},
                        "host": {"name": "host", "title": "Host",
                            "spec_schemas": {"environment": {}, "provider": {}}}
                    }))
                    .execute(&scene.world.db.pool)
                    .await
                    .unwrap();
            }
            let (path, body, table) = match operation {
                "system-host" => (
                    "/hosts/enrollment-tokens",
                    json!({"home_id":null}),
                    "enrollment_tokens",
                ),
                "system-template" => (
                    "/templates",
                    json!({"name":"Created","home_id":null,"driver":"docker","spec":{}}),
                    "env_templates",
                ),
                _ => (
                    "/templates",
                    json!({"name":"Created","home_id":scene.home,"driver":"host","spec":{}}),
                    "env_templates",
                ),
            };
            let pool = scene.world.db.pool.clone();
            let writer = scene.writer.id();
            let (status, value) = after(
                &scene.world,
                &scene.writer,
                "POST",
                path,
                body,
                async move {
                    if revoke {
                        sqlx::query("UPDATE users SET is_system_admin = false WHERE id = $1")
                            .bind(writer)
                            .execute(&pool)
                            .await
                            .unwrap();
                    }
                },
            )
            .await;
            let count: i64 = sqlx::query_scalar(count_query(table))
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            if revoke {
                assert_eq!(status, StatusCode::FORBIDDEN, "{operation}: {value}");
                assert_eq!(value["code"], "authz.denied");
                assert_eq!(value["required"], "system.admin");
                assert_eq!(count, 0);
            } else {
                assert_eq!(status, StatusCode::OK, "{operation}: {value}");
                assert_eq!(count, 1);
            }
        }
    }
}

async fn waiting_action(pool: &sqlx::PgPool) {
    tokio::time::timeout(LOCK_WAIT, async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid
                  WHERE a.datname = current_database() AND NOT l.granted)",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the HTTP action reached a PostgreSQL lock");
}

async fn after_lock(scene: &Scene, method: &str, path: &str) -> (StatusCode, Json) {
    let mut held = scene.world.db.pool.begin().await.unwrap();
    // The request waits on the acting person's guard row, without a shared barrier.
    orch_db::queries::transaction_guard::lock_person(&mut held, scene.writer.id())
        .await
        .unwrap();
    let router = router(&scene.world);
    let request = request(&scene.writer, method, path, Body::empty());
    let action = tokio::spawn(async move { reply(router.oneshot(request).await.unwrap()).await });
    waiting_action(&scene.world.db.pool).await;
    scene.world.advance(Duration::seconds(2));
    held.commit().await.unwrap();
    tokio::time::timeout(LOCK_WAIT, action)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn bodyless_deletes_and_provider_switches_check_manage_after_waiting() {
    for (kind, operation) in [
        (ResourceKind::Provider, "delete"),
        (ResourceKind::Provider, "disable"),
        (ResourceKind::Provider, "enable"),
        (ResourceKind::Agent, "delete"),
        (ResourceKind::Template, "delete"),
    ] {
        for authority in [Authority::Expires, Authority::Stays] {
            let scene = Scene::new().await;
            let id = scene.resource(kind).await;
            if operation == "enable" {
                sqlx::query("UPDATE providers SET status = 'disabled' WHERE id = $1")
                    .bind(id)
                    .execute(&scene.world.db.pool)
                    .await
                    .unwrap();
            }
            let permission = format!("{}.manage", kind.as_str());
            scene.grant(kind.as_str(), id, &permission, authority).await;
            let before = scene.row(kind, id).await;
            let audits = scene.audits(id).await;
            let path = format!(
                "/{}s/{id}{}",
                kind.as_str(),
                if operation == "delete" {
                    String::new()
                } else {
                    format!("/{operation}")
                }
            );
            let (status, value) = after_lock(
                &scene,
                if operation == "delete" {
                    "DELETE"
                } else {
                    "POST"
                },
                &path,
            )
            .await;
            let current = scene.row(kind, id).await;
            if authority == Authority::Stays {
                assert_eq!(
                    status,
                    if operation == "delete" {
                        StatusCode::NO_CONTENT
                    } else {
                        StatusCode::OK
                    },
                    "{kind:?} {operation}: {value}"
                );
                assert_ne!(current, before);
                assert_eq!(scene.audits(id).await, audits + 1);
                if operation == "delete" {
                    assert!(!current["deleted_at"].is_null());
                }
                if operation == "disable" {
                    assert_eq!(current["status"], "disabled");
                }
                if operation == "enable" {
                    assert_ne!(current["status"], "disabled");
                }
            } else {
                denied(status, &value, &permission);
                assert_eq!(current, before);
                assert_eq!(scene.audits(id).await, audits);
            }
        }
    }
}

#[tokio::test]
async fn a_resource_change_waiting_for_authority_records_the_time_of_its_action() {
    let scene = Scene::new().await;
    let id = scene.resource(ResourceKind::Agent).await;
    scene
        .grant("agent", id, "agent.manage", Authority::Stays)
        .await;
    let before = scene.row(ResourceKind::Agent, id).await;
    let mut held = scene.world.db.pool.begin().await.unwrap();
    // The request waits on the acting person's guard row, without a shared barrier.
    orch_db::queries::transaction_guard::lock_person(&mut held, scene.writer.id())
        .await
        .unwrap();
    let router = router(&scene.world);
    let body = json!({"name":"Changed", "expected_revision":before["revision"]});
    let request = request(
        &scene.writer,
        "PATCH",
        &format!("/agents/{id}"),
        Body::from(serde_json::to_vec(&body).unwrap()),
    );
    let action = tokio::spawn(async move { reply(router.oneshot(request).await.unwrap()).await });
    waiting_action(&scene.world.db.pool).await;
    scene.world.advance(Duration::seconds(2));
    held.commit().await.unwrap();
    let (status, value) = tokio::time::timeout(LOCK_WAIT, action)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{value}");
    let (at, actor): (chrono::DateTime<chrono::Utc>, Id) = sqlx::query_as(
        "SELECT at, actor_id FROM audit_log WHERE object_id = $1 AND action = 'agent.updated'",
    )
    .bind(id)
    .fetch_one(&scene.world.db.pool)
    .await
    .unwrap();
    let updated: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM agents WHERE id = $1")
            .bind(id)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
    assert_eq!(at, scene.world.now());
    assert_eq!(updated, scene.world.now());
    assert_eq!(actor, scene.writer.id());
}

#[tokio::test]
async fn removing_a_hosts_runtime_rechecks_host_manage_for_units_and_volumes() {
    for volumes in [false, true] {
        for authority in [Authority::Expires, Authority::Stays] {
            let scene = Scene::new().await;
            let host = scene.resource(ResourceKind::Host).await;
            scene.grant("host", host, "host.manage", authority).await;
            let runtime = new_id();
            let environment = {
                let mut conn = scene.world.conn().await;
                let environment = fixtures::insert_environment(
                    &mut conn,
                    scene.owner.id(),
                    Some(scene.home),
                    "Work",
                    "managed",
                )
                .await
                .unwrap();
                let template: Id =
                    sqlx::query_scalar("SELECT template_id FROM environments WHERE id = $1")
                        .bind(environment)
                        .fetch_one(&mut *conn)
                        .await
                        .unwrap();
                hosting::insert_runtime(
                    &mut conn,
                    &NewRuntime {
                        id: runtime,
                        host_id: host,
                        unit_name: "work-unit",
                        environment_id: environment,
                        driver: "test",
                        template_id: template,
                        spec_hash: &[0; 32],
                        generation: 1,
                        volumes: &json!({}),
                    },
                    scene.world.now(),
                )
                .await
                .unwrap();
                environment
            };
            let state = "SELECT jsonb_build_object('runtime', to_jsonb(r), 'environment', to_jsonb(e)) FROM runtimes r JOIN environments e ON e.id = r.environment_id WHERE r.id = $1";
            let before: Json = sqlx::query_scalar(state)
                .bind(runtime)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            let (status, value) = after_lock(
                &scene,
                "DELETE",
                &format!("/hosts/{host}/runtimes/{runtime}?confirm=work-unit&volumes={volumes}"),
            )
            .await;
            let current: Json = sqlx::query_scalar(state)
                .bind(runtime)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            if authority == Authority::Stays {
                assert_eq!(status, StatusCode::NO_CONTENT, "{value}");
                assert_ne!(current, before);
                if volumes {
                    assert!(!current["environment"]["remove_at"].is_null());
                } else {
                    assert_eq!(current["runtime"]["state"], "retired");
                }
                assert_eq!(scene.audits(host).await, 1);
            } else {
                denied(status, &value, "host.manage");
                assert_eq!(current, before);
                assert_eq!(scene.audits(host).await, 0);
            }
            let remains: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM environments WHERE id = $1)")
                    .bind(environment)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            assert!(
                remains,
                "a unit removal preserves the environment's durable record"
            );
        }
    }
}

#[tokio::test]
async fn a_connected_resource_registration_rechecks_its_owners_identity_after_the_body() {
    for operation in ["agents", "providers"] {
        for suspended in [true, false] {
            let scene = Scene::new().await;
            let machine = resources::machine_forwarding_to(
                &scene.world,
                scene.writer.id(),
                &["https://api.example.test"],
            )
            .await;
            // A harness is registered where the machine reports it can run it: here one that needs no
            // separate program.
            sqlx::query("UPDATE machines SET roles = ARRAY['agent','provider'], facts = facts || $2 WHERE id = $1")
                .bind(machine).bind(json!({"adapters":[{"kind":"fake","needs_binary":false}]})).execute(&scene.world.db.pool).await.unwrap();
            let home = scene.writer.person.personal_org;
            let body = if operation == "agents" {
                json!({"name":"Created","home_id":home,"harness_kind":"fake","profile":{}})
            } else {
                json!({"kind":"api","name":"Created","home_id":home,"upstreams":[{"protocol":"openai-responses","base_url":"https://api.example.test","options":{}}]})
            };
            let pool = scene.world.db.pool.clone();
            let writer = scene.writer.id();
            let (status, value) = after(
                &scene.world,
                &scene.writer,
                "POST",
                &format!("/machines/{machine}/{operation}"),
                body,
                async move {
                    if suspended {
                        sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
                            .bind(writer)
                            .execute(&pool)
                            .await
                            .unwrap();
                    }
                },
            )
            .await;
            let count: i64 = sqlx::query_scalar(count_query(operation))
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            if suspended {
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{value}");
                assert_eq!(value["code"], "auth.suspended");
                assert_eq!(count, 0);
            } else {
                assert_eq!(status, StatusCode::OK, "{operation}: {value}");
                assert_eq!(count, 1);
            }
        }
    }
}

async fn models_of(scene: &Scene, provider: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM provider_models WHERE provider_id = $1")
        .bind(provider)
        .fetch_one(&scene.world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_providers_models_change_only_with_manage_as_it_stands_once_the_body_has_come() {
    for authority in [Authority::Expires, Authority::Revoked, Authority::Stays] {
        let scene = Scene::new().await;
        let id = scene.resource(ResourceKind::Provider).await;
        let entry = scene
            .grant("provider", id, "provider.manage", authority)
            .await;
        let (status, value) = scene
            .delayed(
                "POST",
                &format!("/providers/{id}/models"),
                json!({"model_name": "small-1"}),
                entry,
                authority,
            )
            .await;
        if authority == Authority::Stays {
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_eq!(models_of(&scene, id).await, 1);
        } else {
            denied(status, &value, "provider.manage");
            assert_eq!(models_of(&scene, id).await, 0, "{authority:?}");
        }
    }
}

#[tokio::test]
async fn a_providers_model_is_removed_only_with_manage_as_it_stands_after_waiting() {
    for authority in [Authority::Expires, Authority::Stays] {
        let scene = Scene::new().await;
        let id = scene.resource(ResourceKind::Provider).await;
        let model = scene
            .world
            .app
            .call(orch_api_contract::resources::ProviderModelCreate, &[&id])
            .token(&scene.owner.token)
            .body(&orch_api_contract::resources::ProviderModelCreateBody {
                model_name: "small-1".to_owned(),
                display_name: None,
                parameters: None,
                groups: None,
            })
            .send()
            .await
            .ok()
            .id;
        scene
            .grant("provider", id, "provider.manage", authority)
            .await;
        let (status, value) =
            after_lock(&scene, "DELETE", &format!("/providers/{id}/models/{model}")).await;
        if authority == Authority::Stays {
            assert_eq!(status, StatusCode::NO_CONTENT, "{value}");
            assert_eq!(models_of(&scene, id).await, 0);
        } else {
            denied(status, &value, "provider.manage");
            assert_eq!(models_of(&scene, id).await, 1);
        }
    }
}

#[tokio::test]
async fn an_environment_is_kept_awake_only_by_whoever_manages_it_after_waiting() {
    for authority in [Authority::Expires, Authority::Stays] {
        let scene = Scene::new().await;
        let environment = {
            let mut conn = scene.world.conn().await;
            let environment = fixtures::insert_environment(
                &mut conn,
                scene.owner.id(),
                Some(scene.home),
                "Work",
                "managed",
            )
            .await
            .unwrap();
            start_resource(
                &mut conn,
                ObjectRef {
                    kind: ResourceKind::Environment.into(),
                    id: environment,
                },
                scene.owner.id(),
                NewAccess::Defaults,
                Time::new(scene.world.now()),
            )
            .await
            .unwrap();
            environment
        };
        scene
            .grant("environment", environment, "environment.manage", authority)
            .await;
        let activity = "SELECT last_activity_at::text FROM environments WHERE id = $1";
        let before: Option<String> = sqlx::query_scalar(activity)
            .bind(environment)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        let (status, value) = after_lock(
            &scene,
            "POST",
            &format!("/environments/{environment}/keep-awake"),
        )
        .await;
        let after: Option<String> = sqlx::query_scalar(activity)
            .bind(environment)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        if authority == Authority::Stays {
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_ne!(after, before, "it counts as activity now");
        } else {
            denied(status, &value, "environment.manage");
            assert_eq!(after, before);
        }
    }
}

/// As [`after_lock`], with the writer suspended while the request waits at its guard.
async fn after_lock_suspending(scene: &Scene, method: &str, path: &str) -> (StatusCode, Json) {
    let mut held = scene.world.db.pool.begin().await.unwrap();
    // The request waits on the acting person's guard row, without a shared barrier.
    orch_db::queries::transaction_guard::lock_person(&mut held, scene.writer.id())
        .await
        .unwrap();
    let router = router(&scene.world);
    let request = request(&scene.writer, method, path, Body::empty());
    let action = tokio::spawn(async move { reply(router.oneshot(request).await.unwrap()).await });
    waiting_action(&scene.world.db.pool).await;
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(scene.writer.id())
        .execute(&mut *held)
        .await
        .unwrap();
    held.commit().await.unwrap();
    tokio::time::timeout(LOCK_WAIT, action)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn a_machine_is_revoked_only_by_its_owner_as_they_stand_after_waiting() {
    for suspended in [true, false] {
        let scene = Scene::new().await;
        let machine = resources::machine_forwarding_to(
            &scene.world,
            scene.writer.id(),
            &["https://api.example.test"],
        )
        .await;
        let path = format!("/machines/{machine}/revoke");
        let (status, value) = if suspended {
            after_lock_suspending(&scene, "POST", &path).await
        } else {
            after_lock(&scene, "POST", &path).await
        };
        let state: String = sqlx::query_scalar("SELECT status FROM machines WHERE id = $1")
            .bind(machine)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        if suspended {
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{value}");
            assert_eq!(value["code"], "auth.suspended");
            assert_eq!(state, "active");
        } else {
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_eq!(state, "revoked");
        }
    }
}
