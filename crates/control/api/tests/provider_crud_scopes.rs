//! Provider reads and ordinary mutations use exact operation scopes and their real HTTP writers.
//! Raw replies keep these controls compatible with the baseline before typed receipts are added.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    ProviderDelete, ProviderGet, ProviderUpdate, ProviderUpdateBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity;
use orch_db::queries::resources::{access_fixtures as fixtures, routes};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const PRIVATE_NAME: &str = "Private provider metadata sentinel";
const CHANGED_NAME: &str = "Changed private provider metadata sentinel";

async fn provider(world: &World, company: &resources::Company, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(company.boss.id()),
        Some(company.acme),
        name,
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id,
        },
        company.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}

async fn key(world: &World, user: Id, provider: Id, allow: Json) -> String {
    let scope: orch_access_contract::key_scope::KeyScope = serde_json::from_value(json!({
        "rules":[{"target":{"kind":"provider","id":provider,"reach":"self"},"allow":allow}]
    }))
    .unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Provider CRUD operation control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            // The existing fixed-clock fixture needs only its ordinary one-hour key lifetime.
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

fn change(revision: i64, name: Option<&str>) -> ProviderUpdateBody {
    ProviderUpdateBody {
        name: name.map(str::to_owned),
        upstreams: None,
        config: None,
        arrangements: None,
        runtime_config: None,
        expected_revision: revision,
    }
}

async fn update(world: &World, token: &str, provider: Id, body: &ProviderUpdateBody) -> RawReply {
    world
        .app
        .call(ProviderUpdate, &[&provider])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}

fn success(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

fn receipt(reply: &RawReply, provider: Id, revision: i64) {
    assert_eq!(
        success(reply),
        json!({"projection":"receipt","provider_id":provider,"revision":revision})
    );
}

fn denied(reply: &RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}

fn no_private_content(reply: &RawReply) {
    let text = String::from_utf8_lossy(&reply.raw);
    assert!(
        !text.contains(PRIVATE_NAME) && !text.contains(CHANGED_NAME),
        "{text}"
    );
    assert!(
        !text.contains("api.test"),
        "the private endpoint was disclosed: {text}"
    );
    let problem = reply.problem();
    assert!(
        !problem.extra.contains_key("current"),
        "a stale reply disclosed current content"
    );
}

async fn revision(world: &World, id: Id) -> i64 {
    sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn snapshot(world: &World, id: Id) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('provider',to_jsonb(p),
        'audits',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.at,a.id) FROM audit_log a WHERE a.object_kind='provider' AND a.object_id=p.id))
        FROM providers p WHERE p.id=$1")
        .bind(id).fetch_one(&world.db.pool).await.unwrap()
}

#[tokio::test]
async fn provider_get_exact_read_returns_the_current_provider_without_granting_update() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    let read = key(
        &world,
        company.boss.id(),
        id,
        json!([
            {"operation":"ProviderGet","actions":["read"]}
        ]),
    )
    .await;
    let reply = world
        .app
        .call(ProviderGet, &[&id])
        .token(&read)
        .send_bytes()
        .await;
    let shown = success(&reply);
    assert_eq!(shown["id"], json!(id));
    assert_eq!(shown["name"], PRIVATE_NAME);
    assert_eq!(shown["revision"], revision(&world, id).await);
    assert!(shown.get("projection").is_none());
    assert!(
        shown.get("status").is_some(),
        "this is the ordinary Provider representation"
    );
    let before = snapshot(&world, id).await;
    denied(
        &update(
            &world,
            &read,
            id,
            &change(revision(&world, id).await, Some(CHANGED_NAME)),
        )
        .await,
    );
    assert_eq!(snapshot(&world, id).await, before);
}

#[tokio::test]
async fn provider_get_read_scope_does_not_cross_targets_or_grant_account_visibility() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    let other = provider(&world, &company, "Other private provider").await;
    let allow = json!([{"operation":"ProviderGet","actions":["read"]}]);
    let exact = key(&world, company.boss.id(), id, allow.clone()).await;
    denied(
        &world
            .app
            .call(ProviderGet, &[&other])
            .token(&exact)
            .send_bytes()
            .await,
    );
    let powerless = key(&world, company.viewer.id(), id, allow).await;
    let reply = world
        .app
        .call(ProviderGet, &[&id])
        .token(&powerless)
        .send_bytes()
        .await;
    denied(&reply);
    assert!(!String::from_utf8_lossy(&reply.raw).contains(PRIVATE_NAME));
}

#[tokio::test]
async fn provider_update_manage_only_returns_an_exact_receipt_and_noop_keeps_revision_and_audit() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    let manage = key(
        &world,
        company.boss.id(),
        id,
        json!([
            {"operation":"ProviderUpdate","actions":["manage"]}
        ]),
    )
    .await;
    let original = revision(&world, id).await;
    let written = update(&world, &manage, id, &change(original, Some(CHANGED_NAME))).await;
    let current = revision(&world, id).await;
    receipt(&written, id, current);
    assert!(current > original);
    let stored: String = sqlx::query_scalar("SELECT name FROM providers WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, CHANGED_NAME);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='provider.updated'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1, "the ordinary writer records one mutation");
    let before = snapshot(&world, id).await;
    let noop = update(&world, &manage, id, &change(current, None)).await;
    receipt(&noop, id, current);
    assert_eq!(snapshot(&world, id).await, before);
    denied(
        &world
            .app
            .call(ProviderGet, &[&id])
            .token(&manage)
            .send_bytes()
            .await,
    );
}

#[tokio::test]
async fn provider_update_manage_only_stale_and_stale_noop_disclose_no_current_provider() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    let original = revision(&world, id).await;
    success(
        &update(
            &world,
            &company.boss.token,
            id,
            &change(original, Some(CHANGED_NAME)),
        )
        .await,
    );
    let manage = key(
        &world,
        company.boss.id(),
        id,
        json!([
            {"operation":"ProviderUpdate","actions":["manage"]}
        ]),
    )
    .await;
    let before = snapshot(&world, id).await;
    for name in [Some("Rejected stale rename"), None] {
        let stale = update(&world, &manage, id, &change(original, name)).await;
        assert_eq!(stale.status, StatusCode::CONFLICT);
        assert_eq!(stale.problem().code, ErrorCode::ConflictStaleRevision);
        no_private_content(&stale);
        assert_eq!(snapshot(&world, id).await, before);
    }
}

#[tokio::test]
async fn provider_update_same_operation_read_keeps_full_reply_and_stale_current_projection() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    let full = key(
        &world,
        company.boss.id(),
        id,
        json!([
            {"operation":"ProviderUpdate","actions":["manage","read"]}
        ]),
    )
    .await;
    let original = revision(&world, id).await;
    let reply = update(&world, &full, id, &change(original, Some(CHANGED_NAME))).await;
    let shown = success(&reply);
    assert_eq!(shown["id"], json!(id));
    assert_eq!(shown["name"], CHANGED_NAME);
    assert!(!shown["upstreams"].as_array().unwrap().is_empty());
    assert!(shown.get("projection").is_none());
    assert!(
        shown.get("provider").is_none(),
        "the full ProviderWriteReply remains flattened"
    );
    let current = revision(&world, id).await;
    assert_eq!(shown["revision"], current);
    let before = snapshot(&world, id).await;
    let noop = success(&update(&world, &full, id, &change(current, None)).await);
    assert_eq!(noop["id"], json!(id));
    assert_eq!(noop["revision"], current);
    assert!(noop.get("projection").is_none());
    let stale = update(
        &world,
        &full,
        id,
        &change(original, Some("Rejected stale rename")),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    let problem = stale.problem();
    assert_eq!(problem.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(problem.extra["current"]["id"], json!(id));
    assert_eq!(problem.extra["current"]["name"], CHANGED_NAME);
    assert_eq!(problem.extra["current"]["revision"], current);
    assert_eq!(snapshot(&world, id).await, before);
}

#[tokio::test]
async fn provider_get_read_cannot_supply_provider_update_response_disclosure() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    let split = key(
        &world,
        company.boss.id(),
        id,
        json!([
            {"operation":"ProviderUpdate","actions":["manage"]},
            {"operation":"ProviderGet","actions":["read"]}
        ]),
    )
    .await;
    let original = revision(&world, id).await;
    let reply = update(&world, &split, id, &change(original, Some(CHANGED_NAME))).await;
    receipt(&reply, id, revision(&world, id).await);
    let stale = update(&world, &split, id, &change(original, None)).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.problem().code, ErrorCode::ConflictStaleRevision);
    no_private_content(&stale);
}

#[tokio::test]
async fn provider_update_scope_cannot_cross_targets_or_replace_ordinary_manage_or_borrow_as_admin()
{
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    let other = provider(&world, &company, "Other private provider").await;
    let allow = json!([{"operation":"ProviderUpdate","actions":["manage","read"]}]);
    let exact = key(&world, company.boss.id(), id, allow.clone()).await;
    let before = snapshot(&world, other).await;
    denied(
        &update(
            &world,
            &exact,
            other,
            &change(revision(&world, other).await, Some("Rejected other rename")),
        )
        .await,
    );
    assert_eq!(snapshot(&world, other).await, before);
    // This caller can read the provider but still has no ordinary management right.
    resources::name_person(&world, "provider", id, "provider.view", company.viewer.id()).await;
    let powerless = key(&world, company.viewer.id(), id, allow.clone()).await;
    let before = snapshot(&world, id).await;
    denied(
        &update(
            &world,
            &powerless,
            id,
            &change(revision(&world, id).await, Some(CHANGED_NAME)),
        )
        .await,
    );
    assert_eq!(snapshot(&world, id).await, before);
    // A separate elevated browser session cannot turn the same account's key into As admin.
    let administrator = places::admin(&world, "crud-administrator").await;
    let bounded = key(&world, administrator.id(), id, allow).await;
    let actor = orch_api_core::middleware::authentication::authenticate(
        &world.app.app,
        &[(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&format!("Bearer {bounded}")).unwrap(),
        )]
        .into_iter()
        .collect(),
        world.now(),
    )
    .await
    .unwrap();
    let orch_api_core::middleware::authentication::Caller::User(user) = actor else {
        panic!("the stored key authenticates its account")
    };
    assert!(!user.as_admin);
    assert!(!user.is_system_admin());
    denied(
        &update(
            &world,
            &bounded,
            id,
            &change(revision(&world, id).await, Some(CHANGED_NAME)),
        )
        .await,
    );
    assert_eq!(snapshot(&world, id).await, before);
}

#[tokio::test]
async fn provider_delete_manage_only_keeps_the_static_gate_and_empty_receipt() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    let other = provider(&world, &company, "Retained provider").await;
    let allow = json!([{"operation":"ProviderDelete","actions":["manage"]}]);
    let exact = key(&world, company.boss.id(), id, allow.clone()).await;
    let other_before = snapshot(&world, other).await;
    denied(
        &world
            .app
            .call(ProviderDelete, &[&other])
            .token(&exact)
            .send_bytes()
            .await,
    );
    assert_eq!(snapshot(&world, other).await, other_before);
    let powerless = key(&world, company.viewer.id(), id, allow).await;
    let before = snapshot(&world, id).await;
    denied(
        &world
            .app
            .call(ProviderDelete, &[&id])
            .token(&powerless)
            .send_bytes()
            .await,
    );
    assert_eq!(snapshot(&world, id).await, before);
    let deleted = world
        .app
        .call(ProviderDelete, &[&id])
        .token(&exact)
        .send_bytes()
        .await;
    assert_eq!(
        deleted.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&deleted.raw)
    );
    assert!(deleted.raw.is_empty());
    assert!(
        routes::providers(&mut *world.conn().await, &[id])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(snapshot(&world, other).await, other_before);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='provider.deleted'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

// Legacy permission-key compatibility supplement: these callers carry provider.manage,
// with no modern operation policy or separately granted provider.view permission.
async fn legacy_manage_key(world: &World, user: Id) -> String {
    use orch_api_core::middleware::authentication::{Caller, Via, authenticate};

    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Legacy provider Manage compatibility",
            token_hash: &hash,
            scopes: &["provider.manage".to_owned()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    let actor = authenticate(
        &world.app.app,
        &[(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&format!("Bearer {secret}")).unwrap(),
        )]
        .into_iter()
        .collect(),
        world.now(),
    )
    .await
    .unwrap();
    let Caller::User(actor) = actor else {
        panic!("the real stored legacy key authenticates its account")
    };
    assert!(!actor.as_admin);
    let Via::Token {
        scopes,
        operation_scope,
        ..
    } = &actor.via
    else {
        panic!("the compatibility caller is a token, not a browser session")
    };
    assert_eq!(scopes, &["provider.manage".to_owned()]);
    assert!(operation_scope.is_none());
    secret
}

#[tokio::test]
async fn legacy_provider_manage_keeps_full_update_and_noop_replies_for_an_ordinary_manager() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    // Management comes from the existing page, independently of the key's permission ceiling.
    resources::name_person(
        &world,
        "provider",
        id,
        "provider.manage",
        company.viewer.id(),
    )
    .await;
    let manager = legacy_manage_key(&world, company.viewer.id()).await;
    let initial = revision(&world, id).await;
    let renamed =
        success(&update(&world, &manager, id, &change(initial, Some(CHANGED_NAME))).await);
    assert_eq!(renamed["id"], json!(id));
    assert_eq!(renamed["name"], CHANGED_NAME);
    assert!(!renamed["upstreams"].as_array().unwrap().is_empty());
    assert!(renamed.get("projection").is_none());
    assert!(
        renamed.get("provider").is_none(),
        "the full reply stays flattened"
    );
    let current = revision(&world, id).await;
    assert!(current > initial);
    assert_eq!(renamed["revision"], current);
    let before = snapshot(&world, id).await;
    let unchanged = success(&update(&world, &manager, id, &change(current, None)).await);
    assert_eq!(unchanged["id"], json!(id));
    assert_eq!(unchanged["name"], CHANGED_NAME);
    assert_eq!(unchanged["revision"], current);
    assert!(!unchanged["upstreams"].as_array().unwrap().is_empty());
    assert!(unchanged.get("projection").is_none());
    assert_eq!(snapshot(&world, id).await, before);

    let ungranted = legacy_manage_key(&world, company.stranger.id()).await;
    denied(
        &update(
            &world,
            &ungranted,
            id,
            &change(current, Some("Not permitted")),
        )
        .await,
    );
    assert_eq!(snapshot(&world, id).await, before);
}

#[tokio::test]
async fn legacy_provider_manage_keeps_full_model_create_and_update_for_an_ordinary_manager() {
    use orch_api_contract::resources::{ProviderModelCreate, ProviderModelUpdate};

    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company, PRIVATE_NAME).await;
    resources::name_person(
        &world,
        "provider",
        id,
        "provider.manage",
        company.viewer.id(),
    )
    .await;
    let manager = legacy_manage_key(&world, company.viewer.id()).await;
    let created = success(
        &world.app.call(ProviderModelCreate, &[&id]).token(&manager)
            .raw_body(&json!({"model_name":"legacy-manual-model","display_name":"Legacy model","groups":["small"]}))
            .send_bytes().await,
    );
    let model: Id = serde_json::from_value(created["id"].clone()).unwrap();
    assert_eq!(created["model_name"], "legacy-manual-model");
    assert_eq!(created["display_name"], "Legacy model");
    assert_eq!(created["groups"], json!(["small"]));
    assert_eq!(created["enabled"], true);
    assert_eq!(created["source"], "manual");
    assert!(created.get("parameters").is_some());
    assert!(created.get("projection").is_none());

    let changed = success(
        &world
            .app
            .call(ProviderModelUpdate, &[&id, &model])
            .token(&manager)
            .raw_body(&json!({"display_name":"Renamed legacy model","groups":["ordinary"]}))
            .send_bytes()
            .await,
    );
    assert_eq!(changed["id"], json!(model));
    assert_eq!(changed["model_name"], "legacy-manual-model");
    assert_eq!(changed["display_name"], "Renamed legacy model");
    assert_eq!(changed["groups"], json!(["ordinary"]));
    assert_eq!(changed["enabled"], true);
    assert_eq!(changed["source"], "manual");
    assert!(changed.get("parameters").is_some());
    assert!(changed.get("projection").is_none());
    let stored: (String, String, Vec<String>, bool) = sqlx::query_as(
        "SELECT model_name,display_name,groups,enabled FROM provider_models WHERE id=$1",
    )
    .bind(model)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        (
            "legacy-manual-model".into(),
            "Renamed legacy model".into(),
            vec!["ordinary".into()],
            true
        )
    );

    let ungranted = legacy_manage_key(&world, company.stranger.id()).await;
    let before = snapshot(&world, id).await;
    denied(
        &world
            .app
            .call(ProviderModelUpdate, &[&id, &model])
            .token(&ungranted)
            .raw_body(&json!({"display_name":"Not permitted"}))
            .send_bytes()
            .await,
    );
    denied(
        &world
            .app
            .call(ProviderModelCreate, &[&id])
            .token(&ungranted)
            .raw_body(&json!({"model_name":"not-permitted-model"}))
            .send_bytes()
            .await,
    );
    let after: (String, String, Vec<String>, bool) = sqlx::query_as(
        "SELECT model_name,display_name,groups,enabled FROM provider_models WHERE id=$1",
    )
    .bind(model)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(after, stored);
    let models: i64 =
        sqlx::query_scalar("SELECT count(*) FROM provider_models WHERE provider_id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(models, 1);
    assert_eq!(snapshot(&world, id).await, before);
}
