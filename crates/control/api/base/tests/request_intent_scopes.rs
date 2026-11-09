//! Requests are accepted questions, governed by the actual things asked about.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "controls fail by panicking"
)]
mod chat_world;
#[path = "request_intent_scopes/chats.rs"]
mod chats;
#[path = "request_intent_scopes/checkpoints.rs"]
mod checkpoints;
mod desk;
mod places;
#[path = "request_intent_scopes/privacy.rs"]
mod privacy;
mod support;

use desk::Desk;
use http::StatusCode;
use orch_access_contract::{key_scope::KeyScope, permissions::Permission};
use orch_api_contract::access::{
    AccessRequestCreate, AccessRequestCreateBody, AccessRequestGet, AccessRequestItem,
    AccessRequestObject, HolderInput,
};
use orch_api_contract::approvals::{ApprovalCancel, ApprovalDecide};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::{Caller, Policy, Principal};
use orch_db::queries::identity;
use orch_types::{Id, Json, Time, new_id};
use places::Who;
use serde_json::json;
use support::FullPermissionPageReply;
use support::World;

const CREATE: &str = "AccessRequestCreate";
const GET: &str = "AccessRequestGet";
const JOIN: &str = "ChatJoinRequest";
const REASON: &str = "literal access request reason stays intact";
const DOCUMENT: &str = "Actual requested document title";
struct Key {
    id: Id,
    secret: String,
}
async fn key(world: &World, actor: Id, rules: Vec<Json>) -> Key {
    let policy: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(policy.validate().is_empty());
    let (id, secret) = (new_id(), new_token(API_TOKEN_PREFIX));
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Request intent control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(policy).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn legacy(world: &World, actor: Id) -> Key {
    let (id, secret) = (new_id(), new_token(API_TOKEN_PREFIX));
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Native request compatibility",
            token_hash: &token_hash(&secret),
            scopes: &["*".into()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn rule(kind: &str, id: Id, resource: &str, operation: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[resource],"allow":[{"operation":operation,"actions":actions}]})
}
fn full(org: Id, operation: &str, actions: &[&str]) -> Json {
    // The existing omitted resource filter covers the declared sources within this real org.
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},"allow":[{"operation":operation,"actions":actions}]})
}
fn checked(reply: RawReply, expected: StatusCode) -> Json {
    let body: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(reply.status, expected, "{}", body);
    body
}
fn id(value: &Json) -> Id {
    value.as_str().unwrap().parse().unwrap()
}
fn item(kind: &str, id: Id, permission: &str) -> AccessRequestItem {
    AccessRequestItem {
        object: AccessRequestObject {
            kind: kind.into(),
            id,
        },
        permissions: vec![permission.into()],
        options: None,
        holder: None,
        label: Some("literal caller-supplied object label".into()),
    }
}
fn body(items: Vec<AccessRequestItem>) -> AccessRequestCreateBody {
    AccessRequestCreateBody {
        items,
        reason: REASON.into(),
        expires_at: None,
        for_thread: None,
    }
}
async fn create(world: &World, token: &str, body: &AccessRequestCreateBody) -> RawReply {
    world
        .app
        .call(AccessRequestCreate, &[])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
async fn get(world: &World, token: &str, batch: Id) -> RawReply {
    world
        .app
        .call(AccessRequestGet, &[&batch])
        .token(token)
        .send_bytes()
        .await
}
async fn cancel(world: &World, token: &str, cards: &Json) {
    for card in cards.as_array().unwrap() {
        let request = world
            .app
            .call(ApprovalCancel, &[&id(&card["id"])])
            .token(token);
        checked(request.send_bytes().await, StatusCode::OK);
    }
}
async fn revoke(world: &World, actor: Id, key: &Key) {
    assert!(
        identity::revoke_api_token(&mut *world.conn().await, key.id, actor, world.now())
            .await
            .unwrap()
    );
}
async fn policy(world: &World, key: &Key, rules: Vec<Json>) {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(serde_json::to_value(scope).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
}
async fn footprint(world: &World, actor: Id) -> (i64, i64, i64) {
    let mut conn = world.conn().await;
    let approvals = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE requester_user_id=$1")
        .bind(actor)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let audit = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE actor_id=$1 AND action='access.requested'",
    )
    .bind(actor)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let notices =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind='access.requested'")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    (approvals, audit, notices)
}
async fn can_document(world: &World, actor: Id, document: Id, permission: Permission) -> bool {
    let mut conn = world.conn().await;
    let principal = Principal::load(&mut conn, Caller::session(actor), Time::new(world.now()))
        .await
        .unwrap();
    orch_authz::documents::check(
        &mut conn,
        &principal,
        document,
        permission,
        Policy::default(),
    )
    .await
    .is_ok_and(|decision| decision.is_allowed())
}
struct Office {
    desk: Desk,
    requester: Who,
    document: Id,
}
impl Office {
    async fn new() -> Self {
        let desk = Desk::new().await;
        let requester = desk.stranger("requester").await;
        let document = desk.make(&desk.ada, DOCUMENT).await.base.id;
        assert!(!can_document(&desk.world, requester.id(), document, Permission::DocView).await);
        Self {
            desk,
            requester,
            document,
        }
    }
    fn mixed(&self) -> AccessRequestCreateBody {
        body(vec![
            item("topic", self.desk.topic, "topic.interact"),
            item("document", self.document, "doc.view"),
        ])
    }
    fn document(&self) -> AccessRequestCreateBody {
        body(vec![item("document", self.document, "doc.view")])
    }
    fn writes(&self) -> Vec<Json> {
        vec![
            rule("topic", self.desk.topic, "topic", CREATE, &["write"]),
            rule("document", self.document, "document", CREATE, &["write"]),
        ]
    }
    async fn native(&self, body: &AccessRequestCreateBody) -> Json {
        checked(
            create(&self.desk.world, &self.requester.token, body).await,
            StatusCode::OK,
        )
    }
}

#[tokio::test]
async fn modern_create_and_get_reuse_mixed_native_requests_without_requiring_existing_view() {
    let s = Office::new().await;
    let native = s.native(&s.mixed()).await;
    assert_eq!(native["approvals"].as_array().unwrap().len(), 2);
    let original = checked(
        get(&s.desk.world, &s.requester.token, id(&native["batch_id"])).await,
        StatusCode::OK,
    );
    assert_eq!(original["approvals"], native["approvals"]);
    cancel(&s.desk.world, &s.requester.token, &native["approvals"]).await;
    let old = legacy(&s.desk.world, s.requester.id()).await;
    let native_old = checked(
        create(&s.desk.world, &old.secret, &s.mixed()).await,
        StatusCode::OK,
    );
    cancel(&s.desk.world, &s.requester.token, &native_old["approvals"]).await;
    let key = key(
        &s.desk.world,
        s.requester.id(),
        vec![
            full(s.desk.org, CREATE, &["read", "write", "manage"]),
            full(s.desk.org, GET, &["read"]),
        ],
    )
    .await;
    let made = checked(
        create(&s.desk.world, &key.secret, &s.mixed()).await,
        StatusCode::OK,
    );
    assert!(made.get("projection").is_none());
    assert_eq!(made["approvals"].as_array().unwrap().len(), 2);
    for card in made["approvals"].as_array().unwrap() {
        assert_eq!(card["request"]["reason"], REASON);
    }
    let current = checked(
        get(&s.desk.world, &key.secret, id(&made["batch_id"])).await,
        StatusCode::OK,
    );
    assert_eq!(current["approvals"], made["approvals"]);
    assert_eq!(current["complete"], false);
    assert!(
        !can_document(
            &s.desk.world,
            s.requester.id(),
            s.document,
            Permission::DocView
        )
        .await
    );
}

#[tokio::test]
async fn one_unselected_actual_batch_member_refuses_the_whole_new_request() {
    let s = Office::new().await;
    let native = s.native(&s.mixed()).await;
    cancel(&s.desk.world, &s.requester.token, &native["approvals"]).await;
    let before = footprint(&s.desk.world, s.requester.id()).await;
    let key = key(
        &s.desk.world,
        s.requester.id(),
        vec![rule("topic", s.desk.topic, "topic", CREATE, &["write"])],
    )
    .await;
    checked(
        create(&s.desk.world, &key.secret, &s.mixed()).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(footprint(&s.desk.world, s.requester.id()).await, before);
}

#[tokio::test]
async fn write_only_request_receipt_contains_actual_accepted_ids_without_old_card_content() {
    let s = Office::new().await;
    let key = key(&s.desk.world, s.requester.id(), s.writes()).await;
    let made = checked(
        create(&s.desk.world, &key.secret, &s.mixed()).await,
        StatusCode::OK,
    );
    assert_eq!(made["projection"], "receipt");
    assert!(made.get("approvals").is_none());
    let batch = id(&made["batch_id"]);
    let saved: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM approvals WHERE batch_id=$1 AND requester_user_id=$2 ORDER BY id",
    )
    .bind(batch)
    .bind(s.requester.id())
    .fetch_all(&s.desk.world.db.pool)
    .await
    .unwrap();
    let mut returned: Vec<Id> = made["approval_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(id)
        .collect();
    returned.sort();
    assert_eq!(returned, saved);
    assert_eq!(saved.len(), 2);
    let wire = made.to_string();
    assert!(!wire.contains(DOCUMENT) && !wire.contains(REASON));
    assert!(
        !can_document(
            &s.desk.world,
            s.requester.id(),
            s.document,
            Permission::DocView
        )
        .await
    );
}

#[tokio::test]
async fn another_operations_read_does_not_disclose_the_create_reply() {
    let s = Office::new().await;
    let mut rules = s.writes();
    rules.push(full(s.desk.org, GET, &["read"]));
    let key = key(&s.desk.world, s.requester.id(), rules).await;
    let made = checked(
        create(&s.desk.world, &key.secret, &s.document()).await,
        StatusCode::OK,
    );
    assert_eq!(made["projection"], "receipt");
    let saved = checked(
        get(&s.desk.world, &key.secret, id(&made["batch_id"])).await,
        StatusCode::OK,
    );
    cancel(&s.desk.world, &s.requester.token, &saved["approvals"]).await;
    policy(
        &s.desk.world,
        &key,
        vec![full(s.desk.org, CREATE, &["read", "write", "manage"])],
    )
    .await;
    let shown = checked(
        create(&s.desk.world, &key.secret, &s.document()).await,
        StatusCode::OK,
    );
    assert!(shown.get("projection").is_none());
    assert_eq!(shown["approvals"][0]["request"]["reason"], REASON);
}

#[tokio::test]
async fn get_requires_every_actual_batch_governor_without_fabricating_partial_completion() {
    let s = Office::new().await;
    let native = s.native(&s.mixed()).await;
    let key = key(
        &s.desk.world,
        s.requester.id(),
        vec![rule("topic", s.desk.topic, "approval", GET, &["read"])],
    )
    .await;
    checked(
        get(&s.desk.world, &key.secret, id(&native["batch_id"])).await,
        StatusCode::FORBIDDEN,
    );
    policy(&s.desk.world, &key, vec![full(s.desk.org, GET, &["read"])]).await;
    let shown = checked(
        get(&s.desk.world, &key.secret, id(&native["batch_id"])).await,
        StatusCode::OK,
    );
    assert_eq!(shown["approvals"].as_array().unwrap().len(), 2);
    assert_eq!(shown["complete"], false);
}

#[tokio::test]
async fn a_richer_key_cannot_read_another_persons_request_batch() {
    let s = Office::new().await;
    let native = s.native(&s.document()).await;
    let key = key(
        &s.desk.world,
        s.desk.ada.id(),
        vec![full(s.desk.org, GET, &["read"])],
    )
    .await;
    let native_denied = checked(
        get(&s.desk.world, &s.desk.ada.token, id(&native["batch_id"])).await,
        StatusCode::NOT_FOUND,
    );
    let modern = checked(
        get(&s.desk.world, &key.secret, id(&native["batch_id"])).await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(modern["code"], native_denied["code"]);
}

#[tokio::test]
async fn accepting_a_question_does_not_tie_the_later_independent_grant_to_that_key() {
    let s = Office::new().await;
    let key = key(
        &s.desk.world,
        s.requester.id(),
        vec![full(s.desk.org, CREATE, &["read", "write", "manage"])],
    )
    .await;
    let made = checked(
        create(&s.desk.world, &key.secret, &s.document()).await,
        StatusCode::OK,
    );
    let approval = id(&made["approvals"][0]["id"]);
    let batch = id(&made["batch_id"]);
    revoke(&s.desk.world, s.requester.id(), &key).await;
    checked(
        get(&s.desk.world, &key.secret, batch).await,
        StatusCode::UNAUTHORIZED,
    );
    checked(
        s.desk
            .world
            .app
            .call(ApprovalDecide, &[&approval])
            .token(&s.desk.ada.token)
            .raw_body(&json!({"approve":true}))
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert!(
        can_document(
            &s.desk.world,
            s.requester.id(),
            s.document,
            Permission::DocView
        )
        .await
    );
    let current = checked(
        get(&s.desk.world, &s.requester.token, batch).await,
        StatusCode::OK,
    );
    assert_eq!(current["complete"], true);
}

async fn can_node(world: &World, actor: Id, node: Id, permission: Permission) -> bool {
    let mut conn = world.conn().await;
    let principal = Principal::load(&mut conn, Caller::session(actor), Time::new(world.now()))
        .await
        .unwrap();
    orch_authz::nodes::check(
        &mut conn,
        &principal,
        node,
        permission,
        orch_authz::Act::Use,
        Policy::default(),
    )
    .await
    .is_ok_and(|decision| decision.is_allowed())
}
async fn work_request(s: &Office) -> (Who, Id, AccessRequestCreateBody) {
    work_request_at(s, None).await
}
async fn work_request_at(
    s: &Office,
    expires_at: Option<Time>,
) -> (Who, Id, AccessRequestCreateBody) {
    use orch_api_contract::access::{
        NewResourceAccess, PageChange, PageChangeAdd, PageChangeRow, PermissionPageUpdate,
    };
    use orch_api_contract::resources::{McpServerCreate, McpServerCreateBody};
    let manager = s.desk.member("workmanager", "viewer").await;
    let server = s
        .desk
        .world
        .app
        .call(McpServerCreate, &[])
        .token(&s.desk.ada.token)
        .body(&McpServerCreateBody {
            name: "request-tools".into(),
            description: None,
            home_id: Some(s.desk.org),
            spec: json!({"transport":"stdio","command":["tool-server"],"secret_env":[]}),
            secrets: None,
            access: Some(NewResourceAccess::OnlyMe),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    s.desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &s.desk.topic])
        .token(&s.desk.ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "access.manage".into(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::User { id: manager.id() },
                    options: None,
                    expires_at,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .full_permission_page()
        .ok();
    assert!(
        can_node(
            &s.desk.world,
            manager.id(),
            s.desk.topic,
            Permission::AccessManage
        )
        .await
    );
    let mut request = item("mcp_server", server.id, "mcp.use");
    request.holder = Some(HolderInput::Node { id: s.desk.topic });
    (manager, server.id, body(vec![request]))
}

#[tokio::test]
async fn selected_work_keeps_both_the_requested_resource_and_its_native_manager_boundary() {
    let s = Office::new().await;
    let (manager, server, body) = work_request(&s).await;
    let native = checked(
        create(&s.desk.world, &manager.token, &body).await,
        StatusCode::OK,
    );
    cancel(&s.desk.world, &manager.token, &native["approvals"]).await;
    let key = key(
        &s.desk.world,
        manager.id(),
        vec![rule("mcp_server", server, "mcp_server", CREATE, &["write"])],
    )
    .await;
    let before = footprint(&s.desk.world, manager.id()).await;
    checked(
        create(&s.desk.world, &key.secret, &body).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(footprint(&s.desk.world, manager.id()).await, before);
    policy(
        &s.desk.world,
        &key,
        vec![
            rule("mcp_server", server, "mcp_server", CREATE, &["write"]),
            rule("topic", s.desk.topic, "topic", CREATE, &["manage"]),
            full(s.desk.org, CREATE, &["read"]),
        ],
    )
    .await;
    let made = checked(
        create(&s.desk.world, &key.secret, &body).await,
        StatusCode::OK,
    );
    assert_eq!(
        made["approvals"][0]["request"]["holder"]["node"]["id"],
        json!(s.desk.topic)
    );
}

#[tokio::test]
async fn scope_cannot_lend_the_existing_work_holder_manager_privilege_to_a_stranger() {
    let s = Office::new().await;
    let (_, _, body) = work_request(&s).await;
    assert!(
        !can_node(
            &s.desk.world,
            s.requester.id(),
            s.desk.topic,
            Permission::AccessManage
        )
        .await
    );
    let native = checked(
        create(&s.desk.world, &s.requester.token, &body).await,
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(native["code"], "request.invalid");
    let key = key(
        &s.desk.world,
        s.requester.id(),
        vec![full(s.desk.org, CREATE, &["read", "write", "manage"])],
    )
    .await;
    let denied = checked(
        create(&s.desk.world, &key.secret, &body).await,
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(denied["code"], native["code"]);
    assert_eq!(footprint(&s.desk.world, s.requester.id()).await.0, 0);
}
