//! Administration, settings, secrets and invitations use authority that still holds when their transaction acts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use axum::body::{Body, to_bytes};
use bytes::Bytes;
use chrono::Duration;
use futures::stream;
use http::{Request, StatusCode};
use orch_api_core::middleware::authentication::token_hash;
use orch_api_core::registry::Registry;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::access::{PageKey, testing};
use orch_db::queries::identity;
use orch_types::{Clock, Id, Json, Time};
use places::{Who, admin, join, make_org, make_project, person};
use serde_json::json;
use std::future::Future;
use std::time::Duration as WaitDuration;
use support::World;
use tower::ServiceExt;

// Scheduling headroom for a real database wait on the shared test machine.
const REQUEST_BOUND: WaitDuration = WaitDuration::from_secs(30);
// A defaults-page response is larger than the other fixtures, but remains bounded.
const REPLY_BYTES: usize = 4 * 1024 * 1024;
// Advance past the one-second grant without depending on wall-clock scheduling.
const BODY_DELAY_SECONDS: i64 = 2;
// The shorter grant makes the held-clock crossing unambiguous.
const GRANT_LIFETIME_SECONDS: i64 = 1;

fn router(world: &World) -> axum::Router {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    registry.into_router().with_state(world.app.app.clone())
}

fn request(who: &Who, method: &str, path: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", who.token))
        .header("content-type", "application/json")
        .body(body)
        .unwrap()
}

struct Reply {
    status: StatusCode,
    body: Json,
}

async fn reply(response: axum::response::Response) -> Reply {
    let status = response.status();
    let is_json = response
        .headers()
        .get("content-type")
        .is_some_and(|value| value == "application/json" || value == "application/problem+json");
    let bytes = to_bytes(response.into_body(), REPLY_BYTES).await.unwrap();
    let body = if bytes.is_empty() {
        Json::Null
    } else if is_json {
        serde_json::from_slice(&bytes).unwrap()
    } else {
        json!({"stream_bytes": bytes.len()})
    };
    Reply { status, body }
}

async fn send(world: &World, who: &Who, method: &str, path: &str, body: Option<Json>) -> Reply {
    let body = body.map_or_else(Body::empty, |body| {
        Body::from(serde_json::to_vec(&body).unwrap())
    });
    reply(
        router(world)
            .oneshot(request(who, method, path, body))
            .await
            .unwrap(),
    )
    .await
}

async fn after_body(
    world: &World,
    who: &Who,
    method: &str,
    path: &str,
    value: Json,
    change: impl Future<Output = ()> + Send + 'static,
) -> Reply {
    // The binder authenticates and checks its preflight before polling the body.
    let body = Body::from_stream(stream::once(async move {
        change.await;
        Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&value).unwrap()))
    }));
    reply(
        tokio::time::timeout(
            REQUEST_BOUND,
            router(world).oneshot(request(who, method, path, body)),
        )
        .await
        .expect("the delayed request completes")
        .unwrap(),
    )
    .await
}

async fn revoke_admin(
    conn: &mut orch_db::Conn,
    writer: Id,
    owner: Id,
    now: chrono::DateTime<chrono::Utc>,
) {
    orch_authz::standing::revoke_admin(
        conn,
        writer,
        &orch_authz::standing::Acting {
            by: Actor::User(owner),
            origin: &Origin::default(),
            now,
        },
    )
    .await
    .unwrap();
}

async fn waiting_on(pool: &sqlx::PgPool, holder: i32) {
    tokio::time::timeout(REQUEST_BOUND, async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                 WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(holder)
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real request reaches the held authority lock");
}

async fn after_lock(
    world: &World,
    who: &Who,
    method: &str,
    path: &str,
    demote_by: Option<Id>,
) -> Reply {
    // Bodyless handlers never consume a body; delay their real transaction instead.
    let mut held = world.db.pool.begin().await.unwrap();
    // The request waits on the acting person's guard row, without a shared barrier.
    orch_db::queries::transaction_guard::lock_person(&mut held, who.id())
        .await
        .unwrap();
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let route = router(world);
    let request = request(who, method, path, Body::empty());
    let action = tokio::spawn(async move { route.oneshot(request).await.unwrap() });
    waiting_on(&world.db.pool, holder).await;
    assert!(!action.is_finished());
    if let Some(owner) = demote_by {
        revoke_admin(&mut held, who.id(), owner, world.now()).await;
    }
    world.advance(Duration::seconds(BODY_DELAY_SECONDS));
    held.commit().await.unwrap();
    reply(
        tokio::time::timeout(REQUEST_BOUND, action)
            .await
            .expect("the request completes after the authority lock is released")
            .unwrap(),
    )
    .await
}

async fn effects(world: &World, writer: Id) -> Json {
    // Include the writes and their side effects, but exclude the writer's deliberate demotion
    // and the other person's revocation audit. No credential or secret value is collected.
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'users', (SELECT jsonb_agg(jsonb_build_object('id', id, 'username', username,
                       'status', status, 'admin', is_system_admin, 'org', personal_org_id,
                       'by', created_by) ORDER BY id) FROM users WHERE id <> $1),
          'nodes', (SELECT jsonb_agg(to_jsonb(n) ORDER BY id) FROM nodes n),
          'orgs', (SELECT jsonb_agg(to_jsonb(o) ORDER BY node_id) FROM orgs o),
          'settings', (SELECT jsonb_agg(to_jsonb(s) ORDER BY node_id, key) FROM settings s),
          'secrets', (SELECT jsonb_agg(to_jsonb(s) - 'sealed' ORDER BY id) FROM secrets s),
          'reset_links', (SELECT jsonb_agg(to_jsonb(t) - 'token_hash' ORDER BY id) FROM email_tokens t),
          'agents', (SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM agents a),
          'capabilities', (SELECT jsonb_agg(to_jsonb(c) ORDER BY key) FROM capability_defs c),
          'providers', (SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM identity_providers p),
          'invitations', (SELECT jsonb_agg(to_jsonb(i) - 'token_hash' ORDER BY id) FROM invitations i),
          'uses', (SELECT jsonb_agg(to_jsonb(i) ORDER BY invitation_id, user_id) FROM invitation_uses i),
          'members', (SELECT jsonb_agg(to_jsonb(m) ORDER BY org_id, user_id) FROM org_members m),
          'roles', (SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM node_roles r),
          'rows', (SELECT jsonb_agg(to_jsonb(r) ORDER BY object_kind, object_id, page, profile, permission) FROM permission_rows r),
          'entries', (SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM permission_entries e),
          'audit', (SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_log a WHERE actor_id = $1),
          'outbox', (SELECT jsonb_agg(jsonb_build_object('id', id, 'channel', channel) ORDER BY id) FROM outbox)
        )",
    )
    .bind(writer)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

fn denied(reply: &Reply, permission: &str) {
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.body);
    assert_eq!(reply.body["code"], "authz.denied");
    assert_eq!(reply.body["required"], permission);
}

async fn current_audit(world: &World, writer: Id) {
    let times: Vec<chrono::DateTime<chrono::Utc>> =
        // The fixture's own switch to administrator mode (`places::admin`) is made at the setup's clock,
        // before the action: it is not the action's audit.
        sqlx::query_scalar("SELECT at FROM audit_log WHERE actor_id = $1 AND action <> 'auth.admin_mode_changed'")
            .bind(writer)
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert!(!times.is_empty(), "the successful action is attributed");
    assert!(
        times.iter().all(|at| *at == world.now()),
        "audit uses the action's clock"
    );
}

#[derive(Debug, Clone, Copy)]
enum AdminWrite {
    CreateUser,
    SuspendUser,
    PromoteUser,
    ResetLink,
    SetSetting,
    ClearSetting,
    CreateAgent,
    SetCapability,
    DeleteCapability,
    SetHarness,
    RootLimits,
    CreateIdentityProvider,
    UpdateIdentityProvider,
    DeleteIdentityProvider,
    Defaults,
}

fn capability() -> Json {
    json!({"key": "gpu", "label": "Graphics", "description": "A graphics card",
        "risk": "high", "driver_patch": {}, "default_decision": "ask",
        "default_approver_scope": "org", "builtin": false, "revision": 0})
}

fn provider() -> Json {
    json!({"key": "corp", "name": "Company", "issuer": "https://id.example.com",
        "client_id": "orch", "client_secret": "fixture-client-secret"})
}

struct AdminAction {
    method: &'static str,
    path: String,
    body: Option<Json>,
    target: Id,
}

impl AdminWrite {
    async fn prepare(self, world: &World, owner: &Who) -> AdminAction {
        let target = person(world, "target").await;
        let mut id = target.id();
        let (method, path, body) = match self {
            Self::CreateUser => (
                "POST",
                "/admin/users".into(),
                Some(json!({
                    "username": "newcomer", "display_name": "Newcomer", "is_system_admin": true
                })),
            ),
            Self::SuspendUser => (
                "PATCH",
                format!("/admin/users/{id}"),
                Some(json!({"status": "suspended"})),
            ),
            Self::PromoteUser => (
                "PATCH",
                format!("/admin/users/{id}"),
                Some(json!({
                    "is_system_admin": true, "acknowledge_admin_risk": true
                })),
            ),
            Self::ResetLink => (
                "POST",
                format!("/admin/users/{id}/reset-link"),
                Some(json!({"clear_second_step": true})),
            ),
            Self::SetSetting => (
                "PUT",
                "/admin/settings/chat.enabled".into(),
                Some(json!({"value": false, "locked": true})),
            ),
            Self::ClearSetting => {
                world.set("chat.enabled", json!(false)).await;
                ("DELETE", "/admin/settings/chat.enabled".into(), None)
            }
            Self::CreateAgent => (
                "POST",
                "/admin/agents".into(),
                Some(json!({
                    "name": "Shared", "harness_kind": "codex", "profile": {"harness_version": "1.0.0"}
                })),
            ),
            Self::SetCapability => ("PUT", "/admin/capabilities/gpu".into(), Some(capability())),
            Self::DeleteCapability => {
                let added = send(
                    world,
                    owner,
                    "PUT",
                    "/admin/capabilities/gpu",
                    Some(capability()),
                )
                .await;
                assert!(added.status.is_success(), "{}", added.body);
                ("DELETE", "/admin/capabilities/gpu".into(), None)
            }
            Self::SetHarness => (
                "PUT",
                "/admin/harnesses/codex/1.0.0".into(),
                Some(json!({
                    "kind": "codex", "version": "1.0.0", "os": "linux", "arch": "x86_64",
                    "url": "https://example.com/codex", "sha256": "a".repeat(64),
                    "is_default": true, "source": "admin"
                })),
            ),
            Self::RootLimits => {
                id = make_org(world, owner, "Office", "office").await.base.id;
                (
                    "PATCH",
                    format!("/admin/orgs/{id}"),
                    Some(json!({"closed": true})),
                )
            }
            Self::CreateIdentityProvider => {
                ("POST", "/admin/identity-providers".into(), Some(provider()))
            }
            Self::UpdateIdentityProvider | Self::DeleteIdentityProvider => {
                let added = send(
                    world,
                    owner,
                    "POST",
                    "/admin/identity-providers",
                    Some(provider()),
                )
                .await;
                assert!(added.status.is_success(), "{}", added.body);
                id = added.body["id"].as_str().unwrap().parse().unwrap();
                if matches!(self, Self::UpdateIdentityProvider) {
                    (
                        "PATCH",
                        format!("/admin/identity-providers/{id}"),
                        Some(json!({
                            "name": "Updated", "client_secret": "rotated-fixture-secret",
                            "expected_revision": added.body["revision"]
                        })),
                    )
                } else {
                    ("DELETE", format!("/admin/identity-providers/{id}"), None)
                }
            }
            Self::Defaults => (
                "PATCH",
                "/admin/defaults".into(),
                Some(json!({"whole": "reset"})),
            ),
        };
        AdminAction {
            method,
            path,
            body,
            target: id,
        }
    }

    async fn changed(self, world: &World, writer: Id, action: &AdminAction, reply: &Reply) {
        assert!(reply.status.is_success(), "{self:?}: {}", reply.body);
        match self {
            Self::CreateUser => {
                let (admin, by, org): (bool, Option<Id>, Option<Id>) = sqlx::query_as(
                    "SELECT is_system_admin, created_by, personal_org_id FROM users WHERE username = 'newcomer'",
                ).fetch_one(&world.db.pool).await.unwrap();
                assert!(admin && org.is_some());
                assert_eq!(by, Some(writer));
            }
            Self::SuspendUser | Self::PromoteUser => {
                let (status, admin): (String, bool) =
                    sqlx::query_as("SELECT status, is_system_admin FROM users WHERE id = $1")
                        .bind(action.target)
                        .fetch_one(&world.db.pool)
                        .await
                        .unwrap();
                if matches!(self, Self::SuspendUser) {
                    assert_eq!(status, "suspended");
                    assert!(!admin);
                } else {
                    assert!(admin);
                    assert_eq!(status, "active");
                }
            }
            Self::ResetLink => {
                let (clear, at, expires): (bool, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
                    "SELECT clear_second_step, created_at, expires_at FROM email_tokens WHERE user_id = $1 AND purpose = 'reset_password'",
                ).bind(action.target).fetch_one(&world.db.pool).await.unwrap();
                assert!(clear);
                assert_eq!(at, world.now());
                assert_eq!(reply.body["expires_at"], json!(Time::new(expires)));
            }
            Self::SetSetting | Self::ClearSetting => {
                let own: Option<(Json, bool)> = sqlx::query_as(
                    "SELECT value, locked FROM settings WHERE node_id IS NULL AND key = 'chat.enabled'",
                ).fetch_optional(&world.db.pool).await.unwrap();
                if matches!(self, Self::SetSetting) {
                    assert_eq!(own, Some((json!(false), true)));
                } else {
                    assert_eq!(own, None);
                }
            }
            Self::CreateAgent => {
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM agents WHERE name = 'Shared'")
                        .fetch_one(&world.db.pool)
                        .await
                        .unwrap();
                assert_eq!(count, 1);
            }
            Self::SetCapability | Self::DeleteCapability => {
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM capability_defs WHERE key = 'gpu'")
                        .fetch_one(&world.db.pool)
                        .await
                        .unwrap();
                assert_eq!(count, i64::from(matches!(self, Self::SetCapability)));
            }
            Self::SetHarness => {
                let value: Json = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'harness_build:codex:1.0.0:linux:x86_64'")
                    .fetch_one(&world.db.pool).await.unwrap();
                assert_eq!(value["version"], "1.0.0");
                assert_eq!(value["is_default"], true);
            }
            Self::RootLimits => {
                let closed: bool = sqlx::query_scalar("SELECT closed FROM orgs WHERE node_id = $1")
                    .bind(action.target)
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
                assert!(closed);
            }
            Self::CreateIdentityProvider
            | Self::UpdateIdentityProvider
            | Self::DeleteIdentityProvider => {
                let row: Option<(String, i64)> = sqlx::query_as(
                    "SELECT name, revision FROM identity_providers WHERE key = 'corp'",
                )
                .fetch_optional(&world.db.pool)
                .await
                .unwrap();
                match self {
                    Self::CreateIdentityProvider => assert_eq!(row, Some(("Company".into(), 1))),
                    Self::UpdateIdentityProvider => assert_eq!(row, Some(("Updated".into(), 2))),
                    _ => assert_eq!(row, None),
                }
                let secrets: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM secrets WHERE owner_kind = 'identity_provider'",
                )
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
                assert_eq!(
                    secrets,
                    i64::from(!matches!(self, Self::DeleteIdentityProvider))
                );
            }
            Self::Defaults => {
                assert_eq!(reply.body["page"], "defaults");
            }
        }
        current_audit(world, writer).await;
    }
}

async fn admin_writes(cases: &[AdminWrite]) {
    orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([7; 32]));
    for &case in cases {
        for demote in [true, false] {
            let world = World::new().await;
            let owner = admin(&world, "owner").await;
            let writer = admin(&world, "writer").await;
            let action = case.prepare(&world, &owner).await;
            let before = effects(&world, writer.id()).await;
            let response = match action.body.clone() {
                Some(body) => {
                    let pool = world.db.pool.clone();
                    let clock = world.app.clock.clone();
                    let (writer_id, owner_id) = (writer.id(), owner.id());
                    after_body(
                        &world,
                        &writer,
                        action.method,
                        &action.path,
                        body,
                        async move {
                            if demote {
                                let mut tx = pool.begin().await.unwrap();
                                revoke_admin(&mut tx, writer_id, owner_id, clock.now()).await;
                                tx.commit().await.unwrap();
                            }
                            clock.advance(Duration::seconds(BODY_DELAY_SECONDS));
                        },
                    )
                    .await
                }
                None => {
                    after_lock(
                        &world,
                        &writer,
                        action.method,
                        &action.path,
                        demote.then_some(owner.id()),
                    )
                    .await
                }
            };
            let after = effects(&world, writer.id()).await;
            if demote {
                denied(&response, "system.admin");
                assert_eq!(
                    after, before,
                    "{case:?} has no unauthorized write or side effect"
                );
            } else {
                case.changed(&world, writer.id(), &action, &response).await;
                assert_ne!(after, before, "{case:?} performed the entitled action");
            }
        }
    }
}

#[tokio::test]
async fn a_user_is_created_only_by_a_current_system_admin_after_its_body_arrives() {
    admin_writes(&[AdminWrite::CreateUser]).await;
}

#[tokio::test]
async fn suspension_and_admin_grants_require_the_writers_current_administration() {
    admin_writes(&[AdminWrite::SuspendUser, AdminWrite::PromoteUser]).await;
}

#[tokio::test]
async fn a_reset_link_is_issued_only_with_current_administration_and_action_time() {
    admin_writes(&[AdminWrite::ResetLink]).await;
}

#[tokio::test]
async fn writing_and_clearing_system_settings_require_current_administration() {
    admin_writes(&[AdminWrite::SetSetting, AdminWrite::ClearSetting]).await;
}

#[tokio::test]
async fn catalogue_writes_require_current_administration_including_bodyless_removal() {
    admin_writes(&[
        AdminWrite::CreateAgent,
        AdminWrite::SetCapability,
        AdminWrite::DeleteCapability,
        AdminWrite::SetHarness,
    ])
    .await;
}

#[tokio::test]
async fn root_limits_require_administration_that_still_holds_after_the_body() {
    admin_writes(&[AdminWrite::RootLimits]).await;
}

#[tokio::test]
async fn identity_provider_creation_changes_and_removal_require_current_administration() {
    admin_writes(&[
        AdminWrite::CreateIdentityProvider,
        AdminWrite::UpdateIdentityProvider,
        AdminWrite::DeleteIdentityProvider,
    ])
    .await;
}

#[tokio::test]
async fn resetting_system_defaults_requires_administration_that_still_holds() {
    admin_writes(&[AdminWrite::Defaults]).await;
}

#[tokio::test]
async fn exporting_the_audit_trail_requires_current_administration_before_its_event_is_written() {
    for demote in [true, false] {
        let world = World::new().await;
        let owner = admin(&world, "owner").await;
        let writer = admin(&world, "writer").await;
        let before = effects(&world, writer.id()).await;
        let date = world.now().format("%Y-%m-%dT%H:%M:%SZ");
        let path = format!("/admin/audit/export?format=csv&from={date}&to={date}");
        let response =
            after_lock(&world, &writer, "GET", &path, demote.then_some(owner.id())).await;
        if demote {
            denied(&response, "system.admin");
            assert_eq!(effects(&world, writer.id()).await, before);
        } else {
            assert_eq!(response.status, StatusCode::OK);
            assert!(response.body["stream_bytes"].as_u64().unwrap() > 0);
            current_audit(&world, writer.id()).await;
        }
    }
}

async fn grant(world: &World, writer: &Who, node: Id, permission: &str, ends: bool) -> Id {
    testing::add_entry(
        &mut *world.conn().await,
        &PageKey::new("node", node, "live"),
        "any",
        permission,
        ("user", Some(writer.id()), None),
        ends.then_some(world.now() + Duration::seconds(GRANT_LIFETIME_SECONDS)),
        world.now(),
    )
    .await
    .unwrap()
}

#[derive(Debug, Clone, Copy)]
enum NodeWrite {
    SetPolicy,
    ClearPolicy,
    SetSecret,
    DeleteSecret,
}

#[tokio::test]
async fn node_policy_and_secret_writes_refuse_expired_authority_and_commit_for_current_holders() {
    orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([7; 32]));
    for case in [
        NodeWrite::SetPolicy,
        NodeWrite::ClearPolicy,
        NodeWrite::SetSecret,
        NodeWrite::DeleteSecret,
    ] {
        for ends in [true, false] {
            let world = World::new().await;
            let owner = person(&world, "owner").await;
            let writer = person(&world, "writer").await;
            let org = make_org(&world, &owner, "Office", "office").await;
            join(&world, org.base.id, org.base.id, &writer, "viewer", &owner).await;
            let project = make_project(&world, &owner, org.base.id, "Work")
                .await
                .base
                .id;
            grant(&world, &writer, project, "settings.manage", ends).await;
            let setting = format!("/nodes/{project}/settings/chat.enabled");
            let secret = format!("/nodes/{project}/secrets/DEPLOY_TOKEN");
            let (method, path, body) = match case {
                NodeWrite::SetPolicy => (
                    "PUT",
                    setting.clone(),
                    Some(json!({"value": false, "locked": true})),
                ),
                NodeWrite::ClearPolicy => {
                    let seed = send(
                        &world,
                        &owner,
                        "PUT",
                        &setting,
                        Some(json!({"value": false})),
                    )
                    .await;
                    assert!(seed.status.is_success(), "{}", seed.body);
                    ("DELETE", setting.clone(), None)
                }
                NodeWrite::SetSecret => (
                    "PUT",
                    secret.clone(),
                    Some(json!({"value": "fixture-secret"})),
                ),
                NodeWrite::DeleteSecret => {
                    let seed = send(
                        &world,
                        &owner,
                        "PUT",
                        &secret,
                        Some(json!({"value": "fixture-secret"})),
                    )
                    .await;
                    assert!(seed.status.is_success(), "{}", seed.body);
                    ("DELETE", secret.clone(), None)
                }
            };
            let before = effects(&world, writer.id()).await;
            let response = match body {
                Some(body) => {
                    let clock = world.app.clock.clone();
                    after_body(&world, &writer, method, &path, body, async move {
                        clock.advance(Duration::seconds(BODY_DELAY_SECONDS));
                    })
                    .await
                }
                None => after_lock(&world, &writer, method, &path, None).await,
            };
            if ends {
                denied(&response, "settings.manage");
                assert_eq!(
                    effects(&world, writer.id()).await,
                    before,
                    "{case:?} changes nothing when its grant ends"
                );
            } else {
                assert!(response.status.is_success(), "{case:?}: {}", response.body);
                match case {
                    NodeWrite::SetPolicy | NodeWrite::ClearPolicy => {
                        let own: Option<(Json, bool)> = sqlx::query_as(
                            "SELECT value, locked FROM settings WHERE node_id = $1 AND key = 'chat.enabled'",
                        ).bind(project).fetch_optional(&world.db.pool).await.unwrap();
                        if matches!(case, NodeWrite::SetPolicy) {
                            assert_eq!(own, Some((json!(false), true)));
                        } else {
                            assert_eq!(own, None);
                        }
                    }
                    NodeWrite::SetSecret | NodeWrite::DeleteSecret => {
                        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM secrets WHERE owner_kind = 'setting' AND owner_id = $1 AND name = 'DEPLOY_TOKEN'")
                            .bind(project).fetch_one(&world.db.pool).await.unwrap();
                        assert_eq!(count, i64::from(matches!(case, NodeWrite::SetSecret)));
                    }
                }
                current_audit(&world, writer.id()).await;
            }
        }
    }
}

#[tokio::test]
async fn invitation_creation_and_revocation_require_the_current_permission_for_the_stored_kind() {
    for kind in ["system", "org", "node"] {
        for deleting in [false, true] {
            for loses in [true, false] {
                let world = World::new().await;
                let owner = admin(&world, "owner").await;
                let writer = if kind == "system" {
                    admin(&world, "writer").await
                } else {
                    person(&world, "writer").await
                };
                let org = make_org(&world, &owner, "Office", "office").await;
                join(&world, org.base.id, org.base.id, &writer, "viewer", &owner).await;
                let project = make_project(&world, &owner, org.base.id, "Work")
                    .await
                    .base
                    .id;
                // For node revocation the writer made the link, so losing authority must still
                // be checked rather than treating the old creator attribution as permission.
                let permission = match kind {
                    "org" => {
                        grant(&world, &writer, org.base.id, "members.manage", loses).await;
                        "members.manage"
                    }
                    "node" => {
                        grant(&world, &writer, project, "access.manage", loses).await;
                        grant(&world, &writer, project, "access.external", false).await;
                        "access.manage"
                    }
                    _ => "system.admin",
                };
                let value = match kind {
                    "org" => json!({"kind": "org", "org_id": org.base.id, "role": "viewer"}),
                    "node" => {
                        json!({"kind": "node", "node_id": project, "permissions": ["project.view"]})
                    }
                    _ => json!({"kind": "system"}),
                };
                let (method, path, body) = if deleting {
                    let seed = send(&world, &writer, "POST", "/invitations", Some(value)).await;
                    assert!(seed.status.is_success(), "{}", seed.body);
                    // The creator's earlier event is deliberately excluded from the time check below.
                    let id = seed.body["invitation"]["id"].as_str().unwrap();
                    ("DELETE", format!("/invitations/{id}"), None)
                } else {
                    ("POST", "/invitations".into(), Some(value))
                };
                let before = effects(&world, writer.id()).await;
                let response = match body {
                    Some(body) => {
                        let pool = world.db.pool.clone();
                        let clock = world.app.clock.clone();
                        let (writer_id, owner_id) = (writer.id(), owner.id());
                        after_body(&world, &writer, method, &path, body, async move {
                            if kind == "system" && loses {
                                let mut tx = pool.begin().await.unwrap();
                                revoke_admin(&mut tx, writer_id, owner_id, clock.now()).await;
                                tx.commit().await.unwrap();
                            }
                            clock.advance(Duration::seconds(BODY_DELAY_SECONDS));
                        })
                        .await
                    }
                    None => {
                        after_lock(
                            &world,
                            &writer,
                            method,
                            &path,
                            (kind == "system" && loses).then_some(owner.id()),
                        )
                        .await
                    }
                };
                if loses {
                    denied(&response, permission);
                    assert_eq!(
                        effects(&world, writer.id()).await,
                        before,
                        "{kind} {method} has no mutation after authority ends"
                    );
                } else {
                    assert!(
                        response.status.is_success(),
                        "{kind} {method}: {}",
                        response.body
                    );
                    let (count, revoked): (i64, i64) = sqlx::query_as(
                        "SELECT count(*), count(revoked_at) FROM invitations WHERE created_by = $1",
                    )
                    .bind(writer.id())
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
                    assert_eq!((count, revoked), (1, i64::from(deleting)));
                    let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
                        "SELECT at FROM audit_log WHERE actor_id = $1 AND action = $2",
                    )
                    .bind(writer.id())
                    .bind(if deleting {
                        "invite.revoked"
                    } else {
                        "invite.created"
                    })
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
                    assert_eq!(at, world.now());
                }
            }
        }
    }
}

#[tokio::test]
async fn accepting_an_org_or_node_invitation_rechecks_the_makers_current_permission() {
    for kind in ["org", "node"] {
        for loses in [true, false] {
            let world = World::new().await;
            let owner = person(&world, "owner").await;
            let maker = person(&world, "maker").await;
            let recipient = person(&world, "recipient").await;
            let org = make_org(&world, &owner, "Office", "office").await;
            join(&world, org.base.id, org.base.id, &maker, "viewer", &owner).await;
            let project = make_project(&world, &owner, org.base.id, "Work")
                .await
                .base
                .id;
            let value = if kind == "org" {
                grant(&world, &maker, org.base.id, "members.manage", loses).await;
                json!({"kind": "org", "org_id": org.base.id, "role": "viewer"})
            } else {
                grant(&world, &maker, project, "access.manage", false).await;
                grant(&world, &maker, project, "access.external", loses).await;
                json!({"kind": "node", "node_id": project, "permissions": ["project.view"]})
            };
            let made = send(&world, &maker, "POST", "/invitations", Some(value)).await;
            assert!(made.status.is_success(), "{}", made.body);
            let before = effects(&world, recipient.id()).await;
            let clock = world.app.clock.clone();
            let response = after_body(
                &world,
                &recipient,
                "POST",
                "/invitations/accept",
                json!({"token": made.body["token"]}),
                async move {
                    clock.advance(Duration::seconds(BODY_DELAY_SECONDS));
                },
            )
            .await;
            if loses {
                if kind == "org" {
                    denied(&response, "members.manage");
                } else {
                    assert_eq!(response.status, StatusCode::FORBIDDEN);
                    assert_eq!(response.body["code"], "sharing.external_forbidden");
                    assert_eq!(response.body["required"], "access.external");
                    assert_eq!(response.body["object"]["id"], json!(project));
                }
                assert_eq!(
                    effects(&world, recipient.id()).await,
                    before,
                    "an expired giver writes no membership, page entry, use or audit"
                );
            } else {
                assert_eq!(response.status, StatusCode::OK, "{}", response.body);
                let uses: i32 = sqlx::query_scalar("SELECT uses FROM invitations WHERE id = $1")
                    .bind(
                        made.body["invitation"]["id"]
                            .as_str()
                            .unwrap()
                            .parse::<Id>()
                            .unwrap(),
                    )
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
                assert_eq!(uses, 1);
                let members: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM org_members WHERE org_id = $1 AND user_id = $2",
                )
                .bind(org.base.id)
                .bind(recipient.id())
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
                let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_id = $1 AND holder_id = $2 AND permission = 'project.view'")
                    .bind(project).bind(recipient.id()).fetch_one(&world.db.pool).await.unwrap();
                assert_eq!(
                    (members, entries),
                    if kind == "org" { (1, 0) } else { (0, 1) }
                );
                current_audit(&world, recipient.id()).await;
            }
        }
    }
}

#[tokio::test]
async fn an_invited_role_requires_the_maker_to_still_hold_that_role_when_the_body_arrives() {
    for loses in [true, false] {
        let world = World::new().await;
        let owner = person(&world, "owner").await;
        let maker = person(&world, "maker").await;
        let recipient = person(&world, "recipient").await;
        let org = make_org(&world, &owner, "Office", "office").await;
        join(&world, org.base.id, org.base.id, &maker, "viewer", &owner).await;
        let project = make_project(&world, &owner, org.base.id, "Work")
            .await
            .base
            .id;
        grant(&world, &maker, project, "access.manage", false).await;
        grant(&world, &maker, project, "access.external", false).await;
        testing::give_role(
            &mut *world.conn().await,
            project,
            "user",
            maker.id(),
            "developer",
            owner.id(),
            loses.then_some(world.now() + Duration::seconds(GRANT_LIFETIME_SECONDS)),
        )
        .await
        .unwrap();
        let made = send(
            &world,
            &maker,
            "POST",
            "/invitations",
            Some(json!({
                "kind": "node", "node_id": project, "role": "developer"
            })),
        )
        .await;
        assert!(made.status.is_success(), "{}", made.body);
        let before = effects(&world, recipient.id()).await;
        let clock = world.app.clock.clone();
        let response = after_body(
            &world,
            &recipient,
            "POST",
            "/invitations/accept",
            json!({"token": made.body["token"]}),
            async move {
                clock.advance(Duration::seconds(BODY_DELAY_SECONDS));
            },
        )
        .await;
        if loses {
            assert_eq!(response.status, StatusCode::FORBIDDEN, "{}", response.body);
            assert_eq!(response.body["code"], "authz.denied");
            assert!(
                !response.body["missing"].as_array().unwrap().is_empty(),
                "the role refusal says what its maker lacks"
            );
            assert_eq!(effects(&world, recipient.id()).await, before);
        } else {
            assert_eq!(response.status, StatusCode::OK, "{}", response.body);
            let role: String = sqlx::query_scalar(
                "SELECT role FROM node_roles WHERE node_id = $1 AND subject_id = $2",
            )
            .bind(project)
            .bind(recipient.id())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            assert_eq!(role, "developer");
            current_audit(&world, recipient.id()).await;
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum AcceptanceLoss {
    ConfirmedAddress,
    Session,
    InvitationExpired,
}

#[tokio::test]
async fn invitation_acceptance_uses_the_current_address_session_and_invitation_lifetime() {
    for loss in [
        AcceptanceLoss::ConfirmedAddress,
        AcceptanceLoss::Session,
        AcceptanceLoss::InvitationExpired,
    ] {
        for loses in [true, false] {
            let world = World::new().await;
            let owner = person(&world, "owner").await;
            let recipient = person(&world, "recipient").await;
            let org = make_org(&world, &owner, "Office", "office").await;
            let made = send(
                &world,
                &owner,
                "POST",
                "/invitations",
                Some(json!({
                    "kind": "org", "org_id": org.base.id, "email": recipient.person.email,
                    "role": "viewer"
                })),
            )
            .await;
            assert!(made.status.is_success(), "{}", made.body);
            let invitation: Id = made.body["invitation"]["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            if loses && matches!(loss, AcceptanceLoss::InvitationExpired) {
                sqlx::query("UPDATE invitations SET expires_at = $2 WHERE id = $1")
                    .bind(invitation)
                    .bind(world.now() + Duration::seconds(GRANT_LIFETIME_SECONDS))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            let before = effects(&world, recipient.id()).await;
            let pool = world.db.pool.clone();
            let clock = world.app.clock.clone();
            let recipient_id = recipient.id();
            let hash = token_hash(&recipient.token);
            let response = after_body(
                &world,
                &recipient,
                "POST",
                "/invitations/accept",
                json!({"token": made.body["token"]}),
                async move {
                    if loses {
                        let mut tx = pool.begin().await.unwrap();
                        match loss {
                            AcceptanceLoss::ConfirmedAddress => {
                                sqlx::query(
                                    "UPDATE users SET email = 'changed@example.org' WHERE id = $1",
                                )
                                .bind(recipient_id)
                                .execute(&mut *tx)
                                .await
                                .unwrap();
                            }
                            AcceptanceLoss::Session => {
                                let session =
                                    identity::session_by_access_token(&mut tx, &hash, clock.now())
                                        .await
                                        .unwrap()
                                        .unwrap();
                                identity::revoke_session(
                                    &mut tx,
                                    session.session_id,
                                    recipient_id,
                                    clock.now(),
                                )
                                .await
                                .unwrap();
                            }
                            AcceptanceLoss::InvitationExpired => {}
                        }
                        tx.commit().await.unwrap();
                    }
                    clock.advance(Duration::seconds(BODY_DELAY_SECONDS));
                },
            )
            .await;
            if loses {
                let (status, code) = match loss {
                    AcceptanceLoss::ConfirmedAddress => {
                        (StatusCode::FORBIDDEN, "registration.invite_required")
                    }
                    AcceptanceLoss::Session => (StatusCode::UNAUTHORIZED, "auth.invalid_token"),
                    AcceptanceLoss::InvitationExpired => (StatusCode::GONE, "gone"),
                };
                assert_eq!(response.status, status, "{loss:?}: {}", response.body);
                assert_eq!(response.body["code"], code);
                assert_eq!(
                    effects(&world, recipient.id()).await,
                    before,
                    "{loss:?} leaves no accepted invitation or grant"
                );
            } else {
                assert_eq!(response.status, StatusCode::OK, "{}", response.body);
                let joined: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)",
                )
                .bind(org.base.id)
                .bind(recipient.id())
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
                assert!(joined);
                current_audit(&world, recipient.id()).await;
            }
        }
    }
}
