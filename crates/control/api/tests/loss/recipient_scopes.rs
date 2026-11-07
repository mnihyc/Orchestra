//! The private native preview's actual subjects and current recipient visibility.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::PageQuery;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;

const OPERATION: &str = "OperationRecipientsGet";

fn rule(operation: &str, kind: &str, id: Id, business: &str, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[business],
        "allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native private audience fixture",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    token
}

struct Scene {
    world: World,
    people: resources::Company,
    host: Id,
    environment: Id,
    plan: LossPlan,
}

impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let people = resources::company(&world).await;
        let (host, environment, _) = mixed_host(&world, &people).await;
        let plan = world
            .app
            .call(LossPreflight, &[])
            .token(&people.viewer.token)
            .body(&LossPreflightBody {
                target: LossTarget::Host { id: host },
            })
            .send()
            .await
            .ok();
        Self {
            world,
            people,
            host,
            environment,
            plan,
        }
    }
    async fn key(&self) -> String {
        key(
            &self.world,
            self.people.viewer.id(),
            vec![rule(OPERATION, "host", self.host, "host", "read")],
        )
        .await
    }
    async fn call(&self, token: &str, id: Id, query: &PageQuery) -> RawReply {
        let call = self.world.app.call(OperationRecipientsGet, &[&id]);
        call.token(token).query(query).send_bytes().await
    }
    async fn shown(&self, token: &str, query: &PageQuery) -> OperationRecipients {
        let reply = self.call(token, self.plan.preflight_id, query).await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        serde_json::from_slice(&reply.raw).unwrap()
    }
}

#[tokio::test]
async fn recipients_scope_keeps_the_native_private_page_cursor_counts_and_profile_filters() {
    let s = Scene::new().await;
    sqlx::query("INSERT INTO user_profiles(user_id,audience) VALUES($1,'private') ON CONFLICT(user_id) DO UPDATE SET audience='private'")
        .bind(s.people.stranger.id()).execute(&s.world.db.pool).await.unwrap();
    sqlx::query("INSERT INTO notification_prefs(user_id,disabled_kinds) VALUES($1,'[\"environment.attention\"]') ON CONFLICT(user_id) DO UPDATE SET disabled_kinds=EXCLUDED.disabled_kinds")
        .bind(s.people.boss.id()).execute(&s.world.db.pool).await.unwrap();
    let token = s.key().await;
    for query in [
        PageQuery::default(),
        PageQuery {
            limit: Some(1),
            cursor: None,
        },
        PageQuery {
            limit: Some(1),
            cursor: Some(s.people.viewer.id().to_string()),
        },
    ] {
        let native = s.shown(&s.people.viewer.token, &query).await;
        let bounded = s.shown(&token, &query).await;
        assert_eq!(bounded, native);
        assert_eq!(bounded.hidden, 1);
        assert!(
            !bounded.people.items.iter().any(
                |person| person.id == s.people.stranger.id() || person.id == s.people.boss.id()
            )
        );
    }
    let body = s.shown(&token, &Default::default()).await;
    assert!(
        body.total > 0,
        "the positive preview is an actual nonempty native audience"
    );
}

#[tokio::test]
async fn recipients_require_the_actual_saved_subject_own_operation_action_and_native_issuer() {
    let s = Scene::new().await;
    let token = s.key().await;
    assert!(s.shown(&token, &Default::default()).await.total > 0);
    for rules in [
        vec![rule("HostGet", "host", s.host, "host", "read")],
        vec![rule(OPERATION, "host", s.host, "host", "manage")],
        vec![rule(OPERATION, "host", s.plan.preflight_id, "host", "read")],
        vec![rule(OPERATION, "environment", s.host, "host", "read")],
        vec![rule(
            OPERATION,
            "environment",
            s.environment,
            "environment",
            "read",
        )],
        vec![rule(
            OPERATION,
            "account",
            s.people.viewer.id(),
            "host",
            "read",
        )],
    ] {
        let denied = key(&s.world, s.people.viewer.id(), rules).await;
        assert_eq!(
            s.call(&denied, s.plan.preflight_id, &Default::default())
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    let other_issuer = key(
        &s.world,
        s.people.boss.id(),
        vec![rule(OPERATION, "host", s.host, "host", "read")],
    )
    .await;
    assert_eq!(
        s.call(&other_issuer, s.plan.preflight_id, &Default::default())
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE preflights SET expires_at=$2 WHERE id=$1")
        .bind(s.plan.preflight_id)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        s.call(&token, s.plan.preflight_id, &Default::default())
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn recipients_use_the_native_loss_subject_kind_and_canonical_physical_machine_governor() {
    let s = Scene::new().await;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(s.host)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    for (target, actor, namespace, id, business) in [
        (
            LossTarget::Environment { id: s.environment },
            &s.people.viewer,
            "environment",
            s.environment,
            "environment",
        ),
        (
            LossTarget::Machine { id: machine },
            &s.people.boss,
            "host",
            s.host,
            "machine",
        ),
    ] {
        let native = s
            .world
            .app
            .call(LossPreflight, &[])
            .token(&actor.token)
            .body(&LossPreflightBody { target })
            .send()
            .await
            .ok();
        let token = key(
            &s.world,
            actor.id(),
            vec![rule(OPERATION, namespace, id, business, "read")],
        )
        .await;
        let ordinary = s
            .call(&actor.token, native.preflight_id, &Default::default())
            .await;
        let bounded = s
            .call(&token, native.preflight_id, &Default::default())
            .await;
        assert_eq!(ordinary.status, StatusCode::OK);
        assert_eq!(
            bounded.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bounded.raw)
        );
        let ordinary: OperationRecipients = serde_json::from_slice(&ordinary.raw).unwrap();
        let bounded: OperationRecipients = serde_json::from_slice(&bounded.raw).unwrap();
        assert_eq!(ordinary, bounded);
        let denied = key(
            &s.world,
            actor.id(),
            vec![rule(
                OPERATION,
                namespace,
                id,
                if business == "machine" {
                    "host"
                } else {
                    "provider"
                },
                "read",
            )],
        )
        .await;
        assert_eq!(
            s.call(&denied, native.preflight_id, &Default::default())
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn recipients_publication_rechecks_the_original_visible_people_after_the_real_activity_wait()
{
    for hide in [false, true] {
        let s = Scene::new().await;
        let token = s.key().await;
        let before = s.shown(&token, &Default::default()).await;
        let named = before
            .people
            .items
            .iter()
            .find(|person| person.id != s.people.viewer.id())
            .expect("the actual native preview contains another named person")
            .id;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let id = s.plan.preflight_id;
        let request = tokio::spawn(async move {
            let call = app.call(OperationRecipientsGet, &[&id]);
            call.token(&token)
                .query(&PageQuery::default())
                .send_bytes()
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the native recipient page must reach activity publication");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if waiting { break; } tokio::task::yield_now().await;
            }
        }).await.expect("the actual native recipient activity INSERT wait");
        if hide {
            sqlx::query("INSERT INTO user_profiles(user_id,audience) VALUES($1,'private') ON CONFLICT(user_id) DO UPDATE SET audience='private'")
                .bind(named).execute(&s.world.db.pool).await.unwrap();
            let native = s.shown(&s.people.viewer.token, &Default::default()).await;
            assert!(!native.people.items.iter().any(|person| person.id == named));
            assert_eq!(
                native.total, before.total,
                "the actual receiver remains, only its native name visibility changed"
            );
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if hide {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
    }
}
