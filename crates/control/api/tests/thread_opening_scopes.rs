//! A compound thread read retains its own namespace, actual panes and original publication sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native HTTP integration assertions"
)]
mod places;
mod rig;
mod support;

use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_api_contract::threads::{
    ContentPart, Delivery, ThreadDraftSet, ThreadDraftSetBody, ThreadOpeningGet,
    ThreadOpeningGetQuery, ThreadSyncGet, ThreadSyncGetQuery,
};
use orch_api_contract::usage::{CapInput, CapMeter, CapSplit, CapSubjectInput, UsageCapCreate};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use places::Who;
use rig::Scene;
use serde_json::json;
use std::time::Duration as Wait;
use tokio::task::JoinHandle;

const OP: &str = "ThreadOpeningGet";
const PRIVATE_DRAFT: &str = "The selected author's unfinished private question";
const WAIT: Wait = Wait::from_secs(30);

struct Key {
    id: Id,
    secret: String,
}
fn rule_for(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":[action]}]})
}
fn rule(kind: &str, id: Id) -> Json {
    rule_for(OP, kind, id, "read")
}
fn full(s: &Scene, thread: Id) -> Vec<Json> {
    vec![
        rule("thread", thread),
        rule("provider", s.on.provider),
        rule("agent", s.on.agent),
        rule("environment", s.on.environment),
        rule("org", s.tree.org.base.id),
        rule("project", s.tree.project.base.id),
        rule("topic", s.tree.topic.base.id),
    ]
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
            name: "Compound native reader fixture",
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
async fn open(s: &Scene, token: &str, thread: Id, query: &ThreadOpeningGetQuery) -> RawReply {
    s.rig
        .world
        .app
        .call(ThreadOpeningGet, &[&thread])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
fn body(reply: &RawReply) -> Json {
    serde_json::from_slice(&reply.raw).unwrap()
}
fn ok(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    body(&reply)
}
async fn draft(s: &Scene, thread: Id) {
    s.rig
        .world
        .app
        .call(ThreadDraftSet, &[&thread])
        .token(&s.ada.token)
        .body(&ThreadDraftSetBody {
            native_action: None,
            content: vec![ContentPart::Text {
                text: PRIVATE_DRAFT.into(),
            }],
            delivery: Delivery::Queue,
            review_comments: None,
        })
        .send()
        .await
        .ok();
}
async fn activity_wait(s: &Scene, pid: i32, request: &JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT,async {
        loop {
            assert!(!request.is_finished(),"the native screen must reach its real publication wait");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual activity write follows the successful read snapshot");
}

#[tokio::test]
async fn an_opening_uses_its_own_thread_read_and_preserves_the_callers_draft_and_native_window() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.exchange(
        thread,
        0,
        "Original authored prompt",
        "Original native answer",
    )
    .await;
    rig::eventually("the native terminal accounting is committed", || async {
        !orch_db::queries::threads::turn_end_work_pending(&mut *s.rig.world.conn().await, thread)
            .await
            .unwrap()
    })
    .await;
    draft(&s, thread).await;
    let reader = key(&s, &s.ada, vec![rule("thread", thread)]).await;
    let query = ThreadOpeningGetQuery {
        limit: Some(1),
        ..Default::default()
    };
    let opened = ok(open(&s, &reader.secret, thread, &query).await);
    let ordinary = ok(open(&s, &s.ada.token, thread, &query).await);
    assert_eq!(opened["sync"]["thread"]["id"], json!(thread));
    assert_eq!(opened["sync"]["items"], ordinary["sync"]["items"]);
    assert_eq!(opened["outline"], ordinary["outline"]);
    assert_eq!(opened["sync"]["seq"], opened["outline"]["seq"]);
    assert_eq!(opened["draft"], ordinary["draft"]);
    assert!(opened["draft"].to_string().contains(PRIVATE_DRAFT));
    assert!(
        opened["parameters"].is_null(),
        "binding sources are an independent Read"
    );
    assert!(ordinary["parameters"].is_object());
    assert!(!opened["settings"].as_array().unwrap().is_empty());
    assert_eq!(
        s.rig
            .world
            .app
            .call(ThreadSyncGet, &[&thread])
            .token(&reader.secret)
            .query(&ThreadSyncGetQuery::default())
            .send_bytes()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "the opening's Read never lends its child reader's namespace"
    );
    let viewer = key(&s, &s.cai, vec![rule("thread", thread)]).await;
    let viewed = ok(open(&s, &viewer.secret, thread, &query).await);
    assert!(viewed["draft"].is_null());
    assert!(!viewed.to_string().contains(PRIVATE_DRAFT));
}

#[tokio::test]
async fn child_namespaces_wrong_targets_and_write_do_not_admit_the_opening() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let other = s.thread_by(&s.ada, "Another native thread").await.base.id;
    for rules in [
        vec![rule("thread", other)],
        vec![rule_for(OP, "thread", thread, "write")],
        [
            "ThreadSyncGet",
            "ThreadDraftGet",
            "ThreadParameterGet",
            "ThreadLimitsGet",
            "NodeSettingList",
        ]
        .map(|operation| rule_for(operation, "thread", thread, "read"))
        .to_vec(),
    ] {
        let reader = key(&s, &s.ada, rules).await;
        let reply = open(
            &s,
            &reader.secret,
            thread,
            &ThreadOpeningGetQuery::default(),
        )
        .await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains("Fix login"));
    }
    let outsider = key(&s, &s.bea, full(&s, thread)).await;
    assert_eq!(
        open(
            &s,
            &outsider.secret,
            thread,
            &ThreadOpeningGetQuery::default()
        )
        .await
        .status,
        StatusCode::FORBIDDEN,
        "scope cannot supply the native conversation View"
    );
}

#[tokio::test]
async fn native_parameter_sources_require_this_operation_and_do_not_borrow_provider_get() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let mut borrowed = full(&s, thread);
    borrowed.retain(|rule| rule["target"]["kind"] != "provider");
    borrowed.push(rule_for("ProviderGet", "provider", s.on.provider, "read"));
    let reader = key(&s, &s.ada, borrowed).await;
    let opened = ok(open(
        &s,
        &reader.secret,
        thread,
        &ThreadOpeningGetQuery::default(),
    )
    .await);
    assert!(opened["parameters"].is_null());
    let reader = key(&s, &s.ada, full(&s, thread)).await;
    let opened = ok(open(
        &s,
        &reader.secret,
        thread,
        &ThreadOpeningGetQuery::default(),
    )
    .await);
    let ordinary = ok(open(&s, &s.ada.token, thread, &ThreadOpeningGetQuery::default()).await);
    assert!(opened["parameters"].is_object());
    assert_eq!(
        opened["parameters"]["model"]["options"],
        ordinary["parameters"]["model"]["options"]
    );
    assert_eq!(
        opened["parameters"]["effort"]["value"],
        ordinary["parameters"]["effort"]["value"]
    );
    assert_eq!(opened["parameters"]["provider"]["id"], json!(s.on.provider));
    assert_eq!(
        opened["parameters"]["provider"]["name"],
        ordinary["parameters"]["provider"]["name"]
    );
    assert_eq!(
        opened["parameters"]["provider"]["access"],
        ordinary["parameters"]["provider"]["access"]
    );
}

#[tokio::test]
async fn settings_keep_the_native_value_while_provenance_names_use_the_openings_own_sources() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let topic = s.tree.topic.base.id;
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&topic, &"thread.side_max"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(3),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let reader = key(
        &s,
        &s.ada,
        vec![
            rule("thread", thread),
            rule_for("NodeSettingList", "topic", topic, "read"),
        ],
    )
    .await;
    let opened = ok(open(
        &s,
        &reader.secret,
        thread,
        &ThreadOpeningGetQuery::default(),
    )
    .await);
    let setting = opened["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "thread.side_max")
        .unwrap();
    assert_eq!(setting["value"], 3);
    assert_eq!(setting["source"]["node"]["id"], json!(topic));
    assert!(setting["source"]["node"]["name"].is_null());
    assert_eq!(setting["source"]["node"]["redacted"], true);
    let reader = key(
        &s,
        &s.ada,
        vec![rule("thread", thread), rule("topic", topic)],
    )
    .await;
    let opened = ok(open(
        &s,
        &reader.secret,
        thread,
        &ThreadOpeningGetQuery::default(),
    )
    .await);
    let setting = opened["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "thread.side_max")
        .unwrap();
    assert_eq!(
        setting["source"]["node"]["name"],
        json!(s.tree.topic.base.name)
    );
    assert_eq!(setting["source"]["node"]["redacted"], false);
}

#[tokio::test]
async fn a_limits_pane_keeps_the_actual_reached_cap_or_is_unavailable_as_a_whole() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let running = s
        .thread_by(&s.ada, "Another current native turn")
        .await
        .base
        .id;
    let cap = s
        .rig
        .world
        .app
        .call(UsageCapCreate, &[])
        .token(&s.ada.token)
        .body(&CapInput {
            node_id: Some(s.tree.topic.base.id),
            provider_id: None,
            subject: CapSubjectInput::Everyone,
            split: CapSplit::Shared,
            models: None,
            tiers: None,
            harness_kinds: None,
            meter: CapMeter::Running,
            period: None,
            counted_from: None,
            amount: Some("1".into()),
            at_limit: None,
            warn_percent: None,
            uncounted: None,
            note: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    s.say(
        &s.ada,
        running,
        "Keep this actual native turn running",
        Delivery::Queue,
        "actual-cap-holder",
    )
    .await
    .ok();
    let start = s.rig.turn_start(running, 0).await;
    s.rig.say(running, rig::turn_began(start.turn_id)).await;
    s.rig.wait_turn(start.turn_id, "running").await;
    let ordinary = ok(open(&s, &s.ada.token, thread, &ThreadOpeningGetQuery::default()).await);
    let native = ordinary["limits"]["reached"].as_array().unwrap();
    assert_eq!(
        native.len(),
        1,
        "the real cap is reached before scope assertions"
    );
    assert_eq!(native[0]["cap"]["cap"]["id"], json!(cap.id));
    let narrow = key(&s, &s.ada, vec![rule("thread", thread)]).await;
    let opened = ok(open(
        &s,
        &narrow.secret,
        thread,
        &ThreadOpeningGetQuery::default(),
    )
    .await);
    assert!(
        opened["limits"].is_null(),
        "never a misleading empty list from a partial key"
    );
    let complete = key(
        &s,
        &s.ada,
        vec![rule("thread", thread), rule("topic", s.tree.topic.base.id)],
    )
    .await;
    let opened = ok(open(
        &s,
        &complete.secret,
        thread,
        &ThreadOpeningGetQuery::default(),
    )
    .await);
    assert_eq!(
        opened["limits"]["reached"][0]["cap"]["cap"]["id"],
        json!(cap.id)
    );
    assert_eq!(opened["limits"]["reached"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_opened_parameter_body_cannot_borrow_a_later_narrower_read_at_publication() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    draft(&s, thread).await;
    let reader = key(&s, &s.ada, full(&s, thread)).await;
    let premise = ok(open(
        &s,
        &reader.secret,
        thread,
        &ThreadOpeningGetQuery::default(),
    )
    .await);
    assert!(premise["parameters"].is_object());
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
    let secret = reader.secret.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(ThreadOpeningGet, &[&thread])
            .token(&secret)
            .query(&ThreadOpeningGetQuery::default());
        call.send_bytes().await
    });
    activity_wait(&s, pid, &request).await;
    let scope: KeyScope = serde_json::from_value(json!({"rules":[rule("thread",thread)]})).unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reader.id)
        .bind(json!(scope))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let refused = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&refused.raw).contains(PRIVATE_DRAFT));
    let fresh = ok(open(
        &s,
        &reader.secret,
        thread,
        &ThreadOpeningGetQuery::default(),
    )
    .await);
    assert!(
        fresh["parameters"].is_null(),
        "a new admitted read can still open its real narrower screen"
    );
}

#[tokio::test]
async fn a_native_screen_is_not_returned_after_the_original_key_expires_in_activity_bookkeeping() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    draft(&s, thread).await;
    let reader = key(&s, &s.ada, vec![rule("thread", thread)]).await;
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
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.secret)
            .query(&ThreadOpeningGetQuery::default());
        call.send_bytes().await
    });
    activity_wait(&s, pid, &request).await;
    s.rig.world.advance(Duration::hours(2));
    blocker.commit().await.unwrap();
    let refused = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    assert!(!String::from_utf8_lossy(&refused.raw).contains(PRIVATE_DRAFT));
}

#[tokio::test]
async fn the_opening_keeps_native_branch_membership_and_cursor_validation_under_its_own_read() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let other = s
        .thread_by(&s.ada, "Foreign continuation owner")
        .await
        .base
        .id;
    let foreign = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, other)
        .await
        .unwrap()
        .unwrap()
        .active_continuation_id;
    let reader = key(&s, &s.ada, vec![rule("thread", thread)]).await;
    let query = ThreadOpeningGetQuery {
        continuation_id: Some(foreign),
        ..Default::default()
    };
    assert_eq!(
        open(&s, &reader.secret, thread, &query).await.status,
        StatusCode::NOT_FOUND
    );
    let query = ThreadOpeningGetQuery {
        projection_revision: Some(0),
        ..Default::default()
    };
    assert_eq!(
        open(&s, &reader.secret, thread, &query).await.status,
        StatusCode::CONFLICT
    );
    let query = ThreadOpeningGetQuery {
        limit: Some(0),
        ..Default::default()
    };
    assert_eq!(
        open(&s, &reader.secret, thread, &query)
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
    assert_eq!(
        open(&s, &s.ada.token, thread, &query).await.problem().code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
}
