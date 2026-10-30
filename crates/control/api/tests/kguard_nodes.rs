//! Node, directory and topic changes use authority that still holds when their action transaction begins.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use axum::body::{Body, to_bytes};
use bytes::Bytes;
use chrono::Duration;
use futures::stream;
use http::{Request, StatusCode};
use orch_api_contract::nodes::{Org, Project, Topic};
use orch_api_core::registry::Registry;
use orch_db::queries::access::{self, PageKey, testing};
use orch_db::queries::hierarchy;
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::threads::{self, forking};
use orch_types::{Id, Json, new_id};
use places::{Who, join, make_department, make_org, make_project, make_topic, person};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as WaitDuration;
use support::World;
use tower::ServiceExt;

// Ending authority one second from now and delivering two seconds later makes
// the boundary unambiguous without waiting for the wall clock.
const AUTHORITY_SECONDS: i64 = 1;
const DELIVERY_SECONDS: i64 = 2;
// Scheduling headroom for the shared machine; PostgreSQL's lock state is the
// synchronization signal, so no guessed sleep decides when authority ends.
const LOCK_WAIT_BOUND: WaitDuration = WaitDuration::from_secs(30);

struct Scenario {
    world: World,
    owner: Who,
    writer: Who,
    member: Who,
    root: Org,
    from: Org,
    to: Org,
    project: Project,
    folder: Project,
    topic: Topic,
}

impl Scenario {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = person(&world, "owner").await;
        let writer = person(&world, "writer").await;
        let member = person(&world, "member").await;
        let root = make_org(&world, &owner, "Office", "office").await;
        let from = make_department(&world, &owner, root.base.id, "From", "from").await;
        let to = make_department(&world, &owner, root.base.id, "To", "to").await;
        join(
            &world,
            root.base.id,
            root.base.id,
            &writer,
            "viewer",
            &owner,
        )
        .await;
        join(
            &world,
            root.base.id,
            from.base.id,
            &member,
            "viewer",
            &owner,
        )
        .await;
        let project = make_project(&world, &owner, from.base.id, "Original").await;
        let folder = world
            .app
            .call(
                orch_api_contract::nodes::ProjectChildCreate,
                &[&project.base.id],
            )
            .token(&owner.token)
            .body(&orch_api_contract::nodes::ProjectCreateBody {
                name: "Folder".into(),
                description: None,
                access: None,
            })
            .send()
            .await
            .ok();
        let topic = make_topic(&world, &owner, project.base.id, "Topic").await;
        Self {
            world,
            owner,
            writer,
            member,
            root,
            from,
            to,
            project,
            folder,
            topic,
        }
    }

    async fn grant(&self, node: Id, permission: &str, ends: bool) {
        testing::add_entry(
            &mut *self.world.conn().await,
            &PageKey::new("node", node, "live"),
            "any",
            permission,
            ("user", Some(self.writer.id()), None),
            ends.then_some(self.world.now() + Duration::seconds(AUTHORITY_SECONDS)),
            self.world.now(),
        )
        .await
        .unwrap();
    }

    async fn owner_request(&self, method: &str, path: &str, body: Option<Json>) -> Json {
        let body = if body.is_none()
            && method == "POST"
            && (path.ends_with("/archive") || path.ends_with("/unarchive"))
        {
            let node = path.split('/').nth(2).unwrap().parse().unwrap();
            Some(
                serde_json::to_value(
                    places::archive_body(
                        &self.world,
                        &self.owner.token,
                        node,
                        path.ends_with("/archive"),
                    )
                    .await,
                )
                .unwrap(),
            )
        } else {
            body
        };
        let response = router(&self.world)
            .oneshot(request(
                &self.owner,
                method,
                path,
                body.map_or_else(Body::empty, |body| {
                    Body::from(serde_json::to_vec(&body).unwrap())
                }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = response_json(response).await;
        assert!(
            status.is_success(),
            "setup {method} {path}: {status} {body}"
        );
        body
    }

    async fn group(&self) -> Id {
        testing::insert_group(
            &mut *self.world.conn().await,
            self.from.base.id,
            "Existing",
            self.owner.id(),
        )
        .await
        .unwrap()
    }

    async fn service(&self) -> Id {
        let id = new_id();
        identity::create_service_account(
            &mut *self.world.conn().await,
            id,
            self.from.base.id,
            "existing-service",
            "Existing service",
            self.owner.id(),
            self.world.now(),
        )
        .await
        .unwrap();
        id
    }

    async fn side(&self) -> (Id, Id) {
        let parent = new_id();
        let side = new_id();
        let mut tx = self.world.db.pool.begin().await.unwrap();
        assert!(
            threads::insert_thread(
                &mut tx,
                parent,
                self.topic.base.id,
                "Parent",
                self.owner.id(),
                self.world.now()
            )
            .await
            .unwrap()
        );
        orch_authz::pages::start(
            &mut tx,
            orch_access_contract::pages::StoredKind::Node,
            parent,
            self.owner.id(),
            orch_types::Time::new(self.world.now()),
        )
        .await
        .unwrap();
        // An idle side thread with no open harness session needs no runtime to
        // close it; these are the actual node/thread rows the engine closes.
        assert!(
            forking::insert_side_thread(
                &mut tx,
                &forking::NewSide {
                    id: side,
                    parent,
                    name: "Parent (side)",
                    owner: self.owner.id(),
                    from_turn: 0,
                    at: self.world.now(),
                }
            )
            .await
            .unwrap()
        );
        tx.commit().await.unwrap();
        (parent, side)
    }

    async fn side_manager(&self, ends: bool) -> Id {
        // The current preflight reads a side's inherited defaults. An Admin of
        // its project is an admitted manager of the parent as well as the side.
        testing::give_role(
            &mut *self.world.conn().await,
            self.project.base.id,
            "user",
            self.writer.id(),
            "admin",
            self.owner.id(),
            ends.then_some(self.world.now() + Duration::seconds(AUTHORITY_SECONDS)),
        )
        .await
        .unwrap()
    }
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

async fn response_json(response: axum::response::Response) -> Json {
    // Test replies are bounded by these small fixtures, and never a file stream.
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    if bytes.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

async fn waiting_for_a_lock(world: &World, blocker: i32) {
    tokio::time::timeout(LOCK_WAIT_BOUND, async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid
                   WHERE a.datname = current_database() AND a.pid <> $1 AND NOT l.granted)",
            )
            .bind(blocker)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real request reached its database action lock");
}

async fn delayed(s: &Scenario, call: &Call) -> (StatusCode, Json) {
    let response = if let Some(value) = &call.body {
        let value = value.clone();
        let clock = s.world.app.clock.clone();
        let delivered = Arc::new(AtomicBool::new(false));
        let polled = delivered.clone();
        // The registry polls this stream after its authentication and permission
        // preflight, but before dispatching the decoded action.
        let body = Body::from_stream(stream::once(async move {
            clock.advance(Duration::seconds(DELIVERY_SECONDS));
            polled.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&value).unwrap()))
        }));
        let response = router(&s.world)
            .oneshot(request(&s.writer, call.method, &call.path, body))
            .await
            .unwrap();
        assert!(
            delivered.load(Ordering::SeqCst),
            "{} passed the earlier preflight: {}",
            call.name,
            response.status()
        );
        response
    } else {
        // Bodyless endpoints deliberately consume no body. Hold the authority
        // lock instead, and end permission only once the actual action waits.
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        // No barrier is shared: the action waits at its guard for the writer's own row.
        orch_db::queries::transaction_guard::lock_person(&mut blocker, s.writer.id())
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let action = tokio::spawn(router(&s.world).oneshot(request(
            &s.writer,
            call.method,
            &call.path,
            Body::empty(),
        )));
        waiting_for_a_lock(&s.world, pid).await;
        assert!(
            !action.is_finished(),
            "{} is waiting at its action boundary",
            call.name
        );
        s.world.advance(Duration::seconds(DELIVERY_SECONDS));
        blocker.rollback().await.unwrap();
        action.await.unwrap().unwrap()
    };
    (response.status(), response_json(response).await)
}

// This captures stored business effects and audit together, including changes
// to ownership, membership and identity, rather than only the response status.
async fn snapshot(world: &World) -> Json {
    const TABLES: &[&str] = &[
        "nodes",
        "orgs",
        "topics",
        "tags",
        "node_tags",
        "subscriptions",
        "groups",
        "group_members",
        "org_members",
        "node_roles",
        "org_suspensions",
        "users",
        "api_tokens",
        "permission_entries",
        "permission_rows",
        "audit_log",
        "threads",
        "items",
        "environments",
        "env_templates",
    ];
    let fields = TABLES.iter().map(|table| format!(
        "'{table}', (SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) FROM {table} t)"
    )).collect::<Vec<_>>().join(", ");
    // Every interpolated identifier comes from TABLES above, never the request.
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT jsonb_build_object({fields})"
    )))
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

struct Call {
    name: &'static str,
    method: &'static str,
    path: String,
    body: Option<Json>,
    required: &'static str,
    observe: &'static str,
    object: Id,
    after: Json,
    changes: bool,
}

impl Call {
    async fn exercise(&self, s: &Scenario, ends: bool) {
        let before = snapshot(&s.world).await;
        let (status, answer) = delayed(s, self).await;
        if ends {
            assert_eq!(status, StatusCode::FORBIDDEN, "{}: {answer}", self.name);
            assert_eq!(answer["code"], "authz.denied", "{}: {answer}", self.name);
            let named = answer["required"] == self.required
                || answer["missing"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|missing| {
                        missing["permissions"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|p| p == self.required)
                    });
            assert!(
                named,
                "{} names the missing {}: {answer}",
                self.name, self.required
            );
            assert_eq!(
                snapshot(&s.world).await,
                before,
                "{} writes nothing after authority ends",
                self.name
            );
        } else {
            assert!(status.is_success(), "{}: {status} {answer}", self.name);
            let actual: Json = sqlx::query_scalar(self.observe)
                .bind(self.object)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
            assert_eq!(
                actual, self.after,
                "{} performs its intended change",
                self.name
            );
            if self.changes {
                assert_ne!(
                    snapshot(&s.world).await,
                    before,
                    "{} committed a business change",
                    self.name
                );
            } else {
                assert_eq!(
                    snapshot(&s.world).await,
                    before,
                    "{} only previews",
                    self.name
                );
                assert!(answer["blockers"].as_array().unwrap().is_empty());
            }
        }
        println!(
            "guard_nodes case={} authority={} status={status}",
            self.name,
            if ends { "ended" } else { "retained" }
        );
    }
}

#[tokio::test]
async fn making_departments_projects_folders_and_topics_requires_permission_after_the_body() {
    for kind in [
        "OrgChildCreate",
        "ProjectCreate",
        "ProjectChildCreate",
        "TopicCreate",
    ] {
        for ends in [true, false] {
            let s = Scenario::new().await;
            let (parent, path, required, body) = match kind {
                "OrgChildCreate" => (
                    s.from.base.id,
                    "orgs",
                    "org.manage",
                    json!({"name": "Made", "slug": "made"}),
                ),
                "ProjectCreate" => (
                    s.from.base.id,
                    "projects",
                    "org.interact",
                    json!({"name": "Made"}),
                ),
                "ProjectChildCreate" => (
                    s.project.base.id,
                    "projects",
                    "project.interact",
                    json!({"name": "Made"}),
                ),
                _ => (
                    s.folder.base.id,
                    "topics",
                    "project.interact",
                    json!({"name": "Made"}),
                ),
            };
            s.grant(parent, required, ends).await;
            let prefix = if kind == "ProjectChildCreate" || kind == "TopicCreate" {
                "projects"
            } else {
                "orgs"
            };
            Call {
                name: kind, method: "POST", path: format!("/{prefix}/{parent}/{path}"), body: Some(body), required,
                observe: "SELECT to_jsonb(count(*)) FROM nodes WHERE parent_id = $1 AND name = 'Made'",
                object: parent, after: json!(1), changes: true,
            }.exercise(&s, ends).await;
        }
    }
}

#[tokio::test]
async fn names_automation_topic_state_and_default_environment_need_current_permission() {
    for kind in [
        "NodeUpdate",
        "NodeAuto",
        "TopicUpdate",
        "TopicDefaultEnvironmentSet",
    ] {
        for ends in [true, false] {
            let s = Scenario::new().await;
            let (node, method, path, body, required, observe, after) = match kind {
                "NodeUpdate" => (
                    s.project.base.id,
                    "PATCH",
                    format!("/nodes/{}", s.project.base.id),
                    json!({"name": "Changed", "expected_revision": s.project.base.revision}),
                    "project.manage",
                    "SELECT to_jsonb(name) FROM nodes WHERE id = $1",
                    json!("Changed"),
                ),
                "NodeAuto" => (
                    s.topic.base.id,
                    "POST",
                    format!("/nodes/{}/auto", s.topic.base.id),
                    json!({"field": "title", "action": "resume"}),
                    "topic.manage",
                    "SELECT to_jsonb(title_source) FROM nodes WHERE id = $1",
                    json!("auto"),
                ),
                "TopicUpdate" => (
                    s.topic.base.id,
                    "PATCH",
                    format!("/topics/{}", s.topic.base.id),
                    json!({"status": "done"}),
                    "topic.interact",
                    "SELECT to_jsonb(status) FROM topics WHERE node_id = $1",
                    json!("done"),
                ),
                _ => {
                    let environment = new_id();
                    let template = new_id();
                    let mut conn = s.world.conn().await;
                    // No provisioning runs here: only a stored default choice is
                    // cleared. Keep its managed environment's required template link.
                    sqlx::query("INSERT INTO env_templates (id, name, owner_id, org_id, home_id, driver, spec)
                                 VALUES ($1, 'Default template', $2, $3, $4, 'docker', '{}')")
                        .bind(template).bind(s.owner.id()).bind(s.root.base.id).bind(s.from.base.id)
                        .execute(&mut *conn).await.unwrap();
                    sqlx::query("INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, template_id)
                                 VALUES ($1, 'Default', $2, $3, 'managed', $4, $5)")
                        .bind(environment).bind(s.owner.id()).bind(s.root.base.id).bind(s.topic.base.id)
                        .bind(template)
                        .execute(&mut *conn).await.unwrap();
                    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'env.default_id',to_jsonb($2::uuid::text)) ON CONFLICT (node_id,key) DO UPDATE SET value=excluded.value")
                        .bind(s.topic.base.id)
                        .bind(environment)
                        .execute(&mut *conn)
                        .await
                        .unwrap();
                    (
                        s.topic.base.id,
                        "PUT",
                        format!("/topics/{}/default-environment", s.topic.base.id),
                        json!({"environment_id": null}),
                        "topic.manage",
                        "SELECT to_jsonb(NOT EXISTS (SELECT 1 FROM settings WHERE node_id=$1 AND key='env.default_id'))",
                        json!(true),
                    )
                }
            };
            s.grant(node, required, ends).await;
            Call {
                name: kind,
                method,
                path,
                body: Some(body),
                required,
                observe,
                object: node,
                after,
                changes: true,
            }
            .exercise(&s, ends)
            .await;
        }
    }
}

#[tokio::test]
async fn moving_and_previewing_each_kind_requires_both_ends_after_the_body() {
    for (kind, prefix, preview) in [
        ("OrgMove", "orgs", false),
        ("OrgMovePreview", "orgs", true),
        ("ProjectMove", "projects", false),
        ("ProjectMovePreview", "projects", true),
        ("TopicMove", "topics", false),
        ("TopicMovePreview", "topics", true),
    ] {
        // Prove each end separately, then retain both for the successful control.
        for ends_at in [Some(false), Some(true), None] {
            let s = Scenario::new().await;
            let (node, target, revision, source_permission, target_permission, parent) =
                match prefix {
                    "orgs" => (
                        s.from.base.id,
                        s.to.base.id,
                        s.from.base.revision,
                        "org.manage",
                        "org.manage",
                        s.root.base.id,
                    ),
                    "projects" => (
                        s.project.base.id,
                        s.to.base.id,
                        s.project.base.revision,
                        "project.manage",
                        "org.interact",
                        s.from.base.id,
                    ),
                    _ => (
                        s.topic.base.id,
                        s.folder.base.id,
                        s.topic.base.revision,
                        "topic.manage",
                        "project.interact",
                        s.project.base.id,
                    ),
                };
            s.grant(node, source_permission, ends_at == Some(false))
                .await;
            s.grant(target, target_permission, ends_at == Some(true))
                .await;
            let body = if preview {
                json!({"parent_id": target})
            } else {
                json!({"parent_id": target, "expected_revision": revision})
            };
            let suffix = if preview { "/preview" } else { "" };
            Call {
                name: kind,
                method: "POST",
                path: format!("/{prefix}/{node}/move{suffix}"),
                body: Some(body),
                required: if ends_at == Some(true) {
                    target_permission
                } else {
                    source_permission
                },
                observe: "SELECT to_jsonb(parent_id::text) FROM nodes WHERE id = $1",
                object: node,
                after: json!(if preview { parent } else { target }.to_string()),
                changes: !preview,
            }
            .exercise(&s, ends_at.is_some())
            .await;
        }
    }
}

#[tokio::test]
async fn archive_restore_deletion_and_ownership_transfer_need_current_authority() {
    for kind in [
        "NodeArchive",
        "NodeUnarchive",
        "NodeDelete",
        "NodeRestore",
        "NodeTransfer",
    ] {
        for ends in [true, false] {
            let s = Scenario::new().await;
            let project = s.project.base.id;
            let topic = s.topic.base.id;
            let (method, path, body, required, observe, object, after) = match kind {
                "NodeArchive" | "NodeUnarchive" => {
                    let archiving = kind == "NodeArchive";
                    if !archiving {
                        s.owner_request("POST", &format!("/nodes/{project}/archive"), None)
                            .await;
                    }
                    s.grant(project, "project.archive", ends).await;
                    let review =
                        places::archive_body(&s.world, &s.writer.token, project, archiving).await;
                    (
                        "POST",
                        format!(
                            "/nodes/{project}/{}",
                            if archiving { "archive" } else { "unarchive" }
                        ),
                        Some(serde_json::to_value(review).unwrap()),
                        "project.archive",
                        "SELECT to_jsonb(archived_at IS NOT NULL) FROM nodes WHERE id = $1",
                        project,
                        json!(archiving),
                    )
                }
                "NodeDelete" => {
                    s.grant(project, "project.manage", ends).await;
                    (
                        "DELETE",
                        format!("/nodes/{project}"),
                        Some(json!({})),
                        "project.manage",
                        "SELECT to_jsonb(deleted_at IS NOT NULL) FROM nodes WHERE id = $1",
                        project,
                        json!(true),
                    )
                }
                "NodeRestore" => {
                    s.owner_request("DELETE", &format!("/nodes/{topic}"), Some(json!({})))
                        .await;
                    s.grant(project, "project.manage", ends).await;
                    (
                        "POST",
                        format!("/nodes/{topic}/restore"),
                        None,
                        "project.manage",
                        "SELECT to_jsonb(deleted_at IS NULL) FROM nodes WHERE id = $1",
                        topic,
                        json!(true),
                    )
                }
                _ => {
                    testing::give_role(
                        &mut *s.world.conn().await,
                        s.from.base.id,
                        "user",
                        s.writer.id(),
                        "admin",
                        s.owner.id(),
                        ends.then_some(s.world.now() + Duration::seconds(AUTHORITY_SECONDS)),
                    )
                    .await
                    .unwrap();
                    // An admin passes the project on as its admin, with As admin on.
                    places::admin_mode(&s.world, &s.writer, true).await;
                    (
                        "POST",
                        format!("/nodes/{project}/transfer"),
                        Some(json!({"user_id": s.member.id(), "keep_previous": false})),
                        "access.manage",
                        "SELECT to_jsonb(owner_id::text) FROM nodes WHERE id = $1",
                        project,
                        json!(s.member.id().to_string()),
                    )
                }
            };
            Call {
                name: kind,
                method,
                path,
                body,
                required,
                observe,
                object,
                after,
                changes: true,
            }
            .exercise(&s, ends)
            .await;
        }
    }
}

#[tokio::test]
async fn a_subscription_requires_view_that_still_holds_at_its_write() {
    for deleting in [false, true] {
        for ends in [true, false] {
            let s = Scenario::new().await;
            let project = s.project.base.id;
            testing::set_row(
                &mut *s.world.conn().await,
                &PageKey::new("node", project, "live"),
                "any",
                "project.view",
                false,
                false,
            )
            .await
            .unwrap();
            s.grant(project, "project.view", ends).await;
            if deleting {
                hierarchy::put_subscription(
                    &mut *s.world.conn().await,
                    s.writer.id(),
                    project,
                    "all",
                    "manual",
                    s.world.now(),
                )
                .await
                .unwrap();
            }
            Call {
                name: if deleting {
                    "NodeSubscriptionDelete"
                } else {
                    "NodeSubscriptionSet"
                },
                method: if deleting { "DELETE" } else { "PUT" },
                path: format!("/nodes/{project}/subscription"),
                body: (!deleting).then_some(json!({"level": "all"})),
                required: "project.view",
                observe: "SELECT to_jsonb(count(*)) FROM subscriptions WHERE user_id = $1",
                object: s.writer.id(),
                after: json!(if deleting { 0 } else { 1 }),
                changes: true,
            }
            .exercise(&s, ends)
            .await;
        }
    }
}

#[tokio::test]
async fn suspensions_and_membership_changes_require_current_members_management() {
    for kind in [
        "OrgMemberSuspend",
        "OrgMemberResume",
        "OrgMemberRemove",
        "OrgMemberMove",
    ] {
        for ends in [true, false] {
            let s = Scenario::new().await;
            let org = s.from.base.id;
            let member = s.member.id();
            s.grant(org, "members.manage", ends).await;
            let (method, suffix, body, observe, object, after) = match kind {
                "OrgMemberSuspend" => (
                    "POST",
                    "/suspend",
                    Some(json!({"reason": "Held"})),
                    "SELECT to_jsonb(count(*)) FROM org_suspensions WHERE org_id = $1",
                    org,
                    json!(1),
                ),
                "OrgMemberResume" => {
                    testing::suspend_in(&mut *s.world.conn().await, org, member, s.owner.id())
                        .await
                        .unwrap();
                    (
                        "POST",
                        "/resume",
                        None,
                        "SELECT to_jsonb(count(*)) FROM org_suspensions WHERE org_id = $1",
                        org,
                        json!(0),
                    )
                }
                "OrgMemberRemove" => (
                    "DELETE",
                    "",
                    Some(json!({})),
                    "SELECT to_jsonb(count(*)) FROM org_members WHERE user_id = $1 AND domain_id <> (SELECT personal_org_id FROM users WHERE id = $1)",
                    member,
                    json!(0),
                ),
                _ => {
                    s.grant(s.to.base.id, "members.manage", false).await;
                    (
                        "POST",
                        "/move",
                        Some(json!({"to": s.to.base.id})),
                        "SELECT to_jsonb(domain_id::text) FROM org_members WHERE user_id = $1 AND domain_id <> (SELECT personal_org_id FROM users WHERE id = $1)",
                        member,
                        json!(s.to.base.id.to_string()),
                    )
                }
            };
            Call {
                name: kind,
                method,
                path: format!("/orgs/{org}/members/{member}{suffix}"),
                body,
                required: "members.manage",
                observe,
                object,
                after,
                changes: true,
            }
            .exercise(&s, ends)
            .await;
        }
    }
}

#[tokio::test]
async fn creating_changing_deleting_groups_and_their_members_uses_the_keepers_current_permission() {
    for kind in [
        "OrgGroupCreate",
        "GroupUpdate",
        "GroupDelete",
        "GroupMemberAdd",
        "GroupMemberRemove",
    ] {
        for ends in [true, false] {
            let s = Scenario::new().await;
            let org = s.from.base.id;
            let group = s.group().await;
            s.grant(org, "members.manage", ends).await;
            let (method, path, body, observe, object, after) = match kind {
                "OrgGroupCreate" => (
                    "POST",
                    format!("/orgs/{org}/groups"),
                    Some(json!({"name": "Made"})),
                    "SELECT to_jsonb(count(*)) FROM groups WHERE org_id = $1 AND name = 'Made'",
                    org,
                    json!(1),
                ),
                "GroupUpdate" => (
                    "PATCH",
                    format!("/groups/{group}"),
                    Some(json!({"name": "Changed"})),
                    "SELECT to_jsonb(name) FROM groups WHERE id = $1",
                    group,
                    json!("Changed"),
                ),
                "GroupDelete" => (
                    "DELETE",
                    format!("/groups/{group}"),
                    None,
                    "SELECT to_jsonb(count(*)) FROM groups WHERE id = $1",
                    group,
                    json!(0),
                ),
                _ => {
                    let adding = kind == "GroupMemberAdd";
                    if !adding {
                        testing::add_to_group(&mut *s.world.conn().await, group, s.member.id())
                            .await
                            .unwrap();
                    }
                    (
                        if adding { "PUT" } else { "DELETE" },
                        format!("/groups/{group}/members/{}", s.member.id()),
                        None,
                        "SELECT to_jsonb(count(*)) FROM group_members WHERE group_id = $1",
                        group,
                        json!(if adding { 1 } else { 0 }),
                    )
                }
            };
            Call {
                name: kind,
                method,
                path,
                body,
                required: "members.manage",
                observe,
                object,
                after,
                changes: true,
            }
            .exercise(&s, ends)
            .await;
        }
    }
}

#[tokio::test]
async fn service_identities_and_their_tokens_require_the_keepers_current_permission() {
    for kind in [
        "OrgServiceAccountCreate",
        "ServiceAccountDelete",
        "ServiceAccountTokenCreate",
        "ServiceAccountTokenDelete",
    ] {
        for ends in [true, false] {
            let s = Scenario::new().await;
            let org = s.from.base.id;
            let service = s.service().await;
            s.grant(org, "members.manage", ends).await;
            let (method, path, body, observe, object, after) = match kind {
                "OrgServiceAccountCreate" => (
                    "POST",
                    format!("/orgs/{org}/service-accounts"),
                    Some(json!({"username": "made-service", "display_name": "Made"})),
                    "SELECT to_jsonb(count(*)) FROM users WHERE service_org_id = $1 AND username = 'made-service'",
                    org,
                    json!(1),
                ),
                "ServiceAccountDelete" => (
                    "DELETE",
                    format!("/service-accounts/{service}"),
                    None,
                    "SELECT to_jsonb(status) FROM users WHERE id = $1",
                    service,
                    json!("suspended"),
                ),
                "ServiceAccountTokenCreate" => (
                    "POST",
                    format!("/service-accounts/{service}/tokens"),
                    // Making a token takes its maker's fresh proof of who they are.
                    Some(json!({"name": "Made", "proof": {"password": support::PASSWORD}})),
                    "SELECT to_jsonb(count(*)) FROM api_tokens WHERE user_id = $1 AND revoked_at IS NULL",
                    service,
                    json!(1),
                ),
                _ => {
                    let token = new_id();
                    identity::create_api_token(
                        &mut *s.world.conn().await,
                        &NewApiToken {
                            id: token,
                            user_id: service,
                            name: "Existing",
                            token_hash: &[1; 32],
                            scopes: &["*".into()],
                            node_id: None,
                            at: s.world.now(),
                            expires_at: None,
                        },
                    )
                    .await
                    .unwrap();
                    (
                        "DELETE",
                        format!("/service-accounts/{service}/tokens/{token}"),
                        None,
                        "SELECT to_jsonb(revoked_at IS NOT NULL) FROM api_tokens WHERE id = $1",
                        token,
                        json!(true),
                    )
                }
            };
            Call {
                name: kind,
                method,
                path,
                body,
                required: "members.manage",
                observe,
                object,
                after,
                changes: true,
            }
            .exercise(&s, ends)
            .await;
        }
    }
}

#[tokio::test]
async fn creating_changing_and_removing_tags_requires_the_defining_nodes_current_manage() {
    for kind in ["NodeTagCreate", "TagUpdate", "TagDelete"] {
        for ends in [true, false] {
            let s = Scenario::new().await;
            let project = s.project.base.id;
            let tag = new_id();
            hierarchy::create_tag(
                &mut *s.world.conn().await,
                tag,
                project,
                "Existing",
                1,
                s.world.now(),
            )
            .await
            .unwrap();
            s.grant(project, "project.manage", ends).await;
            let (method, path, body, observe, object, after) = match kind {
                "NodeTagCreate" => (
                    "POST",
                    format!("/nodes/{project}/tags"),
                    Some(json!({"name": "Made", "color": 2})),
                    "SELECT to_jsonb(count(*)) FROM tags WHERE node_id = $1 AND name = 'Made'",
                    project,
                    json!(1),
                ),
                "TagUpdate" => (
                    "PATCH",
                    format!("/tags/{tag}"),
                    Some(json!({"name": "Changed", "color": 2})),
                    "SELECT to_jsonb(name) FROM tags WHERE id = $1",
                    tag,
                    json!("Changed"),
                ),
                _ => (
                    "DELETE",
                    format!("/tags/{tag}"),
                    None,
                    "SELECT to_jsonb(count(*)) FROM tags WHERE id = $1",
                    tag,
                    json!(0),
                ),
            };
            Call {
                name: kind,
                method,
                path,
                body,
                required: "project.manage",
                observe,
                object,
                after,
                changes: true,
            }
            .exercise(&s, ends)
            .await;
        }
    }
}

#[tokio::test]
async fn a_root_org_is_made_only_if_the_session_still_holds_after_the_body() {
    for ends in [true, false] {
        let s = Scenario::new().await;
        // As Orch ships, a team org is made by somebody a system admin invited to make one.
        places::invite_to_make_org(&s.world, &s.writer).await;
        if ends {
            sqlx::query("UPDATE access_tokens SET expires_at = $2 WHERE token_hash = $1")
                .bind(orch_api_core::middleware::authentication::token_hash(
                    &s.writer.token,
                ))
                .bind(s.world.now() + Duration::seconds(AUTHORITY_SECONDS))
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        let before = snapshot(&s.world).await;
        let (status, answer) = delayed(
            &s,
            &Call {
                name: "OrgCreate",
                method: "POST",
                path: "/orgs".into(),
                body: Some(json!({"name": "Made", "slug": "made"})),
                required: "",
                observe: "",
                object: s.writer.id(),
                after: Json::Null,
                changes: true,
            },
        )
        .await;
        if ends {
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
            assert_eq!(answer["code"], "auth.invalid_token");
            assert_eq!(snapshot(&s.world).await, before);
        } else {
            assert_eq!(status, StatusCode::OK, "{answer}");
            assert_eq!(answer["owner"]["id"], s.writer.id().to_string());
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM orgs WHERE slug = 'made'")
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
            assert_eq!(count, 1);
        }
        println!(
            "guard_nodes case=OrgCreate authority={} status={status}",
            if ends { "ended" } else { "retained" }
        );
    }
}

#[tokio::test]
async fn losing_management_of_the_receiving_org_prevents_a_member_move() {
    let s = Scenario::new().await;
    s.grant(s.from.base.id, "members.manage", false).await;
    s.grant(s.to.base.id, "members.manage", true).await;
    Call {
        name: "OrgMemberMove receiving org",
        method: "POST",
        path: format!("/orgs/{}/members/{}/move", s.from.base.id, s.member.id()),
        body: Some(json!({"to": s.to.base.id})),
        required: "members.manage",
        observe: "",
        object: s.member.id(),
        after: Json::Null,
        changes: true,
    }
    .exercise(&s, true)
    .await;
}

#[tokio::test]
async fn a_member_move_preview_reports_permission_that_ended_during_its_body() {
    for ends in [true, false] {
        let s = Scenario::new().await;
        s.grant(s.from.base.id, "members.manage", ends).await;
        s.grant(s.to.base.id, "members.manage", false).await;
        let before = snapshot(&s.world).await;
        let (status, answer) = delayed(
            &s,
            &Call {
                name: "OrgMemberMovePreview",
                method: "POST",
                path: format!(
                    "/orgs/{}/members/{}/move/preview",
                    s.from.base.id,
                    s.member.id()
                ),
                body: Some(json!({"to": s.to.base.id})),
                required: "members.manage",
                observe: "",
                object: s.member.id(),
                after: Json::Null,
                changes: false,
            },
        )
        .await;
        // A caller holding one end gets the plan with what is missing at the
        // other end; a preview itself changes no membership or authority.
        assert_eq!(status, StatusCode::OK, "{answer}");
        let missing = answer["missing"].as_array().unwrap();
        if ends {
            assert!(
                missing.iter().any(|item| item["permissions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|permission| permission == "members.manage")),
                "{answer}"
            );
        } else {
            assert!(missing.is_empty(), "{answer}");
        }
        assert_eq!(snapshot(&s.world).await, before);
        println!(
            "guard_nodes case=OrgMemberMovePreview authority={} status={status}",
            if ends { "ended" } else { "retained" }
        );
    }
}

#[tokio::test]
async fn closing_a_side_thread_uses_the_parents_permission_after_the_body() {
    for ends in [true, false] {
        let s = Scenario::new().await;
        let (_parent, side) = s.side().await;
        s.side_manager(ends).await;
        Call {
            name: "NodeDelete side thread",
            method: "DELETE",
            path: format!("/nodes/{side}"),
            body: Some(json!({})),
            required: "thread.manage",
            observe: "SELECT to_jsonb(deleted_at IS NOT NULL) FROM nodes WHERE id = $1",
            object: side,
            after: json!(true),
            changes: true,
        }
        .exercise(&s, ends)
        .await;
    }
}

#[tokio::test]
async fn a_revocation_committed_while_the_guarded_side_thread_close_waits_refuses_the_close() {
    let s = Scenario::new().await;
    let (_parent, side) = s.side().await;
    let role = s.side_manager(false).await;

    // Stop the engine at its actual close write, after the API has checked permission.
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT node_id FROM threads WHERE node_id = $1 FOR UPDATE")
        .bind(side)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let action = tokio::spawn(router(&s.world).oneshot(request(
        &s.writer,
        "DELETE",
        &format!("/nodes/{side}"),
        Body::from("{}"),
    )));
    waiting_for_a_lock(&s.world, blocker_pid).await;

    // The revocation waits for nobody's open change...
    let mut tx = s.world.db.pool.begin().await.unwrap();
    access::lock_system_defaults(&mut tx).await.unwrap();
    access::end_role(&mut tx, role, s.owner.id(), s.world.now())
        .await
        .unwrap();
    access::bump_epoch(&mut tx).await.unwrap();
    tokio::time::timeout(LOCK_WAIT_BOUND, tx.commit())
        .await
        .expect("a revocation does not wait for the guarded close")
        .unwrap();
    // ...and the close, which decided under the authority it ended, cannot commit after it.
    blocker.rollback().await.unwrap();
    let response = action.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let answer = response_json(response).await;
    assert_eq!(answer["code"], "conflict.stale_revision");
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id = $1")
            .bind(side)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert!(
        !deleted,
        "a close refused at its commit leaves the side thread"
    );
}

#[tokio::test]
async fn a_new_service_token_still_needs_a_future_expiry_after_waiting_for_its_action() {
    let s = Scenario::new().await;
    let service = s.service().await;
    s.grant(s.from.base.id, "members.manage", false).await;
    let before = snapshot(&s.world).await;
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    // The action waits at its guard for the writer's own row; no barrier is shared.
    orch_db::queries::transaction_guard::lock_person(&mut blocker, s.writer.id())
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    // Making a token takes its maker's fresh proof of who they are.
    let value = json!({"name": "Must still be future", "expires_at": orch_types::Time::new(
        s.world.now() + Duration::seconds(AUTHORITY_SECONDS)
    ), "proof": {"password": support::PASSWORD}});
    let action = tokio::spawn(router(&s.world).oneshot(request(
        &s.writer,
        "POST",
        &format!("/service-accounts/{service}/tokens"),
        Body::from(serde_json::to_vec(&value).unwrap()),
    )));
    waiting_for_a_lock(&s.world, pid).await;
    s.world.advance(Duration::seconds(DELIVERY_SECONDS));
    blocker.rollback().await.unwrap();
    let response = action.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let answer = response_json(response).await;
    assert_eq!(answer["code"], "request.invalid");
    assert_eq!(answer["errors"][0]["path"], "expires_at");
    assert_eq!(snapshot(&s.world).await, before);
}

#[tokio::test]
async fn a_standalone_side_close_waits_for_authority_before_choosing_its_time() {
    for waits in [true, false] {
        let s = Scenario::new().await;
        let (_parent, side) = s.side().await;
        let engine = s.world.app.app.engine.clone();
        let actor = s.writer.id();
        if waits {
            // An open change of authority holds nobody up: the close runs now,
            // and chooses its time when it runs.
            let mut blocker = s.world.db.pool.begin().await.unwrap();
            access::lock_system_defaults(&mut blocker).await.unwrap();
            let closed = tokio::time::timeout(
                LOCK_WAIT_BOUND,
                engine.close_side_thread(side, "closed", Some(actor), None),
            )
            .await
            .expect("the close does not wait for an open change of authority")
            .unwrap();
            assert!(closed);
            blocker.rollback().await.unwrap();
        } else {
            assert!(
                engine
                    .close_side_thread(side, "closed", Some(actor), None)
                    .await
                    .unwrap()
            );
        }
        let deleted: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT deleted_at FROM nodes WHERE id = $1")
                .bind(side)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            deleted,
            s.world.now(),
            "the close uses time after its entry"
        );
        println!("guard_nodes standalone_close waited={waits} time={deleted}");
    }
}
