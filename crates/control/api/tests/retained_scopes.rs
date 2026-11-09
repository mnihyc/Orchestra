//! Retained reservation effects and full job disclosure use distinct exact native scopes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ObjectKind};
use orch_api_contract::environment::*;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{
    environments::retained,
    identity::{self, NewApiToken},
    resources::access_fixtures,
};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;
use tokio::task::JoinHandle;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
fn operation(purge: bool) -> &'static str {
    if purge {
        "EnvironmentPurge"
    } else {
        "EnvironmentRestore"
    }
}
fn rule(op: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":"environment","id":id,"reach":"self"},"resources":["environment"],"allow":[{"operation":op,"actions":actions}]})
}
struct Key {
    id: Id,
    secret: String,
}
struct Scene {
    world: World,
    people: resources::Company,
    environment: Id,
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let people = resources::company(&world).await;
        let mut conn = world.conn().await;
        let environment = access_fixtures::insert_environment(
            &mut conn,
            people.viewer.id(),
            Some(people.acme),
            "Kept environment",
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
            people.viewer.id(),
            NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE environments SET deleted_at=$2,deleted_by=$3,status='removed',removal_phase='kept',removal_revision=7,removed_keep_days=14,volumes_purge_at=$4,retained_volumes='[]' WHERE id=$1")
            .bind(environment).bind(world.now()).bind(people.viewer.id()).bind(world.now()+chrono::Duration::days(14)).execute(&mut *conn).await.unwrap();
        drop(conn);
        Self {
            world,
            people,
            environment,
        }
    }
    fn actor(&self, purge: bool) -> &places::Who {
        if purge {
            &self.people.admin
        } else {
            &self.people.viewer
        }
    }
    async fn key(&self, purge: bool, rules: Vec<Json>) -> Key {
        self.key_for(self.actor(purge).id(), rules).await
    }
    async fn key_for(&self, user: Id, rules: Vec<Json>) -> Key {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        let id = new_id();
        let secret = new_token(API_TOKEN_PREFIX);
        identity::create_api_token_scoped(
            &mut *self.world.conn().await,
            &NewApiToken {
                id,
                user_id: user,
                name: "Retained operation fixture",
                token_hash: &token_hash(&secret),
                scopes: &[],
                node_id: None,
                at: self.world.now(),
                expires_at: Some(self.world.now() + chrono::Duration::hours(1)),
            },
            Some(&json!(scope)),
        )
        .await
        .unwrap();
        Key { id, secret }
    }
    async fn call(&self, purge: bool, token: &str) -> RawReply {
        if purge {
            let call = self.world.app.call(EnvironmentPurge, &[&self.environment]);
            call.token(token)
                .query(&EnvironmentPurgeQuery {
                    expected_revision: 7,
                    confirm: "Kept environment".into(),
                })
                .send_bytes()
                .await
        } else {
            let call = self
                .world
                .app
                .call(EnvironmentRestore, &[&self.environment]);
            call.token(token)
                .body(&EnvironmentRestoreBody {
                    expected_revision: 7,
                })
                .send_bytes()
                .await
        }
    }
    fn start(&self, purge: bool, token: String) -> JoinHandle<RawReply> {
        let app = self.world.app.clone();
        let id = self.environment;
        tokio::spawn(async move {
            if purge {
                let call = app.call(EnvironmentPurge, &[&id]);
                call.token(&token)
                    .query(&EnvironmentPurgeQuery {
                        expected_revision: 7,
                        confirm: "Kept environment".into(),
                    })
                    .send_bytes()
                    .await
            } else {
                let call = app.call(EnvironmentRestore, &[&id]);
                call.token(&token)
                    .body(&EnvironmentRestoreBody {
                        expected_revision: 7,
                    })
                    .send_bytes()
                    .await
            }
        })
    }
    async fn count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1")
            .bind(self.environment)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }
    async fn phase(&self) -> String {
        retained::removal(&mut *self.world.conn().await, self.environment)
            .await
            .unwrap()
            .unwrap()
            .removal_phase
            .unwrap()
    }
}
fn receipt(reply: &RawReply) -> RetainedChangeReceipt {
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<RetainedChangeResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap_err()
}
fn full(reply: &RawReply) -> orch_api_contract::workspace::Job {
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<RetainedChangeResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap()
}
async fn wait(s: &Scene, pid: i32, pattern: &str, request: &JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT,async {
        loop {
            assert!(!request.is_finished(),"the real native reservation reaches its SQL boundary");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(&s.world.db.pool).await.unwrap();
            if blocked {break;}tokio::task::yield_now().await;
        }
    }).await.expect("actual retained reservation SQL wait");
}

#[tokio::test]
async fn exact_retained_manage_admits_both_real_reservations_and_never_lends_native_authority() {
    for purge in [false, true] {
        let s = Scene::new().await;
        for rules in [
            vec![rule(operation(purge), s.environment, &["read"])],
            vec![rule(operation(!purge), s.environment, &["manage"])],
            vec![rule(operation(purge), new_id(), &["manage"])],
        ] {
            let key = s.key(purge, rules).await;
            assert_eq!(
                s.call(purge, &key.secret).await.status,
                StatusCode::FORBIDDEN
            );
            assert_eq!(s.count().await, 0);
            assert_eq!(s.phase().await, "kept");
        }
        let stranger = s
            .key_for(
                s.people.stranger.id(),
                vec![rule(operation(purge), s.environment, &["manage"])],
            )
            .await;
        assert_eq!(
            s.call(purge, &stranger.secret).await.status,
            StatusCode::FORBIDDEN
        );
        let key = s
            .key(
                purge,
                vec![rule(operation(purge), s.environment, &["manage"])],
            )
            .await;
        let reply = s.call(purge, &key.secret).await;
        let made = receipt(&reply);
        assert_eq!(
            serde_json::from_slice::<Json>(&reply.raw).unwrap(),
            json!({"projection":"receipt","environment_id":s.environment,"job_id":made.job_id})
        );
        assert_eq!(s.count().await, 1);
        assert_eq!(s.phase().await, if purge { "purging" } else { "restoring" });
        let plan: Json = sqlx::query_scalar("SELECT plan FROM jobs WHERE id=$1")
            .bind(made.job_id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            plan["retained_operation"]["original"]["operation"],
            operation(purge)
        );
        assert_eq!(
            plan["retained_operation"]["environment"],
            json!(s.environment)
        );
        assert_eq!(plan["retained_operation"]["revision"], 7);
        assert_eq!(plan["identity"]["token_id"], json!(key.id));
        assert_eq!(
            receipt(&s.call(purge, &key.secret).await).job_id,
            made.job_id,
            "joining the pending native job is unchanged"
        );
        assert_eq!(s.count().await, 1);
    }
}

#[tokio::test]
async fn retained_full_job_answers_keep_ordinary_content_and_require_their_own_read() {
    for purge in [false, true] {
        let s = Scene::new().await;
        let writer = s
            .key(
                purge,
                vec![
                    rule(operation(purge), s.environment, &["manage"]),
                    rule("EnvironmentGet", s.environment, &["read"]),
                ],
            )
            .await;
        let saved = receipt(&s.call(purge, &writer.secret).await);
        let reader = s
            .key(
                purge,
                vec![rule(operation(purge), s.environment, &["manage", "read"])],
            )
            .await;
        let shown = full(&s.call(purge, &reader.secret).await);
        assert_eq!(shown.id, saved.job_id);
        assert_eq!(shown.environment_id, Some(s.environment));
        let ordinary = full(&s.call(purge, &s.actor(purge).token).await);
        assert_eq!(
            shown, ordinary,
            "the original native Job fields and progress stay intact"
        );
        assert_eq!(s.count().await, 1);
    }
}

#[tokio::test]
async fn retained_reservations_recheck_real_key_deadlines_after_root_and_job_insert_waits() {
    for purge in [false, true] {
        for boundary in ["root", "insert"] {
            for expired in [false, true] {
                let s = Scene::new().await;
                let key = s
                    .key(
                        purge,
                        vec![rule(operation(purge), s.environment, &["manage"])],
                    )
                    .await;
                let mut blocker = s.world.db.pool.begin().await.unwrap();
                if boundary == "root" {
                    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
                        .bind(s.environment)
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                } else {
                    sqlx::query("LOCK TABLE jobs IN SHARE MODE")
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                }
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(&mut *blocker)
                    .await
                    .unwrap();
                let request = s.start(purge, key.secret);
                wait(
                    &s,
                    pid,
                    if boundary == "root" {
                        "%FROM environments WHERE id = $1 FOR UPDATE%"
                    } else {
                        "%INSERT INTO jobs%"
                    },
                    &request,
                )
                .await;
                if expired {
                    s.world.advance(chrono::Duration::hours(2));
                }
                blocker.commit().await.unwrap();
                let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
                if expired {
                    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                    assert_eq!(s.count().await, 0);
                    assert_eq!(s.phase().await, "kept");
                } else {
                    receipt(&reply);
                    assert_eq!(s.count().await, 1);
                }
            }
        }
    }
}

#[tokio::test]
async fn retained_job_publication_keeps_its_committed_reservation_after_read_or_key_loss() {
    for purge in [false, true] {
        for change in ["unchanged", "read_closed", "expired"] {
            let s = Scene::new().await;
            let key = s
                .key(
                    purge,
                    vec![rule(operation(purge), s.environment, &["manage", "read"])],
                )
                .await;
            let mut blocker = s.world.db.pool.begin().await.unwrap();
            sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
                .execute(&mut *blocker)
                .await
                .unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let request = s.start(purge, key.secret);
            wait(&s, pid, "%INSERT INTO api_token_activity%", &request).await;
            let saved: (Id, String, Json) =
                sqlx::query_as("SELECT id,xmin::text,plan FROM jobs WHERE environment_id=$1")
                    .bind(s.environment)
                    .fetch_one(&s.world.db.pool)
                    .await
                    .unwrap();
            match change {
                "read_closed" => {
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(key.id)
                        .bind(json!({"rules":[rule(operation(purge),s.environment,&["manage"])]}))
                        .execute(&s.world.db.pool)
                        .await
                        .unwrap();
                }
                "expired" => s.world.advance(chrono::Duration::hours(2)),
                _ => (),
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
            if change == "unchanged" {
                assert_eq!(full(&reply).id, saved.0);
            } else {
                assert_eq!(receipt(&reply).job_id, saved.0);
            }
            let current: (String, Json) =
                sqlx::query_as("SELECT xmin::text,plan FROM jobs WHERE id=$1")
                    .bind(saved.0)
                    .fetch_one(&s.world.db.pool)
                    .await
                    .unwrap();
            assert_eq!(current, (saved.1, saved.2));
            assert_eq!(
                s.count().await,
                1,
                "publication never repeats the reservation"
            );
        }
    }
}
