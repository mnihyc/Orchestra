//! Codex app-server adapter; Codex-specific vocabulary stays here.
//!
//! [`CodexAdapter`] drives the Codex command line through its app-server protocol: JSON-RPC over the
//! process's standard streams. It starts the harness quiet and pinned ([`process`]), exchanges requests with
//! it ([`protocol`]), opens, resumes and forks its threads as sessions with everything stated each time
//! ([`sessions`]), reports what happens as the neutral events of Orch with each tool call both by its kind and
//! as the harness made it ([`events`]), carries its requests for approval to people and their decisions back
//! ([`approvals`]), signs it in and reads its plan and limits ([`provider`]), finds what it already has
//! ([`discovery`]), and tells a carried conversation to a new thread ([`history`]). What it declares is [`descriptor`].
//!
//! Nothing of Codex's vocabulary leaves this crate: what goes to the connector is the contract of
//! `orch-agent-contract`.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`descriptor`] | What the adapter claims of the harness it found |
//! | [`process`] | Starting the harness: the launcher, the configuration it starts with |
//! | [`protocol`] | JSON-RPC over the harness's streams |
//! | [`runtime`] | The running harness: its sessions, the reading task, the end of the process |
//! | [`sessions`] | The parameters that open a session |
//! | [`events`] | Notifications to neutral events |
//! | [`failure`] | What the harness says when a turn fails, and what Orch makes of it |
//! | [`approvals`] | Requests for approval and their answers |
//! | [`provider`] | Sign-in, plan and limits, models |
//! | [`discovery`] | Sessions and MCP servers the harness already has |
//! | [`history`] | Neutral histories told to a new thread |
//! | [`parameters`] | Effort and the parameters of a turn |

#![forbid(unsafe_code)]

pub mod approvals;
pub mod descriptor;
pub mod discovery;
pub mod events;
pub mod failure;
pub mod history;
mod input_prefix;
mod native_history;
pub mod parameters;
pub mod performance;
pub mod process;
pub mod protocol;
pub mod provider;
pub mod review;
pub mod runtime;
pub mod sessions;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink,
};
use orch_agent_contract::descriptor::{AgentDescriptor, HarnessSupport};
use orch_agent_contract::discovery::{
    McpDefinition, McpFound, McpImportRequest, McpSearch, ModelSuggestion, NativeSession,
};
use orch_agent_contract::events::{SessionEvent, SessionOpenedEvent};
use orch_agent_contract::facts::{AttachMode, HarnessState, ProviderFacts};
use orch_agent_contract::login::{LoginRequest, LoginState, LoginValues};
use orch_agent_contract::session::{
    ApprovalDecision, ProviderAccess, SessionMode, SessionOpened, SessionSpec, ThreadId, TurnId,
    TurnStart, TurnSteer,
};
use orch_types::{Clock, Id, Json, SystemClock};
use serde_json::{Map, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use crate::descriptor::KIND;
use crate::events::{Context, Mapper, MapperConfig, OutputStore};
use crate::failure::Patience;
use crate::parameters::TurnSettings;
use crate::process::{
    DEFAULT_PROGRAM, DirectLauncher, HOME_VARIABLE, LLM_TOKEN_VARIABLE, LaunchRequest, Launcher,
    MCP_TOKEN_VARIABLE, StderrTail,
};
use crate::protocol::RpcError;
use crate::runtime::{ProcessParts, RawTable, SessionRecord, Shared};
use crate::sessions::OpenContext;

/// How long the harness is given to end by itself once its input is closed, before it is ended: it has
/// sessions to put down.
pub const STOP_GRACE: Duration = Duration::from_secs(8);

/// How long an ended process is waited for after it was killed.
pub const KILL_GRACE: Duration = Duration::from_secs(5);

/// How long a one-shot command of the harness (a sign-in by token) may take.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// How much a one-shot command may print that is read: more is of no use to anybody.
const COMMAND_OUTPUT_MAX_BYTES: u64 = 64 * 1024;

/// The most turns of a thread that are asked for to find the one a fork goes through.
const TURNS_PAGE: u32 = 100;

/// The notifications the harness is asked not to send: they concern nothing the adapter reports, and each
/// costs a line to read.
const OPTED_OUT: [&str; 11] = [
    "remoteControl/status/changed",
    "app/list/updated",
    "skills/changed",
    "fuzzyFileSearch/sessionUpdated",
    "fuzzyFileSearch/sessionCompleted",
    "externalAgentConfig/import/progress",
    "externalAgentConfig/import/completed",
    "fs/changed",
    "windows/worldWritableWarning",
    "windowsSandbox/setupCompleted",
    "mcpServer/oauthLogin/completed",
];

/// Configuration provenance retained only after this adapter has launched the home successfully.
#[derive(Debug, Clone, Copy, Default)]
struct HomeFacts {
    goals_disabled_by_orch: bool,
}

impl HomeFacts {
    fn configured(managed: bool, config: &toml::Table) -> Self {
        Self {
            goals_disabled_by_orch: managed
                && config
                    .get("features")
                    .and_then(toml::Value::as_table)
                    .and_then(|features| features.get("goals"))
                    .and_then(toml::Value::as_bool)
                    == Some(false),
        }
    }
}

/// A harness that is running: what is needed to drive it, and to start it again.
struct Running {
    shared: Arc<Shared>,
    start: AgentStart,
    started: AgentStarted,
    /// The harness's home, as it reports it.
    home: Option<PathBuf>,
    /// Whether the home is Orch's own.
    managed: bool,
    /// The restrictions this adapter wrote for this running home.
    home_facts: HomeFacts,
    /// The provider the harness uses when a session names none.
    default_provider: Option<String>,
    program: String,
    env: BTreeMap<String, String>,
    overrides: Vec<String>,
    kill: Arc<Notify>,
    pump: JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.kill.notify_one();
    }
}

/// The adapter of the Codex command line.
pub struct CodexAdapter {
    launcher: Arc<dyn Launcher>,
    clock: Arc<dyn Clock>,
    patience: Patience,
    running: Option<Running>,
    /// Exact lanes observed idle when the native client unsubscribed them.
    prefix_sources: std::collections::HashSet<orch_agent_contract::history::NativePrefixSource>,
    history_inventory: Option<(Id, Arc<native_history::Inventory>)>,
}

impl std::fmt::Debug for CodexAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAdapter")
            .field("running", &self.running.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn map_rpc(error: RpcError) -> AdapterError {
    match error {
        RpcError::Closed(reason) => AdapterError::HarnessUnavailable(reason),
        RpcError::Timeout { .. } => AdapterError::Timeout,
        RpcError::Remote { message, .. } => {
            let lower = message.to_lowercase();
            if (lower.contains("thread") && lower.contains("not found"))
                || lower.contains("no rollout found")
            {
                AdapterError::SessionNotFound(message)
            } else if lower.contains("authentication required") {
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

fn invalid(message: &str) -> AdapterError {
    AdapterError::Other {
        code: "invalid_input".to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}

fn conflict(message: &str) -> AdapterError {
    AdapterError::Other {
        code: "conflict".to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}

/// What a native import is answered: a Codex thread's history is the harness's own and is never carried as a file.
pub(crate) fn native_archive_refused() -> AdapterError {
    AdapterError::Unsupported(
        "A Codex thread's own history stays in its home: it resumes and forks there, and it moves with the home. Into another home it travels as the newest turns, with the whole conversation as a file."
            .to_owned(),
    )
}

fn validate_session_open(spec: &SessionSpec) -> Result<(), AdapterError> {
    if spec.settings.mode == SessionMode::ImportNative {
        return Err(native_archive_refused());
    }
    if let Some(reading) = spec.settings.side_reading {
        return Err(AdapterError::Unsupported(format!(
            "Codex has no declared side-conversation mode for {reading:?}."
        )));
    }
    Ok(())
}

/// A stable identifier for a turn of the harness that Orch never asked for, derived from the harness's id,
/// so that a history read twice names its turns the same.
fn stable_turn_id(native: &str) -> TurnId {
    let hash = orch_types::Hash::of(native.as_bytes()).to_string();
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = hash
            .get(index * 2..index * 2 + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .unwrap_or_default();
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    TurnId::from_bytes(bytes)
}

impl CodexAdapter {
    /// An adapter that runs the harness as a program of the machine.
    #[must_use]
    pub fn new() -> Self {
        Self::with_launcher(Arc::new(DirectLauncher))
    }

    /// An adapter that starts the harness through `launcher`.
    #[must_use]
    pub fn with_launcher(launcher: Arc<dyn Launcher>) -> Self {
        Self {
            launcher,
            clock: Arc::new(SystemClock),
            patience: Patience::default(),
            running: None,
            prefix_sources: Default::default(),
            history_inventory: None,
        }
    }

    /// The same adapter waiting for a harness as long as `patience` says.
    #[must_use]
    pub fn with_patience(mut self, patience: Patience) -> Self {
        self.patience = patience;
        self
    }

    /// The same adapter reading time from `clock`.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    fn live(&self) -> Result<(Arc<Shared>, &Running), AdapterError> {
        let running = self.running.as_ref().ok_or_else(|| {
            AdapterError::HarnessUnavailable("the harness is not started".to_owned())
        })?;
        if running.shared.is_dead() {
            return Err(AdapterError::HarnessUnavailable(
                "the harness process has ended; start it again".to_owned(),
            ));
        }
        Ok((Arc::clone(&running.shared), running))
    }

    // -- starting and stopping -----------------------------------------------------------------------

    #[allow(
        clippy::too_many_lines,
        reason = "starting the harness is one sequence, each step of which depends on the one before"
    )]
    async fn launch(&self, start: AgentStart, sink: EventSink) -> Result<Running, AdapterError> {
        if start.profile.attach_mode() == AttachMode::Daemon {
            return Err(AdapterError::Unsupported(
                "attaching to a daemon: this adapter starts the harness itself and talks to it over its standard streams"
                    .to_owned(),
            ));
        }
        let managed = start.home.is_some()
            && !start
                .profile
                .attach
                .as_ref()
                .and_then(|attach| attach.home.as_ref())
                .is_some_and(|home| home.directory().is_some());
        let own_home = start
            .home
            .clone()
            .or_else(|| {
                start
                    .profile
                    .attach
                    .as_ref()
                    .and_then(|attach| attach.home.as_ref())
                    .and_then(|home| home.directory().map(str::to_owned))
            })
            .map(PathBuf::from);
        let initialize_timeout = provider::initialize_timeout(&start.provider_config)?;
        let options = process::home_options(&start.provider_config, managed);
        let config = process::effective_config(&options, &start.profile.config);
        let mut overrides = Vec::new();
        if managed {
            if let Some(home) = &own_home {
                process::prepare_home(home).map_err(|error| {
                    AdapterError::HarnessUnavailable(format!(
                        "the home of the harness could not be made: {error}"
                    ))
                })?;
                process::write_home_config(home, &config).map_err(|error| {
                    AdapterError::HarnessUnavailable(format!(
                        "the configuration could not be written: {error}"
                    ))
                })?;
                process::write_daemon_settings(home).map_err(|error| {
                    AdapterError::HarnessUnavailable(format!(
                        "the daemon update preference could not be prepared: {error}"
                    ))
                })?;
            }
        } else {
            overrides = process::overrides(&config);
        }
        let program = start
            .binary
            .clone()
            .or_else(|| {
                start
                    .profile
                    .attach
                    .as_ref()
                    .and_then(|attach| attach.binary.clone())
            })
            .unwrap_or_else(|| DEFAULT_PROGRAM.to_owned());
        let mut env: BTreeMap<String, String> = start.profile.env.clone();
        if let Some(home) = &own_home {
            env.insert(
                HOME_VARIABLE.to_owned(),
                home.to_string_lossy().into_owned(),
            );
        }
        let token = start.loopback.token.expose().to_owned();
        env.insert(LLM_TOKEN_VARIABLE.to_owned(), token.clone());
        env.insert(MCP_TOKEN_VARIABLE.to_owned(), token);

        let spawned = self
            .launcher
            .launch(LaunchRequest {
                program: program.clone(),
                args: process::app_server_args(&overrides),
                env: env.clone(),
                cwd: None,
            })
            .await
            .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;

        let raw: RawTable = Arc::default();
        let tap = runtime::raw_tap(sink.clone(), Arc::clone(&raw));
        let (rpc, incoming) = protocol::connect(spawned.stdout, spawned.stdin, Some(tap));
        let tail = StderrTail::default();
        let stderr_task = spawned
            .stderr
            .map(|stderr| tokio::spawn(tail.clone().follow(stderr)));
        let (exit_sender, mut exit_watch) = watch::channel(None::<String>);
        let exit = Arc::new(exit_sender);
        let kill = Arc::new(Notify::new());
        {
            let exit = Arc::clone(&exit);
            let kill = Arc::clone(&kill);
            let tail = tail.clone();
            let listener = sink.clone();
            let controlled_rpc = rpc.clone();
            let mut control = spawned.control;
            tokio::spawn(async move {
                let ended = tokio::select! {
                    ended = control.wait() => ended,
                    () = kill.notified() => {
                        control.kill();
                        control.wait().await
                    }
                    () = listener.closed() => {
                        control.kill();
                        control.wait().await
                    }
                    () = controlled_rpc.closed() => {
                        control.kill();
                        control.wait().await
                    }
                };
                // A final diagnostic can arrive with the exit. A descendant holding stderr cannot hide it.
                if let Some(mut task) = stderr_task
                    && tokio::time::timeout(runtime::EXIT_GRACE / 2, &mut task)
                        .await
                        .is_err()
                {
                    task.abort();
                }
                let said = tail.text();
                let transport = controlled_rpc.closed_reason();
                let reason = if said.is_empty() {
                    format!("the harness {ended}")
                } else {
                    format!("the harness {ended}: {said}")
                };
                let reason =
                    transport.map_or_else(|| reason.clone(), |cause| format!("{reason}: {cause}"));
                controlled_rpc.process_exited(&reason);
                exit.send_replace(Some(reason));
            });
        }
        let failed = |reason: String, said: &StderrTail| {
            let said = said.text();
            AdapterError::HarnessUnavailable(if said.is_empty() || reason.ends_with(&said) {
                reason
            } else {
                format!("{reason}: {said}")
            })
        };
        let answered = rpc
            .call_within(
                "initialize",
                json!({
                    "clientInfo": { "name": "orch", "title": "Orch", "version": env!("CARGO_PKG_VERSION") },
                    "capabilities": {
                        "experimentalApi": true,
                        "requestAttestation": false,
                        // Data forms still need a person in bypass mode; the native client requires this opt-in.
                        "extensions": { "openai/standard-form-input": {} },
                        "optOutNotificationMethods": OPTED_OUT,
                    }
                }),
                initialize_timeout,
            )
            .await;
        let answered = match answered {
            Ok(answer) => answer,
            Err(error) => {
                let failure = error.to_string();
                kill.notify_one();
                let reason = tokio::time::timeout(runtime::EXIT_GRACE, async {
                    loop {
                        if let Some(message) = exit_watch.borrow_and_update().clone() {
                            return message;
                        }
                        if exit_watch.changed().await.is_err() {
                            return failure.clone();
                        }
                    }
                })
                .await
                .unwrap_or_else(|_| failure.clone());
                let reason = if reason == failure {
                    failure
                } else {
                    format!("{failure}; {reason}")
                };
                return Err(failed(reason, &tail));
            }
        };
        let user_agent = answered
            .get("userAgent")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let Some(version) = descriptor::version_from_user_agent(user_agent) else {
            kill.notify_one();
            return Err(AdapterError::HarnessUnavailable(format!(
                "the harness did not state its version ({user_agent:?})"
            )));
        };
        let mut declared = descriptor::descriptor(&version);
        // The harness's no-prompt mode is never declared to a root process.
        if std::fs::read_to_string("/proc/self/status")
            .ok()
            .is_some_and(|status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("Uid:"))
                    .is_some_and(|uids| uids.split_whitespace().nth(1) == Some("0"))
            })
        {
            declared
                .approval_modes
                .retain(|mode| *mode != orch_agent_contract::descriptor::ApprovalMode::Bypass);
        }
        if declared.harness.status == HarnessSupport::Unsupported {
            kill.notify_one();
            return Err(AdapterError::HarnessUnavailable(format!(
                "Codex {version} is older than the range {} this adapter drives",
                descriptor::SUPPORTED_RANGE
            )));
        }
        rpc.notify("initialized", None)
            .await
            .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
        let harness_home = answered
            .get("codexHome")
            .and_then(Json::as_str)
            .map(PathBuf::from);
        let outputs_home = own_home
            .clone()
            .or_else(|| harness_home.clone())
            .unwrap_or_else(|| std::env::temp_dir().join("orch-codex"));
        let default_provider = rpc
            .call("config/read", json!({ "includeLayers": false }))
            .await
            .ok()
            .and_then(|read| {
                read.get("config")
                    .and_then(|config| config.get("model_provider"))
                    .and_then(Json::as_str)
                    .map(str::to_owned)
            });
        let shared = Shared::new(ProcessParts {
            sink: sink.clone(),
            clock: Arc::clone(&self.clock),
            outputs: OutputStore::new(outputs_home.join("outputs")),
            rpc,
            raw,
            version: version.clone(),
            exit: Arc::clone(&exit),
            patience: self.patience,
        });
        if let Some(home) = harness_home.as_deref().filter(|_| managed) {
            let (method, expires_at) = provider::read_login_note(home);
            shared.login.noted(method, expires_at);
            shared.login.issued(provider::read_issued_at(home));
        }
        let pump = tokio::spawn(runtime::pump(Arc::clone(&shared), incoming));
        let _ = sink
            .emit(AdapterEvent::Status(
                shared.status(HarnessState::Online, None),
            ))
            .await;
        Ok(Running {
            shared,
            started: AgentStarted {
                harness_version: version,
                descriptor: declared,
            },
            start,
            home: harness_home,
            managed,
            home_facts: HomeFacts::configured(managed, &config),
            default_provider,
            program,
            env,
            overrides,
            kill,
            pump,
        })
    }

    /// Ends the process, if one runs, and waits for it.
    async fn teardown(&mut self) {
        self.prefix_sources.clear();
        self.history_inventory = None;
        let Some(running) = self.running.take() else {
            return;
        };
        running.shared.stopping();
        running.shared.rpc().finish();
        if running.shared.exit_message(STOP_GRACE).await.is_none() {
            running.kill.notify_one();
            let _ = running.shared.exit_message(KILL_GRACE).await;
        }
        let _ = tokio::time::timeout(KILL_GRACE, async {
            let mut running = running;
            let pump = std::mem::replace(&mut running.pump, tokio::spawn(async {}));
            let _ = pump.await;
        })
        .await;
    }

    /// Starts the harness again as it was started, so that it reads what was written to its home.
    async fn restart(&mut self) -> Result<(), AdapterError> {
        let Some(running) = &self.running else {
            return Err(AdapterError::HarnessUnavailable(
                "the harness is not started".to_owned(),
            ));
        };
        let start = running.start.clone();
        let sink = running.shared.sink().clone();
        self.teardown().await;
        self.running = Some(self.launch(start, sink).await?);
        Ok(())
    }

    // -- sessions ------------------------------------------------------------------------------------

    /// The turn of a thread the `ordinal`-th counting from the first, by the harness's id.
    async fn nth_turn(shared: &Shared, native: &str, ordinal: u32) -> Result<String, AdapterError> {
        let mut seen = 0u32;
        let mut cursor: Option<String> = None;
        loop {
            let page = shared
                .rpc()
                .call(
                    "thread/turns/list",
                    json!({ "threadId": native, "sortDirection": "asc", "limit": TURNS_PAGE, "cursor": cursor,
                            "itemsView": "notLoaded" }),
                )
                .await
                .map_err(map_rpc)?;
            for turn in page
                .get("data")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                seen += 1;
                if seen == ordinal {
                    return turn
                        .get("id")
                        .and_then(Json::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            AdapterError::HarnessError("a turn of the thread has no id".to_owned())
                        });
                }
            }
            cursor = page
                .get("nextCursor")
                .and_then(Json::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                return Err(AdapterError::NotFound(format!(
                    "the thread has {seen} turns, and the history was asked for through turn {ordinal}"
                )));
            }
        }
    }

    /// Tells a thread the turns of a neutral history, as Responses items appended to what the model is told, a page of
    /// them at a time ([`history::INJECT_PAGE_BYTES`]), oldest first. Only the turn being converted and the page being
    /// filled are held.
    async fn tell_history(
        rpc: &protocol::Rpc,
        native: &str,
        path: &std::path::Path,
        through: Option<u32>,
    ) -> Result<(), AdapterError> {
        let mut turns = history::NeutralTurns::open(path, through)?;
        let mut page: Vec<Json> = Vec::new();
        let mut bytes = 0usize;
        while let Some(items) = turns.next_turn()? {
            for item in items {
                bytes = bytes.saturating_add(item.to_string().len());
                page.push(item);
                if bytes >= history::INJECT_PAGE_BYTES {
                    Self::inject(rpc, native, std::mem::take(&mut page)).await?;
                    bytes = 0;
                }
            }
        }
        if !page.is_empty() {
            Self::inject(rpc, native, page).await?;
        }
        Ok(())
    }

    async fn inject(
        rpc: &protocol::Rpc,
        native: &str,
        items: Vec<Json>,
    ) -> Result<(), AdapterError> {
        rpc.call(
            "thread/inject_items",
            json!({ "threadId": native, "items": items }),
        )
        .await
        .map_err(map_rpc)?;
        Ok(())
    }

    async fn open(
        &mut self,
        spec: SessionSpec,
        prefix: Option<&input_prefix::Checkpoint>,
        observing: Option<&orch_agent_contract::history::NativePrefixSource>,
    ) -> Result<SessionOpened, AdapterError> {
        if spec.settings.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Auto
            && !descriptor::auto_proven()
        {
            return Err(AdapterError::Unsupported("Auto is unavailable until its reviewer model has passed the integrator's real trial".to_owned()));
        }
        let approval_reviewer = crate::review::reviewer(&spec.settings)?;
        // The workspace's own configuration, looked for where the harness would read it: not on a machine the
        // tools run on elsewhere, whose files here are not the project's.
        let look = if spec.exec.executor.is_none() {
            sessions::validate_instruction_files(
                &spec.settings.workspace_root,
                &spec.settings.cwd,
                spec.settings.instruction_root_marker,
                spec.settings.workspace_configuration.as_ref(),
            )?
        } else {
            sessions::WorkspaceLook::default()
        };
        let (shared, running) = self.live()?;
        let rpc = shared.rpc().clone();
        let mode = spec.settings.mode;
        if matches!(mode, SessionMode::Resume | SessionMode::Adopt)
            && let Some(native) = spec.settings.native_session_ref.as_deref()
        {
            if shared
                .with_session(spec.settings.thread_id, |record| {
                    record.mapper.turn_in_progress()
                })
                .unwrap_or(false)
            {
                return Err(conflict(
                    "a running turn keeps its session configuration; resume after it ends",
                ));
            }
            shared.unload_before_resume(native).await?;
        }
        if matches!(spec.provider, ProviderAccess::Native { .. })
            && provider::auth_mode(&rpc).await?.is_none()
        {
            return Err(AdapterError::LoginRequired);
        }
        let mut report = None;
        let mut fork_through = None;
        let mut neutral_history: Option<(PathBuf, Option<u32>)> = None;
        match mode {
            SessionMode::Fork => {
                if let (Some(ordinal), Some(source)) = (
                    spec.settings.upto_turn_ordinal,
                    spec.settings.native_session_ref.as_deref(),
                ) {
                    fork_through = Some(Self::nth_turn(&shared, source, ordinal).await?);
                }
            }
            SessionMode::ImportNative => return Err(native_archive_refused()),
            SessionMode::ImportNeutral => {
                let file = spec
                    .history
                    .as_ref()
                    .ok_or_else(|| invalid("a neutral import names the history to import"))?;
                // The file is read a turn at a time, twice: first before a thread is opened for it, so that a
                // history that cannot be read opens nothing and the report is known; then to tell the new
                // thread. Only one turn is held at once, whatever the length of the file.
                let path = PathBuf::from(&file.path);
                let through = spec.settings.upto_turn_ordinal;
                let mut turns = history::NeutralTurns::open(&path, through)?;
                while turns.next_turn()?.is_some() {}
                report = Some(turns.finish());
                neutral_history = Some((path, through));
            }
            SessionMode::New | SessionMode::Resume | SessionMode::Adopt => {}
        }
        // A session is held to the servers it is given: the others the harness would load are switched off.
        let place = (spec.exec.executor.is_none())
            .then_some((spec.settings.directory.as_str(), spec.settings.cwd.as_str()));
        // A configuration of the workspace's own that its environment's managers trusted keeps its servers.
        let context = OpenContext {
            switched_off: discovery::servers_to_switch_off(&rpc, place, !look.trusted).await?,
            default_provider: running.default_provider.clone(),
            fork_through,
            projects: look.projects,
        };
        let mut plan = sessions::open_plan(&spec, &running.start.loopback, &context)?;
        if let Some(source) = observing {
            if plan.method != "thread/resume"
                || spec.settings.thread_id != source.thread_id
                || spec.settings.native_session_ref.as_deref()
                    != Some(source.native_session_ref.as_str())
                || spec.settings.upto_turn_ordinal.is_some()
            {
                return Err(invalid(
                    "A continuation resume names its exact retained lane without an ordinal cut.",
                ));
            }
            let params = plan
                .params
                .as_object_mut()
                .ok_or_else(|| invalid("The native resume parameters are invalid."))?;
            params.insert("deferGoalContinuation".into(), json!(true));
            params.insert("excludeTurns".into(), json!(true));
        }
        if let Some(remote) = &plan.remote {
            rpc.call("environment/add", remote.add_params())
                .await
                .map_err(map_rpc)?;
            remote.ensure_ready(&rpc).await?;
        }
        if let Some(checkpoint) = prefix {
            let current = self
                .resolve_checkpoint(
                    checkpoint.prefix.source.clone(),
                    &checkpoint.prefix.before_input_ref,
                )
                .await?;
            if current != *checkpoint {
                return Err(conflict(
                    "The native checkpoint changed before opening its new session.",
                ));
            }
            let params = plan
                .params
                .as_object_mut()
                .ok_or_else(|| invalid("The native prefix open parameters are invalid."))?;
            if !checkpoint.prefix.empty {
                if plan.method != "thread/fork"
                    || params.contains_key("path")
                    || params.contains_key("lastTurnId")
                {
                    return Err(invalid(
                        "An input checkpoint uses only its exact native before-turn boundary.",
                    ));
                }
                params.insert("beforeTurnId".into(), json!(checkpoint.turn));
                params.insert("deferGoalContinuation".into(), json!(true));
                params.insert("excludeTurns".into(), json!(true));
            } else if plan.method != "thread/start" {
                return Err(invalid(
                    "An empty input checkpoint opens a new native session.",
                ));
            }
        }
        let opened = rpc.call(plan.method, plan.params).await.map_err(map_rpc)?;
        let thread = opened.get("thread").unwrap_or(&Json::Null);
        let native = thread
            .get("id")
            .and_then(Json::as_str)
            .ok_or_else(|| {
                AdapterError::HarnessError(
                    "the harness opened a thread without naming it".to_owned(),
                )
            })?
            .to_owned();
        if prefix.is_some_and(|checkpoint| checkpoint.prefix.source.native_session_ref == native) {
            return Err(conflict(
                "The harness reused the source identity instead of opening a fresh prefix.",
            ));
        }
        if observing.is_some_and(|source| source.native_session_ref != native) {
            return Err(conflict(
                "The harness resumed a different native continuation identity.",
            ));
        }
        if let Some(checkpoint) = prefix {
            if native.is_empty()
                || (!checkpoint.prefix.empty
                    && thread["forkedFromId"] != checkpoint.prefix.source.native_session_ref)
            {
                return Err(conflict(
                    "The harness returned a checkpoint child with another native owner.",
                ));
            }
            if checkpoint.paginated && thread["historyMode"] != "paginated" {
                return Err(conflict(
                    "The native checkpoint did not preserve paginated history.",
                ));
            }
        }
        if let Some((path, through)) = neutral_history {
            Self::tell_history(&rpc, &native, &path, through).await?;
        }
        let resumed = matches!(mode, SessionMode::Resume | SessionMode::Adopt);
        let settings = &spec.settings;
        let record = SessionRecord {
            mapper: Mapper::new(MapperConfig {
                thread_id: settings.thread_id,
                segment_id: settings.segment_id,
                native_thread: native.clone(),
                directory: settings.directory.clone(),
            }),
            directory: settings.directory.clone(),
            cwd: settings.cwd.clone(),
            workspace_root: settings.workspace_root.clone(),
            instruction_root_marker: settings.instruction_root_marker,
            workspace_configuration: settings.workspace_configuration.clone(),
            instructions_notice: resumed.then(|| {
                format!(
                    "orch-instructions-next-session:{}",
                    orch_types::Hash::of(settings.instructions.as_bytes())
                )
            }),
            mounts: settings.mounts.clone(),
            remote: plan.remote,
            approvals: HashMap::new(),
            planning: false,
            approval_mode: settings.approval_mode,
        };
        shared
            .register(
                record,
                settings.raw_events,
                SessionEvent::SessionOpened(SessionOpenedEvent {
                    native_session_ref: native.clone(),
                    resumed,
                }),
            )
            .await;
        // The instruction files the harness reported it reads, in its order.
        let sources = sessions::reported_instruction_sources(
            &opened,
            running.home.as_deref(),
            &settings.workspace_root,
            spec.exec.executor.is_none(),
        );
        let opened = SessionOpened {
            native_session_ref: native,
            resumed,
            import_report: report,
            instruction_sources: sources,
            approval_reviewer,
        };
        if let Some((pass, inventory)) = self.history_inventory.take()
            && let Ok(inventory) = Arc::try_unwrap(inventory)
        {
            let observed = json!({"id":thread["id"],"path":thread["path"],"status":thread["status"],"parentThreadId":thread["parentThreadId"],"forkedFromId":thread["forkedFromId"]});
            match tokio::task::spawn_blocking(move || {
                inventory
                    .observe(&observed)
                    .map(|()| (pass, Arc::new(inventory)))
            })
            .await
            {
                Ok(Ok(updated)) => self.history_inventory = Some(updated),
                Ok(Err(error)) => {
                    tracing::warn!(%error,"native history metadata could not be refreshed")
                }
                Err(error) => tracing::warn!(%error,"native history metadata worker ended"),
            }
        }
        Ok(opened)
    }

    /// Reads the history of a thread from the harness and reports it as events of the session.
    async fn replay(&self, thread: ThreadId) -> Result<u32, AdapterError> {
        let (shared, _) = self.live()?;
        let (native, config) = shared.with_session(thread, |record| {
            (
                record.mapper.native_thread().to_owned(),
                MapperConfig {
                    thread_id: record.mapper.thread_id(),
                    segment_id: record.mapper.segment_id(),
                    native_thread: record.mapper.native_thread().to_owned(),
                    directory: record.directory.clone(),
                },
            )
        })?;
        // A page of turns at a time, oldest first: the events of a page are reported before the next is read, so that
        // a history of any length is held no more than a page at once.
        let outputs = shared.outputs().clone();
        let ctx = Context {
            clock: shared.clock().as_ref(),
            outputs: &outputs,
            windows: None,
        };
        let mut replay = events::Replay::new(config);
        let mut items = 0u32;
        let mut cursor: Option<String> = None;
        loop {
            let page = shared
                .rpc()
                .call(
                    "thread/turns/list",
                    json!({ "threadId": native, "sortDirection": "asc", "limit": TURNS_PAGE, "cursor": cursor,
                            "itemsView": "full" }),
                )
                .await
                .map_err(map_rpc)?;
            let turns = page
                .get("data")
                .and_then(Json::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            for event in replay.page(turns, &ctx, stable_turn_id) {
                if matches!(event, SessionEvent::ItemCompleted(_)) {
                    items += 1;
                }
                shared.report(thread, event).await;
            }
            cursor = page
                .get("nextCursor")
                .and_then(Json::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(items)
    }

    /// The text a one-shot command printed, bounded.
    async fn run_command(
        &self,
        running: &Running,
        args: Vec<String>,
        stdin: &str,
    ) -> Result<(Option<i32>, String), AdapterError> {
        let mut spawned = self
            .launcher
            .launch(LaunchRequest {
                program: running.program.clone(),
                args,
                env: running.env.clone(),
                cwd: None,
            })
            .await
            .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
        let work = async {
            let _ = spawned.stdin.write_all(stdin.as_bytes()).await;
            let _ = spawned.stdin.shutdown().await;
            drop(std::mem::replace(
                &mut spawned.stdin,
                Box::new(tokio::io::sink()),
            ));
            let mut printed = String::new();
            let mut out = (&mut spawned.stdout).take(COMMAND_OUTPUT_MAX_BYTES);
            let mut bytes = Vec::new();
            let _ = out.read_to_end(&mut bytes).await;
            printed.push_str(&String::from_utf8_lossy(&bytes));
            if let Some(stderr) = spawned.stderr.as_mut() {
                let mut err = stderr.take(COMMAND_OUTPUT_MAX_BYTES);
                let mut bytes = Vec::new();
                let _ = err.read_to_end(&mut bytes).await;
                printed.push('\n');
                printed.push_str(&String::from_utf8_lossy(&bytes));
            }
            let ended = spawned.control.wait().await;
            (ended.code, printed)
        };
        tokio::time::timeout(COMMAND_TIMEOUT, work)
            .await
            .map_err(|_| AdapterError::Timeout)
    }

    async fn after_import(&mut self, method: &str) -> Result<(), AdapterError> {
        let sink = self
            .running
            .as_ref()
            .map(|running| running.shared.sink().clone());
        self.restart().await?;
        let (shared, _) = self.live()?;
        let signed_in = provider::auth_mode(shared.rpc()).await?.is_some();
        if let Some(sink) = sink {
            let state = if signed_in {
                LoginState::completed(method)
            } else {
                LoginState::failed(
                    method,
                    "the harness does not accept the sign-in: it may belong to another workspace than the provider's, or no longer be valid",
                )
            };
            let _ = sink.emit(AdapterEvent::Login(state)).await;
        }
        if signed_in {
            Ok(())
        } else {
            Err(AdapterError::Other {
                code: "login_refused".to_owned(),
                message: "the harness signed itself out: the sign-in is not accepted here"
                    .to_owned(),
                retryable: false,
            })
        }
    }

    fn managed_home(&self) -> Result<(&Running, PathBuf), AdapterError> {
        let running = self.running.as_ref().ok_or_else(|| {
            AdapterError::HarnessUnavailable("the harness is not started".to_owned())
        })?;
        match (&running.home, running.managed) {
            (Some(home), true) => Ok((running, home.clone())),
            _ => Err(AdapterError::Other {
                code: "login_on_the_machine".to_owned(),
                message: "this harness uses the sign-in of its machine; signing in and out is done there, with the harness itself".to_owned(),
                retryable: false,
            }),
        }
    }
}

fn object(members: Vec<(&str, Json)>) -> Map<String, Json> {
    members
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

#[async_trait]
impl AgentAdapter for CodexAdapter {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn native_check(&self) -> orch_agent_contract::conformance::NativeCheck {
        use orch_agent_contract::conformance::{ModelPaths, NativeCheck};
        NativeCheck {
            // A check's session runs with permission asked for nothing: no case here expects a prompt unasked.
            approval_mode: orch_agent_contract::descriptor::ApprovalMode::Bypass,
            profile_env: BTreeMap::new(),
            model_paths: ModelPaths {
                reached: vec!["/responses".to_owned()],
                allowed: vec!["/responses".to_owned(), "/models".to_owned()],
                exact: Vec::new(),
            },
        }
    }

    fn mcp_tool_name(&self, server: &str, tool: &str) -> String {
        // The Responses protocol advertises a namespace and a tool in it.
        format!("mcp__{server}.{tool}")
    }

    fn verify_quiet_start(
        &self,
        evidence: &orch_agent_contract::conformance::QuietEvidence<'_>,
    ) -> Result<(), String> {
        let home = evidence
            .home
            .ok_or_else(|| "no disposable native home".to_owned())?;
        let config: toml::Value = toml::from_str(
            &std::fs::read_to_string(home.join("config.toml"))
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        for path in [["analytics", "enabled"], ["feedback", "enabled"]] {
            if config
                .get(path[0])
                .and_then(|value| value.get(path[1]))
                .and_then(toml::Value::as_bool)
                != Some(false)
            {
                return Err(format!("{} traffic was not disabled", path[0]));
            }
        }
        if config
            .get("check_for_update_on_startup")
            .and_then(toml::Value::as_bool)
            != Some(false)
        {
            return Err("native update traffic was not disabled".to_owned());
        }
        for key in ["exporter", "metrics_exporter", "trace_exporter"] {
            if config
                .get("otel")
                .and_then(|value| value.get(key))
                .and_then(toml::Value::as_str)
                != Some("none")
            {
                return Err(format!("native {key} was not disabled"));
            }
        }
        Ok(())
    }

    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        if let Some(running) = &self.running
            && !running.shared.is_dead()
            && running.start == start
        {
            return Ok(running.started.clone());
        }
        self.teardown().await;
        let running = self.launch(start, sink).await?;
        let started = running.started.clone();
        self.running = Some(running);
        Ok(started)
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.teardown().await;
        Ok(())
    }

    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        let (_, running) = self.live()?;
        Ok(running.started.descriptor.clone())
    }

    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        validate_session_open(&spec)?;
        let mode = spec.settings.mode;
        let thread = spec.settings.thread_id;
        let opened = self.open(spec, None, None).await?;
        self.prefix_sources.retain(|source| {
            source.thread_id != thread && source.native_session_ref != opened.native_session_ref
        });
        if mode == SessionMode::Adopt {
            // The conversation the harness already had is shown on the thread's timeline.
            if let Err(error) = self.replay(thread).await {
                tracing::warn!(%error, "the history of an adopted session could not be read");
            }
        }
        Ok(opened)
    }

    async fn inspect_input_prefix(
        &mut self,
        source: orch_agent_contract::history::NativePrefixSource,
        before_input_ref: &str,
    ) -> Result<orch_agent_contract::history::NativeInputPrefix, AdapterError> {
        self.inspect_prefix(source, before_input_ref).await
    }

    async fn open_input_prefix(
        &mut self,
        spec: SessionSpec,
        prefix: orch_agent_contract::history::NativeInputPrefix,
    ) -> Result<orch_agent_contract::history::NativePrefixOpened, AdapterError> {
        self.open_prefix(spec, prefix).await
    }

    async fn resume_continuation(
        &mut self,
        spec: SessionSpec,
        source: orch_agent_contract::history::NativePrefixSource,
        replace_segment_id: Option<orch_types::Id>,
    ) -> Result<SessionOpened, AdapterError> {
        validate_session_open(&spec)?;
        if spec.settings.mode != SessionMode::Resume {
            return Err(invalid(
                "A continuation resume keeps its retained native session.",
            ));
        }
        let (shared, running) = self.live()?;
        if running.started.harness_version != "0.165.0" {
            return Err(AdapterError::Unsupported(
                "Exact continuation resume requires Codex 0.165.0.".into(),
            ));
        }
        if let Ok((native, segment, busy)) = shared.with_session(source.thread_id, |record| {
            (
                record.mapper.native_thread().to_owned(),
                record.mapper.segment_id(),
                record.mapper.turn_in_progress(),
            )
        }) && (busy
            || !(segment == source.segment_id && native == source.native_session_ref
                || replace_segment_id == Some(segment)))
        {
            return Err(conflict(
                "Another or still-running native generation occupies this continuation lane.",
            ));
        }
        self.open(spec, None, Some(&source)).await
    }

    async fn manage_history(
        &mut self,
        sources: Vec<orch_agent_contract::history::NativeHistorySource>,
        remove: bool,
    ) -> Result<Vec<orch_agent_contract::history::NativeHistoryFact>, AdapterError> {
        if sources.is_empty() {
            self.history_inventory = None;
            return Ok(Vec::new());
        }
        let (shared, _) = self.live()?;
        let (running, home) = self.managed_home()?;
        let _ = running;
        let pass = sources.first().and_then(|source| source.inventory_pass);
        if sources.iter().any(|source| source.inventory_pass != pass) {
            return Err(invalid(
                "One native history batch belongs to one inventory pass.",
            ));
        }
        let cached = self
            .history_inventory
            .as_ref()
            .filter(|(id, _)| Some(*id) == pass)
            .map(|(_, inventory)| Arc::clone(inventory));
        let inventory = match if let Some(inventory) = cached {
            Ok(inventory)
        } else {
            native_history::inventory(shared.rpc()).await.map(Arc::new)
        } {
            Ok(inventory) => inventory,
            Err(error) => {
                return Ok(sources
                    .into_iter()
                    .map(|request| orch_agent_contract::history::NativeHistoryFact {
                        source: request.source,
                        bytes: None,
                        removed: false,
                        error: Some(error.code().into()),
                    })
                    .collect());
            }
        };
        if let Some(pass) = pass {
            self.history_inventory = Some((pass, Arc::clone(&inventory)));
        }
        let mut facts = Vec::with_capacity(sources.len());
        for request in sources {
            let started_by_orch = request.started_by_orch;
            let source = request.source;
            let active = shared
                .with_session(source.thread_id, |record| {
                    (
                        record.mapper.native_thread().to_owned(),
                        record.mapper.turn_in_progress(),
                        !record.approvals.is_empty(),
                    )
                })
                .ok();
            let result = async {
                if !started_by_orch {
                    return Err(AdapterError::Other {
                        code: "history.not_owned".into(),
                        message: "Orch did not start this native history.".into(),
                        retryable: false,
                    });
                }
                if remove
                    && active.as_ref().is_some_and(|(native, busy, approvals)| {
                        native == &source.native_session_ref && (*busy || *approvals)
                    })
                {
                    Err(conflict(
                        "The native session still has running work or an approval.",
                    ))
                } else {
                    if remove
                        && active
                            .as_ref()
                            .is_some_and(|(native, _, _)| native == &source.native_session_ref)
                    {
                        self.close_session(source.thread_id).await?;
                        shared
                            .unload_before_resume(&source.native_session_ref)
                            .await?;
                    }
                    native_history::manage(
                        &home,
                        shared.rpc(),
                        Arc::clone(&inventory),
                        source.clone(),
                        remove,
                    )
                    .await
                }
            }
            .await
            .map(|mut fact| {
                // A loaded new session may not have flushed its first rollout yet.
                if !remove
                    && active
                        .as_ref()
                        .is_some_and(|(native, _, _)| native == &source.native_session_ref)
                {
                    fact.removed = false;
                }
                fact
            });
            facts.push(result.unwrap_or_else(|error| {
                orch_agent_contract::history::NativeHistoryFact {
                    source,
                    bytes: None,
                    removed: false,
                    error: Some(error.code().into()),
                }
            }));
        }
        Ok(facts)
    }

    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        let (shared, _) = self.live()?;
        let state = shared.with_session(thread, |record| {
            (
                record.mapper.running_turn(),
                record.mapper.turn_in_progress(),
            )
        });
        let Ok((running_turn, busy)) = state else {
            return Ok(());
        };
        let native =
            shared.with_session(thread, |record| record.mapper.native_thread().to_owned())?;
        let segment = shared.with_session(thread, |record| record.mapper.segment_id())?;
        let idle = !busy;
        if let Some((_, native_turn)) = running_turn {
            let _ = shared
                .rpc()
                .call(
                    "turn/interrupt",
                    json!({ "threadId": native, "turnId": native_turn }),
                )
                .await;
        }
        shared.remove(thread).await;
        let unsubscribed = shared
            .rpc()
            .call("thread/unsubscribe", json!({ "threadId": native }))
            .await;
        if idle && unsubscribed.is_ok() {
            self.prefix_sources
                .insert(orch_agent_contract::history::NativePrefixSource {
                    thread_id: thread,
                    segment_id: segment,
                    native_session_ref: native,
                });
        }
        Ok(())
    }

    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        if turn.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Auto
            && !descriptor::auto_proven()
        {
            return Err(AdapterError::Unsupported("Auto is unavailable until its reviewer model has passed the integrator's real trial".to_owned()));
        }
        let (shared, _) = self.live()?;
        shared.with_session(turn.thread_id, |record| {
            // A configuration that appeared since the session was opened, an agent's own included, is refused
            // until somebody who manages the environment has answered for it.
            if record.remote.is_none() {
                sessions::validate_instruction_files(
                    &record.workspace_root,
                    &record.cwd,
                    record.instruction_root_marker,
                    record.workspace_configuration.as_ref(),
                )?;
            }
            Ok::<_, AdapterError>(())
        })??;
        shared
            .instruction_notice(turn.thread_id, turn.turn_id)
            .await?;
        let (native, cwd, directory, mounts, remote, was_planning) =
            shared.with_session(turn.thread_id, |record| {
                if record.mapper.turn_in_progress() {
                    return Err(conflict("a turn is already running in this session"));
                }
                record.mapper.expect_turn(turn.turn_id);
                record.approval_mode = turn.approval_mode;
                let was_planning = std::mem::replace(&mut record.planning, turn.plan);
                Ok((
                    record.mapper.native_thread().to_owned(),
                    record.cwd.clone(),
                    record.directory.clone(),
                    record.mounts.clone(),
                    record.remote.clone(),
                    was_planning,
                ))
            })??;
        let abandon = |shared: &Shared| {
            let _ = shared.with_session(turn.thread_id, |record| {
                record.mapper.forget_expected_turn()
            });
        };
        if let Some(remote) = &remote
            && let Err(error) = remote.ensure_ready(shared.rpc()).await
        {
            abandon(&shared);
            return Err(error);
        }
        let mut input = vec![json!({ "type": "text", "text": turn.text, "text_elements": [] })];
        for image in &turn.images {
            let Some(path) = &image.path else {
                abandon(&shared);
                return Err(invalid(
                    "an image of a turn comes as a file the connector put on this machine",
                ));
            };
            input.push(json!({ "type": "localImage", "path": path }));
        }
        let mut params = object(vec![
            ("threadId", json!(native)),
            ("clientUserMessageId", json!(turn.turn_id.to_string())),
            ("input", json!(input)),
            ("model", json!(turn.model)),
            (
                "approvalPolicy",
                json!(parameters::approval_policy(turn.approval_mode)),
            ),
            (
                "approvalsReviewer",
                json!(parameters::approval_reviewer(turn.approval_mode)),
            ),
            (
                "sandboxPolicy",
                // A plan turn reads and writes nothing: its sandbox is read-only for that turn alone, since
                // every turn states its own.
                parameters::sandbox_policy(
                    if turn.plan {
                        orch_agent_contract::descriptor::SandboxMode::ReadOnly
                    } else {
                        turn.sandbox_mode
                    },
                    &directory,
                    &mounts,
                ),
            ),
            ("cwd", json!(cwd)),
        ]);
        // Collaboration mode `plan` stays until a turn states another: it is stated for a plan turn and for the
        // first turn after one, which goes back to `default`.
        if turn.plan || was_planning {
            params.insert(
                "collaborationMode".to_owned(),
                parameters::collaboration_mode(turn.plan, &turn.model, turn.effort.as_deref()),
            );
        }
        if let Some(remote) = &remote {
            params.insert("environments".to_owned(), json!([remote.selection()]));
        }
        TurnSettings::from_params(&turn.params).put_into(&mut params, turn.effort.as_deref());
        let asked = shared.clock().time();
        shared.with_session(turn.thread_id, |record| record.mapper.request_asked(asked))?;
        match shared.rpc().call("turn/start", Json::Object(params)).await {
            Ok(_) => Ok(()),
            Err(error) => {
                abandon(&shared);
                Err(map_rpc(error))
            }
        }
    }

    async fn steer(&mut self, steer: TurnSteer) -> Result<(), AdapterError> {
        let (shared, harness) = self.live()?;
        let (native, running) = shared.with_session(steer.thread_id, |record| {
            (
                record.mapper.native_thread().to_owned(),
                record.mapper.running_turn(),
            )
        })?;
        let Some((turn, native_turn)) = running.filter(|(turn, _)| *turn == steer.turn_id) else {
            return Err(conflict("that turn is not running"));
        };
        let _ = turn;
        let mut input = vec![json!({ "type": "text", "text": steer.text, "text_elements": [] })];
        for image in &steer.images {
            let path = image.path.as_ref().ok_or_else(|| {
                invalid("an image comes as a file the connector put on this machine")
            })?;
            input.push(json!({ "type": "localImage", "path": path }));
        }
        let mut params =
            json!({ "threadId": native, "expectedTurnId": native_turn, "input": input });
        if harness.started.harness_version == "0.165.0" {
            params["clientUserMessageId"] = json!(orch_types::new_id().to_string());
        }
        shared
            .rpc()
            .call("turn/steer", params)
            .await
            .map_err(map_rpc)?;
        Ok(())
    }

    async fn background_commands(
        &mut self,
        thread: ThreadId,
    ) -> Result<orch_agent_contract::background::NativeBackground, AdapterError> {
        let (shared, _) = self.live()?;
        shared.background(thread).await
    }

    async fn end_background_command(
        &mut self,
        thread: ThreadId,
        command: &str,
    ) -> Result<orch_agent_contract::background::NativeBackgroundEnd, AdapterError> {
        let (shared, _) = self.live()?;
        shared.end_background(thread, command).await
    }

    async fn interrupt(&mut self, thread: ThreadId, turn: TurnId) -> Result<(), AdapterError> {
        let (shared, _) = self.live()?;
        let (native, running) = shared.with_session(thread, |record| {
            (
                record.mapper.native_thread().to_owned(),
                record.mapper.running_turn(),
            )
        })?;
        // A turn that has already ended is interrupted by nothing: the turn ended as it did.
        let Some((_, native_turn)) = running.filter(|(running, _)| *running == turn) else {
            return Ok(());
        };
        shared
            .rpc()
            .call(
                "turn/interrupt",
                json!({ "threadId": native, "turnId": native_turn }),
            )
            .await
            .map_err(map_rpc)?;
        Ok(())
    }

    async fn resolve_approval(
        &mut self,
        thread: ThreadId,
        request_ref: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let (shared, _) = self.live()?;
        shared.resolve(thread, request_ref, &decision).await
    }

    async fn resync_session(&mut self, thread: ThreadId) -> Result<u32, AdapterError> {
        self.replay(thread).await
    }

    async fn discover_sessions(&mut self) -> Result<Vec<NativeSession>, AdapterError> {
        let (shared, _) = self.live()?;
        discovery::discover_sessions(shared.rpc()).await
    }

    async fn read_session(
        &mut self,
        native_session_ref: &str,
        write: bool,
    ) -> Result<orch_agent_contract::discovery::SessionRecord, AdapterError> {
        let (shared, running) = self.live()?;
        let directory = running
            .home
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("orch-codex"))
            .join(history::EXPORTS_DIRECTORY);
        // A long output the mapper keeps aside is not carried (the record keeps its end, and the harness the
        // whole): the files it makes for one go when the record is written.
        let scratch = directory.join(format!("record-outputs-{}", orch_types::new_id()));
        let outputs = OutputStore::new(scratch.clone());
        let ctx = Context {
            clock: shared.clock().as_ref(),
            outputs: &outputs,
            windows: None,
        };
        let record = discovery::read_session(
            shared.rpc(),
            native_session_ref,
            write,
            &directory,
            &ctx,
            stable_turn_id,
        )
        .await;
        let _ = std::fs::remove_dir_all(&scratch);
        record
    }

    async fn discover_mcp(&mut self, search: McpSearch) -> Result<Vec<McpFound>, AdapterError> {
        let (_, running) = self.live()?;
        Ok(discovery::found(
            running.home.as_deref(),
            &search.directories,
        ))
    }

    async fn read_mcp(&mut self, request: McpImportRequest) -> Result<McpDefinition, AdapterError> {
        let (_, running) = self.live()?;
        discovery::read_definition(running.home.as_deref(), &request)
    }

    async fn login(&mut self, request: LoginRequest) -> Result<(), AdapterError> {
        let (shared, _) = self.live()?;
        let (running, home) = self.managed_home()?;
        let sink = shared.sink().clone();
        match request.method.as_str() {
            provider::method::DEVICE_CODE => {
                let started = shared
                    .rpc()
                    .call(
                        "account/login/start",
                        json!({ "type": "chatgptDeviceCode" }),
                    )
                    .await
                    .map_err(map_rpc)?;
                let login_id = started
                    .get("loginId")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                shared
                    .login
                    .started(login_id, provider::method::DEVICE_CODE);
                let state = provider::device_code_state(&started, shared.clock().time());
                let _ = sink.emit(AdapterEvent::Login(state)).await;
                let _ = provider::write_login_note(&home, provider::method::DEVICE_CODE, None);
                let waiting = login_id.to_owned();
                let tracker = Arc::clone(&shared);
                let clock = Arc::clone(shared.clock());
                tokio::spawn(async move {
                    tokio::time::sleep(provider::DEVICE_CODE_LIFETIME).await;
                    if tracker.login.login_id().as_deref() == Some(waiting.as_str()) {
                        tracker.login.clear();
                        let mut state = LoginState::failed(
                            provider::method::DEVICE_CODE,
                            "the code was not entered in time",
                        );
                        state.state = orch_agent_contract::login::LoginStateKind::Expired;
                        let _ = clock;
                        let _ = tracker.sink().emit(AdapterEvent::Login(state)).await;
                    }
                });
                Ok(())
            }
            provider::method::AUTH_FILE => {
                let _ = sink
                    .emit(AdapterEvent::Login(LoginState::starting(
                        provider::method::AUTH_FILE,
                    )))
                    .await;
                let outcome = async {
                    let contents = request.values.text("auth_json").filter(|text| !text.is_empty());
                    let path = request.values.text("auth_path").filter(|path| !path.is_empty());
                    let text = match (contents, path) {
                        (Some(contents), None) => contents.to_owned(),
                        (None, Some(path)) => std::fs::read_to_string(path).map_err(|error| AdapterError::Other {
                            code: "invalid_login_values".to_owned(),
                            message: format!("the file could not be read: {error}"),
                            retryable: false,
                        })?,
                        _ => {
                            return Err(AdapterError::Other {
                                code: "invalid_login_values".to_owned(),
                                message: "give either the contents of an auth.json or the path of one, not neither and not both".to_owned(),
                                retryable: false,
                            });
                        }
                    };
                    provider::check_auth_file(&text)?;
                    process::write_private(&home.join(process::AUTH_FILE), text.as_bytes()).map_err(|error| {
                        AdapterError::HarnessError(format!("the sign-in could not be kept: {error}"))
                    })?;
                    let _ = provider::write_login_note(&home, provider::method::AUTH_FILE, None);
                    Ok(())
                }
                .await;
                if let Err(error) = outcome {
                    let _ = sink
                        .emit(AdapterEvent::Login(LoginState::failed(
                            provider::method::AUTH_FILE,
                            &error.detail(),
                        )))
                        .await;
                    return Err(error);
                }
                let _ = running;
                self.after_import(provider::method::AUTH_FILE).await
            }
            provider::method::ACCESS_TOKEN => {
                let _ = sink
                    .emit(AdapterEvent::Login(LoginState::starting(
                        provider::method::ACCESS_TOKEN,
                    )))
                    .await;
                let token = request
                    .values
                    .text("access_token")
                    .filter(|token| !token.is_empty())
                    .ok_or_else(|| AdapterError::Other {
                        code: "invalid_login_values".to_owned(),
                        message: "the access token is needed".to_owned(),
                        retryable: false,
                    })?;
                let now = shared.clock().time();
                let imported = async {
                    let mut info = provider::check_access_token(token, now)?;
                    if info.expires_at.is_none() {
                        info.expires_at = request.values.text("expires_at").filter(|day| !day.is_empty())
                            .and_then(|day| format!("{day}T23:59:59.000Z").parse().ok());
                        if info.expires_at.is_some_and(|end| end <= now) {
                            return Err(AdapterError::Other {code:"login_refused".to_owned(),message:"The workspace access token has already lapsed. Bring a fresh access token.".to_owned(),retryable:false});
                        }
                    }
                    match info.kind {
                        provider::AccessTokenKind::Chatgpt => provider::write_chatgpt_access_token(&home, token, now)?,
                        provider::AccessTokenKind::Workspace => {
                            let mut args = vec!["login".to_owned(), "--with-access-token".to_owned()];
                            args.extend(running.overrides.iter().cloned());
                            let (code, printed) = self.run_command(running, args, &format!("{token}\n")).await?;
                            if code != Some(0) || printed.lines().any(|line| line.to_lowercase().contains("error")) {
                                // The command's diagnostic can contain the credential: return only our own sentence.
                                return Err(AdapterError::Other { code: "login_refused".to_owned(),
                                    message: "This is not a workspace access token the harness accepts, or a ChatGPT access token.".to_owned(), retryable: false });
                            }
                        }
                    }
                    provider::write_access_note(&home, &info).map_err(|_| AdapterError::HarnessError("The provider could not keep the token's lifetime.".to_owned()))?;
                    Ok::<(), AdapterError>(())
                }.await;
                if let Err(error) = imported {
                    let _ = sink
                        .emit(AdapterEvent::Login(LoginState::failed(
                            provider::method::ACCESS_TOKEN,
                            &error.detail(),
                        )))
                        .await;
                    return Err(error);
                }
                self.after_import(provider::method::ACCESS_TOKEN).await
            }
            other => Err(AdapterError::Other {
                code: "unknown_login_method".to_owned(),
                message: format!("this harness has no sign-in method {other:?}"),
                retryable: false,
            }),
        }
    }

    async fn login_input(&mut self, values: LoginValues) -> Result<(), AdapterError> {
        let _ = values;
        Err(AdapterError::NotFound(
            "no sign-in is waiting for an answer".to_owned(),
        ))
    }

    async fn cancel_login(&mut self) -> Result<(), AdapterError> {
        let (shared, _) = self.live()?;
        if let Some(login_id) = shared.login.login_id() {
            let _ = shared
                .rpc()
                .call("account/login/cancel", json!({ "loginId": login_id }))
                .await;
            shared.login.clear();
        }
        let _ = shared
            .sink()
            .emit(AdapterEvent::Login(LoginState::idle()))
            .await;
        Ok(())
    }

    async fn logout(&mut self) -> Result<(), AdapterError> {
        let (shared, _) = self.live()?;
        self.managed_home()?;
        shared
            .rpc()
            .call("account/logout", json!({}))
            .await
            .map_err(map_rpc)?;
        shared.login.signed_out();
        let _ = shared
            .sink()
            .emit(AdapterEvent::Login(LoginState::idle()))
            .await;
        let _ = shared
            .sink()
            .emit(AdapterEvent::Facts(ProviderFacts::default()))
            .await;
        Ok(())
    }

    async fn provider_facts(&mut self) -> Result<ProviderFacts, AdapterError> {
        let (shared, running) = self.live()?;
        provider::read_facts(
            shared.rpc(),
            &shared.login,
            running.home.as_deref().filter(|_| running.managed),
            shared.clock().as_ref(),
        )
        .await
    }

    async fn renew_provider_credential(&mut self) -> Result<ProviderFacts, AdapterError> {
        let (shared, running) = self.live()?;
        provider::renew(
            shared.rpc(),
            &shared.login,
            running.home.as_deref().filter(|_| running.managed),
            shared.clock().as_ref(),
        )
        .await
    }

    async fn consume_reset(
        &mut self,
        request: orch_agent_contract::facts::ResetRequest,
    ) -> Result<orch_agent_contract::facts::ResetResult, AdapterError> {
        let (shared, running) = self.live()?;
        provider::consume_reset(
            shared.rpc(),
            &shared.login,
            running.home.as_deref().filter(|_| running.managed),
            shared.clock().as_ref(),
            request,
        )
        .await
    }

    async fn discover_models(&mut self) -> Result<Vec<ModelSuggestion>, AdapterError> {
        let (shared, running) = self.live()?;
        let mut found = provider::list_models(shared.rpc()).await?;
        // The list says nothing of a model's window; the catalogue the harness renders does.
        if found
            .iter()
            .any(|suggestion| suggestion.parameters.context_window.is_none())
        {
            let windows =
                provider::read_windows(self.launcher.as_ref(), &running.program, &running.env)
                    .await;
            provider::add_windows(&mut found, &windows);
        }
        Ok(found)
    }
}
