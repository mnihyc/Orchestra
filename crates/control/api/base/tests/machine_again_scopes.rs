//! Replacement tokens retain their exact original issuer through reservation and redemption.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::resources::MachineEnrollmentAgainTokenCreate;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::{
    identity::{self, NewApiToken},
    resources::access_fixtures as fixtures,
};
use orch_link_contract::{
    enrollment::EnrollRequest,
    session::{Arch, Os, Platform, Role},
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

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
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native replacement issuer",
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
    let id = fixtures::insert_persons_machine(
        &mut *world.conn().await,
        user,
        "Private retained identity",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE machines SET roles=ARRAY['agent'],facts=jsonb_build_object('installation',$2::jsonb) WHERE id=$1").bind(id)
        .bind(json!({"data_dir":"/private/retained-instance","config_file":"/private/retained.toml","service":null,"executable":"/private/connector"})).execute(&world.db.pool).await.unwrap();
    id
}
async fn create(world: &World, token: &str, id: Id) -> RawReply {
    let request = world
        .app
        .call(MachineEnrollmentAgainTokenCreate, &[&id])
        .token(token);
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
async fn count(world: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn state(world: &World, id: Id) -> (String, String, Option<String>, String) {
    sqlx::query_as(
        "SELECT status,cert_serial,cert_next_serial,xmin::text FROM machines WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}
fn request(token: &str, machine: Id) -> EnrollRequest {
    let pending = orch_pki::enrollment::PendingEnrollment::start().unwrap();
    EnrollRequest {
        machine_id: Some(machine),
        token: token.into(),
        csr_pem: pending.csr_pem().into(),
        name: "Replacement local configuration".into(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".into(),
        protocol_versions: vec![orch_link_contract::version::PROTOCOL_VERSION],
        roles: vec![Role::Agent],
    }
}
async fn waited<T>(world: &World, pid: i32, pending: &tokio::task::JoinHandle<T>, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!pending.is_finished(),"the actual native wait must be reached");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(&world.db.pool).await.unwrap();
            if blocked {break;}tokio::task::yield_now().await;
        }
    }).await.expect("the native PostgreSQL lock must actually block this request");
}

#[tokio::test]
async fn replacement_issuance_needs_exact_scope_and_keeps_the_secret_without_retained_installation_data()
 {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let stranger = places::person(&world, "stranger").await;
    let id = machine(&world, owner.id()).await;
    let other = machine(&world, owner.id()).await;
    for rules in [
        vec![rule("MachineUpdate", "machine", id, &["manage"])],
        vec![rule(
            "MachineEnrollmentAgainTokenCreate",
            "machine",
            id,
            &["read"],
        )],
        vec![rule(
            "MachineEnrollmentAgainTokenCreate",
            "environment",
            id,
            &["manage"],
        )],
        vec![rule(
            "MachineEnrollmentAgainTokenCreate",
            "machine",
            other,
            &["manage"],
        )],
    ] {
        let token = key(&world, owner.id(), rules).await;
        assert_eq!(
            create(&world, &token.secret, id).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(count(&world).await, 0);
    }
    let denied = key(
        &world,
        stranger.id(),
        vec![rule(
            "MachineEnrollmentAgainTokenCreate",
            "machine",
            id,
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &denied.secret, id).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(count(&world).await, 0);
    let writer = key(
        &world,
        owner.id(),
        vec![
            rule(
                "MachineEnrollmentAgainTokenCreate",
                "machine",
                id,
                &["manage"],
            ),
            rule("MachineGet", "machine", id, &["read"]),
        ],
    )
    .await;
    let receipt = accepted(create(&world, &writer.secret, id).await);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["machine_id"], json!(id));
    assert!(
        receipt["token"]
            .as_str()
            .unwrap()
            .parse::<orch_link_contract::enrollment::EnrollmentToken>()
            .is_ok()
    );
    assert!(receipt.get("commands").is_none());
    assert!(!receipt.to_string().contains("/private/retained"));
    let reader = key(
        &world,
        owner.id(),
        vec![rule(
            "MachineEnrollmentAgainTokenCreate",
            "machine",
            id,
            &["manage", "read"],
        )],
    )
    .await;
    for token in [&reader.secret, &owner.token] {
        let full = accepted(create(&world, token, id).await);
        assert!(
            full["commands"]["linux"]
                .as_str()
                .unwrap()
                .contains("/private/retained-instance")
        );
        assert!(
            full["commands"]["linux"]
                .as_str()
                .unwrap()
                .contains(full["token"].as_str().unwrap())
        );
    }
    assert_eq!(count(&world).await, 3);
}

#[tokio::test]
async fn replacement_redemption_keeps_the_original_key_operation_raw_lifetime_and_owner() {
    for change in [
        "unchanged",
        "scope",
        "revoked",
        "expired",
        "owner",
        "corrupt",
    ] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let other = places::person(&world, "other").await;
        let id = machine(&world, owner.id()).await;
        let actor = key(
            &world,
            owner.id(),
            vec![rule(
                "MachineEnrollmentAgainTokenCreate",
                "machine",
                id,
                &["manage"],
            )],
        )
        .await;
        let issued = accepted(create(&world, &actor.secret, id).await);
        let token = issued["token"].as_str().unwrap();
        let packet: Json = sqlx::query_scalar(
            "SELECT issuer_origin FROM enrollment_tokens WHERE again_machine_id=$1",
        )
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            packet["origin"]["operation"],
            "MachineEnrollmentAgainTokenCreate"
        );
        assert_eq!(packet["credential"]["kind"], "token");
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":[rule("MachineUpdate","machine",id,&["manage"])]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "revoked" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(world.now())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => world.advance(chrono::Duration::seconds(31)),
            "owner" => {
                sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
                    .bind(id)
                    .bind(other.id())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "corrupt" => {
                sqlx::query(
                    "UPDATE enrollment_tokens SET issuer_origin=$2 WHERE again_machine_id=$1",
                )
                .bind(id)
                .bind(json!({"invalid_origin":true}))
                .execute(&world.db.pool)
                .await
                .unwrap();
            }
            _ => {}
        }
        let before = state(&world, id).await;
        let result = world
            .app
            .app
            .link
            .certificates()
            .enroll(&request(token, id), &orch_db::audit::Origin::default())
            .await;
        let used: bool = sqlx::query_scalar(
            "SELECT used_at IS NOT NULL FROM enrollment_tokens WHERE again_machine_id=$1",
        )
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        if change == "unchanged" {
            assert_eq!(result.unwrap().machine_id, id);
            assert!(used);
            assert_ne!(state(&world, id).await, before);
        } else {
            assert_eq!(result.unwrap_err().code, "enrollment_issuer_closed");
            assert!(!used);
            assert_eq!(state(&world, id).await, before);
        }
    }
}

#[tokio::test]
async fn replacement_token_cannot_be_retargeted_to_another_machine_the_same_key_also_manages() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let id = machine(&world, owner.id()).await;
    let other = machine(&world, owner.id()).await;
    let actor = key(
        &world,
        owner.id(),
        vec![
            rule(
                "MachineEnrollmentAgainTokenCreate",
                "machine",
                id,
                &["manage"],
            ),
            rule(
                "MachineEnrollmentAgainTokenCreate",
                "machine",
                other,
                &["manage"],
            ),
        ],
    )
    .await;
    let issued = accepted(create(&world, &actor.secret, id).await);
    sqlx::query("UPDATE enrollment_tokens SET again_machine_id=$2 WHERE again_machine_id=$1")
        .bind(id)
        .bind(other)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let before = state(&world, other).await;
    let result = world
        .app
        .app
        .link
        .certificates()
        .enroll(
            &request(issued["token"].as_str().unwrap(), other),
            &orch_db::audit::Origin::default(),
        )
        .await;
    assert_eq!(result.unwrap_err().code, "enrollment_issuer_closed");
    assert_eq!(state(&world, other).await, before);
    let used: bool = sqlx::query_scalar(
        "SELECT used_at IS NOT NULL FROM enrollment_tokens WHERE again_machine_id=$1",
    )
    .bind(other)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert!(!used);
}

#[tokio::test]
async fn replacement_issuance_rechecks_after_its_actual_physical_lock_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let actor = key(
            &world,
            owner.id(),
            vec![rule(
                "MachineEnrollmentAgainTokenCreate",
                "machine",
                id,
                &["manage"],
            )],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(MachineEnrollmentAgainTokenCreate, &[&id])
                .token(&actor.secret);
            request.send_bytes().await
        });
        waited(&world, pid, &pending, "%FROM machines%FOR %").await;
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
            assert_eq!(count(&world).await, 0);
        } else {
            assert_eq!(accepted(reply)["projection"], "receipt");
            assert_eq!(count(&world).await, 1);
        }
    }
}

#[tokio::test]
async fn replacement_reservation_rolls_back_if_the_key_expires_at_its_actual_ledger_insert_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let actor = key(
            &world,
            owner.id(),
            vec![rule(
                "MachineEnrollmentAgainTokenCreate",
                "machine",
                id,
                &["manage"],
            )],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE enrollment_tokens IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(MachineEnrollmentAgainTokenCreate, &[&id])
                .token(&actor.secret);
            request.send_bytes().await
        });
        waited(&world, pid, &pending, "%INSERT INTO enrollment_tokens%").await;
        if expire {
            world.advance(chrono::Duration::seconds(31));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert_eq!(count(&world).await, 0);
        } else {
            assert_eq!(accepted(reply)["projection"], "receipt");
            assert_eq!(count(&world).await, 1);
        }
    }
}

#[tokio::test]
async fn replacement_redemption_rechecks_the_original_key_after_its_real_physical_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let actor = key(
            &world,
            owner.id(),
            vec![rule(
                "MachineEnrollmentAgainTokenCreate",
                "machine",
                id,
                &["manage"],
            )],
        )
        .await;
        let issued = accepted(create(&world, &actor.secret, id).await);
        let request = request(issued["token"].as_str().unwrap(), id);
        let before = state(&world, id).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.app.clone();
        let pending = tokio::spawn(async move {
            app.link
                .certificates()
                .enroll(&request, &orch_db::audit::Origin::default())
                .await
        });
        waited(&world, pid, &pending, "%FROM machines%FOR %").await;
        if expire {
            world.advance(chrono::Duration::seconds(31));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(reply.unwrap_err().code, "enrollment_issuer_closed");
            assert_eq!(state(&world, id).await, before);
        } else {
            assert_eq!(reply.unwrap().machine_id, id);
        }
    }
}

#[tokio::test]
async fn replacement_installation_readback_closes_after_commit_without_issuing_a_second_token() {
    for close in ["unchanged", "read", "installation"] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let id = machine(&world, owner.id()).await;
        let actor = key(
            &world,
            owner.id(),
            vec![rule(
                "MachineEnrollmentAgainTokenCreate",
                "machine",
                id,
                &["manage", "read"],
            )],
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
            let request = app
                .call(MachineEnrollmentAgainTokenCreate, &[&id])
                .token(&secret);
            request.send_bytes().await
        });
        waited(&world, pid, &pending, "%INSERT INTO api_token_activity%").await;
        assert_eq!(count(&world).await, 1);
        if close == "read" {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(actor.id).bind(json!({"rules":[rule("MachineEnrollmentAgainTokenCreate","machine",id,&["manage"])]})).execute(&world.db.pool).await.unwrap();
        }
        if close == "installation" {
            sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{installation,data_dir}',$2::jsonb) WHERE id=$1").bind(id).bind(json!("/private/new-instance")).execute(&world.db.pool).await.unwrap();
        }
        blocker.commit().await.unwrap();
        let value = accepted(
            tokio::time::timeout(std::time::Duration::from_secs(30), pending)
                .await
                .unwrap()
                .unwrap(),
        );
        if close == "unchanged" {
            assert!(
                value["commands"]["linux"]
                    .as_str()
                    .unwrap()
                    .contains("/private/retained-instance")
            );
        } else {
            assert_eq!(value["projection"], "receipt");
            assert!(value.get("commands").is_none());
            assert!(!value.to_string().contains("/private/"));
        }
        assert_eq!(count(&world).await, 1);
        assert!(value["token"].as_str().is_some());
    }
}
