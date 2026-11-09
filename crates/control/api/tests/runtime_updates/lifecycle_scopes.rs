//! Actual global lifecycle preparation keeps its own operation, selected sources and receipts.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::loss::{EnvironmentRecoverPreflight, EnvironmentRecoverPreflightBody};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::{identity, jobs::forks};
use std::time::Duration;

const MOVE: &str = "EnvironmentMovePreflight";
const RECOVER: &str = "EnvironmentRecoverPreflight";
struct Key {
    id: Id,
    secret: String,
}
fn rule(op: &str, kind: &str, id: Id, actions: &[&str]) -> serde_json::Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":op,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<serde_json::Value>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native global lifecycle",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn required(team: &Team, target: Id, op: &str) -> Vec<serde_json::Value> {
    vec![
        rule(op, "environment", team.environment, &["manage"]),
        rule(op, "host", target, &["write"]),
        rule(op, "template", team.template, &["write"]),
    ]
}
async fn readable(
    world: &World,
    mut rules: Vec<serde_json::Value>,
    op: &str,
) -> Vec<serde_json::Value> {
    let nodes: Vec<(String, Id)> =
        sqlx::query_as("SELECT kind,id FROM nodes WHERE deleted_at IS NULL")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    rules.extend(
        nodes
            .into_iter()
            .map(|(kind, id)| rule(op, &kind, id, &["read"])),
    );
    for (table, kind) in [
        ("environments", "environment"),
        ("hosts", "host"),
        ("machines", "machine"),
        ("env_templates", "template"),
    ] {
        // Four fixed tables: literal statements, so the query lint sees no formatted SQL.
        let sql = match table {
            "environments" => "SELECT id FROM environments",
            "hosts" => "SELECT id FROM hosts",
            "machines" => "SELECT id FROM machines",
            "env_templates" => "SELECT id FROM env_templates",
            other => unreachable!("an unlisted table {other}"),
        };
        let ids: Vec<Id> = sqlx::query_scalar(sql)
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
        rules.extend(ids.into_iter().map(|id| rule(op, kind, id, &["read"])));
    }
    rules
}
fn request(target: Id) -> api::EnvironmentMovePreflightBody {
    api::EnvironmentMovePreflightBody {
        host_id: Some(target),
        machine_id: None,
        template_id: None,
        copy_directories: None,
        home: None,
        private_views: None,
        files: None,
        check_fingerprints: None,
        fingerprint_request_id: None,
        mounts: None,
    }
}
async fn call_move(world: &World, environment: Id, token: &str, target: Id) -> RawReply {
    world
        .app
        .call(api::EnvironmentMovePreflight, &[&environment])
        .token(token)
        .body(&request(target))
        .send_bytes()
        .await
}
async fn call_recovery(world: &World, environment: Id, token: &str, target: Id) -> RawReply {
    world
        .app
        .call(EnvironmentRecoverPreflight, &[])
        .token(token)
        .body(&EnvironmentRecoverPreflightBody {
            environment_ids: vec![environment, environment],
            host_id: Some(target),
            choices: Vec::new(),
            environment: None,
        })
        .send_bytes()
        .await
}
fn success(reply: RawReply) -> serde_json::Value {
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn count_rows(world: &World) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM preflights WHERE kind IN ('environment_move','recover_batch')),(SELECT count(*) FROM jobs WHERE kind='measurement')")
        .fetch_one(&world.db.pool).await.unwrap()
}

#[tokio::test]
async fn a_global_move_manage_and_selected_use_save_only_the_native_review_receipt() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let actor = key(&world, team.c.admin.id(), required(&team, target, MOVE)).await;
    let value = success(call_move(&world, team.environment, &actor.secret, target).await);
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["projection", "environment_id", "preflight_id", "expires_at"]
            .map(str::to_owned)
            .into_iter()
            .collect()
    );
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["environment_id"], json!(team.environment));
    let id: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
    let row = forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "environment_move");
    assert_eq!(row.environment_id, Some(team.environment));
    assert_eq!(row.created_by, team.c.admin.id());
    let files = &row.plan["recipe"]["files"];
    assert_eq!(files["file_states"]["complete"], false);
    let id: Id =
        serde_json::from_value(files["file_states"]["measurement"]["job_id"].clone()).unwrap();
    let count = orch_db::queries::jobs::job(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count.kind, "measurement");
    assert_eq!(count.state, "queued");
    assert_eq!(count.plan["measurement_source_version"], 1);
    let seed: orch_engine::commands::CommandAuthority =
        serde_json::from_value(count.plan["measurement_source"]["seeds"]["measure"].clone())
            .unwrap();
    assert!(
        matches!(seed, orch_engine::commands::CommandAuthority::Lifecycle(_)),
        "the count uses the one lifecycle family"
    );
    let made: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='environment_move'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(made, 0);
}

#[tokio::test]
async fn a_global_move_refuses_missing_selected_scope_and_never_borrows_a_neighbour_operation() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    for missing in 0..3 {
        let mut rules = required(&team, target, MOVE);
        rules.remove(missing);
        let actor = key(&world, team.c.admin.id(), rules).await;
        let before = count_rows(&world).await;
        let reply = call_move(&world, team.environment, &actor.secret, target).await;
        assert_eq!(reply.status, 403, "{}", String::from_utf8_lossy(&reply.raw));
        assert_eq!(
            count_rows(&world).await,
            before,
            "refused preparation saves no review or count"
        );
    }
    let actor = key(
        &world,
        team.c.admin.id(),
        required(&team, target, "EnvironmentMoveCreate"),
    )
    .await;
    assert_eq!(
        call_move(&world, team.environment, &actor.secret, target)
            .await
            .status,
        403
    );
    assert_eq!(count_rows(&world).await, (0, 0));
}

#[tokio::test]
async fn a_global_move_keeps_the_ordinary_plan_and_requires_its_own_content_read() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let ordinary = success(call_move(&world, team.environment, &team.c.admin.token, target).await);
    let full = key(
        &world,
        team.c.admin.id(),
        readable(&world, required(&team, target, MOVE), MOVE).await,
    )
    .await;
    let scoped = success(call_move(&world, team.environment, &full.secret, target).await);
    let ordinary: orch_api_contract::forks::EnvironmentMovePlan =
        serde_json::from_value(ordinary).unwrap();
    let scoped: orch_api_contract::forks::EnvironmentMovePlan =
        serde_json::from_value(scoped).unwrap();
    assert_eq!(scoped.environment, ordinary.environment);
    assert_eq!(scoped.from, ordinary.from);
    assert_eq!(scoped.to, ordinary.to);
    assert_eq!(scoped.kind, ordinary.kind);
    assert_eq!(scoped.source_has_unit, ordinary.source_has_unit);
    assert!(
        scoped
            .blockers
            .iter()
            .any(|b| b.code == "measurement_pending")
    );
    let other = key(
        &world,
        team.c.admin.id(),
        readable(&world, required(&team, target, MOVE), "EnvironmentGet").await,
    )
    .await;
    assert_eq!(
        success(call_move(&world, team.environment, &other.secret, target).await)["projection"],
        "receipt"
    );
}

#[tokio::test]
async fn terminal_measurement_metadata_alone_does_not_turn_a_scoped_native_count_into_a_fact() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        readable(&world, required(&team, target, MOVE), MOVE).await,
    )
    .await;
    let first: orch_api_contract::forks::EnvironmentMovePlan = serde_json::from_value(success(
        call_move(&world, team.environment, &actor.secret, target).await,
    ))
    .unwrap();
    let original = first
        .files
        .file_states
        .as_ref()
        .unwrap()
        .measurement
        .as_ref()
        .unwrap()
        .job_id;
    // Terminal metadata supplies no native command observation. Keep this negative
    // premise separate from finish_move_count, which drives the real count worker.
    let mut conn = world.conn().await;
    let row = orch_db::queries::jobs::job(&mut conn, original)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "queued");
    assert!(row.plan.get("measurement_observation").is_none());
    assert!(
        orch_db::queries::jobs::finish(
            &mut conn,
            original,
            "succeeded",
            &row.progress,
            Some(&file_state_count()),
            None,
            world.now(),
        )
        .await
        .unwrap()
    );
    drop(conn);
    let second: orch_api_contract::forks::EnvironmentMovePlan = serde_json::from_value(success(
        call_move(&world, team.environment, &actor.secret, target).await,
    ))
    .unwrap();
    let next = second.files.file_states.as_ref().unwrap();
    assert!(!next.complete);
    assert_ne!(next.measurement.as_ref().unwrap().job_id, original);
    assert!(next.measurement.as_ref().unwrap().measured_at.is_none());
    assert!(
        second
            .blockers
            .iter()
            .any(|b| b.code == "measurement_pending")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_global_move_key_expiring_at_the_real_native_source_lock_claims_nothing() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let actor = key(&world, team.c.admin.id(), required(&team, target, MOVE)).await;
    let mut held = world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
        .bind(team.environment)
        .execute(&mut *held)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let running = call_move(&world, team.environment, &actor.secret, target);
    let release = async {
        tokio::time::timeout(Duration::from_secs(30),async {
            loop {let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();if waiting{break;}tokio::time::sleep(Duration::from_millis(10)).await;}
        }).await.expect("the real native source lock was reached");
        world.advance(chrono::Duration::hours(2));
        held.commit().await.unwrap();
    };
    let (reply, ()) = tokio::join!(running, release);
    assert!(!reply.status.is_success());
    assert_eq!(count_rows(&world).await, (0, 0));
    let remains: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='node.runtime.volumes.measure'",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(remains, 0);
    let still: bool = sqlx::query_scalar("SELECT expires_at <= $2 FROM api_tokens WHERE id=$1")
        .bind(actor.id)
        .bind(world.now())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(still);
}

async fn mark_lost(world: &World, team: &Team) {
    sqlx::query("UPDATE runtimes SET lost_at=$2,state='stopped' WHERE id=$1")
        .bind(team.runtime)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET status='lost' WHERE id=$1")
        .bind(team.environment)
        .execute(&world.db.pool)
        .await
        .unwrap();
}
async fn answer_inspect(world: &World, target: Id) -> tokio::task::JoinHandle<()> {
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(target)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let pool = world.db.pool.clone();
    tokio::spawn(async move {
        loop {
            if sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE machine_id=$1 AND kind='node.volume.inspect' AND state IN ('queued','sent','accepted')")
            .bind(machine).bind(json!({"exists":false,"bytes":null})).execute(&pool).await.is_err(){return;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
}
#[tokio::test]
async fn recovery_manage_and_actual_destination_use_save_a_receipt_after_the_native_volume_probes()
{
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    mark_lost(&world, &team).await;
    let answers = answer_inspect(&world, target).await;
    let actor = key(&world, team.c.admin.id(), required(&team, target, RECOVER)).await;
    let reply = call_recovery(&world, team.environment, &actor.secret, target).await;
    answers.abort();
    let value = success(reply);
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["executable"], 1);
    assert!(value.get("rows").is_none());
    assert!(value.get("admin_grants").is_none());
    let review: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
    let saved = forks::preflight(&mut *world.conn().await, review)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.kind, "recover_batch");
    assert_eq!(saved.plan["affected"], json!([team.environment]));
    assert_eq!(saved.plan["members"].as_array().unwrap().len(), 1);
    let probes:i64=sqlx::query_scalar("SELECT count(*) FROM commands WHERE machine_id=(SELECT machine_id FROM hosts WHERE id=$1) AND kind='node.volume.inspect'").bind(target).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(probes, 2);
    let jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1 AND state='queued'")
            .bind(team.environment)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(jobs, 0);
}
#[tokio::test]
async fn recovery_refuses_missing_scope_before_probing_and_does_not_borrow_move_authority() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    mark_lost(&world, &team).await;
    for missing in 0..3 {
        let mut rules = required(&team, target, RECOVER);
        rules.remove(missing);
        let actor = key(&world, team.c.admin.id(), rules).await;
        let reply = call_recovery(&world, team.environment, &actor.secret, target).await;
        assert_eq!(reply.status, 403, "{}", String::from_utf8_lossy(&reply.raw));
    }
    let other = key(&world, team.c.admin.id(), required(&team, target, MOVE)).await;
    assert_eq!(
        call_recovery(&world, team.environment, &other.secret, target)
            .await
            .status,
        403
    );
    let probes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.volume.inspect'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(probes, 0);
    assert_eq!(count_rows(&world).await, (0, 0));
}
#[tokio::test]
async fn recovery_full_plan_is_independent_of_the_saved_receipt_and_another_operations_read() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    mark_lost(&world, &team).await;
    let answers = answer_inspect(&world, target).await;
    let ordinary = call_recovery(&world, team.environment, &team.c.admin.token, target).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        readable(&world, required(&team, target, RECOVER), RECOVER).await,
    )
    .await;
    let full = call_recovery(&world, team.environment, &actor.secret, target).await;
    let other = key(
        &world,
        team.c.admin.id(),
        readable(
            &world,
            required(&team, target, RECOVER),
            "EnvironmentRecoverGet",
        )
        .await,
    )
    .await;
    let receipt = call_recovery(&world, team.environment, &other.secret, target).await;
    answers.abort();
    let ordinary: orch_api_contract::loss::EnvironmentRecoveryPlan =
        serde_json::from_value(success(ordinary)).unwrap();
    let full: orch_api_contract::loss::EnvironmentRecoveryPlan =
        serde_json::from_value(success(full)).unwrap();
    assert_eq!(full.rows, ordinary.rows);
    assert_eq!(full.executable, ordinary.executable);
    assert_eq!(full.others, ordinary.others);
    assert_eq!(full.admin_grants, ordinary.admin_grants);
    assert_eq!(success(receipt)["projection"], "receipt");
}

async fn confirm_move(world: &World, environment: Id, token: &str, preflight: Id) -> RawReply {
    let call = world.app.call(api::EnvironmentMoveCreate, &[&environment]);
    call.token(token)
        .body(&ForkCreateBody {
            preflight_id: preflight,
            options: None,
            notify: None,
        })
        .send_bytes()
        .await
}

#[tokio::test]
async fn confirmation_reuses_the_actual_pending_review_count_under_only_its_new_source() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let preview = key(&world, team.c.admin.id(), required(&team, target, MOVE)).await;
    let saved = success(call_move(&world, team.environment, &preview.secret, target).await);
    let preflight: Id = serde_json::from_value(saved["preflight_id"].clone()).unwrap();
    let original = forks::preflight(&mut *world.conn().await, preflight)
        .await
        .unwrap()
        .unwrap();
    let count: Id = serde_json::from_value(
        original.plan["recipe"]["files"]["file_states"]["measurement"]["job_id"].clone(),
    )
    .unwrap();
    let before: (String, serde_json::Value, Option<Id>) =
        sqlx::query_as("SELECT xmin::text,plan,created_by FROM jobs WHERE id=$1")
            .bind(count)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let totals = count_rows(&world).await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(preview.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let confirm = key(
        &world,
        team.c.admin.id(),
        required(&team, target, "EnvironmentMoveCreate"),
    )
    .await;
    let result = confirm_move(&world, team.environment, &confirm.secret, preflight).await;
    assert_eq!(
        result.problem().code,
        ErrorCode::PlanBlocked,
        "a genuine pending count remains pending; its old credential is neither restored nor replaced"
    );
    assert_eq!(
        count_rows(&world).await,
        totals,
        "confirmation must not claim another count"
    );
    let after: (String, serde_json::Value, Option<Id>) =
        sqlx::query_as("SELECT xmin::text,plan,created_by FROM jobs WHERE id=$1")
            .bind(count)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        after, before,
        "the exact pending job, creator and native claim remain untouched"
    );
    let current = forks::preflight(&mut *world.conn().await, preflight)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.plan, original.plan,
        "pending confirmation consumes no review or hold"
    );
    let moves: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='environment_move'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(moves, 0);
}

#[tokio::test]
async fn move_confirmation_refuses_other_operations_and_missing_roles_before_any_new_native_claim()
{
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let saved = success(call_move(&world, team.environment, &team.c.admin.token, target).await);
    let preflight: Id = serde_json::from_value(saved["preflight_id"].clone()).unwrap();
    let before = count_rows(&world).await;
    let mut policies = vec![required(&team, target, MOVE)];
    for missing in 0..3 {
        let mut policy = required(&team, target, "EnvironmentMoveCreate");
        policy.remove(missing);
        policies.push(policy);
    }
    for policy in policies {
        let actor = key(&world, team.c.admin.id(), policy).await;
        let response = confirm_move(&world, team.environment, &actor.secret, preflight).await;
        assert_eq!(response.problem().code, ErrorCode::AuthzDenied);
        assert_eq!(count_rows(&world).await, before);
    }
}

#[tokio::test]
async fn an_accepted_native_move_keeps_the_original_http_source_after_its_review_is_consumed() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let first = world
        .app
        .call(api::EnvironmentMovePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&request(target))
        .send()
        .await
        .full_move()
        .ok();
    // Keep the existing human fixture's completed-count behavior. This is a native queue/source
    // control; the separate pending control verifies the scoped observation port without forged facts.
    finish_move_count(&world, &team, &first.files).await;
    let plan = world
        .app
        .call(api::EnvironmentMovePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&request(target))
        .send()
        .await
        .full_move()
        .ok();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let made = confirm_move(
        &world,
        team.environment,
        &team.c.admin.token,
        plan.preflight_id,
    )
    .await;
    answers.abort();
    assert_eq!(made.status, 202, "{}", String::from_utf8_lossy(&made.raw));
    let value: serde_json::Value = serde_json::from_slice(&made.raw).unwrap();
    let id: Id = serde_json::from_value(value["id"].clone()).unwrap();
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "environment_move");
    assert_eq!(row.plan["environment_lifecycle_version"], 1);
    assert_eq!(row.plan["environment_lifecycle_required"], true);
    assert!(
        forks::preflight(&mut *world.conn().await, plan.preflight_id)
            .await
            .unwrap()
            .is_none()
    );
    let clock = world.app.app.clock.clone();
    let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
    let mut tx = world.db.pool.begin().await.unwrap();
    let current = orch_engine::jobs::lifecycle_commands::environment::current(
        &mut tx,
        &row,
        clock.as_ref(),
        &access,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        current.principal.caller().admitted_operation(),
        Some("EnvironmentMoveCreate")
    );
    current.check(clock.as_ref()).unwrap();
    tx.commit().await.unwrap();
    // Ordinary authority would still permit this person; the exact ended raw session must not.
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE user_id=$1")
        .bind(team.c.admin.id())
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    let refused = orch_engine::jobs::lifecycle_commands::environment::current(
        &mut tx,
        &row,
        clock.as_ref(),
        &access,
        None,
    )
    .await;
    assert!(matches!(
        refused,
        Err(orch_engine::EngineError::BeforeEffect(_))
    ));
    tx.rollback().await.unwrap();
    let sent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind IN ('node.runtime.stop','node.runtime.start')",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        sent, 0,
        "this source proof authorizes no remote effect after the session ends"
    );
}

#[tokio::test]
async fn native_recovery_prepares_its_complete_queue_before_the_first_configuration_or_job_effect()
{
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    mark_lost(&world, &team).await;
    let answers = answer_inspect(&world, target).await;
    let saved = success(call_recovery(&world, team.environment, &team.c.admin.token, target).await);
    answers.abort();
    let id: Id = serde_json::from_value(saved["preflight_id"].clone()).unwrap();
    let review = forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    let recipe: orch_engine::recovery::Recipe =
        serde_json::from_value(review.plan["members"][0]["recipe"].clone()).unwrap();
    let before: (String, serde_json::Value) =
        sqlx::query_as("SELECT xmin::text,config FROM environments WHERE id=$1")
            .bind(team.environment)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1")
        .bind(team.environment)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    let prepared = orch_engine::recovery::prepare_queue(&mut tx, &recipe)
        .await
        .unwrap();
    assert_eq!(prepared.recipe(), &recipe);
    assert_eq!(prepared.config_change().unwrap().0, team.environment);
    let after_prepare: (String, serde_json::Value) =
        sqlx::query_as("SELECT xmin::text,config FROM environments WHERE id=$1")
            .bind(team.environment)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        after_prepare, before,
        "native preparation only locks the actual source"
    );
    let checks = std::sync::atomic::AtomicUsize::new(0);
    let refused = orch_engine::recovery::queue_prepared(
        &world.app.app.engine,
        &mut tx,
        &prepared,
        None,
        world.now(),
        || {
            checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(orch_engine::updates::UpdateError::Denied)
        },
    )
    .await;
    assert!(matches!(
        refused,
        Err(orch_engine::updates::UpdateError::Denied)
    ));
    assert_eq!(checks.load(std::sync::atomic::Ordering::SeqCst), 1);
    let after_refusal: (String, serde_json::Value) =
        sqlx::query_as("SELECT xmin::text,config FROM environments WHERE id=$1")
            .bind(team.environment)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(after_refusal, before);
    let after_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1")
        .bind(team.environment)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(after_jobs, jobs);
    assert!(
        orch_db::queries::jobs::changes::environment_hold(&mut tx, team.environment)
            .await
            .unwrap()
            .is_none()
    );
    let queued = orch_engine::recovery::queue_prepared(
        &world.app.app.engine,
        &mut tx,
        &prepared,
        None,
        world.now(),
        || Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(queued.environment_id, Some(team.environment));
    assert_eq!(queued.created_by, Some(team.c.admin.id()));
    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM environments WHERE id=$1")
            .bind(team.environment)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let mut expected = before.1;
    expected["env.data_sources"] =
        json!({"workspace":{"kind":"made_by_orch"},"home":{"kind":"made_by_orch"}});
    assert_eq!(
        config, expected,
        "the same native patch preserves every other setting"
    );
    assert_eq!(
        orch_db::queries::jobs::changes::environment_hold(&mut tx, team.environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        queued.id
    );
    tx.rollback().await.unwrap();
}

async fn confirm_recovery(world: &World, token: &str, preflight: Id) -> RawReply {
    let call = world
        .app
        .call(orch_api_contract::loss::EnvironmentRecoverCreate, &[]);
    call.token(token)
        .body(&ForkCreateBody {
            preflight_id: preflight,
            options: None,
            notify: None,
        })
        .send_bytes()
        .await
}

#[tokio::test]
async fn recovery_confirmation_uses_its_new_credential_and_returns_native_jobs_or_an_exact_receipt()
{
    for content in [false, true] {
        let world = World::new().await;
        let team = team(&world).await;
        let target = movable(&world, &team).await;
        mark_lost(&world, &team).await;
        let answers = answer_inspect(&world, target).await;
        let preview = key(&world, team.c.admin.id(), required(&team, target, RECOVER)).await;
        let plan = success(call_recovery(&world, team.environment, &preview.secret, target).await);
        let preflight: Id = serde_json::from_value(plan["preflight_id"].clone()).unwrap();
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(preview.id)
            .bind(world.now())
            .execute(&world.db.pool)
            .await
            .unwrap();
        let mut policy = required(&team, target, "EnvironmentRecoverCreate");
        if content {
            policy = readable(&world, policy, "EnvironmentRecoverCreate").await;
        }
        let creator = key(&world, team.c.admin.id(), policy).await;
        let response = confirm_recovery(&world, &creator.secret, preflight).await;
        answers.abort();
        assert_eq!(
            response.status,
            202,
            "{}",
            String::from_utf8_lossy(&response.raw)
        );
        let value: serde_json::Value = serde_json::from_slice(&response.raw).unwrap();
        let batch: Id = serde_json::from_value(value["batch_id"].clone()).unwrap();
        let rows = orch_db::queries::jobs::forks::of_batch(&mut *world.conn().await, batch)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(
            row.kind,
            orch_db::queries::jobs::kind::ENVIRONMENT_PROVISION
        );
        assert_eq!(row.environment_id, Some(team.environment));
        assert_eq!(row.created_by, Some(team.c.admin.id()));
        assert_eq!(row.plan["environment_lifecycle_version"], 1);
        if content {
            assert!(value.get("projection").is_none());
            assert_eq!(
                value["jobs"][0]["environment"]["id"],
                json!(team.environment)
            );
            assert_eq!(value["jobs"][0]["job"]["id"], json!(row.id));
        } else {
            assert_eq!(
                value,
                json!({"projection":"receipt","batch_id":batch,"job_ids":[row.id]})
            );
        }
        assert!(
            forks::preflight(&mut *world.conn().await, preflight)
                .await
                .unwrap()
                .is_none()
        );
        let clock = world.app.app.clock.clone();
        let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
        let mut tx = world.db.pool.begin().await.unwrap();
        let current = orch_engine::jobs::lifecycle_commands::environment::current(
            &mut tx,
            row,
            clock.as_ref(),
            &access,
            Some(target),
        )
        .await
        .unwrap();
        assert_eq!(
            current.principal.caller().admitted_operation(),
            Some("EnvironmentRecoverCreate")
        );
        current.check(clock.as_ref()).unwrap();
        tx.commit().await.unwrap();
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(creator.id)
            .bind(world.now())
            .execute(&world.db.pool)
            .await
            .unwrap();
        let mut tx = world.db.pool.begin().await.unwrap();
        let denied = orch_engine::jobs::lifecycle_commands::environment::current(
            &mut tx,
            row,
            clock.as_ref(),
            &access,
            Some(target),
        )
        .await;
        assert!(matches!(
            denied,
            Err(orch_engine::EngineError::BeforeEffect(_))
        ));
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
async fn recovery_confirmation_refuses_missing_or_neighbour_authority_before_repeating_volume_inspection()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    mark_lost(&world, &team).await;
    let answers = answer_inspect(&world, target).await;
    let plan = success(call_recovery(&world, team.environment, &team.c.admin.token, target).await);
    let preflight: Id = serde_json::from_value(plan["preflight_id"].clone()).unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.volume.inspect'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let mut policies = vec![required(&team, target, RECOVER)];
    for missing in 0..3 {
        let mut policy = required(&team, target, "EnvironmentRecoverCreate");
        policy.remove(missing);
        policies.push(policy);
    }
    for policy in policies {
        let creator = key(&world, team.c.admin.id(), policy).await;
        let response = confirm_recovery(&world, &creator.secret, preflight).await;
        assert_eq!(response.problem().code, ErrorCode::AuthzDenied);
        let after: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.volume.inspect'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(after, before);
        assert!(
            forks::preflight(&mut *world.conn().await, preflight)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            orch_db::queries::jobs::changes::environment_hold(
                &mut *world.conn().await,
                team.environment
            )
            .await
            .unwrap()
            .is_none()
        );
    }
    answers.abort();
}
