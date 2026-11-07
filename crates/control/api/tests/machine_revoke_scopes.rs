//! Withdrawal keeps its actual physical/facade subject, native cancellation, and completed Read boundary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::resources::MachineRevoke;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::link::{self, NewCommand};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}
fn scope(rules: Vec<Json>) -> Json {
    let value: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(value.validate().is_empty());
    json!(value)
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native Machine withdrawal",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope(rules)),
    )
    .await
    .unwrap();
    key
}
async fn machine(world: &World, owner: Id) -> Id {
    fixtures::insert_persons_machine(&mut *world.conn().await, owner, "Private native Machine")
        .await
        .unwrap()
}
async fn command(world: &World, machine: Id, state: &str) -> Id {
    let id = new_id();
    let mut conn = world.conn().await;
    link::insert_command(
        &mut conn,
        &NewCommand {
            id,
            machine_id: machine,
            kind: "machine.probe",
            body: &json!({}),
            serial_key: Some("machine"),
            deadline_at: world.now() + chrono::Duration::hours(3),
        },
        world.now(),
    )
    .await
    .unwrap();
    if state != "queued" {
        sqlx::query("UPDATE commands SET state=$2 WHERE id=$1")
            .bind(id)
            .bind(state)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    id
}
async fn revoke(world: &World, token: &str, machine: Id) -> RawReply {
    let request = world.app.call(MachineRevoke, &[&machine]).token(token);
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
fn receipt(machine: Id) -> Json {
    json!({"projection":"receipt","machine_id":machine})
}
async fn state(world: &World, machine: Id) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('status',m.status,'online',m.online,'xmin',m.xmin::text,'commands',COALESCE((SELECT jsonb_agg(jsonb_build_array(c.id,c.state,c.xmin::text) ORDER BY c.id) FROM commands c WHERE c.machine_id=m.id),'[]'::jsonb),'audits',(SELECT count(*) FROM audit_log WHERE object_id=m.id AND action='machine.revoked')) FROM machines m WHERE m.id=$1")
        .bind(machine).fetch_one(&world.db.pool).await.unwrap()
}
async fn wait(world: &World, pid: i32, task: &tokio::task::JoinHandle<RawReply>, pattern: &str) {
    tokio::time::timeout(WAIT, async { loop {
        assert!(!task.is_finished(), "the actual native wait was not reached");
        let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
            .bind(pid).bind(pattern).fetch_one(&world.db.pool).await.unwrap();
        if blocked { break; }
        tokio::task::yield_now().await;
    }}).await.expect("observe the literal blocker and actual native query");
}

#[tokio::test]
async fn manage_only_withdrawal_cancels_the_native_open_commands_once_and_keeps_its_receipt() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let machine = machine(&world, owner.id()).await;
    for state in ["queued", "sent", "accepted"] {
        command(&world, machine, state).await;
    }
    let finished = command(&world, machine, "succeeded").await;
    let token = key(
        &world,
        owner.id(),
        vec![
            rule("MachineRevoke", "machine", machine, &["manage"]),
            rule("MachineGet", "machine", machine, &["read"]),
        ],
    )
    .await;
    assert_eq!(
        accepted(revoke(&world, &token.secret, machine).await),
        receipt(machine)
    );
    let stored = state(&world, machine).await;
    assert_eq!(stored["status"], "revoked");
    assert_eq!(stored["online"], false);
    assert_eq!(stored["audits"], 1);
    let commands: Vec<(Id, String)> =
        sqlx::query_as("SELECT id,state FROM commands WHERE machine_id=$1")
            .bind(machine)
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(commands.len(), 4);
    assert!(commands.iter().all(|(id, state)| state
        == if *id == finished {
            "succeeded"
        } else {
            "cancelled"
        }));
    assert_eq!(
        accepted(revoke(&world, &token.secret, machine).await),
        receipt(machine)
    );
    assert_eq!(
        state(&world, machine).await,
        stored,
        "native replay writes no row, cancellation or audit again"
    );
    let full = key(
        &world,
        owner.id(),
        vec![rule(
            "MachineRevoke",
            "machine",
            machine,
            &["manage", "read"],
        )],
    )
    .await;
    let shown = accepted(revoke(&world, &full.secret, machine).await);
    let ordinary = accepted(revoke(&world, &owner.token, machine).await);
    assert_eq!(
        shown, ordinary,
        "the operation's own Read preserves the native body"
    );
    assert_eq!(shown["status"], "revoked");
    assert_eq!(shown["name"], "Private native Machine");
    assert_eq!(state(&world, machine).await, stored);
}

#[tokio::test]
async fn withdrawal_binds_the_actual_machine_or_host_facade_and_never_lends_an_operation_or_native_gate()
 {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let stranger = places::person(&world, "stranger").await;
    let machine = machine(&world, owner.id()).await;
    command(&world, machine, "queued").await;
    let before = state(&world, machine).await;
    for rules in [
        vec![rule("MachineUpdate", "machine", machine, &["manage"])],
        vec![rule("MachineRevoke", "environment", machine, &["manage"])],
        vec![rule("MachineRevoke", "machine", machine, &["read"])],
        vec![rule("MachineRevoke", "machine", new_id(), &["manage"])],
    ] {
        let wrong = key(&world, owner.id(), rules).await;
        assert_eq!(
            revoke(&world, &wrong.secret, machine).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(state(&world, machine).await, before);
    }
    let unowned = key(
        &world,
        stranger.id(),
        vec![rule("MachineRevoke", "machine", machine, &["manage"])],
    )
    .await;
    assert_eq!(
        revoke(&world, &unowned.secret, machine).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(state(&world, machine).await, before);
    let host = fixtures::insert_resource(
        &mut *world.conn().await,
        "host",
        Some(owner.id()),
        Some(owner.person.personal_org),
        "Actual node facade",
    )
    .await
    .unwrap();
    start_resource(
        &mut *world.conn().await,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        owner.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let physical: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let active = state(&world, physical).await;
    for wrong in [
        rule("MachineRevoke", "machine", physical, &["manage"]),
        rule("HostUpdate", "host", host, &["manage"]),
    ] {
        let token = key(&world, owner.id(), vec![wrong]).await;
        assert_eq!(
            revoke(&world, &token.secret, physical).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(state(&world, physical).await, active);
    }
    let token = key(
        &world,
        owner.id(),
        vec![rule("MachineRevoke", "host", host, &["manage", "read"])],
    )
    .await;
    let full = accepted(revoke(&world, &token.secret, physical).await);
    assert_eq!(full["id"], json!(physical));
    assert_eq!(full["status"], "revoked");
    assert_eq!(
        state(&world, machine).await,
        before,
        "another native machine was never substituted"
    );
}

#[tokio::test]
async fn withdrawal_readback_requires_its_own_actual_registered_sources_without_losing_the_act() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let machine = machine(&world, owner.id()).await;
    let mut conn = world.conn().await;
    let provider = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(other.id()),
        Some(other.person.personal_org),
        "private-registration",
    )
    .await
    .unwrap();
    fixtures::register_on(&mut conn, "provider", provider, machine)
        .await
        .unwrap();
    drop(conn);
    let primary = rule("MachineRevoke", "machine", machine, &["manage", "read"]);
    let missing = key(
        &world,
        owner.id(),
        vec![
            primary.clone(),
            rule("MachineGet", "provider", provider, &["read"]),
        ],
    )
    .await;
    assert_eq!(
        accepted(revoke(&world, &missing.secret, machine).await),
        receipt(machine)
    );
    assert_eq!(state(&world, machine).await["status"], "revoked");
    let readable = key(
        &world,
        owner.id(),
        vec![
            primary,
            rule("MachineRevoke", "provider", provider, &["read"]),
        ],
    )
    .await;
    let full = accepted(revoke(&world, &readable.secret, machine).await);
    assert_eq!(full["providers"][0]["id"], json!(provider));
    assert_eq!(full["providers"][0]["name"], "private-registration");
    assert_eq!(state(&world, machine).await["audits"], 1);
}

#[tokio::test]
async fn withdrawal_rechecks_raw_expiry_after_real_machine_facade_and_command_waits() {
    for footprint in ["machine", "facade", "command"] {
        for expire in [false, true] {
            let world = World::new().await;
            let owner = places::person(&world, "owner").await;
            let (machine, kind, subject) = if footprint == "facade" {
                let host = fixtures::insert_resource(
                    &mut *world.conn().await,
                    "host",
                    Some(owner.id()),
                    Some(owner.person.personal_org),
                    "Serialized node facade",
                )
                .await
                .unwrap();
                start_resource(
                    &mut *world.conn().await,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                    owner.id(),
                    NewAccess::OnlyMe,
                    Time::new(world.now()),
                )
                .await
                .unwrap();
                let physical: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
                    .bind(host)
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
                (physical, "host", host)
            } else {
                let id = machine(&world, owner.id()).await;
                (id, "machine", id)
            };
            let command = command(&world, machine, "queued").await;
            let before = state(&world, machine).await;
            let token = key(
                &world,
                owner.id(),
                vec![rule("MachineRevoke", kind, subject, &["manage"])],
            )
            .await;
            let mut blocker = world.db.pool.begin().await.unwrap();
            let pattern = match footprint {
                "machine" => {
                    sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
                        .bind(machine)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%FROM machines%FOR UPDATE%"
                }
                "facade" => {
                    sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR UPDATE")
                        .bind(subject)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%FROM hosts%FOR SHARE%"
                }
                _ => {
                    sqlx::query("SELECT id FROM commands WHERE id=$1 FOR UPDATE")
                        .bind(command)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%FROM commands%FOR UPDATE%"
                }
            };
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = world.app.clone();
            let task = tokio::spawn(async move {
                let request = app.call(MachineRevoke, &[&machine]).token(&token.secret);
                request.send_bytes().await
            });
            wait(&world, pid, &task, pattern).await;
            if expire {
                world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
            if expire {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                assert_eq!(
                    state(&world, machine).await,
                    before,
                    "no native first effect after its actual footprint wait"
                );
            } else {
                assert_eq!(accepted(reply), receipt(machine));
                assert_eq!(state(&world, machine).await["status"], "revoked");
                assert_eq!(state(&world, machine).await["audits"], 1);
            }
        }
    }
}

#[tokio::test]
async fn withdrawal_rolls_back_its_native_ledger_if_raw_expiry_crosses_the_actual_audit_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let machine = machine(&world, owner.id()).await;
        command(&world, machine, "queued").await;
        let before = state(&world, machine).await;
        let token = key(
            &world,
            owner.id(),
            vec![rule("MachineRevoke", "machine", machine, &["manage"])],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE audit_log IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let task = tokio::spawn(async move {
            let request = app.call(MachineRevoke, &[&machine]).token(&token.secret);
            request.send_bytes().await
        });
        wait(&world, pid, &task, "%INSERT INTO audit_log%").await;
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(
                state(&world, machine).await,
                before,
                "registration, cancellation and audit roll back together"
            );
        } else {
            assert_eq!(accepted(reply), receipt(machine));
            assert_eq!(state(&world, machine).await["status"], "revoked");
            assert_eq!(state(&world, machine).await["audits"], 1);
        }
    }
}

#[tokio::test]
async fn completed_withdrawal_publication_keeps_its_receipt_and_never_cancels_or_writes_again() {
    for change in [
        "none",
        "input_manage",
        "response_read",
        "expired",
        "registration",
    ] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let other = places::person(&world, "other").await;
        let machine = machine(&world, owner.id()).await;
        let elsewhere = self::machine(&world, other.id()).await;
        command(&world, machine, "queued").await;
        let mut conn = world.conn().await;
        let provider = fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(other.id()),
            Some(other.person.personal_org),
            "original-private-registration",
        )
        .await
        .unwrap();
        fixtures::register_on(&mut conn, "provider", provider, machine)
            .await
            .unwrap();
        drop(conn);
        let rules = vec![
            rule("MachineRevoke", "machine", machine, &["manage", "read"]),
            rule("MachineRevoke", "provider", provider, &["read"]),
        ];
        let token = key(&world, owner.id(), rules).await;
        let token_id = token.id;
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
        let task = tokio::spawn(async move {
            let request = app.call(MachineRevoke, &[&machine]).token(&token.secret);
            request.send_bytes().await
        });
        wait(&world, pid, &task, "%INSERT INTO api_token_activity%").await;
        let committed = state(&world, machine).await;
        assert_eq!(committed["status"], "revoked");
        assert_eq!(committed["audits"], 1);
        if change == "input_manage" || change == "response_read" {
            let actions: &[&str] = if change == "input_manage" {
                &["read"]
            } else {
                &["manage"]
            };
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token_id)
                .bind(scope(vec![
                    rule("MachineRevoke", "machine", machine, actions),
                    rule("MachineRevoke", "provider", provider, &["read"]),
                ]))
                .execute(&world.db.pool)
                .await
                .unwrap();
        } else if change == "expired" {
            world.advance(chrono::Duration::hours(2));
        } else if change == "registration" {
            fixtures::register_on(&mut *world.conn().await, "provider", provider, elsewhere)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let shown = accepted(tokio::time::timeout(WAIT, task).await.unwrap().unwrap());
        if change == "none" || change == "input_manage" {
            assert_eq!(shown["status"], "revoked");
            assert_eq!(
                shown["providers"][0]["name"],
                "original-private-registration"
            );
        } else {
            assert_eq!(shown, receipt(machine));
        }
        assert_eq!(
            state(&world, machine).await,
            committed,
            "completed publication cannot repeat revocation, cancellation or audit"
        );
    }
}
