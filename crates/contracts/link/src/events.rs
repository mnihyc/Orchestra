//! Session, provider, environment, harness and node events.
//!
//! What a machine tells the system is an event: a kind, and a body that
//! the kind defines. Most events are durable and travel in a stream,
//! numbered and acknowledged; a few are measurements that are worthless
//! when they are late and travel beside the streams. The system applies
//! every durable event in order and exactly once in its effects, so that
//! an event that is sent again changes nothing.
//!
//! [`EventKind`] lists every kind with the streams it travels on.
//! [`Event`] is a durable event with its body and [`Report`] a
//! measurement with its body. The frames that carry them can be read
//! without knowing the kind, so an unknown kind stops no stream.

pub use crate::browser::{EnvironmentFactsBrowser, EnvironmentFactsVnc};
use crate::commands::{BlobRef, CatalogueError, IndexMode, OrchTool, SandboxMode};
use crate::foreign;
use crate::session::{Arch, MachineFacts, Os};
use crate::streams::{Eph, Ev, Stream, StreamFamily};
use orch_types::{Hash, Id, Json, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// At most this many progress reports of one job are sent in a second.
pub const JOB_PROGRESS_PER_SECOND: u32 = 1;
/// How often a host reports what it uses, in seconds, unless a value
/// moved far enough to be reported at once.
pub const NODE_LOAD_EVERY_SECS: u32 = 60;
/// A host reports its load at once when a value moved by more than this
/// share of what the host has.
pub const NODE_LOAD_STEP: f64 = 0.1;

/// An idle update request from this stream's authenticated physical machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineUpdateRequested {
    /// The recommendation this program is ready to take.
    pub recommended: String,
}

/// Why an accepted update left the running version in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineUpdateFailed {
    /// The machine's own explanation, including the local command when policy refused it.
    pub detail: String,
}

// ---------------------------------------------------------------- catalogue

/// Whether an event is kept until the system has taken it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Durability {
    /// Numbered, kept and acknowledged.
    Durable,
    /// Sent once, while a session is up, and never again.
    Ephemeral,
}

macro_rules! event_kinds {
    ($( $(#[$doc:meta])* $variant:ident = $kind:literal, $durability:ident, [$($family:ident),+]; )*) => {
        /// Every kind of event.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum EventKind {
            $( $(#[$doc])* #[serde(rename = $kind)] $variant, )*
        }

        impl EventKind {
            /// Every kind, in the order of the catalogue.
            pub const ALL: &'static [EventKind] = &[ $( EventKind::$variant, )* ];

            /// The kind as it is written.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $kind, )* }
            }

            /// Whether events of this kind are kept until they are taken
            /// in.
            #[must_use]
            pub const fn durability(self) -> Durability {
                match self { $( Self::$variant => Durability::$durability, )* }
            }

            /// The streams events of this kind travel on, or beside.
            #[must_use]
            pub const fn families(self) -> &'static [StreamFamily] {
                match self { $( Self::$variant => &[ $( StreamFamily::$family ),+ ], )* }
            }
        }

        impl std::str::FromStr for EventKind {
            type Err = CatalogueError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $kind => Ok(Self::$variant), )*
                    _ => Err(CatalogueError::UnknownKind(text.to_owned())),
                }
            }
        }
    };
}

event_kinds! {
    /// A session is open in its harness.
    SessionOpened = "session.opened", Durable, [Session];
    /// Instruction source metadata refreshed by the harness, without a model request.
    InstructionSources = "session.instructions", Durable, [Session];
    /// The harness gave the conversation a title.
    SessionTitle = "session.title", Durable, [Session];
    /// The harness no longer holds the session.
    SessionLost = "session.lost", Durable, [Session];
    /// A turn runs.
    TurnStarted = "turn.started", Durable, [Session];
    /// An item of the conversation began.
    ItemStarted = "item.started", Durable, [Session];
    /// The text of an item grew.
    ItemDelta = "item.delta", Durable, [Session];
    /// Members of an item's payload changed.
    ItemUpdated = "item.updated", Durable, [Session];
    /// An item is final.
    ItemCompleted = "item.completed", Durable, [Session];
    /// The harness asks a person to decide something.
    ApprovalRequested = "approval.requested", Durable, [Session];
    /// The harness withdrew what it had asked.
    ApprovalClosed = "approval.closed", Durable, [Session];
    /// A running turn began to shorten its own conversation, or went on.
    TurnPhase = "turn.phase", Durable, [Session];
    /// What a turn and its thread have used so far.
    UsageUpdated = "usage.updated", Durable, [Session];
    /// One neutral model request, from any adapter implementation.
    ModelRequest = "model.request", Durable, [Session];
    /// A turn ended.
    TurnCompleted = "turn.completed", Durable, [Session];
    /// The stopped turn's actual native command cleanup, even after its completion.
    TurnCommandCleanup = "turn.command_cleanup", Durable, [Session];
    /// The agent called one of the system's own tools.
    OrchToolCall = "orch.tool_call", Durable, [Session];
    /// The harness has no credential it can run on, or will soon have
    /// none.
    CredentialNeeded = "credential.needed", Durable, [Session];
    /// One message of the harness's own protocol, for diagnosis.
    RawFrame = "raw.frame", Durable, [Session];
    /// What a machine is and has.
    MachineFacts = "machine.facts", Durable, [Machine];
    /// A physical program asks the server to drain all work using it before updating.
    MachineUpdateRequested = "machine.update.requested", Durable, [Machine];
    /// A physical program could not complete an accepted update; resume ordinary admission.
    MachineUpdateFailed = "machine.update.failed", Durable, [Machine];
    /// A sign-in that a harness on a person's machine already has.
    ProviderFound = "provider.found", Durable, [Machine];
    /// The state of a harness process.
    HarnessStatus = "harness.status", Durable, [Harness];
    /// What an adapter declares about its harness.
    HarnessDescriptor = "harness.descriptor", Durable, [Harness];
    /// The state of a provider.
    ProviderStatus = "provider.status", Durable, [Provider];
    /// What a provider reports about its plan and its limits.
    ProviderFacts = "provider.facts", Durable, [Provider];
    /// How far a sign-in is.
    ProviderLogin = "provider.login", Durable, [Provider];
    /// The forwarder refused a model request.
    LeaseDenied = "lease.denied", Durable, [Provider];
    /// What one forwarded model request used.
    LeaseUsage = "lease.usage", Durable, [Provider];
    /// The state of an environment.
    EnvStatus = "env.status", Durable, [Env];
    /// What an environment is and has.
    EnvFacts = "env.facts", Durable, [Env];
    /// A new list of an environment's whole tree.
    EnvManifest = "env.manifest", Durable, [Env];
    /// How far a job is.
    JobProgress = "job.progress", Durable, [Env, Node];
    /// A known browser changed its state or tab count.
    BrowserState = "browser.state", Durable, [Env];
    /// A known display changed its lifecycle state.
    DisplayState = "display.state", Durable, [Env];
    /// A shell ended while nobody was attached to it.
    PtyExited = "pty.exited", Durable, [Env];
    /// The state a host finds one of its units in.
    NodeRuntime = "node.runtime", Durable, [Node];
    /// The verified policy installed by the registered node of a runtime's host.
    NodeNetwork = "node.network", Durable, [Node];
    /// What a host has and can make.
    NodeFacts = "node.facts", Durable, [Node];
    /// The owned kept-image inventory at one node-local epoch, including archived files.
    NodeKeptImages = "node.kept-images", Durable, [Node];
    /// What a host uses.
    NodeLoad = "node.load", Durable, [Node];
    /// A host opening that its node no longer serves, though nobody closed it.
    PortState = "port.state", Durable, [Machine, Node];
    /// One run of an operator's lifecycle hook for a runtime's unit, and how it ended.
    NodeHook = "node.hook", Durable, [Node];
    /// What an environment used at one moment.
    StatsSample = "stats.sample", Ephemeral, [Env];
    /// The processes of an environment at one moment.
    ProcsSnapshot = "procs.snapshot", Ephemeral, [Env];
    /// Current scoped port observations with their live query reference.
    PortsSnapshot = "ports.snapshot", Ephemeral, [Env];
    /// What changed in a directory that is watched.
    FsChanged = "fs.changed", Ephemeral, [Env];
    /// Git status changed in a watched workspace directory.
    GitChanged = "git.changed", Ephemeral, [Env];
    /// Current shell foreground observations while their list is watched.
    TerminalsSnapshot = "terminals.snapshot", Ephemeral, [Env];
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl EventKind {
    /// Whether events of this kind may travel on, or beside, `stream`.
    #[must_use]
    pub fn travels_on(self, stream: &Stream) -> bool {
        self.families().contains(&stream.family())
    }
}

macro_rules! payloads {
    ($(#[$enum_doc:meta])* $name:ident { $( $(#[$doc:meta])* $variant:ident($body:ty) = $kind:literal; )* }) => {
        $(#[$enum_doc])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "k", content = "b")]
        #[allow(
            clippy::large_enum_variant,
            reason = "an event is read once and applied; boxing its largest body would cost every reader an indirection"
        )]
        pub enum $name {
            $( $(#[$doc])* #[serde(rename = $kind)] $variant($body), )*
        }

        impl $name {
            /// Its kind.
            #[must_use]
            pub const fn kind(&self) -> EventKind {
                match self { $( Self::$variant(_) => EventKind::$variant, )* }
            }

            /// Its body, as it is written.
            pub fn body(&self) -> Result<Json, serde_json::Error> {
                match self { $( Self::$variant(body) => serde_json::to_value(body), )* }
            }

            /// What a frame of `kind` with `body` says.
            pub fn decode(kind: &str, body: Json) -> Result<Self, CatalogueError> {
                match kind {
                    $( $kind => serde_json::from_value::<$body>(body)
                        .map(Self::$variant)
                        .map_err(|source| CatalogueError::Body { kind: $kind, source }), )*
                    _ => Err(CatalogueError::UnknownKind(kind.to_owned())),
                }
            }
        }
    };
}

payloads! {
    /// A durable event: its kind with its body.
    Event {
        /// A session is open in its harness.
        SessionOpened(SessionOpened) = "session.opened";
        /// Instruction files reported by the harness after opening.
        InstructionSources(orch_agent_contract::events::InstructionSources) = "session.instructions";
        /// The harness gave the conversation a title.
        SessionTitle(SessionTitle) = "session.title";
        /// The harness no longer holds the session.
        SessionLost(SessionLost) = "session.lost";
        /// A turn runs.
        TurnStarted(TurnStarted) = "turn.started";
        /// An item of the conversation began.
        ItemStarted(ItemStarted) = "item.started";
        /// The text of an item grew.
        ItemDelta(ItemDelta) = "item.delta";
        /// Members of an item's payload changed.
        ItemUpdated(ItemUpdated) = "item.updated";
        /// An item is final.
        ItemCompleted(ItemCompleted) = "item.completed";
        /// The harness asks a person to decide something.
        ApprovalRequested(ApprovalRequested) = "approval.requested";
        /// The harness withdrew what it had asked.
        ApprovalClosed(ApprovalClosed) = "approval.closed";
        /// A running turn began to shorten its own conversation, or went
        /// on.
        TurnPhase(TurnPhase) = "turn.phase";
        /// What a turn and its thread have used so far.
        UsageUpdated(UsageUpdated) = "usage.updated";
        /// One neutral request observation, including its final counts and outcome.
        ModelRequest(ModelRequest) = "model.request";
        /// A turn ended.
        TurnCompleted(TurnCompleted) = "turn.completed";
        /// Native command cleanup counts of an exact stopped turn.
        TurnCommandCleanup(TurnCommandCleanup) = "turn.command_cleanup";
        /// The agent called one of the system's own tools.
        OrchToolCall(OrchToolCall) = "orch.tool_call";
        /// The harness has no credential it can run on, or will soon
        /// have none.
        CredentialNeeded(CredentialNeeded) = "credential.needed";
        /// One message of the harness's own protocol, for diagnosis.
        RawFrame(RawFrame) = "raw.frame";
        /// What a machine is and has.
        MachineFacts(MachineFacts) = "machine.facts";
        /// An automatic update uses the same server-side drain as the web action.
        MachineUpdateRequested(MachineUpdateRequested) = "machine.update.requested";
        /// A failed verified update keeps the running executable and reports its reason.
        MachineUpdateFailed(MachineUpdateFailed) = "machine.update.failed";
        /// A sign-in that a harness on a person's machine already has.
        ProviderFound(ProviderFound) = "provider.found";
        /// The state of a harness process.
        HarnessStatus(HarnessStatus) = "harness.status";
        /// What an adapter declares about its harness.
        HarnessDescriptor(foreign::AgentDescriptor) = "harness.descriptor";
        /// The state of a provider.
        ProviderStatus(ProviderStatus) = "provider.status";
        /// What a provider reports about its plan and its limits.
        ProviderFacts(foreign::ProviderFacts) = "provider.facts";
        /// How far a sign-in is.
        ProviderLogin(foreign::LoginState) = "provider.login";
        /// The forwarder refused a model request.
        LeaseDenied(LeaseDenied) = "lease.denied";
        /// What one forwarded model request used.
        LeaseUsage(LeaseUsage) = "lease.usage";
        /// The state of an environment.
        EnvStatus(EnvStatus) = "env.status";
        /// What an environment is and has.
        EnvFacts(EnvironmentFacts) = "env.facts";
        /// A new list of an environment's whole tree.
        EnvManifest(EnvManifest) = "env.manifest";
        /// How far a job is.
        JobProgress(JobProgress) = "job.progress";
        /// A known browser changed its state or tab count.
        BrowserState(crate::browser::BrowserState) = "browser.state";
        DisplayState(crate::display::DisplayState) = "display.state";
        /// A shell ended while nobody was attached to it.
        PtyExited(PtyExited) = "pty.exited";
        /// The state a host finds one of its units in.
        NodeRuntime(NodeRuntime) = "node.runtime";
        /// The network policy installed for a runtime, independent of any enrolled unit or thread.
        NodeNetwork(NodeNetwork) = "node.network";
        /// What a host has and can make.
        NodeFacts(NodeFacts) = "node.facts";
        /// Owned images and the epoch of the complete inventory.
        NodeKeptImages(NodeKeptImages) = "node.kept-images";
        /// What a host uses.
        NodeLoad(NodeLoad) = "node.load";
        /// A host opening that its node no longer serves, though nobody closed it.
        PortState(crate::ports::PortState) = "port.state";
        /// One run of an operator's lifecycle hook for a runtime's unit, and how it ended.
        NodeHook(NodeHook) = "node.hook";
    }
}

payloads! {
    /// A measurement that is worthless when it is late: its kind with
    /// its body.
    Report {
        /// What an environment used at one moment.
        StatsSample(foreign::ResourceSample) = "stats.sample";
        /// The processes of an environment at one moment.
        ProcsSnapshot(ProcsSnapshot) = "procs.snapshot";
        /// Current scoped bindings, never a retained command result.
        PortsSnapshot(crate::ports::PortsSnapshot) = "ports.snapshot";
        /// What changed in a directory that is watched.
        FsChanged(FsChanged) = "fs.changed";
        /// Git status after a native file-system change, tied to its watch.
        GitChanged(GitChanged) = "git.changed";
        /// Current shell foreground observations, without terminal bytes.
        TerminalsSnapshot(crate::commands::PtyStatusResult) = "terminals.snapshot";
    }
}

/// Git status from the same native reader used by git.status, never written into the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitChanged {
    /// The stable watch installed by git.status.
    pub watch_id: Id,
    /// The current status, including truthful unavailable reasons.
    pub status: foreign::GitStatus,
}

impl Ev {
    /// The frame for `event` as number `seq` of `stream`.
    pub fn of(
        stream: &Stream,
        seq: Seq,
        at: Time,
        event: &Event,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            stream: stream.to_string(),
            seq,
            at,
            kind: event.kind().as_str().to_owned(),
            body: event.body()?,
        })
    }

    /// What the frame says, when its kind is one of the catalogue and
    /// its body is what that kind defines.
    pub fn event(&self) -> Result<Event, CatalogueError> {
        Event::decode(&self.kind, self.body.clone())
    }
}

impl Eph {
    /// The frame for `report`, taken at `at`, beside `stream`.
    pub fn of(stream: &Stream, at: Time, report: &Report) -> Result<Self, serde_json::Error> {
        Ok(Self {
            stream: stream.to_string(),
            kind: report.kind().as_str().to_owned(),
            at,
            body: report.body()?,
        })
    }

    /// What the frame says, when its kind is one of the catalogue and
    /// its body is what that kind defines.
    pub fn report(&self) -> Result<Report, CatalogueError> {
        Report::decode(&self.kind, self.body.clone())
    }
}

// ---------------------------------------------------------------- a session's stream

/// The body of `session.opened`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpened {
    /// The harness's own name for the conversation.
    pub native_session_ref: String,
    /// Whether it is a conversation the harness already held.
    pub resumed: bool,
}

/// The body of `session.title`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionTitle {
    /// The title.
    pub title: String,
}

/// The body of `session.lost`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLost {
    /// Why.
    pub reason: String,
}

/// The body of `turn.started`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnStarted {
    /// The turn.
    pub turn_id: Id,
    /// The harness's own name for it; `null` where it has none.
    pub native_turn_ref: Option<String>,
}

/// A kind of item an adapter may report. The message a person sent is
/// never one of them: the system makes that item itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmittedItemKind {
    /// What the agent says.
    AgentMessage,
    /// What it thought.
    Reasoning,
    /// What it means to do.
    Plan,
    /// A call to a tool.
    ToolCall,
    /// A mark in the conversation: that the harness shortened it.
    Marker,
    /// Something that went wrong.
    Error,
}

/// The body of `item.started`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(from = "orch_agent_contract::events::ItemStarted")]
pub struct ItemStarted {
    /// The turn the item belongs to.
    pub turn_id: Id,
    /// The adapter's own name for the item, unique in its session.
    pub native_ref: String,
    /// The line of the conversation it is in: the main one, or a
    /// sub-agent's.
    pub stream: String,
    /// The item that owns it, for an item of a sub-agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_native_ref: Option<String>,
    /// What kind of item it is.
    pub kind: EmittedItemKind,
    /// What it holds so far.
    pub payload: foreign::ItemPayload,
}

impl From<orch_agent_contract::events::ItemStarted> for ItemStarted {
    fn from(started: orch_agent_contract::events::ItemStarted) -> Self {
        use foreign::ItemPayload;
        use orch_agent_contract::events::ItemContent;
        let (kind, payload) = match started.content {
            ItemContent::AgentMessage(body) => (
                EmittedItemKind::AgentMessage,
                ItemPayload::AgentMessage(body),
            ),
            ItemContent::Reasoning(body) => {
                (EmittedItemKind::Reasoning, ItemPayload::Reasoning(body))
            }
            ItemContent::Plan(body) => (EmittedItemKind::Plan, ItemPayload::Plan(body)),
            ItemContent::ToolCall(body) => (EmittedItemKind::ToolCall, ItemPayload::ToolCall(body)),
            ItemContent::Marker(body) => (EmittedItemKind::Marker, ItemPayload::Marker(body)),
            ItemContent::Error(body) => (EmittedItemKind::Error, ItemPayload::Error(body)),
        };
        Self {
            turn_id: started.turn_id,
            native_ref: started.native_ref,
            stream: started.stream,
            parent_native_ref: started.parent_native_ref,
            kind,
            payload,
        }
    }
}

/// The part of an item that grows while it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeltaField {
    /// The text of what the agent says, counted in code points.
    Text,
    /// The summary of what it thought, counted in code points.
    Summary,
    /// The output of a command, counted in bytes as the command wrote
    /// them.
    Output,
}

/// The body of `item.delta`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ItemDelta {
    /// The item.
    pub native_ref: String,
    /// The part of it that grew.
    pub field: DeltaField,
    /// How long that part was before, in its own unit.
    pub offset: i64,
    /// How much it grew by, in the same unit.
    pub len: i64,
    /// What was added.
    pub data: String,
}

/// The body of `item.updated`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ItemUpdated {
    /// The item.
    pub native_ref: String,
    /// The members of its payload that changed, each as it now is.
    pub payload: Json,
}

/// The body of `item.completed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ItemCompleted {
    /// The item.
    pub native_ref: String,
    /// How it ended.
    pub status: foreign::ItemStatus,
    /// What it holds for good.
    pub payload: foreign::ItemPayload,
    /// The whole output of a command, where it is too long for the
    /// payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_blob: Option<BlobRef>,
}

/// The body of `approval.requested`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequested {
    /// The turn that waits.
    pub turn_id: Id,
    /// The adapter's own name for the request, unique in its session.
    pub request_ref: String,
    /// What is asked, in a few words.
    pub title: String,
    /// What is asked.
    pub request: foreign::HarnessRequest,
}

/// The body of `approval.closed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalClosed {
    /// The request the harness withdrew.
    pub request_ref: String,
}

/// What a running turn is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhaseKind {
    /// The work itself.
    Working,
    /// Shortening its own conversation, to make room.
    Compacting,
}

/// The body of `turn.phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnPhase {
    /// The turn.
    pub turn_id: Id,
    /// What it is doing now.
    pub kind: TurnPhaseKind,
    /// Since when.
    pub at: Time,
}

/// The body of `usage.updated`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageUpdated {
    /// The turn.
    pub turn_id: Id,
    /// What the turn has used so far.
    pub turn: foreign::TokenUsage,
    /// What the thread has used in all.
    pub thread: foreign::TokenUsage,
    /// How many tokens the model's context holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    /// How many of them the last prompt took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_used: Option<i64>,
    /// What the harness reported beyond the totals: the sub-agents' and each model's share, where it shortens
    /// the conversation, a shortening, and what fills the context by part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<orch_api_contract::context::TurnUsageReport>,
}

/// The body of model.request, using the adapter contract for every harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRequest {
    /// The turn the request belongs to.
    pub turn_id: Id,
    /// Raw observations; all derived arithmetic is shared by the consumers.
    pub request: orch_agent_contract::requests::UsageRequest,
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// By itself.
    Completed,
    /// Because it was stopped.
    Interrupted,
    /// Because something failed.
    Failed,
}

/// Why a turn did not complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnError {
    /// The stable code: one of [`TurnErrorCode`] where one applies.
    pub code: String,
    /// A sentence for whoever reads the thread.
    pub message: String,
    /// Whether the same input may succeed if it is sent again.
    pub retryable: bool,
    /// A refusal's cyber, safety or other cause, or missing model access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The HTTP status actually received, where the harness supplied it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    /// The vendor's requested pause, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<i64>,
    /// The hit ProviderFacts.windows entry's id, where the harness reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// What the harness or its vendor said, word for word, beside the code and the sentence above; kept
    /// whatever the adapter made of it, so that a failure nobody foresaw can still be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// For a limit of the plan: when the vendor says it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Time>,
    /// Whether the answer before this error may have been cut short.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
    /// For a limit: which one, as the adapter read it from the vendor's fields (`scope`: `plan`, `model_family`,
    /// `spending`, `rate`; `window_minutes`, `model_family`, `retry_after_secs` where known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Json>,
}

macro_rules! turn_error_codes {
    ($( $(#[$doc:meta])* $variant:ident = $code:literal; )*) => {
        /// The codes a turn that did not complete is reported with where
        /// they apply.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum TurnErrorCode {
            $( $(#[$doc])* $variant, )*
        }

        impl TurnErrorCode {
            /// Every code.
            pub const ALL: &'static [TurnErrorCode] = &[ $( TurnErrorCode::$variant, )* ];

            /// The code as it is written.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $code, )* }
            }
        }

        impl std::str::FromStr for TurnErrorCode {
            type Err = CatalogueError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $code => Ok(Self::$variant), )*
                    _ => Err(CatalogueError::UnknownKind(text.to_owned())),
                }
            }
        }
    };
}

turn_error_codes! {
    /// The provider's allowance is used up.
    ProviderExhausted = "provider_exhausted";
    /// The provider's credential was refused.
    ProviderUnauthorized = "provider_unauthorized";
    /// The vendor is busy or failed, which passes: the provider keeps its state.
    ProviderBusy = "provider_busy";
    /// The vendor could not be reached, which passes: the provider keeps its state.
    ProviderUnreachable = "provider_unreachable";
    /// The model cannot be used.
    ModelUnavailable = "model_unavailable";
    /// The vendor filter refused this request.
    ModelRefused = "model_refused";
    /// The conversation no longer fits the model's context.
    ContextOverflow = "context_overflow";
    /// The harness cannot reach where its tools run.
    EnvironmentUnreachable = "environment_unreachable";
    /// The harness failed.
    HarnessError = "harness_error";
    /// The harness no longer holds the session.
    SessionLost = "session_lost";
    /// The turn was ended from outside after it did not stop when asked.
    Forced = "forced";
    /// The turn was not taken up in time.
    StartTimeout = "start_timeout";
    /// A cap on use was reached.
    CapReached = "cap_reached";
}

/// The body of `turn.completed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnCompleted {
    /// The turn.
    pub turn_id: Id,
    /// How it ended.
    pub status: TurnOutcome,
    /// Why, when it did not complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TurnError>,
}

/// Actual native cleanup of one exact stopped turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnCommandCleanup {
    /// The stopped turn.
    pub turn_id: Id,
    /// Native factual counts.
    pub result: crate::background::CommandCleanup,
}

/// The body of `orch.tool_call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OrchToolCall {
    /// The turn the call was made in.
    pub turn_id: Id,
    /// The adapter's own name for the call: the same call runs once.
    pub call_ref: String,
    /// One transport attempt at the same call_ref; an explicit retry gets a new attempt, not a new effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Id>,
    /// The tool.
    pub tool: OrchTool,
    /// What it was called with.
    pub arguments: Json,
}

/// What a harness lacks of a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialNeed {
    /// It has none.
    Missing,
    /// The one it has will soon lapse.
    Expiring,
    /// The one it has was refused.
    Rejected,
}

/// The body of `credential.needed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialNeeded {
    /// What the harness lacks.
    pub reason: CredentialNeed,
}

/// The body of `raw.frame`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RawFrame {
    /// The positive number assigned to this captured frame in its segment, unchanged on replay.
    #[serde(default)]
    pub n: i64,
    /// Which way the message went between the adapter and its harness.
    pub direction: String,
    /// The message.
    pub payload: Json,
}

// ---------------------------------------------------------------- machines, harnesses, providers

/// The body of `provider.found`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderFound {
    /// The harness kind that is signed in.
    pub harness_kind: String,
    /// As whom, where the harness says; `null` where it does not.
    pub identity: Option<String>,
}

/// The body of `harness.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessStatus {
    /// The state of the harness process, in the words that state is
    /// written with everywhere.
    pub status: String,
    /// What there is to say about it; `null` when nothing.
    pub detail: Option<String>,
    /// The version that runs; `null` while none is known.
    pub harness_version: Option<String>,
    /// How many sessions it holds open.
    pub sessions_open: i64,
}

/// The body of `provider.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderStatus {
    /// The state of the provider, in the words that state is written
    /// with everywhere.
    pub status: String,
    /// What there is to say about it; `null` when nothing.
    pub detail: Option<String>,
}

/// The body of `lease.denied`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseDenied {
    /// The lease the request ran under.
    pub lease_id: Id,
    /// The model it named.
    pub model: String,
    /// Why it was refused.
    pub reason: String,
}

/// The body of `lease.usage`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseUsage {
    /// The lease the request ran under.
    pub lease_id: Id,
    /// The model it named.
    pub model: String,
    /// What it used.
    pub usage: foreign::TokenUsage,
    /// The model service's own name for the request, where it gave one.
    pub request_ref: Option<String>,
}

// ---------------------------------------------------------------- environments

/// How paths are written on a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PathStyle {
    /// With forward slashes, from one root.
    Posix,
    /// With drive letters and backslashes.
    Windows,
}

/// How a mount is attached to an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountMode {
    /// Nothing is written there.
    ReadOnly,
    /// Writable: the very same files for every environment that has it.
    Shared,
    /// Read-only underneath, with the environment's own changes kept on
    /// top, which nobody else sees.
    Private,
}

/// Whether a mount is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountState {
    /// It is there.
    Attached,
    /// It is not, for the reason given.
    Unavailable,
}

/// Why a mount is not attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountFault {
    /// The machine has no place where the mount's data is.
    NoLocation,
    /// The environment's driver cannot attach mounts.
    Driver,
    /// The directory of the mount's data does not exist on the machine.
    Missing,
    /// The machine's owner does not offer that directory.
    Refused,
    /// Its place collides with the workspace, the home or another mount.
    TargetConflict,
}

/// The shell of an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFactsShell {
    /// Its name.
    pub name: String,
    /// Where its program is.
    pub path: String,
}

/// An executor of an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFactsExecutor {
    /// Its kind.
    pub kind: String,
    /// The build that is there.
    pub version: String,
    /// Whether it can be started.
    pub available: bool,
    /// Why not; `null` when it can.
    pub reason: Option<String>,
    /// What its harness's sandbox can enforce in this environment.
    pub sandbox_modes: Vec<SandboxMode>,
    /// The tool sets it can offer now, for an executor that has several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_sets: Option<Vec<String>>,
}

/// A tool server that a harness in an environment has configured: where
/// it was found, and never how it is started or reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFactsMcp {
    /// The server's name there.
    pub name: String,
    /// The harness kind whose configuration names it.
    pub source: String,
    /// Which of that harness's configurations: its user's own, or one in
    /// a directory of the workspace.
    pub scope: String,
    /// The directory of that configuration; `null` for the user's own.
    pub directory: Option<String>,
    /// How the server is reached.
    pub transport: String,
    /// A value that changes when its definition does.
    pub digest: String,
}

/// What became of a mount an environment was to have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFactsMount {
    /// The mount.
    pub mount_id: Id,
    /// Its name.
    pub name: String,
    /// Whether it is attached.
    pub state: MountState,
    /// Why not; `null` when it is.
    pub reason: Option<MountFault>,
    /// Where it is in the environment; `null` when it is not attached.
    pub path: Option<String>,
    /// How it is attached; `null` when it is not.
    pub mode: Option<MountMode>,
    /// How it was asked for.
    pub asked: MountMode,
    /// Whether the mount itself keeps writes out where none are allowed,
    /// and not only the harness's own sandbox.
    pub enforced: bool,
}

/// Why a workspace is listed live instead of indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndexReason {
    /// Its setting explicitly chose live listing.
    Chosen,
    /// It crossed the entry limit.
    Entries,
    /// The platform could not watch the tree.
    Watches,
    /// One scan took too long.
    ScanTime,
}

/// One hosted server's state, without its definition or any credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFactsMcpServer {
    /// Stable Orch membership when the server was started by a current controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    /// The environment's own server name.
    pub name: String,
    /// stopped, starting, running or failed.
    pub state: String,
    /// On failure, exit and last stderr lines with known secrets masked; otherwise null.
    pub reason: Option<String>,
    /// Changes whenever this server starts again, even when its definition stayed the same.
    #[serde(default)]
    pub revision: u64,
}

/// What an environment is and has, as its connector reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFacts {
    /// The private browser profiles directory, reported from the actual runtime on every machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_profiles_dir: Option<String>,
    /// When browser and display programs were looked for.
    #[serde(default)]
    pub looked_at: Option<Time>,
    /// Browser support discovered where the environment runs.
    #[serde(default)]
    pub browser: EnvironmentFactsBrowser,
    /// Display support discovered where the environment runs.
    #[serde(default)]
    pub vnc: EnvironmentFactsVnc,

    /// Proved reader and carrier transports; old missing facts advertise none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<crate::ports::EnvironmentPortsFacts>,
    /// Its operating system.
    pub os: Os,
    /// Its processor architecture.
    pub arch: Arch,
    /// The shell a person gets there.
    pub shell: EnvironmentFactsShell,
    /// The user its commands run as.
    pub user: String,
    /// That user's home directory.
    pub home: String,
    /// Where its workspace is.
    pub workspace_root: String,
    /// How paths are written there.
    pub path_style: PathStyle,
    /// Whether upper and lower case differ in its file names.
    pub case_sensitive: bool,
    /// The executors it has.
    pub executors: Vec<EnvironmentFactsExecutor>,
    /// The tool servers that the harnesses there have configured.
    pub mcp_found: Vec<EnvironmentFactsMcp>,
    /// Current hosted servers, including failed servers and their masked exit/log explanation.
    #[serde(default)]
    pub mcp_servers: Vec<EnvironmentFactsMcpServer>,
    /// Whether a list of its whole tree is kept.
    pub index: IndexMode,
    /// Why it is live; omitted when indexed and for older connectors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_reason: Option<IndexReason>,
    /// What became of the mounts it was to have.
    pub mounts: Vec<EnvironmentFactsMount>,
    /// Whether a mount can be attached with changes of the environment's
    /// own on top.
    pub private_views: bool,
    /// Whether its agents' processes can be held still.
    pub suspend: bool,
    /// Whether it has `git`.
    pub git: bool,
    /// How many times it was made anew.
    pub generation: i64,
}

/// The body of `env.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvStatus {
    /// The state of the environment, in the words that state is written
    /// with everywhere.
    pub status: String,
    /// What there is to say about it; `null` when nothing.
    pub detail: Option<String>,
    /// How many times it was made anew.
    pub generation: i64,
}

/// The body of `env.manifest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvManifest {
    /// The list of the tree, which the system now holds.
    pub manifest_blob: Hash,
    /// How many files it names.
    pub file_count: i64,
    /// How many bytes they have together.
    pub total_bytes: i64,
}

/// The body of `job.progress`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JobProgress {
    /// The command producing these counters; file work always names its exact owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<Id>,
    /// The job.
    pub job_id: Id,
    /// How far it is.
    pub progress: foreign::JobProgress,
    /// The end of what a build printed, from a host that builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

/// The body of `pty.exited`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyExited {
    /// The shell.
    pub terminal_id: Id,
    /// What it exited with; `null` for a shell its connector no longer
    /// has after a restart.
    pub exit_code: Option<i64>,
    /// How many bytes it printed in all.
    pub bytes_out: i64,
    /// What was kept of its output, where that was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<BlobRef>,
}

/// The body of `procs.snapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProcsSnapshot {
    /// The moment the list was taken.
    pub as_of: Time,
    /// The processes of the environment.
    pub processes: Vec<foreign::ProcessInfo>,
}

/// The body of `fs.changed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FsChanged {
    /// The watch that noticed.
    pub watch_id: Id,
    /// The exact installation that observed these changes; absent for a legacy caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_instance: Option<Id>,
    /// Immutable first-page observation and its interval, only on an initial reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<crate::commands::FsListPageResult>,
    /// A partial directory changed; its old continuation must be restarted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invalidated: bool,
    /// A newly installed watch supplies the initial directory, replacing any older entries.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
    /// The native list exceeded its entry bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The entries of the directory that are new or changed.
    pub changed: Vec<foreign::DirEntry>,
    /// The names of those that are gone.
    pub removed: Vec<String>,
}

// ---------------------------------------------------------------- hosts

/// What a host has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeCapacity {
    /// Its processors.
    pub cpus: i64,
    /// Schedulable processor time when the destination offers fractional processors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis: Option<i64>,
    /// Its memory, in mebibytes.
    pub memory_mb: i64,
    /// Its disk, in gigabytes.
    pub disk_gb: i64,
}

/// A place on a host where volumes are made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeStorage {
    /// Its name.
    pub name: String,
    /// Whether volumes go there when no storage is named.
    pub default: bool,
    /// How much it holds, in gigabytes.
    pub capacity_gb: i64,
    /// How much of that is free.
    pub free_gb: i64,
    /// Whether a mount's private changes can be kept on it.
    pub private_views: bool,
}

/// The body of `node.runtime`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRuntime {
    /// The runtime.
    pub runtime_id: Id,
    /// The state its driver finds the unit in.
    pub state: String,
    /// What there is to say about it; `null` when nothing.
    pub detail: Option<String>,
}

/// Images owned by the node's systems; a changed epoch invalidates a delayed sweep's old whitelist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeKeptImages {
    pub epoch: u64,
    pub images: Vec<orch_driver_contract::kept::KeptImage>,
}

/// The body of `node.network`, sequenced durably on the host node's stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeNetwork {
    /// The runtime reported by its own host.
    pub runtime_id: Id,
    /// What the driver has verified, with safe authorities and denial metadata only.
    pub network: orch_api_contract::environment::NetworkState,
}

/// The moment of a unit's life an operator's lifecycle hook runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// The unit's network exists and the unit is about to start.
    PreUp,
    /// The unit runs.
    PostUp,
    /// The unit is about to stop.
    PreDown,
    /// The unit has stopped.
    PostDown,
}

impl HookEvent {
    /// The event as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreUp => "pre_up",
            Self::PostUp => "post_up",
            Self::PreDown => "pre_down",
            Self::PostDown => "post_down",
        }
    }

    /// Whether a failure of the hook keeps the unit from running: the two up hooks fail closed, the two down
    /// hooks are told and never hold a stop.
    #[must_use]
    pub const fn fails_closed(self) -> bool {
        matches!(self, Self::PreUp | Self::PostUp)
    }
}

/// The body of `node.hook`: one run of an operator's lifecycle hook, written into the history of what the
/// runtime serves. It carries no output of the hook: what a failing up hook printed first goes with the
/// failure of the start it stopped, to whoever manages the environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeHook {
    /// The runtime whose unit the hook ran for.
    pub runtime_id: Id,
    /// The moment it ran at.
    pub event: HookEvent,
    /// Why the unit came up or went down.
    pub reason: orch_driver_contract::runtime::UnitReason,
    /// Whether this is a run again of a hook for a unit the node found running when it started.
    pub replay: bool,
    /// When it began.
    pub at: Time,
    /// How long it ran, in milliseconds.
    pub duration_ms: i64,
    /// Its exit status; `null` when it was killed or could not be run.
    pub exit: Option<i32>,
    /// Whether it ran out of the machine policy's hook timeout and was killed.
    pub timed_out: bool,
    /// Whether the run counts as failed: a non-zero exit, a timeout, or an executable that could not be run.
    pub failed: bool,
}

/// The location a host's mount-size report measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeMountKind {
    /// An existing directory.
    Path,
    /// An owned volume on a host storage.
    Volume,
}

/// A size already known by the host for one exact mount location, without a requested filesystem walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeMountSize {
    /// The mount resource.
    pub mount_id: Id,
    /// Its location kind: path or volume.
    pub kind: NodeMountKind,
    /// The path or volume name this measurement describes.
    pub source: String,
    /// The location's stored storage selector; null for the default storage.
    pub storage: Option<String>,
    /// Known bytes; zero is a measured empty location.
    pub bytes: u64,
}

/// The body of `node.facts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeFacts {
    /// What the host has.
    pub capacity: NodeCapacity,
    /// What each driver it loaded declares, by the driver's name.
    pub drivers: BTreeMap<String, foreign::DriverDescriptor>,
    /// Configuration schemas registered by the program, including drivers that did not load.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub driver_configuration_schemas: BTreeMap<String, Json>,
    /// The places on it where volumes are made.
    pub storages: Vec<NodeStorage>,
    /// Optional sizes the host already knows; an absent entry is unknown, never a request to scan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mount_sizes: Vec<NodeMountSize>,
}

/// The body of `node.load`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeLoad {
    /// How busy its processors are: 100 is one processor fully used.
    pub cpu_percent: f64,
    /// Its memory in use, in mebibytes.
    pub memory_used_mb: i64,
    /// Its disk in use, in gigabytes.
    pub disk_used_gb: i64,
    /// Its disk that is free, in gigabytes.
    pub disk_free_gb: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    const A: &str = "0199c1a0-0000-7000-8000-00000000000a";
    const B: &str = "0199c1a0-0000-7000-8000-00000000000b";

    fn id(text: &str) -> Id {
        text.parse().unwrap()
    }

    fn time() -> Time {
        "2026-10-08T17:04:05.000Z".parse().unwrap()
    }

    fn round_trip<T: Serialize + DeserializeOwned>(example: Json) -> T {
        let read: T = serde_json::from_value(example.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), example);
        read
    }

    /// Reads `body` as an event of `kind`, and expects it written back the same.
    fn event(kind: &str, body: Json) -> Event {
        let event = Event::decode(kind, body.clone()).unwrap();
        assert_eq!(event.kind().as_str(), kind);
        assert_eq!(event.body().unwrap(), body);
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({ "k": kind, "b": body })
        );
        event
    }

    fn environment_facts() -> Json {
        json!({
            "looked_at": null,
            "browser": {"programs":[],"window":false,"playwright_cli":null,"sandbox":null},
            "vnc": {"servers":[],"window_manager":null},
            "os": "linux", "arch": "x86_64",
            "shell": { "name": "bash", "path": "/usr/bin/bash" },
            "user": "agent", "home": "/home/agent",
            "workspace_root": "/workspace",
            "path_style": "posix", "case_sensitive": true,
            "executors": [ { "kind": "codex-exec-server", "version": "0.160.1", "available": true, "reason": null,
                             "sandbox_modes": ["open"] },
                           { "kind": "orch-exec", "version": "0.1.0", "available": true, "reason": null,
                             "sandbox_modes": ["open"],
                             "tool_sets": ["claude-code", "plain", "custom"] } ],
            "mcp_found": [ { "name": "github", "source": "codex", "scope": "user", "directory": null,
                             "transport": "stdio", "digest": "9f2c" } ],
            "mcp_servers": [],
            "index": "full",
            "mounts": [ { "mount_id": A, "name": "src", "state": "attached", "reason": null,
                          "path": "/data/src", "mode": "private", "asked": "private", "enforced": true } ],
            "private_views": true,
            "suspend": true,
            "git": true, "generation": 3
        })
    }

    #[test]
    fn a_host_can_report_an_empty_mount_without_turning_an_unreported_size_into_zero() {
        let absent = json!({"capacity":{"cpus":4,"memory_mb":8192,"disk_gb":100},"drivers":{},"storages":[]});
        let mut facts: NodeFacts = serde_json::from_value(absent.clone()).unwrap();
        assert!(facts.mount_sizes.is_empty());
        assert_eq!(
            serde_json::to_value(&facts).unwrap(),
            absent,
            "older hosts make no size claim"
        );
        facts.mount_sizes.push(NodeMountSize {
            mount_id: id(A),
            kind: NodeMountKind::Path,
            source: "/srv/data".into(),
            storage: None,
            bytes: 0,
        });
        let reported = serde_json::to_value(&facts).unwrap();
        assert_eq!(reported["mount_sizes"][0]["bytes"], 0);
        assert_eq!(reported["mount_sizes"][0]["source"], "/srv/data");
        assert_eq!(
            serde_json::from_value::<NodeFacts>(reported).unwrap(),
            facts
        );
    }

    #[test]
    fn what_a_session_does_reads_and_writes_as_the_catalogue_has_it() {
        event(
            "session.opened",
            json!({ "native_session_ref": "sess_1", "resumed": false }),
        );
        event("session.title", json!({ "title": "Fix the login" }));
        event(
            "session.lost",
            json!({ "reason": "the harness was restarted" }),
        );
        event(
            "turn.started",
            json!({ "turn_id": A, "native_turn_ref": "turn_7" }),
        );
        event(
            "item.started",
            json!({
                "turn_id": A, "native_ref": "i9", "stream": "main", "kind": "agent_message",
                "payload": { "text": "", "text_len": 0, "phase": null }
            }),
        );
        event(
            "item.started",
            json!({
                "turn_id": A, "native_ref": "i12", "stream": "sub:child-1", "parent_native_ref": "i10", "kind": "tool_call",
                "payload": { "tool": { "kind":"other","name":"inspect","arguments":null,"result":null },
                    "title":"Inspect","started_at":"2026-10-08T17:04:05.000Z","duration_ms":null,
                    "raw":{"name":"inspect","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false} }
            }),
        );
        event(
            "item.delta",
            json!({ "native_ref": "i9", "field": "text", "offset": 0, "len": 18, "data": "I'll start by read" }),
        );
        event(
            "item.updated",
            json!({ "native_ref": "i10", "payload": { "process_id": "p1" } }),
        );
        event(
            "item.completed",
            json!({
                "native_ref": "i11", "status": "completed", "payload": {
                    "tool": { "kind":"other","name":"inspect","arguments":null,"result":null },
                    "title":"Inspect","started_at":"2026-10-08T17:04:05.000Z","duration_ms":10,
                    "raw":{"name":"inspect","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false} },
                "output_blob": { "hash": Hash::of(b"output").to_string(), "size": 91000 }
            }),
        );
        event(
            "approval.requested",
            json!({
                "turn_id": A, "request_ref": "r1", "title": "Run cargo test",
                "request": { "request_kind": "command", "options": [], "detail": { "command": "cargo test", "cwd": "/workspace", "reason": null } }
            }),
        );
        event("approval.closed", json!({ "request_ref": "r1" }));
        event(
            "turn.phase",
            json!({ "turn_id": A, "kind": "compacting", "at": "2026-10-08T17:04:05.000Z" }),
        );
        event(
            "usage.updated",
            json!({
                "turn_id": A, "turn": { "input_tokens": 10, "cached_input_tokens":0,"output_tokens":2,"reasoning_tokens":0,"total_tokens": 12 },
                "thread": { "input_tokens": 90, "cached_input_tokens":0,"output_tokens":30,"reasoning_tokens":0,"total_tokens": 120 },
                "context_window": 400000, "context_used": 51000
            }),
        );
        event(
            "orch.tool_call",
            json!({ "turn_id": A, "call_ref": "c1", "tool": "announce", "arguments": { "title": "Root cause" } }),
        );
        event("credential.needed", json!({ "reason": "expiring" }));
        event(
            "raw.frame",
            json!({ "n": 41, "direction": "in", "payload": { "method": "turn/started" } }),
        );
    }

    #[test]
    fn a_turn_ends_completed_or_says_why_it_did_not() {
        event(
            "turn.completed",
            json!({ "turn_id": A, "status": "completed" }),
        );
        let failed = event(
            "turn.completed",
            json!({
                "turn_id": A, "status": "failed",
                "error": { "code": "context_overflow", "message": "the conversation no longer fits", "retryable": false }
            }),
        );
        let Event::TurnCompleted(failed) = failed else {
            panic!("not a completed turn")
        };
        let error = failed.error.unwrap();
        assert_eq!(
            error.code.parse::<TurnErrorCode>().unwrap(),
            TurnErrorCode::ContextOverflow
        );
        assert_eq!(
            (error.said, error.resets_at, error.incomplete),
            (None, None, false),
            "an error from a sender that says nothing of the vendor's words reads as one that has none"
        );
        let limit = event(
            "turn.completed",
            json!({
                "turn_id": A, "status": "failed",
                "error": { "code": "provider_exhausted", "message": "the weekly limit is reached", "retryable": false,
                           "said": "You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)",
                           "resets_at": "2026-10-14T09:00:00.000Z", "incomplete": true }
            }),
        );
        let Event::TurnCompleted(limit) = limit else {
            panic!("not a completed turn")
        };
        let error = limit.error.unwrap();
        assert_eq!(
            error.said.as_deref(),
            Some("You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)")
        );
        assert!(error.resets_at.is_some() && error.incomplete);
        for code in ["provider_busy", "provider_unreachable"] {
            assert!(code.parse::<TurnErrorCode>().is_ok(), "{code}");
        }
        for code in TurnErrorCode::ALL {
            assert_eq!(serde_json::to_value(code).unwrap(), code.as_str());
        }
    }

    #[test]
    fn a_missing_or_legacy_null_failure_window_is_omitted_but_a_known_window_stays()
    -> Result<(), Box<dyn std::error::Error>> {
        let canonical = json!({"code":"provider_exhausted","message":"The plan allowance is used","retryable":true});
        for body in [
            canonical.clone(),
            json!({"code":"provider_exhausted","message":"The plan allowance is used","retryable":true,"window":null}),
        ] {
            let error: TurnError = serde_json::from_value(body)?;
            assert!(error.window.is_none());
            assert_eq!(serde_json::to_value(error)?, canonical);
        }
        let known = json!({"code":"provider_exhausted","message":"The plan allowance is used","retryable":true,"window":"weekly"});
        let error: TurnError = serde_json::from_value(known.clone())?;
        assert_eq!(error.window.as_deref(), Some("weekly"));
        assert_eq!(serde_json::to_value(error)?, known);
        Ok(())
    }

    #[test]
    fn what_machines_harnesses_and_providers_report_reads_and_writes_as_the_catalogue_has_it() {
        event(
            "provider.found",
            json!({ "harness_kind": "codex", "identity": "alice@example.com" }),
        );
        event(
            "harness.status",
            json!({ "status": "online", "detail": null, "harness_version": "0.160.1", "sessions_open": 3 }),
        );
        event(
            "harness.descriptor",
            serde_json::to_value(serde_json::from_value::<foreign::AgentDescriptor>(json!({
                "kind":"codex","adapter_version":1,
                "harness":{"name":"Codex","version":"0.160.1","supported_range":"*","status":"supported"},
                "arrangements":[]
            })).unwrap()).unwrap(),
        );
        event(
            "provider.status",
            json!({ "status": "exhausted", "detail": "the weekly limit is reached" }),
        );
        event(
            "provider.facts",
            serde_json::to_value(foreign::ProviderFacts {
                plan: Some("pro".to_owned()),
                ..Default::default()
            })
            .unwrap(),
        );
        event(
            "provider.login",
            json!({ "state": "awaiting_user", "method": "device_code", "shows": [], "asks": null,"expires_at":null,"error":null }),
        );
        event(
            "lease.denied",
            json!({ "lease_id": A, "model": "gpt-6-sol", "reason": "model_not_allowed" }),
        );
        event(
            "lease.usage",
            json!({ "lease_id": A, "model": "gpt-6-sol", "usage": { "input_tokens":800,"cached_input_tokens":0,
                "output_tokens":12,"reasoning_tokens":0,"total_tokens": 812 }, "request_ref": "resp_1" }),
        );
    }

    #[test]
    fn what_environments_report_reads_and_writes_as_the_catalogue_has_it() {
        event(
            "env.status",
            json!({ "status": "running", "detail": null, "generation": 3 }),
        );
        event("env.facts", environment_facts());
        event(
            "env.manifest",
            json!({ "manifest_blob": Hash::of(b"tree").to_string(), "file_count": 4120, "total_bytes": 88123456 }),
        );
        event(
            "job.progress",
            json!({ "job_id": A, "progress": { "phase": "copy", "percent": 40,"phases":[],
                "files_done":4,"files_total":10,"bytes_done":40,"bytes_total":100,"rate_bps":null,"eta_seconds":null,"message":null } }),
        );
        event(
            "pty.exited",
            json!({
                "terminal_id": A, "exit_code": 0, "bytes_out": 5120, "kept": { "hash": Hash::of(b"screen").to_string(), "size": 5120 }
            }),
        );
        event(
            "pty.exited",
            json!({ "terminal_id": A, "exit_code": null, "bytes_out": 0 }),
        );
    }

    #[test]
    fn what_hosts_report_reads_and_writes_as_the_catalogue_has_it() {
        event(
            "node.runtime",
            json!({ "runtime_id": A, "state": "running", "detail": null }),
        );
        event(
            "node.facts",
            json!({
                "capacity": { "cpus": 16, "memory_mb": 65536, "disk_gb": 500 },
                "drivers": { "docker": serde_json::to_value(serde_json::from_value::<foreign::DriverDescriptor>(json!({
                    "name":"docker","title":"Containers","version":"1","live_changes":{}
                })).unwrap()).unwrap() },
                "storages": [ { "name": "fast", "default": true, "capacity_gb": 500, "free_gb": 310, "private_views": true } ]
            }),
        );
        event(
            "node.load",
            json!({ "cpu_percent": 37.5, "memory_used_mb": 20480, "disk_used_gb": 190, "disk_free_gb": 310 }),
        );
        event(
            "job.progress",
            json!({ "job_id": A, "progress": { "phase": "image", "percent": 12,"phases":[],
                "files_done":0,"files_total":null,"bytes_done":12,"bytes_total":100,"rate_bps":null,"eta_seconds":null,"message":null }, "log": "Step 3/9" }),
        );
    }

    #[test]
    fn measurements_read_and_write_as_the_catalogue_has_them() {
        let stream = Stream::Env {
            environment_id: id(A),
        };
        for (kind, body) in [
            (
                "stats.sample",
                json!({ "at": "2026-10-08T17:04:05.000Z", "cpu_percent": 12.5,"cpu_limit_millis":null,
                    "memory_bytes":4096,"memory_limit_bytes":null,"disk_used_bytes":null,"disk_limit_bytes":null,
                    "net_rx_bps":0,"net_tx_bps":0,"pids": 41 }),
            ),
            (
                "procs.snapshot",
                json!({ "as_of": "2026-10-08T17:04:05.000Z", "processes": [ { "pid": 4711,"ppid":1,"name": "cargo",
                    "command":"cargo test","user":"agent","state":"running","cpu_percent":12.5,"memory_bytes":4096,
                    "started_at":"2026-10-08T17:04:05.000Z","thread_id":null,"item_id":null } ] }),
            ),
            (
                "fs.changed",
                json!({ "watch_id": B, "changed": [ { "name": "main.rs","path":"main.rs","type": "file","size":7,
                    "mtime":"2026-10-08T17:04:05.000Z","mode":420,"link_target":null,"cached":false } ], "removed": ["old.rs"] }),
            ),
        ] {
            let report = Report::decode(kind, body.clone()).unwrap();
            assert_eq!(report.body().unwrap(), body);
            let frame = Eph::of(&stream, time(), &report).unwrap();
            assert_eq!(frame.kind, kind);
            assert_eq!(frame.report().unwrap(), report);
            assert_eq!(report.kind().durability(), Durability::Ephemeral);
        }
    }

    #[test]
    fn an_environment_reports_a_mount_it_could_not_attach_with_the_reason() {
        let mut facts = environment_facts();
        facts["mounts"] = json!([ { "mount_id": A, "name": "data", "state": "unavailable", "reason": "refused",
                                    "path": null, "mode": null, "asked": "shared", "enforced": false } ]);
        let facts: EnvironmentFacts = round_trip(facts);
        assert_eq!(facts.mounts[0].reason, Some(MountFault::Refused));
        assert_eq!(facts.mounts[0].mode, None);
    }

    #[test]
    fn a_frame_carries_an_event_with_its_place_in_its_stream() {
        let stream = Stream::Session {
            thread_id: id(A),
            segment_id: id(B),
        };
        let event = Event::SessionTitle(SessionTitle {
            title: "Fix login".into(),
        });
        let frame = Ev::of(&stream, 41, time(), &event).unwrap();
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            json!({ "s": format!("session:{A}:{B}"), "q": 41, "at": "2026-10-08T17:04:05.000Z",
                    "k": "session.title", "b": { "title": "Fix login" } })
        );
        assert_eq!(frame.event().unwrap(), event);
    }

    #[test]
    fn an_event_of_a_kind_nobody_knows_still_has_its_number_and_can_be_passed_over() {
        let frame: Ev = serde_json::from_value(json!({
            "s": "machine", "q": 7, "at": "2026-10-08T17:04:05.000Z", "k": "machine.mood", "b": { "mood": "fine" }
        }))
        .unwrap();
        assert_eq!(frame.seq, 7);
        assert!(
            matches!(frame.event(), Err(CatalogueError::UnknownKind(kind)) if kind == "machine.mood")
        );
    }

    #[test]
    fn a_body_that_is_not_its_kinds_is_told_apart_from_an_unknown_kind() {
        let wrong = Event::decode(
            "turn.completed",
            json!({ "turn_id": A, "status": "paused" }),
        );
        assert!(matches!(
            wrong,
            Err(CatalogueError::Body {
                kind: "turn.completed",
                ..
            })
        ));
        assert!(matches!(
            Event::decode("stats.sample", json!({})),
            Err(CatalogueError::UnknownKind(_))
        ));
    }

    #[test]
    fn every_kind_is_found_again_by_the_name_it_is_written_with() {
        for kind in EventKind::ALL {
            assert_eq!(kind.as_str().parse::<EventKind>().unwrap(), *kind);
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }

    #[test]
    fn each_kind_travels_on_the_streams_of_what_it_reports_about() {
        let session = Stream::Session {
            thread_id: id(A),
            segment_id: id(B),
        };
        let environment = Stream::Env {
            environment_id: id(A),
        };
        assert!(EventKind::ItemDelta.travels_on(&session));
        assert!(!EventKind::ItemDelta.travels_on(&environment));
        assert!(EventKind::JobProgress.travels_on(&environment));
        assert!(EventKind::JobProgress.travels_on(&Stream::Node));
        assert!(!EventKind::JobProgress.travels_on(&Stream::Machine));
        assert!(EventKind::ProviderFound.travels_on(&Stream::Machine));
        assert!(EventKind::PtyExited.travels_on(&environment));
    }

    #[test]
    fn only_the_kinds_that_begin_as_diagnostics_give_way_to_every_other_event() {
        let diagnostics: Vec<_> = EventKind::ALL
            .iter()
            .filter(|kind| crate::streams::is_diagnostic(kind.as_str()))
            .collect();
        assert_eq!(diagnostics, [&EventKind::RawFrame]);
    }

    #[test]
    fn a_hook_run_is_told_on_the_node_stream_with_its_event_reason_and_exit_and_nothing_it_printed()
    {
        let body = json!({
            "runtime_id": A, "event": "pre_up", "reason": "recreate", "replay": false,
            "at": "2026-10-14T15:00:00.000Z", "duration_ms": 40, "exit": 3, "timed_out": false, "failed": true
        });
        let decoded = Event::decode("node.hook", body.clone()).unwrap();
        let Event::NodeHook(hook) = &decoded else {
            panic!("a hook run reads as one: {decoded:?}");
        };
        assert_eq!(hook.event, HookEvent::PreUp);
        assert!(hook.event.fails_closed() && !HookEvent::PreDown.fails_closed());
        assert_eq!(
            hook.reason,
            orch_driver_contract::runtime::UnitReason::Recreate
        );
        assert_eq!(decoded.body().unwrap(), body);
        assert!(EventKind::NodeHook.travels_on(&Stream::Node));
        assert!(!EventKind::NodeHook.travels_on(&Stream::Machine));
    }
}
