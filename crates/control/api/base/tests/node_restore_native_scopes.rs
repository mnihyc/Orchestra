//! Retained restoration keeps its actual parent/resources, Undo gates and independent readback.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::nodes::{NodeGet, NodeUpdate, NodeUpdateBody};
use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody, NodeRestore, NodeRestoreBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::BTreeSet;
use support::World;

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, action: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],
        "allow":[{"operation":operation,"actions":[action]}]})
}
async fn key(world: &World, actor: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Native restore fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn checked(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn delete(world: &World, token: &str, id: Id) -> Id {
    world
        .app
        .call(NodeDelete, &[&id])
        .token(token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
        .ok()
        .deletion_id
        .expect("a native deletion has a group")
}
async fn restored(world: &World, id: Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='node.restored' AND object_id=$1",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}
async fn deleted(world: &World, id: Id) -> bool {
    sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
fn created_document(reply: RawReply) -> Id {
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<orch_api_contract::documents::DocumentCreationResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .expect("the ordinary native document creation returns full content")
        .base
        .id
}

/// A valid stored bare thread, as native imports create; these controls need no model session.
async fn stored_thread(world: &World, owner: Id, topic: Id, name: &str) -> Id {
    let id = new_id();
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        orch_db::queries::threads::insert_thread(&mut tx, id, topic, name, owner, world.now())
            .await
            .unwrap()
    );
    orch_authz::pages::start(
        &mut tx,
        orch_access_contract::pages::StoredKind::Node,
        id,
        owner,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    orch_db::queries::threads::make_node_chat(&mut tx, id, owner, world.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

struct Scene {
    world: World,
    owner: places::Who,
    org: Id,
    node: Id,
    child: Id,
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = places::person(&world, "restore-owner").await;
        let org = places::make_org(&world, &owner, "Private restore parent", "restore-parent")
            .await
            .base
            .id;
        let node = places::make_project(&world, &owner, org, "Private restored project")
            .await
            .base
            .id;
        let child = places::make_topic(&world, &owner, node, "Private restored child")
            .await
            .base
            .id;
        Self {
            world,
            owner,
            org,
            node,
            child,
        }
    }
    async fn department() -> Self {
        let world = World::new().await;
        let owner = places::person(&world, "restore-owner").await;
        let org = places::make_org(&world, &owner, "Private restore parent", "restore-parent")
            .await
            .base
            .id;
        let node = places::make_department(
            &world,
            &owner,
            org,
            "Private restored department",
            "restore-department",
        )
        .await
        .base
        .id;
        let child = places::make_project(&world, &owner, node, "Private restored project")
            .await
            .base
            .id;
        Self {
            world,
            owner,
            org,
            node,
            child,
        }
    }
    fn manage(&self) -> Vec<Json> {
        vec![rule("NodeRestore", "manage", "org", self.org)]
    }
    fn read(&self) -> Vec<Json> {
        vec![
            rule("NodeRestore", "read", "project", self.node),
            rule("NodeRestore", "read", "org", self.org),
        ]
    }
    async fn call(&self, secret: &str, group: Id, undo: bool) -> RawReply {
        self.world
            .app
            .call(NodeRestore, &[&self.node])
            .token(secret)
            .body(&NodeRestoreBody {
                undo_delete: undo.then_some(group),
                expected_deletion_id: Some(group),
            })
            .send_bytes()
            .await
    }
    async fn template(&self) -> Id {
        let mut conn = self.world.conn().await;
        let id = resources::access_fixtures::insert_resource(
            &mut conn,
            "template",
            Some(self.owner.id()),
            Some(self.node),
            "Private restored template",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Template,
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
    async fn readme(&self) -> Id {
        let id = created_document(
            self.world
                .app
                .call(NodeDocumentCreate, &[&self.node])
                .token(&self.owner.token)
                .body(&NodeDocumentCreateBody {
                    title: "Private restored README".to_owned(),
                    blocks: None,
                    markdown: Some("Private restored text".to_owned()),
                    access: None,
                })
                .send_bytes()
                .await,
        );
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM nodes WHERE id=$1")
            .bind(self.node)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap();
        self.world
            .app
            .call(NodeUpdate, &[&self.node])
            .token(&self.owner.token)
            .body(&NodeUpdateBody {
                name: None,
                description: None,
                color: None,
                custom_color: None,
                readme_document_id: Some(Some(id)),
                expected_revision: revision,
            })
            .send()
            .await
            .ok();
        id
    }
}
fn receipt(body: &Json, node: Id) {
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body["node_id"], json!(node));
    assert!(body.get("name").is_none());
    assert!(body.get("path").is_none());
    assert!(!body.to_string().contains("Private"));
}

#[tokio::test]
async fn restoration_scopes_the_real_parent_and_keeps_native_authority_and_manage_only_output() {
    let scene = Scene::new().await;
    let group = delete(&scene.world, &scene.owner.token, scene.node).await;
    for rules in [
        vec![rule("NodeRestore", "manage", "project", scene.node)],
        vec![rule("NodeRestore", "write", "org", scene.org)],
        vec![rule("NodeDelete", "manage", "org", scene.org)],
    ] {
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        assert_eq!(
            scene.call(&secret.secret, group, false).await.status,
            StatusCode::FORBIDDEN
        );
        assert!(deleted(&scene.world, scene.node).await);
        assert_eq!(restored(&scene.world, scene.node).await, 0);
    }
    let viewer = places::person(&scene.world, "restore-viewer").await;
    places::join(
        &scene.world,
        scene.org,
        scene.org,
        &viewer,
        "viewer",
        &scene.owner,
    )
    .await;
    let secret = key(&scene.world, viewer.id(), scene.manage()).await;
    assert_eq!(
        scene.call(&secret.secret, group, false).await.status,
        StatusCode::FORBIDDEN
    );
    let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
    let body = checked(scene.call(&secret.secret, group, false).await);
    receipt(&body, scene.node);
    assert_eq!(body["kind"], "project");
    let nodes = body["restored_nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| serde_json::from_value::<Id>(id.clone()).unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(nodes, BTreeSet::from([scene.node, scene.child]));
    assert!(!deleted(&scene.world, scene.node).await);
    assert_eq!(restored(&scene.world, scene.node).await, 1);
}

#[tokio::test]
async fn restoration_of_a_retained_root_uses_its_native_owner_rule_and_identity() {
    let scene = Scene::new().await;
    let group = delete(&scene.world, &scene.owner.token, scene.org).await;
    let secret = key(
        &scene.world,
        scene.owner.id(),
        vec![rule("NodeRestore", "manage", "org", scene.org)],
    )
    .await;
    let body = checked(
        scene
            .world
            .app
            .call(NodeRestore, &[&scene.org])
            .token(&secret.secret)
            .body(&NodeRestoreBody {
                undo_delete: None,
                expected_deletion_id: Some(group),
            })
            .send_bytes()
            .await,
    );
    receipt(&body, scene.org);
    assert_eq!(body["kind"], "org");
    assert!(!deleted(&scene.world, scene.org).await);
    assert!(!deleted(&scene.world, scene.node).await);
}

#[tokio::test]
async fn restoration_full_body_needs_its_own_root_names_and_native_readme_sources() {
    for mode in ["borrowed", "missing-parent", "missing-readme", "full"] {
        let scene = Scene::new().await;
        let readme = scene.readme().await;
        let group = delete(&scene.world, &scene.owner.token, scene.node).await;
        let mut rules = scene.manage();
        if mode == "borrowed" {
            rules.extend([
                rule("NodeGet.read", "read", "project", scene.node),
                rule("DocumentGet.read", "read", "document", readme),
            ]);
        } else {
            rules.extend(scene.read().into_iter().filter(|rule| {
                mode != "missing-parent" || rule["target"]["id"] != json!(scene.org)
            }));
            if mode != "missing-readme" {
                rules.push(rule("NodeRestore", "read", "document", readme));
            }
        }
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let body = checked(scene.call(&secret.secret, group, false).await);
        if mode == "full" {
            assert_eq!(body["id"], json!(scene.node));
            assert_eq!(body["readme"]["id"], json!(readme));
            assert_eq!(
                body,
                checked(
                    scene
                        .world
                        .app
                        .call(NodeGet, &[&scene.node])
                        .token(&scene.owner.token)
                        .send_bytes()
                        .await
                )
            );
        } else {
            receipt(&body, scene.node);
        }
        assert!(!deleted(&scene.world, scene.node).await);
    }
}

#[tokio::test]
async fn restoration_undo_binds_every_real_reopened_node_and_preserves_the_archive_states() {
    let scene = Scene::new().await;
    let group = delete(&scene.world, &scene.owner.token, scene.node).await;
    let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
    assert_eq!(
        scene.call(&secret.secret, group, true).await.status,
        StatusCode::FORBIDDEN
    );
    assert!(deleted(&scene.world, scene.node).await);
    let mut rules = scene.manage();
    rules.extend([
        rule("NodeRestore", "manage", "project", scene.node),
        rule("NodeRestore", "manage", "topic", scene.child),
    ]);
    let secret = key(&scene.world, scene.owner.id(), rules).await;
    receipt(
        &checked(scene.call(&secret.secret, group, true).await),
        scene.node,
    );
    let archived: Vec<bool> =
        sqlx::query_scalar("SELECT archived_at IS NOT NULL FROM nodes WHERE id=ANY($1)")
            .bind(vec![scene.node, scene.child])
            .fetch_all(&scene.world.db.pool)
            .await
            .unwrap();
    assert_eq!(archived, [false, false]);
}

#[tokio::test]
async fn restoration_binds_the_native_saved_resources_and_keeps_preexisting_disablement() {
    let scene = Scene::department().await;
    let enabled = scene.template().await;
    let already_off = scene.template().await;
    sqlx::query("UPDATE env_templates SET enabled=false WHERE id=$1")
        .bind(already_off)
        .execute(&scene.world.db.pool)
        .await
        .unwrap();
    let group = delete(&scene.world, &scene.owner.token, scene.node).await;
    let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
    assert_eq!(
        scene.call(&secret.secret, group, false).await.status,
        StatusCode::FORBIDDEN
    );
    assert!(deleted(&scene.world, scene.node).await);
    let mut rules = scene.manage();
    rules.push(rule("NodeRestore", "manage", "template", enabled));
    let secret = key(&scene.world, scene.owner.id(), rules).await;
    let body = checked(scene.call(&secret.secret, group, false).await);
    receipt(&body, scene.node);
    assert_eq!(body["restored_resources"], json!([enabled]));
    let on: bool = sqlx::query_scalar("SELECT enabled FROM env_templates WHERE id=$1")
        .bind(enabled)
        .fetch_one(&scene.world.db.pool)
        .await
        .unwrap();
    let off: bool = sqlx::query_scalar("SELECT enabled FROM env_templates WHERE id=$1")
        .bind(already_off)
        .fetch_one(&scene.world.db.pool)
        .await
        .unwrap();
    assert!(on);
    assert!(!off);
}

#[tokio::test]
async fn restoration_rechecks_expiry_after_the_actual_saved_resource_lock() {
    for mode in ["unchanged", "expired"] {
        let scene = Scene::department().await;
        let template = scene.template().await;
        let group = delete(&scene.world, &scene.owner.token, scene.node).await;
        let mut rules = scene.manage();
        rules.push(rule("NodeRestore", "manage", "template", template));
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let mut lock = scene.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM env_templates WHERE id=$1 FOR UPDATE")
            .bind(template)
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let app = scene.world.app.clone();
        let node = scene.node;
        let token = secret.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(NodeRestore, &[&node])
                .token(&token)
                .body(&NodeRestoreBody {
                    expected_deletion_id: Some(group),
                    undo_delete: None,
                });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the real saved-resource serialization must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%env_templates%' AND query LIKE '%FOR UPDATE%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the native resource row wait must be observed");
        assert!(
            deleted(&scene.world, scene.node).await,
            "no restoration has begun before the complete footprint"
        );
        if mode == "expired" {
            scene.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if mode == "unchanged" {
            receipt(&checked(reply), scene.node);
            assert!(!deleted(&scene.world, scene.node).await);
        } else {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(deleted(&scene.world, scene.node).await);
            assert_eq!(restored(&scene.world, scene.node).await, 0);
            let enabled: bool = sqlx::query_scalar("SELECT enabled FROM env_templates WHERE id=$1")
                .bind(template)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            assert!(!enabled);
        }
    }
}

#[tokio::test]
async fn restoration_keeps_its_committed_receipt_and_original_header_association_at_publication() {
    for mode in [
        "unchanged",
        "drop-manage",
        "drop-read",
        "expired",
        "readme-moved",
    ] {
        let scene = Scene::new().await;
        let readme = scene.readme().await;
        let other = places::make_project(
            &scene.world,
            &scene.owner,
            scene.org,
            "Another still-readable place",
        )
        .await
        .base
        .id;
        let group = delete(&scene.world, &scene.owner.token, scene.node).await;
        let mut rules = scene.manage();
        let mut read = scene.read();
        read.extend([
            rule("NodeRestore", "read", "document", readme),
            rule("NodeRestore", "read", "project", other),
        ]);
        rules.extend(read.clone());
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let mut lock = scene.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = scene.world.app.clone();
        let node = scene.node;
        let token = secret.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(NodeRestore, &[&node])
                .token(&token)
                .body(&NodeRestoreBody {
                    expected_deletion_id: Some(group),
                    undo_delete: None,
                });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the committed restoration must reach publication");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual final activity wait must be observed");
        assert!(!deleted(&scene.world, scene.node).await);
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM nodes WHERE id=$1")
            .bind(scene.node)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        if mode == "expired" {
            scene.world.advance(chrono::Duration::hours(2));
        } else if mode == "drop-manage" || mode == "drop-read" {
            let scope: KeyScope = serde_json::from_value(
                json!({"rules":if mode == "drop-manage" { read } else { scene.manage() }}),
            )
            .unwrap();
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(secret.id)
                .bind(json!(scope))
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        } else if mode == "readme-moved" {
            sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
                .bind(readme)
                .bind(other)
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        }
        lock.rollback().await.unwrap();
        let body = checked(request.await.unwrap());
        if mode == "unchanged" || mode == "drop-manage" {
            assert_eq!(body["readme"]["id"], json!(readme));
        } else {
            receipt(&body, scene.node);
        }
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM nodes WHERE id=$1")
            .bind(scene.node)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            xmin, after,
            "optional publication never repeats the restoration"
        );
        assert_eq!(restored(&scene.world, scene.node).await, 1);
        if mode == "readme-moved" {
            assert!(
                checked(
                    scene
                        .world
                        .app
                        .call(NodeGet, &[&scene.node])
                        .token(&scene.owner.token)
                        .send_bytes()
                        .await
                )["readme"]
                    .is_null()
            );
        }
    }
}

#[tokio::test]
async fn restoration_of_a_thread_keeps_its_native_full_header_after_its_actual_parent_and_path_reads()
 {
    let scene = Scene::new().await;
    let thread = stored_thread(
        &scene.world,
        scene.owner.id(),
        scene.child,
        "Private restored thread",
    )
    .await;
    let group = delete(&scene.world, &scene.owner.token, thread).await;
    let secret = key(
        &scene.world,
        scene.owner.id(),
        vec![
            rule("NodeRestore", "manage", "topic", scene.child),
            rule("NodeRestore", "read", "thread", thread),
            rule("NodeRestore", "read", "topic", scene.child),
            rule("NodeRestore", "read", "project", scene.node),
            rule("NodeRestore", "read", "org", scene.org),
        ],
    )
    .await;
    let body = checked(
        scene
            .world
            .app
            .call(NodeRestore, &[&thread])
            .token(&secret.secret)
            .body(&NodeRestoreBody {
                undo_delete: None,
                expected_deletion_id: Some(group),
            })
            .send_bytes()
            .await,
    );
    assert_eq!(body["id"], json!(thread));
    assert_eq!(body["kind"], "thread");
    assert_eq!(body["archived"], true);
    assert!(body.get("projection").is_none());
}

#[tokio::test]
async fn restoration_pins_its_actual_key_until_the_native_change_commits_and_new_scope_applies() {
    let scene = Scene::department().await;
    let template = scene.template().await;
    let group = delete(&scene.world, &scene.owner.token, scene.node).await;
    let mut rules = scene.manage();
    rules.push(rule("NodeRestore", "manage", "template", template));
    let secret = key(&scene.world, scene.owner.id(), rules).await;
    let mut audit_lock = scene.world.db.pool.begin().await.unwrap();
    let audit_blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *audit_lock)
        .await
        .unwrap();
    // Discovery locks precede admission. Block the real accepted effect tail instead.
    sqlx::query("LOCK TABLE audit_log IN SHARE MODE")
        .execute(&mut *audit_lock)
        .await
        .unwrap();
    let app = scene.world.app.clone();
    let node = scene.node;
    let token = secret.secret.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(NodeRestore, &[&node])
            .token(&token)
            .body(&NodeRestoreBody {
                undo_delete: None,
                expected_deletion_id: Some(group),
            });
        call.send_bytes().await
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the accepted native audit wait retains its admitted key");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO audit_log%')")
                .bind(audit_blocker).fetch_one(&scene.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the accepted native audit INSERT wait must be observed");
    let mut editor = scene.world.conn().await;
    let editor_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *editor)
        .await
        .unwrap();
    let scope: KeyScope = serde_json::from_value(json!({"rules":scene.manage()})).unwrap();
    let id = secret.id;
    let edit = tokio::spawn(async move {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(id)
            .bind(json!(scope))
            .execute(&mut *editor)
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!edit.is_finished(), "the active native frame must pin its real credential");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock' AND cardinality(pg_blocking_pids(pid))>0 AND query LIKE '%UPDATE api_tokens SET operation_scope%')")
                .bind(editor_pid).fetch_one(&scene.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual concurrent key edit must wait for the native frame");
    audit_lock.rollback().await.unwrap();
    receipt(&checked(request.await.unwrap()), scene.node);
    edit.await.unwrap();
    assert!(!deleted(&scene.world, scene.node).await);
    assert_eq!(restored(&scene.world, scene.node).await, 1);
    let next = delete(&scene.world, &scene.owner.token, scene.node).await;
    assert_eq!(
        scene.call(&secret.secret, next, false).await.status,
        StatusCode::FORBIDDEN
    );
    assert!(
        deleted(&scene.world, scene.node).await,
        "the later operation observes the narrowed resource scope"
    );
}
