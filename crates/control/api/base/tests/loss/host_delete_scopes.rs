//! Host cascade observations keep native selection and same-operation disclosure independent.

use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::{identity, jobs::forks};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::time::Duration;

const PREFLIGHT: &str = "HostDeletePreflight";
const WAIT: Duration = Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(world: &World, actor: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Native Host cascade scope control",
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
fn wanted() -> HostDeletePreflightBody {
    HostDeletePreflightBody {
        environments: HostEnvironmentCascade::Remove,
        erase: Some(false),
        ..Default::default()
    }
}
async fn call(world: &World, host: Id, token: &str) -> RawReply {
    let call = world.app.call(HostDeletePreflight, &[&host]);
    call.token(token).body(&wanted()).send_bytes().await
}
fn success(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn prepared(world: &World) -> Vec<Id> {
    sqlx::query_scalar("SELECT id FROM preflights WHERE kind='host_delete' ORDER BY created_at,id")
        .fetch_all(&world.db.pool)
        .await
        .unwrap()
}
async fn scene() -> (World, resources::Company, Id, Id, Id) {
    let world = World::new().await;
    let people = resources::company(&world).await;
    let (host, own, other) = mixed_host(&world, &people).await;
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(host)
    .execute(&world.db.pool)
    .await
    .unwrap();
    (world, people, host, own, other)
}

#[tokio::test]
async fn host_delete_preview_requires_its_actual_selected_cascade_and_keeps_manage_only_receipts() {
    let (world, people, host, own, other) = scene().await;
    let missing = key(
        &world,
        people.viewer.id(),
        vec![rule(PREFLIGHT, "host", host, &["manage"])],
    )
    .await;
    assert_eq!(
        call(&world, host, &missing.secret).await.status.as_u16(),
        403
    );
    assert!(prepared(&world).await.is_empty());
    let actor = key(
        &world,
        people.viewer.id(),
        vec![
            rule(PREFLIGHT, "host", host, &["manage"]),
            rule(PREFLIGHT, "environment", own, &["manage"]),
            rule("HostGet", "host", host, &["read"]),
        ],
    )
    .await;
    let value = success(&call(&world, host, &actor.secret).await);
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["host_id"], json!(host));
    assert!(
        value.get("name").is_none() && value.get("rows").is_none() && value.get("data").is_none()
    );
    let id: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
    let row = forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.created_by, people.viewer.id());
    assert_eq!(row.plan["environments"].as_array().unwrap().len(), 1);
    assert_eq!(row.plan["environments"][0]["id"], json!(own));
    assert!(
        !row.plan.to_string().contains(&actor.id.to_string()),
        "no requesting key is retained in this observation"
    );
    let state: String = sqlx::query_scalar("SELECT status FROM environments WHERE id=$1")
        .bind(other)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(state, "offline");
    let changed: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='host_delete'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(changed, 0, "a preview accepts no cascade execution");
}

#[tokio::test]
async fn host_delete_preview_full_content_matches_the_native_observation_and_preserves_other_managers()
 {
    let (world, people, host, own, _) = scene().await;
    let ordinary: HostDeletePlan =
        serde_json::from_value(success(&call(&world, host, &people.viewer.token).await)).unwrap();
    let actor = key(
        &world,
        people.viewer.id(),
        vec![
            rule(PREFLIGHT, "host", host, &["manage", "read"]),
            rule(PREFLIGHT, "environment", own, &["manage", "read"]),
            rule(PREFLIGHT, "org", people.acme, &["read"]),
        ],
    )
    .await;
    let full: HostDeletePlan =
        serde_json::from_value(success(&call(&world, host, &actor.secret).await)).unwrap();
    assert_eq!(full.name, ordinary.name);
    assert_eq!(full.rows, ordinary.rows);
    assert_eq!(full.others, ordinary.others);
    assert_eq!(full.recovery, ordinary.recovery);
    assert_eq!(full.executable, ordinary.executable);
    assert_eq!(
        full.others, 1,
        "another manager's dependency stays a count under its native authority"
    );
    let wrong = key(
        &world,
        people.viewer.id(),
        vec![rule(PREFLIGHT, "host", new_id(), &["manage", "read"])],
    )
    .await;
    let before = prepared(&world).await.len();
    assert_eq!(call(&world, host, &wrong.secret).await.status.as_u16(), 403);
    assert_eq!(prepared(&world).await.len(), before);
}

#[tokio::test]
async fn host_delete_preview_rechecks_the_actual_request_after_a_native_host_wait() {
    let (world, people, host, own, _) = scene().await;
    let actor = key(
        &world,
        people.viewer.id(),
        vec![
            rule(PREFLIGHT, "host", host, &["manage"]),
            rule(PREFLIGHT, "environment", own, &["manage"]),
        ],
    )
    .await;
    let mut blocker = world.app.app.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR UPDATE")
        .bind(host)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let secret = actor.secret;
    let pending = tokio::spawn(async move {
        let call = app.call(HostDeletePreflight, &[&host]);
        call.token(&secret).body(&wanted()).send_bytes().await
    });
    tokio::time::timeout(WAIT,async {loop {
        assert!(!pending.is_finished(),"the request must reach its real native host wait");
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
            .bind(pid).fetch_one(&world.db.pool).await.unwrap();
        if waiting{break;}tokio::task::yield_now().await;
    }}).await.unwrap();
    world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
    assert!(
        matches!(reply.status.as_u16(), 401 | 403),
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(prepared(&world).await.is_empty());
}

#[tokio::test]
async fn host_delete_preview_postcommit_read_loss_keeps_its_exact_review_receipt() {
    let (world, people, host, own, _) = scene().await;
    let actor = key(
        &world,
        people.viewer.id(),
        vec![
            rule(PREFLIGHT, "host", host, &["manage", "read"]),
            rule(PREFLIGHT, "environment", own, &["manage", "read"]),
            rule(PREFLIGHT, "org", people.acme, &["read"]),
        ],
    )
    .await;
    let mut blocker = world.app.app.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let secret = actor.secret;
    let pending = tokio::spawn(async move {
        let call = app.call(HostDeletePreflight, &[&host]);
        call.token(&secret).body(&wanted()).send_bytes().await
    });
    tokio::time::timeout(WAIT,async {loop {
        assert!(!pending.is_finished(),"the review must reach its real postcommit publication wait");
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
            .bind(pid).fetch_one(&world.db.pool).await.unwrap();
        if waiting{break;}tokio::task::yield_now().await;
    }}).await.unwrap();
    let ids = prepared(&world).await;
    assert_eq!(ids.len(), 1);
    let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM preflights WHERE id=$1")
        .bind(ids[0])
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(actor.id)
        .bind(json!({"rules":[rule(PREFLIGHT,"host",host,&["manage"]),rule(PREFLIGHT,"environment",own,&["manage"])]})).execute(&world.db.pool).await.unwrap();
    blocker.commit().await.unwrap();
    let value = success(&tokio::time::timeout(WAIT, pending).await.unwrap().unwrap());
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["preflight_id"], json!(ids[0]));
    assert!(value.get("name").is_none());
    let after: String = sqlx::query_scalar("SELECT xmin::text FROM preflights WHERE id=$1")
        .bind(ids[0])
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        after, xmin,
        "publication never rebuilds or re-saves the accepted review"
    );
}

const CREATE: &str = "HostDeleteCreate";
async fn deletion(world: &World, host: Id, token: &str, body: &HostDeleteCreateBody) -> RawReply {
    let call = world.app.call(HostDeleteCreate, &[&host]);
    call.token(token).body(body).send_bytes().await
}
async fn reviewed_deletion(world: &World, host: Id, token: &str) -> HostDeleteCreateBody {
    let plan: HostDeletePlan =
        serde_json::from_value(success(&call(world, host, token).await)).unwrap();
    HostDeleteCreateBody {
        preflight_id: plan.preflight_id,
        confirm: plan.name,
        notify: false,
        volumes: vec![],
        data_confirmations: vec![],
    }
}
async fn accepted_deletions(world: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='host_delete'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
#[tokio::test]
async fn host_delete_confirmation_binds_the_whole_review_and_keeps_a_generated_receipt() {
    let (world, people, host, own, other) = scene().await;
    let body = reviewed_deletion(&world, host, &people.viewer.token).await;
    let missing = key(
        &world,
        people.viewer.id(),
        vec![rule(CREATE, "host", host, &["manage"])],
    )
    .await;
    assert_eq!(
        deletion(&world, host, &missing.secret, &body)
            .await
            .status
            .as_u16(),
        403
    );
    assert_eq!(accepted_deletions(&world).await, 0);
    assert!(
        forks::preflight(&mut *world.conn().await, body.preflight_id)
            .await
            .unwrap()
            .is_some()
    );
    let actor = key(
        &world,
        people.viewer.id(),
        vec![
            rule(CREATE, "host", host, &["manage"]),
            rule(CREATE, "environment", own, &["manage"]),
            rule("HostGet", "host", host, &["read"]),
        ],
    )
    .await;
    let reply = deletion(&world, host, &actor.secret, &body).await;
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["host_id"], json!(host));
    assert!(value.get("progress").is_none() && value.get("created_by").is_none());
    let id: Id = serde_json::from_value(value["job_id"].clone()).unwrap();
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "host_delete");
    assert_eq!(row.created_by, Some(people.viewer.id()));
    assert_eq!(row.plan["identity"]["token_id"], json!(actor.id));
    assert!(row.plan["identity"].get("session_id").is_none());
    let principal = orch_engine::jobs::host_delete::authority::principal(
        &mut *world.conn().await,
        &row,
        world.app.app.clock.as_ref(),
    )
    .await
    .expect("the actual accepted key restores its own native parent authority");
    assert_eq!(principal.user(), people.viewer.id());
    let mut malformed = row.clone();
    malformed.plan[orch_engine::jobs::host_delete::authority::SOURCE] = Json::Null;
    assert!(
        orch_engine::jobs::host_delete::authority::principal(
            &mut *world.conn().await,
            &malformed,
            world.app.app.clock.as_ref()
        )
        .await
        .is_err(),
        "a known source never becomes a browser session"
    );
    assert_eq!(accepted_deletions(&world).await, 1);
    assert!(
        forks::preflight(&mut *world.conn().await, body.preflight_id)
            .await
            .unwrap()
            .is_none()
    );
    let untouched:bool=sqlx::query_scalar("SELECT deleted_at IS NULL AND remove_at IS NULL AND removal_phase IS NULL FROM environments WHERE id=$1").bind(other).fetch_one(&world.db.pool).await.unwrap();
    assert!(
        untouched,
        "the native other-manager boundary remains intact"
    );
    assert_ne!(
        deletion(&world, host, &actor.secret, &body)
            .await
            .status
            .as_u16(),
        202
    );
    assert_eq!(
        accepted_deletions(&world).await,
        1,
        "the same consumed review never queues twice"
    );
}
#[tokio::test]
async fn host_delete_confirmation_keeps_exact_consent_and_current_review_ownership() {
    let (world, people, host, own, _) = scene().await;
    let mut body = reviewed_deletion(&world, host, &people.viewer.token).await;
    let actor = key(
        &world,
        people.viewer.id(),
        vec![
            rule(CREATE, "host", host, &["manage"]),
            rule(CREATE, "environment", own, &["manage"]),
        ],
    )
    .await;
    body.confirm.push('!');
    assert_eq!(
        deletion(&world, host, &actor.secret, &body)
            .await
            .status
            .as_u16(),
        400
    );
    body.confirm.pop();
    sqlx::query("UPDATE preflights SET created_by=$2 WHERE id=$1")
        .bind(body.preflight_id)
        .bind(people.boss.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        deletion(&world, host, &actor.secret, &body)
            .await
            .status
            .as_u16(),
        404
    );
    assert_eq!(accepted_deletions(&world).await, 0);
    let schedulable: bool = sqlx::query_scalar("SELECT schedulable FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(
        schedulable,
        "a refused confirmation never pauses scheduling"
    );
}
#[tokio::test]
async fn host_delete_confirmation_rechecks_expiry_after_the_real_host_lock() {
    let (world, people, host, own, _) = scene().await;
    let body = reviewed_deletion(&world, host, &people.viewer.token).await;
    let actor = key(
        &world,
        people.viewer.id(),
        vec![
            rule(CREATE, "host", host, &["manage"]),
            rule(CREATE, "environment", own, &["manage"]),
        ],
    )
    .await;
    let mut blocker = world.app.app.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR UPDATE")
        .bind(host)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let secret = actor.secret;
    let pending = tokio::spawn(async move {
        let call = app.call(HostDeleteCreate, &[&host]);
        call.token(&secret).body(&body).send_bytes().await
    });
    tokio::time::timeout(WAIT,async{loop{assert!(!pending.is_finished());let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&world.db.pool).await.unwrap();if waiting{break;}tokio::task::yield_now().await;}}).await.unwrap();
    world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
    assert!(
        matches!(reply.status.as_u16(), 401 | 403 | 404),
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(accepted_deletions(&world).await, 0);
    assert_eq!(
        prepared(&world).await.len(),
        1,
        "failed acceptance preserves the review"
    );
}
