//! Explicit approval operations use the same native approval ledger, authority and writers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod rig;
mod support;

#[path = "approval_operation_scopes/checkpoints.rs"]
mod checkpoints;
#[path = "approval_operation_scopes/variants.rs"]
mod variants;

use http::StatusCode;
use orch_access_contract::{key_scope::KeyScope, permissions::Permission};
use orch_api_contract::access::{
    AccessRequestCreate, AccessRequestCreateBody, AccessRequestItem, AccessRequestObject,
};
use orch_api_contract::approvals::{
    ApprovalCancel, ApprovalDecide, ApprovalDecision, ApprovalGet, ApprovalList, ApprovalListQuery,
};
use orch_api_contract::threads::Delivery;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::queries::identity;
use orch_link_contract::{
    commands::CommandKind,
    events::{ApprovalRequested, Event},
};
use orch_types::{Id, Json, Time, new_id};
use places::Who;
use rig::Scene;
use serde_json::json;

const GET: &str = "ApprovalGet";
const LIST: &str = "ApprovalList";
const HARNESS: &str = "ApprovalDecide.harness";
const ACCESS: &str = "ApprovalDecide.access";
const CANCEL: &str = "ApprovalCancel";
const COMMAND: &str = "printf 'literal approval command sentinel'";

fn rule(kind: &str, id: Id, resource: &str, op: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[resource],"allow":[{"operation":op,"actions":actions}]})
}
fn names(s: &Scene, op: &str) -> Json {
    json!({"target":{"kind":"org","id":s.tree.org.base.id,"reach":"subtree"},"resources":["org","project","topic","thread"],"allow":[{"operation":op,"actions":["read"]}]})
}
fn read_rules(s: &Scene, thread: Id, op: &str) -> Vec<Json> {
    vec![
        rule("thread", thread, "approval", op, &["read"]),
        names(s, op),
    ]
}
fn decide_rules(s: &Scene, thread: Id, op: &str, action: &str, full: bool) -> Vec<Json> {
    let mut rules = vec![rule("thread", thread, "thread", op, &[action])];
    if full {
        rules.extend(read_rules(s, thread, op));
    }
    rules
}
struct Key {
    id: Id,
    secret: String,
}
async fn key(s: &Scene, actor: Id, rules: Vec<Json>) -> Key {
    let policy: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(policy.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Approval operation control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(policy).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn legacy(s: &Scene, actor: Id) -> Key {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *s.rig.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Legacy approval witness",
            token_hash: &token_hash(&secret),
            scopes: &["*".into()],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn checked(reply: RawReply, status: StatusCode) -> Json {
    let body = if reply.raw.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(&reply.raw).unwrap()
    };
    // Port controls added to this family carry one-time capabilities; do not print successful bodies.
    assert_eq!(
        reply.status, status,
        "code={}, required={}",
        body["code"], body["required"]
    );
    body
}
async fn get(s: &Scene, token: &str, id: Id) -> RawReply {
    s.rig
        .world
        .app
        .call(ApprovalGet, &[&id])
        .token(token)
        .send_bytes()
        .await
}
async fn list(s: &Scene, token: &str, query: &ApprovalListQuery) -> RawReply {
    s.rig
        .world
        .app
        .call(ApprovalList, &[])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
async fn decide(s: &Scene, token: &str, id: Id, body: &Json) -> RawReply {
    s.rig
        .world
        .app
        .call(ApprovalDecide, &[&id])
        .token(token)
        .raw_body(body)
        .send_bytes()
        .await
}
async fn cancel(s: &Scene, token: &str, id: Id) -> RawReply {
    s.rig
        .world
        .app
        .call(ApprovalCancel, &[&id])
        .token(token)
        .send_bytes()
        .await
}
fn command() -> Json {
    json!({"request_kind":"command","detail":{"command":COMMAND,"cwd":"/workspace/web","reason":null},
    "options":[{"id":"allow_once","label":"Allow once","style":"allow","scope":"once"},{"id":"deny","label":"Deny","style":"deny","scope":null}]})
}
fn proposed_plan() -> Json {
    json!({"request_kind":"plan","detail":{"plan":"Inspect, then make the agreed change"},
    "options":[{"id":"proceed","label":"Proceed","style":"allow","scope":"once"},{"id":"keep_planning","label":"Keep planning","style":"deny","scope":null}]})
}
fn question() -> Json {
    json!({"request_kind":"user_input","options":[],"detail":{"questions":[{"id":"branch","header":"Branch","question":"Which branch?","multiple":false,"options":null,"allow_other":true}]}})
}
async fn asking(s: &Scene, thread: Id, requests: &[(&str, Json)]) -> Vec<Id> {
    assert!(
        s.say(
            &s.ada,
            thread,
            "Review the exact request",
            Delivery::Queue,
            "approval-operation-start"
        )
        .await
        .status
        .is_success()
    );
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, rig::turn_began(start.turn_id)).await;
    let mut ids = Vec::new();
    for (reference, request) in requests {
        s.rig
            .say(
                thread,
                Event::ApprovalRequested(ApprovalRequested {
                    turn_id: start.turn_id,
                    request_ref: (*reference).into(),
                    title: format!("Approval request {reference}"),
                    request: serde_json::from_value(request.clone()).unwrap(),
                }),
            )
            .await;
        rig::eventually("the native request is retained", || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM approvals WHERE thread_id=$1 AND native_ref=$2)",
            )
            .bind(thread)
            .bind(reference)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
        })
        .await;
        let id: Id =
            sqlx::query_scalar("SELECT id FROM approvals WHERE thread_id=$1 AND native_ref=$2")
                .bind(thread)
                .bind(reference)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        ids.push(id);
    }
    s.rig.wait_state(thread, "waiting_approval").await;
    ids
}
async fn state(s: &Scene, id: Id) -> String {
    sqlx::query_scalar("SELECT state FROM approvals WHERE id=$1")
        .bind(id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}
async fn has(s: &Scene, who: &Who, thread: Id, permission: Permission) -> bool {
    let mut conn = s.rig.world.conn().await;
    let principal = Principal::load(
        &mut conn,
        Caller::session(who.id()),
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    orch_authz::nodes::check(
        &mut conn,
        &principal,
        thread,
        permission,
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap()
    .is_allowed()
}
async fn access_request(s: &Scene, thread: Id) -> Id {
    assert!(!has(s, &s.cai, thread, Permission::ThreadInteract).await);
    let response = s
        .rig
        .world
        .app
        .call(AccessRequestCreate, &[])
        .token(&s.cai.token)
        .body(&AccessRequestCreateBody {
            items: vec![AccessRequestItem {
                label: None,
                object: AccessRequestObject {
                    kind: "thread".into(),
                    id: thread,
                },
                permissions: vec!["thread.interact".into()],
                options: None,
                holder: None,
            }],
            reason: "Let me contribute".into(),
            expires_at: None,
            for_thread: None,
        })
        .send_bytes()
        .await;
    let body = checked(response, StatusCode::OK);
    serde_json::from_value(body["approvals"][0]["id"].clone()).unwrap()
}
fn under(s: &Scene) -> ApprovalListQuery {
    ApprovalListQuery {
        scope: Some(format!("node:{}", s.tree.topic.base.id)),
        all: Some(true),
        ..Default::default()
    }
}

#[tokio::test]
async fn scoped_get_preserves_the_browser_and_explicit_legacy_full_card() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = asking(&s, thread, &[("read", command())]).await[0];
    let browser = checked(get(&s, &s.ada.token, id).await, StatusCode::OK);
    let old = legacy(&s, s.ada.id()).await;
    assert_eq!(
        checked(get(&s, &old.secret, id).await, StatusCode::OK),
        browser
    );
    let key = key(&s, s.ada.id(), read_rules(&s, thread, GET)).await;
    let shown = checked(get(&s, &key.secret, id).await, StatusCode::OK);
    assert_eq!(shown, browser);
    assert_eq!(shown["request"]["detail"]["command"], COMMAND);
    assert_eq!(state(&s, id).await, "pending");
}
#[tokio::test]
async fn scoped_get_does_not_borrow_the_same_users_other_thread_rule() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let other = s.thread_by(&s.ada, "Other scope").await.base.id;
    let id = asking(&s, thread, &[("read", command())]).await[0];
    checked(get(&s, &s.ada.token, id).await, StatusCode::OK);
    let key = key(&s, s.ada.id(), read_rules(&s, other, GET)).await;
    checked(get(&s, &key.secret, id).await, StatusCode::FORBIDDEN);
    assert_eq!(state(&s, id).await, "pending");
}
#[tokio::test]
async fn scoped_list_pages_past_newer_unselected_cards_to_true_exhaustion() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let wanted = asking(&s, thread, &[("first", command()), ("second", command())]).await;
    let other = s.thread_by(&s.ada, "Unselected newer thread").await.base.id;
    let forbidden = asking(&s, other, &[("newer", command())]).await[0];
    let browser = checked(list(&s, &s.ada.token, &under(&s)).await, StatusCode::OK);
    assert_eq!(browser["items"].as_array().unwrap().len(), 3);
    let key = key(&s, s.ada.id(), read_rules(&s, thread, LIST)).await;
    let mut query = under(&s);
    query.limit = Some(1);
    let first = checked(list(&s, &key.secret, &query).await, StatusCode::OK);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let first_id: Id = serde_json::from_value(first["items"][0]["id"].clone()).unwrap();
    assert!(wanted.contains(&first_id));
    assert_ne!(first_id, forbidden);
    query.cursor = Some(
        first["next_cursor"]
            .as_str()
            .expect("another selected card exists")
            .into(),
    );
    let second = checked(list(&s, &key.secret, &query).await, StatusCode::OK);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    let second_id: Id = serde_json::from_value(second["items"][0]["id"].clone()).unwrap();
    assert!(wanted.contains(&second_id));
    assert_ne!(second_id, first_id);
    assert!(
        second["next_cursor"].is_null(),
        "the selected list is exhausted"
    );
}
#[tokio::test]
async fn scoped_requested_list_uses_the_actual_requested_object() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = access_request(&s, thread).await;
    let query = ApprovalListQuery {
        scope: Some("requested".into()),
        ..Default::default()
    };
    let native = checked(list(&s, &s.cai.token, &query).await, StatusCode::OK);
    assert_eq!(native["items"][0]["id"], json!(id));
    let key = key(&s, s.cai.id(), read_rules(&s, thread, LIST)).await;
    assert_eq!(
        checked(list(&s, &key.secret, &query).await, StatusCode::OK),
        native
    );
}
#[tokio::test]
async fn scoped_harness_manage_keeps_one_native_decision_and_exact_delivery() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let ids = asking(
        &s,
        thread,
        &[
            ("browser", command()),
            ("legacy", command()),
            ("key", command()),
        ],
    )
    .await;
    let allow = json!({"option_id":"allow_once"});
    checked(
        decide(&s, &s.ada.token, ids[0], &allow).await,
        StatusCode::OK,
    );
    let old = legacy(&s, s.ada.id()).await;
    checked(
        decide(&s, &old.secret, ids[1], &allow).await,
        StatusCode::OK,
    );
    let key = key(
        &s,
        s.ada.id(),
        decide_rules(&s, thread, HARNESS, "manage", true),
    )
    .await;
    let shown = checked(
        decide(&s, &key.secret, ids[2], &allow).await,
        StatusCode::OK,
    );
    assert_eq!(shown["state"], "approved");
    assert_eq!(shown["decision"], allow);
    assert_eq!(shown["decided_by"]["id"], json!(s.ada.id()));
    rig::eventually("all three exact native decisions are delivered", || async {
        s.rig.seen.of_kind(CommandKind::ApprovalResolve).len() == 3
    })
    .await;
    let again = checked(
        decide(&s, &key.secret, ids[2], &allow).await,
        StatusCode::CONFLICT,
    );
    assert_eq!(again["code"], "approval.already_decided");
    assert_eq!(again["current"]["id"], json!(ids[2]));
    assert_eq!(s.rig.seen.of_kind(CommandKind::ApprovalResolve).len(), 3);
}
#[tokio::test]
async fn scoped_harness_question_keeps_invalid_answers_pending_then_answers() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = asking(&s, thread, &[("question", question())]).await[0];
    let key = key(
        &s,
        s.ada.id(),
        decide_rules(&s, thread, HARNESS, "manage", true),
    )
    .await;
    let bad = checked(
        decide(&s, &key.secret, id, &json!({"answers":{}})).await,
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(bad["code"], "request.invalid");
    assert_eq!(state(&s, id).await, "pending");
    assert!(s.rig.seen.of_kind(CommandKind::ApprovalResolve).is_empty());
    let answer = json!({"answers":{"branch":["main"]}});
    let shown = checked(decide(&s, &key.secret, id, &answer).await, StatusCode::OK);
    assert_eq!(shown["state"], "answered");
    assert_eq!(shown["decision"], answer);
}
#[tokio::test]
async fn scoped_harness_manage_does_not_replace_the_native_thread_manager() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = asking(&s, thread, &[("ordinary-question", question())]).await[0];
    assert!(!has(&s, &s.cai, thread, Permission::ThreadManage).await);
    let answer = json!({"answers":{"branch":["main"]}});
    let native = checked(
        decide(&s, &s.cai.token, id, &answer).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(native["required"], "thread.manage");
    let key = key(
        &s,
        s.cai.id(),
        decide_rules(&s, thread, HARNESS, "manage", true),
    )
    .await;
    let denied = checked(
        decide(&s, &key.secret, id, &answer).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(denied["required"], "thread.manage");
    assert_eq!(state(&s, id).await, "pending");
    assert!(s.rig.seen.of_kind(CommandKind::ApprovalResolve).is_empty());
}
#[tokio::test]
async fn scoped_harness_mutation_only_returns_an_outcome_without_card_readback() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = asking(&s, thread, &[("receipt", command())]).await[0];
    let key = key(
        &s,
        s.ada.id(),
        decide_rules(&s, thread, HARNESS, "manage", false),
    )
    .await;
    let allow = json!({"option_id":"allow_once"});
    let receipt = checked(decide(&s, &key.secret, id, &allow).await, StatusCode::OK);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["approval_id"], json!(id));
    assert_eq!(receipt["state"], "approved");
    assert!(receipt.get("request").is_none());
    assert!(receipt.get("title").is_none());
    assert_eq!(state(&s, id).await, "approved");
    let late = checked(
        decide(&s, &key.secret, id, &allow).await,
        StatusCode::CONFLICT,
    );
    assert_eq!(late["code"], "approval.already_decided");
    assert!(late["current"].is_null());
}
#[tokio::test]
async fn scoped_access_decision_keeps_the_native_giving_and_requested_permissions() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = access_request(&s, thread).await;
    let key = key(
        &s,
        s.ada.id(),
        decide_rules(&s, thread, ACCESS, "manage", true),
    )
    .await;
    let shown = checked(
        decide(&s, &key.secret, id, &json!({"approve":true})).await,
        StatusCode::OK,
    );
    assert_eq!(shown["state"], "approved");
    assert!(has(&s, &s.cai, thread, Permission::ThreadInteract).await);
    assert!(!has(&s, &s.cai, thread, Permission::ThreadManage).await);
}
#[tokio::test]
async fn scoped_cancel_writes_the_requesters_outcome_and_preserves_the_full_card() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = access_request(&s, thread).await;
    let native = checked(get(&s, &s.cai.token, id).await, StatusCode::OK);
    let mut rules = read_rules(&s, thread, CANCEL);
    rules.push(rule("thread", thread, "approval", CANCEL, &["write"]));
    let key = key(&s, s.cai.id(), rules).await;
    let shown = checked(cancel(&s, &key.secret, id).await, StatusCode::OK);
    assert_eq!(shown["state"], "cancelled");
    assert_eq!(shown["request"], native["request"]);
    assert_eq!(state(&s, id).await, "cancelled");
    assert!(!has(&s, &s.cai, thread, Permission::ThreadInteract).await);
}
#[tokio::test]
async fn scoped_cancel_write_only_keeps_existing_request_content_out_of_its_receipt() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = access_request(&s, thread).await;
    let key = key(
        &s,
        s.cai.id(),
        vec![rule("thread", thread, "approval", CANCEL, &["write"])],
    )
    .await;
    let receipt = checked(cancel(&s, &key.secret, id).await, StatusCode::OK);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["approval_id"], json!(id));
    assert_eq!(receipt["state"], "cancelled");
    assert!(receipt.get("request").is_none());
    assert!(receipt.get("title").is_none());
    assert_eq!(state(&s, id).await, "cancelled");
}
#[tokio::test]
async fn scoped_cancel_cannot_borrow_manage_to_replace_the_requesting_person() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let id = access_request(&s, thread).await;
    let native = checked(cancel(&s, &s.ada.token, id).await, StatusCode::FORBIDDEN);
    assert_eq!(native["required"], "approval.requester");
    let key = key(
        &s,
        s.ada.id(),
        vec![rule("thread", thread, "approval", CANCEL, &["write"])],
    )
    .await;
    let denied = checked(cancel(&s, &key.secret, id).await, StatusCode::FORBIDDEN);
    assert_eq!(denied["required"], "approval.requester");
    assert_eq!(state(&s, id).await, "pending");
}
#[tokio::test]
async fn ordinary_harness_plan_interactor_matches_engine_eligibility_and_http_decision() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig
        .lets_use("node", thread, "thread.interact", &s.cai, json!({}))
        .await;
    assert!(has(&s, &s.cai, thread, Permission::ThreadInteract).await);
    assert!(!has(&s, &s.cai, thread, Permission::ThreadManage).await);
    let ids = asking(
        &s,
        thread,
        &[
            ("engine-plan", proposed_plan()),
            ("http-plan", proposed_plan()),
        ],
    )
    .await;
    let answer = json!({"option_id":"proceed"});
    let native: ApprovalDecision = serde_json::from_value(answer.clone()).unwrap();
    let first = s
        .rig
        .engine
        .decide_approval(&Caller::session(s.cai.id()), ids[0], &native)
        .await
        .unwrap();
    assert_eq!(first.kind, "harness");
    assert_eq!(first.state, "approved");
    let card = checked(get(&s, &s.cai.token, ids[1]).await, StatusCode::OK);
    assert_eq!(
        card["can_decide"], true,
        "the API displays native Interact eligibility"
    );
    let second = checked(
        decide(&s, &s.cai.token, ids[1], &answer).await,
        StatusCode::OK,
    );
    assert_eq!(second["state"], "approved");
}
#[tokio::test]
async fn scoped_harness_plan_uses_the_actual_shared_interact_selection() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig
        .lets_use("node", thread, "thread.interact", &s.cai, json!({}))
        .await;
    assert!(has(&s, &s.cai, thread, Permission::ThreadInteract).await);
    assert!(!has(&s, &s.cai, thread, Permission::ThreadManage).await);
    let id = asking(&s, thread, &[("key-plan", proposed_plan())]).await[0];
    let key = key(
        &s,
        s.cai.id(),
        decide_rules(&s, thread, "ApprovalDecide.plan_interact", "write", true),
    )
    .await;
    let shown = checked(
        decide(&s, &key.secret, id, &json!({"option_id":"proceed"})).await,
        StatusCode::OK,
    );
    assert_eq!(shown["state"], "approved");
    assert_eq!(shown["decided_by"]["id"], json!(s.cai.id()));
}

#[tokio::test]
async fn an_explicitly_accepted_thread_rule_keeps_its_native_lifetime_after_the_deciding_key_is_revoked()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let mut request = command();
    request["detail"]["command"] = json!("cargo test -p web");
    request["options"].as_array_mut().unwrap().insert(1,json!({"id":"allow_thread","label":"Allow for this thread","style":"allow","scope":"thread","rule":{"type":"command_prefix","argv":["cargo","test"],"cwd":"/workspace/web"}}));
    let id = asking(&s, thread, &[("keep-rule", request.clone())]).await[0];
    let key = key(
        &s,
        s.ada.id(),
        decide_rules(&s, thread, HARNESS, "manage", false),
    )
    .await;
    checked(
        decide(&s, &key.secret, id, &json!({"option_id":"allow_thread"})).await,
        StatusCode::OK,
    );
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_approval_rules WHERE thread_id=$1 AND revoked_at IS NULL",
    )
    .bind(thread)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(active, 1);
    let revoked = s
        .rig
        .world
        .app
        .call(orch_api_contract::auth::MeTokenDelete, &[&key.id])
        .token(&s.ada.token)
        .send_bytes()
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    checked(
        decide(&s, &key.secret, id, &json!({"option_id":"allow_thread"})).await,
        StatusCode::UNAUTHORIZED,
    );
    let start = s.rig.turn_start(thread, 0).await;
    s.rig
        .say(
            thread,
            Event::ApprovalRequested(ApprovalRequested {
                turn_id: start.turn_id,
                request_ref: "kept-rule-next".into(),
                title: "The same verified command".into(),
                request: serde_json::from_value(request).unwrap(),
            }),
        )
        .await;
    rig::eventually("the existing accepted thread rule answers its exact next request",||async{
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM approvals WHERE thread_id=$1 AND native_ref='kept-rule-next' AND state='approved' AND decided_by IS NULL)").bind(thread).fetch_one(s.rig.db.pool()).await.unwrap()
    }).await;
    let answer: Json = sqlx::query_scalar(
        "SELECT decision FROM approvals WHERE thread_id=$1 AND native_ref='kept-rule-next'",
    )
    .bind(thread)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(answer["option_id"], "allow_once");
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_approval_rules WHERE thread_id=$1 AND revoked_at IS NULL",
    )
    .bind(thread)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(retained, 1);
}
