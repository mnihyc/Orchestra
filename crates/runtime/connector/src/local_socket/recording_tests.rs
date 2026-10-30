use super::*;
use orch_link_contract::commands::{CommandKind, Empty, ErrorCode, SessionClose};
use orch_link_contract::source::{FsRecord, FsRecordResult, RecordKind, RecordState};
use orch_types::{ManualClock, new_id};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Semaphore, oneshot};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PATIENCE: Duration = Duration::from_secs(30);

struct Hold(Arc<AtomicUsize>);
impl Drop for Hold {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Split {
    tail: Mutex<Option<oneshot::Receiver<CommandOutcome>>>,
    entered: Arc<Semaphore>,
    released: Arc<AtomicUsize>,
    begins: AtomicUsize,
    handles: AtomicUsize,
    answered: Mutex<Vec<Id>>,
}

#[async_trait]
impl CommandHandler for Split {
    fn answered(&self, id: Id) {
        self.answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(id);
    }
    async fn begin(&self, _: CommandContext, command: Command) -> CommandExecution {
        self.begins.fetch_add(1, Ordering::SeqCst);
        if matches!(command, Command::SessionClose(_)) {
            return CommandExecution::ready(Ok(CommandResult::SessionClose(Empty {})));
        }
        let tail = {
            self.tail
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
        };
        let Some(tail) = tail else {
            return CommandExecution::ready(Err(CommandError::new(
                ErrorCode::Internal,
                "synthetic tail already taken",
            )));
        };
        let entered = Arc::clone(&self.entered);
        CommandExecution::Captured {
            finish: Box::pin(async move {
                entered.add_permits(1);
                tail.await.unwrap_or_else(|_| {
                    Err(CommandError::new(
                        ErrorCode::Internal,
                        "synthetic tail sender closed",
                    ))
                })
            }),
            hold: Box::new(Hold(Arc::clone(&self.released))),
        }
    }
    async fn handle(&self, _: CommandContext, _: Command) -> CommandOutcome {
        self.handles.fetch_add(1, Ordering::SeqCst);
        Err(CommandError::new(
            ErrorCode::Internal,
            "the wrapper bypassed phased begin",
        ))
    }
}

struct Fixture {
    wrapper: Recording,
    inner: Arc<Split>,
    activity: Arc<Activity>,
    clock: Arc<ManualClock>,
    release: oneshot::Sender<CommandOutcome>,
}

fn fixture() -> TestResult<Fixture> {
    let (release, tail) = oneshot::channel();
    let inner = Arc::new(Split {
        tail: Mutex::new(Some(tail)),
        entered: Arc::new(Semaphore::new(0)),
        released: Arc::new(AtomicUsize::new(0)),
        begins: AtomicUsize::new(0),
        handles: AtomicUsize::new(0),
        answered: Mutex::new(Vec::new()),
    });
    let at: Time = "2026-10-16T00:00:00Z".parse()?;
    let clock = Arc::new(ManualClock::new(at.get()));
    let activity = Arc::new(Activity::default());
    let handler: Arc<dyn CommandHandler> = inner.clone();
    let wrapper = Recording::new(handler, Arc::clone(&activity), clock.clone());
    Ok(Fixture {
        wrapper,
        inner,
        activity,
        clock,
        release,
    })
}

fn context(clock: &dyn Clock) -> CommandContext {
    CommandContext {
        id: new_id(),
        key: Some(new_id().to_string()),
        deadline: Time::new(
            clock.now()
                + chrono::Duration::seconds(i64::from(CommandKind::FsRecord.deadline_secs())),
        ),
        cancel: CancellationToken::new(),
    }
}

fn record() -> Command {
    Command::FsRecord(FsRecord {
        environment_id: new_id(),
        snapshot_id: new_id(),
        directory: String::new(),
        kind: RecordKind::Turn,
        record_days: 1,
        max_bytes: 1024,
    })
}

fn result() -> CommandResult {
    CommandResult::FsRecord(FsRecordResult {
        state: RecordState::Unavailable,
        tree_ref: None,
        repository_head: None,
        manifest_blob: None,
        files: 0,
        bytes: 0,
        expires_at: None,
        reason: Some("off".to_owned()),
    })
}

#[tokio::test]
async fn begin_returns_the_unfinished_tail_and_original_hold_and_records_only_its_actual_outcome()
-> TestResult {
    for refuses in [false, true] {
        let Fixture {
            wrapper,
            inner,
            activity,
            clock,
            release,
        } = fixture()?;
        let context = context(clock.as_ref());
        let id = context.id;
        // The sender is deliberately not released until begin has returned Captured.
        let execution = tokio::time::timeout(PATIENCE, wrapper.begin(context, record())).await?;
        let CommandExecution::Captured { finish, hold } = execution else {
            return Err("activity wrapper flattened capture".into());
        };
        assert!(activity.latest(ACTIVITY_KEPT).is_empty());
        assert_eq!(inner.released.load(Ordering::SeqCst), 0);
        let finishing = tokio::spawn(finish);
        tokio::time::timeout(PATIENCE, inner.entered.acquire())
            .await??
            .forget();
        assert!(!finishing.is_finished());
        assert!(activity.latest(ACTIVITY_KEPT).is_empty());
        clock.advance(chrono::Duration::seconds(1));
        let answer = if refuses {
            Err(CommandError::new(
                ErrorCode::Conflict,
                "synthetic publication refused",
            ))
        } else {
            Ok(result())
        };
        release
            .send(answer)
            .map_err(|_| "captured receiver disappeared")?;
        let answer = tokio::time::timeout(PATIENCE, finishing).await??;
        if refuses {
            let Err(error) = answer else {
                return Err("refused publication succeeded".into());
            };
            assert_eq!(error.code, "conflict");
            assert_eq!(error.detail, "synthetic publication refused");
        } else {
            let Ok(answer) = answer else {
                return Err("successful publication failed".into());
            };
            assert_eq!(answer.kind(), CommandKind::FsRecord);
            assert_eq!(answer.body()?, result().body()?);
        }
        let rows = activity.latest(ACTIVITY_KEPT);
        assert_eq!(
            rows,
            vec![ActivityRow {
                at: clock.time(),
                kind: "fs.record".to_owned(),
                outcome: (if refuses { "conflict" } else { "done" }).to_owned(),
                detail: refuses.then(|| "synthetic publication refused".to_owned())
            }]
        );
        assert_eq!(
            inner.released.load(Ordering::SeqCst),
            0,
            "the caller still owns the original finalization hold"
        );
        drop(hold);
        assert_eq!(inner.released.load(Ordering::SeqCst), 1);
        wrapper.answered(id);
        assert_eq!(
            *inner
                .answered
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            vec![id]
        );
        assert_eq!(activity.latest(ACTIVITY_KEPT).len(), 1);
        assert_eq!(inner.begins.load(Ordering::SeqCst), 1);
        assert_eq!(inner.handles.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn handle_uses_the_same_phased_path_without_double_recording() -> TestResult {
    let Fixture {
        wrapper,
        inner,
        activity,
        clock,
        release,
    } = fixture()?;
    let context = context(clock.as_ref());
    let task = tokio::spawn(async move { wrapper.handle(context, record()).await });
    tokio::time::timeout(PATIENCE, inner.entered.acquire())
        .await??
        .forget();
    assert!(!task.is_finished());
    assert!(activity.latest(ACTIVITY_KEPT).is_empty());
    assert_eq!(inner.released.load(Ordering::SeqCst), 0);
    release
        .send(Ok(result()))
        .map_err(|_| "captured receiver disappeared")?;
    let Ok(answer) = tokio::time::timeout(PATIENCE, task).await?? else {
        return Err("direct handle failed".into());
    };
    assert_eq!(answer.kind(), CommandKind::FsRecord);
    assert_eq!(answer.body()?, result().body()?);
    assert_eq!(activity.latest(ACTIVITY_KEPT).len(), 1);
    assert_eq!(activity.latest(1)[0].outcome, "done");
    assert_eq!(inner.released.load(Ordering::SeqCst), 1);
    assert_eq!(inner.begins.load(Ordering::SeqCst), 1);
    assert_eq!(inner.handles.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn complete_results_are_forwarded_and_observed_once() -> TestResult {
    let Fixture {
        wrapper,
        inner,
        activity,
        clock,
        ..
    } = fixture()?;
    let command = Command::SessionClose(SessionClose {
        thread_id: new_id(),
    });
    let execution = wrapper.begin(context(clock.as_ref()), command).await;
    let CommandExecution::Complete(answer) = execution else {
        return Err("ordinary result became deferred".into());
    };
    let Ok(answer) = *answer else {
        return Err("ordinary result failed".into());
    };
    assert_eq!(answer.kind(), CommandKind::SessionClose);
    assert_eq!(
        activity.latest(ACTIVITY_KEPT),
        vec![ActivityRow {
            at: clock.time(),
            kind: "session.close".to_owned(),
            outcome: "done".to_owned(),
            detail: None
        }]
    );
    assert_eq!(inner.begins.load(Ordering::SeqCst), 1);
    assert_eq!(inner.handles.load(Ordering::SeqCst), 0);
    Ok(())
}
