//! A new temporary workspace uses its enrolled runtime identity for source files before publishing running and starting its first input.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::{World, eventually};
use orch_api_contract::source::{SourceDestination, SourceInfo, SourceKind, SourceState};
use orch_authz::{Caller, Principal};
use orch_db::queries::{
    environments::{hosting as envs, temporary},
    settings, threads,
};
use orch_engine::environments::{self, Request};
use orch_link_contract::commands::{Command, CommandKind, CommandResult};
use orch_types::{Id, Time};
use serde_json::json;

struct Pending {
    thread: Id,
    input: Id,
    environment: Id,
    job: Id,
    runtime: Id,
    unit: hosting::Unit,
    _host: hosting::ScriptedHost,
}

async fn pending(world: &World, repository: bool) -> Pending {
    hosting::serve(world);
    let host = hosting::ScriptedHost::on(world, "source-host", 16_384).await;
    // This scripted Docker host creates isolated units. The physical driver declaration, not
    // the enrolled unit's label, must report the confinement that admission later checks.
    let mut conn = world.pool.acquire().await.unwrap();
    let host_policy: serde_json::Value =
        sqlx::query_scalar("SELECT facts->'policy' FROM machines WHERE id=$1")
            .bind(host.machine)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let mut driver: serde_json::Value =
        sqlx::query_scalar("SELECT drivers->'docker' FROM hosts WHERE id=$1")
            .bind(host.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    driver["version"] = json!("0.1.0");
    driver["confinement"] = json!("isolated");
    driver["process_scope"] = json!("all");
    driver["network_egress"] = json!(["open"]);
    driver["recreate"] = json!("replace");
    let driver: orch_driver_contract::descriptor::DriverDescriptor =
        serde_json::from_value(driver).unwrap();
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker}',$2) WHERE id=$1")
        .bind(host.id)
        .bind(json!(driver))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let template = hosting::template(world).await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    // The subscription stays in its actual provider/profile home; no credential copy is issued.
    assert_eq!(
        sqlx::query(
            "UPDATE providers SET key_machine_id=machine_id WHERE id=$1 AND kind='subscription'"
        )
        .bind(world.provider)
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected(),
        1
    );
    if repository {
        settings::put(
            &mut tx,
            Some(world.topic),
            "source.repository_url",
            &settings::Stored {
                value: &json!("https://example.test/team/repository.git"),
                locked: false,
                by: Some(world.ada),
                at: world.clock.now(),
            },
            None,
        )
        .await
        .unwrap();
    }
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let made = environments::create(
        &mut tx,
        world.clock.now(),
        &principal,
        &Request {
            owner: world.ada,
            name: Some("Temporary source"),
            home: world.topic,
            template: Some(template),
            host: Some(Some(host.id)),
            config: &json!({"env.lifetime":"thread"}),
        },
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut tx,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: made.id,
        },
        world.ada,
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    // Use the same atomic creation producer as a new-environment thread request. A raw
    // environment-column swap would retain the old colocated binding and old segment.
    let created = world
        .engine()
        .create_thread_in_transaction(
            &mut tx,
            &Caller::session(world.ada),
            &orch_engine::binding::NewThread {
                topic: world.topic,
                name: "First input on temporary source".into(),
                agent: world.agent,
                provider: world.provider,
                environment: made.id,
                directory: String::new(),
                arrangement: Some(orch_agent_contract::descriptor::ArrangementType::Separated),
                model: Some(common::MODEL.into()),
                effort: None,
            },
            &orch_engine::binding::Birth {
                prepare_source: Some(true),
                ..Default::default()
            },
            orch_types::new_id(),
            world.clock.now(),
        )
        .await
        .unwrap();
    let thread = created.thread;
    assert_eq!(
        created.arrangement.kind,
        orch_agent_contract::descriptor::ArrangementType::Separated
    );
    assert_eq!(created.arrangement.executor.as_deref(), Some("orch-exec"));
    let segment = threads::segment(&mut tx, created.segment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(segment.environment_id, made.id);
    assert_eq!(segment.env_generation, 0);
    assert_eq!(segment.arrangement, "separated");
    assert_eq!(segment.executor.as_deref(), Some("orch-exec"));
    tx.commit().await.unwrap();
    let sent = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            common::queued("Keep this first input"),
        )
        .await
        .unwrap();
    world.engine().kick();
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let gate = if repository {
        CommandKind::GitPrepare
    } else {
        CommandKind::FsStat
    };
    let unit = hosting::enroll_holding(world, runtime, gate).await;
    hosting::physical::existing_thread_directory(world, &unit, made.id, "");
    unit.added
        .machine
        .handler
        .on(CommandKind::GitPrepare, move |command| {
            let Command::GitPrepare(body) = command else {
                panic!("the source command is required");
            };
            assert!(
                matches!(
                    &body.destination,
                    SourceDestination::OwnedEmptyRoot { generation: 1, .. }
                ),
                "the new root belongs to the first runtime generation"
            );
            Ok(CommandResult::GitPrepare(SourceInfo {
                kind: SourceKind::Repository,
                directory: body.directory.clone(),
                repository_url: Some(body.repository_url.clone()),
                branch: Some("main".into()),
                head: Some("prepared-head".into()),
                state: SourceState::Ready,
                job: None,
                reason: None,
            }))
        });
    eventually("the source command before running publication", || async {
        unit.began(gate) > 0
    })
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let physical = orch_db::queries::environment_facts::of(&mut conn, made.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(physical.machine_id, Some(host.machine));
    assert_ne!(
        physical.machine_id,
        Some(unit.id),
        "the unit connector cannot supply its physical host's policy"
    );
    assert_eq!(physical.confinement(), "isolated");
    assert!(physical.makes_units());
    assert_eq!(physical.policy, Some(host_policy));
    drop(conn);
    Pending {
        thread,
        input: sent.input.id,
        environment: made.id,
        job: made.job.id,
        runtime,
        unit,
        _host: host,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_temporary_environment_attaches_prepares_files_then_publishes_one_generation_and_runs_its_first_input()
 {
    for repository in [false, true] {
        let world = World::new().await;
        let pending = pending(&world, repository).await;
        let mut conn = world.pool.acquire().await.unwrap();
        let before = envs::environment(&mut conn, pending.environment)
            .await
            .unwrap()
            .unwrap();
        let runtime = envs::runtime(&mut conn, pending.runtime)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.status, "provisioning");
        assert_eq!(before.generation, 0);
        assert_eq!(before.machine_id, None);
        assert_eq!(runtime.generation, 1);
        assert_eq!(runtime.machine_id, Some(pending.unit.id));
        assert_eq!(
            world.commands.issued_of("turn.start"),
            0,
            "the first input waits for files and running publication"
        );
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND state='pending'",
        )
        .bind(pending.thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(waiting, 1);
        let lifetime = temporary::read(&mut conn, pending.environment)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lifetime.thread_id, Some(pending.thread));
        drop(conn);
        let attach = pending.unit.calls_of(CommandKind::EnvAttach);
        assert_eq!(attach.len(), 1);
        assert_eq!(
            attach[0].body["data_generation"], 1,
            "native files use the generation that will be published"
        );
        let order: Vec<_> = pending
            .unit
            .added
            .machine
            .handler
            .executions()
            .iter()
            .map(|run| run.kind)
            .collect();
        let at = |kind| order.iter().position(|seen| *seen == kind).unwrap();
        assert!(at(CommandKind::EnvAttach) < at(CommandKind::FsStat));
        if repository {
            assert!(at(CommandKind::FsStat) < at(CommandKind::GitPrepare));
        }
        pending.unit.let_go();
        let (state, _, error) = hosting::wait_over(&world, pending.job).await;
        assert_eq!(state, "succeeded", "{error:?}");
        let mut conn = world.pool.acquire().await.unwrap();
        let after = envs::environment(&mut conn, pending.environment)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, "running");
        assert_eq!(after.generation, 1);
        assert_eq!(after.machine_id, Some(pending.unit.id));
        drop(conn);
        let source = world.sync(pending.thread).await.thread.source.unwrap();
        assert_eq!(source.state, SourceState::Ready);
        assert_eq!(
            source.kind,
            if repository {
                SourceKind::Repository
            } else {
                SourceKind::Directory
            }
        );
        assert_eq!(
            source.head.as_deref(),
            if repository {
                Some("prepared-head")
            } else {
                None
            }
        );
        hosting::physical::wait(
            &world,
            pending.environment,
            pending.thread,
            "the published temporary source's original first turn",
            || async { !world.turn_starts(pending.thread).is_empty() },
        )
        .await;
        let start = world.turn_start(pending.thread, 0).await;
        assert_eq!(
            common::words_after_where(&start.text),
            "Keep this first input"
        );
        let input: Id = world
            .scalar("SELECT input_id FROM turns WHERE id=$1", start.turn_id)
            .await;
        assert_eq!(input, pending.input);
        let mut conn = world.pool.acquire().await.unwrap();
        let binding = threads::thread(&mut conn, pending.thread)
            .await
            .unwrap()
            .unwrap();
        let segment = threads::segment(&mut conn, binding.current_segment_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(binding.environment_id, Some(pending.environment));
        assert_eq!(binding.arrangement.as_deref(), Some("separated"));
        assert_eq!(binding.executor.as_deref(), Some("orch-exec"));
        assert_eq!(segment.environment_id, pending.environment);
        assert_eq!(segment.env_generation, 1);
        assert_eq!(segment.arrangement, "separated");
        assert_eq!(segment.executor.as_deref(), Some("orch-exec"));
        let opened: Vec<_> = world
            .ordered()
            .into_iter()
            .filter_map(|(machine, command)| match command {
                Command::SessionOpen(body) if body.thread_id == pending.thread => {
                    Some((machine, body))
                }
                _ => None,
            })
            .collect();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].0, world.machine_id);
        assert_eq!(opened[0].1.exec.environment_id, pending.environment);
        assert_eq!(opened[0].1.exec.generation, 1);
        assert_eq!(opened[0].1.exec.executor.as_deref(), Some("orch-exec"));
        assert_eq!(opened[0].1.sandbox_mode, start.sandbox_mode);
        drop(conn);
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                    pending.thread
                )
                .await,
            1
        );
        world.engine().stop();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_environment_file_result_is_not_published_if_its_owned_runtime_changes_before_the_reply()
 {
    for generation_changed in [false, true] {
        let world = World::new().await;
        let pending = pending(&world, true).await;
        let mut conn = world.pool.acquire().await.unwrap();
        if generation_changed {
            sqlx::query("UPDATE runtimes SET generation=generation+1 WHERE id=$1")
                .bind(pending.runtime)
                .execute(&mut *conn)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE runtimes SET machine_id=$2 WHERE id=$1")
                .bind(pending.runtime)
                .bind(world.machine_id)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        drop(conn);
        pending.unit.let_go();
        let (state, _, error) = hosting::wait_over(&world, pending.job).await;
        assert_eq!(state, "failed", "{error:?}");
        assert_eq!(error.unwrap()["code"], "conflict.stale_preflight");
        let shown = world.sync(pending.thread).await.thread.source.unwrap();
        assert_ne!(
            shown.state,
            SourceState::Ready,
            "a different runtime cannot inherit an unverified source result"
        );
        let mut conn = world.pool.acquire().await.unwrap();
        let environment = envs::environment(&mut conn, pending.environment)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(environment.status, "running");
        assert_eq!(environment.generation, 0);
        assert_eq!(world.commands.issued_of("turn.start"), 0);
        assert_eq!(world.input_state(pending.input).await.0, "pending");
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM turns WHERE thread_id=$1",
                    pending.thread
                )
                .await,
            0
        );
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM leases WHERE thread_id=$1",
                    pending.thread
                )
                .await,
            0
        );
        world.engine().stop();
    }
}
