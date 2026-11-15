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
use orch_types::{Clock, Id, Json, Time};
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

/// A small local evidence read must not hold back every session's event stream.
/// The registry supplies this default, and a profile can replace it within the interrupt window.
pub(crate) const DEFAULT_NATIVE_RESULT_WAIT_MS: u64 = 2_000;
/// The registry supplies this replaceable burst default; pressure stops only optional evidence.
pub(crate) const DEFAULT_NATIVE_RESULT_BUFFER_BYTES: usize = 8 * 1024 * 1024;

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
    image_sources: BTreeMap<ThreadId, crate::viewed_images::Sources>,
    result_gates: BTreeMap<ThreadId, Arc<tokio::sync::Mutex<()>>>,
    pending_ends: BTreeMap<ThreadId, PendingEnd>,
    by_native: HashMap<String, ThreadId>,
    unscoped_mcp: HashMap<String, Json>,
    unexpectedly_closed:
        std::collections::HashSet<orch_agent_contract::history::NativePrefixSource>,
}

#[derive(Clone)]
struct Heard {
    at: Time,
    input: Incoming,
}

impl Heard {
    fn params(&self) -> Option<&Json> {
        match &self.input {
            Incoming::Notification { params, .. } | Incoming::Request { params, .. } => {
                Some(params)
            }
            _ => None,
        }
    }

    fn introduces(&self, native: &str) -> bool {
        let Incoming::Notification { method, params } = &self.input else {
            return false;
        };
        if method == "thread/started" {
            return params.pointer("/thread/id").and_then(Json::as_str) == Some(native);
        }
        if method != "item/started" && method != "item/completed" {
            return false;
        }
        let Some(item) = params.get("item") else {
            return false;
        };
        item["type"] == "collabAgentToolCall"
            && item["tool"] == "spawnAgent"
            && item
                .get("receiverThreadIds")
                .and_then(Json::as_array)
                .is_some_and(|children| children.iter().any(|child| child.as_str() == Some(native)))
    }
}

struct PendingEnd {
    token: Id,
    segment: Id,
    root: String,
    end: Heard,
    queued: VecDeque<Heard>,
    bytes: usize,
    task: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Default)]
struct NativeResults {
    views: BTreeMap<crate::viewed_images::Key, crate::viewed_images::Prepared>,
    web: BTreeMap<crate::viewed_images::Key, crate::native_web::Checked>,
}

/// Replay a retained native frame at its actual receipt time, including usage and warning timestamps.
#[derive(Debug)]
struct HeardAt(Time);
impl Clock for HeardAt {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.get()
    }
}

#[cfg(test)]
struct ResultProbe {
    owner: ThreadId,
    started: tokio::sync::oneshot::Sender<()>,
    released: std::sync::mpsc::Receiver<()>,
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
    end_changed: tokio::sync::Notify,
    raw: RawTable,
    dead: AtomicBool,
    stopping: AtomicBool,
    version: String,
    background_control: bool,
    /// Names this process in the references of its requests (see `reference_of`).
    run: String,
    exit: Arc<watch::Sender<Option<String>>>,
    /// The windows of the plan as the harness last reported them, for a failure that is a limit.
    windows: Mutex<Option<Windows>>,
    ended_commands: Mutex<std::collections::HashSet<String>>,
    /// How long a turn is waited for.
    patience: Patience,
    native_result_wait: Duration,
    native_result_buffer_bytes: usize,
    #[cfg(test)]
    result_probe: Mutex<Option<ResultProbe>>,
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
    /// Both terminal-control methods are declared by this exact verified native build's schema.
    pub background_control: bool,
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
    /// Combined evidence wait before the turn's end, also bounding individual history reads.
    pub native_result_wait: Duration,
    /// Pending same-session input while optional evidence is read; pressure ends the read.
    pub native_result_buffer_bytes: usize,
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
            background_control,
            sink,
            clock,
            outputs,
            rpc,
            raw,
            version,
            exit,
            patience,
            native_result_wait,
            native_result_buffer_bytes,
        } = parts;
        Arc::new(Self {
            sink,
            event_gate: tokio::sync::Mutex::new(()),
            event_batch: Mutex::new(VecDeque::new()),
            clock,
            outputs,
            rpc,
            state: Mutex::new(State::default()),
            end_changed: tokio::sync::Notify::new(),
            unloads: Mutex::new(HashMap::new()),
            loaded: Mutex::new(std::collections::HashSet::new()),
            raw,
            dead: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            version,
            run: process_token(),
            background_control,
            exit,
            windows: Mutex::new(None),
            ended_commands: Mutex::new(std::collections::HashSet::new()),
            patience,
            native_result_wait,
            native_result_buffer_bytes,
            login: LoginTracker::default(),
            #[cfg(test)]
            result_probe: Mutex::new(None),
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

    pub(crate) fn background_control(&self) -> bool {
        self.background_control
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
    pub async fn register(
        self: &Arc<Self>,
        record: SessionRecord,
        raw_events: bool,
        opened: SessionEvent,
    ) {
        self.register_with_images(
            record,
            raw_events,
            opened,
            crate::viewed_images::Sources::default(),
        )
        .await;
    }
    pub(crate) async fn register_with_images(
        self: &Arc<Self>,
        record: SessionRecord,
        raw_events: bool,
        opened: SessionEvent,
        sources: crate::viewed_images::Sources,
    ) {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        let thread = record.mapper.thread_id();
        self.drain_pending(thread, "native_result_session_changed");
        self.stage_register(record, raw_events, opened);
        lock(&self.state).image_sources.insert(thread, sources);
        if !self.is_dead() {
            self.stage_event(AdapterEvent::Status(
                self.status(HarnessState::Online, None),
            ));
        }
        self.flush_events().await;
    }
    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn remove(self: &Arc<Self>, thread: ThreadId) -> Option<SessionRecord> {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        self.drain_pending(thread, "native_result_session_closed");
        let result = self.stage_remove(thread);
        if result.is_some() && !self.is_dead() {
            self.stage_event(AdapterEvent::Status(
                self.status(HarnessState::Online, None),
            ));
        }
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
        if method == "warning" && thread_of(params).is_none() {
            let credential_refused = crate::failure::row_name(params) == "credential_refused";
            if !credential_refused
                && let Some(message) = params
                    .get("message")
                    .and_then(Json::as_str)
                    .filter(|message| !message.trim().is_empty())
            {
                self.stage_event(AdapterEvent::Status(
                    self.status(HarnessState::Online, Some(message.into())),
                ));
                self.flush_events().await;
            }
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
                        .find(|(_, record)| !credential_refused || record.mapper.turn_in_progress())
                        .map(|(id, record)| (*id, record.mapper.native_thread().to_owned()))
                };
                let Some((id, native)) = next else { break };
                after = Some(id);
                let mut targeted = params.clone();
                targeted["threadId"] = Json::String(native);
                self.dispatch(
                    Heard {
                        at: self.clock.time(),
                        input: Incoming::Notification {
                            method: method.into(),
                            params: targeted,
                        },
                    },
                    None,
                );
                self.flush_events().await;
            }
        } else {
            self.dispatch(
                Heard {
                    at: self.clock.time(),
                    input: Incoming::Notification {
                        method: method.into(),
                        params: params.clone(),
                    },
                },
                None,
            );
            self.flush_events().await;
        }
    }

    fn owner_of(&self, params: &Json) -> Option<ThreadId> {
        let state = lock(&self.state);
        thread_of(params)
            .and_then(|thread| state.by_native.get(thread))
            .or_else(|| {
                params
                    .pointer("/thread/parentThreadId")
                    .and_then(Json::as_str)
                    .and_then(|parent| state.by_native.get(parent))
            })
            .copied()
    }

    fn waiting_introduction(&self, params: &Json) -> Option<ThreadId> {
        let native = thread_of(params);
        let parent = params
            .pointer("/thread/parentThreadId")
            .and_then(Json::as_str);
        lock(&self.state)
            .pending_ends
            .iter()
            .find_map(|(owner, pending)| {
                pending
                    .queued
                    .iter()
                    .any(|frame| {
                        native.is_some_and(|native| frame.introduces(native))
                            || parent.is_some_and(|parent| frame.introduces(parent))
                    })
                    .then_some(*owner)
            })
    }

    /// Only the session whose end is waiting retains later frames. No optional IO owns the event gate.
    fn dispatch(self: &Arc<Self>, frame: Heard, skipped: Option<&'static str>) {
        let mut frames = VecDeque::from([(frame, skipped)]);
        while let Some((frame, skipped)) = frames.pop_front() {
            let mut owner = frame.params().and_then(|params| self.owner_of(params));
            if owner.is_none()
                && let Some(params) = frame.params()
            {
                // A retained spawn may be the only introduction of this child. Admit its earlier frames
                // before resolving ownership; never lose new child input behind an optional read.
                loop {
                    let waiting = self.waiting_introduction(params);
                    let Some(waiting) = waiting else {
                        break;
                    };
                    self.drain_pending(waiting, "native_result_ordering_unknown");
                }
                owner = frame.params().and_then(|params| self.owner_of(params));
            }
            if let Some(owner) = owner {
                let weight = protocol::incoming_weight(&frame.input);
                let pending = {
                    let mut state = lock(&self.state);
                    state.pending_ends.get_mut(&owner).map(|pending| {
                        if skipped.is_none()
                            && pending.bytes.saturating_add(weight)
                                <= self.native_result_buffer_bytes
                        {
                            pending.bytes = pending.bytes.saturating_add(weight);
                            pending.queued.push_back(frame.clone());
                            true
                        } else {
                            false
                        }
                    })
                };
                if pending == Some(true) {
                    continue;
                }
                if pending == Some(false) {
                    let reason = skipped.unwrap_or("native_result_buffer_full");
                    let mut queued = self.finish_pending(owner, None, Err(reason), true);
                    queued.push_back(frame);
                    for frame in queued.into_iter().rev() {
                        frames.push_front((frame, Some(reason)));
                    }
                    continue;
                }
            }
            if let Incoming::Notification { method, params } = &frame.input
                && method == "turn/completed"
            {
                if skipped.is_none() && self.defer_native_end(&frame) {
                    continue;
                }
                if let Some(reason) = skipped {
                    self.unavailable_native_results(params, reason);
                }
            }
            self.stage_heard(frame);
        }
    }

    fn stage_heard(self: &Arc<Self>, frame: Heard) {
        match frame.input {
            Incoming::Notification { method, params } => {
                self.stage_on_notification(&method, &params, frame.at)
            }
            Incoming::Request { id, method, params } => {
                self.stage_on_request(id, &method, &params, frame.at)
            }
            _ => {}
        }
    }

    fn defer_native_end(self: &Arc<Self>, frame: &Heard) -> bool {
        let Some(params) = frame.params() else {
            return false;
        };
        let Some(native) = thread_of(params) else {
            return false;
        };
        let Some(turn) = params.pointer("/turn/id").and_then(Json::as_str) else {
            return false;
        };
        let Some(owner) = self.owner_of(params) else {
            return false;
        };
        let token = orch_types::new_id();
        let (segment, root, views, snapshots) = {
            let mut state = lock(&self.state);
            let Some(record) = state.sessions.get(&owner) else {
                return false;
            };
            let views = record.mapper.pending_view_keys(native, turn);
            let snapshots = record.mapper.pending_web_results(native, turn);
            if views.is_empty() && snapshots.is_empty() {
                return false;
            }
            let segment = record.mapper.segment_id();
            let root = record.mapper.native_thread().to_owned();
            state.pending_ends.insert(
                owner,
                PendingEnd {
                    token,
                    segment,
                    root: root.clone(),
                    end: frame.clone(),
                    queued: VecDeque::new(),
                    bytes: 0,
                    task: None,
                },
            );
            (segment, root, views, snapshots)
        };
        let shared = Arc::clone(self);
        let task = tokio::spawn(async move {
            let keys = views
                .iter()
                .cloned()
                .chain(snapshots.keys().cloned())
                .collect();
            let work = shared.native_result_work(
                owner,
                segment,
                &root,
                keys,
                move |source, _, outputs| NativeResults {
                    views: source.prepare(&views, outputs),
                    web: crate::native_web::checked_results(
                        source,
                        &snapshots.keys().cloned().collect::<Vec<_>>(),
                        &snapshots,
                        outputs,
                    ),
                },
            );
            let results = tokio::time::timeout(shared.native_result_wait, work)
                .await
                .unwrap_or(Err("native_result_timed_out"));
            let _gate = shared.event_gate.lock().await;
            shared.flush_events().await;
            let queued = shared.finish_pending(owner, Some(token), results, false);
            for frame in queued {
                shared.dispatch(
                    frame,
                    shared.is_dead().then_some("native_result_session_closed"),
                );
            }
            shared.flush_events().await;
        });
        if let Some(pending) = lock(&self.state).pending_ends.get_mut(&owner) {
            pending.task = Some(task);
        }
        true
    }

    /// Called under the event gate. Stale work cannot finish a reopened session or charge its turn.
    fn finish_pending(
        self: &Arc<Self>,
        owner: ThreadId,
        token: Option<Id>,
        results: Result<NativeResults, &'static str>,
        abort: bool,
    ) -> VecDeque<Heard> {
        let pending = {
            let mut state = lock(&self.state);
            if !state
                .pending_ends
                .get(&owner)
                .is_some_and(|pending| token.is_none_or(|token| token == pending.token))
            {
                return VecDeque::new();
            }
            state.pending_ends.remove(&owner)
        };
        let Some(mut pending) = pending else {
            return VecDeque::new();
        };
        self.end_changed.notify_waiters();
        if abort {
            if let Some(task) = pending.task.take() {
                task.abort();
            }
            if let Some(source) = lock(&self.state).image_sources.get(&owner) {
                source.cancellation().store(true, Ordering::Release);
            }
        }
        let valid = lock(&self.state)
            .sessions
            .get(&owner)
            .is_some_and(|record| {
                record.mapper.segment_id() == pending.segment
                    && record.mapper.native_thread() == pending.root
            });
        if !valid {
            return VecDeque::new();
        }
        match results {
            Ok(results) => {
                let mut state = lock(&self.state);
                if let Some(record) = state.sessions.get_mut(&owner) {
                    let mut events = record.mapper.complete_view_results(results.views);
                    events.extend(record.mapper.complete_web_results(results.web));
                    self.emit_mapped(&mut record.mapper, events);
                }
            }
            Err(reason) => {
                if let Some(params) = pending.end.params() {
                    self.unavailable_native_results(params, reason);
                }
            }
        }
        self.stage_heard(pending.end);
        pending.queued
    }

    fn drain_pending(self: &Arc<Self>, owner: ThreadId, reason: &'static str) {
        let queued = self.finish_pending(owner, None, Err(reason), true);
        for frame in queued {
            self.dispatch(frame, Some(reason));
        }
    }

    /// The same cancellable exact-result worker serves image retention and native web proof.
    async fn native_result_work<R: Send + 'static>(
        &self,
        thread: ThreadId,
        segment: orch_types::Id,
        native: &str,
        mut keys: Vec<crate::viewed_images::Key>,
        prepare: impl FnOnce(
            &mut crate::viewed_images::Sources,
            &[crate::viewed_images::Key],
            &OutputStore,
        ) -> R
        + Send
        + 'static,
    ) -> Result<R, &'static str> {
        let gate = lock(&self.state)
            .result_gates
            .entry(thread)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let owner_gate = tokio::time::timeout(self.native_result_wait, gate.lock_owned())
            .await
            .map_err(|_| "native_result_timed_out")?;
        let work = {
            let mut state = lock(&self.state);
            let Some(record) = state.sessions.get(&thread).filter(|record| {
                record.mapper.segment_id() == segment && record.mapper.native_thread() == native
            }) else {
                return Err("native_result_session_changed");
            };
            keys.retain(|key| record.mapper.follows(&key.native_thread));
            state.image_sources.entry(thread).or_default().take_work()
        };
        let mut cancelled = crate::native_history::CancelOnDrop(Some(work.cancellation()));
        let outputs = self.outputs.clone();
        #[cfg(test)]
        let probe = {
            let mut probe = lock(&self.result_probe);
            if probe.as_ref().is_some_and(|probe| probe.owner == thread) {
                probe.take()
            } else {
                None
            }
        };
        let worker = tokio::task::spawn_blocking(move || {
            // Cancellation releases the async wait, but a blocked filesystem call keeps this owner's IO
            // permit until it really returns. Repeated turns cannot pile up blocked readers of one source.
            #[cfg(test)]
            if let Some(probe) = probe {
                let _ = probe.started.send(());
                let _ = probe.released.recv();
            }
            let mut work = work;
            let values = prepare(&mut work, &keys, &outputs);
            (work, values, owner_gate)
        });
        let result = match tokio::time::timeout(self.native_result_wait, worker).await {
            Ok(result) => result,
            Err(_) => return Err("native_result_timed_out"),
        };
        cancelled.disarm();
        match result {
            Ok((work, values, _owner)) => {
                let mut state = lock(&self.state);
                if state.sessions.get(&thread).is_some_and(|record| {
                    record.mapper.segment_id() == segment && record.mapper.native_thread() == native
                }) && let Some(sources) = state.image_sources.get_mut(&thread)
                    && Arc::ptr_eq(&sources.cancellation(), &work.cancellation())
                    && !work.cancellation().load(Ordering::Acquire)
                {
                    sources.finish_work(work);
                    Ok(values)
                } else {
                    Err("native_result_session_changed")
                }
            }
            Err(_) => Err("native_result_worker_unavailable"),
        }
    }

    async fn image_results_owned(
        &self,
        thread: ThreadId,
        segment: orch_types::Id,
        native: &str,
        keys: Vec<crate::viewed_images::Key>,
    ) -> BTreeMap<crate::viewed_images::Key, crate::viewed_images::Prepared> {
        if keys.is_empty() {
            return BTreeMap::new();
        }
        Box::pin(self.native_result_work(
            thread,
            segment,
            native,
            keys.clone(),
            |source, keys, outputs| source.prepare(keys, outputs),
        ))
        .await
        .unwrap_or_else(|reason| keys.into_iter().map(|key| (key, Err(reason))).collect())
    }

    pub(crate) async fn web_results(
        &self,
        thread: ThreadId,
        segment: orch_types::Id,
        native: &str,
        keys: Vec<crate::viewed_images::Key>,
        reported: BTreeMap<crate::viewed_images::Key, Json>,
    ) -> BTreeMap<crate::viewed_images::Key, crate::native_web::Proof> {
        if keys.is_empty() {
            return BTreeMap::new();
        }
        Box::pin(self.native_result_work(
            thread,
            segment,
            native,
            keys.clone(),
            move |source, keys, outputs| source.prepare_web_inputs(keys, &reported, outputs),
        ))
        .await
        .unwrap_or_else(|reason| {
            keys.into_iter()
                .map(|key| (key, crate::native_web::Proof::unavailable(reason)))
                .collect()
        })
    }

    fn unavailable_native_results(&self, params: &Json, reason: &'static str) {
        let Some(native) = thread_of(params) else {
            return;
        };
        let Some(turn) = params.pointer("/turn/id").and_then(Json::as_str) else {
            return;
        };
        let mut state = lock(&self.state);
        let Some(owner) = state.by_native.get(native).copied() else {
            return;
        };
        let Some(record) = state.sessions.get_mut(&owner) else {
            return;
        };
        let views = record
            .mapper
            .pending_view_keys(native, turn)
            .into_iter()
            .map(|key| (key, Err(reason)))
            .collect();
        let mut events = record.mapper.complete_view_results(views);
        let web = record
            .mapper
            .pending_web_results(native, turn)
            .into_keys()
            .map(|key| (key, (crate::native_web::Proof::unavailable(reason), None)))
            .collect();
        events.extend(record.mapper.complete_web_results(web));
        self.emit_mapped(&mut record.mapper, events);
    }

    pub(crate) async fn image_results(
        &self,
        thread: ThreadId,
        segment: orch_types::Id,
        native: &str,
        keys: Vec<crate::viewed_images::Key>,
    ) -> BTreeMap<crate::viewed_images::Key, crate::viewed_images::Prepared> {
        Box::pin(self.image_results_owned(thread, segment, native, keys)).await
    }

    /// Maps and delivers one bounded operation outside the state lock.
    pub async fn on_request(self: &Arc<Self>, id: RequestId, method: &str, params: &Json) {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        self.dispatch(
            Heard {
                at: self.clock.time(),
                input: Incoming::Request {
                    id,
                    method: method.into(),
                    params: params.clone(),
                },
            },
            None,
        );
        self.flush_events().await;
    }

    /// Invalid stdout is process-scoped: retain its text only when exactly one session owns this process.
    pub async fn on_malformed(&self, text: &str) {
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        let sole = {
            let state = lock(&self.state);
            (state.sessions.len() == 1)
                .then(|| state.sessions.keys().next().copied())
                .flatten()
        };
        let mut after = None;
        loop {
            let next = {
                let mut state = lock(&self.state);
                let next = state
                    .sessions
                    .keys()
                    .copied()
                    .find(|id| after.is_none_or(|last| *id > last));
                if let Some(id) = next
                    && let Some(record) = state.sessions.get_mut(&id)
                {
                    let event = record
                        .mapper
                        .malformed_frame((sole == Some(id)).then_some(text));
                    self.emit_session(&record.mapper, event);
                }
                next
            };
            let Some(id) = next else { break };
            after = Some(id);
            self.flush_events().await;
        }
        if after.is_none() {
            self.stage_event(AdapterEvent::Status(
                self.status(
                    HarnessState::Online,
                    Some(
                        "Codex emitted a malformed protocol frame before any session was opened."
                            .into(),
                    ),
                ),
            ));
            self.flush_events().await;
        }
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
    fn stage_register(&self, mut record: SessionRecord, raw_events: bool, opened: SessionEvent) {
        let thread = record.mapper.thread_id();
        let segment = record.mapper.segment_id();
        let native = record.mapper.native_thread().to_owned();
        let mut state = lock(&self.state);
        state.image_sources.remove(&thread);
        state.result_gates.remove(&thread);
        if let Some(earlier) = state.sessions.remove(&thread) {
            state.by_native.retain(|_, owner| *owner != thread);
            lock(&self.raw).retain(|_, (owner, _)| *owner != thread);
            drop(earlier);
        }
        state.by_native.insert(native.clone(), thread);
        lock(&self.loaded).insert(native.clone());
        if raw_events {
            lock(&self.raw).insert(native.clone(), (thread, segment));
        }
        let resumed = matches!(&opened,SessionEvent::SessionOpened(opened) if opened.resumed)
            && state.unexpectedly_closed.remove(
                &orch_agent_contract::history::NativePrefixSource {
                    thread_id: thread,
                    segment_id: segment,
                    native_session_ref: native.clone(),
                },
            );
        self.stage_event(AdapterEvent::Session {
            thread_id: thread,
            segment_id: segment,
            event: opened,
        });
        if resumed {
            let events = record.mapper.resumed_after_close(self.clock.time());
            self.emit_mapped(&mut record.mapper, events);
        }
        state.sessions.insert(thread, record);
    }

    /// Takes a session away; its requests that wait are refused so that the harness does not wait for them.
    fn stage_remove(&self, thread: ThreadId) -> Option<SessionRecord> {
        let mut state = lock(&self.state);
        let mut record = state.sessions.remove(&thread)?;
        state.image_sources.remove(&thread);
        state.result_gates.remove(&thread);
        state.by_native.retain(|_, owner| *owner != thread);
        lock(&self.raw).retain(|_, (owner, _)| *owner != thread);
        for (reference, asked) in record.approvals.drain() {
            if !asked.pending.answered && !asked.pending.closed {
                self.refuse(&asked, &reference, &record.mapper);
            }
        }
        Some(record)
    }

    /// Current native ownership is private to this process, including after catalogue partitioning.
    pub(crate) fn owns_native(&self, native: &str) -> bool {
        lock(&self.state).by_native.contains_key(native)
    }

    /// An acknowledged unload with no session retains no process-owned native work.
    pub(crate) fn quiescent(&self) -> bool {
        let empty = lock(&self.state).sessions.is_empty();
        empty && lock(&self.loaded).is_empty()
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
    fn stage_on_notification(self: &Arc<Self>, method: &str, params: &Json, at: Time) {
        if method == "thread/closed" {
            if let Some(native) = thread_of(params) {
                lock(&self.loaded).remove(native);
                if let Some(waiters) = lock(&self.unloads).remove(native) {
                    for waiter in waiters {
                        let _ = waiter.send(());
                    }
                    return;
                }
                let source = {
                    let state = lock(&self.state);
                    state
                        .by_native
                        .get(native)
                        .and_then(|owner| state.sessions.get(owner))
                        .filter(|record| record.mapper.native_thread() == native)
                        .map(|record| orch_agent_contract::history::NativePrefixSource {
                            thread_id: record.mapper.thread_id(),
                            segment_id: record.mapper.segment_id(),
                            native_session_ref: native.into(),
                        })
                };
                self.route(method, params, at);
                if let Some(source) = source {
                    lock(&self.state).unexpectedly_closed.insert(source.clone());
                    // Reuse ordinary missing-session repair rather than dispatch into an unloaded thread.
                    self.stage_remove(source.thread_id);
                }
            }
            return;
        }
        match method {
            "mcpServer/startupStatus/updated" if thread_of(params).is_none() => {
                let Some(name) = params
                    .get("name")
                    .and_then(Json::as_str)
                    .filter(|name| !name.is_empty())
                else {
                    return;
                };
                let Some(status) = params.get("status").and_then(Json::as_str) else {
                    return;
                };
                let words = params.get("error").and_then(Json::as_str);
                let value = serde_json::json!({"status":status,"error":words,"failure_reason":params.get("failureReason").and_then(Json::as_str)});
                let changed = lock(&self.state)
                    .unscoped_mcp
                    .insert(name.into(), value.clone())
                    .as_ref()
                    != Some(&value);
                if changed {
                    self.stage_event(AdapterEvent::Status(self.status(
                        HarnessState::Online,
                        Some(format!(
                            "Codex MCP server {name}: {status}{}",
                            words.map(|words| format!(". {words}")).unwrap_or_default()
                        )),
                    )));
                }
            }
            "account/rateLimits/updated" | "account/updated" => {
                if method == "account/rateLimits/updated"
                    && let Some(limits) =
                        params.get("rateLimits").filter(|limits| limits.is_object())
                {
                    *lock(&self.windows) = Some(Windows::read(limits));
                }
                if let Some(facts) = provider::facts_from_update(method, params, &self.login, at) {
                    self.stage_event(AdapterEvent::Facts(facts));
                }
            }
            "account/login/completed" => {
                if let Some(state) = self.login.completed(params) {
                    self.stage_event(AdapterEvent::Login(state));
                }
            }
            "serverRequest/resolved" => self.request_resolved(params, at),
            _ => self.route(method, params, at),
        }
    }

    fn route(self: &Arc<Self>, method: &str, params: &Json, at: Time) {
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
            clock: &HeardAt(at),
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
        let image_source = (method == "thread/started" && record.mapper.follows(thread))
            .then(|| serde_json::json!({"id":thread,"path":params.pointer("/thread/path")}));
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
        if let Some(metadata) = image_source
            && let Some(source) = state.image_sources.get_mut(&owner)
        {
            source.observe(&metadata);
        }
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
        let result = if pending == 0 {
            // No reported command belongs to this turn; a list cannot authorize an unknown one.
            orch_agent_contract::background::CommandCleanup::default()
        } else {
            match tokio::time::timeout(
                TERMINAL_CLEANUP_WAIT,
                self.terminate_terminals(thread, turn, native),
            )
            .await
            {
                Ok(Ok(result)) => result,
                _ => orch_agent_contract::background::CommandCleanup {
                    unknown: pending,
                    ..Default::default()
                },
            }
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
        if !self.background_control {
            return Ok(NativeBackground::unsupported(
                crate::native_schema::END_UNAVAILABLE,
            ));
        }
        let (terminals, complete) = match self.background_terminals(&native).await {
            Ok(observed) => observed,
            Err(RpcError::Remote { code, .. }) if code == METHOD_NOT_FOUND => {
                return Ok(NativeBackground::unsupported(
                    "this native build does not offer exact background command control",
                ));
            }
            Err(RpcError::Shape { .. }) => {
                return Ok(NativeBackground::unsupported(
                    crate::native_schema::FORM_UNKNOWN,
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
        if !self.background_control {
            return Ok(NativeBackgroundEnd {
                id: id.to_owned(),
                state: BackgroundEndState::Unknown,
                reason: Some(crate::native_schema::END_UNAVAILABLE.into()),
            });
        }
        if lock(&self.ended_commands).contains(id) {
            return Ok(NativeBackgroundEnd {
                id: id.to_owned(),
                state: BackgroundEndState::AlreadyEnded,
                reason: None,
            });
        }
        let terminals = match self.background_terminals(&native).await {
            Ok((terminals, _)) => terminals,
            Err(RpcError::Shape { .. }) => {
                return Ok(NativeBackgroundEnd {
                    id: id.to_owned(),
                    state: BackgroundEndState::Unknown,
                    reason: Some(crate::native_schema::FORM_UNKNOWN.into()),
                });
            }
            Err(error) => return Err(crate::map_rpc(error)),
        };
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
        if !self.background_control {
            return Err(RpcError::Remote {
                method: method.into(),
                code: METHOD_NOT_FOUND,
                message: crate::native_schema::END_UNAVAILABLE.into(),
                data: None,
            });
        }
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
        if !self.background_control {
            return Err(RpcError::Remote {
                method: method.into(),
                code: METHOD_NOT_FOUND,
                message: crate::native_schema::END_UNAVAILABLE.into(),
                data: None,
            });
        }
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

    fn request_resolved(&self, params: &Json, at: Time) {
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
            let events = record.mapper.user_input_resolved(item, at);
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
    fn stage_on_request(&self, id: RequestId, method: &str, params: &Json, at: Time) {
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
            let windows = lock(&self.windows).clone();
            let ctx = Context {
                clock: &HeardAt(at),
                outputs: &self.outputs,
                windows: windows.as_ref(),
            };
            for event in record.mapper.unrecognized(method, params, &ctx) {
                self.emit_session(&record.mapper, event);
            }
            drop(state);
            self.refuse_unknown(&id, method, params);
            return;
        };
        let Some((turn_id, native_turn)) = record.mapper.running_turn() else {
            // Nobody is waiting on a turn of Orch for this: it cannot be shown, so it is refused.
            let _ = self.queue_refusal(&id, &translated.pending.kind);
            return;
        };
        if params
            .get("turnId")
            .and_then(Json::as_str)
            .is_some_and(|turn| turn != native_turn.as_str())
        {
            // Native requests belong to their originating turn, even after this session starts another.
            let _ = self.queue_refusal(&id, &translated.pending.kind);
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
            && let Some(event) = record
                .mapper
                .user_input_requested(params, &translated.title, at)
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
        let _ = self.queue_refusal(&asked.id, &asked.pending.kind);
        self.emit_session(
            mapper,
            SessionEvent::ApprovalClosed(ApprovalClosed {
                request_ref: reference.to_owned(),
            }),
        );
    }

    fn queue_refusal(&self, id: &RequestId, kind: &PendingKind) -> Result<(), RpcError> {
        if matches!(kind, PendingKind::UnrecognizedRequest) {
            self.queue_rejection(
                id,
                METHOD_NOT_FOUND,
                "this client cannot answer an unrecognized approval",
            )
        } else {
            self.queue_reply(id, approvals::refusal(kind))
        }
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
        let sent = if matches!(asked.pending.kind, PendingKind::UnrecognizedRequest) {
            self.queue_refusal(&asked.id, &asked.pending.kind)
        } else {
            self.queue_reply(&asked.id, result.clone())
        };
        sent.map_err(|error| {
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
    pub async fn on_closed(self: &Arc<Self>, reason: &str) {
        if self.dead.swap(true, Ordering::SeqCst) {
            return;
        }
        self.rpc.process_exited(reason);
        let stopping = self.stopping.load(Ordering::SeqCst);
        // A native end already received still gets its bounded proof, even if stdout closes just after it.
        // Wait outside the event gate: the parallel result tasks need that gate to deliver their ends.
        loop {
            let changed = self.end_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if lock(&self.state).pending_ends.is_empty() {
                break;
            }
            changed.await;
        }
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        loop {
            let owner = lock(&self.state).pending_ends.keys().next().copied();
            let Some(owner) = owner else {
                break;
            };
            self.drain_pending(owner, "native_result_session_closed");
            self.flush_events().await;
        }
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
        lock(&self.state).image_sources.clear();
        lock(&self.state).result_gates.clear();
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
        let ended: std::collections::HashSet<_> =
            lock(&self.state).pending_ends.keys().copied().collect();
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
                    if ended.contains(&id) { return (id, None); }
                    let windows = lock(&self.windows).clone();
                    let ctx = Context { clock: self.clock.as_ref(), outputs: &self.outputs, windows: windows.as_ref() };
                    let expired = record.mapper.expire_silent_web_actions(self.clock.time());
                    self.emit_mapped(&mut record.mapper, expired);
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
                    Some(Incoming::Malformed { text }) => shared.on_malformed(&text).await,
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    use super::*;
    use futures::StreamExt;
    use orch_agent_contract::events::ItemStatus;
    use serde_json::json;

    #[tokio::test]
    async fn a_slow_sessions_evidence_does_not_hold_another_sessions_web_end_or_silence_timer() {
        slow_evidence_with_other_session(false, false, false).await;
    }

    #[tokio::test]
    async fn pending_input_pressure_ends_only_the_optional_evidence_read_and_keeps_every_frame_in_order()
     {
        slow_evidence_with_other_session(true, false, false).await;
    }

    #[tokio::test]
    async fn a_native_end_keeps_its_bounded_evidence_when_stdout_closes_just_after_it() {
        slow_evidence_with_other_session(false, true, false).await;
    }

    #[tokio::test]
    async fn opening_another_native_thread_does_not_cancel_a_sessions_evidence_read() {
        slow_evidence_with_other_session(false, false, true).await;
    }

    async fn slow_evidence_with_other_session(pressure: bool, close: bool, opening: bool) {
        let directory = tempfile::tempdir().unwrap();
        let (sink, mut events) = EventSink::channel();
        let (rpc, _) = protocol::connect(tokio::io::empty(), tokio::io::sink(), None);
        let clock = Arc::new(orch_types::ManualClock::new(chrono::Utc::now()));
        let shared = Shared::new(ProcessParts {
            background_control: false,
            sink,
            clock: clock.clone(),
            outputs: OutputStore::new(directory.path().join("outputs")),
            rpc,
            raw: Arc::new(Mutex::new(HashMap::new())),
            version: "0.167.2".into(),
            exit: Arc::new(watch::channel(None).0),
            patience: Patience::default(),
            native_result_wait: Duration::from_secs(60),
            native_result_buffer_bytes: if pressure {
                8192
            } else {
                DEFAULT_NATIVE_RESULT_BUFFER_BYTES
            },
        });
        for (number, native) in [(1, "slow"), (2, "other")] {
            let thread = Id::from_u128(number);
            let mut mapper = Mapper::new(crate::events::MapperConfig {
                thread_id: thread,
                segment_id: Id::from_u128(number + 10),
                native_thread: native.into(),
                directory: "/workspace".into(),
            })
            .with_native_web_results(true)
            .with_web_action_wait(Duration::from_secs(3));
            mapper.expect_turn(Id::from_u128(number + 20));
            let mut state = lock(&shared.state);
            state.by_native.insert(native.into(), thread);
            state.sessions.insert(
                thread,
                SessionRecord {
                    mapper,
                    directory: "/workspace".into(),
                    cwd: "/workspace".into(),
                    workspace_root: "/workspace".into(),
                    instruction_root_marker: false,
                    workspace_configuration: None,
                    instructions_notice: None,
                    mounts: Vec::new(),
                    remote: None,
                    approvals: HashMap::new(),
                    planning: false,
                    approval_mode: orch_agent_contract::descriptor::ApprovalMode::Bypass,
                },
            );
        }
        let (started, began) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        *lock(&shared.result_probe) = Some(ResultProbe {
            owner: Id::from_u128(1),
            started,
            released,
        });
        let (send, incoming) = orch_agent_contract::flow::channel(|_: &Incoming| 1);
        let task = tokio::spawn(pump(shared.clone(), incoming));
        async fn notification(
            send: &orch_agent_contract::flow::Sender<Incoming>,
            method: &str,
            params: Json,
        ) {
            send.send(Incoming::Notification {
                method: method.into(),
                params,
            })
            .await
            .unwrap();
        }
        for native in ["slow", "other"] {
            notification(
                &send,
                "turn/started",
                json!({"threadId":native,"turn":{"id":native}}),
            )
            .await;
        }
        for (native, item) in [
            ("slow", "slow-web"),
            ("other", "other-web"),
            ("other", "silent-web"),
        ] {
            notification(&send, "item/started", json!({"threadId":native,"turnId":native,"item":{"id":item,"type":"webSearch","query":"a query"}})).await;
        }
        notification(&send, "item/completed", json!({"threadId":"slow","turnId":"slow","item":{"id":"slow-web","type":"webSearch","action":{"type":"search","query":"a query"}}})).await;
        notification(
            &send,
            "turn/completed",
            json!({"threadId":"slow","turn":{"id":"slow","status":"completed"}}),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), began)
            .await
            .unwrap()
            .unwrap();
        if opening {
            shared
                .on_notification(
                    "thread/started",
                    &json!({"thread":{"id":"opening","cwd":"/workspace","name":"A new thread"}}),
                )
                .await;
            assert!(
                lock(&shared.state)
                    .pending_ends
                    .contains_key(&Id::from_u128(1)),
                "a new session has no relation to the waiting session's child introductions"
            );
        }
        let received = clock.time();
        shared
            .on_notification(
                "warning",
                &json!({"threadId":"slow","message":"The first retained warning."}),
            )
            .await;
        notification(&send, "item/completed", json!({"threadId":"other","turnId":"other","item":{"id":"other-web","type":"webSearch","action":{"type":"search","query":"a query"}}})).await;
        clock.advance(chrono::Duration::seconds(3));
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut completed = false;
            let mut expired = false;
            while !(completed && expired) {
                if let Some(AdapterEvent::Session {
                    thread_id,
                    event: SessionEvent::ItemCompleted(item),
                    ..
                }) = events.next().await
                {
                    if thread_id == Id::from_u128(2)
                        && item.native_ref == "other-web"
                        && item.status == ItemStatus::Completed
                    {
                        completed = true;
                    }
                    if thread_id == Id::from_u128(2)
                        && item.native_ref == "silent-web"
                        && item.status == ItemStatus::Failed
                    {
                        expired = true;
                    }
                }
            }
        })
        .await
        .expect("the other session and watchdog progress before the slow read is released");
        assert!(
            lock(&shared.state).sessions[&Id::from_u128(1)]
                .mapper
                .turn_in_progress()
        );
        let closing = close.then(|| {
            let shared = shared.clone();
            tokio::spawn(async move {
                shared.on_closed("native stdout ended").await;
            })
        });
        if let Some(closing) = &closing {
            tokio::task::yield_now().await;
            assert!(
                !closing.is_finished(),
                "an already observed end retains its own proof wait"
            );
        }
        let second = "A large retained warning. ".repeat(1024);
        if pressure {
            shared
                .on_notification("warning", &json!({"threadId":"slow","message":second}))
                .await;
            assert!(
                lock(&shared.state).image_sources[&Id::from_u128(1)]
                    .cancellation()
                    .load(Ordering::Acquire)
            );
        } else {
            release.send(()).unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut ended = false;
            let mut first_seen = false;
            let mut second_seen = !pressure;
            let mut pressure_explained = !pressure;
            while !(ended && first_seen && second_seen && pressure_explained) {
                if let Some(AdapterEvent::Session {
                    thread_id, event, ..
                }) = events.next().await
                    && thread_id == Id::from_u128(1)
                {
                    match event {
                        SessionEvent::TurnCompleted(_) => {
                            assert!(!ended);
                            ended = true;
                        }
                        SessionEvent::ItemCompleted(item)
                            if item.payload["data"]["change"] == "warning" =>
                        {
                            assert!(ended, "retained frames follow their own native end");
                            if item.payload["data"]["value"]["message"]
                                == "The first retained warning."
                            {
                                first_seen = true;
                                assert_eq!(item.payload["data"]["reported_at"], json!(received));
                            } else if item.payload["data"]["value"]["message"] == second {
                                assert!(first_seen);
                                second_seen = true;
                            }
                        }
                        SessionEvent::ItemCompleted(item)
                            if item.payload["details"]["reason"] == "native_result_buffer_full" =>
                        {
                            pressure_explained = true;
                        }
                        SessionEvent::ItemCompleted(item)
                            if close && item.payload["code"] == "native_web_action_unproven" =>
                        {
                            assert_ne!(
                                item.payload["details"]["reason"],
                                "native_result_session_closed"
                            );
                        }
                        _ => {}
                    }
                }
            }
        })
        .await
        .unwrap();
        if pressure {
            release.send(()).unwrap();
        }
        assert!(lock(&shared.state).pending_ends.is_empty());
        if let Some(closing) = closing {
            tokio::time::timeout(Duration::from_secs(5), closing)
                .await
                .unwrap()
                .unwrap();
        }
        task.abort();
        let _ = task.await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_blocked_native_result_read_releases_its_wait_and_cancels_the_worker() {
        let (sink, _events) = EventSink::channel();
        let (rpc, _incoming) = protocol::connect(tokio::io::empty(), tokio::io::sink(), None);
        let directory = tempfile::tempdir().unwrap();
        let shared = Shared::new(ProcessParts {
            background_control: false,
            sink,
            clock: Arc::new(orch_types::SystemClock),
            outputs: OutputStore::new(directory.path().join("outputs")),
            rpc,
            raw: Arc::new(Mutex::new(HashMap::new())),
            version: "0.167.2".into(),
            exit: Arc::new(watch::channel(None).0),
            patience: Patience::default(),
            native_result_wait: Duration::from_secs(3),
            native_result_buffer_bytes: DEFAULT_NATIVE_RESULT_BUFFER_BYTES,
        });
        let thread = Id::from_u128(1);
        let segment = Id::from_u128(2);
        lock(&shared.state).sessions.insert(
            thread,
            SessionRecord {
                mapper: Mapper::new(crate::events::MapperConfig {
                    thread_id: thread,
                    segment_id: segment,
                    native_thread: "native".into(),
                    directory: "/workspace".into(),
                }),
                directory: "/workspace".into(),
                cwd: "/workspace".into(),
                workspace_root: "/workspace".into(),
                instruction_root_marker: false,
                workspace_configuration: None,
                instructions_notice: None,
                mounts: Vec::new(),
                remote: None,
                approvals: HashMap::new(),
                planning: false,
                approval_mode: orch_agent_contract::descriptor::ApprovalMode::Bypass,
            },
        );
        let (started, began) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let working = shared.clone();
        let worker = tokio::spawn(async move {
            working
                .native_result_work(
                    thread,
                    segment,
                    "native",
                    Vec::new(),
                    move |source, _, _| {
                        let _ = started.send(source.cancellation());
                        blocked.recv().unwrap();
                    },
                )
                .await
        });
        let cancelled = began.await.unwrap();
        tokio::time::advance(Duration::from_secs(3)).await;
        assert_eq!(worker.await.unwrap(), Err("native_result_timed_out"));
        assert!(cancelled.load(Ordering::Acquire));
        let began_again = Arc::new(AtomicBool::new(false));
        let began_read = began_again.clone();
        let again =
            shared.native_result_work(thread, segment, "native", Vec::new(), move |_, _, _| {
                began_read.store(true, Ordering::Release);
            });
        tokio::pin!(again);
        tokio::select! {
            biased;
            result = &mut again => panic!("another physical read must wait for its owner: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        tokio::time::advance(Duration::from_secs(3)).await;
        assert_eq!(again.await, Err("native_result_timed_out"));
        assert!(
            !began_again.load(Ordering::Acquire),
            "a timed-out physical read cannot accumulate more readers"
        );
        release.send(()).unwrap();
        let gate = lock(&shared.state).result_gates[&thread].clone();
        let _finished = gate.lock().await;
    }
}
