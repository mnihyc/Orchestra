//! Native recoverable deletion scopes actual resource/member acts and retains intrinsic receipts.
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
use orch_api_contract::access::Role;
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::nodes::{CreateThread, ThreadCreate};
use orch_api_contract::ownership::{
    DeletedResources, NodeDelete, NodeDeleteBody, NodeDeleteMembers, NodeDeletePreview,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources, threads};
use orch_types::{Id, Json, Time, new_id};
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
            name: "Native deletion fixture",
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
        let owner = places::person(&world, "deletion-owner").await;
        let org = places::make_org(&world, &owner, "Private deletion root", "deletion-root")
            .await
            .base
            .id;
        let node = places::make_project(&world, &owner, org, "Private deletion project")
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
    fn manage(&self) -> Vec<Json> {
        vec![rule("NodeDelete", "manage", "project", self.node)]
    }
    async fn delete(&self, token: &str, node: Id, body: &NodeDeleteBody) -> RawReply {
        self.world
            .app
            .call(NodeDelete, &[&node])
            .token(token)
            .body(body)
            .send_bytes()
            .await
    }
    async fn gone(&self, node: Id) -> bool {
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(node)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }
    async fn audits(&self, node: Id) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='node.deleted'",
        )
        .bind(node)
        .fetch_one(&self.world.db.pool)
        .await
        .unwrap()
    }
    async fn resource(&self, kind: &str, home: Id) -> Id {
        let mut conn = self.world.conn().await;
        let id = resources::access_fixtures::insert_resource(
            &mut conn,
            kind,
            Some(self.owner.id()),
            Some(home),
            "Private retained resource",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.parse().unwrap(),
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
    async fn thread(&self) -> Id {
        let topic = places::make_topic(
            &self.world,
            &self.owner,
            self.node,
            "Private deletion topic",
        )
        .await
        .base
        .id;
        let body = checked(
            self.world
                .app
                .call(ThreadCreate, &[&topic])
                .token(&self.owner.token)
                .body(&CreateThread::default())
                .send_bytes()
                .await,
        );
        serde_json::from_value(body["id"].clone()).unwrap()
    }
}
fn receipt(body: &Json) {
    assert!(body["deletion_id"].is_string());
    assert_eq!(body.as_object().unwrap().len(), 1);
    assert!(!body.to_string().contains("Private"));
}

#[tokio::test]
async fn node_deletion_uses_its_exact_manage_and_keeps_the_intrinsic_document_cascade() {
    let s = Scene::new().await;
    let topic = places::make_topic(&s.world, &s.owner, s.node, "Private deleted child")
        .await
        .base
        .id;
    let document = checked(
        s.world
            .app
            .call(NodeDocumentCreate, &[&topic])
            .token(&s.owner.token)
            .body(&NodeDocumentCreateBody {
                title: "Private child document".into(),
                blocks: None,
                markdown: Some("Private contents".into()),
                access: None,
            })
            .send_bytes()
            .await,
    );
    let document: Id = serde_json::from_value(document["id"].clone()).unwrap();
    for rules in [
        vec![rule("NodeDelete", "manage", "org", s.org)],
        vec![rule("NodeDelete", "read", "project", s.node)],
        vec![rule("NodeDeletePreview", "manage", "project", s.node)],
    ] {
        let k = key(&s.world, s.owner.id(), rules).await;
        assert_eq!(
            s.delete(&k.secret, s.node, &NodeDeleteBody::default())
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert!(!s.gone(s.node).await);
        assert_eq!(s.audits(s.node).await, 0);
    }
    let viewer = places::person(&s.world, "deletion-viewer").await;
    places::join(&s.world, s.org, s.org, &viewer, "viewer", &s.owner).await;
    let k = key(&s.world, viewer.id(), s.manage()).await;
    assert_eq!(
        s.delete(&k.secret, s.node, &NodeDeleteBody::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(!s.gone(s.node).await);
    let k = key(&s.world, s.owner.id(), s.manage()).await;
    receipt(&checked(
        s.delete(&k.secret, s.node, &NodeDeleteBody::default())
            .await,
    ));
    assert!(s.gone(s.node).await && s.gone(topic).await);
    assert_eq!(s.audits(s.node).await, 1);
    let gone: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM documents WHERE id=$1")
        .bind(document)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    assert!(
        gone,
        "the native hierarchy cascade needs no invented per-document Manage grant"
    );
}

#[tokio::test]
async fn organization_deletion_scopes_the_actual_independent_native_resource_withdrawals() {
    let s = Scene::new().await;
    let template = s.resource("template", s.org).await;
    let mount = s.resource("mount", s.org).await;
    let root = vec![rule("NodeDelete", "manage", "org", s.org)];
    for extra in [
        vec![],
        vec![rule("NodeDelete", "manage", "template", template)],
        vec![rule("NodeDelete", "manage", "mount", mount)],
    ] {
        let mut rules = root.clone();
        rules.extend(extra);
        let k = key(&s.world, s.owner.id(), rules).await;
        assert_eq!(
            s.delete(&k.secret, s.org, &NodeDeleteBody::default())
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert!(!s.gone(s.org).await);
        assert!(
            !resources::resource_facts(&mut *s.world.conn().await, &[template, mount])
                .await
                .unwrap()
                .iter()
                .any(|row| row.disabled)
        );
    }
    let mut rules = root;
    rules.extend([
        rule("NodeDelete", "manage", "template", template),
        rule("NodeDelete", "manage", "mount", mount),
    ]);
    let k = key(&s.world, s.owner.id(), rules).await;
    receipt(&checked(
        s.delete(&k.secret, s.org, &NodeDeleteBody::default()).await,
    ));
    assert!(s.gone(s.org).await);
    assert!(
        resources::resource_facts(&mut *s.world.conn().await, &[template, mount])
            .await
            .unwrap()
            .iter()
            .all(|row| row.disabled)
    );
}

#[tokio::test]
async fn deleting_a_department_keeps_member_roles_and_scopes_the_real_receiving_parent() {
    for handover in [false, true] {
        let s = Scene::new().await;
        let dept = places::make_department(
            &s.world,
            &s.owner,
            s.org,
            "Private deleted department",
            "deleted-department",
        )
        .await
        .base
        .id;
        let member = places::person(&s.world, "deletion-member").await;
        places::join(&s.world, s.org, dept, &member, "viewer", &s.owner).await;
        places::give_role(&s.world, s.org, &member, "admin", &s.owner).await;
        let template = if handover {
            Some(s.resource("template", dept).await)
        } else {
            None
        };
        let mut rules = vec![rule("NodeDelete", "manage", "org", dept)];
        if let Some(id) = template {
            rules.push(rule("NodeDelete", "manage", "template", id));
        }
        let body = NodeDeleteBody {
            members: Some(NodeDeleteMembers {
                role: Some(Role::Developer),
            }),
            resources: handover.then_some(DeletedResources::ToParent),
            ..NodeDeleteBody::default()
        };
        let k = key(&s.world, s.owner.id(), rules.clone()).await;
        assert_eq!(
            s.delete(&k.secret, dept, &body).await.status,
            StatusCode::FORBIDDEN
        );
        assert!(!s.gone(dept).await);
        rules.push(rule("NodeDelete", "manage", "org", s.org));
        let k = key(&s.world, s.owner.id(), rules).await;
        receipt(&checked(s.delete(&k.secret, dept, &body).await));
        let domain: Id =
            sqlx::query_scalar("SELECT domain_id FROM org_members WHERE org_id=$1 AND user_id=$2")
                .bind(s.org)
                .bind(member.id())
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(domain, s.org);
        let role:String=sqlx::query_scalar("SELECT role FROM node_roles WHERE node_id=$1 AND subject_kind='user' AND subject_id=$2 AND removed_at IS NULL").bind(s.org).bind(member.id()).fetch_one(&s.world.db.pool).await.unwrap();
        assert_eq!(role, "admin");
        if let Some(template) = template {
            let fact = resources::resource_facts(&mut *s.world.conn().await, &[template])
                .await
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(fact.home_id, Some(s.org));
            assert!(!fact.disabled);
        }
    }
}

#[tokio::test]
async fn native_thread_and_side_maker_or_environment_closes_keep_their_original_authority() {
    for mode in ["thread", "maker", "environment"] {
        let s = Scene::new().await;
        let parent = s.thread().await;
        let actor = if mode == "thread" {
            None
        } else {
            let who = places::person(&s.world, "side-deletion-actor").await;
            places::join(&s.world, s.org, s.org, &who, "viewer", &s.owner).await;
            Some(who)
        };
        let target = if let Some(actor) = &actor {
            let side = new_id();
            let mut conn = s.world.conn().await;
            assert!(
                threads::forking::insert_side_thread(
                    &mut conn,
                    &threads::forking::NewSide {
                        id: side,
                        parent,
                        name: "Private side",
                        owner: if mode == "maker" {
                            actor.id()
                        } else {
                            s.owner.id()
                        },
                        from_turn: 0,
                        at: s.world.now()
                    }
                )
                .await
                .unwrap()
            );
            if mode == "environment" {
                let environment = resources::access_fixtures::insert_environment(
                    &mut conn,
                    actor.id(),
                    Some(s.node),
                    "Private managed side environment",
                    "managed",
                )
                .await
                .unwrap();
                start_resource(
                    &mut conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: environment,
                    },
                    actor.id(),
                    NewAccess::Defaults,
                    Time::new(s.world.now()),
                )
                .await
                .unwrap();
                sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                    .bind(side)
                    .bind(environment)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
            side
        } else {
            parent
        };
        let actor = actor.as_ref().unwrap_or(&s.owner);
        let k = key(
            &s.world,
            actor.id(),
            vec![rule("NodeDelete", "manage", "thread", target)],
        )
        .await;
        let body = checked(
            s.delete(&k.secret, target, &NodeDeleteBody::default())
                .await,
        );
        if target == parent {
            receipt(&body);
        } else {
            assert!(body["deletion_id"].is_null());
            assert!(!s.gone(parent).await);
        }
        assert!(s.gone(target).await);
    }
}

#[tokio::test]
async fn deletion_rechecks_lifetime_after_the_actual_native_document_row_wait() {
    for expire in [false, true] {
        let s = Scene::new().await;
        let document = checked(
            s.world
                .app
                .call(NodeDocumentCreate, &[&s.node])
                .token(&s.owner.token)
                .body(&NodeDocumentCreateBody {
                    title: "Private locked document".into(),
                    blocks: None,
                    markdown: Some("Private contents".into()),
                    access: None,
                })
                .send_bytes()
                .await,
        );
        let document: Id = serde_json::from_value(document["id"].clone()).unwrap();
        let k = key(&s.world, s.owner.id(), s.manage()).await;
        let mut lock = s.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
            .bind(document)
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let node = s.node;
        let token = k.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(NodeDelete, &[&node])
                .token(&token)
                .body(&NodeDeleteBody::default());
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{assert!(!request.is_finished());let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM documents%' AND query LIKE '%FOR UPDATE%')").bind(blocker).fetch_one(&s.world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("the actual native document footprint wait must be observed");
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(!s.gone(s.node).await);
            assert_eq!(s.audits(s.node).await, 0);
        } else {
            receipt(&checked(reply));
            assert!(s.gone(s.node).await);
        }
    }
}

#[tokio::test]
async fn deletion_keeps_its_intrinsic_committed_id_receipt_after_final_key_loss() {
    for mode in ["unchanged", "expired", "revoked"] {
        let s = Scene::new().await;
        let k = key(&s.world, s.owner.id(), s.manage()).await;
        let mut lock = s.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let node = s.node;
        let token = k.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(NodeDelete, &[&node])
                .token(&token)
                .body(&NodeDeleteBody::default());
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{assert!(!request.is_finished());let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(blocker).fetch_one(&s.world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("the committed deletion must reach actual activity publication");
        assert!(s.gone(s.node).await);
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM nodes WHERE id=$1")
            .bind(s.node)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        if mode == "expired" {
            s.world.advance(chrono::Duration::hours(2));
        } else if mode == "revoked" {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(k.id)
                .bind(s.world.now())
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        lock.rollback().await.unwrap();
        receipt(&checked(request.await.unwrap()));
        assert_eq!(s.audits(s.node).await, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT revision FROM nodes WHERE id=$1")
                .bind(s.node)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap(),
            revision
        );
    }
}

#[tokio::test]
async fn stale_keyed_deletions_keep_an_intrinsic_new_review_and_its_exact_original_key() {
    for revoke in [false, true] {
        let s = Scene::new().await;
        let preview = checked(
            s.world
                .app
                .call(NodeDeletePreview, &[&s.node])
                .token(&s.owner.token)
                .send_bytes()
                .await,
        );
        let old: Id = serde_json::from_value(preview["preflight_id"].clone()).unwrap();
        places::make_topic(&s.world, &s.owner, s.node, "A new private child").await;
        let k = key(&s.world, s.owner.id(), s.manage()).await;
        let stale = s
            .delete(
                &k.secret,
                s.node,
                &NodeDeleteBody {
                    preflight_id: Some(old),
                    expected_revision: preview["revision"].as_i64(),
                    ..NodeDeleteBody::default()
                },
            )
            .await;
        assert_eq!(stale.status, StatusCode::CONFLICT);
        let stale: Json = serde_json::from_slice(&stale.raw).unwrap();
        let current = &stale["current"];
        assert_eq!(current["projection"], "receipt");
        assert!(!stale.to_string().contains("Private"));
        assert!(!s.gone(s.node).await);
        let next: Id = serde_json::from_value(current["preflight_id"].clone()).unwrap();
        if revoke {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(k.id)
                .bind(s.world.now())
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        let confirmed = s
            .delete(
                &s.owner.token,
                s.node,
                &NodeDeleteBody {
                    preflight_id: Some(next),
                    expected_revision: current["revision"].as_i64(),
                    ..NodeDeleteBody::default()
                },
            )
            .await;
        if revoke {
            assert_eq!(confirmed.status, StatusCode::FORBIDDEN);
            assert!(!s.gone(s.node).await);
        } else {
            receipt(&checked(confirmed));
            assert!(s.gone(s.node).await);
        }
    }
}

#[tokio::test]
async fn an_original_preview_expiring_at_confirmation_sql_rolls_back_the_whole_deletion_and_plan_use()
 {
    for expire in [false, true] {
        let s = Scene::new().await;
        let original = key(
            &s.world,
            s.owner.id(),
            vec![rule("NodeDeletePreview", "manage", "project", s.node)],
        )
        .await;
        let plan = checked(
            s.world
                .app
                .call(NodeDeletePreview, &[&s.node])
                .token(&original.secret)
                .send_bytes()
                .await,
        );
        let preflight: Id = serde_json::from_value(plan["preflight_id"].clone()).unwrap();
        let mut lock = s.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE preflights IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let node = s.node;
        let token = s.owner.token.clone();
        let body = NodeDeleteBody {
            preflight_id: Some(preflight),
            expected_revision: plan["revision"].as_i64(),
            ..NodeDeleteBody::default()
        };
        let request = tokio::spawn(async move {
            let call = app.call(NodeDelete, &[&node]).token(&token).body(&body);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{
            assert!(!request.is_finished(),"the real confirmation writer must wait after its original review source check");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%DELETE FROM preflights%')")
                .bind(blocker).fetch_one(&s.world.db.pool).await.unwrap();
            if waiting{break;}tokio::task::yield_now().await;
        }}).await.expect("the original review must reach its actual one-use plan SQL wait");
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert!(!s.gone(s.node).await);
            assert_eq!(s.audits(s.node).await, 0);
            assert!(
                orch_db::queries::jobs::forks::preflight(&mut *s.world.conn().await, preflight)
                    .await
                    .unwrap()
                    .is_some(),
                "an expired original review must not be consumed"
            );
        } else {
            receipt(&checked(reply));
            assert!(s.gone(s.node).await);
        }
    }
}
