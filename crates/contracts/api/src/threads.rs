//! Thread sync; items with the markers that are the thread's history; each step of the agent's work as a display for its kind beside the call as its harness made it; inputs and turns with model and effort, the phase a turn is in and the limits each was let past; segments; side threads; a thread's directory and where its agent's notes are kept; its lock and input access; and exports at three depths.
//!
//! A thread is a conversation with an agent. Its timeline is a list of
//! [`Item`]s; what an item holds depends on its kind, and the two are written
//! side by side as `kind` and `payload`. A tool call carries the call as
//! Orch understood it, for a display made for its kind ([`ToolPayload`]), and
//! the same call as its harness made it, whole ([`RawCall`]). A marker is a
//! line of the thread's history, a few facts and never content. What the
//! person says goes in as a [`ThreadInput`], which a [`Turn`] answers; a
//! [`ThreadSync`] brings a client up to date.

use orch_types::{Hash, Id, Json, Revision, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::MissingAccess;
use crate::binding::{ArrangementType, CredentialPath, Isolation};
use crate::chats::ChatMessage;
use crate::forks::{Fidelity, HistoryReport, PathMap};
use crate::nodes::{EnvironmentRef, JobRef, NodeBase, NodeRef};
use crate::personal::Draft;
use crate::routes::Access::{Permission, Rule, Session, User};
use crate::usage::CapStatus;
use crate::users::{ChatRef, UserRef};

pub mod continuations;
pub use continuations::{
    ContinuationActiveTurn, ContinuationRunningAction, PromptEditAvailability, PromptEditReason,
    ThreadContinuationAccepted, ThreadContinuationActivateBody, ThreadContinuationProjection,
    ThreadContinuationStatus, ThreadInputListQuery, ThreadInputPage, ThreadPromptSummary,
    ThreadPromptVersion, ThreadPromptVersionCreateBody, ThreadPromptVersionListQuery,
    ThreadPromptVersionsPage,
};

/// How many items a sync without a position holds, when no limit is given.
pub const THREAD_SYNC_DEFAULT_LIMIT: u32 = 200;
/// The most items a sync holds.
pub const THREAD_SYNC_MAX_LIMIT: u32 = 500;
/// How many changed rows a sync from a position holds; past this it says it is
/// not complete, and the client loads again without a position.
pub const THREAD_SYNC_MAX_CHANGES: i64 = 2_000;
/// How many ended turns a sync without a position holds, beside those that
/// have not ended.
pub const THREAD_SYNC_ENDED_TURNS: u32 = 20;
/// How many items a read of a stretch of the timeline holds, when no limit is given.
pub const ITEMS_DEFAULT_LIMIT: u32 = 100;
/// The most items a read of a stretch of the timeline holds.
pub const ITEMS_MAX_LIMIT: u32 = 500;
/// How many bytes of a command's output one read returns, when no limit is given.
pub const OUTPUT_DEFAULT_LIMIT_BYTES: i64 = 262_144;
/// The most bytes of a long text kept inline in an [`OutputRef`]: its end.
pub const OUTPUT_INLINE_MAX_BYTES: i64 = 16 * 1024;
/// The most bytes of the diff of one file in a [`FileChange`]; a longer one
/// is cut and the file opens whole in the code view.
pub const FILE_CHANGE_DIFF_MAX_BYTES: i64 = 256 * 1024;
/// How many matches a file search shows.
pub const FILE_SEARCH_MAX_MATCHES: u32 = 200;
/// The most characters of text in one input, across all its text parts.
pub const INPUT_TEXT_MAX_CHARS: u32 = 100_000;
/// How many characters of a message make the title of its line in the outline.
pub const OUTLINE_TITLE_CHARS: u32 = 80;

/// The most bytes of an image sent to the API; the figure in force is also told
/// in the answer to a request for the system's metadata.
pub const IMAGE_UPLOAD_MAX_BYTES: i64 = 20 * 1024 * 1024;
/// The stream of a thread's main conversation, as an item names it.
pub const MAIN_STREAM: &str = "main";
/// What the stream of a sub-agent's items begins with; the sub-agent's
/// identifier follows.
pub const SUB_STREAM_PREFIX: &str = "sub:";
/// The detail of an input that waits because the thread's first owner may not
/// use its provider.
pub const OWNER_LACKS_ACCESS: &str = "owner_lacks_access";
/// The header that carries the name of a file that is sent as a raw body.
pub const FILENAME_HEADER: &str = "x-orch-filename";
/// The header that tells the whole length of a command's output when only a
/// piece of it is read.
pub const TOTAL_LENGTH_HEADER: &str = "x-total-length";

/// What a thread is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadState {
    /// Waiting for something to be said.
    Idle,
    /// Getting ready to take a turn.
    Starting,
    /// A turn is under way.
    Running,
    /// A turn waits for an approval.
    WaitingApproval,
    /// It cannot go on.
    Blocked,
}

/// The one reader-specific presentation of a thread, independent of its execution state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadDisplayState {
    /// A request or held input waits for a person.
    WaitingApproval,
    /// The thread cannot continue.
    Blocked,
    /// The last turn failed without a later accepted input.
    Failed,
    /// A turn is starting or running.
    Running,
    /// Messages wait to start.
    Queued,
    /// A person held the queue until Resume.
    Held,
    /// A completed turn has not been seen by this reader.
    Finished,
    /// No work or unseen result needs an icon.
    #[default]
    Idle,
    /// The thread or a place above it is archived.
    Archived,
}

/// The priority of visible work inside a place; quiet and archived work add no indicator.
pub const THREAD_DISPLAY_URGENCY: [ThreadDisplayState; 7] = [
    ThreadDisplayState::WaitingApproval,
    ThreadDisplayState::Blocked,
    ThreadDisplayState::Failed,
    ThreadDisplayState::Running,
    ThreadDisplayState::Queued,
    ThreadDisplayState::Held,
    ThreadDisplayState::Finished,
];

/// Safe particulars for a thread's state icon, supplied by the server with its existing read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ThreadDisplay {
    /// The thread sequence of these execution facts, for ordering existing live deliveries.
    pub seq: Seq,
    /// This reader's active-history position, for ordering personal updates at the same sequence.
    pub read_seq: Seq,
    /// The presentation selected by the server's precedence rules.
    pub state: ThreadDisplayState,
    /// Inputs waiting to start, excluding those held by a person.
    pub queued_inputs: i64,
    /// Inputs held until Resume.
    pub held_inputs: i64,
    /// Inputs waiting for a person's authorization or recovery.
    pub waiting_inputs: i64,
    /// An outstanding native request asks for an answer rather than permission.
    pub waiting_for_answer: bool,
    /// The safe reason for a blocked state, without diagnostic text or private names.
    pub blocked_reason: Option<BlockedReason>,
    /// When the latest completed turn finished; null when no turn completed.
    pub finished_at: Option<Time>,
    /// Safe automatic-recovery particulars when they determine the selected presentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Box<ThreadDisplayRecovery>>,
}

/// Retry timing and stable causes without vendor text or configured candidate parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ThreadDisplayRecovery {
    /// The recorded rule identifier.
    pub rule_id: String,
    /// The reserved action, without any candidate's configuration.
    pub step: Option<ThreadRecoveryStep>,
    /// The attempt within the current step.
    pub attempt: i64,
    /// The recorded step's configured attempt count, absent for waits and failovers.
    pub attempts: Option<i64>,
    /// When automatic handling can try again.
    pub due_at: Option<Time>,
    /// Retained effects require a visible continuation.
    pub continuing: bool,
    /// The localized cause, using only recognized failure codes.
    pub failure_kind: ThreadRecoveryFailure,
    /// The stable reason automatic handling stopped.
    pub stop_reason: Option<ThreadRecoveryStop>,
    /// The occurrences recorded by a repeat guard.
    pub count: Option<i64>,
    /// The repeat guard's recorded window in seconds.
    pub window_secs: Option<i64>,
}

/// The kind of reserved recovery action whose particulars an icon may disclose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadRecoveryStep {
    /// Repeat the interrupted input.
    Retry,
    /// Continue retained work.
    Continue,
    /// Use a configured alternative.
    Failover,
    /// Wait for a known condition.
    Wait,
}

/// Recognized call failures, with unknown extension codes kept generic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadRecoveryFailure {
    /// The provider is busy.
    ProviderBusy,
    /// The provider cannot be reached.
    ProviderUnreachable,
    /// The model refused the request.
    ModelRefused,
    /// The model is unavailable.
    ModelUnavailable,
    /// Model use was not granted.
    Access,
    /// A provider limit was reached.
    ProviderExhausted,
    /// The provider refused its credential.
    ProviderUnauthorized,
    /// The context is full.
    ContextOverflow,
    /// The agent program failed.
    HarnessError,
    /// No recognized failure code was supplied.
    #[default]
    Any,
}

/// Recognized recovery stops; arbitrary diagnostic detail never enters an icon payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadRecoveryStop {
    /// A repeat guard stopped the loop.
    RepeatLimit,
    /// The selected history changed.
    HistoryChanged,
    /// The original author is unavailable.
    AuthorMissing,
    /// The original author cannot drive the thread.
    AuthorLacksInteract,
    /// The original author cannot use the model.
    AuthorLacksUse,
    /// The configured switch failed.
    SwitchFailed,
    /// The configured switch needs attention.
    SwitchBlocked,
    /// A person stopped automatic handling.
    StoppedByPerson,
    /// The original author cannot use the candidate.
    CandidateNotAllowed,
    /// The candidate has a different embedding identity.
    EmbeddingIdentity,
    /// The interrupted input is unavailable.
    OriginMissing,
    /// The recovery step is unavailable.
    RuleUnavailable,
    /// The recovery rules changed.
    RulesChanged,
    /// Automatic recovery is disabled.
    AutoDisabled,
    /// The repeat limit changed.
    RepeatLimitChanged,
    /// A model-call precondition needs attention.
    ServicePrecondition,
    /// The configured retry time is unavailable.
    DelayUnrepresentable,
    /// The candidate parameters are unavailable.
    CandidateParameters,
    /// The configured candidate is unavailable.
    CandidateUnavailable,
    /// The original submission's authority is unavailable.
    OriginAuthority,
}

/// Why a thread cannot go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockedReason {
    /// It has no provider and agent yet.
    Unbound,
    /// The machine its agent runs on is offline.
    AgentOffline,
    /// Its agent profile is switched off.
    AgentDisabled,
    /// Its provider is not ready.
    ProviderUnavailable,
    /// Its provider has no room left.
    ProviderExhausted,
    /// Its environment is not ready.
    EnvironmentUnavailable,
    /// Its binding does not hold together.
    BindingInvalid,
    /// The provider's address is outside the policy of this place.
    ProviderNotAllowed,
    /// No model is set.
    ModelNotSet,
    /// The model set is not allowed.
    ModelNotAllowed,
    /// A cap has no room.
    QuotaExceeded,
    /// Only inputs whose files cannot reach the agent wait, each held for its author to recover.
    Attachments,
    /// Its workspace holds a configuration of the harness's own, and the people who manage the environment have
    /// not yet answered whether the harness may use it.
    WorkspaceTrust,
    /// A switch holds it.
    Switching,
    /// Something went wrong.
    Error,
}

/// What a starting thread is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StartingStep {
    /// Its environment.
    Environment,
    /// Its declared repository, worktree or directory, before files and the first turn.
    Source,
    /// Its provider.
    Provider,
    /// Its harness.
    Harness,
    /// The credential.
    Credential,
    /// A session.
    Session,
    /// Its files.
    Files,
    /// The turn itself.
    Turn,
}

/// The `kind` of a thread: always `thread`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    /// A thread.
    #[default]
    Thread,
}

pub use crate::resources::{AgentRef, ProviderKind, ProviderRef, ProviderStatus};

/// What a thread runs on: an agent profile on a provider, placed in one of the arrangement types. A
/// thread holds no access of its own; whoever acts on it uses what they themselves hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadBinding {
    /// The reported program build and its compatibility result, when the harness has reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<crate::admin::RunningHarnessBuild>,
    /// The agent profile.
    pub agent: AgentRef,
    /// The provider.
    pub provider: ProviderRef,
    /// The arrangement type; `isolation` says what it comes to here.
    pub arrangement: ArrangementType,
    /// What the arrangement amounts to.
    pub isolation: Isolation,
    /// Whether its agent program can work a message in plan mode, so that the composer offers Plan.
    #[serde(default)]
    pub plan_mode: bool,
    /// How plan mode works in its agent program: how the plan is put to the person and what Proceed does;
    /// `null` where plan mode is not offered.
    #[serde(default)]
    pub plan_mechanics: Option<crate::binding::PlanMechanics>,
}

/// How many tokens a turn or a thread used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TokenUsage {
    /// Tokens read as input.
    pub input_tokens: i64,
    /// Of those, tokens read from a cache.
    pub cached_input_tokens: i64,
    /// Of the input, tokens written to a cache. Older records omit zero.
    #[serde(default, skip_serializing_if = "usage_is_zero")]
    pub cache_write_tokens: i64,
    /// Tokens written as output.
    pub output_tokens: i64,
    /// Of those, tokens spent on reasoning.
    pub reasoning_tokens: i64,
    /// All of them.
    pub total_tokens: i64,
}

fn usage_is_zero(count: &i64) -> bool {
    *count == 0
}

/// Counts reported separately by an upstream, combined by TokenUsage's owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokensReportedApart {
    /// Input neither read from nor written to a cache.
    pub input: i64,
    /// Input read from a cache.
    pub cache_read: i64,
    /// Input written to a cache.
    pub cache_write: i64,
    /// Output other than reasoning.
    pub output: i64,
    /// Reasoning.
    pub reasoning: i64,
}

impl TokenUsage {
    /// Combine independently reported parts without counting cache or reasoning twice.
    #[must_use]
    pub fn from_parts(parts: TokensReportedApart) -> Self {
        let input = parts.input + parts.cache_read + parts.cache_write;
        let output = parts.output + parts.reasoning;
        Self {
            input_tokens: input,
            cached_input_tokens: parts.cache_read,
            cache_write_tokens: parts.cache_write,
            output_tokens: output,
            reasoning_tokens: parts.reasoning,
            total_tokens: input + output,
        }
    }

    /// The sum of two usages.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens + other.input_tokens,
            cached_input_tokens: self.cached_input_tokens + other.cached_input_tokens,
            cache_write_tokens: self.cache_write_tokens + other.cache_write_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            reasoning_tokens: self.reasoning_tokens + other.reasoning_tokens,
            total_tokens: self.total_tokens + other.total_tokens,
        }
    }

    /// Nonnegative counts whose parts and total agree.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.total_tokens == self.input_tokens + self.output_tokens
            && self.cached_input_tokens + self.cache_write_tokens <= self.input_tokens
            && self.reasoning_tokens <= self.output_tokens
            && [
                self.input_tokens,
                self.cached_input_tokens,
                self.cache_write_tokens,
                self.output_tokens,
                self.reasoning_tokens,
            ]
            .iter()
            .all(|count| *count >= 0)
    }
}

/// What a thread used in all, with how full its context is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadUsage {
    /// The tokens.
    #[serde(flatten)]
    pub tokens: TokenUsage,
    /// Whether the program reports token totals. When it does not (it says so in its descriptor), `tokens` are zero
    /// and mean nothing: the totals are not reported, and a screen says that instead of a count.
    #[serde(
        default = "tokens_reported_by_default",
        skip_serializing_if = "tokens_are_reported"
    )]
    pub tokens_reported: bool,
    /// How many tokens the model's context holds.
    pub context_window: Option<i64>,
    /// How many of them are used.
    pub context_used: Option<i64>,
    /// Where the harness shortens the conversation, in tokens; `null` where that is not known.
    #[serde(default)]
    pub compact_at: Option<i64>,
    /// When this was last updated.
    pub updated_at: Time,
}

const fn tokens_reported_by_default() -> bool {
    true
}

const fn tokens_are_reported(reported: &bool) -> bool {
    *reported
}

/// Credits already counted in the ledger, with estimates and missing rates kept explicit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordedCost {
    /// Counted credits; null when nothing had a rate.
    pub credits: Option<String>,
    /// The counted share estimated by the shipped catalogue.
    pub estimated_credits: String,
    /// Tokens for which no price was known.
    pub unrated_tokens: i64,
}

/// The current turn's counted credits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnRecordedCost {
    /// The turn these figures belong to.
    pub turn_id: Id,
    /// Counted amounts.
    #[serde(flatten)]
    pub amounts: RecordedCost,
}

/// Cost summaries carried by the ordinary thread feed instead of timer-driven detail reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadCosts {
    /// All counted spending on this thread, retaining the rates recorded at the time.
    pub total: RecordedCost,
    /// The turn under way, if any.
    pub turn: Option<TurnRecordedCost>,
}

/// How many side threads a thread has open and may have, and how one reads here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSides {
    /// How many are open.
    pub open: i64,
    /// How many it may have.
    pub max: i64,
    /// Whether any may be started here.
    pub allowed: bool,
    /// How an aside here reads without acting, as the thread's harness verified it; `null` where none can.
    pub mode: Option<SideReading>,
    /// Why an aside cannot start here; `null` where it can.
    pub reason: Option<String>,
}

/// How a side conversation reads and never acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SideReading {
    /// The harness's own enforced read-only mode.
    ReadOnly,
    /// Every permission request declined before any effect, with only verified reading admitted.
    ApprovalGated,
    /// No tools at all: only the conversation so far and the message.
    NoTools,
}

/// Why a thread cannot go on, in detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadBlocked {
    /// The reason.
    pub reason: BlockedReason,
    /// A sentence about this occurrence.
    pub detail: Option<String>,
    /// A current sandbox refusal and permitted recovery acts, when this is the cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<crate::environment::EnvironmentSandboxReadiness>,
}

/// The thread's queue as a person's Stop left it. A nonempty Stop hold also holds new arrivals,
/// appended after the work already waiting. They stay editable in the queue until somebody who
/// drives the thread resumes it (naming this revision), or removes its last input. An empty Stop
/// creates no hold. No active turn is needed to read or resume one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadQueue {
    /// Whether a Stop holds inputs.
    pub held: bool,
    /// Who stopped; `null` when nothing is held.
    pub by: Option<UserRef>,
    /// When; `null` when nothing is held.
    pub at: Option<Time>,
    /// How many inputs the hold still keeps.
    pub held_inputs: i64,
    /// Changes with arrivals, content, delivery, order, state and holds. A resume names
    /// the current revision so that it resumes the queue the person actually saw.
    pub revision: Revision,
}

/// Resumes what a Stop held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadQueueResumeBody {
    /// The `Thread.queue.revision` the person saw.
    pub revision: Revision,
}

/// Which collection of an input's part a file that could not be delivered is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentCollection {
    /// The part itself.
    Part,
    /// The images of a forwarded part.
    Images,
    /// The files of a forwarded part.
    Attachments,
}

/// Why a file of an input cannot reach the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentIssueCode {
    /// The model the turn would run on cannot take it (an image to a model without images).
    Unsupported,
    /// It could not be written where the agent reads it.
    DeliveryFailed,
}

/// One file of an input that holds it before a turn or a steer: where it is in the input, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentIssue {
    /// The part, counted from zero, in the input's content.
    pub part: i64,
    /// The part itself, or one of a forwarded part's collections.
    pub collection: AttachmentCollection,
    /// The place in that collection, counted from zero; `null` for the part itself.
    pub index: Option<i64>,
    /// The file's name.
    pub name: String,
    /// Why.
    pub code: AttachmentIssueCode,
    /// A safe sentence: no credential, signed address or bytes of the file.
    pub message: String,
    /// Whether trying again may help.
    pub retryable: bool,
}

/// Who keeps the notes an agent makes for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKeeper {
    /// Its harness, with a memory of its own.
    Harness,
    /// The agent, with its tools, as well as it can.
    Agent,
    /// Nobody.
    Nobody,
}

/// Why a harness does not keep an agent's notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWhy {
    /// The memory setting is off.
    SettingOff,
    /// The memory setting asks for the harness alone.
    SettingHarnessOnly,
    /// The harness has no memory.
    HarnessNone,
    /// The harness keeps one memory for its whole home, which is always switched off.
    HarnessHome,
    /// It runs on its provider's side, away from the directory.
    WithProvider,
}

/// Where the notes an agent makes for itself are kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadMemory {
    /// Who keeps them in the directory's `.orch/memory` now.
    pub kept_by: MemoryKeeper,
    /// Whether this harness, as placed for this thread, has a memory of its own that it can keep in
    /// the directory.
    pub native: bool,
    /// Why the harness does not keep them; `null` where it does.
    pub why: Option<MemoryWhy>,
}

/// Whether the caller may give a thread input now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadInputAccess {
    /// Whether they may.
    pub allowed: bool,
    /// When not, everything they lack for it. It drives the composer.
    pub missing: Vec<MissingAccess>,
}

/// What the caller may do with a thread beyond reading it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadCan {
    /// Fork it: they hold its advanced row, or manage it.
    pub fork: bool,
    /// Archive it.
    pub archive: bool,
    /// Take its conversation away as a file: they hold its Export row (`thread.export`).
    pub export: bool,
    /// Whether they manage its environment, and so may change the thread's directory, how far it
    /// is confined, and which environment it is in.
    pub place: bool,
}

/// A thread: a conversation with an agent, in a topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Thread {
    /// Selected execution, independent of the read selector; omitted only by an older producer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_continuation_id: Option<Id>,
    /// Monotonic selection CAS revision; a reader cannot infer it from item sequences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_revision: Option<i64>,
    /// What every node has.
    #[serde(flatten)]
    pub base: NodeBase<ThreadKind>,
    /// The topic it belongs to.
    pub topic_id: Id,
    /// What it is doing.
    pub state: ThreadState,
    /// The server-selected icon and its safe particulars for this reader.
    #[serde(default)]
    pub display: ThreadDisplay,
    /// Set on a side thread only: the thread it was taken from, its parent.
    pub side_of: Option<NodeRef>,
    /// For a side thread, the last turn of that parent whose history it took.
    pub side_from_turn: Option<i64>,
    /// Only on a thread that is not itself a side thread: its side threads.
    pub sides: Option<ThreadSides>,
    /// For a side thread: how it reads without acting; `null` on every other thread.
    pub side_reading: Option<SideReading>,
    /// What a starting thread is waiting for.
    pub starting_step: Option<StartingStep>,
    /// Why it cannot go on.
    pub blocked: Option<ThreadBlocked>,
    /// What a person's Stop holds of its queue.
    pub queue: ThreadQueue,
    /// What it runs on; `null` until it is bound.
    pub binding: Option<ThreadBinding>,
    /// Where the thread's tools run. Somebody managing that environment put the thread there, and
    /// other threads may share it; it is `null` only when the environment was removed.
    pub environment: Option<EnvironmentRef>,
    /// What the caller holds on that environment, sorted: `environment.view` and `environment.manage`
    /// and the rows of its four doors (`files.*`, `terminal.*`, `browser.*`, `vnc.*`). It is what the
    /// views beside the conversation ask, since a thread's own page holds none of them. Empty without
    /// an environment.
    pub environment_permissions: Vec<String>,
    /// The thread's directory in that environment, relative to the workspace root; the empty string
    /// is the root itself. The agent starts there and the file panel opens there, though whoever
    /// looks into the environment reaches the whole workspace.
    pub directory: String,
    /// Safe source preparation or own-worktree facts, when configured.
    pub source: Option<crate::source::SourceInfo>,
    /// Links explicitly accepted by an interactor.
    pub links: Vec<crate::source::ThreadLink>,
    /// Safe pull-request links from the most recent answer, accepted only by an interactor.
    #[serde(default)]
    pub suggested_links: Vec<crate::source::ThreadLink>,
    /// The last requested Git work observation, with unknowns and age retained.
    pub work: Option<crate::source::DirectoryWork>,
    /// Independent work flags and an unresolved last failed turn.
    pub attention: crate::source::ThreadAttention,
    /// The model's routing name, as resolved from the thread's settings.
    pub model: Option<String>,
    /// The model's display name on the thread's provider.
    pub model_display: Option<String>,
    /// The effort, one of that model's; `null` when it takes none.
    pub effort: Option<String>,
    /// The absolute path in the environment where the agent starts.
    pub cwd: String,
    /// How many other threads work in the same directory, sharing its
    /// instruction file and what an agent noted there.
    pub directory_threads: i64,
    /// Where its agent's notes are kept.
    pub memory: ThreadMemory,
    /// The switch, move, recreation or update that currently holds the thread.
    pub lock: Option<JobRef>,
    /// Whether the caller may send it input at this moment.
    pub input_access: ThreadInputAccess,
    /// What the caller may do with it.
    pub can: ThreadCan,
    /// The turn under way.
    pub current_turn_id: Option<Id>,
    /// The segment under way.
    pub current_segment_id: Option<Id>,
    /// How many approvals wait.
    pub pending_approvals: i64,
    /// How many inputs wait.
    pub pending_inputs: i64,
    /// Its goal: the one under way, else the last that ended and was not cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<crate::goals::ThreadGoal>,
    /// What it used in all.
    pub usage: Option<ThreadUsage>,
    /// Recorded ledger credits; absent on older projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub costs: Option<ThreadCosts>,
    /// When something last happened in it.
    pub last_activity_at: Option<Time>,
    /// The highest thread sequence issued.
    pub seq: Seq,
    /// The sequence of the last change worth a reader's attention.
    pub activity_seq: Seq,
    /// How far the caller has read.
    pub my_last_read_seq: Option<Seq>,
}

/// What kind of item a timeline entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// What a person said to the agent.
    UserMessage,
    /// What the agent said.
    AgentMessage,
    /// The agent's reasoning, or a summary of it.
    Reasoning,
    /// The agent's plan.
    Plan,
    /// A call of a tool.
    ToolCall,
    /// A request for approval.
    Approval,
    /// An announcement that reached the thread.
    Announcement,
    /// A line of the thread's history.
    Marker,
    /// Something that went wrong.
    Error,
}

/// How far an item has come: progress of the agent's work, permission decisions,
/// and answers to requests for person-supplied content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Still being produced.
    InProgress,
    /// Done.
    Completed,
    /// Failed.
    Failed,
    /// Stopped.
    Cancelled,
    /// An approval that waits.
    Pending,
    /// An approval that was given.
    Approved,
    /// A harness question or form for which a person supplied content.
    Answered,
    /// An approval that was refused.
    Denied,
    /// An approval that ran out.
    Expired,
}

/// How an input reaches the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Added to the queue; if the thread is idle it starts a turn right away.
    Queue,
    /// Delivered into the turn that is running; first in the queue otherwise.
    Steer,
    /// Interrupts the running turn, then goes first in the queue.
    Interrupt,
}

/// Where an input came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    /// A person.
    User,
    /// An announcement that was sent to the thread.
    Announcement,
    /// The system.
    System,
    /// The loop of the thread's goal, in the name of the person who set it.
    Goal,
}

/// The kinds of image an input may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ImageMediaType {
    /// PNG.
    #[serde(rename = "image/png")]
    Png,
    /// JPEG.
    #[serde(rename = "image/jpeg")]
    Jpeg,
    /// WebP.
    #[serde(rename = "image/webp")]
    Webp,
    /// GIF.
    #[serde(rename = "image/gif")]
    Gif,
}

/// Where the original of a forwarded message is, for readers who may go there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForwardedFrom {
    /// A chat.
    Chat {
        /// The chat; redacted for those who may not go there.
        chat: ChatRef,
    },
    /// A thread.
    Thread {
        /// The thread; redacted for those who may not go there.
        thread: NodeRef,
    },
}

/// An agent, named by what it is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentName {
    /// The agent's name.
    pub agent: String,
}

/// Who wrote a forwarded message: a person, or an agent known by its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ForwardedAuthor {
    /// A person.
    User(UserRef),
    /// An agent.
    Agent(AgentName),
}

/// An image of a forwarded message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForwardedImage {
    /// The image, by its hash.
    pub blob: Hash,
    /// Its type.
    pub media_type: String,
    /// Its name.
    pub name: String,
}

/// A file of a forwarded message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForwardedAttachment {
    /// The file, by its hash.
    pub blob: Hash,
    /// Its name.
    pub name: String,
    /// Its size in bytes.
    pub size: i64,
    /// Its type.
    pub media_type: String,
}

/// The discriminator of a forwarded message, including its standalone shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ForwardedType {
    /// A forwarded message.
    Forwarded,
}

/// A copy of a single message, taken when it was forwarded. In a thread, the copy's images and
/// files are placed in the agent's workspace when the input is delivered. As a part it is written
/// with `type` set to `forwarded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Forwarded {
    /// Always `forwarded`; the same tag in a chat and a conversation.
    pub r#type: ForwardedType,
    /// Where the original is.
    pub from: ForwardedFrom,
    /// The chat message or timeline item that it copies.
    pub source_id: Id,
    /// Who wrote the original.
    pub author: Option<ForwardedAuthor>,
    /// When the original was written.
    pub at: Time,
    /// Its text, as Markdown.
    pub text: String,
    /// Its images.
    pub images: Vec<ForwardedImage>,
    /// Its files.
    pub attachments: Vec<ForwardedAttachment>,
}

/// One part of what is said to an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Text.
    Text {
        /// The text.
        text: String,
    },
    /// An image that was uploaded.
    Image {
        /// The image, by its hash.
        blob: Hash,
        /// Its type.
        media_type: ImageMediaType,
        /// Its name.
        name: String,
        /// Its allocated relative destination; older records use their original input-numbered path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// A file sent with the message.
    File {
        /// Its path, relative to the thread's directory.
        path: String,
        /// Its name.
        name: String,
        /// Its size in bytes.
        size: i64,
        /// What to write to `path` when the input is delivered; `null` for a
        /// file that is in the workspace already.
        blob: Option<Hash>,
    },
    /// Pasted text kept folded while its original bytes remain readable and editable.
    #[serde(untagged)]
    PastedText(PastedText),
    /// A message forwarded into this thread.
    #[serde(untagged)]
    Forwarded(Box<Forwarded>),
}

/// The discriminator of folded pasted text in either conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PastedTextType {
    /// Folded pasted text.
    PastedText,
}

/// A folded text upload; delivery records the original words when they fit the remaining context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PastedText {
    /// The sender's choice for this paste; absent follows the place's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<PasteDelivery>,
    /// Always pasted_text, including the standalone schema shared by both part unions.
    pub r#type: PastedTextType,
    /// Its original UTF-8 bytes.
    pub blob: Hash,
    /// Its generated text filename.
    pub name: String,
    /// Its size in bytes.
    pub size: i64,
    /// The number of Unicode characters in the original.
    pub characters: i64,
    /// The ASCII characters, for the same language-aware token estimate as context fill.
    pub ascii_characters: i64,
    /// Where a file delivery will be written, relative to the thread's directory; empty for a chat.
    pub path: String,
    /// Words verified from the blob when sending; absent in drafts and for a file delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_text: Option<String>,
    /// Verified provenance when a chat's paste was forwarded or relayed to an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Box<Forwarded>>,
}

/// Whether one paste is sent in the message or supplied by its file path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PasteDelivery {
    /// Keep its verified words in the model's message, allowing native compaction.
    Inline,
    /// Supply the stored file for the agent to read when needed.
    File,
}

/// Long text of which only the end is kept in the timeline.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutputRef {
    /// The last [`OUTPUT_INLINE_MAX_BYTES`] at most.
    pub inline: String,
    /// How many bytes were produced so far.
    pub total_len: i64,
    /// Whether `inline` is shorter than the whole.
    pub truncated: bool,
    /// The whole, once the item is complete and it was longer than `inline`.
    pub blob: Option<Hash>,
}

impl OutputRef {
    /// A complete text, bounded inline by the owner's byte limit.
    #[must_use]
    pub fn of(text: &str) -> Self {
        let mut output = Self::default();
        output.push(text);
        output
    }

    /// Add a chunk, keeping the tail and cutting only at a UTF-8 boundary.
    pub fn push(&mut self, chunk: &str) {
        self.total_len += i64::try_from(chunk.len()).unwrap_or(i64::MAX);
        self.inline.push_str(chunk);
        let most = usize::try_from(OUTPUT_INLINE_MAX_BYTES).unwrap_or(usize::MAX);
        if self.inline.len() > most {
            let mut cut = self.inline.len() - most;
            while !self.inline.is_char_boundary(cut) {
                cut += 1;
            }
            self.inline.drain(..cut);
        }
        self.truncated = i64::try_from(self.inline.len()).unwrap_or(i64::MAX) < self.total_len;
    }

    /// Retain the complete blob only when inline text was truncated.
    #[must_use]
    pub fn with_blob(mut self, blob: Hash) -> Self {
        if self.truncated {
            self.blob = Some(blob);
        }
        self
    }
}

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
    /// The change as a unified diff with three context lines; `null` for a file that is not text
    /// and while the change is still being written.
    pub diff: Option<String>,
    /// Lines added.
    pub added: i64,
    /// Lines removed.
    pub removed: i64,
    /// Whether it is no text, so that it is shown by its size before and after.
    pub binary: bool,
    /// Whether the diff was cut at [`FILE_CHANGE_DIFF_MAX_BYTES`].
    pub truncated: bool,
}

/// A tool call exactly as the harness made it, with nothing summarised or left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RawCall {
    /// The harness's own name for the tool, such as `Bash` or `mcp__github__create_issue`.
    pub name: String,
    /// Its arguments as the harness passed them, as JSON text.
    pub input: OutputRef,
    /// What came back, as text. `null` while the call runs and for a call that was refused.
    pub output: Option<OutputRef>,
    /// Whether the harness marked the result as an error.
    pub is_error: bool,
}

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

/// What a search of files looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileSearchMode {
    /// A pattern in the files.
    Content,
    /// A pattern of names.
    Name,
}

/// A result of a web search.
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

/// The tools that Orch itself gives an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrchTool {
    /// Lists what was shared with the agent.
    ListShared,
    /// Reads something that was shared.
    ReadShared,
    /// Searches what was shared.
    SearchShared,
    /// Downloads something that was shared.
    DownloadShared,
    /// Edits a document.
    EditDocument,
    /// Organizes documents and folders, including version restore; only deletion requires confirmation.
    ManageShared,
    /// Posts to a chat or queues a note to a thread's agent, according to its typed target.
    SendMessage,
    /// Historical chat calls retained for displaying stored conversations; not an offered tool.
    SendChatMessage,
    /// Historical thread calls retained for displaying stored conversations; not an offered tool.
    SendThreadMessage,
    /// Announces a notice or a milestone.
    Announce,
    /// Asks for a capability.
    RequestCapability,
    /// Opens, joins, announces or closes a browser in the agent's environment.
    UseBrowser,
    /// Lists, opens, announces or closes a display in the agent's environment.
    UseDisplay,
    /// Lists the ports of the agent's environment, their outside addresses and who may reach them.
    ListPorts,
    /// Asks for a port of the agent's environment to be opened to more people, which a person decides.
    OpenPort,
    /// Closes an opening of a port that the thread asked for.
    ClosePort,
    /// Lists who reached a port of the agent's environment, with both ends of each connection.
    ListPortConnections,
    /// Reads the thread's goal and reports where the agent stands on it.
    Goal,
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
        /// The file's path: relative to the thread's directory if it lies inside it, absolute
        /// otherwise.
        path: String,
        /// The first line read.
        start_line: Option<i64>,
        /// The last line read.
        end_line: Option<i64>,
        /// How many lines the file has.
        total_lines: Option<i64>,
        /// What the agent was given: the text, or for an image its media type.
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
    /// An image that the agent viewed or produced.
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
        /// The stream of the sub-agent's own items.
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
    /// Anything else.
    Other {
        /// The tool's name.
        name: String,
        /// What it was called with.
        arguments: Option<Json>,
        /// What it returned.
        result: Option<Json>,
    },
}

/// What a person said, as an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserMessagePayload {
    /// The rule and failure behind a visible continuation of the original author's input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automatic: Option<AutomaticInput>,
    /// What was said.
    pub parts: Vec<ContentPart>,
    /// The input it came in as; `null` for words said in a session before Orch took it in, which came in as
    /// no input.
    pub input_id: Option<Id>,
    /// How it was delivered.
    pub delivery: Delivery,
    /// Where it came from.
    pub source: InputSource,
    /// What the input of a goal's loop says of its goal; the timeline shows a block made of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<crate::goals::GoalInputFacts>,
}

/// A continuation remains its input author's while naming the rule that sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AutomaticInput {
    /// The configured recovery rule.
    pub rule_id: String,
    /// The failure that interrupted work.
    pub kind: String,
    /// The attempt within the rule step.
    pub attempt: u32,
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

/// What the agent said, as an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentMessagePayload {
    /// What was said, as Markdown.
    pub text: String,
    /// Its length in Unicode code points so far, matching the offsets of text deltas.
    pub text_len: i64,
    /// How far into its work.
    pub phase: Option<AgentMessagePhase>,
}

/// The agent's reasoning, as an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningPayload {
    /// A summary of it.
    pub summary: String,
    /// The length of the whole in Unicode code points, matching the offsets of text deltas.
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

/// The agent's plan, as an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanPayload {
    /// What the plan is about.
    pub explanation: Option<String>,
    /// Its steps.
    pub steps: Vec<PlanStep>,
    /// The accepted response to a proposed plan; absent while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<crate::approvals::RequestDecisionStatus>,
    /// The sole decision record for this proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<Id>,
}

/// A call of a tool, as an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallPayload {
    /// The call as Orch understood it, in a form that lets the display suit its kind.
    pub tool: ToolPayload,
    /// A line to show for it.
    pub title: String,
    /// When it began.
    pub started_at: Time,
    /// How long it took, once it ended.
    pub duration_ms: Option<i64>,
    /// The identical call exactly as the harness issued it.
    pub raw: RawCall,
}

/// A request for approval. Harness cards carry their current projection for this reader,
/// so a question is usable with its timeline item without a separate approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalPayload {
    /// The approval.
    pub approval_id: Id,
    /// Current harness request and decision authority for this reader. Added at delivery,
    /// never stored or shared between readers. Older/non-harness items may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Box<crate::approvals::Approval>>,
}

/// What an announcement is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementCategory {
    /// A notice.
    Notice,
    /// A milestone.
    Milestone,
}

/// How much an announcement matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementSeverity {
    /// For information.
    Info,
    /// Important.
    Important,
}

/// A thread, named by what it is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadMention {
    /// The thread.
    pub id: Id,
    /// Its name.
    pub name: String,
}

/// Who an announcement is from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementFrom {
    /// The agent of another thread.
    Thread(ThreadMention),
    /// A person.
    User(UserRef),
}

/// An announcement, as an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnnouncementPayload {
    /// The announcement.
    pub announcement_id: Id,
    /// A notice or a milestone.
    pub category: AnnouncementCategory,
    /// True when another thread's agent sent it to this one thread alone, so
    /// that it is shown as a message from that thread.
    pub to_thread: bool,
    /// Its title.
    pub title: String,
    /// Its text.
    pub body: String,
    /// How much it matters.
    pub severity: AnnouncementSeverity,
    /// Who it is from.
    pub from: AnnouncementFrom,
}

/// Something that went wrong, as an item of the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorPayload {
    /// A stable code.
    pub code: String,
    /// What went wrong, in English.
    pub message: String,
    /// Whether trying again may help.
    pub retryable: bool,
}

/// The kinds of line a thread's history holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    /// A resource's resumption continued its stopped original task or kept a refusal visible.
    ResourceResumed,
    /// A resource's switch or deletion stopped the turn immediately.
    ResourceStopped,
    /// A configured rule retried, continued or failed over a model call.
    CallRecovery,
    /// What Orch supplied to the model, linked to its readable record.
    OrchContext,
    /// The first line of every thread.
    Created,
    /// A new title.
    Renamed,
    /// A person wrote the description.
    Described,
    /// A new model, effort, or both.
    ModelChanged,
    /// Another setting of the thread's own.
    SettingChanged,
    /// Its page was changed.
    AccessChanged,
    /// Its first owner changed.
    OwnerChanged,
    /// A segment began.
    SegmentStarted,
    /// A native history cache was removed or its conversation restored to a new session.
    AgentHistory,
    /// What it runs on changed.
    Switched,
    /// This thread was made as a fork.
    ForkedFrom,
    /// A fork was taken from this thread.
    Forked,
    /// Its environment's unit was replaced.
    EnvironmentRecreated,
    /// Its existing environment unit was stopped and started by a person.
    EnvironmentRestarted,
    /// Its environment moved to another host.
    EnvironmentMoved,
    /// Its environment was stopped.
    EnvironmentStopped,
    /// Its environment was started.
    EnvironmentStarted,
    /// What its agent works with there changed.
    EnvironmentTools,
    /// A capability was granted.
    CapabilityGranted,
    /// A capability was taken back.
    CapabilityRevoked,
    /// The harness shortened its own conversation.
    Compacted,
    /// The resumed session keeps its original instructions until a new session is made.
    InstructionsDeferred,
    /// A turn was let past limits.
    LimitBypassed,
    /// It, or a place above it, was archived.
    Archived,
    /// It, or a place above it, was taken out of the archive.
    Unarchived,
    /// It, or a place above it, was brought back after deletion.
    Restored,
    /// Something was shared with its agent.
    Shared,
    /// A live share ended.
    Unshared,
    /// A side thread was started.
    SideStarted,
    /// A side thread was closed.
    SideClosed,
    /// A person's Stop held what waited in the queue.
    QueueHeld,
    /// What a Stop held was resumed.
    QueueResumed,
    /// A narrow answer for this thread was allowed, taken back, or ended by a change of its binding.
    ApprovalRulesChanged,
    /// The frozen repository source became ready.
    SourcePrepared,
    /// One recorded file state was restored, with a complete undo state.
    FilesRestored,
    /// An interactor replaced the accepted labelled links.
    LinksChanged,
    /// The same main thread was moved to another topic of its tree.
    ThreadMoved,
    /// A session begun outside Orch was taken in as this thread.
    Adopted,
    /// A goal was set.
    GoalSet,
    /// A goal was edited.
    GoalChanged,
    /// A goal was paused.
    GoalPaused,
    /// A goal was set going again.
    GoalResumed,
    /// A goal was met.
    GoalMet,
    /// A goal stopped without being met.
    GoalStopped,
    /// A goal was taken away.
    GoalCleared,
}

/// One compact record of an automatic retry or continuation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerCallRecovery {
    /// Retry, continue or failover.
    pub action: String,
    /// The rule that reserved this attempt.
    pub rule: String,
    /// The failure kind that matched.
    pub kind: String,
    /// The attempt within its step.
    pub attempt: u32,
    /// The recorded step's attempt limit; absent in older history and steps without a limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    /// This line was caused by a configured rule.
    pub automatic: bool,
    /// The input whose author still answers for the work.
    pub input_id: Id,
    /// The configured destination's routing name, present for a failover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The provider of that model, used to inspect the configured destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
    /// Why a configured candidate was skipped, where this line records a skip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Why an adopted thread does not run on the model its session ran on, and a person chooses one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdoptModelReason {
    /// The harness did not say which model the session ran on.
    NotReported,
    /// The provider offers no model of that name, or not switched on.
    NotOffered,
    /// The model is not allowed here: the place's list of models, or what the person may use of the provider.
    NotAllowed,
}

/// Why nothing of what an adopted session held before is on the thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdoptHistoryMissing {
    /// The harness cannot read what it recorded of a session.
    Unsupported,
    /// It could not this time, or what it gave could not be read.
    Unreadable,
}

/// Availability of the original history of an adopted session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdoptHistoryState {
    /// Its earlier history was read, or it was empty.
    Read,
    /// The last read failed and may be retried.
    Unreadable,
    /// The adapter cannot read sessions; another attempt would do nothing.
    Unsupported,
    /// The session's machine is offline.
    Offline,
    /// Its provider, environment or machine can no longer be reached.
    Unavailable,
}

/// The before-Orch band's factual state and the caller's act, without asking the harness or starting work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdoptHistoryStatus {
    /// What is known of the earlier record.
    pub state: AdoptHistoryState,
    /// The harness's last failure words, or a local read failure, where available.
    pub reason: Option<String>,
    /// Current Manage and an available supported read; never a promise that a read will succeed.
    pub can_read_again: bool,
}

/// A session begun outside Orch was taken in as this thread: who took it in, what of what it held before came
/// with it, and the model it runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerAdopted {
    /// Who took it in.
    pub by: UserRef,
    /// The harness's own reference to the session.
    pub native_session_ref: String,
    /// How many of its earlier turns are on the thread, as items from the session before Orch.
    pub turns: i64,
    /// How many items they came to.
    pub items: i64,
    /// Why nothing of what it held before is there, where nothing is; `null` when it came, or there was none.
    pub history_missing: Option<AdoptHistoryMissing>,
    /// The actual failure of the earlier-history read, where there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_error: Option<String>,
    /// The model the session ran on, as its harness said; `null` where it did not say.
    pub session_model: Option<String>,
    /// Why the thread does not run on that model, where it does not: a person chooses one.
    pub model_reason: Option<AdoptModelReason>,
}

/// The queue held by a person's Stop, or resumed: who, how many inputs, and the queue's revision after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerQueue {
    /// Who held or resumed it; `null` when nobody did.
    pub by: Option<UserRef>,
    /// How many inputs it concerned.
    pub inputs: i64,
    /// The queue's revision after the change.
    pub revision: Revision,
}

/// Who moved this conversation between the two named topics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerThreadMoved {
    /// The topic it came from.
    pub from: NodeRef,
    /// The topic it now lies in.
    pub to: NodeRef,
    /// The person who moved it.
    pub by: UserRef,
}

/// What became of a narrow answer allowed for a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRuleAction {
    /// A person allowed it.
    Allowed,
    /// A person took it back, or the thread's binding changed.
    Revoked,
}

/// A narrow answer of a thread, as its line in the thread says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerApprovalRule {
    /// The rule.
    pub rule_id: Id,
    /// What became of it.
    pub action: ApprovalRuleAction,
    /// Who did it; `null` when the thread's binding changed.
    pub by: Option<UserRef>,
}

/// The first line of every thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerCreated {
    /// Who made it.
    pub by: UserRef,
    /// The agent profile it started with.
    pub agent: Option<AgentRef>,
    /// The provider it started with.
    pub provider: Option<ProviderRef>,
    /// The model.
    pub model: Option<String>,
    /// The effort.
    pub effort: Option<String>,
    /// The environment.
    pub environment: Option<EnvironmentRef>,
}

/// A new title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerRenamed {
    /// The old title.
    pub from: String,
    /// The new one.
    pub to: String,
    /// Who renamed it; `null` when automation wrote the title.
    pub by: Option<UserRef>,
}

/// A person wrote the description; descriptions written by automation leave no line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerDescribed {
    /// Who wrote it.
    pub by: UserRef,
}

/// A change of effort.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerEffortChange {
    /// The old effort.
    pub from: Option<String>,
    /// The new effort.
    pub to: Option<String>,
}

/// A new model, effort, or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerModelChanged {
    /// The old model.
    pub from: Option<String>,
    /// The new model.
    pub to: String,
    /// The effort before and after.
    pub effort: MarkerEffortChange,
    /// Who changed it.
    pub by: UserRef,
}

/// Another of the thread's own settings changed. The instructions are never shown: both values are
/// `null` and the line merely says that they changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerSettingChanged {
    /// The setting's key.
    pub key: String,
    /// What to call it.
    pub label: String,
    /// The old value.
    pub from: Option<Json>,
    /// The new value.
    pub to: Option<Json>,
    /// Who changed it.
    pub by: UserRef,
}

/// The page of the thread was changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerAccessChanged {
    /// Who was added or removed where, or that the page was made private or follows its topic
    /// again.
    pub summary: String,
    /// Who changed it.
    pub by: UserRef,
}

/// The first owner changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerOwnerChanged {
    /// The old first owner.
    pub from: UserRef,
    /// The new one.
    pub to: UserRef,
    /// Who made the change.
    pub by: UserRef,
}

/// A segment began.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerSegmentStarted {
    /// The segment.
    pub segment_id: Id,
    /// Its number in the thread.
    pub ordinal: i64,
    /// The agent profile.
    pub agent: AgentRef,
    /// The provider.
    pub provider: ProviderRef,
    /// Whether it took up a session that existed.
    pub resumed: bool,
}

/// A native history cache's confirmed removal or reconstruction from Orch's conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerAgentHistory {
    /// The native history's source segment.
    pub segment_id: Id,
    /// The familiar agent program name.
    pub program: String,
    /// Measured bytes before removal, or unknown.
    pub bytes: Option<i64>,
    /// When removal was confirmed, or unknown for unexpectedly missing history.
    pub removed_at: Option<Time>,
    /// Who requested removal, or automatic retention when absent.
    pub by: Option<UserRef>,
    /// The history rule used for a new session; absent for the removal itself.
    pub recovered: Option<AgentHistoryRecovery>,
}

/// How the new native session received the recorded conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentHistoryRecovery {
    /// The adapter imported recorded turns natively.
    Native,
    /// Recent turns were carried word for word beside the transcript file.
    Carry,
    /// A configured summary accompanied the transcript file.
    Summary,
    /// Brief recent turns accompanied the transcript when a summary or model window was unavailable.
    Brief,
    /// Only the indexed full transcript file was available to the new session.
    File,
    /// The place explicitly selected a fresh session without history.
    None,
}

/// What a thread ran on before a switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerSwitchedFrom {
    /// The agent profile.
    pub agent: AgentRef,
    /// The provider.
    pub provider: ProviderRef,
    /// The environment, if it had one.
    pub environment: Option<EnvironmentRef>,
    /// The directory there.
    pub directory: String,
}

/// What a thread runs on after a switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerSwitchedTo {
    /// The agent profile.
    pub agent: AgentRef,
    /// The provider.
    pub provider: ProviderRef,
    /// The environment.
    pub environment: EnvironmentRef,
    /// The directory there.
    pub directory: String,
}

/// The thread now runs on something else: another provider or profile, another environment, or
/// another directory in it. Its owner and its page are unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerSwitched {
    /// Before.
    pub from: MarkerSwitchedFrom,
    /// After.
    pub to: MarkerSwitchedTo,
    /// How the history travelled.
    pub report: HistoryReport,
    /// Who switched it.
    pub by: UserRef,
}

/// In a new thread: it was made as a fork of another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerForkedFrom {
    /// The thread it was taken from.
    pub thread: NodeRef,
    /// The turn whose end it took.
    pub turn_id: Id,
    /// How the history travelled.
    pub report: HistoryReport,
    /// Who forked it.
    pub by: UserRef,
}

/// Recorded in the thread that the fork was taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerForked {
    /// The new thread.
    pub into: NodeRef,
    /// The turn whose end it took.
    pub turn_id: Id,
    /// Who forked it.
    pub by: UserRef,
}

/// Why the unit of an environment was replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecreatedReason {
    /// An update was applied.
    Update,
    /// A person asked.
    Recreate,
    /// Its template changed.
    Template,
    /// Its configuration changed.
    Configuration,
    /// A capability was granted or taken back.
    Capability,
    /// The old unit failed.
    Failure,
}

/// The template an environment was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerTemplate {
    /// The template.
    pub id: Id,
    /// Its name.
    pub name: String,
}

/// The unit of the thread's environment was replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerEnvironmentRecreated {
    /// Which unit it is now.
    pub generation: i64,
    /// Why.
    pub reason: RecreatedReason,
    /// The template.
    pub template: MarkerTemplate,
}

/// The thread's environment moved to another host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerEnvironmentMoved {
    /// The environment before.
    pub from: EnvironmentRef,
    /// The environment after.
    pub to: EnvironmentRef,
    /// How paths changed.
    pub path_map: PathMap,
    /// The mounts that were detached.
    pub mounts_detached: Vec<String>,
    /// Who moved it.
    pub by: UserRef,
}

/// Why an environment was stopped or started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentRunReason {
    /// A person did it.
    Person,
    /// It stopped for being idle.
    Idle,
    /// The recycle did.
    Recycle,
    /// Started because a turn needed it.
    Turn,
}

/// The unit of an environment was stopped, which ends what ran there, or started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerEnvironmentRun {
    /// The environment.
    pub environment: EnvironmentRef,
    /// Why.
    pub reason: EnvironmentRunReason,
    /// Who did it, if a person did.
    pub by: Option<UserRef>,
}

/// What the agent works with in the environment changed. It applies from the thread's next turn or
/// session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerEnvironmentTools {
    /// What changed: what the exec node offers, Orch's own tools, or an MCP server.
    pub summary: String,
    /// Who changed it.
    pub by: UserRef,
}

/// A capability was granted or taken back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerCapability {
    /// The capability.
    pub capability: String,
    /// Who did it; `null` when policy allowed it automatically.
    pub by: Option<UserRef>,
}

/// The harness shortened its own conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerCompacted {
    /// How long it took, where the harness said when it began.
    pub duration_ms: Option<i64>,
}

/// The resumed session keeps its original instructions until a new session is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerInstructionsDeferred {
    /// The program that keeps the original instructions.
    pub program: String,
}

/// A limit that was reached, with what it showed then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerLimit {
    /// The cap; `null` for the provider's own limit.
    pub cap: Option<Id>,
    /// What to call it.
    pub label: String,
    /// How much was used, as a decimal string.
    pub used: String,
    /// How much was allowed, as a decimal string.
    pub amount: Option<String>,
}

/// Somebody let a turn past limits that were reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerLimitBypassed {
    /// Who let it past.
    pub by: UserRef,
    /// The turn.
    pub turn_id: Id,
    /// The limits and what each showed.
    pub limits: Vec<MarkerLimit>,
}

/// The thread itself, or a place above it, was archived, taken out of the
/// archive, or brought back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerNodeAct {
    /// What the act was done to.
    pub node: NodeRef,
    /// Who did it.
    pub by: UserRef,
}

/// What was shared with an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SharedThing {
    /// A node.
    Node(NodeRef),
    /// A chat.
    Chat(ChatRef),
    /// A document, named as it was when shared.
    Document(MarkerShareDocument),
}

/// The document named by a share-history line, without carrying its text or current permission page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerShareDocument {
    /// Its identifier.
    pub id: Id,
    /// Its title at the time of the share.
    pub title: String,
}

/// How something was shared with an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShareMode {
    /// A copy, written once.
    Snapshot,
    /// A share that follows the original.
    Live,
}

/// Something was shared with the thread's agent, or a live share ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerShare {
    /// The areas and levels at this moment, when retained by a current producer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<crate::share::ShareParts>,
    /// Whether included document material was offered for first reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_first: Option<bool>,
    /// What was shared.
    pub what: SharedThing,
    /// A copy or a live share.
    pub mode: ShareMode,
    /// Where a snapshot was written.
    pub path: Option<String>,
    /// Who did it.
    pub by: UserRef,
}

/// A side thread was started, in the thread it was taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerSideStarted {
    /// The side thread.
    pub side: NodeRef,
    /// The point of the conversation at which it was started.
    pub from_turn: i64,
    /// Who started it.
    pub by: UserRef,
}

/// Why a side thread was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SideClosedWhy {
    /// Somebody closed it.
    Closed,
    /// It was left alone for too long.
    Idle,
    /// It was turned into a thread in its own right.
    Kept,
    /// Its parent was closed.
    Parent,
}

/// A side thread was closed; nothing of what was said in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerSideClosed {
    /// The side thread.
    pub side: NodeRef,
    /// How many turns it had.
    pub turns: i64,
    /// Who closed it, if somebody did.
    pub by: Option<UserRef>,
    /// Why.
    pub why: SideClosedWhy,
    /// For `kept`: the thread it became.
    pub into: Option<NodeRef>,
}

/// A line of the thread's history, shown in the timeline at the point where it happened. Whoever
/// changes the thread through an act that needs a permission on it writes one, in the same
/// transaction and naming themself; so does whatever happens to its environment that the work there
/// would notice. A marker states a few facts and carries no content: no file text, no shell output,
/// no conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "marker", content = "data", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "a marker is built once to be written or read; boxing its data would put a layer in every use"
)]
pub enum MarkerPayload {
    /// The factual result of continuing original work after a resource becomes available.
    ResourceResumed(MarkerResourceResumed),
    /// A resource's switch or deletion stopped the turn immediately.
    ResourceStopped(MarkerResourceStopped),
    /// A visible automatic recovery step.
    CallRecovery(MarkerCallRecovery),
    /// A recorded block of context supplied by Orch.
    OrchContext(MarkerOrchContext),
    /// The first line of every thread.
    Created(MarkerCreated),
    /// A new title.
    Renamed(MarkerRenamed),
    /// A person wrote the description.
    Described(MarkerDescribed),
    /// A new model, effort, or both.
    ModelChanged(MarkerModelChanged),
    /// Another setting of the thread's own.
    SettingChanged(MarkerSettingChanged),
    /// Its page was changed.
    AccessChanged(MarkerAccessChanged),
    /// Its first owner changed.
    OwnerChanged(MarkerOwnerChanged),
    /// A segment began.
    SegmentStarted(MarkerSegmentStarted),
    /// A native history cache was removed or restored from the recorded conversation.
    AgentHistory(MarkerAgentHistory),
    /// What it runs on changed.
    Switched(MarkerSwitched),
    /// This thread was made as a fork.
    ForkedFrom(MarkerForkedFrom),
    /// A fork was taken from this thread.
    Forked(MarkerForked),
    /// The unit of its environment was replaced.
    EnvironmentRecreated(MarkerEnvironmentRecreated),
    /// Its existing environment unit was stopped and started, with every file kept.
    EnvironmentRestarted(MarkerEnvironmentRun),
    /// Its environment moved to another host.
    EnvironmentMoved(MarkerEnvironmentMoved),
    /// Its environment was stopped.
    EnvironmentStopped(MarkerEnvironmentRun),
    /// Its environment was started.
    EnvironmentStarted(MarkerEnvironmentRun),
    /// What its agent works with there changed.
    EnvironmentTools(MarkerEnvironmentTools),
    /// A capability was granted.
    CapabilityGranted(MarkerCapability),
    /// A capability was taken back.
    CapabilityRevoked(MarkerCapability),
    /// The harness shortened its own conversation.
    Compacted(MarkerCompacted),
    /// The resumed session keeps its original instructions until a new session is made.
    InstructionsDeferred(MarkerInstructionsDeferred),
    /// A turn was let past limits.
    LimitBypassed(MarkerLimitBypassed),
    /// It, or a place above it, was archived.
    Archived(MarkerNodeAct),
    /// It, or a place above it, was taken out of the archive.
    Unarchived(MarkerNodeAct),
    /// It, or a place above it, was brought back after deletion.
    Restored(MarkerNodeAct),
    /// Something was shared with its agent.
    Shared(MarkerShare),
    /// A live share ended.
    Unshared(MarkerShare),
    /// A side thread was started.
    SideStarted(MarkerSideStarted),
    /// A side thread was closed.
    SideClosed(MarkerSideClosed),
    /// A person's Stop held what waited in the queue.
    QueueHeld(MarkerQueue),
    /// What a Stop held was resumed.
    QueueResumed(MarkerQueue),
    /// A narrow answer for this thread was allowed, taken back, or ended by a change of its binding.
    ApprovalRulesChanged(MarkerApprovalRule),
    /// Safe source facts after one preparation job.
    SourcePrepared(MarkerSourcePrepared),
    /// The recorded target and complete undo of one restore act.
    FilesRestored(MarkerFilesRestored),
    /// Who replaced the links and how many the new list holds.
    LinksChanged(MarkerLinksChanged),
    /// The same conversation moved between topics, retaining its owner and workspace.
    ThreadMoved(MarkerThreadMoved),
    /// A session begun outside Orch was taken in as this thread.
    Adopted(MarkerAdopted),
    /// A goal was set.
    GoalSet(crate::goals::MarkerGoalSet),
    /// A goal was edited.
    GoalChanged(crate::goals::MarkerGoalChanged),
    /// A goal was paused.
    GoalPaused(crate::goals::MarkerGoalPaused),
    /// A goal was set going again.
    GoalResumed(crate::goals::MarkerGoalResumed),
    /// A goal was met.
    GoalMet(crate::goals::MarkerGoalMet),
    /// A goal stopped without being met.
    GoalStopped(crate::goals::MarkerGoalStopped),
    /// A goal was taken away.
    GoalCleared(crate::goals::MarkerGoalCleared),
}

impl MarkerPayload {
    /// The kind of line this is.
    #[must_use]
    pub const fn kind(&self) -> MarkerKind {
        match self {
            Self::ResourceResumed(_) => MarkerKind::ResourceResumed,
            Self::ResourceStopped(_) => MarkerKind::ResourceStopped,
            Self::CallRecovery(_) => MarkerKind::CallRecovery,
            Self::OrchContext(_) => MarkerKind::OrchContext,
            Self::Created(_) => MarkerKind::Created,
            Self::Renamed(_) => MarkerKind::Renamed,
            Self::Described(_) => MarkerKind::Described,
            Self::ModelChanged(_) => MarkerKind::ModelChanged,
            Self::SettingChanged(_) => MarkerKind::SettingChanged,
            Self::AccessChanged(_) => MarkerKind::AccessChanged,
            Self::OwnerChanged(_) => MarkerKind::OwnerChanged,
            Self::SegmentStarted(_) => MarkerKind::SegmentStarted,
            Self::AgentHistory(_) => MarkerKind::AgentHistory,
            Self::Switched(_) => MarkerKind::Switched,
            Self::ForkedFrom(_) => MarkerKind::ForkedFrom,
            Self::Forked(_) => MarkerKind::Forked,
            Self::EnvironmentRecreated(_) => MarkerKind::EnvironmentRecreated,
            Self::EnvironmentRestarted(_) => MarkerKind::EnvironmentRestarted,
            Self::EnvironmentMoved(_) => MarkerKind::EnvironmentMoved,
            Self::EnvironmentStopped(_) => MarkerKind::EnvironmentStopped,
            Self::EnvironmentStarted(_) => MarkerKind::EnvironmentStarted,
            Self::EnvironmentTools(_) => MarkerKind::EnvironmentTools,
            Self::CapabilityGranted(_) => MarkerKind::CapabilityGranted,
            Self::CapabilityRevoked(_) => MarkerKind::CapabilityRevoked,
            Self::Compacted(_) => MarkerKind::Compacted,
            Self::InstructionsDeferred(_) => MarkerKind::InstructionsDeferred,
            Self::LimitBypassed(_) => MarkerKind::LimitBypassed,
            Self::Archived(_) => MarkerKind::Archived,
            Self::Unarchived(_) => MarkerKind::Unarchived,
            Self::Restored(_) => MarkerKind::Restored,
            Self::Shared(_) => MarkerKind::Shared,
            Self::Unshared(_) => MarkerKind::Unshared,
            Self::SideStarted(_) => MarkerKind::SideStarted,
            Self::SideClosed(_) => MarkerKind::SideClosed,
            Self::QueueHeld(_) => MarkerKind::QueueHeld,
            Self::QueueResumed(_) => MarkerKind::QueueResumed,
            Self::ApprovalRulesChanged(_) => MarkerKind::ApprovalRulesChanged,
            Self::SourcePrepared(_) => MarkerKind::SourcePrepared,
            Self::FilesRestored(_) => MarkerKind::FilesRestored,
            Self::LinksChanged(_) => MarkerKind::LinksChanged,
            Self::ThreadMoved(_) => MarkerKind::ThreadMoved,
            Self::Adopted(_) => MarkerKind::Adopted,
            Self::GoalSet(_) => MarkerKind::GoalSet,
            Self::GoalChanged(_) => MarkerKind::GoalChanged,
            Self::GoalPaused(_) => MarkerKind::GoalPaused,
            Self::GoalResumed(_) => MarkerKind::GoalResumed,
            Self::GoalMet(_) => MarkerKind::GoalMet,
            Self::GoalStopped(_) => MarkerKind::GoalStopped,
            Self::GoalCleared(_) => MarkerKind::GoalCleared,
        }
    }
}

/// The captured person and resource responsible for stopping a running turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerResourceStopped {
    /// The resource's ordinary permission-page identity.
    pub object: orch_access_contract::decisions::ObjectRef,
    /// Its captured visible name.
    pub name: String,
    /// A model's exact routing name, or null for the whole resource.
    pub model: Option<String>,
    /// Who accepted the immediate stop.
    pub by: UserRef,
    /// The optional one-line note on the pause.
    pub note: Option<String>,
    /// Whether the resource was ended rather than paused.
    pub deleted: bool,
}

/// One original-author continuation attempted by an accepted resource resumption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerResourceResumed {
    /// The resource whose switch was restored.
    pub object: orch_access_contract::decisions::ObjectRef,
    /// Its captured name.
    pub name: String,
    /// A model's routing name, or null for the whole resource.
    pub model: Option<String>,
    /// The person accepting the resource operation, distinct from the original input author.
    pub by: UserRef,
    /// The exact interrupted turn this resource stopped.
    pub turn_id: Id,
    /// A stable reference unique to this operation and stopped turn.
    pub reference: Id,
    /// The actual accepted Continue input, or null when current authority refused it.
    pub input_id: Option<Id>,
    /// A safe refusal code, or null for an accepted continuation.
    pub reason: Option<String>,
}

/// Safe source preparation facts in a thread's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerSourcePrepared {
    /// The durable preparation job.
    pub job_id: Id,
    /// The prepared workspace-relative directory.
    pub directory: String,
    /// The resolved branch, when known.
    pub branch: Option<String>,
    /// The resolved commit, when known.
    pub head: Option<String>,
    /// Who authorized the preparation.
    pub by: UserRef,
}

/// One recorded-file restore with its complete saved prior state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerFilesRestored {
    /// The durable restore job.
    pub job_id: Id,
    /// The recorded target.
    pub snapshot_id: Id,
    /// The complete prior state offered as Undo.
    pub undo_snapshot_id: Id,
    /// How many regular files were restored.
    pub files: i64,
    /// Who authorized the restore.
    pub by: UserRef,
}

/// One accepted replacement of a thread's link list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerLinksChanged {
    /// The interactor who changed it.
    pub by: UserRef,
    /// How many links the resulting list holds.
    pub count: i64,
}

/// What an item holds, by its kind. It is written beside the item's other
/// members as `kind` and `payload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "an item is built once to be written or read; boxing its payload would put a layer in every use"
)]
pub enum ItemPayload {
    /// What a person said.
    UserMessage(UserMessagePayload),
    /// What the agent said.
    AgentMessage(AgentMessagePayload),
    /// The agent's reasoning.
    Reasoning(ReasoningPayload),
    /// The agent's plan.
    Plan(PlanPayload),
    /// A call of a tool.
    ToolCall(ToolCallPayload),
    /// A request for approval.
    Approval(ApprovalPayload),
    /// An announcement.
    Announcement(AnnouncementPayload),
    /// A line of the thread's history.
    Marker(MarkerPayload),
    /// Something that went wrong.
    Error(ErrorPayload),
}

impl ItemPayload {
    /// The kind of item this is.
    #[must_use]
    pub const fn kind(&self) -> ItemKind {
        match self {
            Self::UserMessage(_) => ItemKind::UserMessage,
            Self::AgentMessage(_) => ItemKind::AgentMessage,
            Self::Reasoning(_) => ItemKind::Reasoning,
            Self::Plan(_) => ItemKind::Plan,
            Self::ToolCall(_) => ItemKind::ToolCall,
            Self::Approval(_) => ItemKind::Approval,
            Self::Announcement(_) => ItemKind::Announcement,
            Self::Marker(_) => ItemKind::Marker,
            Self::Error(_) => ItemKind::Error,
        }
    }
}

/// A timeline pointer to the exact context recorded before it was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkerOrchContext {
    /// The readable record of the context, under this node's current view permission.
    pub model_input_id: Id,
    /// What it was supplied for.
    pub purpose: crate::model_inputs::ModelInputPurpose,
}

/// Who pinned an item, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ItemPin {
    /// When.
    pub at: Time,
    /// Who.
    pub by: UserRef,
}

/// An entry of a thread's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Item {
    /// Actual owning suffix, including an item referenced by another continuation's prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    /// The item's identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The segment it belongs to.
    pub segment_id: Id,
    /// The turn it belongs to, if it belongs to one.
    pub turn_id: Option<Id>,
    /// The exact model-input record this item stands beside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_input_id: Option<Id>,
    /// `main`, or `sub:` followed by the sub-agent's identifier.
    pub stream: String,
    /// The native item owning this sub-agent item or raising this content request.
    pub parent_item_id: Option<Id>,
    /// What it holds, written as `kind` and `payload`.
    #[serde(flatten)]
    pub payload: ItemPayload,
    /// How far it has come.
    pub status: ItemStatus,
    /// Who it is by, for what a person said.
    pub author: Option<UserRef>,
    /// Whether it is from the session before Orch: an earlier turn of a session a person adopted, as its
    /// harness recorded it. It is read-only, belongs to no turn, and is never sent to the agent again.
    #[serde(default)]
    pub before_orch: bool,
    /// Whether it is pinned, and by whom.
    pub pinned: Option<ItemPin>,
    /// How many messages of the thread's chat quote it.
    pub chat_refs: i64,
    /// The sequence at which it was made.
    pub seq_created: Seq,
    /// The sequence at which it last changed.
    pub seq_updated: Seq,
    /// When it was made.
    pub created_at: Time,
    /// When it last changed.
    pub updated_at: Time,
}

impl Item {
    /// The kind of item this is.
    #[must_use]
    pub const fn kind(&self) -> ItemKind {
        self.payload.kind()
    }
}

/// How far a turn has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    /// Getting ready.
    Starting,
    /// Under way.
    Running,
    /// Done.
    Completed,
    /// Stopped.
    Interrupted,
    /// Failed.
    Failed,
}

/// What a running turn is busy with, when its harness reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhaseKind {
    /// Working on the answer.
    Working,
    /// Shortening its own conversation to make room.
    Compacting,
}

/// What a running turn is doing, and since when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnPhase {
    /// What.
    pub kind: TurnPhaseKind,
    /// Since when.
    pub since: Time,
}

/// Limits that were let past, and by whom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LimitBypass {
    /// Who let it past, who may be somebody else than the author of the input.
    pub by: UserRef,
    /// When.
    pub at: Time,
    /// The caps it is let past.
    pub caps: Vec<Id>,
    /// Whether the provider's own limit, as the vendor last reported it, is passed as well.
    pub provider_limit: bool,
}

/// Why a turn failed: the code and the sentence Orch gives it, and what its harness or its vendor said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnError {
    /// A stable code.
    pub code: String,
    /// What went wrong, in English.
    pub message: String,
    /// Whether trying again may help.
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
    /// The id of the hit ProviderFacts.windows entry; null when unknown or no plan window applies.
    #[serde(default)]
    pub window: Option<String>,
    /// What the harness or its vendor said, word for word and masked like every stored text. It is kept
    /// whatever the adapter made of it, so that a failure nobody foresaw is still read by a person.
    #[serde(default)]
    pub said: Option<String>,
    /// For a limit of the plan: when the vendor says it ends.
    #[serde(default)]
    pub resets_at: Option<Time>,
    /// Whether the answer above it may have been cut short.
    #[serde(default)]
    pub incomplete: bool,
    /// For a limit: which one, as the adapter read it from the vendor's fields; a screen says it and offers what is
    /// left from it, never from the vendor's words. Absent where the vendor reported none.
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
    #[serde(default)]
    pub window_minutes: Option<i64>,
    /// The family of models it holds, where it holds only some, as the vendor names it.
    #[serde(default)]
    pub model_family: Option<String>,
    /// How long the vendor asks to wait before trying again, in seconds, where it says.
    #[serde(default)]
    pub retry_after_secs: Option<i64>,
}

/// One turn of the agent: its answer to one input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Turn {
    /// The turn's identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The segment.
    pub segment_id: Id,
    /// Its number in the thread.
    pub ordinal: i64,
    /// How far it has come.
    pub state: TurnState,
    /// The input it answers.
    pub input_id: Id,
    /// The complete or explicitly unavailable recorded files before this turn.
    pub before_snapshot_id: Option<Id>,
    /// The model's routing name.
    pub model: String,
    /// The model's display name.
    pub model_display: String,
    /// The effort.
    pub effort: Option<String>,
    /// The parameters it ran with.
    pub params: Json,
    /// What ended it, if it failed.
    pub error: Option<TurnError>,
    /// What it used.
    pub usage: Option<TokenUsage>,
    /// What its harness reported of its use beyond the totals: the sub-agents' and each model's share, the
    /// context it ended with and the times it was shortened; `null` where nothing more was reported.
    #[serde(default)]
    pub usage_detail: Option<crate::context::TurnUsageDetail>,
    /// What filled the context at its end, by part; `null` where nothing reported the size of a prompt.
    #[serde(default)]
    pub context: Option<crate::context::ContextFill>,
    /// Who stopped it.
    pub interrupted_by: Option<UserRef>,
    /// The stopped turn's actual native cleanup, including a report that arrives after completion.
    #[serde(default)]
    pub command_cleanup: Option<crate::background::CommandCleanup>,
    /// The limits it was let past when it started, and by whom.
    pub limit_bypass: Vec<LimitBypass>,
    /// What it is doing now; `null` once it has ended.
    pub phase: Option<TurnPhase>,
    /// The beginning of the current wait for a person; absent when no answer is pending.
    #[serde(default)]
    pub waiting_since: Option<Time>,
    /// Completed waits for people, in milliseconds, excluded from the turn's running time.
    #[serde(default)]
    pub waited_ms: i64,
    /// When it began.
    pub started_at: Option<Time>,
    /// When it ended.
    pub completed_at: Option<Time>,
    /// The sequence at which it last changed.
    pub seq_updated: Seq,
    /// Whether it planned: the agent proposed a plan and changed nothing until somebody proceeded.
    #[serde(default)]
    pub plan: bool,
}

/// Where an input stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputState {
    /// Waiting its turn.
    Pending,
    /// Being handed to the agent.
    Delivering,
    /// Handed over.
    Delivered,
    /// Cancelled before it was.
    Cancelled,
    /// It could not be.
    Failed,
}

/// What a person said to an agent, queued for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadInput {
    /// The actual owning queue/suffix; omitted only in older stored projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    /// The input's identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// Who said it; `null` when nobody did.
    pub author: Option<UserRef>,
    /// Who answers for its turn when that is not its author: somebody who ran it on their own use of the
    /// thread's model, or the thread's first owner where the place lets writers' messages run on theirs.
    #[serde(default)]
    pub run_as: Option<UserRef>,
    /// Who accepted this message's model-use wait and when; absent for an ordinary input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<crate::approvals::RequestDecisionStatus>,
    /// The sole decision record of this message's current or latest model-use wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<Id>,
    /// Whether it is worked in plan mode.
    #[serde(default)]
    pub plan: bool,
    /// Where it came from.
    pub source: InputSource,
    /// What was said.
    pub content: Vec<ContentPart>,
    /// The explicitly selected native action; ordinary text is never interpreted as its selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_action: Option<crate::input_schedules::NativeInvocation>,
    /// How it reaches the agent.
    pub delivery: Delivery,
    /// Where it stands.
    pub state: InputState,
    /// More about the state, such as `owner_lacks_access`, or `attachments` while its files hold it.
    pub state_detail: Option<String>,
    /// Empty when it is ready; otherwise the files that hold it before delivery, each with why.
    pub attachment_issues: Vec<AttachmentIssue>,
    /// Whether a person's Stop holds it; apart from the holds of its files and of caps.
    pub queue_held: bool,
    /// One entry per person who let it past limits that were reached.
    pub limit_bypass: Vec<LimitBypass>,
    /// Its place in the queue, as a decimal string; ascending is delivery order.
    pub position: String,
    /// The turn it started or joined.
    pub turn_id: Option<Id>,
    /// The value its sender chose to make sending it again safe.
    pub client_id: Option<String>,
    /// When it was made.
    pub created_at: Time,
    /// When it was delivered.
    pub delivered_at: Option<Time>,
    /// The sequence at which it last changed.
    pub seq_updated: Seq,
}

/// An input after a change, with every input changed by that same act.
/// Moving one pending input can renumber its neighbors: apply these rows together,
/// using their ordinary sequences, just as a thread feed does. An unchanged input
/// is still returned in `input`; `inputs` contains only rows actually stamped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadInputUpdateReply {
    /// The input the person changed, as it stands when the act commits.
    pub input: ThreadInput,
    /// Every input changed in the act, including renumbered neighbors, for a caller
    /// who holds `thread.view`. Empty for a drive-only caller: ordering does not
    /// grant the right to read other inputs. The requested `input` remains above.
    pub inputs: Vec<ThreadInput>,
}

/// Limits to let an input past: caps by identifier, and the provider's own limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LimitBypassRequest {
    /// The caps; one that has room is left out without an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps: Option<Vec<Id>>,
    /// Whether to let it past the provider's own limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_limit: Option<bool>,
}

/// The provider's own limit, as the vendor last reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderLimit {
    /// The provider.
    pub provider: ProviderRef,
    /// The window it counts in.
    pub window: String,
    /// When it resets.
    pub resets_at: Option<Time>,
    /// When the vendor said so.
    pub as_of: Time,
}

/// One limit that stands in the way of a turn, and who may let it past.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LimitReached {
    /// A cap that has no room, as it stands for the author of the input; `null` when the provider's
    /// own limit is what is reached.
    pub cap: Option<CapStatus>,
    /// The provider's own limit, when that is what is reached.
    pub provider_limit: Option<ProviderLimit>,
    /// Whether the caller holds the bypass row for it.
    pub can_bypass: bool,
    /// For an input that waits: who already let it past this limit.
    pub passed_by: Option<UserRef>,
    /// The nearest people who hold it, at most five.
    pub ask: Vec<UserRef>,
    /// How many more hold it.
    pub ask_more: i64,
}

/// What now stands in the way of a turn in the thread, and who may let it past.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadLimits {
    /// The limits that are reached.
    pub reached: Vec<LimitReached>,
    /// Whether the caller may pass every limit that nobody has passed yet, so that an option to go
    /// ahead anyway is shown.
    pub bypass_all: bool,
}

/// One stretch of a thread on one agent profile, provider and environment. A
/// switch ends one and begins the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Segment {
    /// The segment's identifier.
    pub id: Id,
    /// Its number in the thread.
    pub ordinal: i64,
    /// The agent profile; absent for read-only imported history.
    pub agent: Option<AgentRef>,
    /// The provider; absent for read-only imported history.
    pub provider: Option<ProviderRef>,
    /// The environment; absent for read-only imported history.
    pub environment: Option<EnvironmentRef>,
    /// Which unit of the environment it ran in.
    pub env_generation: i64,
    /// The arrangement type.
    pub arrangement: ArrangementType,
    /// The executor that carried out its tool calls.
    pub executor: Option<String>,
    /// The tool set it worked with.
    pub tool_set: Option<String>,
    /// The same executor and tool set, named in each language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_ref: Option<crate::programs::ExecutorRef>,
    /// How its harness reached the model.
    pub credential_path: CredentialPath,
    /// Who started it.
    pub started_by: Option<UserRef>,
    /// When it began.
    pub started_at: Time,
    /// When it ended.
    pub ended_at: Option<Time>,
    /// Why it ended.
    pub end_reason: Option<String>,
    /// How well the history it took in was kept.
    pub import_fidelity: Option<Fidelity>,
    /// What was kept, converted and dropped.
    pub import_report: Option<HistoryReport>,
    /// The sequence at which it last changed.
    pub seq_updated: Seq,
}

/// Everything a client needs to show a thread, or what changed since it last did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSync {
    /// Exact history scope of this cut. Modern producers always supply it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    /// Bounded metadata for actual returned or affected prompt input identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_summaries: Vec<ThreadPromptSummary>,
    /// This viewed continuation's own edited prompt/status, independent of the active execution/job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewed_prompt: Option<ThreadPromptVersion>,
    /// Further pending/failed inputs are read lazily, without delivered-input fanout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs_next_after: Option<Id>,
    /// Ordering revision of the viewed queue, including an inactive read-only view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs_queue_revision: Option<i64>,
    /// The thread, always.
    pub thread: Thread,
    /// The thread's highest issued sequence.
    pub seq: Seq,
    /// The sequence the caller asked from, if it did.
    pub since: Option<Seq>,
    /// False when more rows changed than a sync from a position holds; the
    /// client loads again without one.
    pub complete: bool,
    /// On a first load, the union of the latest requested window overall and in the main
    /// stream, in creation order and without duplicates; incremental syncs contain changed items.
    pub items: Vec<Item>,
    /// Current work, the recent-ended allowance, and turns referenced by this item window.
    /// Incremental reads include changed turns and required item/input references; merge by identifier.
    pub turns: Vec<Turn>,
    /// The inputs.
    pub inputs: Vec<ThreadInput>,
    /// The current segment and segments referenced by the returned items/turns, or changed segments.
    /// Older metadata remains discoverable through the explicit history pages and must be merged.
    pub segments: Vec<Segment>,
    /// Whether the main stream has items older than its returned window; on a first load only.
    pub has_older: bool,
}

/// The first visible thread screen, using the same projections as the individual readers.
/// Incremental updates continue through sync; this does not contain hidden panels' data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadOpening {
    /// The complete admitted thread and its initial timeline window.
    pub sync: ThreadSync,
    /// Only this person's draft, or none when empty or they cannot interact.
    pub draft: Option<Draft>,
    /// The native model controls; absent only when unavailable to this reader.
    pub parameters: Option<crate::settings::ThreadParameters>,
    /// The native limits for a new input; absent only when unavailable to this reader.
    pub limits: Option<ThreadLimits>,
    /// The settings, including each setting's own writability and source.
    pub settings: Vec<crate::settings::ResolvedSetting>,
    /// The initial bounded outline page, from the same conversation cut as sync.
    pub outline: ThreadOutlinePage,
}

/// What kinds of line the outline of a thread holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutlineKind {
    /// A numbered turn, linked to its first visible item.
    Turn,
    /// What a person said in the main stream.
    UserMessage,
    /// What the agent said in the main stream.
    AgentMessage,
    /// A milestone the agent announced.
    Milestone,
    /// An announcement that reached the thread.
    Announcement,
    /// An approval.
    Approval,
    /// A line of the history.
    Marker,
    /// An error.
    Error,
    /// A pinned item.
    Pin,
}

/// One line of a thread's outline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutlineEntry {
    /// The item it points at.
    pub item_id: Id,
    /// The sequence at which the item was made.
    pub seq: Seq,
    /// When.
    pub at: Time,
    /// A title; for a message its first [`OUTLINE_TITLE_CHARS`] characters.
    pub title: String,
    /// What kind of line it is.
    pub kind: OutlineKind,
    /// A numbered turn's ordinal; its label is made in the reader's language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_ordinal: Option<i32>,
    /// A goal input's bounded condition; its label is made in the reader's language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_condition: Option<String>,
}

/// An item that a catch-up points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatchupItem {
    /// The item.
    pub item_id: Id,
    /// Its title.
    pub title: String,
}

/// What happened in a thread after a sequence, in numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Catchup {
    /// The history counted, independently of the selected execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    /// The sequence it counts from.
    pub since: Seq,
    /// The thread's sequence now.
    pub seq: Seq,
    /// Items created after `since`.
    pub items: i64,
    /// Turns that ended well.
    pub turns_completed: i64,
    /// Turns that failed.
    pub turns_failed: i64,
    /// Commands that ran.
    pub commands: i64,
    /// Files that changed.
    pub files_changed: i64,
    /// Approvals that wait.
    pub approvals_pending: i64,
    /// Errors.
    pub errors: i64,
    /// Milestones announced.
    pub milestones: Vec<CatchupItem>,
    /// Announcements that reached the thread.
    pub announcements: Vec<CatchupItem>,
    /// Continue only the milestone title list; counts always describe the whole requested cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestones_next_after: Option<Seq>,
    /// Continue only the announcement title list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announcements_next_after: Option<Seq>,
}

/// Which way a raw event travelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RawDirection {
    /// To the harness.
    ToHarness,
    /// From the harness.
    FromHarness,
}

/// One event exactly as it crossed between Orch and a harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RawEvent {
    /// Its number in the segment.
    pub n: i64,
    /// When.
    pub at: Time,
    /// Which way it travelled.
    pub direction: RawDirection,
    /// The event.
    pub payload: Json,
}

/// The format of an exported thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// The conversation in written form, the same text an agent that may read the thread is given.
    Markdown,
    /// A thread, its segments, turns and items.
    Json,
}

/// How much of a thread an export holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportDetail {
    /// The least.
    Brief,
    /// The usual.
    #[default]
    Normal,
    /// Everything.
    Full,
}

/// What an export holds beside the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportInclude {
    /// The agent's reasoning.
    Reasoning,
    /// The items of sub-agents; on unless the query says otherwise.
    SubStreams,
    /// The thread's chat, which needs `chat.view` and `chat.export` as well.
    Chat,
}

/// The exported thread, in the JSON format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadExportJson {
    /// The exact history exported; exporting never selects it for execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    /// The thread.
    pub thread: Thread,
    /// Its segments.
    pub segments: Vec<Segment>,
    /// Its turns.
    pub turns: Vec<Turn>,
    /// Its items.
    pub items: Vec<Item>,
    /// With `chat` included: the messages of its chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_messages: Option<Vec<ChatMessage>>,
}

/// The items to read around a place of the timeline: exactly one of `before`
/// and `after` (a sequence at which an item was made) or `around` (an item).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadItemListQuery {
    /// Read-only history selector; absence follows selected execution at the response snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    /// The scope of a positioned cursor; a changed projection requires a reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// The items made before this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Seq>,
    /// The items made after this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Seq>,
    /// The items around this item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<Id>,
    /// Begin at the first item of this numbered turn, then read forward; exclusive with other windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<i64>,
    /// How many, at most [`ITEMS_MAX_LIMIT`]; [`ITEMS_DEFAULT_LIMIT`] when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Only this stream: `main` or `sub:` followed by an identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
}

/// A stretch of the timeline, ascending by when its items were made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadItemListReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_summaries: Vec<ThreadPromptSummary>,
    /// The items.
    pub items: Vec<Item>,
    /// Exact turns referenced by these items, including older turns absent from initial sync.
    pub turns: Vec<Turn>,
    /// Exact segments referenced by these items and turns; merge rather than replacing older references.
    pub segments: Vec<Segment>,
    /// Whether older ones exist in the requested stream, or any stream when none was specified.
    pub has_older: bool,
    /// Whether newer ones exist in the requested stream, or any stream when none was specified.
    pub has_newer: bool,
}

/// A bounded outline page; a turn header and message at one item stay together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadOutlinePage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    /// Conversation points in ascending creation order.
    pub entries: Vec<OutlineEntry>,
    /// Fetch the next older page with before set to this value; null means the beginning.
    pub next_before: Option<Seq>,
    /// The coherent conversation sequence of this page.
    pub seq: Seq,
}

/// One outline neighborhood or older page, never the entire historical payload table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadOutlineGetQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// Retained-content inspection under the same restoration authority as other thread reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Source points older than this item sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Seq>,
    /// A bounded neighborhood around this item's creation position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<Id>,
    /// A bounded neighborhood around this positive turn number; exclusive with before/around.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<i64>,
    /// Source points, default THREAD_SYNC_DEFAULT_LIMIT and at most THREAD_SYNC_MAX_LIMIT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Explicit historical turn/segment pagination; a missing cursor starts at the most recent window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadHistoryListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// Retained-content inspection under the ordinary restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Permanent ordinals older than this cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i32>,
    /// Rows, default ITEMS_DEFAULT_LIMIT and at most ITEMS_MAX_LIMIT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A complete discoverable page of segment metadata, without unrelated historical item bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSegmentPage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    /// Ascending permanent ordinal order.
    pub segments: Vec<Segment>,
    /// Cursor for the next older page, null at the beginning.
    pub next_before: Option<i32>,
    /// Coherent conversation sequence.
    pub seq: Seq,
}

/// A historical turn page also carries every segment needed to display those turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadTurnPage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    /// Ascending permanent ordinal order, including turns with no visible item.
    pub turns: Vec<Turn>,
    /// Referenced segments only; keep earlier loaded references when merging.
    pub segments: Vec<Segment>,
    /// Cursor for the next older page, null at the beginning.
    pub next_before: Option<i32>,
    /// Coherent conversation sequence.
    pub seq: Seq,
}

/// The initial screen's window, without an incremental cursor or deleted-content inspection mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadOpeningGetQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// Items in the initial window; the same bounds as [`ThreadSyncGetQuery::limit`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// What a client asks to bring a thread up to date.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadSyncGetQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Only what changed after this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Seq>,
    /// Items per initial window or the incremental change budget, at most [`THREAD_SYNC_MAX_LIMIT`];
    /// [`THREAD_SYNC_DEFAULT_LIMIT`] when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// What happened after a sequence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadCatchupGetQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// The sequence to count from.
    pub since: Seq,
    /// Continue milestone titles, without changing the aggregate's since position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestones_after: Option<Seq>,
    /// Continue announcement titles independently of the milestone title page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announcements_after: Option<Seq>,
    /// Titles per list, default ITEMS_DEFAULT_LIMIT and at most ITEMS_MAX_LIMIT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Which part of a command's output to read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadItemOutputGetQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Where to begin, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    /// How many bytes, at most; [`OUTPUT_DEFAULT_LIMIT_BYTES`] when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// The raw events of one segment, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadRawListQuery {
    /// Read-only scope for the requested segment's visible history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// The segment.
    pub segment_id: Id,
    /// Only events numbered below this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    /// How many events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Raw events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadRawListReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    /// The events.
    pub events: Vec<RawEvent>,
    /// Whether older ones exist.
    pub has_older: bool,
}

/// How an image or a file of an input is to be delivered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadBlobGetQuery {
    /// Return a bounded static preview instead of the original file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<bool>,
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Whether to offer it as a download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<bool>,
}

/// A finished upload read privately by its uploader before it is sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadBlobGetQuery {
    /// List the archive without extracting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<bool>,
    /// A directory inside an archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<String>,
    /// Read one bounded archive entry instead of listing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// The filename to display; it confers no authority over the hash.
    pub name: String,
    /// Return a bounded static preview instead of the original file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<bool>,
    /// Offer the original as a download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<bool>,
}

/// What of a thread to export, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadExportQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// The format.
    pub format: ExportFormat,
    /// How much; [`ExportDetail::Normal`] when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ExportDetail>,
    /// The first turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    /// The last turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    /// A comma list of the values of [`ExportInclude`]; `sub_streams` when
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
}

/// Says something to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadInputCreateBody {
    /// What to say; at most [`INPUT_TEXT_MAX_CHARS`] characters of text.
    pub content: Vec<ContentPart>,
    /// How it is to reach the agent.
    pub delivery: Delivery,
    /// A value of the client's own: a repeated pair of author and value returns
    /// the stored input instead of queueing another.
    pub client_id: String,
    /// Reached limits that the sender lets this input's turn past.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass: Option<LimitBypassRequest>,
    /// Optional selection from this exact session's native catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_action: Option<crate::input_schedules::NativeInvocation>,
    /// Work it in plan mode: the agent reads and proposes a plan, and changes nothing until somebody proceeds.
    /// Refused where the thread's agent program has no plan mode; the approval mode stays as it is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan: bool,
}

/// The input that was queued, and the thread's sequence then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadInputCreateReply {
    /// The input.
    pub input: ThreadInput,
    /// The thread's sequence.
    pub seq: Seq,
}

/// Answers a proposed plan once, under the current policy for who may answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadPlanDecideBody {
    /// Proceed, or keep planning.
    pub proceed: bool,
    /// What to change when keeping planning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The one input created by a plan response, and the settled line shown to every reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadPlanDecideReply {
    /// The accepted input.
    pub input: ThreadInput,
    /// Who decided what and when.
    pub decision: crate::approvals::RequestDecisionStatus,
}

/// The limits to look at before sending.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadLimitsGetQuery {
    /// A pending input; an input the caller would send now when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<Id>,
}

/// Changes a pending input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadInputUpdateBody {
    /// Omitted preserves the selected command; null removes it without losing the words.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub native_action: Option<Option<crate::input_schedules::NativeInvocation>>,
    /// New content, which only the author may give.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentPart>>,
    /// Changes how this pending input reaches the agent, by whoever may drive the thread.
    /// Queue puts it last. Steer joins a running turn, or refuses with an interrupt
    /// alternative when the harness does not support steering. With no running turn,
    /// steer and interrupt put it first. Interrupt also stops an active turn.
    /// Existing holds stay in force. Omitted preserves delivery and position.
    /// Delivery sets the queue position; steer/interrupt also accept an explicit first (`null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
    /// Where to put it in the queue: directly after this pending input, or
    /// first when `null`; where it is when left out. A delivery may accompany only
    /// the same first position for steer/interrupt, never a conflicting position.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<Id>")]
    pub after_input_id: Option<Option<Id>>,
}

/// Stops a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadInterruptBody {
    /// The turn.
    pub turn_id: Id,
}

/// An image or a file that was uploaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadedBlob {
    /// Its hash, which the server computed.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
    /// Its type.
    pub media_type: String,
    /// Its name.
    pub name: String,
}

/// Begins an upload in pieces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadSessionCreateBody {
    /// The size of the whole, in bytes.
    pub size: i64,
    /// Its name.
    pub name: String,
    /// Its type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// An upload in pieces that was begun.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadSessionCreateReply {
    /// The session.
    pub upload_id: Id,
    /// The size of a piece, which is `crate::common::UPLOAD_CHUNK_BYTES`.
    pub chunk_bytes: i64,
    /// When the session ends.
    pub expires_at: Time,
}

/// Where a piece of an upload goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadSessionPutQuery {
    /// Must equal the bytes received so far. Another value is refused with the
    /// bytes received, and the client goes on from there.
    pub offset: i64,
}

/// How much of an upload the server holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadSessionPutReply {
    /// The bytes received so far.
    pub received: i64,
}

/// An upload that is attached to a thread from a session of pieces, instead
/// of a raw body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadAttachmentBlobBody {
    /// The upload's hash.
    pub blob: Hash,
    /// Its name.
    pub name: String,
    /// An owned unsent attachment path whose private bytes are being edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
}

/// The private attachment being discarded, when a completed upload already has a reserved name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadBlobDeleteQuery {
    /// Its thread, absent for a cancelled upload that was never attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// Its reserved relative path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A file attached to a thread, ready for a `file` part. It is stored as a blob and written to
/// `path` only when the input is delivered, so attaching never needs the environment to be running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadAttachmentCreateReply {
    /// Where it will be written, in the thread's directory.
    pub path: String,
    /// Its name.
    pub name: String,
    /// Its size in bytes.
    pub size: i64,
    /// Its hash.
    pub blob: Hash,
}

/// Queues a copy of the input of a turn that failed or was stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadTurnRetryBody {
    /// A value of the client's own that makes sending it again safe.
    pub client_id: String,
    /// Limits that are reached and that the caller lets it past.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass: Option<LimitBypassRequest>,
}

/// Saves what was begun in the composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadDraftSetBody {
    /// Omitted preserves another device's selection; null removes it.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub native_action: Option<Option<crate::input_schedules::NativeInvocation>>,
    /// What was written.
    pub content: Vec<ContentPart>,
    /// How it is to be delivered.
    pub delivery: Delivery,
    /// Frozen line comments; omitted preserves the author's existing comments across devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_comments: Option<Vec<crate::source::ReviewComment>>,
}

/// Whether a thread's side threads are listed open or closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SideState {
    /// Those that are open.
    #[default]
    Open,
    /// Those that were closed.
    Closed,
}

/// Starts a side thread: the quick form of a fork, with the same binding and
/// environment, the history up to one turn, and one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadSideCreateBody {
    /// The message, which is queued in it as its first input.
    pub content: Vec<ContentPart>,
    /// A value of the client's own that makes sending it again safe.
    pub client_id: String,
    /// The turn whose end it starts from; the last completed turn when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_turn: Option<i64>,
}

/// A side thread, with the message queued in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSideCreateReply {
    /// The side thread.
    pub thread: Thread,
    /// Its first input.
    pub input: ThreadInput,
    /// The sequence.
    pub seq: Seq,
}

/// The side threads of a thread, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadSideListQuery {
    /// Open ones by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SideState>,
}

crate::endpoints! {
    /// Reads neighboring versions without changing execution, queue, files or native sessions.
    ThreadPromptVersionList: GET "/threads/{id}/inputs/{input_id}/versions", Permission("thread.view"), query ThreadPromptVersionListQuery, none, json ThreadPromptVersionsPage, 200;
    /// Reads only one continuation's pending and failed inputs in a bounded stable queue page.
    ThreadInputList: GET "/threads/{id}/inputs", Permission("thread.view"), query ThreadInputListQuery, none, json ThreadInputPage, 200;
    /// Prepares and selects revised words as their actual author under current interaction/provider use.
    ThreadPromptVersionCreate: POST "/threads/{id}/inputs/{input_id}/versions", Rule("thread.view and thread.interact, the original input's actual author, and current provider/model/effort use"), none, json ThreadPromptVersionCreateBody, json ThreadContinuationAccepted, 202;
    /// Explicitly selects a retained continuation for future execution, preserving its held queue.
    ThreadContinuationActivate: POST "/threads/{id}/continuations/{continuation_id}/activate", Rule("thread.view and thread.interact, and current provider/model/effort use"), none, json ThreadContinuationActivateBody, json ThreadContinuationAccepted, 202;
    /// Opens the signed-in person's thread screen in one read, preserving every component's native access checks.
    ThreadOpeningGet: GET "/threads/{id}/opening", Session, query ThreadOpeningGetQuery, none, json ThreadOpening, 200;
    /// Brings a client up to date on a thread: the thread, its latest items, turns, inputs and segments, or what changed since a sequence.
    ThreadSyncGet: GET "/threads/{id}/sync", Permission("thread.view"), query ThreadSyncGetQuery, none, json ThreadSync, 200;
    /// Reads a stretch of a thread's timeline before, after or around a place.
    ThreadItemList: GET "/threads/{id}/items", Permission("thread.view"), query ThreadItemListQuery, none, json ThreadItemListReply, 200;
    /// Counts what happened in a thread after a sequence.
    ThreadCatchupGet: GET "/threads/{id}/catchup", Permission("thread.view"), query ThreadCatchupGetQuery, none, json Catchup, 200;
    /// Reads one item of a thread.
    ThreadItemGet: GET "/threads/{id}/items/{item_id}", Permission("thread.view"), query continuations::ThreadReadSelector, none, json Item, 200;
    /// Reads a piece of a command's output as plain text, with its total length in a header.
    ThreadItemOutputGet: GET "/threads/{id}/items/{item_id}/output", Permission("thread.view"), query ThreadItemOutputGetQuery, none, raw, 200;
    /// Reads the raw events between Orch and a harness for one segment.
    ThreadRawList: GET "/threads/{id}/raw", Permission("thread.manage"), query ThreadRawListQuery, none, json ThreadRawListReply, 200;
    /// Reads the missing history of an adopted session again, importing its earlier entries once without starting a turn.
    ThreadAdoptRetryHistory: POST "/threads/{id}/adopt/retry-history", Permission("thread.manage"), none, none, json Thread, 200;
    /// Reads the before-Orch band's last read reason, present machine state and current retry act, without native I/O.
    ThreadAdoptHistoryGet: GET "/threads/{id}/adopt/history", Permission("thread.view"), none, none, json Option<AdoptHistoryStatus>, 200;
    /// Lists the outline of a thread, ascending.
    ThreadOutlineGet: GET "/threads/{id}/outline", Permission("thread.view"), query ThreadOutlineGetQuery, none, json ThreadOutlinePage, 200;
    /// Reads a bounded historical segment page; all older segments remain explicitly discoverable.
    ThreadSegmentList: GET "/threads/{id}/segments", Permission("thread.view"), query ThreadHistoryListQuery, none, json ThreadSegmentPage, 200;
    /// Reads a bounded historical turn page with its exact segment references.
    ThreadTurnList: GET "/threads/{id}/turns", Permission("thread.view"), query ThreadHistoryListQuery, none, json ThreadTurnPage, 200;
    /// Reads an image or a file that an input of the thread carries.
    ThreadBlobGet: GET "/threads/{id}/blobs/{hash}", Permission("thread.view"), query ThreadBlobGetQuery, none, raw, 200;
    /// Reads a finished upload only for the caller who holds its upload authority.
    UploadBlobGet: GET "/uploads/{hash}", User, query UploadBlobGetQuery, none, raw, 200;
    /// Exports a thread as a file, in Markdown or JSON, at a depth. It takes the thread's Export row, and reading the thread as well.
    ThreadExport: GET "/threads/{id}/export", Permission("thread.export"), query ThreadExportQuery, none, raw, 200;
    /// Says something to the agent. A repeated `client_id` of the same author answers `200` with the stored input instead of `201`.
    ThreadInputCreate: POST "/threads/{id}/inputs", Rule("thread.interact on the thread, and the sender's own use of the model it runs on"), none, json ThreadInputCreateBody, json ThreadInputCreateReply, 201;
    /// Lists the limits that stand in the way of a turn, and which of them the caller may let past.
    ThreadLimitsGet: GET "/threads/{id}/limits", Permission("thread.view"), query ThreadLimitsGetQuery, none, json ThreadLimits, 200;
    /// Lets a pending input past the limits named.
    ThreadInputBypass: POST "/threads/{id}/inputs/{input_id}/bypass", Rule("thread.interact, and the bypass row for each limit named"), none, json LimitBypassRequest, json ThreadInput, 200;
    /// Runs a message that waits for somebody who may use the thread's model on the caller's own access.
    ThreadInputRun: POST "/threads/{id}/inputs/{input_id}/run", Rule("thread.interact, and the caller's own use of the model, model and effort the thread runs on"), none, none, json ThreadInput, 200;
    /// Answers a proposed plan once, creating one attributed follow-up input.
    ThreadPlanDecide: POST "/threads/{id}/plans/{item_id}/decision", Rule("the current permission for answering the thread's plans and the caller's own model use"), none, json ThreadPlanDecideBody, json ThreadPlanDecideReply, 200;
    /// Changes a pending input's delivery, place or words, preserving existing holds.
    ThreadInputUpdate: PATCH "/threads/{id}/inputs/{input_id}", Rule("thread.interact; its content only by its author"), none, json ThreadInputUpdateBody, json ThreadInputUpdateReply, 200;
    /// Cancels a pending input, whoever's it is.
    ThreadInputCancel: DELETE "/threads/{id}/inputs/{input_id}", Permission("thread.interact"), none, none, json ThreadInput, 200;
    /// Tries again to deliver the files that hold an input; the same input, held again if they still cannot go.
    ThreadInputRetry: POST "/threads/{id}/inputs/{input_id}/retry", Rule("thread.interact, its author, and their own use of the current model"), none, none, json ThreadInput, 200;
    /// Stops a running turn, and holds what waits in the queue until somebody resumes it.
    ThreadInterrupt: POST "/threads/{id}/interrupt", Permission("thread.interact"), none, json ThreadInterruptBody, json Turn, 200;
    /// Resumes what a person's Stop held, naming the revision of the queue that was seen.
    ThreadQueueResume: POST "/threads/{id}/queue/resume", Permission("thread.interact"), none, json ThreadQueueResumeBody, json ThreadQueue, 200;
    /// Uploads an image in one request.
    UploadCreate: POST "/uploads", User, none, bytes, json UploadedBlob, 200;
    /// Begins an upload in pieces.
    UploadSessionCreate: POST "/uploads/sessions", User, none, json UploadSessionCreateBody, json UploadSessionCreateReply, 200;
    /// Sends one piece of an upload, at the offset where the last one ended.
    UploadSessionPut: PUT "/uploads/sessions/{upload_id}", Rule("the person who began the upload"), query UploadSessionPutQuery, bytes, json UploadSessionPutReply, 200;
    /// Ends an upload in pieces; the server computes the hash.
    UploadSessionComplete: POST "/uploads/sessions/{upload_id}/complete", Rule("the person who began the upload"), none, none, json UploadedBlob, 200;
    /// Cancels an unfinished upload owned by the caller and discards its partial bytes.
    UploadSessionDelete: DELETE "/uploads/sessions/{upload_id}", Rule("the person who began the upload"), none, none, none, 204;
    /// Releases the caller's upload reference; saved sources keep their own bytes.
    UploadBlobDelete: DELETE "/uploads/blobs/{hash}", User, query UploadBlobDeleteQuery, none, none, 204;
    /// Attaches a file to a thread, from a raw body with a filename header or from a finished upload.
    ThreadAttachmentCreate: POST "/threads/{id}/attachments", Permission("thread.interact"), none, bytes, json ThreadAttachmentCreateReply, 200;
    /// Queues a copy of the input of a turn that failed or was stopped.
    ThreadTurnRetry: POST "/threads/{id}/turns/{turn_id}/retry", Rule("thread.interact, and the caller's own access as for an input"), none, json ThreadTurnRetryBody, json ThreadInputCreateReply, 201;
    /// Pins an item.
    ThreadItemPin: PUT "/threads/{id}/items/{item_id}/pin", Permission("thread.interact"), none, none, json Item, 200;
    /// Takes the pin off an item.
    ThreadItemUnpin: DELETE "/threads/{id}/items/{item_id}/pin", Permission("thread.interact"), none, none, json Item, 200;
    /// Reads the caller's draft for a thread, if they have one.
    ThreadDraftGet: GET "/threads/{id}/draft", Permission("thread.interact"), none, none, json Option<Draft>, 200;
    /// Saves the caller's draft for a thread.
    ThreadDraftSet: PUT "/threads/{id}/draft", Permission("thread.interact"), none, json ThreadDraftSetBody, json Draft, 200;
    /// Deletes the caller's draft for a thread.
    ThreadDraftDelete: DELETE "/threads/{id}/draft", Permission("thread.interact"), none, none, none, 204;
    /// Starts a side thread from any turn of a thread, with one message queued in it.
    ThreadSideCreate: POST "/threads/{id}/side", Rule("thread.interact, and the caller's own access as for an input"), none, json ThreadSideCreateBody, json ThreadSideCreateReply, 201;
    /// Lists the side threads of a thread, oldest first.
    ThreadSideList: GET "/threads/{id}/side", Permission("thread.view"), query ThreadSideListQuery, none, json Vec<Thread>, 200;
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_thread_presentation_has_one_wire_state_and_explicit_safe_particulars() {
        let written = serde_json::json!({
            "seq": 42, "read_seq": 7, "state": "queued", "queued_inputs": 2,
            "held_inputs": 0, "waiting_inputs": 0, "waiting_for_answer": false,
            "blocked_reason": null, "finished_at": null
        });
        let display: super::ThreadDisplay = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(display.state, super::ThreadDisplayState::Queued);
        assert_eq!(serde_json::to_value(display).unwrap(), written);
        assert!(
            serde_json::from_value::<super::ThreadDisplayState>(serde_json::json!("starting"))
                .is_err()
        );
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn history_pages_have_explicit_cuts_references_and_continuations()
    -> Result<(), serde_json::Error> {
        let outline = json!({"entries":[],"next_before":41,"seq":900});
        let read: ThreadOutlinePage = serde_json::from_value(outline.clone())?;
        assert_eq!(serde_json::to_value(read)?, outline);
        let turns = json!({"turns":[],"segments":[],"next_before":12,"seq":900});
        let read: ThreadTurnPage = serde_json::from_value(turns.clone())?;
        assert_eq!(serde_json::to_value(read)?, turns);
        let segments = json!({"segments":[],"next_before":null,"seq":900});
        let read: ThreadSegmentPage = serde_json::from_value(segments.clone())?;
        assert_eq!(serde_json::to_value(read)?, segments);
        let items = json!({"items":[],"turns":[],"segments":[],"has_older":true,"has_newer":false});
        let read: ThreadItemListReply = serde_json::from_value(items.clone())?;
        assert_eq!(serde_json::to_value(read)?, items);
        assert!(
            serde_json::from_value::<ThreadItemListReply>(
                json!({"items":[],"has_older":true,"has_newer":false})
            )
            .is_err(),
            "old items cannot silently omit the references their renderer needs"
        );
        let query: ThreadOutlineGetQuery = serde_json::from_value(json!({"turn":12,"limit":50}))?;
        assert_eq!(serde_json::to_value(query)?, json!({"turn":12,"limit":50}));
        assert!(
            serde_json::from_value::<ThreadOutlineGetQuery>(json!({"unknown_cursor":12})).is_err()
        );
        Ok(())
    }

    #[test]
    fn a_display_tool_call_can_be_read_and_returned_in_a_threads_history() {
        let literal = json!({"kind":"orch","tool":"use_display",
            "arguments":{"command":"list"},"result":{"displays":[]}});
        let call: ToolPayload = serde_json::from_value(literal.clone()).unwrap();
        assert!(matches!(
            &call,
            ToolPayload::Orch {
                tool: OrchTool::UseDisplay,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(call).unwrap(), literal);
    }

    fn user() -> serde_json::Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "human", "username": "bo",
            "display_name": "Bo", "avatar": { "text": "B", "color": 2 }
        })
    }

    #[test]
    fn a_forwarded_copy_has_one_discriminator_by_itself_and_in_either_conversation() {
        let literal = json!({
            "type": "forwarded",
            "from": {"kind": "thread", "thread": {"id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "thread", "name": "Review", "redacted": false}},
            "source_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02", "author": null,
            "at": "2026-10-14T10:00:00.000Z", "text": "Ready for review", "images": [], "attachments": []
        });
        let copy: Forwarded = serde_json::from_value(literal.clone()).unwrap();
        for written in [
            serde_json::to_string(&copy).unwrap(),
            serde_json::to_string(&ContentPart::Forwarded(Box::new(copy.clone()))).unwrap(),
            serde_json::to_string(&crate::chats::ChatPart::Forwarded(Box::new(copy))).unwrap(),
        ] {
            assert_eq!(written.matches("\"type\"").count(), 1);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&written).unwrap(),
                literal
            );
        }
        assert!(matches!(
            serde_json::from_value::<ContentPart>(literal.clone()).unwrap(),
            ContentPart::Forwarded(_)
        ));
        assert!(matches!(
            serde_json::from_value::<crate::chats::ChatPart>(literal.clone()).unwrap(),
            crate::chats::ChatPart::Forwarded(_)
        ));
        let schema = serde_json::to_value(schemars::schema_for!(Forwarded)).unwrap();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("type"))
        );
        let mut wrong = literal;
        wrong["type"] = json!("text");
        assert!(serde_json::from_value::<Forwarded>(wrong).is_err());
    }

    #[test]
    fn an_item_carries_its_kind_beside_a_payload_that_fits_it() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01",
            "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02",
            "segment_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03",
            "turn_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a04",
            "stream": "main",
            "parent_item_id": null,
            "kind": "tool_call",
            "payload": {
                "tool": {
                    "kind": "command",
                    "command": "cargo test",
                    "cwd": "/work",
                    "exit_code": null,
                    "output": { "inline": "running", "total_len": 7, "truncated": false, "blob": null },
                    "process_id": null,
                    "background": false,
                    "actions": [{ "type": "search", "query": "fn main", "path": null }, { "type": "other" }]
                },
                "title": "Run the tests",
                "started_at": "2026-10-08T17:04:05.123Z",
                "duration_ms": null,
                "raw": {
                    "name": "Bash",
                    "input": { "inline": "{\"command\":\"cargo test\"}", "total_len": 24, "truncated": false, "blob": null },
                    "output": null,
                    "is_error": false
                }
            },
            "status": "in_progress",
            "author": null,
            "before_orch": false,
            "pinned": null,
            "chat_refs": 0,
            "seq_created": 12,
            "seq_updated": 13,
            "created_at": "2026-10-08T17:04:05.123Z",
            "updated_at": "2026-10-08T17:04:06.000Z"
        });
        let item: Item = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(item.kind(), ItemKind::ToolCall);
        assert!(
            matches!(&item.payload, ItemPayload::ToolCall(call) if matches!(call.tool, ToolPayload::Command { .. }))
        );
        assert_eq!(serde_json::to_value(&item).unwrap(), written);
    }

    #[test]
    fn a_turns_error_carries_the_vendors_words_the_time_a_limit_ends_and_whether_the_answer_was_cut()
     {
        let written = json!({
            "code": "provider_exhausted", "message": "the weekly limit of the plan is reached", "retryable": false,
            "window": null,
            "said": "You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)",
            "resets_at": "2026-10-14T09:00:00.000Z", "incomplete": false
        });
        let error: TurnError = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(error.code, "provider_exhausted");
        assert!(error.said.is_some() && error.resets_at.is_some());
        assert_eq!(serde_json::to_value(&error).unwrap(), written);
        // A turn stored before the three members existed reads with none of them.
        let stored: TurnError = serde_json::from_value(
            json!({ "code": "harness_error", "message": "it failed", "retryable": false }),
        )
        .unwrap();
        assert_eq!(
            (stored.said, stored.resets_at, stored.incomplete),
            (None, None, false)
        );
    }

    #[test]
    fn an_item_whose_payload_does_not_fit_its_kind_is_refused() {
        let wrong = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01",
            "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02",
            "segment_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03",
            "turn_id": null, "stream": "main", "parent_item_id": null,
            "kind": "error",
            "payload": { "text": "this is a message", "text_len": 17, "phase": null },
            "status": "failed", "author": null, "pinned": null, "chat_refs": 0,
            "seq_created": 1, "seq_updated": 1,
            "created_at": "2026-10-08T17:04:05.123Z", "updated_at": "2026-10-08T17:04:05.123Z"
        });
        assert!(serde_json::from_value::<Item>(wrong).is_err());
    }

    #[test]
    fn a_marker_is_written_with_its_kind_and_its_facts() {
        let written = json!({
            "marker": "model_changed",
            "data": { "from": "gpt-6-sol", "to": "gpt-6-terra", "effort": { "from": null, "to": "high" }, "by": user() }
        });
        let marker: MarkerPayload = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(marker.kind(), MarkerKind::ModelChanged);
        assert_eq!(serde_json::to_value(&marker).unwrap(), written);
        let shared = json!({
            "marker": "shared",
            "data": { "what": { "chat": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a07", "kind": "group", "org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a08", "node": null, "name": "Release" } },
                      "mode": "live", "path": null, "by": user() }
        });
        let marker: MarkerPayload = serde_json::from_value(shared).unwrap();
        assert!(matches!(
            marker,
            MarkerPayload::Shared(MarkerShare {
                what: SharedThing::Chat(_),
                mode: ShareMode::Live,
                ..
            })
        ));
    }

    #[test]
    fn a_forwarded_message_is_a_part_told_by_its_type() {
        let written = json!({
            "type": "forwarded",
            "from": { "kind": "thread", "thread": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a09", "kind": "thread", "name": null, "redacted": true } },
            "source_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a0a",
            "author": { "agent": "claude-code" },
            "at": "2026-10-08T17:04:05.123Z",
            "text": "The build is green.",
            "images": [],
            "attachments": []
        });
        let part: ContentPart = serde_json::from_value(written.clone()).unwrap();
        match &part {
            ContentPart::Forwarded(copy) => assert_eq!(
                copy.author,
                Some(ForwardedAuthor::Agent(AgentName {
                    agent: "claude-code".into()
                }))
            ),
            other => panic!("not a forwarded part: {other:?}"),
        }
        assert_eq!(serde_json::to_value(&part).unwrap(), written);
    }

    #[test]
    fn an_input_moves_to_the_front_with_null_and_stays_put_when_left_out() {
        let front: ThreadInputUpdateBody =
            serde_json::from_value(json!({ "after_input_id": null })).unwrap();
        let stay: ThreadInputUpdateBody =
            serde_json::from_value(json!({ "content": [{ "type": "text", "text": "better" }] }))
                .unwrap();
        assert_eq!(front.after_input_id, Some(None));
        assert_eq!(stay.after_input_id, None);
        assert_eq!(
            serde_json::to_value(&front).unwrap(),
            json!({ "after_input_id": null })
        );
    }

    #[test]
    fn a_waiting_inputs_delivery_changes_without_replacing_its_words_or_native_selection() {
        for (word, delivery) in [
            ("queue", Delivery::Queue),
            ("steer", Delivery::Steer),
            ("interrupt", Delivery::Interrupt),
        ] {
            let change: ThreadInputUpdateBody =
                serde_json::from_value(json!({ "delivery": word })).unwrap();
            assert_eq!(change.delivery, Some(delivery));
            assert_eq!(change.content, None);
            assert_eq!(change.native_action, None);
            assert_eq!(change.after_input_id, None);
            assert_eq!(
                serde_json::to_value(&change).unwrap(),
                json!({ "delivery": word })
            );
        }
        assert_eq!(ThreadInputUpdateBody::default().delivery, None);
        assert!(
            serde_json::from_value::<ThreadInputUpdateBody>(json!({ "delivery": "now" })).is_err()
        );
    }

    #[test]
    fn recovery_history_keeps_its_recorded_attempt_limit_without_inventing_one_for_old_lines() {
        let old = json!({"action":"continue","rule":"refused","kind":"model_refused","attempt":1,"automatic":true,"input_id":orch_types::new_id()});
        let legacy: MarkerCallRecovery = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(legacy.total, None);
        assert_eq!(serde_json::to_value(legacy).unwrap(), old);
        let mut recorded = old;
        recorded["total"] = json!(3);
        let current: MarkerCallRecovery = serde_json::from_value(recorded.clone()).unwrap();
        assert_eq!(current.total, Some(3));
        assert_eq!(serde_json::to_value(current).unwrap(), recorded);
    }

    #[test]
    fn pasted_text_keeps_its_tag_in_both_part_unions_and_its_shared_schema() {
        let wire = json!({"type":"pasted_text","blob":Hash::of(b"sent words"),"name":"pasted.txt","path":".orch/attachments/pasted.txt","size":10,"characters":10,"ascii_characters":10});
        let part: ContentPart = serde_json::from_value(wire.clone()).unwrap();
        assert!(matches!(part, ContentPart::PastedText(_)));
        assert_eq!(serde_json::to_value(&part).unwrap(), wire);
        let chat: crate::chats::ChatPart = serde_json::from_value(wire.clone()).unwrap();
        assert!(matches!(chat, crate::chats::ChatPart::PastedText(_)));
        assert_eq!(serde_json::to_value(&chat).unwrap(), wire);
        let schema = serde_json::to_value(schemars::schema_for!(PastedText)).unwrap();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("type"))
        );
    }

    #[test]
    fn image_destinations_round_trip_without_changing_legacy_records() {
        let old = json!({"type":"image","blob":Hash::of(b"image"),"media_type":"image/png","name":"shot.png"});
        let legacy: ContentPart = serde_json::from_value(old.clone()).unwrap();
        assert!(matches!(&legacy, ContentPart::Image { path: None, .. }));
        assert_eq!(serde_json::to_value(&legacy).unwrap(), old);
        let mut assigned = old;
        assigned["path"] = json!(".orch/attachments/shot.png");
        let named: ContentPart = serde_json::from_value(assigned.clone()).unwrap();
        assert!(
            matches!(&named, ContentPart::Image { path: Some(path), .. } if path == ".orch/attachments/shot.png")
        );
        assert_eq!(serde_json::to_value(&named).unwrap(), assigned);
    }

    #[test]
    fn a_limit_the_provider_reports_has_no_cap_beside_it() {
        let limits: ThreadLimits = serde_json::from_value(json!({
            "reached": [{
                "cap": null,
                "provider_limit": {
                    "provider": {
                        "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a0b", "name": "Team Pro", "kind": "subscription", "harness_kind": "codex",
                        "address": "acme.team-pro", "placement": "managed", "status": "exhausted"
                    },
                    "window": "5h", "resets_at": "2026-10-08T20:00:00.000Z", "as_of": "2026-10-08T17:04:05.123Z"
                },
                "can_bypass": false, "passed_by": null, "ask": [], "ask_more": 0
            }],
            "bypass_all": false
        }))
        .unwrap();
        assert!(limits.reached[0].cap.is_none() && limits.reached[0].provider_limit.is_some());
    }
}

#[cfg(test)]
mod display_recovery_contract {
    use super::*;
    use serde_json::json;

    #[test]
    fn recovery_counts_round_trip_without_narrowing_and_quiet_displays_keep_their_wire_shape() {
        let wire = json!({
            "rule_id":"busy", "step":"retry", "attempt":4_294_967_295_u64,
            "attempts":4_294_967_295_u64, "due_at":"2026-10-28T00:00:55.000Z",
            "continuing":false, "failure_kind":"provider_busy", "stop_reason":null,
            "count":null, "window_secs":null
        });
        let details: ThreadDisplayRecovery = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&details).unwrap(), wire);
        let display = ThreadDisplay {
            state: ThreadDisplayState::Queued,
            recovery: Some(Box::new(details)),
            ..Default::default()
        };
        let read: ThreadDisplay =
            serde_json::from_value(serde_json::to_value(&display).unwrap()).unwrap();
        assert_eq!(read, display);
        assert!(
            serde_json::to_value(ThreadDisplay::default())
                .unwrap()
                .get("recovery")
                .is_none()
        );
    }
}
