//! One conversation of an ACP agent: its process, its turns, what it asks, and what ends it.
//!
//! A [`Session`] owns the connection to one program. A task reads what the agent says, one message at a time in
//! the order it said it, and is the only place where the updates of a turn become events, so that events never
//! overtake each other: the answer to a prompt is delivered into the same order as the updates before it
//! ([`crate::rpc::Rpc::call_ordered`]). The adapter's calls (a turn to start, an interrupt, an approval to answer)
//! reach into the state under one lock that is never held across a wait.
//!
//! **A turn** begins with `turn.started`, applies the model, the effort and the mode the turn states (one
//! `session/set_config_option` or `session/set_mode` each, only where it differs), and sends the prompt. It ends
//! when the agent answers the prompt, with the reason it gives, or when the process ends under it, or when it has
//! been asked to stop and has not stopped within its grace.
//!
//! **A request of the agent** (permission, a question) becomes an approval and waits for a person; an interrupt
//! answers every one that still waits, as the protocol asks, with `cancelled`.
//!
//! **A run of the program** is one process of it. A thread goes on through several (a program that ends is started
//! again and resumes the conversation), and Orch keeps one approval for each reference a thread was ever given: so
//! the reference of a request carries the token of its run, because a number that counts from nothing in every run
//! would give the first question of a new run the reference of the first question of the run before, and Orch would
//! take it for that one again and show no card. What is said of the end of a run is also of this run: how its process
//! ended, and what it wrote on its error output after the session was up.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use orch_agent_contract::adapter::{AdapterError, EventSink};
use orch_agent_contract::events::{
    ApprovalClosed, ApprovalRequested, SessionEvent, SessionLost, TurnError, TurnOutcome,
    turn_error,
};
use orch_agent_contract::requests::UsageRequest;
use orch_agent_contract::session::{ApprovalDecision, ThreadId, TurnId, TurnStart, TurnSteer};
use orch_types::{Clock, Id, Json};
use serde_json::json;
use tokio::sync::{Notify, watch};

use crate::approvals::{self, Form, PermissionRequest};
use crate::descriptor::Learned;
use crate::extensions::{self, Extra, StreamNotice};
use crate::mapper::{Mapper, MapperConfig, OutputStore};
use crate::process::StderrTail;
use crate::program::AcpProgram;
use crate::rpc::{Incoming, REQUEST_TIMEOUT, RequestId, Rpc, RpcError};
use crate::wire::{self, ConfigOption, Modes, StopReason};

/// How long closing a session is waited for.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the end of a program's process is waited for once its output has closed: the way it ended is known a
/// moment after the closing, and a person is told how the program ended, not only that its output did. A process that
/// closed its output and lingers is reported without that.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// What every session of an adapter shares.
pub struct Env {
    /// The manifest.
    pub program: Arc<AcpProgram>,
    /// Where time comes from.
    pub clock: Arc<dyn Clock>,
    /// Where events go.
    pub sink: EventSink,
    /// Where long outputs are kept.
    pub outputs: OutputStore,
}

/// The pieces of a session that are given when it is made.
pub struct Parts {
    /// What sessions share.
    pub env: Arc<Env>,
    /// The thread.
    pub thread_id: ThreadId,
    /// The segment.
    pub segment_id: Id,
    /// The agent's id for the session.
    pub acp_id: String,
    /// The connection.
    pub rpc: Rpc,
    /// Ends the process.
    pub kill: Arc<Notify>,
    /// Set to why the process ended.
    pub exit: watch::Receiver<Option<String>>,
    /// The last lines the agent wrote on its error output.
    pub stderr: StderrTail,
    /// What was learned of the agent.
    pub learned: Learned,
    /// The mapper's configuration.
    pub mapper: MapperConfig,
    /// The `_meta.orch` object sent with every prompt.
    pub prompt_meta: Json,
    /// The options and modes the agent reported when the session opened.
    pub options: Vec<ConfigOption>,
    /// The modes.
    pub modes: Option<Modes>,
    /// Whether the agent's client methods are carried out (the arrangement `separated`).
    pub client: Option<Arc<crate::client::Methods>>,
    /// The approval mode the session was opened in, for the extras that care.
    pub raw_events: bool,
}

struct Prompt {
    tag: u64,
    interrupted: bool,
}

struct State {
    mapper: Mapper,
    permissions: HashMap<String, (RequestId, PermissionRequest)>,
    forms: HashMap<String, (RequestId, Form)>,
    options: Vec<ConfigOption>,
    modes: Option<Modes>,
    initial_mode: Option<String>,
    prompt: Option<Prompt>,
    gone: Option<String>,
    serial: u64,
    /// Whether the plugin's plan mode is on, as far as the adapter knows.
    plan_active: bool,
    /// The way changes are treated that the plugin was last told.
    approval: Option<&'static str>,
    /// Extras that were on and went off while the program ran.
    off: HashSet<Extra>,
    /// The session is being closed on purpose: the end of its process is no loss.
    closing: bool,
    /// How the program's process ended, once it was seen to have ended.
    ended: Option<String>,
}

/// One conversation of an agent, with its process.
pub struct Session {
    event_gate: tokio::sync::Mutex<()>,
    event_batch: Mutex<std::collections::VecDeque<orch_agent_contract::adapter::AdapterEvent>>,
    env: Arc<Env>,
    /// The thread.
    pub thread_id: ThreadId,
    /// The segment.
    pub segment_id: Id,
    /// The agent's id for it.
    pub acp_id: String,
    rpc: Rpc,
    kill: Arc<Notify>,
    exit: watch::Receiver<Option<String>>,
    stderr: StderrTail,
    learned: Learned,
    prompt_meta: Json,
    client: Option<Arc<crate::client::Methods>>,
    state: Mutex<State>,
    tags: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn extra_on(learned: &Learned, state: &State, extra: Extra) -> bool {
    learned.has(extra) && !state.off.contains(&extra)
}

fn invalid(message: &str) -> AdapterError {
    AdapterError::Other {
        code: "invalid_input".to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}

/// A reference for a request of the agent that no request of the thread has had before.
///
/// Orch keeps one approval for each reference a thread has been given and takes a second request with a reference it
/// has seen for the same request again, so a number alone is not a name: it starts again in each run of the program.
/// The reference carries the token of this run, then the agent's own name for the call where there is one, then the
/// number.
fn fresh_request_ref(state: &mut State, kind: &str, call: Option<&str>) -> String {
    state.serial += 1;
    let run = state.mapper.run();
    match call {
        Some(call) => format!("{kind}:{run}:{call}:{}", state.serial),
        None => format!("{kind}:{run}:{}", state.serial),
    }
}

/// Why the program is gone, in words: how its process ended where that is known, else how the connection closed, then
/// what the program wrote on its error output since the session was up.
fn lost(closed: &str, ended: Option<&str>, said: &str) -> String {
    let what = ended.unwrap_or(closed);
    if said.is_empty() {
        what.to_owned()
    } else {
        format!("{what}: {said}")
    }
}

fn map_rpc(error: RpcError, stderr: &StderrTail) -> AdapterError {
    match error {
        RpcError::Closed(reason) => {
            let said = stderr.recent();
            AdapterError::HarnessUnavailable(if said.is_empty() {
                reason
            } else {
                format!("{reason}: {said}")
            })
        }
        RpcError::Timeout { .. } => AdapterError::Timeout,
        RpcError::Remote { message, code, .. } => {
            let lower = message.to_lowercase();
            if lower.contains("unknown session")
                || lower.contains("not resumable")
                || lower.contains("session not found")
            {
                AdapterError::SessionNotFound(message)
            } else if code == -32000 || lower.contains("authentication required") {
                AdapterError::LoginRequired
            } else {
                AdapterError::HarnessError(message)
            }
        }
        RpcError::Shape { method, detail } => AdapterError::HarnessError(format!(
            "the answer to {method} is not what was expected: {detail}"
        )),
    }
}

impl Session {
    /// A session over a connection that has answered `session/new` or `session/resume`.
    #[must_use]
    pub fn new(parts: Parts) -> Arc<Self> {
        // What the program wrote while it started (a note that a part of it is off, say) explains a failure to start,
        // and is not why a session that was up has ended: from here on the tail is what the program said since.
        parts.stderr.mark();
        let mapper = Mapper::new(
            parts.mapper,
            Arc::clone(&parts.env.clock),
            parts.env.outputs.clone(),
        );
        let initial_mode = parts.modes.as_ref().map(|modes| modes.current.clone());
        Arc::new(Self {
            event_gate: tokio::sync::Mutex::new(()),
            event_batch: Mutex::new(std::collections::VecDeque::new()),
            env: parts.env,
            thread_id: parts.thread_id,
            segment_id: parts.segment_id,
            acp_id: parts.acp_id,
            rpc: parts.rpc,
            kill: parts.kill,
            exit: parts.exit,
            stderr: parts.stderr,
            learned: parts.learned,
            prompt_meta: parts.prompt_meta,
            client: parts.client,
            state: Mutex::new(State {
                mapper,
                permissions: HashMap::new(),
                forms: HashMap::new(),
                options: parts.options,
                modes: parts.modes,
                initial_mode,
                prompt: None,
                gone: None,
                serial: 0,
                plan_active: false,
                approval: None,
                off: HashSet::new(),
                closing: false,
                ended: None,
            }),
            tags: AtomicU64::new(1),
        })
    }

    /// Whether the session is gone, and why.
    #[must_use]
    pub fn gone(&self) -> Option<String> {
        lock(&self.state).gone.clone()
    }

    /// Whether a turn runs.
    #[must_use]
    pub fn busy(&self) -> bool {
        lock(&self.state).prompt.is_some()
    }

    /// Whether an extra of the companion plugin works now.
    #[must_use]
    pub fn has(&self, extra: Extra) -> bool {
        extra_on(&self.learned, &lock(&self.state), extra)
    }

    fn emit<I: IntoIterator<Item = SessionEvent>>(&self, events: I) {
        for event in events {
            lock(&self.event_batch).push_back(
                orch_agent_contract::adapter::AdapterEvent::Session {
                    thread_id: self.thread_id,
                    segment_id: self.segment_id,
                    event,
                },
            );
        }
    }

    fn report(&self, turn_id: TurnId, request: UsageRequest) {
        lock(&self.event_batch).push_back(
            orch_agent_contract::adapter::AdapterEvent::ModelRequest(Box::new(
                orch_agent_contract::requests::ModelRequestReport {
                    thread_id: self.thread_id,
                    segment_id: self.segment_id,
                    turn_id,
                    request,
                },
            )),
        );
    }

    async fn flush_events(&self) {
        loop {
            let event = lock(&self.event_batch).front().cloned();
            let Some(event) = event else { return };
            if self.env.sink.emit(event).await.is_err() {
                self.kill.notify_one();
                lock(&self.event_batch).clear();
                return;
            }
            lock(&self.event_batch).pop_front();
        }
    }

    // -- what the agent says ---------------------------------------------------------------------------

    /// Reads what the agent says, in order, until it goes away.
    pub async fn run(self: Arc<Self>, mut incoming: orch_agent_contract::flow::Receiver<Incoming>) {
        loop {
            let message = tokio::select! {
                message = incoming.recv() => message,
                () = self.env.sink.closed() => { self.kill.notify_one(); break; }
            };
            let Some(message) = message else { break };
            let _gate = self.event_gate.lock().await;
            self.flush_events().await;
            let closes = matches!(
                &message,
                Incoming::Closed { .. }
                    | Incoming::Answer {
                        result: Err(RpcError::Closed(_)),
                        ..
                    }
            );
            // A session that is being closed on purpose is told nothing of how its program ended.
            if closes && !lock(&self.state).closing {
                self.learn_how_it_ended().await;
            }
            let mut ended = false;
            match message {
                Incoming::Notification { method, params } => self.on_notification(&method, &params),
                Incoming::Request { id, method, params } => self.on_request(id, &method, params),
                Incoming::Answer { tag, result } => self.on_prompt_answer(tag, result),
                Incoming::Closed { reason } => {
                    self.on_closed(&reason);
                    ended = true;
                }
            }
            self.flush_events().await;
            if ended {
                break;
            }
        }
    }

    /// Waits a moment for the end of the program's process to be known, and keeps how it ended: what a person is told
    /// of a session that is lost.
    async fn learn_how_it_ended(&self) {
        let mut exit = self.exit.clone();
        let known = tokio::time::timeout(EXIT_GRACE, async {
            loop {
                if let Some(how) = exit.borrow_and_update().clone() {
                    return Some(how);
                }
                if exit.changed().await.is_err() {
                    return None;
                }
            }
        })
        .await;
        if let Ok(Some(how)) = known {
            lock(&self.state).ended = Some(how);
        }
    }

    fn on_notification(&self, method: &str, params: &Json) {
        match method {
            wire::client::SESSION_UPDATE => {
                if let Some(update) = params.get("update") {
                    self.on_update(update);
                }
            }
            wire::orch::STREAM => self.on_stream(params),
            wire::orch::USAGE => self.on_model_call(params),
            wire::orch::COMPACTION => self.on_compaction(params),
            wire::orch::PLAN_STATE => {
                if let Some(active) = params.get("active").and_then(Json::as_bool) {
                    lock(&self.state).plan_active = active;
                }
            }
            wire::orch::QUESTION_CLOSED => self.on_question_closed(params),
            wire::orch::EXTRA_STATE => self.on_extra_state(params),
            wire::orch::HELLO
            | wire::orch::INPUT_STATE
            | wire::orch::PLAN_REFUSED
            | wire::client::ELICITATION_COMPLETE => {}
            "$/cancel_request" => {}
            other => tracing::debug!(
                method = other,
                "a notification of the agent that means nothing to Orch"
            ),
        }
    }

    fn on_update(&self, update: &Json) {
        let mut state = lock(&self.state);
        match update.get("sessionUpdate").and_then(Json::as_str) {
            Some("current_mode_update") => {
                if let (Some(id), Some(modes)) = (
                    update.get("currentModeId").and_then(Json::as_str),
                    state.modes.as_mut(),
                ) {
                    modes.current = id.to_owned();
                }
            }
            Some("config_option_update") => {
                state.options = ConfigOption::read_all(update.get("configOptions"));
            }
            Some("user_message_chunk" | "available_commands_update") => {}
            _ => {
                let events = state.mapper.update(update);
                self.emit(events);
            }
        }
    }

    fn on_stream(&self, params: &Json) {
        let Some(notice) = StreamNotice::read(params) else {
            return;
        };
        let mut state = lock(&self.state);
        if !extra_on(&self.learned, &state, Extra::Streaming) {
            return;
        }
        let events = state.mapper.stream(&notice);
        drop(state);
        self.emit(events);
    }

    fn on_compaction(&self, params: &Json) {
        let compacting = match params.get("phase").and_then(Json::as_str) {
            Some("started") => true,
            Some("completed" | "failed") => false,
            _ => return,
        };
        let events = lock(&self.state).mapper.phase(compacting);
        self.emit(events);
    }

    fn on_extra_state(&self, params: &Json) {
        let Some(extra) = params
            .get("extra")
            .and_then(Json::as_str)
            .and_then(Extra::parse)
        else {
            return;
        };
        if params.pointer("/state/status").and_then(Json::as_str) == Some("off") {
            tracing::warn!(
                extra = extra.name(),
                reason = params
                    .pointer("/state/reason")
                    .and_then(Json::as_str)
                    .unwrap_or("unsaid"),
                "an extra of the companion plugin went off while the program ran"
            );
            lock(&self.state).off.insert(extra);
        }
    }

    fn on_question_closed(&self, params: &Json) {
        let Some(call) = params.get("callId").and_then(Json::as_str) else {
            return;
        };
        let mut state = lock(&self.state);
        let mut refs: Vec<String> = state
            .forms
            .iter()
            .filter(|(_, (_, form))| form.call_id.as_deref() == Some(call))
            .map(|(request_ref, _)| request_ref.clone())
            .collect();
        refs.sort();
        let mut closed = Vec::new();
        for request_ref in refs {
            if let Some((id, _)) = state.forms.remove(&request_ref) {
                let _ = self.rpc.respond_error(
                    &id,
                    approvals::DECLINED,
                    "the question is not waited for any more",
                );
                closed.push(SessionEvent::ApprovalClosed(ApprovalClosed { request_ref }));
            }
        }
        drop(state);
        self.emit(closed);
    }

    fn on_model_call(&self, params: &Json) {
        let Some(request) = extensions::usage_request(params) else {
            return;
        };
        if let Err(error) = request.check() {
            tracing::warn!(%error, "a model call the plugin reported is not valid, and is not counted");
            return;
        }
        let reported = lock(&self.state).mapper.model_call(request);
        for (turn, request) in reported {
            self.report(turn, request);
        }
    }

    fn on_request(self: &Arc<Self>, id: RequestId, method: &str, params: Json) {
        match method {
            wire::client::REQUEST_PERMISSION => self.on_permission(id, &params),
            wire::client::ELICITATION_CREATE => self.on_elicitation(id, &params),
            wire::orch::QUESTION_ASK => self.on_question(id, &params),
            wire::orch::PLAN_REVIEW => self.on_plan_review(id, &params),
            other => {
                if let Some(client) = &self.client
                    && crate::client::is_client_method(other)
                {
                    let session = Arc::clone(self);
                    let client = Arc::clone(client);
                    let method = other.to_owned();
                    tokio::spawn(async move {
                        let answer = client.handle(&method, &params).await;
                        let _ = match answer {
                            Ok(result) => session.rpc.respond(&id, result),
                            Err(error) => {
                                session.rpc.respond_error(&id, error.code, &error.message)
                            }
                        };
                    });
                    return;
                }
                let _ = self.rpc.respond_error(
                    &id,
                    crate::rpc::METHOD_NOT_FOUND,
                    &format!("{other} is not a method this client has"),
                );
            }
        }
    }

    fn on_permission(&self, id: RequestId, params: &Json) {
        let mut state = lock(&self.state);
        let Some(turn_id) = state.mapper.turn_id() else {
            let _ = self.rpc.respond(&id, approvals::permission_cancelled());
            return;
        };
        let tool = params.get("toolCall").cloned().unwrap_or(Json::Null);
        let known = tool
            .get("toolCallId")
            .and_then(Json::as_str)
            .and_then(|call| state.mapper.tool(call))
            .cloned();
        let Some(view) = state.mapper.view_of(&tool, known.as_ref()) else {
            let _ = self.rpc.respond(&id, approvals::permission_cancelled());
            return;
        };
        let asked = approvals::permission(&view, params, &state.mapper.place());
        let request_ref = fresh_request_ref(&mut state, "perm", Some(view.id.as_str()));
        let event = SessionEvent::ApprovalRequested(ApprovalRequested {
            turn_id,
            request_ref: request_ref.clone(),
            title: asked.title.clone(),
            request: asked.request.clone(),
        });
        state.permissions.insert(request_ref, (id, asked));
        self.emit([event]);
    }

    fn on_elicitation(&self, id: RequestId, params: &Json) {
        let mut state = lock(&self.state);
        let form = state
            .mapper
            .turn_id()
            .filter(|_| {
                params
                    .get("mode")
                    .and_then(Json::as_str)
                    .is_none_or(|mode| mode == "form")
            })
            .zip(approvals::elicitation(params));
        let Some((turn_id, form)) = form else {
            let _ = self.rpc.respond(&id, json!({ "action": "decline" }));
            return;
        };
        let request_ref = fresh_request_ref(&mut state, "ask", None);
        let event = SessionEvent::ApprovalRequested(ApprovalRequested {
            turn_id,
            request_ref: request_ref.clone(),
            title: form.title.clone(),
            request: form.request.clone(),
        });
        state.forms.insert(request_ref, (id, form));
        self.emit([event]);
    }

    /// The questions a companion plugin asks: one card, answered with the plugin's own answers.
    fn on_question(&self, id: RequestId, params: &Json) {
        let mut state = lock(&self.state);
        let form = state
            .mapper
            .turn_id()
            .filter(|_| extra_on(&self.learned, &state, Extra::Questions))
            .zip(approvals::questions(params));
        let Some((turn_id, form)) = form else {
            let _ = self.rpc.respond_error(
                &id,
                approvals::DECLINED,
                "no turn of Orch waits for an answer",
            );
            return;
        };
        let request_ref = fresh_request_ref(&mut state, "ask", None);
        let event = SessionEvent::ApprovalRequested(ApprovalRequested {
            turn_id,
            request_ref: request_ref.clone(),
            title: form.title.clone(),
            request: form.request.clone(),
        });
        state.forms.insert(request_ref, (id, form));
        drop(state);
        self.emit([event]);
    }

    /// A plan the plugin puts up for review: the card of the plan, with Proceed and Keep planning.
    fn on_plan_review(&self, id: RequestId, params: &Json) {
        let mut state = lock(&self.state);
        let asked = state
            .mapper
            .turn_id()
            .filter(|_| extra_on(&self.learned, &state, Extra::PlanMode))
            .zip(approvals::plan_review(params));
        let Some((turn_id, asked)) = asked else {
            let _ = self.rpc.respond(&id, json!({ "decision": "dismiss" }));
            return;
        };
        let request_ref = fresh_request_ref(&mut state, "plan", Some(asked.tool_call_id.as_str()));
        let event = SessionEvent::ApprovalRequested(ApprovalRequested {
            turn_id,
            request_ref: request_ref.clone(),
            title: asked.title.clone(),
            request: asked.request.clone(),
        });
        state.permissions.insert(request_ref, (id, asked));
        drop(state);
        self.emit([event]);
    }

    fn on_prompt_answer(&self, tag: u64, result: Result<Json, RpcError>) {
        let mut state = lock(&self.state);
        let Some(prompt) = state.prompt.take_if(|prompt| prompt.tag == tag) else {
            return;
        };
        // What still waits for a person is withdrawn: the agent does not ask about a turn that is over.
        let closed = withdraw(&mut state, &self.rpc);
        let (outcome, error) = match result {
            Ok(answer) => match StopReason::read(&answer) {
                StopReason::EndTurn => (TurnOutcome::Completed, None),
                StopReason::Cancelled => (TurnOutcome::Interrupted, None),
                StopReason::MaxTokens => (
                    TurnOutcome::Failed,
                    Some(
                        TurnError::new(
                            turn_error::HARNESS_ERROR,
                            "The model stopped at the most it is allowed to write for one answer.",
                            false,
                        )
                        .cut_short(true),
                    ),
                ),
                StopReason::MaxTurnRequests => (
                    TurnOutcome::Failed,
                    Some(TurnError::new(
                        turn_error::HARNESS_ERROR,
                        "The agent stopped at the most model requests it makes for one prompt.",
                        false,
                    )),
                ),
                StopReason::Refusal => (
                    TurnOutcome::Failed,
                    Some(TurnError::new(
                        turn_error::MODEL_REFUSED,
                        "The model refused to go on.",
                        false,
                    )),
                ),
                StopReason::Other => (
                    TurnOutcome::Failed,
                    Some(TurnError::new(
                        turn_error::HARNESS_ERROR,
                        "The agent ended the turn for a reason this adapter does not know.",
                        false,
                    )),
                ),
            },
            Err(RpcError::Remote { code, message, .. }) if prompt.interrupted && code == -32800 => {
                let _ = message;
                (TurnOutcome::Interrupted, None)
            }
            Err(RpcError::Remote { code, message, .. }) => {
                (TurnOutcome::Failed, Some(failure_of(code, &message)))
            }
            Err(error) => {
                let said = self.stderr.recent();
                let message = match &error {
                    RpcError::Closed(closed) => lost(closed, state.ended.as_deref(), &said),
                    other if said.is_empty() => other.to_string(),
                    other => format!("{other}: {said}"),
                };
                (
                    TurnOutcome::Failed,
                    Some(TurnError::new(turn_error::HARNESS_ERROR, &message, false).saying(&said)),
                )
            }
        };
        let (events, request) = state.mapper.end_turn(outcome, error);
        drop(state);
        self.emit(closed);
        if let Some((turn, request)) = request {
            self.report(turn, request);
        }
        self.emit(events);
    }

    fn on_closed(&self, reason: &str) {
        let mut state = lock(&self.state);
        if state.gone.is_some() {
            return;
        }
        let closing = state.closing;
        let said = self.stderr.recent();
        let why = lost(reason, state.ended.as_deref(), &said);
        state.gone = Some(if closing {
            "the session was closed".to_owned()
        } else {
            why.clone()
        });
        state.prompt = None;
        let closed = withdraw(&mut state, &self.rpc);
        // A turn that a closing session leaves unfinished was stopped; one that the process dropped, failed.
        let (events, request) = if closing {
            state.mapper.end_turn(TurnOutcome::Interrupted, None)
        } else {
            state.mapper.end_turn(
                TurnOutcome::Failed,
                Some(TurnError::new(turn_error::HARNESS_ERROR, &why, false).saying(&said)),
            )
        };
        drop(state);
        self.emit(closed);
        if let Some((turn, request)) = request {
            self.report(turn, request);
        }
        self.emit(events);
        if !closing {
            self.emit([SessionEvent::SessionLost(SessionLost { reason: why })]);
        }
    }

    // -- what Orch asks --------------------------------------------------------------------------------

    /// Starts a turn: states what it needs to the agent, and sends the prompt.
    pub async fn start_turn(&self, turn: TurnStart) -> Result<(), AdapterError> {
        {
            let state = lock(&self.state);
            if let Some(why) = &state.gone {
                return Err(AdapterError::HarnessUnavailable(why.clone()));
            }
            if state.prompt.is_some() {
                return Err(AdapterError::Other {
                    code: "conflict".to_owned(),
                    message: "a turn is already running in this session".to_owned(),
                    retryable: false,
                });
            }
        }
        let blocks = self.prompt_blocks(&turn.text, &turn.images)?;
        self.apply(&turn).await?;
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        let tag = self.tags.fetch_add(1, Ordering::Relaxed);
        let (events, native) = {
            let mut state = lock(&self.state);
            state.serial += 1;
            let native = format!("{}:{}", self.acp_id, state.serial);
            let events = state.mapper.begin_turn(turn.turn_id, &native, &turn.model);
            state.prompt = Some(Prompt {
                tag,
                interrupted: false,
            });
            (events, native)
        };
        self.emit(events);
        let mut params = json!({ "sessionId": self.acp_id, "prompt": blocks });
        let mut meta = self.prompt_meta.clone();
        if let Some(members) = meta.pointer_mut("/orch").and_then(Json::as_object_mut) {
            members.insert("turn_id".into(), json!(turn.turn_id));
            members.insert("approval_mode".into(), json!(turn.approval_mode));
            members.insert("sandbox_mode".into(), json!(turn.sandbox_mode));
            members.insert("plan".into(), json!(turn.plan));
            members.insert("native_turn_ref".into(), json!(native));
        }
        params["_meta"] = meta;
        if let Err(error) = self
            .rpc
            .call_ordered(wire::agent::SESSION_PROMPT, params, tag)
        {
            // The prompt never left: the turn ends here, so that nothing waits for an answer.
            self.on_prompt_answer(tag, Err(error.clone()));
            self.flush_events().await;
            return Err(map_rpc(error, &self.stderr));
        }
        self.flush_events().await;
        Ok(())
    }

    fn prompt_blocks(
        &self,
        text: &str,
        images: &[orch_agent_contract::session::ImageInput],
    ) -> Result<Vec<Json>, AdapterError> {
        let mut blocks = vec![wire::text_block(text)];
        for image in images {
            let Some(path) = &image.path else {
                return Err(invalid(
                    "an image of a turn is given as a file the adapter reads",
                ));
            };
            if !self
                .learned
                .initialized
                .agent_capabilities
                .prompt_capabilities
                .image
                && self.env.program.images != Some(true)
            {
                return Err(AdapterError::Unsupported("input_images".to_owned()));
            }
            let bytes = std::fs::read(path).map_err(|error| {
                invalid(&format!("the image {path} could not be read: {error}"))
            })?;
            blocks.push(wire::image_block(
                &base64::engine::general_purpose::STANDARD.encode(bytes),
                &image.media_type,
            ));
        }
        Ok(blocks)
    }

    /// Makes the agent's model, effort and mode what the turn states, changing only what differs.
    async fn apply(&self, turn: &TurnStart) -> Result<(), AdapterError> {
        let changes = self.changes_for(turn)?;
        for change in changes {
            let (method, params) = match &change {
                Change::Option { id, value } => (
                    wire::agent::SESSION_SET_CONFIG_OPTION,
                    json!({ "sessionId": self.acp_id, "configId": id, "value": value }),
                ),
                Change::Mode(id) => (
                    wire::agent::SESSION_SET_MODE,
                    json!({ "sessionId": self.acp_id, "modeId": id }),
                ),
                Change::Plan(active) => (
                    wire::orch::PLAN_SET,
                    json!({ "sessionId": self.acp_id, "active": active }),
                ),
                Change::Approval(mode) => (
                    wire::orch::APPROVAL_SET,
                    json!({ "sessionId": self.acp_id, "mode": mode }),
                ),
            };
            let answer = self
                .rpc
                .call_within(method, params, REQUEST_TIMEOUT)
                .await
                .map_err(|error| map_rpc(error, &self.stderr))?;
            let mut state = lock(&self.state);
            match change {
                Change::Option { .. } => {
                    if answer.get("configOptions").is_some() {
                        state.options = ConfigOption::read_all(answer.get("configOptions"));
                    }
                }
                Change::Mode(id) => {
                    if let Some(modes) = state.modes.as_mut() {
                        modes.current = id;
                    }
                }
                Change::Plan(active) => {
                    if answer.get("outcome").and_then(Json::as_str) != Some("cancelled") {
                        state.plan_active = active;
                    }
                }
                Change::Approval(mode) => state.approval = Some(mode),
            }
        }
        Ok(())
    }

    fn changes_for(&self, turn: &TurnStart) -> Result<Vec<Change>, AdapterError> {
        let state = lock(&self.state);
        let program = &self.env.program;
        let mut changes = Vec::new();
        let by_category = |category: &str, id: &str| {
            state
                .options
                .iter()
                .find(|option| option.id == id)
                .or_else(|| {
                    state
                        .options
                        .iter()
                        .find(|option| option.category.as_deref() == Some(category))
                })
        };
        if let Some(option) = by_category("model", &program.model_option) {
            match option.choose(&turn.model) {
                Some(choice) => {
                    if option.current_text() != Some(choice.value.as_str()) {
                        changes.push(Change::Option {
                            id: option.id.clone(),
                            value: Json::String(choice.value.clone()),
                        });
                    }
                }
                None => {
                    return Err(AdapterError::Other {
                        code: "model_unavailable".to_owned(),
                        message: format!("the program does not offer the model {}", turn.model),
                        retryable: false,
                    });
                }
            }
        }
        if let Some(effort) = &turn.effort
            && let Some(option) = by_category("thought_level", &program.effort_option)
        {
            match option.choose(effort) {
                Some(choice) => {
                    if option.current_text() != Some(choice.value.as_str()) {
                        changes.push(Change::Option {
                            id: option.id.clone(),
                            value: Json::String(choice.value.clone()),
                        });
                    }
                }
                None => {
                    return Err(AdapterError::Other {
                        code: "effort_unavailable".to_owned(),
                        message: format!(
                            "the program does not offer the effort {effort} for this model"
                        ),
                        retryable: false,
                    });
                }
            }
        }
        if program.plan_modes.is_none()
            && extra_on(&self.learned, &state, Extra::PlanMode)
            && state.plan_active != turn.plan
        {
            changes.push(Change::Plan(turn.plan));
        }
        if extra_on(&self.learned, &state, Extra::Approvals) {
            let wanted =
                if turn.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Bypass {
                    "bypass"
                } else {
                    "ask"
                };
            if state.approval != Some(wanted) {
                changes.push(Change::Approval(wanted));
            }
        }
        if let (Some(plan_modes), Some(modes)) = (&program.plan_modes, &state.modes) {
            let wanted = if turn.plan {
                Some(plan_modes.plan.clone())
            } else if modes.current == plan_modes.plan {
                plan_modes
                    .default
                    .clone()
                    .or_else(|| state.initial_mode.clone())
            } else {
                None
            };
            if let Some(wanted) = wanted
                && wanted != modes.current
            {
                if modes.has(&wanted) {
                    changes.push(Change::Mode(wanted));
                } else if turn.plan {
                    return Err(AdapterError::Unsupported("plan_mode".to_owned()));
                }
            }
        }
        Ok(changes)
    }

    /// Adds to the running turn, through the companion plugin where it announced that it can.
    pub async fn steer(&self, steer: TurnSteer) -> Result<(), AdapterError> {
        if !self.has(Extra::Steering) {
            return Err(AdapterError::Unsupported("steer".to_owned()));
        }
        if lock(&self.state).mapper.turn_id() != Some(steer.turn_id) {
            return Err(AdapterError::Other {
                code: "conflict".to_owned(),
                message: "the turn that was to be added to is not the one that runs".to_owned(),
                retryable: false,
            });
        }
        let content = self.prompt_blocks(&steer.text, &steer.images)?;
        let answer = self
            .rpc
            .call(
                wire::orch::INPUT_STEER,
                json!({ "sessionId": self.acp_id, "messageId": orch_types::new_id().to_string(), "content": content }),
            )
            .await
            .map_err(|error| map_rpc(error, &self.stderr))?;
        if answer.get("accepted").and_then(Json::as_bool) == Some(false) {
            return Err(AdapterError::Other {
                code: "conflict".to_owned(),
                message: "the program did not take the message into the turn".to_owned(),
                retryable: false,
            });
        }
        Ok(())
    }

    /// Stops the running turn: the agent is asked to, and what waits for a person is answered `cancelled`.
    /// An agent that does not stop within its grace is ended.
    pub async fn interrupt(self: &Arc<Self>, turn: TurnId) -> Result<(), AdapterError> {
        let event_gate = self.event_gate.lock().await;
        self.flush_events().await;
        let (tag, closed) = {
            let mut state = lock(&self.state);
            if state.mapper.turn_id() != Some(turn) {
                return Ok(());
            }
            let Some(prompt) = state.prompt.as_mut() else {
                return Ok(());
            };
            if prompt.interrupted {
                return Ok(());
            }
            prompt.interrupted = true;
            let tag = prompt.tag;
            (tag, withdraw(&mut state, &self.rpc))
        };
        self.emit(closed);
        self.rpc
            .notify(
                wire::agent::SESSION_CANCEL,
                Some(json!({ "sessionId": self.acp_id })),
            )
            .map_err(|error| map_rpc(error, &self.stderr))?;
        let grace = Duration::from_millis(self.env.program.cancel_grace_ms);
        let session = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            session.force_end(tag).await;
        });
        self.flush_events().await;
        drop(event_gate);
        Ok(())
    }

    /// The agent did not stop: the turn ends as interrupted, the process is ended, and the session is lost.
    async fn force_end(&self, tag: u64) {
        let ended = {
            let mut state = lock(&self.state);
            if state.prompt.as_ref().map(|prompt| prompt.tag) != Some(tag) {
                return;
            }
            state.prompt = None;
            state.gone = Some("the program did not stop when it was asked to".to_owned());
            state.mapper.end_turn(TurnOutcome::Interrupted, None)
        };
        self.kill.notify_one();
        let _gate = self.event_gate.lock().await;
        self.flush_events().await;
        if let Some((turn, request)) = ended.1 {
            self.report(turn, request);
        }
        self.emit(ended.0);
        self.emit([SessionEvent::SessionLost(SessionLost {
            reason: "the program did not stop when it was asked to; it was ended".to_owned(),
        })]);
        self.flush_events().await;
    }

    /// Answers a request of the agent with a person's decision.
    pub fn resolve(
        &self,
        request_ref: &str,
        decision: &ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let mut state = lock(&self.state);
        if let Some((id, asked)) = state.permissions.get(request_ref) {
            let answer = approvals::permission_answer(asked, decision)?;
            let id = id.clone();
            state.permissions.remove(request_ref);
            return self
                .rpc
                .respond(&id, answer)
                .map_err(|error| map_rpc(error, &self.stderr));
        }
        if let Some((id, form)) = state.forms.get(request_ref) {
            let answer = approvals::reply(form, decision)?;
            let id = id.clone();
            state.forms.remove(request_ref);
            let sent = match answer {
                approvals::Reply::Result(result) => self.rpc.respond(&id, result),
                approvals::Reply::Declined => {
                    self.rpc
                        .respond_error(&id, approvals::DECLINED, "the person did not answer")
                }
            };
            return sent.map_err(|error| map_rpc(error, &self.stderr));
        }
        Err(AdapterError::NotFound(format!(
            "the request {request_ref} is not waiting for an answer"
        )))
    }

    /// Ends the conversation: the agent is asked to close it where it can, then its process ends.
    pub async fn close(&self) {
        lock(&self.state).closing = true;
        let supports_close = self
            .learned
            .initialized
            .agent_capabilities
            .session_capabilities
            .close
            .is_some();
        if self.gone().is_none() {
            if lock(&self.state).prompt.is_some() {
                let _ = self.rpc.notify(
                    wire::agent::SESSION_CANCEL,
                    Some(json!({ "sessionId": self.acp_id })),
                );
            }
            if supports_close {
                let _ = self
                    .rpc
                    .call_within(
                        wire::agent::SESSION_CLOSE,
                        json!({ "sessionId": self.acp_id }),
                        CLOSE_TIMEOUT,
                    )
                    .await;
            }
        }
        self.rpc.finish();
        let mut exit = self.exit.clone();
        let ended = tokio::time::timeout(crate::launch::STOP_GRACE, async {
            loop {
                if exit.borrow_and_update().is_some() {
                    return;
                }
                if exit.changed().await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok();
        if !ended {
            self.kill.notify_one();
        }
        let mut state = lock(&self.state);
        if state.gone.is_none() {
            state.gone = Some("the session was closed".to_owned());
        }
    }

    /// Ends the process now, without asking.
    pub fn end(&self) {
        self.kill.notify_one();
    }
}

enum Change {
    Option { id: String, value: Json },
    Mode(String),
    Plan(bool),
    Approval(&'static str),
}

/// Answers every request that waits for a person with `cancelled` and says that it was withdrawn.
fn withdraw(state: &mut State, rpc: &Rpc) -> Vec<SessionEvent> {
    let mut closed = Vec::new();
    let mut refs: Vec<String> = state.permissions.keys().cloned().collect();
    refs.sort();
    for request_ref in refs {
        if let Some((id, asked)) = state.permissions.remove(&request_ref) {
            let _ = rpc.respond(&id, approvals::withdrawn(&asked));
            closed.push(SessionEvent::ApprovalClosed(ApprovalClosed { request_ref }));
        }
    }
    let mut refs: Vec<String> = state.forms.keys().cloned().collect();
    refs.sort();
    for request_ref in refs {
        if let Some((id, form)) = state.forms.remove(&request_ref) {
            let _ = match form.style {
                approvals::FormStyle::Elicitation => {
                    rpc.respond(&id, json!({ "action": "cancel" }))
                }
                approvals::FormStyle::Questions => rpc.respond_error(
                    &id,
                    approvals::DECLINED,
                    "the turn ended before anyone answered",
                ),
            };
            closed.push(SessionEvent::ApprovalClosed(ApprovalClosed { request_ref }));
        }
    }
    closed
}

/// How a turn ends that the agent answered with an error.
fn failure_of(code: i64, message: &str) -> TurnError {
    let lower = message.to_lowercase();
    let (kind, retryable) =
        if code == -32000 || lower.contains("authentication") || lower.contains("unauthorized") {
            (turn_error::PROVIDER_UNAUTHORIZED, false)
        } else if lower.contains("rate limit")
            || lower.contains("overloaded")
            || lower.contains("capacity")
        {
            (turn_error::PROVIDER_BUSY, true)
        } else if lower.contains("context") && lower.contains("length") {
            (turn_error::CONTEXT_OVERFLOW, false)
        } else {
            (turn_error::HARNESS_ERROR, false)
        };
    TurnError::new(kind, message, retryable).saying(message)
}

/// The directory a session's files live in, for a test that wants to see them.
#[must_use]
pub fn outputs_directory(home: &std::path::Path) -> PathBuf {
    home.join("outputs")
}
