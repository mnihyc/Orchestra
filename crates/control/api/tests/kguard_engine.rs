//! Engine actions need current request authority, and machine commands follow the committed decision.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use bytes::Bytes;
use chrono::Duration;
use futures::stream;
use http::{Request, StatusCode};
use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::{Machine, Played};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::access::{PageKey, testing};
use orch_db::queries::environments::hosting::{self, NewRuntime};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::{access_fixtures as fixtures, provider_runtime};
use orch_link_contract::commands::{Cmd, CommandError, CommandKind};
use orch_types::{Id, Json, ManualClock, Time, new_id};
use places::{Who, join, make_org, person};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as Wait;
use support::World;
use tower::ServiceExt;

// Scheduling headroom on the shared machine; PostgreSQL's actual wait decides the order.
const LOCK_WAIT: Wait = Wait::from_secs(90);

#[test]
fn every_new_changing_request_needs_an_explicit_review_of_its_action_authority() {
    // The ledger the registry serves by: a `guarded` row is held to its guard at run time by every
    // test that serves the request (see kguard_rest.rs); this gate keeps the ledger whole and honest.
    let reviewed: Json =
        serde_json::from_str(orch_api_core::middleware::transaction_guard::LEDGER).unwrap();
    let records = reviewed.as_array().unwrap();
    let names: BTreeSet<&str> = records
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains("ProviderSourcesSet"),
        "choosing a detail source is a reviewed change of provider authority"
    );
    assert_eq!(
        names.len(),
        records.len(),
        "a request is reviewed exactly once"
    );
    for row in records {
        assert!(matches!(
            row["boundary"].as_str(),
            Some("guarded" | "open" | "separate")
        ));
        let reason = row["reason"].as_str().unwrap();
        assert!(!reason.is_empty());
        assert!(
            !reason.contains("needs an authority review"),
            "{} must say where its authority is asked, not that it was not looked at",
            row["name"]
        );
    }
    let current: BTreeSet<&str> = orch_api_contract::routes::all()
        .into_iter()
        .filter(|route| route.method.as_str() != "GET")
        .map(|route| route.name)
        .collect();
    assert_eq!(
        names, current,
        "a new changing request must name and review the authority boundary before it ships"
    );
}

#[test]
fn a_request_is_left_outside_the_guard_only_when_nobody_is_signed_in_to_ask() {
    use orch_api_contract::routes::Access;
    use orch_api_core::middleware::transaction_guard::{Boundary, boundary_of};
    for route in orch_api_contract::routes::all()
        .into_iter()
        .filter(|route| route.method.as_str() != "GET")
    {
        let nobody = matches!(route.access, Access::Public | Access::Cookie);
        let boundary = boundary_of(route.name);
        assert_eq!(
            boundary == Boundary::Separate,
            nobody,
            "{}: only a request without a signed-in caller is separate from the guard",
            route.name
        );
    }
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

    async fn resource(&self, kind: ResourceKind, home: Id, owner: Id) -> Id {
        let mut conn = self.world.conn().await;
        let id = fixtures::insert_resource(
            &mut conn,
            kind.as_str(),
            Some(owner),
            Some(home),
            "Original",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.into(),
                id,
            },
            owner,
            NewAccess::Defaults,
            Time::new(self.world.now()),
        )
        .await
        .unwrap();
        id
    }

    async fn grant(&self, kind: &str, id: Id, permission: &str, ends: bool) {
        testing::add_entry(
            &mut *self.world.conn().await,
            &PageKey::new(kind, id, "live"),
            "any",
            permission,
            ("user", Some(self.writer.id()), None),
            ends.then(|| self.world.now() + Duration::seconds(1)),
            self.world.now(),
        )
        .await
        .unwrap();
    }

    async fn machine(&self, owner: Id) -> Id {
        let mut conn = self.world.conn().await;
        let id = fixtures::insert_persons_machine(&mut conn, owner, "laptop")
            .await
            .unwrap();
        // A connector reports its adapters, a harness that needs no program, and the directory runtime
        // service a provider's runtime on it uses.
        sqlx::query("UPDATE machines SET online = true, roles = ARRAY['agent','provider'], facts = $2 WHERE id = $1")
            .bind(id)
            .bind(json!({"adapters":[{"kind":"fake","needs_binary":false}],
                "provider_runtime_descriptor":{"lifecycle":"attach"}}))
            .execute(&mut *conn).await.unwrap();
        id
    }

    async fn provider(&self, managed: bool) -> (Id, Id, Id) {
        let machine = self.machine(self.owner.id()).await;
        let agent = self
            .resource(ResourceKind::Agent, self.home, self.owner.id())
            .await;
        let provider = self
            .resource(ResourceKind::Provider, self.home, self.owner.id())
            .await;
        sqlx::query("UPDATE agents SET descriptor = $2, machine_id = $3 WHERE id = $1")
            .bind(agent)
            .bind(descriptor())
            .bind(machine)
            .execute(&self.world.db.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE providers SET kind = 'subscription', harness_kind = 'fake', agent_id = $2,
            machine_id = $3, upstreams = '[]', protocols = '{}',
            arrangements = ARRAY['separated'], facts = $4, login_state = $5 WHERE id = $1")
            .bind(provider).bind(agent).bind(machine).bind(json!({"plan":"before","windows":[]}))
            .bind(json!({"state":"completed","method":"import","shows":[],"asks":null,"expires_at":null,"error":null}))
            .execute(&self.world.db.pool).await.unwrap();
        if managed {
            let host = self
                .resource(ResourceKind::Host, self.home, self.owner.id())
                .await;
            let template = self
                .resource(ResourceKind::Template, self.home, self.owner.id())
                .await;
            let runtime = new_id();
            let mut conn = self.world.conn().await;
            sqlx::query("UPDATE env_templates SET kind = 'provider' WHERE id = $1")
                .bind(template)
                .execute(&mut *conn)
                .await
                .unwrap();
            provider_runtime::insert_runtime(
                &mut conn,
                &provider_runtime::NewRuntime {
                    id: runtime,
                    host_id: host,
                    unit_name: "provider-unit",
                    provider_id: provider,
                    driver: "test",
                    template_id: template,
                    spec_hash: &[0; 32],
                    generation: 1,
                    volumes: &json!({}),
                },
                self.world.now(),
            )
            .await
            .unwrap();
            // Its machine stays the connector of its running unit, as provisioning sets it; none while
            // the unit is not up.
            sqlx::query("UPDATE providers SET runtime_config = $2 WHERE id = $1")
                .bind(provider)
                .bind(json!({"runtime.template_id":template}))
                .execute(&mut *conn)
                .await
                .unwrap();
            orch_db::queries::resources::hosting::adopt_machine(
                &mut conn,
                machine,
                runtime,
                self.world.now(),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE runtimes SET state = 'running' WHERE id = $1")
                .bind(runtime)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        (provider, agent, machine)
    }

    async fn environment(&self, host: Id, name: &str, state: &str) -> (Id, Id) {
        let mut conn = self.world.conn().await;
        let environment = fixtures::insert_environment(
            &mut conn,
            self.owner.id(),
            Some(self.home),
            name,
            "managed",
        )
        .await
        .unwrap();
        let template = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let runtime = new_id();
        hosting::insert_runtime(
            &mut conn,
            &NewRuntime {
                id: runtime,
                host_id: host,
                unit_name: name,
                environment_id: environment,
                driver: "test",
                template_id: template,
                spec_hash: &[0; 32],
                generation: 1,
                volumes: &json!({}),
            },
            self.world.now(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE runtimes SET state=$2 WHERE id=$1")
            .bind(runtime)
            .bind(state)
            .execute(&mut *conn)
            .await
            .unwrap();
        // Its last activity is on the test's clock: the row's own default is the database's.
        sqlx::query("UPDATE environments SET status=$2, last_activity_at=$3 WHERE id=$1")
            .bind(environment)
            .bind(state)
            .bind(self.world.now())
            .execute(&mut *conn)
            .await
            .unwrap();
        (environment, runtime)
    }

    fn play(&self, machine: Id) -> Played {
        self.world.app.play(
            machine,
            Arc::new(Answer {
                pool: self.world.db.pool.clone(),
                signed_out: AtomicBool::new(false),
            }),
        )
    }
}

struct Answer {
    pool: sqlx::PgPool,
    signed_out: AtomicBool,
}

#[async_trait]
impl Machine for Answer {
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        let provider = command
            .body
            .get("provider_id")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok());
        if let Some(provider) = provider {
            let accepted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log
                WHERE object_id = $1 AND (details->>'decision' = 'accepted' OR action = 'provider.login'))")
                .bind(provider).fetch_one(&self.pool).await.unwrap();
            assert!(
                accepted,
                "the decision is visible to another connection before the machine is told"
            );
        }
        Ok(match command.kind.parse::<CommandKind>().unwrap() {
            CommandKind::AgentStart => json!({"harness_version":"1.0.0","descriptor":descriptor()}),
            CommandKind::ProviderLogout => {
                self.signed_out.store(true, Ordering::SeqCst);
                json!({})
            }
            CommandKind::ProviderRefresh => {
                if self.signed_out.load(Ordering::SeqCst) {
                    json!({"status":"needs_setup","facts":{}})
                } else {
                    json!({"status":"ready","facts":{"plan":"after","windows":[]}})
                }
            }
            CommandKind::NodeRuntimeStop => {
                let runtime = command.body["runtime_id"]
                    .as_str()
                    .unwrap()
                    .parse::<Id>()
                    .unwrap();
                // The decision is on what the unit serves, or, for a host manager's stop of a
                // provider's unit, on the host naming the unit.
                let accepted: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM audit_log a JOIN runtimes r ON r.id = $1
                    WHERE a.details->>'decision' = 'accepted'
                      AND (a.object_id = coalesce(r.environment_id, r.provider_id)
                           OR (a.object_id = r.host_id AND a.details->>'runtime' = r.id::text)))",
                )
                .bind(runtime)
                .fetch_one(&self.pool)
                .await
                .unwrap();
                assert!(accepted, "the host sees a committed stop decision");
                json!({})
            }
            _ => json!({}),
        })
    }
}

fn descriptor() -> Json {
    let mut descriptor = rig::descriptor();
    descriptor["native_provider"]["login_methods"] = json!([{
        "id":"device_code", "title":"Code", "summary":"Sign in with a code.",
        "kind":"interactive", "fields":[], "yields":"login", "lifetime_secs":null,
        "requires":null, "recommended":true
    }]);
    descriptor
}

fn router(world: &World) -> axum::Router {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    registry.into_router().with_state(world.app.app.clone())
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
    let body = to_bytes(
        response.into_body(),
        orch_api_contract::common::MAX_JSON_BYTES,
    )
    .await
    .unwrap();
    (
        status,
        if body.is_empty() {
            Json::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        },
    )
}

async fn delayed(scene: &Scene, method: &str, path: &str, value: Json) -> (StatusCode, Json) {
    let clock = scene.world.app.clock.clone();
    let body = Body::from_stream(stream::once(async move {
        clock.advance(Duration::seconds(2));
        Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&value).unwrap()))
    }));
    reply(
        router(&scene.world)
            .oneshot(request(&scene.writer, method, path, body))
            .await
            .unwrap(),
    )
    .await
}

async fn after_lock(scene: &Scene, method: &str, path: &str) -> (StatusCode, Json) {
    let body = if path.ends_with("/refresh") {
        Body::from("{}")
    } else {
        Body::empty()
    };
    locked_request(scene, method, path, body).await
}

async fn locked_request(scene: &Scene, method: &str, path: &str, body: Body) -> (StatusCode, Json) {
    let mut block = scene.world.db.pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *block)
        .await
        .unwrap();
    // The request waits at its guard for the acting person's own row, without a shared barrier.
    orch_db::queries::transaction_guard::lock_person(&mut block, scene.writer.id())
        .await
        .unwrap();
    let action =
        tokio::spawn(router(&scene.world).oneshot(request(&scene.writer, method, path, body)));
    tokio::time::timeout(LOCK_WAIT, async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(pid)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            assert!(
                !action.is_finished(),
                "the request must reach its action's authority lock"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real request reached its authority lock");
    scene.world.advance(Duration::seconds(2));
    block.commit().await.unwrap();
    reply(action.await.unwrap().unwrap()).await
}

fn denied(status: StatusCode, value: &Json, permission: &str) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
    assert_eq!(value["code"], "authz.denied", "{value}");
    assert_eq!(value["required"], permission, "{value}");
    assert!(
        value["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|missing| missing["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == permission)),
        "{value}"
    );
}

async fn no_command_was_queued(scene: &Scene) {
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&scene.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        queued, 0,
        "refusal leaves no command in the durable spool either"
    );
}

#[tokio::test]
async fn a_host_manager_whose_body_outlasts_manage_stops_nothing_and_a_current_manager_stops_the_unit()
 {
    for live in [false, true] {
        for ends in [true, false] {
            let scene = Scene::new().await;
            let host = scene
                .resource(ResourceKind::Host, scene.home, scene.owner.id())
                .await;
            scene.grant("host", host, "host.manage", ends).await;
            // The host reports the driver its environment's unit was made with: what it offers says
            // that the environment has a unit to stop, once that unit is retired too.
            sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
                .bind(host)
                .bind(json!({"test": {"name": "test", "version": "1", "title": "Test", "spec_schemas": {"environment": {}}}}))
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
            let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id = $1")
                .bind(host)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            let played = scene.play(machine);
            let (environment, runtime) = {
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
                let runtime = new_id();
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
                sqlx::query("UPDATE runtimes SET state = $2 WHERE id = $1")
                    .bind(runtime)
                    .bind(if live { "running" } else { "retired" })
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                sqlx::query("UPDATE environments SET status = 'running' WHERE id = $1")
                    .bind(environment)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                (environment, runtime)
            };
            let before: Json =
                sqlx::query_scalar("SELECT to_jsonb(e) FROM environments e WHERE id = $1")
                    .bind(environment)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            let (status, value) = delayed(
                &scene,
                "POST",
                &format!("/hosts/{host}/runtimes/{runtime}/stop"),
                json!({"interrupt":false}),
            )
            .await;
            let current: Json =
                sqlx::query_scalar("SELECT to_jsonb(e) FROM environments e WHERE id = $1")
                    .bind(environment)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            if ends {
                denied(status, &value, "host.manage");
                assert_eq!(current, before);
                assert!(played.seen().is_empty());
                let decisions: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id = $1")
                        .bind(environment)
                        .fetch_one(&scene.world.db.pool)
                        .await
                        .unwrap();
                assert_eq!(decisions, 0);
                no_command_was_queued(&scene).await;
            } else {
                assert_eq!(status, StatusCode::OK, "{value}");
                assert_eq!(current["status"], "stopped");
                assert_eq!(
                    played.seen_of(CommandKind::NodeRuntimeStop).len(),
                    usize::from(live)
                );
                let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT at FROM audit_log WHERE object_id = $1 AND details->>'decision' = 'accepted'")
                    .bind(environment).fetch_one(&scene.world.db.pool).await.unwrap();
                assert_eq!(at, scene.world.now());
            }
        }
    }
}

#[tokio::test]
async fn a_start_and_an_empty_host_removal_use_the_guard_clock_and_caller_after_the_authority_wait()
{
    for removing in [false, true] {
        for ends in [true, false] {
            let scene = Scene::new().await;
            let host = scene
                .resource(ResourceKind::Host, scene.home, scene.owner.id())
                .await;
            let environment = if removing {
                scene.grant("host", host, "host.manage", ends).await;
                None
            } else {
                let (environment, _) = scene.environment(host, "stopped-unit", "stopped").await;
                scene
                    .grant("environment", environment, "environment.manage", ends)
                    .await;
                Some(environment)
            };
            let path = environment.map_or_else(
                || format!("/hosts/{host}?confirm=Original"),
                |id| format!("/environments/{id}/start"),
            );
            let (status, value) =
                after_lock(&scene, if removing { "DELETE" } else { "POST" }, &path).await;
            if ends {
                denied(
                    status,
                    &value,
                    if removing {
                        "host.manage"
                    } else {
                        "environment.manage"
                    },
                );
                no_command_was_queued(&scene).await;
                let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
                assert_eq!(jobs, 0);
                let removed: bool = sqlx::query_scalar("SELECT m.deleted_at IS NOT NULL FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1")
                    .bind(host).fetch_one(&scene.world.db.pool).await.unwrap();
                assert!(!removed);
            } else if let Some(environment) = environment {
                assert_eq!(status, StatusCode::ACCEPTED, "{value}");
                let (by, at): (Option<Id>, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
                    "SELECT created_by,created_at FROM jobs WHERE environment_id=$1",
                )
                .bind(environment)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
                assert_eq!(by, Some(scene.writer.id()));
                assert_eq!(at, scene.world.now());
            } else {
                assert_eq!(status, StatusCode::NO_CONTENT, "{value}");
                let (by, at): (Option<Id>, chrono::DateTime<chrono::Utc>) = sqlx::query_as("SELECT actor_id,at FROM audit_log WHERE object_id=$1 AND action='host.deleted'")
                    .bind(host).fetch_one(&scene.world.db.pool).await.unwrap();
                assert_eq!(by, Some(scene.writer.id()));
                assert_eq!(at, scene.world.now());
            }
        }
    }
}

struct TimePassesOnStop(ManualClock);

#[async_trait]
impl Machine for TimePassesOnStop {
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        if command.kind.parse::<CommandKind>().unwrap() == CommandKind::NodeRuntimeStop {
            self.0.advance(Duration::seconds(2));
        }
        Ok(json!({}))
    }
}

#[tokio::test]
async fn a_recycle_rechecks_authority_for_each_stop_after_an_earlier_unit_took_time() {
    for on_host in [false, true] {
        for ends in [true, false] {
            let scene = Scene::new().await;
            let host = scene
                .resource(ResourceKind::Host, scene.home, scene.owner.id())
                .await;
            let (a, _) = scene.environment(host, "recycle-one", "running").await;
            let (b, _) = scene.environment(host, "recycle-two", "running").await;
            if on_host {
                scene.grant("host", host, "host.manage", ends).await;
            } else {
                for environment in [a, b] {
                    scene
                        .grant("environment", environment, "environment.manage", ends)
                        .await;
                }
            }
            let node: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
                .bind(host)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            let played = scene.world.app.play(
                node,
                Arc::new(TimePassesOnStop(scene.world.app.clock.clone())),
            );
            let path = if on_host {
                format!("/hosts/{host}/recycle")
            } else {
                format!("/nodes/{}/environments/recycle", scene.home)
            };
            let (status, value) = reply(
                router(&scene.world)
                    .oneshot(request(
                        &scene.writer,
                        "POST",
                        &path,
                        Body::from(
                            json!({"idle_minutes":0,"dry_run":false,"include_with_processes":true})
                                .to_string(),
                        ),
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            if ends && on_host {
                denied(status, &value, "host.manage");
            } else {
                assert_eq!(status, StatusCode::OK, "{value}");
                assert_eq!(value["stopped"], if ends { 1 } else { 2 });
                if ends {
                    assert_eq!(value["skipped_no_permission"], 1);
                }
            }
            assert_eq!(
                played.seen_of(CommandKind::NodeRuntimeStop).len(),
                if ends { 1 } else { 2 }
            );
            let stopped: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM environments WHERE id=ANY($1) AND status='stopped'",
            )
            .bind([a, b].as_slice())
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
            assert_eq!(stopped, if ends { 1 } else { 2 });
        }
    }
}

#[tokio::test]
async fn a_host_manager_stops_the_selected_provider_unit_with_current_authority_even_when_it_is_kept()
 {
    for kept in [false, true] {
        for ends in [true, false] {
            let scene = Scene::new().await;
            let (provider, _agent, _machine) = scene.provider(true).await;
            let (runtime, host, template): (Id, Id, Id) =
                sqlx::query_as("SELECT id,host_id,template_id FROM runtimes WHERE provider_id=$1")
                    .bind(provider)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            let mut successor = None;
            if kept {
                let other_host = scene
                    .resource(ResourceKind::Host, scene.home, scene.owner.id())
                    .await;
                let mut conn = scene.world.conn().await;
                sqlx::query("UPDATE runtimes SET state='retired' WHERE id=$1")
                    .bind(runtime)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                let next = new_id();
                provider_runtime::insert_runtime(
                    &mut conn,
                    &provider_runtime::NewRuntime {
                        id: next,
                        host_id: other_host,
                        unit_name: "new-provider-unit",
                        provider_id: provider,
                        driver: "test",
                        template_id: template,
                        spec_hash: &[1; 32],
                        generation: 2,
                        volumes: &json!({}),
                    },
                    scene.world.now(),
                )
                .await
                .unwrap();
                sqlx::query("UPDATE runtimes SET state='running' WHERE id=$1")
                    .bind(next)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                successor = Some(next);
            }
            scene.grant("host", host, "host.manage", ends).await;
            let node: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
                .bind(host)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            let played = scene.play(node);
            let before: Json =
                sqlx::query_scalar("SELECT to_jsonb(p) FROM providers p WHERE id=$1")
                    .bind(provider)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            let (status, value) = delayed(
                &scene,
                "POST",
                &format!("/hosts/{host}/runtimes/{runtime}/stop"),
                json!({"interrupt":false}),
            )
            .await;
            let current: Json =
                sqlx::query_scalar("SELECT to_jsonb(p) FROM providers p WHERE id=$1")
                    .bind(provider)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            if ends {
                denied(status, &value, "host.manage");
                assert_eq!(current, before);
                assert!(played.seen().is_empty());
                no_command_was_queued(&scene).await;
            } else {
                assert_eq!(status, StatusCode::OK, "{value}");
                let stopped = played.seen_of(CommandKind::NodeRuntimeStop);
                assert_eq!(stopped.len(), 1);
                assert_eq!(stopped[0].body["runtime_id"], runtime.to_string());
                assert_eq!(value["state"], if kept { "retired" } else { "stopped" });
                if let Some(next) = successor {
                    assert_eq!(current["machine_id"], before["machine_id"]);
                    let next_state: String =
                        sqlx::query_scalar("SELECT state FROM runtimes WHERE id=$1")
                            .bind(next)
                            .fetch_one(&scene.world.db.pool)
                            .await
                            .unwrap();
                    assert_eq!(next_state, "running");
                }
                let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT at FROM audit_log WHERE object_id=$1 AND details->>'runtime'=$2 AND details->>'decision'='accepted'")
                    .bind(host).bind(runtime.to_string()).fetch_one(&scene.world.db.pool).await.unwrap();
                assert_eq!(at, scene.world.now());
            }
        }
    }
}

#[tokio::test]
async fn refresh_restart_sign_out_and_deletion_need_permission_at_the_engine_decision() {
    for operation in ["refresh", "restart", "logout", "delete"] {
        for ends in [true, false] {
            let scene = Scene::new().await;
            let (provider, agent, machine) = scene
                .provider(matches!(operation, "logout" | "delete"))
                .await;
            let permission = if operation == "refresh" {
                "provider.view"
            } else {
                "provider.manage"
            };
            scene.grant("provider", provider, permission, ends).await;
            let played = scene.play(machine);
            let before: Json =
                sqlx::query_scalar("SELECT to_jsonb(p) FROM providers p WHERE id = $1")
                    .bind(provider)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            let (method, path) = match operation {
                "restart" => (
                    "POST",
                    format!("/providers/{provider}/harnesses/{agent}/restart"),
                ),
                "delete" => ("DELETE", format!("/providers/{provider}")),
                _ => ("POST", format!("/providers/{provider}/{operation}")),
            };
            let (status, value) = after_lock(&scene, method, &path).await;
            let current: Json =
                sqlx::query_scalar("SELECT to_jsonb(p) FROM providers p WHERE id = $1")
                    .bind(provider)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            if ends {
                denied(status, &value, permission);
                assert_eq!(
                    current, before,
                    "{operation} is refused before any state changes"
                );
                assert!(played.seen().is_empty());
                let decisions: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id = $1")
                        .bind(provider)
                        .fetch_one(&scene.world.db.pool)
                        .await
                        .unwrap();
                assert_eq!(decisions, 0);
                no_command_was_queued(&scene).await;
            } else {
                assert_eq!(
                    status,
                    if operation == "delete" {
                        StatusCode::NO_CONTENT
                    } else {
                        StatusCode::OK
                    },
                    "{operation}: {value}"
                );
                let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT at FROM audit_log WHERE object_id = $1 AND details->>'decision' = 'accepted'")
                    .bind(provider).fetch_one(&scene.world.db.pool).await.unwrap();
                assert_eq!(
                    at,
                    scene.world.now(),
                    "the accepted decision uses the time after the guard's wait"
                );
                match operation {
                    "refresh" => assert_eq!(current["facts"]["plan"], "after"),
                    "restart" => assert_eq!(played.seen_of(CommandKind::AgentStop).len(), 1),
                    "logout" => assert_eq!(current["status"], "needs_setup"),
                    "delete" => assert!(!current["deleted_at"].is_null()),
                    _ => unreachable!(),
                }
                if matches!(operation, "logout" | "delete") {
                    assert_eq!(played.seen_of(CommandKind::ProviderLogout).len(), 1);
                }
            }
        }
    }
}

#[tokio::test]
async fn a_connected_subscription_checks_its_owner_after_the_body() {
    for ends in [true, false] {
        let scene = Scene::new().await;
        let home = scene.writer.person.personal_org;
        let machine = scene.machine(scene.writer.id()).await;
        let agent = scene
            .resource(ResourceKind::Agent, home, scene.writer.id())
            .await;
        sqlx::query("UPDATE agents SET machine_id = $2, descriptor = $3 WHERE id = $1")
            .bind(agent)
            .bind(machine)
            .bind(rig::descriptor())
            .execute(&scene.world.db.pool)
            .await
            .unwrap();
        if ends {
            sqlx::query("UPDATE access_tokens SET expires_at = $2 WHERE token_hash = $1")
                .bind(token_hash(&scene.writer.token))
                .bind(scene.world.now() + Duration::seconds(1))
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        }
        let played = scene.play(machine);
        let (status, value) = delayed(
            &scene,
            "POST",
            &format!("/machines/{machine}/providers"),
            json!({"kind":"subscription","name":"Found","home_id":home,"agent_id":agent}),
        )
        .await;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM providers WHERE deleted_at IS NULL")
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
        if ends {
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{value}");
            assert_eq!(value["code"], "auth.invalid_token");
            assert_eq!(count, 0);
            assert!(played.seen().is_empty());
            no_command_was_queued(&scene).await;
        } else {
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_eq!(count, 1);
            // It is the sign-in the machine reports; registering it asks the harness nothing.
            assert!(played.seen().is_empty());
            let at: chrono::DateTime<chrono::Utc> =
                sqlx::query_scalar("SELECT at FROM audit_log WHERE action = 'provider.created'")
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            assert_eq!(at, scene.world.now(), "the guard's time, after the body");
        }
    }
}

#[tokio::test]
async fn runtime_starts_stops_and_sign_in_changes_need_current_manage_before_jobs_secrets_or_commands()
 {
    for operation in ["start", "stop", "login", "login/input", "login/cancel"] {
        for ends in [true, false] {
            let scene = Scene::new().await;
            let (provider, _, machine) = scene.provider(true).await;
            scene
                .grant("provider", provider, "provider.manage", ends)
                .await;
            let played = scene.play(machine);
            let host_machine: Id = sqlx::query_scalar(
                "SELECT h.machine_id FROM hosts h JOIN runtimes r
                ON r.host_id = h.id WHERE r.provider_id = $1",
            )
            .bind(provider)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
            let host_played = scene.play(host_machine);
            if operation == "start" {
                sqlx::query("UPDATE runtimes SET state = 'stopped' WHERE provider_id = $1")
                    .bind(provider)
                    .execute(&scene.world.db.pool)
                    .await
                    .unwrap();
                sqlx::query("UPDATE providers SET machine_id = NULL WHERE id = $1")
                    .bind(provider)
                    .execute(&scene.world.db.pool)
                    .await
                    .unwrap();
            }
            if operation == "login/input" {
                sqlx::query("UPDATE providers SET login_state = $2 WHERE id = $1")
                    .bind(provider).bind(json!({"state":"awaiting_user","method":"device_code","shows":[],
                        "asks":[{"name":"answer","label":"Answer","type":"text","required":true,
                            "help":null,"choices":null,"default":null,"max_bytes":null}],"expires_at":null,"error":null}))
                    .execute(&scene.world.db.pool).await.unwrap();
            }
            let before: Json =
                sqlx::query_scalar("SELECT to_jsonb(p) FROM providers p WHERE id = $1")
                    .bind(provider)
                    .fetch_one(&scene.world.db.pool)
                    .await
                    .unwrap();
            let (status, value) = if operation == "login/cancel" {
                after_lock(
                    &scene,
                    "POST",
                    &format!("/providers/{provider}/{operation}"),
                )
                .await
            } else {
                let body = match operation {
                    "login" => json!({"method":"device_code"}),
                    "login/input" => json!({"values":{"answer":"given"}}),
                    _ => json!({"interrupt":false}),
                };
                delayed(
                    &scene,
                    "POST",
                    &format!("/providers/{provider}/{operation}"),
                    body,
                )
                .await
            };
            if ends {
                denied(status, &value, "provider.manage");
                let current: Json =
                    sqlx::query_scalar("SELECT to_jsonb(p) FROM providers p WHERE id = $1")
                        .bind(provider)
                        .fetch_one(&scene.world.db.pool)
                        .await
                        .unwrap();
                assert_eq!(current, before, "{operation}");
                assert!(played.seen().is_empty());
                assert!(host_played.seen().is_empty());
                no_command_was_queued(&scene).await;
                let jobs: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE provider_id = $1")
                        .bind(provider)
                        .fetch_one(&scene.world.db.pool)
                        .await
                        .unwrap();
                assert_eq!(jobs, 0);
            } else {
                assert_eq!(status, StatusCode::OK, "{operation}: {value}");
                match operation {
                    "start" => {
                        let jobs: i64 =
                            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE provider_id = $1")
                                .bind(provider)
                                .fetch_one(&scene.world.db.pool)
                                .await
                                .unwrap();
                        assert_eq!(jobs, 1);
                    }
                    "stop" => {
                        assert_eq!(host_played.seen_of(CommandKind::NodeRuntimeStop).len(), 1)
                    }
                    "login" => assert_eq!(played.seen_of(CommandKind::ProviderLoginStart).len(), 1),
                    "login/input" => {
                        assert_eq!(played.seen_of(CommandKind::ProviderLoginInput).len(), 1)
                    }
                    "login/cancel" => {
                        assert_eq!(played.seen_of(CommandKind::ProviderLoginCancel).len(), 1)
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}

#[tokio::test]
async fn a_subscription_registered_from_a_machine_requires_the_callers_current_agent_use() {
    for loses_use in [true, false] {
        let mut scene = Scene::new().await;
        let home = scene.writer.person.personal_org;
        let machine = scene.machine(scene.writer.id()).await;
        let agent = scene
            .resource(ResourceKind::Agent, home, scene.writer.id())
            .await;
        sqlx::query("UPDATE agents SET machine_id = $2, descriptor = $3 WHERE id = $1")
            .bind(agent)
            .bind(machine)
            .bind(descriptor())
            .execute(&scene.world.db.pool)
            .await
            .unwrap();
        let token = new_id();
        scene.writer.token = new_token(API_TOKEN_PREFIX);
        identity::create_api_token(
            &mut *scene.world.conn().await,
            &NewApiToken {
                id: token,
                user_id: scene.writer.id(),
                name: "registration",
                token_hash: &token_hash(&scene.writer.token),
                scopes: &["*".to_owned()],
                node_id: None,
                at: scene.world.now(),
                expires_at: None,
            },
        )
        .await
        .unwrap();
        let played = scene.play(machine);
        let pool = scene.world.db.pool.clone();
        let clock = scene.world.app.clock.clone();
        // The token loses agent use while the body is on its way.
        let body = Body::from_stream(stream::once(async move {
            clock.advance(Duration::seconds(2));
            if loses_use {
                sqlx::query("UPDATE api_tokens SET scopes = ARRAY['provider.view'] WHERE id = $1")
                    .bind(token)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            Ok::<_, std::io::Error>(Bytes::from(
                serde_json::to_vec(&json!({
                    "kind":"subscription","name":"Found","home_id":home,"agent_id":agent
                }))
                .unwrap(),
            ))
        }));
        let (status, value) = reply(
            router(&scene.world)
                .oneshot(request(
                    &scene.writer,
                    "POST",
                    &format!("/machines/{machine}/providers"),
                    body,
                ))
                .await
                .unwrap(),
        )
        .await;
        if loses_use {
            assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
            assert_eq!(value["code"], "authz.denied", "{value}");
            assert_eq!(value["required"], "agent.use", "{value}");
            assert_eq!(
                value["requestable"], false,
                "a token's scopes are not enlarged by a page grant"
            );
            no_command_was_queued(&scene).await;
        } else {
            assert_eq!(status, StatusCode::OK, "{value}");
        }
        // Registering it asks the machine's harness nothing either way.
        assert!(played.seen().is_empty());
        let live: i64 =
            sqlx::query_scalar("SELECT count(*) FROM providers WHERE deleted_at IS NULL")
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
        assert_eq!(live, i64::from(!loses_use));
    }
}

#[tokio::test]
async fn declaring_a_handover_lost_uses_manage_and_time_after_the_action_wait() {
    for ends in [true, false] {
        let scene = Scene::new().await;
        let (provider, _, machine) = scene.provider(true).await;
        let environment = fixtures::insert_environment(
            &mut *scene.world.conn().await,
            scene.owner.id(),
            Some(scene.home),
            "Holder",
            "managed",
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE providers SET held_by_environment_id = $2, held_since = $3 WHERE id = $1",
        )
        .bind(provider)
        .bind(environment)
        .bind(scene.world.now())
        .execute(&scene.world.db.pool)
        .await
        .unwrap();
        scene
            .grant("provider", provider, "provider.manage", ends)
            .await;
        let played = scene.play(machine);
        let before: Json = sqlx::query_scalar("SELECT to_jsonb(p) FROM providers p WHERE id = $1")
            .bind(provider)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        let body = Body::from(serde_json::to_vec(&json!({"declare_lost":true})).unwrap());
        let (status, value) = locked_request(
            &scene,
            "POST",
            &format!("/providers/{provider}/handover/return"),
            body,
        )
        .await;
        let current: Json = sqlx::query_scalar("SELECT to_jsonb(p) FROM providers p WHERE id = $1")
            .bind(provider)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        if ends {
            denied(status, &value, "provider.manage");
            assert_eq!(current, before);
        } else {
            assert_eq!(status, StatusCode::OK, "{value}");
            assert!(current["held_by_environment_id"].is_null());
            assert_eq!(current["status"], "needs_setup");
            let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT at FROM audit_log WHERE object_id = $1 AND details->>'decision' = 'accepted'")
                .bind(provider).fetch_one(&scene.world.db.pool).await.unwrap();
            assert_eq!(at, scene.world.now());
        }
        assert!(
            played.seen().is_empty(),
            "declaring lost makes no machine hold a second credential"
        );
    }
}
