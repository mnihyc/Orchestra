//! Native duplicate requests use source View and the destination's creation rules.
use crate::operations::{FacetSpec as Facet, OperationSpec as Op, TargetResolver as Target};
use orch_access_contract::key_scope::{
    KeyAction::{Manage, Read, Write},
    KeyIdentityKind,
    KeyTargetKind::{Identity, Object},
};
use orch_access_contract::permissions::ObjectKind;
use orch_types::{Id, Revision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An explicit creation place; omission keeps the source's current place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DuplicateDestination {
    /// A real hierarchy place; the native family validates which node kinds it accepts.
    Node { id: Id },
    /// System-owned configuration, subject to the ordinary system creation rule.
    System,
}
/// The result's actual native family, never a caller-supplied permission target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateKind {
    Provider,
    Agent,
    Template,
    McpServer,
    Mount,
    Skill,
    Environment,
    Chat,
}
/// Native subjects for the common destination picker. Host and Machine only support Move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CopySubjectKind {
    Provider,
    Agent,
    Template,
    McpServer,
    Mount,
    Skill,
    Environment,
    Thread,
    Document,
    DocumentContainer,
    Chat,
    Host,
    Machine,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopySubject {
    pub kind: CopySubjectKind,
    pub id: Id,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CopyIntent {
    Duplicate,
    Move,
}
/// Read-only judgement of explicit candidates, never an attempt to create or delete an object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyDestinationsRequest {
    pub subject: CopySubject,
    pub intent: CopyIntent,
    /// One picker page; no implicit enumeration of unreadable organizations.
    pub destinations: Vec<DuplicateDestination>,
}
/// The picker can retain refused candidates and offer the operation's actual next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CopyDestinationCause {
    SourcePolicy,
    ExportPermission,
    DestinationCreation,
    OwnersOnly,
    NotAvailable,
    DeleteRefused,
    RegistrationOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CopyDestinationChoice {
    InOrgMove,
    EnvironmentPlacement,
    McpMachine,
    ForkPlan,
    DocumentTreePreview,
    DeletePreview,
    NativeOperationScope,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CopyDestinationOutcome {
    Allowed,
    Blocked {
        cause: CopyDestinationCause,
        problem: Box<crate::common::Problem>,
    },
    /// Additional native selection or operation admission is required; this grants no creation or deletion.
    NeedsChoice {
        choice: CopyDestinationChoice,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyDestinationJudgement {
    pub destination: DuplicateDestination,
    pub cross_root: bool,
    pub outcome: CopyDestinationOutcome,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyDestinationsResponse {
    pub subject: CopySubject,
    pub intent: CopyIntent,
    /// Current native Owners standing; active manager standing for a group.
    pub source_owned: bool,
    pub destinations: Vec<CopyDestinationJudgement>,
}
/// Matches the destination picker's ordinary one-page bound, keeping native checks bounded.
pub const COPY_DESTINATIONS_MAX: usize = 100;

/// Every supported native business family of the read-only picker observation.
pub const COPY_SUBJECT_TARGETS: &[orch_access_contract::key_scope::KeyTargetKind] = &[
    Object(ObjectKind::Provider),
    Object(ObjectKind::Agent),
    Object(ObjectKind::Template),
    Object(ObjectKind::McpServer),
    Object(ObjectKind::Mount),
    Object(ObjectKind::Skill),
    Object(ObjectKind::Environment),
    Object(ObjectKind::Thread),
    Object(ObjectKind::Document),
    Object(ObjectKind::DocumentContainer),
    Identity(KeyIdentityKind::Chat),
    Object(ObjectKind::Host),
    Object(ObjectKind::Machine),
];
/// Server-owned source configuration is copied; no page, owner list or secret value is input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateBody {
    /// Omit for the native copy name.
    pub name: Option<String>,
    /// Omit to duplicate in the source's current place.
    pub destination: Option<DuplicateDestination>,
    /// When provided, refuse if the source configuration changed after it was read.
    pub expected_revision: Option<Revision>,
}
/// The current native deletion consequences shown before an atomic copy-and-delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoveOutDeleteEffect {
    RemoveSource,
    StopBoundWork,
    EndGroupAndHistory,
    KeepRegistration,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveOutDeletePlan {
    pub subject: CopySubject,
    pub destination: DuplicateDestination,
    pub revision: Revision,
    /// A current footprint acknowledgement, never authority or a reservation.
    pub stamp: String,
    pub effects: Vec<MoveOutDeleteEffect>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveOutConfirmation {
    pub revision: Revision,
    pub stamp: String,
}
/// A single native act. The server repeats every copy and delete check before either change commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveOutBody {
    pub copy: DuplicateBody,
    pub delete_plan: MoveOutConfirmation,
}
/// A native MCP snapshot may name a usable destination custodian explicitly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpDuplicateBody {
    pub name: Option<String>,
    pub destination: Option<DuplicateDestination>,
    pub expected_revision: Option<Revision>,
    /// Omit to keep the original machine when its native Use policy admits the destination.
    pub machine_id: Option<Id>,
}

/// A native environment copy and its reviewed original deletion, accepted as one act.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentMoveOutBody {
    /// The native connected or managed destination, with fresh owned workspace and home.
    pub copy: EnvironmentDuplicateBody,
    /// The current source deletion acknowledgement.
    pub delete_plan: MoveOutConfirmation,
}

/// A duplicate may explicitly choose the destination's managed placement defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDuplicateBody {
    /// Omit for the source's native copy name.
    pub name: Option<String>,
    /// Omit for the source's current home.
    pub destination: Option<DuplicateDestination>,
    /// Refuse a configuration changed since this revision.
    pub expected_revision: Option<Revision>,
    /// An explicit managed choice; omission preserves the original placement class.
    pub managed: Option<EnvironmentDuplicateManaged>,
    /// An explicit usable connected machine; omission keeps an admitted source machine.
    pub machine_id: Option<Id>,
}

/// Only the actual new template and host choices; roots and home are always freshly owned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDuplicateManaged {
    /// Omit to resolve the destination's native template default.
    pub template_id: Option<Id>,
    /// Omit to use its native host selector.
    pub host_id: Option<Id>,
}
impl From<DuplicateBody> for McpDuplicateBody {
    fn from(value: DuplicateBody) -> Self {
        Self {
            name: value.name,
            destination: value.destination,
            expected_revision: value.expected_revision,
            machine_id: None,
        }
    }
}
impl From<McpDuplicateBody> for DuplicateBody {
    fn from(value: McpDuplicateBody) -> Self {
        Self {
            name: value.name,
            destination: value.destination,
            expected_revision: value.expected_revision,
        }
    }
}
/// Conversation copy options and the existing explicit contributor-notification choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatDuplicateBody {
    /// Native name, destination and optional source revision; omitted for the source's current place.
    #[serde(default)]
    pub copy: DuplicateBody,
    /// Tell eligible original contributors when explicitly selected, with no notice to the copier.
    pub notify: Option<bool>,
    /// The native contributor set returned by the preview, required when notifying a nonempty audience.
    pub notify_preview_id: Option<Id>,
}
/// Current eligible contributor count for the existing Notify N people choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatDuplicateAudience {
    /// Distinct eligible original contributors, excluding the copier.
    pub people: u64,
    /// The existing synchronous explicit-notice batch limit.
    pub notify_limit: u32,
    /// The native reviewed recipient set; absent for no recipients or an audience above the synchronous limit.
    pub preview_id: Option<Id>,
}
/// The existing fork plan and the original native deletion reviewed in the same preflight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadMoveOutPreviewResponse {
    /// Full native fork details, or their existing write-only plan receipt.
    pub plan: crate::forks::ForkPlanResponse,
    /// Present only when that exact source's response Read is admitted.
    pub delete_plan: Option<MoveOutDeletePlan>,
    /// Current eligible original contributors; absent on the write-only plan receipt.
    pub notification: Option<ChatDuplicateAudience>,
    /// Open side conversations the native fork does not carry; close or keep each before confirming.
    /// Absent on the write-only plan receipt.
    pub open_side_threads: Option<Vec<crate::nodes::NodeRef>>,
}
/// Accepted native creation identity, without existing configuration or credential content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DuplicateReceipt {
    /// Restricted response marker; no current resource View is implied.
    pub projection: crate::common::ReceiptProjection,
    /// The actual native created kind.
    pub kind: DuplicateKind,
    /// The actual new identity.
    pub id: Id,
}
/// The native MCP copy choice with its current original deletion acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpMoveOutBody {
    pub copy: McpDuplicateBody,
    pub delete_plan: MoveOutConfirmation,
}
/// One owner-manager act with the reviewed original member notice choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMoveOutBody {
    pub copy: DuplicateBody,
    pub delete_plan: MoveOutConfirmation,
    pub notify: Option<bool>,
    pub notify_preview_id: Option<Id>,
}
/// Irreversible native ending consequences and the current eligible original members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMoveOutPlan {
    pub delete_plan: MoveOutDeletePlan,
    pub notification: ChatDuplicateAudience,
}
const CHAT_MOVE_FACETS: [Facet; 12] = [
    Facet::required(
        Manage,
        &[Identity(KeyIdentityKind::Chat)],
        Target::TypedPath {
            parameter: "id",
            kind: Identity(KeyIdentityKind::Chat),
        },
    ),
    Facet::required(
        Write,
        &[Identity(KeyIdentityKind::Chat)],
        Target::Resolved("duplicate_destination"),
    ),
    Facet::required(
        Read,
        &[Identity(KeyIdentityKind::Chat)],
        Target::ResolvedCollection("message_source_chats"),
    ),
    Facet::required(
        Read,
        &[Object(ObjectKind::Thread)],
        Target::ResolvedCollection("message_source_threads"),
    ),
    Facet::required(
        Read,
        &[Object(ObjectKind::Document)],
        Target::ResolvedCollection("message_source_documents"),
    ),
    Facet::required(
        Read,
        &[Object(ObjectKind::Environment)],
        Target::ResolvedCollection("message_source_environments"),
    ),
    Facet::required(
        Read,
        &[Identity(KeyIdentityKind::Upload)],
        Target::ResolvedCollection("message_retained_uploads"),
    ),
    Facet::required(
        Read,
        &[Object(ObjectKind::Org)],
        Target::ResolvedCollection("message_name_orgs"),
    ),
    Facet::required(
        Read,
        &[Object(ObjectKind::Project)],
        Target::ResolvedCollection("message_name_projects"),
    ),
    Facet::required(
        Read,
        &[Object(ObjectKind::Topic)],
        Target::ResolvedCollection("message_name_topics"),
    ),
    Facet::required(
        Read,
        &[Object(ObjectKind::Thread)],
        Target::ResolvedCollection("message_name_threads"),
    ),
    Facet::required(
        Read,
        &[Identity(KeyIdentityKind::Group)],
        Target::ResolvedCollection("message_name_groups"),
    ),
];
const CHAT_MOVE_PREVIEW_FACETS: [Facet; 13] = [
    CHAT_MOVE_FACETS[0],
    CHAT_MOVE_FACETS[1],
    CHAT_MOVE_FACETS[2],
    CHAT_MOVE_FACETS[3],
    CHAT_MOVE_FACETS[4],
    CHAT_MOVE_FACETS[5],
    CHAT_MOVE_FACETS[6],
    CHAT_MOVE_FACETS[7],
    CHAT_MOVE_FACETS[8],
    CHAT_MOVE_FACETS[9],
    CHAT_MOVE_FACETS[10],
    CHAT_MOVE_FACETS[11],
    Facet::response_content(
        &[Identity(KeyIdentityKind::Chat)],
        Target::TypedPath {
            parameter: "id",
            kind: Identity(KeyIdentityKind::Chat),
        },
    ),
];
crate::endpoints! {
    /// Reviews an active manager's copy and irreversible ending of this group and history.
    ChatMoveOutPreview: POST "/chats/{id}/move-out/preview", crate::routes::Access::Rule("active source manager, native copied-source Export and destination opening"), none, json DuplicateBody, json ChatMoveOutPlan, 200,
        operation Op::facets(&CHAT_MOVE_PREVIEW_FACETS);
    /// Copies a group and ends it here in one act, with fresh creator-only membership.
    ChatMoveOut: POST "/chats/{id}/move-out", crate::routes::Access::Rule("active manager, confirmed original ending, native source Export and destination opening"), none, json ChatMoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&CHAT_MOVE_FACETS));

    /// Reviews the original native MCP definition and every copied-file deletion consequence.
    McpServerMoveOutPreview: POST "/mcp-servers/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, native copy destination and complete native source Delete"), none, json McpDuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::McpServer)],Target::Resolved("duplicate_subject")),
            Facet::required(Write,&[Object(ObjectKind::McpServer)],Target::Resolved("duplicate_destination")),
            Facet::required(Manage,&[Object(ObjectKind::Environment)],Target::ResolvedCollection("duplicate_destination_environments")),
            Facet::required(Write,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("duplicate_native_mcp_machine")),
            Facet::required(Manage,&[Object(ObjectKind::Environment)],Target::ResolvedCollection("duplicate_source_environments")),
            Facet::required(Manage,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("move_deleted_copies")),
            Facet::response_content(&[Object(ObjectKind::McpServer)],Target::Resolved("duplicate_subject")),
        ]);
    /// Copies the native configuration and ends its original definition in the same accepted act.
    McpServerMoveOut: POST "/mcp-servers/{id}/move-out", crate::routes::Access::Rule("actual owner, confirmed complete native source Delete and native copy destination"), none, json McpMoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::McpServer)],Target::Resolved("duplicate_subject")),
            Facet::required(Write,&[Object(ObjectKind::McpServer)],Target::Resolved("duplicate_destination")),
            Facet::required(Manage,&[Object(ObjectKind::Environment)],Target::ResolvedCollection("duplicate_destination_environments")),
            Facet::required(Write,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("duplicate_native_mcp_machine")),
            Facet::required(Manage,&[Object(ObjectKind::Environment)],Target::ResolvedCollection("duplicate_source_environments")),
            Facet::required(Manage,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("move_deleted_copies")),
        ]));

    /// Reviews an owner-only registration-preserving move to another root organization.
    HostMoveOutPreview: POST "/hosts/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, native destination creation and empty native deletion inventory"), none, json DuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Host)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Host)}),
            Facet::required(Write,&[Object(ObjectKind::Host)],Target::Resolved("registration_move_destination")),
            Facet::response_content(&[Object(ObjectKind::Host)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Host)}),
        ]);
    /// Keeps the physical registration while resetting ownership, entries and tags at the new home.
    HostMoveOut: POST "/hosts/{id}/move-out", crate::routes::Access::Rule("actual owner, confirmed empty native deletion inventory and native destination creation"), none, json MoveOutBody, json crate::resource_groups::ResourceHomeSetReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Host)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Host)}),
            Facet::required(Write,&[Object(ObjectKind::Host)],Target::Resolved("registration_move_destination")),
        ]));
    /// Reviews an owner-only registration-preserving move to another root organization.
    MachineMoveOutPreview: POST "/machines/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, native destination creation and empty native deletion inventory"), none, json DuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Machine)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Machine)}),
            Facet::required(Write,&[Object(ObjectKind::Machine)],Target::Resolved("registration_move_destination")),
            Facet::response_content(&[Object(ObjectKind::Machine)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Machine)}),
        ]);
    /// Keeps the physical registration while resetting ownership, entries and tags at the new home.
    MachineMoveOut: POST "/machines/{id}/move-out", crate::routes::Access::Rule("actual owner, confirmed empty native deletion inventory and native destination creation"), none, json MoveOutBody, json crate::resource_groups::ResourceHomeSetReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Machine)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Machine)}),
            Facet::required(Write,&[Object(ObjectKind::Machine)],Target::Resolved("registration_move_destination")),
        ]));
    /// Reviews an owner's native profile copy and source stop/deletion consequences.
    AgentMoveOutPreview: POST "/agents/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, native creation/physical profile Use and native Delete"), none, json DuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Agent)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Agent)}),
            Facet::required(Write,&[Object(ObjectKind::Agent)],Target::Resolved("duplicate_destination")),
            Facet::required(Write,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("duplicate_profile_machine")),
            Facet::response_content(&[Object(ObjectKind::Agent)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Agent)}),
        ]);
    /// Creates the new native profile and atomically accepts the original profile's stop and deletion.
    AgentMoveOut: POST "/agents/{id}/move-out", crate::routes::Access::Rule("actual owner, confirmed native deletion/stop consequences and native creation/physical profile rules"), none, json MoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Agent)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Agent)}),
            Facet::required(Write,&[Object(ObjectKind::Agent)],Target::Resolved("duplicate_destination")),
            Facet::required(Write,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("duplicate_profile_machine")),
        ]));
    /// Reviews the current published skill and source deletion before an owner moves it.
    SkillMoveOutPreview: POST "/skills/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, current native skill source and destination creation rules, and native Delete"), none, json DuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Skill)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Skill)}),
            Facet::required(Write,&[Object(ObjectKind::Skill)],Target::Resolved("duplicate_destination")),
            Facet::response_content(&[Object(ObjectKind::Skill)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Skill)}),
        ]);
    /// Copies a published skill through its existing byte/version supplier and deletes its source atomically.
    SkillMoveOut: POST "/skills/{id}/move-out", crate::routes::Access::Rule("actual owner, confirmed current deletion, native source and destination creation rules"), none, json MoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Skill)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Skill)}),
            Facet::required(Write,&[Object(ObjectKind::Skill)],Target::Resolved("duplicate_destination")),
        ]));
    /// Reviews a current unpublished native draft before an owner moves it.
    SkillDraftMoveOutPreview: POST "/skills/{id}/draft/move-out/preview", crate::routes::Access::Rule("actual owner and native draft Manage, destination creation and Delete"), none, json DuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Skill)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Skill)}),
            Facet::required(Write,&[Object(ObjectKind::Skill)],Target::Resolved("duplicate_destination")),
            Facet::response_content(&[Object(ObjectKind::Skill)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Skill)}),
        ]);
    /// Copies an unpublished skill through its native draft supplier and deletes the source in one act.
    SkillDraftMoveOut: POST "/skills/{id}/draft/move-out", crate::routes::Access::Rule("actual owner, confirmed current draft and deletion, native destination creation rules"), none, json MoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Skill)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Skill)}),
            Facet::required(Write,&[Object(ObjectKind::Skill)],Target::Resolved("duplicate_destination")),
        ]));
    /// Reads the native uses, physical-location and deletion plan of an owner's cross-root mount move.
    MountMoveOutPreview: POST "/mounts/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, native source/destination rules and every native mount Delete and location management prerequisite"), none, json DuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Mount)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Mount)}),
            Facet::required(Write,&[Object(ObjectKind::Mount)],Target::Resolved("duplicate_destination")),
            Facet::required(Manage,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("move_source_machines")),
            Facet::response_content(&[Object(ObjectKind::Mount)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Mount)}),
        ]);
    /// Atomically copies native mount configuration and deletes the source with its original guards.
    MountMoveOut: POST "/mounts/{id}/move-out", crate::routes::Access::Rule("actual owner, confirmed current delete plan, native creation and every mount Delete and location management prerequisite"), none, json MoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Mount)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Mount)}),
            Facet::required(Write,&[Object(ObjectKind::Mount)],Target::Resolved("duplicate_destination")),
            Facet::required(Manage,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("move_source_machines")),
        ]));
    /// Reads the native creation and deletion plan for an owner's cross-root template move.
    TemplateMoveOutPreview: POST "/templates/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, native source and destination creation rules, and every native template Delete prerequisite"), none, json DuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Template)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Template)}),
            Facet::required(Write,&[Object(ObjectKind::Template)],Target::Resolved("duplicate_destination")),
            Facet::required(Manage,&[Object(ObjectKind::Template)],Target::ResolvedCollection("duplicate_driver_administration")),
            Facet::response_content(&[Object(ObjectKind::Template)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Template)}),
        ]);
    /// Atomically creates an independent native template at its destination and deletes the source.
    TemplateMoveOut: POST "/templates/{id}/move-out", crate::routes::Access::Rule("actual owner, current confirmed delete plan, native source and destination creation rules, and every native template Delete prerequisite"), none, json MoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Template)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Template)}),
            Facet::required(Write,&[Object(ObjectKind::Template)],Target::Resolved("duplicate_destination")),
            Facet::required(Manage,&[Object(ObjectKind::Template)],Target::ResolvedCollection("duplicate_driver_administration")),
        ]));
    /// Judges explicit copy and move destinations through current native source and creation rules without effects.
    CopyDestinationsJudge: POST "/copy-destinations/judge", crate::routes::Access::Rule("read the actual source and explicit destination observations; this reader grants no creation or deletion authority"), none, json CopyDestinationsRequest, json CopyDestinationsResponse, 200,
        operation Op::returning(Read, COPY_SUBJECT_TARGETS, Target::ResolvedCollection("copy_destination_sources"));
    /// Copies a readable Environment workspace into a fresh native environment of the chosen placement class.
    EnvironmentDuplicate: POST "/environments/{id}/duplicate", crate::routes::Access::Rule("environment.view and files.view on the source, native environment creation at the destination and actual template, host and mount Use"), none, json EnvironmentDuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::variants(&[
            crate::operations::VariantSpec {
                name: "managed",
                selector: "a managed source with no machine choice, or an explicit managed destination",
                facets: &crate::environment_sources::duplicating::MANAGED_FACETS,
            },
            crate::operations::VariantSpec {
                name: "connected",
                selector: "a connected source with no managed choice, or an explicit connected machine",
                facets: &crate::environment_sources::duplicating::CONNECTED_FACETS,
            },
        ]));
    /// Plans a native full conversation fork and its original owner-only deletion as one act.
    ThreadMoveOutPreview: POST "/threads/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, source fork and Export, real destination creation and every original Thread Delete prerequisite"), none, json crate::forks::ForkRequest, json ThreadMoveOutPreviewResponse, 200,
        operation Op::receipt(&Op::facets(crate::fork_sources::MOVE_FACETS));
    /// Confirms the same private native Move preflight; the original ends only in its successful fork commit.
    ThreadMoveOut: POST "/threads/{id}/move-out", crate::routes::Access::Rule("actual owner, confirmed native Move preflight, all original fork/create and Delete checks"), none, json crate::forks::ForkCreateBody, json crate::forks::ForkCreateResponse, 202,
        operation Op::receipt(&Op::facets(crate::fork_sources::MOVE_FACETS));
    /// Reviews the original native deletion before independently copying an environment.
    EnvironmentMoveOutPreview: POST "/environments/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, native workspace byte Read and destination creation, and every original Environment Delete prerequisite"), none, json EnvironmentDuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::variants(&[
            crate::operations::VariantSpec{name:"managed",selector:"a native managed destination",facets:&crate::environment_sources::duplicating::MOVE_MANAGED_PREVIEW_FACETS},
            crate::operations::VariantSpec{name:"connected",selector:"a native connected directory destination",facets:&crate::environment_sources::duplicating::MOVE_CONNECTED_PREVIEW_FACETS},
        ]);
    /// Copies through the original provision and ends the original only at successful native commit.
    EnvironmentMoveOut: POST "/environments/{id}/move-out", crate::routes::Access::Rule("actual owner, confirmed native deletion, byte Read and destination creation, with every original Delete check"), none, json EnvironmentMoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::variants(&[
            crate::operations::VariantSpec{name:"managed",selector:"a native managed destination",facets:&crate::environment_sources::duplicating::MOVE_MANAGED_FACETS},
            crate::operations::VariantSpec{name:"connected",selector:"a native connected directory destination",facets:&crate::environment_sources::duplicating::MOVE_CONNECTED_FACETS},
        ]));
    /// Duplicates readable provider configuration through its native creator, with independent credentials and runtime.
    ProviderDuplicate: POST "/providers/{id}/duplicate", crate::routes::Access::Rule("provider.view on the source and the native provider creation rule at the destination; secrets only for an actual owner"), none, json DuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Read,&[Object(ObjectKind::Provider)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Provider)}),
            Facet::required(Write,&[Object(ObjectKind::Provider)],Target::Resolved("duplicate_destination")),
        ]));

    /// Reviews the native source stop before moving a provider to another organization.
    ProviderMoveOutPreview: POST "/providers/{id}/move-out/preview", crate::routes::Access::Rule("actual owner, native provider creation at the destination, and every original provider Delete prerequisite"), none, json DuplicateBody, json MoveOutDeletePlan, 200,
        operation Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Provider)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Provider)}),
            Facet::required(Write,&[Object(ObjectKind::Provider)],Target::Resolved("duplicate_destination")),
            Facet::response_content(&[Object(ObjectKind::Provider)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Provider)}),
        ]);
    /// Prepares the copy through its native initial Make and ends the source only at successful commit.
    ProviderMoveOut: POST "/providers/{id}/move-out", crate::routes::Access::Rule("actual owner, current confirmed Delete plan, native destination creation, and every original provider Delete prerequisite"), none, json MoveOutBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Provider)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Provider)}),
            Facet::required(Write,&[Object(ObjectKind::Provider)],Target::Resolved("duplicate_destination")),
        ]));

    /// Duplicates a mount's readable configuration under the source copy-out policy, with fresh destination privileges.
    MountDuplicate: POST "/mounts/{id}/duplicate", crate::routes::Access::Rule("mount.view on the source and the ordinary mount creation rule at the destination"), none, json DuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Read,&[Object(ObjectKind::Mount)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Mount)}),
            Facet::required(Write,&[Object(ObjectKind::Mount)],Target::Resolved("duplicate_destination")),
        ]));
    /// Duplicates a readable native template with current driver validation and fresh destination defaults.
    TemplateDuplicate: POST "/templates/{id}/duplicate", crate::routes::Access::Rule("template.view on the source and the ordinary template creation and driver administration rules at the destination"), none, json DuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Read,&[Object(ObjectKind::Template)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Template)}),
            Facet::required(Write,&[Object(ObjectKind::Template)],Target::Resolved("duplicate_destination")),
            Facet::required(Manage,&[Object(ObjectKind::Template)],Target::ResolvedCollection("duplicate_driver_administration")),
        ]));

    /// Duplicates an agent profile's readable configuration using the native destination creation and machine rules.
    AgentDuplicate: POST "/agents/{id}/duplicate", crate::routes::Access::Rule("agent.view on the source, native creation at the destination and use of a retained physical harness machine"), none, json DuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Read,&[Object(ObjectKind::Agent)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Agent)}),
            Facet::required(Write,&[Object(ObjectKind::Agent)],Target::Resolved("duplicate_destination")),
            Facet::required(Write,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("duplicate_profile_machine")),
        ]));

    /// Duplicates a readable native MCP definition, carrying declared secrets only for an actual owner.
    McpServerDuplicate: POST "/mcp-servers/{id}/duplicate", crate::routes::Access::Rule("mcp.view on the source and native preset or environment-parent creation at the destination; secret values only for an actual owner"), none, json McpDuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Read,&[Object(ObjectKind::McpServer)],Target::Resolved("duplicate_subject")),
            Facet::required(Write,&[Object(ObjectKind::McpServer)],Target::Resolved("duplicate_destination")),
            Facet::required(Manage,&[Object(ObjectKind::Environment)],Target::ResolvedCollection("duplicate_destination_environments")),
            Facet::required(Write,&[Object(ObjectKind::Machine)],Target::ResolvedCollection("duplicate_native_mcp_machine")),
        ]));

    /// Duplicates readable current skill files through the native version or draft writer, with fresh destination privileges.
    SkillDuplicate: POST "/skills/{id}/duplicate", crate::routes::Access::Rule("skill.view on the source and the ordinary skill creation rule at the destination"), none, json DuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Read,&[Object(ObjectKind::Skill)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Skill)}),
            Facet::required(Write,&[Object(ObjectKind::Skill)],Target::Resolved("duplicate_destination")),
        ]));

    /// Duplicates the current unpublished skill draft under its existing native draft-reading rule.
    SkillDraftDuplicate: POST "/skills/{id}/draft/duplicate", crate::routes::Access::Rule("skill.manage to read the unpublished source draft and the native skill creation rule at the destination"), none, json DuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(ObjectKind::Skill)],Target::TypedPath{parameter:"id",kind:Object(ObjectKind::Skill)}),
            Facet::required(Write,&[Object(ObjectKind::Skill)],Target::Resolved("duplicate_destination")),
        ]));

    /// Duplicates a readable group conversation with a fresh native group and a creator-only roster.
    ChatDuplicate: POST "/chats/{id}/duplicate", crate::routes::Access::Rule("read the source group chat and satisfy native group creation at the destination"), none, json ChatDuplicateBody, json DuplicateReceipt, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Read,&[Identity(KeyIdentityKind::Chat)],Target::TypedPath{parameter:"id",kind:Identity(KeyIdentityKind::Chat)}),
            Facet::required(Write,&[Identity(KeyIdentityKind::Chat)],Target::Resolved("duplicate_destination")),
            Facet::required(Read,&[Identity(KeyIdentityKind::Chat)],Target::ResolvedCollection("message_source_chats")),
            Facet::required(Read,&[Object(ObjectKind::Thread)],Target::ResolvedCollection("message_source_threads")),
            Facet::required(Read,&[Object(ObjectKind::Document)],Target::ResolvedCollection("message_source_documents")),
            Facet::required(Read,&[Object(ObjectKind::Environment)],Target::ResolvedCollection("message_source_environments")),
            Facet::required(Read,&[Identity(KeyIdentityKind::Upload)],Target::ResolvedCollection("message_retained_uploads")),
            Facet::required(Read,&[Object(ObjectKind::Org)],Target::ResolvedCollection("message_name_orgs")),
            Facet::required(Read,&[Object(ObjectKind::Project)],Target::ResolvedCollection("message_name_projects")),
            Facet::required(Read,&[Object(ObjectKind::Topic)],Target::ResolvedCollection("message_name_topics")),
            Facet::required(Read,&[Object(ObjectKind::Thread)],Target::ResolvedCollection("message_name_threads")),
            Facet::required(Read,&[Identity(KeyIdentityKind::Group)],Target::ResolvedCollection("message_name_groups")),
        ]));

    /// Counts the original contributors eligible for the existing optional explicit notice.
    ChatDuplicatePreview: POST "/chats/{id}/duplicate/preview", crate::routes::Access::Rule("read the source group chat"), none, none, json ChatDuplicateAudience, 200,
        operation Op::returning(Read,&[Identity(KeyIdentityKind::Chat)],Target::TypedPath{parameter:"id",kind:Identity(KeyIdentityKind::Chat)});

}
