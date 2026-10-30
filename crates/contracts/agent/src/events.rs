//! Neutral harness events: what was said, and each tool call both as Orch understood it, by kind, and as the harness made it.
//!
//! Everything a harness session does reaches Orch as a stream of these events, in the order they happened,
//! each applied idempotently by whoever receives it ([`SessionEvent`]). The vocabulary is neutral: nothing
//! of a harness's own protocol shows in it, and what an adapter does not recognise is kept, whole, as a tool
//! call of kind `other` or as a raw frame.
//!
//! **An item** is one step of the conversation: what the agent said, its reasoning, its plan, a tool call,
//! a marker, an error. An adapter never emits what a person said: the engine writes that item when the
//! input is delivered, and the adapter suppresses the harness's echo of it.
//!
//! **A tool call is reported twice over, and neither form is a summary.** [`ToolPayload`] is the call as the
//! adapter understood it, by the kind of thing it was (a command, a change to files, a read, a search of
//! files, a search or a fetch on the web, an image, an MCP call, a sub-agent, one of Orch's own tools), for
//! a display made for that kind. [`RawCall`] is the same call as the harness made it: its own name for the
//! tool, its arguments as JSON text, and what came back as text. An adapter gives the most particular kind
//! it can and never drops the raw form for having done so.
//!
//! Growing text arrives as deltas ([`ItemDelta`]) and is checked against what was received
//! ([`delta_fit`]); a long output keeps its last 16 KiB inline ([`OutputRef`]).

use orch_types::{Hash, Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::credentials::CredentialNeed;

/// How much of the end of a long text stays in the timeline: 16 KiB, which a screen shows at a glance and
/// a row carries without cost. The whole is kept as a blob.
pub use orch_api_contract::threads::OUTPUT_INLINE_MAX_BYTES;
/// How much of a command's output is kept at most: 64 MiB, beyond which the middle is dropped and the
/// payload says so.
pub const OUTPUT_MAX_BYTES: i64 = 64 * 1024 * 1024;
/// How large a diff may be: 256 KiB, past which it is cut and the file is opened whole instead.
pub const FILE_CHANGE_DIFF_MAX_BYTES: i64 = 256 * 1024;
/// How many matches of a file search a payload lists: the first 200.
pub const FILE_SEARCH_MAX_MATCHES: u32 = 200;

/// The kinds of error code a turn can end with where they apply.
pub mod turn_error {
    /// The provider's usage limit is reached; the provider also becomes `exhausted`.
    pub const PROVIDER_EXHAUSTED: &str = "provider_exhausted";
    /// The upstream refused the credential; the provider becomes `error`.
    pub const PROVIDER_UNAUTHORIZED: &str = "provider_unauthorized";
    /// What passes: throttling that is not the plan's, an overloaded vendor, a vendor that failed. The
    /// provider keeps its state and the thread is not blocked.
    pub const PROVIDER_BUSY: &str = "provider_busy";
    /// What passes: a request that timed out, a connection that dropped or was refused, a certificate the
    /// runtime does not trust. The provider keeps its state and the thread is not blocked.
    pub const PROVIDER_UNREACHABLE: &str = "provider_unreachable";
    /// The model is unknown to the upstream.
    pub const MODEL_UNAVAILABLE: &str = "model_unavailable";
    /// The vendor's filter refused this request without rejecting the credential.
    pub const MODEL_REFUSED: &str = "model_refused";
    /// The conversation no longer fits the model's context.
    pub const CONTEXT_OVERFLOW: &str = "context_overflow";
    /// The environment could not be reached.
    pub const ENVIRONMENT_UNREACHABLE: &str = "environment_unreachable";
    /// The harness failed.
    pub const HARNESS_ERROR: &str = "harness_error";
    /// The session was lost.
    pub const SESSION_LOST: &str = "session_lost";
    /// The engine ended the turn itself after an interrupt was not answered.
    pub const FORCED: &str = "forced";
    /// The turn was not accepted in time.
    pub const START_TIMEOUT: &str = "start_timeout";
    /// A usage cap was reached.
    pub const CAP_REACHED: &str = "cap_reached";
    /// Not a failure: the harness ran the turn on another model than the one chosen, and the turn says so.
    pub const MODEL_REROUTED: &str = "model_rerouted";
}

// ---------------------------------------------------------------------------------------------------------
// Usage

/// Raw per-request reports are owned by the adapter contract.
pub use crate::requests::UsageRequest;
/// The API owns the usage vocabulary and its arithmetic.
pub use orch_api_contract::threads::{TokenUsage, TokensReportedApart};
/// The timing vocabulary is shared by every adapter, independent of its provider.
pub use orch_api_contract::usage::{RequestTiming, UsageMeasurement};
/// Shared arithmetic for every adapter's neutral observations, including conformance checks.
pub fn request_timing(request: &UsageRequest) -> RequestTiming {
    request.measured()
}
/// What a harness reports beyond a turn's totals, and what fills its context, as the API keeps them.
pub use orch_api_contract::context::{
    ContextCount, ContextFill, ContextPart, ContextPartKind, ContextRow, ContextRowKind,
    ContextShortening, ModelTokens, StreamTokens, TurnUsageReport,
};

// ---------------------------------------------------------------------------------------------------------
// Long text, file changes, raw calls

/// The API owns long output and its bounded UTF-8 tail operations.
pub use orch_api_contract::threads::OutputRef;

/// What a change did to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    /// The file was made.
    Add,
    /// The file was changed.
    Modify,
    /// The file was removed.
    Delete,
    /// The file was renamed.
    Rename,
}

/// The change of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChange {
    /// The file's path.
    pub path: String,
    /// What was done to it.
    pub change: FileChangeKind,
    /// Its path before, for a rename.
    pub old_path: Option<String>,
    /// The change as a unified diff with three lines of context; `null` for a file that is no text and
    /// while the change is still being written.
    pub diff: Option<String>,
    /// Lines added.
    pub added: i64,
    /// Lines removed.
    pub removed: i64,
    /// Whether it is no text, so that it is shown by its size before and after.
    pub binary: bool,
    /// Whether the diff was cut at [`FILE_CHANGE_DIFF_MAX_BYTES`]: the file opens whole in the code view.
    pub truncated: bool,
}

/// How many lines a unified diff adds and removes: those that begin with one `+` or one `-`, the file
/// headers (`+++`, `---`) not counted.
#[must_use]
pub fn count_diff_lines(diff: &str) -> (i64, i64) {
    let (mut added, mut removed) = (0, 0);
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

/// The raw call is the same type an API reader receives.
pub use orch_api_contract::threads::RawCall;

/// What a command was for, where the harness says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandAction {
    /// A read of a file.
    Read {
        /// The file.
        path: String,
    },
    /// A listing of a directory.
    List {
        /// The directory, if one was named.
        path: Option<String>,
    },
    /// A search.
    Search {
        /// What was looked for.
        query: String,
        /// Where, if a place was named.
        path: Option<String>,
    },
    /// Anything else.
    Other,
}

/// What a search of files looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileSearchMode {
    /// A pattern in the files.
    Content,
    /// A pattern of names.
    Name,
}

/// A match of a search in files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileSearchMatch {
    /// The file.
    pub path: String,
    /// The line, for a search in content.
    pub line: Option<i64>,
    /// The text of the line.
    pub text: Option<String>,
}

/// A result of a search of the web.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchResult {
    /// Its title.
    pub title: String,
    /// Its address.
    pub url: String,
}

/// Where a sub-agent stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubagentState {
    /// At work.
    Running,
    /// Done.
    Completed,
    /// Failed.
    Failed,
    /// Stopped.
    Interrupted,
}

/// The tools that Orch itself gives an agent, in the server named `orch`. [`OrchTool::ALL`] is the list of them,
/// and the one place that says how many there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrchTool {
    /// Lists what was shared with the agent.
    ListShared,
    /// Reads something that was shared.
    ReadShared,
    /// Searches what was shared.
    SearchShared,
    /// Copies something that was shared into the workspace.
    DownloadShared,
    /// Makes or changes a document.
    EditDocument,
    /// Organizes documents and folders; only deletion requires a preview and confirmation.
    ManageShared,
    /// Posts to people in a chat or queues a note to another thread's agent, according to its typed target.
    SendMessage,
    /// Historical chat calls retained for reading stored events; not an offered tool.
    SendChatMessage,
    /// Historical thread calls retained for reading stored events; not an offered tool.
    SendThreadMessage,
    /// Tells everybody on the work about something.
    Announce,
    /// Asks for a capability of the environment.
    RequestCapability,
    /// Opens, joins, announces or closes a browser in the agent's environment.
    UseBrowser,
    /// Opens, announces or closes a graphical display in the agent's environment.
    UseDisplay,
    /// Lists the ports of the agent's environment, their outside addresses and who may reach them.
    ListPorts,
    /// Asks for a port of the agent's environment to be opened to more people, which a person decides.
    OpenPort,
    /// Closes an opening of a port that the thread asked for.
    ClosePort,
    /// Lists who reached a port of the agent's environment, with both ends of each connection.
    ListPortConnections,
    /// Reads the thread's goal, and reports where the agent stands on it; given only to a thread that has one.
    Goal,
}

impl OrchTool {
    /// All of them, in their fixed order: the order the server lists them in.
    pub const ALL: &[Self] = &[
        Self::ListShared,
        Self::ReadShared,
        Self::SearchShared,
        Self::DownloadShared,
        Self::EditDocument,
        Self::ManageShared,
        Self::SendMessage,
        Self::Announce,
        Self::RequestCapability,
        Self::UseBrowser,
        Self::UseDisplay,
        Self::ListPorts,
        Self::OpenPort,
        Self::ClosePort,
        Self::ListPortConnections,
        Self::Goal,
    ];

    /// The tool's name as the server lists it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ListShared => "list_shared",
            Self::ReadShared => "read_shared",
            Self::SearchShared => "search_shared",
            Self::DownloadShared => "download_shared",
            Self::EditDocument => "edit_document",
            Self::ManageShared => "manage_shared",
            Self::SendMessage => "send_message",
            Self::SendChatMessage => "send_chat_message",
            Self::SendThreadMessage => "send_thread_message",
            Self::Announce => "announce",
            Self::RequestCapability => "request_capability",
            Self::UseBrowser => "use_browser",
            Self::UseDisplay => "use_display",
            Self::ListPorts => "list_ports",
            Self::OpenPort => "open_port",
            Self::ClosePort => "close_port",
            Self::ListPortConnections => "list_port_connections",
            Self::Goal => "goal",
        }
    }

    /// The tool of that name, if it is one of them.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            // Native histories may contain calls made before the single message interface.
            "send_chat_message" => Some(Self::SendChatMessage),
            "send_thread_message" => Some(Self::SendThreadMessage),
            _ => Self::ALL.iter().copied().find(|tool| tool.name() == name),
        }
    }
}

/// A tool call as Orch understood it, in a form that lets the display suit the kind of call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolPayload {
    /// A command of a shell.
    Command {
        /// The command line.
        command: String,
        /// Where it ran.
        cwd: String,
        /// How it ended; `null` while it runs.
        exit_code: Option<i64>,
        /// What it printed.
        output: OutputRef,
        /// Its process, where the harness names one.
        process_id: Option<String>,
        /// Whether the harness left it running and went on.
        background: bool,
        /// What it was for, where the harness says.
        actions: Vec<CommandAction>,
    },
    /// A change of files.
    FileChange {
        /// The changes.
        changes: Vec<FileChange>,
    },
    /// A read of a file.
    FileRead {
        /// The file's path: relative to the thread's directory where it lies in it, else absolute.
        path: String,
        /// The first line read.
        start_line: Option<i64>,
        /// The last line read.
        end_line: Option<i64>,
        /// How many lines the file has.
        total_lines: Option<i64>,
        /// What the agent was given: the text, or for an image its type.
        content: OutputRef,
        /// The type of what was read, for an image.
        media_type: Option<String>,
    },
    /// A search in files.
    FileSearch {
        /// What the search looks at.
        mode: FileSearchMode,
        /// The pattern.
        query: String,
        /// Under which directory.
        path: Option<String>,
        /// The first [`FILE_SEARCH_MAX_MATCHES`] matches.
        matches: Vec<FileSearchMatch>,
        /// How many there were.
        total: Option<i64>,
        /// Whether more were found than are listed.
        truncated: bool,
    },
    /// A call of a tool of an MCP server.
    Mcp {
        /// The server.
        server: String,
        /// The tool.
        tool: String,
        /// What it was called with.
        arguments: Json,
        /// What it returned.
        result: Option<Json>,
        /// What went wrong.
        error: Option<String>,
    },
    /// A search of the web.
    WebSearch {
        /// What was searched for.
        query: String,
        /// What was found.
        results: Vec<WebSearchResult>,
    },
    /// A fetch of a page.
    WebFetch {
        /// The address.
        url: String,
        /// The page's title.
        title: Option<String>,
        /// The HTTP status.
        status: Option<u16>,
        /// What came back.
        content: OutputRef,
    },
    /// An image that the agent looked at or made.
    Image {
        /// The image, by its hash.
        blob: Hash,
        /// Its type.
        media_type: String,
        /// Its name.
        name: Option<String>,
        /// What it shows.
        caption: Option<String>,
    },
    /// Work handed to a sub-agent.
    Subagent {
        /// The stream of the sub-agent's own items: `sub:<id>`.
        child_stream: String,
        /// Its name.
        name: Option<String>,
        /// Its role.
        role: Option<String>,
        /// What it was asked to do.
        prompt: Option<String>,
        /// Where it stands.
        state: SubagentState,
        /// The model it runs on, where its program says, as the program names it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// A tool that Orch itself gives the agent.
    Orch {
        /// Which.
        tool: OrchTool,
        /// What it was called with.
        arguments: Json,
        /// What it returned.
        result: Option<Json>,
    },
    /// Anything the adapter does not recognise, kept whole.
    Other {
        /// The tool's name.
        name: String,
        /// What it was called with.
        arguments: Option<Json>,
        /// What it returned.
        result: Option<Json>,
    },
}

// ---------------------------------------------------------------------------------------------------------
// Items

/// The kinds of item an adapter emits. It never emits what a person said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// What the agent said.
    AgentMessage,
    /// Its reasoning.
    Reasoning,
    /// Its plan.
    Plan,
    /// A tool call.
    ToolCall,
    /// A line of the thread's history; an adapter emits only `compacted`.
    Marker,
    /// An error.
    Error,
}

/// Where an item stands, for the output of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// It is being written.
    InProgress,
    /// It is done.
    Completed,
    /// It failed.
    Failed,
    /// It was stopped.
    Cancelled,
}

impl ItemStatus {
    /// Whether it can no longer change.
    #[must_use]
    pub const fn is_final(self) -> bool {
        !matches!(self, Self::InProgress)
    }
}

/// How far into its work an agent's message was said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessagePhase {
    /// Said on the way.
    Commentary,
    /// The answer.
    Final,
}

/// What the agent said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentMessagePayload {
    /// What was said, as Markdown.
    pub text: String,
    /// Its length so far, in Unicode code points.
    pub text_len: i64,
    /// How far into its work.
    pub phase: Option<AgentMessagePhase>,
}

/// The agent's reasoning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningPayload {
    /// A summary of it.
    pub summary: String,
    /// Its length so far, in Unicode code points.
    pub text_len: i64,
    /// True when the upstream withheld the full reasoning.
    pub hidden: bool,
}

/// How far a step of a plan has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    /// Not begun.
    Pending,
    /// Under way.
    InProgress,
    /// Done.
    Completed,
}

/// One step of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanStep {
    /// What it is.
    pub text: String,
    /// How far it has come.
    pub status: PlanStepStatus,
}

/// The agent's plan: one item for each turn, updated in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanPayload {
    /// What the plan is about.
    pub explanation: Option<String>,
    /// Its steps.
    pub steps: Vec<PlanStep>,
}

/// A call of a tool, both ways.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallPayload {
    /// The call as Orch understood it, for a display made for its kind.
    pub tool: ToolPayload,
    /// A line to show for it.
    pub title: String,
    /// When it began.
    pub started_at: Time,
    /// How long it took, once it ended.
    pub duration_ms: Option<i64>,
    /// The same call exactly as the harness made it, whole.
    pub raw: RawCall,
}

/// An error that happened in the harness's work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorPayload {
    /// A code; the ones of [`turn_error`] where they apply.
    pub code: String,
    /// What happened, in a sentence.
    pub message: String,
    /// Whether trying again may help.
    pub retryable: bool,
}

/// A factual line emitted by the harness adapter, without changing the model's input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "marker", content = "data", rename_all = "snake_case")]
pub enum MarkerPayload {
    /// The resumed session keeps its original instructions until a new session is made.
    InstructionsDeferred {
        /// The program that keeps the original instructions.
        program: String,
    },
    /// It compacted.
    Compacted {
        /// How long it took, where the harness said when it began.
        duration_ms: Option<i64>,
    },
}

/// The kind of an item together with its payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "an item is built once to be written or read; boxing the tool call would put a layer in every use"
)]
pub enum ItemContent {
    /// What the agent said.
    AgentMessage(AgentMessagePayload),
    /// Its reasoning.
    Reasoning(ReasoningPayload),
    /// Its plan.
    Plan(PlanPayload),
    /// A tool call.
    ToolCall(ToolCallPayload),
    /// A line of the thread's history.
    Marker(MarkerPayload),
    /// An error.
    Error(ErrorPayload),
}

impl ItemContent {
    /// The kind of item it is.
    #[must_use]
    pub const fn kind(&self) -> ItemKind {
        match self {
            Self::AgentMessage(_) => ItemKind::AgentMessage,
            Self::Reasoning(_) => ItemKind::Reasoning,
            Self::Plan(_) => ItemKind::Plan,
            Self::ToolCall(_) => ItemKind::ToolCall,
            Self::Marker(_) => ItemKind::Marker,
            Self::Error(_) => ItemKind::Error,
        }
    }
}

/// The stream of an item that belongs to the conversation itself.
pub const MAIN_STREAM: &str = "main";

/// The stream of the items of a sub-agent known as `id`.
#[must_use]
pub fn sub_stream(id: &str) -> String {
    format!("sub:{id}")
}

// ---------------------------------------------------------------------------------------------------------
// Approvals

/// What the harness asks to be allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessRequestKind {
    /// To run a command.
    Command,
    /// To change files.
    FileChange,
    /// To be given more permissions.
    Permissions,
    /// To ask the person a question.
    UserInput,
    /// An MCP server's request for input.
    McpElicitation,
    /// A plan the agent proposes at the end of planning: a person proceeds with it, or keeps planning
    /// with words of their own.
    Plan,
    /// Anything else.
    Other,
}

/// What an option of an approval comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptionStyle {
    /// It allows.
    Allow,
    /// It refuses.
    Deny,
    /// It neither allows nor refuses.
    Neutral,
}

/// How long an allowing option holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptionScope {
    /// For this request.
    Once,
    /// For the rest of the session.
    Session,
    /// Always.
    Always,
    /// For this thread: Orch keeps the rule, held to the matcher the option names; never the harness's settings.
    Thread,
}

/// What a rule kept for a thread answers. An adapter offers one only where it verifies the whole of it on a later
/// request, by offering the same matcher in that request's own option of scope `thread`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalMatcher {
    /// A command that begins with these arguments, run in this directory.
    CommandPrefix {
        /// The arguments it begins with.
        argv: Vec<String>,
        /// Where it runs.
        cwd: String,
    },
    /// Exactly this command, in this directory.
    CommandExact {
        /// The command line.
        command: String,
        /// Where it runs.
        cwd: String,
    },
    /// One host, by protocol and port.
    Host {
        /// The host.
        host: String,
        /// The protocol.
        protocol: String,
        /// The port.
        port: u16,
    },
}

/// One answer a person can give to an approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalOption {
    /// Its id, which the decision names.
    pub id: String,
    /// What it says.
    pub label: String,
    /// What it comes to.
    pub style: OptionStyle,
    /// How long it holds.
    pub scope: Option<OptionScope>,
    /// For scope `thread`: the matcher the adapter verifies on later requests; none otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<ApprovalMatcher>,
}

/// One choice of a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOption {
    /// What it says.
    pub label: String,
    /// A sentence on it.
    pub description: String,
}

/// A question the harness puts to the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    /// Its id, which the answers name.
    pub id: String,
    /// A heading.
    pub header: String,
    /// The question.
    pub question: String,
    /// Whether several choices may be given.
    pub multiple: bool,
    /// Whether an answer of the person's own is allowed.
    pub allow_other: bool,
    /// The choices; `null` for a free answer.
    pub options: Option<Vec<QuestionOption>>,
}

/// What a request is about, by its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum HarnessRequestDetail {
    /// A command to run.
    Command {
        /// The command line.
        command: String,
        /// Where it would run.
        cwd: String,
        /// Why the harness asks.
        reason: Option<String>,
    },
    /// Changes to files.
    FileChange {
        /// The changes.
        changes: Vec<FileChange>,
        /// Why the harness asks.
        reason: Option<String>,
    },
    /// More permissions.
    Permissions {
        /// What is asked, as the harness states it.
        permissions: Json,
        /// Why the harness asks.
        reason: Option<String>,
    },
    /// Questions for the person.
    UserInput {
        /// The native tool item that raised this content request, when the harness supplies one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_item_ref: Option<String>,
        /// The questions.
        questions: Vec<Question>,
    },
    /// An MCP server's request for input.
    McpElicitation {
        /// The native tool item that raised this content request, when the harness supplies one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_item_ref: Option<String>,
        /// The server.
        server: String,
        /// What it says.
        message: String,
        /// The schema of what it wants back.
        schema: Json,
    },
    /// A plan to proceed with.
    Plan {
        /// The plan, in the agent's words (Markdown).
        plan: String,
    },
    /// Anything else.
    Other {
        /// A sentence on it.
        summary: String,
        /// The request as the harness made it.
        raw: Json,
    },
}

impl HarnessRequestDetail {
    /// The native source of a question/form; permission requests retain their ordinary tool rows.
    #[must_use]
    pub fn native_item_ref(&self) -> Option<&str> {
        match self {
            Self::UserInput {
                native_item_ref, ..
            }
            | Self::McpElicitation {
                native_item_ref, ..
            } => native_item_ref.as_deref(),
            _ => None,
        }
    }
}

/// What a harness asks for, in the neutral form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessRequest {
    /// What kind of request it is.
    pub request_kind: HarnessRequestKind,
    /// The answers a person can give; empty for `user_input` and `mcp_elicitation`.
    pub options: Vec<ApprovalOption>,
    /// What it is about.
    pub detail: HarnessRequestDetail,
}

impl HarnessRequest {
    /// The option the engine answers with when a request arrives under the mode `auto`: the first whose
    /// style is `allow`.
    #[must_use]
    pub fn first_allowing(&self) -> Option<&ApprovalOption> {
        self.options
            .iter()
            .find(|option| option.style == OptionStyle::Allow)
    }
}

// ---------------------------------------------------------------------------------------------------------
// The events

/// A blob that holds the whole of an output.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct BlobRef {
    /// Its hash.
    pub hash: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// A session was opened: the body of the event `session.opened`. What opening it answers is
/// [`crate::session::SessionOpened`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenedEvent {
    /// The harness's id for the session.
    pub native_session_ref: String,
    /// Whether it continues one that existed.
    pub resumed: bool,
}

/// A factual refresh of the session's instruction files. It introduces no text or user input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstructionSources {
    /// Newly reported files in native load order; earlier files with the same identity are refreshed.
    pub sources: Vec<crate::session::InstructionFile>,
}

/// The harness reports a title for the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionTitle {
    /// The title; at most 120 characters are used.
    pub title: String,
}

/// The session is gone; the next turn reopens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLost {
    /// Why.
    pub reason: String,
}

/// A turn began.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnStarted {
    /// Orch's id for the turn.
    pub turn_id: Id,
    /// The harness's id for it.
    pub native_turn_ref: String,
}

/// An item began. It is inserted as in progress, unless one with that `native_ref` exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ItemStarted {
    /// The turn it belongs to.
    pub turn_id: Id,
    /// The harness's id for it, unique within the session.
    pub native_ref: String,
    /// `main`, or `sub:<id>` for the items of a sub-agent.
    pub stream: String,
    /// For an item of a sub-agent, the item that started it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_native_ref: Option<String>,
    /// Its kind and payload.
    #[serde(flatten)]
    pub content: ItemContent,
}

/// What grows in an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeltaField {
    /// The text of an agent message; offsets count Unicode code points.
    Text,
    /// The summary of a reasoning; offsets count Unicode code points.
    Summary,
    /// The output of a command; offsets count bytes of the original output.
    Output,
}

/// A piece of growing text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ItemDelta {
    /// The item.
    pub native_ref: String,
    /// What grows.
    pub field: DeltaField,
    /// Where this piece starts, in the field's unit.
    pub offset: i64,
    /// Its length, in the field's unit.
    pub len: i64,
    /// The text; for output, the bytes decoded as UTF-8 with invalid sequences replaced.
    pub data: String,
}

impl ItemDelta {
    /// A piece of text or of a summary that begins `offset` code points into the field.
    #[must_use]
    pub fn text(native_ref: &str, field: DeltaField, offset: i64, data: &str) -> Self {
        Self {
            native_ref: native_ref.to_owned(),
            field,
            offset,
            len: i64::try_from(data.chars().count()).unwrap_or(i64::MAX),
            data: data.to_owned(),
        }
    }

    /// A piece of output that begins `offset` bytes into it: `len` is the length of the bytes as they were,
    /// and `data` is them decoded.
    #[must_use]
    pub fn output(native_ref: &str, offset: i64, bytes: &[u8]) -> Self {
        Self {
            native_ref: native_ref.to_owned(),
            field: DeltaField::Output,
            offset,
            len: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            data: String::from_utf8_lossy(bytes).into_owned(),
        }
    }
}

/// What a receiver does with a piece of growing text, given how much of the field it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaFit {
    /// It continues what is held: append it.
    Append,
    /// It starts before the end of what is held: it was seen already, and is ignored.
    Duplicate,
    /// It starts after the end of what is held: something between was missed, and the item is read whole.
    Gap,
}

/// Whether a delta starting at `offset` continues a field of which `held` is held already: it does when
/// the two are equal.
#[must_use]
pub const fn delta_fit(held: i64, offset: i64) -> DeltaFit {
    if offset == held {
        DeltaFit::Append
    } else if offset < held {
        DeltaFit::Duplicate
    } else {
        DeltaFit::Gap
    }
}

/// Members of an item changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ItemUpdated {
    /// The item.
    pub native_ref: String,
    /// The members of its payload that are replaced (plan steps, a process id, a sub-agent's state).
    pub payload: Json,
}

/// An item is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ItemCompleted {
    /// The item.
    pub native_ref: String,
    /// How it ended.
    pub status: ItemStatus,
    /// Its final payload.
    pub payload: Json,
    /// The blob that holds the whole of its output, where it was longer than what stays inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_blob: Option<BlobRef>,
}

/// The harness asks to be allowed something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequested {
    /// The turn it arose in.
    pub turn_id: Id,
    /// An id unique in the session, which the answer names.
    pub request_ref: String,
    /// A line to show.
    pub title: String,
    /// What is asked.
    pub request: HarnessRequest,
}

/// The harness withdrew a request: the turn was interrupted, or the harness timed out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalClosed {
    /// The request.
    pub request_ref: String,
}

/// What a running turn is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhaseKind {
    /// Working.
    Working,
    /// Shortening its own conversation to make room.
    Compacting,
}

/// A running turn starts to shorten its own conversation, or goes on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnPhase {
    /// The turn.
    pub turn_id: Id,
    /// What it does now.
    pub kind: TurnPhaseKind,
    /// Since when.
    pub at: Time,
}

/// The use of a turn and of its thread so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageUpdated {
    /// The turn.
    pub turn_id: Id,
    /// What the turn has used.
    pub turn: TokenUsage,
    /// What the thread has used.
    pub thread: TokenUsage,
    /// The size of the model's context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    /// The size of the prompt as the harness last reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_used: Option<i64>,
    /// What the harness reported beyond the totals: the sub-agents' and each model's share, where it shortens
    /// the conversation, a shortening, and what fills the context by part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<TurnUsageReport>,
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// It finished.
    Completed,
    /// It was stopped.
    Interrupted,
    /// It failed.
    Failed,
}

/// Why a turn failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnError {
    /// A code; the ones of [`turn_error`] where they apply.
    pub code: String,
    /// What happened, in a sentence.
    pub message: String,
    /// Whether trying again may help.
    pub retryable: bool,
    /// A refusal's cyber, safety or other cause, or a model's missing access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The HTTP status actually received, where supplied by the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    /// The vendor's requested pause before the next attempt, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<i64>,
    /// The hit ProviderFacts.windows entry's id, where the harness reports one.
    #[serde(default)]
    pub window: Option<String>,
    /// What the harness or its vendor said, word for word, beside the code and the sentence above. It is kept
    /// whatever the adapter made of it, so that a failure nobody foresaw can still be read and looked up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// For a limit of the plan: when the vendor says it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Time>,
    /// Whether the answer above this error may have been cut short.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
    /// For a limit: which one, as the adapter read it from the vendor's fields, so that a screen says it and
    /// offers what is left without reading the vendor's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<TurnLimit>,
}

/// What a limit that stopped a turn counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LimitScope {
    /// The plan's own use, over a window.
    Plan,
    /// The use of one family of models; the others of the plan remain.
    ModelFamily,
    /// Spending beyond the plan.
    Spending,
    /// Requests over a short time: the vendor asks to wait.
    Rate,
}

/// A limit that stopped a turn, as the vendor's fields name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnLimit {
    /// What it counts.
    pub scope: LimitScope,
    /// The window it counts over, in minutes, where it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<i64>,
    /// The family of models it holds, where it holds only some, as the vendor names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_family: Option<String>,
    /// How long the vendor asks to wait before trying again, in seconds, where it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<i64>,
}

impl TurnError {
    /// An error with a code and a sentence, that says nothing of the vendor's words, no time and no cut answer.
    #[must_use]
    pub fn new(code: &str, message: &str, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
            reason: None,
            status: None,
            retry_after_secs: None,
            window: None,
            said: None,
            resets_at: None,
            incomplete: false,
            limit: None,
        }
    }

    /// The same error carrying the limit that stopped the turn.
    #[must_use]
    pub fn limited(mut self, limit: Option<TurnLimit>) -> Self {
        self.limit = limit;
        self
    }

    /// The same error carrying what the harness or its vendor said.
    #[must_use]
    pub fn saying(mut self, said: &str) -> Self {
        self.said = Some(said.to_owned());
        self
    }

    /// The same error carrying the time the vendor says a limit ends.
    #[must_use]
    pub fn resetting_at(mut self, resets_at: Option<Time>) -> Self {
        self.resets_at = resets_at;
        self
    }

    /// The same error, marked as one that may have cut an answer short.
    #[must_use]
    pub fn cut_short(mut self, incomplete: bool) -> Self {
        self.incomplete = incomplete;
        self
    }
}

/// A turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnCompleted {
    /// The turn.
    pub turn_id: Id,
    /// How it ended.
    pub status: TurnOutcome,
    /// Why, for a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TurnError>,
}

/// Native cleanup belongs to its exact stopped turn even if completion arrived first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnCommandCleanup {
    /// The stopped turn.
    pub turn_id: Id,
    /// Actual native outcomes, never inferred from a forced controller end.
    pub result: crate::background::CommandCleanup,
}

/// The agent called a tool of the `orch` server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OrchToolCall {
    /// The turn.
    pub turn_id: Id,
    /// The call's id; the same one runs once.
    pub call_ref: String,
    /// One transport attempt at the same logical call; replaying the event preserves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Id>,
    /// The tool's name as the server lists it ([`OrchTool::name`]).
    pub tool: String,
    /// What it was called with.
    pub arguments: Json,
}

/// The harness needs another credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialNeeded {
    /// Why.
    pub reason: CredentialNeed,
}

/// A frame of the harness's own protocol, kept for diagnosis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RawFrame {
    /// Which way it went, as the adapter names it.
    pub direction: String,
    /// The frame.
    pub payload: Json,
}

/// Everything a harness session does, in the order it happened: stream `session:<thread id>:<segment id>`.
/// Written as `{ "kind": "item.started", "body": { … } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "body")]
pub enum SessionEvent {
    /// The session is open.
    #[serde(rename = "session.opened")]
    SessionOpened(SessionOpenedEvent),
    /// Instruction files the harness reported loading, including later imports and on-demand files.
    #[serde(rename = "session.instructions")]
    InstructionSources(InstructionSources),
    /// The harness reports a title.
    #[serde(rename = "session.title")]
    SessionTitle(SessionTitle),
    /// The session is gone.
    #[serde(rename = "session.lost")]
    SessionLost(SessionLost),
    /// A turn began.
    #[serde(rename = "turn.started")]
    TurnStarted(TurnStarted),
    /// An item began.
    #[serde(rename = "item.started")]
    ItemStarted(ItemStarted),
    /// A piece of growing text.
    #[serde(rename = "item.delta")]
    ItemDelta(ItemDelta),
    /// Members of an item changed.
    #[serde(rename = "item.updated")]
    ItemUpdated(ItemUpdated),
    /// An item is done.
    #[serde(rename = "item.completed")]
    ItemCompleted(ItemCompleted),
    /// The harness asks to be allowed something.
    #[serde(rename = "approval.requested")]
    ApprovalRequested(ApprovalRequested),
    /// The harness withdrew a request.
    #[serde(rename = "approval.closed")]
    ApprovalClosed(ApprovalClosed),
    /// A running turn starts to compact, or goes on.
    #[serde(rename = "turn.phase")]
    TurnPhase(TurnPhase),
    /// The use so far.
    #[serde(rename = "usage.updated")]
    UsageUpdated(UsageUpdated),
    /// A turn ended.
    #[serde(rename = "turn.completed")]
    TurnCompleted(TurnCompleted),
    /// Actual cleanup of the stopped turn's commands, including a late report.
    #[serde(rename = "turn.command_cleanup")]
    TurnCommandCleanup(TurnCommandCleanup),
    /// The agent called a tool of the `orch` server.
    #[serde(rename = "orch.tool_call")]
    OrchToolCall(OrchToolCall),
    /// The harness needs another credential.
    #[serde(rename = "credential.needed")]
    CredentialNeeded(CredentialNeeded),
    /// A frame of the harness's own protocol.
    #[serde(rename = "raw.frame")]
    RawFrame(RawFrame),
}

/// The kinds of a session event, as they are written.
pub const SESSION_EVENT_KINDS: [&str; 18] = [
    "session.opened",
    "session.instructions",
    "session.title",
    "session.lost",
    "turn.started",
    "item.started",
    "item.delta",
    "item.updated",
    "item.completed",
    "approval.requested",
    "approval.closed",
    "turn.phase",
    "usage.updated",
    "turn.completed",
    "turn.command_cleanup",
    "orch.tool_call",
    "credential.needed",
    "raw.frame",
];

impl SessionEvent {
    /// The kind as it is written.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SessionOpened(_) => "session.opened",
            Self::InstructionSources(_) => "session.instructions",
            Self::SessionTitle(_) => "session.title",
            Self::SessionLost(_) => "session.lost",
            Self::TurnStarted(_) => "turn.started",
            Self::ItemStarted(_) => "item.started",
            Self::ItemDelta(_) => "item.delta",
            Self::ItemUpdated(_) => "item.updated",
            Self::ItemCompleted(_) => "item.completed",
            Self::ApprovalRequested(_) => "approval.requested",
            Self::ApprovalClosed(_) => "approval.closed",
            Self::TurnPhase(_) => "turn.phase",
            Self::UsageUpdated(_) => "usage.updated",
            Self::TurnCompleted(_) => "turn.completed",
            Self::TurnCommandCleanup(_) => "turn.command_cleanup",
            Self::OrchToolCall(_) => "orch.tool_call",
            Self::CredentialNeeded(_) => "credential.needed",
            Self::RawFrame(_) => "raw.frame",
        }
    }

    /// Whether the event is a diagnostic: sent only when no other event is waiting on any stream.
    #[must_use]
    pub const fn is_raw(&self) -> bool {
        matches!(self, Self::RawFrame(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u8) -> String {
        format!("0199abcd-1234-7000-8000-0000000000{n:02}")
    }

    fn read(event: Json) -> SessionEvent {
        serde_json::from_value(event).unwrap()
    }

    #[test]
    fn every_kind_of_event_of_the_catalogue_reads_and_writes_as_the_spec_shows_it() {
        let events = [
            json!({ "kind": "session.opened", "body": { "native_session_ref": "thr_1", "resumed": false } }),
            json!({ "kind": "session.instructions", "body": { "sources": [] } }),
            json!({ "kind": "session.title", "body": { "title": "Fix the login redirect" } }),
            json!({ "kind": "session.lost", "body": { "reason": "the harness process exited" } }),
            json!({ "kind": "turn.started", "body": { "turn_id": id(1), "native_turn_ref": "turn_9" } }),
            json!({ "kind": "item.started", "body": {
                "turn_id": id(1), "native_ref": "item_3", "stream": "main", "kind": "agent_message",
                "payload": { "text": "", "text_len": 0, "phase": null } } }),
            json!({ "kind": "item.delta", "body": {
                "native_ref": "item_3", "field": "text", "offset": 0, "len": 5, "data": "Hello" } }),
            json!({ "kind": "item.updated", "body": {
                "native_ref": "item_4", "payload": { "steps": [ { "text": "read", "status": "completed" } ] } } }),
            json!({ "kind": "item.completed", "body": {
                "native_ref": "item_3", "status": "completed",
                "payload": { "text": "Hello", "text_len": 5, "phase": "final" } } }),
            json!({ "kind": "approval.requested", "body": {
                "turn_id": id(1), "request_ref": "req_1", "title": "Run npm test", "request": {
                    "request_kind": "command",
                    "options": [ { "id": "once", "label": "Allow once", "style": "allow", "scope": "once" },
                                 { "id": "deny", "label": "Deny", "style": "deny", "scope": null } ],
                    "detail": { "command": "npm test", "cwd": "/workspace/web", "reason": null } } } }),
            json!({ "kind": "approval.closed", "body": { "request_ref": "req_1" } }),
            json!({ "kind": "turn.phase", "body": { "turn_id": id(1), "kind": "compacting", "at": "2026-10-11T10:00:00.000Z" } }),
            json!({ "kind": "usage.updated", "body": {
                "turn_id": id(1),
                "turn": { "input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 20, "reasoning_tokens": 5, "total_tokens": 120 },
                "thread": { "input_tokens": 300, "cached_input_tokens": 90, "output_tokens": 60, "reasoning_tokens": 8, "total_tokens": 360 },
                "context_window": 272000, "context_used": 12000 } }),
            json!({ "kind": "turn.completed", "body": {
                "turn_id": id(1), "status": "failed",
                "error": { "code": "provider_exhausted", "message": "usage limit", "retryable": false,
                           "said": "You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)",
                           "resets_at": "2026-10-14T09:00:00.000Z", "incomplete": true, "window": null } } }),
            json!({ "kind": "turn.command_cleanup", "body": { "turn_id": id(1), "result": { "ended": 1, "still_running": 1, "unknown": 0 } } }),
            json!({ "kind": "orch.tool_call", "body": {
                "turn_id": id(1), "call_ref": "call_7", "tool": "list_shared", "arguments": { "depth": 2 } } }),
            json!({ "kind": "credential.needed", "body": { "reason": "expiring" } }),
            json!({ "kind": "raw.frame", "body": { "direction": "in", "payload": { "method": "x" } } }),
        ];
        assert_eq!(events.len(), SESSION_EVENT_KINDS.len());
        for (written, expected_kind) in events.iter().zip(SESSION_EVENT_KINDS) {
            let event = read(written.clone());
            assert_eq!(event.kind(), expected_kind);
            assert_eq!(
                &serde_json::to_value(&event).unwrap(),
                written,
                "{expected_kind}"
            );
        }
    }

    #[test]
    fn an_error_that_says_nothing_of_the_vendors_words_reads_and_writes_without_them() {
        let old = json!({ "code": "harness_error", "message": "it failed", "retryable": true });
        let read: TurnError = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(read, TurnError::new("harness_error", "it failed", true));
        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            json!({"code":"harness_error","message":"it failed","retryable":true,"window":null})
        );
        let time: Time = "2026-10-14T09:00:00.000Z".parse().unwrap();
        let limit = TurnError::new(
            turn_error::PROVIDER_EXHAUSTED,
            "the weekly limit is reached",
            false,
        )
        .saying("You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)")
        .resetting_at(Some(time))
        .cut_short(true);
        let written = serde_json::to_value(&limit).unwrap();
        assert_eq!(written["resets_at"], "2026-10-14T09:00:00.000Z");
        assert_eq!(written["incomplete"], true);
        assert_eq!(serde_json::from_value::<TurnError>(written).unwrap(), limit);
    }

    #[test]
    fn a_tool_call_is_reported_by_kind_and_as_the_harness_made_it() {
        let started = read(json!({ "kind": "item.started", "body": {
            "turn_id": id(1), "native_ref": "item_5", "stream": "sub:abc", "parent_native_ref": "item_2",
            "kind": "tool_call", "payload": {
                "tool": { "kind": "command", "command": "cargo test", "cwd": "/workspace/web", "exit_code": null,
                          "output": { "inline": "", "total_len": 0, "truncated": false, "blob": null },
                          "process_id": "p-1", "background": false,
                          "actions": [ { "type": "search", "query": "fn main", "path": null }, { "type": "other" } ] },
                "title": "cargo test", "started_at": "2026-10-11T10:00:00.000Z", "duration_ms": null,
                "raw": { "name": "commandExecution",
                         "input": { "inline": "{\"cmd\":\"cargo test\"}", "total_len": 19, "truncated": false, "blob": null },
                         "output": null, "is_error": false } } } }));
        let SessionEvent::ItemStarted(item) = &started else {
            panic!("an item started");
        };
        assert_eq!(item.content.kind(), ItemKind::ToolCall);
        assert_eq!(item.parent_native_ref.as_deref(), Some("item_2"));
        let ItemContent::ToolCall(call) = &item.content else {
            panic!("a tool call");
        };
        assert!(matches!(
            &call.tool,
            ToolPayload::Command {
                exit_code: None,
                background: false,
                ..
            }
        ));
        assert_eq!(
            call.raw.name, "commandExecution",
            "the harness's own name for the tool is kept"
        );
        // Written again, an absent parent is left out and a present one stays.
        let written = serde_json::to_value(&started).unwrap();
        assert_eq!(written["body"]["parent_native_ref"], "item_2");
    }

    #[test]
    fn every_kind_of_tool_payload_reads_as_the_spec_writes_it() {
        let output = json!({ "inline": "x", "total_len": 1, "truncated": false, "blob": null });
        let payloads = [
            json!({ "kind": "file_change", "changes": [ { "path": "a.rs", "change": "modify", "old_path": null,
                    "diff": "@@\n-a\n+b", "added": 1, "removed": 1, "binary": false, "truncated": false } ] }),
            json!({ "kind": "file_read", "path": "src/main.rs", "start_line": 1, "end_line": 40, "total_lines": 120,
                    "content": output, "media_type": null }),
            json!({ "kind": "file_search", "mode": "content", "query": "TODO", "path": "src",
                    "matches": [ { "path": "src/a.rs", "line": 3, "text": "// TODO" } ], "total": 1, "truncated": false }),
            json!({ "kind": "mcp", "server": "github", "tool": "create_issue", "arguments": { "title": "x" },
                    "result": null, "error": null }),
            json!({ "kind": "web_search", "query": "rust async", "results": [ { "title": "Async book", "url": "https://rust-lang.github.io/async-book/" } ] }),
            json!({ "kind": "web_fetch", "url": "https://example.com", "title": null, "status": 200, "content": output }),
            json!({ "kind": "image", "blob": "0".repeat(64), "media_type": "image/png", "name": null, "caption": "a chart" }),
            json!({ "kind": "subagent", "child_stream": "sub:abc", "name": "explorer", "role": null, "prompt": "look",
                    "state": "running" }),
            json!({ "kind": "orch", "tool": "announce", "arguments": { "title": "t" }, "result": null }),
            json!({ "kind": "other", "name": "sleep", "arguments": { "ms": 5 }, "result": null }),
        ];
        for written in payloads {
            let read: ToolPayload = serde_json::from_value(written.clone()).unwrap();
            assert_eq!(serde_json::to_value(&read).unwrap(), written);
        }
    }

    #[test]
    fn each_kind_of_request_has_its_own_detail() {
        let user_input: HarnessRequest = serde_json::from_value(json!({
            "request_kind": "user_input", "options": [],
            "detail": { "questions": [ { "id": "q1", "header": "Which?", "question": "Which database?", "multiple": false,
                                         "allow_other": true, "options": [ { "label": "Postgres", "description": "The default" } ] } ] }
        }))
        .unwrap();
        assert!(matches!(
            user_input.detail,
            HarnessRequestDetail::UserInput { .. }
        ));
        assert!(user_input.first_allowing().is_none());
        let other: HarnessRequest = serde_json::from_value(json!({
            "request_kind": "other",
            "options": [ { "id": "no", "label": "No", "style": "deny", "scope": null },
                         { "id": "yes", "label": "Yes", "style": "allow", "scope": "session" } ],
            "detail": { "summary": "something new", "raw": { "x": 1 } }
        }))
        .unwrap();
        assert_eq!(other.first_allowing().map(|o| o.id.as_str()), Some("yes"));
        for (written, wanted) in [
            (json!({ "changes": [], "reason": "why" }), "file_change"),
            (
                json!({ "permissions": { "network": true }, "reason": null }),
                "permissions",
            ),
            (
                json!({ "server": "s", "message": "m", "schema": {} }),
                "mcp_elicitation",
            ),
        ] {
            let detail: HarnessRequestDetail = serde_json::from_value(written.clone()).unwrap();
            let name = match detail {
                HarnessRequestDetail::FileChange { .. } => "file_change",
                HarnessRequestDetail::Permissions { .. } => "permissions",
                HarnessRequestDetail::McpElicitation { .. } => "mcp_elicitation",
                _ => "wrong",
            };
            assert_eq!(name, wanted);
        }
    }

    #[test]
    fn a_long_output_keeps_its_last_sixteen_kib_and_counts_the_rest() {
        let mut output = OutputRef::default();
        let chunk = "é".repeat(5_000); // 10 000 bytes
        output.push(&chunk);
        assert!(!output.truncated);
        output.push(&chunk);
        output.push("tail");
        assert_eq!(output.total_len, 20_004);
        assert!(output.truncated);
        assert!(i64::try_from(output.inline.len()).unwrap() <= OUTPUT_INLINE_MAX_BYTES);
        assert!(
            output.inline.ends_with("étail"),
            "the end is what stays, and it is cut at a character"
        );
        assert!(output.inline.is_char_boundary(0));
        let blob: Hash = Hash::of(b"whole");
        assert_eq!(output.clone().with_blob(blob).blob, Some(blob));
        assert_eq!(
            OutputRef::of("short").with_blob(blob).blob,
            None,
            "nothing is cut, so there is no blob"
        );
    }

    #[test]
    fn growing_text_is_appended_only_where_it_continues_what_is_held() {
        assert_eq!(delta_fit(18, 18), DeltaFit::Append);
        assert_eq!(delta_fit(18, 0), DeltaFit::Duplicate);
        assert_eq!(delta_fit(18, 17), DeltaFit::Duplicate);
        assert_eq!(delta_fit(18, 19), DeltaFit::Gap);
        let first = ItemDelta::text("i9", DeltaField::Text, 0, "I'll start by read");
        let next = ItemDelta::text("i9", DeltaField::Text, first.len, "ing the handler.");
        assert_eq!((first.len, next.offset, next.len), (18, 18, 16));
        let multibyte = ItemDelta::text("i9", DeltaField::Summary, 0, "añb");
        assert_eq!(multibyte.len, 3, "code points, not bytes");
        let output = ItemDelta::output("i9", 0, &[b'o', 0xff, b'k']);
        assert_eq!(output.len, 3, "bytes of the original");
        assert_eq!(output.data, "o\u{fffd}k");
    }

    #[test]
    fn usage_counts_the_parts_into_the_input_and_the_output() {
        let usage = TokenUsage::from_parts(TokensReportedApart {
            input: 60,
            cache_read: 30,
            cache_write: 10,
            output: 15,
            reasoning: 5,
        });
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cached_input_tokens, 30);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.reasoning_tokens, 5);
        assert_eq!(usage.total_tokens, 120);
        assert!(usage.is_consistent());
        let twice = usage.plus(usage);
        assert_eq!(twice.total_tokens, 240);
        assert!(
            !TokenUsage {
                total_tokens: 5,
                ..TokenUsage::default()
            }
            .is_consistent()
        );
    }

    #[test]
    fn api_readers_receive_the_adapters_exact_usage_and_raw_call_without_a_second_shape() {
        let wire = json!({ "input_tokens": 100, "cached_input_tokens": 30, "cache_write_tokens": 10,
            "output_tokens": 20, "reasoning_tokens": 5, "total_tokens": 120 });
        let emitted: TokenUsage = serde_json::from_value(wire.clone()).unwrap();
        let read: orch_api_contract::threads::TokenUsage = emitted;
        assert_eq!(serde_json::to_value(read).unwrap(), wire);
        assert!(read.is_consistent());
        let old: TokenUsage =
            serde_json::from_value(json!({ "input_tokens": 1, "cached_input_tokens": 0,
            "output_tokens": 2, "reasoning_tokens": 0, "total_tokens": 3 }))
            .unwrap();
        assert_eq!(old.cache_write_tokens, 0);
        assert!(
            serde_json::to_value(old)
                .unwrap()
                .get("cache_write_tokens")
                .is_none()
        );
        let call = RawCall {
            name: "read_file".to_owned(),
            input: OutputRef::of("{\"path\":\"你好.rs\"}"),
            output: Some(OutputRef::of("the exact native result")),
            is_error: false,
        };
        let read: orch_api_contract::threads::RawCall = call.clone();
        assert_eq!(read, call);
        assert_eq!(
            serde_json::to_value(read).unwrap()["output"]["inline"],
            "the exact native result"
        );
        assert_eq!(
            schemars::schema_for!(TokenUsage),
            schemars::schema_for!(orch_api_contract::threads::TokenUsage)
        );
        assert_eq!(
            schemars::schema_for!(RawCall),
            schemars::schema_for!(orch_api_contract::threads::RawCall)
        );
    }

    #[test]
    fn the_lines_a_diff_adds_and_removes_are_counted_without_its_headers() {
        let diff = "--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n context\n-old\n+new\n+more\n";
        assert_eq!(count_diff_lines(diff), (2, 1));
        assert_eq!(count_diff_lines(""), (0, 0));
    }

    /// Every string the schema of a type names as one of its values, wherever it stands in it.
    fn named_values(schema: &Json, out: &mut std::collections::BTreeSet<String>) {
        match schema {
            Json::Object(members) => {
                if let Some(Json::String(word)) = members.get("const") {
                    out.insert(word.clone());
                }
                if let Some(Json::Array(words)) = members.get("enum") {
                    out.extend(words.iter().filter_map(Json::as_str).map(str::to_owned));
                }
                members.values().for_each(|each| named_values(each, out));
            }
            Json::Array(each) => each.iter().for_each(|each| named_values(each, out)),
            _ => {}
        }
    }

    #[test]
    fn the_tools_of_orch_are_known_by_name_and_the_list_holds_each_of_them_once() {
        for tool in OrchTool::ALL.iter().copied() {
            assert_eq!(OrchTool::parse(tool.name()), Some(tool));
            assert_eq!(serde_json::to_value(tool).unwrap(), json!(tool.name()));
        }
        assert_eq!(OrchTool::parse("rm_rf"), None);
        // The history schema also reads the two retired names; the advertised list contains neither.
        let mut declared = std::collections::BTreeSet::new();
        named_values(
            &serde_json::to_value(schemars::schema_for!(OrchTool)).unwrap(),
            &mut declared,
        );
        let listed: std::collections::BTreeSet<String> = OrchTool::ALL
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect();
        let mut readable = listed.clone();
        for historical in [OrchTool::SendChatMessage, OrchTool::SendThreadMessage] {
            assert!(!OrchTool::ALL.contains(&historical));
            assert_eq!(OrchTool::parse(historical.name()), Some(historical));
            assert_eq!(
                serde_json::from_value::<OrchTool>(json!(historical.name())).unwrap(),
                historical
            );
            let api: orch_api_contract::threads::OrchTool =
                serde_json::from_value(json!(historical.name())).unwrap();
            assert_eq!(serde_json::to_value(api).unwrap(), json!(historical.name()));
            readable.insert(historical.name().to_owned());
        }
        assert_eq!(readable, declared);
        assert_eq!(listed.len(), OrchTool::ALL.len(), "no tool is listed twice");
    }

    #[test]
    fn a_marker_an_adapter_emits_is_a_compaction_with_or_without_a_duration() {
        let marker = read(json!({ "kind": "item.started", "body": {
            "turn_id": id(1), "native_ref": "m1", "stream": "main", "kind": "marker",
            "payload": { "marker": "compacted", "data": { "duration_ms": null } } } }));
        let SessionEvent::ItemStarted(item) = marker else {
            panic!("an item started");
        };
        assert_eq!(
            item.content,
            ItemContent::Marker(MarkerPayload::Compacted { duration_ms: None })
        );
        assert_eq!(sub_stream("abc"), "sub:abc");
        assert_eq!(MAIN_STREAM, "main");
    }

    #[test]
    fn deferred_instructions_are_an_informational_marker_in_both_contracts() {
        let written = json!({
            "marker": "instructions_deferred", "data": { "program": "Codex" }
        });
        let marker: MarkerPayload = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(
            marker,
            MarkerPayload::InstructionsDeferred {
                program: "Codex".to_owned()
            }
        );
        assert_eq!(serde_json::to_value(&marker).unwrap(), written);
        let api: orch_api_contract::threads::MarkerPayload =
            serde_json::from_value(written.clone()).unwrap();
        assert_eq!(
            api.kind(),
            orch_api_contract::threads::MarkerKind::InstructionsDeferred
        );
        assert_eq!(serde_json::to_value(api).unwrap(), written);
        let content = ItemContent::Marker(marker);
        assert_eq!(content.kind(), ItemKind::Marker);
    }

    #[test]
    fn a_diagnostic_is_the_only_event_that_waits_for_quiet() {
        let raw =
            read(json!({ "kind": "raw.frame", "body": { "direction": "out", "payload": null } }));
        assert!(raw.is_raw());
        assert!(!read(json!({ "kind": "session.title", "body": { "title": "t" } })).is_raw());
        assert!(ItemStatus::Completed.is_final() && !ItemStatus::InProgress.is_final());
    }
}
