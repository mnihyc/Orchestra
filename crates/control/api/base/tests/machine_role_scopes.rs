//! Delegated roles keep their native prerequisites, exact command origin and optional readback.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{
    common::ErrorCode,
    resources::{MachineRolesSet, MachineRolesSetBody},
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
    thread_access::AuthzAccess,
};
use orch_db::queries::{
    identity::{self, NewApiToken},
    link,
    resources::{access_fixtures as fixtures, set_presence},
};
use orch_engine::commands::CommandAuthority;
use orch_link_contract::policy::LocalPolicy;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native role selection",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    key
}
fn body() -> MachineRolesSetBody {
    serde_json::from_value(json!({"roles":["agent"]})).unwrap()
}
fn receipt(id: Id) -> Json {
    json!({"projection":"receipt","machine_id":id,"requested_roles":["agent"],"reconnecting":true})
}
async fn machine(world: &World, user: Id) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_persons_machine(&mut conn, user, "Private role-selection machine")
        .await
        .unwrap();
    let policy = LocalPolicy {
        machine_roles: true,
        policy_delegated: vec!["machine.roles".into()],
        ..LocalPolicy::default()
    };
    sqlx::query("UPDATE machines SET roles=ARRAY['agent','provider'],facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1").bind(id).bind(json!(policy)).execute(&mut *conn).await.unwrap();
    set_presence(&mut conn, id, true, None, world.now())
        .await
        .unwrap();
    id
}
async fn call(world: &World, token: &str, id: Id) -> RawReply {
    let request = world
        .app
        .call(MachineRolesSet, &[&id])
        .token(token)
        .body(&body());
    request.send_bytes().await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn asked(world: &World, id: Id) -> Id {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let id: Option<Id> = sqlx::query_scalar("SELECT id FROM commands WHERE machine_id=$1 AND kind='machine.roles.set' AND state IN ('queued','sent','accepted') ORDER BY created_at,id LIMIT 1")
                .bind(id).fetch_optional(&world.db.pool).await.unwrap();
            if let Some(id) = id { break id; }
            tokio::task::yield_now().await;
        }
    }).await.expect("a permitted role selection reaches the real native queue")
}
async fn answer(world: &World, command: Id) {
    sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE id=$1")
        .bind(command)
        .bind(json!({"roles":["agent"],"reconnecting":true}))
        .execute(&world.db.pool)
        .await
        .unwrap();
}
async fn run(world: &World, token: &str, id: Id) -> Json {
    let response = call(world, token, id);
    let native = async {
        let command = asked(world, id).await;
        answer(world, command).await;
    };
    let (reply, ()) = tokio::join!(response, native);
    accepted(reply)
}
async fn counts(world: &World, id: Id) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='machine.roles.set'),(SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='machine.roles_changed')")
        .bind(id).fetch_one(&world.db.pool).await.unwrap()
}
async fn stored(world: &World, id: Id) -> (Vec<String>, Json, String) {
    sqlx::query_as("SELECT roles,facts,xmin::text FROM machines WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn waited(
    world: &World,
    pid: i32,
    request: &tokio::task::JoinHandle<RawReply>,
    pattern: &str,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the actual native wait must be reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the real native SQL lock must block this request");
}

#[tokio::test]
async fn roles_require_exact_scope_and_keep_generated_outcome_without_inventing_active_roles() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let stranger = places::person(&world, "stranger").await;
    let id = machine(&world, owner.id()).await;
    let other = machine(&world, owner.id()).await;
    let before = stored(&world, id).await;
    for rules in [
        vec![rule("MachineUpdate", "machine", id, &["manage"])],
        vec![rule("MachineRolesSet", "machine", id, &["read"])],
        vec![rule("MachineRolesSet", "environment", id, &["manage"])],
        vec![rule("MachineRolesSet", "machine", other, &["manage"])],
    ] {
        let token = key(&world, owner.id(), rules).await;
        assert_eq!(
            call(&world, &token.secret, id).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(counts(&world, id).await, (0, 0));
        assert_eq!(stored(&world, id).await, before);
    }
    let denied = key(
        &world,
        stranger.id(),
        vec![rule("MachineRolesSet", "machine", id, &["manage"])],
    )
    .await;
    assert_eq!(
        call(&world, &denied.secret, id).await.status,
        StatusCode::NOT_FOUND
    );
    let writer = key(
        &world,
        owner.id(),
        vec![
            rule("MachineRolesSet", "machine", id, &["manage"]),
            rule("MachineGet", "machine", id, &["read"]),
        ],
    )
    .await;
    assert_eq!(run(&world, &writer.secret, id).await, receipt(id));
    let claimed = stored(&world, id).await;
    assert_eq!(
        claimed.0, before.0,
        "only the next hello changes active roles"
    );
    assert_eq!(
        claimed.1, before.1,
        "the revision claim does not change facts"
    );
    assert_ne!(
        claimed.2, before.2,
        "the accepted replacement claims its revision"
    );
    assert_eq!(counts(&world, id).await, (1, 1));
    let reader = key(
        &world,
        owner.id(),
        vec![rule("MachineRolesSet", "machine", id, &["manage", "read"])],
    )
    .await;
    let full = run(&world, &reader.secret, id).await;
    assert_eq!(full["machine_view"]["id"], json!(id));
    assert_eq!(full["machine_view"]["roles"], json!(["agent", "provider"]));
    assert_eq!(full["requested_roles"], json!(["agent"]));
    assert_eq!(full["reconnecting"], true);
    let ordinary = run(&world, &owner.token, other).await;
    assert_eq!(ordinary["machine_view"]["id"], json!(other));
    assert_eq!(ordinary["requested_roles"], json!(["agent"]));
}

#[tokio::test]
async fn scoped_roles_keep_local_delegation_and_the_actual_registered_resource_safeguard() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let id = machine(&world, owner.id()).await;
    let token = key(
        &world,
        owner.id(),
        vec![rule("MachineRolesSet", "machine", id, &["manage"])],
    )
    .await;
    let policy = LocalPolicy::default();
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(id)
        .bind(json!(policy))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let reply = call(&world, &token.secret, id).await;
    assert_eq!(reply.status.as_u16(), ErrorCode::RequestInvalid.status());
    assert_eq!(counts(&world, id).await, (0, 0));
    let policy = LocalPolicy {
        machine_roles: true,
        policy_delegated: vec!["machine.roles".into()],
        ..LocalPolicy::default()
    };
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(id)
        .bind(json!(policy))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut conn = world.conn().await;
    let provider = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(owner.id()),
        Some(owner.person.personal_org),
        "Registered native provider",
    )
    .await
    .unwrap();
    fixtures::register_on(&mut conn, "provider", provider, id)
        .await
        .unwrap();
    drop(conn);
    let reply = call(&world, &token.secret, id).await;
    assert_eq!(reply.status.as_u16(), ErrorCode::RequestInvalid.status());
    assert_eq!(counts(&world, id).await, (0, 0));
}

#[tokio::test]
async fn private_role_origin_keeps_typed_intent_and_rechecks_native_delegation_at_delivery() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let id = machine(&world, owner.id()).await;
    let token = key(
        &world,
        owner.id(),
        vec![rule("MachineRolesSet", "machine", id, &["manage"])],
    )
    .await;
    run(&world, &token.secret, id).await;
    let command: Id = sqlx::query_scalar(
        "SELECT id FROM commands WHERE machine_id=$1 AND kind='machine.roles.set'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    let kept = link::command_with_authority(&mut *world.conn().await, command)
        .await
        .unwrap()
        .unwrap();
    let private = kept.authority.unwrap();
    assert_eq!(private["kind"], "machine_roles");
    assert_eq!(
        private["authority"]["origin"]["operation"],
        "MachineRolesSet"
    );
    assert_eq!(private["authority"]["credential"]["kind"], "token");
    let authority: CommandAuthority = serde_json::from_value(private.clone()).unwrap();
    let access = AuthzAccess::new(world.app.app.clock.clone());
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &kept.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_some()
    );
    tx.commit().await.unwrap();
    let mut forged = private;
    forged["authority"]["roles_request"]["roles"] = json!(["provider"]);
    let forged: CommandAuthority = serde_json::from_value(forged).unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        forged
            .current_callers(
                &mut tx,
                &kept.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(id)
        .bind(json!(LocalPolicy::default()))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &kept.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn role_selection_rechecks_expiry_after_its_actual_command_insert_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let token = key(
            &world,
            owner.id(),
            vec![rule("MachineRolesSet", "machine", id, &["manage"])],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE commands IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(MachineRolesSet, &[&id])
                .token(&token.secret)
                .body(&body());
            request.send_bytes().await
        });
        waited(&world, pid, &request, "%INSERT INTO commands%").await;
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        if !expire {
            let command = asked(&world, id).await;
            answer(&world, command).await;
        }
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert_eq!(counts(&world, id).await, (0, 0));
        } else {
            assert_eq!(accepted(reply), receipt(id));
            assert_eq!(counts(&world, id).await, (1, 1));
        }
    }
}

#[tokio::test]
async fn role_selection_rechecks_the_original_key_after_its_native_answer_and_physical_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let token = key(
            &world,
            owner.id(),
            vec![rule("MachineRolesSet", "machine", id, &["manage"])],
        )
        .await;
        let app = world.app.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(MachineRolesSet, &[&id])
                .token(&token.secret)
                .body(&body());
            request.send_bytes().await
        });
        let command = asked(&world, id).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        answer(&world, command).await;
        waited(&world, pid, &request, "%FROM machines%FOR %").await;
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(counts(&world, id).await, (1, 0));
        } else {
            assert_eq!(accepted(reply), receipt(id));
            assert_eq!(counts(&world, id).await, (1, 1));
        }
    }
}

#[tokio::test]
async fn role_response_read_loss_after_commit_keeps_the_native_selection_and_one_audit() {
    for close in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let token = key(
            &world,
            owner.id(),
            vec![rule("MachineRolesSet", "machine", id, &["manage", "read"])],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let secret = token.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(MachineRolesSet, &[&id])
                .token(&secret)
                .body(&body());
            request.send_bytes().await
        });
        let command = asked(&world, id).await;
        answer(&world, command).await;
        waited(&world, pid, &request, "%INSERT INTO api_token_activity%").await;
        let before = stored(&world, id).await;
        assert_eq!(counts(&world, id).await, (1, 1));
        if close {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(json!({"rules":[rule("MachineRolesSet","machine",id,&["manage"])]}))
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let result = accepted(
            tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap(),
        );
        if close {
            assert_eq!(result, receipt(id));
        } else {
            assert_eq!(result["machine_view"]["id"], json!(id));
        }
        assert_eq!(counts(&world, id).await, (1, 1));
        assert_eq!(stored(&world, id).await, before);
    }
}
