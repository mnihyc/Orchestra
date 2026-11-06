//! Harness, capability and access requests and decisions.
//!
//! Three kinds of thing wait for somebody to decide: what a harness asks
//! before it acts, a capability that an environment asks for, and a request
//! for access to something. An [`Approval`] is the one shape for all of them;
//! what it asks is its `request`, written beside its `kind`. The body of a
//! decision depends on that kind ([`ApprovalDecision`]), and so does who may
//! decide it.

use crate::operations::{
    FacetSpec as Facet, OperationSpec as Op, TargetResolver as Target, VariantSpec as Variant,
};
use orch_access_contract::key_scope::KeyAction::{Manage, Read, Write};
use orch_access_contract::key_scope::KeyIdentityKind as I;
use orch_access_contract::key_scope::KeyTargetKind::{Identity, Object};
use orch_access_contract::permissions::ObjectKind as K;

use std::collections::BTreeMap;

use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A person's decision on a plan or a held message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestDecisionAction {
    /// Continue the proposed plan in the normal mode.
    Proceed,
    /// Continue planning with the person's words.
    KeepPlanning,
    /// Run a held message on the deciding person's access.
    Run,
}

/// The shared settled line for a decision kept on its source card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestDecisionStatus {
    /// What was decided.
    pub action: Option<RequestDecisionAction>,
    /// The canonical outcome of the request.
    pub state: ApprovalState,
    /// Who decided, where that person can be shown.
    pub decided_by: Option<crate::users::UserRef>,
    /// When the decision was accepted.
    pub decided_at: Time,
}

use crate::access::{AccessOptions, Holder, ObjectRef};
use crate::binding::HostRef;
use crate::common::{Page, PageQuery};
use crate::nodes::{EnvironmentRef, NodeRef};
use crate::routes::Access::{Permission, Rule, User};
use crate::threads::{FileChange, ThreadMention};
use crate::users::UserRef;

/// Where an approval stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    /// Waiting for a decision.
    Pending,
    /// Given.
    Approved,
    /// Content supplied in answer to the harness's question or form.
    Answered,
    /// Refused.
    Denied,
    /// Taken back by whoever asked.
    Cancelled,
    /// Ran out.
    Expired,
}

/// What kind of thing an approval is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    /// What a harness asks before it acts.
    Harness,
    /// A capability an environment asks for.
    Capability,
    /// A request for access to something.
    Access,
    /// A thread's request to let more people reach a port of its environment.
    Port,
    /// A plan proposed by a completed planning turn.
    Plan,
    /// A message held until somebody runs it on their own model access.
    Input,
}

/// Who asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequester {
    /// A person.
    User(UserRef),
    /// The agent of a thread.
    Thread(ThreadMention),
}

/// How an option of a harness approval is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptionStyle {
    /// To allow.
    Allow,
    /// To deny.
    Deny,
    /// Neither.
    Neutral,
}

/// How long a choice holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptionScope {
    /// For this once.
    Once,
    /// For the session.
    Session,
    /// Always.
    Always,
    /// For this thread: a narrow rule Orch keeps for it, held to the matcher shown, never the harness's own settings.
    Thread,
}

/// What a rule kept for a thread answers: the requests a harness can hold to exactly this.
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

/// One answer the harness offers to its own question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalOption {
    /// What a decision names.
    pub id: String,
    /// What it is called.
    pub label: String,
    /// How it is drawn.
    pub style: OptionStyle,
    /// How long it holds, where that is a choice.
    pub scope: Option<OptionScope>,
    /// For scope `thread`: the matcher the adapter verifies on later requests, shown before it is chosen; `null`
    /// otherwise. An option of scope `thread` without one offers nothing for the thread.
    pub rule: Option<ApprovalMatcher>,
}

/// An answer allowed for one thread: later requests that offer the same matcher, while the thread keeps its
/// binding, are answered once each by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadApprovalRule {
    /// The rule's identifier.
    pub id: Id,
    /// What it answers.
    pub matcher: ApprovalMatcher,
    /// Who allowed it.
    pub by: UserRef,
    /// When.
    pub at: Time,
    /// Whether it still answers.
    pub active: bool,
    /// Why not, when not: taken back, or the thread's binding changed.
    pub reason: Option<String>,
    /// How many times it changed.
    pub revision: i64,
}

/// A choice offered by a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOption {
    /// What it is called.
    pub label: String,
    /// What it means.
    pub description: String,
}

/// A question the harness asks the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    /// What an answer names.
    pub id: String,
    /// A short heading.
    pub header: String,
    /// The question.
    pub question: String,
    /// Whether several choices may be given.
    pub multiple: bool,
    /// Whether an answer of their own is allowed.
    pub allow_other: bool,
    /// The choices; `null` for a free answer.
    pub options: Option<Vec<QuestionOption>>,
}

/// What a harness asks, by its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessRequestKind {
    /// To run a command.
    Command,
    /// To change files.
    FileChange,
    /// For permissions.
    Permissions,
    /// For input from the person.
    UserInput,
    /// An MCP server asks for something.
    McpElicitation,
    /// A plan the agent proposes, to proceed with or to keep planning.
    Plan,
    /// Anything else.
    Other,
}

/// A command the harness asks to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandRequestDetail {
    /// The command line.
    pub command: String,
    /// Where it would run.
    pub cwd: String,
    /// Why, in the harness's words.
    pub reason: Option<String>,
}

/// File changes the harness asks to make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChangeRequestDetail {
    /// The changes.
    pub changes: Vec<FileChange>,
    /// Why, in the harness's words.
    pub reason: Option<String>,
}

/// Permissions the harness asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionsRequestDetail {
    /// What it asks for, as the harness words it.
    pub permissions: Json,
    /// Why, in the harness's words.
    pub reason: Option<String>,
}

/// Questions the harness asks the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserInputRequestDetail {
    /// The questions.
    pub questions: Vec<Question>,
}

/// Something an MCP server asks of the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpElicitationRequestDetail {
    /// The server.
    pub server: String,
    /// What it says.
    pub message: String,
    /// A JSON Schema of what it wants back.
    pub schema: Json,
}

/// A plan the agent proposes at the end of planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanRequestDetail {
    /// The plan, in the agent's words (Markdown).
    pub plan: String,
}

/// A request the harness makes that has no kind of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OtherRequestDetail {
    /// What it is, in a sentence.
    pub summary: String,
    /// What the harness sent.
    pub raw: Json,
}

/// What a harness asks, by its kind. It is written beside the request's other
/// members as `request_kind` and `detail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "request_kind", content = "detail", rename_all = "snake_case")]
pub enum HarnessRequestDetail {
    /// To run a command.
    Command(CommandRequestDetail),
    /// To change files.
    FileChange(FileChangeRequestDetail),
    /// For permissions.
    Permissions(PermissionsRequestDetail),
    /// For input from the person.
    UserInput(UserInputRequestDetail),
    /// An MCP server asks for something.
    McpElicitation(McpElicitationRequestDetail),
    /// A plan to proceed with.
    Plan(PlanRequestDetail),
    /// Anything else.
    Other(OtherRequestDetail),
}

impl HarnessRequestDetail {
    /// The kind of request this is.
    #[must_use]
    pub const fn kind(&self) -> HarnessRequestKind {
        match self {
            Self::Command(_) => HarnessRequestKind::Command,
            Self::FileChange(_) => HarnessRequestKind::FileChange,
            Self::Permissions(_) => HarnessRequestKind::Permissions,
            Self::UserInput(_) => HarnessRequestKind::UserInput,
            Self::McpElicitation(_) => HarnessRequestKind::McpElicitation,
            Self::Plan(_) => HarnessRequestKind::Plan,
            Self::Other(_) => HarnessRequestKind::Other,
        }
    }
}

/// What a harness asks before it acts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessRequest {
    /// The kind of request and its detail.
    #[serde(flatten)]
    pub detail: HarnessRequestDetail,
    /// The answers it offers; empty for input and for an MCP server's question.
    pub options: Vec<ApprovalOption>,
}

/// How much harm granting a capability could do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Little.
    Low,
    /// Some.
    Medium,
    /// A good deal.
    High,
    /// The most.
    Critical,
}

/// A capability an environment asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityRequest {
    /// The capability's key.
    pub capability: String,
    /// What it is called.
    pub label: String,
    /// How risky it is.
    pub risk: Risk,
    /// Why it is asked for.
    pub reason: String,
    /// The environment that asks.
    pub environment: EnvironmentRef,
    /// The host it runs on.
    pub host: HostRef,
    /// What granting it would do, in words.
    pub consequences: Vec<String>,
}

/// A request for access to something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessRequest {
    /// This request asks to join a group chat instead of naming permission-page rows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub membership: bool,
    /// The shared label kept with a request from a reference card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The permissions of the object's page. A choice of everyday access is
    /// sent as the permissions it names.
    pub permissions: Vec<String>,
    /// For the use of a provider: the models and efforts.
    pub options: Option<AccessOptions>,
    /// Who the entries would name: a selected person (normally the requester), or all the work in a place.
    pub holder: Holder,
    /// Why.
    pub reason: String,
    /// The proposed end of the access, or no end when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// The thread that the person was attempting to act on.
    pub for_thread: Option<NodeRef>,
}

/// A plan waiting for one attributed response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposedPlanRequest {
    /// Its source card.
    pub item_id: Id,
    /// The plan as it was proposed.
    pub plan: crate::threads::PlanPayload,
}

/// A message waiting for somebody who may use the thread's model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HeldInputRequest {
    /// Its source queue entry.
    pub input_id: Id,
    /// The accepted words the decision concerns.
    pub revision: i64,
    /// The message to review before running it.
    pub content: Vec<crate::threads::ContentPart>,
}

/// What an approval asks, by its kind. It is written beside the approval's
/// other members as `kind` and `request`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "request", rename_all = "snake_case")]
pub enum ApprovalRequest {
    /// What a harness asks before it acts.
    Harness(HarnessRequest),
    /// A capability.
    Capability(CapabilityRequest),
    /// Access.
    Access(AccessRequest),
    /// A port.
    Port(Box<crate::ports::PortRequest>),
    /// A proposed plan.
    Plan(ProposedPlanRequest),
    /// A model-use wait.
    Input(HeldInputRequest),
}

impl ApprovalRequest {
    /// The kind of approval this is.
    #[must_use]
    pub const fn kind(&self) -> ApprovalKind {
        match self {
            Self::Harness(_) => ApprovalKind::Harness,
            Self::Capability(_) => ApprovalKind::Capability,
            Self::Access(_) => ApprovalKind::Access,
            Self::Port(_) => ApprovalKind::Port,
            Self::Plan(_) => ApprovalKind::Plan,
            Self::Input(_) => ApprovalKind::Input,
        }
    }
}

/// Whose approvers decide a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApproverScope {
    /// Ports Manage on the environment whose boundary would open.
    Environment,
    /// Those of the host.
    Host,
    /// Those of the org.
    Org,
    /// Those of the system.
    System,
}

/// Something that waits for a decision, or was decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Approval {
    /// The approval's identifier.
    pub id: Id,
    /// What it asks, written as `kind` and `request`.
    #[serde(flatten)]
    pub request: ApprovalRequest,
    /// Where it stands.
    pub state: ApprovalState,
    /// A line to show for it.
    pub title: String,
    /// The node it is about.
    pub node: Option<NodeRef>,
    /// The object it is about.
    pub object: Option<ObjectRef>,
    /// The thread it came from.
    pub thread_id: Option<Id>,
    /// The item that shows it in the thread.
    pub item_id: Option<Id>,
    /// Who asked.
    pub requester: ApprovalRequester,
    /// The permission that decides it; empty for a capability, whose approvers
    /// its scope names.
    pub required_permission: String,
    /// For a capability: whose approvers decide it.
    pub approver_scope: Option<ApproverScope>,
    /// For access: the requests one person sent together.
    pub batch_id: Option<Id>,
    /// Whether the caller may decide it.
    pub can_decide: bool,
    /// The place's quick grant durations for an access request, in their configured order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_grant_durations: Option<Vec<String>>,
    /// Who decided it.
    pub decided_by: Option<UserRef>,
    /// Whether policy decided it by itself.
    pub decided_automatically: bool,
    /// When it was decided.
    pub decided_at: Option<Time>,
    /// What was decided, as the decision was sent.
    pub decision: Option<Json>,
    /// When it runs out.
    pub expires_at: Option<Time>,
    /// When it was made.
    pub created_at: Time,
}

/// The answer to a person's decision: the approval as it stands now and, for a port request, the
/// opening's one result as that decision made it, with the link once when the deciding person kept
/// it to themself. Only this answer carries it: a list, a read or a sync of the approval never does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalDecisionReply {
    /// The approval.
    #[serde(flatten)]
    pub approval: Approval,
    /// For a port request: the opening, its decision and the link's one intended delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_result: Option<crate::ports::PortOpenReply>,
}

impl From<Approval> for ApprovalDecisionReply {
    fn from(approval: Approval) -> Self {
        Self {
            approval,
            opening_result: None,
        }
    }
}

/// The accepted decision without returning its existing request or reference payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalWriteReceipt {
    /// This is an acknowledgement, not an approval projection.
    pub projection: crate::common::ReceiptProjection,
    /// The exact approval whose outcome was accepted.
    pub approval_id: Id,
    /// The actual state left by the ordinary writer.
    pub state: ApprovalState,
    /// The decision's retained actor, without a private account projection.
    pub decided_by: Option<Id>,
    /// When the decision was accepted.
    pub decided_at: Option<Time>,
    /// For a port decision, its exact reviewed opening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_id: Option<Id>,
    /// Only the generated proof already intended for this deciding person, once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<crate::ports::PortLinkDelivery>,
}

/// The native approval result or the same committed outcome's restricted receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ApprovalWriteResponse {
    /// Existing content is readable under this same operation.
    Full(ApprovalDecisionReply),
    /// The action succeeded without permission to disclose its existing card.
    Receipt(ApprovalWriteReceipt),
}
impl ApprovalWriteResponse {
    /// Native consumers must handle acknowledgement separately; it never invents a card.
    pub fn into_full(self) -> Result<ApprovalDecisionReply, ApprovalWriteReceipt> {
        match self {
            Self::Full(full) => Ok(full),
            Self::Receipt(receipt) => Err(receipt),
        }
    }
}

impl std::ops::Deref for ApprovalDecisionReply {
    type Target = Approval;

    fn deref(&self) -> &Approval {
        &self.approval
    }
}

/// Whether the approvals wanted are those waiting or those decided.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalListState {
    /// Waiting for a decision.
    #[default]
    Pending,
    /// Decided.
    Decided,
}

/// Which approvals are wanted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalListQuery {
    /// Waiting, which is the default, or decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ApprovalListState>,
    /// `mine` (the default): approvals sent to the caller that they can decide now, as one of the
    /// nearest approvers. `requested`: approvals the caller asked for. `node:` followed by a node
    /// identifier: approvals attached to that node or anything below it, which needs `node.view`
    /// there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Only this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ApprovalKind>,
    /// With `mine`: everything the caller could decide, which for an admin of a place higher up
    /// also takes in what went to the admins nearer the thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The answer to a harness approval that offered options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarnessOptionDecision {
    /// The option chosen.
    pub option_id: String,
    /// A note for the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The answers to the questions of a harness approval for input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarnessAnswersDecision {
    /// The answers: the choices or the words given, by question.
    pub answers: BTreeMap<String, Vec<String>>,
    /// A note for the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What the person does with an MCP server's question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    /// Answer it.
    Accept,
    /// Refuse to.
    Decline,
    /// Drop it.
    Cancel,
}

/// The answer to a question from an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarnessElicitationDecision {
    /// What they do.
    pub action: ElicitationAction,
    /// What they answer, when they accept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Json>,
}

/// The decision on a capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDecision {
    /// Whether to grant it.
    pub approve: bool,
    /// A note for whoever asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The decision on a request for access. The approver may grant fewer permissions, narrow the
/// options and set an end date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessDecision {
    /// Whether to give it.
    pub approve: bool,
    /// The permissions to give, if fewer than were asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<String>>,
    /// Narrower options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Json>,
    /// When what is given ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// One currently configured quick duration; its expiry is computed at the server's decision time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_duration: Option<String>,
    /// A note for whoever asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Runs the reviewed held message on the deciding person's own model access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeldInputDecision {
    /// True to run; cancelling remains the queue's existing action.
    pub run: bool,
}

/// A decision. Its shape is told by the kind of the approval it decides, and
/// what the members say; a body that does not fit that approval is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ApprovalDecision {
    /// A harness approval with options.
    Option(HarnessOptionDecision),
    /// A harness approval for input.
    Answers(HarnessAnswersDecision),
    /// A harness approval that is an MCP server's question.
    Elicitation(HarnessElicitationDecision),
    /// A capability.
    Capability(CapabilityDecision),
    /// A request for access.
    Access(AccessDecision),
    /// A port request.
    Port(crate::ports::PortDecision),
    /// A held message.
    Input(HeldInputDecision),
}

impl ApprovalDecision {
    /// The existing short approve/note shape is shared by capability and access requests.
    /// Normalize it once; the stored approval kind still selects the ordinary writer.
    #[must_use]
    pub fn as_access_decision(&self) -> Option<AccessDecision> {
        match self {
            Self::Capability(decision) => Some(AccessDecision {
                approve: decision.approve,
                permissions: None,
                options: None,
                expires_at: None,
                quick_duration: None,
                note: decision.note.clone(),
            }),
            Self::Access(decision) => Some(decision.clone()),
            _ => None,
        }
    }
}

// These are the typed references the existing common card can disclose. Literal request,
// answer and configuration text and public UserRefs are not independently reinterpreted.
const APPROVAL_SOURCE_FACETS: &[Facet] = &[
    Facet::response_content(
        &[Object(K::Org)],
        Target::ResolvedCollection("approval_nodes_orgs"),
    ),
    Facet::response_content(
        &[Object(K::Project)],
        Target::ResolvedCollection("approval_nodes_projects"),
    ),
    Facet::response_content(
        &[Object(K::Topic)],
        Target::ResolvedCollection("approval_nodes_topics"),
    ),
    Facet::response_content(
        &[Object(K::Thread)],
        Target::ResolvedCollection("approval_nodes_threads"),
    ),
    Facet::response_content(
        &[Object(K::Document)],
        Target::ResolvedCollection("approval_documents"),
    ),
    Facet::response_content(
        &[Object(K::DocumentContainer)],
        Target::ResolvedCollection("approval_document_containers"),
    ),
    Facet::response_content(
        &[Object(K::Provider)],
        Target::ResolvedCollection("approval_providers"),
    ),
    Facet::response_content(
        &[Object(K::Agent)],
        Target::ResolvedCollection("approval_agents"),
    ),
    Facet::response_content(
        &[Object(K::Environment)],
        Target::ResolvedCollection("approval_environments"),
    ),
    Facet::response_content(
        &[Object(K::Host)],
        Target::ResolvedCollection("approval_hosts"),
    ),
    Facet::response_content(
        &[Object(K::Machine)],
        Target::ResolvedCollection("approval_machines"),
    ),
    Facet::response_content(
        &[Object(K::Template)],
        Target::ResolvedCollection("approval_templates"),
    ),
    Facet::response_content(
        &[Object(K::McpServer)],
        Target::ResolvedCollection("approval_mcp_servers"),
    ),
    Facet::response_content(
        &[Object(K::Mount)],
        Target::ResolvedCollection("approval_mounts"),
    ),
    Facet::response_content(
        &[Object(K::Skill)],
        Target::ResolvedCollection("approval_skills"),
    ),
    Facet::response_content(
        &[Identity(I::Chat)],
        Target::ResolvedCollection("approval_chats"),
    ),
];
const fn approval_read_facets(target: Target) -> [Facet; APPROVAL_SOURCE_FACETS.len() + 1] {
    let primary = Facet::required(Read, &[Identity(I::Approval)], target);
    let mut facets = [primary; APPROVAL_SOURCE_FACETS.len() + 1];
    let mut index = 0;
    while index < APPROVAL_SOURCE_FACETS.len() {
        facets[index + 1] = APPROVAL_SOURCE_FACETS[index];
        index += 1;
    }
    facets
}
const fn approval_write_facets(primary: Facet) -> [Facet; APPROVAL_SOURCE_FACETS.len() + 2] {
    let mut facets = [primary; APPROVAL_SOURCE_FACETS.len() + 2];
    facets[1] =
        Facet::response_content(&[Identity(I::Approval)], Target::Resolved("approval_scope"));
    let mut index = 0;
    while index < APPROVAL_SOURCE_FACETS.len() {
        facets[index + 2] = APPROVAL_SOURCE_FACETS[index];
        index += 1;
    }
    facets
}
const fn approval_with_extra(
    primary: Facet,
    extra: Facet,
) -> [Facet; APPROVAL_SOURCE_FACETS.len() + 3] {
    let base = approval_write_facets(primary);
    let mut facets = [base[0]; APPROVAL_SOURCE_FACETS.len() + 3];
    let mut index = 0;
    while index < base.len() {
        facets[index] = base[index];
        index += 1;
    }
    facets[base.len()] = extra;
    facets
}
const fn approval_port_facets() -> [Facet; APPROVAL_SOURCE_FACETS.len() + 3] {
    // Once/Deny have no additional grant. Grant binds exactly its native approver scope.
    approval_with_extra(
        Facet::required(
            Manage,
            &[Object(K::Environment)],
            Target::Resolved("approval_subject"),
        ),
        Facet::required(
            Manage,
            &[
                Object(K::Environment),
                Object(K::Host),
                Object(K::Org),
                Identity(I::System),
            ],
            Target::ResolvedCollection("approval_capability_grants"),
        ),
    )
}
const fn approval_input_facets() -> [Facet; APPROVAL_SOURCE_FACETS.len() + 3] {
    // Same conditional native Manage footprint as the existing ThreadInputRun producer.
    approval_with_extra(
        Facet::required(
            Write,
            &[Object(K::Thread)],
            Target::Resolved("approval_subject"),
        ),
        Facet::required(
            Manage,
            &[Object(K::Thread)],
            Target::ResolvedCollection("input_native_manage"),
        ),
    )
}

crate::endpoints! {
    /// Lists approvals: those waiting or decided, sent to the caller, asked by them, or attached to a node.
    ApprovalList: GET "/approvals", User, query ApprovalListQuery, none, json Page<Approval>, 200,
        operation Op::facets(&approval_read_facets(Target::ResolvedCollection("approval_scopes")));
    /// Reads one approval.
    ApprovalGet: GET "/approvals/{id}", Rule("an eligible approver, the requester, or thread.view on its thread"), none, none, json Approval, 200,
        operation Op::facets(&approval_read_facets(Target::Resolved("approval_scope")));
    /// Decides an approval, by the permission its current native kind requires.
    ApprovalDecide: POST "/approvals/{id}/decision", Rule("the permission the approval requires: thread.manage for a harness request except the shared plan-answer rule; access.manage on the requested object; ports.manage on the port environment; or the capability's current eligible scoped approver"), none, json ApprovalDecision, json ApprovalWriteResponse, 200,
        operation Op::receipt(&Op::variants(&[
            Variant {
                name: "harness",
                selector: "stored kind=harness except request_kind=plan",
                facets: &approval_write_facets(Facet::required(Manage, &[Object(K::Thread)], Target::Resolved("approval_subject"))),
            },
            Variant {
                name: "access",
                selector: "stored kind=access; actual requested object or Chat membership",
                facets: &approval_write_facets(Facet::required(Manage, &[
                    Object(K::Org), Object(K::Project), Object(K::Topic), Object(K::Thread),
                    Object(K::Document), Object(K::DocumentContainer), Object(K::Provider), Object(K::Agent),
                    Object(K::Environment), Object(K::Host), Object(K::Machine), Object(K::Template),
                    Object(K::McpServer), Object(K::Mount), Object(K::Skill), Identity(I::Chat),
                ], Target::Resolved("approval_subject"))),
            },
            Variant {
                name: "capability",
                selector: "stored kind=capability; resolve its current effective approver_scope",
                facets: &approval_write_facets(Facet::required(Manage,
                    &[Object(K::Environment), Object(K::Host), Object(K::Org), Identity(I::System)],
                    Target::Resolved("approval_subject"))),
            },
            Variant {
                name: "port",
                selector: "stored kind=port; a Grant additionally resolves its actual capability approver scope",
                facets: &approval_port_facets(),
            },
            Variant {
                name: "plan_interact",
                selector: "stored kind=plan, or harness request_kind=plan, and shared deciding_permission is thread.interact",
                facets: &approval_write_facets(Facet::required(Write, &[Object(K::Thread)], Target::Resolved("approval_subject"))),
            },
            Variant {
                name: "plan_manage",
                selector: "stored kind=plan, or harness request_kind=plan, and shared deciding_permission is thread.manage",
                facets: &approval_write_facets(Facet::required(Manage, &[Object(K::Thread)], Target::Resolved("approval_subject"))),
            },
            Variant {
                name: "input",
                selector: "stored kind=input",
                facets: &approval_input_facets(),
            },
        ]));
    /// Takes back one's own request for access.
    ApprovalCancel: POST "/approvals/{id}/cancel", Rule("the person who asked"), none, none, json ApprovalWriteResponse, 200,
        operation Op::receipt(&Op::facets(&approval_write_facets(Facet::required(Write, &[Identity(I::Approval)], Target::Resolved("approval_subject")))));
    /// Lists the answers allowed for a thread, newest first, those no longer active with why.
    ThreadApprovalRuleList: GET "/threads/{id}/approval-rules", Permission("thread.manage"), query PageQuery, none, json Page<ThreadApprovalRule>, 200;
    /// Takes back an answer allowed for a thread: the next undecided request asks again; an action already allowed goes on.
    ThreadApprovalRuleDelete: DELETE "/threads/{id}/approval-rules/{rule_id}", Permission("thread.manage"), none, none, none, 204;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_approval_carries_its_kind_beside_a_request_that_fits_it() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b",
            "kind": "harness",
            "request": {
                "request_kind": "command",
                "detail": { "command": "rm -rf build", "cwd": "/work", "reason": null },
                "options": [{ "id": "allow_once", "label": "Allow once", "style": "allow", "scope": "once", "rule": null }]
            },
            "state": "pending",
            "title": "Run rm -rf build",
            "node": null, "object": null,
            "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c",
            "item_id": null,
            "requester": { "thread": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c", "name": "Fix login" } },
            "required_permission": "thread.manage",
            "approver_scope": null, "batch_id": null, "can_decide": true,
            "decided_by": null, "decided_automatically": false, "decided_at": null, "decision": null,
            "expires_at": null, "created_at": "2026-10-08T17:04:05.123Z"
        });
        let approval: Approval = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(approval.request.kind(), ApprovalKind::Harness);
        assert!(matches!(
            &approval.request,
            ApprovalRequest::Harness(HarnessRequest { detail: HarnessRequestDetail::Command(_), options }) if options.len() == 1
        ));
        assert_eq!(serde_json::to_value(&approval).unwrap(), written);
    }

    #[test]
    fn a_decision_is_told_by_the_members_it_has() {
        let option: ApprovalDecision =
            serde_json::from_value(json!({ "option_id": "allow_once", "note": "fine" })).unwrap();
        assert!(matches!(option, ApprovalDecision::Option(_)));
        let answers: ApprovalDecision =
            serde_json::from_value(json!({ "answers": { "q1": ["yes", "later"] } })).unwrap();
        assert!(matches!(answers, ApprovalDecision::Answers(_)));
        let access: ApprovalDecision = serde_json::from_value(
            json!({ "approve": true, "permissions": ["thread.view"], "expires_at": null }),
        )
        .unwrap();
        assert!(matches!(access, ApprovalDecision::Access(_)));
        let capability: ApprovalDecision =
            serde_json::from_value(json!({ "approve": false })).unwrap();
        assert!(matches!(capability, ApprovalDecision::Capability(_)));
        assert!(
            serde_json::from_value::<ApprovalDecision>(
                json!({ "approve": true, "option_id": "x" })
            )
            .is_err()
        );
    }
}
