//! Real stored machine facts register a profile through separate machine/home scope roles.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::new_id;

const OP: &str = "MachineAgentCreate";
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
fn rule(
    kind: &str,
    id: Id,
    reach: &str,
    business: &str,
    operation: &str,
    actions: &[&str],
) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":[business],"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(s: &Scene, user: Id, rules: Vec<Value>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Stored machine profile creation",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    key
}
async fn rules(s: &Scene, read: bool) -> Vec<Value> {
    let host: Option<Id> = sqlx::query_scalar("SELECT id FROM hosts WHERE machine_id=$1")
        .bind(s.rig.machine_id)
        .fetch_optional(s.rig.db.pool())
        .await
        .unwrap();
    let (kind, machine) = host.map_or(("machine", s.rig.machine_id), |host| ("host", host));
    let home = s.tree.org.base.id;
    let mut rules = vec![
        rule(kind, machine, "self", "machine", OP, &["write"]),
        rule("org", home, "self", "agent", OP, &["manage"]),
    ];
    if read {
        rules.push(rule(kind, machine, "self", "machine", OP, &["read"]));
        rules.push(rule("org", home, "subtree", "agent", OP, &["read"]));
        rules.push(rule("org", home, "self", "org", OP, &["read"]));
    }
    rules
}
fn body(s: &Scene, name: &str) -> MachineAgentCreateBody {
    MachineAgentCreateBody {
        home_id: s.tree.org.base.id,
        harness_kind: "fake".into(),
        name: name.into(),
        profile: Some(json!({"config":{"model":"native-test"}})),
    }
}
async fn send(s: &Scene, token: &str, body: &MachineAgentCreateBody) -> RawReply {
    let request = s
        .rig
        .world
        .app
        .call(MachineAgentCreate, &[&s.rig.machine_id])
        .token(token)
        .body(body);
    request.send_bytes().await
}
fn accepted(reply: RawReply) -> Value {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn state(s: &Scene, name: &str) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM agents WHERE machine_id=$1 AND name=$2),(SELECT count(*) FROM permission_entries WHERE object_kind='agent'),(SELECT count(*) FROM audit_log WHERE object_kind='agent')").bind(s.rig.machine_id).bind(name).fetch_one(s.rig.db.pool()).await.unwrap()
}
async fn wait(s: &Scene, pid: i32, request: &tokio::task::JoinHandle<RawReply>, pattern: &str) {
    tokio::time::timeout(WAIT,async { loop {
        assert!(!request.is_finished(),"the actual native wait must be reached");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(s.rig.db.pool()).await.unwrap();
        if blocked { break; } tokio::task::yield_now().await;
    }}).await.expect("observe the literal blocker and actual native query");
}

#[tokio::test]
async fn profile_registration_requires_machine_write_and_its_actual_agent_home_without_operation_loans()
 {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let native = rules(&s, false).await;
    for mut denied in [
        vec![native[0].clone()],
        vec![native[1].clone()],
        native.clone(),
    ] {
        if denied.len() == 2 {
            denied[1]["allow"][0]["operation"] = json!("OrgAgentCreate");
        }
        let key = key(&s, s.ada.id(), denied).await;
        let body = body(&s, "Denied profile");
        let before = state(&s, &body.name).await;
        assert_eq!(
            send(&s, &key.secret, &body).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(state(&s, &body.name).await, before);
    }
    let key = key(&s, s.bea.id(), native).await;
    assert!(
        !send(&s, &key.secret, &body(&s, "Native non-owner"))
            .await
            .status
            .is_success(),
        "scope lends no ordinary Machine Use"
    );
}

#[tokio::test]
async fn stored_machine_registration_keeps_write_only_receipts_and_its_own_full_native_profile() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let mut limited = rules(&s, false).await;
    limited.push(rule(
        "org",
        s.tree.org.base.id,
        "subtree",
        "agent",
        "AgentGet",
        &["read"],
    ));
    let writer = key(&s, s.ada.id(), limited).await;
    let supplied = body(&s, "Write-only native profile");
    let value = accepted(send(&s, &writer.secret, &supplied).await);
    let id: Id = serde_json::from_value(value["agent_id"].clone()).unwrap();
    assert_eq!(value, json!({"projection":"receipt","agent_id":id}));
    let stored: (Id, Option<Id>, String, Value) =
        sqlx::query_as("SELECT home_id,machine_id,harness_kind,profile FROM agents WHERE id=$1")
            .bind(id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        (stored.0, stored.1, stored.2),
        (supplied.home_id, Some(s.rig.machine_id), "fake".into())
    );
    assert_eq!(stored.3["config"]["model"], "native-test");
    let full = key(&s, s.ada.id(), rules(&s, true).await).await;
    let shown = accepted(send(&s, &full.secret, &body(&s, "Full native profile")).await);
    assert_eq!(shown["name"], "Full native profile");
    assert_eq!(shown["harness_kind"], "fake");
    assert_eq!(shown["profile"]["config"]["model"], "native-test");
}

#[tokio::test]
async fn registration_rechecks_raw_expiry_after_actual_home_and_machine_footprints() {
    for stage in ["home", "machine", "audit"] {
        let s = Scene::new().await;
        ada_owns_the_machine(&s, reported()).await;
        let supplied = body(&s, "Held native registration");
        let key = key(&s, s.ada.id(), rules(&s, true).await).await;
        let before = state(&s, &supplied.name).await;
        let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
        let pattern = match stage {
            "home" => {
                sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                    .bind(supplied.home_id)
                    .fetch_one(&mut *blocker)
                    .await
                    .unwrap();
                "%SELECT id FROM nodes WHERE id=$1 FOR SHARE%"
            }
            "machine" => {
                sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
                    .bind(s.rig.machine_id)
                    .fetch_one(&mut *blocker)
                    .await
                    .unwrap();
                "%SELECT id FROM machines WHERE id=$1 FOR UPDATE%"
            }
            _ => {
                sqlx::query("LOCK TABLE audit_log IN SHARE MODE")
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
                "%INSERT INTO audit_log%"
            }
        };
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.rig.world.app.clone();
        let machine = s.rig.machine_id;
        let secret = key.secret.clone();
        let held = supplied.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(MachineAgentCreate, &[&machine])
                .token(&secret)
                .body(&held);
            request.send_bytes().await
        });
        wait(&s, pid, &request, pattern).await;
        s.rig.world.advance(chrono::Duration::hours(2));
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            state(&s, &supplied.name).await,
            before,
            "no accepted profile, ownership or audit survives the ended origin"
        );
    }
}

#[tokio::test]
async fn completed_machine_profile_readback_keeps_its_receipt_and_never_reapplies_input_writes() {
    for change in [
        "none",
        "input_write_closed",
        "result_read_closed",
        "expired",
    ] {
        let s = Scene::new().await;
        ada_owns_the_machine(&s, reported()).await;
        let supplied = body(&s, "Published native profile");
        let token = key(&s, s.ada.id(), rules(&s, true).await).await;
        let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.rig.world.app.clone();
        let machine = s.rig.machine_id;
        let held = supplied.clone();
        let secret = token.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(MachineAgentCreate, &[&machine])
                .token(&secret)
                .body(&held);
            request.send_bytes().await
        });
        wait(&s, pid, &request, "%INSERT INTO api_token_activity%").await;
        let written: (Id, String) =
            sqlx::query_as("SELECT id,xmin::text FROM agents WHERE machine_id=$1 AND name=$2")
                .bind(machine)
                .bind(&supplied.name)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        let saved = state(&s, &supplied.name).await;
        if change == "expired" {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        if change == "input_write_closed" || change == "result_read_closed" {
            let mut current = rules(&s, change != "result_read_closed").await;
            if change == "input_write_closed" {
                current.retain(|rule| {
                    rule["allow"][0]["actions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|action| action == "read")
                });
            }
            let scope: KeyScope = serde_json::from_value(json!({"rules":current})).unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(json!(scope))
                .execute(s.rig.db.pool())
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let value = accepted(tokio::time::timeout(WAIT, request).await.unwrap().unwrap());
        if change == "none" || change == "input_write_closed" {
            assert_eq!(value["id"], json!(written.0));
            assert_eq!(value["name"], supplied.name);
        } else {
            assert_eq!(value, json!({"projection":"receipt","agent_id":written.0}));
        }
        let actual: (Id, String) = sqlx::query_as("SELECT id,xmin::text FROM agents WHERE id=$1")
            .bind(written.0)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(actual, written);
        assert_eq!(state(&s, &supplied.name).await, saved);
    }
}
