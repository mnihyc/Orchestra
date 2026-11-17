//! Ordinary node reports must distinguish the serving provider unit from its unchecked successor.
//! Architecture: docs/development/architecture.md keeps the old unit until new checks pass;
//! jobs/recreate.rs keeps its identity current until the publication transaction.
use super::*;
use crate::{EngineParts, test_common::World};
use orch_db::queries::environments::{changes, hosting as runtime_rows};
use orch_db::queries::resources::{provider_runtime as provider_rows, testing::insert_machine};
use orch_link_contract::native_target::RuntimeSource;
use orch_types::new_id;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: World,
    engine: Engine,
    host: Id,
    old: Id,
    candidate: Id,
    candidate_machine: Id,
}

impl Fixture {
    async fn new(adopt_candidate: bool) -> TestResult<Self> {
        let world = World::observed().await;
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: crate::test_common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let (host, template, old, candidate) = (new_id(), new_id(), new_id(), new_id());
        let mut tx = world.pool.begin().await?;
        // These are ordinary stored node/connector identities; no native command or batch
        // job is needed to apply a host's existing runtime-report handler.
        let host_machine = insert_machine(&mut tx, "Runtime report node", "connected").await?;
        let candidate_machine = if adopt_candidate {
            insert_machine(&mut tx, "Runtime report successor", "connected").await?
        } else {
            world.machine_id
        };
        assert_ne!(host_machine, world.machine_id);
        if adopt_candidate {
            assert_ne!(candidate_machine, world.machine_id);
        }
        assert_ne!(host_machine, candidate_machine);
        sqlx::query(
            "INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,'Provider runtime report host')",
        )
        .bind(host)
        .bind(host_machine)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO env_templates(id,name,kind,driver,spec) VALUES($1,'Provider runtime report template','provider','test','{}')")
            .bind(template).execute(&mut *tx).await?;
        provider_rows::insert_runtime(
            &mut tx,
            &provider_rows::NewRuntime {
                id: old,
                host_id: host,
                unit_name: "current-provider-report-unit",
                provider_id: world.provider,
                driver: "test",
                template_id: template,
                spec_hash: &[0; 32],
                generation: 1,
                volumes: &json!({"state":{"name":"provider-report-state","storage":null}}),
            },
            world.clock.now(),
        )
        .await?;
        assert!(hosts::adopt_machine(&mut tx, world.machine_id, old, world.clock.now()).await?);
        assert!(
            runtime_rows::set_runtime_state(
                &mut tx,
                old,
                &["creating"],
                "running",
                None,
                world.clock.now()
            )
            .await?
        );
        let current = runtime_rows::runtime(&mut tx, old)
            .await?
            .ok_or("current runtime absent")?;
        changes::successor(
            &mut tx,
            &current,
            candidate,
            "candidate-provider-report-unit",
            template,
            &[1; 32],
            world.clock.now(),
        )
        .await?;
        if adopt_candidate {
            assert!(
                hosts::adopt_machine(&mut tx, candidate_machine, candidate, world.clock.now())
                    .await?
            );
        }
        assert!(
            provider_rows::set_machine(
                &mut tx,
                world.provider,
                Some(world.machine_id),
                world.clock.now()
            )
            .await?
                || provider_rows::state(&mut tx, world.provider)
                    .await?
                    .is_some_and(|row| row.machine_id == Some(world.machine_id))
        );
        tx.commit().await?;
        Ok(Self {
            world,
            engine,
            host,
            old,
            candidate,
            candidate_machine,
        })
    }

    async fn failed_report(&self, runtime: Id) -> TestResult {
        self.report(runtime, "failed").await
    }

    async fn report(&self, runtime: Id, state: &str) -> TestResult {
        self.report_from(runtime, state, RuntimeSource::untracked())
            .await
    }

    async fn report_from(&self, runtime: Id, state: &str, source: RuntimeSource) -> TestResult {
        let mut tx = self.world.pool.begin().await?;
        let mut unit = Unit::new(&mut tx, self.world.clock.now());
        self.engine
            .on_node_runtime(
                &mut unit,
                self.host,
                &NodeRuntime {
                    source,
                    runtime_id: runtime,
                    state: state.into(),
                    detail: Some("synthetic provider unit failure".into()),
                },
            )
            .await?;
        unit.finish().await?;
        tx.commit().await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_host_that_confirms_a_provider_stop_finishes_it_without_waiting_for_the_command_reply()
-> TestResult {
    for state in ["absent", "stopped", "failed"] {
        let s = Fixture::new(true).await?;
        let reason = provider_rows::STOPPED_PERSON;
        sqlx::query("UPDATE runtimes SET state='stopping',state_detail=$2 WHERE id=$1")
            .bind(s.old)
            .bind(reason)
            .execute(s.world.pool.pool())
            .await?;
        sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
            .bind(s.world.provider)
            .execute(s.world.pool.pool())
            .await?;
        s.report(s.old, state).await?;
        s.report(s.old, state).await?;
        let mut conn = s.world.pool.acquire().await?;
        let stopped = runtime_rows::runtime(&mut conn, s.old)
            .await?
            .ok_or("runtime absent")?;
        assert_eq!(stopped.state, "stopped", "host report: {state}");
        assert_eq!(stopped.state_detail.as_deref(), Some(reason));
        let missing: bool = sqlx::query_scalar("SELECT loss_missing FROM runtimes WHERE id=$1")
            .bind(s.old)
            .fetch_one(&mut *conn)
            .await?;
        assert_eq!(missing, state == "absent");
        let provider = provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?;
        assert_eq!(provider.status, "disabled");
        assert_eq!(provider.machine_id, None);
        let candidate = runtime_rows::runtime(&mut conn, s.candidate)
            .await?
            .ok_or("candidate absent")?;
        assert_eq!(candidate.state, "creating");
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn a_stopped_successor_report_keeps_the_current_provider_and_its_connector() -> TestResult {
    let s = Fixture::new(false).await?;
    sqlx::query("UPDATE runtimes SET state='stopping' WHERE id=$1")
        .bind(s.candidate)
        .execute(s.world.pool.pool())
        .await?;
    s.report(s.candidate, "absent").await?;
    let mut conn = s.world.pool.acquire().await?;
    let candidate = runtime_rows::runtime(&mut conn, s.candidate)
        .await?
        .ok_or("candidate absent")?;
    assert_eq!(candidate.state, "stopped");
    let current = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("current absent")?;
    assert_eq!((current.id, current.state.as_str()), (s.old, "running"));
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        Some(s.world.machine_id)
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn failed_provider_successor_does_not_detach_the_current_serving_unit() -> TestResult {
    let s = Fixture::new(true).await?;
    s.failed_report(s.candidate).await?;
    let mut conn = s.world.pool.acquire().await?;
    let candidate = runtime_rows::runtime(&mut conn, s.candidate)
        .await?
        .ok_or("candidate absent")?;
    assert_eq!(candidate.state, "failed");
    assert_eq!(candidate.machine_id, Some(s.candidate_machine));
    assert_eq!(candidate.successor_of, Some(s.old));
    let current = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("current provider runtime absent")?;
    assert_eq!(current.id, s.old);
    assert_eq!(current.state, "running");
    assert_eq!(current.machine_id, Some(s.world.machine_id));
    let provider = provider_rows::state(&mut conn, s.world.provider)
        .await?
        .ok_or("provider absent")?;
    assert_eq!(
        provider.machine_id,
        Some(s.world.machine_id),
        "a failed unchecked successor must not clear the current provider's machine"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn failed_current_provider_unit_still_detaches_and_marks_its_runtime_failed() -> TestResult {
    let s = Fixture::new(true).await?;
    s.failed_report(s.old).await?;
    let mut conn = s.world.pool.acquire().await?;
    let current = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("current provider runtime absent")?;
    assert_eq!(current.id, s.old);
    assert_eq!(current.state, "failed");
    assert_eq!(
        current.state_detail.as_deref(),
        Some("synthetic provider unit failure")
    );
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        None,
        "a report for the serving unit must keep its existing unavailable behavior"
    );
    let candidate = runtime_rows::runtime(&mut conn, s.candidate)
        .await?
        .ok_or("candidate absent")?;
    assert_eq!(candidate.state, "creating");
    assert_eq!(candidate.successor_of, Some(s.old));
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn failed_unadopted_provider_successor_preserves_the_current_connector_and_sessions()
-> TestResult {
    let s = Fixture::new(false).await?;
    let mut conn = s.world.pool.acquire().await?;
    let candidate = runtime_rows::runtime(&mut conn, s.candidate)
        .await?
        .ok_or("candidate absent before its report")?;
    assert_eq!(candidate.state, "creating");
    assert_eq!(candidate.successor_of, Some(s.old));
    assert_eq!(
        candidate.machine_id,
        Some(s.world.machine_id),
        "ordinary successor creation inherits the old connector before adoption"
    );
    let current = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("current runtime absent before the report")?;
    assert_eq!(current.id, s.old);
    assert_eq!(current.state, "running");
    assert_eq!(current.machine_id, Some(s.world.machine_id));
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent before the report")?
            .machine_id,
        Some(s.world.machine_id)
    );
    drop(conn);

    // This is the ordinary current thread/segment and the observed process cache,
    // not a native opening or a replacement candidate adopting the old connector.
    let thread = s.world.thread("Current provider report session").await;
    let segment = s.world.segment_of(thread).await;
    let key = crate::harnesses::HarnessKey {
        provider: s.world.provider,
        agent: s.world.agent,
        machine: s.world.machine_id,
    };
    let mut conn = s.world.pool.acquire().await?;
    orch_db::queries::resources::thread_engine::set_harness_status(
        &mut conn,
        key.provider,
        key.agent,
        key.machine,
        &orch_db::queries::resources::thread_engine::HarnessReport {
            status: "online",
            detail: None,
            version: Some("1.0.0"),
            sessions_open: 1,
        },
        s.world.clock.now(),
    )
    .await?;
    let harness: (String, i32) = sqlx::query_as(
        "SELECT status,sessions_open FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3",
    )
    .bind(key.provider)
    .bind(key.agent)
    .bind(key.machine)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(harness, ("online".to_owned(), 1));
    drop(conn);
    crate::engine::lock(&s.engine.inner.sessions).opened(segment, thread, key);
    assert!(crate::engine::lock(&s.engine.inner.sessions).is_open(segment, key));

    s.failed_report(s.candidate).await?;
    let mut conn = s.world.pool.acquire().await?;
    let candidate = runtime_rows::runtime(&mut conn, s.candidate)
        .await?
        .ok_or("candidate absent after its report")?;
    assert_eq!(candidate.state, "failed");
    assert_eq!(candidate.successor_of, Some(s.old));
    assert_eq!(candidate.machine_id, Some(s.world.machine_id));
    let current = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("current runtime absent after the candidate failed")?;
    assert_eq!(current.id, s.old);
    assert_eq!(current.state, "running");
    assert_eq!(current.machine_id, Some(s.world.machine_id));
    let provider_machine = provider_rows::state(&mut conn, s.world.provider)
        .await?
        .ok_or("provider absent after the candidate failed")?
        .machine_id;
    let harness: (String, i32) = sqlx::query_as(
        "SELECT status,sessions_open FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3",
    )
    .bind(key.provider)
    .bind(key.agent)
    .bind(key.machine)
    .fetch_one(&mut *conn)
    .await?;
    let session_open = crate::engine::lock(&s.engine.inner.sessions).is_open(segment, key);
    assert_eq!(
        (provider_machine, harness, session_open),
        (Some(s.world.machine_id), ("online".to_owned(), 1), true),
        "an unadopted successor must preserve the current provider connector, harness and observed session"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn late_reports_from_an_old_native_start_leave_its_replacement_serving() -> TestResult {
    let s = Fixture::new(true).await?;
    let first = RuntimeSource {
        incarnation: Some(new_id()),
    };
    let replacement = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", first).await?;
    s.report_from(s.old, "running", replacement).await?;
    for state in ["failed", "absent", "stopped"] {
        s.report_from(s.old, state, first).await?;
        let mut conn = s.world.pool.acquire().await?;
        let current = runtime_rows::runtime(&mut conn, s.old)
            .await?
            .ok_or("runtime absent")?;
        assert_eq!(current.state, "running", "late original report: {state}");
        let missing: bool = sqlx::query_scalar("SELECT loss_missing FROM runtimes WHERE id=$1")
            .bind(s.old)
            .fetch_one(&mut *conn)
            .await?;
        assert!(
            !missing,
            "an old absent report must not describe the replacement"
        );
        assert_eq!(
            provider_rows::state(&mut conn, s.world.provider)
                .await?
                .ok_or("provider absent")?
                .machine_id,
            Some(s.world.machine_id)
        );
    }
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_terminal_native_start_cannot_clear_its_absence_with_a_late_running_report() -> TestResult
{
    let s = Fixture::new(true).await?;
    let source = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", source).await?;
    sqlx::query("UPDATE runtimes SET state='stopping' WHERE id=$1")
        .bind(s.old)
        .execute(s.world.pool.pool())
        .await?;
    s.report_from(s.old, "absent", source).await?;
    s.report_from(s.old, "running", source).await?;
    let mut conn = s.world.pool.acquire().await?;
    let current = runtime_rows::runtime(&mut conn, s.old)
        .await?
        .ok_or("runtime absent")?;
    assert_eq!(current.state, "stopped");
    let missing: bool = sqlx::query_scalar("SELECT loss_missing FROM runtimes WHERE id=$1")
        .bind(s.old)
        .fetch_one(&mut *conn)
        .await?;
    assert!(
        missing,
        "a closed physical start cannot become present again"
    );
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        None
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn an_untracked_native_report_does_not_confirm_a_tracked_stop() -> TestResult {
    let s = Fixture::new(true).await?;
    let source = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", source).await?;
    sqlx::query("UPDATE runtimes SET state='stopping' WHERE id=$1")
        .bind(s.old)
        .execute(s.world.pool.pool())
        .await?;
    s.report(s.old, "absent").await?;
    let mut conn = s.world.pool.acquire().await?;
    let current = runtime_rows::runtime(&mut conn, s.old)
        .await?
        .ok_or("runtime absent")?;
    assert_eq!(current.state, "stopping");
    let missing: bool = sqlx::query_scalar("SELECT loss_missing FROM runtimes WHERE id=$1")
        .bind(s.old)
        .fetch_one(&mut *conn)
        .await?;
    assert!(!missing);
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        Some(s.world.machine_id)
    );
    drop(conn);
    s.report_from(s.old, "absent", source).await?;
    let mut conn = s.world.pool.acquire().await?;
    assert_eq!(
        runtime_rows::runtime(&mut conn, s.old)
            .await?
            .ok_or("runtime absent")?
            .state,
        "stopped"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_native_source_of_another_runtime_cannot_fail_the_serving_unit() -> TestResult {
    let s = Fixture::new(true).await?;
    let foreign = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.candidate, "running", foreign).await?;
    s.report_from(s.old, "failed", foreign).await?;
    let mut conn = s.world.pool.acquire().await?;
    let current = runtime_rows::runtime(&mut conn, s.old)
        .await?
        .ok_or("runtime absent")?;
    assert_eq!(current.state, "running");
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        Some(s.world.machine_id)
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_different_native_starts_terminal_report_cannot_complete_the_original_stop() -> TestResult
{
    let s = Fixture::new(true).await?;
    let original = RuntimeSource {
        incarnation: Some(new_id()),
    };
    let replacement = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", original).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let control = orch_db::queries::runtime_controls::accept(&mut tx, s.old, new_id()).await?;
    assert!(
        orch_db::queries::runtime_incarnations::begin_stop(
            &mut tx,
            s.old,
            &control,
            s.world.clock.now()
        )
        .await?
    );
    tx.commit().await?;
    s.report_from(s.old, "running", replacement).await?;
    s.report_from(s.old, "absent", replacement).await?;
    let mut conn = s.world.pool.acquire().await?;
    let current = runtime_rows::runtime(&mut conn, s.old)
        .await?
        .ok_or("runtime absent")?;
    assert_eq!(
        current.state, "stopping",
        "no terminal report confirmed the admitted original"
    );
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        Some(s.world.machine_id)
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_finished_original_stop_does_not_capture_the_next_native_starts_stop() -> TestResult {
    let s = Fixture::new(true).await?;
    let first = RuntimeSource {
        incarnation: Some(new_id()),
    };
    let replacement = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", first).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let control = orch_db::queries::runtime_controls::accept(&mut tx, s.old, new_id()).await?;
    assert!(
        orch_db::queries::runtime_incarnations::begin_stop(
            &mut tx,
            s.old,
            &control,
            s.world.clock.now()
        )
        .await?
    );
    tx.commit().await?;
    s.report_from(s.old, "stopped", first).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    assert!(
        runtime_rows::set_runtime_state(
            &mut tx,
            s.old,
            &["stopped"],
            "running",
            None,
            s.world.clock.now()
        )
        .await?
    );
    assert!(
        provider_rows::set_machine(
            &mut tx,
            s.world.provider,
            Some(s.world.machine_id),
            s.world.clock.now()
        )
        .await?
    );
    tx.commit().await?;
    s.report_from(s.old, "running", replacement).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    assert!(
        runtime_rows::set_runtime_state(
            &mut tx,
            s.old,
            &["running"],
            "stopping",
            None,
            s.world.clock.now()
        )
        .await?
    );
    tx.commit().await?;
    s.report_from(s.old, "stopped", replacement).await?;
    let mut conn = s.world.pool.acquire().await?;
    let current = runtime_rows::runtime(&mut conn, s.old)
        .await?
        .ok_or("runtime absent")?;
    assert_eq!(
        current.state, "stopped",
        "the completed first stop must not own a later job stop"
    );
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        None
    );
    let pending: bool =
        sqlx::query_scalar("SELECT native_stop IS NOT NULL FROM runtimes WHERE id=$1")
            .bind(s.old)
            .fetch_one(&mut *conn)
            .await?;
    assert!(
        !pending,
        "the original stop is retained only while it is pending"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_newer_independent_stop_keeps_an_old_restart_from_preparing_its_next_start() -> TestResult
{
    use orch_db::queries::runtime_controls as controls;
    let s = Fixture::new(true).await?;
    let source = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", source).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let restart = controls::accept(&mut tx, s.old, new_id()).await?;
    let stop = controls::phase(
        &mut tx,
        &restart,
        new_id(),
        &json!({"kind":"node.runtime.stop","source":source}),
    )
    .await?
    .ok_or("own stop was refused")?;
    let start = controls::phase(
        &mut tx,
        &restart,
        new_id(),
        &json!({"kind":"node.runtime.start","source":source}),
    )
    .await?
    .ok_or("own next phase was refused")?;
    assert!(
        start > stop,
        "one original operation retains its own ordered continuation"
    );
    let later = controls::accept(&mut tx, s.old, new_id()).await?;
    assert!(!controls::is_current(&mut tx, &restart).await?);
    assert!(controls::is_current(&mut tx, &later).await?);
    assert_eq!(
        controls::phase(
            &mut tx,
            &restart,
            new_id(),
            &json!({"kind":"node.runtime.start","source":source})
        )
        .await?,
        None,
        "the old operation must not allocate a newer order after supersession"
    );
    let stopped = controls::phase(
        &mut tx,
        &later,
        new_id(),
        &json!({"kind":"node.runtime.stop","source":source}),
    )
    .await?
    .ok_or("later stop absent")?;
    assert!(stopped > start);
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn retrying_an_original_intent_keeps_its_first_source_and_exact_phase_operands() -> TestResult
{
    use orch_db::queries::runtime_controls as controls;
    let s = Fixture::new(true).await?;
    let original = RuntimeSource {
        incarnation: Some(new_id()),
    };
    let replacement = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", original).await?;
    let id = new_id();
    let phase = new_id();
    let operands = json!({"kind":"node.runtime.stop","source":original});
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let first = controls::accept(&mut tx, s.old, id).await?;
    let order = controls::phase(&mut tx, &first, phase, &operands)
        .await?
        .ok_or("phase absent")?;
    tx.commit().await?;
    s.report_from(s.old, "running", replacement).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let retried = controls::accept(&mut tx, s.old, id).await?;
    assert_eq!(
        retried, first,
        "retry must not look up and substitute the replacement"
    );
    assert_eq!(retried.source(), original);
    assert_eq!(
        controls::phase(&mut tx, &retried, phase, &operands).await?,
        Some(order)
    );
    assert!(
        matches!(
            controls::phase(
                &mut tx,
                &retried,
                phase,
                &json!({"kind":"node.runtime.stop","source":replacement})
            )
            .await,
            Err(orch_db::DbError::Rule { .. })
        ),
        "the same phase ID cannot replace physical operands"
    );
    let later = controls::accept(&mut tx, s.old, new_id()).await?;
    assert_eq!(controls::accept(&mut tx, s.old, id).await?, first);
    assert!(
        controls::is_current(&mut tx, &later).await?,
        "retry never makes the retired intent current again"
    );
    assert!(!controls::is_current(&mut tx, &retried).await?);
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn an_original_runtime_control_never_follows_a_host_onto_its_new_node() -> TestResult {
    use orch_db::queries::runtime_controls as controls;
    let s = Fixture::new(true).await?;
    let source = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", source).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let old_node: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(s.host)
        .fetch_one(&mut *tx)
        .await?;
    let original_id = new_id();
    let original = controls::accept(&mut tx, s.old, original_id).await?;
    assert_eq!(original.host(), s.host);
    assert_eq!(original.machine(), old_node);
    let phase = new_id();
    let body = json!({"runtime_id":s.old});
    let operands =
        json!({"kind":"node.runtime.stop","runtime_id":s.old,"source":source,"body":body});
    let first = controls::phase(&mut tx, &original, phase, &operands)
        .await?
        .ok_or("original phase absent")?;
    let new_node = insert_machine(&mut tx, "The host's new node", "connected").await?;
    sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
        .bind(s.host)
        .bind(new_node)
        .execute(&mut *tx)
        .await?;
    let retried = controls::accept(&mut tx, s.old, original_id).await?;
    assert_eq!(retried, original, "a retry retains its original receiver");
    assert_eq!(retried.machine(), old_node);
    assert!(!controls::is_current(&mut tx, &original).await?);
    let target = orch_link_contract::native_target::NativeTarget::Lifecycle(
        orch_link_contract::native_target::LifecycleTarget {
            subject: orch_link_contract::native_target::LifecycleSubject::Runtime {
                runtime_id: s.old,
            },
            observed: source.incarnation,
            opening: None,
            revision: first,
        },
    );
    let key = orch_link_contract::commands::SerialKey::Runtime(s.old).wire();
    for receiver in [old_node, new_node] {
        assert!(
            matches!(
                orch_db::queries::link::insert_command(
                    &mut tx,
                    &orch_db::queries::link::NewCommand {
                        native_target: Some(&target),
                        id: phase,
                        machine_id: receiver,
                        kind: "node.runtime.stop",
                        body: &body,
                        serial_key: key.as_deref(),
                        deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
                    },
                    s.world.clock.now()
                )
                .await,
                Err(orch_db::DbError::Rule { .. })
            ),
            "a fresh old-phase frame cannot be redirected after receiver replacement"
        );
    }
    let unqueued: bool =
        sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM commands WHERE id=$1)")
            .bind(phase)
            .fetch_one(&mut *tx)
            .await?;
    assert!(unqueued);
    assert_eq!(
        controls::phase(&mut tx, &original, phase, &operands).await?,
        None
    );
    assert_eq!(
        controls::phase(&mut tx, &original, new_id(), &operands).await?,
        None,
        "old work cannot allocate a later order against the new receiver"
    );
    let fresh = controls::accept(&mut tx, s.old, new_id()).await?;
    assert_eq!(fresh.machine(), new_node);
    assert!(controls::is_current(&mut tx, &fresh).await?);
    assert!(
        controls::phase(&mut tx, &fresh, new_id(), &operands)
            .await?
            .ok_or("new receiver phase absent")?
            > first
    );
    let other_node = insert_machine(&mut tx, "A different physical Host", "connected").await?;
    let other_host = new_id();
    sqlx::query("INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,'Different runtime Host')")
        .bind(other_host)
        .bind(other_node)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE runtimes SET host_id=$2 WHERE id=$1")
        .bind(s.old)
        .bind(other_host)
        .execute(&mut *tx)
        .await?;
    assert!(!controls::is_current(&mut tx, &fresh).await?);
    assert_eq!(
        controls::phase(&mut tx, &fresh, new_id(), &operands).await?,
        None
    );
    assert!(
        matches!(
            controls::accept(&mut tx, s.old, new_id()).await,
            Err(orch_db::DbError::Rule { .. })
        ),
        "a different Host cannot inherit another Host's tracked physical source"
    );
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn separate_runtimes_keep_separate_orders_and_never_borrow_another_intent_or_phase()
-> TestResult {
    use orch_db::queries::runtime_controls as controls;
    let s = Fixture::new(true).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let id = new_id();
    let original = controls::accept(&mut tx, s.old, id).await?;
    assert!(matches!(
        controls::accept(&mut tx, s.candidate, id).await,
        Err(orch_db::DbError::Rule { .. })
    ));
    let candidate = controls::accept(&mut tx, s.candidate, new_id()).await?;
    let phase = new_id();
    let operands = json!({"kind":"node.runtime.stop"});
    let first = controls::phase(&mut tx, &original, phase, &operands).await?;
    assert!(matches!(
        controls::phase(&mut tx, &candidate, phase, &operands).await,
        Err(orch_db::DbError::Rule { .. })
    ));
    let second = controls::phase(&mut tx, &candidate, new_id(), &operands).await?;
    assert_eq!(
        first, second,
        "independent subjects have independent native order spaces"
    );
    assert!(controls::is_current(&mut tx, &original).await?);
    assert!(controls::is_current(&mut tx, &candidate).await?);
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn original_control_is_never_partially_accepted_outside_its_resource_transaction()
-> TestResult {
    let s = Fixture::new(true).await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(matches!(
        orch_db::queries::runtime_controls::accept(&mut conn, s.old, new_id()).await,
        Err(orch_db::DbError::Rule { .. })
    ));
    let order: i64 = sqlx::query_scalar("SELECT native_control_revision FROM runtimes WHERE id=$1")
        .bind(s.old)
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(order, 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn native_queue_admission_keeps_the_original_phase_target_and_rejects_substituted_subjects()
-> TestResult {
    use orch_db::queries::{link, runtime_controls};
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget, NativeTarget};
    let s = Fixture::new(true).await?;
    let source = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", source).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let control = runtime_controls::accept(&mut tx, s.old, new_id()).await?;
    assert!(
        orch_db::queries::runtime_incarnations::begin_stop(
            &mut tx,
            s.old,
            &control,
            s.world.clock.now()
        )
        .await?
    );
    let original = orch_db::queries::runtime_incarnations::stop(&mut tx, s.old)
        .await?
        .ok_or("pending original stop absent")?;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(s.host)
        .fetch_one(&mut *tx)
        .await?;
    let key = orch_link_contract::commands::SerialKey::Runtime(s.old).wire();
    let body = json!({"runtime_id":s.old});
    let target = LifecycleTarget {
        subject: LifecycleSubject::Runtime { runtime_id: s.old },
        observed: source.incarnation,
        opening: None,
        revision: original.revision,
    };
    let wrong = NativeTarget::Lifecycle(LifecycleTarget {
        subject: LifecycleSubject::Runtime {
            runtime_id: s.candidate,
        },
        ..target.clone()
    });
    let deadline = s.world.clock.now()
        + chrono::Duration::seconds(i64::from(
            orch_link_contract::commands::CommandKind::NodeRuntimeStop.deadline_secs(),
        ));
    assert!(matches!(
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                native_target: Some(&wrong),
                id: original.id,
                machine_id: machine,
                kind: "node.runtime.stop",
                body: &body,
                serial_key: key.as_deref(),
                deadline_at: deadline,
            },
            s.world.clock.now()
        )
        .await,
        Err(orch_db::DbError::Rule { .. })
    ));
    let absent: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM commands WHERE id=$1)")
        .bind(original.id)
        .fetch_one(&mut *tx)
        .await?;
    assert!(
        absent,
        "a rejected target must not poison the original command ID"
    );
    let valid = NativeTarget::Lifecycle(target.clone());
    assert!(matches!(
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                native_target: Some(&valid),
                id: original.id,
                machine_id: s.world.machine_id,
                kind: "node.runtime.stop",
                body: &body,
                serial_key: key.as_deref(),
                deadline_at: deadline,
            },
            s.world.clock.now()
        )
        .await,
        Err(orch_db::DbError::Rule { .. })
    ));
    assert!(
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                native_target: Some(&valid),
                id: original.id,
                machine_id: machine,
                kind: "node.runtime.stop",
                body: &body,
                serial_key: key.as_deref(),
                deadline_at: deadline,
            },
            s.world.clock.now()
        )
        .await?
    );
    let stored =
        orch_db::queries::session_dependencies::command_native_target(&mut tx, original.id)
            .await?
            .ok_or("original native target not retained")?;
    assert_eq!(stored, valid);
    assert!(
        !link::insert_command(
            &mut tx,
            &link::NewCommand {
                native_target: Some(&wrong),
                id: original.id,
                machine_id: machine,
                kind: "node.runtime.stop",
                body: &body,
                serial_key: key.as_deref(),
                deadline_at: deadline,
            },
            s.world.clock.now()
        )
        .await?,
        "canonical duplicate keeps its first facts"
    );
    assert_eq!(
        orch_db::queries::session_dependencies::command_native_target(&mut tx, original.id).await?,
        Some(NativeTarget::Lifecycle(target))
    );
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}
