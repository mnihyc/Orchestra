//! Fresh native membership movement keeps scope, current authority and readback independent.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::Role;
use orch_api_contract::ownership::{MemberMoveBody, OrgMemberMove, OrgMemberMovePreview};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{access, identity};
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
            name: "Native member move fixture",
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
    member: places::Who,
    org: Id,
    source: Id,
    to: Id,
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = places::person(&world, "move-owner").await;
        let member = places::person(&world, "move-member").await;
        let org = places::make_org(&world, &owner, "Private move root", "move-root")
            .await
            .base
            .id;
        let source =
            places::make_department(&world, &owner, org, "Private move source", "move-source")
                .await
                .base
                .id;
        let to = places::make_department(
            &world,
            &owner,
            org,
            "Private move destination",
            "move-destination",
        )
        .await
        .base
        .id;
        places::join(&world, org, source, &member, "viewer", &owner).await;
        Self {
            world,
            owner,
            member,
            org,
            source,
            to,
        }
    }
    fn manage(&self) -> Vec<Json> {
        vec![
            rule("OrgMemberMove", "manage", "org", self.source),
            rule("OrgMemberMove", "manage", "org", self.to),
        ]
    }
    fn read(&self) -> Vec<Json> {
        vec![
            rule("OrgMemberMove", "read", "account", self.member.id()),
            rule("OrgMemberMove", "read", "org", self.org),
            rule("OrgMemberMove", "read", "org", self.to),
        ]
    }
    async fn call(&self, secret: &str, path: Id) -> RawReply {
        self.world
            .app
            .call(OrgMemberMove, &[&path, &self.member.id()])
            .token(secret)
            .body(&MemberMoveBody {
                to: self.to,
                role: Some(Role::Viewer),
            })
            .send_bytes()
            .await
    }
    async fn domain(&self) -> Id {
        sqlx::query_scalar("SELECT domain_id FROM org_members WHERE org_id=$1 AND user_id=$2")
            .bind(self.org)
            .bind(self.member.id())
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }
    fn preview_manage(&self) -> Vec<Json> {
        vec![
            rule("OrgMemberMovePreview", "manage", "org", self.source),
            rule("OrgMemberMovePreview", "manage", "org", self.to),
        ]
    }
    fn preview_read(&self) -> Vec<Json> {
        vec![
            rule("OrgMemberMovePreview", "read", "account", self.member.id()),
            rule("OrgMemberMovePreview", "read", "org", self.org),
            rule("OrgMemberMovePreview", "read", "org", self.source),
            rule("OrgMemberMovePreview", "read", "org", self.to),
        ]
    }
    async fn preview(&self, secret: &str) -> RawReply {
        self.world
            .app
            .call(OrgMemberMovePreview, &[&self.org, &self.member.id()])
            .token(secret)
            .body(&MemberMoveBody {
                to: self.to,
                role: None,
            })
            .send_bytes()
            .await
    }
    async fn moves(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action='member.moved' AND object_id=$1",
        )
        .bind(self.org)
        .fetch_one(&self.world.db.pool)
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn member_move_preview_has_exact_scope_and_keeps_the_native_one_side_gate() {
    let scene = Scene::new().await;
    for rules in [
        vec![
            rule("OrgMemberMovePreview", "manage", "org", scene.org),
            rule("OrgMemberMovePreview", "manage", "org", scene.to),
        ],
        vec![rule("OrgMemberMovePreview", "manage", "org", scene.source)],
        scene.manage(),
    ] {
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        assert_eq!(
            scene.preview(&secret.secret).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let observer = places::person(&scene.world, "preview-observer").await;
    places::join(
        &scene.world,
        scene.org,
        scene.org,
        &observer,
        "viewer",
        &scene.owner,
    )
    .await;
    let refused = key(&scene.world, observer.id(), scene.preview_manage()).await;
    assert_eq!(
        scene.preview(&refused.secret).await.status,
        StatusCode::FORBIDDEN
    );
    let one_side = places::person(&scene.world, "preview-one-side").await;
    places::join(
        &scene.world,
        scene.org,
        scene.to,
        &one_side,
        "admin",
        &scene.owner,
    )
    .await;
    let secret = key(&scene.world, one_side.id(), scene.preview_manage()).await;
    let summary = checked(scene.preview(&secret.secret).await);
    assert_eq!(summary["projection"], "receipt");
    assert_eq!(summary["from"], json!(scene.source));
    assert_eq!(summary["to"], json!(scene.to));
    assert_eq!(summary["missing"], json!([scene.source]));
    assert!(summary.get("user").is_none());
    assert!(summary.get("role").is_none());
    assert!(!summary.to_string().contains("Private move"));
    let mut rules = scene.preview_manage();
    rules.extend(scene.preview_read());
    let full = key(&scene.world, one_side.id(), rules).await;
    let full = checked(scene.preview(&full.secret).await);
    let ordinary = checked(scene.preview(&one_side.token).await);
    assert_eq!(
        full, ordinary,
        "the same native one-side plan survives scope admission"
    );
    assert_eq!(full["missing"][0]["object"]["id"], json!(scene.source));
    assert_eq!(scene.domain().await, scene.source);
    assert_eq!(scene.moves().await, 0);
}

#[tokio::test]
async fn member_move_preview_full_plan_needs_its_own_role_content_sources() {
    let scene = Scene::new().await;
    let project = places::make_project(
        &scene.world,
        &scene.owner,
        scene.org,
        "Private role elsewhere",
    )
    .await
    .base
    .id;
    places::give_role(
        &scene.world,
        project,
        &scene.member,
        "developer",
        &scene.owner,
    )
    .await;
    for mode in ["manage", "borrowed", "missing-role", "full"] {
        let mut rules = scene.preview_manage();
        if mode == "borrowed" {
            rules.extend(scene.read());
            rules.push(rule("NodeGet", "read", "project", project));
        } else if mode != "manage" {
            rules.extend(scene.preview_read());
            if mode == "full" {
                rules.push(rule("OrgMemberMovePreview", "read", "project", project));
            }
        }
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let body = checked(scene.preview(&secret.secret).await);
        if mode == "full" {
            assert_eq!(body["stays"]["roles"][0]["node"]["id"], json!(project));
            assert_eq!(body["stays"]["roles"][0]["role"], "developer");
            assert_eq!(body, checked(scene.preview(&scene.owner.token).await));
        } else {
            assert_eq!(body["projection"], "receipt");
            assert_eq!(body["role_ids"].as_array().unwrap().len(), 1);
            assert!(!body.to_string().contains("Private role elsewhere"));
            assert!(body.get("role").is_none());
        }
    }
    assert_eq!(scene.domain().await, scene.source);
    assert_eq!(scene.moves().await, 0);
}

#[tokio::test]
async fn member_move_preview_retains_the_producing_role_association_and_read_authority() {
    for mode in [
        "unchanged",
        "role-place",
        "native-name",
        "drop-read",
        "expire",
    ] {
        let scene = Scene::new().await;
        let first = places::make_project(
            &scene.world,
            &scene.owner,
            scene.org,
            "First private role source",
        )
        .await
        .base
        .id;
        let second = places::make_project(
            &scene.world,
            &scene.owner,
            scene.org,
            "Second private role source",
        )
        .await
        .base
        .id;
        places::give_role(&scene.world, first, &scene.member, "viewer", &scene.owner).await;
        let actor = if mode == "native-name" {
            let actor = places::person(&scene.world, "preview-name-reader").await;
            places::join(
                &scene.world,
                scene.org,
                scene.to,
                &actor,
                "admin",
                &scene.owner,
            )
            .await;
            places::give_role(&scene.world, first, &actor, "viewer", &scene.owner).await;
            actor.id()
        } else {
            scene.owner.id()
        };
        let mut rules = scene.preview_manage();
        rules.extend(scene.preview_read());
        rules.extend([
            rule("OrgMemberMovePreview", "read", "project", first),
            rule("OrgMemberMovePreview", "read", "project", second),
        ]);
        let secret = key(&scene.world, actor, rules).await;
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
        let to = scene.to;
        let token = secret.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(OrgMemberMovePreview, &[&org, &user])
                .token(&token)
                .body(&MemberMoveBody { to, role: None });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the pure preview must reach publication");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the real activity INSERT must be observed");
        if mode == "role-place" {
            sqlx::query("UPDATE node_roles SET node_id=$3 WHERE node_id=$1 AND subject_kind='user' AND subject_id=$2 AND removed_at IS NULL")
                .bind(first).bind(scene.member.id()).bind(second).execute(&scene.world.db.pool).await.unwrap();
        } else if mode == "drop-read" {
            let scope: KeyScope =
                serde_json::from_value(json!({"rules":scene.preview_manage()})).unwrap();
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(secret.id)
                .bind(json!(scope))
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        } else if mode == "native-name" {
            sqlx::query("UPDATE node_roles SET removed_at=$3,revision=revision+1 WHERE node_id=$1 AND subject_kind='user' AND subject_id=$2 AND removed_at IS NULL")
                .bind(first).bind(actor).bind(scene.world.now()).execute(&scene.world.db.pool).await.unwrap();
        } else if mode == "expire" {
            scene.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if mode == "unchanged" {
            assert_eq!(
                checked(reply)["stays"]["roles"][0]["node"]["id"],
                json!(first)
            );
        } else if mode == "expire" {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
        }
        if mode == "role-place" {
            assert_eq!(
                checked(scene.preview(&secret.secret).await)["stays"]["roles"][0]["node"]["id"],
                json!(second)
            );
        } else if mode == "native-name" {
            let fresh = checked(scene.preview(&secret.secret).await);
            assert_eq!(fresh["stays"]["roles"][0]["node"]["id"], json!(first));
            assert_eq!(fresh["stays"]["roles"][0]["node"]["redacted"], true);
        }
        assert_eq!(scene.domain().await, scene.source);
        assert_eq!(scene.moves().await, 0);
    }
}
fn receipt(body: &Json, scene: &Scene) {
    assert_eq!(
        body,
        &json!({"projection":"receipt","org_id":scene.org,"user_id":scene.member.id(),"from":scene.source,"to":scene.to})
    );
    assert!(!body.to_string().contains("Private move"));
}

#[tokio::test]
async fn member_move_manage_only_keeps_the_native_effect_and_stronger_destination_role() {
    let scene = Scene::new().await;
    places::give_role(
        &scene.world,
        scene.to,
        &scene.member,
        "developer",
        &scene.owner,
    )
    .await;
    let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
    receipt(
        &checked(scene.call(&secret.secret, scene.org).await),
        &scene,
    );
    assert_eq!(scene.domain().await, scene.to);
    let member = access::member_row(
        &mut *scene.world.conn().await,
        scene.org,
        scene.member.id(),
        scene.world.now(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(member.role.as_deref(), Some("developer"));
    assert_eq!(scene.moves().await, 1);
}

#[tokio::test]
async fn member_move_requires_its_actual_source_destination_action_and_ordinary_authority() {
    let scene = Scene::new().await;
    for rules in [
        vec![
            rule("OrgMemberMove", "manage", "org", scene.org),
            rule("OrgMemberMove", "manage", "org", scene.to),
        ],
        vec![rule("OrgMemberMove", "manage", "org", scene.source)],
        vec![
            rule("OrgMemberMove", "write", "org", scene.source),
            rule("OrgMemberMove", "write", "org", scene.to),
        ],
        vec![
            rule("OrgMemberMovePreview", "manage", "org", scene.source),
            rule("OrgMemberMovePreview", "manage", "org", scene.to),
        ],
    ] {
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let reply = scene.call(&secret.secret, scene.org).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert_eq!(scene.domain().await, scene.source);
    }
    let observer = places::person(&scene.world, "move-observer").await;
    places::join(
        &scene.world,
        scene.org,
        scene.org,
        &observer,
        "viewer",
        &scene.owner,
    )
    .await;
    let secret = key(&scene.world, observer.id(), scene.manage()).await;
    assert_eq!(
        scene.call(&secret.secret, scene.org).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(scene.domain().await, scene.source);
    assert_eq!(scene.moves().await, 0);
}

#[tokio::test]
async fn member_move_full_content_needs_its_own_account_and_qualification_reads() {
    for mode in ["borrowed", "missing-root", "full"] {
        let scene = Scene::new().await;
        let mut rules = scene.manage();
        if mode == "borrowed" {
            rules.extend([
                rule("UserSearch", "read", "account", scene.member.id()),
                rule("NodeGet", "read", "org", scene.org),
                rule("NodeGet", "read", "org", scene.to),
            ]);
        } else {
            rules.extend(
                scene.read().into_iter().filter(|rule| {
                    mode != "missing-root" || rule["target"]["id"] != json!(scene.org)
                }),
            );
        }
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let body = checked(scene.call(&secret.secret, scene.org).await);
        if mode == "full" {
            assert!(body.get("projection").is_none());
            assert_eq!(body["user"]["id"], json!(scene.member.id()));
            assert_eq!(body["domain"]["id"], json!(scene.to));
            assert!(body["address"].as_str().unwrap().contains("move-root"));
        } else {
            receipt(&body, &scene);
        }
        assert_eq!(scene.domain().await, scene.to);
        assert_eq!(scene.moves().await, 1);
    }
}

#[tokio::test]
async fn member_move_rechecks_expiry_after_the_actual_membership_wait_before_effect() {
    for expire in [false, true] {
        let scene = Scene::new().await;
        let secret = key(&scene.world, scene.owner.id(), scene.manage()).await;
        let mut lock = scene.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("SELECT domain_id FROM org_members WHERE org_id=$1 AND user_id=$2 FOR UPDATE")
            .bind(scene.org)
            .bind(scene.member.id())
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let app = scene.world.app.clone();
        let org = scene.org;
        let user = scene.member.id();
        let destination = scene.to;
        let token = secret.secret.clone();
        let request = tokio::spawn(async move {
            let call =
                app.call(OrgMemberMove, &[&org, &user])
                    .token(&token)
                    .body(&MemberMoveBody {
                        to: destination,
                        role: Some(Role::Viewer),
                    });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the native membership wait was not reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM org_members%' AND query LIKE '%FOR UPDATE%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual member row must serialize the move");
        if expire {
            scene.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(scene.domain().await, scene.source);
            assert_eq!(scene.moves().await, 0);
        } else {
            receipt(&checked(reply), &scene);
            assert_eq!(scene.domain().await, scene.to);
        }
    }
}

#[tokio::test]
async fn member_move_publication_does_not_readmit_completed_manage_or_replay_after_expiry() {
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
        let org = scene.org;
        let user = scene.member.id();
        let destination = scene.to;
        let token = secret.secret.clone();
        let request = tokio::spawn(async move {
            let call =
                app.call(OrgMemberMove, &[&org, &user])
                    .token(&token)
                    .body(&MemberMoveBody {
                        to: destination,
                        role: Some(Role::Viewer),
                    });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the completed move must reach activity publication");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the final activity INSERT must be observed");
        assert_eq!(
            scene.domain().await,
            scene.to,
            "the native move has already committed"
        );
        let xmin: String =
            sqlx::query_scalar("SELECT xmin::text FROM org_members WHERE org_id=$1 AND user_id=$2")
                .bind(scene.org)
                .bind(scene.member.id())
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
        if mode == "drop-manage" || mode == "drop-read" {
            let scope: KeyScope = serde_json::from_value(
                json!({"rules": if mode == "drop-manage" { scene.read() } else { scene.manage() }}),
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
        if mode == "drop-read" || mode == "expire" {
            receipt(&body, &scene);
        } else {
            assert_eq!(body["domain"]["id"], json!(scene.to));
            assert!(body.get("projection").is_none());
        }
        assert_eq!(scene.domain().await, scene.to);
        assert_eq!(scene.moves().await, 1);
        let after: String =
            sqlx::query_scalar("SELECT xmin::text FROM org_members WHERE org_id=$1 AND user_id=$2")
                .bind(scene.org)
                .bind(scene.member.id())
                .fetch_one(&scene.world.db.pool)
                .await
                .unwrap();
        assert_eq!(after, xmin);
    }
}
