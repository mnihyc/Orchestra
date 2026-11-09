//! Native setting edit/clear retains exact subjects, lock/revision/no-op and receipt semantics.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod rig;
#[path = "node_setting_native_scopes/runtime.rs"]
mod runtime;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::settings::{
    NodeSettingDelete, NodeSettingDeleteQuery, NodeSettingSet, NodeSettingSetBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources, settings};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, action: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":[action]}]})
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
            name: "Native setting fixture",
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
        let owner = places::person(&world, "setting-owner").await;
        let org = places::make_org(&world, &owner, "Private setting parent", "setting-parent")
            .await
            .base
            .id;
        let node = places::make_project(&world, &owner, org, "Private setting project")
            .await
            .base
            .id;
        Self {
            world,
            owner,
            org,
            node,
        }
    }
    fn manage(&self, operation: &str) -> Vec<Json> {
        vec![rule(operation, "manage", "project", self.node)]
    }
    async fn set(&self, token: &str, value: &str, revision: Option<i64>) -> RawReply {
        self.world
            .app
            .call(NodeSettingSet, &[&self.node, &keys::AGENT_INSTRUCTIONS])
            .token(token)
            .body(&NodeSettingSetBody {
                value: json!(value),
                locked: None,
                expected_revision: revision,
            })
            .send_bytes()
            .await
    }
    async fn clear(&self, token: &str, revision: Option<i64>) -> RawReply {
        self.world
            .app
            .call(NodeSettingDelete, &[&self.node, &keys::AGENT_INSTRUCTIONS])
            .token(token)
            .query(&NodeSettingDeleteQuery {
                expected_revision: revision,
            })
            .send_bytes()
            .await
    }
    async fn own(&self) -> Option<settings::SettingRow> {
        settings::row(
            &mut *self.world.conn().await,
            Some(self.node),
            keys::AGENT_INSTRUCTIONS,
        )
        .await
        .unwrap()
    }
    async fn updates(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='settings.updated' AND details->>'key'=$2")
            .bind(self.node).bind(keys::AGENT_INSTRUCTIONS).fetch_one(&self.world.db.pool).await.unwrap()
    }
    async fn resets(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='settings.reset' AND details->>'key'=$2")
            .bind(self.node).bind(keys::AGENT_INSTRUCTIONS).fetch_one(&self.world.db.pool).await.unwrap()
    }
}
fn receipt(body: &Json, node: Id) {
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body["node_id"], json!(node));
    assert_eq!(body["key"], keys::AGENT_INSTRUCTIONS);
    assert!(body.get("value").is_none());
    assert!(body.get("source").is_none());
    assert!(!body.to_string().contains("Private"));
}

#[tokio::test]
async fn setting_set_checks_actual_operation_action_node_and_native_authority_before_writing() {
    let scene = Scene::new().await;
    for rules in [
        vec![rule("NodeSettingSet", "manage", "org", scene.org)],
        vec![rule("NodeSettingSet", "write", "project", scene.node)],
        scene.manage("NodeSettingDelete"),
    ] {
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        assert_eq!(
            scene
                .set(&secret.secret, "Private unaccepted value", None)
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert!(scene.own().await.is_none());
    }
    let viewer = places::person(&scene.world, "setting-viewer").await;
    places::join(
        &scene.world,
        scene.org,
        scene.org,
        &viewer,
        "viewer",
        &scene.owner,
    )
    .await;
    let secret = key(&scene.world, viewer.id(), scene.manage("NodeSettingSet")).await;
    assert_eq!(
        scene
            .set(&secret.secret, "Private unaccepted value", None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(scene.own().await.is_none());
    let secret = key(
        &scene.world,
        scene.owner.id(),
        scene.manage("NodeSettingSet"),
    )
    .await;
    let body = checked(
        scene
            .set(&secret.secret, "Private accepted value", Some(0))
            .await,
    );
    receipt(&body, scene.node);
    assert_eq!(body["own_revision"], 1);
    assert_eq!(body["changed"], true);
    assert_eq!(
        scene.own().await.unwrap().value,
        json!("Private accepted value")
    );
    assert_eq!(scene.updates().await, 1);
}

#[tokio::test]
async fn setting_set_noop_and_revision_fences_keep_the_native_row_and_effect_count() {
    let scene = Scene::new().await;
    let secret = key(
        &scene.world,
        scene.owner.id(),
        scene.manage("NodeSettingSet"),
    )
    .await;
    checked(
        scene
            .set(&secret.secret, "Private unchanged value", Some(0))
            .await,
    );
    let before = scene.own().await.unwrap();
    let body = checked(
        scene
            .set(
                &secret.secret,
                "Private unchanged value",
                Some(before.revision),
            )
            .await,
    );
    receipt(&body, scene.node);
    assert_eq!(body["changed"], false);
    assert_eq!(body["own_revision"], before.revision);
    assert_eq!(scene.own().await.unwrap(), before);
    assert_eq!(scene.updates().await, 1);
    assert_eq!(
        scene
            .set(&secret.secret, "Private refused overwrite", Some(0))
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(scene.own().await.unwrap(), before);
    assert_eq!(scene.updates().await, 1);
}

#[tokio::test]
async fn setting_clear_checks_its_own_manage_and_keeps_a_truthful_empty_row_receipt() {
    let scene = Scene::new().await;
    checked(
        scene
            .set(&scene.owner.token, "Private own value", Some(0))
            .await,
    );
    for rules in [
        scene.manage("NodeSettingSet"),
        vec![rule("NodeSettingDelete", "manage", "org", scene.org)],
        vec![rule("NodeSettingDelete", "read", "project", scene.node)],
    ] {
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        assert_eq!(
            scene.clear(&secret.secret, None).await.status,
            StatusCode::FORBIDDEN
        );
        assert!(scene.own().await.is_some());
    }
    let secret = key(
        &scene.world,
        scene.owner.id(),
        scene.manage("NodeSettingDelete"),
    )
    .await;
    assert_eq!(
        scene.clear(&secret.secret, Some(99)).await.status,
        StatusCode::CONFLICT
    );
    assert!(scene.own().await.is_some());
    let body = checked(scene.clear(&secret.secret, Some(1)).await);
    receipt(&body, scene.node);
    assert!(body["own_revision"].is_null());
    assert_eq!(body["changed"], true);
    assert!(scene.own().await.is_none());
    assert_eq!(scene.resets().await, 1);
    let body = checked(scene.clear(&secret.secret, None).await);
    receipt(&body, scene.node);
    assert_eq!(body["changed"], false);
    assert_eq!(scene.resets().await, 1);
}

#[tokio::test]
async fn setting_full_readback_uses_its_own_read_and_bounds_inherited_provenance_names() {
    for clear in [false, true] {
        for mode in ["borrowed", "own", "names"] {
            let scene = Scene::new().await;
            checked(
                scene
                    .world
                    .app
                    .call(NodeSettingSet, &[&scene.org, &keys::AGENT_INSTRUCTIONS])
                    .token(&scene.owner.token)
                    .body(&NodeSettingSetBody {
                        value: json!("Private inherited instructions"),
                        locked: None,
                        expected_revision: None,
                    })
                    .send_bytes()
                    .await,
            );
            if clear {
                checked(
                    scene
                        .set(&scene.owner.token, "Private own instructions", None)
                        .await,
                );
            }
            let operation = if clear {
                "NodeSettingDelete"
            } else {
                "NodeSettingSet"
            };
            let mut rules = scene.manage(operation);
            rules.push(rule(
                if mode == "borrowed" {
                    "NodeGet.read"
                } else {
                    operation
                },
                "read",
                "project",
                scene.node,
            ));
            if mode == "names" {
                rules.push(rule(operation, "read", "org", scene.org));
            }
            let secret = key(&scene.world, scene.owner.id(), rules).await;
            let body = checked(if clear {
                scene.clear(&secret.secret, None).await
            } else {
                scene
                    .set(&secret.secret, "Private own instructions", None)
                    .await
            });
            if mode == "borrowed" {
                receipt(&body, scene.node);
            } else {
                assert!(body.get("projection").is_none());
                assert_eq!(body["key"], keys::AGENT_INSTRUCTIONS);
                assert!(
                    body["value"]
                        .as_str()
                        .unwrap()
                        .contains("Private inherited instructions")
                );
                if clear {
                    assert_eq!(body["source"]["node"]["id"], json!(scene.org));
                    assert_eq!(body["source"]["node"]["name"].is_null(), mode != "names");
                }
            }
        }
    }
}

#[tokio::test]
async fn setting_write_and_clear_refresh_lifetime_after_the_actual_snapshot_row_wait() {
    for clear in [false, true] {
        for expire in [false, true] {
            let scene = Scene::new().await;
            checked(
                scene
                    .set(&scene.owner.token, "Private original instructions", None)
                    .await,
            );
            let before = scene.own().await.unwrap();
            let operation = if clear {
                "NodeSettingDelete"
            } else {
                "NodeSettingSet"
            };
            let secret = key(&scene.world, scene.owner.id(), scene.manage(operation)).await;
            let mut lock = scene.world.db.pool.begin().await.unwrap();
            let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *lock)
                .await
                .unwrap();
            sqlx::query("SELECT node_id FROM settings WHERE node_id=$1 AND key=$2 FOR UPDATE")
                .bind(scene.node)
                .bind(keys::AGENT_INSTRUCTIONS)
                .fetch_one(&mut *lock)
                .await
                .unwrap();
            let app = scene.world.app.clone();
            let node = scene.node;
            let token = secret.secret;
            let request = tokio::spawn(async move {
                if clear {
                    let call = app
                        .call(NodeSettingDelete, &[&node, &keys::AGENT_INSTRUCTIONS])
                        .token(&token)
                        .query(&NodeSettingDeleteQuery::default());
                    call.send_bytes().await
                } else {
                    let call = app
                        .call(NodeSettingSet, &[&node, &keys::AGENT_INSTRUCTIONS])
                        .token(&token)
                        .body(&NodeSettingSetBody {
                            value: json!("Private replacement instructions"),
                            locked: None,
                            expected_revision: None,
                        });
                    call.send_bytes().await
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(30), async { loop {
            assert!(!request.is_finished(), "the actual setting snapshot wait must be reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM settings%' AND query LIKE '%FOR UPDATE%')")
                .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }}).await.expect("the actual native setting row lock must be observed");
            if expire {
                scene.world.advance(chrono::Duration::hours(2));
            }
            lock.rollback().await.unwrap();
            let reply = request.await.unwrap();
            if expire {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                assert_eq!(scene.own().await.unwrap(), before);
                assert_eq!(scene.resets().await, 0);
                assert_eq!(scene.updates().await, 1);
            } else {
                receipt(&checked(reply), scene.node);
                assert_eq!(scene.own().await.is_none(), clear);
            }
        }
    }
}

#[tokio::test]
async fn setting_committed_value_or_clear_keeps_its_receipt_after_publication_read_or_key_loss() {
    for clear in [false, true] {
        for mode in ["unchanged", "drop-manage", "drop-read", "expired"] {
            let scene = Scene::new().await;
            checked(
                scene
                    .set(&scene.owner.token, "Private original instructions", None)
                    .await,
            );
            let operation = if clear {
                "NodeSettingDelete"
            } else {
                "NodeSettingSet"
            };
            let read = vec![rule(operation, "read", "project", scene.node)];
            let mut rules = scene.manage(operation);
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
                if clear {
                    let call = app
                        .call(NodeSettingDelete, &[&node, &keys::AGENT_INSTRUCTIONS])
                        .token(&token)
                        .query(&NodeSettingDeleteQuery::default());
                    call.send_bytes().await
                } else {
                    let call = app
                        .call(NodeSettingSet, &[&node, &keys::AGENT_INSTRUCTIONS])
                        .token(&token)
                        .body(&NodeSettingSetBody {
                            value: json!("Private committed instructions"),
                            locked: None,
                            expected_revision: None,
                        });
                    call.send_bytes().await
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(30), async { loop {
            assert!(!request.is_finished(), "the committed native edit must reach activity publication");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }}).await.expect("the actual final activity INSERT must be observed");
            let own = scene.own().await;
            assert_eq!(own.is_none(), clear);
            if mode == "expired" {
                scene.world.advance(chrono::Duration::hours(2));
            } else if mode == "drop-manage" || mode == "drop-read" {
                let scope: KeyScope = serde_json::from_value(json!({"rules":if mode == "drop-manage" { read } else { scene.manage(operation) }})).unwrap();
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(secret.id)
                    .bind(json!(scope))
                    .execute(&scene.world.db.pool)
                    .await
                    .unwrap();
            }
            lock.rollback().await.unwrap();
            let body = checked(request.await.unwrap());
            if mode == "drop-read" || mode == "expired" {
                receipt(&body, scene.node);
            } else {
                assert!(body.get("projection").is_none());
            }
            assert_eq!(scene.own().await, own);
            assert_eq!(scene.updates().await, if clear { 1 } else { 2 });
            assert_eq!(scene.resets().await, i64::from(clear));
        }
    }
}

#[tokio::test]
async fn setting_a_bound_threads_directory_scopes_its_actual_environment_governor() {
    let scene = rig::Scene::new().await;
    let world = &scene.rig.world;
    // Directory application uses the actual existing provider/profile/environment binding.
    // Setting only threads.environment_id leaves a genuinely unbound thread.
    let thread = scene.thread().await.base.id;
    let environment = scene.on.environment;
    let wrong = key(
        world,
        scene.ada.id(),
        vec![rule("NodeSettingSet", "manage", "thread", thread)],
    )
    .await;
    let request = NodeSettingSetBody {
        value: json!("src"),
        locked: None,
        expected_revision: None,
    };
    let refused = world
        .app
        .call(NodeSettingSet, &[&thread, &keys::THREAD_CWD])
        .token(&wrong.secret)
        .body(&request);
    assert_eq!(refused.send_bytes().await.status, StatusCode::FORBIDDEN);
    let right = key(
        world,
        scene.ada.id(),
        vec![rule("NodeSettingSet", "manage", "environment", environment)],
    )
    .await;
    let applied = world
        .app
        .call(NodeSettingSet, &[&thread, &keys::THREAD_CWD])
        .token(&right.secret)
        .body(&request);
    let body = checked(applied.send_bytes().await);
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body["node_id"], json!(thread));
    assert_eq!(
        settings::row(&mut *world.conn().await, Some(thread), keys::THREAD_CWD)
            .await
            .unwrap()
            .unwrap()
            .value,
        json!("src")
    );
}

#[tokio::test]
async fn setting_refusals_never_disclose_current_values_or_ancestor_names_without_their_own_read() {
    for clear in [false, true] {
        let scene = Scene::new().await;
        checked(
            scene
                .set(&scene.owner.token, "Private stale setting contents", None)
                .await,
        );
        let operation = if clear {
            "NodeSettingDelete"
        } else {
            "NodeSettingSet"
        };
        let secret = key(&scene.world, scene.owner.id(), scene.manage(operation)).await;
        let stale = if clear {
            scene.clear(&secret.secret, Some(99)).await
        } else {
            scene
                .set(&secret.secret, "Unaccepted value", Some(99))
                .await
        };
        assert_eq!(stale.status, StatusCode::CONFLICT);
        let stale: Json = serde_json::from_slice(&stale.raw).unwrap();
        assert!(
            stale.get("current").is_none(),
            "a revision refusal is not a loan of current setting Read"
        );
        assert!(stale.get("changed_by").is_none());
        assert!(!stale.to_string().contains("Private"));
        checked(
            scene
                .world
                .app
                .call(NodeSettingSet, &[&scene.org, &keys::AGENT_INSTRUCTIONS])
                .token(&scene.owner.token)
                .body(&NodeSettingSetBody {
                    value: json!("Private locked parent contents"),
                    locked: Some(true),
                    expected_revision: None,
                })
                .send_bytes()
                .await,
        );
        let locked = if clear {
            scene.clear(&secret.secret, None).await
        } else {
            scene.set(&secret.secret, "Unaccepted value", None).await
        };
        assert_eq!(locked.status, StatusCode::CONFLICT);
        assert!(!String::from_utf8_lossy(&locked.raw).contains("Private"));
    }
}
