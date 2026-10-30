//! Update requests require management, expire and confirm once; batches hold each unit independently.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use async_trait::async_trait;
use orch_api_contract::{
    common::ErrorCode,
    environment as api,
    forks::{EnvironmentUpdatePlan, FilesPlan, ForkCreateBody},
    resources as provider_api,
};
use orch_types::{Id, Time, new_id};
use resources::{Company, company};
use serde_json::json;
use std::sync::Arc;
use support::World;

struct Builds {
    programs: bool,
}
#[async_trait]
impl orch_engine::hosting::HostServices for Builds {
    async fn enrollment(
        &self,
        _: &mut orch_db::Conn,
        _: &orch_engine::hosting::EnrollmentAsk,
    ) -> Result<orch_engine::hosting::Enrollment, orch_engine::hosting::HostingError> {
        Err(orch_engine::hosting::HostingError::Failed(
            "not executed by this API test".into(),
        ))
    }
    async fn tools(
        &self,
        _: &mut orch_db::Conn,
        _: &str,
        _: &str,
        harnesses: &[String],
    ) -> Result<Vec<orch_engine::hosting::ToolBuild>, orch_engine::hosting::HostingError> {
        Ok(if self.programs {
            harnesses
                .iter()
                .map(|kind| orch_engine::hosting::ToolBuild {
                    name: kind.clone(),
                    version: "0.164.0".into(),
                    url: "https://build.test/program".into(),
                    sha256: "a".repeat(64),
                })
                .collect()
        } else {
            vec![]
        })
    }
}

struct Team {
    c: Company,
    environment: Id,
    host: Id,
    template: Id,
    runtime: Id,
    provider: Id,
}
async fn team(world: &World) -> Team {
    world
        .app
        .app
        .engine
        .attach_host_services(Arc::new(Builds { programs: false }));
    let c = company(world).await;
    let mut conn = world.conn().await;
    let environment = orch_db::queries::resources::access_fixtures::insert_environment(
        &mut conn,
        c.admin.id(),
        Some(c.acme),
        "Main",
        "managed",
    )
    .await
    .unwrap();
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(c.admin.id()),
        Some(c.acme),
        "Build",
    )
    .await
    .unwrap();
    let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id = $1")
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE env_templates SET driver = 'docker', spec = '{\"image\":\"busybox:latest\",\"resources\":{},\"colocated_harnesses\":[]}' WHERE id = $1").bind(template).execute(&mut *conn).await.unwrap();
    sqlx::query(
        "UPDATE machines SET online = true WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)",
    )
    .bind(host)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET status = 'running', generation = 1, last_activity_at = $2 - INTERVAL '2 hours' WHERE id = $1").bind(environment).bind(world.now()).execute(&mut *conn).await.unwrap();
    let runtime = new_id();
    orch_db::queries::environments::hosting::insert_runtime(
        &mut conn,
        &orch_db::queries::environments::hosting::NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: "update-test-unit",
            environment_id: environment,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({}),
        },
        world.now(),
    )
    .await
    .unwrap();
    let provider = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(c.admin.id()),
        Some(c.acme),
        "Service",
    )
    .await
    .unwrap();
    let provider_template = new_id();
    sqlx::query("INSERT INTO env_templates (id,name,kind,driver,spec) VALUES ($1,'Runtime','provider','docker','{}')").bind(provider_template).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO runtimes (id,kind,host_id,unit_name,provider_id,driver,template_id,spec_hash,state) VALUES ($1,'provider',$2,'provider-update-unit',$3,'docker',$4,$5,'running')").bind(new_id()).bind(host).bind(provider).bind(provider_template).bind(vec![0u8;32]).execute(&mut *conn).await.unwrap();
    for (kind, id) in [
        ("environment", environment),
        ("host", host),
        ("provider", provider),
    ] {
        orch_authz::ownership::start_resource(
            &mut conn,
            orch_access_contract::decisions::ObjectRef {
                kind: match kind {
                    "environment" => orch_access_contract::permissions::ResourceKind::Environment,
                    "host" => orch_access_contract::permissions::ResourceKind::Host,
                    _ => orch_access_contract::permissions::ResourceKind::Provider,
                }
                .into(),
                id,
            },
            c.admin.id(),
            orch_authz::ownership::NewAccess::Defaults,
            Time::new(world.now()),
        )
        .await
        .unwrap();
    }
    Team {
        c,
        environment,
        host,
        template,
        runtime,
        provider,
    }
}

#[tokio::test]
async fn an_update_plan_explains_persistence_and_confirmation_makes_a_job_once() {
    let world = World::new().await;
    let team = team(&world).await;
    let plan: EnvironmentUpdatePlan = world
        .app
        .call(api::EnvironmentUpdatePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await
        .ok();
    assert!(!plan.updates.is_empty());
    assert!(!plan.kept.is_empty());
    assert!(!plan.lost.is_empty());
    let body = ForkCreateBody {
        notify: None,
        preflight_id: plan.preflight_id,
        options: None,
    };
    let job = world
        .app
        .call(api::EnvironmentUpdateCreate, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(job.kind, "environment_recreate");
    let reply = world
        .app
        .call(api::EnvironmentUpdateCreate, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(reply.status.as_u16(), 404);
    let status: String = sqlx::query_scalar("SELECT status FROM environments WHERE id = $1")
        .bind(team.environment)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "recreating");
}

#[tokio::test]
async fn update_and_recreate_requests_are_refused_to_somebody_without_manage() {
    let world = World::new().await;
    let team = team(&world).await;
    let plan: EnvironmentUpdatePlan = world
        .app
        .call(api::EnvironmentUpdatePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await
        .ok();
    let refused = world
        .app
        .call(api::EnvironmentUpdatePreflight, &[&team.environment])
        .token(&team.c.stranger.token)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let refused = world
        .app
        .call(api::EnvironmentUpdateCreate, &[&team.environment])
        .token(&team.c.stranger.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
}

#[tokio::test]
async fn an_expired_plan_cannot_take_a_hold() {
    let world = World::new().await;
    let team = team(&world).await;
    let plan: EnvironmentUpdatePlan = world
        .app
        .call(api::EnvironmentUpdatePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await
        .ok();
    world.advance(chrono::Duration::minutes(16));
    let refused = world
        .app
        .call(api::EnvironmentUpdateCreate, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await;
    assert_eq!(refused.problem().code, ErrorCode::Gone);
}

#[tokio::test]
async fn a_batch_keeps_its_scope_and_queues_each_distinct_environment_once() {
    let world = World::new().await;
    let team = team(&world).await;
    let refused = world
        .app
        .call(api::NodeEnvironmentUpdatePreflight, &[&team.c.acme])
        .token(&team.c.stranger.token)
        .body(&api::NodeEnvironmentUpdatePreflightBody {
            environment_ids: vec![team.environment],
        })
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let plan = world
        .app
        .call(api::NodeEnvironmentUpdatePreflight, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&api::NodeEnvironmentUpdatePreflightBody {
            environment_ids: vec![team.environment, team.environment],
        })
        .send()
        .await
        .ok();
    assert_eq!(plan.executable, 1);
    assert_eq!(plan.plans.len(), 1);
    let refused = world
        .app
        .call(api::NodeEnvironmentUpdateCreate, &[&team.c.acme])
        .token(&team.c.stranger.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let batch = world
        .app
        .call(api::NodeEnvironmentUpdateCreate, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(batch.jobs.len(), 1);
    assert_eq!(batch.jobs[0].job.batch_id, Some(batch.batch_id));
}

#[tokio::test]
async fn a_cross_host_batch_rechecks_each_member_and_never_confirms_a_place_plan() {
    let world = World::new().await;
    let team = team(&world).await;
    let mut conn = world.conn().await;
    let other = orch_db::queries::resources::access_fixtures::insert_environment(
        &mut conn,
        team.c.admin.id(),
        Some(team.c.acme),
        "Other",
        "managed",
    )
    .await
    .unwrap();
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(team.c.admin.id()),
        Some(team.c.acme),
        "Other host",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(host)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET template_id=$2,status='running',generation=1 WHERE id=$1")
        .bind(other)
        .bind(team.template)
        .execute(&mut *conn)
        .await
        .unwrap();
    orch_db::queries::environments::hosting::insert_runtime(
        &mut conn,
        &orch_db::queries::environments::hosting::NewRuntime {
            id: new_id(),
            host_id: host,
            unit_name: "other-update-unit",
            environment_id: other,
            driver: "docker",
            template_id: team.template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({}),
        },
        world.now(),
    )
    .await
    .unwrap();
    for (kind, id) in [
        (
            orch_access_contract::permissions::ResourceKind::Environment,
            other,
        ),
        (orch_access_contract::permissions::ResourceKind::Host, host),
    ] {
        orch_authz::ownership::start_resource(
            &mut conn,
            orch_access_contract::decisions::ObjectRef {
                kind: kind.into(),
                id,
            },
            team.c.admin.id(),
            orch_authz::ownership::NewAccess::Defaults,
            Time::new(world.now()),
        )
        .await
        .unwrap();
    }
    drop(conn);
    let plan = world
        .app
        .call(api::EnvironmentUpdateBatchPreflight, &[])
        .token(&team.c.admin.token)
        .body(&api::NodeEnvironmentUpdatePreflightBody {
            environment_ids: vec![team.environment, other, team.environment],
        })
        .send()
        .await
        .ok();
    assert_eq!(plan.executable, 2);
    assert_eq!(plan.plans.len(), 2);
    let wrong = world
        .app
        .call(api::NodeEnvironmentUpdateCreate, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::NotFound);
    let batch = world
        .app
        .call(api::EnvironmentUpdateBatchCreate, &[])
        .token(&team.c.admin.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(batch.jobs.len(), 2);
    assert_eq!(batch.jobs[0].job.batch_id, Some(batch.batch_id));
    let hidden = world
        .app
        .call(api::EnvironmentUpdateBatchPreflight, &[])
        .token(&team.c.stranger.token)
        .body(&api::NodeEnvironmentUpdatePreflightBody {
            environment_ids: vec![team.environment],
        })
        .send()
        .await
        .ok();
    assert_eq!(hidden.executable, 0);
    assert!(hidden.plans.is_empty());
}

#[tokio::test]
async fn an_update_notice_is_optional_and_reaches_a_person_once_for_several_affected_threads() {
    for notify in [false, true] {
        let world = World::new().await;
        let team = team(&world).await;
        let project = places::make_project(&world, &team.c.boss, team.c.acme, "Program work").await;
        let topic = places::make_topic(&world, &team.c.boss, project.base.id, "Runs").await;
        let mut conn = world.conn().await;
        for name in ["One", "Two"] {
            let thread = new_id();
            orch_db::queries::hierarchy::testing::insert_node(
                &mut conn,
                thread,
                "thread",
                Some(topic.base.id),
                name,
                team.c.boss.id(),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                .bind(thread)
                .bind(team.environment)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        drop(conn);
        let plan = world
            .app
            .call(api::EnvironmentUpdateBatchPreflight, &[])
            .token(&team.c.admin.token)
            .body(&api::NodeEnvironmentUpdatePreflightBody {
                environment_ids: vec![team.environment],
            })
            .send()
            .await
            .ok();
        let audience = plan.notify.unwrap();
        assert_eq!(
            audience
                .people
                .iter()
                .map(|person| person.id)
                .collect::<Vec<_>>(),
            [team.c.boss.id()]
        );
        assert_eq!(audience.hidden, 0);
        let batch = world
            .app
            .call(api::EnvironmentUpdateBatchCreate, &[])
            .token(&team.c.admin.token)
            .query(&orch_api_contract::forks::UpdateConfirmQuery {
                notify: Some(notify),
            })
            .body(&ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: Some(orch_api_contract::forks::ChangeOptions {
                    wait_minutes: Some(1500),
                    ..Default::default()
                }),
            })
            .send()
            .await
            .ok();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE user_id=$1 AND data->>'operation'=$2",
        )
        .bind(team.c.boss.id())
        .bind(batch.batch_id.to_string())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(count, i64::from(notify));
        if notify {
            let (thread,data):(Option<Id>,orch_types::Json)=sqlx::query_as("SELECT thread_id,data FROM notifications WHERE user_id=$1 AND data->>'operation'=$2")
                .bind(team.c.boss.id()).bind(batch.batch_id.to_string()).fetch_one(&world.db.pool).await.unwrap();
            assert!(
                thread.is_some(),
                "the notice opens affected work rather than an unfiltered inventory"
            );
            assert_eq!(data["affected_threads"], 2);
            assert_eq!(data["reason"], "requested");
        }
    }
}

#[tokio::test]
async fn the_plan_carries_the_check_of_the_exact_selected_artifact_instead_of_another_processor() {
    use orch_api_contract::admin::{
        HarnessArch, HarnessBuild, HarnessBuildCheck, HarnessBuildSource, HarnessBuildStatus,
        HarnessCheckStage,
    };
    let world = World::new().await;
    let team = team(&world).await;
    world
        .app
        .app
        .engine
        .attach_host_services(Arc::new(Builds { programs: true }));
    let mut conn = world.conn().await;
    sqlx::query("UPDATE env_templates SET spec=jsonb_set(spec,'{colocated_harnesses}','[\"codex\"]') WHERE id=$1").bind(team.template).execute(&mut *conn).await.unwrap();
    let selected = HarnessBuild {
        kind: "codex".into(),
        version: "0.164.0".into(),
        os: provider_api::MachineOs::Linux,
        arch: HarnessArch::X86_64,
        url: "https://build.test/program".into(),
        sha256: "a".repeat(64),
        is_default: false,
        source: HarnessBuildSource::Channel,
        status: HarnessBuildStatus::Failed,
        check: Some(HarnessBuildCheck {
            at: Time::new(world.now()),
            machine: Some("Selected host".into()),
            missing: vec!["TurnStartParams.collaborationMode".into()],
            off: vec!["plan_mode".into()],
            error: None,
            stage: HarnessCheckStage::Complete,
            cases: vec![],
        }),
        carry: None,
    };
    let other = HarnessBuild {
        arch: HarnessArch::Aarch64,
        sha256: "b".repeat(64),
        status: HarnessBuildStatus::Untested,
        check: None,
        ..selected.clone()
    };
    orch_api_core::harness_channels::keep(&mut conn, &[other, selected.clone()], world.now())
        .await
        .unwrap();
    drop(conn);
    let plan = world
        .app
        .call(api::EnvironmentUpdatePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await
        .ok();
    let target = plan
        .target_programs
        .unwrap()
        .into_iter()
        .find(|program| program.kind == "codex")
        .unwrap();
    assert_eq!(target.build, Some(selected));
    assert!(
        plan.blockers.is_empty(),
        "a completed failed check is a warning rather than a block"
    );
}

#[tokio::test]
async fn a_selected_target_that_changes_after_review_is_refused_before_any_unit_is_held() {
    for batch in [false, true] {
        let world = World::new().await;
        let team = team(&world).await;
        let preflight = if batch {
            world
                .app
                .call(api::EnvironmentUpdateBatchPreflight, &[])
                .token(&team.c.admin.token)
                .body(&api::NodeEnvironmentUpdatePreflightBody {
                    environment_ids: vec![team.environment],
                })
                .send()
                .await
                .ok()
                .preflight_id
        } else {
            world
                .app
                .call(api::EnvironmentUpdatePreflight, &[&team.environment])
                .token(&team.c.admin.token)
                .body(&api::UpdatePreflightBody::default())
                .send()
                .await
                .ok()
                .preflight_id
        };
        sqlx::query("UPDATE env_templates SET spec=jsonb_set(spec,'{image}','\"busybox:other\"') WHERE id=$1").bind(team.template).execute(&world.db.pool).await.unwrap();
        let body = ForkCreateBody {
            notify: None,
            preflight_id: preflight,
            options: None,
        };
        let error = if batch {
            world
                .app
                .call(api::EnvironmentUpdateBatchCreate, &[])
                .token(&team.c.admin.token)
                .body(&body)
                .send()
                .await
                .problem()
        } else {
            world
                .app
                .call(api::EnvironmentUpdateCreate, &[&team.environment])
                .token(&team.c.admin.token)
                .body(&body)
                .send()
                .await
                .problem()
        };
        assert_eq!(error.code, ErrorCode::ConflictStaleRevision);
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1")
            .bind(team.environment)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(jobs, 0);
        let status: String = sqlx::query_scalar("SELECT status FROM environments WHERE id=$1")
            .bind(team.environment)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(status, "running");
    }
}

#[tokio::test]
async fn a_provider_runtime_has_the_same_plan_and_job_with_its_state_kept() {
    let world = World::new().await;
    let team = team(&world).await;
    let plan = world
        .app
        .call(
            provider_api::ProviderRuntimeUpdatePreflight,
            &[&team.provider],
        )
        .token(&team.c.admin.token)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await
        .ok();
    assert_eq!(plan.provider.id, team.provider);
    assert!(plan.kept.iter().any(|s| s.contains("state")));
    let refused = world
        .app
        .call(provider_api::ProviderRuntimeUpdateCreate, &[&team.provider])
        .token(&team.c.stranger.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let job = world
        .app
        .call(provider_api::ProviderRuntimeUpdateCreate, &[&team.provider])
        .token(&team.c.admin.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(job.kind, "provider_recreate");
    let refused = world
        .app
        .call(
            provider_api::ProviderRuntimeUpdatePreflight,
            &[&team.provider],
        )
        .token(&team.c.stranger.token)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
}

#[tokio::test]
async fn provider_batches_recheck_permissions_and_group_their_jobs() {
    let world = World::new().await;
    let team = team(&world).await;
    let hidden = world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchPreflight, &[])
        .token(&team.c.stranger.token)
        .body(&provider_api::ProviderRuntimeUpdateBatchPreflightBody {
            provider_ids: vec![team.provider],
        })
        .send()
        .await
        .ok();
    assert_eq!(hidden.executable, 0);
    assert!(hidden.plans.is_empty());
    let plan = world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchPreflight, &[])
        .token(&team.c.admin.token)
        .body(&provider_api::ProviderRuntimeUpdateBatchPreflightBody {
            provider_ids: vec![team.provider],
        })
        .send()
        .await
        .ok();
    assert_eq!(plan.executable, 1);
    let body = ForkCreateBody {
        notify: None,
        preflight_id: plan.preflight_id,
        options: None,
    };
    let refused = world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchCreate, &[])
        .token(&team.c.stranger.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 404);
    let batch = world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchCreate, &[])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(batch.jobs.len(), 1);
    assert_eq!(batch.jobs[0].job.batch_id, Some(batch.batch_id));
}

#[tokio::test]
async fn host_removal_is_refused_while_units_exist_and_revokes_an_empty_hosts_machine() {
    let world = World::new().await;
    let team = team(&world).await;
    let query = provider_api::HostDeleteQuery {
        confirm: "Build".into(),
    };
    let refused = world
        .app
        .call(provider_api::HostDelete, &[&team.host])
        .token(&team.c.admin.token)
        .query(&query)
        .send()
        .await;
    assert_eq!(refused.problem().code, ErrorCode::ResourceInUse);
    let refused = world
        .app
        .call(provider_api::HostDelete, &[&team.host])
        .token(&team.c.stranger.token)
        .query(&query)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    sqlx::query("UPDATE runtimes SET state = 'removed' WHERE host_id = $1")
        .bind(team.host)
        .execute(&world.db.pool)
        .await
        .unwrap();
    world
        .app
        .call(provider_api::HostDelete, &[&team.host])
        .token(&team.c.admin.token)
        .query(&query)
        .send()
        .await
        .ok();
    let (status,deleted):(String,bool)=sqlx::query_as("SELECT status, deleted_at IS NOT NULL FROM machines WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)").bind(team.host).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(status, "revoked");
    assert!(deleted);
    let removed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id = $1 AND action = 'host.deleted'",
    )
    .bind(team.host)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(removed, 1);
}

#[tokio::test]
async fn host_metrics_are_filtered_to_that_host_and_protected_by_its_manage_permission() {
    let world = World::new().await;
    let team = team(&world).await;
    let mut conn = world.conn().await;
    orch_db::migrations::ensure_partitions(&mut conn, world.now())
        .await
        .unwrap();
    for host in [team.host, new_id()] {
        sqlx::query("INSERT INTO metric_samples (at,name,labels,value) VALUES ($1,'host_memory_bytes',$2,128)").bind(world.now()).bind(json!({"host_id":host})).execute(&mut *conn).await.unwrap();
    }
    drop(conn);
    let query = provider_api::HostMetricsListQuery {
        from: Time::new(world.now() - chrono::Duration::minutes(1)),
        to: Time::new(world.now() + chrono::Duration::minutes(1)),
        step: Some(1),
    };
    let shown = world
        .app
        .call(provider_api::HostMetricsList, &[&team.host])
        .token(&team.c.admin.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].labels["host_id"], team.host.to_string());
    let refused = world
        .app
        .call(provider_api::HostMetricsList, &[&team.host])
        .token(&team.c.stranger.token)
        .query(&query)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
}

#[tokio::test]
async fn recycle_skips_kept_awake_units_and_units_held_by_a_job_in_its_preview() {
    let world = World::new().await;
    let team = team(&world).await;
    sqlx::query("UPDATE environments SET config = '{\"env.idle_stop_minutes\":0}' WHERE id = $1")
        .bind(team.environment)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let body = api::RecycleBody {
        idle_minutes: 60,
        dry_run: true,
        include_with_processes: Some(true),
    };
    let refused = world
        .app
        .call(api::NodeEnvironmentRecycle, &[&team.c.acme])
        .token(&team.c.stranger.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let preview = world
        .app
        .call(api::NodeEnvironmentRecycle, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(preview.candidates.is_empty());
    assert_eq!(preview.skipped_active, 1);
    let preview = world
        .app
        .call(provider_api::HostRecycle, &[&team.host])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(preview.candidates.is_empty());
    let refused = world
        .app
        .call(provider_api::HostRecycle, &[&team.host])
        .token(&team.c.stranger.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    sqlx::query("UPDATE environments SET config = '{}' WHERE id = $1")
        .bind(team.environment)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let plan = world
        .app
        .call(api::EnvironmentUpdatePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await
        .ok();
    world
        .app
        .call(api::EnvironmentUpdateCreate, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .ok();
    let preview = world
        .app
        .call(provider_api::HostRecycle, &[&team.host])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(preview.candidates.is_empty());
    assert_eq!(preview.skipped_active, 1);
    let _ = (team.template, team.runtime);
}

#[tokio::test]
async fn recycle_previews_and_stops_the_same_idle_units_and_returns_a_permission_skip_count() {
    let world = World::new().await;
    let team = team(&world).await;
    let body = api::RecycleBody {
        idle_minutes: 60,
        dry_run: true,
        include_with_processes: Some(true),
    };
    let preview = world
        .app
        .call(api::NodeEnvironmentRecycle, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(preview.candidates.len(), 1);
    assert_eq!(preview.stopped, 0);
    let mut conn = world.conn().await;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id = $1")
        .bind(team.host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE runtimes SET state = 'running' WHERE id = $1")
        .bind(team.runtime)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let answer = resources::answer(&world, machine, json!({}));
    let stopped = world
        .app
        .call(provider_api::HostRecycle, &[&team.host])
        .token(&team.c.admin.token)
        .body(&api::RecycleBody {
            dry_run: false,
            ..body
        })
        .send()
        .await
        .ok();
    answer.await.unwrap();
    assert_eq!(stopped.stopped, 1);
    assert_eq!(stopped.candidates[0].environment.id, team.environment);
    let status: String = sqlx::query_scalar("SELECT status FROM environments WHERE id = $1")
        .bind(team.environment)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "stopped");
}

async fn movable(world: &World, team: &Team) -> Id {
    let mut conn = world.conn().await;
    sqlx::query("UPDATE env_templates SET owner_id = $2,home_id = $3,org_id = $3 WHERE id = $1")
        .bind(team.template)
        .bind(team.c.admin.id())
        .bind(team.c.acme)
        .execute(&mut *conn)
        .await
        .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ResourceKind::Template.into(),
            id: team.template,
        },
        team.c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let template: orch_types::Json =
        sqlx::query_scalar("SELECT spec FROM env_templates WHERE id = $1")
            .bind(team.template)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    sqlx::query("INSERT INTO jobs (id,kind,environment_id,state,plan) VALUES ($1,'environment_provision',$2,'succeeded',$3)").bind(new_id()).bind(team.environment).bind(json!({"runtime":team.runtime,"created_spec":{"spec":template,"resources":{},"tools":[]}})).execute(&mut *conn).await.unwrap();
    let header = json!({"v":1,"root":"/workspace","generated_at":Time::new(world.now()),"generation":1,"case_sensitive":true});
    let bytes = zstd::stream::encode_all(format!("{header}\n").as_bytes(), 0).unwrap();
    let manifest = world
        .app
        .app
        .engine
        .put_manifest(&mut conn, bytes)
        .await
        .unwrap();
    orch_db::queries::environments::workspace::keep_manifest(
        &mut conn,
        team.environment,
        &manifest.hash,
        world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET workspace_root = '/workspace' WHERE id = $1")
        .bind(team.environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(team.c.admin.id()),
        Some(team.c.acme),
        "Target",
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ResourceKind::Host.into(),
            id: host,
        },
        team.c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET drivers = $2,capacity = $3,storages = $4 WHERE id = $1").bind(host).bind(json!({"docker":{"name":"docker","title":"Docker","spec_schemas":{"environment":{},"provider":{}},"runtime_classes":["runc"],"sources":["image","build","archive"]}})).bind(json!({"cpus":8,"memory_mb":16_384,"disk_gb":100})).bind(json!([{"name":"fast","default":true,"capacity_gb":100,"free_gb":100,"private_views":false}])).execute(&mut *conn).await.unwrap();
    // The destination writes the moved files, which its machine's own policy must allow.
    sqlx::query(
        "UPDATE machines SET online = true,
                facts = jsonb_set(COALESCE(facts, '{}'), '{policy}',
                                  COALESCE(facts->'policy', '{}') || '{\"environment.remote_write\": true}')
          WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)",
    )
    .bind(host)
    .execute(&mut *conn)
    .await
    .unwrap();
    // A managed unit owns its home volume, which keeps the environment's file-state store: every
    // whole-environment move measures that store on the source and carries it.
    sqlx::query("UPDATE runtimes SET volumes = $2 WHERE id = $1")
        .bind(team.runtime)
        .bind(json!({
            "workspace": {"name": "update-test-workspace", "storage": null},
            "home": {"name": "update-test-home", "storage": null},
        }))
        .execute(&mut *conn)
        .await
        .unwrap();
    host
}

/// The complete, empty recorded-state store this fixture's source machine reports.
fn file_state_count() -> serde_json::Value {
    use orch_driver_contract::volumes::{FileStatesMeasure, OwnedVolumeSelector, VolumeMeasure};
    use orch_link_contract::commands::{MeasuredVolume, NodeRuntimeVolumesMeasureResult};
    serde_json::to_value(NodeRuntimeVolumesMeasureResult {
        volumes: vec![MeasuredVolume {
            selector: OwnedVolumeSelector::FileStates,
            measure: VolumeMeasure {
                file_count: 0,
                total_bytes: 0,
                file_states: Some(FileStatesMeasure {
                    roots: 0,
                    objects: 0,
                    total_bytes: 0,
                    journals: 0,
                    complete: true,
                }),
            },
        }],
    })
    .unwrap()
}

/// Completes the detached application's count, as the worker whose full path is tested by the engine.
async fn finish_move_count(world: &World, team: &Team, files: &FilesPlan) {
    use orch_driver_contract::volumes::OwnedVolumeSelector;
    use orch_link_contract::commands::NodeRuntimeVolumesMeasure;
    let reference = files
        .file_states
        .as_ref()
        .unwrap()
        .measurement
        .as_ref()
        .unwrap();
    assert!(reference.measured_at.is_none());
    let mut conn = world.conn().await;
    let row = orch_db::queries::jobs::job(&mut conn, reference.job_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "measurement");
    assert_eq!(row.state, "queued");
    assert_eq!(row.created_by, Some(team.c.admin.id()));
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(team.host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(row.plan["machine"], json!(machine));
    assert_eq!(row.plan["command_kind"], "node.runtime.volumes.measure");
    let request: NodeRuntimeVolumesMeasure =
        serde_json::from_value(row.plan["body"].clone()).unwrap();
    assert_eq!(request.runtime_id, team.runtime);
    assert_eq!(request.volumes.len(), 1);
    assert_eq!(request.volumes[0].selector, OwnedVolumeSelector::FileStates);
    let mut progress = row.progress;
    progress["percent"] = json!(100);
    assert!(
        orch_db::queries::jobs::finish(
            &mut conn,
            row.id,
            "succeeded",
            &progress,
            Some(&file_state_count()),
            None,
            world.now()
        )
        .await
        .unwrap()
    );
}

/// Answers, for as long as the test runs, what planning a move asks of the two machines: the source
/// measures its whole file-state store, and the destination says that it takes whole volumes.
async fn answer_move_plans(world: &World, team: &Team, target: Id) -> tokio::task::JoinHandle<()> {
    use orch_link_contract::commands::NodeVolumeCarryCheckResult;
    let machine_of = |host: Id| {
        let pool = world.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, Id>("SELECT machine_id FROM hosts WHERE id = $1")
                .bind(host)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let measured = file_state_count();
    let takes = serde_json::to_value(NodeVolumeCarryCheckResult {
        available: true,
        reason: None,
    })
    .unwrap();
    let answers = [
        (
            machine_of(team.host).await,
            "node.runtime.volumes.measure",
            measured,
        ),
        (machine_of(target).await, "node.volume.carry-check", takes),
    ];
    let pool = world.db.pool.clone();
    tokio::spawn(async move {
        loop {
            for (machine, kind, result) in &answers {
                let answered = sqlx::query(
                    "UPDATE commands SET state = 'succeeded', result = $3
                      WHERE machine_id = $1 AND kind = $2 AND state IN ('queued', 'sent', 'accepted')",
                )
                .bind(machine)
                .bind(kind)
                .bind(result)
                .execute(&pool)
                .await;
                if answered.is_err() {
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
}

#[tokio::test]
async fn moving_one_environment_preserves_its_identity_and_requires_its_manage_permission() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let request = api::EnvironmentMovePreflightBody {
        machine_id: None,
        template_id: None,
        copy_directories: None,
        host_id: Some(target),
        home: None,
        private_views: None,
        files: None,
        check_fingerprints: None,
        fingerprint_request_id: None,
        mounts: None,
    };
    let plan = world
        .app
        .call(api::EnvironmentMovePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&request)
        .send()
        .await;
    assert!(plan.body.is_ok(), "{:?}", plan.body);
    let plan = plan.ok();
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.code == "measurement_pending")
    );
    finish_move_count(&world, &team, &plan.files).await;
    let plan = world
        .app
        .call(api::EnvironmentMovePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(plan.files.file_states.as_ref().unwrap().complete);
    assert_eq!(
        plan.files
            .file_states
            .as_ref()
            .unwrap()
            .measurement
            .as_ref()
            .unwrap()
            .measured_at,
        Some(Time::new(world.now()))
    );
    assert_eq!(plan.environment.id, team.environment);
    let refused = world
        .app
        .call(api::EnvironmentMovePreflight, &[&team.environment])
        .token(&team.c.stranger.token)
        .body(&request)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let body = ForkCreateBody {
        notify: None,
        preflight_id: plan.preflight_id,
        options: None,
    };
    let refused = world
        .app
        .call(api::EnvironmentMoveCreate, &[&team.environment])
        .token(&team.c.stranger.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 404);
    let job = world
        .app
        .call(api::EnvironmentMoveCreate, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(job.kind, "environment_move");
    assert_eq!(job.environment_id, Some(team.environment));
    answers.abort();
}

#[tokio::test]
async fn a_fleet_move_plans_and_confirms_each_distinct_unit_and_is_refused_outside_the_callers_place()
 {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let request = api::NodeEnvironmentMovePreflightBody {
        machine_id: None,
        template_id: None,
        copy_directories: None,
        environment_ids: vec![team.environment, team.environment],
        host_id: Some(target),
    };
    let refused = world
        .app
        .call(api::NodeEnvironmentMovePreflight, &[&team.c.acme])
        .token(&team.c.stranger.token)
        .body(&request)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let plan = world
        .app
        .call(api::NodeEnvironmentMovePreflight, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(plan.executable, 0);
    assert_eq!(plan.plans.len(), 1);
    assert!(
        plan.plans[0]
            .blockers
            .iter()
            .any(|blocker| blocker.code == "measurement_pending")
    );
    finish_move_count(&world, &team, &plan.plans[0].files).await;
    let plan = world
        .app
        .call(api::NodeEnvironmentMovePreflight, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(plan.executable, 1, "{:?}", plan.plans);
    assert_eq!(plan.plans.len(), 1);
    assert!(plan.plans[0].files.file_states.as_ref().unwrap().complete);
    let body = ForkCreateBody {
        notify: None,
        preflight_id: plan.preflight_id,
        options: None,
    };
    let refused = world
        .app
        .call(api::NodeEnvironmentMoveCreate, &[&team.c.acme])
        .token(&team.c.stranger.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let batch = world
        .app
        .call(api::NodeEnvironmentMoveCreate, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(batch.jobs.len(), 1);
    assert_eq!(batch.jobs[0].job.kind, "environment_move");
    answers.abort();
}

#[tokio::test]
async fn single_updates_keep_both_notification_transports_with_explicit_query_precedence() {
    use orch_api_contract::forks::UpdateConfirmQuery;
    for provider in [false, true] {
        for (query, body_notify, expected) in [
            (None, false, false),
            (Some(false), true, false),
            (Some(true), false, true),
        ] {
            let world = World::new().await;
            let team = team(&world).await;
            let project =
                places::make_project(&world, &team.c.boss, team.c.acme, "Notify transports").await;
            let topic =
                places::make_topic(&world, &team.c.boss, project.base.id, "Affected work").await;
            let mut conn = world.conn().await;
            let thread = new_id();
            orch_db::queries::hierarchy::testing::insert_node(
                &mut conn,
                thread,
                "thread",
                Some(topic.base.id),
                "Working here",
                team.c.boss.id(),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE threads SET environment_id=$2,provider_id=$3 WHERE node_id=$1")
                .bind(thread)
                .bind(team.environment)
                .bind(team.provider)
                .execute(&mut *conn)
                .await
                .unwrap();
            drop(conn);
            let (preflight_id, audience) = if provider {
                let plan = world
                    .app
                    .call(
                        provider_api::ProviderRuntimeUpdatePreflight,
                        &[&team.provider],
                    )
                    .token(&team.c.admin.token)
                    .body(&api::UpdatePreflightBody::default())
                    .send()
                    .await
                    .ok();
                (plan.preflight_id, plan.notify.unwrap())
            } else {
                let plan = world
                    .app
                    .call(api::EnvironmentUpdatePreflight, &[&team.environment])
                    .token(&team.c.admin.token)
                    .body(&api::UpdatePreflightBody::default())
                    .send()
                    .await
                    .ok();
                (plan.preflight_id, plan.notify.unwrap())
            };
            assert_eq!(
                audience
                    .people
                    .iter()
                    .map(|person| person.id)
                    .collect::<Vec<_>>(),
                [team.c.boss.id()]
            );
            let body = ForkCreateBody {
                notify: Some(body_notify),
                preflight_id,
                options: None,
            };
            let query = UpdateConfirmQuery { notify: query };
            let job = if provider {
                world
                    .app
                    .call(provider_api::ProviderRuntimeUpdateCreate, &[&team.provider])
                    .token(&team.c.admin.token)
                    .query(&query)
                    .body(&body)
                    .send()
                    .await
                    .ok()
            } else {
                world
                    .app
                    .call(api::EnvironmentUpdateCreate, &[&team.environment])
                    .token(&team.c.admin.token)
                    .query(&query)
                    .body(&body)
                    .send()
                    .await
                    .ok()
            };
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM notifications WHERE user_id=$1 AND data->>'operation'=$2",
            )
            .bind(team.c.boss.id())
            .bind(job.id.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            assert_eq!(count, i64::from(expected));
        }
    }
}
