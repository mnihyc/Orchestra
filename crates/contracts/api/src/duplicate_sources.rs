//! The native duplicate binds its real source and actual destination independently.
use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver as Target},
};
use orch_access_contract::{
    key_scope::{
        KeyIdentityKind,
        KeyTargetKind::{Identity, Object},
    },
    permissions::ObjectKind,
};
/// Factual duplicate operands selected by the native family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Exact online native copied definitions ended by a Move.
    ForgottenCopies,
    /// Actual environment whose own definition is ended; absent for a flat source.
    SourceEnvironments,

    /// Actual source and the real explicit destination namespaces of the read-only picker judgement.
    DestinationJudgement,
    /// The identical native observations published by the judgement's own current Read.
    DestinationJudgementContent,
    /// Actual stored source configuration, with its current native placement.
    Subject,
    /// The same actual native subject whose preview is being published.
    PreviewContent,
    /// Actual selected native creation place, never an asserted ancestor.
    Destination,
    /// System administration required by an actual restricted native template driver.
    DriverAdministration,
    /// Actual physical machine written by a native profile registration or stored MCP copy.
    Machine,
    /// A new environment-owned server uses its real parent creation gate.
    DestinationEnvironments,
    /// Real retained media witnesses selected from the copied conversation.
    BlobChats,
    /// Current thread byte sources.
    BlobThreads,
    /// Current document byte sources.
    BlobDocuments,
    /// Current environment byte sources.
    BlobEnvironments,
    /// Caller-retained upload custody.
    BlobUploads,
    /// Materialized protected organization labels.
    NameOrgs,
    /// Materialized protected project labels.
    NameProjects,
    /// Materialized protected topic labels.
    NameTopics,
    /// Materialized protected thread labels.
    NameThreads,
    /// Materialized protected group labels.
    NameGroups,
}
pub const COPY_DESTINATIONS: Declaration<Source> = Declaration {
    operation: "CopyDestinationsJudge",
    bindings: &[
        Binding {
            source: Source::DestinationJudgement,
            target: Target::ResolvedCollection("copy_destination_sources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::DestinationJudgementContent,
            target: Target::ResolvedCollection("copy_destination_sources"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
/// Source configuration and the actual creation place stay independent in a provider's initial Make.
pub const PROVIDER: Declaration<Source> = Declaration {
    operation: "ProviderDuplicate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Provider),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("duplicate_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};

/// The native provider creator's moving case retains its real source and destination.
pub const PROVIDER_MOVE: Declaration<Source> = Declaration {
    operation: "ProviderMoveOut",
    bindings: PROVIDER.bindings,
};
pub const PROVIDER_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "ProviderMoveOutPreview",
    bindings: &[
        PROVIDER.bindings[0],
        PROVIDER.bindings[1],
        Binding {
            source: Source::PreviewContent,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Provider),
            },
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// The same mount validation/writer serves ordinary creation and duplication.
pub const MOUNT: Declaration<Source> = Declaration {
    operation: "MountDuplicate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Mount),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("duplicate_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};
pub const MOUNT_MOVE: Declaration<Source> = Declaration {
    operation: "MountMoveOut",
    bindings: &[
        MOUNT.bindings[0],
        MOUNT.bindings[1],
        Binding {
            source: Source::Machine,
            target: Target::ResolvedCollection("move_source_machines"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
pub const MOUNT_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "MountMoveOutPreview",
    bindings: &[
        MOUNT_MOVE.bindings[0],
        MOUNT_MOVE.bindings[1],
        MOUNT_MOVE.bindings[2],
        Binding {
            source: Source::PreviewContent,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Mount),
            },
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// A native recipe's actual driver administration is indivisible, including an unrestricted empty set.
pub const TEMPLATE: Declaration<Source> = Declaration {
    operation: "TemplateDuplicate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Template),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("duplicate_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::DriverAdministration,
            target: Target::ResolvedCollection("duplicate_driver_administration"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
pub const TEMPLATE_MOVE: Declaration<Source> = Declaration {
    operation: "TemplateMoveOut",
    bindings: TEMPLATE.bindings,
};
pub const TEMPLATE_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "TemplateMoveOutPreview",
    bindings: &[
        TEMPLATE.bindings[0],
        TEMPLATE.bindings[1],
        TEMPLATE.bindings[2],
        Binding {
            source: Source::PreviewContent,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Template),
            },
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// A physical profile keeps its actual native machine registration obligation.
pub const AGENT: Declaration<Source> = Declaration {
    operation: "AgentDuplicate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Agent),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("duplicate_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Machine,
            target: Target::ResolvedCollection("duplicate_profile_machine"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
pub const AGENT_MOVE: Declaration<Source> = Declaration {
    operation: "AgentMoveOut",
    bindings: AGENT.bindings,
};
pub const AGENT_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "AgentMoveOutPreview",
    bindings: &[
        AGENT.bindings[0],
        AGENT.bindings[1],
        AGENT.bindings[2],
        Binding {
            source: Source::PreviewContent,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Agent),
            },
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// Native MCP preset sources, independent of a readback or secret value.
pub const MCP: Declaration<Source> = Declaration {
    operation: "McpServerDuplicate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::Resolved("duplicate_subject"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("duplicate_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::DestinationEnvironments,
            target: Target::ResolvedCollection("duplicate_destination_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Machine,
            target: Target::ResolvedCollection("duplicate_native_mcp_machine"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native skill bytes are authorized once, with their exact source kept through the final recording.
pub const SKILL: Declaration<Source> = Declaration {
    operation: "SkillDuplicate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Skill),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("duplicate_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};
pub const SKILL_MOVE: Declaration<Source> = Declaration {
    operation: "SkillMoveOut",
    bindings: SKILL.bindings,
};
pub const SKILL_DRAFT_MOVE: Declaration<Source> = Declaration {
    operation: "SkillDraftMoveOut",
    bindings: SKILL.bindings,
};
const SKILL_MOVING_PREVIEW: &[Binding<Source>] = &[
    SKILL.bindings[0],
    SKILL.bindings[1],
    Binding {
        source: Source::PreviewContent,
        target: Target::TypedPath {
            parameter: "id",
            kind: Object(ObjectKind::Skill),
        },
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
];
pub const SKILL_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "SkillMoveOutPreview",
    bindings: SKILL_MOVING_PREVIEW,
};
pub const SKILL_DRAFT_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "SkillDraftMoveOutPreview",
    bindings: SKILL_MOVING_PREVIEW,
};

/// Draft bytes keep their existing Manage door; no current published-version Read can borrow it.
pub const SKILL_DRAFT: Declaration<Source> = Declaration {
    operation: "SkillDraftDuplicate",
    bindings: SKILL.bindings,
};

/// Current message media and label sources use the same native forwarding roles.
pub const CHAT: Declaration<Source> = Declaration {
    operation: "ChatDuplicate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Identity(KeyIdentityKind::Chat),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("duplicate_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::BlobChats,
            target: Target::ResolvedCollection("message_source_chats"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::BlobThreads,
            target: Target::ResolvedCollection("message_source_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::BlobDocuments,
            target: Target::ResolvedCollection("message_source_documents"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::BlobEnvironments,
            target: Target::ResolvedCollection("message_source_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::BlobUploads,
            target: Target::ResolvedCollection("message_retained_uploads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameOrgs,
            target: Target::ResolvedCollection("message_name_orgs"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameProjects,
            target: Target::ResolvedCollection("message_name_projects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameTopics,
            target: Target::ResolvedCollection("message_name_topics"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameThreads,
            target: Target::ResolvedCollection("message_name_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::NameGroups,
            target: Target::ResolvedCollection("message_name_groups"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The audience preview selects only the current real group; it creates no destination.
pub const CHAT_PREVIEW: Declaration<Source> = Declaration {
    operation: "ChatDuplicatePreview",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Identity(KeyIdentityKind::Chat),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::PreviewContent,
            target: Target::TypedPath {
                parameter: "id",
                kind: Identity(KeyIdentityKind::Chat),
            },
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// The native host registration and its actual new creation home.
pub const HOST_MOVE: Declaration<Source> = Declaration {
    operation: "HostMoveOut",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Host),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("registration_move_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};
pub const HOST_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "HostMoveOutPreview",
    bindings: &[
        HOST_MOVE.bindings[0],
        HOST_MOVE.bindings[1],
        Binding {
            source: Source::PreviewContent,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Host),
            },
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// The native machine registration and its actual new creation home.
pub const MACHINE_MOVE: Declaration<Source> = Declaration {
    operation: "MachineMoveOut",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Machine),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Destination,
            target: Target::Resolved("registration_move_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};
pub const MACHINE_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "MachineMoveOutPreview",
    bindings: &[
        MACHINE_MOVE.bindings[0],
        MACHINE_MOVE.bindings[1],
        Binding {
            source: Source::PreviewContent,
            target: Target::TypedPath {
                parameter: "id",
                kind: Object(ObjectKind::Machine),
            },
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// Native source creation and original deletion belong to the same MCP move.
pub const MCP_MOVE: Declaration<Source> = Declaration {
    operation: "McpServerMoveOut",
    bindings: &[
        MCP.bindings[0],
        MCP.bindings[1],
        MCP.bindings[2],
        MCP.bindings[3],
        Binding {
            source: Source::SourceEnvironments,
            target: Target::ResolvedCollection("duplicate_source_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ForgottenCopies,
            target: Target::ResolvedCollection("move_deleted_copies"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
pub const MCP_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "McpServerMoveOutPreview",
    bindings: &[
        MCP_MOVE.bindings[0],
        MCP_MOVE.bindings[1],
        MCP_MOVE.bindings[2],
        MCP_MOVE.bindings[3],
        MCP_MOVE.bindings[4],
        MCP_MOVE.bindings[5],
        Binding {
            source: Source::PreviewContent,
            target: Target::Resolved("duplicate_subject"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// Actual native group Move and all copied media/name sources.
pub const CHAT_MOVE: Declaration<Source> = Declaration {
    operation: "ChatMoveOut",
    bindings: CHAT.bindings,
};
pub const CHAT_MOVE_PREVIEW: Declaration<Source> = Declaration {
    operation: "ChatMoveOutPreview",
    bindings: &[
        CHAT.bindings[0],
        CHAT.bindings[1],
        CHAT.bindings[2],
        CHAT.bindings[3],
        CHAT.bindings[4],
        CHAT.bindings[5],
        CHAT.bindings[6],
        CHAT.bindings[7],
        CHAT.bindings[8],
        CHAT.bindings[9],
        CHAT.bindings[10],
        CHAT.bindings[11],
        Binding {
            source: Source::PreviewContent,
            target: Target::TypedPath {
                parameter: "id",
                kind: Identity(KeyIdentityKind::Chat),
            },
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};
