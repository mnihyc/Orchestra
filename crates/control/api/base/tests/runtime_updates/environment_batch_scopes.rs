//! Actual cross-place review sources, once-only observations and truthful write-only receipts.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, jobs};
use orch_engine::hosting::{HostServices, HostingError, ToolBuild};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

const OP: &str = "EnvironmentUpdateBatchPreflight";
struct Key {
    id: Id,
    secret: String,
}
fn rule(kind: &str, id: Id, resources: &[&str], actions: &[&str]) -> serde_json::Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":resources,"allow":[{"operation":OP,"actions":actions}]})
}
fn rules(team: &Team, read: bool) -> Vec<serde_json::Value> {
    let actions = if read {
        vec!["manage", "read"]
    } else {
        vec!["manage"]
    };
    let mut rules = vec![rule(
        "environment",
        team.environment,
        &["environment"],
        &actions,
    )];
    if read {
        rules.push(rule("template", team.template, &["template"], &["read"]));
        rules.push(rule("org", team.c.acme, &["org"], &["read"]));
    }
    rules
}
async fn key(world: &World, user: Id, rules: Vec<serde_json::Value>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Environment update review control",
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
fn confirmation_rules(team: &Team, read: bool) -> Vec<serde_json::Value> {
    let mut rules = rules(team, read);
    for rule in &mut rules {
        rule["allow"][0]["operation"] = json!("EnvironmentUpdateBatchCreate");
    }
    rules
}
async fn confirm(world: &World, token: &str, id: Id) -> (u16, serde_json::Value) {
    let reply = world
        .app
        .call(api::EnvironmentUpdateBatchCreate, &[])
        .token(token)
        .query(&orch_api_contract::forks::UpdateConfirmQuery {
            notify: Some(false),
        })
        .body(&ForkCreateBody {
            preflight_id: id,
            options: None,
            notify: None,
        })
        .send_bytes()
        .await;
    (
        reply.status.as_u16(),
        serde_json::from_slice(&reply.raw).unwrap(),
    )
}
async fn preview(world: &World, token: &str, ids: Vec<Id>) -> (u16, serde_json::Value) {
    let reply = world
        .app
        .call(api::EnvironmentUpdateBatchPreflight, &[])
        .token(token)
        .body(&api::NodeEnvironmentUpdatePreflightBody {
            environment_ids: ids,
        })
        .send_bytes()
        .await;
    (
        reply.status.as_u16(),
        serde_json::from_slice(&reply.raw).unwrap(),
    )
}
async fn count(world: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='update_batch'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
struct ObservedBuilds {
    calls: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
    hold: bool,
}
impl ObservedBuilds {
    fn new(hold: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            hold,
        }
    }
}
#[async_trait]
impl HostServices for ObservedBuilds {
    async fn enrollment(
        &self,
        _: &mut orch_db::Conn,
        _: &orch_engine::hosting::EnrollmentAsk,
    ) -> Result<orch_engine::hosting::Enrollment, HostingError> {
        Err(HostingError::Failed("review never enrolls a unit".into()))
    }
    async fn tools(
        &self,
        _: &mut orch_db::Conn,
        _: &str,
        _: &str,
        _: &[String],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        if self.hold {
            self.release.acquire().await.unwrap().forget();
        }
        Ok(Vec::new())
    }
}
#[tokio::test]
async fn environment_update_review_keeps_the_native_manage_act_and_optional_same_operation_read() {
    let world = World::new().await;
    let team = team(&world).await;
    let builds = Arc::new(ObservedBuilds::new(false));
    world.app.app.engine.attach_host_services(builds.clone());
    let writer = key(&world, team.c.admin.id(), rules(&team, false)).await;
    let (status, receipt) = preview(
        &world,
        &writer.secret,
        vec![team.environment, team.environment],
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt.as_object().unwrap().len(), 2);
    let id: Id = serde_json::from_value(receipt["preflight_id"].clone()).unwrap();
    let kept = jobs::forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.plan["recipes"].as_array().unwrap().len(), 1);
    assert_eq!(
        kept.plan["update_review_sources"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        builds.calls.load(Ordering::SeqCst),
        1,
        "the repeated source and response checks cannot consult the catalogue again"
    );
    let before = count(&world).await;
    let mut wrong_operation = rules(&team, true);
    for rule in &mut wrong_operation {
        rule["allow"][0]["operation"] = json!("NodeEnvironmentUpdatePreflight");
    }
    let denied = key(&world, team.c.admin.id(), wrong_operation).await;
    let (status, problem) = preview(&world, &denied.secret, vec![team.environment]).await;
    assert_eq!(status, 403, "{problem}");
    assert_eq!(count(&world).await, before);
    let read_only = key(
        &world,
        team.c.admin.id(),
        vec![rule(
            "environment",
            team.environment,
            &["environment"],
            &["read"],
        )],
    )
    .await;
    let before = count(&world).await;
    let (status, problem) = preview(&world, &read_only.secret, vec![team.environment]).await;
    assert_eq!(status, 403, "{problem}");
    assert_eq!(
        count(&world).await,
        before,
        "Read cannot admit an empty Manage act"
    );
    let other = orch_db::queries::resources::access_fixtures::insert_environment(
        &mut *world.conn().await,
        team.c.admin.id(),
        Some(team.c.acme),
        "Another managed unit",
        "managed",
    )
    .await
    .unwrap();
    let mut skipped_rules = rules(&team, true);
    skipped_rules[0]["allow"][0]["actions"] = json!(["read"]);
    skipped_rules.push(rule("environment", other, &["environment"], &["manage"]));
    let other_manager = key(&world, team.c.admin.id(), skipped_rules).await;
    let (status, skipped) = preview(&world, &other_manager.secret, vec![team.environment]).await;
    assert_eq!(status, 200, "{skipped}");
    assert_eq!(skipped["executable"], 0);
    assert!(skipped["plans"].as_array().unwrap().is_empty());
    assert_eq!(skipped["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(
        skipped["skipped"][0]["environment"]["id"],
        json!(team.environment)
    );
    assert_eq!(skipped["skipped"][0]["reason"], "no_permission");
    assert_eq!(
        builds.calls.load(Ordering::SeqCst),
        1,
        "a member without this act's Manage remains skipped before native IO"
    );
    let reader = key(&world, team.c.admin.id(), rules(&team, true)).await;
    let (status, full) = preview(&world, &reader.secret, vec![team.environment]).await;
    assert_eq!(status, 200, "{full}");
    assert!(full.get("projection").is_none());
    assert_eq!(full["executable"], 1);
    assert_eq!(
        full["plans"][0]["environment"]["id"],
        json!(team.environment)
    );
}
#[tokio::test]
async fn environment_update_review_rechecks_the_original_key_after_the_real_catalogue_wait() {
    let world = World::new().await;
    let team = team(&world).await;
    let builds = Arc::new(ObservedBuilds::new(true));
    world.app.app.engine.attach_host_services(builds.clone());
    let original = key(&world, team.c.admin.id(), rules(&team, true)).await;
    let app = world.app.clone();
    let token = original.secret.clone();
    let environment = team.environment;
    let request = tokio::spawn(async move {
        app.call(api::EnvironmentUpdateBatchPreflight, &[])
            .token(&token)
            .body(&api::NodeEnvironmentUpdatePreflightBody {
                environment_ids: vec![environment],
            })
            .send_bytes()
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), builds.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(original.id)
        .bind(json!({"rules":[rule("environment",team.environment,&["environment"],&["read"])]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    builds.release.add_permits(1);
    let reply = request.await.unwrap();
    assert_eq!(
        reply.status.as_u16(),
        403,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(count(&world).await, 0);
    assert_eq!(builds.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn environment_update_review_refuses_a_new_runtime_after_observing_the_old_one() {
    let world = World::new().await;
    let team = team(&world).await;
    let builds = Arc::new(ObservedBuilds::new(true));
    world.app.app.engine.attach_host_services(builds.clone());
    let original = key(&world, team.c.admin.id(), rules(&team, true)).await;
    let app = world.app.clone();
    let token = original.secret;
    let environment = team.environment;
    let request = tokio::spawn(async move {
        app.call(api::EnvironmentUpdateBatchPreflight, &[])
            .token(&token)
            .body(&api::NodeEnvironmentUpdatePreflightBody {
                environment_ids: vec![environment],
            })
            .send_bytes()
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), builds.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
        .bind(team.environment)
        .execute(&world.db.pool)
        .await
        .unwrap();
    builds.release.add_permits(1);
    let reply = request.await.unwrap();
    assert_eq!(
        reply.status.as_u16(),
        403,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(count(&world).await, 0);
    assert_eq!(builds.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn environment_update_review_preserves_the_native_empty_batch_without_a_fake_subject() {
    let world = World::new().await;
    let team = team(&world).await;
    let (status, full) = preview(&world, &team.c.admin.token, Vec::new()).await;
    assert_eq!(status, 200, "{full}");
    assert_eq!(full["executable"], 0);
    assert_eq!(full["plans"], json!([]));
    assert_eq!(full["skipped"], json!([]));
    let id: Id = serde_json::from_value(full["preflight_id"].clone()).unwrap();
    let kept = jobs::forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.plan["recipes"], json!([]));
}

#[tokio::test]
async fn environment_update_confirmation_keeps_two_independent_operations_and_queued_current_sources()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let builds = Arc::new(ObservedBuilds::new(false));
    world.app.app.engine.attach_host_services(builds.clone());
    let original_rules = rules(&team, false);
    let reviewer = key(&world, team.c.admin.id(), original_rules.clone()).await;
    let writer = key(&world, team.c.admin.id(), confirmation_rules(&team, false)).await;
    let (status, review) = preview(&world, &reviewer.secret, vec![team.environment]).await;
    assert_eq!(status, 200, "{review}");
    let id: Id = serde_json::from_value(review["preflight_id"].clone()).unwrap();
    let (status, outcome) = confirm(&world, &writer.secret, id).await;
    assert_eq!(status, 202, "{outcome}");
    assert_eq!(outcome["projection"], "receipt");
    let job: Id = serde_json::from_value(outcome["jobs"][0]["job_id"].clone()).unwrap();
    assert_eq!(
        outcome["jobs"][0]["environment_id"],
        json!(team.environment)
    );
    assert_eq!(
        builds.calls.load(Ordering::SeqCst),
        2,
        "one actual catalogue answer per review and confirmation, never per checkpoint"
    );
    let row = jobs::job(&mut *world.conn().await, job)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.environment_id, Some(team.environment));
    assert_eq!(row.state, "queued");
    let bound = &row.plan[orch_engine::jobs::recreate::environment_authority::BINDING];
    assert_eq!(bound["review"]["origin"]["operation"], OP);
    assert_eq!(
        bound["confirmation"]["origin"]["operation"],
        "EnvironmentUpdateBatchCreate"
    );
    let mut tx = world.app.app.db.begin().await.unwrap();
    assert!(
        orch_engine::jobs::recreate::environment_authority::current(
            &mut tx,
            &row,
            world.app.app.clock.as_ref()
        )
        .await
        .unwrap()
        .is_some()
    );
    tx.rollback().await.unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reviewer.id)
        .bind(json!(KeyScope::default()))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.app.app.db.begin().await.unwrap();
    assert!(
        orch_engine::jobs::recreate::environment_authority::current(
            &mut tx,
            &row,
            world.app.app.clock.as_ref()
        )
        .await
        .unwrap()
        .is_none(),
        "a broader confirmation never restores the withdrawn review act"
    );
    tx.rollback().await.unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reviewer.id)
        .bind(json!({"rules":original_rules}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.app.app.db.begin().await.unwrap();
    assert!(
        orch_engine::jobs::recreate::environment_authority::current(
            &mut tx,
            &row,
            world.app.app.clock.as_ref()
        )
        .await
        .unwrap()
        .is_some()
    );
    let mut changed = row.clone();
    changed.plan["old"] = json!(new_id());
    assert!(
        orch_engine::jobs::recreate::environment_authority::current(
            &mut tx,
            &changed,
            world.app.app.clock.as_ref()
        )
        .await
        .unwrap()
        .is_none(),
        "the receipt cannot redirect a queued update to another native runtime"
    );
    tx.rollback().await.unwrap();
    assert!(
        jobs::forks::preflight(&mut *world.conn().await, id)
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn environment_update_confirmation_refuses_an_ended_review_before_new_observation_or_queueing()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let builds = Arc::new(ObservedBuilds::new(false));
    world.app.app.engine.attach_host_services(builds.clone());
    let reviewer = key(&world, team.c.admin.id(), rules(&team, true)).await;
    let writer = key(&world, team.c.admin.id(), confirmation_rules(&team, true)).await;
    let (status, review) = preview(&world, &reviewer.secret, vec![team.environment]).await;
    assert_eq!(status, 200, "{review}");
    let id: Id = serde_json::from_value(review["preflight_id"].clone()).unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reviewer.id)
        .bind(json!(KeyScope::default()))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let (status, problem) = confirm(&world, &writer.secret, id).await;
    assert_eq!(status, 403, "{problem}");
    assert_eq!(builds.calls.load(Ordering::SeqCst), 1);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='environment_recreate' AND environment_id=$1",
    )
    .bind(team.environment)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(queued, 0);
    assert!(
        jobs::forks::preflight(&mut *world.conn().await, id)
            .await
            .unwrap()
            .is_some()
    );
}
