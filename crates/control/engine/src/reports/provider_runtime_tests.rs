//! Ordinary node reports must distinguish the serving provider unit from its unchecked successor.
//! Architecture: docs/development/architecture.md keeps the old unit until new checks pass;
//! jobs/recreate.rs keeps its identity current until the publication transaction.
use super::*;
use crate::{EngineParts, test_common::World};
use orch_db::queries::environments::{changes, hosting as runtime_rows};
use orch_db::queries::resources::{provider_runtime as provider_rows, testing::insert_machine};
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
        let mut tx = self.world.pool.begin().await?;
        let mut unit = Unit::new(&mut tx, self.world.clock.now());
        self.engine
            .on_node_runtime(
                &mut unit,
                self.host,
                &NodeRuntime {
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
