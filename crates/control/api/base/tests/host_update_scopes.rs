//! Native Host updates keep the actual request and original scope through drain and delivery.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::resources::HostUpdateNow;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::{RawReply, advance_component_updates, authorize_machine},
    thread_access::AuthzAccess,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{
    component_updates as updates, identity, link,
    resources::{self as rows, access_fixtures as fixtures},
};
use orch_engine::commands::CommandAuthority;
use orch_link_contract::{
    backplane::AuthQuery,
    policy::LocalPolicy,
    session::{Role, TlsMode},
};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
async fn key(world: &World, user: Id, rules: Vec<Json>, seconds: i64) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Host physical update",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::seconds(seconds)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}
async fn host(world: &World, owner: Id, home: Id, node: bool, enabled: bool) -> (Id, Id) {
    if std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
        let _ = support::Statements::now();
    }
    let host = fixtures::insert_resource(
        &mut *world.conn().await,
        "host",
        Some(owner),
        Some(home),
        "Physical update host",
    )
    .await
    .unwrap();
    start_resource(
        &mut *world.conn().await,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id: host,
        },
        owner,
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let policy = LocalPolicy {
        update_auto: enabled,
        ..LocalPolicy::default()
    };
    sqlx::query("UPDATE machines SET online=true,connector_version='0.0.0',roles=$2,facts=jsonb_build_object('policy',$3::jsonb) WHERE id=$1")
        .bind(machine).bind(if node {vec!["node"]} else {Vec::<&str>::new()}).bind(json!(policy)).execute(&world.db.pool).await.unwrap();
    let mut conn = world.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(owner),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_authz::resources::effective(
            &mut conn,
            &principal,
            ObjectRef {
                kind: ResourceKind::Host.into(),
                id: host
            },
            orch_authz::Policy::default(),
        )
        .await
        .unwrap()
        .owner,
        "the physical Host fixture belongs to its actual company member"
    );
    (host, machine)
}
async fn call(world: &World, token: &str, host: Id) -> RawReply {
    world
        .app
        .call(HostUpdateNow, &[&host])
        .token(token)
        .send_bytes()
        .await
}
#[track_caller]
fn body(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn record(world: &World, machine: Id) -> updates::OriginRow {
    updates::origin(&mut *world.conn().await, machine)
        .await
        .unwrap()
        .unwrap()
}
fn receipt(value: &Json, host: Id, machine: Id, requested: bool) {
    assert_eq!(
        value,
        &json!({"projection":"receipt","host_id":host,"machine_id":machine,"requested":requested})
    );
}

#[tokio::test]
async fn host_update_manage_only_records_the_exact_source_and_replay_never_reowns_it() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = host(&world, company.boss.id(), company.acme, false, true).await;
    resources::name_person(&world, "host", host, "host.manage", company.viewer.id()).await;
    let first = key(
        &world,
        company.viewer.id(),
        vec![rule("HostUpdateNow", "host", host, &["manage"])],
        3600,
    )
    .await;
    receipt(
        &body(call(&world, &first.secret, host).await),
        host,
        machine,
        true,
    );
    let original = record(&world, machine).await;
    assert!(original.origin_required);
    let origin = original.origin.clone().unwrap();
    assert_eq!(origin["origin"]["operation"], "HostUpdateNow");
    assert_eq!(origin["request"], json!(original.request_id));
    assert_eq!(origin["host"], json!(host));
    assert_eq!(origin["machine"], json!(machine));
    assert!(origin.to_string().contains(&first.id.to_string()));
    assert!(!origin.to_string().contains(&first.secret));
    let later = key(
        &world,
        company.boss.id(),
        vec![rule("HostUpdateNow", "host", host, &["manage"])],
        3600,
    )
    .await;
    receipt(
        &body(call(&world, &later.secret, host).await),
        host,
        machine,
        false,
    );
    let unchanged = record(&world, machine).await;
    assert_eq!(unchanged.request_id, original.request_id);
    assert_eq!(unchanged.origin, original.origin);
    assert_eq!(unchanged.requested_by, Some(company.viewer.id()));
}

#[tokio::test]
async fn host_update_read_uses_this_operations_host_facet_and_preserves_native_status() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = host(&world, company.boss.id(), company.acme, false, true).await;
    let scoped = key(
        &world,
        company.boss.id(),
        vec![rule("HostUpdateNow", "host", host, &["manage", "read"])],
        3600,
    )
    .await;
    let full = body(call(&world, &scoped.secret, host).await);
    assert_eq!(full["status"], "queued");
    assert_eq!(full["running_version"], "0.0.0");
    assert!(full["projection"].is_null());
    assert_eq!(full, body(call(&world, &company.boss.token, host).await));
    let unrelated = key(
        &world,
        company.boss.id(),
        vec![
            rule("HostUpdateNow", "host", host, &["manage"]),
            rule("HostGet", "host", host, &["read"]),
        ],
        3600,
    )
    .await;
    receipt(
        &body(call(&world, &unrelated.secret, host).await),
        host,
        machine,
        false,
    );
}

#[tokio::test]
async fn host_update_requires_its_real_host_manage_and_current_ordinary_management() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = host(&world, company.boss.id(), company.acme, false, true).await;
    for (user, rules) in [
        (
            company.boss.id(),
            vec![rule("MachineUpdateNow", "machine", machine, &["manage"])],
        ),
        (
            company.boss.id(),
            vec![rule("HostUpdateNow", "machine", machine, &["manage"])],
        ),
        (
            company.boss.id(),
            vec![rule("HostUpdateNow", "host", host, &["read"])],
        ),
        (
            company.stranger.id(),
            vec![rule("HostUpdateNow", "host", host, &["manage"])],
        ),
    ] {
        let denied = key(&world, user, rules, 3600).await;
        assert_eq!(
            call(&world, &denied.secret, host).await.status,
            StatusCode::FORBIDDEN
        );
        assert!(
            updates::origin(&mut *world.conn().await, machine)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn deferred_host_update_closes_expired_revoked_and_damaged_origins_before_native_queueing() {
    for change in ["expiry", "scope", "missing"] {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let (host, machine) = host(&world, company.boss.id(), company.acme, false, true).await;
        let actor = key(
            &world,
            company.boss.id(),
            vec![rule("HostUpdateNow", "host", host, &["manage"])],
            3600,
        )
        .await;
        receipt(
            &body(call(&world, &actor.secret, host).await),
            host,
            machine,
            true,
        );
        match change {
            "expiry" => world.advance(chrono::Duration::hours(2)),
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!(KeyScope::default()))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE component_updates SET origin=NULL WHERE machine_id=$1")
                    .bind(machine)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
        }
        advance_component_updates(&world.app.app).await.unwrap();
        let row = record(&world, machine).await;
        assert_eq!(row.state, "refused");
        assert!(row.command_id.is_none());
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='machine.update.now'",
        )
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn deferred_host_update_uses_the_real_command_authority_and_a_successor_cannot_validate_it() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = host(&world, company.boss.id(), company.acme, false, true).await;
    let actor = key(
        &world,
        company.boss.id(),
        vec![rule("HostUpdateNow", "host", host, &["manage"])],
        3600,
    )
    .await;
    body(call(&world, &actor.secret, host).await);
    advance_component_updates(&world.app.app).await.unwrap();
    let request = record(&world, machine).await;
    let id = request.command_id.unwrap();
    let saved = link::command_with_authority(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    let authority: CommandAuthority =
        serde_json::from_value(saved.authority.clone().unwrap()).unwrap();
    assert_eq!(
        saved.authority.as_ref().unwrap()["kind"],
        "component_update"
    );
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &saved.command,
                world.app.app.clock.as_ref(),
                &AuthzAccess::new(world.app.app.clock.clone())
            )
            .await
            .unwrap()
            .is_some()
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE component_updates SET request_id=$2 WHERE machine_id=$1")
        .bind(machine)
        .bind(new_id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &saved.command,
                world.app.app.clock.as_ref(),
                &AuthzAccess::new(world.app.app.clock.clone())
            )
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn deferred_host_update_keeps_its_original_browser_session_at_native_delivery() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = host(&world, company.boss.id(), company.acme, false, true).await;
    let full = body(call(&world, &company.boss.token, host).await);
    assert_eq!(full["status"], "queued");
    let request = record(&world, machine).await;
    let origin = request.origin.unwrap();
    assert!(!origin["until"].is_null());
    let session: Id = origin["session"].as_str().unwrap().parse().unwrap();
    let user: Id = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(user, company.boss.id());
    advance_component_updates(&world.app.app).await.unwrap();
    let request = record(&world, machine).await;
    let saved = link::command_with_authority(&mut *world.conn().await, request.command_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    let authority: CommandAuthority = serde_json::from_value(saved.authority.unwrap()).unwrap();
    for revoked in [false, true] {
        if revoked {
            sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=$1")
                .bind(session)
                .bind(world.now())
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        let mut tx = world.db.pool.begin().await.unwrap();
        let allowed = authority
            .current_callers(
                &mut tx,
                &saved.command,
                world.app.app.clock.as_ref(),
                &AuthzAccess::new(world.app.app.clock.clone()),
            )
            .await
            .unwrap()
            .is_some();
        assert_eq!(allowed, !revoked, "the original raw session is required");
        tx.commit().await.unwrap();
    }
}

async fn certificate_query(world: &World, machine: Id) -> AuthQuery {
    let row = rows::machine(&mut *world.conn().await, machine)
        .await
        .unwrap()
        .unwrap();
    AuthQuery {
        purpose: Default::default(),
        capabilities: Vec::new(),
        endpoints: None,
        hosted_endpoints: None,
        n: 1,
        machine_id: machine,
        fingerprint: hex::encode(row.cert_fingerprint),
        tls: TlsMode::Server,
        ip: "127.0.0.1".parse().unwrap(),
        connector_version: "0.0.9".into(),
        roles: vec![Role::Node],
    }
}
#[tokio::test]
async fn minimum_recovery_uses_its_manual_request_and_never_borrows_automatic_authority() {
    for expire in [false, true] {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let (host, machine) = host(&world, company.boss.id(), company.acme, true, true).await;
        let actor = key(
            &world,
            company.boss.id(),
            vec![rule("HostUpdateNow", "host", host, &["manage"])],
            3600,
        )
        .await;
        body(call(&world, &actor.secret, host).await);
        let original = record(&world, machine).await;
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        let reply =
            authorize_machine(&world.app.app, &certificate_query(&world, machine).await).await;
        assert!(!reply.ok);
        assert_eq!(reply.update_permit.is_some(), !expire);
        let current = record(&world, machine).await;
        assert_eq!(current.request_id, original.request_id);
        assert_eq!(current.origin, original.origin);
        if expire {
            assert_eq!(current.state, "refused");
        } else {
            assert_eq!(current.state, "updating");
        }
    }
}

#[tokio::test]
async fn host_update_keeps_local_policy_refusal_and_rechecks_expiry_after_the_native_physical_lock()
{
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = host(&world, company.boss.id(), company.acme, false, false).await;
    let actor = key(
        &world,
        company.boss.id(),
        vec![rule("HostUpdateNow", "host", host, &["manage", "read"])],
        3600,
    )
    .await;
    let refused = body(call(&world, &actor.secret, host).await);
    assert_eq!(refused["status"], "refused");
    assert!(
        refused["detail"]
            .as_str()
            .unwrap()
            .contains("update.auto is off")
    );
    assert_eq!(record(&world, machine).await.state, "refused");
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
        .bind(machine)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(HostUpdateNow, &[&host]).token(&actor.secret);
        call.send_bytes().await
    });
    tokio::time::timeout(WAIT,async {loop {
        assert!(!request.is_finished(),"the native physical program lock must be reached");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT id FROM machines%FOR SHARE%')")
            .bind(pid).fetch_one(&world.db.pool).await.unwrap();if blocked {break;}tokio::task::yield_now().await;
    }}).await.expect("the actual machine source lock is waiting");
    let before = record(&world, machine).await;
    world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    assert_eq!(
        tokio::time::timeout(WAIT, request)
            .await
            .unwrap()
            .unwrap()
            .status,
        StatusCode::UNAUTHORIZED
    );
    let after = record(&world, machine).await;
    assert_eq!(after.request_id, before.request_id);
    assert_eq!(after.origin, before.origin);
}

#[tokio::test]
async fn deferred_host_update_never_borrows_the_owners_rights_after_its_requester_loses_management()
{
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = host(&world, company.boss.id(), company.acme, false, true).await;
    let grant =
        resources::name_person(&world, "host", host, "host.manage", company.viewer.id()).await;
    let actor = key(
        &world,
        company.viewer.id(),
        vec![rule("HostUpdateNow", "host", host, &["manage"])],
        3600,
    )
    .await;
    body(call(&world, &actor.secret, host).await);
    sqlx::query("UPDATE permission_entries SET removed_at=$2,removed_by=$3 WHERE id=$1")
        .bind(grant)
        .bind(world.now())
        .bind(company.boss.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    advance_component_updates(&world.app.app).await.unwrap();
    let request = record(&world, machine).await;
    assert_eq!(request.state, "refused");
    assert!(request.command_id.is_none());
    assert_eq!(request.requested_by, Some(company.viewer.id()));
}

#[tokio::test]
async fn host_update_commits_once_and_a_closed_response_read_publishes_only_its_original_receipt() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = host(&world, company.boss.id(), company.acme, false, true).await;
    let actor = key(
        &world,
        company.boss.id(),
        vec![rule("HostUpdateNow", "host", host, &["manage", "read"])],
        3600,
    )
    .await;
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let secret = actor.secret.clone();
    let request = tokio::spawn(async move {
        let call = app.call(HostUpdateNow, &[&host]).token(&secret);
        call.send_bytes().await
    });
    tokio::time::timeout(WAIT,async {loop {
        assert!(!request.is_finished(),"the native status must reach its real activity publication");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
            .bind(pid).fetch_one(&world.db.pool).await.unwrap();if blocked {break;}tokio::task::yield_now().await;
    }}).await.expect("the actual activity wait follows the accepted physical request");
    let committed = record(&world, machine).await;
    let scope: KeyScope =
        serde_json::from_value(json!({"rules":[rule("HostUpdateNow","host",host,&["manage"])]}))
            .unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!(scope))
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    receipt(
        &body(tokio::time::timeout(WAIT, request).await.unwrap().unwrap()),
        host,
        machine,
        true,
    );
    let after = record(&world, machine).await;
    assert_eq!(after.request_id, committed.request_id);
    assert_eq!(after.origin, committed.origin);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='machine.update_requested' AND object_id=$1",
    )
    .bind(machine)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audits, 1,
        "response validation never reruns the native writer"
    );
}
