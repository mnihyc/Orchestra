//! A managed provider's runtime: made by a job whose phases are a provider's, on a host its owner may use, with the one volume it keeps for as long as the provider exists; started again when it is stopped, by hand or by the turn that needs it; stopped by a person, never with a turn running unless that is asked for, and when it has been idle; retired with the provider and taken away by a job of its own; and a sign-in that waited for it is forgotten if it is not made.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod runtimes;

use chrono::{Duration, Utc};
use common::{World, eventually, meanwhile};
use hosting::{ScriptedHost, phases, serve, wait_over};
use orch_authz::{Caller, Principal};
use orch_db::queries::resources::provider_runtime as rows;
use orch_engine::jobs::provider_provision::Mode;
use orch_engine::provider_runtime::{HoldWhy, RuntimeError, RuntimeStart, StopWhy};
use orch_link_contract::commands::{CommandError, CommandKind as K, ErrorCode};
use orch_types::{Id, Time, new_id};
use runtimes::{
    enrol, made, managed_subscription, managed_world_provider, provider_template, queue,
};
use serde_json::json;

async fn runtime_state(world: &World, runtime: Id) -> String {
    sqlx::query_scalar("SELECT state FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

async fn machine_of(world: &World, provider: Id) -> Option<Id> {
    sqlx::query_scalar("SELECT machine_id FROM providers WHERE id = $1")
        .bind(provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

async fn age_provider_runtime(world: &World, runtime: Id) {
    let old = world.clock.now() - Duration::minutes(2);
    sqlx::query("UPDATE runtimes SET created_at=$2,last_active_at=$2 WHERE id=$1")
        .bind(runtime)
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE harnesses SET last_active_at=$2 WHERE provider_id=$1")
        .bind(world.provider)
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
}

async fn wait_provider_runtime_job(world: &World) {
    let mut conn = world.pool.acquire().await.unwrap();
    let job = orch_db::queries::jobs::provider_jobs::latest_of_provider(&mut conn, world.provider)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    let (state, _, error) = wait_over(world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
}

#[tokio::test]
async fn scheduled_reads_preserve_deliberate_stops_and_refresh_restarts_them() {
    use orch_db::queries::provider_facts;
    for idle in [true, false] {
        let world = World::observed().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "observation-host", 16_384).await;
        let template = provider_template(&world).await;
        let runtime = managed_world_provider(&world, &host, template).await;
        if idle {
            world
                .set(world.root, "runtime.idle_stop_minutes", json!(1))
                .await;
            age_provider_runtime(&world, runtime).await;
            eventually("the configured idle stop", || async {
                runtime_state(&world, runtime).await == "stopped"
            })
            .await;
        } else {
            world
                .engine()
                .stop_provider_runtime(world.provider, false, StopWhy::Person(world.ada))
                .await
                .unwrap();
        }
        host.tell(runtime, "stopped", None).await;
        let detail: Option<String> =
            sqlx::query_scalar("SELECT state_detail FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert_eq!(
            detail.as_deref(),
            Some(if idle {
                rows::STOPPED_IDLE
            } else {
                rows::STOPPED_PERSON
            })
        );
        let old = world.clock.now() - Duration::hours(2);
        let saved = json!({"plan":"saved observation"});
        sqlx::query("UPDATE providers SET facts=$2,facts_at=$3,facts_requested_at=$3 WHERE id=$1")
            .bind(world.provider)
            .bind(&saved)
            .bind(old)
            .execute(world.pool.pool())
            .await
            .unwrap();
        let old: chrono::DateTime<Utc> =
            sqlx::query_scalar("SELECT facts_requested_at FROM providers WHERE id=$1")
                .bind(world.provider)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        let starts = host.count(K::NodeRuntimeStart);
        let mut conn = world.pool.acquire().await.unwrap();
        assert!(
            !world
                .engine()
                .due_provider_fact_reads(&mut conn, world.clock.now(), 100)
                .await
                .unwrap()
                .contains(&world.provider)
        );
        drop(conn);
        let mut tx = world.pool.begin().await.unwrap();
        assert!(
            !provider_facts::claim_scheduled_read(
                &mut tx,
                world.provider,
                world.clock.now(),
                Duration::minutes(1)
            )
            .await
            .unwrap(),
            "a stale queued read must not spend an observation interval after a stop"
        );
        tx.commit().await.unwrap();
        world.engine().refresh_due_provider_facts().await.unwrap();
        world
            .engine()
            .refresh_scheduled_provider(world.provider)
            .await
            .unwrap();
        // Four fixture reaper intervals expose an incorrectly reopened runtime or model source.
        meanwhile(std::time::Duration::from_millis(600)).await;
        assert_eq!(host.count(K::NodeRuntimeStart), starts);
        assert_eq!(runtime_state(&world, runtime).await, "stopped");
        let retained: (
            serde_json::Value,
            Option<chrono::DateTime<Utc>>,
            Option<chrono::DateTime<Utc>>,
        ) = sqlx::query_as("SELECT facts,facts_at,facts_requested_at FROM providers WHERE id=$1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        assert_eq!(
            retained,
            (saved, Some(old), Some(old)),
            "the saved facts and both clocks survive scheduled reads"
        );
        sqlx::query("UPDATE providers SET facts_requested_at=$2 WHERE id=$1")
            .bind(world.provider)
            .bind(world.clock.now())
            .execute(world.pool.pool())
            .await
            .unwrap();
        world
            .engine()
            .refresh_provider(world.provider)
            .await
            .unwrap();
        host.wait_for(K::NodeRuntimeStart, starts + 1).await;
        wait_provider_runtime_job(&world).await;
        eventually("the explicitly refreshed runtime to run", || async {
            runtime_state(&world, runtime).await == "running"
        })
        .await;
        let detail: Option<String> =
            sqlx::query_scalar("SELECT state_detail FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert!(
            detail.is_none(),
            "starting the runtime clears the deliberate stop reason"
        );
    }
}

#[tokio::test]
async fn scheduled_reads_restart_a_runtime_stopped_by_its_host() {
    let world = World::observed().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "unexpected-stop-host", 16_384).await;
    let template = provider_template(&world).await;
    let runtime = managed_world_provider(&world, &host, template).await;
    let starts = host.count(K::NodeRuntimeStart);
    host.tell(runtime, "stopped", None).await;
    eventually(
        "the real host report to detach the stopped unit",
        || async {
            runtime_state(&world, runtime).await == "failed"
                && machine_of(&world, world.provider).await.is_none()
        },
    )
    .await;
    sqlx::query("UPDATE providers SET facts_requested_at=$2 WHERE id=$1")
        .bind(world.provider)
        .bind(world.clock.now() - Duration::hours(2))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().refresh_due_provider_facts().await.unwrap();
    host.wait_for(K::NodeRuntimeStart, starts + 1).await;
    let restarted = host.started().unwrap();
    if restarted != runtime {
        let _unit = enrol(&world, restarted).await;
    }
    wait_provider_runtime_job(&world).await;
    eventually("the unexpectedly stopped provider to run again", || async {
        runtime_state(&world, restarted).await == "running"
            && machine_of(&world, world.provider).await.is_some()
    })
    .await;
}

#[tokio::test]
async fn renewal_starts_an_idle_stopped_runtime_and_only_its_idle_setting_stops_it_again() {
    for idle_minutes in [0, 1] {
        let world = World::observed().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "idle-renewal-host", 16_384).await;
        let template = provider_template(&world).await;
        let runtime = managed_world_provider(&world, &host, template).await;
        world.machine.handler.on(K::ProviderRefresh, |_| {
            Ok(
                orch_link_contract::commands::CommandResult::ProviderRefresh(
                    orch_link_contract::commands::ProviderRefreshResult {
                        source: None,
                        status: "ready".into(),
                        facts: serde_json::from_value(
                            json!({"credential":{"yields":"login","expires_at":null}}),
                        )
                        .unwrap(),
                    },
                ),
            )
        });
        world
            .set(world.root, "runtime.idle_stop_minutes", json!(1))
            .await;
        age_provider_runtime(&world, runtime).await;
        eventually("the provider to stop before renewal", || async {
            runtime_state(&world, runtime).await == "stopped"
        })
        .await;
        assert_eq!(runtime_state(&world, runtime).await, "stopped");
        world
            .set(world.root, "runtime.idle_stop_minutes", json!(idle_minutes))
            .await;
        let RuntimeStart::Job(job) = world
            .engine()
            .renew_provider_runtime(world.provider)
            .await
            .unwrap()
        else {
            panic!("renewal starts an idle-stopped runtime by its ordinary job");
        };
        let (state, _, error) = wait_over(&world, job.id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        assert_eq!(runtime_state(&world, runtime).await, "running");
        assert_eq!(host.count(K::NodeRuntimeStart), 1);
        age_provider_runtime(&world, runtime).await;
        if idle_minutes > 0 {
            eventually(
                "renewal's runtime to obey its opted-in idle stop",
                || async { runtime_state(&world, runtime).await == "stopped" },
            )
            .await;
        } else {
            meanwhile(std::time::Duration::from_millis(600)).await;
            assert_eq!(runtime_state(&world, runtime).await, "running");
            assert_eq!(
                host.count(K::NodeRuntimeStop),
                1,
                "renewal itself never stops the runtime"
            );
        }
    }
}

async fn remove_from_host(
    world: &World,
    host: Id,
    runtime: Id,
    volumes: bool,
) -> Result<(), RuntimeError> {
    remove_from_host_with(world, host, runtime, volumes, false).await
}

async fn remove_from_host_with(
    world: &World,
    host: Id,
    runtime: Id,
    volumes: bool,
    force: bool,
) -> Result<(), RuntimeError> {
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    world
        .engine()
        .remove_host_provider_unit_in(
            tx,
            &principal,
            &orch_db::audit::Origin::default(),
            host,
            runtime,
            volumes,
            force,
        )
        .await
}

#[tokio::test]
async fn a_host_removes_a_provider_unit_and_the_next_turn_makes_a_new_one_with_only_requested_volumes_deleted()
 {
    for volumes in [false, true] {
        let world = World::new().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "host-removal", 16_384).await;
        let _template = provider_template(&world).await;
        sqlx::query("UPDATE providers SET machine_id=NULL WHERE id=$1")
            .bind(world.provider)
            .execute(world.pool.pool())
            .await
            .unwrap();
        let (runtime, unit) = made(&world, &host, world.provider).await;
        remove_from_host(&world, host.id, runtime, volumes)
            .await
            .unwrap();
        eventually("the requested provider unit to be removed", || async {
            runtime_state(&world, runtime).await == "removed"
        })
        .await;
        assert!(machine_of(&world, world.provider).await.is_none());
        let revoked: String = sqlx::query_scalar("SELECT status FROM machines WHERE id=$1")
            .bind(unit.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        assert_eq!(revoked, "revoked");
        let removed = host
            .calls()
            .into_iter()
            .filter(|call| {
                matches!(
                    call.kind,
                    K::NodeRuntimeStop | K::NodeRuntimeRemove | K::NodeVolumeRemove
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            removed.iter().map(|call| call.kind).collect::<Vec<_>>(),
            if volumes {
                vec![
                    K::NodeRuntimeStop,
                    K::NodeRuntimeRemove,
                    K::NodeVolumeRemove,
                ]
            } else {
                vec![K::NodeRuntimeStop, K::NodeRuntimeRemove]
            }
        );
        assert_eq!(removed[0].body["runtime_id"], json!(runtime));
        assert_eq!(removed[1].body["runtime_id"], json!(runtime));
        assert_eq!(
            removed[1].body["keep_volumes"], true,
            "only the explicit volume phase may delete data"
        );
        if volumes {
            assert_eq!(
                removed[2].body["name"],
                format!("orch-state-{}", world.provider)
            );
        }
        // Use the actual turn start path, rather than a manual Start request.
        let thread = world.thread("After host removal").await;
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                common::queued("Continue after removal"),
            )
            .await
            .unwrap();
        host.wait_for(K::NodeRuntimeStart, 2).await;
        let next = host.started().unwrap();
        assert_ne!(next, runtime);
        let next_unit = enrol(&world, next).await;
        let _start = world.turn_start(thread, 0).await;
        assert_eq!(machine_of(&world, world.provider).await, Some(next_unit.id));
        assert_eq!(runtime_state(&world, next).await, "running");
        assert_eq!(runtime_state(&world, runtime).await, "removed");
        let volumes_ensured = host
            .calls()
            .into_iter()
            .filter(|call| call.kind == K::NodeVolumeEnsure)
            .collect::<Vec<_>>();
        assert_eq!(volumes_ensured.len(), 2);
        assert_eq!(
            volumes_ensured[0].body["name"], volumes_ensured[1].body["name"],
            "retained data is reused; purged data is made anew at its owned name"
        );
    }
}

#[tokio::test]
async fn retrying_a_failed_host_removal_keeps_its_volume_consent() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "retry-removal", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Keep the data").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    host.refuse(
        K::NodeRuntimeRemove,
        CommandError::new(ErrorCode::Internal, "the host could not remove the unit"),
    );
    remove_from_host(&world, host.id, runtime, false)
        .await
        .unwrap();
    eventually("the first removal attempt to fail", || async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM jobs WHERE provider_id=$1 AND kind='environment_remove' AND state='failed')").bind(provider).fetch_one(world.pool.pool()).await.unwrap()
    }).await;
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    let mut tx = world.pool.begin().await.unwrap();
    let queued = orch_engine::jobs::provider_provision::queue_removal(
        &mut tx,
        world.clock.now(),
        provider,
        runtime,
    )
    .await
    .unwrap();
    let job = match queued {
        orch_db::queries::jobs::provider_jobs::Queued::Made(job) => job,
        other => panic!("the failed removal can be retried: {other:?}"),
    };
    assert_eq!(
        job.plan["volumes"], false,
        "the reaper's entry point retains the original choice"
    );
    tx.commit().await.unwrap();
    world.engine().kick();
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(runtime_state(&world, runtime).await, "removed");
    assert_eq!(host.count(K::NodeRuntimeRemove), 2);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
}

#[tokio::test]
async fn removing_an_older_kept_provider_unit_does_not_detach_or_stop_its_current_unit() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "kept-removal", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Current and kept").await;
    let (old, _old_unit) = made(&world, &host, provider).await;
    sqlx::query("UPDATE runtimes SET state='retired',remove_at=NULL WHERE id=$1")
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET machine_id=NULL WHERE id=$1")
        .bind(provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let (current, current_unit) = made(&world, &host, provider).await;
    let before = host.count(K::NodeRuntimeStop);
    remove_from_host(&world, host.id, old, true).await.unwrap();
    eventually("only the kept unit to be removed", || async {
        runtime_state(&world, old).await == "removed"
    })
    .await;
    assert_eq!(machine_of(&world, provider).await, Some(current_unit.id));
    assert_eq!(runtime_state(&world, current).await, "running");
    let stops = host
        .calls()
        .into_iter()
        .filter(|call| call.kind == K::NodeRuntimeStop)
        .collect::<Vec<_>>();
    assert_eq!(stops.len(), before + 1);
    assert_eq!(stops.last().unwrap().body["runtime_id"], json!(old));
    assert_eq!(
        host.count(K::NodeVolumeRemove),
        0,
        "the current unit still owns the shared state volume"
    );
}

#[tokio::test]
async fn renewing_a_running_unit_neither_starts_stops_nor_recreates_it_even_when_verification_fails()
 {
    for valid_roles in [true, false] {
        let world = World::new().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "renewal-host", 16_384).await;
        let template = provider_template(&world).await;
        let runtime = managed_world_provider(&world, &host, template).await;
        world
            .set(world.root, "runtime.update", json!("on_start"))
            .await;
        if !valid_roles {
            sqlx::query("UPDATE machines SET roles = ARRAY['provider'] WHERE id = $1")
                .bind(world.machine_id)
                .execute(world.pool.pool())
                .await
                .unwrap();
        }
        world.machine.handler.on(K::ProviderRefresh, |_| {
            Ok(
                orch_link_contract::commands::CommandResult::ProviderRefresh(
                    orch_link_contract::commands::ProviderRefreshResult {
                        source: None,
                        status: "ready".into(),
                        facts: serde_json::from_value(
                            json!({"credential":{"yields":"login","expires_at":null}}),
                        )
                        .unwrap(),
                    },
                ),
            )
        });
        let RuntimeStart::Job(job) = world
            .engine()
            .renew_provider_runtime(world.provider)
            .await
            .unwrap()
        else {
            panic!("a running unit still needs a renewal job")
        };
        let (state, _, error) = wait_over(&world, job.id).await;
        assert_eq!(
            state,
            if valid_roles { "succeeded" } else { "failed" },
            "{error:?}"
        );
        assert_eq!(runtime_state(&world, runtime).await, "running");
        assert_eq!(
            machine_of(&world, world.provider).await,
            Some(world.machine_id)
        );
        for kind in [
            K::NodeRuntimeStart,
            K::NodeRuntimeStop,
            K::NodeRuntimeRemove,
            K::NodeRuntimeCreate,
        ] {
            assert_eq!(
                host.count(kind),
                0,
                "renewal must leave the unit in place: {kind:?}"
            );
        }
        let notices: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'",
        )
        .bind(world.ada)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
        assert_eq!(
            notices,
            i64::from(!valid_roles),
            "failed renewal tells a current manager"
        );
    }
}

#[tokio::test]
async fn a_providers_runtime_is_made_with_the_phases_of_a_provider_one_volume_and_the_roles_of_a_runtime()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Codex").await;
    let job = queue(&world, provider, Mode::Make).await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    assert!(
        machine_of(&world, provider).await.is_none(),
        "until the unit is up the provider has no machine"
    );
    let unit = enrol(&world, runtime).await;
    let (state, progress, error) = wait_over(&world, job).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(
        host.kinds(),
        [
            K::NodeImageEnsure,
            K::NodeVolumeEnsure,
            K::NodeToolEnsure,
            K::NodeToolEnsure,
            K::NodeRuntimeCreate,
            K::NodeRuntimeStart,
        ],
        "the image, the one volume, the connector and the harness, and the unit"
    );
    let listed = phases(&progress);
    assert_eq!(
        listed
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        [
            "schedule", "image", "volumes", "tools", "create", "enroll", "after_up", "verify",
            "commit"
        ]
    );
    assert!(
        listed.iter().all(|(name, state)| state
            == if name == "after_up" {
                "skipped"
            } else {
                "done"
            }),
        "{listed:?}"
    );
    assert_eq!(progress["percent"], 100);
    let calls = host.calls();
    let create = calls
        .iter()
        .find(|call| call.kind == K::NodeRuntimeCreate)
        .unwrap();
    assert_eq!(create.body["kind"], "provider");
    assert_eq!(create.body["roles"], json!(["provider", "agent"]));
    assert_eq!(create.body["mounts"], json!([]));
    assert_eq!(
        create.body["volumes"],
        json!({ "state": { "name": format!("orch-state-{provider}"), "storage": null } }),
        "a provider's runtime keeps its state on a volume that is made with the provider"
    );
    assert_eq!(create.body["labels"]["orch.kind"], "provider");
    assert_eq!(create.body["labels"]["orch.provider_id"], json!(provider));
    assert!(
        create.body["name"]
            .as_str()
            .unwrap()
            .starts_with("orch-provider-")
    );
    assert_eq!(
        create.body["tools"],
        json!([{ "name": "fake", "version": "1.0.0" }]),
        "the build of the harness the provider is a login of, and not the connector"
    );
    assert_eq!(machine_of(&world, provider).await, Some(unit.id));
    assert_eq!(runtime_state(&world, runtime).await, "running");
    let leftover: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_id = $1 AND name = 'enroll_token'",
    )
    .bind(runtime)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(leftover, 0, "the token was single use and is gone");
}

#[tokio::test]
async fn a_runtime_no_host_can_take_ends_its_job_failed_and_a_sign_in_that_waited_is_forgotten_with_its_secrets()
 {
    let world = World::new().await;
    serve(&world);
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Codex").await;
    // A sign-in waits for the runtime, with its secret in a row of its own.
    {
        let mut conn = world.pool.acquire().await.unwrap();
        common::put_secret(&mut conn, provider, "login:token", "sk-never-to-be-kept").await;
        rows::keep_pending(
            &mut conn,
            provider,
            br#"{"method":"setup_token","values":{},"by":null}"#,
            Utc::now(),
        )
        .await
        .unwrap();
    }
    let job = queue(&world, provider, Mode::Make).await;
    let (state, _, error) = wait_over(&world, job).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["code"], "no_host", "{error}");
    assert_eq!(error["phase"], "schedule");
    let (pending, secrets, detail): (Option<Vec<u8>>, i64, Option<String>) = sqlx::query_as(
        "SELECT login_pending,
                (SELECT count(*) FROM secrets WHERE owner_id = $1),
                status_detail FROM providers WHERE id = $1",
    )
    .bind(provider)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert!(
        pending.is_none(),
        "what waited for the runtime is forgotten"
    );
    assert_eq!(secrets, 0, "and so are the secrets in it");
    let detail = detail.unwrap();
    assert!(detail.contains("could not be made"), "{detail}");
    let tables = world.tables_holding("sk-never-to-be-kept").await;
    assert!(tables.is_empty(), "the secret is in no table: {tables:?}");
}

#[tokio::test]
async fn a_unit_the_host_cannot_make_is_undone_and_the_volume_it_made_is_removed_with_it() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Codex").await;
    host.refuse(
        K::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Internal, "out of memory"),
    );
    let job = queue(&world, provider, Mode::Make).await;
    let (state, _, error) = wait_over(&world, job).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "create");
    assert!(
        error["message"].as_str().unwrap().contains("out of memory"),
        "{error}"
    );
    assert_eq!(error["undone"], true);
    assert_eq!(
        host.kinds()[host.kinds().len() - 2..],
        [K::NodeRuntimeRemove, K::NodeVolumeRemove],
        "the unit that was not made is removed, and the volume that was first made with it"
    );
    assert!(machine_of(&world, provider).await.is_none());
}

#[tokio::test]
async fn a_runtime_is_stopped_by_a_person_with_its_volume_kept_and_started_again_by_a_job_of_four_phases()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Codex").await;
    let (runtime, unit) = made(&world, &host, provider).await;
    // A harness report's foreign key pins the provider's identity, not its current machine.
    // Stopping must not wait for that pin while the report can be waiting on a harness row.
    let mut reference = world.pool.begin().await.unwrap();
    let pinned: Id = sqlx::query_scalar("SELECT id FROM providers WHERE id=$1 FOR KEY SHARE")
        .bind(provider)
        .fetch_one(&mut *reference)
        .await
        .unwrap();
    assert_eq!(pinned, provider);
    tokio::time::timeout(
        common::PATIENCE,
        world
            .engine()
            .stop_provider_runtime(provider, false, StopWhy::Person(world.ada)),
    )
    .await
    .expect("a harness identity pin must not block the acknowledged runtime stop")
    .unwrap();
    reference.rollback().await.unwrap();
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0, "the unit is kept");
    assert_eq!(runtime_state(&world, runtime).await, "stopped");
    assert!(
        machine_of(&world, provider).await.is_none(),
        "a provider whose unit is stopped has no machine, which is how a turn knows to start it"
    );
    // Stopping what is stopped is the same stop.
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    let started = world
        .engine()
        .start_provider_runtime(provider, Some(world.ada))
        .await
        .unwrap();
    let RuntimeStart::Job(job) = started else {
        panic!("a stopped runtime is started by a job");
    };
    assert_eq!(job.kind, "provider_provision");
    let (state, progress, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(
        phases(&progress)
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["start", "enroll", "after_up", "verify", "commit"]
    );
    assert_eq!(host.count(K::NodeRuntimeStart), 2);
    assert_eq!(
        host.count(K::NodeRuntimeCreate),
        1,
        "the same unit is started, not made again"
    );
    assert_eq!(runtime_state(&world, runtime).await, "running");
    assert_eq!(machine_of(&world, provider).await, Some(unit.id));
    // A runtime that is up is left as it is.
    assert!(matches!(
        world
            .engine()
            .start_provider_runtime(provider, Some(world.ada))
            .await
            .unwrap(),
        RuntimeStart::Up
    ));
}

#[tokio::test]
async fn a_runtime_with_a_turn_running_on_its_provider_is_not_stopped_unless_the_turn_is_interrupted()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = provider_template(&world).await;
    let runtime = managed_world_provider(&world, &host, template).await;
    let thread = world.thread("work").await;
    let (start, _) = world.running_turn(thread, "go").await;
    let refused = world
        .engine()
        .stop_provider_runtime(world.provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap_err();
    assert!(matches!(refused, RuntimeError::Busy(1)), "{refused:?}");
    assert_eq!(runtime_state(&world, runtime).await, "running");
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    let removal = remove_from_host(&world, host.id, runtime, true)
        .await
        .unwrap_err();
    assert!(matches!(removal, RuntimeError::Busy(1)), "{removal:?}");
    assert_eq!(runtime_state(&world, runtime).await, "running");
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE provider_id=$1 AND kind='environment_remove'",
    )
    .bind(world.provider)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        pending, 0,
        "a refused removal rolls back its queued decision"
    );
    // With the interrupt asked for, the turn is stopped first.
    let engine = world.engine();
    let provider = world.provider;
    let ada = world.ada;
    let stopping = tokio::spawn(async move {
        engine
            .stop_provider_runtime(provider, true, StopWhy::Person(ada))
            .await
    });
    eventually("the harness to be asked to interrupt", || async {
        world.ran(K::TurnInterrupt) == 1
    })
    .await;
    world
        .say(
            thread,
            orch_link_contract::events::Event::TurnCompleted(
                orch_link_contract::events::TurnCompleted {
                    turn_id: start.turn_id,
                    status: orch_link_contract::events::TurnOutcome::Interrupted,
                    error: None,
                },
            ),
        )
        .await;
    stopping.await.unwrap().unwrap();
    assert_eq!(runtime_state(&world, runtime).await, "stopped");
}

#[tokio::test]
async fn a_runtime_that_has_been_idle_for_as_long_as_its_setting_allows_is_stopped_and_one_in_use_is_not()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    world
        .set(world.root, "runtime.idle_stop_minutes", json!(0))
        .await;
    let idle = managed_subscription(&world, "Idle").await;
    let (idle_runtime, _unit) = made(&world, &host, idle).await;
    let waiting = managed_subscription(&world, "Waiting for a person").await;
    let (waiting_runtime, _other) = made(&world, &host, waiting).await;
    eventually("both providers' harnesses to be online", || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(DISTINCT provider_id) FROM harnesses WHERE provider_id = ANY($1) AND status = 'online'",
        )
        .bind([idle, waiting])
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
            == 2
    })
    .await;
    // Both were last active three hours ago; one has a sign-in that waits for the person.
    sqlx::query(
        "UPDATE runtimes SET created_at = $2, last_active_at = $2, updated_at = $2 WHERE id = ANY($1)",
    )
    .bind([idle_runtime, waiting_runtime])
    .bind(Utc::now() - Duration::hours(3))
    .execute(world.pool.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE harnesses SET last_active_at = $2 WHERE provider_id = ANY($1)")
        .bind([idle, waiting])
        .bind(Utc::now() - Duration::hours(3))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE providers SET login_state = '{\"state\":\"awaiting_user\",\"method\":\"code\"}' WHERE id = $1",
    )
    .bind(waiting)
    .execute(world.pool.pool())
    .await
    .unwrap();
    meanwhile(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        runtime_state(&world, idle_runtime).await,
        "running",
        "a timeout of 0 never stops"
    );
    world
        .set(world.root, "runtime.idle_stop_minutes", json!(60))
        .await;
    eventually("the idle runtime to be stopped", || async {
        runtime_state(&world, idle_runtime).await == "stopped"
    })
    .await;
    assert!(machine_of(&world, idle).await.is_none());
    meanwhile(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        runtime_state(&world, waiting_runtime).await,
        "running",
        "a sign-in that waits for the person keeps the runtime up"
    );
}

#[tokio::test]
async fn a_native_background_task_keeps_an_idle_provider_runtime_busy_without_an_open_lease() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "background-runtime", 16_384).await;
    let template = provider_template(&world).await;
    let runtime = managed_world_provider(&world, &host, template).await;
    let idle = managed_subscription(&world, "Idle control").await;
    let (idle_runtime, _unit) = made(&world, &host, idle).await;
    let thread = world.thread("Retained native work").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let mut task = common::background::command(&world, thread, first.turn_id).await;
    world
        .say(
            thread,
            common::turn_ended(
                first.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.wait_turn(first.turn_id, "completed").await;
    world.wait_followed_up(thread).await;
    let leases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM leases WHERE provider_id=$1 AND closed_at IS NULL",
    )
    .bind(world.provider)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        leases, 0,
        "this control exercises native task busy state independently of lease retention"
    );
    eventually("both provider harnesses to finish startup",||async {
        sqlx::query_scalar::<_,i64>("SELECT count(DISTINCT provider_id) FROM harnesses WHERE provider_id=ANY($1) AND status='online'")
            .bind([world.provider,idle]).fetch_one(world.pool.pool()).await.unwrap()==2
    }).await;
    let old = Utc::now() - Duration::hours(3);
    sqlx::query(
        "UPDATE runtimes SET created_at=$2,last_active_at=$2,updated_at=$2 WHERE id=ANY($1)",
    )
    .bind([runtime, idle_runtime])
    .bind(old)
    .execute(world.pool.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE turns SET created_at=$2,started_at=$2,completed_at=$2 WHERE thread_id=$1")
        .bind(thread)
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE harnesses SET last_active_at=$2 WHERE provider_id=ANY($1)")
        .bind([world.provider, idle])
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(world.root, "runtime.idle_stop_minutes", json!(60))
        .await;
    eventually("the idle provider control to stop", || async {
        runtime_state(&world, idle_runtime).await == "stopped"
    })
    .await;
    // Four ordinary 150ms reaper intervals, matching the existing lifecycle controls.
    meanwhile(std::time::Duration::from_millis(600)).await;
    assert_eq!(runtime_state(&world, runtime).await, "running");
    task.state = orch_link_contract::background::BackgroundTaskState::Stopped;
    common::background::update(&world, thread, task).await;
    sqlx::query("UPDATE harnesses SET last_active_at=$2 WHERE provider_id=$1")
        .bind(world.provider)
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the provider becomes idle after its task ends", || async {
        runtime_state(&world, runtime).await == "stopped"
    })
    .await;
}

#[tokio::test]
async fn a_deleted_providers_runtime_is_retired_and_the_reaper_removes_its_unit_and_its_state_volume()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Gone soon").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    let mut tx = world.pool.begin().await.unwrap();
    let retired = orch_engine::provider_runtime::retire_runtime(&mut tx, provider, Utc::now())
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET deleted_at = now(), status = 'disabled' WHERE id = $1")
        .bind(provider)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(retired, Some(runtime));
    assert!(machine_of(&world, provider).await.is_none());
    eventually("the unit and its volume to be removed", || async {
        runtime_state(&world, runtime).await == "removed"
    })
    .await;
    let kinds = host.kinds();
    let tail = &kinds[kinds.len() - 3..];
    assert_eq!(
        tail,
        [
            K::NodeRuntimeStop,
            K::NodeRuntimeRemove,
            K::NodeVolumeRemove
        ],
        "the unit is stopped and removed, and then the volume the sign-in was kept on"
    );
    let removed = host
        .calls()
        .into_iter()
        .rev()
        .find(|call| call.kind == K::NodeVolumeRemove)
        .unwrap();
    assert_eq!(removed.body["name"], format!("orch-state-{provider}"));
}

#[tokio::test]
async fn a_unit_that_its_host_finds_gone_leaves_the_provider_without_a_machine_and_the_next_start_makes_it_again()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Crashes").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    // This control exercises an explicit start; the separate scheduled-recovery case makes reads due.
    sqlx::query("UPDATE providers SET facts_requested_at=$2 WHERE id=$1")
        .bind(provider)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    host.tell(runtime, "absent", Some("the unit is gone")).await;
    eventually("the runtime to be marked failed", || async {
        runtime_state(&world, runtime).await == "failed"
    })
    .await;
    assert!(machine_of(&world, provider).await.is_none());
    let RuntimeStart::Job(job) = world
        .engine()
        .start_provider_runtime(provider, Some(world.ada))
        .await
        .unwrap()
    else {
        panic!("a failed runtime is made again by a job");
    };
    host.wait_for(K::NodeRuntimeStart, 2).await;
    let second = host.started().unwrap();
    assert_ne!(second, runtime, "a new unit is made on the same volume");
    let _unit = enrol(&world, second).await;
    let (state, progress, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(phases(&progress)[0].0, "schedule");
    assert_eq!(
        host.count(K::NodeVolumeEnsure),
        2,
        "the volume is ensured again and not made over"
    );
    assert_eq!(runtime_state(&world, runtime).await, "retired");
    assert_eq!(
        host.count(K::NodeVolumeRemove),
        0,
        "a failed unit's volume is the new unit's"
    );
}

#[tokio::test]
async fn a_declared_directory_runtime_attaches_and_detaches_without_starting_or_stopping_its_machine()
 {
    let world = World::new().await;
    serve(&world);
    let unread = world
        .engine()
        .start_provider_runtime(world.provider, Some(world.ada))
        .await
        .unwrap_err();
    assert!(
        matches!(unread,RuntimeError::Unavailable(ref detail) if detail.contains("reported usable provider runtime services")),
        "{unread:?}"
    );
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET facts=facts || $2 WHERE id=$1")
        .bind(world.machine_id)
        .bind(json!({"provider_runtime_descriptor":descriptor}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let started = world
        .engine()
        .start_provider_runtime(world.provider, Some(world.ada))
        .await
        .unwrap();
    assert!(matches!(started, RuntimeStart::Up));
    world.refuse(
        K::AgentStop,
        vec![CommandError::new(
            ErrorCode::Internal,
            "the harness could not stop",
        )],
    );
    let refused = world
        .engine()
        .stop_provider_runtime(world.provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap_err();
    assert!(matches!(refused, RuntimeError::Host(_)), "{refused:?}");
    let harness_status = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3",
        )
        .bind(world.provider)
        .bind(world.agent)
        .bind(world.machine_id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
    };
    assert_eq!(
        harness_status().await,
        "online",
        "an unconfirmed stop stays online"
    );
    world
        .engine()
        .stop_provider_runtime(world.provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
    assert_eq!(
        harness_status().await,
        "offline",
        "the confirmed retry stops this harness"
    );
    assert_eq!(world.began(K::AgentStop), 2);
    assert!(
        world
            .ordered()
            .iter()
            .any(|(_, command)| command.kind() == K::AgentStop)
    );
    assert!(world.ordered().iter().all(|(_, command)| !matches!(
        command.kind(),
        K::NodeRuntimeStart | K::NodeRuntimeStop | K::NodeRuntimeCreate
    )));
    let none = world
        .engine()
        .start_provider_runtime(new_id(), None)
        .await
        .unwrap_err();
    assert!(matches!(none, RuntimeError::NotFound), "{none:?}");
}

/// Marks a runtime `stopping` as the sweep of a disabled resource does, without the host having said
/// anything yet.
async fn begin_stopping(world: &World, runtime: Id, why: &str) {
    sqlx::query("UPDATE runtimes SET state = 'stopping', state_detail = $2 WHERE id = $1")
        .bind(runtime)
        .bind(why)
        .execute(world.pool.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_unit_whose_container_exited_while_it_was_stopping_reads_as_stopped_and_is_started_as_it_is()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-exited", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Exited while stopping").await;
    let (runtime, unit) = made(&world, &host, provider).await;
    begin_stopping(&world, runtime, rows::STOPPED_PERSON).await;
    host.tell(runtime, "stopped", Some("the unit stopped by itself"))
        .await;
    eventually(
        "the unit the host finds exited to read as stopped",
        || async { runtime_state(&world, runtime).await == "stopped" },
    )
    .await;
    let detail: Option<String> =
        sqlx::query_scalar("SELECT state_detail FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        detail.as_deref(),
        Some(rows::STOPPED_PERSON),
        "a person's stop stays a person's stop, so that nothing starts it by itself"
    );
    // The same unit is started again, not a new one made.
    let RuntimeStart::Job(job) = world
        .engine()
        .start_provider_runtime(provider, Some(world.ada))
        .await
        .unwrap()
    else {
        panic!("a stopped runtime is started by a job");
    };
    let (state, progress, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(
        phases(&progress)
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["start", "enroll", "after_up", "verify", "commit"]
    );
    assert_eq!(host.count(K::NodeRuntimeCreate), 1);
    assert_eq!(runtime_state(&world, runtime).await, "running");
    assert_eq!(machine_of(&world, provider).await, Some(unit.id));
}

#[tokio::test]
async fn a_unit_the_host_no_longer_has_while_it_was_stopping_reads_as_stopped_and_the_next_start_makes_a_new_one()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-deleted", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Deleted while stopping").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    begin_stopping(&world, runtime, "Its resource was disabled").await;
    host.tell(runtime, "absent", Some("the unit is gone")).await;
    eventually(
        "the unit the host does not have to read as stopped",
        || async { runtime_state(&world, runtime).await == "stopped" },
    )
    .await;
    assert!(machine_of(&world, provider).await.is_none());
    let RuntimeStart::Job(job) = world
        .engine()
        .start_provider_runtime(provider, Some(world.ada))
        .await
        .unwrap()
    else {
        panic!("a stopped runtime is started by a job");
    };
    host.wait_for(K::NodeRuntimeStart, 2).await;
    let second = host.started().unwrap();
    assert_ne!(
        second, runtime,
        "a unit the host does not have is made again, not started"
    );
    let _unit = enrol(&world, second).await;
    let (state, progress, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(phases(&progress)[0].0, "schedule");
    assert_eq!(runtime_state(&world, runtime).await, "retired");
    assert_eq!(runtime_state(&world, second).await, "running");
    // Each unit is told which runtime it is of, so that the connector in the new unit does not take the
    // machine of the old one, which was revoked, for its own.
    let creates: Vec<_> = host
        .calls()
        .into_iter()
        .filter(|call| call.kind == K::NodeRuntimeCreate)
        .collect();
    assert_eq!(creates.len(), 2);
    assert_eq!(creates[0].body["env"]["ORCH_RUNTIME_ID"], json!(runtime));
    assert_eq!(creates[1].body["env"]["ORCH_RUNTIME_ID"], json!(second));
}

#[tokio::test]
async fn a_stop_the_host_answers_with_no_such_unit_settles_as_stopped_and_does_not_put_the_unit_back_to_running()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-no-unit", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Container deleted by hand").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    host.refuse(
        K::NodeRuntimeStop,
        CommandError::new(ErrorCode::NotFound, "this host has no unit for the runtime"),
    );
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
    assert_eq!(runtime_state(&world, runtime).await, "stopped");
    assert!(
        machine_of(&world, provider).await.is_none(),
        "a provider whose unit is not there has no machine"
    );
    let missing: bool = sqlx::query_scalar("SELECT loss_missing FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert!(
        missing,
        "the host's word that it has no such unit is written down, so that the next start makes one"
    );
}

#[tokio::test]
async fn a_forced_stop_settles_a_unit_whose_host_says_nothing_and_the_host_is_told_when_it_can_be()
{
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-silent", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "The host says nothing").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    tokio::time::timeout(
        common::PATIENCE,
        world.engine().stop_provider_runtime_with(
            provider,
            false,
            StopWhy::Person(world.ada),
            true,
        ),
    )
    .await
    .expect("a forced stop does not wait for the host")
    .unwrap();
    assert_eq!(runtime_state(&world, runtime).await, "stopped");
    assert!(
        machine_of(&world, provider).await.is_none(),
        "a provider whose unit is stopped has no machine"
    );
    let detail: Option<String> =
        sqlx::query_scalar("SELECT state_detail FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        detail.as_deref(),
        Some(rows::STOPPED_FORCED),
        "a stop by force that the host has not confirmed says so, and nothing starts the unit by itself"
    );
    host.added.machine.handler.release();
    eventually("the host to be told to stop the unit", || async {
        host.calls().iter().any(|call| {
            call.kind == K::NodeRuntimeStop && call.body["runtime_id"] == json!(runtime)
        })
    })
    .await;
    eventually(
        "the host's answer to turn the reason into the plain one of a person's stop",
        || async { detail_of(&world, runtime).await.as_deref() == Some(rows::STOPPED_PERSON) },
    )
    .await;
}

#[tokio::test]
async fn a_job_that_makes_a_unit_is_named_when_it_stops_a_stop_and_is_ended_by_a_forced_one() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-waiting", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Waiting for its connector").await;
    // The unit is made and started, and its connector never comes online: the job waits for it.
    let job = queue(&world, provider, Mode::Make).await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    eventually("the job to wait for the connector in the unit", || async {
        sqlx::query_scalar::<_, String>("SELECT progress->>'phase' FROM jobs WHERE id=$1")
            .bind(job)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
            == "enroll"
    })
    .await;
    let refused = world
        .engine()
        .stop_provider_runtime_with(provider, false, StopWhy::Person(world.ada), false)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, RuntimeError::Held(hold) if hold.why == HoldWhy::OwnWork
            && hold.work == Some("make")
            && hold.step.as_deref() == Some("enroll")),
        "the reason names the work and where it is: {refused:?}"
    );
    world
        .engine()
        .stop_provider_runtime_with(provider, false, StopWhy::Person(world.ada), true)
        .await
        .unwrap();
    let (state, _, _) = wait_over(&world, job).await;
    assert_eq!(state, "cancelled", "the job a person overruled is over");
    assert_eq!(runtime_state(&world, runtime).await, "stopped");
}

#[tokio::test]
async fn a_host_manager_forces_the_removal_of_a_unit_a_job_works_on_and_the_choice_of_volumes_stays()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-forced-removal", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Forced removal").await;
    let job = queue(&world, provider, Mode::Make).await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    eventually("the job to wait for the connector in the unit", || async {
        sqlx::query_scalar::<_, String>("SELECT progress->>'phase' FROM jobs WHERE id=$1")
            .bind(job)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
            == "enroll"
    })
    .await;
    let refused = remove_from_host_with(&world, host.id, runtime, false, false)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, RuntimeError::Held(hold) if hold.why == HoldWhy::OwnWork && hold.work == Some("make")),
        "the reason names the work: {refused:?}"
    );
    assert_eq!(
        runtime_state(&world, runtime).await,
        "creating",
        "a refused removal changes nothing"
    );
    remove_from_host_with(&world, host.id, runtime, false, true)
        .await
        .unwrap();
    eventually("the unit to be removed", || async {
        runtime_state(&world, runtime).await == "removed"
    })
    .await;
    let (state, _, _) = wait_over(&world, job).await;
    assert_eq!(state, "cancelled");
    assert_eq!(
        host.count(K::NodeVolumeRemove),
        0,
        "the volumes were kept, as the manager chose"
    );
}

/// Stops a unit as a host manager does, in a transaction of its own.
async fn stop_from_host(
    world: &World,
    host: Id,
    runtime: Id,
    interrupt: bool,
    force: bool,
) -> Result<(), RuntimeError> {
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    world
        .engine()
        .stop_host_provider_unit_in(
            tx,
            &principal,
            &orch_db::audit::Origin::default(),
            host,
            runtime,
            interrupt,
            force,
        )
        .await
}

async fn job_state(world: &World, job: Id) -> String {
    sqlx::query_scalar("SELECT state FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

async fn detail_of(world: &World, runtime: Id) -> Option<String> {
    sqlx::query_scalar("SELECT state_detail FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_forced_stop_or_removal_that_is_refused_for_a_turn_that_runs_ends_no_job() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-refused", 16_384).await;
    let template = provider_template(&world).await;
    let runtime = managed_world_provider(&world, &host, template).await;
    let thread = world.thread("work").await;
    let (_start, _) = world.running_turn(thread, "go").await;
    // A job works on the unit, and the host does not answer what it asked.
    host.added.machine.handler.hold(K::NodeRuntimeStart);
    let job = queue(&world, world.provider, Mode::Start).await;
    host.wait_began(K::NodeRuntimeStart, 1).await;
    let stop = stop_from_host(&world, host.id, runtime, false, true)
        .await
        .unwrap_err();
    assert!(matches!(stop, RuntimeError::Busy(1)), "{stop:?}");
    let removal = remove_from_host_with(&world, host.id, runtime, false, true)
        .await
        .unwrap_err();
    assert!(matches!(removal, RuntimeError::Busy(1)), "{removal:?}");
    assert!(
        matches!(job_state(&world, job).await.as_str(), "queued" | "running"),
        "a person who was refused ended nothing: {}",
        job_state(&world, job).await
    );
    assert_eq!(runtime_state(&world, runtime).await, "running");
    host.added.machine.handler.release();
}

#[tokio::test]
async fn a_host_that_refuses_the_stop_a_person_forced_is_told_again_until_it_answers() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-refusing", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "The host refuses once").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    host.refuse(
        K::NodeRuntimeStop,
        CommandError::new(ErrorCode::Conflict, "the host cannot stop it just now"),
    );
    world
        .engine()
        .stop_provider_runtime_with(provider, false, StopWhy::Person(world.ada), true)
        .await
        .unwrap();
    assert_eq!(runtime_state(&world, runtime).await, "stopped");
    host.wait_for(K::NodeRuntimeStop, 2).await;
    eventually(
        "the host to have taken the stop, which the record says by the plain reason of a person's stop",
        || async { detail_of(&world, runtime).await.as_deref() == Some(rows::STOPPED_PERSON) },
    )
    .await;
}

#[tokio::test]
async fn a_unit_stopped_by_force_that_its_host_then_reports_stopped_is_not_told_again() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-reports", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "The host says it stopped").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    world
        .engine()
        .stop_provider_runtime_with(provider, false, StopWhy::Person(world.ada), true)
        .await
        .unwrap();
    assert_eq!(
        detail_of(&world, runtime).await.as_deref(),
        Some(rows::STOPPED_FORCED)
    );
    // The host does not answer the command, and says by itself that the unit stopped.
    host.tell(runtime, "stopped", Some("the unit stopped by itself"))
        .await;
    eventually(
        "the host's report to turn the reason into the plain one of a person's stop",
        || async { detail_of(&world, runtime).await.as_deref() == Some(rows::STOPPED_PERSON) },
    )
    .await;
    assert_eq!(runtime_state(&world, runtime).await, "stopped");
    host.added.machine.handler.release();
}

#[tokio::test]
async fn a_forced_stop_of_a_unit_the_record_calls_stopped_tells_its_host_all_the_same() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-ghost", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "A ghost the record does not know").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    sqlx::query("UPDATE runtimes SET state = 'stopped', state_detail = $2 WHERE id = $1")
        .bind(runtime)
        .bind(rows::STOPPED_PERSON)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let before = host.count(K::NodeRuntimeStop);
    world
        .engine()
        .stop_provider_runtime_with(provider, false, StopWhy::Person(world.ada), true)
        .await
        .unwrap();
    host.wait_for(K::NodeRuntimeStop, before + 1).await;
    assert_eq!(runtime_state(&world, runtime).await, "stopped");
}

#[tokio::test]
async fn a_unit_shown_as_being_made_that_no_job_makes_is_settled_by_what_its_host_reports() {
    for told in ["running", "stopped", "absent"] {
        let world = World::new().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "host-half-made", 16_384).await;
        let _template = provider_template(&world).await;
        let provider = managed_subscription(&world, "Left half made").await;
        let (runtime, _unit) = made(&world, &host, provider).await;
        // The job that was making it is over and the unit still reads as being made.
        sqlx::query("UPDATE runtimes SET state = 'creating' WHERE id = $1")
            .bind(runtime)
            .execute(world.pool.pool())
            .await
            .unwrap();
        host.tell(runtime, told, Some("what the host found of it"))
            .await;
        eventually(
            &format!("a unit that its host tells as {told} to stop reading as being made"),
            || async { runtime_state(&world, runtime).await != "creating" },
        )
        .await;
        assert_eq!(runtime_state(&world, runtime).await, "failed", "{told}");
        assert!(
            detail_of(&world, runtime).await.is_some(),
            "the reason is kept with the unit: {told}"
        );
    }
}

#[tokio::test]
async fn a_stop_the_host_never_took_is_asked_again_with_a_new_command_and_not_with_the_one_that_expired()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-expired", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "A stop that expired").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    // The stop was begun long ago and its command ran out of time before the host took it.
    let long_ago = world.clock.now() - Duration::minutes(10);
    sqlx::query(
        "UPDATE runtimes SET state = 'stopping', state_detail = $2, updated_at = $3 WHERE id = $1",
    )
    .bind(runtime)
    .bind(rows::STOPPED_PERSON)
    .bind(long_ago)
    .execute(world.pool.pool())
    .await
    .unwrap();
    let updated: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM runtimes WHERE id = $1")
            .bind(runtime)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let expired = orch_engine::ids::derived(
        "node.runtime.stop",
        &[
            runtime.as_bytes().as_slice(),
            &updated.timestamp_micros().to_le_bytes(),
        ],
    );
    sqlx::query(
        "INSERT INTO commands (id, machine_id, kind, body, state, deadline_at, created_at, updated_at)
         VALUES ($1, $2, 'node.runtime.stop', $3, 'expired', $4, $5, $4)",
    )
    .bind(expired)
    .bind(host.machine)
    .bind(json!({ "runtime_id": runtime }))
    .bind(world.clock.now() - Duration::minutes(8))
    .bind(long_ago)
    .execute(world.pool.pool())
    .await
    .unwrap();
    // The schedule asks, finds its command run out, and dates the unit again, so that the next asking is another command.
    let updated_at = || async {
        sqlx::query_scalar::<_, chrono::DateTime<Utc>>(
            "SELECT updated_at FROM runtimes WHERE id = $1",
        )
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
    };
    eventually(
        "the stop that ran out to be found, and the unit to be dated again",
        || async { updated_at().await > updated },
    )
    .await;
    // By the schedule's measure the stop is old again; this time it is a command of its own, and the host takes it.
    sqlx::query("UPDATE runtimes SET updated_at = $2 WHERE id = $1 AND state = 'stopping'")
        .bind(runtime)
        .bind(long_ago - Duration::minutes(1))
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually(
        "the stop to be asked again, and the host's answer to settle the unit",
        || async { runtime_state(&world, runtime).await == "stopped" },
    )
    .await;
}

/// What the engine writes to its log while a test runs, to be read back.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Captured {
    /// The lines written so far.
    fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }

    /// Whether a line says all of `words`.
    fn says(&self, words: &[&str]) -> bool {
        self.lines()
            .iter()
            .any(|line| words.iter().all(|word| line.contains(word)))
    }
}

/// Writes the log of this test thread, where the engine's tasks run too, into what [`Captured`] reads back.
fn capture_log() -> (Captured, tracing::subscriber::DefaultGuard) {
    let captured = Captured::default();
    let guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::INFO)
            .with_writer(captured.clone())
            .finish(),
    );
    (captured, guard)
}

#[tokio::test]
async fn every_step_of_making_a_unit_is_in_the_log_with_the_provider_and_the_unit_and_a_failure_with_its_reason()
 {
    let (log, _guard) = capture_log();
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-logged", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "A unit that is logged").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    let (provider_is, runtime_is) = (format!("provider={provider}"), format!("runtime={runtime}"));
    // Each step of making it begins and ends in the log, and the step that makes the unit says which unit.
    for phase in ["schedule", "image", "create", "enroll", "commit"] {
        let at = format!("phase=\"{phase}\"");
        assert!(
            log.says(&["a step begins", &provider_is, &at]),
            "the beginning of {phase} is logged with the provider"
        );
        // A step that has nothing to do may be skipped, which is logged as that.
        if phase != "image" {
            assert!(
                log.says(&["a step is done", &provider_is, &runtime_is, &at]),
                "the end of {phase} is logged with the provider and the unit"
            );
        }
    }
    // A unit that cannot be made says why, with the step it failed at.
    let failing = managed_subscription(&world, "A unit that cannot be made").await;
    host.refuse(
        K::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Conflict, "there is no room for the unit"),
    );
    let job = queue(&world, failing, Mode::Make).await;
    let (state, _, _) = wait_over(&world, job).await;
    assert_eq!(state, "failed");
    assert!(
        log.says(&[
            "a step failed",
            &format!("provider={failing}"),
            "phase=\"create\"",
            "there is no room for the unit",
        ]),
        "the failure is logged with the provider, the step and the host's reason: {:?}",
        log.lines()
    );
}

#[tokio::test]
async fn stopping_and_removing_a_unit_are_in_the_log_with_the_provider_the_unit_and_the_reason_of_a_refusal()
 {
    let (log, _guard) = capture_log();
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-stopping", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "A unit that is stopped").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    let (provider_is, runtime_is) = (format!("provider={provider}"), format!("runtime={runtime}"));
    // The host refuses the stop: the log says who refused what and why, and the unit reads as running again.
    host.refuse(
        K::NodeRuntimeStop,
        CommandError::new(ErrorCode::Conflict, "the host cannot stop it just now"),
    );
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap_err();
    assert!(
        log.says(&[
            "refused to stop the unit",
            &provider_is,
            &runtime_is,
            "the host cannot stop it just now",
        ]),
        "{:?}",
        log.lines()
    );
    // The stop that the host takes is logged with the unit it is of.
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
    assert!(log.says(&[
        "the unit of a provider's runtime is stopped",
        &provider_is,
        &runtime_is
    ]));
    // The removal begins and ends each step in the log with the provider and the unit.
    remove_from_host(&world, host.id, runtime, false)
        .await
        .unwrap();
    eventually("the unit to be removed", || async {
        runtime_state(&world, runtime).await == "removed"
    })
    .await;
    for phase in ["stop", "unit", "forget"] {
        let at = format!("phase=\"{phase}\"");
        assert!(
            log.says(&[
                "a step of its removal begins",
                &provider_is,
                &runtime_is,
                &at
            ]),
            "{phase}"
        );
        assert!(
            log.says(&[
                "a step of its removal is done",
                &provider_is,
                &runtime_is,
                &at
            ]),
            "{phase}"
        );
    }
}

#[tokio::test]
async fn a_forced_stop_is_in_the_log_with_the_provider_and_the_unit_and_a_unit_replaced_by_an_update_is_not_stopped_by_force()
 {
    let (log, _guard) = capture_log();
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-forced-logged", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "A unit stopped by force").await;
    let (runtime, _unit) = made(&world, &host, provider).await;
    world
        .engine()
        .stop_provider_runtime_with(provider, false, StopWhy::Person(world.ada), true)
        .await
        .unwrap();
    assert!(
        log.says(&[
            "stopped the unit of a provider's runtime by force",
            &format!("provider={provider}"),
            &format!("runtime={runtime}"),
        ]),
        "{:?}",
        log.lines()
    );
    // A unit that an update replaced waits to be taken away; stopping it by force is refused, with its reason, and
    // the host is told nothing.
    let replaced = managed_subscription(&world, "A unit an update replaced").await;
    let (old, _other) = made(&world, &host, replaced).await;
    sqlx::query(
        "UPDATE runtimes SET state = 'retired', remove_at = now() + interval '1 day' WHERE id = $1",
    )
    .bind(old)
    .execute(world.pool.pool())
    .await
    .unwrap();
    let refused = stop_from_host(&world, host.id, old, false, true)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, RuntimeError::Held(hold) if hold.why == HoldWhy::Replaced),
        "{refused:?}"
    );
    assert!(
        host.calls().iter().all(|call| !(call.kind == K::NodeRuntimeStop
            && call.body["runtime_id"] == json!(old))),
        "the host was told nothing of the unit that waits to be taken away"
    );
}
