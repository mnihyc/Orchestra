//! Membership ending keeps native self-leave, exact cascade subjects and its guarded first write.
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
use orch_api_contract::ownership::{
    NodeDelete, NodeDeleteBody, NodeTransfer, OrgMemberRemove, OrgMemberRemoveBody, ResourceFate,
    TransferBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::BTreeMap;
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
            name: "Native ending fixture",
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
struct Scene {
    world: World,
    owner: places::Who,
    member: places::Who,
    org: Id,
    domain: Id,
}
impl Scene {
    async fn new(role: &str) -> Self {
        let world = World::new().await;
        let owner = places::person(&world, "ending-owner").await;
        let member = places::person(&world, "ending-member").await;
        let org = places::make_org(&world, &owner, "Private ending root", "ending-root")
            .await
            .base
            .id;
        let domain = places::make_department(
            &world,
            &owner,
            org,
            "Private ending domain",
            "ending-domain",
        )
        .await
        .base
        .id;
        places::join(&world, org, domain, &member, role, &owner).await;
        Self {
            world,
            owner,
            member,
            org,
            domain,
        }
    }
    fn manage(&self, operation: &str) -> Vec<Json> {
        vec![rule(operation, "manage", "org", self.domain)]
    }
    async fn owned_project(&self, name: &str) -> Id {
        let project = places::make_project(&self.world, &self.owner, self.domain, name)
            .await
            .base
            .id;
        self.world
            .app
            .call(NodeTransfer, &[&project])
            .token(&self.owner.token)
            .body(&TransferBody {
                user_id: self.member.id(),
                keep_previous: Some(false),
            })
            .send()
            .await
            .ok();
        assert_eq!(
            sqlx::query_scalar::<_, Id>("SELECT owner_id FROM nodes WHERE id=$1")
                .bind(project)
                .fetch_one(&self.world.db.pool)
                .await
                .unwrap(),
            self.member.id()
        );
        project
    }
    async fn remove(&self, secret: &str, body: &OrgMemberRemoveBody) -> RawReply {
        self.world
            .app
            .call(OrgMemberRemove, &[&self.org, &self.member.id()])
            .token(secret)
            .body(body)
            .send_bytes()
            .await
    }
    async fn present(&self) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id=$1 AND user_id=$2)",
        )
        .bind(self.org)
        .bind(self.member.id())
        .fetch_one(&self.world.db.pool)
        .await
        .unwrap()
    }
    async fn endings(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action='member.removed' AND object_id=$1",
        )
        .bind(self.member.id())
        .fetch_one(&self.world.db.pool)
        .await
        .unwrap()
    }
}
fn empty(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(reply.raw.is_empty());
}

#[tokio::test]
async fn membership_ending_selects_the_native_variant_and_actual_domain_without_lending_authority()
{
    let scene = Scene::new("viewer").await;
    for rules in [
        scene.manage("OrgMemberRemove.leave"),
        vec![rule("OrgMemberRemove.remove", "manage", "org", scene.org)],
        vec![rule("OrgMemberRemove.remove", "write", "org", scene.domain)],
        scene.manage("OrgMemberHoldings"),
    ] {
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        assert_eq!(
            scene
                .remove(&secret.secret, &OrgMemberRemoveBody::default())
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert!(scene.present().await);
        assert_eq!(scene.endings().await, 0);
    }
    let viewer = places::person(&scene.world, "ending-viewer").await;
    places::join(
        &scene.world,
        scene.org,
        scene.org,
        &viewer,
        "viewer",
        &scene.owner,
    )
    .await;
    let secret = key(
        &scene.world,
        viewer.id(),
        scene.manage("OrgMemberRemove.remove"),
    )
    .await;
    assert_eq!(
        scene
            .remove(&secret.secret, &OrgMemberRemoveBody::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let secret = key(
        &scene.world,
        scene.owner.id(),
        scene.manage("OrgMemberRemove.remove"),
    )
    .await;
    empty(
        scene
            .remove(&secret.secret, &OrgMemberRemoveBody::default())
            .await,
    );
    assert!(!scene.present().await);
    assert_eq!(scene.endings().await, 1);
}

#[tokio::test]
async fn membership_self_leave_needs_no_new_manage_permission_and_keeps_the_last_owner_rule() {
    let scene = Scene::new("viewer").await;
    let wrong = key(
        &scene.world,
        scene.member.id(),
        scene.manage("OrgMemberRemove.remove"),
    )
    .await;
    assert_eq!(
        scene
            .remove(&wrong.secret, &OrgMemberRemoveBody::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let secret = key(
        &scene.world,
        scene.member.id(),
        scene.manage("OrgMemberRemove.leave"),
    )
    .await;
    let reply = scene
        .world
        .app
        .call(OrgMemberRemove, &[&scene.org, &"me"])
        .token(&secret.secret)
        .body(&OrgMemberRemoveBody::default())
        .send_bytes()
        .await;
    empty(reply);
    assert!(!scene.present().await);
    let secret = key(
        &scene.world,
        scene.owner.id(),
        vec![rule("OrgMemberRemove.leave", "manage", "org", scene.org)],
    )
    .await;
    let reply = scene
        .world
        .app
        .call(OrgMemberRemove, &[&scene.org, &"me"])
        .token(&secret.secret)
        .body(&OrgMemberRemoveBody::default())
        .send_bytes()
        .await;
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap()["code"],
        json!(orch_api_contract::common::ErrorCode::OwnerLast)
    );
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id=$1 AND user_id=$2)",
    )
    .bind(scene.org)
    .bind(scene.owner.id())
    .fetch_one(&scene.world.db.pool)
    .await
    .unwrap();
    assert!(present);
}

#[tokio::test]
async fn membership_ending_binds_live_and_retained_owned_pages_from_the_native_plan() {
    for retained in [false, true] {
        let scene = Scene::new("developer").await;
        let project = scene.owned_project("Member's private project").await;
        if retained {
            scene
                .world
                .app
                .call(NodeDelete, &[&project])
                .token(&scene.owner.token)
                .body(&NodeDeleteBody::default())
                .send()
                .await
                .ok();
        }
        let secret = key(
            &scene.world,
            scene.owner.id(),
            scene.manage("OrgMemberRemove.remove"),
        )
        .await;
        assert_eq!(
            scene
                .remove(&secret.secret, &OrgMemberRemoveBody::default())
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert!(scene.present().await);
        let owner: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
            .bind(project)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        assert_eq!(owner, scene.member.id());
        let mut rules = scene.manage("OrgMemberRemove.remove");
        rules.push(rule("OrgMemberRemove.remove", "manage", "project", project));
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        empty(
            scene
                .remove(&secret.secret, &OrgMemberRemoveBody::default())
                .await,
        );
        let (owner, deleted): (Id, bool) =
            sqlx::query_as("SELECT owner_id,deleted_at IS NOT NULL FROM nodes WHERE id=$1")
                .bind(project)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
        assert_eq!(owner, scene.owner.id());
        assert_eq!(
            deleted, retained,
            "the native ending transfers a kept page without restoring it"
        );
        assert!(!scene.present().await);
        assert_eq!(scene.endings().await, 1);
    }
}

#[tokio::test]
async fn membership_ending_keeps_managed_resource_choices_on_the_exact_resource() {
    let scene = Scene::new("viewer").await;
    let mut conn = scene.world.conn().await;
    let template = resources::access_fixtures::insert_resource(
        &mut conn,
        "template",
        Some(scene.member.id()),
        Some(scene.domain),
        "Private ending template",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Template,
            id: template,
        },
        scene.member.id(),
        NewAccess::Defaults,
        Time::new(scene.world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    let body = OrgMemberRemoveBody {
        resources: Some(BTreeMap::from([(
            template.to_string(),
            ResourceFate::Disable,
        )])),
    };
    let secret = key(
        &scene.world,
        scene.owner.id(),
        scene.manage("OrgMemberRemove.remove"),
    )
    .await;
    assert_eq!(
        scene.remove(&secret.secret, &body).await.status,
        StatusCode::FORBIDDEN
    );
    let mut rules = scene.manage("OrgMemberRemove.remove");
    rules.push(rule(
        "OrgMemberRemove.remove",
        "manage",
        "template",
        template,
    ));
    let secret = key(&scene.world, scene.owner.id(), rules).await;
    empty(scene.remove(&secret.secret, &body).await);
    let state: (bool, bool, Option<Id>) =
        sqlx::query_as("SELECT enabled,deleted_at IS NULL,owner_id FROM env_templates WHERE id=$1")
            .bind(template)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
    assert_eq!(state, (false, true, Some(scene.owner.id())));
    assert!(!scene.present().await);
}

#[tokio::test]
async fn membership_ending_requires_the_native_successor_org_as_an_independent_destination() {
    let scene = Scene::new("developer").await;
    let project = scene
        .owned_project("Project with a leaving domain owner")
        .await;
    scene
        .world
        .app
        .call(NodeTransfer, &[&scene.domain])
        .token(&scene.owner.token)
        .body(&TransferBody {
            user_id: scene.member.id(),
            keep_previous: Some(false),
        })
        .send()
        .await
        .ok();
    let mut rules = scene.manage("OrgMemberRemove.remove");
    rules.push(rule("OrgMemberRemove.remove", "manage", "project", project));
    let secret = key(&scene.world, scene.owner.id(), rules.clone()).await;
    assert_eq!(
        scene
            .remove(&secret.secret, &OrgMemberRemoveBody::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(scene.present().await);
    rules.push(rule("OrgMemberRemove.remove", "manage", "org", scene.org));
    let secret = key(&scene.world, scene.owner.id(), rules).await;
    empty(
        scene
            .remove(&secret.secret, &OrgMemberRemoveBody::default())
            .await,
    );
    for id in [scene.domain, project] {
        let owner: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
            .bind(id)
            .fetch_one(&scene.world.db.pool)
            .await
            .unwrap();
        assert_eq!(owner, scene.owner.id());
    }
    assert!(!scene.present().await);
}

#[tokio::test]
async fn membership_ending_rechecks_expiry_after_the_native_owned_page_wait_before_writing() {
    for expire in [false, true] {
        let scene = Scene::new("developer").await;
        let project = scene.owned_project("Locked owned project").await;
        let mut rules = scene.manage("OrgMemberRemove.remove");
        rules.push(rule("OrgMemberRemove.remove", "manage", "project", project));
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let mut lock = scene.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(project)
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let app = scene.world.app.clone();
        let org = scene.org;
        let user = scene.member.id();
        let token = secret.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(OrgMemberRemove, &[&org, &user])
                .token(&token)
                .body(&OrgMemberRemoveBody::default());
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the inner native ownership wait must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM nodes%' AND query LIKE '%FOR UPDATE%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual owned node wait must be observed");
        if expire {
            scene.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(scene.present().await);
            let owner: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
                .bind(project)
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
            assert_eq!(owner, scene.member.id());
            assert_eq!(scene.endings().await, 0);
        } else {
            empty(reply);
            assert!(!scene.present().await);
        }
    }
}

#[tokio::test]
async fn membership_ending_keeps_its_empty_success_after_committed_cleanup_and_late_key_loss() {
    for expire in [false, true] {
        let scene = Scene::new("viewer").await;
        let secret = key(
            &scene.world,
            scene.owner.id(),
            scene.manage("OrgMemberRemove.remove"),
        )
        .await;
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
        let org = scene.org;
        let user = scene.member.id();
        let token = secret.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(OrgMemberRemove, &[&org, &user])
                .token(&token)
                .body(&OrgMemberRemoveBody::default());
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the completed empty act must reach publication");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual final activity wait must be observed");
        assert!(!scene.present().await, "the native ending has committed");
        if expire {
            scene.world.advance(chrono::Duration::hours(2));
        } else {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(secret.id)
                .bind(scene.world.now())
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        }
        lock.rollback().await.unwrap();
        empty(request.await.unwrap());
        assert_eq!(scene.endings().await, 1);
    }
}
