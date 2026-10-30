//! AgentAdapter interface and error vocabulary.
//!
//! An adapter is code that drives one harness kind and translates both ways: what Orch asks becomes what the
//! harness understands, and what the harness does becomes the neutral events of [`crate::events`]. Orch's
//! core never asks which adapter it is talking to: it reads the adapter's [`AgentDescriptor`] and calls the
//! trait. An operation the adapter does not support returns [`AdapterError::Unsupported`] with the name of
//! the capability, and is absent from its descriptor; the trait's default for each such operation does
//! exactly that, so an adapter implements what it declares and nothing else.
//!
//! The methods that every adapter has are `start`, `stop`, `probe`, `open_session`, `close_session` and
//! `start_turn`. For an external adapter, a program beside the connector, the methods are the agent-role
//! commands of the link, by those names ([`method`]), and what it emits are the events of
//! [`AdapterEvent`] on the plugin transport ([`AdapterEvent::to_wire`]).
//!
//! The connector gives an adapter, when it starts the harness ([`AgentStart`]): the home directory to use;
//! the base address and token of its own listener, for model calls, the executor and MCP; and the harness
//! binary's location when the tool cache provides one. The adapter never needs to know about tunnels, leases
//! or the gateway.

use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::credentials::{GivenCredential, IssuedCredential};
use crate::descriptor::AgentDescriptor;
use crate::discovery::{
    McpDefinition, McpFound, McpImportRequest, McpSearch, ModelSuggestion, NativeSession,
    SessionRecord,
};
use crate::events::SessionEvent;
use crate::facts::{AgentProfile, HarnessStatus, ProviderFacts};
use crate::history::{ExportKind, SessionArchive};
use crate::login::{LoginRequest, LoginState, LoginValues};
use crate::loopback::Loopback;
use crate::session::{
    ApprovalDecision, SessionOpened, SessionSpec, ThreadId, ToolError, TurnId, TurnStart, TurnSteer,
};

/// The names of the methods of an adapter on the plugin transport: the agent-role commands of the link.
pub mod method {
    /// Start the harness or attach to a running one; idempotent. Answers the harness version and the
    /// descriptor, which is also what `probe` asks for.
    pub const AGENT_START: &str = "agent.start";
    /// Stop the harness.
    pub const AGENT_STOP: &str = "agent.stop";
    /// List the sessions the harness already has.
    pub const AGENT_DISCOVER_SESSIONS: &str = "agent.discover_sessions";
    /// Read what the harness recorded of one of them, for its adoption.
    pub const AGENT_READ_SESSION: &str = "agent.read_session";
    /// Open a session.
    pub const SESSION_OPEN: &str = "session.open";
    /// Inspect an exact retained native input without model input: source and before_input_ref.
    pub const SESSION_PREFIX_INSPECT: &str = "session.prefix.inspect";
    /// Open one verified prefix: spec and prefix; returns NativePrefixOpened.
    pub const SESSION_PREFIX_OPEN: &str = "session.prefix.open";
    /// Observe/resume the exact retained source without autonomous input: spec, source, replace_segment_id.
    pub const SESSION_CONTINUATION_RESUME: &str = "session.continuation.resume";
    /// Close a session.
    pub const SESSION_CLOSE: &str = "session.close";
    /// Export a session.
    pub const SESSION_EXPORT: &str = "session.export";
    /// Re-emit the items of a session from the harness's own history.
    pub const SESSION_RESYNC: &str = "session.resync";
    /// Reads the effective native actions of an existing session.
    pub const SESSION_NATIVE_ACTIONS: &str = "session.native_actions";
    /// Start a turn.
    pub const TURN_START: &str = "turn.start";
    /// Add to a running turn.
    pub const TURN_STEER: &str = "turn.steer";
    /// Stop a running turn.
    pub const TURN_INTERRUPT: &str = "turn.interrupt";
    /// Answer an approval.
    pub const APPROVAL_RESOLVE: &str = "approval.resolve";
    /// Answer a call of one of Orch's tools.
    pub const ORCH_TOOL_RESULT: &str = "orch.tool_result";
    /// Start a sign-in.
    pub const PROVIDER_LOGIN_START: &str = "provider.login.start";
    /// Answer what a running sign-in asked.
    pub const PROVIDER_LOGIN_INPUT: &str = "provider.login.input";
    /// Cancel a running sign-in.
    pub const PROVIDER_LOGIN_CANCEL: &str = "provider.login.cancel";
    /// Sign the harness out.
    pub const PROVIDER_LOGOUT: &str = "provider.logout";
    /// Ask for the provider's status and facts.
    pub const PROVIDER_REFRESH: &str = "provider.refresh";
    /// List the models of the provider.
    pub const PROVIDER_DISCOVER_MODELS: &str = "provider.discover_models";
    /// Produce a credential in a form from the login the harness holds.
    pub const PROVIDER_CREDENTIAL_ISSUE: &str = "provider.credential.issue";
    /// Take back a handed-over credential and sign the harness in with it.
    pub const PROVIDER_CREDENTIAL_ACCEPT: &str = "provider.credential.accept";
    /// Stop using a handed-over credential and give it back.
    pub const AGENT_CREDENTIAL_RETURN: &str = "agent.credential.return";
    /// Install a credential for the harness to run on.
    pub const AGENT_CREDENTIAL_SET: &str = "agent.credential.set";
    /// List the MCP servers the harness has configured.
    pub const MCP_DISCOVER: &str = "mcp.discover";
    /// Read the definition of one MCP server out of the harness's configuration.
    pub const MCP_IMPORT: &str = "mcp.import";
    /// Reload the offered tools of an existing native session without changing its turn.
    pub const MCP_RELOAD: &str = "mcp.reload";
}

/// The kinds of refusal by a machine's local policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRefusal {
    /// The directory is outside the roots the machine allows.
    RootNotAllowed,
    /// The machine does not allow the mode `auto`.
    AutoApprovalNotAllowed,
    /// The endpoint is not among the machine's upstreams.
    UpstreamNotAllowed,
    /// The machine does not allow that arrangement.
    ArrangementNotAllowed,
    /// The machine does not let the server write files.
    RemoteWriteDisabled,
    /// The machine does not take delivered files.
    DeliverFilesDisabled,
    /// The machine does not offer that executor.
    ExecNotOffered,
    /// The machine does not allow MCP servers to be imported.
    McpImportDisabled,
}

impl PolicyRefusal {
    /// The code, `policy.` and then the refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::RootNotAllowed => "policy.root_not_allowed",
            Self::AutoApprovalNotAllowed => "policy.auto_approval_not_allowed",
            Self::UpstreamNotAllowed => "policy.upstream_not_allowed",
            Self::ArrangementNotAllowed => "policy.arrangement_not_allowed",
            Self::RemoteWriteDisabled => "policy.remote_write_disabled",
            Self::DeliverFilesDisabled => "policy.deliver_files_disabled",
            Self::ExecNotOffered => "policy.exec_not_offered",
            Self::McpImportDisabled => "policy.mcp_import_disabled",
        }
    }

    /// The refusal of a code, if it is one.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::RootNotAllowed,
            Self::AutoApprovalNotAllowed,
            Self::UpstreamNotAllowed,
            Self::ArrangementNotAllowed,
            Self::RemoteWriteDisabled,
            Self::DeliverFilesDisabled,
            Self::ExecNotOffered,
            Self::McpImportDisabled,
        ]
        .into_iter()
        .find(|refusal| refusal.code() == code)
    }
}

/// Why an operation of an adapter, or of an executor, did not succeed. The codes are those a command's
/// result uses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    /// The adapter does not support the operation; it names the capability, and the descriptor does not
    /// declare it.
    #[error("not supported: {0}")]
    Unsupported(String),
    /// Something asked for is not there.
    #[error("not found: {0}")]
    NotFound(String),
    /// The harness does not hold that session. The engine ends the segment and opens a new one by neutral
    /// import.
    #[error("the harness has no session {0}")]
    SessionNotFound(String),
    /// The harness cannot be run here: it is not installed, or not at a version this adapter drives.
    #[error("harness unavailable: {0}")]
    HarnessUnavailable(String),
    /// The harness failed.
    #[error("harness error: {0}")]
    HarnessError(String),
    /// The harness has to be signed in first.
    #[error("a sign-in is needed")]
    LoginRequired,
    /// The machine's local policy refuses it.
    #[error("refused by the machine's policy: {}", .0.code())]
    Policy(PolicyRefusal),
    /// The lease the request ran under is closed.
    #[error("the lease is closed")]
    LeaseClosed,
    /// The harness cannot run on the credential it was given.
    #[error("the harness cannot run on the credential it was given: {0}")]
    CredentialUnusable(String),
    /// A native archive could not be imported; the engine falls back to a neutral import.
    #[error("the native archive could not be imported: {0}")]
    ImportNativeFailed(String),
    /// It took longer than its deadline.
    #[error("timed out")]
    Timeout,
    /// The adapter failed in a way that is its own.
    #[error("internal error: {0}")]
    Internal(String),
    /// Any other code, as an external adapter states it.
    #[error("{code}: {message}")]
    Other {
        /// The code.
        code: String,
        /// What happened.
        message: String,
        /// Whether trying again may help.
        retryable: bool,
    },
}

/// How the hash of the files found is written in the refusal of a workspace's own configuration.
const FILES_LINE: &str = "files ";

impl AdapterError {
    /// The code, as a command's result and the plugin transport state it.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::Unsupported(_) => "unsupported",
            Self::NotFound(_) => "not_found",
            Self::SessionNotFound(_) => "session_not_found",
            Self::HarnessUnavailable(_) => "harness_unavailable",
            Self::HarnessError(_) => "harness_error",
            Self::LoginRequired => "login_required",
            Self::Policy(refusal) => refusal.code(),
            Self::LeaseClosed => "lease_closed",
            Self::CredentialUnusable(_) => "credential_unusable",
            Self::ImportNativeFailed(_) => "import_native_failed",
            Self::Timeout => "timeout",
            Self::Internal(_) => "internal",
            Self::Other { code, .. } => code,
        }
    }

    /// What happened, without the code.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Unsupported(what)
            | Self::NotFound(what)
            | Self::SessionNotFound(what)
            | Self::HarnessUnavailable(what)
            | Self::HarnessError(what)
            | Self::CredentialUnusable(what)
            | Self::ImportNativeFailed(what)
            | Self::Internal(what) => what.clone(),
            Self::Other { message, .. } => message.clone(),
            Self::LoginRequired | Self::Policy(_) | Self::LeaseClosed | Self::Timeout => {
                String::new()
            }
        }
    }

    /// Whether trying again may help: a timeout does, and an error that says so.
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            Self::Timeout => true,
            Self::Other { retryable, .. } => *retryable,
            _ => false,
        }
    }

    /// The refusal of a session whose workspace holds a configuration of `harness`'s own that nobody has
    /// answered for, or answered for other files: a sentence, the hash of what was found (`files <hash>`), then
    /// each place found, relative to the workspace root, on a line of its own.
    #[must_use]
    pub fn workspace_configuration(harness: &str, files: &str, found: &[String]) -> Self {
        let mut message = format!(
            "this workspace holds a configuration of {harness}'s own; the people who manage the environment decide whether it is used\n{FILES_LINE}{files}"
        );
        for place in found {
            message.push('\n');
            message.push_str(place);
        }
        Self::Other {
            code: crate::session::WORKSPACE_CONFIGURATION.to_owned(),
            message,
            retryable: false,
        }
    }

    /// The places a [`AdapterError::workspace_configuration`] refusal names, read back from its detail.
    #[must_use]
    pub fn configuration_found(detail: &str) -> Vec<String> {
        detail
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with(FILES_LINE))
            .map(str::to_owned)
            .collect()
    }

    /// The hash of the files a [`AdapterError::workspace_configuration`] refusal names, read back from its detail.
    #[must_use]
    pub fn configuration_files(detail: &str) -> String {
        detail
            .lines()
            .find_map(|line| line.trim().strip_prefix(FILES_LINE))
            .unwrap_or_default()
            .to_owned()
    }

    /// The error that a code, a message and a flag stand for: a known code gives its own variant, any
    /// other [`AdapterError::Other`].
    #[must_use]
    pub fn from_code(code: &str, message: &str, retryable: bool) -> Self {
        let text = message.to_owned();
        if let Some(refusal) = PolicyRefusal::from_code(code) {
            return Self::Policy(refusal);
        }
        match code {
            "unsupported" => Self::Unsupported(text),
            "not_found" => Self::NotFound(text),
            "session_not_found" => Self::SessionNotFound(text),
            "harness_unavailable" => Self::HarnessUnavailable(text),
            "harness_error" => Self::HarnessError(text),
            "login_required" => Self::LoginRequired,
            "lease_closed" => Self::LeaseClosed,
            "credential_unusable" => Self::CredentialUnusable(text),
            "import_native_failed" => Self::ImportNativeFailed(text),
            "timeout" => Self::Timeout,
            "internal" => Self::Internal(text),
            _ => Self::Other {
                code: code.to_owned(),
                message: text,
                retryable,
            },
        }
    }
}

/// What the connector gives an adapter when it starts the harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentStart {
    /// The provider whose harness process this is.
    pub provider_id: Id,
    /// The agent profile.
    pub agent_id: Id,
    /// The harness kind.
    pub harness_kind: String,
    /// How the harness is configured.
    pub profile: AgentProfile,
    /// The home directory to use. In a managed runtime it is `harness/<provider id>/<agent id>` under the
    /// connector's data directory; `null` on a connected machine, where the profile's `attach.home` or the
    /// harness's own default applies.
    pub home: Option<String>,
    /// What the provider's owner configured about the harness as a whole (`providers.config`), as the
    /// adapter's options schema allows.
    #[serde(default)]
    pub provider_config: Json,
    /// The connector's listener: where model calls, the executor and MCP are reached, and the token.
    pub loopback: Loopback,
    /// Where the harness binary is, when the tool cache provides one.
    pub binary: Option<String>,
}

/// The answer to starting the harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentStarted {
    /// The version of the harness that was started.
    pub harness_version: String,
    /// What the adapter declares of it.
    pub descriptor: AgentDescriptor,
}

/// Everything an adapter reports without being asked, besides the answer to a call.
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterEvent {
    /// One raw model-request observation, on the same stream for every adapter implementation.
    ModelRequest(Box<crate::requests::ModelRequestReport>),
    /// Something a session did: stream `session:<thread id>:<segment id>`.
    Session {
        /// The thread.
        thread_id: ThreadId,
        /// The segment of the thread.
        segment_id: Id,
        /// What happened.
        event: SessionEvent,
    },
    /// The progress of a sign-in (`provider.login`).
    Login(LoginState),
    /// The facts of the provider, on the path `native` (`provider.facts`).
    Facts(ProviderFacts),
    /// The descriptor changed (`harness.descriptor`).
    Descriptor(Box<AgentDescriptor>),
    /// The state of the harness process (`harness.status`).
    Status(HarnessStatus),
}

/// An event could not be read from the plugin transport.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    /// The name is not one of an adapter's events.
    #[error("{0:?} is not an event of an adapter")]
    UnknownEvent(String),
    /// A session event lacks the thread and the segment it belongs to.
    #[error("a session event names its thread and its segment")]
    NoStream,
    /// The parameters are not the shape of that event.
    #[error("the parameters of {event:?} are not its shape: {source}")]
    Shape {
        /// The event.
        event: String,
        /// What is wrong.
        source: serde_json::Error,
    },
}

impl AdapterEvent {
    /// The name of the event on the plugin transport: the kind for a session event, and otherwise
    /// `provider.login`, `provider.facts`, `harness.descriptor` or `harness.status`.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::ModelRequest(_) => crate::requests::MODEL_REQUEST_EVENT,
            Self::Session { event, .. } => event.kind(),
            Self::Login(_) => "provider.login",
            Self::Facts(_) => "provider.facts",
            Self::Descriptor(_) => "harness.descriptor",
            Self::Status(_) => "harness.status",
        }
    }

    /// The name and the parameters of the event as the plugin transport writes it: for a session event the
    /// members of its body together with `thread_id` and `segment_id`; otherwise the value itself.
    pub fn to_wire(&self) -> Result<(&'static str, Json), serde_json::Error> {
        let params = match self {
            Self::ModelRequest(report) => serde_json::to_value(report)?,
            Self::Session {
                thread_id,
                segment_id,
                event,
            } => {
                let mut written = serde_json::to_value(event)?;
                let mut params = match written.get_mut("body").map(Json::take) {
                    Some(Json::Object(body)) => body,
                    _ => serde_json::Map::new(),
                };
                params.insert("thread_id".into(), serde_json::to_value(thread_id)?);
                params.insert("segment_id".into(), serde_json::to_value(segment_id)?);
                Json::Object(params)
            }
            Self::Login(state) => serde_json::to_value(state)?,
            Self::Facts(facts) => serde_json::to_value(facts)?,
            Self::Descriptor(descriptor) => serde_json::to_value(descriptor)?,
            Self::Status(status) => serde_json::to_value(status)?,
        };
        Ok((self.name(), params))
    }

    /// Reads an event of the plugin transport.
    pub fn from_wire(name: &str, params: Json) -> Result<Self, WireError> {
        let shape = |source| WireError::Shape {
            event: name.to_owned(),
            source,
        };
        match name {
            crate::requests::MODEL_REQUEST_EVENT => serde_json::from_value(params)
                .map(|report| Self::ModelRequest(Box::new(report)))
                .map_err(shape),
            "provider.login" => serde_json::from_value(params)
                .map(Self::Login)
                .map_err(shape),
            "provider.facts" => serde_json::from_value(params)
                .map(Self::Facts)
                .map_err(shape),
            "harness.descriptor" => serde_json::from_value(params)
                .map(|descriptor| Self::Descriptor(Box::new(descriptor)))
                .map_err(shape),
            "harness.status" => serde_json::from_value(params)
                .map(Self::Status)
                .map_err(shape),
            kind if crate::events::SESSION_EVENT_KINDS.contains(&kind) => {
                let Json::Object(mut body) = params else {
                    return Err(WireError::NoStream);
                };
                let mut take_id = |member: &str| {
                    body.remove(member)
                        .and_then(|value| serde_json::from_value::<Id>(value).ok())
                        .ok_or(WireError::NoStream)
                };
                let thread_id = take_id("thread_id")?;
                let segment_id = take_id("segment_id")?;
                let event =
                    serde_json::from_value(serde_json::json!({ "kind": kind, "body": body }))
                        .map_err(shape)?;
                Ok(Self::Session {
                    thread_id,
                    segment_id,
                    event,
                })
            }
            other => Err(WireError::UnknownEvent(other.to_owned())),
        }
    }
}

/// The adapter has no one to report to: the host stopped listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("nobody is listening to the adapter any more")]
pub struct SinkClosed;

/// Where an adapter puts what it reports. It is handed over when the harness is started, and it can be
/// cloned freely, for the tasks that read the harness's output.
///
/// A full node or byte budget suspends its producer; native readers await delivery before reading more.
#[derive(Debug, Clone)]
pub struct EventSink {
    sender: crate::flow::Sender<AdapterEvent>,
}

/// What the host reads of the events an adapter reports.
pub type EventStream = crate::flow::Receiver<AdapterEvent>;

impl EventSink {
    /// Reports raw request boundaries and counts through the common adapter event transport.
    pub async fn request(
        &self,
        thread_id: Id,
        segment_id: Id,
        turn_id: Id,
        request: crate::requests::UsageRequest,
    ) -> Result<(), SinkClosed> {
        self.emit(AdapterEvent::ModelRequest(Box::new(
            crate::requests::ModelRequestReport {
                thread_id,
                segment_id,
                turn_id,
                request,
            },
        )))
        .await
    }
    /// A sink and the stream that reads what is put into it.
    #[must_use]
    pub fn channel() -> (Self, EventStream) {
        let (sender, receiver) = crate::flow::channel(|event: &AdapterEvent| {
            event
                .to_wire()
                .map(|(_, body)| crate::flow::json_weight(&body))
                .unwrap_or(usize::MAX)
                .saturating_add(std::mem::size_of::<AdapterEvent>())
        });
        (Self { sender }, receiver)
    }

    /// Reports an event, waiting for capacity instead of discarding a full-queue item.
    pub async fn emit(&self, event: AdapterEvent) -> Result<(), SinkClosed> {
        self.sender.send(event).await.map_err(|_| SinkClosed)
    }

    /// Whether the host has stopped consuming this adapter's events.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// Waits for host loss so a native process can be stopped even while it is idle.
    pub async fn closed(&self) {
        self.sender.closed().await;
    }

    /// Reports an event of a session.
    pub async fn session(
        &self,
        thread_id: ThreadId,
        segment_id: Id,
        event: SessionEvent,
    ) -> Result<(), SinkClosed> {
        self.emit(AdapterEvent::Session {
            thread_id,
            segment_id,
            event,
        })
        .await
    }
}

fn unsupported<T>(capability: &str) -> Result<T, AdapterError> {
    Err(AdapterError::Unsupported(capability.to_owned()))
}

/// One implementation for each harness kind, used by the connector in the `agent` role.
#[async_trait::async_trait]
pub trait AgentAdapter: Send + 'static {
    /// The harness kind it drives, such as `codex`.
    fn kind(&self) -> &'static str;

    /// The manifest of the program this adapter drives, for an adapter that is not part of Orch: the program says
    /// what it is in the `hello` of its plugin, and the system lists it and checks its builds like any other's. An
    /// adapter that is part of Orch has its manifest in the registry of the programs it ships and answers none.
    fn manifest(&self) -> Option<orch_agent_programs::ProgramManifest> {
        None
    }

    /// What the shared native conformance cases need to know of this harness; the defaults suit a harness that
    /// needs no special treatment.
    fn native_check(&self) -> crate::conformance::NativeCheck {
        crate::conformance::NativeCheck::default()
    }

    /// The name the harness gives the tool `tool` of the MCP server `server`, which a conformance case calls
    /// by.
    fn mcp_tool_name(&self, server: &str, tool: &str) -> String {
        format!("mcp__{server}__{tool}")
    }

    /// Whether what a start left behind shows that nothing but the work left the machine: the harness's own
    /// telemetry, update checks and reports are off. The shared cases have checked the paths of the scripted
    /// model already; this is what only the adapter can read (a configuration it wrote, a variable it set).
    fn verify_quiet_start(
        &self,
        evidence: &crate::conformance::QuietEvidence<'_>,
    ) -> Result<(), String> {
        let _ = evidence;
        Ok(())
    }

    /// Starts the harness or attaches to a running one. Idempotent.
    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError>;

    /// Stops the harness and everything it runs.
    async fn stop(&mut self) -> Result<(), AdapterError>;

    /// What the adapter declares of the harness as it is now.
    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError>;

    /// Opens a session: a new one, a resumed one, a fork, an import or an adoption.
    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError>;

    /// Resolves a current native user reference to its retained item/client identity
    /// and inspects that boundary in a quiescent exact native lane.
    /// No model call, source mutation, session replacement or file restore.
    async fn inspect_input_prefix(
        &mut self,
        source: crate::history::NativePrefixSource,
        before_input_ref: &str,
    ) -> Result<crate::history::NativeInputPrefix, AdapterError> {
        let _ = (source, before_input_ref);
        unsupported("history.before_input")
    }

    /// Opens a fresh same-thread segment from a reverified prefix. Its old lane
    /// must be closed; choosing/authorizing execution remains the caller's work.
    /// A source without this exact capability refuses instead of copying more.
    async fn open_input_prefix(
        &mut self,
        spec: SessionSpec,
        prefix: crate::history::NativeInputPrefix,
    ) -> Result<crate::history::NativePrefixOpened, AdapterError> {
        let _ = (spec, prefix);
        unsupported("history.before_input")
    }

    /// Reobserves one retained exact idle lane for preparation/recovery. It must
    /// retain its native identity and submit no model input or autonomous goal
    /// continuation. Unlike a generic resume, this default never guesses support.
    async fn resume_continuation(
        &mut self,
        spec: SessionSpec,
        source: crate::history::NativePrefixSource,
        replace_segment_id: Option<orch_types::Id>,
    ) -> Result<SessionOpened, AdapterError> {
        let _ = (spec, source, replace_segment_id);
        unsupported("history.resume_continuation")
    }

    /// Closes a session.
    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError>;

    /// Measures exact sessions or removes their native history in an Orch-managed home.
    /// Every requested source receives a result; unsupported adapters never guess file paths.
    async fn manage_history(
        &mut self,
        sources: Vec<crate::history::NativeHistorySource>,
        remove: bool,
    ) -> Result<Vec<crate::history::NativeHistoryFact>, AdapterError> {
        let _ = (sources, remove);
        unsupported("history.manage")
    }

    /// Removes old documented cache leftovers from an otherwise closed managed home.
    async fn cleanup_history_home(
        &mut self,
        before: orch_types::Time,
    ) -> Result<Vec<crate::history::NativeHomeCleanupItem>, AdapterError> {
        let _ = before;
        unsupported("history.home_cleanup")
    }

    /// Replaces just the session's offered server names, where mcp.reload is declared.
    async fn reload_mcp(
        &mut self,
        thread: ThreadId,
        servers: Vec<String>,
    ) -> Result<(), AdapterError> {
        let _ = (thread, servers);
        Err(AdapterError::Unsupported("mcp.reload".into()))
    }

    /// Has the harness count what fills a thread's context now, with its vendor's counter, where it breaks its
    /// context down on request: only between turns, in a session that is open, never starting one. `Ok(None)`
    /// where it cannot; the answer is also reported as a `usage.updated` of the turn it was counted after.
    async fn count_context(
        &mut self,
        thread: ThreadId,
    ) -> Result<Option<crate::events::ContextFill>, AdapterError> {
        let _ = thread;
        Ok(None)
    }

    /// Reports only selectors whose loaded native mapping can be preserved through dispatch.
    /// A cold or unsupported session must not claim an empty ready catalogue.
    async fn native_actions(
        &mut self,
        thread: ThreadId,
        refresh: bool,
    ) -> Result<crate::native::NativeCatalog, AdapterError> {
        let _ = (thread, refresh);
        Ok(crate::native::NativeCatalog::unavailable(
            crate::native::NativeCatalogState::Unsupported,
            "This harness cannot select native commands in this arrangement",
        ))
    }

    /// Atomically verifies and invokes the loaded selector, with the remaining ordinary input once.
    /// Never falls back to interpreting the person's text or a supplied filesystem path.
    async fn start_native_turn(
        &mut self,
        turn: TurnStart,
        selection: crate::native::NativeInvocation,
    ) -> Result<(), AdapterError> {
        let _ = (turn, selection);
        unsupported("native_actions")
    }

    /// Native steering is accepted only by an adapter that has proved this exact path.
    async fn steer_native(
        &mut self,
        steer: TurnSteer,
        selection: crate::native::NativeInvocation,
    ) -> Result<(), AdapterError> {
        let _ = (steer, selection);
        unsupported("native_actions.steer")
    }

    /// Reads live commands of an existing session without starting or resuming it.
    async fn background_commands(
        &mut self,
        thread: ThreadId,
    ) -> Result<crate::background::NativeBackground, AdapterError> {
        let _ = thread;
        Ok(crate::background::NativeBackground::unsupported(
            "exact native background control is unavailable",
        ))
    }

    /// Ends one exact session-owned command, never by sending model input or signalling an arbitrary pid.
    async fn end_background_command(
        &mut self,
        thread: ThreadId,
        command: &str,
    ) -> Result<crate::background::NativeBackgroundEnd, AdapterError> {
        let _ = (thread, command);
        unsupported("background_commands")
    }

    /// Whether closing this session is proven to preserve earlier retained commands.
    fn close_preserves_background(&self) -> bool {
        false
    }

    /// Starts a turn. The model, the effort, the parameters, the approval mode and the sandbox mode are
    /// stated explicitly, and passed to the harness every time.
    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError>;

    /// Adds to a running turn (`capabilities.steer`).
    async fn steer(&mut self, steer: TurnSteer) -> Result<(), AdapterError> {
        let _ = steer;
        unsupported("steer")
    }

    /// Stops a running turn (`capabilities.interrupt`). The turn ends with `turn.completed` and the status
    /// `interrupted`.
    async fn interrupt(&mut self, thread: ThreadId, turn: TurnId) -> Result<(), AdapterError> {
        let _ = (thread, turn);
        unsupported("interrupt")
    }

    /// Answers a request the harness made (`capabilities.approvals`).
    async fn resolve_approval(
        &mut self,
        thread: ThreadId,
        request_ref: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let _ = (thread, request_ref, decision);
        unsupported("approvals")
    }

    /// Answers a call of one of Orch's tools (`capabilities.orch_tools`).
    async fn tool_result(
        &mut self,
        thread: ThreadId,
        call_ref: &str,
        result: Result<Json, ToolError>,
    ) -> Result<(), AdapterError> {
        let _ = (thread, call_ref, result);
        unsupported("orch_tools")
    }

    /// Exports a session as a file: the harness's own record (`capabilities.export_native`) or the neutral
    /// one, through the turn `upto_turn_ordinal` when it is given.
    ///
    /// The file may be as large as the session's history: it is written a page at a time, never held whole.
    /// The call is dropped before it ends when the system asks for the command not to be carried out, or
    /// when the command has taken longer than it is allowed: an adapter that writes a file stops at its
    /// next page and removes what it wrote. The file of an archive that was made is the connector's, which
    /// puts it up and removes it, whichever way that ends.
    async fn export_session(
        &mut self,
        thread: ThreadId,
        kind: ExportKind,
        upto_turn_ordinal: Option<u32>,
    ) -> Result<SessionArchive, AdapterError> {
        let _ = (thread, upto_turn_ordinal);
        match kind {
            ExportKind::Native => unsupported("export_native"),
            ExportKind::Neutral => unsupported("export_neutral"),
        }
    }

    /// Reads the session's native history and re-emits, on the current stream, `item.completed` for every
    /// item it finds (with the `native_ref` it had) and `turn.completed` for every finished turn. Applied
    /// idempotently, this fills a gap that a reset left. Answers how many items it re-emitted.
    async fn resync_session(&mut self, thread: ThreadId) -> Result<u32, AdapterError> {
        let _ = thread;
        unsupported("resync")
    }

    /// Lists the sessions the harness already has (`capabilities.discover_sessions`).
    async fn discover_sessions(&mut self) -> Result<Vec<NativeSession>, AdapterError> {
        unsupported("discover_sessions")
    }

    /// Reads what the harness recorded of a session it holds, for a person who adopts it: the model and the
    /// effort it ran with, where the harness says, and with `write` its earlier turns as a file of entries
    /// ([`crate::discovery::RecordedEntry`]) for the connector to upload. The harness is not asked to open the
    /// session, and nothing of it changes. An adapter that discovers sessions and cannot read one answers
    /// `Unsupported`, and the session is adopted without its history.
    async fn read_session(
        &mut self,
        native_session_ref: &str,
        write: bool,
    ) -> Result<SessionRecord, AdapterError> {
        let _ = (native_session_ref, write);
        unsupported("read_session")
    }

    /// Lists the MCP servers the harness has configured, by name (`capabilities.mcp_discovery`). No command,
    /// address or secret is reported.
    async fn discover_mcp(&mut self, search: McpSearch) -> Result<Vec<McpFound>, AdapterError> {
        let _ = search;
        unsupported("mcp_discovery")
    }

    /// Reads the definition of one found MCP server out of the harness's configuration, for an import
    /// (`capabilities.mcp_discovery`). The connector keeps it on this machine.
    async fn read_mcp(&mut self, request: McpImportRequest) -> Result<McpDefinition, AdapterError> {
        let _ = request;
        unsupported("mcp_discovery")
    }

    // Only for adapters whose descriptor has `native_provider`.

    /// Starts one of the declared sign-in methods with what the person gave up front. Progress arrives as
    /// events ([`AdapterEvent::Login`]).
    async fn login(&mut self, request: LoginRequest) -> Result<(), AdapterError> {
        let _ = request;
        unsupported("native_provider")
    }

    /// Answers what a running sign-in asked for (`LoginState.asks`).
    async fn login_input(&mut self, values: LoginValues) -> Result<(), AdapterError> {
        let _ = values;
        unsupported("native_provider")
    }

    /// Cancels the running sign-in.
    async fn cancel_login(&mut self) -> Result<(), AdapterError> {
        unsupported("native_provider")
    }

    /// Signs the harness out.
    async fn logout(&mut self) -> Result<(), AdapterError> {
        unsupported("native_provider")
    }

    /// What the harness reports of its provider: the plan, the windows, who is signed in
    /// (`capabilities.provider_facts`).
    async fn provider_facts(&mut self) -> Result<ProviderFacts, AdapterError> {
        unsupported("provider_facts")
    }

    /// Asks the harness to renew its native login without stopping it and observes the resulting facts.
    /// A harness whose normal identity read triggers renewal may use that same read.
    async fn renew_provider_credential(&mut self) -> Result<ProviderFacts, AdapterError> {
        self.provider_facts().await
    }

    /// Uses the confirmed reset credit once and reads the resulting facts.
    async fn consume_reset(
        &mut self,
        request: crate::facts::ResetRequest,
    ) -> Result<crate::facts::ResetResult, AdapterError> {
        let _ = request;
        unsupported("consume_reset")
    }

    /// The models of the provider as the harness lists them (`capabilities.model_discovery`).
    async fn discover_models(&mut self) -> Result<Vec<ModelSuggestion>, AdapterError> {
        unsupported("model_discovery")
    }

    // Only for adapters whose descriptor has `credentials`.

    /// In the provider's runtime: produces `form` from the login this harness holds. Never more than the
    /// form declares. For a form with custody `transferred`, also stops using the login here.
    async fn issue_credential(&mut self, form: &str) -> Result<IssuedCredential, AdapterError> {
        let _ = form;
        unsupported("credentials")
    }

    /// Where the harness runs on a credential it is given: installs it, or replaces the one it has.
    async fn set_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        let _ = credential;
        unsupported("credentials")
    }

    /// Where a transferred credential is held: stops using it and gives back its current material.
    async fn return_credential(&mut self) -> Result<GivenCredential, AdapterError> {
        unsupported("credentials")
    }

    /// In the provider's runtime: takes a returned credential back and signs the harness in with it again.
    async fn accept_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        let _ = credential;
        unsupported("credentials")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ApprovalMode, SandboxMode};
    use crate::events::{ItemDelta, SessionEvent, TurnCompleted, TurnOutcome, TurnStarted};
    use futures::StreamExt;
    use futures::executor::block_on;
    use serde_json::json;

    /// The least an adapter does: it implements the six methods that every adapter has.
    struct Minimal;

    #[async_trait::async_trait]
    impl AgentAdapter for Minimal {
        fn kind(&self) -> &'static str {
            "minimal"
        }
        async fn start(
            &mut self,
            _: AgentStart,
            _: EventSink,
        ) -> Result<AgentStarted, AdapterError> {
            Err(AdapterError::HarnessUnavailable("not installed".into()))
        }
        async fn stop(&mut self) -> Result<(), AdapterError> {
            Ok(())
        }
        async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
            Err(AdapterError::HarnessUnavailable("not installed".into()))
        }
        async fn open_session(&mut self, _: SessionSpec) -> Result<SessionOpened, AdapterError> {
            Err(AdapterError::SessionNotFound("none".into()))
        }
        async fn close_session(&mut self, _: ThreadId) -> Result<(), AdapterError> {
            Ok(())
        }
        async fn start_turn(&mut self, _: TurnStart) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    fn id(n: u8) -> Id {
        format!("0199abcd-1234-7000-8000-0000000000{n:02}")
            .parse()
            .unwrap()
    }

    #[test]
    fn an_operation_the_adapter_does_not_have_says_which_capability_it_lacks() {
        block_on(an_operation_without_support_says_which_capability_it_lacks());
    }

    async fn an_operation_without_support_says_which_capability_it_lacks() {
        let mut adapter = Minimal;
        let steer = TurnSteer {
            thread_id: id(1),
            turn_id: id(2),
            text: "also".into(),
            images: vec![],
        };
        assert_eq!(
            adapter.steer(steer).await,
            Err(AdapterError::Unsupported("steer".into()))
        );
        assert_eq!(
            adapter.interrupt(id(1), id(2)).await,
            Err(AdapterError::Unsupported("interrupt".into()))
        );
        assert_eq!(
            adapter
                .export_session(id(1), ExportKind::Native, None)
                .await,
            Err(AdapterError::Unsupported("export_native".into()))
        );
        assert_eq!(
            adapter
                .inspect_input_prefix(
                    crate::history::NativePrefixSource {
                        thread_id: id(1),
                        segment_id: id(2),
                        native_session_ref: "native-source".into(),
                    },
                    "actual-native-input"
                )
                .await,
            Err(AdapterError::Unsupported("history.before_input".into()))
        );
        assert_eq!(
            adapter.logout().await,
            Err(AdapterError::Unsupported("native_provider".into()))
        );
        assert_eq!(
            adapter.issue_credential("api_key").await,
            Err(AdapterError::Unsupported("credentials".into()))
        );
        assert_eq!(
            adapter.discover_mcp(McpSearch::default()).await,
            Err(AdapterError::Unsupported("mcp_discovery".into()))
        );
        assert_eq!(adapter.kind(), "minimal");
        let turn = TurnStart {
            plan: false,
            thread_id: id(1),
            turn_id: id(2),
            text: "hi".into(),
            images: vec![],
            model: "m".into(),
            effort: None,
            params: json!({}),
            approval_mode: ApprovalMode::Bypass,
            sandbox_mode: SandboxMode::Open,
        };
        assert_eq!(adapter.start_turn(turn).await, Ok(()));
    }

    #[test]
    fn what_an_adapter_reports_reaches_the_host_in_order_and_a_closed_host_is_noticed() {
        block_on(reports_reach_the_host());
    }

    async fn reports_reach_the_host() {
        let (sink, mut stream) = EventSink::channel();
        let clone = sink.clone();
        sink.session(
            id(1),
            id(3),
            SessionEvent::TurnStarted(TurnStarted {
                turn_id: id(2),
                native_turn_ref: "t1".into(),
            }),
        )
        .await
        .unwrap();
        clone
            .emit(AdapterEvent::Session {
                thread_id: id(1),
                segment_id: id(3),
                event: SessionEvent::TurnCompleted(TurnCompleted {
                    turn_id: id(2),
                    status: TurnOutcome::Completed,
                    error: None,
                }),
            })
            .await
            .unwrap();
        let first = stream.next().await.unwrap();
        let second = stream.next().await.unwrap();
        assert_eq!(first.name(), "turn.started");
        assert_eq!(second.name(), "turn.completed");
        drop(stream);
        assert_eq!(
            sink.emit(AdapterEvent::Status(HarnessStatus {
                status: crate::facts::HarnessState::Offline,
                detail: None,
                harness_version: None,
                sessions_open: 0,
            }))
            .await,
            Err(SinkClosed)
        );
    }

    #[test]
    fn a_session_event_goes_over_the_plugin_transport_with_its_thread_and_segment() {
        let event = AdapterEvent::Session {
            thread_id: id(1),
            segment_id: id(3),
            event: SessionEvent::ItemDelta(ItemDelta {
                native_ref: "item_3".into(),
                field: crate::events::DeltaField::Text,
                offset: 0,
                len: 5,
                data: "Hello".into(),
            }),
        };
        let (name, params) = event.to_wire().unwrap();
        assert_eq!(name, "item.delta");
        assert_eq!(
            params,
            json!({ "native_ref": "item_3", "field": "text", "offset": 0, "len": 5, "data": "Hello",
                    "thread_id": id(1), "segment_id": id(3) })
        );
        assert_eq!(AdapterEvent::from_wire(name, params).unwrap(), event);
    }

    #[test]
    fn the_other_events_of_an_adapter_go_by_their_own_names() {
        let status = AdapterEvent::Status(HarnessStatus {
            status: crate::facts::HarnessState::Online,
            detail: None,
            harness_version: Some("0.160.1".into()),
            sessions_open: 2,
        });
        let (name, params) = status.to_wire().unwrap();
        assert_eq!(name, "harness.status");
        assert_eq!(AdapterEvent::from_wire(name, params).unwrap(), status);
        let login = AdapterEvent::Login(LoginState::starting("device_code"));
        let (name, params) = login.to_wire().unwrap();
        assert_eq!(name, "provider.login");
        assert_eq!(AdapterEvent::from_wire(name, params).unwrap(), login);
        assert!(matches!(
            AdapterEvent::from_wire("mystery", json!({})),
            Err(WireError::UnknownEvent(_))
        ));
        assert!(matches!(
            AdapterEvent::from_wire("item.delta", json!({ "native_ref": "x" })),
            Err(WireError::NoStream)
        ));
        assert!(matches!(
            AdapterEvent::from_wire(
                "item.delta",
                json!({ "thread_id": id(1), "segment_id": id(2), "native_ref": 4 })
            ),
            Err(WireError::Shape { .. })
        ));
    }

    #[test]
    fn an_error_is_known_by_its_code_and_a_code_by_its_error() {
        for (code, retryable) in [
            ("unsupported", false),
            ("not_found", false),
            ("session_not_found", false),
            ("harness_unavailable", false),
            ("harness_error", false),
            ("login_required", false),
            ("policy.auto_approval_not_allowed", false),
            ("lease_closed", false),
            ("credential_unusable", false),
            ("import_native_failed", false),
            ("timeout", true),
            ("internal", false),
        ] {
            let error = AdapterError::from_code(code, "detail", retryable);
            assert_eq!(error.code(), code);
            assert_eq!(error.retryable(), retryable, "{code}");
        }
        let own = AdapterError::from_code("vendor.rate_limited", "slow down", true);
        assert_eq!(own.code(), "vendor.rate_limited");
        assert!(own.retryable());
        assert_eq!(own.detail(), "slow down");
        assert_eq!(
            AdapterError::Policy(PolicyRefusal::AutoApprovalNotAllowed).to_string(),
            "refused by the machine's policy: policy.auto_approval_not_allowed"
        );
        assert_eq!(PolicyRefusal::from_code("policy.mystery"), None);
    }

    #[test]
    fn the_methods_of_an_external_adapter_are_the_commands_of_the_agent_role() {
        assert_eq!(method::AGENT_START, "agent.start");
        assert_eq!(method::TURN_STEER, "turn.steer");
        assert_eq!(method::PROVIDER_LOGIN_START, "provider.login.start");
        assert_eq!(method::ORCH_TOOL_RESULT, "orch.tool_result");
    }
    #[tokio::test]
    async fn a_stalled_host_bounds_the_shared_sink_and_preserves_a_million_ordered_events() {
        use futures::StreamExt;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        const EVENTS: usize = 1_000_000;
        let (sink, mut stream) = EventSink::channel();
        let sent = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&sent);
        let writer = sink.clone();
        let (full_tx, full_rx) = tokio::sync::oneshot::channel();
        let producer = tokio::spawn(async move {
            let mut full_tx = Some(full_tx);
            for index in 0..EVENTS {
                writer
                    .emit(AdapterEvent::Status(HarnessStatus {
                        status: crate::facts::HarnessState::Online,
                        detail: Some(format!("native-event-{index}")),
                        harness_version: None,
                        sessions_open: 0,
                    }))
                    .await
                    .unwrap();
                count.fetch_add(1, Ordering::AcqRel);
                if index + 1 == crate::flow::QUEUE_ITEMS {
                    full_tx.take().unwrap().send(()).unwrap();
                }
            }
        });
        full_rx.await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(sent.load(Ordering::Acquire), crate::flow::QUEUE_ITEMS);
        assert_eq!(sink.sender.usage().items, crate::flow::QUEUE_ITEMS);
        assert!(sink.sender.usage().bytes <= crate::flow::QUEUE_BYTES);
        assert!(!producer.is_finished());
        for index in 0..EVENTS {
            let Some(AdapterEvent::Status(status)) = stream.next().await else {
                panic!("every accepted event must arrive, in order");
            };
            assert_eq!(
                status.detail.as_deref(),
                Some(format!("native-event-{index}").as_str())
            );
        }
        producer.await.unwrap();
        assert_eq!(sent.load(Ordering::Acquire), EVENTS);
        assert_eq!(sink.sender.usage().items, 0);
    }
}
