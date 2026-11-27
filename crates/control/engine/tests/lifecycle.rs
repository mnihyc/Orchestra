//! An environment is stopped by a person or when it has been idle for as long as its setting allows, never with a turn running unless that is asked for; what is unused is removed after a notice that can be answered by keeping it; removal is a job of its own that is tried again with growing pauses; and what a host reports of a unit is applied.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use chrono::{Duration, Utc};
use common::{World, eventually, meanwhile, queued};
use hosting::{ScriptedHost, running, serve, status, template};
use orch_authz::Caller;
use orch_engine::environments::{StopError, StopReason};
use orch_link_contract::commands::{CommandError, CommandKind as K, ErrorCode};
use serde_json::json;

#[tokio::test]
async fn stopping_a_new_environment_before_its_create_reply_keeps_its_exact_birth_and_data() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Held environment birth", 16_384).await;
    let template = template(&world).await;
    let birth = hosting::hold_created_reply(&host);
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    let made =
        hosting::make_with(&world, template, "Pending birth", Some(host.id), &json!({})).await;
    eventually(
        "the actual allocation before its held reply or job failure",
        || async {
            let allocated = birth.lock().unwrap().is_some();
            allocated
                || matches!(
                    hosting::job(&world, made.job.id).await.0.as_str(),
                    "failed" | "cancelled" | "succeeded"
                )
        },
    )
    .await;
    let (runtime, source) = birth
        .lock()
        .unwrap()
        .expect("the original Create must be admitted before its effect");
    assert_eq!(
        orch_db::queries::runtime_incarnations::current(
            &mut world.pool.acquire().await.unwrap(),
            runtime
        )
        .await
        .unwrap(),
        Some(orch_link_contract::native_target::RuntimeSource::untracked())
    );
    let engine = world.engine();
    let by = world.ada;
    let environment = made.id;
    let stopping = tokio::spawn(async move {
        engine
            .stop_environment(environment, false, StopReason::Person(by))
            .await
    });
    eventually(
        "Stop freezes the birth before waiting for native confirmation",
        || async {
            stopping.is_finished()
                || sqlx::query_scalar::<_, bool>(
                    "SELECT native_stop IS NOT NULL FROM runtimes WHERE id=$1",
                )
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap()
        },
    )
    .await;
    assert!(
        !stopping.is_finished(),
        "Stop cannot finish or refuse before confirming its admitted birth"
    );
    let control: serde_json::Value =
        sqlx::query_scalar("SELECT native_stop->'control' FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(control["source"], json!({"incarnation":null}));
    assert_eq!(control["pending"], json!([source.incarnation.unwrap()]));
    host.added
        .machine
        .handler
        .release_kind(K::NodeRuntimeCreate);
    host.added.machine.handler.release_kind(K::NodeRuntimeStop);
    tokio::time::timeout(common::PATIENCE, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(status(&world, made.id).await.0, "stopped");
    assert_eq!(
        orch_db::queries::runtime_incarnations::current(
            &mut world.pool.acquire().await.unwrap(),
            runtime
        )
        .await
        .unwrap(),
        Some(source)
    );
    assert_ne!(hosting::wait_over(&world, made.job.id).await.0, "succeeded");
    assert_eq!(host.count(K::NodeRuntimeCreate), 1);
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    assert_eq!(host.count(K::NodeRuntimeStart), 0);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
}

#[tokio::test]
async fn a_new_environment_starts_from_its_canonical_creation_source_and_records_its_admitted_start()
 {
    use orch_link_contract::native_target::{NativeTarget, RuntimeSource};
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Original creation host", 16_384).await;
    let template = template(&world).await;
    let (created, starts) = hosting::canonical_creation_replies(&host);
    let made = hosting::make_with(
        &world,
        template,
        "Original creation",
        Some(host.id),
        &json!({}),
    )
    .await;
    eventually(
        "creation starts its own source or reports its terminal failure",
        || async {
            let started = !starts.lock().unwrap().is_empty();
            started
                || matches!(
                    hosting::job(&world, made.job.id).await.0.as_str(),
                    "failed" | "cancelled" | "succeeded"
                )
        },
    )
    .await;
    let state = hosting::job(&world, made.job.id).await;
    let captured = starts.lock().unwrap().first().cloned();
    assert!(
        captured.is_some(),
        "the admitted creation failed before its native Start: {state:?}"
    );
    let (command, target) = captured.unwrap();
    let Some(NativeTarget::Lifecycle(target)) = target else {
        panic!(
            "Start must consume its own canonical Create source before choosing native operands"
        );
    };
    let runtime: orch_types::Id =
        sqlx::query_scalar("SELECT (plan->>'runtime')::uuid FROM jobs WHERE id=$1")
            .bind(made.job.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        target.subject,
        orch_link_contract::native_target::LifecycleSubject::Runtime {
            runtime_id: runtime
        }
    );
    let created = created
        .lock()
        .unwrap()
        .expect("the native model actually allocated its birth")
        .incarnation
        .expect("the actual birth is tracked");
    assert_eq!(target.observed, Some(created));
    let opening = target
        .opening
        .expect("the new Start has its admitted candidate before its first native effect");
    assert_ne!(opening, created);
    let phase: serde_json::Value =
        sqlx::query_scalar("SELECT operands FROM runtime_control_phases WHERE id=$1")
            .bind(command)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(phase["source"], json!({"incarnation":created}));
    assert_eq!(phase["opening"], json!(opening));
    let _unit = hosting::enroll(&world, runtime).await;
    let ended = hosting::wait_over(&world, made.job.id).await;
    assert_eq!(ended.0, "succeeded", "{ended:?}");
    assert_eq!(
        orch_db::queries::runtime_incarnations::current(
            &mut world.pool.acquire().await.unwrap(),
            runtime
        )
        .await
        .unwrap(),
        Some(RuntimeSource {
            incarnation: Some(opening)
        }),
        "the actual canonical Start reply supplies the source that the environment publishes"
    );
}

#[tokio::test]
async fn a_provisioning_reply_never_publishes_or_cleans_up_a_later_unrelated_native_source() {
    use orch_link_contract::native_target::{NativeTarget, RuntimeSource};
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Superseded creation host", 16_384).await;
    let template = template(&world).await;
    let (_, starts) = hosting::canonical_creation_replies(&host);
    let made = hosting::make_with(
        &world,
        template,
        "Superseded creation",
        Some(host.id),
        &json!({}),
    )
    .await;
    eventually(
        "creation reaches its actual native Start or fails",
        || async {
            let started = !starts.lock().unwrap().is_empty();
            started
                || matches!(
                    hosting::job(&world, made.job.id).await.0.as_str(),
                    "failed" | "cancelled" | "succeeded"
                )
        },
    )
    .await;
    let captured = starts.lock().unwrap().first().cloned();
    let Some((_, Some(NativeTarget::Lifecycle(target)))) = captured else {
        panic!("the original creation must reach its source-bound native Start");
    };
    let runtime: orch_types::Id =
        sqlx::query_scalar("SELECT (plan->>'runtime')::uuid FROM jobs WHERE id=$1")
            .bind(made.job.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let opened = RuntimeSource {
        incarnation: target.opening,
    };
    eventually(
        "the original canonical Start result is recorded before enrollment",
        || async {
            orch_db::queries::runtime_incarnations::current(
                &mut world.pool.acquire().await.unwrap(),
                runtime,
            )
            .await
            .unwrap()
                == Some(opened)
        },
    )
    .await;
    let later = RuntimeSource {
        incarnation: Some(orch_types::new_id()),
    };
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::environments::hosting::lock_environment(&mut tx, made.id)
        .await
        .unwrap();
    assert!(
        orch_db::queries::runtime_incarnations::observe(&mut tx, host.id, runtime, later, false)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE runtimes SET state='running' WHERE id=$1")
        .bind(runtime)
        .execute(&mut *tx)
        .await
        .unwrap();
    let volumes: serde_json::Value = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let _unit = hosting::enroll(&world, runtime).await;
    let ended = hosting::wait_over(&world, made.job.id).await;
    assert_ne!(
        ended.0, "succeeded",
        "a different current native source cannot finish this creation"
    );
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    let row: (String, Option<orch_types::Id>, serde_json::Value) =
        sqlx::query_as("SELECT state,native_incarnation,volumes FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        row,
        ("running".into(), later.incarnation, volumes),
        "old forward work and undo both leave the later physical instance and its files alone"
    );
}

#[tokio::test]
async fn starting_a_kept_environment_consumes_its_admitted_source_and_records_the_actual_new_start()
{
    use orch_link_contract::native_target::{NativeTarget, RuntimeSource};
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Kept environment start", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Kept workspace").await;
    let original = orch_types::new_id();
    record_native_start(&world, &host, environment, runtime, original).await;
    world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap();
    let (_, starts) = hosting::canonical_creation_replies(&host);
    let orch_engine::environments::Starting::Job(job) = world
        .engine()
        .start_environment(environment, Some(world.ada))
        .await
        .unwrap()
    else {
        panic!("the kept stopped environment needs its own Start job");
    };
    eventually(
        "the kept environment reaches its admitted Start or fails",
        || async {
            let started = !starts.lock().unwrap().is_empty();
            started
                || matches!(
                    hosting::job(&world, job.id).await.0.as_str(),
                    "failed" | "cancelled" | "succeeded"
                )
        },
    )
    .await;
    let captured = starts.lock().unwrap().first().cloned();
    let Some((command, Some(NativeTarget::Lifecycle(target)))) = captured else {
        panic!("Start must retain the kept environment's original native source");
    };
    assert_eq!(target.observed, Some(original));
    let next = target
        .opening
        .expect("the next native Start has its admitted opening");
    assert_ne!(next, original);
    let mut conn = world.pool.acquire().await.unwrap();
    let admitted = orch_db::queries::runtime_controls::for_job(&mut conn, job.id, runtime)
        .await
        .unwrap()
        .expect("Start admission was retained");
    assert_eq!(admitted.source().incarnation, Some(original));
    assert_eq!(job.plan["runtime"], json!(runtime));
    let operands: serde_json::Value =
        sqlx::query_scalar("SELECT operands FROM runtime_control_phases WHERE id=$1")
            .bind(command)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(operands["source"], json!({"incarnation":original}));
    assert_eq!(operands["opening"], json!(next));
    drop(conn);
    let ended = hosting::wait_over(&world, job.id).await;
    assert_eq!(ended.0, "succeeded", "{ended:?}");
    assert_eq!(
        orch_db::queries::runtime_incarnations::current(
            &mut world.pool.acquire().await.unwrap(),
            runtime
        )
        .await
        .unwrap(),
        Some(RuntimeSource {
            incarnation: Some(next)
        })
    );
    assert_eq!(host.count(K::NodeRuntimeCreate), 0);
    assert_eq!(host.count(K::NodeRuntimeStart), 1);
}

#[tokio::test]
async fn an_existing_environment_start_keeps_its_admitted_source_when_a_later_report_arrives_before_work()
 {
    use orch_link_contract::native_target::RuntimeSource;
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Superseded kept Start", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, unit) = running(&world, &host, template, "Current workspace").await;
    let original = orch_types::new_id();
    record_native_start(&world, &host, environment, runtime, original).await;
    world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let orch_engine::environments::Starting::Job(job) =
        orch_engine::environments::start(&mut tx, world.clock.now(), environment, Some(world.ada))
            .await
            .unwrap()
    else {
        panic!("the Start must have an admitted job before any work");
    };
    let admitted = orch_db::queries::runtime_controls::for_job(&mut tx, job.id, runtime)
        .await
        .unwrap()
        .expect("the original source is captured before the queue commit");
    assert_eq!(admitted.source().incarnation, Some(original));
    let orch_engine::environments::Starting::Job(joined) =
        orch_engine::environments::start(&mut tx, world.clock.now(), environment, Some(world.ada))
            .await
            .unwrap()
    else {
        panic!("the second request joins the original Start");
    };
    assert_eq!(joined.id, job.id);
    assert_eq!(
        orch_db::queries::runtime_controls::for_job(&mut tx, job.id, runtime)
            .await
            .unwrap(),
        Some(admitted)
    );
    let later = RuntimeSource {
        incarnation: Some(orch_types::new_id()),
    };
    assert!(
        orch_db::queries::runtime_incarnations::observe(&mut tx, host.id, runtime, later, false)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE runtimes SET state='running' WHERE id=$1")
        .bind(runtime)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET status='running',machine_id=$2 WHERE id=$1")
        .bind(environment)
        .bind(unit.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let volumes: serde_json::Value = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    let ended = hosting::wait_over(&world, job.id).await;
    assert_ne!(
        ended.0, "succeeded",
        "the older Start cannot publish a later source"
    );
    assert_eq!(host.count(K::NodeRuntimeStart), 0);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    assert_eq!(
        host.count(K::NodeRuntimeStop),
        1,
        "only the original completed Stop ran"
    );
    let row: (String, Option<orch_types::Id>, serde_json::Value) =
        sqlx::query_as("SELECT state,native_incarnation,volumes FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(row, ("running".into(), later.incarnation, volumes));
    assert_eq!(status(&world, environment).await.0, "running");
}

#[tokio::test]
async fn stopping_an_environment_owns_the_already_started_candidate_while_its_start_reply_is_still_held()
 {
    use orch_link_contract::native_target::RuntimeSource;
    use std::sync::atomic::Ordering;
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Pending environment Start", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Pending workspace").await;
    let original = orch_types::new_id();
    record_native_start(&world, &host, environment, runtime, original).await;
    world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap();
    let pending = hosting::hold_started_reply(
        &host,
        runtime,
        RuntimeSource {
            incarnation: Some(original),
        },
    );
    let orch_engine::environments::Starting::Job(job) = world
        .engine()
        .start_environment(environment, Some(world.ada))
        .await
        .unwrap()
    else {
        panic!("a Start job owns this stopped unit");
    };
    eventually("the actual new Start effect before its held reply", || {
        let running = pending.running.load(Ordering::SeqCst);
        async move { running }
    })
    .await;
    let opened = *pending.source.lock().unwrap();
    assert!(opened.incarnation.is_some());
    assert_ne!(opened.incarnation, Some(original));
    assert_eq!(
        orch_db::queries::runtime_incarnations::current(
            &mut world.pool.acquire().await.unwrap(),
            runtime
        )
        .await
        .unwrap(),
        Some(RuntimeSource {
            incarnation: Some(original)
        }),
        "the Start effect happened while its canonical reply remains unreceived"
    );
    let engine = world.engine();
    let by = world.ada;
    let mut stopping = tokio::spawn(async move {
        engine
            .stop_environment(environment, false, StopReason::Person(by))
            .await
    });
    eventually(
        "Stop is admitted against the original pending opening or reports its refusal",
        || async {
            stopping.is_finished()
                || sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM runtimes WHERE id=$1 AND state='stopping')",
                )
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap()
        },
    )
    .await;
    let already_finished = stopping.is_finished();
    if already_finished {
        (&mut stopping)
            .await
            .unwrap()
            .expect("Stop admits the pending original instead of refusing the creating state");
    }
    let control: serde_json::Value =
        sqlx::query_scalar("SELECT native_stop->'control' FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(control["source"], json!({"incarnation":original}));
    assert_eq!(
        control["pending"],
        json!([opened.incarnation.expect("the actual start is tracked")])
    );
    host.added.machine.handler.release_kind(K::NodeRuntimeStart);
    if !already_finished {
        tokio::time::timeout(common::PATIENCE, &mut stopping)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    assert!(!pending.running.load(Ordering::SeqCst));
    assert_eq!(pending.stops.lock().unwrap().len(), 1);
    assert_eq!(status(&world, environment).await.0, "stopped");
    assert_eq!(
        orch_db::queries::runtime_incarnations::current(
            &mut world.pool.acquire().await.unwrap(),
            runtime
        )
        .await
        .unwrap(),
        Some(opened)
    );
    assert_ne!(hosting::wait_over(&world, job.id).await.0, "succeeded");
    let orch_engine::environments::Starting::Job(next) = world
        .engine()
        .start_environment(environment, Some(world.ada))
        .await
        .unwrap()
    else {
        panic!("the kept original is available for another ordinary Start");
    };
    let ended = hosting::wait_over(&world, next.id).await;
    assert_eq!(
        ended.0, "succeeded",
        "the confirmed source supports the next Start without waiting for another report: {ended:?}"
    );
    assert!(pending.running.load(Ordering::SeqCst));
    let next_source = *pending.source.lock().unwrap();
    assert_ne!(next_source, opened);
    assert_eq!(pending.starts.lock().unwrap().len(), 2);
    assert_eq!(
        orch_db::queries::runtime_incarnations::current(
            &mut world.pool.acquire().await.unwrap(),
            runtime
        )
        .await
        .unwrap(),
        Some(next_source)
    );
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
}

#[tokio::test]
async fn a_person_stops_an_environment_and_its_unit_and_volumes_are_kept() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Main").await;
    world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap();
    assert_eq!(
        host.kinds(),
        [K::NodeRuntimeStop],
        "the unit is stopped and nothing is removed"
    );
    assert_eq!(status(&world, environment).await.0, "stopped");
    let state: String = sqlx::query_scalar("SELECT state FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(state, "stopped");
    // Stopping what is stopped is the same stop.
    world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap();
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
}

#[tokio::test]
async fn a_host_report_finishes_an_environment_stop_while_its_command_reply_is_held() {
    for state in ["absent", "stopped", "failed"] {
        let world = World::new().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "Reporting host", 16_384).await;
        let template = template(&world).await;
        let (environment, runtime, _unit) =
            running(&world, &host, template, "Stopped workspace").await;
        host.added.machine.handler.hold(K::NodeRuntimeStop);
        let engine = world.engine();
        let by = world.ada;
        let stopping = tokio::spawn(async move {
            engine
                .stop_environment(environment, false, StopReason::Person(by))
                .await
        });
        host.wait_began(K::NodeRuntimeStop, 1).await;
        host.tell(runtime, state, None).await;
        eventually(
            "the reported stop to settle before the command reply",
            || async { status(&world, environment).await.0 == "stopped" },
        )
        .await;
        let row: (String, bool) =
            sqlx::query_as("SELECT state,loss_missing FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert_eq!(row, ("stopped".into(), state == "absent"));
        host.added.machine.handler.release();
        stopping.await.unwrap().unwrap();
        assert_eq!(status(&world, environment).await.0, "stopped");
    }
}

#[tokio::test]
async fn a_late_stop_reply_keeps_the_environment_of_a_newer_start_running() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Reporting host", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) =
        running(&world, &host, template, "Restarted workspace").await;
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    let engine = world.engine();
    let by = world.ada;
    let stopping = tokio::spawn(async move {
        engine
            .stop_environment(environment, false, StopReason::Person(by))
            .await
    });
    host.wait_began(K::NodeRuntimeStop, 1).await;
    // A completed stop report and a subsequent start can be committed before the first waiter's reply.
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::environments::hosting::lock_environment(&mut tx, environment)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE runtimes SET state='running',updated_at=updated_at+interval '1 second' WHERE id=$1",
    )
    .bind(runtime)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    host.added.machine.handler.release();
    stopping.await.unwrap().unwrap();
    assert_eq!(status(&world, environment).await.0, "running");
    let state: String = sqlx::query_scalar("SELECT state FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(state, "running");
}

#[tokio::test]
async fn starting_after_a_reported_absent_stop_makes_an_environment_unit_on_the_same_owned_volumes()
{
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Reporting host", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Missing workspace").await;
    let volumes: serde_json::Value = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE runtimes SET state='stopping' WHERE id=$1")
        .bind(runtime)
        .execute(world.pool.pool())
        .await
        .unwrap();
    host.tell(runtime, "absent", None).await;
    eventually("the absent stop to settle", || async {
        status(&world, environment).await.0 == "stopped"
    })
    .await;
    let orch_engine::environments::Starting::Job(job) = world
        .engine()
        .start_environment(environment, Some(world.ada))
        .await
        .unwrap()
    else {
        panic!("a unit reported absent must be made again");
    };
    assert_eq!(
        job.plan["mode"], "make",
        "Start cannot target a unit the host has reported absent"
    );
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let replacement = host.started().unwrap();
    assert_ne!(runtime, replacement);
    let _replacement = hosting::enroll(&world, replacement).await;
    let (state, _, error) = hosting::wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let kept: serde_json::Value = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(replacement)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(volumes, kept);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    assert_eq!(status(&world, environment).await.0, "running");
}

#[tokio::test]
async fn an_environment_with_a_turn_running_is_not_stopped_unless_the_turns_are_interrupted() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Main").await;
    let thread = world.thread("work").await;
    hosting::bind(&world, thread, environment).await;
    let (start, _) = world.running_turn(thread, "go").await;
    let refused = world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap_err();
    assert!(matches!(refused, StopError::Busy(1)), "{refused:?}");
    assert_eq!(status(&world, environment).await.0, "running");
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    // With the interrupt asked for, the turn is stopped first.
    let engine = world.engine();
    let stopping = tokio::spawn(async move {
        engine
            .stop_environment(environment, true, StopReason::Person(world.ada))
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
    assert_eq!(status(&world, environment).await.0, "stopped");
}

#[tokio::test]
async fn an_environment_idle_for_as_long_as_its_setting_allows_is_stopped_and_activity_starts_the_clock_over()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (idle, _, _u1) = running(&world, &host, template, "Idle").await;
    let (kept, _, _u2) = running(&world, &host, template, "Kept").await;
    // Nothing stops while the two are made ready: never, until the hour is set.
    world
        .set(world.root, "env.idle_stop_minutes", json!(0))
        .await;
    // Both were last active two hours ago; one is then kept awake.
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = ANY($1)")
        .bind([idle, kept])
        .bind(Utc::now() - Duration::hours(2))
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert!(
        world.engine().keep_awake(kept).await.unwrap(),
        "Keep awake counts as activity now"
    );
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    eventually("the idle environment to be stopped", || async {
        status(&world, idle).await.0 == "stopped"
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        status(&world, kept).await.0,
        "running",
        "the clock started over"
    );
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
}

#[tokio::test]
async fn an_idle_environment_whose_stop_the_host_has_not_answered_is_not_asked_to_stop_a_second_time()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Slow").await;
    // The host takes the stop and is slow to answer it.
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = $1")
        .bind(environment)
        .bind(Utc::now() - Duration::hours(2))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    host.wait_began(K::NodeRuntimeStop, 1).await;
    // The schedule looks again every 150 ms in this world, several times over this wait: while the host
    // has not answered, the stop is not asked for a second time.
    meanwhile(std::time::Duration::from_millis(900)).await;
    assert_eq!(host.began(K::NodeRuntimeStop), 1);
    host.added.machine.handler.release();
    eventually("the environment to be stopped", || async {
        status(&world, environment).await.0 == "stopped"
    })
    .await;
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
}

#[tokio::test]
async fn an_idle_stop_that_was_decided_on_an_older_look_leaves_an_environment_that_has_been_active_since()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Busy").await;
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    // It was active a moment ago: whoever found it idle looked before that.
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = $1")
        .bind(environment)
        .bind(Utc::now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .engine()
        .stop_environment(environment, false, StopReason::Idle)
        .await
        .unwrap();
    assert_eq!(status(&world, environment).await.0, "running");
    assert!(host.kinds().is_empty());
}

#[tokio::test]
async fn an_environment_that_is_never_to_stop_is_left_up_however_idle() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Main").await;
    world
        .set(world.root, "env.idle_stop_minutes", json!(0))
        .await;
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = $1")
        .bind(environment)
        .bind(Utc::now() - Duration::days(30))
        .execute(world.pool.pool())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert_eq!(status(&world, environment).await.0, "running");
    assert!(host.kinds().is_empty());
}

#[tokio::test]
async fn a_background_task_keeps_its_environment_up_after_the_foreground_turn_ends() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "background-idle", 16_384).await;
    let template = template(&world).await;
    let (busy, _, _busy_unit) = running(&world, &host, template, "Background work").await;
    let (idle, _, _idle_unit) = running(&world, &host, template, "Idle control").await;
    let thread = world.thread("Native background work").await;
    hosting::bind(&world, thread, busy).await;
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
    let old = Utc::now() - Duration::hours(3);
    sqlx::query("UPDATE environments SET last_activity_at=$2 WHERE id=ANY($1)")
        .bind([busy, idle])
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    eventually("the idle environment control to stop", || async {
        status(&world, idle).await.0 == "stopped"
    })
    .await;
    meanwhile(std::time::Duration::from_millis(600)).await;
    assert_eq!(status(&world, busy).await.0, "running");
    task.state = orch_link_contract::background::BackgroundTaskState::Stopped;
    common::background::update(&world, thread, task).await;
    sqlx::query("UPDATE environments SET last_activity_at=$2 WHERE id=$1")
        .bind(busy)
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually(
        "the environment becomes idle after its task ends",
        || async { status(&world, busy).await.0 == "stopped" },
    )
    .await;
}

#[tokio::test]
async fn removal_is_a_job_of_its_own_that_stops_and_removes_the_unit_and_keeps_its_volumes() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Main").await;
    world
        .set(world.root, "env.removed_keep_days", json!(1))
        .await;
    sqlx::query("UPDATE environments SET remove_at = now() WHERE id = $1")
        .bind(environment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the environment to be removed", || async {
        let gone: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT deleted_at FROM environments WHERE id = $1")
                .bind(environment)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        gone.is_some()
    })
    .await;
    assert_eq!(host.kinds(), [K::NodeRuntimeStop, K::NodeRuntimeRemove]);
    let state: String = sqlx::query_scalar("SELECT state FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(state, "removed");
    // The environment's other job is the record of how its unit was made, which the fixture keeps as
    // provisioning does.
    eventually("the job to end", || async {
        let state: String = sqlx::query_scalar(
            "SELECT state FROM jobs WHERE environment_id = $1 AND kind <> 'environment_provision'",
        )
        .bind(environment)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
        state == "succeeded"
    })
    .await;
    let kind: String = sqlx::query_scalar(
        "SELECT kind FROM jobs WHERE environment_id = $1 AND kind <> 'environment_provision'",
    )
    .bind(environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(kind, "environment_remove");
    let kept: bool = sqlx::query_scalar(
        "SELECT volumes_purge_at >= deleted_at + interval '1 day' FROM environments WHERE id = $1",
    )
    .bind(environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert!(
        kept,
        "the minimum positive retention day keeps the volumes instead of immediately purging them"
    );
}

#[tokio::test]
async fn a_removal_that_fails_is_tried_again_with_pauses_that_grow_and_the_reason_is_shown() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Main").await;
    for _ in 0..2 {
        host.refuse(
            K::NodeRuntimeRemove,
            CommandError::new(ErrorCode::Unsupported, "the daemon is busy"),
        );
    }
    sqlx::query("UPDATE environments SET remove_at = now() WHERE id = $1")
        .bind(environment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the first removal to fail", || async {
        host.count(K::NodeRuntimeRemove) == 1
    })
    .await;
    eventually("the reason to be shown on the environment", || async {
        status(&world, environment).await.1.is_some_and(|detail| {
            detail.contains("the daemon is busy") && detail.contains("tried again")
        })
    })
    .await;
    eventually(
        "the environment to be removed after two failures",
        || async {
            let gone: Option<chrono::DateTime<Utc>> =
                sqlx::query_scalar("SELECT deleted_at FROM environments WHERE id = $1")
                    .bind(environment)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            gone.is_some()
        },
    )
    .await;
    assert_eq!(
        host.count(K::NodeRuntimeRemove),
        3,
        "two failures, then it went"
    );
    let failed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE environment_id = $1 AND kind = 'environment_remove' AND state = 'failed'",
    )
    .bind(environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(failed, 2);
    let attempts: Vec<Option<serde_json::Value>> = sqlx::query_scalar(
        "SELECT plan->'lifecycle_wait' FROM jobs WHERE environment_id=$1
         AND kind='environment_remove' AND state='failed' ORDER BY created_at,id",
    )
    .bind(environment)
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    for (index, attempt) in attempts.into_iter().enumerate() {
        let attempt =
            attempt.expect("each actual failed removal retains its accepted retry boundary");
        let at: chrono::DateTime<Utc> = attempt["last_attempt_at"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let retry: chrono::DateTime<Utc> = attempt["retry_eligible_at"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let expected = common::config().hosting.removal_first_pause * (1u32 << index);
        assert_eq!(retry - at, chrono::Duration::from_std(expected).unwrap());
        assert_eq!(attempt["reason_code"], "unsupported");
    }
}

#[tokio::test]
async fn an_environment_unused_for_as_long_as_its_setting_says_is_scheduled_for_removal_after_a_notice_and_can_be_kept()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (unused, _, _u1) = running(&world, &host, template, "Unused").await;
    let (never, _, _u2) = running(&world, &host, template, "Never").await;
    world
        .set(world.root, "env.unused_remove_days", json!(3))
        .await;
    // `Never` has the setting off of its own; `Unused` has had no thread for four days.
    sqlx::query("UPDATE environments SET config = jsonb_build_object('env.unused_remove_days', 0) WHERE id = $1")
        .bind(never)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET created_at = $2, unused_since = $2 WHERE id = ANY($1)")
        .bind([unused, never])
        .bind(Utc::now() - Duration::days(4))
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the removal to be scheduled", || async {
        let at: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
                .bind(unused)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        at.is_some()
    })
    .await;
    let at: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
            .bind(unused)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        at > Utc::now() + Duration::minutes(50),
        "ahead by the notice, an hour in the tests: {at}"
    );
    let none: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
            .bind(never)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(none.is_none(), "its own setting says never");
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'environment.removal_scheduled' AND object_id = $1",
    )
    .bind(unused)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(told, 1, "a line in its history says it");
    // Keep it: the schedule is withdrawn and the count starts over.
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_db::queries::environments::hosting::keep(&mut conn, unused, Utc::now())
            .await
            .unwrap()
    );
    drop(conn);
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    let again: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
            .bind(unused)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(again.is_none(), "it was kept, and the days start from now");
}

#[tokio::test]
async fn a_removal_is_scheduled_together_with_the_line_in_its_history_or_not_at_all() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (unused, _, _unit) = running(&world, &host, template, "Unused").await;
    // The history cannot be written.
    world
        .exec(
            "CREATE FUNCTION refuse_history() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'the history is not written'; END $$",
        )
        .await;
    world
        .exec(
            "CREATE TRIGGER refuse_history BEFORE INSERT ON audit_log FOR EACH ROW
             WHEN (NEW.action = 'environment.removal_scheduled') EXECUTE FUNCTION refuse_history()",
        )
        .await;
    world
        .set(world.root, "env.unused_remove_days", json!(3))
        .await;
    sqlx::query("UPDATE environments SET created_at = $2, unused_since = $2 WHERE id = $1")
        .bind(unused)
        .bind(Utc::now() - Duration::days(4))
        .execute(world.pool.pool())
        .await
        .unwrap();
    // The schedule looks every 150 ms in this world, and tries each time.
    meanwhile(std::time::Duration::from_millis(900)).await;
    let at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
            .bind(unused)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        at.is_none(),
        "no removal stands that its history does not say"
    );
}

/// A person's Delete schedules the removal and writes its line with the request's origin (the address among
/// it), as the API does; the reaper queues the removal when it falls due (a first attempt that failed is dated again
/// the same way), and the job keeps who asked and from where.
async fn schedule_from_a_browser(world: &World, environment: orch_types::Id, address: &str) {
    let now = Utc::now();
    let mut tx = world.pool.begin().await.unwrap();
    assert!(
        world
            .engine()
            .schedule_environment_removal(
                &mut tx,
                environment,
                Some(world.ada),
                now + Duration::hours(1),
                now
            )
            .await
            .unwrap()
    );
    let promise = orch_db::queries::environments::retained::removal(&mut tx, environment)
        .await
        .unwrap()
        .unwrap();
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(world.ada),
        "environment.removal_scheduled",
        "environment",
        Some(environment),
        "Ada scheduled the removal of the environment",
    );
    entry.details =
        json!({"environment": environment, "removal_revision": promise.removal_revision});
    let origin = orch_db::audit::Origin {
        as_admin: None,
        ip: Some(address.parse().unwrap()),
        user_agent: Some("Firefox".to_owned()),
        request_id: Some(format!("delete-{environment}")),
    };
    orch_db::audit::append(&mut tx, &entry, &origin, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn removal_plan(world: &World, environment: orch_types::Id) -> Option<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT plan FROM jobs WHERE environment_id = $1 AND kind = 'environment_remove' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(environment)
    .fetch_optional(world.pool.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_due_removal_whose_scheduling_line_carries_the_persons_address_is_queued_and_keeps_that_origin()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Deleted from a browser").await;
    schedule_from_a_browser(&world, environment, "203.0.113.7").await;
    sqlx::query("UPDATE environments SET remove_at = now() WHERE id = $1")
        .bind(environment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the due removal to be queued", || async {
        removal_plan(&world, environment).await.is_some()
    })
    .await;
    let plan = removal_plan(&world, environment).await.unwrap();
    assert_eq!(plan["origin"]["ip"], "203.0.113.7");
    assert_eq!(plan["origin"]["user_agent"], "Firefox");
    assert_eq!(
        plan["origin"]["request_id"],
        format!("delete-{environment}")
    );
}

#[tokio::test]
async fn one_due_removal_that_cannot_be_queued_holds_up_no_other() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (stuck, _, _first) = running(&world, &host, template, "Stuck").await;
    let (next, _, _second) = running(&world, &host, template, "Next").await;
    // The first in line cannot be written; the sweep goes past it to the next.
    world
        .exec(
            "CREATE FUNCTION refuse_removal() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'this removal cannot be written'; END $$",
        )
        .await;
    world
        .exec(&format!(
            "CREATE TRIGGER refuse_removal BEFORE INSERT ON jobs FOR EACH ROW
             WHEN (NEW.kind = 'environment_remove' AND NEW.environment_id = '{stuck}'::uuid)
             EXECUTE FUNCTION refuse_removal()"
        ))
        .await;
    sqlx::query(
        "UPDATE environments SET remove_at = CASE WHEN id = $1 THEN now() - interval '1 minute' ELSE now() END
          WHERE id = ANY($2)",
    )
    .bind(stuck)
    .bind(vec![stuck, next])
    .execute(world.pool.pool())
    .await
    .unwrap();
    eventually("the removal behind the stuck one to be queued", || async {
        removal_plan(&world, next).await.is_some()
    })
    .await;
    assert!(removal_plan(&world, stuck).await.is_none());
}

#[tokio::test]
async fn a_unit_the_node_finds_gone_makes_its_environment_failed_with_the_reason() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Main").await;
    let thread = world.thread("work").await;
    hosting::bind(&world, thread, environment).await;
    host.tell(runtime, "failed", Some("exit status 137")).await;
    eventually("the environment to fail", || async {
        status(&world, environment).await.0 == "failed"
    })
    .await;
    assert_eq!(
        status(&world, environment).await.1.as_deref(),
        Some("exit status 137")
    );
    // A thread that needs it is blocked with that reason, and keeps its message.
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
}

#[tokio::test]
async fn an_environment_near_its_idle_stop_warns_its_people_once_and_is_not_stopped_before_its_time()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Quiet").await;
    world
        .set(world.root, "env.idle_stop_minutes", json!(0))
        .await;
    // Half an hour without activity, an hour allowed, a warning 45 minutes ahead: it is due now.
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = $1")
        .bind(environment)
        .bind(Utc::now() - Duration::minutes(30))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(world.root, "env.idle_warning_minutes", json!(45))
        .await;
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    let warnings = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications WHERE kind = 'environment.idle_stop'
              AND data ->> 'environment_id' = $1::text AND (data ->> 'warning')::boolean",
        )
        .bind(environment)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
    };
    eventually("the idle warning", || async { warnings().await == 1 }).await;
    meanwhile(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        warnings().await,
        1,
        "told once for this stretch without activity"
    );
    assert_eq!(
        status(&world, environment).await.0,
        "running",
        "its time has not come"
    );
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
}

async fn record_native_start(
    world: &World,
    host: &ScriptedHost,
    environment: orch_types::Id,
    runtime: orch_types::Id,
    incarnation: orch_types::Id,
) {
    host.record_native_source(
        runtime,
        orch_link_contract::native_target::RuntimeSource {
            incarnation: Some(incarnation),
        },
        true,
    );
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::environments::hosting::lock_environment(&mut tx, environment)
        .await
        .unwrap();
    assert!(
        orch_db::queries::runtime_incarnations::observe(
            &mut tx,
            host.id,
            runtime,
            orch_link_contract::native_target::RuntimeSource {
                incarnation: Some(incarnation)
            },
            false
        )
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();
}

async fn accept_stop(
    world: &World,
    environment: orch_types::Id,
) -> orch_engine::environments::AcceptedStop {
    accept_stop_with_interrupt(world, environment, false).await
}

async fn accept_stop_with_interrupt(
    world: &World,
    environment: orch_types::Id,
    interrupt: bool,
) -> orch_engine::environments::AcceptedStop {
    let mut tx = world.pool.begin().await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut tx,
        Caller::session(world.ada),
        orch_types::Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    world
        .engine()
        .accept_environment_stop_in(
            tx,
            &principal,
            &orch_db::audit::Origin::default(),
            orch_engine::environments::StopRequest {
                environment,
                interrupt,
                reason: StopReason::Person(world.ada),
            },
            || Ok(()),
            || Ok(()),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn an_accepted_environment_stop_withdraws_its_original_before_preparation() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Preparing environment Stop", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Preparing Stop").await;
    record_native_start(&world, &host, environment, runtime, orch_types::new_id()).await;
    let starts = host.count(K::NodeRuntimeStart);
    let accepted = accept_stop(&world, environment).await;
    let stop = orch_db::queries::runtime_incarnations::stop(
        &mut world.pool.acquire().await.unwrap(),
        runtime,
    )
    .await
    .unwrap()
    .expect("the original is retained before preparation");
    assert!(!stop.ready);
    assert_eq!(status(&world, environment).await.0, "running");
    let started = world
        .engine()
        .start_environment(environment, Some(world.ada))
        .await;
    assert!(
        matches!(
            started,
            Err(orch_engine::environments::StartError::Unavailable(_))
        ),
        "new work cannot reuse a withdrawn original: {started:?}"
    );
    assert!(
        !orch_engine::resource_facts::activity::accepts_context(
            &mut world.pool.acquire().await.unwrap(),
            world.provider,
            common::MODEL,
            None,
            Some(environment),
        )
        .await
        .unwrap()
    );
    assert_eq!(host.count(K::NodeRuntimeStart), starts);
    world
        .engine()
        .finish_accepted_environment_stop(accepted)
        .await
        .unwrap();
    assert_eq!(status(&world, environment).await.0, "stopped");
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
}

#[tokio::test]
async fn an_old_environment_stop_does_not_withdraw_a_new_home_on_the_same_unit() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Independent environment home", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Independent home").await;
    record_native_start(&world, &host, environment, runtime, orch_types::new_id()).await;
    let home: orch_types::Id =
        sqlx::query_scalar("SELECT home_node_id FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let next = if home == world.topic {
        world.root
    } else {
        world.topic
    };
    let accepted = accept_stop(&world, environment).await;
    sqlx::query("UPDATE environments SET home_node_id=$2 WHERE id=$1")
        .bind(environment)
        .bind(next)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let started = world
        .engine()
        .start_environment(environment, Some(world.ada))
        .await
        .unwrap();
    assert!(matches!(started, orch_engine::environments::Starting::Up));
    assert!(
        orch_engine::resource_facts::activity::accepts_context(
            &mut world.pool.acquire().await.unwrap(),
            world.provider,
            common::MODEL,
            None,
            Some(environment),
        )
        .await
        .unwrap()
    );
    let stopped = world
        .engine()
        .finish_accepted_environment_stop(accepted)
        .await;
    assert!(
        matches!(stopped, Err(StopError::Unavailable(_))),
        "the old Stop cannot act in the new home: {stopped:?}"
    );
    assert_eq!(status(&world, environment).await.0, "running");
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
}

#[tokio::test]
async fn an_accepted_stop_keeps_a_later_native_start_of_the_same_runtime_running() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Original stop host", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) =
        running(&world, &host, template, "Original stop workspace").await;
    let original = orch_types::new_id();
    record_native_start(&world, &host, environment, runtime, original).await;
    let accepted = accept_stop(&world, environment).await;
    record_native_start(&world, &host, environment, runtime, orch_types::new_id()).await;
    let stopped = world
        .engine()
        .finish_accepted_environment_stop(accepted)
        .await;
    assert!(
        matches!(stopped, Err(StopError::Unavailable(_))),
        "{stopped:?}"
    );
    assert_eq!(
        host.count(K::NodeRuntimeStop),
        0,
        "the original stop never adopts the replacement"
    );
    assert_eq!(status(&world, environment).await.0, "running");
    let state: String = sqlx::query_scalar("SELECT state FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(state, "running");
}

#[tokio::test]
async fn an_old_successful_stop_reply_does_not_confirm_a_later_native_start() {
    for changed_stamp in [false, true] {
        let world = World::new().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "Held original stop host", 16_384).await;
        let template = template(&world).await;
        let (environment, runtime, _unit) =
            running(&world, &host, template, "Held original stop").await;
        record_native_start(&world, &host, environment, runtime, orch_types::new_id()).await;
        host.added.machine.handler.hold(K::NodeRuntimeStop);
        let engine = world.engine();
        let person = world.ada;
        let stopping = tokio::spawn(async move {
            engine
                .stop_environment(environment, false, StopReason::Person(person))
                .await
        });
        host.wait_began(K::NodeRuntimeStop, 1).await;
        let stamp: chrono::DateTime<Utc> =
            sqlx::query_scalar("SELECT updated_at FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        let replacement = orch_types::new_id();
        record_native_start(&world, &host, environment, runtime, replacement).await;
        let pending: (String, chrono::DateTime<Utc>) =
            sqlx::query_as("SELECT state,updated_at FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert_eq!(
            pending,
            ("stopping".into(), stamp),
            "the old timestamp guard alone cannot distinguish native starts"
        );
        if changed_stamp {
            let mut tx = world.pool.begin().await.unwrap();
            orch_db::queries::environments::hosting::lock_environment(&mut tx, environment)
                .await
                .unwrap();
            sqlx::query(
                "UPDATE runtimes SET updated_at=updated_at+interval '1 second' WHERE id=$1",
            )
            .bind(runtime)
            .execute(&mut *tx)
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        host.added.machine.handler.release();
        let stopped = stopping.await.unwrap();
        let observed: (String, Option<orch_types::Id>, chrono::DateTime<Utc>) =
            sqlx::query_as("SELECT state,native_incarnation,updated_at FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert!(
            matches!(stopped, Err(StopError::Unavailable(_))),
            "{stopped:?}; changed timestamp: {changed_stamp}; original timestamp: {stamp:?}; actual runtime: {observed:?}"
        );
        assert_eq!(status(&world, environment).await.0, "running");
        let current: (String, Option<orch_types::Id>) =
            sqlx::query_as("SELECT state,native_incarnation FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert_eq!(
            current,
            ("stopping".into(), Some(replacement)),
            "the replacement has no confirmed Stop"
        );
    }
}

#[tokio::test]
async fn a_late_environment_stop_receipt_does_not_publish_into_a_changed_home() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Stop reply changed home", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) =
        running(&world, &host, template, "Changed home reply").await;
    let original = orch_types::new_id();
    record_native_start(&world, &host, environment, runtime, original).await;
    host.added.machine.handler.hold_result(K::NodeRuntimeStop);
    let engine = world.engine();
    let by = world.ada;
    let stopping = tokio::spawn(async move {
        engine
            .stop_environment(environment, false, StopReason::Person(by))
            .await
    });
    host.wait_for(K::NodeRuntimeStop, 1).await;
    let mut tx = world.pool.begin().await.unwrap();
    let previous = orch_db::queries::environments::hosting::lock_environment(&mut tx, environment)
        .await
        .unwrap()
        .unwrap();
    let stop = orch_db::queries::runtime_incarnations::stop(&mut tx, runtime)
        .await
        .unwrap()
        .unwrap();
    let next = if previous.home_node_id == world.topic {
        world.root
    } else {
        world.topic
    };
    sqlx::query("UPDATE environments SET home_node_id=$2 WHERE id=$1")
        .bind(environment)
        .bind(next)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    host.added.machine.handler.release_kind(K::NodeRuntimeStop);
    let result = tokio::time::timeout(common::PATIENCE, stopping)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(StopError::Unavailable(_))),
        "the old reply records its physical receipt without becoming a new-home operation: {result:?}"
    );
    let current = orch_db::queries::environments::hosting::environment(
        &mut world.pool.acquire().await.unwrap(),
        environment,
    )
    .await
    .unwrap()
    .unwrap();
    let outcome: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT outcome FROM runtime_control_phases WHERE id=$1")
            .bind(stop.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        outcome,
        Some(json!({"outcome":"stopped","source":{"incarnation":original}})),
        "the original command retains the verified host receipt even after the registration changes"
    );
    assert_eq!(current.home_node_id, next);
    assert_eq!(
        current.status, previous.status,
        "the old action does not stamp the new registration"
    );
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
}

#[tokio::test]
async fn an_environment_stop_finishes_its_original_phase_after_its_progress_detail_changes() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Same original Stop progress", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Stop progress").await;
    record_native_start(&world, &host, environment, runtime, orch_types::new_id()).await;
    host.added.machine.handler.hold_result(K::NodeRuntimeStop);
    let engine = world.engine();
    let by = world.ada;
    let stopping = tokio::spawn(async move {
        engine
            .stop_environment(environment, false, StopReason::Person(by))
            .await
    });
    host.wait_for(K::NodeRuntimeStop, 1).await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::environments::hosting::lock_environment(&mut tx, environment)
        .await
        .unwrap();
    let original = orch_db::queries::runtime_incarnations::stop(&mut tx, runtime)
        .await
        .unwrap()
        .unwrap();
    let command = orch_db::queries::link::command(&mut tx, original.id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        command.state.as_str(),
        "queued" | "sent" | "accepted"
    ));
    let cleanup = orch_db::queries::lifecycle::controls(&mut tx, i64::MAX)
        .await
        .unwrap();
    assert!(
        cleanup.iter().all(|row| row.id != original.id),
        "the environment owns its Stop while the machine's reply is pending"
    );
    let updated: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        orch_db::queries::environments::hosting::set_runtime_state(
            &mut tx,
            runtime,
            &["stopping"],
            "stopping",
            Some("the same original Stop is still waiting"),
            updated + Duration::microseconds(1),
        )
        .await
        .unwrap()
    );
    assert_eq!(
        orch_db::queries::runtime_incarnations::stop(&mut tx, runtime)
            .await
            .unwrap(),
        Some(original)
    );
    tx.commit().await.unwrap();
    host.added.machine.handler.release_kind(K::NodeRuntimeStop);
    tokio::time::timeout(common::PATIENCE, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        status(&world, environment).await.0,
        "stopped",
        "a matching original receipt completes despite a progress-only edit"
    );
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
}

#[tokio::test]
async fn an_accepted_stop_of_the_same_native_start_still_finishes_and_keeps_its_data() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Matching stop host", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Matching stop").await;
    let incarnation = orch_types::new_id();
    record_native_start(&world, &host, environment, runtime, incarnation).await;
    let volumes: serde_json::Value = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    let accepted = accept_stop(&world, environment).await;
    world
        .engine()
        .finish_accepted_environment_stop(accepted)
        .await
        .unwrap();
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    let sent = world.commands.issued_commands("node.runtime.stop");
    assert_eq!(sent.len(), 1);
    let (machine, command) = &sent[0];
    assert_eq!(*machine, host.machine);
    let Some(orch_link_contract::native_target::NativeTarget::Lifecycle(target)) =
        &command.native_target
    else {
        panic!("the original stop must reach the native role with its physical target");
    };
    assert_eq!(
        target.subject,
        orch_link_contract::native_target::LifecycleSubject::Runtime {
            runtime_id: runtime
        }
    );
    assert_eq!(target.observed, Some(incarnation));
    assert_eq!(target.opening, None);
    let revision: i64 =
        sqlx::query_scalar("SELECT revision FROM runtime_control_phases WHERE id=$1")
            .bind(command.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(target.revision, revision);
    assert_eq!(status(&world, environment).await.0, "stopped");
    let kept: serde_json::Value = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(kept, volumes);
}

#[tokio::test]
async fn a_registration_change_during_stop_preparation_keeps_the_new_registration_running() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Original registration host", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) =
        running(&world, &host, template, "Original registration").await;
    let source = orch_types::new_id();
    record_native_start(&world, &host, environment, runtime, source).await;
    let thread = world.thread("Work before the move").await;
    hosting::bind(&world, thread, environment).await;
    let (turn, _) = world.running_turn(thread, "go").await;
    let accepted = accept_stop_with_interrupt(&world, environment, true).await;
    let engine = world.engine();
    let stopping =
        tokio::spawn(async move { engine.finish_accepted_environment_stop(accepted).await });
    eventually("the original stop to wait for its actual turn", || async {
        world.ran(K::TurnInterrupt) == 1
    })
    .await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::environments::hosting::lock_environment(&mut tx, environment)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET home_node_id=$2 WHERE id=$1")
        .bind(environment)
        .bind(world.root)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world
        .say(
            thread,
            orch_link_contract::events::Event::TurnCompleted(
                orch_link_contract::events::TurnCompleted {
                    turn_id: turn.turn_id,
                    status: orch_link_contract::events::TurnOutcome::Interrupted,
                    error: None,
                },
            ),
        )
        .await;
    let result = stopping.await.unwrap();
    assert!(
        matches!(result, Err(StopError::Unavailable(_))),
        "{result:?}"
    );
    assert_eq!(
        host.count(K::NodeRuntimeStop),
        0,
        "the accepted stop cannot follow the moved registration"
    );
    assert_eq!(status(&world, environment).await.0, "running");
    let current: (String, Option<orch_types::Id>) =
        sqlx::query_as("SELECT state,native_incarnation FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        current,
        ("running".into(), Some(source)),
        "unchanged physical identity does not substitute for the original registration"
    );
}

#[tokio::test]
async fn a_newer_control_during_stop_preparation_keeps_the_same_native_start_untouched() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Original intent host", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Original intent").await;
    let source = orch_types::new_id();
    record_native_start(&world, &host, environment, runtime, source).await;
    let thread = world.thread("Work before the new control").await;
    hosting::bind(&world, thread, environment).await;
    let (turn, _) = world.running_turn(thread, "go").await;
    let accepted = accept_stop_with_interrupt(&world, environment, true).await;
    let engine = world.engine();
    let stopping =
        tokio::spawn(async move { engine.finish_accepted_environment_stop(accepted).await });
    eventually(
        "the original intent to wait for turn settlement",
        || async { world.ran(K::TurnInterrupt) == 1 },
    )
    .await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::environments::hosting::lock_environment(&mut tx, environment)
        .await
        .unwrap();
    let later = orch_db::queries::runtime_controls::accept(&mut tx, runtime, orch_types::new_id())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world
        .say(
            thread,
            orch_link_contract::events::Event::TurnCompleted(
                orch_link_contract::events::TurnCompleted {
                    turn_id: turn.turn_id,
                    status: orch_link_contract::events::TurnOutcome::Interrupted,
                    error: None,
                },
            ),
        )
        .await;
    let result = stopping.await.unwrap();
    assert!(
        matches!(result, Err(StopError::Unavailable(_))),
        "{result:?}"
    );
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    assert_eq!(status(&world, environment).await.0, "running");
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_db::queries::runtime_controls::is_current(&mut conn, &later)
            .await
            .unwrap()
    );
    let current: (String, Option<orch_types::Id>) =
        sqlx::query_as("SELECT state,native_incarnation FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        current,
        ("running".into(), Some(source)),
        "a newer intent matters even without replacing the physical process"
    );
}

#[tokio::test]
async fn shells_browsers_and_displays_withdraw_during_stop_preparation_and_follow_a_new_home() {
    use orch_db::queries::{browsers, terminals};
    use orch_engine::{browsers::Browsers, displays::Displays, shells::Shells};

    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Interactive Stop host", 16_384).await;
    let template = template(&world).await;
    let (env, _, unit) = running(&world, &host, template, "Interactive Stop").await;
    sqlx::query("UPDATE environments SET terminal=true,browser=true,vnc=true WHERE id=$1")
        .bind(env)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=true,facts=jsonb_set(COALESCE(facts,'{}'),'{policy}',COALESCE(facts->'policy','{}')||$2) WHERE id=ANY($1)")
        .bind([host.machine, unit.id].as_slice())
        .bind(json!({"environment.terminal":true,"environment.browser":true,"environment.vnc":true}))
        .execute(world.pool.pool()).await.unwrap();
    let mut accepted = None;
    for step in 0..3 {
        if step == 1 {
            accepted = Some(accept_stop(&world, env).await);
        } else if step == 2 {
            sqlx::query("UPDATE environments SET home_node_id=$2 WHERE id=$1")
                .bind(env)
                .bind(world.root)
                .execute(world.pool.pool())
                .await
                .unwrap();
        }
        let mut conn = world.pool.acquire().await.unwrap();
        let screen = browsers::site(&mut conn, env).await.unwrap().unwrap();
        let shell = terminals::site(&mut conn, env).await.unwrap().unwrap();
        assert_eq!(screen.status, "running");
        let reasons = [
            json!(
                Shells::opening_gates(&mut conn, &shell, orch_settings::Place::Environment(env))
                    .await
                    .unwrap()
            ),
            json!(Browsers::off(&mut conn, &screen).await.unwrap()),
            json!(Displays::off(&mut conn, &screen).await.unwrap()),
        ];
        let expected = if step == 1 {
            json!("environment_stopping")
        } else {
            serde_json::Value::Null
        };
        assert!(
            reasons.iter().all(|reason| reason == &expected),
            "step {step}: {reasons:?}"
        );
    }
    assert!(matches!(
        world
            .engine()
            .finish_accepted_environment_stop(accepted.unwrap())
            .await,
        Err(StopError::Unavailable(_))
    ));
}
