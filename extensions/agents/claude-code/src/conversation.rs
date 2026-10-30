//! One conversation: its process, its turn and the events it yields.
//!
//! Every open session is a task of its own that owns the session's process (when it has one), reads what the
//! process writes, and answers what the adapter asks of it, one thing at a time. That keeps every rule of a
//! turn in one place: a process is started when a turn needs it and ended when it is idle; the model, the
//! effort and the mode are brought up to date between turns; a permission request waits for a person; a
//! message added to a running turn is either taken in or taken back; and a process that dies takes its turn
//! with it, reported as a lost session, to be resumed by the next.
//!
//! A call into the task can be dropped by its caller at any moment (the link gives every call a deadline);
//! the task finishes what it was asked all the same, so that the session is never left half changed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use orch_agent_contract::adapter::{AdapterError, EventSink};
use orch_agent_contract::credentials::CredentialNeed;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{
    ApprovalClosed, ContextCount, ContextFill, CredentialNeeded, HarnessRequestKind, SessionEvent,
    SessionLost, TurnStarted, turn_error,
};
use orch_agent_contract::flow;
use orch_agent_contract::session::{
    ApprovalDecision, ProviderAccess, SessionSpec, ThreadId, TurnId, TurnStart, TurnSteer,
};
use orch_types::{Clock, Id, Json};
use serde_json::json;
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::approvals::PermissionAsk;
use crate::config::{EXIT_GRACE, READY_TIMEOUT, STEER_ACK_WAIT};
use crate::events::{EventMapper, MapperSettings};
use crate::failure;
use crate::process::{ExitInfo, Launcher, ProcessEvent, Running, spawn};
use crate::provider::LOGIN_FILE;
use crate::sessions::{HarnessSetup, Launch, Origin, TurnSettings, launch, through_executor};
use crate::stream::{
    ControlOutcome, Frame, FrameKind, control_error, control_request, control_success, encode_line,
    text_block, user_message,
};
use crate::tools::ToolContext;

/// The credential the harness runs on, shared with the adapter that installs it.
pub type SharedToken = Arc<Mutex<Option<String>>>;

/// What the adapter can ask of a conversation.
#[derive(Debug)]
pub enum Command {
    /// Starts a turn: the text and the images are already the blocks of the message.
    StartTurn {
        /// The turn.
        turn: Box<TurnStart>,
        /// The content blocks of the message: the text, then the images.
        blocks: Vec<Json>,
        /// The answer.
        reply: oneshot::Sender<Result<(), AdapterError>>,
    },
    /// Adds to the running turn.
    Steer {
        /// What to add.
        steer: TurnSteer,
        /// The content blocks of the message.
        blocks: Vec<Json>,
        /// The answer.
        reply: oneshot::Sender<Result<(), AdapterError>>,
    },
    /// Stops the running turn.
    Interrupt {
        /// The turn.
        turn_id: TurnId,
        /// The answer.
        reply: oneshot::Sender<Result<(), AdapterError>>,
    },
    /// Answers a request of the harness.
    Resolve {
        /// The request.
        request_ref: String,
        /// What the person decided.
        decision: ApprovalDecision,
        /// The answer.
        reply: oneshot::Sender<Result<(), AdapterError>>,
    },
    /// The session is opened again with another setting: it takes effect with the next turn.
    Reconfigure {
        /// The session as it is now.
        spec: Box<SessionSpec>,
        /// The answer.
        reply: oneshot::Sender<()>,
    },
    /// The credential the harness runs on changed: the next turn starts a process that has it.
    CredentialChanged {
        /// The answer.
        reply: oneshot::Sender<()>,
    },
    /// Starts the process now, so that a fork holds the history of this moment.
    Warm {
        /// What the process is started with.
        turn: Box<TurnSettings>,
        /// The answer.
        reply: oneshot::Sender<Result<(), AdapterError>>,
    },
    /// The source metadata already observed, including loads during a warmed fork opening.
    InstructionSources {
        /// The answer, in native load order.
        reply: oneshot::Sender<Vec<orch_agent_contract::session::InstructionFile>>,
    },
    /// Has the harness count what fills the context with its vendor's counter: only between turns, in a
    /// process that runs. `None` where it cannot.
    CountContext {
        /// The answer.
        reply: oneshot::Sender<Option<ContextFill>>,
    },
    /// Whether this exact conversation has no running turn or pending steer.
    Quiescent {
        /// The answer is observed on the conversation's own command lane.
        reply: oneshot::Sender<bool>,
    },
    /// Ends the session.
    Close {
        /// The answer.
        reply: oneshot::Sender<()>,
    },
}

fn encoded_weight(value: &impl serde::Serialize) -> usize {
    serde_json::to_value(value)
        .map(|value| flow::json_weight(&value))
        .unwrap_or(usize::MAX)
}

fn command_weight(command: &Command) -> usize {
    let blocks = |blocks: &Vec<Json>| {
        blocks
            .capacity()
            .saturating_mul(std::mem::size_of::<Json>())
            .saturating_add(
                blocks
                    .iter()
                    .map(flow::json_weight)
                    .fold(0usize, usize::saturating_add),
            )
    };
    std::mem::size_of::<Command>().saturating_add(match command {
        Command::StartTurn {
            turn,
            blocks: values,
            ..
        } => encoded_weight(turn).saturating_add(blocks(values)),
        Command::Steer {
            steer,
            blocks: values,
            ..
        } => encoded_weight(steer).saturating_add(blocks(values)),
        Command::Resolve {
            request_ref,
            decision,
            ..
        } => request_ref
            .capacity()
            .saturating_add(encoded_weight(decision)),
        Command::Reconfigure { spec, .. } => encoded_weight(spec),
        Command::Warm { turn, .. } => turn
            .model
            .capacity()
            .saturating_add(turn.effort.as_ref().map_or(0, String::capacity))
            .saturating_add(flow::json_weight(&turn.params)),
        _ => 0,
    })
}

/// A handle on a conversation.
#[derive(Debug, Clone)]
pub struct ConversationHandle {
    sender: flow::Sender<Command>,
}

impl ConversationHandle {
    /// Asks and waits for the answer. A conversation that has ended answers that the session is gone.
    pub async fn ask<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<T>) -> Command,
    ) -> Result<T, AdapterError> {
        let (reply, answer) = oneshot::channel();
        self.sender
            .send(command(reply))
            .await
            .map_err(|_| AdapterError::SessionNotFound("the session has ended".to_owned()))?;
        answer
            .await
            .map_err(|_| AdapterError::SessionNotFound("the session has ended".to_owned()))
    }

    /// Whether the conversation still runs.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.sender.is_closed()
    }
}

/// What a conversation is made from.
pub struct Params {
    /// The thread.
    pub thread_id: ThreadId,
    /// The segment of the thread this session serves.
    pub segment_id: Id,
    /// The session.
    pub spec: SessionSpec,
    /// What fixes the harness.
    pub setup: Arc<HarnessSetup>,
    /// How processes are started.
    pub launcher: Arc<dyn Launcher>,
    /// Where events go.
    pub sink: EventSink,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// The harness's id of the session.
    pub session_id: String,
    /// Where the first process takes the conversation from.
    pub origin: Origin,
    /// The credential the harness runs on, when it has one.
    pub token: SharedToken,
}

/// Starts the task of a conversation.
#[must_use]
pub fn start(params: Params) -> ConversationHandle {
    let (sender, receiver) = flow::channel(command_weight);
    let (process_tx, process_rx) = crate::process::event_channel();
    let mapper = EventMapper::new(
        MapperSettings {
            directory: params.spec.settings.directory.clone(),
            cwd: params.spec.settings.cwd.clone(),
            raw_events: params.spec.settings.raw_events,
        },
        Arc::clone(&params.clock),
    );
    let first_origin = params.origin.clone();
    let conversation = Conversation {
        params,
        mapper,
        process: None,
        process_tx,
        process_rx,
        generation: 0,
        retiring: std::collections::HashSet::new(),
        first_origin,
        materialized: false,
        turn: None,
        last_turn_id: None,
        live: None,
        needs_restart: false,
        requests: HashMap::new(),
        context_asks: HashMap::new(),
        asks: HashMap::new(),
        steers: HashMap::new(),
        request_counter: 0,
        idle_since: None,
        last_exit: None,
        heard_at: Instant::now(),
        retrying_since: None,
        reported_permission_mode: None,
        plan_proceeded: None,
        instruction_sources: Vec::new(),
    };
    tokio::spawn(conversation.run(receiver));
    ConversationHandle { sender }
}

/// The harness's permission mode for what a turn states: plan mode while it plans, otherwise the mode of the
/// approval mode.
fn mode_of(settings: &TurnSettings) -> &'static str {
    if settings.plan {
        crate::approvals::PLAN_MODE
    } else {
        crate::approvals::permission_mode(settings.approval_mode)
    }
}

struct ActiveTurn {
    turn_id: TurnId,
}

/// A request for what fills the context that waits for its answer.
struct ContextAsk {
    /// Counted by the vendor's counter (`full`), or estimated by the harness (`summary`).
    count: ContextCount,
    /// Whoever waits for it; nobody for the breakdown asked at the end of a turn.
    reply: Option<oneshot::Sender<Option<ContextFill>>>,
}

struct PendingSteer {
    reply: Option<oneshot::Sender<Result<(), AdapterError>>>,
    deadline: Instant,
}

struct Conversation {
    params: Params,
    mapper: EventMapper,
    process: Option<Running>,
    process_tx: flow::Sender<(u64, ProcessEvent)>,
    process_rx: flow::Receiver<(u64, ProcessEvent)>,
    generation: u64,
    /// Native generations whose quiet shutdown has not reported its actual exit.
    retiring: std::collections::HashSet<u64>,
    /// Where the conversation comes from until a message has made it exist in the harness's home.
    first_origin: Origin,
    materialized: bool,
    turn: Option<ActiveTurn>,
    last_turn_id: Option<TurnId>,
    live: Option<TurnSettings>,
    needs_restart: bool,
    /// The tool use of a plan a person proceeded with, until its result shows that plan mode was left.
    plan_proceeded: Option<String>,
    requests: HashMap<String, oneshot::Sender<ControlOutcome>>,
    /// Requests for what fills the context, by their id.
    context_asks: HashMap<String, ContextAsk>,
    asks: HashMap<String, PermissionAsk>,
    steers: HashMap<String, PendingSteer>,
    request_counter: u64,
    idle_since: Option<Instant>,
    last_exit: Option<String>,
    /// When the process last wrote a line.
    heard_at: Instant,
    /// Since when the harness has been announcing that it tries again, with nothing else between.
    retrying_since: Option<Instant>,
    /// The native permission mode confirmed by this process's system/init.
    reported_permission_mode: Option<String>,
    instruction_sources: Vec<orch_agent_contract::session::InstructionFile>,
}

impl Conversation {
    async fn run(mut self, mut commands: flow::Receiver<Command>) {
        loop {
            let wake = self.next_wake();
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => {
                        if self.handle(command).await {
                            break;
                        }
                    }
                    None => {
                        self.shutdown().await;
                        break;
                    }
                },
                event = self.process_rx.recv() => {
                    if let Some((generation, event)) = event {
                        self.on_process_event(generation, event).await;
                    }
                }
                () = sleep_until(wake) => self.on_timer().await,
                () = self.params.sink.closed() => {
                    if let Some(mut process) = self.process.take() { process.kill(); }
                    break;
                },
            }
        }
    }

    /// The nearest moment something is due: an idle process to end, or a steer to answer.
    fn next_wake(&self) -> Option<Instant> {
        let idle = self.process.as_ref().and_then(|_| {
            self.idle_since.and_then(|since| {
                self.params
                    .setup
                    .config
                    .idle_exit()
                    .filter(|_| through_executor(&self.params.spec))
                    .map(|after| since + after)
            })
        });
        let steer = self
            .steers
            .values()
            .filter(|steer| steer.reply.is_some())
            .map(|steer| steer.deadline)
            .min();
        [idle, steer, self.impatience_due()]
            .into_iter()
            .flatten()
            .min()
    }

    /// The moment a turn runs out of the patience it is given, if one does: it has heard nothing for too long
    /// while the harness waits for the model alone, or the harness has gone on trying again for too long.
    fn impatience_due(&self) -> Option<Instant> {
        self.turn.as_ref()?;
        self.process.as_ref()?;
        let config = &self.params.setup.config;
        let silent = config
            .silence()
            .filter(|_| self.asks.is_empty() && !self.mapper.waiting_on_a_tool())
            .map(|after| self.heard_at + after);
        let trying = config
            .retry_patience()
            .zip(self.retrying_since)
            .map(|(after, since)| since + after);
        [silent, trying].into_iter().flatten().min()
    }

    async fn on_timer(&mut self) {
        let now = Instant::now();
        for steer in self.steers.values_mut() {
            if steer.deadline <= now
                && let Some(reply) = steer.reply.take()
            {
                let _ = reply.send(Ok(()));
            }
        }
        if self.impatience_due().is_some_and(|due| due <= now) {
            self.give_up_on_the_harness(now).await;
            return;
        }
        let idle_due = self.turn.is_none()
            && self.idle_since.is_some_and(|since| {
                self.params
                    .setup
                    .config
                    .idle_exit()
                    .is_some_and(|after| since + after <= now)
            });
        if idle_due && through_executor(&self.params.spec) {
            self.stop_process_quietly();
        }
    }

    async fn emit(&self, event: SessionEvent) {
        if self
            .params
            .sink
            .session(
                self.params.thread_id,
                self.params.spec.settings.segment_id,
                event,
            )
            .await
            .is_err()
            && let Some(kill) = self.process.as_ref().and_then(Running::kill_handle)
        {
            kill.notify_one();
        }
    }

    async fn emit_all(&mut self, events: Vec<SessionEvent>) {
        let mut requests = Some(self.mapper.take_requests());
        for event in events {
            if matches!(event, SessionEvent::TurnCompleted(_)) {
                for (turn, request) in requests.take().unwrap_or_default() {
                    let _ = self
                        .params
                        .sink
                        .request(
                            self.params.thread_id,
                            self.params.spec.settings.segment_id,
                            turn,
                            request,
                        )
                        .await;
                }
            }
            self.emit(event).await;
        }
        for (turn, request) in requests.take().unwrap_or_default() {
            let _ = self
                .params
                .sink
                .request(
                    self.params.thread_id,
                    self.params.spec.settings.segment_id,
                    turn,
                    request,
                )
                .await;
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // Commands

    /// Handles one command; `true` when the conversation is over.
    async fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::StartTurn {
                turn,
                blocks,
                reply,
            } => {
                let outcome = self.start_turn(*turn, blocks).await;
                let _ = reply.send(outcome);
            }
            Command::Steer {
                steer,
                blocks,
                reply,
            } => self.steer(&steer, blocks, reply).await,
            Command::Interrupt { turn_id, reply } => {
                let _ = reply.send(self.interrupt(turn_id).await);
            }
            Command::Resolve {
                request_ref,
                decision,
                reply,
            } => {
                let _ = reply.send(self.resolve(&request_ref, &decision).await);
            }
            Command::Reconfigure { spec, reply } => {
                self.reconfigure(*spec);
                let _ = reply.send(());
            }
            Command::CredentialChanged { reply } => {
                if self.process.is_some() {
                    self.needs_restart = true;
                }
                let _ = reply.send(());
            }
            Command::Warm { turn, reply } => {
                let outcome = self.ensure_process(&turn).await;
                let _ = reply.send(outcome);
            }
            Command::InstructionSources { reply } => {
                let _ = reply.send(self.instruction_sources.clone());
            }
            Command::CountContext { reply } => {
                // Never while a turn runs: a count is asked of an idle harness, between turns.
                if self.turn.is_some() || self.process.is_none() {
                    let _ = reply.send(None);
                } else {
                    self.ask_context(ContextCount::Counted, Some(reply)).await;
                }
            }
            Command::Quiescent { reply } => {
                let _ = reply.send(
                    self.turn.is_none() && self.steers.is_empty() && self.retiring.is_empty(),
                );
            }
            Command::Close { reply } => {
                self.shutdown().await;
                let _ = reply.send(());
                return true;
            }
        }
        false
    }

    fn reconfigure(&mut self, spec: SessionSpec) {
        let changed = spec.mcp != self.params.spec.mcp
            || spec.settings.subagent_limits != self.params.spec.settings.subagent_limits
            || spec.settings.instructions != self.params.spec.settings.instructions
            || spec.settings.memory != self.params.spec.settings.memory
            || spec.settings.orch_tools != self.params.spec.settings.orch_tools
            || spec.settings.orch_tool_schemas != self.params.spec.settings.orch_tool_schemas
            || spec.settings.cwd != self.params.spec.settings.cwd
            || spec.settings.directory != self.params.spec.settings.directory
            || spec.settings.mounts != self.params.spec.settings.mounts
            || spec.session_key != self.params.spec.session_key
            || spec.exec != self.params.spec.exec
            || spec.provider != self.params.spec.provider;
        self.mapper = EventMapper::new(
            MapperSettings {
                directory: spec.settings.directory.clone(),
                cwd: spec.settings.cwd.clone(),
                raw_events: spec.settings.raw_events,
            },
            Arc::clone(&self.params.clock),
        );
        self.params.spec = spec;
        if changed && self.process.is_some() {
            self.needs_restart = true;
        }
    }

    fn turn_settings(turn: &TurnStart) -> TurnSettings {
        TurnSettings {
            model: turn.model.clone(),
            effort: turn.effort.clone(),
            approval_mode: turn.approval_mode,
            sandbox_mode: turn.sandbox_mode,
            params: turn.params.clone(),
            plan: turn.plan,
        }
    }

    async fn start_turn(&mut self, turn: TurnStart, blocks: Vec<Json>) -> Result<(), AdapterError> {
        if turn.sandbox_mode == orch_agent_contract::descriptor::SandboxMode::ReadOnly {
            return Err(AdapterError::Unsupported(
                "a native read-only sandbox has not been proved for this Claude Code build"
                    .to_owned(),
            ));
        }
        if self.turn.is_some() {
            return Err(AdapterError::HarnessError(
                "a turn is running in this session".to_owned(),
            ));
        }
        self.check_credential_is_there().await?;
        // Directory files are cached by a running native process. A resumed process reads the
        // actual files again; doing this at an idle turn boundary also preserves in-turn steering.
        if self.materialized
            && !crate::sessions::through_executor(&self.params.spec)
            && self.process.is_some()
        {
            self.needs_restart = true;
        }
        let settings = Self::turn_settings(&turn);
        if settings.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Auto {
            if !crate::descriptor::auto_proven() {
                return Err(AdapterError::Unsupported(
                    "Auto is unavailable until the integrator's real trial".to_owned(),
                ));
            }
            if self.params.spec.settings.approval_mode != settings.approval_mode {
                self.needs_restart = true;
            }
        }
        self.params.spec.settings.approval_mode = settings.approval_mode;
        self.bring_up_to_date(&settings).await?;
        self.ensure_process(&settings).await?;
        self.idle_since = None;
        self.heard_at = Instant::now();
        self.retrying_since = None;
        self.mapper.begin_turn(turn.turn_id);
        self.last_turn_id = Some(turn.turn_id);
        self.turn = Some(ActiveTurn {
            turn_id: turn.turn_id,
        });
        let message = user_message(&turn.turn_id.to_string(), blocks);
        self.emit(SessionEvent::TurnStarted(TurnStarted {
            turn_id: turn.turn_id,
            native_turn_ref: turn.turn_id.to_string(),
        }))
        .await;
        self.emit_all(Vec::new()).await;
        if !self.write(&message).await {
            // The process went between the handshake and now; the turn ends with the session.
            self.lose_session("the harness process ended before it took the input")
                .await;
            return Err(AdapterError::HarnessError(
                "the harness process ended before it took the input".to_owned(),
            ));
        }
        self.materialized = true;
        Ok(())
    }

    /// Makes a running process agree with what the turn states, live where the harness allows and by a
    /// restart where it does not.
    async fn bring_up_to_date(&mut self, wanted: &TurnSettings) -> Result<(), AdapterError> {
        if self.process.is_none() {
            return Ok(());
        }
        let Some(live) = self.live.clone() else {
            return Ok(());
        };
        let params_changed = crate::parameters::Values::from_json(&live.params)
            != crate::parameters::Values::from_json(&wanted.params);
        // The model and the effort reach the harness through its environment, which only a new process
        // reads; the permission mode is the one thing that is changed in the running one.
        let must_restart = self.needs_restart
            || live.model != wanted.model
            || live.effort != wanted.effort
            || live.sandbox_mode != wanted.sandbox_mode
            || (live.approval_mode == ApprovalMode::Bypass)
                != (wanted.approval_mode == ApprovalMode::Bypass)
            || params_changed;
        if must_restart {
            self.restart_process().await;
            return Ok(());
        }
        if live.approval_mode != wanted.approval_mode || live.plan != wanted.plan {
            let accepted = self
                .control(json!({ "subtype": "set_permission_mode", "mode": mode_of(wanted) }))
                .await;
            if matches!(accepted, Some(ControlOutcome::Success(_))) {
                if let Some(live) = &mut self.live {
                    live.approval_mode = wanted.approval_mode;
                    live.plan = wanted.plan;
                }
            } else {
                self.restart_process().await;
            }
        }
        Ok(())
    }

    /// Once a plan that a person proceeded with has left plan mode, the harness is put back into the mode of
    /// the thread's approval mode, which leaving plan mode does not restore. Sent without waiting: the answer
    /// changes nothing that the adapter does next.
    async fn restore_after_plan(&mut self) {
        self.plan_proceeded = None;
        let Some(live) = &mut self.live else {
            return;
        };
        live.plan = false;
        let mode = crate::approvals::permission_mode(live.approval_mode);
        let id = self.next_request_id();
        let (reply, _) = oneshot::channel();
        self.requests.insert(id.clone(), reply);
        if !self
            .write(&control_request(
                &id,
                json!({ "subtype": "set_permission_mode", "mode": mode }),
            ))
            .await
        {
            self.requests.remove(&id);
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // The process

    async fn ensure_process(&mut self, settings: &TurnSettings) -> Result<(), AdapterError> {
        if self.process.is_some() {
            return Ok(());
        }
        let session_id = self.params.session_id.clone();
        let origin = if self.materialized {
            Origin::Resume
        } else {
            self.first_origin.clone()
        };
        let token = self
            .params
            .token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if !self.params.spec.settings.instructions.trim().is_empty() {
            let path = crate::sessions::instructions_path(&self.params.setup, &self.params.spec);
            let parent = path.parent().ok_or_else(|| {
                AdapterError::HarnessError("the instruction file has no directory".to_owned())
            })?;
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                AdapterError::HarnessError(format!(
                    "the instruction directory could not be made: {error}"
                ))
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                    .await
                    .map_err(|error| {
                        AdapterError::HarnessError(format!(
                            "the instruction directory could not be protected: {error}"
                        ))
                    })?;
            }
            tokio::fs::write(&path, self.params.spec.settings.instructions.as_bytes())
                .await
                .map_err(|error| {
                    AdapterError::HarnessError(format!(
                        "the instruction file could not be written: {error}"
                    ))
                })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .await
                    .map_err(|error| {
                        AdapterError::HarnessError(format!(
                            "the instruction file could not be protected: {error}"
                        ))
                    })?;
            }
        }
        let spec = launch(&Launch {
            setup: &self.params.setup,
            spec: &self.params.spec,
            session_id: &session_id,
            origin: &origin,
            turn: settings,
            oauth_token: token.as_deref(),
        });
        if through_executor(&self.params.spec)
            && let Err(error) = tokio::fs::create_dir_all(&spec.cwd).await
        {
            return Err(AdapterError::HarnessError(format!(
                "the working directory of the session could not be made: {error}"
            )));
        }
        self.generation += 1;
        let running = match spawn(
            self.params.launcher.as_ref(),
            &spec,
            self.generation,
            self.process_tx.clone(),
        ) {
            Ok(running) => running,
            Err(error) => return Err(AdapterError::HarnessUnavailable(error.to_string())),
        };
        self.process = Some(running);
        // A process that begins the conversation counts its use from nothing; one that resumes or forks it
        // starts from what the harness restored.
        self.mapper.process_started(matches!(origin, Origin::New));
        self.reported_permission_mode = None;
        self.needs_restart = false;
        match self.handshake().await {
            Ok(()) => {
                self.live = Some(settings.clone());
                Ok(())
            }
            Err(error) => {
                self.stop_process_quietly();
                Err(error)
            }
        }
    }

    /// The first request of a process, which only a harness that has started and loaded its session can
    /// answer: a session that cannot be resumed ends the process before this is answered.
    async fn handshake(&mut self) -> Result<(), AdapterError> {
        let id = self.next_request_id();
        let (reply, answer) = oneshot::channel();
        self.requests.insert(id.clone(), reply);
        let line = control_request(
            &id,
            json!({ "subtype": "initialize", "hooks": crate::review::hooks() }),
        );
        if !self.write(&line).await {
            return Err(AdapterError::HarnessError(
                "the harness process could not be written to".to_owned(),
            ));
        }
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut answer = answer;
        loop {
            tokio::select! {
                outcome = &mut answer => {
                    return match outcome {
                        Ok(ControlOutcome::Success(_)) => {
                            if self.params.spec.settings.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Auto {
                                crate::review::confirmed(self.reported_permission_mode.as_deref(), None)
                            } else { Ok(()) }
                        },
                        Ok(ControlOutcome::Error(error)) => Err(AdapterError::HarnessError(error)),
                        Err(_) => Err(self.startup_failure()),
                    };
                }
                event = self.process_rx.recv() => {
                    if let Some((generation, event)) = event {
                        self.on_process_event(generation, event).await;
                        if self.process.is_none() {
                            self.requests.remove(&id);
                            return Err(self.startup_failure());
                        }
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    self.requests.remove(&id);
                    return Err(AdapterError::Timeout);
                }
            }
        }
    }

    fn startup_failure(&mut self) -> AdapterError {
        let said = self
            .last_exit
            .take()
            .unwrap_or_else(|| "the harness process ended".to_owned());
        if said.contains("No conversation found") {
            AdapterError::SessionNotFound(self.params.session_id.clone())
        } else {
            AdapterError::HarnessError(said)
        }
    }

    fn next_request_id(&mut self) -> String {
        self.request_counter += 1;
        format!("orch-{}", self.request_counter)
    }

    /// Writes a message to the process; `false` when there is none.
    async fn write(&self, message: &Json) -> bool {
        if let Some(raw) = self.mapper.raw_outgoing(message) {
            self.emit(raw).await;
        }
        match self.process.as_ref() {
            Some(process) => process.write(encode_line(message)).await,
            None => false,
        }
    }

    /// A request to the harness and its answer; `None` when the process went before it answered.
    async fn control(&mut self, request: Json) -> Option<ControlOutcome> {
        let id = self.next_request_id();
        let (reply, mut answer) = oneshot::channel();
        self.requests.insert(id.clone(), reply);
        if !self.write(&control_request(&id, request)).await {
            self.requests.remove(&id);
            return None;
        }
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            tokio::select! {
                outcome = &mut answer => return outcome.ok(),
                event = self.process_rx.recv() => {
                    if let Some((generation, event)) = event {
                        self.on_process_event(generation, event).await;
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    self.requests.remove(&id);
                    return None;
                }
            }
        }
    }

    /// Asks the harness what fills its context, without waiting: the answer comes as a control response, and
    /// whoever waits for it gets it then. A harness that cannot be written to answers nothing.
    async fn ask_context(
        &mut self,
        count: ContextCount,
        reply: Option<oneshot::Sender<Option<ContextFill>>>,
    ) {
        let id = self.next_request_id();
        if self
            .write(&control_request(&id, crate::context_usage::request(count)))
            .await
        {
            self.context_asks.insert(id, ContextAsk { count, reply });
        } else if let Some(reply) = reply {
            let _ = reply.send(None);
        }
    }

    /// The harness answered what fills its context. The breakdown is reported with the last use; an older
    /// harness that does not know the request says so, which leaves the context without a breakdown and fails
    /// nothing.
    async fn on_context_answer(&mut self, ask: ContextAsk, outcome: ControlOutcome) {
        let fill = match outcome {
            ControlOutcome::Success(answer) => {
                let (fill, event) = self.mapper.context_answered(&answer, ask.count);
                if let Some(event) = event {
                    self.emit(event).await;
                }
                fill
            }
            ControlOutcome::Error(error) => {
                tracing::debug!(%error, "the harness gave no breakdown of its context");
                // Its own estimate, which needs nothing beyond the machine, refused: it has none. A count by the
                // vendor's counter may fail where the summary works (an upstream without one).
                if ask.count == ContextCount::Estimated {
                    self.mapper.breakdown_refused();
                }
                None
            }
        };
        if let Some(reply) = ask.reply {
            let _ = reply.send(fill);
        }
    }

    /// Ends the process and starts another from the same conversation.
    async fn restart_process(&mut self) {
        // The old process is told to end; the new one is a different generation, so its lines are never
        // mistaken for the old one's.
        self.stop_process_quietly();
    }

    /// Ends the process without it being an event: input closed, killed if it lingers.
    fn stop_process_quietly(&mut self) {
        if let Some(mut process) = self.process.take() {
            self.retiring.insert(process.generation());
            process.close_input();
            // A process that has not gone within the grace period is killed. The process object is moved
            // into a task that waits, so the conversation does not.
            tokio::spawn(async move {
                tokio::time::sleep(EXIT_GRACE).await;
                process.kill();
            });
        }
        self.live = None;
        self.idle_since = None;
    }

    async fn shutdown(&mut self) {
        let refusals: Vec<_> = self.asks.drain().collect();
        for (request_ref, ask) in refusals {
            let _ = self
                .write(&control_success(
                    &ask.control_id,
                    ask.refusal("the session was closed"),
                ))
                .await;
            self.emit(SessionEvent::ApprovalClosed(ApprovalClosed { request_ref }))
                .await;
        }
        for (_, steer) in self.steers.drain() {
            if let Some(reply) = steer.reply {
                let _ = reply.send(Err(AdapterError::SessionNotFound(
                    "the session was closed".to_owned(),
                )));
            }
        }
        self.stop_process_quietly();
    }

    // -----------------------------------------------------------------------------------------------------
    // Interrupt, steer, approvals

    async fn interrupt(&mut self, turn_id: TurnId) -> Result<(), AdapterError> {
        let Some(active) = &self.turn else {
            return Ok(());
        };
        if active.turn_id != turn_id {
            return Ok(());
        }
        self.mapper.note_interrupt();
        let id = self.next_request_id();
        let line = control_request(&id, json!({ "subtype": "interrupt" }));
        if self.write(&line).await {
            Ok(())
        } else {
            Err(AdapterError::HarnessError(
                "the harness process is gone".to_owned(),
            ))
        }
    }

    async fn steer(
        &mut self,
        steer: &TurnSteer,
        blocks: Vec<Json>,
        reply: oneshot::Sender<Result<(), AdapterError>>,
    ) {
        let running = self
            .turn
            .as_ref()
            .is_some_and(|active| active.turn_id == steer.turn_id);
        if !running {
            let _ = reply.send(Err(AdapterError::NotFound(
                "no such turn is running".to_owned(),
            )));
            return;
        }
        let uuid = orch_types::new_id().to_string();
        if !self.write(&user_message(&uuid, blocks)).await {
            let _ = reply.send(Err(AdapterError::HarnessError(
                "the harness process is gone".to_owned(),
            )));
            return;
        }
        self.steers.insert(
            uuid,
            PendingSteer {
                reply: Some(reply),
                deadline: Instant::now() + STEER_ACK_WAIT,
            },
        );
    }

    async fn resolve(
        &mut self,
        request_ref: &str,
        decision: &ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let Some(ask) = self.asks.get(request_ref) else {
            return Err(AdapterError::NotFound(format!(
                "there is no request {request_ref:?} to answer"
            )));
        };
        let answer = ask.answer(decision)?;
        let line = control_success(&ask.control_id, answer);
        // A plan proceeded with leaves plan mode when the harness has run its tool; the thread's own mode is
        // set again once that tool's result is seen.
        if ask.tool_name == crate::approvals::EXIT_PLAN_TOOL
            && matches!(decision, ApprovalDecision::Option { option_id, .. } if option_id == crate::approvals::OPTION_PROCEED)
        {
            self.plan_proceeded = Some(ask.tool_use_id.clone().unwrap_or_default());
        }
        self.asks.remove(request_ref);
        if self.write(&line).await {
            Ok(())
        } else {
            Err(AdapterError::HarnessError(
                "the harness process is gone".to_owned(),
            ))
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // What the process says

    async fn on_process_event(&mut self, generation: u64, event: ProcessEvent) {
        if generation != self.generation && !matches!(event, ProcessEvent::Exited(_)) {
            return;
        }
        match event {
            ProcessEvent::Line(line) => self.on_line(&line).await,
            ProcessEvent::TooLong => {
                tracing::warn!("the harness wrote a line longer than a frame may be")
            }
            ProcessEvent::Unreadable(error) => {
                tracing::warn!("a line of the harness could not be read: {error}")
            }
            ProcessEvent::Exited(info) => self.on_exit(generation, &info).await,
        }
    }

    async fn on_line(&mut self, line: &str) {
        self.heard_at = Instant::now();
        let frame = match Frame::parse(line) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::debug!("{error}");
                return;
            }
        };
        crate::provider::observe_frame(
            frame.raw(),
            self.params.setup.home.as_deref(),
            &self.params.sink,
            self.params.clock.time(),
        )
        .await;
        self.note_trying_again(&frame);
        if frame.kind() == FrameKind::System
            && let Some(mode) = frame.text("permissionMode")
        {
            self.reported_permission_mode = Some(mode.to_owned());
        }
        if let Some(tool_use) = &self.plan_proceeded
            && frame.kind() == FrameKind::User
            && frame.blocks().iter().any(|block| {
                block.get("type").and_then(Json::as_str) == Some("tool_result")
                    && (tool_use.is_empty()
                        || block.get("tool_use_id").and_then(Json::as_str) == Some(tool_use))
            })
        {
            self.restore_after_plan().await;
        }
        match frame.kind() {
            FrameKind::ControlRequest => self.on_control_request(&frame).await,
            FrameKind::ControlResponse => {
                if let Some((id, outcome)) = frame.control_outcome() {
                    if let Some(ask) = self.context_asks.remove(&id) {
                        self.on_context_answer(ask, outcome).await;
                    } else if let Some(reply) = self.requests.remove(&id) {
                        let _ = reply.send(outcome);
                    }
                }
            }
            FrameKind::ControlCancelRequest => {
                if let Some(id) = frame.text("request_id")
                    && self.asks.remove(id).is_some()
                {
                    self.emit(SessionEvent::ApprovalClosed(ApprovalClosed {
                        request_ref: id.to_owned(),
                    }))
                    .await;
                }
            }
            _ => {
                let is_result = frame.kind() == FrameKind::Result;
                let shortened = frame.kind() == FrameKind::System
                    && frame.subtype() == Some("compact_boundary");
                let mut events = self.mapper.on_frame(&frame);
                if is_result {
                    self.say_when_the_credential_was_refused(&mut events);
                }
                self.emit_all(events).await;
                for uuid in self.mapper.take_echoed() {
                    if let Some(steer) = self.steers.remove(&uuid)
                        && let Some(reply) = steer.reply
                    {
                        let _ = reply.send(Ok(()));
                    }
                }
                // What fills the context after a turn, and after the conversation was shortened, as the harness
                // estimates it from its last response: no request leaves the machine for it.
                if is_result || shortened {
                    self.ask_context(ContextCount::Estimated, None).await;
                }
                if is_result {
                    self.on_turn_over().await;
                } else if self.turn.is_some()
                    && is_unauthorized_retry(&frame)
                    && self.token_cannot_be_renewed()
                {
                    self.give_up_on_the_credential().await;
                }
            }
        }
    }

    async fn on_control_request(&mut self, frame: &Frame) {
        let Some(request_id) = frame.text("request_id") else {
            return;
        };
        if let Some(request) = frame.get("request")
            && request.get("subtype").and_then(Json::as_str) == Some("hook_callback")
            && request.get("callback_id").and_then(Json::as_str) == Some("orch-instructions-loaded")
        {
            // This callback only observes. It never changes loading or supplies an input.
            let _ = self.write(&control_success(request_id, json!({}))).await;
            if let Some(source) = request
                .get("input")
                .and_then(|input| crate::instruction_sources::loaded(input, &self.params.spec))
            {
                if let Some(old) = self.instruction_sources.iter_mut().find(|old| {
                    old.path == source.path && old.label == source.label && old.home == source.home
                }) {
                    *old = source.clone();
                } else {
                    self.instruction_sources.push(source.clone());
                }
                self.emit(SessionEvent::InstructionSources(
                    orch_agent_contract::events::InstructionSources {
                        sources: vec![source],
                    },
                ))
                .await;
            }
            return;
        }
        if let Some(request) = frame.get("request")
            && request.get("subtype").and_then(Json::as_str) == Some("hook_callback")
            && request.get("callback_id").and_then(Json::as_str) == Some("orch-permission-review")
        {
            if let Some(input) = request.get("input") {
                let events = crate::review::denial(
                    input,
                    &self.params.spec.settings,
                    self.turn
                        .as_ref()
                        .map(|turn| turn.turn_id)
                        .or(self.last_turn_id),
                    self.params.clock.time(),
                );
                self.emit_all(events).await;
            }
            // Observe the native decision; do not retry, approve, deny or inject words.
            let _ = self.write(&control_success(request_id, json!({}))).await;
            return;
        }
        let Some(ask) = PermissionAsk::from_frame(frame) else {
            let subtype = frame
                .get("request")
                .and_then(|request| request.get("subtype"))
                .and_then(Json::as_str)
                .unwrap_or("a request");
            let _ = self
                .write(&control_error(
                    request_id,
                    &format!("{subtype} is not something this host answers"),
                ))
                .await;
            return;
        };
        let turn_id = self
            .turn
            .as_ref()
            .map(|turn| turn.turn_id)
            .or(self.last_turn_id);
        let Some(turn_id) = turn_id else {
            let _ = self
                .write(&control_success(
                    &ask.control_id,
                    ask.refusal("no turn is running to ask a person in"),
                ))
                .await;
            return;
        };
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.approval_mode == ApprovalMode::Bypass)
            && !matches!(
                ask.kind(),
                HarnessRequestKind::Plan | HarnessRequestKind::UserInput
            )
        {
            let answer = if self.turn.is_none() {
                ask.refusal("no turn is running to ask a person in")
            } else if self.live.as_ref().is_some_and(|live| live.plan) && !ask.read_only() {
                ask.refusal("Plan mode cannot approve tool actions.")
            } else {
                ask.allow()
            };
            let _ = self.write(&control_success(&ask.control_id, answer)).await;
            return;
        }
        let ctx = ToolContext {
            directory: &self.params.spec.settings.directory,
            cwd: &self.params.spec.settings.cwd,
        };
        let approval = ask.approval(turn_id, &ctx);
        self.asks.insert(approval.request_ref.clone(), ask);
        self.emit(SessionEvent::ApprovalRequested(approval)).await;
    }

    // -----------------------------------------------------------------------------------------------------
    // The credential

    /// The token the process runs on, if the adapter holds one.
    fn held_token(&self) -> Option<String> {
        self.params
            .token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the connector can issue the credential again: it is a copy or a login that it gave.
    fn connector_issues(&self) -> bool {
        matches!(
            self.params.spec.provider,
            ProviderAccess::Shared { .. } | ProviderAccess::Handover { .. }
        )
    }

    /// Whether the process runs on a token that nothing renews but a new copy or a new sign-in: a harness
    /// that is refused with it has no other way to go on, so it is not left to try again for minutes.
    fn token_cannot_be_renewed(&self) -> bool {
        match self.params.spec.provider {
            ProviderAccess::Shared { .. } => true,
            ProviderAccess::Native { .. } => self.held_token().is_some(),
            ProviderAccess::Forwarded { .. } | ProviderAccess::Handover { .. } => false,
        }
    }

    /// A session that runs on a copy or a login it is given has nothing to run on before it has one: the
    /// connector is asked for it, and the turn does not start.
    async fn check_credential_is_there(&self) -> Result<(), AdapterError> {
        let missing = match &self.params.spec.provider {
            ProviderAccess::Shared { .. } => self.held_token().is_none(),
            ProviderAccess::Handover { .. } => self
                .params
                .setup
                .home
                .as_ref()
                .is_none_or(|home| !home.join(LOGIN_FILE).exists()),
            ProviderAccess::Forwarded { .. } | ProviderAccess::Native { .. } => false,
        };
        if missing {
            self.emit(SessionEvent::CredentialNeeded(CredentialNeeded {
                reason: CredentialNeed::Missing,
            }))
            .await;
            return Err(AdapterError::CredentialUnusable(
                "the session has no credential to run on yet".to_owned(),
            ));
        }
        Ok(())
    }

    /// A turn that ends because the provider refused the credential: where the connector can issue it again,
    /// it is told before the turn ends.
    fn say_when_the_credential_was_refused(&self, events: &mut Vec<SessionEvent>) {
        if !self.connector_issues() {
            return;
        }
        let refused = events.iter().position(|event| {
            matches!(event, SessionEvent::TurnCompleted(end)
                if end.error.as_ref().is_some_and(|error| error.code == turn_error::PROVIDER_UNAUTHORIZED))
        });
        if let Some(at) = refused {
            events.insert(
                at,
                SessionEvent::CredentialNeeded(CredentialNeeded {
                    reason: CredentialNeed::Rejected,
                }),
            );
        }
    }

    /// The provider refused a token that nothing renews: the process is ended now, the turn fails, and where
    /// the connector issues the token it is asked for a new copy. The next turn starts a process on whatever
    /// credential it then has.
    async fn give_up_on_the_credential(&mut self) {
        let shared = matches!(self.params.spec.provider, ProviderAccess::Shared { .. });
        if shared {
            self.emit(SessionEvent::CredentialNeeded(CredentialNeeded {
                reason: CredentialNeed::Rejected,
            }))
            .await;
        }
        if let Some(mut process) = self.process.take() {
            process.kill();
        }
        self.live = None;
        let message = if shared {
            "The provider refused the copy of the credential this session was given; a new copy is needed."
        } else {
            "The provider refused the token the harness is signed in with; sign in again."
        };
        let events = self
            .mapper
            .fail_turn(turn_error::PROVIDER_UNAUTHORIZED, message, shared);
        self.emit_all(events).await;
        self.on_turn_over().await;
    }

    /// Notes whether the harness is trying again or has got further: only a message of the conversation, or a
    /// result, is progress.
    fn note_trying_again(&mut self, frame: &Frame) {
        match frame.kind() {
            FrameKind::System if frame.subtype() == Some("api_retry") => {
                self.retrying_since.get_or_insert_with(Instant::now);
            }
            FrameKind::StreamEvent | FrameKind::Assistant | FrameKind::User | FrameKind::Result => {
                self.retrying_since = None;
            }
            _ => {}
        }
    }

    /// The turn has run out of patience: the harness is ended, and the turn with it, with what the harness
    /// last said. The next turn starts a process of its own that takes the session up where its transcript is.
    async fn give_up_on_the_harness(&mut self, now: Instant) {
        let error = if let Some(since) = self.retrying_since {
            failure::gave_up_trying(
                self.mapper.retries(),
                now.saturating_duration_since(since).as_secs(),
            )
        } else {
            failure::gave_up_waiting(now.saturating_duration_since(self.heard_at).as_secs())
        };
        tracing::warn!(code = %error.code, "the harness is stopped and its turn ended: {:?}", error.said);
        if let Some(mut process) = self.process.take() {
            process.kill();
        }
        self.live = None;
        self.retrying_since = None;
        let events = self.mapper.fail_turn_with(error);
        self.emit_all(events).await;
        self.on_turn_over().await;
    }

    /// The turn is over: whatever was still waiting on a person or on the turn is let go.
    async fn on_turn_over(&mut self) {
        self.turn = None;
        let open: Vec<_> = self.asks.drain().collect();
        for (request_ref, ask) in open {
            let _ = self
                .write(&control_success(
                    &ask.control_id,
                    ask.refusal("the turn ended"),
                ))
                .await;
            self.emit(SessionEvent::ApprovalClosed(ApprovalClosed { request_ref }))
                .await;
        }
        // A message added too late to be taken in would start a turn of the harness's own: it is taken back.
        let late: Vec<String> = self.steers.keys().cloned().collect();
        for uuid in late {
            if let Some(steer) = self.steers.remove(&uuid) {
                let id = self.next_request_id();
                let _ = self
                    .write(&control_request(
                        &id,
                        json!({ "subtype": "cancel_async_message", "message_uuid": uuid }),
                    ))
                    .await;
                if let Some(reply) = steer.reply {
                    let _ = reply.send(Err(AdapterError::NotFound(
                        "the turn ended before the message was taken in".to_owned(),
                    )));
                }
            }
        }
        self.idle_since = Some(Instant::now());
    }

    async fn on_exit(&mut self, generation: u64, info: &ExitInfo) {
        let expected = self.retiring.remove(&generation);
        if generation != self.generation {
            return;
        }
        self.last_exit = Some(info.describe());
        // Whatever waited for an answer from the process will not get one.
        self.requests.clear();
        for (_, ask) in self.context_asks.drain() {
            if let Some(reply) = ask.reply {
                let _ = reply.send(None);
            }
        }
        let had_turn = self.turn.is_some();
        let was_live = self.live.is_some();
        self.lose_session(&info.describe()).await;
        if !expected && !had_turn && was_live {
            self.emit(SessionEvent::SessionLost(SessionLost {
                reason: format!("the harness process ended: {}", info.describe()),
            }))
            .await;
        }
    }

    /// The process is gone and took the turn with it.
    async fn lose_session(&mut self, why: &str) {
        self.process = None;
        self.live = None;
        let open: Vec<_> = self.asks.drain().collect();
        for (request_ref, _) in open {
            self.emit(SessionEvent::ApprovalClosed(ApprovalClosed { request_ref }))
                .await;
        }
        for (_, steer) in self.steers.drain() {
            if let Some(reply) = steer.reply {
                let _ = reply.send(Err(AdapterError::SessionNotFound(why.to_owned())));
            }
        }
        if self.turn.take().is_some() {
            self.mapper.abort_requests(turn_error::SESSION_LOST, why);
            let events = self.mapper.abandon_turn();
            self.emit_all(events).await;
            self.emit(SessionEvent::SessionLost(SessionLost {
                reason: format!("the harness process ended: {why}"),
            }))
            .await;
        }
    }
}

/// Whether a frame says the harness is going to try again because the provider answered 401.
fn is_unauthorized_retry(frame: &Frame) -> bool {
    frame.kind() == FrameKind::System
        && frame.text("subtype") == Some("api_retry")
        && frame.get("error_status").and_then(Json::as_i64) == Some(401)
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending::<()>().await,
    }
}

/// The content blocks of a message: its text, then its images.
#[must_use]
pub fn message_blocks(text: &str, images: Vec<Json>) -> Vec<Json> {
    let mut blocks = vec![text_block(text)];
    blocks.extend(images);
    blocks
}
