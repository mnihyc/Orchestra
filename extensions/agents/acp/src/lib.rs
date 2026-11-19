//! Generic Agent Client Protocol adapter: any ACP v1 agent over stdio, driven by a program's manifest.
//!
//! [`AcpAdapter`] drives an agent program that speaks the Agent Client Protocol: JSON-RPC in lines over the
//! program's standard streams ([`rpc`]). Everything that is not in the protocol is the program's manifest
//! ([`program`]): how it is started, which approval modes it has, how its tools are named, which plugin of Orch's
//! may run inside it. So a program that speaks the protocol is one more program by data, with no Rust of its own.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`program`] | The manifest's launch data, and the templates it is written in |
//! | [`programs`] | The launch data of the programs this build ships |
//! | [`process`] | Starting a program: the launcher, its streams, its end |
//! | [`rpc`] | JSON-RPC over the streams, calls that are answered in order with what was said before |
//! | [`wire`] | The protocol's names and the shapes the adapter reads |
//! | [`launch`] | Starting the program as the manifest says, and `initialize` |
//! | [`descriptor`] | What the adapter claims of the program |
//! | [`session`] | One conversation: turns, requests, interrupts, the end of the process |
//! | [`mapper`] | The updates of a turn as neutral events |
//! | [`tools`] | What a tool call is understood as |
//! | [`approvals`] | Requests to be allowed something and questions, as approvals |
//! | [`client`] | The client's file and terminal methods, carried out through Orch's executor |
//! | [`extensions`] | Orch's own `_orch/*` messages and the companion plugin's hello |
//!
//! **A process for each session.** The adapter starts the program once, when the harness is started, to learn its
//! version and what it says in `initialize` (the descriptor), and then ends it; a program is started again for each
//! session, in a home of its own, with the address the session's model calls go to and the token for them in its
//! variables. That keeps the key and the session's lease out of any shared state, and a session that ends takes
//! its process with it.
//!
//! The adapter never invents what the protocol does not give: token counts of a turn stay unreported until a
//! companion plugin reports them, and a turn with no finer report is observed as one model request whose
//! boundaries and counts are not measured.

#![forbid(unsafe_code)]

pub mod approvals;
pub mod client;
pub mod descriptor;
pub mod extensions;
pub mod launch;
pub mod mapper;
pub mod process;
pub mod program;
pub mod programs;
pub mod rpc;
pub mod session;
pub mod tools;
pub mod wire;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink,
};
use orch_agent_contract::conformance::QuietEvidence;
use orch_agent_contract::descriptor::{AgentDescriptor, ArrangementType, HarnessSupport};
use orch_agent_contract::events::{RawFrame, SessionEvent, SessionOpenedEvent};
use orch_agent_contract::facts::{AttachMode, HarnessState, HarnessStatus};
use orch_agent_contract::loopback::{EXEC_SERVER, ORCH_SERVER};
use orch_agent_contract::session::{
    ApprovalDecision, InstructionEvidence, InstructionFile, ProviderAccess, SessionMode,
    SessionOpened, SessionSpec, ThreadId, TurnId, TurnStart, TurnSteer, directory_chain,
};
use orch_types::{Clock, Id, Json, SystemClock};
use serde_json::json;

use crate::descriptor::Learned;
use crate::launch::KillGuard;
use crate::mapper::{MapperConfig, OutputStore};
use crate::process::{DirectLauncher, Launcher};
use crate::program::{AcpProgram, ProgramError, Variables};
use crate::rpc::{Direction, FrameTap};
use crate::session::{Env, Parts, Session};
use crate::wire::{ConfigOption, Modes};

/// How long opening a session is waited for: a program that restores a long conversation takes a while, and a
/// person is told what is being waited for.
pub const OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// The window that context use is measured against where neither the model's entry nor the agent says one: it only
/// labels the bar, and the agent's own report replaces it as soon as there is one.
pub const ASSUMED_CONTEXT_WINDOW: i64 = 128_000;

/// The names of members whose values never go into a raw frame.
const SECRET_MEMBERS: [&str; 8] = [
    "authorization",
    "token",
    "password",
    "secret",
    "api_key",
    "apikey",
    "access_token",
    "bearer",
];

/// Header and variable names whose values never go into a raw frame, where they are given as `{name, value}`.
const SECRET_NAMES: [&str; 6] = [
    "authorization",
    "x-api-key",
    "cookie",
    "proxy-authorization",
    "token",
    "secret",
];

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A frame with what is secret in it hidden: the value of a header or a variable that carries a credential, and
/// every member that is named for one.
pub fn redact(frame: &mut Json) {
    match frame {
        Json::Object(map) => {
            let secret_pair = map
                .get("name")
                .and_then(Json::as_str)
                .map(str::to_ascii_lowercase)
                .is_some_and(|name| {
                    SECRET_NAMES.iter().any(|secret| name == *secret)
                        || ["key", "token", "secret", "password"]
                            .iter()
                            .any(|word| name.contains(word))
                });
            for (key, value) in map.iter_mut() {
                let lower = key.to_ascii_lowercase();
                if (secret_pair && lower == "value") || SECRET_MEMBERS.contains(&lower.as_str()) {
                    *value = Json::String("[hidden]".to_owned());
                } else {
                    redact(value);
                }
            }
        }
        Json::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

/// The tap that keeps the frames of a session as raw frames of its thread.
fn raw_tap(sink: EventSink, thread: ThreadId, segment: Id) -> FrameTap {
    Arc::new(
        move |direction: Direction, frame: &Json| -> futures::future::BoxFuture<'static, bool> {
            let mut payload = frame.clone();
            redact(&mut payload);
            let sink = sink.clone();
            Box::pin(async move {
                sink.session(
                    thread,
                    segment,
                    SessionEvent::RawFrame(RawFrame {
                        direction: direction.name().to_owned(),
                        payload,
                    }),
                )
                .await
                .is_ok()
            })
        },
    )
}

struct Running {
    shared: Arc<Shared>,
    start: AgentStart,
    started: AgentStarted,
}

/// What the sessions of a started adapter share.
struct Shared {
    env: Arc<Env>,
    plugin: Option<Arc<[u8]>>,
    launcher: Arc<dyn Launcher>,
    start: AgentStart,
    binary: String,
    home_root: PathBuf,
    learned: Learned,
    sessions: Mutex<HashMap<ThreadId, Arc<Session>>>,
}

/// The adapter of an agent program that speaks the Agent Client Protocol.
pub struct AcpAdapter {
    program: Arc<AcpProgram>,
    kind: &'static str,
    launcher: Arc<dyn Launcher>,
    clock: Arc<dyn Clock>,
    /// The companion plugin's file, which each session of the program is given in its home.
    plugin: Option<Arc<[u8]>>,
    running: Option<Running>,
}

impl std::fmt::Debug for AcpAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpAdapter")
            .field("kind", &self.kind)
            .field("running", &self.running.is_some())
            .finish_non_exhaustive()
    }
}

fn unsupported(what: impl Into<String>) -> AdapterError {
    AdapterError::Unsupported(what.into())
}

/// How a protocol of the forwarder is said to a person.
fn protocol_words(protocol: &str) -> &str {
    match protocol {
        "openai-chat" => "an OpenAI Chat Completions endpoint",
        "openai-responses" => "an OpenAI Responses endpoint",
        "anthropic-messages" => "an Anthropic Messages endpoint",
        other => other,
    }
}

/// Why a thread whose provider speaks a protocol the program does not cannot run it: what to do, then why.
fn not_spoken(program: &AcpProgram, protocol: &str) -> AdapterError {
    let spoken: Vec<&str> = program
        .protocols
        .api
        .keys()
        .map(|name| protocol_words(name))
        .collect();
    unsupported(format!(
        "To run {}, give the environment a key for a provider it supports ({}): it does not speak {}.",
        program.title,
        spoken.join(", "),
        protocol_words(protocol)
    ))
}

fn program_fault(error: &ProgramError) -> AdapterError {
    match error {
        ProgramError::ProtocolNotSpoken(protocol) => unsupported(format!(
            "the program does not speak {protocol} to the forwarder"
        )),
        other => AdapterError::HarnessUnavailable(other.to_string()),
    }
}

impl AcpAdapter {
    /// An adapter for the program the manifest's launch data describes, running it as a process of the machine.
    #[must_use]
    pub fn new(program: AcpProgram) -> Self {
        Self::with_launcher(program, Arc::new(DirectLauncher), Arc::new(SystemClock))
    }

    /// The same, starting the program through `launcher` and reading time from `clock`.
    #[must_use]
    pub fn with_launcher(
        program: AcpProgram,
        launcher: Arc<dyn Launcher>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        // One kind is leaked for the life of the process: a connector makes a handful of adapters, once.
        let kind: &'static str = Box::leak(program.kind.clone().into_boxed_str());
        let plugin = programs::plugin_bundle(&program.kind).map(Arc::<[u8]>::from);
        Self {
            program: Arc::new(program),
            kind,
            launcher,
            clock,
            plugin,
            running: None,
        }
    }

    /// The same, with this file as the program's companion plugin in place of the one the adapter carries for
    /// the kind (a test's, or a program the adapter carries no plugin for).
    #[must_use]
    pub fn with_plugin(mut self, bundle: &[u8]) -> Self {
        self.plugin = Some(Arc::from(bundle));
        self
    }

    /// The program's launch data.
    #[must_use]
    pub fn program(&self) -> &AcpProgram {
        &self.program
    }

    fn live(&self) -> Result<&Arc<Shared>, AdapterError> {
        self.running
            .as_ref()
            .map(|running| &running.shared)
            .ok_or_else(|| {
                AdapterError::HarnessUnavailable("the harness is not started".to_owned())
            })
    }

    fn session(&self, thread: ThreadId) -> Result<Arc<Session>, AdapterError> {
        let shared = self.live()?;
        lock(&shared.sessions)
            .get(&thread)
            .cloned()
            .ok_or_else(|| AdapterError::SessionNotFound(thread.to_string()))
    }

    async fn teardown(&mut self) {
        if let Some(running) = self.running.take() {
            let sessions: Vec<Arc<Session>> = lock(&running.shared.sessions)
                .drain()
                .map(|(_, session)| session)
                .collect();
            futures::future::join_all(sessions.iter().map(|session| session.close())).await;
        }
    }
}

/// The home of the program's own files for a session.
fn session_home(shared: &Shared, spec: &SessionSpec, program: &AcpProgram) -> PathBuf {
    match program.home_scope {
        program::HomeScope::Segment => shared
            .home_root
            .join("threads")
            .join(spec.settings.thread_id.to_string())
            .join(spec.settings.segment_id.to_string()),
        program::HomeScope::Profile => shared.home_root.join("home"),
    }
}

/// The program's executable: where the connector put it, else where the profile says, else the program's name.
fn binary_of(start: &AgentStart, program: &AcpProgram) -> Result<String, AdapterError> {
    start
        .binary
        .clone()
        .or_else(|| {
            start
                .profile
                .attach
                .as_ref()
                .and_then(|attach| attach.binary.clone())
        })
        .or_else(|| program.programs.first().cloned())
        .ok_or_else(|| {
            AdapterError::HarnessUnavailable(format!(
                "the build of {} is not installed on this machine",
                program.title
            ))
        })
}

/// Where the companion plugin's file goes in a session's home: where the program names it, if the adapter carries
/// a plugin for it. The file is written at every start, so there is one way to find it and it is always the one
/// that shipped with the adapter.
fn plugin_path(home: &Path, program: &AcpProgram, plugin: Option<&[u8]>) -> Option<PathBuf> {
    plugin?;
    let file = program.companion.as_ref()?.file.as_deref()?;
    Some(home.join(file))
}

/// Adds the plugin's file to what a launch writes first.
fn add_plugin(
    plan: &mut program::LaunchPlan,
    home: &Path,
    program: &AcpProgram,
    plugin: Option<&[u8]>,
) {
    if let (Some(path), Some(bundle)) = (plugin_path(home, program, plugin), plugin) {
        plan.files.push(program::PlannedFile {
            path,
            content: bundle.to_vec(),
            rewrite: true,
        });
    }
}

fn inherited(variables: &mut Variables, program: &AcpProgram) {
    for name in &program.inherit {
        if let Ok(value) = std::env::var(name) {
            variables.inherit(name, &value);
        }
    }
}

/// The values of a probe: a session that is never opened, so that every template can be written and the program
/// can say what it is.
fn probe_variables(
    shared_home: &Path,
    start: &AgentStart,
    program: &AcpProgram,
    plugin: Option<&[u8]>,
) -> Variables {
    let mut variables = Variables::new(shared_home.join("probe"));
    variables.set("profile_home", shared_home.to_string_lossy().into_owned());
    let key = "0".repeat(orch_agent_contract::loopback::SESSION_KEY_DIGITS);
    let (protocol, api) = program
        .protocols
        .api
        .iter()
        .next()
        .map(|(protocol, api)| (protocol.clone(), api.clone()))
        .unwrap_or_default();
    let suffix = program
        .protocols
        .base_suffix
        .get(&protocol)
        .cloned()
        .unwrap_or_default();
    let neutral = variables
        .home()
        .join("neutral")
        .to_string_lossy()
        .into_owned();
    variables.set("cwd", neutral.clone());
    variables.set("workspace", neutral);
    variables.set("skills_dir", "");
    variables.set("llm_url", format!("{}{suffix}", start.loopback.llm(&key)));
    variables.set("llm_token", start.loopback.token.expose().to_owned());
    variables.set("model", "probe");
    variables.set("models_json", "[\"probe\"]");
    variables.models = vec!["probe".to_owned()];
    variables.set("effort", "");
    variables.set("api", api);
    variables.set("protocol", protocol);
    variables.set("context_window", ASSUMED_CONTEXT_WINDOW.to_string());
    variables.set("image_input", "true");
    variables.set("approval_mode", "ask");
    variables.set("permission_mode", "");
    variables.set("thread_id", "probe");
    variables.set("segment_id", "probe");
    variables.set("session_key", key.clone());
    variables.set("orch_url", start.loopback.orch(&key));
    variables.set("instructions_json", "\"\"");
    if let Some(file) = plugin_path(variables.home(), program, plugin) {
        variables.set("plugin", file.to_string_lossy().into_owned());
        // The probe is started the way a separated session is, so that the plugin says whether it can serve one.
        if program.arrangements.contains(&ArrangementType::Separated) {
            variables.set("separated", "true");
        }
    }
    if let Some(mode) = program.approval_modes().first() {
        variables.approval = program.approval(*mode);
        variables.set(
            "permission_mode",
            variables.approval.permission_mode.clone(),
        );
    }
    inherited(&mut variables, program);
    variables
}

/// The values of one session.
fn session_variables(
    shared: &Shared,
    spec: &SessionSpec,
    program: &AcpProgram,
    home: &Path,
) -> Result<Variables, AdapterError> {
    let settings = &spec.settings;
    let mut variables = Variables::new(home.to_path_buf());
    variables.set(
        "profile_home",
        shared.home_root.to_string_lossy().into_owned(),
    );
    variables.set("cwd", settings.cwd.clone());
    variables.set("workspace", settings.workspace_root.clone());
    variables.set(
        "skills_dir",
        spec.skills
            .as_ref()
            .map(|skills| skills.dir.clone())
            .unwrap_or_default(),
    );
    if let ProviderAccess::Forwarded { protocol, .. } = &spec.provider {
        let api = program
            .api_of(protocol)
            .ok_or_else(|| not_spoken(program, protocol))?;
        let suffix = program
            .protocols
            .base_suffix
            .get(protocol)
            .cloned()
            .unwrap_or_default();
        variables.set(
            "llm_url",
            format!("{}{suffix}", shared.start.loopback.llm(&spec.session_key)),
        );
        variables.set("llm_token", shared.start.loopback.token.expose().to_owned());
        variables.set("api", api.to_owned());
        variables.set("protocol", protocol.clone());
    }
    variables.set("model", settings.model.clone());
    let mut models = vec![settings.model.clone()];
    for model in &settings.available_models {
        if !models.contains(model) {
            models.push(model.clone());
        }
    }
    variables.set(
        "models_json",
        serde_json::to_string(&models).unwrap_or_else(|_| "[]".to_owned()),
    );
    variables.models = models;
    variables.set("effort", settings.effort.clone().unwrap_or_default());
    let window = settings
        .params
        .get(orch_agent_contract::parameters::MODEL_WINDOW_KEY)
        .and_then(Json::as_i64)
        .filter(|window| *window > 0)
        .unwrap_or(ASSUMED_CONTEXT_WINDOW);
    variables.set("context_window", window.to_string());
    variables.set("image_input", "true");
    let mode_word = serde_json::to_value(settings.approval_mode)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    variables.approval = program.approval(settings.approval_mode);
    variables.set("approval_mode", mode_word);
    variables.set(
        "permission_mode",
        variables.approval.permission_mode.clone(),
    );
    variables.set("thread_id", settings.thread_id.to_string());
    variables.set("segment_id", settings.segment_id.to_string());
    variables.set("session_key", spec.session_key.clone());
    variables.set("orch_url", shared.start.loopback.orch(&spec.session_key));
    variables.set(
        "instructions_json",
        serde_json::to_string(&settings.instructions).unwrap_or_else(|_| "\"\"".to_owned()),
    );
    if let Some(file) = plugin_path(home, program, shared.plugin.as_deref()) {
        variables.set("plugin", file.to_string_lossy().into_owned());
        if settings.arrangement == ArrangementType::Separated {
            variables.set("separated", "true");
        }
    }
    inherited(&mut variables, program);
    Ok(variables)
}

/// The MCP servers a session is given: Orch's tools, and the environment's own.
fn mcp_servers(shared: &Shared, spec: &SessionSpec, http: bool) -> Result<Vec<Json>, AdapterError> {
    let loopback = &shared.start.loopback;
    let header = json!([{ "name": "Authorization", "value": format!("Bearer {}", loopback.token.expose()) }]);
    let mut servers = Vec::new();
    let mut add = |name: &str, url: String| {
        servers
            .push(json!({ "type": "http", "name": name, "url": url, "headers": header.clone() }));
    };
    if !spec.settings.orch_tools.is_empty() {
        add(ORCH_SERVER, loopback.orch(&spec.session_key));
    }
    for name in &spec.mcp {
        add(name, loopback.mcp(&spec.session_key, name));
    }
    if !servers.is_empty() && !http {
        return Err(unsupported(
            "mcp: the program takes MCP servers only as programs it starts, and Orch's are reached by address",
        ));
    }
    Ok(servers)
}

/// The instruction files the program reads for a session, as its manifest's rule finds them.
fn instruction_sources(spec: &SessionSpec, program: &AcpProgram) -> Vec<InstructionFile> {
    let Some(chain) = directory_chain(&spec.settings.workspace_root, &spec.settings.cwd) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for directory in chain {
        for name in &program.instruction_files {
            let path = directory.join(name);
            if path.is_file() {
                found.push(InstructionFile::measured(
                    name.clone(),
                    &path,
                    InstructionEvidence::DocumentedRule,
                    None,
                ));
            }
        }
    }
    found
}

#[async_trait]
impl AgentAdapter for AcpAdapter {
    fn kind(&self) -> &'static str {
        self.kind
    }

    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        if start.profile.attach_mode() == AttachMode::Daemon {
            return Err(unsupported(
                "attaching to a daemon: this adapter starts the program itself and talks to it over its standard streams",
            ));
        }
        if self.program.process == program::ProcessModel::Shared {
            return Err(unsupported(
                "a program that serves every session from one process is not driven by this build",
            ));
        }
        if let Some(running) = &self.running
            && running.start == start
            && running.shared.learned.initialized.protocol_version == wire::PROTOCOL_VERSION
        {
            return Ok(running.started.clone());
        }
        self.teardown().await;
        let program = Arc::clone(&self.program);
        let binary = binary_of(&start, &program)?;
        let home_root = start
            .home
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join(format!("orch-{}", program.kind)));
        launch::prepare_home(&home_root)?;
        // The probe: the program as a session would start it, asked what it is, and ended. Its version is asked with
        // the same variables, which give it a home it may write to.
        let variables = probe_variables(&home_root, &start, &program, self.plugin.as_deref());
        let mut plan = program
            .plan(&variables)
            .map_err(|error| program_fault(&error))?;
        add_plugin(
            &mut plan,
            variables.home(),
            &program,
            self.plugin.as_deref(),
        );
        launch::prepare_home(variables.home())?;
        let version = launch::version(&*self.launcher, &binary, &program, &plan.env).await?;
        let mut launched = launch::start(
            &*self.launcher,
            &binary,
            &plan,
            variables.home().join("neutral"),
            None,
        )
        .await?;
        let guard = KillGuard::over(&launched);
        let initialization = tokio::time::timeout(
            rpc::INITIALIZE_TIMEOUT + std::time::Duration::from_secs(10),
            launched.initialize(
                &program,
                false,
                plugin_path(variables.home(), &program, self.plugin.as_deref()).is_some(),
            ),
        )
        .await
        .map_err(|_| AdapterError::Timeout)??;
        drop(guard);
        launched.end();
        let (extras, note) = launch::plugin_note(&program, initialization.hello.as_ref());
        let learned = Learned {
            version: version.clone(),
            initialized: initialization.initialized,
            hello: initialization.hello,
            extras,
        };
        let mut declared = descriptor::descriptor(&program, &learned);
        if !note.is_null() {
            let _ = declared.extensions.insert("x_acp_plugin", note);
        }
        if declared.harness.status == HarnessSupport::Unsupported {
            return Err(AdapterError::HarnessUnavailable(format!(
                "{} {version} is older than the range {} this adapter drives",
                program.title, program.supported_range
            )));
        }
        let started = AgentStarted {
            harness_version: version.clone(),
            descriptor: declared,
        };
        let env = Arc::new(Env {
            program: Arc::clone(&program),
            clock: Arc::clone(&self.clock),
            sink: sink.clone(),
            outputs: OutputStore::new(home_root.join("outputs")),
        });
        let shared = Arc::new(Shared {
            env,
            plugin: self.plugin.clone(),
            launcher: Arc::clone(&self.launcher),
            start: start.clone(),
            binary,
            home_root,
            learned,
            sessions: Mutex::new(HashMap::new()),
        });
        let _ = sink
            .emit(AdapterEvent::Status(HarnessStatus {
                status: HarnessState::Online,
                detail: None,
                harness_version: Some(version),
                sessions_open: 0,
            }))
            .await;
        self.running = Some(Running {
            shared,
            start,
            started: started.clone(),
        });
        Ok(started)
    }

    /// What a quiet start leaves behind: no model request carries a member the program was told not to send, and
    /// every start of the program had the variables that switch its telemetry off.
    fn verify_quiet_start(&self, evidence: &QuietEvidence<'_>) -> Result<(), String> {
        let privacy = &self.program.privacy;
        for request in evidence.requests {
            if let Some(member) = privacy
                .refuse_members
                .iter()
                .find(|member| request.body.get(member.as_str()).is_some())
            {
                return Err(format!(
                    "a model request to {} carries {member:?}, a record of the session that the program was set not to send",
                    request.path
                ));
            }
        }
        let home = self.program.home_variable.as_deref();
        for (name, wanted) in &self.program.env {
            let upper = name.to_ascii_uppercase();
            if !(upper.contains("TELEMETRY") || upper.contains("ANALYTICS")) || wanted.contains('{')
            {
                continue;
            }
            for launch in evidence
                .launches
                .iter()
                .filter(|launch| home.is_none_or(|variable| launch.contains_key(variable)))
            {
                if launch.get(name) != Some(wanted) {
                    return Err(format!(
                        "{name} was not {wanted:?} at a start of the program"
                    ));
                }
            }
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        let sink = self
            .running
            .as_ref()
            .map(|running| running.shared.env.sink.clone());
        self.teardown().await;
        if let Some(sink) = sink {
            let _ = sink
                .emit(AdapterEvent::Status(HarnessStatus {
                    status: HarnessState::Offline,
                    detail: None,
                    harness_version: None,
                    sessions_open: 0,
                }))
                .await;
        }
        Ok(())
    }

    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        self.running
            .as_ref()
            .map(|running| running.started.descriptor.clone())
            .ok_or_else(|| {
                AdapterError::HarnessUnavailable("the harness is not started".to_owned())
            })
    }

    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        let shared = Arc::clone(self.live()?);
        let program = Arc::clone(&self.program);
        let settings = &spec.settings;
        if let Some(reading) = settings.side_reading {
            return Err(unsupported(format!(
                "{} has no declared side-conversation mode for {reading:?}",
                program.title
            )));
        }
        if !matches!(
            settings.mode,
            SessionMode::New
                | SessionMode::Resume
                | SessionMode::Adopt
                | SessionMode::ImportNeutral
        ) {
            return Err(unsupported(format!(
                "a session of {} is not opened by {:?}",
                program.title, settings.mode
            )));
        }
        // One live session for a thread: an earlier one ends first.
        let earlier = lock(&shared.sessions).remove(&settings.thread_id);
        if let Some(earlier) = earlier {
            earlier.close().await;
        }
        let home = session_home(&shared, &spec, &program);
        let variables = session_variables(&shared, &spec, &program, &home)?;
        let mut plan = program
            .plan(&variables)
            .map_err(|error| program_fault(&error))?;
        add_plugin(&mut plan, &home, &program, shared.plugin.as_deref());
        launch::prepare_home(&home)?;
        let tap = settings.raw_events.then(|| {
            raw_tap(
                shared.env.sink.clone(),
                settings.thread_id,
                settings.segment_id,
            )
        });
        let mut launched = launch::start(
            &*shared.launcher,
            &shared.binary,
            &plan,
            home.join("neutral"),
            tap,
        )
        .await?;
        let guard = KillGuard::over(&launched);
        let uses_client_methods = program.client_methods && spec.exec.executor.is_some();
        let initialization = launched
            .initialize(
                &program,
                uses_client_methods,
                plugin_path(&home, &program, shared.plugin.as_deref()).is_some(),
            )
            .await?;
        let (extras, _) = launch::plugin_note(&program, initialization.hello.as_ref());
        let learned = Learned {
            version: shared.learned.version.clone(),
            initialized: initialization.initialized,
            hello: initialization.hello,
            extras,
        };
        let capabilities = &learned.initialized.agent_capabilities;
        let servers = mcp_servers(&shared, &spec, capabilities.mcp_capabilities.http)?;
        let server_names: Vec<String> = servers
            .iter()
            .filter_map(|server| server.get("name").and_then(Json::as_str).map(str::to_owned))
            .collect();
        let mut meta_members = json!({
            "version": extensions::VERSION,
            "thread_id": settings.thread_id,
            "segment_id": settings.segment_id,
            "session_key": spec.session_key,
            "arrangement": settings.arrangement,
            "approval_mode": settings.approval_mode,
            "sandbox_mode": settings.sandbox_mode,
            "model": settings.model,
            "skills_dir": spec.skills.as_ref().map(|skills| skills.dir.clone()),
            "instructions": settings.instructions,
        });
        if spec.exec.executor.is_some() {
            meta_members["exec"] = json!({
                "url": shared.start.loopback.mcp(&spec.session_key, EXEC_SERVER),
                "token": shared.start.loopback.token.expose(),
                "tool_set": spec.exec.tool_set,
            });
        }
        let meta = extensions::meta(meta_members);
        let resume_method = if capabilities.session_capabilities.resume.is_some() {
            Some(wire::agent::SESSION_RESUME)
        } else if capabilities.load_session {
            Some(wire::agent::SESSION_LOAD)
        } else {
            None
        };
        let mut import_report = None;
        let (method, params, native) = match settings.mode {
            SessionMode::New => (
                wire::agent::SESSION_NEW,
                json!({ "cwd": settings.cwd, "mcpServers": servers, "_meta": meta }),
                None,
            ),
            SessionMode::ImportNeutral => {
                if !learned.has(extensions::Extra::NativeCarry) {
                    return Err(unsupported("import_neutral"));
                }
                let history = spec.history.as_ref().ok_or_else(|| {
                    AdapterError::NotFound("an import names the history it carries".to_owned())
                })?;
                if let Some(companion) = &program.companion
                    && u64::try_from(history.size).unwrap_or(u64::MAX) > companion.max_import_bytes
                {
                    return Err(unsupported(format!(
                        "import_neutral: to carry this conversation as a summary or as a file, choose that instead: its history is {} bytes and {} takes at most {} in one go",
                        history.size, program.title, companion.max_import_bytes
                    )));
                }
                let mut import = json!({
                    "path": history.path,
                    "format": extensions::NEUTRAL_FORMAT,
                    "cwd": settings.cwd,
                    "provider": program.provider_id(),
                    "model": settings.model,
                });
                if let Some(through) = settings.upto_turn_ordinal {
                    import["throughTurn"] = json!(through);
                }
                let imported = launched
                    .rpc
                    .call_within(wire::orch::HISTORY_IMPORT, import, OPEN_TIMEOUT)
                    .await
                    .map_err(|error| {
                        let cause = launched.explain(error);
                        AdapterError::Other {
                            code: "history_import_failed".to_owned(),
                            message: format!(
                                "Retry with Summary or None, or ask an operator to check the agent's history support: the carried history could not be imported: {cause}"
                            ),
                            retryable: cause.retryable(),
                        }
                    })?;
                let native = imported
                    .get("sessionId")
                    .and_then(Json::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        AdapterError::HarnessError(
                            "the program did not name the session it wrote the history into"
                                .to_owned(),
                        )
                    })?;
                import_report = imported.get("report").and_then(extensions::history_report);
                let method = resume_method.ok_or_else(|| unsupported("resume"))?;
                (
                    method,
                    json!({ "sessionId": native, "cwd": settings.cwd, "mcpServers": servers, "_meta": meta }),
                    Some(native),
                )
            }
            _ => {
                let native = settings.native_session_ref.clone().ok_or_else(|| {
                    AdapterError::NotFound(
                        "a session that is resumed names the program's id for it".to_owned(),
                    )
                })?;
                let method = resume_method.ok_or_else(|| unsupported("resume"))?;
                (
                    method,
                    json!({ "sessionId": native, "cwd": settings.cwd, "mcpServers": servers, "_meta": meta }),
                    Some(native),
                )
            }
        };
        let answer = launched
            .rpc
            .call_within(method, params, OPEN_TIMEOUT)
            .await
            .map_err(|error| launched.explain(error))?;
        let acp_id = match &native {
            Some(native) => native.clone(),
            None => answer
                .get("sessionId")
                .and_then(Json::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    AdapterError::HarnessError(
                        "the program's answer to session/new has no session id".to_owned(),
                    )
                })?,
        };
        // What `session/load` replays is history Orch already has: it is read and dropped.
        if method == wire::agent::SESSION_LOAD {
            while launched.incoming.try_recv().is_ok() {}
        }
        let session = Session::new(Parts {
            env: Arc::clone(&shared.env),
            thread_id: settings.thread_id,
            segment_id: settings.segment_id,
            acp_id: acp_id.clone(),
            rpc: launched.rpc.clone(),
            kill: Arc::clone(&launched.kill),
            exit: launched.exit.clone(),
            stderr: launched.stderr.clone(),
            learned,
            mapper: MapperConfig {
                thread_id: settings.thread_id,
                segment_id: settings.segment_id,
                program: Arc::clone(&program),
                directory: settings.directory.clone(),
                cwd: settings.cwd.clone(),
                servers: server_names,
            },
            prompt_meta: extensions::meta(json!({
                "version": extensions::VERSION,
                "thread_id": settings.thread_id,
                "segment_id": settings.segment_id,
            })),
            options: ConfigOption::read_all(answer.get("configOptions")),
            modes: Modes::read(answer.get("modes")),
            client: uses_client_methods.then(|| {
                Arc::new(client::Methods::new(
                    client::Executor::new(
                        shared.start.loopback.mcp(&spec.session_key, EXEC_SERVER),
                        shared.start.loopback.token.expose().to_owned(),
                    ),
                    settings.cwd.clone(),
                ))
            }),
            raw_events: settings.raw_events,
        });
        let incoming = std::mem::replace(
            &mut launched.incoming,
            orch_agent_contract::flow::channel(|_: &rpc::Incoming| {
                std::mem::size_of::<rpc::Incoming>()
            })
            .1,
        );
        tokio::spawn(Arc::clone(&session).run(incoming));
        guard.disarm();
        lock(&shared.sessions).insert(settings.thread_id, Arc::clone(&session));
        let resumed = native.is_some() && settings.mode != SessionMode::ImportNeutral;
        let _ = shared
            .env
            .sink
            .session(
                settings.thread_id,
                settings.segment_id,
                SessionEvent::SessionOpened(SessionOpenedEvent {
                    native_session_ref: acp_id.clone(),
                    resumed,
                }),
            )
            .await;
        let open = lock(&shared.sessions).len();
        let _ = shared
            .env
            .sink
            .emit(AdapterEvent::Status(HarnessStatus {
                status: HarnessState::Online,
                detail: None,
                harness_version: Some(shared.learned.version.clone()),
                sessions_open: u32::try_from(open).unwrap_or(u32::MAX),
            }))
            .await;
        Ok(SessionOpened {
            native_session_ref: acp_id,
            resumed,
            import_report,
            instruction_sources: instruction_sources(&spec, &program),
            approval_reviewer: None,
        })
    }

    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        let shared = Arc::clone(self.live()?);
        let session = lock(&shared.sessions).remove(&thread);
        if let Some(session) = session {
            session.close().await;
        }
        Ok(())
    }

    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        self.session(turn.thread_id)?.start_turn(turn).await
    }

    async fn steer(&mut self, steer: TurnSteer) -> Result<(), AdapterError> {
        self.session(steer.thread_id)?.steer(steer).await
    }

    async fn interrupt(&mut self, thread: ThreadId, turn: TurnId) -> Result<(), AdapterError> {
        self.session(thread)?.interrupt(turn).await
    }

    async fn resolve_approval(
        &mut self,
        thread: ThreadId,
        request_ref: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AdapterError> {
        self.session(thread)?.resolve(request_ref, &decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_start_has_no_refused_member_in_a_request_and_the_telemetry_switch_at_every_start() {
        use orch_agent_contract::conformance::ObservedRequest;
        use std::collections::BTreeMap;
        let adapter = AcpAdapter::new(
            AcpProgram::from_json(&json!({
                "kind": "demo", "title": "Demo", "supported_range": "*",
                "home_variable": "DEMO_HOME", "env": { "DEMO_TELEMETRY_DISABLED": "1", "DEMO_KEY": "{llm_token}" },
                "privacy": { "refuse_members": ["session_log"] }
            }))
            .unwrap(),
        );
        let request = |body: Json| ObservedRequest {
            path: "/chat/completions".into(),
            body,
        };
        let launch = |telemetry: &str| {
            BTreeMap::from([
                ("DEMO_HOME".to_owned(), "/h".to_owned()),
                ("DEMO_TELEMETRY_DISABLED".to_owned(), telemetry.to_owned()),
            ])
        };
        let version_launch = BTreeMap::from([("PATH".to_owned(), "/bin".to_owned())]);
        let quiet = [request(json!({ "model": "m" }))];
        let ok = [launch("1"), version_launch];
        let verdict = |requests: &[ObservedRequest], launches: &[BTreeMap<String, String>]| {
            adapter.verify_quiet_start(&QuietEvidence {
                requests,
                home: None,
                launches,
            })
        };
        assert_eq!(
            verdict(&quiet, &ok),
            Ok(()),
            "a start that only asked for its version is not a start of the program"
        );
        let leaking = [request(json!({ "model": "m", "session_log": "x" }))];
        assert!(verdict(&leaking, &ok).unwrap_err().contains("session_log"));
        assert!(
            verdict(&quiet, &[launch("0")])
                .unwrap_err()
                .contains("DEMO_TELEMETRY_DISABLED")
        );
    }

    #[test]
    fn what_is_secret_in_a_frame_is_hidden_wherever_it_stands() {
        let mut frame = json!({
            "method": "session/new",
            "params": { "mcpServers": [
                { "name": "orch", "url": "http://127.0.0.1:1/mcp/k/orch",
                  "headers": [ { "name": "Authorization", "value": "Bearer abc" } ] },
                { "name": "local", "command": "x", "env": [ { "name": "GITHUB_TOKEN", "value": "ghp_1" }, { "name": "LANG", "value": "C" } ] } ],
              "_meta": { "orch": { "exec": { "url": "u", "token": "tok" } } } }
        });
        redact(&mut frame);
        let text = frame.to_string();
        assert!(
            !text.contains("abc") && !text.contains("ghp_1") && !text.contains("\"tok\""),
            "{text}"
        );
        assert!(text.contains("\"C\""), "a value that is no secret stays");
    }
}
