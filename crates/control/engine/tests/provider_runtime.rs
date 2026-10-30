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
use orch_engine::provider_runtime::{RuntimeError, RuntimeStart, StopWhy};
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

async fn remove_from_host(
    world: &World,
    host: Id,
    runtime: Id,
    volumes: bool,
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
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
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
