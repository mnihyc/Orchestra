//! Ordinary node reports must distinguish the serving provider unit from its unchecked successor.
//! Architecture: docs/development/architecture.md keeps the old unit until new checks pass;
//! jobs/recreate.rs keeps its identity current until the publication transaction.
use super::*;
use crate::{EngineParts, test_common::World};
use orch_db::queries::environments::{changes, hosting as runtime_rows};
use orch_db::queries::resources::{provider_runtime as provider_rows, testing::insert_machine};
use orch_link_contract::native_target::RuntimeSource;
use orch_types::{Json, new_id};

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
            home: None,
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
async fn an_original_provider_stop_keeps_its_key_through_its_own_pending_connector_enrollment()
-> TestResult {
    use orch_db::queries::{link, runtime_controls as controls, runtime_incarnations as sources};
    use orch_link_contract::{commands::SerialKey, native_target::NativeTarget};
    let s = Fixture::new(true).await?;
    let original = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", original).await?;
    let (token, _, http) = provider_stop_key(&s).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let starting = controls::accept_job(&mut tx, s.old, new_id(), new_id()).await?;
    let command = new_id();
    let body = json!({"runtime_id":s.old});
    let target = controls::start_phase(&mut tx, &starting, command, original, &body)
        .await?
        .ok_or("the admitted pending Start is absent")?;
    let opening = target.opening.ok_or("the admitted opening is absent")?;
    let header = NativeTarget::Lifecycle(target);
    let key = SerialKey::Runtime(s.old).wire();
    assert!(
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                content_transit: false,
                native_target: Some(&header),
                id: command,
                machine_id: starting.machine(),
                kind: "node.runtime.start",
                body: &body,
                serial_key: key.as_deref(),
                deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
            },
            s.world.clock.now()
        )
        .await?
    );
    let control = controls::accept(&mut tx, s.old, new_id()).await?;
    assert_eq!(control.pending(), [opening]);
    assert!(sources::prepare_stop(&mut tx, s.old, &control, s.world.clock.now()).await?);
    let prepared = sources::stop(&mut tx, s.old)
        .await?
        .ok_or("prepared Stop missing")?;
    let command = http
        .scope(crate::commands::provider_unit::pending(
            &mut tx,
            s.world.clock.as_ref(),
            s.world.provider,
            &prepared,
        ))
        .await?
        .ok_or("the real original ProviderStop command is absent")?;
    assert!(
        sources::bind_provider_command(&mut tx, &prepared, &serde_json::to_value(&command)?)
            .await?
    );
    let enrolled =
        insert_machine(&mut tx, "The original pending unit's connector", "managed").await?;
    assert!(hosts::adopt_machine(&mut tx, enrolled, s.old, s.world.clock.now()).await?);
    provider_rows::set_machine(
        &mut tx,
        s.world.provider,
        Some(enrolled),
        s.world.clock.now(),
    )
    .await?;
    assert!(sources::begin_stop(&mut tx, s.old, &control, s.world.clock.now()).await?);
    tx.commit().await?;
    drop(http);
    let saved = sources::stop(&mut *s.world.pool.acquire().await?, s.old)
        .await?
        .ok_or("retained original missing")?;
    let restored = crate::commands::provider_unit::restore(s.world.provider, &saved)?;
    assert_eq!(
        restored, command,
        "enrollment cannot replace its original operation or key"
    );
    let authority = restored
        .authority
        .as_ref()
        .ok_or("original authority missing")?;
    let serial = restored.key.wire();
    let proposed = link::NewCommand {
        content_transit: restored.content_transit,
        native_target: restored.native_target.as_ref(),
        id: restored.id,
        machine_id: restored.machine,
        kind: restored.kind.as_str(),
        body: &restored.body,
        serial_key: serial.as_deref(),
        deadline_at: s.world.clock.now()
            + chrono::Duration::seconds(i64::from(restored.kind.deadline_secs())),
    };
    let mut checking = s.world.pool.begin().await?;
    assert!(
        sources::withdrawn(&mut checking, s.old).await?,
        "the pending unit's own connector stays in the withdrawn footprint"
    );
    assert!(
        authority
            .current_issue_callers(
                &mut checking,
                &proposed,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_some(),
        "its own enrollment cannot make the actual Stop-only key lose the original target"
    );
    checking.commit().await?;
    let unrelated = insert_machine(
        &mut *s.world.pool.acquire().await?,
        "An unrelated connector",
        "connected",
    )
    .await?;
    let mut changing = s.world.pool.begin().await?;
    provider_rows::lock(&mut changing, s.world.provider).await?;
    provider_rows::set_machine(
        &mut changing,
        s.world.provider,
        Some(unrelated),
        s.world.clock.now(),
    )
    .await?;
    changing.commit().await?;
    let mut checking = s.world.pool.begin().await?;
    assert!(
        !sources::withdrawn(&mut checking, s.old).await?,
        "the unrelated registration does not inherit the old withdrawal"
    );
    assert!(
        authority
            .current_issue_callers(
                &mut checking,
                &proposed,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_none(),
        "the enrollment exception cannot follow an unrelated connector"
    );
    checking.commit().await?;
    let mut changing = s.world.pool.begin().await?;
    provider_rows::lock(&mut changing, s.world.provider).await?;
    provider_rows::set_machine(
        &mut changing,
        s.world.provider,
        Some(enrolled),
        s.world.clock.now(),
    )
    .await?;
    changing.commit().await?;
    sqlx::query("UPDATE machines SET status='revoked',online=false WHERE id=$1")
        .bind(enrolled)
        .execute(s.world.pool.pool())
        .await?;
    let mut checking = s.world.pool.begin().await?;
    assert!(
        sources::withdrawn(&mut checking, s.old).await?,
        "revoking an owned connector cannot reopen the original pending unit"
    );
    assert!(
        authority
            .current_issue_callers(
                &mut checking,
                &proposed,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_some(),
        "the original Host still owns stopping this unit after its connector ends"
    );
    checking.commit().await?;
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(token)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let mut checking = s.world.pool.begin().await?;
    assert!(
        authority
            .current_issue_callers(
                &mut checking,
                &proposed,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_none(),
        "the enrollment exception grants no new credential lifetime"
    );
    checking.commit().await?;
    assert!(
        s.world
            .commands
            .issued_commands("node.runtime.stop")
            .is_empty()
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_persisted_provider_stop_restores_its_original_key_and_allows_a_fresh_request_after_that_key_ends()
-> TestResult {
    let s = Fixture::new(true).await?;
    s.report_from(
        s.old,
        "running",
        RuntimeSource {
            incarnation: Some(new_id()),
        },
    )
    .await?;
    let (token, _, http) = provider_stop_key(&s).await?;
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
    let pending = orch_db::queries::runtime_incarnations::stop(&mut tx, s.old)
        .await?
        .ok_or("pending Stop absent")?;
    let command = http
        .scope(crate::commands::provider_unit::pending(
            &mut tx,
            s.world.clock.as_ref(),
            s.world.provider,
            &pending,
        ))
        .await?
        .ok_or("the actual Provider Stop has no native command")?;
    assert!(
        orch_db::queries::runtime_incarnations::bind_provider_command(
            &mut tx,
            &pending,
            &serde_json::to_value(&command)?,
        )
        .await?
    );
    tx.commit().await?;
    drop(http);
    let saved =
        orch_db::queries::runtime_incarnations::stop(&mut *s.world.pool.acquire().await?, s.old)
            .await?
            .ok_or("committed Stop absent")?;
    let restored = crate::commands::provider_unit::restore(s.world.provider, &saved)?;
    assert_eq!(
        restored, command,
        "crash continuation keeps the existing typed command carrier"
    );
    assert!(
        !format!("{saved:?}").contains("credential"),
        "private continuation identity is never Debug output"
    );
    let authority = restored
        .authority
        .as_ref()
        .ok_or("original command authority absent")?;
    sqlx::query("UPDATE providers SET runtime_config=jsonb_set(runtime_config,'{runtime.variables}',$2) WHERE id=$1")
        .bind(s.world.provider).bind(json!({"NEXT_START_ONLY":"kept"}))
        .execute(s.world.pool.pool()).await?;
    let serial = restored.key.wire();
    let proposed = orch_db::queries::link::NewCommand {
        content_transit: restored.content_transit,
        native_target: restored.native_target.as_ref(),
        id: restored.id,
        machine_id: restored.machine,
        kind: restored.kind.as_str(),
        body: &restored.body,
        serial_key: serial.as_deref(),
        deadline_at: s.world.clock.now()
            + chrono::Duration::seconds(i64::from(restored.kind.deadline_secs())),
    };
    let mut checking = s.world.pool.begin().await?;
    let current = authority
        .current_issue_callers(
            &mut checking,
            &proposed,
            s.world.clock.as_ref(),
            s.world.access.as_ref(),
        )
        .await?;
    let current = current.ok_or(
        "the original Stop remains usable after an independent next-start variable change",
    )?;
    assert!(current.iter().any(|(caller, _)| {
        caller
            .as_ref()
            .is_some_and(|caller| caller.admitted_operation() == Some("ProviderStop"))
    }));
    checking.commit().await?;
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(token)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let mut checking = s.world.pool.begin().await?;
    assert!(
        authority
            .current_issue_callers(
                &mut checking,
                &proposed,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_none(),
        "restoration never turns an ended key into an ordinary caller"
    );
    checking.commit().await?;
    assert!(
        s.world
            .commands
            .issued_commands("node.runtime.stop")
            .is_empty()
    );
    let mut refused = s.world.pool.begin().await?;
    provider_rows::lock(&mut refused, s.world.provider).await?;
    assert!(
        orch_db::queries::runtime_incarnations::refuse_provider_command(&mut refused, &saved)
            .await?
    );
    refused.commit().await?;
    let (_, principal, fresh) = provider_stop_key(&s).await?;
    let tx = s.world.pool.begin().await?;
    let accepted = fresh
        .scope(s.engine.accept_provider_stop_in(
            tx,
            &principal,
            &orch_db::audit::Origin::default(),
            s.world.provider,
            false,
        ))
        .await?;
    assert!(
        !orch_db::queries::runtime_controls::is_current(
            &mut *s.world.pool.acquire().await?,
            &control
        )
        .await?,
        "a fresh authorized request accepts its own control after the original command was refused"
    );
    drop(accepted);
    let row = runtime_rows::runtime(&mut *s.world.pool.acquire().await?, s.old)
        .await?
        .ok_or("runtime absent")?;
    assert_eq!(
        row.state, "stopping",
        "authority loss alone confirms no physical outcome"
    );
    assert_eq!(
        row.volumes,
        json!({"state":{"name":"provider-report-state","storage":null}})
    );
    s.engine.stop();
    Ok(())
}

async fn provider_stop_key(
    s: &Fixture,
) -> TestResult<(
    Id,
    orch_authz::Principal,
    crate::operation_scope::http_origin::HttpOrigin,
)> {
    let token = new_id();
    let hash = orch_types::Hash::of(token.as_bytes());
    let expires = orch_types::Time::new(s.world.clock.now() + chrono::Duration::hours(1));
    let policy: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":[{
            "target":{"kind":"provider","id":s.world.provider,"reach":"self"},
            "allow":[{"operation":"ProviderStop","actions":["manage"]}]
        }]}))?;
    assert!(policy.validate().is_empty());
    let mut tx = s.world.pool.begin().await?;
    orch_db::queries::identity::create_api_token_scoped(
        &mut tx,
        &orch_db::queries::identity::NewApiToken {
            id: token,
            user_id: s.world.ada,
            name: "Original provider Stop origin",
            token_hash: hash.as_bytes(),
            scopes: &[],
            node_id: None,
            at: s.world.clock.now(),
            expires_at: Some(expires.get()),
        },
        Some(&json!(policy.clone())),
    )
    .await?;
    let caller = orch_authz::Caller::token(
        s.world.ada,
        orch_authz::TokenGrant::from_stored_scope(&[], None, Some(policy), Some(expires))?
            .for_token(token),
    );
    let caller = crate::commands::provider_unit::admit(&mut tx, &caller, s.world.provider).await?;
    let principal =
        orch_authz::Principal::load(&mut tx, caller.clone(), s.world.clock.time()).await?;
    let http = crate::operation_scope::http_origin::HttpOrigin::capture(
        orch_authz::CapturedOperation::capture(&caller)?,
        crate::operation_scope::request_origin::Credential::Token {
            id: token,
            hash: hash.as_bytes().to_vec(),
        },
        false,
        principal.has_system_admin_standing(),
        Some(expires),
    )?;
    tx.commit().await?;
    Ok((token, principal, http))
}

#[tokio::test]
async fn a_pending_provider_stop_with_lost_original_command_facts_never_falls_back_to_an_unbound_stop()
-> TestResult {
    for removed in ["producer", "command", "both"] {
        let s = Fixture::new(true).await?;
        let source = RuntimeSource {
            incarnation: Some(new_id()),
        };
        s.report_from(s.old, "running", source).await?;
        let mut tx = s.world.pool.begin().await?;
        provider_rows::lock(&mut tx, s.world.provider).await?;
        let control = orch_db::queries::runtime_controls::accept(&mut tx, s.old, new_id()).await?;
        assert!(
            orch_db::queries::runtime_incarnations::begin_stop(
                &mut tx,
                s.old,
                &control,
                s.world.clock.now(),
            )
            .await?
        );
        let stripped = match removed {
            "producer" => "command",
            "command" => "producer",
            _ => "neither",
        };
        sqlx::query(
            "UPDATE runtimes SET native_stop = CASE $2
             WHEN 'command' THEN (native_stop - 'producer') || '{\"command\":{}}'::jsonb
             WHEN 'producer' THEN (native_stop - 'command') || '{\"producer\":\"provider_stop\"}'::jsonb
             ELSE native_stop - 'producer' - 'command' END WHERE id=$1",
        ).bind(s.old).bind(stripped).execute(&mut *tx).await?;
        tx.commit().await?;
        let stored = orch_db::queries::runtime_incarnations::stop(
            &mut *s.world.pool.acquire().await?,
            s.old,
        )
        .await;
        assert!(
            stored.is_err(),
            "missing {removed} cannot become an ordinary native Stop"
        );
        assert!(
            s.engine
                .finish_provider_stop(s.world.provider, s.old)
                .await
                .is_err()
        );
        assert!(
            s.world
                .commands
                .issued_commands("node.runtime.stop")
                .is_empty()
        );
        let row = runtime_rows::runtime(&mut *s.world.pool.acquire().await?, s.old)
            .await?
            .ok_or("runtime absent")?;
        assert_eq!(row.state, "stopping");
        assert_eq!(
            row.volumes,
            json!({"state":{"name":"provider-report-state","storage":null}})
        );
        s.engine.stop();
    }
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
async fn an_unqueued_start_placeholder_cannot_block_a_later_stop_or_supply_a_pending_identity()
-> TestResult {
    use orch_db::queries::runtime_controls as controls;
    let s = Fixture::new(true).await?;
    let original = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", original).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let earlier = controls::accept(&mut tx, s.old, new_id()).await?;
    let placeholder = new_id();
    controls::phase(
        &mut tx,
        &earlier,
        placeholder,
        &json!({"kind":"node.runtime.start","source":original}),
    )
    .await?
    .ok_or("the planned continuation is absent")?;
    assert!(
        orch_db::queries::link::command(&mut tx, placeholder)
            .await?
            .is_none()
    );
    let stop = controls::accept(&mut tx, s.old, new_id()).await?;
    assert_eq!(stop.source(), original);
    assert!(
        stop.pending().is_empty(),
        "an unqueued placeholder is no physical opening"
    );
    assert!(controls::is_current(&mut tx, &stop).await?);
    assert!(!controls::is_current(&mut tx, &earlier).await?);
    tx.commit().await?;
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
            pending: Vec::new(),
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
                        content_transit: false,
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
async fn a_stop_is_recorded_before_preparation_but_cannot_reach_native_work_until_it_is_ready()
-> TestResult {
    use orch_db::queries::{link, runtime_controls as controls, runtime_incarnations as sources};
    use orch_link_contract::{
        commands::SerialKey,
        native_target::{LifecycleSubject, LifecycleTarget, NativeTarget},
    };
    let s = Fixture::new(true).await?;
    let observed = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", observed).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let original = controls::accept(&mut tx, s.old, new_id()).await?;
    assert!(sources::prepare_stop(&mut tx, s.old, &original, s.world.clock.now()).await?);
    let preparing = sources::stop(&mut tx, s.old)
        .await?
        .ok_or("the accepted Stop is missing")?;
    assert!(!preparing.ready);
    assert_eq!(
        runtime_rows::runtime(&mut tx, s.old)
            .await?
            .ok_or("runtime missing")?
            .state,
        "running",
        "preparation is not a physical Stop observation"
    );
    let header = NativeTarget::Lifecycle(LifecycleTarget {
        subject: LifecycleSubject::Runtime { runtime_id: s.old },
        observed: observed.incarnation,
        opening: None,
        pending: original.pending().to_vec(),
        revision: preparing.revision,
    });
    let key = SerialKey::Runtime(s.old).wire();
    let body = json!({"runtime_id":s.old});
    let command = link::NewCommand {
        content_transit: false,
        native_target: Some(&header),
        id: preparing.id,
        machine_id: original.machine(),
        kind: "node.runtime.stop",
        body: &body,
        serial_key: key.as_deref(),
        deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
    };
    assert!(
        matches!(
            link::insert_command(&mut tx, &command, s.world.clock.now()).await,
            Err(orch_db::DbError::Rule { .. })
        ),
        "even an unbound producer cannot bypass required preparation"
    );
    assert!(link::command(&mut tx, preparing.id).await?.is_none());
    assert!(sources::begin_stop(&mut tx, s.old, &original, s.world.clock.now()).await?);
    let ready = sources::stop(&mut tx, s.old)
        .await?
        .ok_or("the ready Stop is missing")?;
    assert!(ready.ready);
    assert_eq!(
        (ready.id, ready.revision),
        (preparing.id, preparing.revision),
        "arming keeps the original native phase and command once"
    );
    assert_eq!(
        runtime_rows::runtime(&mut tx, s.old)
            .await?
            .ok_or("runtime missing")?
            .state,
        "stopping"
    );
    assert!(sources::prepare_stop(&mut tx, s.old, &original, s.world.clock.now()).await?);
    assert!(
        sources::stop(&mut tx, s.old)
            .await?
            .ok_or("the joined Stop is missing")?
            .ready,
        "a retry cannot make an armed original unready"
    );
    assert!(link::insert_command(&mut tx, &command, s.world.clock.now()).await?);
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_preparing_stop_survives_status_refresh_and_ends_with_its_original_native_report()
-> TestResult {
    use orch_db::queries::{runtime_controls as controls, runtime_incarnations as sources};
    let s = Fixture::new(true).await?;
    let source = RuntimeSource {
        incarnation: Some(new_id()),
    };
    s.report_from(s.old, "running", source).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let original = controls::accept(&mut tx, s.old, new_id()).await?;
    assert!(sources::prepare_stop(&mut tx, s.old, &original, s.world.clock.now()).await?);
    let preparing = sources::stop(&mut tx, s.old)
        .await?
        .ok_or("the admitted Stop must survive its preparation write")?;
    tx.commit().await?;
    s.report_from(s.old, "running", source).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    assert!(
        runtime_rows::set_runtime_state(
            &mut tx,
            s.old,
            &["running"],
            "running",
            Some("the original Stop is preparing"),
            s.world.clock.now(),
        )
        .await?
    );
    assert_eq!(
        sources::stop(&mut tx, s.old).await?,
        Some(preparing.clone()),
        "an unchanged physical state cannot erase the original preparation or its command"
    );
    assert!(sources::begin_stop(&mut tx, s.old, &original, s.world.clock.now()).await?);
    let ready = sources::stop(&mut tx, s.old)
        .await?
        .ok_or("the armed original must remain recorded")?;
    assert_eq!(
        (ready.id, ready.revision),
        (preparing.id, preparing.revision)
    );
    assert!(ready.ready);
    tx.commit().await?;
    s.report_from(s.old, "stopped", source).await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(sources::stop(&mut conn, s.old).await?.is_none());
    assert_eq!(
        runtime_rows::runtime(&mut conn, s.old)
            .await?
            .ok_or("the stopped original and its data remain")?
            .state,
        "stopped"
    );
    assert!(controls::is_current(&mut conn, &original).await?);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_native_queue_waits_for_the_original_runtime_before_locking_its_child_job() -> TestResult
{
    use orch_db::queries::{link, runtime_controls as controls};
    use orch_link_contract::{commands::SerialKey, native_target::NativeTarget};
    let s = Fixture::new(true).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let original = controls::accept_job(&mut tx, s.old, new_id(), new_id()).await?;
    let runtime = runtime_rows::runtime(&mut tx, s.old)
        .await?
        .ok_or("runtime missing")?;
    let command = new_id();
    let body = json!({"runtime_id":s.old,"kind":"provider","name":runtime.unit_name,"driver":runtime.driver});
    let header = NativeTarget::Lifecycle(
        controls::create_phase(&mut tx, &original, command, &body)
            .await?
            .ok_or("the birth phase is absent")?,
    );
    tx.commit().await?;
    let mut stopping = s.world.pool.begin().await?;
    provider_rows::lock(&mut stopping, s.world.provider).await?;
    let stop_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *stopping)
        .await?;
    let stop = controls::accept(&mut stopping, s.old, new_id()).await?;
    assert!(
        stop.pending().is_empty(),
        "the earlier birth was not yet queued"
    );
    let mut queuing = s.world.pool.begin().await?;
    let queue_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *queuing)
        .await?;
    {
        let preparing = crate::jobs::provision::lock_creator_command_source(&mut queuing, command);
        tokio::pin!(preparing);
        let proof = async {
            loop {
                let blocked: bool =
                    sqlx::query_scalar("SELECT pg_blocking_pids($1) @> ARRAY[$2]::integer[]")
                        .bind(queue_pid)
                        .bind(stop_pid)
                        .fetch_one(s.world.pool.pool())
                        .await?;
                if blocked {
                    return Ok::<(), sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        };
        tokio::select! {
            result=&mut preparing=>{result?;panic!("the actual dispatcher boundary must wait for the runtime before any child-job fence");},
            proof=tokio::time::timeout(crate::test_common::PATIENCE,proof)=>{proof.expect("the native queue must wait on the actual winning Stop transaction")?;},
        }
        stopping.commit().await?;
        preparing.await?;
    }
    let key = SerialKey::Runtime(s.old).wire();
    assert!(
        matches!(
            link::insert_command(
                &mut queuing,
                &link::NewCommand {
                    content_transit: false,
                    native_target: Some(&header),
                    id: command,
                    machine_id: original.machine(),
                    kind: "node.runtime.create",
                    body: &body,
                    serial_key: key.as_deref(),
                    deadline_at: s.world.clock.now() + chrono::Duration::minutes(1)
                },
                s.world.clock.now()
            )
            .await,
            Err(orch_db::DbError::Rule { .. })
        ),
        "the Stop winner prevents late first admission of the old birth"
    );
    assert!(link::command(&mut queuing, command).await?.is_none());
    queuing.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_stop_waits_for_native_admission_to_commit_before_freezing_its_pending_openings()
-> TestResult {
    use orch_db::queries::{link, runtime_controls as controls};
    use orch_link_contract::{commands::SerialKey, native_target::NativeTarget};
    for creating in [false, true] {
        let s = Fixture::new(true).await?;
        let mut tx = s.world.pool.begin().await?;
        provider_rows::lock(&mut tx, s.world.provider).await?;
        let source = if creating {
            RuntimeSource::untracked()
        } else {
            RuntimeSource {
                incarnation: Some(new_id()),
            }
        };
        if !creating {
            assert!(
                orch_db::queries::runtime_incarnations::observe(
                    &mut tx, s.host, s.old, source, false
                )
                .await?
            );
        }
        let original = controls::accept_job(&mut tx, s.old, new_id(), new_id()).await?;
        let runtime = runtime_rows::runtime(&mut tx, s.old)
            .await?
            .ok_or("runtime missing")?;
        let command = new_id();
        let kind = if creating {
            "node.runtime.create"
        } else {
            "node.runtime.start"
        };
        let body = if creating {
            json!({"runtime_id":s.old,"kind":"provider","name":runtime.unit_name,"driver":runtime.driver})
        } else {
            json!({"runtime_id":s.old})
        };
        let target = if creating {
            controls::create_phase(&mut tx, &original, command, &body).await?
        } else {
            controls::start_phase(&mut tx, &original, command, source, &body).await?
        }
        .ok_or("the admitted phase is absent")?;
        let opening = target.opening.ok_or("the admitted opening is absent")?;
        tx.commit().await?;
        let header = NativeTarget::Lifecycle(target);
        let key = SerialKey::Runtime(s.old).wire();
        let mut queuing = s.world.pool.begin().await?;
        let queue_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *queuing)
            .await?;
        assert!(
            link::insert_command(
                &mut queuing,
                &link::NewCommand {
                    content_transit: false,
                    native_target: Some(&header),
                    id: command,
                    machine_id: original.machine(),
                    kind,
                    body: &body,
                    serial_key: key.as_deref(),
                    deadline_at: s.world.clock.now() + chrono::Duration::minutes(1)
                },
                s.world.clock.now()
            )
            .await?
        );
        let mut stopping = s.world.pool.begin().await?;
        provider_rows::lock(&mut stopping, s.world.provider).await?;
        let stop_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *stopping)
            .await?;
        let accepted = {
            let accepting = controls::accept(&mut stopping, s.old, new_id());
            tokio::pin!(accepting);
            let proof = async {
                loop {
                    let blocked: bool =
                        sqlx::query_scalar("SELECT pg_blocking_pids($1) @> ARRAY[$2]::integer[]")
                            .bind(stop_pid)
                            .bind(queue_pid)
                            .fetch_one(s.world.pool.pool())
                            .await?;
                    if blocked {
                        return Ok::<(), sqlx::Error>(());
                    }
                    tokio::task::yield_now().await;
                }
            };
            tokio::select! {
                result=&mut accepting=>{result?;panic!("Stop cannot freeze its footprint before the real native admission commits");},
                proof=tokio::time::timeout(crate::test_common::PATIENCE,proof)=>{proof.expect("Stop must actually wait on the native queue transaction")?;},
            }
            queuing.commit().await?;
            accepting.await?
        };
        assert_eq!(accepted.source(), source);
        assert_eq!(
            accepted.pending(),
            [opening],
            "the committed original opening is part of Stop, for {kind}"
        );
        stopping.commit().await?;
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn a_stop_owns_an_admitted_birth_before_its_create_reply_without_publishing_the_old_job()
-> TestResult {
    use orch_db::queries::{link, runtime_controls as controls};
    use orch_link_contract::{
        commands::{NodeRuntimeCreateResult, SerialKey},
        native_target::NativeTarget,
    };
    let s = Fixture::new(true).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let original = controls::accept_job(&mut tx, s.candidate, new_id(), new_id()).await?;
    let runtime = runtime_rows::runtime(&mut tx, s.candidate)
        .await?
        .ok_or("the fresh runtime is absent")?;
    let command = new_id();
    let body = json!({"runtime_id":s.candidate,"kind":"provider","name":runtime.unit_name,"driver":runtime.driver});
    let target = controls::create_phase(&mut tx, &original, command, &body)
        .await?
        .ok_or("the birth phase is absent")?;
    assert_eq!(target.observed, None);
    assert!(target.pending.is_empty());
    let birth = target.opening.ok_or("the birth has no opening")?;
    assert_eq!(
        controls::create_phase(&mut tx, &original, command, &body).await?,
        Some(target.clone())
    );
    let header = NativeTarget::Lifecycle(target);
    let key = SerialKey::Runtime(s.candidate).wire();
    link::insert_command(
        &mut tx,
        &link::NewCommand {
            content_transit: false,
            native_target: Some(&header),
            id: command,
            machine_id: original.machine(),
            kind: "node.runtime.create",
            body: &body,
            serial_key: key.as_deref(),
            deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
        },
        s.world.clock.now(),
    )
    .await?;
    let stop = controls::accept(&mut tx, s.candidate, new_id()).await?;
    assert_eq!(stop.source(), RuntimeSource::untracked());
    assert_eq!(
        stop.pending(),
        [birth],
        "an admitted allocation can already have happened before its reply"
    );
    let result = json!(NodeRuntimeCreateResult {
        source: RuntimeSource {
            incarnation: Some(birth)
        },
        unit: orch_driver_contract::units::UnitRef {
            reference: "actual-pending-birth".into(),
            name: runtime.unit_name,
            runtime_id: s.candidate,
            kind: orch_driver_contract::runtime::RuntimeKind::Provider
        }
    });
    link::finish_command(
        &mut tx,
        command,
        &link::Ending::Succeeded(&result),
        s.world.clock.now(),
    )
    .await?;
    assert_eq!(
        controls::bind_created(&mut tx, &original, command, &result).await?,
        None
    );
    let saved: Option<Json> =
        sqlx::query_scalar("SELECT outcome FROM runtime_control_phases WHERE id=$1")
            .bind(command)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        saved,
        Some(result),
        "supersession retains factual creation history without publishing the old job"
    );
    assert_eq!(
        orch_db::queries::runtime_incarnations::current(&mut tx, s.candidate).await?,
        Some(RuntimeSource::untracked())
    );
    assert!(controls::is_current(&mut tx, &stop).await?);
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn canonical_native_creation_and_start_outcomes_own_their_sources_without_overwriting_a_later_one()
-> TestResult {
    use orch_db::queries::{link, runtime_controls as controls};
    use orch_link_contract::commands::{
        NodeRuntimeCreateResult, NodeRuntimeStartResult, SerialKey,
    };
    for reused in [false, true] {
        let s = Fixture::new(true).await?;
        let mut tx = s.world.pool.begin().await?;
        provider_rows::lock(&mut tx, s.world.provider).await?;
        let job = new_id();
        let control = controls::accept_job(&mut tx, s.old, new_id(), job).await?;
        assert_eq!(
            controls::for_job(&mut tx, job, s.old).await?,
            Some(control.clone())
        );
        let runtime = runtime_rows::runtime(&mut tx, s.old)
            .await?
            .ok_or("runtime absent")?;
        let command = new_id();
        let key = SerialKey::Runtime(s.old).wire();
        let body = json!({"runtime_id":s.old,"kind":"provider","name":runtime.unit_name,"driver":runtime.driver});
        let birth = controls::create_phase(&mut tx, &control, command, &body)
            .await?
            .ok_or("creation phase absent")?;
        let created = RuntimeSource {
            incarnation: birth.opening,
        };
        assert!(created.incarnation.is_some());
        let header = orch_link_contract::native_target::NativeTarget::Lifecycle(birth);
        assert!(
            link::insert_command(
                &mut tx,
                &link::NewCommand {
                    content_transit: false,
                    native_target: Some(&header),
                    id: command,
                    machine_id: control.machine(),
                    kind: "node.runtime.create",
                    body: &body,
                    serial_key: key.as_deref(),
                    deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
                },
                s.world.clock.now()
            )
            .await?
        );
        let result = json!(NodeRuntimeCreateResult {
            source: created,
            unit: orch_driver_contract::units::UnitRef {
                reference: "actual-canonical-unit".into(),
                name: runtime.unit_name,
                runtime_id: s.old,
                kind: orch_driver_contract::runtime::RuntimeKind::Provider
            }
        });
        assert!(
            link::finish_command(
                &mut tx,
                command,
                &link::Ending::Succeeded(&result),
                s.world.clock.now()
            )
            .await?
        );
        let mut foreign = result.clone();
        foreign["source"] = json!({"incarnation":new_id()});
        assert!(
            controls::bind_created(&mut tx, &control, command, &foreign)
                .await
                .is_err(),
            "a payload from a different native source cannot claim this creation"
        );
        assert_eq!(
            orch_db::queries::runtime_incarnations::current(&mut tx, s.old).await?,
            Some(RuntimeSource::untracked())
        );
        assert!(
            orch_db::queries::runtime_incarnations::observe(&mut tx, s.host, s.old, created, false)
                .await?
        );
        assert!(controls::awaiting_creation(&mut tx, &control).await?);
        assert!(
            !controls::owns_current_source(&mut tx, &control).await?,
            "an early report permits consuming the queued Create reply, never Start or cleanup ownership"
        );
        assert_eq!(
            controls::bind_created(&mut tx, &control, command, &result).await?,
            Some(created)
        );
        assert!(!controls::awaiting_creation(&mut tx, &control).await?);
        assert!(controls::owns_current_source(&mut tx, &control).await?);
        sqlx::query("UPDATE commands SET result=NULL WHERE id=$1")
            .bind(command)
            .execute(&mut *tx)
            .await?;
        assert_eq!(
            controls::bind_created(&mut tx, &control, command, &result).await?,
            Some(created),
            "the validated once-only outcome survives ordinary command-result archival"
        );
        let start = new_id();
        let start_body = json!({"runtime_id":s.old});
        let target = controls::start_phase(&mut tx, &control, start, created, &start_body)
            .await?
            .ok_or("original Start phase absent")?;
        assert_eq!(
            controls::start_phase(&mut tx, &control, start, created, &start_body).await?,
            Some(target.clone()),
            "retrying the original phase never invents another candidate"
        );
        assert_eq!(target.observed, created.incarnation);
        let proposed = RuntimeSource {
            incarnation: target.opening,
        };
        assert!(proposed.incarnation.is_some());
        assert_ne!(proposed, created);
        let opened = if reused { created } else { proposed };
        let header = orch_link_contract::native_target::NativeTarget::Lifecycle(target);
        assert!(
            link::insert_command(
                &mut tx,
                &link::NewCommand {
                    content_transit: false,
                    native_target: Some(&header),
                    id: start,
                    machine_id: control.machine(),
                    kind: "node.runtime.start",
                    body: &start_body,
                    serial_key: key.as_deref(),
                    deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
                },
                s.world.clock.now()
            )
            .await?
        );
        let started = json!(NodeRuntimeStartResult {
            source: opened,
            reused
        });
        assert!(
            link::finish_command(
                &mut tx,
                start,
                &link::Ending::Succeeded(&started),
                s.world.clock.now()
            )
            .await?
        );
        assert_eq!(
            controls::bind_started(&mut tx, &control, start, &started)
                .await?
                .map(|reply| reply.source),
            Some(opened)
        );
        assert_eq!(
            controls::bind_created(&mut tx, &control, command, &result).await?,
            Some(created),
            "a retry keeps original creation operands without rewinding its actual Start source"
        );
        assert_eq!(
            orch_db::queries::runtime_incarnations::current(&mut tx, s.old).await?,
            Some(opened)
        );
        if reused {
            assert!(
                !orch_db::queries::runtime_incarnations::observe(
                    &mut tx, s.host, s.old, proposed, false
                )
                .await?,
                "the actual reused result proves its proposed opening was unused"
            );
            assert_eq!(
                orch_db::queries::runtime_incarnations::current(&mut tx, s.old).await?,
                Some(opened)
            );
        }
        let later = RuntimeSource {
            incarnation: Some(new_id()),
        };
        assert!(
            orch_db::queries::runtime_incarnations::observe(&mut tx, s.host, s.old, later, false)
                .await?
        );
        assert_eq!(
            controls::bind_created(&mut tx, &control, command, &result).await?,
            None
        );
        assert_eq!(
            controls::bind_started(&mut tx, &control, start, &started).await?,
            None
        );
        assert_eq!(
            orch_db::queries::runtime_incarnations::current(&mut tx, s.old).await?,
            Some(later)
        );
        tx.commit().await?;
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn an_unused_or_superseded_pending_opening_cannot_be_promoted_by_a_later_host_report()
-> TestResult {
    use orch_db::queries::{link, runtime_controls as controls, runtime_incarnations as sources};
    use orch_link_contract::{
        commands::{NodeRuntimeStartResult, SerialKey},
        native_target::NativeTarget,
    };
    for reused in [false, true] {
        let s = Fixture::new(true).await?;
        let mut tx = s.world.pool.begin().await?;
        provider_rows::lock(&mut tx, s.world.provider).await?;
        let observed = RuntimeSource {
            incarnation: Some(new_id()),
        };
        assert!(sources::observe(&mut tx, s.host, s.old, observed, false).await?);
        let original = controls::accept_job(&mut tx, s.old, new_id(), new_id()).await?;
        let command = new_id();
        let body = json!({"runtime_id":s.old});
        let key = SerialKey::Runtime(s.old).wire();
        let target = controls::start_phase(&mut tx, &original, command, observed, &body)
            .await?
            .ok_or("original phase missing")?;
        let opening = RuntimeSource {
            incarnation: target.opening,
        };
        let header = NativeTarget::Lifecycle(target);
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                content_transit: false,
                native_target: Some(&header),
                id: command,
                machine_id: original.machine(),
                kind: "node.runtime.start",
                body: &body,
                serial_key: key.as_deref(),
                deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
            },
            s.world.clock.now(),
        )
        .await?;
        if reused {
            let result = json!(NodeRuntimeStartResult {
                source: observed,
                reused: true
            });
            link::finish_command(
                &mut tx,
                command,
                &link::Ending::Succeeded(&result),
                s.world.clock.now(),
            )
            .await?;
        } else {
            let unrelated = RuntimeSource {
                incarnation: Some(new_id()),
            };
            assert!(sources::observe(&mut tx, s.host, s.old, unrelated, false).await?);
            controls::accept(&mut tx, s.old, new_id()).await?;
        }
        let before = sources::current(&mut tx, s.old).await?;
        assert!(!sources::observe(&mut tx, s.host, s.old, opening, false).await?);
        assert_eq!(sources::current(&mut tx, s.old).await?, before);
        tx.commit().await?;
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn a_stop_waits_for_every_admitted_opening_before_a_terminal_report_completes_it()
-> TestResult {
    stop_reports_complete_all_openings("stopped").await
}

#[tokio::test]
async fn an_old_missing_report_cannot_declare_an_unresolved_opening_absent() -> TestResult {
    stop_reports_complete_all_openings("absent").await
}

async fn stop_reports_complete_all_openings(terminal_report: &str) -> TestResult {
    use orch_db::queries::{link, runtime_controls as controls, runtime_incarnations as sources};
    use orch_link_contract::{
        commands::{NodeRuntimeStartResult, SerialKey},
        native_target::NativeTarget,
    };
    for ready in [false, true] {
        for unused in [false, true] {
            let s = Fixture::new(true).await?;
            let observed = RuntimeSource {
                incarnation: Some(new_id()),
            };
            s.report_from(s.old, "running", observed).await?;
            let mut tx = s.world.pool.begin().await?;
            provider_rows::lock(&mut tx, s.world.provider).await?;
            let original = controls::accept_job(&mut tx, s.old, new_id(), new_id()).await?;
            let command = new_id();
            let body = json!({"runtime_id":s.old});
            let key = SerialKey::Runtime(s.old).wire();
            let target = controls::start_phase(&mut tx, &original, command, observed, &body)
                .await?
                .ok_or("the admitted Start is absent")?;
            let opening = RuntimeSource {
                incarnation: Some(target.opening.ok_or("the admitted opening is absent")?),
            };
            let header = NativeTarget::Lifecycle(target);
            assert!(
                link::insert_command(
                    &mut tx,
                    &link::NewCommand {
                        content_transit: false,
                        native_target: Some(&header),
                        id: command,
                        machine_id: original.machine(),
                        kind: "node.runtime.start",
                        body: &body,
                        serial_key: key.as_deref(),
                        deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
                    },
                    s.world.clock.now(),
                )
                .await?
            );
            let control = controls::accept(&mut tx, s.old, new_id()).await?;
            assert_eq!(control.source(), observed);
            assert_eq!(
                control.pending(),
                [opening.incarnation.ok_or("opening is tracked")?]
            );
            assert!(sources::prepare_stop(&mut tx, s.old, &control, s.world.clock.now()).await?);
            if ready {
                assert!(sources::begin_stop(&mut tx, s.old, &control, s.world.clock.now()).await?);
            }
            let pending = sources::stop(&mut tx, s.old)
                .await?
                .ok_or("the accepted original Stop is absent")?;
            let volumes = runtime_rows::runtime(&mut tx, s.old)
                .await?
                .ok_or("runtime is absent")?
                .volumes;
            tx.commit().await?;
            s.report_from(s.old, terminal_report, observed).await?;
            let mut tx = s.world.pool.begin().await?;
            provider_rows::lock(&mut tx, s.world.provider).await?;
            assert_eq!(
                sources::stop(&mut tx, s.old).await?,
                Some(pending),
                "one ended source cannot complete an unresolved opening; ready={ready}, unused={unused}"
            );
            let row = runtime_rows::runtime(&mut tx, s.old)
                .await?
                .ok_or("the original runtime and its data remain")?;
            assert_eq!(row.state, if ready { "stopping" } else { "running" });
            assert_eq!(row.volumes, volumes);
            assert!(
                !row.loss_missing,
                "one source cannot prove the whole footprint absent"
            );
            assert!(sources::terminal(&mut tx, s.old, observed).await?);
            assert_eq!(
                provider_rows::state(&mut tx, s.world.provider)
                    .await?
                    .ok_or("the original registration remains")?
                    .machine_id,
                Some(s.world.machine_id)
            );
            if unused {
                let result = json!(NodeRuntimeStartResult {
                    source: observed,
                    reused: true,
                });
                assert!(
                    link::finish_command(
                        &mut tx,
                        command,
                        &link::Ending::Succeeded(&result),
                        s.world.clock.now(),
                    )
                    .await?
                );
                assert!(
                    controls::bind_started(&mut tx, &original, command, &result)
                        .await?
                        .is_none()
                );
            }
            tx.commit().await?;
            let ended = if unused { observed } else { opening };
            s.report_from(s.old, terminal_report, ended).await?;
            let mut conn = s.world.pool.acquire().await?;
            assert!(sources::stop(&mut conn, s.old).await?.is_none());
            let row = runtime_rows::runtime(&mut conn, s.old)
                .await?
                .ok_or("completion keeps the original data")?;
            assert_eq!(row.state, "stopped");
            assert_eq!(row.loss_missing, terminal_report == "absent");
            assert_eq!(row.volumes, volumes);
            assert_eq!(sources::current(&mut conn, s.old).await?, Some(ended));
            assert_eq!(
                provider_rows::state(&mut conn, s.world.provider)
                    .await?
                    .ok_or("provider remains registered")?
                    .machine_id,
                None
            );
            s.engine.stop();
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_canonical_stop_publishes_its_actual_pending_source_but_never_a_later_unrelated_one()
-> TestResult {
    use orch_db::queries::{link, runtime_controls as controls, runtime_incarnations as sources};
    use orch_link_contract::{
        commands::{NodeRuntimeStopResult, SerialKey},
        native_target::{LifecycleSubject, LifecycleTarget, NativeTarget},
    };
    let s = Fixture::new(true).await?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::lock(&mut tx, s.world.provider).await?;
    let observed = RuntimeSource {
        incarnation: Some(new_id()),
    };
    assert!(sources::observe(&mut tx, s.host, s.old, observed, false).await?);
    let old = controls::accept_job(&mut tx, s.old, new_id(), new_id()).await?;
    let command = new_id();
    let body = json!({"runtime_id":s.old});
    let key = SerialKey::Runtime(s.old).wire();
    let target = controls::start_phase(&mut tx, &old, command, observed, &body)
        .await?
        .ok_or("pending Start phase missing")?;
    let opening = target.opening.ok_or("pending opening missing")?;
    let header = NativeTarget::Lifecycle(target);
    link::insert_command(
        &mut tx,
        &link::NewCommand {
            content_transit: false,
            native_target: Some(&header),
            id: command,
            machine_id: old.machine(),
            kind: "node.runtime.start",
            body: &body,
            serial_key: key.as_deref(),
            deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
        },
        s.world.clock.now(),
    )
    .await?;
    let accepted = controls::accept(&mut tx, s.old, new_id()).await?;
    assert_eq!(accepted.pending(), [opening]);
    assert!(sources::begin_stop(&mut tx, s.old, &accepted, s.world.clock.now()).await?);
    let stop = sources::stop(&mut tx, s.old)
        .await?
        .ok_or("original Stop missing")?;
    let header = NativeTarget::Lifecycle(LifecycleTarget {
        subject: LifecycleSubject::Runtime { runtime_id: s.old },
        observed: observed.incarnation,
        pending: accepted.pending().to_vec(),
        opening: None,
        revision: stop.revision,
    });
    link::insert_command(
        &mut tx,
        &link::NewCommand {
            content_transit: false,
            native_target: Some(&header),
            id: stop.id,
            machine_id: accepted.machine(),
            kind: "node.runtime.stop",
            body: &body,
            serial_key: key.as_deref(),
            deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
        },
        s.world.clock.now(),
    )
    .await?;
    let stopped = RuntimeSource {
        incarnation: Some(opening),
    };
    let result = json!(NodeRuntimeStopResult::Stopped { source: stopped });
    link::finish_command(
        &mut tx,
        stop.id,
        &link::Ending::Succeeded(&result),
        s.world.clock.now(),
    )
    .await?;
    assert_eq!(
        controls::bind_stopped(
            &mut tx,
            &accepted,
            stop.id,
            &result,
            Some(&stop.registration)
        )
        .await?,
        Some(NodeRuntimeStopResult::Stopped { source: stopped })
    );
    assert_eq!(sources::current(&mut tx, s.old).await?, Some(stopped));
    assert!(sources::terminal(&mut tx, s.old, stopped).await?);
    let later = RuntimeSource {
        incarnation: Some(new_id()),
    };
    assert!(sources::observe(&mut tx, s.host, s.old, later, false).await?);
    assert_eq!(
        controls::bind_stopped(
            &mut tx,
            &accepted,
            stop.id,
            &result,
            Some(&stop.registration)
        )
        .await?,
        None
    );
    assert_eq!(sources::current(&mut tx, s.old).await?, Some(later));
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_new_control_captures_only_its_original_admitted_pending_openings_and_never_recaptures_a_later_report()
-> TestResult {
    use orch_db::queries::{link, runtime_controls as controls};
    use orch_link_contract::{
        commands::{NodeRuntimeStartResult, SerialKey},
        native_target::NativeTarget,
    };
    for state in [
        "unqueued",
        "queued",
        "accepted_then_cancelled",
        "succeeded_reused",
        "other_kind",
    ] {
        let s = Fixture::new(true).await?;
        let mut tx = s.world.pool.begin().await?;
        provider_rows::lock(&mut tx, s.world.provider).await?;
        let observed = RuntimeSource {
            incarnation: Some(new_id()),
        };
        assert!(
            orch_db::queries::runtime_incarnations::observe(
                &mut tx, s.host, s.old, observed, false
            )
            .await?
        );
        let old = controls::accept_job(&mut tx, s.old, new_id(), new_id()).await?;
        let command = new_id();
        let body = json!({"runtime_id":s.old});
        let key = SerialKey::Runtime(s.old).wire();
        let target = controls::start_phase(&mut tx, &old, command, observed, &body)
            .await?
            .ok_or("original Start phase missing")?;
        let opening = target.opening.ok_or("original opening missing")?;
        if state == "other_kind" {
            let other = new_id();
            controls::phase(&mut tx,&old,other,&json!({"kind":"node.image.ensure","opening":opening,"source":observed,"body":body})).await?.ok_or("other phase missing")?;
            link::insert_command(
                &mut tx,
                &link::NewCommand {
                    content_transit: false,
                    native_target: None,
                    id: other,
                    machine_id: old.machine(),
                    kind: "node.image.ensure",
                    body: &body,
                    serial_key: key.as_deref(),
                    deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
                },
                s.world.clock.now(),
            )
            .await?;
        } else if state != "unqueued" {
            let header = NativeTarget::Lifecycle(target);
            link::insert_command(
                &mut tx,
                &link::NewCommand {
                    content_transit: false,
                    native_target: Some(&header),
                    id: command,
                    machine_id: old.machine(),
                    kind: "node.runtime.start",
                    body: &body,
                    serial_key: key.as_deref(),
                    deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
                },
                s.world.clock.now(),
            )
            .await?;
            if state == "accepted_then_cancelled" {
                assert!(link::mark_accepted(&mut tx, command, s.world.clock.now()).await?);
                link::finish_command(
                    &mut tx,
                    command,
                    &link::Ending::Cancelled,
                    s.world.clock.now(),
                )
                .await?;
            } else if state == "succeeded_reused" {
                let result = json!(NodeRuntimeStartResult {
                    source: observed,
                    reused: true
                });
                link::finish_command(
                    &mut tx,
                    command,
                    &link::Ending::Succeeded(&result),
                    s.world.clock.now(),
                )
                .await?;
            }
        }
        let id = new_id();
        let accepted = controls::accept(&mut tx, s.old, id).await?;
        let snapshot = serde_json::to_value(&accepted)?;
        let expected = if matches!(state, "queued" | "accepted_then_cancelled") {
            json!([opening])
        } else {
            json!([])
        };
        assert_eq!(snapshot["pending"], expected, "{state}");
        assert_eq!(accepted.source(), observed);
        let later = RuntimeSource {
            incarnation: Some(new_id()),
        };
        assert!(
            orch_db::queries::runtime_incarnations::observe(&mut tx, s.host, s.old, later, false)
                .await?
        );
        let fresh = controls::accept(&mut tx, s.old, new_id()).await?;
        assert_eq!(fresh.source(), later);
        assert_eq!(
            serde_json::to_value(&fresh)?["pending"],
            json!([]),
            "an unrelated report cannot inherit older candidates"
        );
        assert_eq!(
            controls::accept(&mut tx, s.old, id).await?,
            accepted,
            "retry retains the first snapshot instead of today's source"
        );
        assert!(!controls::is_current(&mut tx, &accepted).await?);
        tx.commit().await?;
        s.engine.stop();
    }
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
        pending: Vec::new(),
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
                content_transit: false,
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
                content_transit: false,
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
                content_transit: false,
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
                content_transit: false,
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
