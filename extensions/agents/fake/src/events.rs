//! Scripted neutral events and failure injection.
//!
//! [`Emitter`] is how one turn of a session speaks: every event it sends goes to the host's sink in
//! the order the turn made it, and every item that is final is written into the session's file, so
//! that a session that is resumed, forked, exported or reported again holds exactly what was said.
//! [`Faults`] is how a test makes a call of the adapter fail: the profile names the call, the code
//! and how many times.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use orch_agent_contract::adapter::{AdapterError, AdapterEvent, EventSink};
use orch_agent_contract::credentials::CredentialNeed;
use orch_agent_contract::events::{
    AgentMessagePayload, AgentMessagePhase, ApprovalClosed, ApprovalRequested, BlobRef,
    CredentialNeeded, DeltaField, ErrorPayload, FileChange, FileChangeKind, HarnessRequest,
    ItemCompleted, ItemContent, ItemDelta, ItemStarted, ItemStatus, ItemUpdated, MAIN_STREAM,
    MarkerPayload, OrchToolCall, OutputRef, PlanPayload, PlanStep, PlanStepStatus, RawCall,
    SessionEvent, SessionLost, SessionOpenedEvent, SessionTitle, TokenUsage, ToolCallPayload,
    ToolPayload, TurnCompleted, TurnError, TurnOutcome, TurnPhase, TurnPhaseKind, TurnStarted,
    UsageUpdated, count_diff_lines,
};
use orch_agent_contract::session::ThreadId;
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};

use crate::sessions::{ItemRecord, LiveSession};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How many characters of text one piece of a streamed message holds: the spec's "40 characters per
/// 100 ms".
pub const PIECE_CHARS: usize = 40;
/// How long a streamed piece takes, in milliseconds: the other half of "40 characters per 100 ms".
pub const PIECE_MILLIS: u64 = 100;
/// How many bytes of a command's output one piece holds. The same rhythm as text; bytes, because the
/// offsets of an output count bytes.
pub const OUTPUT_PIECE_BYTES: usize = 40;
/// The size of the model's context that the fake reports with its usage when it was told none: the order of
/// a large model's window, so that a context meter has something to show.
pub const CONTEXT_WINDOW: i64 = 200_000;

/// The context a turn was started with, which is what the fake reports its usage against: the context that
/// was chosen or that the model starts with, else the model's own window that it was given, else
/// [`CONTEXT_WINDOW`].
#[must_use]
pub fn context_in_force(params: &Json) -> i64 {
    params
        .get("context")
        .and_then(Json::as_i64)
        .or_else(|| {
            params
                .get(orch_agent_contract::parameters::MODEL_WINDOW_KEY)
                .and_then(Json::as_i64)
        })
        .unwrap_or(CONTEXT_WINDOW)
}

// ---------------------------------------------------------------------------------------------------------
// Failure injection

/// The calls of the adapter that a failure can be injected into, by the name a profile gives them.
pub const FAULT_CALLS: [&str; 21] = [
    "start",
    "open_session",
    "resume",
    "close_session",
    "start_turn",
    "steer",
    "interrupt",
    "resolve_approval",
    "tool_result",
    "export",
    "resync",
    "discover_sessions",
    "discover_mcp",
    "read_mcp",
    "login",
    "login_input",
    "logout",
    "issue_credential",
    "set_credential",
    "return_credential",
    "accept_credential",
];

fn once() -> u32 {
    1
}

/// A failure a profile asks for: which calls fail, how, and how often.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    /// The code of the error, as a command's result states it (`harness_error`, `login_required`,
    /// `session_not_found`, `timeout`…).
    pub code: String,
    /// What the error says.
    #[serde(default)]
    pub message: String,
    /// Whether trying again may help.
    #[serde(default)]
    pub retryable: bool,
    /// How many calls fail; `0` fails every call. One, unless said.
    #[serde(default = "once")]
    pub times: u32,
    /// How many calls succeed before the first one fails.
    #[serde(default)]
    pub after: u32,
}

struct Armed {
    fault: Fault,
    seen: u32,
    failed: u32,
}

/// The failures that are armed, by call.
#[derive(Default)]
pub struct Faults {
    armed: Mutex<HashMap<String, Armed>>,
}

impl std::fmt::Debug for Faults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Faults").finish_non_exhaustive()
    }
}

impl Faults {
    /// The failures a profile asks for.
    pub fn new(table: &BTreeMap<String, Fault>) -> Result<Self, AdapterError> {
        let mut armed = HashMap::new();
        for (call, fault) in table {
            if !FAULT_CALLS.contains(&call.as_str()) {
                return Err(AdapterError::Other {
                    code: "invalid_config".to_owned(),
                    message: format!(
                        "faults: {call:?} is not a call that can fail; the calls are {}",
                        FAULT_CALLS.join(", ")
                    ),
                    retryable: false,
                });
            }
            armed.insert(
                call.clone(),
                Armed {
                    fault: fault.clone(),
                    seen: 0,
                    failed: 0,
                },
            );
        }
        Ok(Self {
            armed: Mutex::new(armed),
        })
    }

    /// Counts a call of the adapter, and fails it if a failure is due.
    pub fn check(&self, call: &str) -> Result<(), AdapterError> {
        let mut armed = lock(&self.armed);
        let Some(entry) = armed.get_mut(call) else {
            return Ok(());
        };
        entry.seen += 1;
        if entry.seen <= entry.fault.after {
            return Ok(());
        }
        if entry.fault.times != 0 && entry.failed >= entry.fault.times {
            return Ok(());
        }
        entry.failed += 1;
        let message = if entry.fault.message.is_empty() {
            format!("the fake harness was told to fail {call}")
        } else {
            entry.fault.message.clone()
        };
        Err(AdapterError::from_code(
            &entry.fault.code,
            &message,
            entry.fault.retryable,
        ))
    }
}

// ---------------------------------------------------------------------------------------------------------
// Payloads

/// The payload of a tool call that has just begun.
#[must_use]
pub fn tool_call(
    tool: ToolPayload,
    title: &str,
    started_at: Time,
    raw_name: &str,
    raw_input: &str,
) -> ToolCallPayload {
    ToolCallPayload {
        tool,
        title: title.to_owned(),
        started_at,
        duration_ms: None,
        raw: RawCall {
            name: raw_name.to_owned(),
            input: OutputRef::of(raw_input),
            output: None,
            is_error: false,
        },
    }
}

/// A unified diff for a change of `added` and `removed` lines in `path`, with three lines of context on
/// both sides, and whether it had to be cut at `limit` bytes.
#[must_use]
pub fn synthetic_diff(path: &str, added: u32, removed: u32, limit: usize) -> (String, bool) {
    const CONTEXT: u32 = 3;
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n");
    if added == 0 && removed == 0 {
        return (diff, false);
    }
    diff.push_str(&format!(
        "@@ -1,{} +1,{} @@\n",
        CONTEXT + removed + CONTEXT,
        CONTEXT + added + CONTEXT
    ));
    for i in 1..=CONTEXT {
        diff.push_str(&format!(" context line {i}\n"));
    }
    for i in 1..=removed {
        diff.push_str(&format!("-old line {i}\n"));
    }
    for i in 1..=added {
        diff.push_str(&format!("+new line {i}\n"));
    }
    for i in 1..=CONTEXT {
        diff.push_str(&format!(" context line {}\n", CONTEXT + i));
    }
    if diff.len() <= limit {
        return (diff, false);
    }
    let mut cut = limit;
    while !diff.is_char_boundary(cut) {
        cut -= 1;
    }
    let cut = diff[..cut].rfind('\n').map_or(0, |at| at + 1);
    diff.truncate(cut);
    (diff, true)
}

/// A change of a file as the fake reports it.
#[must_use]
pub fn file_change(path: &str, added: u32, removed: u32, diff_limit: usize) -> FileChange {
    let (diff, truncated) = synthetic_diff(path, added, removed, diff_limit);
    let (counted_added, counted_removed) = count_diff_lines(&diff);
    FileChange {
        path: path.to_owned(),
        change: FileChangeKind::Modify,
        old_path: None,
        diff: Some(diff),
        added: if truncated {
            counted_added
        } else {
            i64::from(added)
        },
        removed: if truncated {
            counted_removed
        } else {
            i64::from(removed)
        },
        binary: false,
        truncated,
    }
}

// ---------------------------------------------------------------------------------------------------------
// The emitter

/// An item that has begun and has not ended: what is needed to end it as it stands.
#[derive(Debug, Clone)]
pub struct OpenItem {
    /// The stream it is in.
    pub stream: String,
    /// The item that started it, for a sub-agent's.
    pub parent: Option<String>,
    /// Its payload as it stands.
    pub payload: Json,
    kind: orch_agent_contract::events::ItemKind,
}

/// How one turn speaks.
pub struct Emitter {
    model: String,
    requests: Mutex<RequestState>,
    sink: EventSink,
    live: Arc<LiveSession>,
    clock: Arc<dyn Clock>,
    turn_id: Id,
    ordinal: u32,
    open: Mutex<BTreeMap<String, OpenItem>>,
    closed: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct RequestState {
    serial: u64,
    current: Option<orch_agent_contract::requests::UsageRequest>,
    next_asked: Option<Time>,
    last_output: Option<Time>,
    counts: Option<TokenUsage>,
    tools: std::collections::HashSet<String>,
}

impl std::fmt::Debug for Emitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emitter")
            .field("turn", &self.turn_id)
            .finish_non_exhaustive()
    }
}

impl Emitter {
    /// An emitter for the turn `turn_id`, the `ordinal`th of its session.
    #[must_use]
    pub fn new(
        sink: EventSink,
        live: Arc<LiveSession>,
        clock: Arc<dyn Clock>,
        turn_id: Id,
        ordinal: u32,
        model: String,
    ) -> Self {
        Self {
            model,
            requests: Mutex::new(RequestState::default()),
            sink,
            live,
            clock,
            turn_id,
            ordinal,
            open: Mutex::new(BTreeMap::new()),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The turn.
    #[must_use]
    pub const fn turn_id(&self) -> Id {
        self.turn_id
    }

    /// The moment, by the clock the adapter was given.
    #[must_use]
    pub fn now(&self) -> Time {
        self.clock.time()
    }

    /// Whether the host stopped listening: nothing more is worth saying.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Relaxed) || self.sink.is_closed()
    }

    async fn send(&self, event: SessionEvent) {
        let sent = self
            .sink
            .session(self.live.thread_id, self.live.segment_id, event)
            .await;
        if sent.is_err() {
            self.closed
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Sends an event that is no item's.
    pub async fn event(&self, event: SessionEvent) {
        self.send(event).await;
    }

    /// The turn began.
    pub async fn turn_started(&self, native_turn_ref: &str) {
        self.send(SessionEvent::TurnStarted(TurnStarted {
            turn_id: self.turn_id,
            native_turn_ref: native_turn_ref.to_owned(),
        }))
        .await;
    }

    /// The turn ended.
    pub async fn turn_completed(&self, status: TurnOutcome, error: Option<TurnError>) {
        self.request_ended(
            if status == TurnOutcome::Interrupted {
                "forced"
            } else if status == TurnOutcome::Failed {
                error.as_ref().map_or("harness_error", |e| e.code.as_str())
            } else {
                "done"
            },
            error.as_ref(),
            false,
        )
        .await;
        self.send(SessionEvent::TurnCompleted(TurnCompleted {
            turn_id: self.turn_id,
            status,
            error,
        }))
        .await;
    }

    /// A phase of the turn began.
    pub async fn phase(&self, kind: TurnPhaseKind) {
        self.send(SessionEvent::TurnPhase(TurnPhase {
            turn_id: self.turn_id,
            kind,
            at: self.now(),
        }))
        .await;
    }

    /// The use so far, against the context the turn was started with.
    pub async fn usage(
        &self,
        turn: TokenUsage,
        thread: TokenUsage,
        context_used: i64,
        context_window: i64,
    ) {
        // Only a turn with one simulated model request has request-level counts;
        // later cumulative directives remain turn totals, without fabricated attribution.
        {
            let mut requests = lock(&self.requests);
            if requests.serial == 1 {
                requests.counts = Some(turn);
            }
        }
        self.send(SessionEvent::UsageUpdated(UsageUpdated {
            turn_id: self.turn_id,
            turn,
            thread,
            context_window: Some(context_window),
            context_used: Some(context_used),
            report: None,
        }))
        .await;
    }

    /// The harness gave the session a title.
    pub async fn title(&self, title: &str) {
        self.send(SessionEvent::SessionTitle(SessionTitle {
            title: title.to_owned(),
        }))
        .await;
    }

    /// The session is gone.
    pub async fn lost(&self, reason: &str) {
        self.request_ended("session_lost", None, false).await;
        self.send(SessionEvent::SessionLost(SessionLost {
            reason: reason.to_owned(),
        }))
        .await;
    }

    /// The harness needs another credential.
    pub async fn credential_needed(&self, reason: CredentialNeed) {
        self.send(SessionEvent::CredentialNeeded(CredentialNeeded { reason }))
            .await;
    }

    /// The harness asks to be allowed something.
    pub async fn approval_requested(
        &self,
        request_ref: &str,
        title: &str,
        request: HarnessRequest,
    ) {
        self.request_ended("done", None, true).await;
        self.send(SessionEvent::ApprovalRequested(ApprovalRequested {
            turn_id: self.turn_id,
            request_ref: request_ref.to_owned(),
            title: title.to_owned(),
            request,
        }))
        .await;
    }

    /// The harness withdrew a request.
    pub async fn approval_closed(&self, request_ref: &str) {
        self.send(SessionEvent::ApprovalClosed(ApprovalClosed {
            request_ref: request_ref.to_owned(),
        }))
        .await;
    }

    /// The agent called a tool of the `orch` server.
    pub async fn orch_tool_call(&self, call_ref: &str, tool: &str, arguments: Json) {
        self.send(SessionEvent::OrchToolCall(OrchToolCall {
            turn_id: self.turn_id,
            call_ref: call_ref.to_owned(),
            attempt: None,
            tool: tool.to_owned(),
            arguments,
        }))
        .await;
    }

    /// A new reference for an item, unique in the session.
    #[must_use]
    pub fn next_item_ref(&self) -> String {
        format!(
            "item_{}",
            self.live.next_counter(crate::sessions::Counter::Item)
        )
    }

    /// An item began.
    pub async fn item_started(
        &self,
        native_ref: &str,
        stream: &str,
        parent: Option<&str>,
        content: ItemContent,
    ) {
        if stream == MAIN_STREAM
            && matches!(
                &content,
                ItemContent::AgentMessage(_)
                    | ItemContent::Reasoning(_)
                    | ItemContent::Plan(_)
                    | ItemContent::ToolCall(_)
            )
        {
            self.resume_request().await;
        }
        if stream == MAIN_STREAM && matches!(&content, ItemContent::ToolCall(_)) {
            self.request_ended("done", None, true).await;
            lock(&self.requests).tools.insert(native_ref.to_owned());
        }
        let kind = content.kind();
        let payload = match &content {
            ItemContent::AgentMessage(p) => serde_json::to_value(p),
            ItemContent::Reasoning(p) => serde_json::to_value(p),
            ItemContent::Plan(p) => serde_json::to_value(p),
            ItemContent::ToolCall(p) => serde_json::to_value(p),
            ItemContent::Marker(p) => serde_json::to_value(p),
            ItemContent::Error(p) => serde_json::to_value(p),
        }
        .unwrap_or(Json::Null);
        lock(&self.open).insert(
            native_ref.to_owned(),
            OpenItem {
                stream: stream.to_owned(),
                parent: parent.map(str::to_owned),
                payload,
                kind,
            },
        );
        self.send(SessionEvent::ItemStarted(ItemStarted {
            turn_id: self.turn_id,
            native_ref: native_ref.to_owned(),
            stream: stream.to_owned(),
            parent_native_ref: parent.map(str::to_owned),
            content,
        }))
        .await;
    }

    /// The text of an item grew.
    pub async fn item_delta(&self, native_ref: &str, field: DeltaField, offset: i64, data: &str) {
        if !data.is_empty()
            && matches!(field, DeltaField::Text | DeltaField::Summary)
            && lock(&self.open)
                .get(native_ref)
                .is_some_and(|open| open.stream == MAIN_STREAM)
        {
            let at = self.now();
            let first = {
                let mut requests = lock(&self.requests);
                let mut first = None;
                if let Some(request) = &mut requests.current {
                    request.output_streamed = Some(true);
                    if request.first_output.is_none() {
                        request.first_output = Some(at);
                        first = Some(request.clone());
                    }
                }
                requests.last_output = Some(at);
                first
            };
            if let Some(request) = first {
                self.report_request(request).await;
            }
        }
        let delta = match field {
            DeltaField::Output => ItemDelta::output(native_ref, offset, data.as_bytes()),
            DeltaField::Text | DeltaField::Summary => {
                ItemDelta::text(native_ref, field, offset, data)
            }
        };
        self.send(SessionEvent::ItemDelta(delta)).await;
    }

    /// Members of an item's payload changed. The item's payload as it stands takes them too.
    pub async fn item_updated(&self, native_ref: &str, patch: Json) {
        if let Some(open) = lock(&self.open).get_mut(native_ref)
            && let (Json::Object(members), Json::Object(patch)) = (&mut open.payload, &patch)
        {
            for (name, value) in patch {
                members.insert(name.clone(), value.clone());
            }
        }
        self.send(SessionEvent::ItemUpdated(ItemUpdated {
            native_ref: native_ref.to_owned(),
            payload: patch,
        }))
        .await;
    }

    /// The payload of an item as it stands.
    #[must_use]
    pub fn payload_of(&self, native_ref: &str) -> Option<Json> {
        lock(&self.open)
            .get(native_ref)
            .map(|open| open.payload.clone())
    }

    /// An item is final: it is told, and it is written into the session.
    pub async fn item_completed(
        &self,
        native_ref: &str,
        status: ItemStatus,
        payload: Json,
        output_blob: Option<BlobRef>,
    ) {
        let open = lock(&self.open).remove(native_ref);
        if let Some(open) = open {
            self.live.record_item(ItemRecord {
                native_ref: native_ref.to_owned(),
                turn_ordinal: self.ordinal,
                stream: open.stream,
                parent_native_ref: open.parent,
                kind: open.kind,
                status,
                payload: payload.clone(),
                output_blob: output_blob.clone(),
            });
        }
        self.send(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref: native_ref.to_owned(),
            status,
            payload,
            output_blob,
        }))
        .await;
        let mut requests = lock(&self.requests);
        let resumed = requests.tools.remove(native_ref) && requests.tools.is_empty();
        if resumed {
            requests.next_asked = Some(self.now());
        }
        drop(requests);
    }

    async fn report_request(&self, request: orch_agent_contract::requests::UsageRequest) {
        let _ = self
            .sink
            .request(
                self.live.thread_id,
                self.live.segment_id,
                self.turn_id,
                request,
            )
            .await;
    }

    /// Starts a model step only after the input parsed and its credential was accepted.
    pub(crate) async fn request_asked(&self) {
        let request = {
            let mut state = lock(&self.requests);
            if state.current.is_some() {
                return;
            }
            state.serial += 1;
            state.counts = None;
            state.last_output = None;
            let request = orch_agent_contract::requests::UsageRequest {
                id: format!("request-{}", state.serial),
                model: Some(self.model.clone()),
                asked: Some(state.next_asked.take().unwrap_or_else(|| self.now())),
                ..Default::default()
            };
            state.current = Some(request.clone());
            request
        };
        self.report_request(request).await;
    }

    /// A tool result starts the next step's asked boundary, but a trailing tool creates no empty call.
    pub(crate) async fn resume_request(&self) {
        let resume = {
            let state = lock(&self.requests);
            state.current.is_none() && state.next_asked.is_some() && state.tools.is_empty()
        };
        if resume {
            self.request_asked().await;
        }
    }

    /// A person's answer is input for the next model step, with their waiting time outside it.
    pub(crate) fn input_returned(&self) {
        let mut state = lock(&self.requests);
        if state.current.is_none() {
            state.next_asked = Some(self.now());
        }
    }

    /// Finalizes a simulated request with observations, never a computed duration or speed.
    pub async fn request_ended(&self, code: &str, error: Option<&TurnError>, tool_begins: bool) {
        let request = {
            let mut state = lock(&self.requests);
            state.next_asked = None;
            let Some(mut request) = state.current.take() else {
                return;
            };
            if tool_begins {
                let now = self.now();
                request.first_output.get_or_insert(now);
                request.output_streamed.get_or_insert(false);
                state.last_output = Some(now);
            }
            request.end = state.last_output;
            if let Some(counts) = state.counts.take() {
                request.tokens = counts.into();
            }
            request.finish(
                code,
                error.map(|e| e.message.as_str()),
                error.and_then(|e| e.said.as_deref()),
            );
            request
        };
        self.report_request(request).await;
    }

    /// The items that began and did not end, with the payload each has now.
    #[must_use]
    pub fn open_items(&self) -> Vec<(String, OpenItem)> {
        lock(&self.open)
            .iter()
            .map(|(native_ref, open)| (native_ref.clone(), open.clone()))
            .collect()
    }

    // ---- whole items

    /// An agent message that is said at once, in one piece.
    pub async fn say_whole(&self, text: &str, phase: Option<AgentMessagePhase>) {
        let native_ref = self.next_item_ref();
        let length = char_count(text);
        self.item_started(
            &native_ref,
            MAIN_STREAM,
            None,
            ItemContent::AgentMessage(AgentMessagePayload {
                text: String::new(),
                text_len: 0,
                phase,
            }),
        )
        .await;
        if !text.is_empty() {
            self.item_delta(&native_ref, DeltaField::Text, 0, text)
                .await;
        }
        self.item_completed(
            &native_ref,
            ItemStatus::Completed,
            serde_json::to_value(AgentMessagePayload {
                text: text.to_owned(),
                text_len: length,
                phase,
            })
            .unwrap_or(Json::Null),
            None,
        )
        .await;
    }

    /// An error in the harness's work.
    pub async fn error_item(&self, code: &str, message: &str, retryable: bool) {
        let native_ref = self.next_item_ref();
        let payload = ErrorPayload {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
        };
        self.item_started(
            &native_ref,
            MAIN_STREAM,
            None,
            ItemContent::Error(payload.clone()),
        )
        .await;
        self.item_completed(
            &native_ref,
            ItemStatus::Failed,
            serde_json::to_value(payload).unwrap_or(Json::Null),
            None,
        )
        .await;
    }

    /// The marker that the harness shortened its own conversation.
    pub async fn compacted_marker(&self, duration_ms: Option<i64>) {
        let native_ref = self.next_item_ref();
        let payload = MarkerPayload::Compacted { duration_ms };
        self.item_started(
            &native_ref,
            MAIN_STREAM,
            None,
            ItemContent::Marker(payload.clone()),
        )
        .await;
        self.item_completed(
            &native_ref,
            ItemStatus::Completed,
            serde_json::to_value(payload).unwrap_or(Json::Null),
            None,
        )
        .await;
    }

    /// The plan, as its steps stand.
    #[must_use]
    pub fn plan_payload(steps: &[(String, PlanStepStatus)]) -> PlanPayload {
        PlanPayload {
            explanation: None,
            steps: steps
                .iter()
                .map(|(text, status)| PlanStep {
                    text: text.clone(),
                    status: *status,
                })
                .collect(),
        }
    }
}

/// How many Unicode code points a text has: the unit of the offsets of a message.
#[must_use]
pub fn char_count(text: &str) -> i64 {
    i64::try_from(text.chars().count()).unwrap_or(i64::MAX)
}

/// A text cut into pieces of at most `size` characters, never in the middle of one.
#[must_use]
pub fn pieces(text: &str, size: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(size.max(1))
        .map(|piece| piece.iter().collect())
        .collect()
}

/// A text cut into pieces of at most `size` bytes, each ending at a character.
#[must_use]
pub fn byte_pieces(text: &str, size: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for c in text.chars() {
        if !piece.is_empty() && piece.len() + c.len_utf8() > size.max(1) {
            pieces.push(std::mem::take(&mut piece));
        }
        piece.push(c);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// The hash of an output that is kept whole in a file beside the session.
#[must_use]
pub fn output_hash(output: &str) -> Hash {
    Hash::of(output.as_bytes())
}

/// An error of a turn.
#[must_use]
pub fn turn_error(code: &str, message: &str, retryable: bool) -> TurnError {
    TurnError::new(code, message, retryable)
}

/// The event that tells the host the session opened.
#[must_use]
pub fn session_opened(native_session_ref: &str, resumed: bool) -> SessionEvent {
    SessionEvent::SessionOpened(SessionOpenedEvent {
        native_session_ref: native_session_ref.to_owned(),
        resumed,
    })
}

/// Sends a session event that is not a turn's, from the adapter itself.
pub async fn announce(sink: &EventSink, thread: ThreadId, segment: Id, event: SessionEvent) {
    let _ = sink
        .emit(AdapterEvent::Session {
            thread_id: thread,
            segment_id: segment,
            event,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_context_a_turn_reports_against_is_the_one_it_was_started_with_else_the_window_it_was_given()
     {
        assert_eq!(
            context_in_force(&json!({ "context": 1_000_000, "context_window": 200_000 })),
            1_000_000
        );
        assert_eq!(
            context_in_force(&json!({ "context_window": 64_000 })),
            64_000
        );
        assert_eq!(context_in_force(&json!({})), CONTEXT_WINDOW);
        assert_eq!(
            context_in_force(&json!({ "context": "big" })),
            CONTEXT_WINDOW
        );
    }

    fn fault(code: &str, times: u32, after: u32) -> BTreeMap<String, Fault> {
        BTreeMap::from([(
            "start_turn".to_owned(),
            Fault {
                code: code.to_owned(),
                message: String::new(),
                retryable: false,
                times,
                after,
            },
        )])
    }

    #[test]
    fn a_fault_fails_the_number_of_calls_it_says_after_the_calls_it_lets_pass() {
        let faults = Faults::new(&fault("harness_error", 2, 1)).unwrap();
        assert!(faults.check("start_turn").is_ok(), "the first call passes");
        assert_eq!(
            faults.check("start_turn").unwrap_err().code(),
            "harness_error"
        );
        assert_eq!(
            faults.check("start_turn").unwrap_err().code(),
            "harness_error"
        );
        assert!(
            faults.check("start_turn").is_ok(),
            "then the harness recovers"
        );
        assert!(
            faults.check("open_session").is_ok(),
            "another call is not touched"
        );
    }

    #[test]
    fn a_fault_with_no_end_fails_every_call() {
        let faults = Faults::new(&fault("login_required", 0, 0)).unwrap();
        for _ in 0..5 {
            assert_eq!(
                faults.check("start_turn").unwrap_err().code(),
                "login_required"
            );
        }
    }

    #[test]
    fn a_fault_on_a_call_that_does_not_exist_is_refused_with_the_list() {
        let table = BTreeMap::from([(
            "start_tern".to_owned(),
            Fault {
                code: "x".into(),
                message: String::new(),
                retryable: false,
                times: 1,
                after: 0,
            },
        )]);
        let error = Faults::new(&table).unwrap_err();
        assert!(error.detail().contains("start_turn"), "{error}");
    }

    #[test]
    fn text_is_cut_by_characters_and_output_by_bytes_never_inside_a_character() {
        assert_eq!(pieces("héllo wörld", 4), ["héll", "o wö", "rld"]);
        let cut = byte_pieces("héllo", 2);
        assert!(cut.iter().all(|piece| piece.len() <= 3));
        assert_eq!(cut.concat(), "héllo");
        assert!(pieces("", 40).is_empty());
    }

    #[test]
    fn a_diff_has_the_lines_it_was_asked_for_and_three_lines_of_context() {
        let change = file_change("src/main.rs", 2, 1, 256 * 1024);
        assert_eq!((change.added, change.removed), (2, 1));
        let diff = change.diff.unwrap();
        assert!(diff.contains("@@ -1,7 +1,8 @@"), "{diff}");
        assert_eq!(count_diff_lines(&diff), (2, 1));
        assert!(!change.truncated);
    }

    #[test]
    fn a_diff_over_the_limit_is_cut_at_a_line_and_says_so() {
        let change = file_change("big.txt", 100, 0, 200);
        assert!(change.truncated);
        let diff = change.diff.unwrap();
        assert!(diff.len() <= 200 && diff.ends_with('\n'));
        assert_eq!(count_diff_lines(&diff).0, change.added);
    }
}
