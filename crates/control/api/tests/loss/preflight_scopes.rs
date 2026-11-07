//! Actual saved loss reviews keep native custody, frozen sources and independent response Read.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::{
    identity::{self, NewApiToken},
    jobs::forks,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use tokio::task::JoinHandle;

#[path = "declare_scopes.rs"]
mod declaring;

const OP: &str = "LossPreflight";
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":[action]}]})
}
struct Key {
    id: Id,
    secret: String,
}
async fn key(world: &World, actor: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: actor,
            name: "Native loss preview fixture",
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
    people: resources::Company,
    host: Id,
    own: Id,
    other: Id,
    thread: Id,
    topic: Id,
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let people = resources::company(&world).await;
        let (host, own, other) = mixed_host(&world, &people).await;
        let project =
            places::make_project(&world, &people.boss, people.acme, "Loss name sources").await;
        let topic = places::make_topic(&world, &people.boss, project.base.id, "Bound work")
            .await
            .base
            .id;
        let thread = new_id();
        assert!(
            orch_db::queries::threads::insert_thread(
                &mut *world.conn().await,
                thread,
                topic,
                "Recorded thread name",
                people.viewer.id(),
                world.now()
            )
            .await
            .unwrap()
        );
        sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
            .bind(thread)
            .bind(own)
            .execute(&world.db.pool)
            .await
            .unwrap();
        resources::name_person(&world, "node", thread, "node.view", people.viewer.id()).await;
        Self {
            world,
            people,
            host,
            own,
            other,
            thread,
            topic,
        }
    }
    fn manage(&self) -> Vec<Json> {
        vec![rule(OP, "host", self.host, "manage")]
    }
    fn full(&self, names: bool) -> Vec<Json> {
        let mut rules = self.manage();
        rules.push(rule(OP, "host", self.host, "read"));
        rules.push(rule(OP, "environment", self.own, "read"));
        rules.push(rule(OP, "environment", self.other, "read"));
        if names {
            rules.push(rule(OP, "thread", self.thread, "read"));
        }
        rules
    }
    async fn call(&self, token: &str, target: LossTarget) -> RawReply {
        let call = self.world.app.call(LossPreflight, &[]);
        call.token(token)
            .body(&LossPreflightBody { target })
            .send_bytes()
            .await
    }
    fn start(&self, token: String) -> JoinHandle<RawReply> {
        let app = self.world.app.clone();
        let host = self.host;
        tokio::spawn(async move {
            let call = app.call(LossPreflight, &[]);
            call.token(&token)
                .body(&LossPreflightBody {
                    target: LossTarget::Host { id: host },
                })
                .send_bytes()
                .await
        })
    }
    async fn previews(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='loss'")
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }
}
async fn done(request: JoinHandle<RawReply>) -> RawReply {
    tokio::time::timeout(WAIT, request)
        .await
        .expect("native loss preview completes")
        .unwrap()
}
fn full(reply: &RawReply) -> LossPlan {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<LossPreflightResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap()
}
fn receipt(reply: &RawReply) -> LossPreflightReceipt {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<LossPreflightResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap_err()
}
async fn wait(s: &Scene, pid: i32, pattern: &str, request: &JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT, async {
        loop {
            assert!(!request.is_finished(), "the real native preview must reach its declared SQL boundary");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(&s.world.db.pool).await.unwrap();
            if blocked {break;} tokio::task::yield_now().await;
        }
    }).await.expect("actual native preview SQL wait");
}

#[tokio::test]
async fn loss_preview_binds_the_actual_target_and_preserves_manage_only_saved_review_custody() {
    let s = Scene::new().await;
    let target = LossTarget::Host { id: s.host };
    for rules in [
        vec![rule(OP, "host", s.host, "read")],
        vec![rule("LossDeclare", "host", s.host, "manage")],
        vec![rule(OP, "environment", s.own, "manage")],
    ] {
        let actor = key(&s.world, s.people.viewer.id(), rules).await;
        assert_eq!(
            s.call(&actor.secret, target).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let stranger = key(&s.world, s.people.stranger.id(), s.manage()).await;
    assert_eq!(
        s.call(&stranger.secret, target).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(s.previews().await, 0);
    let actor = key(&s.world, s.people.viewer.id(), s.manage()).await;
    let reply = s.call(&actor.secret, target).await;
    let made = receipt(&reply);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        json!({"projection":"receipt","preflight_id":made.preflight_id,"expires_at":made.expires_at})
    );
    let row = forks::preflight(&mut *s.world.conn().await, made.preflight_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.created_by, s.people.viewer.id());
    assert_eq!(row.kind, "loss");
    assert_eq!(row.plan["preview_origin"]["operation"], OP);
    assert_eq!(row.plan["plan"]["target"], json!(target));
    assert_eq!(row.expires_at, made.expires_at.get());
    assert_eq!(s.previews().await, 1);
    let audience: i64 =
        sqlx::query_scalar("SELECT count(*) FROM operation_recipient_previews WHERE id=$1")
            .bind(made.preflight_id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert_eq!(audience, 1);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Mixed host"));
    let lost: bool = sqlx::query_scalar("SELECT m.lost_at IS NOT NULL FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1")
        .bind(s.host).fetch_one(&s.world.db.pool).await.unwrap();
    assert!(
        !lost,
        "saving a native review performs no permanent-loss cascade"
    );
}

#[tokio::test]
async fn loss_preview_keeps_native_full_content_and_bounds_only_its_recorded_thread_names() {
    let s = Scene::new().await;
    let target = LossTarget::Host { id: s.host };
    let ordinary = full(&s.call(&s.people.viewer.token, target).await);
    assert!(ordinary.environments.iter().any(|environment| {
        environment
            .threads
            .iter()
            .any(|thread| thread.id == s.thread)
    }));
    let actor = key(&s.world, s.people.viewer.id(), s.full(true)).await;
    let projected = full(&s.call(&actor.secret, target).await);
    let mut expected = ordinary.clone();
    expected.preflight_id = projected.preflight_id;
    expected.notification.preview_id = projected.notification.preview_id;
    assert_eq!(projected, expected);
    let actor = key(&s.world, s.people.viewer.id(), s.full(false)).await;
    let bounded = full(&s.call(&actor.secret, target).await);
    let environment = bounded
        .environments
        .iter()
        .find(|environment| environment.environment.id == s.own)
        .unwrap();
    assert!(environment.threads.is_empty());
    assert_eq!(environment.threads_hidden, 1);
    assert_eq!(bounded.environments.len(), ordinary.environments.len());
    assert_eq!(bounded.jobs, ordinary.jobs);
    assert_eq!(bounded.kept_bytes, ordinary.kept_bytes);
    let mut missing = s.full(false);
    missing.retain(|rule| rule["target"]["id"] != s.other.to_string());
    let actor = key(&s.world, s.people.viewer.id(), missing).await;
    receipt(&s.call(&actor.secret, target).await);
}

#[tokio::test]
async fn loss_preview_checks_the_actual_key_after_root_and_preview_insert_waits_without_partial_saved_rows()
 {
    for boundary in ["root", "insert"] {
        for expired in [false, true] {
            let s = Scene::new().await;
            let actor = key(&s.world, s.people.viewer.id(), s.manage()).await;
            let mut blocker = s.world.db.pool.begin().await.unwrap();
            if boundary == "root" {
                sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR UPDATE")
                    .bind(s.host)
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
            } else {
                sqlx::query("LOCK TABLE preflights IN SHARE MODE")
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
            }
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let request = s.start(actor.secret);
            wait(
                &s,
                pid,
                if boundary == "root" {
                    "%FROM hosts WHERE id = $1 FOR UPDATE%"
                } else {
                    "%INSERT INTO preflights%"
                },
                &request,
            )
            .await;
            assert_eq!(s.previews().await, 0);
            if expired {
                s.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = done(request).await;
            if expired {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                assert_eq!(s.previews().await, 0);
                let audience: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM operation_recipient_previews")
                        .fetch_one(&s.world.db.pool)
                        .await
                        .unwrap();
                assert_eq!(audience, 0);
            } else {
                receipt(&reply);
                assert_eq!(s.previews().await, 1);
            }
        }
    }
}

#[tokio::test]
async fn loss_preview_publication_keeps_original_sources_and_returns_its_committed_receipt_on_read_loss()
 {
    for change in ["unchanged", "new_thread", "names_closed", "expired"] {
        let s = Scene::new().await;
        let actor = key(&s.world, s.people.viewer.id(), s.full(true)).await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = s.start(actor.secret);
        wait(&s, pid, "%INSERT INTO api_token_activity%", &request).await;
        let saved: (Id, String, Json) =
            sqlx::query_as("SELECT id,xmin::text,plan FROM preflights WHERE kind='loss'")
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        match change {
            "new_thread" => {
                let id = new_id();
                assert!(
                    orch_db::queries::threads::insert_thread(
                        &mut *s.world.conn().await,
                        id,
                        s.topic,
                        "New binding must not enter old review",
                        s.people.viewer.id(),
                        s.world.now()
                    )
                    .await
                    .unwrap()
                );
                sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                    .bind(id)
                    .bind(s.own)
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
                resources::name_person(&s.world, "node", id, "node.view", s.people.viewer.id())
                    .await;
            }
            "names_closed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":s.full(false)}))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => s.world.advance(chrono::Duration::hours(2)),
            _ => (),
        }
        blocker.commit().await.unwrap();
        let reply = done(request).await;
        if matches!(change, "names_closed" | "expired") {
            assert_eq!(receipt(&reply).preflight_id, saved.0);
        } else {
            let plan = full(&reply);
            assert_eq!(plan.preflight_id, saved.0);
            assert!(
                !String::from_utf8_lossy(&reply.raw)
                    .contains("New binding must not enter old review")
            );
        }
        assert_eq!(s.previews().await, 1);
        let current: (String, Json) =
            sqlx::query_as("SELECT xmin::text,plan FROM preflights WHERE id=$1")
                .bind(saved.0)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            current,
            (saved.1, saved.2),
            "publication never re-plans or re-saves a completed native review"
        );
    }
}
