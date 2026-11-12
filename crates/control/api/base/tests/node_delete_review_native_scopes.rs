//! Native stored deletion reviews bind exact scope, optional content and confirmation origin.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody, NodeDeletePreview};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::CapturedOperation;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use serde_json::json;
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
            name: "Native review fixture",
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
struct Scene {
    world: World,
    owner: places::Who,
    org: Id,
    node: Id,
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = places::person(&world, "review-owner").await;
        let org = places::make_org(&world, &owner, "Private review parent", "review-parent")
            .await
            .base
            .id;
        let node = places::make_project(&world, &owner, org, "Private review project")
            .await
            .base
            .id;
        places::make_topic(&world, &owner, node, "Private child").await;
        world
            .app
            .call(NodeDocumentCreate, &[&node])
            .token(&owner.token)
            .body(&NodeDocumentCreateBody {
                title: "Private reviewed document".to_owned(),
                blocks: None,
                markdown: Some("Private body".to_owned()),
                access: None,
            })
            .send()
            .await
            .ok();
        Self {
            world,
            owner,
            org,
            node,
        }
    }
    fn manage(&self) -> Vec<Json> {
        vec![rule("NodeDeletePreview", "manage", "project", self.node)]
    }
    fn read(&self) -> Vec<Json> {
        vec![
            rule("NodeDeletePreview", "read", "project", self.node),
            rule("NodeDeletePreview", "read", "org", self.org),
        ]
    }
    async fn preview(&self, secret: &str) -> RawReply {
        self.world
            .app
            .call(NodeDeletePreview, &[&self.node])
            .token(secret)
            .send_bytes()
            .await
    }
    async fn reviews(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM preflights WHERE kind='node_delete' AND node_id=$1",
        )
        .bind(self.node)
        .fetch_one(&self.world.db.pool)
        .await
        .unwrap()
    }
    async fn deleted(&self) -> bool {
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(self.node)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn deletion_review_manage_only_stores_the_native_counts_and_original_capability() {
    let scene = Scene::new().await;
    for rules in [
        vec![rule("NodeDeletePreview", "manage", "org", scene.org)],
        vec![rule("NodeDeletePreview", "write", "project", scene.node)],
        vec![rule("NodeDelete", "manage", "project", scene.node)],
    ] {
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        assert_eq!(
            scene.preview(&secret.secret).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(scene.reviews().await, 0);
    }
    let viewer = places::person(&scene.world, "review-viewer").await;
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
        scene.preview(&secret.secret).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(scene.reviews().await, 0);
    let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
    let body = checked(scene.preview(&secret.secret).await);
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body["node_id"], json!(scene.node));
    assert_eq!(
        body["counts"],
        json!({"orgs":0,"projects":1,"topics":1,"threads":0,"documents":1})
    );
    assert!(body.get("node").is_none());
    assert!(body.get("restore_by").is_none());
    assert!(!body.to_string().contains("Private"));
    let preflight: Id = serde_json::from_value(body["preflight_id"].clone()).unwrap();
    let row = orch_db::queries::jobs::forks::preflight(&mut *scene.world.conn().await, preflight)
        .await
        .unwrap()
        .unwrap();
    let origin: CapturedOperation = serde_json::from_value(row.plan["origins"][0].clone()).unwrap();
    assert_eq!(origin.operation(), "NodeDeletePreview");
    assert_eq!(origin.user(), scene.owner.id());
    assert_eq!(scene.reviews().await, 1);
    assert!(
        !scene.deleted().await,
        "storing a review never deletes its subject"
    );
}

#[tokio::test]
async fn deletion_review_full_plan_needs_its_own_node_and_restore_reference_read() {
    let scene = Scene::new().await;
    for mode in ["borrowed", "missing-parent", "full"] {
        let mut rules = scene.manage();
        if mode == "borrowed" {
            rules.extend([
                rule("NodeGet", "read", "project", scene.node),
                rule("NodeGet", "read", "org", scene.org),
            ]);
        } else {
            rules.extend(scene.read().into_iter().filter(|rule| {
                mode != "missing-parent" || rule["target"]["id"] != json!(scene.org)
            }));
        }
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let body = checked(scene.preview(&secret.secret).await);
        if mode == "full" {
            assert_eq!(body["node"]["id"], json!(scene.node));
            assert_eq!(body["restore_by"]["place"]["id"], json!(scene.org));
            let mut ordinary = checked(scene.preview(&scene.owner.token).await);
            assert_ne!(ordinary["preflight_id"], body["preflight_id"]);
            ordinary["preflight_id"] = body["preflight_id"].clone();
            assert_eq!(
                body, ordinary,
                "only the independently generated review id differs"
            );
        } else {
            assert_eq!(body["projection"], "receipt");
            assert!(!body.to_string().contains("Private"));
        }
    }
    assert!(!scene.deleted().await);
}

#[tokio::test]
async fn deletion_review_refreshes_expiry_after_the_actual_node_lock_before_saving() {
    for expire in [false, true] {
        let scene = Scene::new().await;
        let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
        let mut lock = scene.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(scene.node)
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let app = scene.world.app.clone();
        let node = scene.node;
        let token = secret.secret;
        let request = tokio::spawn(async move {
            let call = app.call(NodeDeletePreview, &[&node]).token(&token);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the native review node wait must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM nodes WHERE id=$1 FOR UPDATE%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the real node-row wait must be observed");
        if expire {
            scene.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(scene.reviews().await, 0);
        } else {
            assert_eq!(checked(reply)["projection"], "receipt");
            assert_eq!(scene.reviews().await, 1);
        }
        assert!(!scene.deleted().await);
    }
}

#[tokio::test]
async fn deletion_review_publication_preserves_one_stored_result_when_content_closes() {
    for mode in ["unchanged", "drop-manage", "drop-read", "expire"] {
        let scene = Scene::new().await;
        let mut rules = scene.manage();
        rules.extend(scene.read());
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
            let call = app.call(NodeDeletePreview, &[&node]).token(&token);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the committed review must reach activity publication");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the real final activity wait must be observed");
        assert_eq!(scene.reviews().await, 1);
        let before: (Id, String) = sqlx::query_as(
            "SELECT id,xmin::text FROM preflights WHERE kind='node_delete' AND node_id=$1",
        )
        .bind(scene.node)
        .fetch_one(&scene.world.db.pool)
        .await
        .unwrap();
        if mode == "drop-manage" || mode == "drop-read" {
            let scope: KeyScope = serde_json::from_value(
                json!({"rules":if mode == "drop-manage" { scene.read() } else { scene.manage() }}),
            )
            .unwrap();
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(secret.id)
                .bind(json!(scope))
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        } else if mode == "expire" {
            scene.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let body = checked(request.await.unwrap());
        assert_eq!(body["preflight_id"], json!(before.0));
        if mode == "drop-read" || mode == "expire" {
            assert_eq!(body["projection"], "receipt");
        } else {
            assert_eq!(body["node"]["id"], json!(scene.node));
        }
        let after: (Id, String) = sqlx::query_as(
            "SELECT id,xmin::text FROM preflights WHERE kind='node_delete' AND node_id=$1",
        )
        .bind(scene.node)
        .fetch_one(&scene.world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            before, after,
            "publication cannot save a replacement review"
        );
        assert!(!scene.deleted().await);
    }
}

#[tokio::test]
async fn deletion_review_confirmation_retains_the_original_key_even_for_a_browser_confirmer() {
    for mode in ["unchanged", "revoked", "narrowed"] {
        let scene = Scene::new().await;
        let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
        let plan = checked(scene.preview(&secret.secret).await);
        let id: Id = serde_json::from_value(plan["preflight_id"].clone()).unwrap();
        if mode == "revoked" {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(secret.id)
                .bind(scene.world.now())
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        }
        if mode == "narrowed" {
            let scope: KeyScope = serde_json::from_value(
                json!({"rules":[rule("NodeDelete", "manage", "project", scene.node)]}),
            )
            .unwrap();
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(secret.id)
                .bind(json!(scope))
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        }
        let reply = scene
            .world
            .app
            .call(NodeDelete, &[&scene.node])
            .token(&scene.owner.token)
            .body(&NodeDeleteBody {
                preflight_id: Some(id),
                expected_revision: plan["revision"].as_i64(),
                ..NodeDeleteBody::default()
            })
            .send_bytes()
            .await;
        if mode != "unchanged" {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert!(!scene.deleted().await);
            assert_eq!(
                scene.reviews().await,
                1,
                "a refused origin does not consume the review"
            );
        } else {
            assert_eq!(
                reply.status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert!(scene.deleted().await);
            assert_eq!(scene.reviews().await, 0);
        }
    }
}

#[tokio::test]
async fn deletion_review_original_key_is_rechecked_after_the_actual_confirmation_tree_wait() {
    for revoke in [false, true] {
        let scene = Scene::new().await;
        let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
        let plan = checked(scene.preview(&secret.secret).await);
        let preflight: Id = serde_json::from_value(plan["preflight_id"].clone()).unwrap();
        let revision = plan["revision"].as_i64();
        let child: Id =
            sqlx::query_scalar("SELECT id FROM nodes WHERE parent_id=$1 AND kind='topic'")
                .bind(scene.node)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
        let mut lock = scene.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(child)
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let app = scene.world.app.clone();
        let node = scene.node;
        let token = scene.owner.token.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(NodeDelete, &[&node])
                .token(&token)
                .body(&NodeDeleteBody {
                    preflight_id: Some(preflight),
                    expected_revision: revision,
                    ..NodeDeleteBody::default()
                });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "confirmation must reach the native tree wait");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%nodes%' AND query LIKE '%FOR UPDATE%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual reviewed subtree lock must be observed");
        if revoke {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(secret.id)
                .bind(scene.world.now())
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if revoke {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert!(!scene.deleted().await);
            assert_eq!(scene.reviews().await, 1);
        } else {
            assert_eq!(
                reply.status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert!(scene.deleted().await);
            assert_eq!(scene.reviews().await, 0);
        }
    }
}

#[tokio::test]
async fn a_replacement_review_keeps_the_original_key_boundary_instead_of_becoming_a_browser_plan() {
    for revoke in [false, true] {
        let scene = Scene::new().await;
        let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
        let old = checked(scene.preview(&secret.secret).await);
        let old_id: Id = serde_json::from_value(old["preflight_id"].clone()).unwrap();
        places::make_topic(
            &scene.world,
            &scene.owner,
            scene.node,
            "A newly reviewed child",
        )
        .await;
        let stale = scene
            .world
            .app
            .call(NodeDelete, &[&scene.node])
            .token(&scene.owner.token)
            .body(&NodeDeleteBody {
                preflight_id: Some(old_id),
                expected_revision: old["revision"].as_i64(),
                ..NodeDeleteBody::default()
            })
            .send_bytes()
            .await;
        assert_eq!(stale.status, StatusCode::CONFLICT);
        let problem: Json = serde_json::from_slice(&stale.raw).unwrap();
        let current = &problem["current"];
        let id: Id = serde_json::from_value(current["preflight_id"].clone()).unwrap();
        assert_ne!(id, old_id);
        assert!(!scene.deleted().await);
        let row = orch_db::queries::jobs::forks::preflight(&mut *scene.world.conn().await, id)
            .await
            .unwrap()
            .unwrap();
        let origins: Vec<CapturedOperation> =
            serde_json::from_value(row.plan["origins"].clone()).unwrap();
        assert_eq!(origins.len(), 1);
        assert_eq!(origins[0].operation(), "NodeDeletePreview");
        if revoke {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(secret.id)
                .bind(scene.world.now())
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        }
        let reply = scene
            .world
            .app
            .call(NodeDelete, &[&scene.node])
            .token(&scene.owner.token)
            .body(&NodeDeleteBody {
                preflight_id: Some(id),
                expected_revision: current["revision"].as_i64(),
                ..NodeDeleteBody::default()
            })
            .send_bytes()
            .await;
        if revoke {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert!(!scene.deleted().await);
            assert_eq!(scene.reviews().await, 1);
        } else {
            assert_eq!(
                reply.status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert!(scene.deleted().await);
        }
    }
}
