//! Issuer-only native recovery, exact completed sources and frozen pending publication.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use http::{StatusCode, header::CACHE_CONTROL};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{EnrollmentProgress, MachineEnrollmentStatusGet, MachineGet};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{
    identity::{self, NewApiToken},
    resources::{self as rows, access_fixtures as fixtures},
};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const OP: &str = "MachineEnrollmentStatusGet";
struct Key {
    id: Id,
    secret: String,
}
struct Receipt {
    hash: Vec<u8>,
    value: String,
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, owner: Id, rules: Option<Vec<Json>>) -> Key {
    let scope = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        json!(scope)
    });
    let legacy = if scope.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: owner,
            name: "Native enrollment recovery scope",
            token_hash: &token_hash(&key.secret),
            scopes: &legacy,
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    key
}
async fn receipt(
    world: &World,
    issuer: Id,
    purpose: &str,
    home: Option<Id>,
    machine: Option<Id>,
) -> Receipt {
    let hash = token_hash(&new_token("test-enrollment_"));
    let receipt = Receipt {
        value: hex::encode(&hash),
        hash,
    };
    assert!(
        rows::insert_enrollment_token(
            &mut *world.conn().await,
            &rows::TokenRow {
                token_hash: receipt.hash.clone(),
                created_by: issuer,
                purpose: purpose.into(),
                org_id: home,
                name_hint: Some("Native progress fixture".into()),
                expires_at: world.now() + chrono::Duration::hours(1),
                used_at: None,
                machine_id: None,
                again_machine_id: None
            }
        )
        .await
        .unwrap()
    );
    if let Some(machine) = machine {
        assert!(
            rows::spend_enrollment_token(
                &mut *world.conn().await,
                &receipt.hash,
                machine,
                world.now()
            )
            .await
            .unwrap()
        );
    }
    receipt
}
async fn get(world: &World, token: &str, receipt: &str) -> RawReply {
    world
        .app
        .call(MachineEnrollmentStatusGet, &[&receipt])
        .token(token)
        .send_bytes()
        .await
}
fn shown(reply: RawReply) -> EnrollmentProgress {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(
        reply.headers[CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn machine(world: &World, owner: &places::Who, name: &str) -> Id {
    fixtures::insert_persons_machine(&mut *world.conn().await, owner.id(), name)
        .await
        .unwrap()
}
async fn own(world: &World, kind: ObjectKind, id: Id, owner: Id) {
    start_resource(
        &mut *world.conn().await,
        ObjectRef { kind, id },
        owner,
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
}
async fn host(world: &World, owner: &places::Who) -> (Id, Id) {
    let host = fixtures::insert_resource(
        &mut *world.conn().await,
        "host",
        Some(owner.id()),
        Some(owner.person.personal_org),
        "Native recovered host",
    )
    .await
    .unwrap();
    own(world, ObjectKind::Host, host, owner.id()).await;
    let machine = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    (host, machine)
}
async fn environment(world: &World, owner: &places::Who, machine: Option<Id>) -> Id {
    let id = fixtures::insert_environment(
        &mut *world.conn().await,
        owner.id(),
        Some(owner.person.personal_org),
        "Native recovery environment",
        if machine.is_some() {
            "connected"
        } else {
            "managed"
        },
    )
    .await
    .unwrap();
    if let Some(machine) = machine {
        fixtures::register_on(&mut *world.conn().await, "environment", id, machine)
            .await
            .unwrap();
    }
    own(world, ObjectKind::Environment, id, owner.id()).await;
    id
}
async fn images(world: &World, host: Id, environment: Id, generation: i64) {
    sqlx::query("UPDATE hosts SET kept_images=$2 WHERE id=$1").bind(host).bind(json!({"epoch":7,"images":[{"system":"test","environment":environment,"generation":generation,"capture":new_id(),"reference":format!("orch-keep-{environment}:{generation}"),"bytes":768,"total_bytes":900,"layers":2}]})).execute(&world.db.pool).await.unwrap();
}
async fn activity_wait(world: &World, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the native progress response missed activity bookkeeping");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("the actual progress response must reach its blocked activity INSERT");
}

#[tokio::test]
async fn enrollment_progress_requires_the_real_issuer_account_for_pending_state() {
    let world = World::new().await;
    let issuer = places::person(&world, "issuer").await;
    let other = places::person(&world, "other").await;
    let receipt = receipt(&world, issuer.id(), "connected", None, None).await;
    let scoped = key(
        &world,
        issuer.id(),
        Some(vec![rule(OP, "account", issuer.id())]),
    )
    .await;
    let pending = shown(get(&world, &scoped.secret, &receipt.value).await);
    assert_eq!(
        json!(pending),
        json!(shown(get(&world, &issuer.token, &receipt.value).await))
    );
    assert!(pending.machine.is_none() && pending.host.is_none());
    for wrong in [
        rule(OP, "machine", issuer.id()),
        rule("MachineEnrollmentInfoGet", "account", issuer.id()),
    ] {
        let wrong = key(&world, issuer.id(), Some(vec![wrong])).await;
        assert_eq!(
            get(&world, &wrong.secret, &receipt.value).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let other_key = key(
        &world,
        other.id(),
        Some(vec![rule(OP, "account", other.id())]),
    )
    .await;
    assert_eq!(
        get(&world, &other_key.secret, &receipt.value).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&world, &scoped.secret, "invalid-receipt").await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&world, &scoped.secret, &hex::encode([0; 32]))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let legacy = key(&world, issuer.id(), None).await;
    assert_eq!(
        world
            .app
            .call(MachineEnrollmentStatusGet, &[&receipt.value])
            .token(&legacy.secret)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSessionRequired
    );
    sqlx::query("UPDATE enrollment_tokens SET expires_at=$2 WHERE token_hash=$1")
        .bind(&receipt.hash)
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        shown(get(&world, &scoped.secret, &receipt.value).await)
            .machine
            .is_none(),
        "recovery is not enrollment-token use"
    );
}

#[tokio::test]
async fn enrollment_progress_completed_machine_keeps_native_readiness_under_its_own_operation() {
    let world = World::new().await;
    let issuer = places::person(&world, "issuer").await;
    let machine = machine(&world, &issuer, "Native recovered machine").await;
    let environment = environment(&world, &issuer, Some(machine)).await;
    let receipt = receipt(&world, issuer.id(), "connected", None, Some(machine)).await;
    let scoped = key(
        &world,
        issuer.id(),
        Some(vec![
            rule(OP, "account", issuer.id()),
            rule(OP, "machine", machine),
            rule(OP, "environment", environment),
            rule(OP, "org", issuer.person.personal_org),
        ]),
    )
    .await;
    let progress = shown(get(&world, &scoped.secret, &receipt.value).await);
    let native = world
        .app
        .call(MachineGet, &[&machine])
        .token(&issuer.token)
        .send()
        .await
        .ok();
    assert_eq!(json!(progress.machine.as_ref().unwrap()), json!(native));
    assert!(progress.host.is_none());
    assert_eq!(
        progress.machine.unwrap().environment_sandboxes[0]
            .environment
            .id,
        environment
    );
    let partial = key(
        &world,
        issuer.id(),
        Some(vec![
            rule(OP, "account", issuer.id()),
            rule(OP, "machine", machine),
            rule("MachineGet", "environment", environment),
        ]),
    )
    .await;
    let partial = shown(get(&world, &partial.secret, &receipt.value).await)
        .machine
        .unwrap();
    assert_eq!(partial.base.id, machine);
    assert!(partial.environments.is_empty());
    assert!(partial.environment_sandboxes.is_empty());
    for rules in [
        vec![rule(OP, "machine", machine)],
        vec![
            rule(OP, "account", issuer.id()),
            rule("MachineGet", "machine", machine),
        ],
    ] {
        let wrong = key(&world, issuer.id(), Some(rules)).await;
        assert_eq!(
            get(&world, &wrong.secret, &receipt.value).await.status,
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn enrollment_progress_uses_the_actual_business_kind_of_the_completed_host_facade() {
    let world = World::new().await;
    let issuer = places::person(&world, "issuer").await;
    let (host, machine) = host(&world, &issuer).await;
    let host_receipt = receipt(
        &world,
        issuer.id(),
        "host",
        Some(issuer.person.personal_org),
        Some(machine),
    )
    .await;
    let machine_receipt = receipt(&world, issuer.id(), "connected", None, Some(machine)).await;
    let scoped = key(
        &world,
        issuer.id(),
        Some(vec![
            rule(OP, "account", issuer.id()),
            rule(OP, "host", host),
            rule(OP, "org", issuer.person.personal_org),
        ]),
    )
    .await;
    let progress = shown(get(&world, &scoped.secret, &host_receipt.value).await);
    assert!(progress.machine.is_none());
    assert_eq!(progress.host.as_ref().unwrap().base.id, host);
    assert_eq!(progress.host.unwrap().machine.id, machine);
    let restricted = key(&world, issuer.id(), Some(vec![rule(OP, "account", issuer.id()), json!({"target":{"kind":"host","id":host,"reach":"self"},"resources":["machine"],"allow":[{"operation":OP,"actions":["read"]}]})])).await;
    assert_eq!(
        shown(get(&world, &restricted.secret, &machine_receipt.value).await)
            .machine
            .unwrap()
            .base
            .id,
        machine
    );
    assert_eq!(
        get(&world, &restricted.secret, &host_receipt.value)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let alias = key(
        &world,
        issuer.id(),
        Some(vec![
            rule(OP, "account", issuer.id()),
            rule(OP, "machine", machine),
        ]),
    )
    .await;
    assert_eq!(
        get(&world, &alias.secret, &host_receipt.value).await.status,
        StatusCode::FORBIDDEN
    );
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        commands, 0,
        "passive enrollment recovery does not issue Host probe work"
    );
}

#[tokio::test]
async fn enrollment_progress_host_retained_names_need_its_own_read_while_intrinsic_tuples_remain() {
    let world = World::new().await;
    let issuer = places::person(&world, "issuer").await;
    let (host, machine) = host(&world, &issuer).await;
    let environment = environment(&world, &issuer, None).await;
    images(&world, host, environment, 4).await;
    let receipt = receipt(
        &world,
        issuer.id(),
        "host",
        Some(issuer.person.personal_org),
        Some(machine),
    )
    .await;
    for operation in [None, Some("HostGet"), Some(OP)] {
        let mut rules = vec![rule(OP, "account", issuer.id()), rule(OP, "host", host)];
        if let Some(operation) = operation {
            rules.push(rule(operation, "environment", environment));
            rules.push(rule(operation, "org", issuer.person.personal_org));
        }
        let key = key(&world, issuer.id(), Some(rules)).await;
        let value = shown(get(&world, &key.secret, &receipt.value).await)
            .host
            .unwrap();
        let layer = &value.kept_layers[0];
        assert_eq!(layer.environment_id, environment);
        assert_eq!(layer.generation, 4);
        assert_eq!(layer.bytes, 768);
        if operation == Some(OP) {
            assert_eq!(layer.environment.as_ref().unwrap().id, environment);
        } else {
            assert!(layer.environment.is_none());
        }
    }
}

#[tokio::test]
async fn enrollment_progress_pending_publication_does_not_borrow_future_machine_authority() {
    let world = World::new().await;
    let issuer = places::person(&world, "issuer").await;
    let machine = machine(&world, &issuer, "Future completed machine").await;
    let receipt = receipt(&world, issuer.id(), "connected", None, None).await;
    let scoped = key(
        &world,
        issuer.id(),
        Some(vec![rule(OP, "account", issuer.id())]),
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
    let secret = scoped.secret.clone();
    let receipt_value = receipt.value.clone();
    let request = tokio::spawn(async move {
        let request = app
            .call(MachineEnrollmentStatusGet, &[&receipt_value])
            .token(&secret);
        request.send_bytes().await
    });
    activity_wait(&world, pid, &request).await;
    assert!(
        rows::spend_enrollment_token(
            &mut *world.conn().await,
            &receipt.hash,
            machine,
            world.now()
        )
        .await
        .unwrap()
    );
    blocker.commit().await.unwrap();
    let value = shown(
        tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap(),
    );
    assert!(value.machine.is_none() && value.host.is_none());
    assert_eq!(
        get(&world, &scoped.secret, &receipt.value).await.status,
        StatusCode::FORBIDDEN
    );
    let current = key(
        &world,
        issuer.id(),
        Some(vec![
            rule(OP, "account", issuer.id()),
            rule(OP, "machine", machine),
        ]),
    )
    .await;
    assert_eq!(
        shown(get(&world, &current.secret, &receipt.value).await)
            .machine
            .unwrap()
            .base
            .id,
        machine
    );
}

#[tokio::test]
async fn enrollment_progress_publication_rechecks_the_issued_receipt_and_raw_key_after_activity_wait()
 {
    for change in ["none", "expired", "receipt_machine"] {
        let world = World::new().await;
        let issuer = places::person(&world, "issuer").await;
        let original = machine(&world, &issuer, "Original completed source").await;
        let replacement = machine(&world, &issuer, "Replacement completed source").await;
        let receipt = receipt(&world, issuer.id(), "connected", None, Some(original)).await;
        let token = key(
            &world,
            issuer.id(),
            Some(vec![
                rule(OP, "account", issuer.id()),
                rule(OP, "machine", original),
                rule(OP, "machine", replacement),
            ]),
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
        let secret = token.secret.clone();
        let value = receipt.value.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(MachineEnrollmentStatusGet, &[&value])
                .token(&secret);
            request.send_bytes().await
        });
        activity_wait(&world, pid, &request).await;
        if change == "expired" {
            sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                .bind(token.id)
                .bind(world.now())
                .execute(&world.db.pool)
                .await
                .unwrap();
        } else if change == "receipt_machine" {
            sqlx::query("UPDATE enrollment_tokens SET machine_id=$2 WHERE token_hash=$1")
                .bind(&receipt.hash)
                .bind(replacement)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if change == "none" {
            assert_eq!(shown(reply).machine.unwrap().base.id, original);
        } else if change == "expired" {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Original completed source"));
            assert_eq!(
                shown(get(&world, &token.secret, &receipt.value).await)
                    .machine
                    .unwrap()
                    .base
                    .id,
                replacement
            );
        }
    }
}

#[tokio::test]
async fn enrollment_progress_publication_rechecks_native_retained_layer_generation() {
    let world = World::new().await;
    let issuer = places::person(&world, "issuer").await;
    let (host, machine) = host(&world, &issuer).await;
    let environment = environment(&world, &issuer, None).await;
    images(&world, host, environment, 4).await;
    let receipt = receipt(
        &world,
        issuer.id(),
        "host",
        Some(issuer.person.personal_org),
        Some(machine),
    )
    .await;
    let token = key(
        &world,
        issuer.id(),
        Some(vec![
            rule(OP, "account", issuer.id()),
            rule(OP, "host", host),
            rule(OP, "environment", environment),
        ]),
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
    let secret = token.secret.clone();
    let value = receipt.value.clone();
    let request = tokio::spawn(async move {
        let request = app
            .call(MachineEnrollmentStatusGet, &[&value])
            .token(&secret);
        request.send_bytes().await
    });
    activity_wait(&world, pid, &request).await;
    images(&world, host, environment, 5).await;
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(
        shown(get(&world, &token.secret, &receipt.value).await)
            .host
            .unwrap()
            .kept_layers[0]
            .generation,
        5
    );
}

#[tokio::test]
async fn enrollment_progress_names_a_real_retained_volume_only_under_current_environment_read() {
    let world = World::new().await;
    let issuer = places::person(&world, "issuer").await;
    let (host, machine) = host(&world, &issuer).await;
    let environment = environment(&world, &issuer, None).await;
    let purge_at = world.now() + chrono::Duration::days(7);
    let runtime = new_id();
    sqlx::query("UPDATE environments SET status='removed',deleted_at=$2,removed_keep_days=7,volumes_purge_at=$3,removal_phase='kept',removal_revision=17,retained_volumes=$4,retained_bytes=NULL WHERE id=$1").bind(environment).bind(world.now()).bind(purge_at).bind(json!([{"runtime_id":runtime,"environment_id":environment,"host_id":host,"driver":"test","role":"workspace","name":"private-kept-volume","storage":"fast"}])).execute(&world.db.pool).await.unwrap();
    let receipt = receipt(
        &world,
        issuer.id(),
        "host",
        Some(issuer.person.personal_org),
        Some(machine),
    )
    .await;
    for named in [false, true] {
        let mut rules = vec![rule(OP, "account", issuer.id()), rule(OP, "host", host)];
        if named {
            rules.push(rule(OP, "environment", environment));
        }
        let token = key(&world, issuer.id(), Some(rules)).await;
        let value = shown(get(&world, &token.secret, &receipt.value).await)
            .host
            .unwrap();
        assert_eq!(value.kept_data.len(), 1);
        let data = &value.kept_data[0];
        assert_eq!(data.environment_id, environment);
        assert_eq!(
            data.name.as_deref(),
            named.then_some("Native recovery environment")
        );
        assert_eq!(data.purge_at, Some(Time::new(purge_at)));
        assert!(!json!(value).to_string().contains("private-kept-volume"));
    }
}
