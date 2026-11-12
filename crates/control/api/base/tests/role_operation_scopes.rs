//! Real operation-scoped role writes retain ordinary delegation and disclose only their own Read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use axum::body::{Body, to_bytes};
use chrono::Duration;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::{NodeRoleCreate, NodeRoleList, RoleDelete, RoleUpdate};
use orch_api_contract::common::{ErrorCode, MAX_JSON_BYTES};
use orch_api_contract::directory::{OrgGroupCreate, OrgGroupCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_db::queries::{access, hierarchy, identity, transaction_guard};
use orch_types::{Id, Json, new_id};
use places::{Who, join, make_org, make_project, person};
use serde_json::json;
use support::World;
use tower::ServiceExt;

struct Key {
    id: Id,
    secret: String,
}

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, owner: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: owner,
            name: "Role operation control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}

struct Office {
    world: World,
    owner: Who,
    recipient: Who,
    org: Id,
    project: Id,
}

impl Office {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = person(&world, "owner").await;
        let recipient = person(&world, "recipient").await;
        let org = make_org(&world, &owner, "Office", "office").await.base.id;
        join(&world, org, org, &recipient, "viewer", &owner).await;
        let project = make_project(&world, &owner, org, "Role place")
            .await
            .base
            .id;
        Self {
            world,
            owner,
            recipient,
            org,
            project,
        }
    }

    async fn key(&self, rules: Vec<Json>) -> Key {
        key(&self.world, self.owner.id(), rules).await
    }

    async fn give(&self, subject: Json, note: &str) -> Json {
        success(
            create(
                &self.world,
                &self.owner.token,
                self.project,
                json!({
                    "subject":subject,"role":"viewer","note":note
                }),
            )
            .await,
        )
    }
}

async fn create(world: &World, token: &str, node: Id, body: Json) -> RawReply {
    world
        .app
        .call(NodeRoleCreate, &[&node])
        .token(token)
        .raw_body(&body)
        .send_bytes()
        .await
}

async fn update(world: &World, token: &str, role: Id, body: Json) -> RawReply {
    world
        .app
        .call(RoleUpdate, &[&role])
        .token(token)
        .raw_body(&body)
        .send_bytes()
        .await
}

async fn delete(world: &World, token: &str, role: Id) -> RawReply {
    world
        .app
        .call(RoleDelete, &[&role])
        .token(token)
        .send_bytes()
        .await
}

fn success(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

fn id(value: &Json) -> Id {
    serde_json::from_value(value.clone()).unwrap()
}

fn receipt(value: &Json, role: Id, node: Id, revision: i64) {
    assert_eq!(
        value,
        &json!({"projection":"receipt","role_id":role,"node_id":node,"revision":revision})
    );
}

async fn stored(world: &World, role: Id) -> access::RoleRow {
    access::role_row(&mut *world.conn().await, role)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn role_create_manage_only_commits_and_returns_identifiers_without_existing_content() {
    let d = Office::new().await;
    let writer = d
        .key(vec![rule(
            "NodeRoleCreate",
            "project",
            d.project,
            &["manage"],
        )])
        .await;
    let value = success(create(&d.world, &writer.secret, d.project, json!({
        "subject":{"kind":"user","id":d.recipient.id()},"role":"viewer","note":"new caller content"
    })).await);
    let role = id(&value["role_id"]);
    let row = stored(&d.world, role).await;
    receipt(&value, role, d.project, row.revision);
    assert_eq!(
        (row.node_id, row.subject_id, row.created_by),
        (d.project, d.recipient.id(), d.owner.id())
    );
    assert_eq!(row.note, "new caller content");
    assert_eq!(row.role, "viewer");
}

#[tokio::test]
async fn role_update_manage_only_commits_but_success_and_stale_reply_do_not_disclose_old_role_content()
 {
    let d = Office::new().await;
    let given = d
        .give(
            json!({"kind":"user","id":d.recipient.id()}),
            "existing private note",
        )
        .await;
    let role = id(&given["id"]);
    let revision = given["revision"].as_i64().unwrap();
    let writer = d
        .key(vec![rule("RoleUpdate", "project", d.project, &["manage"])])
        .await;
    let value = success(
        update(
            &d.world,
            &writer.secret,
            role,
            json!({"expected_revision":revision,"note":"changed note"}),
        )
        .await,
    );
    receipt(&value, role, d.project, revision + 1);
    assert_eq!(stored(&d.world, role).await.note, "changed note");
    let stale = update(
        &d.world,
        &writer.secret,
        role,
        json!({"expected_revision":revision,"note":"must not commit"}),
    )
    .await;
    assert_eq!(stale.problem().code, ErrorCode::ConflictStaleRevision);
    assert!(!String::from_utf8_lossy(&stale.raw).contains("changed note"));
    assert!(stale.problem().extra.get("current").is_none());
    assert_eq!(stored(&d.world, role).await.note, "changed note");
}

#[tokio::test]
async fn role_delete_uses_the_stored_role_place_and_needs_no_response_read() {
    let d = Office::new().await;
    let given = d
        .give(json!({"kind":"user","id":d.recipient.id()}), "delete me")
        .await;
    let role = id(&given["id"]);
    let other = make_project(&d.world, &d.owner, d.org, "Other role place")
        .await
        .base
        .id;
    let foreign = d
        .key(vec![rule("RoleDelete", "project", other, &["manage"])])
        .await;
    assert_eq!(
        delete(&d.world, &foreign.secret, role).await.problem().code,
        ErrorCode::AuthzDenied
    );
    let unrelated = d
        .key(vec![rule("RoleUpdate", "project", d.project, &["manage"])])
        .await;
    assert_eq!(
        delete(&d.world, &unrelated.secret, role)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(stored(&d.world, role).await.note, "delete me");
    let writer = d
        .key(vec![rule("RoleDelete", "project", d.project, &["manage"])])
        .await;
    let gone = delete(&d.world, &writer.secret, role).await;
    assert_eq!(
        gone.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&gone.raw)
    );
    assert!(gone.raw.is_empty());
    assert!(
        access::role_row(&mut *d.world.conn().await, role)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn full_role_group_content_requires_the_same_operation_reads_and_missing_sources_keep_the_mutation()
 {
    let d = Office::new().await;
    let group = d
        .world
        .app
        .call(OrgGroupCreate, &[&d.org])
        .token(&d.owner.token)
        .body(&OrgGroupCreateBody {
            name: "Secret group title".into(),
            description: Some("Secret group description".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    let writer = d
        .key(vec![
            rule("NodeRoleCreate", "project", d.project, &["manage", "read"]),
            rule("NodeRoleCreate", "group", group.id, &["read"]),
        ])
        .await;
    let full = success(create(&d.world, &writer.secret, d.project, json!({
        "subject":{"kind":"group","id":group.id},"role":"viewer","note":"original group role note"
    })).await);
    assert_eq!(full["subject"]["group"]["name"], "Secret group title");
    assert_eq!(
        full["subject"]["group"]["description"],
        "Secret group description"
    );
    let role = id(&full["id"]);
    let revision = full["revision"].as_i64().unwrap();
    let unrelated_read = d
        .key(vec![
            rule("RoleUpdate", "project", d.project, &["manage", "read"]),
            rule("NodeRoleCreate", "group", group.id, &["read"]),
        ])
        .await;
    let reduced = success(
        update(
            &d.world,
            &unrelated_read.secret,
            role,
            json!({"expected_revision":revision,"note":"updated group role note"}),
        )
        .await,
    );
    receipt(&reduced, role, d.project, revision + 1);
    assert_eq!(stored(&d.world, role).await.note, "updated group role note");
    let reader = d
        .key(vec![
            rule("RoleUpdate", "project", d.project, &["manage", "read"]),
            rule("RoleUpdate", "group", group.id, &["read"]),
        ])
        .await;
    let stale = update(
        &d.world,
        &reader.secret,
        role,
        json!({"expected_revision":revision,"note":"not applied"}),
    )
    .await;
    assert_eq!(stale.problem().code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        stale.problem().extra["current"]["subject"]["group"]["name"],
        "Secret group title"
    );
    let hidden_stale = update(
        &d.world,
        &unrelated_read.secret,
        role,
        json!({"expected_revision":revision,"note":"not applied"}),
    )
    .await;
    assert_eq!(
        hidden_stale.problem().code,
        ErrorCode::ConflictStaleRevision
    );
    assert!(hidden_stale.problem().extra.get("current").is_none());
    let full = success(
        update(
            &d.world,
            &reader.secret,
            role,
            json!({"expected_revision":revision + 1,"note":"full result"}),
        )
        .await,
    );
    assert_eq!(full["subject"]["group"]["name"], "Secret group title");
    assert_eq!(full["note"], "full result");
}

#[tokio::test]
async fn role_reads_remain_independent_from_manage_and_unrelated_target_rules_do_not_combine() {
    let d = Office::new().await;
    let other = make_project(&d.world, &d.owner, d.org, "Other")
        .await
        .base
        .id;
    let reader = d
        .key(vec![rule(
            "NodeRoleCreate",
            "project",
            d.project,
            &["read"],
        )])
        .await;
    let body = json!({"subject":{"kind":"user","id":d.recipient.id()},"role":"viewer"});
    assert_eq!(
        create(&d.world, &reader.secret, d.project, body.clone())
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let crossed = d
        .key(vec![
            rule("NodeRoleCreate", "project", d.project, &["read"]),
            rule("NodeRoleCreate", "project", other, &["manage"]),
        ])
        .await;
    assert_eq!(
        create(&d.world, &crossed.secret, d.project, body.clone())
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let writer = d
        .key(vec![
            rule("NodeRoleCreate", "project", d.project, &["manage"]),
            rule("PermissionPageGet.view", "project", d.project, &["read"]),
        ])
        .await;
    let value = success(create(&d.world, &writer.secret, d.project, body).await);
    let role = id(&value["role_id"]);
    receipt(
        &value,
        role,
        d.project,
        stored(&d.world, role).await.revision,
    );
}

#[tokio::test]
async fn ordinary_original_giver_bound_admin_and_domain_role_safeguards_remain_current() {
    let d = Office::new().await;
    let giver = person(&d.world, "giver").await;
    join(&d.world, d.org, d.org, &giver, "developer", &d.owner).await;
    let giver_key = key(
        &d.world,
        giver.id(),
        vec![
            rule("NodeRoleCreate", "project", d.project, &["manage", "read"]),
            rule("RoleUpdate", "project", d.project, &["manage"]),
            rule("RoleDelete", "project", d.project, &["manage"]),
        ],
    )
    .await;
    let given = success(
        create(
            &d.world,
            &giver_key.secret,
            d.project,
            json!({
                "subject":{"kind":"user","id":d.recipient.id()},"role":"interactor"
            }),
        )
        .await,
    );
    assert_eq!(given["created_by"]["id"], json!(giver.id()));
    let role = id(&given["id"]);
    let revision = given["revision"].as_i64().unwrap();
    assert_eq!(
        update(
            &d.world,
            &giver_key.secret,
            role,
            json!({"expected_revision":revision,"role":"admin"})
        )
        .await
        .problem()
        .code,
        ErrorCode::AuthzDenied
    );
    let listed = d
        .world
        .app
        .call(NodeRoleList, &[&d.org])
        .token(&d.owner.token)
        .send()
        .await
        .ok();
    let domain = listed
        .own
        .iter()
        .find(|row| row.subject.id == giver.id())
        .unwrap();
    success(
        update(
            &d.world,
            &d.owner.token,
            domain.id,
            json!({"expected_revision":domain.revision,"role":"viewer"}),
        )
        .await,
    );
    assert_eq!(
        update(
            &d.world,
            &giver_key.secret,
            role,
            json!({"expected_revision":revision,"note":"no longer giveable"})
        )
        .await
        .problem()
        .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        delete(&d.world, &giver_key.secret, role)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let manager = d
        .key(vec![rule("RoleDelete", "project", d.project, &["manage"])])
        .await;
    assert_eq!(
        delete(&d.world, &manager.secret, role).await.status,
        StatusCode::NO_CONTENT
    );
    let org_manager = d
        .key(vec![rule("RoleDelete", "org", d.org, &["manage"])])
        .await;
    assert_eq!(
        delete(&d.world, &org_manager.secret, domain.id)
            .await
            .problem()
            .code,
        ErrorCode::MemberDomainRole
    );
}

#[tokio::test]
async fn existing_role_scope_follows_current_node_ancestry_and_exact_identity() {
    let d = Office::new().await;
    let destination = make_project(&d.world, &d.owner, d.org, "New ancestor")
        .await
        .base
        .id;
    let child = make_project(&d.world, &d.owner, d.org, "Moving role place")
        .await
        .base
        .id;
    hierarchy::move_under(&mut *d.world.conn().await, child, d.project, d.world.now())
        .await
        .unwrap();
    let given = success(
        create(
            &d.world,
            &d.owner.token,
            child,
            json!({
                "subject":{"kind":"user","id":d.recipient.id()},"role":"viewer"
            }),
        )
        .await,
    );
    let role = id(&given["id"]);
    let revision = given["revision"].as_i64().unwrap();
    let old = d.key(vec![json!({"target":{"kind":"project","id":d.project,"reach":"subtree"},"allow":[{"operation":"RoleUpdate","actions":["manage"]}]})]).await;
    let exact = d
        .key(vec![rule("RoleUpdate", "project", child, &["manage"])])
        .await;
    hierarchy::move_under(
        &mut *d.world.conn().await,
        child,
        destination,
        d.world.now(),
    )
    .await
    .unwrap();
    assert_eq!(
        update(
            &d.world,
            &old.secret,
            role,
            json!({"expected_revision":revision,"note":"old ancestry"})
        )
        .await
        .problem()
        .code,
        ErrorCode::AuthzDenied
    );
    let value = success(
        update(
            &d.world,
            &exact.secret,
            role,
            json!({"expected_revision":revision,"note":"exact object"}),
        )
        .await,
    );
    receipt(&value, role, child, revision + 1);
    assert_eq!(stored(&d.world, role).await.note, "exact object");
}

async fn guarded_update(narrow: bool) {
    let d = Office::new().await;
    let given = d
        .give(json!({"kind":"user","id":d.recipient.id()}), "before guard")
        .await;
    let role = id(&given["id"]);
    let revision = given["revision"].as_i64().unwrap();
    let writer = d
        .key(vec![rule("RoleUpdate", "project", d.project, &["manage"])])
        .await;
    let mut blocker = d.world.db.pool.begin().await.unwrap();
    transaction_guard::lock_person(&mut blocker, d.owner.id())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(writer.id)
        .bind(d.owner.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(d.world.app.app.clone());
    let request = Request::builder()
        .method("PATCH")
        .uri(format!("/roles/{role}"))
        .header("authorization", format!("Bearer {}", writer.secret))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"expected_revision":revision,"note":"after guard"}).to_string(),
        ))
        .unwrap();
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            if action.is_finished() { break; }
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&d.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the real action reaches its existing credential guard or answers");
    if narrow {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1 AND user_id=$3")
            .bind(writer.id)
            .bind(serde_json::to_value(KeyScope::default()).unwrap())
            .bind(d.owner.id())
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(wait, action).await.unwrap().unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    if narrow {
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(stored(&d.world, role).await.note, "before guard");
    } else {
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        receipt(
            &serde_json::from_slice(&bytes).unwrap(),
            role,
            d.project,
            revision + 1,
        );
        assert_eq!(stored(&d.world, role).await.note, "after guard");
    }
}

#[tokio::test]
async fn role_update_rechecks_current_credential_scope_at_the_existing_action_guard() {
    guarded_update(false).await;
    guarded_update(true).await;
}
