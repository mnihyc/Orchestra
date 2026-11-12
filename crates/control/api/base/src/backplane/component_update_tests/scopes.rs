//! Scoped update requests exercise the actual existing API, drain loop, journal and handshake.

use super::*;

use crate::middleware::authentication::API_TOKEN_PREFIX;

use crate::middleware::authentication::new_token;

use crate::middleware::authentication::token_hash;

use crate::support::World;

use crate::testing::RawReply;

use http::StatusCode;

use orch_access_contract::key_scope::KeyScope;

use orch_api_contract::resources::MachineUpdateNow;

use orch_db::queries::component_updates as updates;

use orch_db::queries::identity;

use orch_db::queries::identity::NewApiToken;

use orch_db::queries::link;

use orch_db::queries::resources::access_fixtures as fixtures;

use orch_link_contract::policy::LocalPolicy;

use orch_types::Json;

use serde_json::json;
struct Key {
    id: Id,
    secret: String,
}

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let key = Key {
        id: orch_types::new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native update request issuer",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::seconds(30)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    key
}

async fn machine(world: &World, user: Id) -> Id {
    let id =
        fixtures::insert_persons_machine(&mut *world.conn().await, user, "Private update machine")
            .await
            .unwrap();
    let policy = LocalPolicy {
        update_auto: true,
        ..LocalPolicy::default()
    };
    sqlx::query("UPDATE machines SET connector_version='0.0.0',online=true,roles=ARRAY['agent','environment'],facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1").bind(id).bind(json!(policy)).execute(&world.db.pool).await.unwrap();
    id
}

async fn call(world: &World, token: &str, id: Id) -> RawReply {
    let request = world.app.call(MachineUpdateNow, &[&id]).token(token);
    request.send_bytes().await
}

fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

fn receipt(id: Id) -> Json {
    json!({"projection":"receipt","machine_id":id})
}

async fn counts(world: &World, id: Id) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM component_updates WHERE machine_id=$1),(SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='machine.update.now'),(SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='machine.update_requested')").bind(id).fetch_one(&world.db.pool).await.unwrap()
}

async fn origin(world: &World, id: Id) -> Json {
    updates::issuer_origin(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
}

async fn waited<T>(world: &World, pid: i32, pending: &tokio::task::JoinHandle<T>, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {loop {
        assert!(!pending.is_finished(),"the actual native wait must be reached");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&world.db.pool).await.unwrap();
        if blocked{break;}tokio::task::yield_now().await;
    }}).await.expect("the real native PostgreSQL lock must block this request");
}

#[cfg(any(test, feature = "test-support"))]
async fn update_needs_exact_scope_and_returns_native_progress_only_under_its_own_read() {
    let world = World::new().await;
    let owner = crate::places::person(&world, "owner").await;
    let stranger = crate::places::person(&world, "stranger").await;
    let id = machine(&world, owner.id()).await;
    let other = machine(&world, owner.id()).await;
    for rules in [
        vec![rule("MachineUpdate", "machine", id, &["manage"])],
        vec![rule("MachineUpdateNow", "machine", id, &["read"])],
        vec![rule("MachineUpdateNow", "environment", id, &["manage"])],
        vec![rule("MachineUpdateNow", "machine", other, &["manage"])],
    ] {
        let token = key(&world, owner.id(), rules).await;
        assert_eq!(
            call(&world, &token.secret, id).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(counts(&world, id).await, (0, 0, 0));
    }
    let denied = key(
        &world,
        stranger.id(),
        vec![rule("MachineUpdateNow", "machine", id, &["manage"])],
    )
    .await;
    assert_eq!(
        call(&world, &denied.secret, id).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(counts(&world, id).await, (0, 0, 0));
    let writer = key(
        &world,
        owner.id(),
        vec![
            rule("MachineUpdateNow", "machine", id, &["manage"]),
            rule("MachineGet", "machine", id, &["read"]),
        ],
    )
    .await;
    assert_eq!(
        accepted(call(&world, &writer.secret, id).await),
        receipt(id)
    );
    assert_eq!(counts(&world, id).await, (1, 0, 1));
    let packet = origin(&world, id).await;
    assert_eq!(packet["origin"]["operation"], "MachineUpdateNow");
    assert_eq!(packet["credential"]["kind"], "token");
    let reader = key(
        &world,
        owner.id(),
        vec![rule("MachineUpdateNow", "machine", id, &["manage", "read"])],
    )
    .await;
    let full = accepted(call(&world, &reader.secret, id).await);
    assert_eq!(full["status"], "queued");
    assert_eq!(full["running_version"], "0.0.0");
    assert_eq!(
        origin(&world, id).await,
        packet,
        "another click never replaces the retained original request"
    );
    let ordinary = accepted(call(&world, &owner.token, other).await);
    assert_eq!(ordinary["status"], "queued");
    assert_eq!(ordinary["running_version"], "0.0.0");
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_update_needs_exact_scope_and_returns_native_progress_only_under_its_own_read()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::update_needs_exact_scope_and_returns_native_progress_only_under_its_own_read(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn actual_drain_loop_rechecks_original_scope_raw_expiry_revocation_and_physical_source() {
    for change in [
        "unchanged",
        "scope",
        "expired",
        "revoked",
        "owner",
        "corrupt",
        "missing",
        "unmarked",
        "request",
    ] {
        let world = World::new().await;
        let owner = crate::places::person(&world, "owner").await;
        let other = crate::places::person(&world, "other").await;
        let id = machine(&world, owner.id()).await;
        let actor = key(
            &world,
            owner.id(),
            vec![rule("MachineUpdateNow", "machine", id, &["manage"])],
        )
        .await;
        accepted(call(&world, &actor.secret, id).await);
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":[rule("MachineUpdate","machine",id,&["manage"])]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => world.advance(chrono::Duration::seconds(31)),
            "revoked" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(world.now())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "owner" => {
                sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
                    .bind(id)
                    .bind(other.id())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "corrupt" => {
                sqlx::query("UPDATE component_updates SET origin=$2 WHERE machine_id=$1")
                    .bind(id)
                    .bind(json!({"invalid_origin":true}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "missing" => {
                sqlx::query("UPDATE component_updates SET origin=NULL WHERE machine_id=$1")
                    .bind(id)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "unmarked" => {
                sqlx::query(
                    "UPDATE component_updates SET origin_required=false WHERE machine_id=$1",
                )
                .bind(id)
                .execute(&world.db.pool)
                .await
                .unwrap();
            }
            "request" => {
                sqlx::query("UPDATE component_updates SET request_id=$2 WHERE machine_id=$1")
                    .bind(id)
                    .bind(orch_types::new_id())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        world
            .app
            .app
            .link
            .advance_component_updates()
            .await
            .unwrap();
        let row = updates::among(&mut *world.conn().await, &[id])
            .await
            .unwrap()
            .pop()
            .unwrap();
        if change == "unchanged" {
            assert_eq!(row.state, "updating");
            assert!(row.command_id.is_some());
            assert_eq!(counts(&world, id).await, (1, 1, 1));
        } else {
            assert_eq!(row.state, "refused");
            assert_eq!(counts(&world, id).await, (1, 0, 1));
        }
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_actual_drain_loop_rechecks_original_scope_raw_expiry_revocation_and_physical_source()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::actual_drain_loop_rechecks_original_scope_raw_expiry_revocation_and_physical_source(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn a_later_controller_does_not_replace_a_closed_original_update_issuer() {
    let world = World::new().await;
    let owner = crate::places::person(&world, "owner").await;
    let id = machine(&world, owner.id()).await;
    let first = key(
        &world,
        owner.id(),
        vec![rule("MachineUpdateNow", "machine", id, &["manage"])],
    )
    .await;
    let second = key(
        &world,
        owner.id(),
        vec![rule("MachineUpdateNow", "machine", id, &["manage"])],
    )
    .await;
    accepted(call(&world, &first.secret, id).await);
    let original = origin(&world, id).await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(first.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    accepted(call(&world, &second.secret, id).await);
    assert_eq!(origin(&world, id).await, original);
    world
        .app
        .app
        .link
        .advance_component_updates()
        .await
        .unwrap();
    assert_eq!(counts(&world, id).await, (1, 0, 2));
    assert_eq!(
        updates::among(&mut *world.conn().await, &[id])
            .await
            .unwrap()[0]
            .state,
        "refused"
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_later_controller_does_not_replace_a_closed_original_update_issuer()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::a_later_controller_does_not_replace_a_closed_original_update_issuer(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn retained_native_update_command_cannot_borrow_a_replacement_request_or_another_body() {
    let world = World::new().await;
    let owner = crate::places::person(&world, "owner").await;
    let id = machine(&world, owner.id()).await;
    let actor = key(
        &world,
        owner.id(),
        vec![rule("MachineUpdateNow", "machine", id, &["manage"])],
    )
    .await;
    accepted(call(&world, &actor.secret, id).await);
    world
        .app
        .app
        .link
        .advance_component_updates()
        .await
        .unwrap();
    let update = updates::among(&mut *world.conn().await, &[id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let command = update.command_id.unwrap();
    let kept = link::command_with_authority(&mut *world.conn().await, command)
        .await
        .unwrap()
        .unwrap();
    let private = kept.authority.unwrap();
    assert_eq!(private["kind"], "component_update");
    assert_eq!(
        private["authority"]["request"]["origin"]["operation"],
        "MachineUpdateNow"
    );
    let authority: orch_engine::commands::CommandAuthority =
        serde_json::from_value(private).unwrap();
    let access = crate::thread_access::AuthzAccess::new(world.app.app.clock.clone());
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &kept.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_some()
    );
    let mut wrong = kept.command.clone();
    wrong.body = json!({"recommended":"99.0.0"});
    assert!(
        authority
            .current_callers(&mut tx, &wrong, world.app.app.clock.as_ref(), &access)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":[rule("MachineUpdate","machine",id,&["manage"])]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &kept.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_none(),
        "an unaccepted queued update never borrows another operation at delivery"
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":[rule("MachineUpdateNow","machine",id,&["manage"])]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    updates::finish(&mut *world.conn().await, id, "refused", None, world.now())
        .await
        .unwrap();
    let old = origin(&world, id).await;
    accepted(call(&world, &actor.secret, id).await);
    assert_ne!(origin(&world, id).await, old);
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &kept.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_none()
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_retained_native_update_command_cannot_borrow_a_replacement_request_or_another_body()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::retained_native_update_command_cannot_borrow_a_replacement_request_or_another_body(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn explicit_update_rechecks_at_its_actual_physical_and_first_request_insert_waits() {
    for point in ["physical", "insert"] {
        for expire in [false, true] {
            let world = World::new().await;
            let owner = crate::places::person(&world, "owner").await;
            let id = machine(&world, owner.id()).await;
            let actor = key(
                &world,
                owner.id(),
                vec![rule("MachineUpdateNow", "machine", id, &["manage"])],
            )
            .await;
            let mut blocker = world.db.pool.begin().await.unwrap();
            if point == "physical" {
                sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
                    .bind(id)
                    .fetch_one(&mut *blocker)
                    .await
                    .unwrap();
            } else {
                sqlx::query("LOCK TABLE component_updates IN SHARE MODE")
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
            }
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = world.app.clone();
            let pending = tokio::spawn(async move {
                let request = app.call(MachineUpdateNow, &[&id]).token(&actor.secret);
                request.send_bytes().await
            });
            waited(
                &world,
                pid,
                &pending,
                if point == "physical" {
                    "%FROM machines%FOR %"
                } else {
                    "%INSERT INTO component_updates%"
                },
            )
            .await;
            if expire {
                world.advance(chrono::Duration::seconds(31));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
                .await
                .unwrap()
                .unwrap();
            if expire {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                assert_eq!(counts(&world, id).await, (0, 0, 0));
            } else {
                assert_eq!(accepted(reply), receipt(id));
                assert_eq!(counts(&world, id).await, (1, 0, 1));
            }
        }
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_explicit_update_rechecks_at_its_actual_physical_and_first_request_insert_waits()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::explicit_update_rechecks_at_its_actual_physical_and_first_request_insert_waits(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn completed_update_choice_keeps_one_request_when_response_read_closes_at_activity() {
    for close in [false, true] {
        let world = World::new().await;
        let owner = crate::places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let actor = key(
            &world,
            owner.id(),
            vec![rule("MachineUpdateNow", "machine", id, &["manage", "read"])],
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
        let pending = tokio::spawn(async move {
            let request = app.call(MachineUpdateNow, &[&id]).token(&secret);
            request.send_bytes().await
        });
        waited(&world, pid, &pending, "%INSERT INTO api_token_activity%").await;
        let original = origin(&world, id).await;
        assert_eq!(counts(&world, id).await, (1, 0, 1));
        if close {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(actor.id)
                .bind(json!({"rules":[rule("MachineUpdateNow","machine",id,&["manage"])]}))
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let value = accepted(
            tokio::time::timeout(std::time::Duration::from_secs(30), pending)
                .await
                .unwrap()
                .unwrap(),
        );
        if close {
            assert_eq!(value, receipt(id));
        } else {
            assert_eq!(value["status"], "queued");
        }
        assert_eq!(counts(&world, id).await, (1, 0, 1));
        assert_eq!(origin(&world, id).await, original);
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_completed_update_choice_keeps_one_request_when_response_read_closes_at_activity()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::completed_update_choice_keeps_one_request_when_response_read_closes_at_activity(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn minimum_version_handshake_never_lends_the_automatic_path_to_a_closed_explicit_origin() {
    let world = World::new().await;
    let owner = crate::places::person(&world, "owner").await;
    let mut conn = world.conn().await;
    let host = fixtures::insert_resource(
        &mut conn,
        "host",
        Some(owner.id()),
        Some(owner.person.personal_org),
        "Native update host",
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Host,
            id: host,
        },
        owner.id(),
        orch_authz::ownership::NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let id: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let policy = LocalPolicy {
        update_auto: true,
        ..LocalPolicy::default()
    };
    sqlx::query("UPDATE machines SET connector_version='0.0.9',roles=ARRAY['node'],online=true,facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1").bind(id).bind(json!(policy)).execute(&mut *conn).await.unwrap();
    let row = resources::machine(&mut conn, id).await.unwrap().unwrap();
    drop(conn);
    let actor = key(
        &world,
        owner.id(),
        vec![rule("MachineUpdateNow", "host", host, &["manage"])],
    )
    .await;
    let request = world
        .app
        .call(MachineUpdateNow, &[&id])
        .token(&actor.secret);
    assert_eq!(request.send_bytes().await.status, StatusCode::OK);
    let original = origin(&world, id).await;
    assert_eq!(original["origin"]["operation"], "MachineUpdateNow");
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let query = AuthQuery {
        purpose: Default::default(),
        capabilities: Vec::new(),
        endpoints: None,
        hosted_endpoints: None,
        n: 1,
        machine_id: id,
        fingerprint: hex::encode(row.cert_fingerprint),
        tls: TlsMode::Server,
        ip: "127.0.0.1".parse().unwrap(),
        connector_version: "0.0.9".into(),
        roles: vec![Role::Node],
    };
    let reply = world.app.app.link.authorize(&query).await;
    assert_eq!(reply.code, Some(RejectCode::UpgradeRequired));
    assert!(reply.update_permit.is_none());
    assert_eq!(origin(&world, id).await, original);
    assert_eq!(
        updates::among(&mut *world.conn().await, &[id])
            .await
            .unwrap()[0]
            .state,
        "refused"
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_minimum_version_handshake_never_lends_the_automatic_path_to_a_closed_explicit_origin()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::minimum_version_handshake_never_lends_the_automatic_path_to_a_closed_explicit_origin(
        ),
    )
}
