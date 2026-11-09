//! Confirmed native batches retain actual member/credential sources and truthful optional readback.

use super::node_preflight_scopes::{HeldBuilds, key, preview, rule, stored, success};
use super::*;
use orch_api_core::testing::RawReply;
use std::sync::atomic::Ordering;
use std::time::Duration;

const OP: &str = "NodeEnvironmentUpdateCreate";
const WAIT: Duration = Duration::from_secs(30);
async fn plan(world: &World, team: &Team) -> Id {
    serde_json::from_value(success(&preview(world,team.c.acme,&team.c.admin.token,&[team.environment]).await)["preflight_id"].clone()).unwrap()
}
async fn confirm(world: &World, node: Id, token: &str, id: Id) -> RawReply {
    let call = world.app.call(api::NodeEnvironmentUpdateCreate, &[&node]);
    call.token(token)
        .query(&orch_api_contract::forks::UpdateConfirmQuery {
            notify: Some(false),
        })
        .body(&ForkCreateBody {
            preflight_id: id,
            options: None,
            notify: None,
        })
        .send_bytes()
        .await
}
fn accepted(reply: &RawReply) -> serde_json::Value {
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn jobs(world: &World) -> Vec<orch_db::queries::jobs::JobRow> {
    let ids: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE kind='environment_recreate' ORDER BY created_at,id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    let mut conn = world.conn().await;
    let mut rows = Vec::new();
    for id in ids {
        rows.push(
            orch_db::queries::jobs::job(&mut conn, id)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    rows
}

async fn current(world: &World, row: &orch_db::queries::jobs::JobRow) -> bool {
    let mut tx = world.app.app.db.begin().await.unwrap();
    let allowed = orch_engine::jobs::recreate::batch_authority::current_with_clock(
        &mut tx,
        row,
        world.app.app.clock.as_ref(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    allowed
}

#[tokio::test]
async fn node_confirmation_without_read_queues_once_retains_the_actual_operation_and_cannot_borrow_job_get()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let id = plan(&world, &team).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(OP, "environment", team.environment, &["manage"]),
            rule("JobGet", "environment", team.environment, &["read"]),
        ],
    )
    .await;
    let value = accepted(&confirm(&world, team.c.acme, &actor.secret, id).await);
    assert_eq!(value["projection"], "receipt");
    assert!(value.get("jobs").is_none());
    let rows = jobs(&world).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(value["batch_id"], json!(rows[0].batch_id.unwrap()));
    assert_eq!(value["job_ids"], json!([rows[0].id]));
    assert!(
        orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, id)
            .await
            .unwrap()
            .is_none()
    );
    let source: orch_engine::updates::operation::Input =
        serde_json::from_value(rows[0].plan["environment_update"]["source"].clone()).unwrap();
    assert_eq!(source.origin().operation(), OP);
    assert_eq!(source.origin().user(), team.c.admin.id());
    assert_eq!(source.environment(), team.environment);
    assert_eq!(source.runtime(), Some(team.runtime));
    assert_eq!(source.execution_template(), Some(team.template));
    assert_eq!(
        rows[0].plan["environment_update"]["source"]["credential"]["kind"],
        "token"
    );
    assert!(
        rows[0].plan["environment_update"]["source"]
            .get("review")
            .is_none()
    );
    assert!(current(&world, &rows[0]).await);
    let replay = confirm(&world, team.c.acme, &actor.secret, id).await;
    assert_eq!(replay.status.as_u16(), 404);
    assert_eq!(jobs(&world).await.len(), 1);
}

#[tokio::test]
async fn node_confirmation_keeps_full_native_content_and_empty_batches_without_fabricated_targets()
{
    let world = World::new().await;
    let team = team(&world).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![rule(
            OP,
            "environment",
            team.environment,
            &["manage", "write", "read"],
        )],
    )
    .await;
    let id = plan(&world, &team).await;
    let value = accepted(&confirm(&world, team.c.acme, &actor.secret, id).await);
    let full: api::EnvironmentBatch = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(full.jobs.len(), 1);
    assert_eq!(full.jobs[0].environment.id, team.environment);
    assert_eq!(full.jobs[0].job.environment_id, Some(team.environment));
    assert_eq!(full.jobs[0].job.batch_id, Some(full.batch_id));
    assert!(
        !value.to_string().contains("environment_update"),
        "private native source metadata is never the Job DTO"
    );
    let empty = success(&preview(&world, team.c.acme, &team.c.admin.token, &[]).await);
    let empty_id: Id = serde_json::from_value(empty["preflight_id"].clone()).unwrap();
    let full: api::EnvironmentBatch = serde_json::from_value(accepted(
        &confirm(&world, team.c.acme, &actor.secret, empty_id).await,
    ))
    .unwrap();
    assert!(full.jobs.is_empty());
    assert_eq!(jobs(&world).await.len(), 1);
    assert!(
        orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, empty_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn node_confirmation_scope_and_ordinary_authority_are_checked_for_every_actual_stored_member()
{
    let world = World::new().await;
    let team = team(&world).await;
    let id = plan(&world, &team).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![rule(OP, "environment", new_id(), &["manage", "read"])],
    )
    .await;
    let reply = confirm(&world, team.c.acme, &actor.secret, id).await;
    assert_eq!(reply.status.as_u16(), 403);
    assert!(jobs(&world).await.is_empty());
    assert_eq!(stored(&world, id).await.created_by, team.c.admin.id());
    let stranger = key(
        &world,
        team.c.stranger.id(),
        vec![rule(
            OP,
            "environment",
            team.environment,
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        confirm(&world, team.c.acme, &stranger.secret, id)
            .await
            .status
            .as_u16(),
        403
    );
    assert!(jobs(&world).await.is_empty());
    world.advance(chrono::Duration::minutes(16));
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
    assert_eq!(
        confirm(&world, team.c.acme, &actor.secret, id)
            .await
            .status
            .as_u16(),
        410
    );
    assert!(jobs(&world).await.is_empty());
}

#[tokio::test]
async fn node_confirmation_checks_new_template_use_and_freezes_it_without_inheriting_review_expiry()
{
    let world = World::new().await;
    let team = team(&world).await;
    let template = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut *world.conn().await,
        "template",
        Some(team.c.admin.id()),
        Some(team.c.acme),
        "Selected update template",
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut *world.conn().await,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Template,
            id: template,
        },
        team.c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE env_templates SET driver='docker',spec=$2 WHERE id=$1")
        .bind(template)
        .bind(json!({"image":"busybox:latest","resources":{},"colocated_harnesses":[]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE environments SET config=jsonb_set(config,'{env.template_id}',$2) WHERE id=$1",
    )
    .bind(team.environment)
    .bind(json!(template))
    .execute(&world.db.pool)
    .await
    .unwrap();
    let id = plan(&world, &team).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(OP, "environment", team.environment, &["manage", "read"]),
            rule("EnvironmentCreate", "template", template, &["write"]),
        ],
    )
    .await;
    let services = Arc::new(HeldBuilds::new());
    world.app.app.engine.attach_host_services(services.clone());
    assert_eq!(
        confirm(&world, team.c.acme, &actor.secret, id)
            .await
            .status
            .as_u16(),
        403
    );
    assert_eq!(
        services.calls.load(Ordering::SeqCst),
        0,
        "missing known destination scope refuses before catalogue IO"
    );
    assert!(jobs(&world).await.is_empty());
    services.release.notify_one();
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(OP, "environment", team.environment, &["manage", "read"]),
            rule(OP, "template", template, &["write"]),
        ],
    )
    .await;
    let full: api::EnvironmentBatch = serde_json::from_value(accepted(
        &confirm(&world, team.c.acme, &actor.secret, id).await,
    ))
    .unwrap();
    assert_eq!(full.jobs.len(), 1);
    let row = jobs(&world).await.pop().unwrap();
    assert!(current(&world, &row).await);
    // The reviewed target is an accepted immutable recipe; a later setting is another update.
    sqlx::query("UPDATE environments SET config=config-'env.template_id' WHERE id=$1")
        .bind(team.environment)
        .execute(&world.db.pool)
        .await
        .unwrap();
    world.advance(chrono::Duration::minutes(16));
    assert!(
        current(&world, &row).await,
        "review expiry and subsequent settings do not withdraw the accepted frozen recipe"
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":[rule(OP,"environment",team.environment,&["manage","read"])]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        !current(&world, &row).await,
        "deferred work still requires that original key's current frozen Template Use"
    );
}

#[tokio::test]
async fn node_confirmation_rechecks_after_catalogue_wait_and_never_consumes_a_closed_review() {
    for close in ["scope", "review_expiry", "review_changed"] {
        let world = World::new().await;
        let team = team(&world).await;
        let id = plan(&world, &team).await;
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
        let secret = actor.secret;
        let pending = tokio::spawn(async move {
            let call = app.call(api::NodeEnvironmentUpdateCreate, &[&node]);
            call.token(&secret)
                .query(&orch_api_contract::forks::UpdateConfirmQuery {
                    notify: Some(false),
                })
                .body(&ForkCreateBody {
                    preflight_id: id,
                    options: None,
                    notify: None,
                })
                .send_bytes()
                .await
        });
        tokio::time::timeout(WAIT, services.entered.notified())
            .await
            .unwrap();
        match close {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":[rule(OP,"environment",team.environment,&["read"])]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "review_expiry" => world.advance(chrono::Duration::minutes(16)),
            "review_changed" => {
                sqlx::query("UPDATE preflights SET plan=plan||$2 WHERE id=$1")
                    .bind(id)
                    .bind(json!({"changed_after_observation":true}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        services.release.notify_one();
        let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
        assert_eq!(
            reply.status.as_u16(),
            if close == "review_expiry" { 410 } else { 403 },
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(jobs(&world).await.is_empty());
        assert!(
            orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, id)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(services.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn node_confirmation_read_loss_after_commit_keeps_the_exact_jobs_and_generated_receipt() {
    for close in [false, true] {
        let world = World::new().await;
        let team = team(&world).await;
        let id = plan(&world, &team).await;
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
        let secret = actor.secret;
        let pending = tokio::spawn(async move {
            let call = app.call(api::NodeEnvironmentUpdateCreate, &[&node]);
            call.token(&secret)
                .query(&orch_api_contract::forks::UpdateConfirmQuery {
                    notify: Some(false),
                })
                .body(&ForkCreateBody {
                    preflight_id: id,
                    options: None,
                    notify: None,
                })
                .send_bytes()
                .await
        });
        tokio::time::timeout(WAIT,async {
            loop {
                assert!(!pending.is_finished(),"the native queue must reach its real postcommit publication wait");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked{break;}tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let rows = jobs(&world).await;
        assert_eq!(rows.len(), 1);
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM jobs WHERE id=$1")
            .bind(rows[0].id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        if close {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(actor.id)
                .bind(json!({"rules":[rule(OP,"environment",team.environment,&["manage"])]}))
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let value = accepted(&tokio::time::timeout(WAIT, pending).await.unwrap().unwrap());
        if close {
            assert_eq!(
                value,
                json!({"projection":"receipt","node_id":team.c.acme,"batch_id":rows[0].batch_id.unwrap(),"job_ids":[rows[0].id]})
            );
        } else {
            assert_eq!(value["jobs"][0]["job"]["id"], json!(rows[0].id));
        }
        assert_eq!(jobs(&world).await.len(), 1);
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM jobs WHERE id=$1")
            .bind(rows[0].id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            after, xmin,
            "response refresh never requeues or rewrites accepted jobs"
        );
        assert_eq!(services.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn node_confirmation_restores_the_real_retained_credential_without_a_time_only_or_legacy_fallback()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let id = plan(&world, &team).await;
    let actor = key(
        &world,
        team.c.admin.id(),
        vec![rule(OP, "environment", team.environment, &["manage"])],
    )
    .await;
    accepted(&confirm(&world, team.c.acme, &actor.secret, id).await);
    let row = jobs(&world).await.pop().unwrap();
    assert!(current(&world, &row).await);
    let mut tx = world.app.app.db.begin().await.unwrap();
    assert!(
        !orch_engine::jobs::recreate::batch_authority::current(
            &mut tx,
            &row,
            Time::new(world.now())
        )
        .await
        .unwrap(),
        "a sampled timestamp is not this native source's authority"
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(!current(&world, &row).await);
    let mut malformed = row.clone();
    malformed.plan["environment_update"] = serde_json::Value::Null;
    let mut tx = world.app.app.db.begin().await.unwrap();
    let answer = orch_engine::jobs::recreate::batch_authority::current_with_clock(
        &mut tx,
        &malformed,
        world.app.app.clock.as_ref(),
    )
    .await;
    assert!(
        answer.is_err() || answer.is_ok_and(|allowed| !allowed),
        "a known malformed source never becomes an ordinary user-id job"
    );
}
