//! Serial command execution and deadlines.
//!
//! The system asks a machine to do things with commands. Two ends meet
//! here.
//!
//! [`CommandServer`] is the machine's end. It writes a command down before it
//! says that it has it, runs commands that share a serial key one at a time in
//! the order they arrived and the rest side by side, does not start one whose
//! deadline has passed, writes the result down before it says how the command
//! ended, and answers a command it has seen before from what it wrote down
//! instead of running it again.
//!
//! [`CommandIssuer`] is the system's end. It sends a command when a session is
//! ready, sends the ones that have not ended again whenever a session becomes
//! ready, gives a machine that does not acknowledge a command one more try
//! and then fails the command as retryable, lets a command that was never
//! handed to a machine expire at its deadline, and lets whoever issued a
//! command wait for its result.

use crate::journal::{Entry, Journal, JournalError, Step};
use crate::session::SessionSender;
use async_trait::async_trait;
use orch_link_contract::backplane::DownFrame;
use orch_link_contract::commands::{
    ACK_RESENDS, ACK_TIMEOUT_SECS, Cmd, CmdAck, CmdCancel, CmdRes, Command, CommandBody,
    CommandError, CommandKind, CommandResult, ErrorCode, SerialKey,
};
use orch_link_contract::frames::Frame;
use orch_link_contract::session::Role;
use orch_types::{Clock, Id, Json, Time};
use std::any::Any;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The code a machine answers with when it did not start a command in time.
const TIMEOUT: ErrorCode = ErrorCode::Timeout;

/// Retry disk failures once a second: keep the chosen outcome without spinning on a failed disk.
const JOURNAL_RETRY: Duration = Duration::from_secs(1);

/// The final result and the resources that must survive until its journal write completes.
pub type CommandOutcome = Result<CommandResult, CommandError>;

/// An operation's ownership, independent of the future that uses its captured data.
pub type CompletionHold = Box<dyn Any + Send>;

/// Local preparation either completes a command or hands its immutable publication to the server.
pub enum CommandExecution {
    /// The whole operation has ended.
    Complete(Box<CommandOutcome>),
    /// Capture is durable; only `fs.record` can release its serial key before this future ends.
    Captured {
        /// Publication owns its input and never reads the live workspace.
        finish: Pin<Box<dyn Future<Output = CommandOutcome> + Send + 'static>>,
        /// Kept through the terminal journal flush, including a failed flush's retry.
        hold: CompletionHold,
    },
}

impl CommandExecution {
    /// A finished operation, boxed so the small captured-continuation variant stays small too.
    #[must_use]
    pub fn ready(outcome: CommandOutcome) -> Self {
        Self::Complete(Box::new(outcome))
    }

    /// Completes both phases for a caller that does not manage serial keys or a journal.
    pub async fn complete(self) -> CommandOutcome {
        match self {
            Self::Complete(outcome) => *outcome,
            Self::Captured { finish, hold } => {
                let outcome = finish.await;
                drop(hold);
                outcome
            }
        }
    }
}

/// Bounded labels for command timings; command, machine and environment IDs are never labels.
#[derive(Clone, Copy)]
pub enum CommandPhase {
    /// Durable acceptance until this serial-key worker takes the command.
    KeyWait,
    /// Start checks and journal write through local preparation, immutable input and lease acquisition.
    Capture,
    /// Publication after key release, including transfer admission and network waits.
    Publication,
    /// Terminal outcome selection through its successful journal flush.
    Finalization,
}

/// Records a monotonic duration for one command phase.
pub fn command_phase(kind: CommandKind, phase: CommandPhase, elapsed: Duration) {
    let phase = match phase {
        CommandPhase::KeyWait => "key_wait",
        CommandPhase::Capture => "capture",
        CommandPhase::Publication => "publication",
        CommandPhase::Finalization => "finalization",
    };
    metrics::histogram!("orch_machine_command_phase_seconds", "kind" => kind.as_str(), "phase" => phase)
        .record(elapsed.as_secs_f64());
}

struct PublicationCount;
impl PublicationCount {
    fn new() -> Self {
        metrics::gauge!("orch_machine_record_publications").increment(1.0);
        Self
    }
}
impl Drop for PublicationCount {
    fn drop(&mut self) {
        metrics::gauge!("orch_machine_record_publications").decrement(1.0);
    }
}

fn joined(outcome: Result<CommandOutcome, tokio::task::JoinError>) -> CommandOutcome {
    outcome.unwrap_or_else(|_| Err(CommandError::new(ErrorCode::Internal, "the handler failed")))
}

fn result_body(kind: CommandKind, outcome: CommandOutcome) -> Result<Json, CommandError> {
    let result = outcome?;
    if result.kind() != kind {
        return Err(CommandError::new(
            ErrorCode::Internal,
            "the handler answered with the result of another kind of command",
        ));
    }
    result
        .body()
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))
}

/// What a machine answers when the deadline of a command passed before it could start it.
fn too_late() -> CommandError {
    CommandError::new(
        TIMEOUT,
        "the deadline passed before the command could start",
    )
}

// ---------------------------------------------------------------- the machine's end

/// What a handler is told about the command it runs.
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// The command.
    pub id: Id,
    /// Its serial key.
    pub key: Option<String>,
    /// The moment after which it was not to be started.
    pub deadline: Time,
    /// Fires when the system asks for the command not to be carried out. A
    /// handler that can stop stops, and answers.
    pub cancel: CancellationToken,
}

/// Something that carries out the commands of a machine.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Settles external work from a started journal entry before its interrupted result is sent.
    async fn interrupted(&self, _id: Id, _command: &Command) -> Result<(), CommandError> {
        Ok(())
    }
    /// The result is journaled and queued for sending. A program may now perform a requested restart;
    /// redelivery after reconnect still reads this same durable result.
    fn answered(&self, _id: Id) {}

    /// Prepares a command. Ordinary handlers retain their complete, serialized execution.
    async fn begin(&self, context: CommandContext, command: Command) -> CommandExecution {
        CommandExecution::ready(self.handle(context, command).await)
    }

    /// Carries out a command of the catalogue. The result has to be that of
    /// the command's kind.
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError>;
}

/// What the command server needs to know about its machine.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// The roles the machine has switched on.
    pub roles: Vec<Role>,
    /// Whether a person connected the machine: the system made none of
    /// those that are not.
    pub connected: bool,
}

struct ServerState {
    /// A cancellation received before admission prevents a crossing command from starting.
    cancelled_before_arrival: HashSet<Id>,
    session: Option<SessionSender>,
    /// Commands waiting for the one before them with the same key, in the
    /// order they arrived.
    queues: HashMap<String, VecDeque<Queued>>,
    /// The keys that have a worker running.
    working: HashSet<String>,
    /// Commands the server has in hand: being written down, queued or running.
    in_hand: HashMap<Id, InHand>,
}

/// A command in the line of its key, with the word, once it is written down,
/// whether it is to run: not when its deadline had passed when it came, when
/// it was cancelled meanwhile, or when it could not be written down.
struct Queued {
    cmd: Cmd,
    admitted: oneshot::Receiver<bool>,
}

struct InHand {
    cancel: CancellationToken,
    queued_under: Option<String>,
    /// Whether it is written down, and so acknowledged: a copy that arrives
    /// before is not acknowledged, since the machine does not have it yet.
    written: bool,
    accepted: Instant,
    finishing: bool,
    finalizer: Option<tokio::task::JoinHandle<()>>,
}

struct ServerInner {
    journal: Arc<Mutex<Journal>>,
    /// What carries commands out, once the machine has said what it is.
    handler: watch::Sender<Option<Arc<dyn CommandHandler>>>,
    clock: Arc<dyn Clock>,
    options: ServerOptions,
    state: Mutex<ServerState>,
    /// How far this machine's clock is ahead of the gateway's, in
    /// milliseconds: deadlines are in the system's time.
    clock_offset_ms: AtomicI64,
    recordings_stopped: AtomicBool,
    changed: tokio::sync::Notify,
}

/// The machine's end of commands.
#[derive(Clone)]
pub struct CommandServer {
    inner: Arc<ServerInner>,
}

impl std::fmt::Debug for CommandServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandServer").finish_non_exhaustive()
    }
}

/// Runs work that blocks on the disk where blocking is fine.
async fn on_disk<R: Send + 'static>(
    work: impl FnOnce() -> Result<R, JournalError> + Send + 'static,
) -> Result<R, JournalError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| JournalError::Io {
            path: "journal".to_owned(),
            source: std::io::Error::other(e),
        })?
}

impl CommandServer {
    /// A server that writes its journal to `journal`.
    #[must_use]
    pub fn new(journal: Journal, clock: Arc<dyn Clock>, options: ServerOptions) -> Self {
        Self {
            inner: Arc::new(ServerInner {
                journal: Arc::new(Mutex::new(journal)),
                handler: watch::channel(None).0,
                clock,
                options,
                state: Mutex::new(ServerState {
                    cancelled_before_arrival: HashSet::new(),
                    session: None,
                    queues: HashMap::new(),
                    working: HashSet::new(),
                    in_hand: HashMap::new(),
                }),
                clock_offset_ms: AtomicI64::new(0),
                recordings_stopped: AtomicBool::new(false),
                changed: tokio::sync::Notify::new(),
            }),
        }
    }

    /// Registers what carries commands out. A command that reached the machine before this was called
    /// waits for it.
    pub fn serve(&self, handler: Arc<dyn CommandHandler>) {
        self.inner.handler.send_replace(Some(handler));
    }

    /// What carries commands out. A machine opens its link before it has built what carries them (the
    /// roles are made with the link in hand), so a session can stand, and the system can send a command
    /// and be told it is in hand, before [`CommandServer::serve`] is called: the command waits for it, for
    /// as long as it is not cancelled and its deadline has not passed, and is not failed for good.
    async fn handler_for(
        &self,
        cmd: &Cmd,
        cancel: &CancellationToken,
    ) -> Result<Arc<dyn CommandHandler>, CommandError> {
        let mut registered = self.inner.handler.subscribe();
        loop {
            if let Some(handler) = registered.borrow_and_update().clone() {
                return Ok(handler);
            }
            let left = (cmd.deadline.get() - self.system_now().get())
                .to_std()
                .unwrap_or_default();
            tokio::select! {
                changed = registered.changed() => {
                    if changed.is_err() {
                        return Err(CommandError::new(
                            ErrorCode::Unsupported,
                            "nothing here carries commands out",
                        ));
                    }
                }
                () = cancel.cancelled() => {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "cancelled before it started",
                    ));
                }
                () = tokio::time::sleep(left) => return Err(too_late()),
            }
        }
    }

    /// Tells the server which session its answers go to, and how far the
    /// machine's clock is ahead of the system's.
    pub fn set_session(&self, session: Option<SessionSender>, clock_offset_ms: i64) {
        self.inner
            .clock_offset_ms
            .store(clock_offset_ms, Ordering::Relaxed);
        lock(&self.inner.state).session = session;
    }

    /// How far this machine's clock is ahead of the system's, in milliseconds,
    /// as the last handshake showed it; 0 before the first.
    #[must_use]
    pub fn clock_offset_ms(&self) -> i64 {
        self.inner.clock_offset_ms.load(Ordering::Relaxed)
    }

    /// The system's present moment, by this machine's clock and the offset
    /// the handshake showed.
    fn system_now(&self) -> Time {
        let offset = self.inner.clock_offset_ms.load(Ordering::Relaxed);
        Time::new(self.inner.clock.now() - chrono::Duration::milliseconds(offset))
    }

    fn send(&self, frame: impl Into<Frame>) {
        let session = lock(&self.inner.state).session.clone();
        if let Some(session) = session {
            let _ = session.send(frame);
        }
    }

    /// Writes a command down, durably. The journal is held only to stage the entry and to keep it: the
    /// disk's work is done with no hold on it, so that commands that arrive together are written down side
    /// by side instead of serializing every command behind the other commands' disk flushes.
    async fn write_accepted(&self, cmd: &Cmd, now: Time) -> Result<(), JournalError> {
        let staged = lock(&self.inner.journal).stage_accept(cmd, now);
        let Some(staged) = staged else {
            return Ok(());
        };
        let entry = staged.entry.clone();
        on_disk(move || staged.write()).await?;
        lock(&self.inner.journal).keep(entry);
        Ok(())
    }

    /// Writes that a command started.
    async fn write_started(&self, id: Id) -> Result<(), JournalError> {
        let staged = lock(&self.inner.journal).stage_started(id)?;
        if let Some(staged) = staged {
            on_disk(move || staged.write()).await?;
        }
        Ok(())
    }

    /// Writes how a command ended, and returns what is said of it: the first result stands.
    async fn write_finished(
        &self,
        id: Id,
        outcome: Result<Json, CommandError>,
    ) -> Result<CmdRes, JournalError> {
        let (result, staged) = lock(&self.inner.journal).stage_finish(id, outcome)?;
        if let Some(staged) = staged {
            let entry = staged.entry.clone();
            on_disk(move || staged.write()).await?;
            lock(&self.inner.journal).keep_finished(entry);
        }
        Ok(result)
    }

    /// What is written about a command.
    #[must_use]
    pub fn entry(&self, id: Id) -> Option<Entry> {
        lock(&self.inner.journal).get(id).cloned()
    }

    /// Forgets what is older than the journal keeps.
    pub fn purge(&self) -> Result<usize, JournalError> {
        let now = self.system_now();
        // Admission uses this same state-before-journal order, so collection cannot act on a
        // stale live-set snapshot while a command takes ownership.
        let state = lock(&self.inner.state);
        let live = state.in_hand.keys().copied().collect();
        lock(&self.inner.journal).purge_except(now, &live)
    }

    /// A command arrived: from the system, or again. It is taken in hand at
    /// once, in its key's place, and written down on a task of its own: the
    /// machine's link does not await each command's disk flush before reading the next frame.
    pub async fn on_cmd(&self, cmd: Cmd) {
        self.take_in_hand(cmd);
    }

    /// Takes a command in hand: in the line of its key, or to run beside the others, once it is written
    /// down. One that is written down already runs in its turn; one that is not is written down first, by a
    /// task of its own ([`CommandServer::write_down`]).
    fn take_in_hand(&self, cmd: Cmd) {
        let id = cmd.id;
        let (admit, admitted) = oneshot::channel();
        let mut state = lock(&self.inner.state);
        if let Some(hand) = state.in_hand.get(&id) {
            let written = hand.written;
            drop(state);
            if written {
                self.send(CmdAck { id });
            }
            return;
        }
        // Reserve ownership against the current journal view. Reading it before checking in_hand
        // could replay a stale Accepted entry after its original owner has already finished.
        let known = lock(&self.inner.journal).get(id).cloned();
        let (cmd, written, interrupted) = match known {
            Some(Entry {
                step: Step::Done { result },
                ..
            }) => {
                drop(state);
                self.send(CmdAck { id });
                self.send(result);
                return;
            }
            Some(Entry {
                cmd,
                step: Step::Started,
                ..
            }) => (cmd, true, true),
            Some(Entry {
                cmd,
                step: Step::Accepted,
                ..
            }) => (cmd, true, false),
            None => (cmd, false, false),
        };
        let cancel = CancellationToken::new();
        if state.cancelled_before_arrival.contains(&id) {
            cancel.cancel();
        }
        state.in_hand.insert(
            id,
            InHand {
                cancel,
                queued_under: if interrupted { None } else { cmd.key.clone() },
                written,
                accepted: Instant::now(),
                finishing: false,
                finalizer: None,
            },
        );
        if interrupted {
            drop(state);
            self.send(CmdAck { id });
            let server = self.clone();
            tokio::spawn(async move {
                loop {
                    let handler = { server.inner.handler.borrow().clone() };
                    if let Some(handler) = handler
                        && let Ok(command) = cmd.command()
                    {
                        match handler.interrupted(id, &command).await {
                            Ok(()) => break,
                            Err(error) => {
                                tracing::warn!(%error, command=%id, "interrupted command cleanup is still pending")
                            }
                        }
                    }
                    tokio::time::sleep(JOURNAL_RETRY).await;
                }
                server
                    .end(
                        id,
                        Err(CommandError::new(
                            ErrorCode::Internal,
                            "the machine stopped while the command was running",
                        )
                        .retryable()),
                    )
                    .await;
            });
            return;
        }
        if let Some(key) = cmd.key.clone() {
            state
                .queues
                .entry(key.clone())
                .or_default()
                .push_back(Queued {
                    cmd: cmd.clone(),
                    admitted,
                });
            if state.working.insert(key.clone()) {
                let server = self.clone();
                tokio::spawn(async move { server.work_through(key).await });
            }
        }
        drop(state);
        let keyed = cmd.key.is_some();
        let server = self.clone();
        if written {
            self.send(CmdAck { id });
            if keyed {
                let _ = admit.send(true);
            } else {
                tokio::spawn(async move { server.execute(cmd).await });
            }
        } else {
            let admit = keyed.then_some(admit);
            tokio::spawn(async move { server.write_down(cmd, admit).await });
        }
    }

    /// Writes a command down, says that the machine has it, and lets it run: at once where it has no key,
    /// in its turn where it has one. One that cannot be written down is let go unacknowledged, so that
    /// the system sends it again; one whose deadline had passed when it came, or that was cancelled
    /// meanwhile, is answered so and does not run.
    async fn write_down(&self, cmd: Cmd, admit: Option<oneshot::Sender<bool>>) {
        let id = cmd.id;
        let now = self.system_now();
        if let Err(error) = self.write_accepted(&cmd, now).await {
            // Not written down: not acknowledged either, so that the system
            // sends it again when the disk is there.
            tracing::error!(%error, command = %id, "a command could not be written down");
            self.let_go(id);
            if let Some(admit) = admit {
                let _ = admit.send(false);
            }
            return;
        }
        let cancel = {
            let mut state = lock(&self.inner.state);
            state.in_hand.get_mut(&id).map(|hand| {
                hand.written = true;
                hand.accepted = Instant::now();
                hand.cancel.clone()
            })
        };
        self.send(CmdAck { id });
        let refused = if cmd.deadline <= now {
            Some(CommandError::new(
                TIMEOUT,
                "the deadline had passed when the command arrived",
            ))
        } else if cancel.as_ref().is_none_or(CancellationToken::is_cancelled) {
            Some(CommandError::new(
                ErrorCode::Conflict,
                "cancelled before it started",
            ))
        } else {
            None
        };
        if let Some(error) = refused {
            if let Some(admit) = admit {
                let _ = admit.send(false);
            }
            self.end(id, Err(error)).await;
            return;
        }
        match admit {
            Some(admit) => {
                let _ = admit.send(true);
            }
            None => self.execute(cmd).await,
        }
    }

    /// Forgets a command that is in hand and was not written down.
    fn let_go(&self, id: Id) {
        let mut state = lock(&self.inner.state);
        if let Some(hand) = state.in_hand.remove(&id)
            && let Some(key) = hand.queued_under
            && let Some(queue) = state.queues.get_mut(&key)
        {
            queue.retain(|queued| queued.cmd.id != id);
        }
        drop(state);
        self.inner.changed.notify_waiters();
    }

    /// Runs the commands of one key, one after the other in the order they
    /// came, until none is left. Each runs once it is written down, unless it
    /// is not to run.
    async fn work_through(&self, key: String) {
        loop {
            let next = {
                let mut state = lock(&self.inner.state);
                let next = state.queues.get_mut(&key).and_then(VecDeque::pop_front);
                if next.is_none() {
                    state.queues.remove(&key);
                    state.working.remove(&key);
                } else if let Some(queued) = &next
                    && let Some(hand) = state.in_hand.get_mut(&queued.cmd.id)
                {
                    hand.queued_under = None;
                }
                next
            };
            match next {
                Some(Queued { cmd, admitted }) => {
                    if matches!(admitted.await, Ok(true)) {
                        self.execute(cmd).await;
                    }
                }
                None => return,
            }
        }
    }

    async fn execute(&self, cmd: Cmd) {
        let id = cmd.id;
        let cancel = lock(&self.inner.state)
            .in_hand
            .get(&id)
            .map(|hand| hand.cancel.clone())
            .unwrap_or_default();
        let accepted = {
            lock(&self.inner.state)
                .in_hand
                .get(&id)
                .map(|hand| hand.accepted)
        };
        if let (Ok(kind), Some(accepted)) = (cmd.kind.parse::<CommandKind>(), accepted) {
            command_phase(kind, CommandPhase::KeyWait, accepted.elapsed());
        }
        let captured = Instant::now();
        let execution = self.run(&cmd, cancel).await;
        if cmd.kind == CommandKind::FsRecord.as_str() {
            command_phase(
                CommandKind::FsRecord,
                CommandPhase::Capture,
                captured.elapsed(),
            );
        }
        match execution {
            Err(error) => self.end(id, Err(error)).await,
            Ok((kind, CommandExecution::Complete(outcome))) => {
                self.end(id, result_body(kind, *outcome)).await;
            }
            Ok((kind, CommandExecution::Captured { finish, hold }))
                if kind == CommandKind::FsRecord =>
            {
                // Register ownership before releasing this key. No continuation belongs to a transient
                // link session, and a duplicate still sees this command in hand while it publishes.
                let (start, began) = oneshot::channel();
                let server = self.clone();
                let count = PublicationCount::new();
                let task = tokio::spawn(async move {
                    if began.await.is_err() {
                        return;
                    }
                    let started = Instant::now();
                    let outcome = joined(tokio::spawn(finish).await);
                    command_phase(kind, CommandPhase::Publication, started.elapsed());
                    server
                        .end_held(
                            id,
                            result_body(kind, outcome),
                            Some(Box::new((hold, count))),
                        )
                        .await;
                });
                {
                    let mut state = lock(&self.inner.state);
                    if let Some(hand) = state.in_hand.get_mut(&id) {
                        hand.finalizer = Some(task);
                    }
                }
                let _ = start.send(());
            }
            Ok((kind, CommandExecution::Captured { finish, hold })) => {
                // A mistaken opt-in by another kind cannot weaken ordinary serial ordering.
                let outcome = joined(tokio::spawn(finish).await);
                self.end_held(id, result_body(kind, outcome), Some(hold))
                    .await;
            }
        }
    }

    async fn run(
        &self,
        cmd: &Cmd,
        cancel: CancellationToken,
    ) -> Result<(CommandKind, CommandExecution), CommandError> {
        if cancel.is_cancelled() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "cancelled before it started",
            ));
        }
        if cmd.deadline <= self.system_now() {
            return Err(too_late());
        }
        let command = match cmd.command() {
            Ok(command) => command,
            Err(error) => {
                return Err(CommandError::new(ErrorCode::Unsupported, error.to_string()));
            }
        };
        let kind = command.kind();
        if kind == CommandKind::FsRecord && self.inner.recordings_stopped.load(Ordering::SeqCst) {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the connector is stopping; recording was cancelled",
            ));
        }
        if !kind
            .addressee()
            .serves(&self.inner.options.roles, self.inner.options.connected)
        {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                format!("this machine does not take {kind} commands"),
            ));
        }
        let handler = self.handler_for(cmd, &cancel).await?;
        if cmd.deadline <= self.system_now() {
            return Err(too_late());
        }
        let id = cmd.id;
        if self.write_started(id).await.is_err() {
            return Err(CommandError::new(
                ErrorCode::Internal,
                "the journal cannot be written",
            ));
        }
        let context = CommandContext {
            id,
            key: cmd.key.clone(),
            deadline: cmd.deadline,
            cancel,
        };
        // On a task of its own, so that a handler that panics fails its
        // command and nothing else.
        let work = tokio::spawn(async move { handler.begin(context, command).await });
        match work.await {
            Ok(execution) => Ok((kind, execution)),
            Err(_) => Err(CommandError::new(ErrorCode::Internal, "the handler failed")),
        }
    }

    /// Writes how a command ended and says so.
    async fn end(&self, id: Id, outcome: Result<Json, CommandError>) {
        self.end_held(id, outcome, None).await;
    }

    async fn end_held(
        &self,
        id: Id,
        outcome: Result<Json, CommandError>,
        hold: Option<CompletionHold>,
    ) {
        let owned = {
            let mut state = lock(&self.inner.state);
            let hand = state.in_hand.entry(id).or_insert_with(|| InHand {
                cancel: CancellationToken::new(),
                queued_under: None,
                written: true,
                accepted: Instant::now(),
                finishing: false,
                finalizer: None,
            });
            if hand.finishing {
                false
            } else {
                hand.finishing = true;
                true
            }
        };
        if !owned {
            return;
        }
        let started = Instant::now();
        let kind = self
            .entry(id)
            .and_then(|entry| entry.cmd.kind.parse::<CommandKind>().ok());
        // Live session reads retain only a refusal tombstone. Redelivery asks for a fresh read with a new
        // request identity instead of replaying native commands or load from the on-disk journal.
        let transient = self
            .entry(id)
            .is_some_and(|entry| entry.cmd.kind == "session.background");
        let live = transient.then(|| CmdRes {
            id,
            outcome: outcome.clone(),
        });
        let outcome = if transient {
            Err(CommandError::new(
                ErrorCode::Conflict,
                "the live observation expired; read the current session again",
            ))
        } else {
            outcome
        };
        loop {
            match self.write_finished(id, outcome.clone()).await {
                Ok(result) => {
                    if let Some(kind) = kind {
                        command_phase(kind, CommandPhase::Finalization, started.elapsed());
                    }
                    self.send(live.unwrap_or(result));
                    drop(hold);
                    {
                        let mut state = lock(&self.inner.state);
                        state.in_hand.remove(&id);
                        state.cancelled_before_arrival.remove(&id);
                    }
                    self.inner.changed.notify_waiters();
                    let handler = self.inner.handler.borrow().clone();
                    if let Some(handler) = handler {
                        handler.answered(id);
                    }
                    return;
                }
                Err(error) => {
                    metrics::counter!("orch_machine_command_journal_retries").increment(1);
                    tracing::error!(%error, command = %id, "a result could not be written down; the same outcome remains pending");
                    tokio::time::sleep(JOURNAL_RETRY).await;
                }
            }
        }
    }

    /// The system asks for a command not to be carried out. One waiting in
    /// its key's line that is written down is answered at once, on a task of
    /// its own; one being written down is answered when it is; one that runs
    /// is told by its token, and stops if it can.
    pub async fn on_cancel(&self, cancel: CmdCancel) {
        let id = cancel.id;
        let missing = {
            let mut state = lock(&self.inner.state);
            if state.in_hand.contains_key(&id) {
                None
            } else {
                state.cancelled_before_arrival.insert(id);
                Some(lock(&self.inner.journal).get(id).cloned())
            }
        };
        if let Some(entry) = missing {
            if let Some(entry) = entry
                && matches!(entry.step, Step::Started)
            {
                self.take_in_hand(entry.cmd);
                return;
            }
            // No handler or queued entry owns this identifier. A replayed stop after reconnect
            // still needs an answer, without reviving or changing an earlier terminal result.
            self.send(CmdRes {
                id,
                outcome: Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the command is stopped",
                )),
            });
            return;
        }
        let answer_now = {
            let mut state = lock(&self.inner.state);
            let Some(hand) = state.in_hand.get(&id) else {
                return;
            };
            hand.cancel.cancel();
            let written = hand.written;
            let queued_under = hand.queued_under.clone();
            if let Some(key) = queued_under {
                if let Some(queue) = state.queues.get_mut(&key) {
                    queue.retain(|queued| queued.cmd.id != id);
                }
                written
            } else {
                false
            }
        };
        if answer_now {
            let server = self.clone();
            tokio::spawn(async move {
                server
                    .end(
                        id,
                        Err(CommandError::new(
                            ErrorCode::Conflict,
                            "cancelled before it started",
                        )),
                    )
                    .await;
            });
        }
    }

    /// Commands the server has in hand: being written down, waiting or running.
    #[must_use]
    pub fn in_hand(&self) -> usize {
        lock(&self.inner.state).in_hand.len()
    }

    /// Captured publications still owned by this server, including their terminal disk writes.
    #[must_use]
    pub fn publications(&self) -> usize {
        lock(&self.inner.state)
            .in_hand
            .values()
            .filter(|hand| hand.finalizer.is_some())
            .count()
    }

    /// Stops admitting captures and joins their cooperative cancellation and durable finalization.
    /// A link reconnect does not call this; the physical program's graceful stop does.
    pub async fn cancel_recordings(&self) {
        self.inner.recordings_stopped.store(true, Ordering::SeqCst);
        let ids: Vec<_> = {
            let journal = lock(&self.inner.journal);
            journal
                .unfinished()
                .into_iter()
                .filter(|entry| entry.cmd.kind == "fs.record")
                .map(|entry| entry.cmd.id)
                .collect()
        };
        for id in &ids {
            self.on_cancel(CmdCancel { id: *id }).await;
        }
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let pending = {
                let state = lock(&self.inner.state);
                ids.iter().any(|id| state.in_hand.contains_key(id))
            };
            if !pending {
                return;
            }
            changed.await;
        }
    }
}

// ---------------------------------------------------------------- the system's end

#[cfg(test)]
mod recording_tests;

/// Where a command stands, as the system keeps it.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// Made; the machine is offline or the frame is not yet sent.
    Queued,
    /// Handed to the gateway.
    Sent,
    /// The machine wrote it down.
    Accepted,
    /// It ended well.
    Succeeded(Json),
    /// It ended badly, or the machine did not answer.
    Failed(CommandError),
    /// Its deadline passed before a machine had it.
    Expired,
    /// It was cancelled before it ended.
    Cancelled,
}

impl Progress {
    /// Whether nothing more will happen to the command.
    #[must_use]
    pub const fn is_final(&self) -> bool {
        matches!(
            self,
            Self::Succeeded(_) | Self::Failed(_) | Self::Expired | Self::Cancelled
        )
    }
}

struct Issued {
    answered: bool,
    cmd: Cmd,
    tx: watch::Sender<Progress>,
    sent_at: Option<Instant>,
    resends: u32,
}

struct IssuerState {
    ready: bool,
    commands: HashMap<Id, Issued>,
}

/// The system's end of commands, for one machine.
#[derive(Clone)]
pub struct CommandIssuer {
    state: Arc<Mutex<IssuerState>>,
    down: mpsc::UnboundedSender<DownFrame>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for CommandIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandIssuer").finish_non_exhaustive()
    }
}

/// What the issuer of a command waits on.
#[derive(Debug, Clone)]
pub struct CommandHandle {
    id: Id,
    progress: watch::Receiver<Progress>,
}

impl CommandHandle {
    /// The command.
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// Where the command stands.
    #[must_use]
    pub fn progress(&self) -> Progress {
        self.progress.borrow().clone()
    }

    /// Waits for the command to end, and returns how.
    pub async fn wait(&mut self) -> Progress {
        loop {
            let now = self.progress.borrow_and_update().clone();
            if now.is_final() {
                return now;
            }
            if self.progress.changed().await.is_err() {
                return self.progress.borrow().clone();
            }
        }
    }

    /// Waits for the command to end, and returns its result as the kind of
    /// `B` defines it.
    pub async fn wait_for<B: CommandBody>(&mut self) -> Result<B::Result, CommandFailure> {
        match self.wait().await {
            Progress::Succeeded(body) => {
                serde_json::from_value(body).map_err(|e| CommandFailure::Unreadable(e.to_string()))
            }
            Progress::Failed(error) => Err(CommandFailure::Failed(error)),
            Progress::Expired => Err(CommandFailure::Expired),
            Progress::Cancelled => Err(CommandFailure::Cancelled),
            Progress::Queued | Progress::Sent | Progress::Accepted => Err(
                CommandFailure::Unreadable("the command did not end".to_owned()),
            ),
        }
    }
}

/// Why a command did not give a result.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CommandFailure {
    /// The machine, or the system on its behalf, failed it.
    #[error("{0}")]
    Failed(CommandError),
    /// The deadline passed before a machine had it.
    #[error("the command expired before a machine had it")]
    Expired,
    /// It was cancelled.
    #[error("the command was cancelled")]
    Cancelled,
    /// The result is not what the kind defines.
    #[error("the result is not readable: {0}")]
    Unreadable(String),
}

impl CommandIssuer {
    /// An issuer whose frames go to `down`, which the gateway carries to the
    /// machine.
    #[must_use]
    pub fn new(down: mpsc::UnboundedSender<DownFrame>, clock: Arc<dyn Clock>) -> Self {
        Self {
            state: Arc::new(Mutex::new(IssuerState {
                ready: false,
                commands: HashMap::new(),
            })),
            down,
            clock,
        }
    }

    fn transmit(&self, issued: &mut Issued, now: Instant) {
        if self.down.send(DownFrame::Cmd(issued.cmd.clone())).is_ok() {
            issued.sent_at = Some(now);
            issued.tx.send_if_modified(|progress| {
                if *progress == Progress::Queued {
                    *progress = Progress::Sent;
                    true
                } else {
                    false
                }
            });
        }
    }

    /// Issues a command. It is sent now if a session is ready, and whenever
    /// one becomes ready until the command ends.
    pub fn issue(&self, cmd: Cmd) -> CommandHandle {
        self.adopt(cmd, Progress::Queued)
    }

    /// Takes a command the system already has under `progress`, after the
    /// system restarted: the ones that did not end are sent when a session is
    /// ready. An identity already followed by this issuer keeps its original
    /// command, deadline and progress; durable terminal changes use reconciliation.
    pub fn adopt(&self, cmd: Cmd, progress: Progress) -> CommandHandle {
        let id = cmd.id;
        let mut state = lock(&self.state);
        if let Some(issued) = state.commands.get(&id) {
            return CommandHandle {
                id,
                progress: issued.tx.subscribe(),
            };
        }
        let (tx, rx) = watch::channel(progress);
        let mut issued = Issued {
            answered: false,
            cmd,
            tx,
            sent_at: None,
            resends: 0,
        };
        if state.ready && !issued.tx.borrow().is_final() {
            self.transmit(&mut issued, Instant::now());
        }
        state.commands.insert(id, issued);
        CommandHandle { id, progress: rx }
    }

    /// Issues a command of the catalogue.
    pub fn issue_typed<B: CommandBody>(
        &self,
        id: Id,
        key: &SerialKey,
        deadline: Time,
        body: &B,
    ) -> Result<CommandHandle, serde_json::Error> {
        Ok(self.issue(Cmd::new(id, key, deadline, body)?))
    }

    /// A session is ready: every command that did not end is sent again.
    pub fn session_ready(&self) {
        let now = Instant::now();
        let mut state = lock(&self.state);
        state.ready = true;
        for issued in state.commands.values_mut() {
            if !issued.tx.borrow().is_final() {
                issued.resends = 0;
                self.transmit(issued, now);
            }
        }
    }

    /// The session is gone: what was sent and not answered goes back to
    /// waiting for the next.
    pub fn session_down(&self) {
        let mut state = lock(&self.state);
        state.ready = false;
        for issued in state.commands.values_mut() {
            issued.sent_at = None;
            issued.tx.send_if_modified(|progress| {
                if *progress == Progress::Sent {
                    *progress = Progress::Queued;
                    true
                } else {
                    false
                }
            });
        }
    }

    /// The machine wrote a command down.
    pub fn on_cmd_ack(&self, ack: &CmdAck) {
        let mut state = lock(&self.state);
        if let Some(issued) = state.commands.get_mut(&ack.id) {
            issued.sent_at = None;
            issued.tx.send_if_modified(|progress| {
                if matches!(*progress, Progress::Queued | Progress::Sent) {
                    *progress = Progress::Accepted;
                    true
                } else {
                    false
                }
            });
        }
    }

    /// The machine says how a command ended. The first word stands.
    pub fn on_cmd_res(&self, result: &CmdRes) {
        let mut state = lock(&self.state);
        if let Some(issued) = state.commands.get_mut(&result.id) {
            issued.answered = true;
            let ended = match &result.outcome {
                Ok(body) => Progress::Succeeded(body.clone()),
                Err(error) => Progress::Failed(error.clone()),
            };
            issued.sent_at = None;
            issued.tx.send_if_modified(|progress| {
                if progress.is_final() {
                    false
                } else {
                    *progress = ended;
                    true
                }
            });
        }
    }

    /// Whether the machine has returned from this command's handler, even after a cancellation request.
    #[must_use]
    pub fn answered(&self, id: Id) -> bool {
        lock(&self.state)
            .commands
            .get(&id)
            .is_some_and(|issued| issued.answered)
    }

    /// Reconciles a command which the durable controller store already ended.
    /// No frame is sent and an already observed terminal outcome never changes.
    /// The API uses this when an older adoption reaches its final output gate.
    pub fn reconcile_terminal(&self, id: Id, terminal: Progress) -> bool {
        if !terminal.is_final() {
            return false;
        }
        let mut state = lock(&self.state);
        let Some(issued) = state.commands.get_mut(&id) else {
            return false;
        };
        issued.sent_at = None;
        issued.tx.send_if_modified(|current| {
            if current.is_final() {
                false
            } else {
                *current = terminal;
                true
            }
        })
    }

    /// Asks the machine not to carry a command out, and treats it as
    /// cancelled whatever the machine says next.
    pub fn cancel(&self, id: Id) {
        let mut state = lock(&self.state);
        if let Some(issued) = state.commands.get_mut(&id) {
            let was_open = issued.tx.send_if_modified(|progress| {
                if progress.is_final() {
                    false
                } else {
                    *progress = Progress::Cancelled;
                    true
                }
            });
            let _ = was_open;
        }
        // Cancellation is replayable after reconnect, including when only its terminal row remains.
        let _ = self.down.send(DownFrame::CmdCancel(CmdCancel { id }));
    }

    /// Looks at the clocks: a command the machine did not acknowledge in time
    /// is sent once more and then failed as retryable; one that was never
    /// handed to a machine expires at its deadline.
    pub fn tick(&self, now: Instant) {
        let wall = self.clock.time();
        let patience = Duration::from_secs(u64::from(ACK_TIMEOUT_SECS));
        let mut state = lock(&self.state);
        let ready = state.ready;
        for issued in state.commands.values_mut() {
            if issued.tx.borrow().is_final() {
                continue;
            }
            let handed_over = matches!(*issued.tx.borrow(), Progress::Accepted);
            if !handed_over && issued.cmd.deadline <= wall {
                issued.tx.send_replace(Progress::Expired);
                continue;
            }
            let Some(sent_at) = issued.sent_at else {
                continue;
            };
            if ready && now.duration_since(sent_at) >= patience {
                if issued.resends < ACK_RESENDS {
                    issued.resends += 1;
                    self.transmit(issued, now);
                } else {
                    let error = CommandError::new(
                        ErrorCode::Timeout,
                        "the machine did not acknowledge the command",
                    )
                    .retryable();
                    issued.tx.send_replace(Progress::Failed(error));
                }
            }
        }
    }

    /// Where a command stands.
    #[must_use]
    pub fn progress(&self, id: Id) -> Option<Progress> {
        lock(&self.state)
            .commands
            .get(&id)
            .map(|issued| issued.tx.borrow().clone())
    }

    /// Forgets the commands that ended, so that the table does not grow
    /// for ever. Returns how many.
    pub fn forget_ended(&self) -> usize {
        let mut state = lock(&self.state);
        let before = state.commands.len();
        state
            .commands
            .retain(|_, issued| !issued.tx.borrow().is_final());
        before - state.commands.len()
    }
}
