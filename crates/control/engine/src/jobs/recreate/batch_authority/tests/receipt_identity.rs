//! A real accepted native frame is not proof that another phase owns its receipt.
//!
//! Only the first status observation is a simulated pre-ACK snapshot. Actual Accepted,
//! retained frames and outcomes come from the fixture's ordinary scripted Node link.
use super::*;
use crate::commands::{Commands, CommandsError, Info, Outcome, RetainedCommand, Stage};
use async_trait::async_trait;
use orch_link_contract::commands::{Cmd, Command};
use orch_testkit::doubles::machine::ScriptedMachine;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct SnapshotCommands {
    inner: Arc<dyn Commands>,
    id: Id,
    queued_snapshot: AtomicBool,
    snapshots: AtomicUsize,
    status_reads: AtomicUsize,
    retained_reads: AtomicUsize,
    holder: Arc<ScriptedMachine>,
}

#[async_trait]
impl Commands for SnapshotCommands {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        let replay = command.id == self.id;
        self.inner.issue(command).await?;
        // No second-status dependency: both early-Accepted and the ACK-race path
        // release when the runner elects to consume this already retained frame.
        if replay {
            self.holder.handler.release();
        }
        Ok(())
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        self.inner.retained_body(id).await
    }
    async fn retained_command(&self, id: Id) -> Result<Option<RetainedCommand>, CommandsError> {
        if id == self.id {
            self.retained_reads.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.retained_command(id).await
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        let mut actual = self.inner.status(id).await?;
        if id == self.id {
            self.status_reads.fetch_add(1, Ordering::SeqCst);
            if self.queued_snapshot.swap(false, Ordering::SeqCst) {
                let Some(info) = actual.as_mut() else {
                    return Err(CommandsError::Other(
                        "the observed real frame disappeared".into(),
                    ));
                };
                if info.stage != Stage::Accepted {
                    return Err(CommandsError::Other(
                        "the real accepted fixture changed before its pre-ACK snapshot".into(),
                    ));
                }
                info.stage = Stage::Queued;
                info.outcome = None;
                self.snapshots.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(actual)
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.inner.wait(id).await
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.cancel(id).await
    }
    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.stopped(id).await
    }
}

#[derive(Clone, Copy, Debug)]
enum Difference {
    Body,
    Recipient,
    Journal,
    SerialKey,
    Exact,
}

struct Accepted {
    expected: Outgoing,
    inner: Arc<dyn Commands>,
    wrapper: Arc<SnapshotCommands>,
    actual_machine: Id,
    frame: Cmd,
    journal: Json,
}

async fn accepted(
    s: &Fixture,
    difference: Difference,
    queued_snapshot: bool,
) -> TestResult<Accepted> {
    let expected = s.image()?;
    s.reserve(&expected).await?;
    let mut actual = expected.clone();
    let holder = if matches!(difference, Difference::Recipient) {
        actual.machine = s.old_host.id;
        Arc::clone(&s.old_host.machine)
    } else {
        Arc::clone(&s.target_host.machine)
    };
    match difference {
        Difference::Body => actual.body["spec"]["image"] = json!("foreign-accepted-image"),
        Difference::SerialKey => actual.key = SerialKey::Image("foreign-serial-lane".into()),
        Difference::Journal => {
            let row = s.job.row().await?;
            let mut values = row.plan[COMMANDS].clone();
            values[expected.id.to_string()]["body_hash"] =
                json!(Hash::of(b"another phase's journal").to_string());
            let mut tx = s.world.pool.begin().await?;
            jobs::merge_plan(
                &mut tx,
                s.job.id(),
                &json!({COMMANDS:values}),
                s.world.clock.now(),
            )
            .await?;
            tx.commit().await?;
        }
        _ => {}
    }
    holder.handler.hold(CommandKind::NodeImageEnsure);
    holder.handler.on(CommandKind::NodeImageEnsure, |command| {
        let Command::NodeImageEnsure(body) = command else {
            unreachable!("registered image handler");
        };
        Ok(CommandResult::NodeImageEnsure(NodeImageEnsureResult {
            digest: Some(format!(
                "retained:{}",
                body.spec["image"].as_str().unwrap_or("unnamed")
            )),
            built: None,
            programs: std::collections::BTreeMap::new(),
        }))
    });
    let inner = s.world.commands.as_engine();
    inner.issue(actual.clone()).await?;
    let id = expected.id;
    // This existing state-predicate wait proves a real native acknowledgement. The
    // synthetic snapshot below never manufactures Accepted/Over or an outcome.
    crate::test_common::eventually("the actual Node image frame is Accepted", || {
        let inner = Arc::clone(&inner);
        async move {
            inner
                .status(id)
                .await
                .is_ok_and(|info| info.is_some_and(|info| info.stage == Stage::Accepted))
        }
    })
    .await;
    holder.handler.started(expected.id).await;
    assert_eq!(
        inner
            .status(expected.id)
            .await?
            .ok_or("accepted frame absent")?
            .stage,
        Stage::Accepted
    );
    let (actual_machine, frame) = s
        .world
        .commands
        .issued_commands("node.image.ensure")
        .into_iter()
        .find(|(_, frame)| frame.id == expected.id)
        .ok_or("issued image frame absent")?;
    match difference {
        Difference::Body => assert_ne!(frame.body, expected.body),
        Difference::Recipient => assert_ne!(actual_machine, expected.machine),
        Difference::SerialKey => assert_ne!(frame.key, expected.key.wire()),
        _ => {}
    }
    let journal = s.job.row().await?.plan[COMMANDS].clone();
    assert!(
        s.current().await?,
        "identity cases keep original keys and current account rights valid"
    );
    let wrapper = Arc::new(SnapshotCommands {
        inner: Arc::clone(&inner),
        id: expected.id,
        queued_snapshot: AtomicBool::new(queued_snapshot),
        snapshots: AtomicUsize::new(0),
        status_reads: AtomicUsize::new(0),
        retained_reads: AtomicUsize::new(0),
        holder,
    });
    Ok(Accepted {
        expected,
        inner,
        wrapper,
        actual_machine,
        frame,
        journal,
    })
}

async fn wrapped_job(s: &Fixture, wrapper: Arc<SnapshotCommands>) -> TestResult<(Engine, Job)> {
    let row = s.job.row().await?;
    let runner: Id = serde_json::from_value(row.plan["runner"]["id"].clone())?;
    let engine = Engine::new(EngineParts {
        db: s.world.pool.clone(),
        clock: s.world.clock.clone(),
        config: crate::test_common::config(),
        commands: wrapper,
        access: s.world.access.clone(),
        live: s.world.live.clone(),
    });
    let job = Job::claimed(engine.clone(), s.job.id(), runner);
    Ok((engine, job))
}

fn assert_same_frame(before: &Accepted, after: &(Id, Cmd)) {
    assert_eq!(after.0, before.actual_machine);
    assert_eq!(after.1.id, before.frame.id);
    assert_eq!(after.1.kind, before.frame.kind);
    assert_eq!(after.1.key, before.frame.key);
    assert_eq!(after.1.body, before.frame.body);
    assert_eq!(
        after.1.deadline, before.frame.deadline,
        "the retained deadline is not renewed"
    );
}

async fn refuses(difference: Difference, queued_snapshot: bool) -> TestResult {
    let s = Fixture::new().await?;
    let before = accepted(&s, difference, queued_snapshot).await?;
    let (engine, job) = wrapped_job(&s, Arc::clone(&before.wrapper)).await?;
    let result = job.command(before.expected.clone()).await;
    // A corrected runner can refuse before issue/release. Always release and drain
    // the real foreign frame so the negative control leaves no held work behind.
    before.wrapper.holder.handler.release();
    let native = before.inner.wait(before.expected.id).await?;
    let actual = s
        .world
        .commands
        .issued_commands("node.image.ensure")
        .into_iter()
        .find(|(_, frame)| frame.id == before.expected.id)
        .ok_or("retained image frame absent")?;
    assert_same_frame(&before, &actual);
    assert_eq!(before.wrapper.holder.handler.runs_of(before.expected.id), 1);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 1);
    assert_eq!(
        s.job.row().await?.plan[COMMANDS],
        before.journal,
        "a refusal must not rewrite the journal"
    );
    assert!(
        matches!(&native, Outcome::Succeeded(_)),
        "the actual native execution succeeded: {native:?}"
    );
    assert_eq!(
        before.wrapper.snapshots.load(Ordering::SeqCst),
        if queued_snapshot { 1 } else { 0 }
    );
    engine.stop();
    s.engine.stop();
    assert!(
        matches!(&result,Err(Stop::Failed(failure)) if failure.code=="access.denied"),
        "{difference:?}, queued_snapshot={queued_snapshot}: expected canonical authority refusal; job result={result:?}; real native result={native:?}; status_reads={}; retained_reads={}; actual recipient={}; actual key={:?}",
        before.wrapper.status_reads.load(Ordering::SeqCst),
        before.wrapper.retained_reads.load(Ordering::SeqCst),
        before.actual_machine,
        before.frame.key
    );
    Ok(())
}

#[tokio::test]
async fn ack_during_guard_cannot_swallow_a_foreign_retained_body() -> TestResult {
    refuses(Difference::Body, true).await
}

#[tokio::test]
async fn ack_during_guard_cannot_swallow_a_foreign_recipient() -> TestResult {
    refuses(Difference::Recipient, true).await
}

#[tokio::test]
async fn ack_during_guard_cannot_swallow_another_journal_identity() -> TestResult {
    refuses(Difference::Journal, true).await
}

#[tokio::test]
async fn initial_accepted_status_is_not_phase_receipt_identity_proof() -> TestResult {
    refuses(Difference::Body, false).await
}

#[tokio::test]
async fn actual_serial_key_is_a_retained_fact_not_the_reconstructed_request_key() -> TestResult {
    // Unlike body/recipient/journal mismatch, the old RetainedCommand carries no
    // actual key. This control diagnoses incomplete frame facts, not only the catch.
    refuses(Difference::SerialKey, true).await
}

#[tokio::test]
async fn exact_accepted_and_terminal_replay_still_reconcile_after_revocation() -> TestResult {
    let s = Fixture::new().await?;
    let before = accepted(&s, Difference::Exact, true).await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.keys[1], s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(!s.current().await?);
    let (engine, job) = wrapped_job(&s, Arc::clone(&before.wrapper)).await?;
    let first = job.command(before.expected.clone()).await;
    before.wrapper.holder.handler.release();
    let native = before.inner.wait(before.expected.id).await?;
    let Outcome::Succeeded(actual) = native else {
        return Err("the real exact frame did not succeed".into());
    };
    assert_eq!(
        first.as_ref().map_err(|error| format!("{error:?}")),
        Ok(&actual)
    );
    assert_eq!(actual["digest"], json!("retained:provider-batch-new"));
    assert_eq!(
        job.command(before.expected.clone())
            .await
            .map_err(|error| format!("{error:?}"))?,
        actual
    );
    assert_eq!(before.wrapper.snapshots.load(Ordering::SeqCst), 1);
    assert_eq!(before.wrapper.holder.handler.runs_of(before.expected.id), 1);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 1);
    let after = s
        .world
        .commands
        .issued_commands("node.image.ensure")
        .into_iter()
        .find(|(_, frame)| frame.id == before.expected.id)
        .ok_or("exact retained frame absent")?;
    assert_same_frame(&before, &after);
    assert_eq!(s.job.row().await?.plan[COMMANDS], before.journal);
    assert!(
        matches!(job.command(s.old_stop()?).await,Err(Stop::Failed(ref failure)) if failure.code=="access.denied"),
        "the exact existing receipt cannot authorize a later new native identity"
    );
    assert_eq!(s.world.commands.issued_of("node.runtime.stop"), 0);
    engine.stop();
    s.engine.stop();
    Ok(())
}
