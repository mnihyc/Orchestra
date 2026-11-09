//! Real node update reviews: scoped selection, native observations, current storage and readback.

use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{identity, jobs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

const OP: &str = "NodeEnvironmentUpdatePreflight";
const WAIT: Duration = Duration::from_secs(30);
pub(super) struct Key {
    pub(super) id: Id,
    pub(super) secret: String,
}
pub(super) fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> serde_json::Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":actions}]})
}
pub(super) async fn key(world: &World, user: Id, rules: Vec<serde_json::Value>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native update review control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}
pub(super) async fn preview(world: &World, node: Id, token: &str, ids: &[Id]) -> RawReply {
    let call = world
        .app
        .call(api::NodeEnvironmentUpdatePreflight, &[&node]);
    call.token(token)
        .body(&api::NodeEnvironmentUpdatePreflightBody {
            environment_ids: ids.to_vec(),
        })
        .send_bytes()
        .await
}
pub(super) fn success(reply: &RawReply) -> serde_json::Value {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
pub(super) async fn stored(world: &World, id: Id) -> jobs::forks::PreflightRow {
    jobs::forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
}
pub(super) async fn review_count(world: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='update_batch'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

pub(super) async fn outside_home(world: &World, team: &Team) -> Id {
    places::make_org(world, &team.c.stranger, "Other selector", "other-selector")
        .await
        .base
        .id
}
pub(super) async fn record_definition(world: &World, team: &Team, definition: serde_json::Value) {
    sqlx::query("INSERT INTO jobs (id,kind,environment_id,state,plan) VALUES ($1,'environment_provision',$2,'succeeded',$3)")
        .bind(new_id()).bind(team.environment)
        .bind(json!({"runtime":team.runtime,"created_spec":{"spec":definition,"resources":{},"tools":[]}}))
        .execute(&world.db.pool).await.unwrap();
}

#[tokio::test]
async fn node_review_manage_without_read_stores_one_native_recipe_and_cannot_borrow_another_reader()
{
    let world = World::new().await;
    let team = team(&world).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(OP, "environment", team.environment, &["manage"]),
            rule("EnvironmentGet", "environment", team.environment, &["read"]),
        ],
    )
    .await;
    let reply = preview(
        &world,
        team.c.acme,
        &actor.secret,
        &[team.environment, team.environment],
    )
    .await;
    let value = success(&reply);
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["executable"], 1);
    assert_eq!(value["skipped"], 0);
    assert!(value.get("plans").is_none());
    assert!(value.get("environment").is_none());
    let id: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
    let row = stored(&world, id).await;
    assert_eq!(row.node_id, Some(team.c.acme));
    assert_eq!(row.created_by, team.c.admin.id());
    let recipes: Vec<orch_engine::updates::Recipe> =
        serde_json::from_value(row.plan["recipes"].clone()).unwrap();
    assert_eq!(recipes.len(), 1);
    assert_eq!(
        recipes[0].target,
        orch_engine::updates::Target::Environment(team.environment)
    );
    assert_eq!(recipes[0].old, team.runtime);
    let jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='environment_recreate'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        jobs, 0,
        "preflight stores a review, not a replacement or a hold"
    );
}

#[tokio::test]
async fn node_review_full_reader_uses_the_native_plan_and_bounds_optional_template_and_address_names()
 {
    let world = World::new().await;
    let team = team(&world).await;
    record_definition(
        &world,
        &team,
        json!({"image":"busybox:older","resources":{},"colocated_harnesses":[]}),
    )
    .await;
    let ordinary: api::EnvironmentUpdateBatchPlan = serde_json::from_value(success(
        &preview(
            &world,
            team.c.acme,
            &team.c.admin.token,
            &[team.environment],
        )
        .await,
    ))
    .unwrap();
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![rule(
            OP,
            "environment",
            team.environment,
            &["manage", "read"],
        )],
    )
    .await;
    let value = success(&preview(&world, team.c.acme, &actor.secret, &[team.environment]).await);
    assert!(value.get("projection").is_none());
    let scoped: api::EnvironmentUpdateBatchPlan = serde_json::from_value(value).unwrap();
    assert_eq!(scoped.executable, ordinary.executable);
    assert_eq!(scoped.plans.len(), 1);
    assert_eq!(scoped.plans[0].environment.id, team.environment);
    assert_eq!(scoped.plans[0].kept, ordinary.plans[0].kept);
    assert_eq!(scoped.plans[0].lost, ordinary.plans[0].lost);
    assert_eq!(scoped.plans[0].options, ordinary.plans[0].options);
    let template_update = scoped.plans[0]
        .updates
        .iter()
        .find(|update| update.what == api::UpdateWhat::Template)
        .expect("a recorded older image gives the native template-name premise");
    assert_eq!(template_update.name, "Template");
    let reader = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(OP, "environment", team.environment, &["manage", "read"]),
            rule(OP, "template", team.template, &["read"]),
            rule(OP, "org", team.c.acme, &["read"]),
        ],
    )
    .await;
    let full: api::EnvironmentUpdateBatchPlan = serde_json::from_value(success(
        &preview(&world, team.c.acme, &reader.secret, &[team.environment]).await,
    ))
    .unwrap();
    assert_eq!(full.plans[0].environment, ordinary.plans[0].environment);
    assert_eq!(full.plans[0].updates, ordinary.plans[0].updates);
}

#[tokio::test]
async fn node_review_preserves_skips_empty_batches_and_current_node_membership() {
    let world = World::new().await;
    let team = team(&world).await;
    let other = orch_db::queries::resources::access_fixtures::insert_environment(
        &mut *world.conn().await,
        team.c.admin.id(),
        Some(team.c.acme),
        "Other update member",
        "managed",
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut *world.conn().await,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: other,
        },
        team.c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(OP, "environment", team.environment, &["manage", "read"]),
            rule(OP, "environment", other, &["read"]),
        ],
    )
    .await;
    let full: api::EnvironmentUpdateBatchPlan = serde_json::from_value(success(
        &preview(
            &world,
            team.c.acme,
            &actor.secret,
            &[team.environment, other],
        )
        .await,
    ))
    .unwrap();
    assert_eq!(full.plans.len(), 1);
    assert_eq!(full.skipped.len(), 1);
    assert_eq!(full.skipped[0].environment.id, other);
    assert_eq!(full.skipped[0].reason, api::UpdateSkipReason::NoPermission);
    let empty: api::EnvironmentUpdateBatchPlan = serde_json::from_value(success(
        &preview(&world, team.c.acme, &actor.secret, &[]).await,
    ))
    .unwrap();
    assert!(empty.plans.is_empty() && empty.skipped.is_empty());
    assert_eq!(empty.executable, 0);
    assert_eq!(
        stored(&world, empty.preflight_id).await.plan["recipes"],
        json!([])
    );
    let outside = outside_home(&world, &team).await;
    let foreign = orch_db::queries::resources::access_fixtures::insert_environment(
        &mut *world.conn().await,
        team.c.admin.id(),
        Some(outside),
        "Outside update selector",
        "managed",
    )
    .await
    .unwrap();
    let before = review_count(&world).await;
    let refused = preview(&world, team.c.acme, &actor.secret, &[foreign]).await;
    assert_eq!(refused.status.as_u16(), 404);
    assert_eq!(review_count(&world).await, before);
}

pub(super) struct HeldBuilds {
    pub(super) entered: Notify,
    pub(super) release: Notify,
    pub(super) calls: AtomicUsize,
}
impl HeldBuilds {
    pub(super) fn new() -> Self {
        Self {
            entered: Notify::new(),
            release: Notify::new(),
            calls: AtomicUsize::new(0),
        }
    }
}
#[async_trait]
impl orch_engine::hosting::HostServices for HeldBuilds {
    async fn enrollment(
        &self,
        _: &mut orch_db::Conn,
        _: &orch_engine::hosting::EnrollmentAsk,
    ) -> Result<orch_engine::hosting::Enrollment, orch_engine::hosting::HostingError> {
        Err(orch_engine::hosting::HostingError::Failed(
            "this test only observes the update planner".into(),
        ))
    }
    async fn tools(
        &self,
        _: &mut orch_db::Conn,
        _: &str,
        _: &str,
        _: &[String],
    ) -> Result<Vec<orch_engine::hosting::ToolBuild>, orch_engine::hosting::HostingError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn node_review_rechecks_the_real_credential_and_source_after_its_once_only_catalogue_wait() {
    for close in ["expiry", "revocation", "membership", "runtime"] {
        let world = World::new().await;
        let team = team(&world).await;
        let outside = outside_home(&world, &team).await;
        let actor = key(
            &world,
            team.c.admin.id(),
            vec![rule(
                OP,
                "environment",
                team.environment,
                &["manage", "read"],
            )],
        )
        .await;
        let services = Arc::new(HeldBuilds::new());
        world.app.app.engine.attach_host_services(services.clone());
        let app = world.app.clone();
        let node = team.c.acme;
        let environment = team.environment;
        let secret = actor.secret;
        let before = review_count(&world).await;
        let pending = tokio::spawn(async move {
            let call = app.call(api::NodeEnvironmentUpdatePreflight, &[&node]);
            call.token(&secret)
                .body(&api::NodeEnvironmentUpdatePreflightBody {
                    environment_ids: vec![environment],
                })
                .send_bytes()
                .await
        });
        tokio::time::timeout(WAIT, services.entered.notified())
            .await
            .unwrap();
        match close {
            "expiry" => world.advance(chrono::Duration::hours(2)),
            "revocation" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(world.now())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "membership" => {
                sqlx::query("UPDATE environments SET home_node_id=$2,org_id=$2 WHERE id=$1")
                    .bind(environment)
                    .bind(outside)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "runtime" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(environment)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        services.release.notify_one();
        let refused = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
        assert!(
            matches!(refused.status.as_u16(), 401 | 403),
            "{close}: {}",
            String::from_utf8_lossy(&refused.raw)
        );
        assert_eq!(
            review_count(&world).await,
            before,
            "{close}: a closed source must not store a review"
        );
        assert_eq!(services.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn node_review_losing_read_after_storage_keeps_the_exact_receipt_without_replanning_or_restoring_read()
 {
    for close in [false, true] {
        let world = World::new().await;
        let team = team(&world).await;
        let actor = key(
            &world,
            team.c.admin.id(),
            vec![rule(
                OP,
                "environment",
                team.environment,
                &["manage", "read"],
            )],
        )
        .await;
        let services = Arc::new(HeldBuilds::new());
        services.release.notify_one();
        world.app.app.engine.attach_host_services(services.clone());
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
        let environment = team.environment;
        let secret = actor.secret;
        let pending = tokio::spawn(async move {
            let call = app.call(api::NodeEnvironmentUpdatePreflight, &[&node]);
            call.token(&secret)
                .body(&api::NodeEnvironmentUpdatePreflightBody {
                    environment_ids: vec![environment],
                })
                .send_bytes()
                .await
        });
        tokio::time::timeout(WAIT, async {
            loop {
                assert!(!pending.is_finished(), "the native request must reach its actual postcommit activity wait");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let (id, xmin): (Id, String) =
            sqlx::query_as("SELECT id,xmin::text FROM preflights WHERE kind='update_batch'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        if close {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(actor.id)
                .bind(json!({"rules":[rule(OP, "environment", environment, &["manage"])]}))
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
        let value = success(&reply);
        assert_eq!(value["preflight_id"], json!(id));
        if close {
            assert_eq!(value["projection"], "receipt");
            assert_eq!(value["executable"], 1);
            assert!(value.get("plans").is_none());
        } else {
            assert_eq!(value["plans"][0]["environment"]["id"], json!(environment));
        }
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM preflights WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            after, xmin,
            "optional readback never re-saves the native review"
        );
        assert_eq!(
            services.calls.load(Ordering::SeqCst),
            1,
            "current-read rechecks never repeat catalogue IO"
        );
        assert_eq!(review_count(&world).await, 1);
    }
}

#[tokio::test]
async fn node_review_file_estimate_has_exact_native_evidence_and_is_not_repeated_by_preparation() {
    for close in [false, true] {
        let world = World::new().await;
        let team = team(&world).await;
        record_definition(
            &world,
            &team,
            json!({"image":"busybox:latest","resources":{},"colocated_harnesses":[]}),
        )
        .await;
        sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
            .bind(team.host)
            .bind(json!({"docker":{"recreate":"snapshot","recreate_preserves_files":true}}))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let actor = key(
            &world,
            team.c.admin.id(),
            vec![rule(
                OP,
                "environment",
                team.environment,
                &["manage", "read"],
            )],
        )
        .await;
        let app = world.app.clone();
        let node = team.c.acme;
        let environment = team.environment;
        let secret = actor.secret;
        let pending = tokio::spawn(async move {
            let call = app.call(api::NodeEnvironmentUpdatePreflight, &[&node]);
            call.token(&secret)
                .body(&api::NodeEnvironmentUpdatePreflightBody {
                    environment_ids: vec![environment],
                })
                .send_bytes()
                .await
        });
        let id: Id = tokio::time::timeout(WAIT, async {
            loop {
                assert!(!pending.is_finished(), "the native preserved-file plan must reach its real probe");
                let id: Option<Id> = sqlx::query_scalar("SELECT id FROM commands WHERE kind='node.runtime.keep-estimate' AND state IN ('queued','sent','accepted')")
                    .fetch_optional(&world.db.pool).await.unwrap();
                if let Some(id) = id { break id; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let mut tx = world.app.app.db.begin().await.unwrap();
        let command = orch_db::queries::link::command(&mut tx, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.body["runtime_id"], json!(team.runtime));
        let evidence = orch_db::queries::link::command_authority(&mut tx, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            evidence["authority"]["input"]["credential"]["kind"],
            "token"
        );
        let authority: orch_engine::commands::CommandAuthority =
            serde_json::from_value(evidence).unwrap();
        assert_eq!(authority.command_kind(), "node.runtime.keep-estimate");
        let access = orch_api_core::thread_access::AuthzAccess::new(world.app.app.clock.clone());
        assert!(
            authority
                .current_callers(&mut tx, &command, world.app.app.clock.as_ref(), &access)
                .await
                .unwrap()
                .is_some()
        );
        let mut changed = command.clone();
        changed.body["runtime_id"] = json!(new_id());
        assert!(
            authority
                .current_callers(&mut tx, &changed, world.app.app.clock.as_ref(), &access)
                .await
                .unwrap()
                .is_none()
        );
        tx.commit().await.unwrap();
        if close {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(actor.id)
                .bind(json!({"rules":[rule(OP, "environment", environment, &["read"])]}))
                .execute(&world.db.pool)
                .await
                .unwrap();
            let mut tx = world.app.app.db.begin().await.unwrap();
            assert!(
                authority
                    .current_callers(&mut tx, &command, world.app.app.clock.as_ref(), &access)
                    .await
                    .unwrap()
                    .is_none()
            );
            tx.commit().await.unwrap();
        }
        sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE id=$1")
            .bind(id)
            .bind(json!({"bytes":17,"seconds":1,"flatten":false}))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
        if close {
            assert_eq!(reply.status.as_u16(), 403);
            assert_eq!(review_count(&world).await, 0);
        } else {
            let full: api::EnvironmentUpdateBatchPlan =
                serde_json::from_value(success(&reply)).unwrap();
            assert!(
                full.plans[0]
                    .kept
                    .iter()
                    .any(|line| line.contains("1 MiB") && line.contains("1 seconds"))
            );
            let row = stored(&world, full.preflight_id).await;
            assert_eq!(
                row.plan["recipes"][0]["keep_estimate"]["bytes"], 17,
                "the exact native observation is retained in the executable recipe"
            );
            assert_eq!(review_count(&world).await, 1);
        }
        let probes: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commands WHERE kind='node.runtime.keep-estimate'",
        )
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            probes, 1,
            "action and publication preparation never issue another native observation"
        );
    }
}

#[tokio::test]
async fn node_review_keeps_the_observed_key_minimum_when_read_is_later_widened_again() {
    let world = World::new().await;
    let team = team(&world).await;
    let rules = vec![rule(
        OP,
        "environment",
        team.environment,
        &["manage", "read"],
    )];
    let actor = key(&world, team.c.admin.id(), rules.clone()).await;
    let services = Arc::new(HeldBuilds::new());
    world.app.app.engine.attach_host_services(services.clone());
    let app = world.app.clone();
    let node = team.c.acme;
    let environment = team.environment;
    let secret = actor.secret;
    let pending = tokio::spawn(async move {
        let call = app.call(api::NodeEnvironmentUpdatePreflight, &[&node]);
        call.token(&secret)
            .body(&api::NodeEnvironmentUpdatePreflightBody {
                environment_ids: vec![environment],
            })
            .send_bytes()
            .await
    });
    tokio::time::timeout(WAIT, services.entered.notified())
        .await
        .unwrap();
    let mut blocker = world.app.app.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":[rule(OP, "environment", environment, &["manage"])]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    services.release.notify_one();
    tokio::time::timeout(WAIT, async {
        loop {
            assert!(!pending.is_finished(), "the native review must reach its real postcommit publication wait");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert_eq!(
        review_count(&world).await,
        1,
        "the native review committed before the widening"
    );
    // A publisher cannot restore Read omitted by the accepted native review and its observations.
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":rules}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let value = success(&tokio::time::timeout(WAIT, pending).await.unwrap().unwrap());
    assert_eq!(value["projection"], "receipt");
    assert!(value.get("plans").is_none());
    assert_eq!(value["executable"], 1);
    assert_eq!(review_count(&world).await, 1);
    assert_eq!(services.calls.load(Ordering::SeqCst), 1);
}
