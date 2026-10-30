use super::*;
use crate::journal::TestWrites;
use orch_link_contract::commands::{Empty, SessionClose};
use orch_link_contract::source::{FsRecord, FsRecordResult, RecordKind, RecordState};
use orch_types::{ManualClock, new_id};
use std::sync::atomic::AtomicUsize;
use tokio::sync::Semaphore;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PATIENCE: Duration = Duration::from_secs(30);

struct Latch {
    entered: Semaphore,
    released: Semaphore,
}
impl Latch {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            released: Semaphore::new(0),
        }
    }
    async fn pass(&self) {
        self.entered.add_permits(1);
        if let Ok(permit) = self.released.acquire().await {
            permit.forget();
        }
    }
    async fn entered(&self) -> TestResult {
        tokio::time::timeout(PATIENCE, self.entered.acquire())
            .await??
            .forget();
        Ok(())
    }
    fn release(&self) {
        self.released.add_permits(1);
    }
}

struct Hold(Arc<AtomicUsize>);
impl Drop for Hold {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct ReleaseWrites(Arc<TestWrites>);
impl Drop for ReleaseWrites {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[derive(Clone, Copy)]
enum Ending {
    Correct,
    WrongKind,
    Panic,
}

struct Handler {
    first: Id,
    capture: Arc<Latch>,
    upload: Arc<Latch>,
    dropped: Arc<AtomicUsize>,
    calls: AtomicUsize,
    trace: Arc<Mutex<Vec<(Id, &'static str)>>>,
    ending: Ending,
    defer_close: bool,
}

fn fallback() -> FsRecordResult {
    FsRecordResult {
        state: RecordState::Unavailable,
        tree_ref: None,
        repository_head: None,
        manifest_blob: None,
        files: 0,
        bytes: 0,
        expires_at: None,
        reason: Some("off".to_owned()),
    }
}

#[async_trait]
impl CommandHandler for Handler {
    async fn handle(&self, context: CommandContext, command: Command) -> CommandOutcome {
        lock(&self.trace).push((context.id, "ordinary"));
        match command {
            Command::SessionClose(_) => Ok(CommandResult::SessionClose(Empty {})),
            Command::FsRecord(_) => Ok(CommandResult::FsRecord(fallback())),
            _ => Err(CommandError::new(
                ErrorCode::Unsupported,
                "test role does not take this command",
            )),
        }
    }

    async fn begin(&self, context: CommandContext, command: Command) -> CommandExecution {
        if context.id != self.first
            || (!self.defer_close && command.kind() != CommandKind::FsRecord)
        {
            return CommandExecution::ready(self.handle(context, command).await);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        lock(&self.trace).push((context.id, "capture"));
        self.capture.pass().await;
        if context.cancel.is_cancelled() {
            return CommandExecution::ready(Err(CommandError::new(
                ErrorCode::Conflict,
                "cancelled during capture",
            )));
        }
        let upload = Arc::clone(&self.upload);
        let trace = Arc::clone(&self.trace);
        let ending = self.ending;
        let close = self.defer_close;
        CommandExecution::Captured {
            finish: Box::pin(async move {
                tokio::select! {
                    biased;
                    () = context.cancel.cancelled() => Err(CommandError::new(ErrorCode::Conflict, "cancelled during upload")),
                    () = upload.pass() => {
                        lock(&trace).push((context.id, "published"));
                        match ending {
                            Ending::Panic => panic!("synthetic publication panic"),
                            Ending::WrongKind => Ok(CommandResult::SessionClose(Empty {})),
                            Ending::Correct if close => Ok(CommandResult::SessionClose(Empty {})),
                            Ending::Correct => Ok(CommandResult::FsRecord(fallback())),
                        }
                    }
                }
            }),
            hold: Box::new(Hold(Arc::clone(&self.dropped))),
        }
    }
}

struct Rig {
    directory: tempfile::TempDir,
    server: CommandServer,
    handler: Arc<Handler>,
    now: Time,
}
fn rig(
    first: Id,
    ending: Ending,
    defer_close: bool,
    writes: Option<Arc<TestWrites>>,
) -> TestResult<Rig> {
    let directory = tempfile::tempdir()?;
    let now: Time = "2026-10-16T00:00:00Z".parse()?;
    let clock = ManualClock::new(now.get());
    let mut journal = Journal::open(directory.path(), Duration::from_secs(86_400), now)?;
    if let Some(writes) = writes {
        journal.terminal_writes(writes);
    }
    let server = CommandServer::new(
        journal,
        Arc::new(clock),
        ServerOptions {
            roles: vec![Role::Environment, Role::Agent],
            connected: true,
        },
    );
    let handler = Arc::new(Handler {
        first,
        capture: Arc::new(Latch::new()),
        upload: Arc::new(Latch::new()),
        dropped: Arc::new(AtomicUsize::new(0)),
        calls: AtomicUsize::new(0),
        trace: Arc::new(Mutex::new(Vec::new())),
        ending,
        defer_close,
    });
    let serving: Arc<dyn CommandHandler> = handler.clone();
    server.serve(serving);
    Ok(Rig {
        directory,
        server,
        handler,
        now,
    })
}

fn record(rig: &Rig, id: Id, environment: Id) -> TestResult<Cmd> {
    Ok(Cmd::new(
        id,
        &SerialKey::Environment(environment),
        Time::new(rig.now.get() + chrono::Duration::hours(6)),
        &FsRecord {
            environment_id: environment,
            snapshot_id: new_id(),
            directory: String::new(),
            kind: RecordKind::Turn,
            record_days: 1,
            max_bytes: 1024,
        },
    )?)
}

fn close(rig: &Rig, id: Id, environment: Id) -> TestResult<Cmd> {
    Ok(Cmd::new(
        id,
        &SerialKey::Environment(environment),
        Time::new(rig.now.get() + chrono::Duration::hours(6)),
        &SessionClose {
            thread_id: new_id(),
        },
    )?)
}

async fn settled(server: &CommandServer, id: Id) -> TestResult<CmdRes> {
    Ok(tokio::time::timeout(PATIENCE, async {
        loop {
            let changed = server.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let result = server.entry(id).and_then(|entry| entry.result().cloned());
            let active = { lock(&server.inner.state).in_hand.contains_key(&id) };
            if let Some(result) = result
                && !active
            {
                return result;
            }
            changed.await;
        }
    })
    .await?)
}

#[tokio::test]
async fn concurrent_delivery_before_acceptance_has_one_capture_and_one_publication() -> TestResult {
    let first = new_id();
    let rig = rig(first, Ending::Correct, false, None)?;
    let cmd = record(&rig, first, new_id())?;
    tokio::join!(
        rig.server.on_cmd(cmd.clone()),
        rig.server.on_cmd(cmd.clone())
    );
    rig.handler.capture.entered().await?;
    assert_eq!(rig.handler.calls.load(Ordering::SeqCst), 1);
    rig.handler.capture.release();
    rig.handler.upload.entered().await?;
    rig.server.on_cmd(cmd).await;
    rig.handler.upload.release();
    assert!(settled(&rig.server, first).await?.outcome.is_ok());
    assert_eq!(rig.handler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(rig.handler.dropped.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn duplicate_recovery_of_started_state_has_one_durable_refusal_without_recapture()
-> TestResult {
    let first = new_id();
    let rig = rig(first, Ending::Correct, false, None)?;
    let cmd = record(&rig, first, new_id())?;
    {
        let mut journal = lock(&rig.server.inner.journal);
        journal.accept(&cmd, rig.now)?;
        journal.started(first)?;
    }
    tokio::join!(
        rig.server.on_cmd(cmd.clone()),
        rig.server.on_cmd(cmd.clone())
    );
    let result = settled(&rig.server, first).await?;
    let Err(error) = &result.outcome else {
        return Err("interrupted state was run again".into());
    };
    assert!(error.retryable);
    assert_eq!(
        error.detail,
        "the machine stopped while the command was running"
    );
    rig.server.on_cmd(cmd).await;
    assert_eq!(rig.handler.calls.load(Ordering::SeqCst), 0);
    let journal = Journal::open(rig.directory.path(), Duration::from_secs(86_400), rig.now)?;
    assert_eq!(journal.get(first).and_then(Entry::result), Some(&result));
    assert_eq!(rig.server.in_hand(), 0);
    Ok(())
}

#[tokio::test]
async fn slow_publication_releases_only_the_captured_record_key_and_keeps_its_identity_in_hand()
-> TestResult {
    let first = new_id();
    let environment = new_id();
    let rig = rig(first, Ending::Correct, false, None)?;
    let cmd = record(&rig, first, environment)?;
    rig.server.on_cmd(cmd.clone()).await;
    rig.handler.capture.entered().await?;
    let second = new_id();
    let other = new_id();
    rig.server.on_cmd(close(&rig, second, environment)?).await;
    rig.server.on_cmd(close(&rig, other, new_id())?).await;
    assert!(settled(&rig.server, other).await?.outcome.is_ok());
    assert!(!lock(&rig.handler.trace).iter().any(|(id, _)| *id == second));
    rig.handler.capture.release();
    rig.handler.upload.entered().await?;
    assert!(settled(&rig.server, second).await?.outcome.is_ok());
    assert!(
        rig.server
            .entry(first)
            .is_some_and(|entry| entry.result().is_none())
    );
    assert_eq!(rig.server.publications(), 1);
    rig.server.on_cmd(cmd.clone()).await;
    assert_eq!(rig.handler.calls.load(Ordering::SeqCst), 1);
    rig.handler.upload.release();
    let answer = settled(&rig.server, first).await?;
    assert!(answer.outcome.is_ok());
    assert_eq!(rig.handler.dropped.load(Ordering::SeqCst), 1);
    rig.server.on_cmd(cmd).await;
    assert_eq!(
        rig.server
            .entry(first)
            .and_then(|entry| entry.result().cloned()),
        Some(answer)
    );
    assert_eq!(rig.handler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(rig.server.publications(), 0);
    Ok(())
}

#[tokio::test]
async fn ordinary_commands_keep_the_key_even_if_a_handler_mistakenly_returns_a_captured_phase()
-> TestResult {
    let first = new_id();
    let environment = new_id();
    let rig = rig(first, Ending::Correct, true, None)?;
    rig.server.on_cmd(close(&rig, first, environment)?).await;
    rig.handler.capture.entered().await?;
    rig.handler.capture.release();
    rig.handler.upload.entered().await?;
    let second = new_id();
    let other = new_id();
    rig.server.on_cmd(close(&rig, second, environment)?).await;
    rig.server.on_cmd(close(&rig, other, new_id())?).await;
    settled(&rig.server, other).await?;
    assert!(!lock(&rig.handler.trace).iter().any(|(id, _)| *id == second));
    rig.handler.upload.release();
    settled(&rig.server, first).await?;
    settled(&rig.server, second).await?;
    let trace = lock(&rig.handler.trace);
    let published = trace
        .iter()
        .position(|item| *item == (first, "published"))
        .ok_or("no publication")?;
    let later = trace
        .iter()
        .position(|item| *item == (second, "ordinary"))
        .ok_or("no second command")?;
    assert!(published < later);
    Ok(())
}

#[tokio::test]
async fn finalization_holds_survive_a_blocked_flush_and_a_duplicate_cannot_expose_staged_done()
-> TestResult {
    let first = new_id();
    let environment = new_id();
    let writes = Arc::new(TestWrites::new(first, true, 0));
    let rig = rig(first, Ending::Correct, false, Some(Arc::clone(&writes)))?;
    let _release = ReleaseWrites(Arc::clone(&writes));
    let cmd = record(&rig, first, environment)?;
    rig.server.on_cmd(cmd.clone()).await;
    rig.handler.capture.entered().await?;
    rig.handler.capture.release();
    rig.handler.upload.entered().await?;
    rig.handler.upload.release();
    tokio::time::timeout(PATIENCE, writes.entered()).await??;
    assert!(
        rig.server
            .entry(first)
            .is_some_and(|entry| matches!(entry.step, Step::Started))
    );
    rig.server.on_cmd(cmd).await;
    assert_eq!(rig.handler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(rig.handler.dropped.load(Ordering::SeqCst), 0);
    let second = new_id();
    rig.server.on_cmd(close(&rig, second, environment)?).await;
    assert!(settled(&rig.server, second).await?.outcome.is_ok());
    assert_eq!(rig.server.publications(), 1);
    writes.release();
    settled(&rig.server, first).await?;
    assert_eq!(rig.handler.dropped.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_terminal_disk_failure_retries_the_same_outcome_without_rerunning_or_losing_its_hold()
-> TestResult {
    let first = new_id();
    let writes = Arc::new(TestWrites::new(first, false, 1).hold_retry());
    let rig = rig(first, Ending::Correct, false, Some(Arc::clone(&writes)))?;
    let _release = ReleaseWrites(Arc::clone(&writes));
    let cmd = record(&rig, first, new_id())?;
    rig.server.on_cmd(cmd.clone()).await;
    rig.handler.capture.entered().await?;
    rig.handler.capture.release();
    rig.handler.upload.entered().await?;
    rig.handler.upload.release();
    tokio::time::timeout(PATIENCE, writes.entered()).await??;
    tokio::time::timeout(PATIENCE, writes.entered()).await??;
    assert!(
        rig.server
            .entry(first)
            .is_some_and(|entry| matches!(entry.step, Step::Started))
    );
    assert_eq!(rig.handler.dropped.load(Ordering::SeqCst), 0);
    rig.server.on_cmd(cmd).await;
    assert_eq!(rig.handler.calls.load(Ordering::SeqCst), 1);
    rig.server.on_cancel(CmdCancel { id: first }).await;
    writes.release();
    assert!(settled(&rig.server, first).await?.outcome.is_ok());
    assert_eq!(rig.handler.dropped.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_record_cancelled_while_queued_never_starts_its_capture() -> TestResult {
    let first = new_id();
    let environment = new_id();
    let rig = rig(first, Ending::Correct, true, None)?;
    rig.server.on_cmd(close(&rig, first, environment)?).await;
    rig.handler.capture.entered().await?;
    let cancelled = new_id();
    rig.server
        .on_cmd(record(&rig, cancelled, environment)?)
        .await;
    rig.server.on_cancel(CmdCancel { id: cancelled }).await;
    let result = settled(&rig.server, cancelled).await?;
    assert!(matches!(result.outcome, Err(error) if error.code == "conflict"));
    assert!(
        !lock(&rig.handler.trace)
            .iter()
            .any(|(id, _)| *id == cancelled)
    );
    rig.handler.capture.release();
    rig.handler.upload.entered().await?;
    rig.handler.upload.release();
    assert!(settled(&rig.server, first).await?.outcome.is_ok());
    assert_eq!(rig.handler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(rig.server.in_hand(), 0);
    Ok(())
}

#[tokio::test]
async fn cancelling_capture_does_not_release_the_key_until_its_owner_has_joined() -> TestResult {
    let first = new_id();
    let environment = new_id();
    let rig = rig(first, Ending::Correct, false, None)?;
    rig.server.on_cmd(record(&rig, first, environment)?).await;
    rig.handler.capture.entered().await?;
    let second = new_id();
    rig.server.on_cmd(close(&rig, second, environment)?).await;
    rig.server.on_cancel(CmdCancel { id: first }).await;
    assert!(
        rig.server
            .entry(first)
            .is_some_and(|entry| entry.result().is_none())
    );
    assert!(!lock(&rig.handler.trace).iter().any(|(id, _)| *id == second));
    rig.handler.capture.release();
    assert!(settled(&rig.server, first).await?.outcome.is_err());
    assert!(settled(&rig.server, second).await?.outcome.is_ok());
    assert_eq!(rig.server.publications(), 0);
    Ok(())
}

#[tokio::test]
async fn publication_cancellation_panic_and_wrong_result_each_finalize_once_and_release_the_hold()
-> TestResult {
    for ending in [Ending::Correct, Ending::Panic, Ending::WrongKind] {
        let first = new_id();
        let rig = rig(first, ending, false, None)?;
        rig.server.on_cmd(record(&rig, first, new_id())?).await;
        rig.handler.capture.entered().await?;
        rig.handler.capture.release();
        rig.handler.upload.entered().await?;
        if matches!(ending, Ending::Correct) {
            rig.server.cancel_recordings().await;
        } else {
            rig.handler.upload.release();
        }
        let answer = settled(&rig.server, first).await?;
        assert!(answer.outcome.is_err());
        assert_eq!(rig.handler.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(rig.server.publications(), 0);
    }
    Ok(())
}
