//! The running harness: the sessions it serves, the task that reads what it says and turns it into events, the answers to its requests, and what is reported when it ends.
//!
//! One [`Shared`] belongs to one harness process. It holds the connection ([`Rpc`]), the sessions the process
//! serves (each with its [`Mapper`] and the requests of the harness that wait for an answer), and the sink the
//! events go to. Everything the harness says arrives in one task ([`pump`]), in order, and is handled under
//! one lock so that the events of a session leave in the order they happened, whoever else is reporting at
//! the same moment.
//!
//! When the process ends, by itself or because it was told to ([`Shared::on_closed`]), every session is
//! lost: its open items end, `session.lost` is reported, and the state of the harness becomes `error` (or
//! `offline` when the adapter stopped it). A harness that was started again serves no session of the one
//! before; the engine opens them anew by `resume`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use orch_agent_contract::adapter::{AdapterError, AdapterEvent, EventSink};
use orch_agent_contract::events::{
    ApprovalClosed, ApprovalRequested, SessionEvent, TurnOutcome, turn_error,
};
use orch_agent_contract::facts::{HarnessState, HarnessStatus, ProviderFacts};
use orch_agent_contract::session::{MountGrant, ThreadId};
use orch_types::{Clock, Id, Json};
use serde::Deserialize;
use tokio::sync::watch;

use crate::approvals::{self, Pending, PendingKind};
use crate::events::{Context, Mapper, OutputStore, raw_frame};
use crate::failure::{Patience, Windows};
use crate::protocol::{
    self, Direction, FrameTap, Incoming, METHOD_NOT_FOUND, RequestId, Rpc, RpcError,
};
use crate::provider::{self, LoginTracker};
use crate::sessions::RemoteEnvironment;

/// How long the reader of the harness's output waits, once that output has ended, for the process's exit to
/// be known, so that a harness that failed is reported with the reason it gave and not just as gone.
pub const EXIT_GRACE: Duration = Duration::from_secs(2);

/// How often the host looks whether a turn has run out of patience: a second, since the shortest patience is
/// minutes and a turn that has run out is not worth a quarter of a minute more.
pub const WATCH_EVERY: Duration = Duration::from_secs(1);

/// How long the host waits for the harness to take the interrupt of a turn it has given up on.
const INTERRUPT_WAIT: Duration = Duration::from_secs(10);

/// Stop's terminal cleanup has the same bound as an interrupt request, and runs beside the event reader.
const TERMINAL_CLEANUP_WAIT: Duration = INTERRUPT_WAIT;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundTerminal {
    item_id: String,
    process_id: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundPage {
    data: Vec<BackgroundTerminal>,
    next_cursor: Option<String>,
}

/// What the adapter keeps of one session.
#[derive(Debug)]
pub struct SessionRecord {
    /// Follows the harness's notifications for the session.
    pub mapper: Mapper,
    /// The thread's directory.
    pub directory: String,
    /// Where the agent starts.
    pub cwd: String,
    /// The root supplied independently of the thread's working directory.
    pub workspace_root: String,
    /// Whether this session uses the verified root-only control directory.
    pub instruction_root_marker: bool,
    /// The environment's answer about a configuration of Codex's own in the workspace, as the session was opened
    /// with it.
    pub workspace_configuration: Option<orch_agent_contract::session::WorkspaceConfiguration>,
    /// A once-per-revision explanation of the pinned resume instruction limitation.
    pub instructions_notice: Option<String>,
    /// The attached mounts.
    pub mounts: Vec<MountGrant>,
    /// Where the tools are carried out, if that is not where the harness is.
    pub remote: Option<RemoteEnvironment>,
    /// The requests of the harness that wait for an answer, by their reference: the id the harness gave, behind the
    /// token of its process.
    pub approvals: HashMap<String, Asked>,
    /// Whether the last turn left the harness in collaboration mode `plan`, which stays for later turns
    /// until a turn states another.
    pub planning: bool,
    /// The approval mode of the current or most recently started turn.
    pub approval_mode: orch_agent_contract::descriptor::ApprovalMode,
}

/// A request of the harness that waits for an answer.
#[derive(Debug)]
pub struct Asked {
    /// The id the harness gave the request, which the answer must name. It is not the reference Orch is given: the
    /// harness counts its requests from 0 in each process, and the reference has the token of the process before the id.
    pub id: RequestId,
    /// What the adapter keeps of the request until it is answered.
    pub pending: Pending,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<ThreadId, SessionRecord>,
    by_native: HashMap<String, ThreadId>,
}

/// The sessions that want their raw frames reported, by the harness's thread id.
pub type RawTable = Arc<Mutex<HashMap<String, (ThreadId, Id)>>>;

#[derive(Clone)]
enum Delivery {
    Event(Box<AdapterEvent>),
    Reply(RequestId, Json),
    Rejected(RequestId, i64, String),
}

/// The state of one harness process.
pub struct Shared {
    sink: EventSink,
    event_gate: tokio::sync::Mutex<()>,
    event_batch: Mutex<VecDeque<Delivery>>,
    clock: Arc<dyn Clock>,
    outputs: OutputStore,
    rpc: Rpc,
    unloads: Mutex<HashMap<String, Vec<tokio::sync::oneshot::Sender<()>>>>,
    /// The native threads this process has loaded and not acknowledged unloading: a process started after
    /// one that was killed has loaded none, and has nothing to unload before a resume.
    loaded: Mutex<std::collections::HashSet<String>>,
    state: Mutex<State>,
    raw: RawTable,
    dead: AtomicBool,
    stopping: AtomicBool,
    version: String,
    /// Names this process in the references of its requests (see `reference_of`).
    run: String,
    exit: Arc<watch::Sender<Option<String>>>,
    /// The windows of the plan as the harness last reported them, for a failure that is a limit.
    windows: Mutex<Option<Windows>>,
    ended_commands: Mutex<std::collections::HashSet<String>>,
    /// How long a turn is waited for.
    patience: Patience,
    /// The sign-in that runs, and what is known of who is signed in.
    pub login: LoginTracker,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("dead", &self.dead.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How many characters of a new identifier make the token of a process: its last twelve hexadecimal digits hold at least
/// 32 bits that are drawn at random for each identifier, and a thread meets a few dozen processes in all its life, so
/// two that share a token are not to be expected.
const PROCESS_TOKEN_CHARS: usize = 12;

/// A token that names one process of the harness, from its start to its end.
///
/// The app-server counts the requests it makes of the client from 0 in each process. Orch keeps one approval for each
/// name a thread has been given, for the whole life of the thread, and takes a request that comes under a name it has
/// seen for the same request again. So when a process was started anew inside a thread's life (the connector restarted,
/// or the app-server crashed), the first questions of the new process had the names of the first questions of the one
/// before, Orch dropped them, and the turn waited for an answer nobody could give. The reference of a request carries
/// this token, so that no process repeats the references of an earlier one.
fn process_token() -> String {
    let id = orch_types::new_id().simple().to_string();
    id[id.len().saturating_sub(PROCESS_TOKEN_CHARS)..].to_owned()
}

fn command_identity(native: &str, item: &str, process: &str) -> String {
    // Both the item and session scope a reused number; lengths make native delimiters unambiguous.
    orch_types::Hash::of(
        format!(
            "{}:{native}{}:{item}{}:{process}",
            native.len(),
            item.len(),
            process.len()
        )
        .as_bytes(),
    )
    .to_string()
}

/// The thread of the harness a notification or a request is about: `threadId`, or the thread a
/// `thread/started` announces.
fn thread_of(params: &Json) -> Option<&str> {
    params.get("threadId").and_then(Json::as_str).or_else(|| {
        params
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Json::as_str)
    })
}

/// Makes the function that reports the raw frames of the sessions that asked for them.
#[must_use]
pub fn raw_tap(sink: EventSink, table: RawTable) -> FrameTap {
    Arc::new(
        move |direction: Direction, frame: &Json| -> futures::future::BoxFuture<'static, bool> {
            let target = frame
                .get("params")
                .and_then(thread_of)
                .and_then(|thread| lock(&table).get(thread).copied());
            let Some((thread_id, segment_id)) = target else {
                let sink = sink.clone();
                return Box::pin(async move { !sink.is_closed() });
            };
            let event = raw_frame(direction.name(), frame);
            let sink = sink.clone();
            Box::pin(async move { sink.session(thread_id, segment_id, event).await.is_ok() })
        },
    )
}

/// What a harness process that answered `initialize` is made of, for [`Shared::new`].
pub struct ProcessParts {
    /// Where the adapter reports.
    pub sink: EventSink,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// Where long outputs are kept.
    pub outputs: OutputStore,
    /// The connection to the process.
    pub rpc: Rpc,
    /// The sessions that want their raw frames reported.
    pub raw: RawTable,
    /// The version the process said it is.
    pub version: String,
    /// Where the task that waits for the process says how it ended.
    pub exit: Arc<watch::Sender<Option<String>>>,
    /// How long a turn is waited for.
    pub patience: Patience,
}

impl Shared {
    /// Waits for the native unload before resume can safely resolve a different instruction root.
    pub async fn unload_before_resume(&self, native: &str) -> Result<(), AdapterError> {
        // A session close has sixty seconds on the link; leave half for the RPCs around unloading.
        const WAIT: Duration = Duration::from_secs(30);
        // This process never loaded it (the one that did was killed, or this is a fresh start): no override of
        // this process can be active for it, and the resume takes it up as before.
        if !lock(&self.loaded).contains(native) {
            return Ok(());
        }
        let (send, receive) = tokio::sync::oneshot::channel();
        lock(&self.unloads)
            .entry(native.to_owned())
            .or_default()
            .push(send);
        let answer = self
            .rpc
            .call(
                "thread/unsubscribe",
                serde_json::json!({"threadId": native}),
            )
            .await;
        match answer {
            Ok(answer) if answer.get("status").and_then(Json::as_str) == Some("notLoaded") => {
                lock(&self.unloads).remove(native);
                return Ok(());
            }
            Err(error) => {
                lock(&self.unloads).remove(native);
                return Err(AdapterError::HarnessError(format!(
                    "the previous instruction root could not be released: {error}"
                )));
            }
            _ => {}
        }
        let result = tokio::time::timeout(WAIT, receive).await;
        lock(&self.unloads).remove(native);
        match result {
            Ok(Ok(())) => {
                lock(&self.loaded).remove(native);
                Ok(())
            }
            _ => Err(AdapterError::HarnessError("Codex did not acknowledge unloading; the old instruction-root override may still be active, so resume was refused".to_owned())),
        }
    }
    /// The state of a harness process that answered `initialize`.
    #[must_use]
    pub fn new(parts: ProcessParts) -> Arc<Self> {
        let ProcessParts {
            sink,
            clock,
            outputs,
            rpc,
            raw,
            version,
            exit,
            patience,
        } = parts;
        Arc::new(Self {
            sink,
            event_gate: tokio::sync::Mutex::new(()),
            event_batch: Mutex::new(VecDeque::new()),
            clock,
            outputs,
            rpc,
            state: Mutex::new(State::default()),
            unloads: Mutex::new(HashMap::new()),
            loaded: Mutex::new(std::collections::HashSet::new()),
            raw,
            dead: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            version,
            run: process_token(),
            exit,
            windows: Mutex::new(None),
            ended_commands: Mutex::new(std::collections::HashSet::new()),
            patience,
            login: LoginTracker::default(),
        })
    }

    /// The connection to the harness.
    #[must_use]
    pub fn rpc(&self) -> &Rpc {
        &self.rpc
    }

    /// Where the time comes from.
    #[must_use]
    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    /// Where long outputs are kept.
    #[must_use]
    pub fn outputs(&self) -> &OutputStore {
        &self.outputs
    }

    /// Where the adapter reports.
    #[must_use]
    pub fn sink(&self) -> &EventSink {
        &self.sink
    }

    /// Whether the process is gone.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
            || self.rpc.is_closed()
            || self.exit.borrow().is_some()
            || self.sink.is_closed()
    }

    /// Says that the adapter is ending the process on purpose.
    pub fn stopping(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    /// Waits for the process's exit to be known, for at most `grace`.
    pub async fn exit_message(&self, grace: Duration) -> Option<String> {
        let mut watcher = self.exit.subscribe();
        tokio::time::timeout(grace, async {
            loop {
                if let Some(message) = watcher.borrow_and_update().clone() {
                    return message;
                }
                if watcher.changed().await.is_err() {
                    return String::new();
                }
            }
        })
        .await
        .ok()
        .filter(|message| !message.is_empty())
    }

    /// How many sessions are open.
    #[must_use]
    pub fn sessions_open(&self) -> u32 {
        u32::try_from(lock(&self.state).sessions.len()).unwrap_or(u32::MAX)
    }

    /// The state of the process, reported for its row.
    #[must_use]
    pub fn status(&self, state: HarnessState, detail: Option<String>) -> HarnessStatus {
        HarnessStatus {
            status: state,
            detail,
            harness_version: Some(self.version.clone()),
            sessions_open: self.sessions_open(),
        }
    }

    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn instruction_notice(&self, thread: ThreadId, turn: Id) -> Result<(), AdapterError> {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        let result = self.stage_instruction_notice(thread, turn);
        self.flush_events().await;
        result
    }
    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn register(&self, record: SessionRecord, raw_events: bool, opened: SessionEvent) {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        self.stage_register(record, raw_events, opened);
        self.flush_events().await;
    }
    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn remove(&self, thread: ThreadId) -> Option<SessionRecord> {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        let result = self.stage_remove(thread);
        self.flush_events().await;
        result
    }
    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn report(&self, thread: ThreadId, event: SessionEvent) {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        self.stage_report(thread, event);
        self.flush_events().await;
    }
    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn on_notification(self: &Arc<Self>, method: &str, params: &Json) {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        if method == "warning"
            && thread_of(params).is_none()
            && crate::failure::row_name(params) == "credential_refused"
        {
            let mut after = None;
            loop {
                let next = {
                    let state = lock(&self.state);
                    let range = (
                        after.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded),
                        std::ops::Bound::Unbounded,
                    );
                    state
                        .sessions
                        .range(range)
                        .find(|(_, record)| record.mapper.turn_in_progress())
                        .map(|(id, record)| (*id, record.mapper.native_thread().to_owned()))
                };
                let Some((id, native)) = next else { break };
                after = Some(id);
                let mut targeted = params.clone();
                targeted["threadId"] = Json::String(native);
                self.route(method, &targeted);
                self.flush_events().await;
            }
        } else {
            self.stage_on_notification(method, params);
            self.flush_events().await;
        }
    }

    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn on_request(&self, id: RequestId, method: &str, params: &Json) {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        self.stage_on_request(id, method, params);
        self.flush_events().await;
    }
    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn report_facts(&self, facts: ProviderFacts) {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        self.stage_report_facts(facts);
        self.flush_events().await;
    }

    fn stage_event(&self, event: AdapterEvent) {
        lock(&self.event_batch).push_back(Delivery::Event(Box::new(event)));
    }

    fn queue_reply(&self, id: &RequestId, answer: Json) -> Result<(), RpcError> {
        self.can_reply()?;
        lock(&self.event_batch).push_back(Delivery::Reply(id.clone(), answer));
        Ok(())
    }

    fn queue_rejection(&self, id: &RequestId, code: i64, message: &str) -> Result<(), RpcError> {
        self.can_reply()?;
        lock(&self.event_batch).push_back(Delivery::Rejected(id.clone(), code, message.to_owned()));
        Ok(())
    }

    fn can_reply(&self) -> Result<(), RpcError> {
        if self.rpc.is_closed() {
            return Err(RpcError::Closed(
                self.rpc
                    .closed_reason()
                    .unwrap_or_else(|| "the harness input is closing".into()),
            ));
        }
        Ok(())
    }

    async fn flush_events(&self) -> bool {
        let mut delivered = true;
        loop {
            let next = lock(&self.event_batch).front().cloned();
            let Some(next) = next else { return delivered };
            let result = match next {
                Delivery::Event(event) => {
                    if self.sink.emit(*event).await.is_err() {
                        self.rpc
                            .fail_connection("the host stopped consuming the adapter's events");
                        lock(&self.event_batch).clear();
                        return false;
                    }
                    Ok(())
                }
                Delivery::Reply(id, answer) => self.rpc.respond(&id, answer).await,
                Delivery::Rejected(id, code, message) => {
                    self.rpc.respond_error(&id, code, &message).await
                }
            };
            if let Err(error) = result {
                self.rpc.fail_connection(&error.to_string());
                delivered = false;
            }
            // Retain the original until admission succeeds or its connection has explicitly failed.
            lock(&self.event_batch).pop_front();
        }
    }

    fn emit_session(&self, mapper: &Mapper, event: SessionEvent) {
        self.stage_event(AdapterEvent::Session {
            thread_id: mapper.thread_id(),
            segment_id: mapper.segment_id(),
            event,
        });
    }

    fn emit_mapped(&self, mapper: &mut Mapper, events: Vec<SessionEvent>) {
        let mut requests = Some(mapper.take_requests());
        let mut flush = || {
            for (turn_id, request) in requests.take().unwrap_or_default() {
                self.stage_event(AdapterEvent::ModelRequest(Box::new(
                    orch_agent_contract::requests::ModelRequestReport {
                        thread_id: mapper.thread_id(),
                        segment_id: mapper.segment_id(),
                        turn_id,
                        request,
                    },
                )));
            }
        };
        for event in events {
            if matches!(event, SessionEvent::TurnCompleted(_)) {
                flush();
            }
            self.emit_session(mapper, event);
        }
        flush();
    }

    /// Makes the resume limitation readable in the thread, without adding a model input.
    fn stage_instruction_notice(&self, thread: ThreadId, turn: Id) -> Result<(), AdapterError> {
        use orch_agent_contract::events::{
            ItemCompleted, ItemContent, ItemStarted, ItemStatus, MarkerPayload,
        };
        self.with_session(thread, |record| {
            if let Some(reference) = record.instructions_notice.take() {
                let payload = MarkerPayload::InstructionsDeferred {
                    program: "Codex".to_owned(),
                };
                let value = serde_json::json!(&payload);
                self.emit_session(
                    &record.mapper,
                    SessionEvent::ItemStarted(ItemStarted {
                        turn_id: turn,
                        native_ref: reference.clone(),
                        stream: "main".to_owned(),
                        parent_native_ref: None,
                        content: ItemContent::Marker(payload),
                    }),
                );
                self.emit_session(
                    &record.mapper,
                    SessionEvent::ItemCompleted(ItemCompleted {
                        native_ref: reference,
                        status: ItemStatus::Completed,
                        payload: value,
                        output_blob: None,
                    }),
                );
            }
        })
    }

    /// Adds a session to those the process serves, and reports that it is open.
    fn stage_register(&self, record: SessionRecord, raw_events: bool, opened: SessionEvent) {
        let thread = record.mapper.thread_id();
        let segment = record.mapper.segment_id();
        let native = record.mapper.native_thread().to_owned();
        let mut state = lock(&self.state);
        if let Some(earlier) = state.sessions.remove(&thread) {
            state.by_native.retain(|_, owner| *owner != thread);
            lock(&self.raw).retain(|_, (owner, _)| *owner != thread);
            drop(earlier);
        }
        state.by_native.insert(native.clone(), thread);
        lock(&self.loaded).insert(native.clone());
        if raw_events {
            lock(&self.raw).insert(native, (thread, segment));
        }
        self.stage_event(AdapterEvent::Session {
            thread_id: thread,
            segment_id: segment,
            event: opened,
        });
        state.sessions.insert(thread, record);
    }

    /// Takes a session away; its requests that wait are refused so that the harness does not wait for them.
    fn stage_remove(&self, thread: ThreadId) -> Option<SessionRecord> {
        let mut state = lock(&self.state);
        let mut record = state.sessions.remove(&thread)?;
        state.by_native.retain(|_, owner| *owner != thread);
        lock(&self.raw).retain(|_, (owner, _)| *owner != thread);
        for (reference, asked) in record.approvals.drain() {
            if !asked.pending.answered && !asked.pending.closed {
                self.refuse(&asked, &reference, &record.mapper);
            }
        }
        Some(record)
    }

    /// Runs `f` on a session.
    pub fn with_session<R>(
        &self,
        thread: ThreadId,
        f: impl FnOnce(&mut SessionRecord) -> R,
    ) -> Result<R, AdapterError> {
        let mut state = lock(&self.state);
        let record = state
            .sessions
            .get_mut(&thread)
            .ok_or_else(|| AdapterError::SessionNotFound(thread.to_string()))?;
        Ok(f(record))
    }

    /// Reports an event of a session, in order with the rest.
    fn stage_report(&self, thread: ThreadId, event: SessionEvent) {
        let state = lock(&self.state);
        if let Some(record) = state.sessions.get(&thread) {
            self.emit_session(&record.mapper, event);
        }
    }

    // -- what the harness says -----------------------------------------------------------------------

    /// Handles a notification of the harness.
    fn stage_on_notification(self: &Arc<Self>, method: &str, params: &Json) {
        if method == "thread/closed" {
            if let Some(native) = thread_of(params)
                && let Some(waiters) = lock(&self.unloads).remove(native)
            {
                for waiter in waiters {
                    let _ = waiter.send(());
                }
            }
            return;
        }
        match method {
            "account/rateLimits/updated" | "account/updated" => {
                if method == "account/rateLimits/updated"
                    && let Some(limits) =
                        params.get("rateLimits").filter(|limits| limits.is_object())
                {
                    *lock(&self.windows) = Some(Windows::read(limits));
                }
                if let Some(facts) =
                    provider::facts_from_update(method, params, &self.login, self.clock.time())
                {
                    self.stage_event(AdapterEvent::Facts(facts));
                }
            }
            "account/login/completed" => {
                if let Some(state) = self.login.completed(params) {
                    self.stage_event(AdapterEvent::Login(state));
                }
            }
            "serverRequest/resolved" => self.request_resolved(params),
            _ => self.route(method, params),
        }
    }

    fn route(self: &Arc<Self>, method: &str, params: &Json) {
        let Some(thread) = thread_of(params) else {
            return;
        };
        let parent = params
            .get("thread")
            .and_then(|thread| thread.get("parentThreadId"))
            .and_then(Json::as_str);
        let mut state = lock(&self.state);
        let owner = state
            .by_native
            .get(thread)
            .or_else(|| parent.and_then(|parent| state.by_native.get(parent)))
            .copied();
        let Some(owner) = owner else {
            return;
        };
        let windows = lock(&self.windows).clone();
        let ctx = Context {
            clock: self.clock.as_ref(),
            outputs: &self.outputs,
            windows: windows.as_ref(),
        };
        let Some(record) = state.sessions.get_mut(&owner) else {
            return;
        };
        let ends_the_turn = method == "turn/completed" && thread == record.mapper.native_thread();
        if ends_the_turn {
            // A turn that ends takes its requests with it; the harness says so after it, and that is not
            // reported twice.
            let mut open: Vec<String> = record
                .approvals
                .iter()
                .filter(|(_, asked)| !asked.pending.answered && !asked.pending.closed)
                .map(|(reference, _)| reference.clone())
                .collect();
            open.sort();
            for reference in open {
                if let Some(asked) = record.approvals.get_mut(&reference) {
                    asked.pending.closed = true;
                }
                self.emit_session(
                    &record.mapper,
                    SessionEvent::ApprovalClosed(ApprovalClosed {
                        request_ref: reference,
                    }),
                );
            }
        }
        let active = record.mapper.running_turn();
        let native_thread = record.mapper.native_thread().to_owned();
        let events = record.mapper.handle(method, params, &ctx);
        let rejected = events.iter().any(|event| matches!(event,
            SessionEvent::TurnCompleted(end) if end.status == TurnOutcome::Failed
                && end.error.as_ref().is_some_and(|error| error.code == turn_error::PROVIDER_UNAUTHORIZED)));
        let interrupted = events.iter().any(|event| {
            matches!(event,
            SessionEvent::TurnCompleted(end) if end.status == TurnOutcome::Interrupted)
        });
        let adopted = method == "thread/started" && record.mapper.follows(thread);
        let children: Vec<String> = record.mapper.child_threads().map(str::to_owned).collect();
        let raw_target = (record.mapper.thread_id(), record.mapper.segment_id());
        self.emit_mapped(&mut record.mapper, events);
        if adopted {
            state.by_native.insert(thread.to_owned(), owner);
            let mut raw = lock(&self.raw);
            if let Some(parent) = parent
                && raw.contains_key(parent)
            {
                raw.insert(thread.to_owned(), raw_target);
            }
        }
        // A spawn result may name a child before, or without, a separate thread/started notice.
        let root_is_raw = lock(&self.raw).contains_key(&native_thread);
        for child in children {
            if *state.by_native.entry(child.clone()).or_insert(owner) == owner && root_is_raw {
                lock(&self.raw).entry(child).or_insert(raw_target);
            }
        }
        drop(state);
        if let Some((turn, native_turn)) = active.filter(|_| rejected || interrupted) {
            let shared = Arc::clone(self);
            tokio::spawn(async move {
                if rejected {
                    let _ = shared
                        .rpc
                        .call_within(
                            "turn/interrupt",
                            serde_json::json!({ "threadId": native_thread, "turnId": native_turn }),
                            INTERRUPT_WAIT,
                        )
                        .await;
                }
                shared.end_commands(owner, turn, &native_thread).await;
            });
        }
    }

    /// Lists first, across every page, then ends only terminals whose items belong to the stopped turn.
    async fn end_commands(&self, thread: ThreadId, turn: Id, native: &str) {
        let pending = self
            .with_session(thread, |record| record.mapper.open_commands(turn))
            .unwrap_or_default();
        let ended = tokio::time::timeout(
            TERMINAL_CLEANUP_WAIT,
            self.terminate_terminals(thread, turn, native),
        )
        .await;
        let result = match ended {
            Ok(Ok(result)) => result,
            _ => orch_agent_contract::background::CommandCleanup {
                unknown: pending,
                ..Default::default()
            },
        };
        if result.unknown > 0 || result.still_running > 0 {
            tracing::warn!(%thread, "the harness could not confirm Stop's terminal cleanup; commands may still run");
            let _ = self.with_session(thread, |record| {
                for event in record.mapper.commands_may_still_run(turn) {
                    self.emit_session(&record.mapper, event);
                }
            });
        }
        let _ = self.with_session(thread, |record| {
            self.emit_session(
                &record.mapper,
                SessionEvent::TurnCommandCleanup(orch_agent_contract::events::TurnCommandCleanup {
                    turn_id: turn,
                    result,
                }),
            )
        });
    }

    /// Reads only this existing native session, retaining unknown facts as unknown.
    pub async fn background(
        &self,
        thread: ThreadId,
    ) -> Result<orch_agent_contract::background::NativeBackground, AdapterError> {
        use orch_agent_contract::background::{
            BackgroundPlace, BackgroundState, COMMAND_PREVIEW_BYTES, NativeBackground,
            NativeBackgroundCommand, SNAPSHOT_COMMANDS,
        };
        let (native, place) = self.with_session(thread, |record| {
            (
                record.mapper.native_thread().to_owned(),
                if record.remote.is_some() {
                    BackgroundPlace::Environment
                } else {
                    BackgroundPlace::Provider
                },
            )
        })?;
        let (terminals, complete) = match self.background_terminals(&native).await {
            Ok(observed) => observed,
            Err(RpcError::Remote { code, .. }) if code == METHOD_NOT_FOUND => {
                return Ok(NativeBackground::unsupported(
                    "this native build does not offer exact background command control",
                ));
            }
            Err(error) => return Err(crate::map_rpc(error)),
        };
        let mut items = Vec::new();
        for terminal in terminals.into_iter().take(SNAPSHOT_COMMANDS) {
            let turn_id = self.with_session(thread, |record| {
                record.mapper.command_turn(&terminal.item_id)
            })?;
            let mut command = terminal.command;
            let command_truncated = command.as_mut().is_some_and(|text| {
                let mut end = text.len().min(COMMAND_PREVIEW_BYTES);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                let shortened = end < text.len();
                text.truncate(end);
                shortened
            });
            items.push(NativeBackgroundCommand {
                id: command_identity(&native, &terminal.item_id, &terminal.process_id),
                native_item_ref: Some(terminal.item_id),
                turn_id,
                command,
                command_truncated,
                cwd: terminal.cwd,
                place,
                since: None,
                cpu_percent: None,
                memory_bytes: None,
            });
        }
        let total = complete.then(|| i64::try_from(items.len()).unwrap_or(i64::MAX));
        Ok(NativeBackground {
            supported: true,
            as_of: Some(self.clock.time()),
            state: BackgroundState::Live,
            reason: (!complete).then(|| {
                "the native list is too large; narrow it in the native program".to_owned()
            }),
            items,
            next_cursor: None,
            total,
        })
    }

    /// Re-lists before exact End so an old process number cannot name a replacement command.
    pub async fn end_background(
        &self,
        thread: ThreadId,
        id: &str,
    ) -> Result<orch_agent_contract::background::NativeBackgroundEnd, AdapterError> {
        use orch_agent_contract::background::{BackgroundEndState, NativeBackgroundEnd};
        let native =
            self.with_session(thread, |record| record.mapper.native_thread().to_owned())?;
        if lock(&self.ended_commands).contains(id) {
            return Ok(NativeBackgroundEnd {
                id: id.to_owned(),
                state: BackgroundEndState::AlreadyEnded,
                reason: None,
            });
        }
        let (terminals, _) = self
            .background_terminals(&native)
            .await
            .map_err(crate::map_rpc)?;
        let Some(terminal) = terminals.into_iter().find(|terminal| {
            command_identity(&native, &terminal.item_id, &terminal.process_id) == id
        }) else {
            return Ok(NativeBackgroundEnd {
                id: id.to_owned(),
                state: BackgroundEndState::Unknown,
                reason: Some("the exact command is no longer reported by this session".to_owned()),
            });
        };
        let answer = self
            .rpc
            .call(
                "thread/backgroundTerminals/terminate",
                serde_json::json!({ "threadId": native, "processId": terminal.process_id }),
            )
            .await
            .map_err(crate::map_rpc)?;
        let state = if answer.get("terminated").and_then(Json::as_bool) == Some(true) {
            lock(&self.ended_commands).insert(id.to_owned());
            BackgroundEndState::Ended
        } else {
            BackgroundEndState::Unknown
        };
        Ok(NativeBackgroundEnd {
            id: id.to_owned(),
            state,
            reason: (state == BackgroundEndState::Unknown)
                .then(|| "the native program did not confirm the command's end".to_owned()),
        })
    }

    async fn background_terminals(
        &self,
        native: &str,
    ) -> Result<(Vec<BackgroundTerminal>, bool), RpcError> {
        let method = "thread/backgroundTerminals/list";
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        loop {
            let page: BackgroundPage = self
                .rpc
                .request(
                    method,
                    &serde_json::json!({"threadId": native, "cursor": cursor}),
                )
                .await?;
            items.extend(page.data);
            if items.len() > orch_agent_contract::background::SNAPSHOT_COMMANDS {
                items.truncate(orch_agent_contract::background::SNAPSHOT_COMMANDS);
                return Ok((items, false));
            }
            cursor = page.next_cursor;
            let Some(next) = &cursor else {
                return Ok((items, true));
            };
            if !seen.insert(next.clone()) {
                return Err(RpcError::Shape {
                    method: method.to_owned(),
                    detail: "the terminal list repeated its cursor".to_owned(),
                });
            }
        }
    }

    async fn terminate_terminals(
        &self,
        thread: ThreadId,
        turn: Id,
        native: &str,
    ) -> Result<orch_agent_contract::background::CommandCleanup, RpcError> {
        let method = "thread/backgroundTerminals/list";
        let mut terminals = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        loop {
            let page: BackgroundPage = self
                .rpc
                .request(
                    method,
                    &serde_json::json!({ "threadId": native, "cursor": cursor }),
                )
                .await?;
            terminals.extend(page.data);
            cursor = page.next_cursor;
            let Some(next) = &cursor else { break };
            if !seen.insert(next.clone()) {
                return Err(RpcError::Shape {
                    method: method.to_owned(),
                    detail: "the terminal list repeated its cursor".to_owned(),
                });
            }
        }
        let mut result = orch_agent_contract::background::CommandCleanup::default();
        for terminal in terminals {
            let owned = self
                .with_session(thread, |record| {
                    record.mapper.owns_command(turn, &terminal.item_id)
                })
                .unwrap_or(false);
            if !owned {
                continue;
            }
            let answer = self
                .rpc
                .call(
                    "thread/backgroundTerminals/terminate",
                    serde_json::json!({ "threadId": native, "processId": terminal.process_id }),
                )
                .await;
            match answer {
                Ok(answer) if answer.get("terminated").and_then(Json::as_bool) == Some(true) => {
                    result.ended += 1;
                }
                Ok(_) => {
                    result.unknown += 1;
                }
                Err(_) => {
                    result.unknown += 1;
                }
            }
            // The app server reports item/completed when the process ends. It is that end, including its
            // exit code, that is shown; an RPC acknowledgement is never a synthetic cancellation.
        }
        Ok(result)
    }

    /// The reference by which Orch knows a request of the harness: the id the harness gave it, behind the token of this
    /// process. The harness counts its requests from 0 in each process, and the token keeps the reference of a request
    /// of a later process from being the reference of one of an earlier process (see `process_token`).
    fn reference_of(&self, id: &RequestId) -> String {
        format!("{}:{id}", self.run)
    }

    fn request_resolved(&self, params: &Json) {
        let Some(id) = params.get("requestId").and_then(RequestId::from_json) else {
            return;
        };
        let reference = self.reference_of(&id);
        let Some(thread) = thread_of(params) else {
            return;
        };
        let mut state = lock(&self.state);
        let Some(owner) = state.by_native.get(thread).copied() else {
            return;
        };
        let Some(record) = state.sessions.get_mut(&owner) else {
            return;
        };
        let Some(asked) = record.approvals.remove(&reference) else {
            return;
        };
        if matches!(asked.pending.kind, PendingKind::UserInput(_))
            && let Some(item) = &asked.pending.item
        {
            let events = record.mapper.user_input_resolved(item, self.clock.time());
            self.emit_mapped(&mut record.mapper, events);
        }
        if !asked.pending.answered && !asked.pending.closed {
            // Settled without an answer from here: the turn was interrupted, or the harness gave up.
            self.emit_session(
                &record.mapper,
                SessionEvent::ApprovalClosed(ApprovalClosed {
                    request_ref: reference,
                }),
            );
        }
    }

    /// Handles a request of the harness, which waits for an answer.
    fn stage_on_request(&self, id: RequestId, method: &str, params: &Json) {
        let thread = thread_of(params).map(str::to_owned);
        let mut state = lock(&self.state);
        let owner = thread
            .as_deref()
            .and_then(|thread| state.by_native.get(thread).copied());
        let Some(record) = owner.and_then(|owner| state.sessions.get_mut(&owner)) else {
            drop(state);
            self.refuse_unknown(&id, method, params);
            return;
        };
        let changes = params
            .get("itemId")
            .and_then(Json::as_str)
            .map(|item| record.mapper.file_changes_of(item))
            .unwrap_or_default();
        let Some(translated) = approvals::translate(method, params, changes) else {
            drop(state);
            self.refuse_unknown(&id, method, params);
            return;
        };
        let Some((turn_id, native_turn)) = record.mapper.running_turn() else {
            // Nobody is waiting on a turn of Orch for this: it cannot be shown, so it is refused.
            let _ = self.queue_reply(&id, approvals::refusal(&translated.pending.kind));
            return;
        };
        if params
            .get("turnId")
            .and_then(Json::as_str)
            .is_some_and(|turn| turn != native_turn.as_str())
        {
            // Native requests belong to their originating turn, even after this session starts another.
            let _ = self.queue_reply(&id, approvals::refusal(&translated.pending.kind));
            return;
        }
        if record.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Bypass
            && let Some(answer) = approvals::bypass_answer(&translated.pending.kind)
        {
            // Do not grant an uncorrelated request or let a planning turn leave its read-only boundary.
            let current_turn =
                params.get("turnId").and_then(Json::as_str) == Some(native_turn.as_str());
            let answer = if record.planning || !current_turn {
                approvals::refusal(&translated.pending.kind)
            } else {
                answer
            };
            let _ = self.queue_reply(&id, answer);
            return;
        }
        let reference = self.reference_of(&id);
        if matches!(translated.pending.kind, PendingKind::UserInput(_))
            && let Some(event) =
                record
                    .mapper
                    .user_input_requested(params, &translated.title, self.clock.time())
        {
            self.emit_session(&record.mapper, event);
        }
        record.approvals.insert(
            reference.clone(),
            Asked {
                id,
                pending: translated.pending,
            },
        );
        let event = SessionEvent::ApprovalRequested(ApprovalRequested {
            turn_id,
            request_ref: reference,
            title: translated.title,
            request: translated.request,
        });
        self.emit_session(&record.mapper, event);
        // The answer is sent when the person has chosen, by `resolve`, to the id the harness gave the request.
    }

    fn refuse(&self, asked: &Asked, reference: &str, mapper: &Mapper) {
        let _ = self.queue_reply(&asked.id, approvals::refusal(&asked.pending.kind));
        self.emit_session(
            mapper,
            SessionEvent::ApprovalClosed(ApprovalClosed {
                request_ref: reference.to_owned(),
            }),
        );
    }

    fn refuse_unknown(&self, id: &RequestId, method: &str, params: &Json) {
        tracing::debug!(method, "the harness made a request nobody can answer");
        let answer = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                self.queue_reply(id, serde_json::json!({ "decision": "cancel" }))
            }
            "item/permissions/requestApproval" => self.queue_reply(
                id,
                serde_json::json!({ "permissions": {}, "scope": "turn" }),
            ),
            "item/tool/requestUserInput" => {
                self.queue_reply(id, serde_json::json!({ "answers": {} }))
            }
            "mcpServer/elicitation/request" => self.queue_reply(
                id,
                serde_json::json!({ "action": "cancel", "content": null, "_meta": null }),
            ),
            _ => {
                let _ = params;
                self.queue_rejection(
                    id,
                    METHOD_NOT_FOUND,
                    "this client does not handle that request",
                )
            }
        };
        let _ = answer;
    }

    /// Answers a request of the harness with the decision of a person.
    pub async fn resolve(
        &self,
        thread: ThreadId,
        reference: &str,
        decision: &orch_agent_contract::session::ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let _gate = self.event_gate.lock().await;
        if !self.flush_events().await {
            return Err(AdapterError::HarnessUnavailable(
                "the prior harness reply could not be delivered".into(),
            ));
        }
        let result = self.stage_resolve(thread, reference, decision);
        if !self.flush_events().await {
            return Err(AdapterError::HarnessUnavailable(
                "the answer could not be given to the harness".into(),
            ));
        }
        result
    }

    fn stage_resolve(
        &self,
        thread: ThreadId,
        reference: &str,
        decision: &orch_agent_contract::session::ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let mut state = lock(&self.state);
        let record = state
            .sessions
            .get_mut(&thread)
            .ok_or_else(|| AdapterError::SessionNotFound(thread.to_string()))?;
        let asked = record.approvals.get_mut(reference).ok_or_else(|| {
            AdapterError::NotFound(format!("no request {reference} waits for an answer"))
        })?;
        if asked.pending.answered {
            return Ok(());
        }
        if asked.pending.closed {
            return Err(AdapterError::NotFound(format!(
                "the request {reference} was withdrawn by the harness"
            )));
        }
        let result = approvals::answer(&asked.pending, decision)?;
        self.queue_reply(&asked.id, result.clone())
            .map_err(|error| {
                AdapterError::HarnessUnavailable(format!(
                    "the answer could not be given to the harness: {error}"
                ))
            })?;
        asked.pending.answered = true;
        if matches!(asked.pending.kind, PendingKind::UserInput(_))
            && let Some(item) = &asked.pending.item
        {
            record.mapper.user_input_answered(item, &result);
        }
        Ok(())
    }

    /// Ends every session because the process is gone.
    pub async fn on_closed(&self, reason: &str) {
        if self.dead.swap(true, Ordering::SeqCst) {
            return;
        }
        self.rpc.process_exited(reason);
        let stopping = self.stopping.load(Ordering::SeqCst);
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        loop {
            let record = lock(&self.state).sessions.pop_first();
            let Some((_, mut record)) = record else { break };
            if !stopping {
                let events = record.mapper.lost(reason);
                self.emit_mapped(&mut record.mapper, events);
                self.flush_events().await;
            }
        }
        lock(&self.state).by_native.clear();
        lock(&self.raw).clear();
        self.stage_event(AdapterEvent::Status(HarnessStatus {
            status: if stopping {
                HarnessState::Offline
            } else {
                HarnessState::Error
            },
            detail: (!stopping).then(|| reason.to_owned()),
            harness_version: Some(self.version.clone()),
            sessions_open: 0,
        }));
        self.flush_events().await;
    }

    /// Reports the facts of the provider, as the harness last said.
    fn stage_report_facts(&self, facts: ProviderFacts) {
        self.stage_event(AdapterEvent::Facts(facts));
    }

    /// Looks whether a turn has run out of the patience it is given (see [`Patience`]) and, for each that has,
    /// ends it as failed and asks the harness to stop it. A turn's end is reported before the harness is asked,
    /// so that nothing waits on a harness that does not answer.
    pub async fn watch(&self) {
        let event_gate = self.event_gate.lock().await;
        self.flush_events().await;
        let mut after = None;
        let mut tired = Vec::new();
        loop {
            let observed = {
                let mut state = lock(&self.state);
                let range = (
                    after.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded),
                    std::ops::Bound::Unbounded,
                );
                let next = state.sessions.range_mut(range).next();
                next.map(|(id, record)| {
                    let id = *id;
                    let windows = lock(&self.windows).clone();
                    let ctx = Context { clock: self.clock.as_ref(), outputs: &self.outputs, windows: windows.as_ref() };
                    let asking = record.approvals.values().any(|asked| !asked.pending.answered && !asked.pending.closed);
                    let exhausted = record.mapper.give_up_if_tired(&ctx, &self.patience, asking);
                    if let Some((native_turn, events)) = exhausted {
                        tracing::warn!(thread = %record.mapper.thread_id(), "the harness is given up on and its turn ended");
                        let mut open: Vec<_> = record.approvals.iter()
                            .filter(|(_, asked)| !asked.pending.answered && !asked.pending.closed)
                            .map(|(reference, _)| reference.clone()).collect();
                        open.sort();
                        for reference in open {
                            if let Some(asked) = record.approvals.get_mut(&reference) { asked.pending.closed = true; }
                            self.emit_session(&record.mapper, SessionEvent::ApprovalClosed(ApprovalClosed {request_ref: reference}));
                        }
                        self.emit_mapped(&mut record.mapper, events);
                        (id, Some((record.mapper.native_thread().to_owned(), native_turn)))
                    } else { (id, None) }
                })
            };
            let Some((id, interrupt)) = observed else {
                break;
            };
            after = Some(id);
            if let Some(interrupt) = interrupt {
                tired.push(interrupt);
            }
            self.flush_events().await;
        }
        drop(event_gate);
        for (thread, turn) in tired {
            let _ = self
                .rpc
                .call_within(
                    "turn/interrupt",
                    serde_json::json!({"threadId":thread,"turnId":turn}),
                    INTERRUPT_WAIT,
                )
                .await;
        }
    }
}

/// Reads what the harness says until it ends, and hands it to the state.
pub async fn pump(
    shared: Arc<Shared>,
    mut incoming: orch_agent_contract::flow::Receiver<Incoming>,
) {
    let mut exit = shared.exit.subscribe();
    let mut reason = None;
    let mut watch = tokio::time::interval(WATCH_EVERY);
    watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if reason.is_none()
            && let Some(ended) = exit.borrow_and_update().clone()
        {
            reason = Some(ended);
            // The exit reporter uses at most the other half for stderr. Keep the original total bound.
        }
        tokio::select! {
            message = incoming.recv() => {
                match message {
                    Some(Incoming::Notification { method, params }) => shared.on_notification(&method, &params).await,
                    Some(Incoming::Request { id, method, params }) => shared.on_request(id, &method, &params).await,
                    Some(Incoming::Closed { reason: closed }) => {
                        let ended = match reason { Some(reason) => reason, None => shared.exit_message(EXIT_GRACE).await.unwrap_or(closed) };
                        shared.on_closed(&ended).await;
                        return;
                    }
                    None => break,
                }
            }
            _ = exit.changed(), if reason.is_none() => {},
            _ = watch.tick(), if reason.is_none() => shared.watch().await,
        }
    }
    let ended = match reason {
        Some(reason) => Some(reason),
        None => shared.exit_message(EXIT_GRACE).await,
    };
    shared
        .on_closed(
            ended
                .as_deref()
                .unwrap_or("the connection to the harness ended"),
        )
        .await;
}

/// The tools of the protocol module, for the code that connects.
pub use protocol::connect;
