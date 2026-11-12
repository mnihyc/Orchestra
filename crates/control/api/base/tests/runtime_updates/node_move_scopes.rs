//! Node move observations use actual native member/destination facts and keep pending results truthful.

use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::{identity, jobs::forks};
use std::time::Duration;

#[path = "node_move_scopes/host_delete_children.rs"]
mod host_delete_children;

const OP: &str = "NodeEnvironmentMovePreflight";
const WAIT: Duration = Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> serde_json::Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":actions}]})
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
            name: "Native node move observation",
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
fn request(environment: Id, target: Id) -> api::NodeEnvironmentMovePreflightBody {
    api::NodeEnvironmentMovePreflightBody {
        environment_ids: vec![environment, environment],
        host_id: Some(target),
        machine_id: None,
        template_id: None,
        copy_directories: None,
    }
}
async fn call(
    world: &World,
    node: Id,
    token: &str,
    body: &api::NodeEnvironmentMovePreflightBody,
) -> RawReply {
    report_api_failures();
    let call = world.app.call(api::NodeEnvironmentMovePreflight, &[&node]);
    call.token(token).body(body).send_bytes().await
}
fn success(reply: &RawReply) -> serde_json::Value {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn rules(team: &Team, target: Id, read: bool) -> Vec<serde_json::Value> {
    let mut rules = vec![
        rule(
            OP,
            "environment",
            team.environment,
            if read {
                &["manage", "read"]
            } else {
                &["manage"]
            },
        ),
        rule(
            OP,
            "host",
            target,
            if read { &["write", "read"] } else { &["write"] },
        ),
        rule(
            OP,
            "template",
            team.template,
            if read { &["write", "read"] } else { &["write"] },
        ),
    ];
    if read {
        rules.extend([
            rule(OP, "host", team.host, &["read"]),
            rule(OP, "org", team.c.acme, &["read"]),
        ]);
        // Physical machine facts use their Host's permission page, while the
        // disclosed business family remains Machine rather than Host.
        let mut machine = rule(OP, "host", team.host, &["read"]);
        machine["resources"] = json!(["machine"]);
        rules.push(machine);
    }
    rules
}

#[tokio::test]
async fn node_move_empty_observation_keeps_both_declared_actions_without_probing_or_inventing_destinations()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let measurement_before = measurement_sources(&world).await;
    let required = vec![
        rule(OP, "environment", team.environment, &["manage"]),
        rule(OP, "host", team.host, &["write"]),
    ];
    for missing in 0..required.len() {
        let mut incomplete = required.clone();
        incomplete.remove(missing);
        let actor = key(&world, team.c.admin.id(), incomplete).await;
        let body = api::NodeEnvironmentMovePreflightBody {
            environment_ids: Vec::new(),
            host_id: None,
            machine_id: None,
            template_id: None,
            copy_directories: None,
        };
        let refused = call(&world, team.c.acme, &actor.secret, &body).await;
        assert_eq!(
            refused.status.as_u16(),
            403,
            "{}",
            String::from_utf8_lossy(&refused.raw)
        );
    }
    let reviews: i64 =
        sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='move_batch'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        reviews, 0,
        "honest emptiness does not waive either canonical action"
    );

    let actor = key(&world, team.c.admin.id(), required).await;
    for host in [None, Some(team.host)] {
        let body = api::NodeEnvironmentMovePreflightBody {
            environment_ids: Vec::new(),
            host_id: host,
            machine_id: None,
            template_id: None,
            copy_directories: None,
        };
        let value = success(&call(&world, team.c.acme, &actor.secret, &body).await);
        assert_eq!(value["executable"], 0);
        let id: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
        let row = forks::preflight(&mut *world.conn().await, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.kind, "move_batch");
        assert_eq!(row.node_id, Some(team.c.acme));
        assert_eq!(row.created_by, team.c.admin.id());
        assert_eq!(row.plan["recipes"], json!([]));
        assert_eq!(row.plan["portable_recipes"], json!([]));
    }
    assert_eq!(
        measurement_sources(&world).await,
        measurement_before,
        "an unused host choice must not start a native probe"
    );
    assert!(move_jobs(&world).await.is_empty());
}

#[tokio::test]
async fn node_move_observation_manage_use_without_read_keeps_a_true_pending_receipt_and_one_native_review()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let actor = key(&world, team.c.admin.id(), rules(&team, target, false)).await;
    let value = success(
        &call(
            &world,
            team.c.acme,
            &actor.secret,
            &request(team.environment, target),
        )
        .await,
    );
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["executable"], 0);
    assert!(value.get("plans").is_none());
    let id: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
    let row = forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "move_batch");
    assert_eq!(row.node_id, Some(team.c.acme));
    assert_eq!(
        row.plan["recipes"],
        json!([]),
        "pending native observations are not executable recipes"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='environment_move'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    answers.abort();
}

#[tokio::test]
async fn node_move_observation_keeps_native_full_results_and_does_not_borrow_another_operations_read()
 {
    super::creation_replays::trace_native_failures();
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let native: api::EnvironmentMoveBatchPlan = serde_json::from_value(success(
        &call(
            &world,
            team.c.acme,
            &team.c.admin.token,
            &request(team.environment, target),
        )
        .await,
    ))
    .unwrap();
    assert_eq!(native.plans.len(), 1);
    assert!(
        native.plans[0]
            .blockers
            .iter()
            .any(|row| row.code == "measurement_pending")
    );
    let mut without_machine = rules(&team, target, true);
    without_machine.retain(|rule| rule["resources"] != json!(["machine"]));
    let partial = key(&world, team.c.admin.id(), without_machine).await;
    let receipt = success(
        &call(
            &world,
            team.c.acme,
            &partial.secret,
            &request(team.environment, target),
        )
        .await,
    );
    assert_eq!(
        receipt["projection"], "receipt",
        "Host Read alone is not Read of the separately declared Machine business family"
    );
    assert!(receipt.get("plans").is_none());
    let actor = key(&world, team.c.admin.id(), rules(&team, target, true)).await;
    let full: api::EnvironmentMoveBatchPlan = serde_json::from_value(success(
        &call(
            &world,
            team.c.acme,
            &actor.secret,
            &request(team.environment, target),
        )
        .await,
    ))
    .unwrap();
    assert_eq!(full.plans.len(), native.plans.len());
    assert_eq!(full.executable, native.executable);
    assert_eq!(full.plans[0].environment, native.plans[0].environment);
    assert_eq!(full.plans[0].from, native.plans[0].from);
    assert!(full.plans[0].from_machine.is_none());
    assert_eq!(full.plans[0].to, native.plans[0].to);
    assert!(
        full.plans[0]
            .files
            .file_states
            .as_ref()
            .unwrap()
            .measurement
            .as_ref()
            .unwrap()
            .measured_at
            .is_none()
    );
    let mut without_source_read = rules(&team, target, true);
    assert_ne!(team.host, target);
    without_source_read.retain(|rule| {
        rule["target"]["id"] != json!(team.host) || rule["resources"] != json!(["host"])
    });
    let actor = key(&world, team.c.admin.id(), without_source_read).await;
    assert_eq!(
        success(
            &call(
                &world,
                team.c.acme,
                &actor.secret,
                &request(team.environment, target),
            )
            .await
        )["projection"],
        "receipt",
        "the published source Host still needs this operation's Read"
    );
    let mut scoped = rules(&team, target, false);
    scoped.push(rule(
        "EnvironmentMovePreflight",
        "environment",
        team.environment,
        &["read"],
    ));
    let actor = key(&world, team.c.admin.id(), scoped).await;
    assert_eq!(
        success(
            &call(
                &world,
                team.c.acme,
                &actor.secret,
                &request(team.environment, target)
            )
            .await
        )["projection"],
        "receipt"
    );
    answers.abort();
}

#[tokio::test]
async fn node_move_observation_refuses_missing_selected_destination_and_wrong_current_node_without_execution()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(OP, "environment", team.environment, &["manage", "read"]),
            rule(OP, "template", team.template, &["write"]),
        ],
    )
    .await;
    assert_eq!(
        call(
            &world,
            team.c.acme,
            &actor.secret,
            &request(team.environment, target)
        )
        .await
        .status
        .as_u16(),
        403
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='move_batch'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let actor = key(&world, team.c.admin.id(), rules(&team, target, true)).await;
    let foreign = places::make_org(
        &world,
        &team.c.admin,
        "Unrelated move node",
        "unrelated-move",
    )
    .await
    .base
    .id;
    assert_eq!(
        call(
            &world,
            foreign,
            &actor.secret,
            &request(team.environment, target)
        )
        .await
        .status
        .as_u16(),
        404
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='environment_move'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
    answers.abort();
}

#[tokio::test]
async fn node_move_observation_read_loss_at_real_postcommit_wait_keeps_the_stored_review() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let actor = key(&world, team.c.admin.id(), rules(&team, target, true)).await;
    let mut blocker = world.app.app.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let secret = actor.secret;
    let node = team.c.acme;
    let body = request(team.environment, target);
    let pending = tokio::spawn(async move {
        let call = app.call(api::NodeEnvironmentMovePreflight, &[&node]);
        call.token(&secret).body(&body).send_bytes().await
    });
    tokio::time::timeout(WAIT,async {loop {
        assert!(!pending.is_finished(),"the native observation must reach its actual postcommit wait");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
            .bind(pid).fetch_one(&world.db.pool).await.unwrap();
        if blocked{break;}tokio::task::yield_now().await;
    }}).await.unwrap();
    let (id, xmin): (Id, String) =
        sqlx::query_as("SELECT id,xmin::text FROM preflights WHERE kind='move_batch'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":rules(&team,target,false)}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let value = success(&tokio::time::timeout(WAIT, pending).await.unwrap().unwrap());
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["preflight_id"], json!(id));
    let after: String = sqlx::query_scalar("SELECT xmin::text FROM preflights WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        after, xmin,
        "disclosure does not rerun the planner or rewrite its native review"
    );
    answers.abort();
}

const CREATE: &str = "NodeEnvironmentMoveCreate";
fn create_rules(team: &Team, target: Id, read: bool) -> Vec<serde_json::Value> {
    rules(team, target, read)
        .into_iter()
        .map(|mut rule| {
            for grant in rule["allow"].as_array_mut().unwrap() {
                grant["operation"] = json!(CREATE);
            }
            rule
        })
        .collect()
}
async fn ready_review(world: &World, team: &Team, target: Id) -> ForkCreateBody {
    let wanted = request(team.environment, target);
    let plan: api::EnvironmentMoveBatchPlan = serde_json::from_value(success(
        &call(world, team.c.acme, &team.c.admin.token, &wanted).await,
    ))
    .unwrap();
    finish_move_count(world, team, &plan.plans[0].files).await;
    let plan: api::EnvironmentMoveBatchPlan = serde_json::from_value(success(
        &call(world, team.c.acme, &team.c.admin.token, &wanted).await,
    ))
    .unwrap();
    assert_eq!(plan.executable, 1, "{:?}", plan.plans);
    ForkCreateBody {
        preflight_id: plan.preflight_id,
        notify: Some(false),
        options: None,
    }
}
async fn confirm(world: &World, node: Id, token: &str, body: &ForkCreateBody) -> RawReply {
    let call = world.app.call(api::NodeEnvironmentMoveCreate, &[&node]);
    call.token(token).body(body).send_bytes().await
}
async fn measurement_sources(world: &World) -> serde_json::Value {
    sqlx::query_scalar("SELECT COALESCE(jsonb_agg(jsonb_build_object('id',id,'by',created_by,'finished_at',finished_at,'keep_until',plan->'keep_until','source',plan->'measurement_source','answer',plan->'measurement_observation') ORDER BY id),'[]'::jsonb) FROM jobs WHERE kind='measurement'")
        .fetch_one(&world.db.pool).await.unwrap()
}
async fn move_jobs(world: &World) -> Vec<Id> {
    sqlx::query_scalar("SELECT id FROM jobs WHERE kind='environment_move' ORDER BY id")
        .fetch_all(&world.db.pool)
        .await
        .unwrap()
}
#[tokio::test]
async fn node_move_confirmation_requires_the_actual_destination_and_keeps_its_native_generated_receipt()
 {
    super::creation_replays::trace_native_failures();
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let body = ready_review(&world, &team, target).await;
    let mut missing = create_rules(&team, target, false);
    missing.retain(|rule| rule["target"]["kind"] != "template");
    let actor = key(&world, team.c.admin.id(), missing).await;
    assert_eq!(
        confirm(&world, team.c.acme, &actor.secret, &body)
            .await
            .status
            .as_u16(),
        403
    );
    assert!(move_jobs(&world).await.is_empty());
    let actor = key(
        &world,
        team.c.admin.id(),
        create_rules(&team, target, false),
    )
    .await;
    let measured_before = measurement_sources(&world).await;
    let reply = confirm(&world, team.c.acme, &actor.secret, &body).await;
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(
        measurement_sources(&world).await,
        measured_before,
        "confirmation reuses the original native observation without adopting, reissuing or retiming it"
    );
    let value: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["node_id"], json!(team.c.acme));
    assert!(value.get("jobs").is_none());
    let ids: Vec<Id> = serde_json::from_value(value["job_ids"].clone()).unwrap();
    assert_eq!(ids, move_jobs(&world).await);
    assert_eq!(ids.len(), 1);
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, ids[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.environment_id, Some(team.environment));
    let source = orch_engine::jobs::environment_move::authority::SOURCE;
    assert_eq!(row.plan[source]["origin"]["credential"]["kind"], "token");
    assert_eq!(
        row.plan[source]["origin"]["credential"]["id"],
        json!(actor.id)
    );
    let principal = orch_engine::jobs::environment_move::authority::principal(
        &mut *world.conn().await,
        &row,
        world.app.app.clock.as_ref(),
    )
    .await
    .expect("the native worker retains the actual key rather than inventing a session");
    assert_eq!(principal.user(), team.c.admin.id());
    let lifecycle = orch_engine::jobs::lifecycle_commands::environment::present(&row);
    assert!(
        lifecycle,
        "the accepted node move enters the native lifecycle channel"
    );
    let access = orch_api_core::thread_access::AuthzAccess::new(world.app.app.clock.clone());
    let checkpoint = orch_engine::jobs::lifecycle_commands::environment::principal(
        &mut *world.conn().await,
        &row,
        world.app.app.clock.as_ref(),
        &access,
        None,
    )
    .await
    .expect("the lifecycle worker keeps the node confirmation's actual source");
    assert_eq!(checkpoint.principal.user(), team.c.admin.id());
    assert_eq!(
        checkpoint
            .principal
            .caller()
            .grant()
            .and_then(|grant| grant.token()),
        Some(actor.id),
        "the native lifecycle keeps the actual HTTP key"
    );
    assert_eq!(
        checkpoint.principal.caller().admitted_operation(),
        Some(CREATE)
    );
    let mut malformed_lifecycle = row.clone();
    malformed_lifecycle.plan["environment_lifecycle"] = serde_json::Value::Null;
    assert!(
        orch_engine::jobs::lifecycle_commands::environment::principal(
            &mut *world.conn().await,
            &malformed_lifecycle,
            world.app.app.clock.as_ref(),
            &access,
            None,
        )
        .await
        .is_err(),
        "a known malformed node lifecycle source cannot fall back to an account"
    );
    let mut malformed = row.clone();
    malformed.plan[source] = serde_json::Value::Null;
    assert!(
        orch_engine::jobs::environment_move::authority::principal(
            &mut *world.conn().await,
            &malformed,
            world.app.app.clock.as_ref()
        )
        .await
        .is_err()
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":[rule(CREATE,"environment",team.environment,&["manage"])]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        orch_engine::jobs::environment_move::authority::principal(
            &mut *world.conn().await,
            &row,
            world.app.app.clock.as_ref()
        )
        .await
        .is_err(),
        "withdrawn destination use never becomes a browser principal"
    );
    assert!(
        orch_engine::jobs::lifecycle_commands::environment::principal(
            &mut *world.conn().await,
            &row,
            world.app.app.clock.as_ref(),
            &access,
            None,
        )
        .await
        .is_err(),
        "the same current native destination floor governs the lifecycle worker"
    );
    assert!(
        forks::preflight(&mut *world.conn().await, body.preflight_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_ne!(
        confirm(&world, team.c.acme, &team.c.admin.token, &body)
            .await
            .status
            .as_u16(),
        202
    );
    assert_eq!(move_jobs(&world).await, ids);
    answers.abort();
}
#[tokio::test]
async fn node_move_confirmation_keeps_native_full_jobs_and_current_node_and_review_ownership() {
    super::creation_replays::trace_native_failures();
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let body = ready_review(&world, &team, target).await;
    let actor = key(&world, team.c.admin.id(), create_rules(&team, target, true)).await;
    let foreign = places::make_org(
        &world,
        &team.c.admin,
        "Unrelated confirmation node",
        "unrelated-confirmation",
    )
    .await
    .base
    .id;
    let wrong = confirm(&world, foreign, &actor.secret, &body).await;
    assert_eq!(wrong.status.as_u16(), 404);
    assert!(move_jobs(&world).await.is_empty());
    let reply = confirm(&world, team.c.acme, &actor.secret, &body).await;
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let full: api::EnvironmentBatch = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(full.jobs.len(), 1);
    assert_eq!(full.jobs[0].environment.id, team.environment);
    assert_eq!(full.jobs[0].job.kind, "environment_move");
    assert_eq!(full.jobs[0].job.batch_id, Some(full.batch_id));
    answers.abort();
}
#[tokio::test]
async fn node_move_confirmation_expiry_after_its_real_environment_wait_accepts_no_job() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let body = ready_review(&world, &team, target).await;
    let kept = body.preflight_id;
    let actor = key(
        &world,
        team.c.admin.id(),
        create_rules(&team, target, false),
    )
    .await;
    let mut blocker = world.app.app.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
        .bind(team.environment)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let node = team.c.acme;
    let secret = actor.secret;
    let pending = tokio::spawn(async move {
        let call = app.call(api::NodeEnvironmentMoveCreate, &[&node]);
        call.token(&secret).body(&body).send_bytes().await
    });
    tokio::time::timeout(WAIT,async{loop{assert!(!pending.is_finished());let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&world.db.pool).await.unwrap();if waiting{break;}tokio::task::yield_now().await;}}).await.unwrap();
    world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
    assert!(
        matches!(reply.status.as_u16(), 401 | 403 | 404),
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(move_jobs(&world).await.is_empty());
    assert!(
        forks::preflight(&mut *world.conn().await, kept)
            .await
            .unwrap()
            .is_some()
    );
    answers.abort();
}

#[tokio::test]
async fn node_move_confirmation_postcommit_read_loss_keeps_the_accepted_batch_and_jobs() {
    super::creation_replays::trace_native_failures();
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let body = ready_review(&world, &team, target).await;
    let kept = body.preflight_id;
    let actor = key(&world, team.c.admin.id(), create_rules(&team, target, true)).await;
    let mut blocker = world.app.app.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let node = team.c.acme;
    let secret = actor.secret;
    let pending = tokio::spawn(async move {
        let call = app.call(api::NodeEnvironmentMoveCreate, &[&node]);
        call.token(&secret).body(&body).send_bytes().await
    });
    tokio::time::timeout(WAIT,async{loop{assert!(!pending.is_finished());let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();if waiting{break;}tokio::task::yield_now().await;}}).await.unwrap();
    let ids = move_jobs(&world).await;
    assert_eq!(ids.len(), 1);
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, ids[0])
        .await
        .unwrap()
        .unwrap();
    let batch = row.batch_id.unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":create_rules(&team,target,false)}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["batch_id"], json!(batch));
    assert_eq!(value["job_ids"], json!(ids));
    assert!(value.get("jobs").is_none());
    assert_eq!(move_jobs(&world).await, ids);
    assert!(
        forks::preflight(&mut *world.conn().await, kept)
            .await
            .unwrap()
            .is_none()
    );
    answers.abort();
}

#[tokio::test]
async fn node_move_confirmation_never_turns_a_missing_native_witness_into_another_count_or_accepted_move()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let body = ready_review(&world, &team, target).await;
    let review = forks::preflight(&mut *world.conn().await, body.preflight_id)
        .await
        .unwrap()
        .unwrap();
    let reference: orch_api_contract::forks::Measurement = serde_json::from_value(
        review.plan["observations"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .clone(),
    )
    .unwrap();
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{measurement_observation}','null'::jsonb) WHERE id=$1")
        .bind(reference.job_id).execute(&world.db.pool).await.unwrap();
    let before = measurement_sources(&world).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        create_rules(&team, target, false),
    )
    .await;
    let reply = confirm(&world, team.c.acme, &actor.secret, &body).await;
    assert_eq!(
        reply.status.as_u16(),
        403,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(
        measurement_sources(&world).await,
        before,
        "no guessed successful witness, new count or comparison retime"
    );
    assert!(move_jobs(&world).await.is_empty());
    answers.abort();
}
