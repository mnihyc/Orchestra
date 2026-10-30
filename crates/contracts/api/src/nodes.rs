//! Hierarchy with orgs inside orgs as deep as the system allows, a person's own org and the two limits of a root org, tags, archive, node views, a place's Activity, and a project or a department moved inside its tree with what that changes.
//!
//! A node is an org, a project, a topic or a thread. Every view of one shares
//! the members of [`NodeBase`] and adds its own, and tells itself apart by
//! `kind`; [`Node`] is any of the four. An org may lie in another org, in which
//! case people call it a department, and it is still an org: the same
//! requests serve both. A project may lie in another project in the same way
//! and is then called a folder; a topic lies in a project or in any folder.
//! A place's history is its [`ActivityEntry`] lines, and moving a project, a
//! topic or a department is planned first as a [`NodeMovePlan`].

use std::collections::BTreeMap;

use orch_types::{Id, Json, Revision, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{HistoryActor, PagePreview, Role, ThreadAccess};
use crate::binding::{ArrangementType, EnvironmentChoice};
use crate::common::{HiddenItem, Listed, Page, SubscriptionLevel};
use crate::forks::PlanBlocker;
use crate::routes::Access::{Permission, Rule, User};
use crate::threads::{BlockedReason, ContentPart, Thread, ThreadState};
use crate::users::UserRef;

pub use orch_access_contract::audit::AuditKind;

/// How many changed nodes a tree answer may hold; past this it says it is not
/// complete, and the client loads the whole tree again.
pub const TREE_SYNC_MAX_CHANGES: i64 = 5_000;

/// What kind of node something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// An org. It lies in another org or stands alone; inside another org it is what people usually
    /// call a department.
    Org,
    /// A project, inside an org or inside another project; one that lies in a project is what the
    /// screens call a folder.
    Project,
    /// A topic, inside a project or a folder.
    Topic,
    /// A thread, inside a topic.
    Thread,
}

/// A node, named wherever another thing points at one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct NodeRef {
    /// A readable thread's current icon; absent for other nodes and readers without thread View.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_display: Option<Box<crate::threads::ThreadDisplay>>,
    /// The node's identifier.
    pub id: Id,
    /// What kind of node it is.
    pub kind: NodeKind,
    /// Its name; `null` where the reader may not know it.
    pub name: Option<String>,
    /// Whether the name was left out because the reader may not see the node.
    pub redacted: bool,
}

/// The one value of a member that is always the same word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SystemMark {
    /// The whole system.
    System,
}

/// A permission source: a hierarchy place, the system, or a document folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum NodeOrSystem {
    /// The system itself.
    System(SystemMark),
    /// A node.
    Node(NodeRef),
    /// A document-only folder whose live row is inherited, with its name redacted when unreadable.
    DocumentContainer(crate::document_tree::DocumentContainerRef),
}

impl NodeOrSystem {
    /// The system, as a place.
    pub const SYSTEM: Self = Self::System(SystemMark::System);
}

/// What a person holds on a thing, and how they hold it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Access {
    /// The actual source of native-only permissions, even when ordinary View leaves no marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_place: Option<NodeOrSystem>,
    /// Native reach alone gives View; absent when the person's ordinary pages already do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<crate::access::AdminAccess>,
    /// Held permissions absent from the ordinary frame, in catalogue order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_only: Vec<String>,
    /// A personal/group assignment here or below, or a follow; independent of View and As admin.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub participating: bool,
    /// The permissions the caller holds on this object, in sorted order.
    pub permissions: Vec<String>,
    /// The highest role the caller holds along the node's path. It is `null` on a resource, and for
    /// somebody who is named on a page without holding a role.
    pub role: Option<Role>,
    /// The place that gives the role, or the system itself for a system administrator.
    pub role_from: Option<NodeOrSystem>,
    /// Whether the caller is an admin of the place the node is in. Such a caller may change its
    /// page, and may force an act that the page does not give them.
    pub administers: bool,
    /// Whether the caller is one of its owners.
    pub owns: bool,
    /// The caller's own subscription row on this node.
    pub subscription: Option<SubscriptionLevel>,
    /// The nearest subscription on this node or above it; off when there is none.
    pub effective_subscription: SubscriptionLevel,
    /// The level inherited strictly above this node, excluding its own row; Off if none exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_subscription: Option<SubscriptionLevel>,
}

/// How a node's permission page stands against the defaults it inherits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageState {
    /// Every row follows the defaults.
    Follows,
    /// Somebody changed a row.
    Custom,
    /// Its View row no longer follows the defaults, which other products
    /// call private.
    Restricted,
}

/// Who keeps a title, a description or a set of tags up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldSource {
    /// A person wrote it.
    User,
    /// Automation writes it.
    Auto,
}

/// The `kind` of an org: always `org`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrgKind {
    /// An org.
    #[default]
    Org,
}

/// The `kind` of a project: always `project`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    /// A project.
    #[default]
    Project,
}

/// The `kind` of a topic: always `topic`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TopicKind {
    /// A topic.
    #[default]
    Topic,
}

/// What every view of a node has, whatever its kind. `K` is the one value the
/// `kind` of that view takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeBase<K> {
    /// Deletion facts on an explicitly requested read-only inspection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection: Option<crate::nodes::DeletedInspection>,
    /// The node's identifier.
    pub id: Id,
    /// What kind of node it is.
    pub kind: K,
    /// The node it lies in; `null` for a root org.
    pub parent_id: Option<Id>,
    /// Its root org.
    pub org_id: Id,
    /// The way down from the root org to it, orgs inside one another included.
    pub path: Vec<NodeRef>,
    /// Its title.
    pub name: String,
    /// Its description, whose first line is shown beside the title.
    pub description: String,
    /// The icon's palette colour, from 1 to 12; null uses the colour of its kind.
    pub color: Option<u32>,
    /// A colour of its own for the icon, `#rrggbb`, chosen in place of the twelve; absent where it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_color: Option<String>,
    /// Its README, where it named one of its own documents and the caller may read it; null otherwise. Its view
    /// shows it at the top, up to the document's first page break.
    pub readme: Option<crate::documents::DocumentRef>,
    /// Its first owner.
    pub owner: UserRef,
    /// How many people its Owners row names.
    pub owners: i64,
    /// Who maintains the title.
    pub title_source: FieldSource,
    /// Who maintains the description.
    pub description_source: FieldSource,
    /// Tags assigned to this place, regardless of its kind.
    pub tags: Vec<Tag>,
    /// Who maintains its assigned tags.
    pub tags_source: FieldSource,
    /// When automation last wrote the title or the description.
    pub auto_updated_at: Option<Time>,
    /// How its permission page stands against the defaults.
    pub page: PageState,
    /// The node's chat; reading it needs its own permission.
    pub chat_id: Id,
    /// Whether it is archived, itself or through a place above it.
    pub archived: bool,
    /// When it was archived itself.
    pub archived_at: Option<Time>,
    /// Who archived it itself.
    pub archived_by: Option<UserRef>,
    /// How many times it was changed.
    pub revision: Revision,
    /// When it was made.
    pub created_at: Time,
    /// When it was last changed.
    pub updated_at: Time,
    /// What the caller holds on it.
    pub access: Access,
}

/// Whether an org is a person's own or a team's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrgType {
    /// Made with a person, theirs alone, and empty.
    Personal,
    /// Made by somebody for a group of people.
    Team,
}

/// An org at any level; its `parent_id` is `null` for a root org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Org {
    /// What every node has.
    #[serde(flatten)]
    pub base: NodeBase<OrgKind>,
    /// A short name, unique among the orgs under the same parent, that forms one segment of the
    /// org's address.
    pub slug: String,
    /// The slugs from the root org down to this one, joined by dots.
    pub address: String,
    /// `0` for a root org; otherwise how many orgs lie above it.
    pub depth: u32,
    /// A person's own org, or a team's.
    pub org_kind: OrgType,
    /// A root org's: whether its tree takes no member beyond those it has.
    pub closed: bool,
    /// A root org's: whether it holds no org inside it. Both this and `closed`
    /// are set from above, by a system admin.
    pub flat: bool,
    /// How many people have this org, or an org inside it, as their domain.
    pub member_count: i64,
    /// Active people working in this branch: domain members and Viewer-or-higher roles here,
    /// including inherited/group roles, counted once. Absent only from an older projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub people_count: Option<i64>,
    /// The orgs directly inside it.
    pub org_count: i64,
    /// The projects directly inside it.
    pub project_count: i64,
    /// When anything in its root org's tree last changed: the rail lists a person's orgs by it, the
    /// most recently changed first.
    pub changed_at: Time,
}

/// A project, whose parent is an org at any level or another project, which the screens then call
/// a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    /// What every node has.
    #[serde(flatten)]
    pub base: NodeBase<ProjectKind>,
    /// How many topics it holds directly.
    pub topic_count: i64,
    /// The projects that lie directly in it.
    pub project_count: i64,
}

/// Whether the work of a topic is under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TopicStatus {
    /// Work goes on.
    Open,
    /// It is finished.
    Done,
}

/// How many threads of a topic need a look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TopicAttention {
    /// Threads with a turn running.
    pub running: i64,
    /// Threads waiting for an approval.
    pub waiting_approval: i64,
    /// Threads that cannot go on.
    pub blocked: i64,
}

/// Distinct visible main threads in each independently observed kind; unknown flags are not counted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TopicWorkAttention {
    /// Threads observed with included uncommitted files.
    pub uncommitted: i64,
    /// Threads observed with local commits not sent upstream.
    pub unpushed: i64,
    /// Threads whose last failure has no newer accepted input.
    pub failed: i64,
}

/// A label for topics, defined by an org or a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Tag {
    /// The tag's identifier.
    pub id: Id,
    /// The place that defines it.
    pub node: NodeRef,
    /// Its name.
    pub name: String,
    /// Its colour, an index below [`crate::users::COLOR_COUNT`].
    pub color: u32,
}

pub use crate::environment::{EnvironmentRef, EnvironmentStatus, Placement};

/// A topic: a subject of work, with its threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Topic {
    /// The most urgent presentation among threads this reader may view; null when all are quiet.
    #[serde(default)]
    pub thread_display_state: Option<crate::threads::ThreadDisplayState>,
    /// What every node has.
    #[serde(flatten)]
    pub base: NodeBase<TopicKind>,
    /// The environments that its threads run in or that have it as their home. Their owners keep
    /// them; the topic owns none.
    pub environments: Vec<EnvironmentRef>,
    /// The environment a new thread joins when it does not ask for another, provided its creator
    /// manages it.
    pub default_environment_id: Option<Id>,
    /// Whether work goes on.
    pub status: TopicStatus,
    /// When it was marked done.
    pub done_at: Option<Time>,
    /// How many threads it holds.
    pub thread_count: i64,
    /// How many of them need a look.
    pub attention: TopicAttention,
    /// Work flags are separate from running, approval and blocked counts.
    #[serde(default)]
    pub work_attention: TopicWorkAttention,
    /// When something last happened in it.
    pub last_activity_at: Option<Time>,
    /// The sequence of the topic's feed (`topic:<id>`): how far a reader of that feed has come.
    pub seq: Seq,
}

/// Any node, told apart by its `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[expect(
    clippy::large_enum_variant,
    reason = "a node is built once to be written or read; boxing the thread would put a layer in every use"
)]
pub enum Node {
    /// An org.
    Org(Org),
    /// A project.
    Project(Project),
    /// A topic.
    Topic(Topic),
    /// A thread.
    Thread(Thread),
}

/// The state of a job, as a short reference or the job itself says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for its turn.
    Queued,
    /// Under way.
    Running,
    /// Finished well.
    Succeeded,
    /// Failed.
    Failed,
    /// Stopped by somebody.
    Cancelled,
    /// The machine needed for this work was explicitly declared lost.
    Lost,
}

/// A job, in the few words that a thing it holds locked carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobRef {
    /// The job's identifier.
    pub id: Id,
    /// What it does, as a word such as `thread_fork`.
    pub kind: String,
    /// Where it stands.
    pub state: JobState,
    /// The phase it is in.
    pub phase: String,
    /// How far it is, from 0 to 100.
    pub percent: u32,
}

/// A node of the tree, with what a tree view needs and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TreeNode {
    /// Own and rolled-up thread, document and chat activity; absent only on a deletion stub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_at: Option<Time>,
    /// Native reach alone gives View; the same marker as the object's page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<crate::access::AdminAccess>,
    /// A personal/group assignment here or below, or a follow; independent of View and As admin.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub participating: bool,
    /// An org's address, including every org above it; absent for other kinds.
    pub address: Option<String>,
    /// The node's identifier.
    pub id: Id,
    /// What kind of node it is.
    pub kind: NodeKind,
    /// The node it lies in.
    pub parent_id: Option<Id>,
    /// Its name.
    pub name: String,
    /// The icon's palette colour, from 1 to 12; null uses the colour of its kind.
    pub color: Option<u32>,
    /// A colour of its own for the icon, `#rrggbb`, chosen in place of the twelve; absent where it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_color: Option<String>,
    /// Whether it is archived.
    pub archived: bool,
    /// Whether its View row no longer follows the defaults.
    pub restricted: bool,
    /// Whether it was deleted; set on a change that removes it from the view.
    pub deleted: bool,
    /// The tree sequence at which it last changed.
    pub tree_seq: Seq,
    /// What a topic adds.
    pub topic: Option<TreeTopic>,
    /// What a thread adds.
    pub thread: Option<TreeThread>,
    /// Its chat and how far the chat has come.
    pub chat: TreeChat,
    /// Live documents beside it that follow its documents rows; private ones are listed separately.
    pub docs: i64,
}

/// What a tree view says of a topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TreeTopic {
    /// Whether work goes on.
    pub status: TopicStatus,
    /// Its tags, by identifier.
    pub tag_ids: Vec<Id>,
}

/// What a tree view says of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TreeThread {
    /// The server-selected icon and its particulars for this reader.
    #[serde(default)]
    pub display: Box<crate::threads::ThreadDisplay>,
    /// Independent nullable work flags and an unresolved last failed turn.
    #[serde(default)]
    pub attention: crate::source::ThreadAttention,
    /// What it is doing.
    pub state: ThreadState,
    /// Why it cannot go on, when it cannot.
    pub blocked_reason: Option<BlockedReason>,
    /// A known setup refusal, classified without exposing diagnostic text or internal setting keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_cause: Option<TreeBlockedCause>,
    /// The outstanding request to name, with current decision authority for this reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<TreeWaiting>,
    /// How many approvals wait.
    pub pending_approvals: i64,
    /// The sequence of the last change worth a reader's attention.
    pub activity_seq: Seq,
    /// When something last happened in it.
    pub last_activity_at: Option<Time>,
}

/// A specific cause within an otherwise invalid agent/environment binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TreeBlockedCause {
    /// No sandbox fits this agent, environment and their current limits.
    Sandbox,
    /// The selected approval mode is unavailable or disallowed.
    ApprovalMode,
    /// The sender must complete their account's second step.
    SecondStep,
}

/// What a thread's outstanding request asks of a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TreeWaitingKind {
    /// Permission to act.
    Approval,
    /// An answer to a question or an MCP elicitation.
    Answer,
}

/// A compact request summary; it carries no request content or decision control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TreeWaiting {
    /// Whether permission or an answer is wanted.
    pub kind: TreeWaitingKind,
    /// This reader can decide this request now, under that request's own authority.
    pub for_caller: bool,
}

/// A node's chat, as the tree names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TreeChat {
    /// The chat's identifier.
    pub id: Id,
    /// The sequence of its last message.
    pub seq: Seq,
}

/// The nodes of an org's tree that the caller may view, whole or as a change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OrgTreeGetReply {
    /// The native settings of this root, on the first full-load page only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Vec<crate::settings::ResolvedSetting>>,
    /// The tree sequence this is as of.
    pub seq: Seq,
    /// The sequence the caller asked from, if it did.
    pub since: Option<Seq>,
    /// False while a full load has another page, or when more than
    /// [`TREE_SYNC_MAX_CHANGES`] nodes changed since then. In the latter
    /// case there is no cursor and the client reloads without `since`.
    pub complete: bool,
    /// The next full-load page. All pages retain the first page's sequence;
    /// after collecting them the client can repair changes from that sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// The nodes: all of them, or those changed since, deleted and no longer
    /// visible ones marked as `deleted`. A private node its reader administers
    /// and may not view is its minimal row.
    pub nodes: Vec<Listed<TreeNode>>,
}

impl OrgTreeGetReply {
    /// The rows the caller may read, minimal rows left out.
    #[must_use]
    pub fn readable(&self) -> Vec<&TreeNode> {
        self.nodes.iter().filter_map(Listed::as_visible).collect()
    }

    /// The minimal rows of private nodes the caller administers.
    #[must_use]
    pub fn hidden(&self) -> Vec<&HiddenItem> {
        self.nodes.iter().filter_map(Listed::as_hidden).collect()
    }
}

/// What kind of line a place's Activity holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// Something done to the place or to a thing directly inside it, such as making, renaming,
    /// switching, forking, archiving, or changing its page or settings.
    History,
    /// A turn of one of its threads ended well.
    TurnCompleted,
    /// A turn of one of its threads failed.
    TurnFailed,
    /// One of its threads' agents announced a milestone.
    Milestone,
    /// An announcement reached one of its threads.
    Announcement,
    /// An approval of one of its threads was decided.
    ApprovalDecided,
}

/// The thing an act was about, named only as this reader may know it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityObject {
    /// The thing, independently of the place whose Activity carries it.
    pub id: Id,
    /// Its ordinary node or resource kind, such as `project`, `document` or `provider`.
    pub kind: String,
    /// Its current display name; absent for a removed or concealed thing.
    pub name: Option<String>,
    /// Whether its name is withheld.
    pub redacted: bool,
}

/// One line of a place's Activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityEntry {
    /// When it happened.
    pub at: Time,
    /// Who did it: a person, or the agent of a thread.
    pub actor: Option<HistoryActor>,
    /// The reader-safe display title of the object, never a server-composed sentence.
    /// The client composes the act from `actor`, `wording`, `object` and `node`; audit exports retain
    /// the audit's original sentence.
    pub title: String,
    /// The actual thing acted on, which may be a resource inside `node`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<ActivityObject>,
    /// Known versioned history facts for localized wording, without interpreting the fallback title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wording: Option<crate::access::HistoryWording>,
    /// How many acts this entry represents; one unless Summary grouped them.
    pub count: i64,
    /// What the line is about: the place itself, or something directly inside it.
    pub node: NodeRef,
    /// What kind of line it is.
    pub kind: ActivityKind,
    /// For a history line, the audit action such as `node.created`.
    pub action: Option<String>,
    /// The place to jump to when the line reports what a thread did.
    pub thread: Option<NodeRef>,
    /// The item to jump to in that thread.
    pub item_id: Option<Id>,
}

/// Who did something that was audited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditActor {
    /// A person.
    User {
        /// The person.
        user: UserRef,
    },
    /// The system itself.
    System,
    /// The agent of a thread.
    Agent {
        /// The thread.
        thread: NodeRef,
        /// The person for whom the act ran, where recorded.
        for_user: Option<UserRef>,
    },
}

/// Read-only deletion facts returned by an authorized inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeletedInspection {
    /// When the object was deleted.
    pub deleted_at: Time,
    /// Who deleted it, where known.
    pub deleted_by: Option<UserRef>,
    /// Its promised purge day.
    pub purge_at: Time,
    /// Always true for an inspection.
    pub readonly: bool,
}

/// Explicitly asks an existing stored-content reader to inspect a deleted thing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeletedInspectionQuery {
    /// Apply this thing's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
}

/// Current lifecycle of the thing an audit row points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditObjectState {
    /// The object is live.
    Live,
    /// Retained after deletion.
    Deleted,
    /// Physical purge was confirmed.
    Purged,
}

/// What an audited act was done to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuditObject {
    /// The current deletion group, or unknown for a legacy record.
    pub deletion_id: Option<Id>,
    /// The name captured when this event happened, or unknown.
    pub name: Option<String>,
    /// Current lifecycle state of the object.
    pub state: AuditObjectState,
    /// The current deletion time.
    pub deleted_at: Option<Time>,
    /// The deleting person, where known.
    pub deleted_by: Option<UserRef>,
    /// Its promised purge day.
    pub purge_at: Option<Time>,
    /// Actual confirmed purge day.
    pub purged_at: Option<Time>,
    /// Current ordinary View permits opening this live thing.
    pub can_open: bool,
    /// This reader has the current restoration authority on retained content.
    pub can_inspect: bool,
    /// This reader may currently restore this retained thing.
    pub can_restore: bool,
    /// What kind of thing it is.
    pub kind: String,
    /// Its identifier, where it has one.
    pub id: Option<Id>,
}

/// One row of the audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuditEntry {
    /// The human act used the session's explicit As admin switch.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub as_admin: bool,
    /// The group captured by this deletion event, independent of the current group.
    pub event_deletion_id: Option<Id>,
    /// The exact catalogue kind; null when a stored action is not known to this reader.
    pub kind: Option<AuditKind>,
    /// Common request identity, where recorded.
    pub request_id: Option<String>,
    /// The row's identifier.
    pub id: Id,
    /// When it happened.
    pub at: Time,
    /// Who did it.
    pub actor: AuditActor,
    /// What was done, as `kind.verb`.
    pub action: String,
    /// What it was done to.
    pub object: AuditObject,
    /// Whether it is also a line of the object's history.
    pub history: bool,
    /// Set on a permission an admin put themself on to do what the page did not
    /// give them.
    pub r#override: bool,
    /// The node it is about.
    pub node: Option<NodeRef>,
    /// What happened, in a sentence.
    pub summary: String,
    /// The facts of it. Never a secret, never content.
    pub details: Json,
    /// The address the act came from.
    pub ip: Option<String>,
}

/// A setting that would resolve differently in the place a node is moved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoveSetting {
    /// The setting's key.
    pub key: String,
    /// The value now.
    pub from: Option<Json>,
    /// The value there.
    pub to: Option<Json>,
    /// The place whose lock decides it there, if one does.
    pub locked_by: Option<NodeOrSystem>,
}

/// How many people would hold a role in the moved place from above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoveRoles {
    /// People who would start to hold one.
    pub gaining: i64,
    /// People who would stop holding one.
    pub losing: i64,
}

/// What would change the address of things.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoveAddresses {
    /// People whose address begins with the path of the moved org.
    pub members: i64,
    /// Resources whose address begins with it.
    pub resources: i64,
}

/// What a project, a topic, or an org inside another, would inherit if it were moved elsewhere in
/// its tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeMovePlan {
    /// Caller-bound review of these exact consequences.
    pub preflight_id: Id,
    /// The reviewed subtree and consequence stamp.
    pub stamp: String,
    /// The node's reviewed revision.
    pub revision: Revision,
    /// When this review expires.
    pub expires_at: Time,
    /// What is moved.
    pub node: NodeRef,
    /// Where it lies now.
    pub from: NodeRef,
    /// Where it would lie.
    pub to: NodeRef,
    /// Who would gain or lose what on the pages inside it that follow the defaults.
    pub access: PagePreview,
    /// The settings that resolve differently there.
    pub settings: Vec<MoveSetting>,
    /// Roles it receives from a place above: how many people start to hold one, and how many stop.
    pub roles: MoveRoles,
    /// What changes address; zero for a project and for a topic, which have no part in one.
    pub addresses: MoveAddresses,
    /// What stops it; when there is any, it cannot be moved.
    pub blockers: Vec<PlanBlocker>,
}

/// How a node's page starts when it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewNodeAccess {
    /// Every row follows the defaults.
    Defaults,
    /// Every row is cut loose: its maker's alone until somebody is named.
    Private,
}

/// Which pieces of a node's maintenance automation may write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutoField {
    /// The title.
    Title,
    /// The description.
    Description,
    /// The tags.
    Tags,
}

/// What to ask of automation about a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutoAction {
    /// Hand the field back to automation.
    Resume,
    /// Hand it back and ask for a new value now.
    Refresh,
}

/// The binding a new thread is made with, when it is not left to the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateThreadBinding {
    /// The agent profile.
    pub agent_id: Id,
    /// The provider.
    pub provider_id: Id,
    /// How the agent is placed; the first permitted arrangement when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<ArrangementType>,
}

/// The first input of a thread, sent with its creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirstInput {
    /// What to say to the agent.
    pub content: Vec<ContentPart>,
    /// A value of the client's own that makes sending it again safe.
    pub client_id: String,
}

/// Everything that makes a thread, all of it optional: where the topic has
/// defaults, `{}` makes a thread that is ready, and `first_input` starts it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateThread {
    /// Its name; `New thread` until renamed, with its title kept by automation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The page it starts with; it follows its topic's defaults when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<ThreadAccess>,
    /// The agent and provider; the resolved defaults when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<CreateThreadBinding>,
    /// The model, written as the thread's `agent.model` setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The effort, written as its `agent.effort` setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The parameters, written as its `agent.params` setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, Json>>,
    /// Where it runs; by the topic's default for new threads when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentChoice>,
    /// Its directory in that environment, relative to the workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Prepare the repository when the directory is missing on any machine, unless explicitly false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare_source: Option<bool>,
    /// An own branch and unused worktree directory in an existing environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<crate::source::WorktreeChoice>,
    /// A subdirectory of that directory where the agent starts, written as
    /// `thread.cwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// What to say to the agent at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_input: Option<FirstInput>,
}

/// Makes a root org of kind team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgCreateBody {
    /// Its name.
    pub name: String,
    /// Its short name.
    pub slug: String,
}

/// Makes an org inside another, which people call a department.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgChildCreateBody {
    /// Its name.
    pub name: String,
    /// Its short name, unique among the orgs of that parent.
    pub slug: String,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether to give it a project named General.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general: Option<bool>,
    /// How its page starts; by the defaults when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewNodeAccess>,
}

/// The place to preview moving a project, a topic, or an org inside another, into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MovePreviewBody {
    /// The place to move it under: an org for an org; an org or a project for a project; a
    /// project, at any depth, for a topic.
    pub parent_id: Id,
}

/// Moves a project, a topic, or an org that lies in another, under another place of the same
/// tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeMoveBody {
    /// The place to move it under: an org for an org; an org or a project for a project; a
    /// project, at any depth, for a topic.
    pub parent_id: Id,
    /// The revision the caller saw.
    pub expected_revision: Revision,
    /// The current consequences review, when the caller confirms a displayed plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_id: Option<Id>,
}

/// Which archived nodes a list holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchivedFilter {
    /// None of them.
    #[default]
    Exclude,
    /// All nodes, archived ones among them.
    Include,
    /// Only the archived ones.
    Only,
}

/// The nodes directly inside a node, by when they were made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeChildListQuery {
    /// Only ordinary page access, independent of native administrator reach; before paging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participating: Option<bool>,
    /// Inspect retained deleted content under this object's current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    /// Only this kind of child, which the parent must be able to hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<NodeKind>,
    /// Whether archived nodes are listed; they are not by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<ArchivedFilter>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Makes a project in an org, or inside another project, where it is called a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectCreateBody {
    /// Its name.
    pub name: String,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How its page starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewNodeAccess>,
}

/// Makes a topic, and with `first_thread` its first thread in the same step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopicCreateBody {
    /// Its name; `New topic`, with its title kept by automation, when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How its page starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewNodeAccess>,
    /// A thread to make with it, in one transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_thread: Option<CreateThread>,
}

/// A topic and the thread that was made with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TopicWithThread {
    /// The topic.
    pub topic: Topic,
    /// Its first thread.
    pub thread: Thread,
}

/// What making a topic answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[expect(
    clippy::large_enum_variant,
    reason = "an answer is built once to be written; boxing the thread would put a layer in every use"
)]
pub enum TopicCreateReply {
    /// The topic and its first thread, when one was asked for.
    WithThread(TopicWithThread),
    /// The topic alone.
    Topic(Topic),
}

/// Attaches a thread to a session that already exists in a provider's harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadAdoptBody {
    /// The provider whose harness holds the session.
    pub provider_id: Id,
    /// The agent profile.
    pub agent_id: Id,
    /// The harness's own reference to the session.
    pub native_session_ref: String,
    /// The environment; the caller's one on that machine whose root holds the
    /// session's directory when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<Id>,
    /// Its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// What adopting a session would do, before anything does: the thread it would become, where it would run,
/// the model it would run on, and what of what it held before would come with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdoptPlan {
    /// The session, as the harness lists it.
    pub session: crate::resources::NativeSession,
    /// The name the thread would have.
    pub name: String,
    /// The environment it would be in: the one named, or the caller's on the harness's machine whose folder
    /// holds the session's directory; `null` where none does.
    pub environment: Option<crate::environment::EnvironmentRef>,
    /// The thread's directory there, relative to its folder; `null` without an environment.
    pub directory: Option<String>,
    /// The folder to register first, where no environment of the caller's holds the session's directory: the
    /// directory the session ran in, as its machine writes it.
    pub register_first: Option<String>,
    /// The model the session ran on, as its harness says; `null` where it does not.
    pub session_model: Option<String>,
    /// The model the thread would run on: the session's own; `null` where a person chooses one.
    pub model: Option<String>,
    /// Why it would not run on the session's model, where it would not.
    pub model_reason: Option<crate::threads::AdoptModelReason>,
    /// How many of its earlier turns would come with it, as items from the session before Orch.
    pub turns: i64,
    /// Why nothing of what it held before would come, where nothing would.
    pub history_missing: Option<crate::threads::AdoptHistoryMissing>,
    /// The thread that took the session in already, when one did: adopting it again is refused.
    pub adopted_thread_id: Option<Id>,
}

/// How a list of topics is filtered by what is open or done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TopicStatusFilter {
    /// Topics that are open.
    #[default]
    Open,
    /// Topics that are done.
    Done,
    /// Both.
    All,
}

/// Which kind of attention a list of topics is narrowed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionFilter {
    /// Topics with a thread waiting for an approval or unable to go on.
    NeedsAttention,
    /// Topics with a turn running.
    Running,
    /// Topics with a thread waiting for an approval.
    WaitingApproval,
    /// Topics with a thread that cannot go on.
    Blocked,
}

/// How a list of topics is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TopicSort {
    /// The most recent activity first.
    #[default]
    Activity,
    /// The most recently made first.
    Created,
    /// By name.
    Name,
}

/// The topics of a project that the caller may view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopicListQuery {
    /// Only ordinary page access, independent of native administrator reach; before paging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participating: Option<bool>,
    /// Open topics by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TopicStatusFilter>,
    /// Only topics that carry every one of these tags; repeat the parameter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tag_id: Vec<Id>,
    /// Only topics with this kind of attention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<AttentionFilter>,
    /// Whether the topics of every folder below the project are listed too, each with its folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_folders: Option<bool>,
    /// Only topics whose name or description holds this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// The order; by activity by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<TopicSort>,
    /// Whether archived topics are listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<ArchivedFilter>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Changes a topic's status or tags. Setting the tags makes a person their
/// keeper.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopicUpdateBody {
    /// Whether work goes on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TopicStatus>,
    /// The tags it has from now on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_ids: Option<Vec<Id>>,
    /// The revision of the topic that whoever asks has seen: a change, and the change that undoes
    /// it, is refused when somebody changed the topic since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// Replaces the assigned tags of any place, using definitions at that place or above it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeTagSetBody {
    /// The tags assigned from now on; an empty list removes all assignments.
    pub tag_ids: Vec<Id>,
    /// The node revision that was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// Changes a node's name, description or icon colour. Writing text hands that field to
/// the person, until it is given back to automation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeUpdateBody {
    /// The new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The new description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The icon's palette colour, from 1 to 12; null restores the colour of its kind.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<u32>")]
    pub color: Option<Option<u32>>,
    /// A colour of its own for the icon, `#rrggbb`, in place of the twelve's; null takes it away and leaves the
    /// colour of its kind. One of the twelve takes the place of a colour of its own, so a request gives one or the
    /// other.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub custom_color: Option<Option<String>>,
    /// One of the node's own documents to show at the top of its view as its README; null for none.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<Id>")]
    pub readme_document_id: Option<Option<Id>>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// Hands a field back to automation, or asks it for a new value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeAutoBody {
    /// The field.
    pub field: AutoField,
    /// What to ask.
    pub action: AutoAction,
}

/// How much of a place's Activity is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityDepth {
    /// Meaningful acts, with repeated acts grouped within an hour.
    #[default]
    Summary,
    /// Every recorded change in the place.
    Every,
}

/// The lines of a place's Activity, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeActivityListQuery {
    /// Summary when absent; Every change when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<ActivityDepth>,
    /// Only these kinds of line; repeat the parameter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kind: Vec<ActivityKind>,
    /// Only lines after this moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Time>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// How much of a place the caller wants to be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSubscriptionSetBody {
    /// The level.
    pub level: SubscriptionLevel,
}

/// The audit trail of a place, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeAuditListQuery {
    /// Include agent acts for this person as well as their direct acts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_agents: Option<bool>,
    /// Only this exact meaning of act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::nodes::AuditKind>,
    /// Only what this person did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Id>,
    /// Only this action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Only rows from this moment on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Time>,
    /// Only rows up to this moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Time>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// What an org's tree answers with, from a sequence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgTreeGetQuery {
    /// Only the nodes that changed after this tree sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Seq>,
    /// Continue a full load; cannot be combined with `since`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Nodes scanned in a full-load page, from 1 to [`TREE_SYNC_MAX_CHANGES`].
    /// Omitted means that maximum. A delta always uses the change bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A readable shared place or a distinct document-only folder.
/// Existing places serialize exactly as their ordinary Node response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SharedListItem {
    /// A place, retaining its existing flat JSON shape.
    Node(Box<Node>),
    /// A readable document whose containing hierarchy place cannot be browsed.
    Document {
        /// A text document, independent of hierarchy and document folders.
        kind: crate::documents::DocumentMark,
        /// The same authorized reference as an ordinary document list, without content.
        document: Box<crate::documents::DocumentRef>,
        /// Its actual hierarchy path, with unreadable names redacted.
        path: Vec<NodeRef>,
        /// Ordinary participation, independent of administrative reach.
        participating: bool,
    },
    /// A document-only folder, opened by its own identity and document Read.
    DocumentContainer {
        /// The distinct folder kind, never a hierarchy project or text document.
        kind: crate::document_tree::DocumentContainerMark,
        /// Its current readable name and stable identity.
        folder: crate::document_tree::DocumentContainerRef,
        /// Its actual hierarchy path, with unreadable names independently redacted.
        path: Vec<NodeRef>,
        /// Ordinary participation from the native document frame, never inferred from admin standing.
        participating: bool,
        /// View comes only through the current session's administrative reach.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        admin_access: Option<crate::access::AdminAccess>,
    },
}

/// Newest named shares, including documents and owned document folders. The cursor
/// advances over scanned candidates, including any whose access was withdrawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharedListReply {
    /// Readable shared places, documents and document-only folders, newest first.
    pub items: Vec<SharedListItem>,
    /// More candidates remain; an empty page may still be partial.
    pub partial: bool,
    /// Continue from this scanned position, or null at the end.
    pub next_cursor: Option<String>,
}

/// Filters and format for a permanent trail exported from one place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeAuditExportQuery {
    /// Only this person's acts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Id>,
    /// Also include acts of agents for that person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_agents: Option<bool>,
    /// Only this exact action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Only this meaning of act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AuditKind>,
    /// Start of the selected span.
    pub from: Time,
    /// End of the selected span, no more than 366 days later.
    pub to: Time,
    /// File format.
    pub format: crate::admin::AuditExportFormat,
}

/// One node covered by an archive review; its own archive mark is distinct from an inherited one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeArchiveItem {
    /// Its readable name, or a private reference.
    pub node: NodeRef,
    /// The node's own revision.
    pub revision: Revision,
    /// Whether this node has an archive mark of its own.
    pub own_archived: bool,
    /// Whether it remains read-only after the reviewed act, including marks on its ancestors.
    pub read_only_after: bool,
}

/// Work settled by archiving one thread; withheld counts are null, never a claim of no work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeArchiveWork {
    /// The thread, redacted under its ordinary View right.
    pub thread: NodeRef,
    /// Running or starting turns that are interrupted.
    pub turns: Option<i64>,
    /// Pending or delivering inputs that are cancelled.
    pub inputs: Option<i64>,
    /// Pending approvals that are cancelled.
    pub approvals: Option<i64>,
    /// Open leases that are closed.
    pub leases: Option<i64>,
    /// Whether the current harness segment is closed.
    pub session: Option<bool>,
    /// External links kept, with editing read-only while archived.
    pub links: Option<i64>,
}

/// An environment retained by an archive; its name is withheld without its ordinary View right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeArchiveEnvironment {
    /// Its identifier.
    pub id: Id,
    /// Its name, if readable.
    pub name: Option<String>,
    /// Whether the name is withheld.
    pub redacted: bool,
}

/// The complete read-only review of archiving or unarchiving a subtree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeArchivePlan {
    /// Caller-bound stored review, for this node and direction only.
    pub preflight_id: Id,
    /// When the review expires.
    pub expires_at: Time,
    /// The exact subtree/work/share stamp the act must still match.
    pub stamp: String,
    /// True for archive, false for unarchive.
    pub archiving: bool,
    /// The selected node.
    pub node: NodeRef,
    /// Its revision at review.
    pub revision: Revision,
    /// Every live node covered, including private references and independently archived descendants.
    pub subtree: Vec<NodeArchiveItem>,
    /// Counts of nodes and documents covered by the read-only change.
    pub counts: crate::ownership::NodeDeleteCounts,
    /// Per-thread work; an unarchive restarts none of it.
    pub work: Vec<NodeArchiveWork>,
    /// Environments are retained and not stopped by this act; names require their ordinary View right.
    pub environments: Vec<NodeArchiveEnvironment>,
    /// Existing copies retained, never revoked or re-delivered by archiving.
    pub copies_kept: i64,
    /// Live agent entries retained; archived source content permits reading and refuses writing.
    pub live_shares_kept: i64,
    /// Reasons the reviewed act cannot run.
    pub blockers: Vec<crate::forks::PlanBlocker>,
}

/// Confirms the exact archive review, never an unreviewed subtree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeArchiveBody {
    /// The stored plan returned for this node and direction.
    pub preflight_id: Id,
    /// Its subtree stamp.
    pub stamp: String,
    /// Its node revision; the same guard also fences Undo.
    pub expected_revision: Revision,
}

/// The settled node and a reviewed inverse fenced to this exact success; no inverse is made for a no-op.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeArchiveResult {
    /// The node after the act.
    pub node: Node,
    /// Undo's exact reverse review, which restarts no cancelled work.
    pub undo: Option<NodeArchivePlan>,
}

crate::endpoints! {
    /// Exports the filtered permanent trail of this event-time place.
    NodeAuditExport: GET "/nodes/{id}/audit/export", Permission("audit.view"), query NodeAuditExportQuery, none, raw, 200;
    /// Lists the root orgs the caller owns or belongs to, their own org first.
    OrgList: GET "/orgs", User, none, none, json Vec<Org>, 200;
    /// Makes a root org of kind team, with the caller as its first owner and member, where `orgs.created_by` lets the caller: anybody signed in, a system admin, or somebody a system admin invited to make one, whose invitation this uses up.
    OrgCreate: POST "/orgs", User, none, json OrgCreateBody, json Org, 200;
    /// Makes an org inside another, which people call a department.
    OrgChildCreate: POST "/orgs/{id}/orgs", Permission("org.manage"), none, json OrgChildCreateBody, json Org, 200;
    /// Shows what moving an org that lies in another would change, before anything does.
    OrgMovePreview: POST "/orgs/{id}/move/preview", Rule("the same as the move itself"), none, json MovePreviewBody, json NodeMovePlan, 200;
    /// Moves an org that lies in another under another org of the same root org.
    OrgMove: POST "/orgs/{id}/move", Rule("org.manage on the org and on the org it moves under"), none, json NodeMoveBody, json Org, 200;
    /// Reads an org by the short name of a root org, or by an address.
    OrgBySlugGet: GET "/orgs/by-slug/{slug}", Permission("node.view"), none, none, json Org, 200;
    /// Reads the nodes of an org's tree that the caller may view, whole or since a sequence.
    OrgTreeGet: GET "/orgs/{id}/tree", User, query OrgTreeGetQuery, none, json OrgTreeGetReply, 200;
    /// Lists named places whose parent is unreadable and current named/owned document folders.
    SharedList: GET "/shared", User, query crate::common::PageQuery, none, json SharedListReply, 200;
    /// Reads a node of any kind.
    NodeGet: GET "/nodes/{id}", Permission("node.view"), query crate::nodes::DeletedInspectionQuery, none, json Node, 200;
    /// Changes a node's name, description or icon colour.
    NodeUpdate: PATCH "/nodes/{id}", Permission("node.manage"), none, json NodeUpdateBody, json Node, 200;
    /// Lists what lies directly inside a node, by when it was made.
    NodeChildList: GET "/nodes/{id}/children", Permission("node.view"), query NodeChildListQuery, none, json Page<Listed<Node>>, 200;
    /// Makes a project in an org, at whatever level it lies.
    ProjectCreate: POST "/orgs/{id}/projects", Permission("org.interact"), none, json ProjectCreateBody, json Project, 200;
    /// Makes a project inside a project, beside its topics: a folder, which is a project like any other.
    ProjectChildCreate: POST "/projects/{id}/projects", Permission("project.interact"), none, json ProjectCreateBody, json Project, 200;
    /// Shows what moving a project would change, before anything does.
    ProjectMovePreview: POST "/projects/{id}/move/preview", Rule("the same as the move itself"), none, json MovePreviewBody, json NodeMovePlan, 200;
    /// Moves a project to another org, or into another project, of its root org.
    ProjectMove: POST "/projects/{id}/move", Rule("project.manage on the project, and org.interact on the org or project.interact on the project it moves under"), none, json NodeMoveBody, json Project, 200;
    /// Makes a topic in a project, and with it a first thread if one is asked for.
    TopicCreate: POST "/projects/{id}/topics", Permission("project.interact"), none, json TopicCreateBody, json TopicCreateReply, 200;
    /// Shows what moving a topic to another project or folder would change, before anything does.
    TopicMovePreview: POST "/topics/{id}/move/preview", Rule("the same as the move itself"), none, json MovePreviewBody, json NodeMovePlan, 200;
    /// Moves a topic with its threads to another project or folder of its root org.
    TopicMove: POST "/topics/{id}/move", Rule("topic.manage on the topic, and project.interact on the project or folder it moves to"), none, json NodeMoveBody, json Topic, 200;
    /// Makes a thread in a topic.
    ThreadCreate: POST "/topics/{id}/threads", Permission("topic.interact"), none, json CreateThread, json Thread, 200;
    /// Attaches a new thread to a session that already exists in a provider's harness.
    ThreadAdopt: POST "/topics/{id}/threads/adopt", Rule("topic.interact, the use of the provider and agent, and environment.manage on the environment"), none, json ThreadAdoptBody, json Thread, 200;
    /// Shows what adopting a session would do (where it would run, the model, the history it brings), before anything does.
    ThreadAdoptPreview: POST "/topics/{id}/threads/adopt/preview", Rule("the same as adopting it"), none, json ThreadAdoptBody, json AdoptPlan, 200;
    /// Lists the topics of a project that the caller may view.
    TopicList: GET "/projects/{id}/topics", Permission("node.view"), query TopicListQuery, none, json Page<Listed<Topic>>, 200;
    /// Changes a topic's status or tags.
    TopicUpdate: PATCH "/topics/{id}", Permission("topic.interact"), none, json TopicUpdateBody, json Topic, 200;
    /// Replaces the tags assigned to an org, project, topic or thread.
    NodeTagSet: PUT "/nodes/{id}/tags", Permission("node.interact"), none, json NodeTagSetBody, json Node, 200;
    /// Hands a title, a description or the tags back to automation, or asks it for a new value.
    NodeAuto: POST "/nodes/{id}/auto", Permission("node.manage"), none, json NodeAutoBody, json Node, 200;
    /// Archives an org, a project, a topic or a thread, with everything inside it; the system has no archive.
    NodeArchive: POST "/nodes/{id}/archive", Rule("node.manage or node.archive on the node, and its current reviewed stamp"), none, json NodeArchiveBody, json NodeArchiveResult, 200;
    /// Takes a node out of the archive.
    NodeUnarchive: POST "/nodes/{id}/unarchive", Rule("node.manage or node.archive on the node, and its current reviewed stamp"), none, json NodeArchiveBody, json NodeArchiveResult, 200;
    /// Reviews archiving the complete subtree without changing its work or archive marks.
    NodeArchivePreview: POST "/nodes/{id}/archive/preview", Rule("node.manage or node.archive on the node"), none, none, json NodeArchivePlan, 200;
    /// Reviews clearing this node's own archive mark; nothing is restarted.
    NodeUnarchivePreview: POST "/nodes/{id}/unarchive/preview", Rule("node.manage or node.archive on the node"), none, none, json NodeArchivePlan, 200;
    /// Lists the history of a place and what its threads did, newest first.
    NodeActivityList: GET "/nodes/{id}/activity", Permission("node.view"), query NodeActivityListQuery, none, json Page<ActivityEntry>, 200;
    /// Sets how much of a node the caller wants to be told.
    NodeSubscriptionSet: PUT "/nodes/{id}/subscription", Permission("node.view"), none, json NodeSubscriptionSetBody, json Access, 200;
    /// Removes the caller's own subscription row on a node.
    NodeSubscriptionDelete: DELETE "/nodes/{id}/subscription", Permission("node.view"), none, none, json Access, 200;
    /// Lists the audit trail of a place, newest first.
    NodeAuditList: GET "/nodes/{id}/audit", Permission("audit.view"), query NodeAuditListQuery, none, json Page<AuditEntry>, 200;
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_shared_document_folder_has_a_distinct_typed_identity_and_redacted_hierarchy_path() {
        use crate::document_tree::{DocumentContainerMark, DocumentContainerRef};
        let id = Id::from_u128(101);
        let item = SharedListItem::DocumentContainer {
            kind: DocumentContainerMark::DocumentContainer,
            folder: DocumentContainerRef {
                id,
                kind: DocumentContainerMark::DocumentContainer,
                name: Some("Research".into()),
                redacted: false,
            },
            path: vec![NodeRef {
                thread_display: None,
                id: Id::from_u128(100),
                kind: NodeKind::Topic,
                name: None,
                redacted: true,
            }],
            participating: true,
            admin_access: None,
        };
        let value = serde_json::to_value(&item).unwrap();
        assert_eq!(value["kind"], "document_container");
        assert_eq!(value["folder"]["id"], id.to_string());
        assert_eq!(value["path"][0]["name"], serde_json::Value::Null);
        assert_eq!(
            serde_json::from_value::<SharedListItem>(value).unwrap(),
            item
        );
    }

    #[test]
    fn a_persons_trail_filters_agent_responsibility_and_the_exact_kind_without_widening_inspection()
    {
        let query: super::NodeAuditListQuery = serde_json::from_value(serde_json::json!({
            "include_agents": true, "kind": "access", "action": "deleted.inspected"
        }))
        .unwrap();
        assert_eq!(query.kind, Some(super::AuditKind::Access));
        assert_eq!(query.include_agents, Some(true));
        assert!(
            serde_json::from_value::<super::NodeAuditListQuery>(
                serde_json::json!({"kind":"unknown"})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<super::NodeAuditExportQuery>(serde_json::json!({
            "format":"jsonl", "from":"2026-10-13T00:00:00.000Z", "to":"2026-10-14T00:00:00.000Z", "inspect_deleted":true
        })).is_err());
    }
    use super::*;
    use serde_json::{Value, json};

    fn user() -> Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "human", "username": "alice",
            "display_name": "Alice Chen", "avatar": { "text": "AC", "color": 4 }
        })
    }

    /// The members every kind of node has, written as the API writes them.
    fn node(kind: &str, extra: Value) -> Value {
        let mut written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01",
            "kind": kind,
            "parent_id": null,
            "org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02",
            "path": [{ "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02", "kind": "org", "name": "Acme", "redacted": false }],
            "name": "Acme",
            "description": "The company\nand its parts",
            "color": null,
            "readme": null,
            "owner": user(),
            "owners": 1,
            "title_source": "user",
            "description_source": "auto",
            "tags": [], "tags_source": "auto",
            "auto_updated_at": "2026-10-09T08:00:00.000Z",
            "page": "follows",
            "chat_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03",
            "archived": false,
            "archived_at": null,
            "archived_by": null,
            "revision": 3,
            "created_at": "2026-10-08T17:04:05.123Z",
            "updated_at": "2026-10-09T08:00:00.000Z",
            "access": {
                "permissions": ["node.view", "org.manage"],
                "role": "admin",
                "role_from": "system",
                "administers": true,
                "owns": true,
                "subscription": null,
                "effective_subscription": "off"
            }
        });
        if let (Some(base), Value::Object(more)) = (written.as_object_mut(), extra) {
            base.extend(more);
        }
        written
    }

    #[test]
    fn a_node_is_told_by_its_kind_among_the_four() {
        let org = node(
            "org",
            json!({ "slug": "acme", "address": "acme", "depth": 0, "org_kind": "team", "closed": false, "flat": false,
                    "member_count": 3, "org_count": 1, "project_count": 2, "changed_at": "2026-10-09T08:00:00.000Z" }),
        );
        let parsed: Node = serde_json::from_value(org.clone()).unwrap();
        assert!(
            matches!(&parsed, Node::Org(found) if found.slug == "acme" && found.base.access.role_from == Some(NodeOrSystem::SYSTEM))
        );
        assert_eq!(serde_json::to_value(&parsed).unwrap(), org);

        let project = node("project", json!({ "topic_count": 5, "project_count": 2 }));
        assert!(
            matches!(serde_json::from_value::<Node>(project).unwrap(), Node::Project(found) if found.topic_count == 5 && found.project_count == 2)
        );

        let wrong = node(
            "project",
            json!({ "slug": "acme", "address": "acme", "depth": 0, "org_kind": "team", "closed": false, "flat": false,
                                             "member_count": 3, "org_count": 1, "project_count": 2 }),
        );
        assert!(
            serde_json::from_value::<Org>(wrong).is_err(),
            "an org is not made of a project that has a slug"
        );
    }

    #[test]
    fn a_thread_may_be_made_from_nothing_at_all() {
        let nothing: CreateThread = serde_json::from_value(json!({})).unwrap();
        assert_eq!(nothing, CreateThread::default());
        assert_eq!(serde_json::to_value(&nothing).unwrap(), json!({}));
        assert!(serde_json::from_value::<CreateThread>(json!({ "title": "Fix login" })).is_err());
    }

    #[test]
    fn a_colour_of_its_own_is_kept_when_absent_and_taken_away_when_null() {
        let keep: NodeUpdateBody = serde_json::from_value(json!({"expected_revision": 1})).unwrap();
        let clear: NodeUpdateBody =
            serde_json::from_value(json!({"custom_color": null, "expected_revision": 1})).unwrap();
        let set: NodeUpdateBody =
            serde_json::from_value(json!({"custom_color": "#3a7bd5", "expected_revision": 1}))
                .unwrap();
        assert_eq!(keep.custom_color, None);
        assert_eq!(clear.custom_color, Some(None));
        assert_eq!(set.custom_color, Some(Some("#3a7bd5".to_owned())));
        assert_eq!(
            serde_json::to_value(clear).unwrap(),
            json!({"custom_color": null, "expected_revision": 1})
        );
    }

    #[test]
    fn a_colour_is_kept_when_absent_and_cleared_when_null() {
        let keep: NodeUpdateBody = serde_json::from_value(json!({"expected_revision": 1})).unwrap();
        let clear: NodeUpdateBody =
            serde_json::from_value(json!({"color": null, "expected_revision": 1})).unwrap();
        let set: NodeUpdateBody =
            serde_json::from_value(json!({"color": 12, "expected_revision": 1})).unwrap();
        assert_eq!(keep.color, None);
        assert_eq!(clear.color, Some(None));
        assert_eq!(set.color, Some(Some(12)));
        assert_eq!(
            serde_json::to_value(keep).unwrap(),
            json!({"expected_revision": 1})
        );
        assert_eq!(
            serde_json::to_value(clear).unwrap(),
            json!({"color": null, "expected_revision": 1})
        );
    }

    #[test]
    fn creation_accepts_a_first_message_and_does_not_offer_to_write_an_instruction_file() {
        let first = json!({"content":[{"type":"text","text":"Follow the place's instructions."}],"client_id":"first"});
        let request: CreateThread = serde_json::from_value(json!({"first_input":first})).unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"first_input":first})
        );
        assert!(
            serde_json::from_value::<CreateThread>(
                json!({"instruction_file":{"text":"Be brief.","expected":"none"}})
            )
            .is_err()
        );
    }

    #[test]
    fn an_audited_act_is_by_a_person_the_system_or_an_agent() {
        let by_system: AuditActor = serde_json::from_value(json!({ "kind": "system" })).unwrap();
        assert_eq!(by_system, AuditActor::System);
        assert_eq!(
            serde_json::to_value(&by_system).unwrap(),
            json!({ "kind": "system" })
        );
        let by_person: AuditActor =
            serde_json::from_value(json!({ "kind": "user", "user": user() })).unwrap();
        assert!(matches!(by_person, AuditActor::User { .. }));
    }
}
