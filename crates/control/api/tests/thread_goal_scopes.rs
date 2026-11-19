//! Goal creation keeps the native sender/model path and exact completed response bounds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native scope integration assertions"
)]
mod places;
mod rig;
mod support;

use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::goals::{
    GoalAction, ThreadGoalCreate, ThreadGoalCreateBody, ThreadGoalUpdate, ThreadGoalUpdateBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_authz::Caller;
use orch_db::queries::{goals, identity};
use orch_types::{Id, Json, new_id};
use places::Who;
use rig::Scene;
use serde_json::json;

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, thread: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":"thread","id":thread,"reach":"self"},"resources":["thread"],
        "allow":[{"operation":operation,"actions":actions}]})
}
async fn key(s: &Scene, actor: &Who, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let value = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &identity::NewApiToken {
            id: value.id,
            user_id: actor.id(),
            name: "Native goal fixture",
            token_hash: &token_hash(&value.secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    value
}
fn body() -> ThreadGoalCreateBody {
    ThreadGoalCreateBody {
        objective: "  Make the build green  ".into(),
        condition: "  The build passes  ".into(),
        limits: None,
        plan: false,
        notify_on_end: false,
    }
}
async fn held(s: &Scene, name: &str) -> Id {
    let created = s.thread_by(&s.ada, name).await;
    assert!(
        created.input_access.allowed,
        "the ordinary sender/model premise is positive"
    );
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(created.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    created.base.id
}
async fn count(s: &Scene, thread: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_goals WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}
async fn create(s: &Scene, thread: Id, token: &str) -> orch_api_core::testing::RawReply {
    s.rig
        .world
        .app
        .call(ThreadGoalCreate, &[&thread])
        .token(token)
        .body(&body())
        .send_bytes()
        .await
}

#[tokio::test]
async fn goal_creation_uses_exact_write_and_only_its_own_read_opens_the_native_full_reply() {
    let s = Scene::new().await;
    let thread = held(&s, "Goal writer").await;
    let other = held(&s, "Another goal thread").await;
    for refused in [
        rule("ThreadGoalCreate", other, &["write"]),
        rule("ThreadGoalCreate", thread, &["read"]),
        rule("ThreadGoalUpdate", thread, &["write"]),
    ] {
        let token = key(&s, &s.ada, vec![refused]).await;
        let reply = create(&s, thread, &token.secret).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert_eq!(count(&s, thread).await, 0);
    }
    let writer = key(
        &s,
        &s.ada,
        vec![
            rule("ThreadGoalCreate", thread, &["write"]),
            rule("ThreadGoalGet", thread, &["read"]),
        ],
    )
    .await;
    let reply = create(&s, thread, &writer.secret).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let receipt: Json = serde_json::from_slice(&reply.raw).unwrap();
    let stored = goals::current(&mut *s.rig.world.conn().await, thread)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt,
        json!({"projection":"receipt","thread_id":thread,"goal_id":stored.id})
    );
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Make the build green"));
    let exists = create(&s, thread, &writer.secret).await;
    assert_eq!(
        exists.problem().code,
        orch_api_contract::common::ErrorCode::GoalExists
    );
    assert!(!String::from_utf8_lossy(&exists.raw).contains("Make the build green"));
    assert_eq!(count(&s, thread).await, 1);
    let reader = key(
        &s,
        &s.ada,
        vec![rule("ThreadGoalCreate", other, &["write", "read"])],
    )
    .await;
    let full = create(&s, other, &reader.secret).await;
    assert_eq!(full.status, StatusCode::CREATED);
    let full: Json = serde_json::from_slice(&full.raw).unwrap();
    assert_eq!(full["objective"], "Make the build green");
    assert_eq!(full["condition"], "The build passes");
    assert!(
        full.get("projection").is_none(),
        "ordinary full wire shape is unchanged"
    );
    let browser_thread = held(&s, "Native full control").await;
    let browser = create(&s, browser_thread, &s.ada.token).await;
    assert_eq!(browser.status, StatusCode::CREATED);
    let browser: Json = serde_json::from_slice(&browser.raw).unwrap();
    for field in [
        "objective",
        "condition",
        "status",
        "limits",
        "budgets",
        "helper",
        "agent_reports",
    ] {
        assert_eq!(full[field], browser[field], "{field}");
    }
}

#[tokio::test]
async fn goal_scope_keeps_native_interact_and_model_use_without_adding_manage_or_provider_api_scope()
 {
    let s = Scene::new().await;
    for actor in [&s.cai, &s.dan] {
        let thread = held(&s, "Ordinary goal refusal").await;
        let token = key(
            &s,
            actor,
            vec![rule("ThreadGoalCreate", thread, &["write", "read"])],
        )
        .await;
        assert_eq!(
            create(&s, thread, &token.secret).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(count(&s, thread).await, 0);
    }
    let thread = held(&s, "Only ordinary interaction").await;
    let root = s.tree.org.base.id;
    orch_db::queries::access::add_member(
        &mut *s.rig.world.conn().await,
        root,
        s.bea.id(),
        root,
        Some(s.ada.id()),
        s.rig.world.now(),
    )
    .await
    .unwrap();
    s.rig
        .lets_use("node", thread, "thread.interact", &s.bea, json!({}))
        .await;
    s.rig
        .lets_use("provider", s.on.provider, "provider.use", &s.bea, json!({}))
        .await;
    let mut conn = s.rig.world.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(s.bea.id()),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    for (permission, expected) in [
        (
            orch_access_contract::permissions::Permission::ThreadView,
            true,
        ),
        (
            orch_access_contract::permissions::Permission::ThreadManage,
            false,
        ),
        (
            orch_access_contract::permissions::Permission::ThreadInteract,
            true,
        ),
    ] {
        assert_eq!(
            orch_authz::nodes::check(
                &mut conn,
                &principal,
                thread,
                permission,
                orch_authz::Act::Use,
                orch_authz::Policy::default()
            )
            .await
            .unwrap()
            .is_allowed(),
            expected
        );
    }
    drop(conn);
    let token = key(
        &s,
        &s.bea,
        vec![rule("ThreadGoalCreate", thread, &["write", "read"])],
    )
    .await;
    let reply = create(&s, thread, &token.secret).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let goal: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(goal["objective"], "Make the build green");
    assert!(
        goal.get("projection").is_none(),
        "the setter keeps its native full response authority"
    );
}

#[tokio::test]
async fn goal_creation_rechecks_the_actual_key_after_the_native_thread_wait_before_inserting() {
    for expire in [false, true] {
        let s = Scene::new().await;
        let thread = held(&s, "Goal target wait").await;
        let token = key(
            &s,
            &s.ada,
            vec![rule("ThreadGoalCreate", thread, &["write"])],
        )
        .await;
        let mut blocker = s.rig.db.pool().begin().await.unwrap();
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(thread)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.rig.world.app.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(ThreadGoalCreate, &[&thread])
                .token(&token.secret)
                .body(&body());
            call.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the native goal thread lock must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM threads t%FOR UPDATE OF t%')")
                    .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual goal target row serializes the request");
        if expire {
            s.rig.world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::CREATED
            }
        );
        assert_eq!(count(&s, thread).await, i64::from(!expire));
    }
}

#[tokio::test]
async fn completed_goal_publication_keeps_its_receipt_after_activity_wait_without_replaying() {
    let s = Scene::new().await;
    let thread = held(&s, "Accepted goal publication").await;
    let token = key(
        &s,
        &s.ada,
        vec![rule("ThreadGoalCreate", thread, &["write", "read"])],
    )
    .await;
    let mut blocker = s.rig.db.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = s.rig.world.app.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(ThreadGoalCreate, &[&thread])
            .token(&token.secret)
            .body(&body());
        call.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the completed goal's activity wait must be reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the already completed goal is waiting at actual activity publication");
    assert_eq!(count(&s, thread).await, 1);
    let goal = goals::current(&mut *s.rig.world.conn().await, thread)
        .await
        .unwrap()
        .unwrap();
    // Pausing is a real independent native act, keeping the lifecycle loop quiet while time advances.
    s.rig
        .world
        .app
        .call(ThreadGoalUpdate, &[&thread])
        .token(&s.ada.token)
        .body(&ThreadGoalUpdateBody {
            objective: None,
            condition: None,
            limits: None,
            notify_on_end: None,
            action: Some(GoalAction::Pause),
        })
        .send()
        .await
        .ok();
    let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM thread_goals WHERE id=$1")
        .bind(goal.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    s.rig.world.advance(Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        json!({"projection":"receipt","thread_id":thread,"goal_id":goal.id})
    );
    assert_eq!(count(&s, thread).await, 1);
    let later: String = sqlx::query_scalar("SELECT xmin::text FROM thread_goals WHERE id=$1")
        .bind(goal.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(later, xmin);
    let markers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items WHERE thread_id=$1 AND payload->>'marker'='goal_set'",
    )
    .bind(thread)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(markers, 1);
}

#[tokio::test]
async fn a_goal_continuation_retains_its_creation_key_without_borrowing_input_create() {
    for revoked in [false, true] {
        let s = Scene::new().await;
        let thread = held(&s, "Goal origin continuation").await;
        let token = key(
            &s,
            &s.ada,
            vec![rule("ThreadGoalCreate", thread, &["write"])],
        )
        .await;
        assert_eq!(
            create(&s, thread, &token.secret).await.status,
            StatusCode::CREATED
        );
        let goal = goals::current(&mut *s.rig.world.conn().await, thread)
            .await
            .unwrap()
            .unwrap();
        if revoked {
            assert!(
                identity::revoke_api_token(
                    &mut *s.rig.world.conn().await,
                    token.id,
                    s.ada.id(),
                    s.rig.world.now()
                )
                .await
                .unwrap()
            );
        }
        let revision: i64 =
            sqlx::query_scalar("SELECT queue_revision FROM threads WHERE node_id=$1")
                .bind(thread)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        s.rig
            .engine
            .resume_queue(&Caller::session(s.ada.id()), thread, revision)
            .await
            .unwrap();
        if revoked {
            rig::eventually("the goal's original key refusal", || async {
                sqlx::query_scalar::<_, bool>(
                    "SELECT status='stopped' AND reason='access' FROM thread_goals WHERE id=$1",
                )
                .bind(goal.id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
            })
            .await;
            assert!(s.rig.turn_starts(thread).is_empty());
        } else {
            let start = s.rig.turn_start(thread, 0).await;
            let author: Option<Id> = sqlx::query_scalar(
                "SELECT author_id FROM thread_inputs WHERE turn_id=$1 AND source='goal'",
            )
            .bind(start.turn_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
            assert_eq!(author, Some(s.ada.id()));
            assert!(start.text.contains("Make the build green"));
        }
    }
}
