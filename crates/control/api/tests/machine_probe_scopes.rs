//! Facts probes retain their exact physical source and raw credential through the command wait.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::resources::MachineProbe;
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
            name: "Native facts probe",
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
fn facts() -> Json {
    json!({"hostname":"Fresh native probe","os_version":"synthetic","cpus":2,"memory_bytes":1024,
        "harnesses":[],"adapters":[],"plugins":[],"policy":null,"rates":{"up_bps":0,"down_bps":0,"round_trip_ms":1},"clock_offset_ms":0})
}
async fn machine(world: &World, user: Id) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_persons_machine(&mut conn, user, "Private probe machine")
        .await
        .unwrap();
    set_presence(&mut conn, id, true, None, world.now())
        .await
        .unwrap();
    id
}
async fn call(world: &World, secret: &str, id: Id) -> RawReply {
    let request = world.app.call(MachineProbe, &[&id]).token(secret);
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
fn receipt(id: Id) -> Json {
    json!({"projection":"receipt","machine_id":id})
}
async fn asked(world: &World, machine: Id) -> Id {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let id: Option<Id> = sqlx::query_scalar("SELECT id FROM commands WHERE machine_id=$1 AND kind='machine.probe' AND state IN ('queued','sent','accepted') ORDER BY created_at,id LIMIT 1")
                .bind(machine).fetch_optional(&world.db.pool).await.unwrap();
            if let Some(id) = id { break id; }
            tokio::task::yield_now().await;
        }
    }).await.expect("a permitted request reaches its real native command queue")
}
async fn answer(world: &World, id: Id) {
    sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE id=$1")
        .bind(id)
        .bind(facts())
        .execute(&world.db.pool)
        .await
        .unwrap();
}
async fn run(world: &World, token: &str, id: Id) -> Json {
    let response = call(world, token, id);
    let machine = async {
        let command = asked(world, id).await;
        answer(world, command).await;
    };
    let (response, ()) = tokio::join!(response, machine);
    accepted(response)
}
async fn stored(world: &World, id: Id) -> (Json, String) {
    sqlx::query_as("SELECT facts,xmin::text FROM machines WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn count(world: &World, id: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='machine.probe'")
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
    }).await.expect("the actual native SQL lock must block this request");
}

#[tokio::test]
async fn probe_requires_its_exact_scope_and_keeps_manage_only_effects_and_ordinary_content() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let id = machine(&world, owner.id()).await;
    let elsewhere = machine(&world, owner.id()).await;
    let before = stored(&world, id).await;
    for rules in [
        vec![rule("MachineRevoke", "machine", id, &["manage"])],
        vec![rule("MachineProbe", "machine", id, &["read"])],
        vec![rule("MachineProbe", "environment", id, &["manage"])],
        vec![rule("MachineProbe", "machine", elsewhere, &["manage"])],
    ] {
        let token = key(&world, owner.id(), rules).await;
        assert_eq!(
            call(&world, &token.secret, id).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(count(&world, id).await, 0);
        assert_eq!(stored(&world, id).await, before);
    }
    let stranger = key(
        &world,
        other.id(),
        vec![rule("MachineProbe", "machine", id, &["manage"])],
    )
    .await;
    assert_eq!(
        call(&world, &stranger.secret, id).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(count(&world, id).await, 0);
    let writer = key(
        &world,
        owner.id(),
        vec![
            rule("MachineProbe", "machine", id, &["manage"]),
            rule("MachineGet", "machine", id, &["read"]),
        ],
    )
    .await;
    assert_eq!(run(&world, &writer.secret, id).await, receipt(id));
    assert_eq!(stored(&world, id).await.0["hostname"], "Fresh native probe");
    assert_eq!(count(&world, id).await, 1);
    let reader = key(
        &world,
        owner.id(),
        vec![rule("MachineProbe", "machine", id, &["manage", "read"])],
    )
    .await;
    let full = run(&world, &reader.secret, id).await;
    assert_eq!(full["id"], json!(id));
    assert_eq!(full["facts"]["hostname"], "Fresh native probe");
    let ordinary = run(&world, &owner.token, elsewhere).await;
    assert_eq!(ordinary["id"], json!(elsewhere));
    assert_eq!(ordinary["facts"]["hostname"], "Fresh native probe");
}

#[tokio::test]
async fn the_private_probe_command_keeps_its_exact_original_operation_and_canonical_body() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let id = machine(&world, owner.id()).await;
    let token = key(
        &world,
        owner.id(),
        vec![rule("MachineProbe", "machine", id, &["manage"])],
    )
    .await;
    run(&world, &token.secret, id).await;
    let command: Id =
        sqlx::query_scalar("SELECT id FROM commands WHERE machine_id=$1 AND kind='machine.probe'")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let kept = link::command_with_authority(&mut *world.conn().await, command)
        .await
        .unwrap()
        .unwrap();
    let private = kept.authority.unwrap();
    assert_eq!(private["kind"], "machine_probe");
    assert_eq!(private["authority"]["origin"]["operation"], "MachineProbe");
    assert_eq!(private["authority"]["credential"]["kind"], "token");
    let authority: CommandAuthority = serde_json::from_value(private).unwrap();
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
    let mut forged = kept.command;
    forged.body = json!({"another_machine":id});
    assert!(
        authority
            .current_callers(&mut tx, &forged, world.app.app.clock.as_ref(), &access)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token.id)
        .bind(json!({"rules":[rule("MachineRevoke", "machine", id, &["manage"])]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let current = link::command(&mut *world.conn().await, command)
        .await
        .unwrap()
        .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(&mut tx, &current, world.app.app.clock.as_ref(), &access)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn probe_rechecks_expiry_after_its_actual_first_command_insert_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let token = key(
            &world,
            owner.id(),
            vec![rule("MachineProbe", "machine", id, &["manage"])],
        )
        .await;
        let before = stored(&world, id).await;
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
            let request = app.call(MachineProbe, &[&id]).token(&token.secret);
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
            assert_eq!(count(&world, id).await, 0);
            assert_eq!(stored(&world, id).await, before);
        } else {
            assert_eq!(accepted(reply), receipt(id));
            assert_eq!(count(&world, id).await, 1);
        }
    }
}

#[tokio::test]
async fn probe_rechecks_before_retaining_an_answer_after_the_physical_row_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let token = key(
            &world,
            owner.id(),
            vec![rule("MachineProbe", "machine", id, &["manage"])],
        )
        .await;
        let app = world.app.clone();
        let request = tokio::spawn(async move {
            let request = app.call(MachineProbe, &[&id]).token(&token.secret);
            request.send_bytes().await
        });
        let command = asked(&world, id).await;
        let before = stored(&world, id).await;
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
            assert_eq!(stored(&world, id).await, before);
        } else {
            assert_eq!(accepted(reply), receipt(id));
            assert_eq!(stored(&world, id).await.0["hostname"], "Fresh native probe");
        }
        assert_eq!(
            count(&world, id).await,
            1,
            "the command runs once even when its result may not be retained"
        );
        assert_eq!(
            link::command(&mut *world.conn().await, command)
                .await
                .unwrap()
                .unwrap()
                .state,
            "succeeded"
        );
    }
}

#[tokio::test]
async fn probe_losing_response_read_after_commit_returns_its_receipt_without_a_second_command() {
    for close_read in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let token = key(
            &world,
            owner.id(),
            vec![rule("MachineProbe", "machine", id, &["manage", "read"])],
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
            let request = app.call(MachineProbe, &[&id]).token(&secret);
            request.send_bytes().await
        });
        let command = asked(&world, id).await;
        answer(&world, command).await;
        waited(&world, pid, &request, "%INSERT INTO api_token_activity%").await;
        let done = stored(&world, id).await;
        assert_eq!(done.0["hostname"], "Fresh native probe");
        if close_read {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(json!({"rules":[rule("MachineProbe", "machine", id, &["manage"])]}))
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let body = accepted(
            tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap(),
        );
        if close_read {
            assert_eq!(body, receipt(id));
        } else {
            assert_eq!(body["facts"]["hostname"], "Fresh native probe");
        }
        assert_eq!(stored(&world, id).await, done);
        assert_eq!(count(&world, id).await, 1);
    }
}
