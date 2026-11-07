//! Native Environment collection sources shared with the resource read adapter.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::FacetUsage;
use crate::operations::{FacetSpec, TargetResolver};
use crate::resource_reads::{Family, NameRoles, SourceRole};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_access_contract::permissions::ResourceKind;

/// The ordinary Environment list's selected content and protected context names.
/// The Get operation keeps its separate retained and through-thread reading shapes.
pub const LIST: Family = Family {
    kind: ResourceKind::Environment,
    sources: &[
        SourceRole {
            role: "environment_templates",
            kind: ResourceKind::Template,
        },
        SourceRole {
            role: "environment_hosts",
            kind: ResourceKind::Host,
        },
        SourceRole {
            role: "environment_machines",
            kind: ResourceKind::Machine,
        },
        SourceRole {
            role: "environment_mounts",
            kind: ResourceKind::Mount,
        },
    ],
    names: NameRoles {
        groups: Some("environment_groups"),
        nodes: [
            "environment_names_orgs",
            "environment_names_projects",
            "environment_names_topics",
            "environment_names_threads",
        ],
        containers: None,
    },
};

/// One native list row, expanded into the consuming operation's canonical facets.
pub const LIST_FACETS: [FacetSpec; 10] = LIST.facets(TargetResolver::ResolvedCollection("results"));

/// Stored subjects selected by the existing kept-environment page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedSource {
    /// Actual retained Environment identities under their native restoration authority.
    Environments,
    /// Host context emitted by the native retained view.
    Hosts,
    /// Actual home labels admitted for this operation.
    HomeOrgs,
    /// Actual project home labels.
    HomeProjects,
    /// Actual topic home labels.
    HomeTopics,
    /// Actual thread home labels.
    HomeThreads,
}

/// Source declaration for the native retained page, including honest empty collections.
pub const DELETED_LIST: Declaration<RetainedSource> = Declaration {
    operation: "NodeDeletedEnvironmentList",
    bindings: &[
        Binding {
            source: RetainedSource::Environments,
            target: TargetResolver::ResolvedCollection("results"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RetainedSource::Hosts,
            target: TargetResolver::ResolvedCollection("deleted_environment_hosts"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RetainedSource::HomeOrgs,
            target: TargetResolver::ResolvedCollection("deleted_environment_names_orgs"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RetainedSource::HomeProjects,
            target: TargetResolver::ResolvedCollection("deleted_environment_names_projects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RetainedSource::HomeTopics,
            target: TargetResolver::ResolvedCollection("deleted_environment_names_topics"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RetainedSource::HomeThreads,
            target: TargetResolver::ResolvedCollection("deleted_environment_names_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The retained page is a privileged inspection, so its primary action stays Manage.
pub const DELETED_LIST_FACETS: [FacetSpec; 6] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("results"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Host)],
        TargetResolver::ResolvedCollection("deleted_environment_hosts"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("deleted_environment_names_orgs"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("deleted_environment_names_projects"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("deleted_environment_names_topics"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("deleted_environment_names_threads"),
    ),
];

/// The stored trust subject and optional native Environment response sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustSource {
    /// The existing live Environment, shared by the action and its optional Read.
    Environment,
    /// Actual native Template, Host, Machine and Mount content in the response.
    Foreign,
    /// Protected group names selected by the native Environment projection.
    Groups,
    /// Protected org labels.
    Orgs,
    /// Protected project labels.
    Projects,
    /// Protected topic labels.
    Topics,
    /// Protected thread labels.
    Threads,
}

/// One stored-subject declaration; optional content is selected only after the effect.
pub const TRUST_SET: Declaration<TrustSource> = Declaration {
    operation: "EnvironmentConfigTrustSet",
    bindings: &[
        Binding {
            source: TrustSource::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: TrustSource::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: TrustSource::Foreign,
            target: TargetResolver::ResolvedCollection("trust_reply_sources"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: TrustSource::Groups,
            target: TargetResolver::ResolvedCollection("trust_reply_groups"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: TrustSource::Orgs,
            target: TargetResolver::ResolvedCollection("trust_reply_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: TrustSource::Projects,
            target: TargetResolver::ResolvedCollection("trust_reply_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: TrustSource::Topics,
            target: TargetResolver::ResolvedCollection("trust_reply_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: TrustSource::Threads,
            target: TargetResolver::ResolvedCollection("trust_reply_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The native person-only Manage action and independently bounded response content.
pub const TRUST_SET_FACETS: [FacetSpec; 8] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Mount),
        ],
        TargetResolver::ResolvedCollection("trust_reply_sources"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection("trust_reply_groups"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("trust_reply_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("trust_reply_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("trust_reply_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("trust_reply_threads"),
    ),
];

/// The native enabled-server selection and its independently bounded references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpSource {
    /// The live Environment whose native whole reading door was selected.
    Environment,
    /// Native enabled presets, imports and Environment-owned servers.
    Servers,
    /// Actual offering-machine references selected by the native reader.
    Machines,
    /// Native Library dependent names, after its existing privacy projection.
    LibraryNames,
    /// Names of org settings that enabled a server.
    Orgs,
    /// Names of project settings that enabled a server.
    Projects,
    /// Names of topic settings that enabled a server.
    Topics,
    /// Names of thread settings that enabled a server.
    Threads,
}

/// One native source row; unselected optional content is a real selected empty set.
pub const MCP_GET: Declaration<McpSource> = Declaration {
    operation: "EnvironmentMcpGet",
    bindings: &[
        Binding {
            source: McpSource::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: McpSource::Servers,
            target: TargetResolver::ResolvedCollection("environment_mcp_servers"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpSource::Machines,
            target: TargetResolver::ResolvedCollection("environment_mcp_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpSource::LibraryNames,
            target: TargetResolver::ResolvedCollection("environment_mcp_library_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpSource::Orgs,
            target: TargetResolver::ResolvedCollection("environment_mcp_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpSource::Projects,
            target: TargetResolver::ResolvedCollection("environment_mcp_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpSource::Topics,
            target: TargetResolver::ResolvedCollection("environment_mcp_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpSource::Threads,
            target: TargetResolver::ResolvedCollection("environment_mcp_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The native primary context, selected server content and optional context names.
pub const MCP_GET_FACETS: [FacetSpec; 8] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::McpServer)],
        TargetResolver::ResolvedCollection("environment_mcp_servers"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::ResolvedCollection("environment_mcp_machines"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
            KeyTargetKind::Object(ObjectKind::Document),
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
            KeyTargetKind::Object(ObjectKind::Provider),
            KeyTargetKind::Object(ObjectKind::Agent),
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::McpServer),
            KeyTargetKind::Object(ObjectKind::Mount),
            KeyTargetKind::Object(ObjectKind::Skill),
        ],
        TargetResolver::ResolvedCollection("environment_mcp_library_names"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("environment_mcp_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("environment_mcp_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("environment_mcp_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("environment_mcp_threads"),
    ),
];

/// Stored recovery-batch members and the native job reader's fresh name sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReadSource {
    /// Actual Environments selected by the stored native job members.
    Environments,
    /// Native waiting-machine names retained by full job projections.
    Machines,
}

/// The batch UUID selects a record; each actual member supplies its own Environment identity.
pub const RECOVERY_GET: Declaration<RecoveryReadSource> = Declaration {
    operation: "EnvironmentRecoverGet",
    bindings: &[
        Binding {
            source: RecoveryReadSource::Environments,
            target: TargetResolver::ResolvedCollection("batch_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: RecoveryReadSource::Machines,
            target: TargetResolver::ResolvedCollection("batch_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Stored job payloads stay under their native Environment governor; fresh names stay separate.
pub const RECOVERY_GET_FACETS: [FacetSpec; 2] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("batch_environments"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::ResolvedCollection("batch_machines"),
    ),
];

/// Actual heterogeneous fleet subjects and emitted native context references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetReadSource {
    /// Returned Environments and managed Host aggregates, with explicit business kinds.
    Subjects,
    /// Host names actually returned on Environment entries.
    Hosts,
    /// Template references actually returned on whole Environment entries.
    Templates,
    /// Providers whose custody the native manager projection returns.
    Providers,
    /// Protected org homes.
    Orgs,
    /// Protected project homes.
    Projects,
    /// Protected topic homes.
    Topics,
    /// Protected thread homes.
    Threads,
}

/// One native fleet declaration; its URL node remains the native selector's filter and gate.
pub const FLEET: Declaration<FleetReadSource> = Declaration {
    operation: "NodeEnvironmentList",
    bindings: &[
        Binding {
            source: FleetReadSource::Subjects,
            target: TargetResolver::ResolvedCollection("fleet_subjects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: FleetReadSource::Hosts,
            target: TargetResolver::ResolvedCollection("fleet_hosts"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: FleetReadSource::Templates,
            target: TargetResolver::ResolvedCollection("fleet_templates"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: FleetReadSource::Providers,
            target: TargetResolver::ResolvedCollection("fleet_providers"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: FleetReadSource::Orgs,
            target: TargetResolver::ResolvedCollection("fleet_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: FleetReadSource::Projects,
            target: TargetResolver::ResolvedCollection("fleet_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: FleetReadSource::Topics,
            target: TargetResolver::ResolvedCollection("fleet_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: FleetReadSource::Threads,
            target: TargetResolver::ResolvedCollection("fleet_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Managed Host loads are first-class subjects; counts retain their native resource governor.
pub const FLEET_FACETS: [FacetSpec; 8] = [
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Host),
        ],
        TargetResolver::ResolvedCollection("fleet_subjects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Host)],
        TargetResolver::ResolvedCollection("fleet_hosts"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Template)],
        TargetResolver::ResolvedCollection("fleet_templates"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Provider)],
        TargetResolver::ResolvedCollection("fleet_providers"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("fleet_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("fleet_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("fleet_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("fleet_threads"),
    ),
];

/// Native loss/recovery inventory subjects, before names are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryInventorySource {
    /// The actual selected Host/Machine/Environment and returned manageable Environments.
    Subjects,
}

/// One source row serves the trusted effect-free body reader.
pub const LOSS_RECOVERY: Declaration<RecoveryInventorySource> = Declaration {
    operation: "LossRecoveryList",
    bindings: &[Binding {
        source: RecoveryInventorySource::Subjects,
        target: TargetResolver::ResolvedCollection("affected_resources"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }],
};

/// This is a privileged native inspection; its genuine action remains Manage.
pub const LOSS_RECOVERY_FACETS: [FacetSpec; 1] = [FacetSpec::required(
    KeyAction::Manage,
    &[
        KeyTargetKind::Object(ObjectKind::Host),
        KeyTargetKind::Object(ObjectKind::Machine),
        KeyTargetKind::Object(ObjectKind::Environment),
    ],
    TargetResolver::ResolvedCollection("affected_resources"),
)];

/// Actual parent and newly stored subject sources for an owned MCP creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCreateSource {
    /// Existing Environment management.
    Environment,
    /// MCP creation at that actual Environment parent.
    Parent,
    /// The actual created server's native Read governor.
    Created,
    /// Native foreign Environment/Machine references in the full projection.
    Foreign,
    /// Native group-holder names.
    Groups,
    /// Native org labels.
    Orgs,
    /// Native project labels.
    Projects,
    /// Native topic labels.
    Topics,
    /// Native thread labels.
    Threads,
}

/// One row connects effects and independently selected created-resource content.
pub const MCP_CREATE: Declaration<McpCreateSource> = Declaration {
    operation: "EnvironmentMcpServerCreate",
    bindings: &[
        Binding {
            source: McpCreateSource::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: McpCreateSource::Parent,
            target: TargetResolver::Resolved("server_parent"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: McpCreateSource::Created,
            target: TargetResolver::Resolved("created_server"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: McpCreateSource::Foreign,
            target: TargetResolver::ResolvedCollection("mcp_created_sources"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpCreateSource::Groups,
            target: TargetResolver::ResolvedCollection("mcp_created_groups"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpCreateSource::Orgs,
            target: TargetResolver::ResolvedCollection("mcp_created_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpCreateSource::Projects,
            target: TargetResolver::ResolvedCollection("mcp_created_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpCreateSource::Topics,
            target: TargetResolver::ResolvedCollection("mcp_created_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: McpCreateSource::Threads,
            target: TargetResolver::ResolvedCollection("mcp_created_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// A copied machine offering has the same real creation parent and independent Read roles.
pub const MCP_IMPORT: Declaration<McpCreateSource> = Declaration {
    operation: "EnvironmentMcpImportCreate",
    bindings: MCP_CREATE.bindings,
};

/// Existing management acts remain required; response Read never becomes a creation prerequisite.
pub const MCP_CREATE_FACETS: [FacetSpec; 9] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::McpServer)],
        TargetResolver::Resolved("server_parent"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::McpServer)],
        TargetResolver::Resolved("created_server"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Machine),
        ],
        TargetResolver::ResolvedCollection("mcp_created_sources"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection("mcp_created_groups"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("mcp_created_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("mcp_created_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("mcp_created_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("mcp_created_threads"),
    ),
];

/// Real inputs and selected choices of the native placement inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceReadSource {
    /// Actual returned Host placements and managed Environments.
    Subjects,
    /// The actual home whose inherited configuration is inspected.
    Home,
    /// The explicit or inherited stored Template used by the calculation.
    Template,
    /// Host references on returned Environment choices.
    Hosts,
    /// Physical Machine references on returned Environment choices.
    Machines,
    /// Stored Mount rows named by returned Host placements.
    Mounts,
    /// Protected Topic labels on returned Environment choices.
    Topics,
}

/// One selected-row source declaration for both kinds of native place.
pub const PLACE_LIST: Declaration<PlaceReadSource> = Declaration {
    operation: "NodePlaceList",
    bindings: &[
        Binding {
            source: PlaceReadSource::Subjects,
            target: TargetResolver::ResolvedCollection("available_places"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: PlaceReadSource::Home,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: PlaceReadSource::Template,
            target: TargetResolver::Resolved("place_template"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: PlaceReadSource::Hosts,
            target: TargetResolver::ResolvedCollection("place_hosts"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: PlaceReadSource::Machines,
            target: TargetResolver::ResolvedCollection("place_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: PlaceReadSource::Mounts,
            target: TargetResolver::ResolvedCollection("place_mounts"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: PlaceReadSource::Topics,
            target: TargetResolver::ResolvedCollection("place_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// A native calculation reads its actual home and Template, then independently scoped choices.
pub const PLACE_LIST_FACETS: [FacetSpec; 7] = [
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Environment),
        ],
        TargetResolver::ResolvedCollection("available_places"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Template)],
        TargetResolver::Resolved("place_template"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Host)],
        TargetResolver::ResolvedCollection("place_hosts"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::ResolvedCollection("place_machines"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Mount)],
        TargetResolver::ResolvedCollection("place_mounts"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("place_topics"),
    ),
];

/// Native individual Environment view sources, including its retained inspection shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetSource {
    /// The actual stored live or retained Environment.
    Environment,
    /// Foreign content actually kept by the native whole-view cut.
    Foreign,
    /// Native Group holder names.
    Groups,
    /// Protected org labels.
    Orgs,
    /// Protected project labels.
    Projects,
    /// Protected topic labels.
    Topics,
    /// Protected thread labels.
    Threads,
    /// Native independently visible Library binding names in the sync member.
    SyncLibraryThreads,
    /// Actual named machine waits of native feed jobs.
    SyncJobMachines,
}

/// One operation row supplies both native intent variants with the same factual roles.
pub const GET_READ: Declaration<GetSource> = Declaration {
    operation: "EnvironmentGet.read",
    bindings: &[
        Binding {
            source: GetSource::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: GetSource::Foreign,
            target: TargetResolver::ResolvedCollection("environment_read_sources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: GetSource::Groups,
            target: TargetResolver::ResolvedCollection("environment_read_groups"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: GetSource::Orgs,
            target: TargetResolver::ResolvedCollection("environment_read_orgs"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: GetSource::Projects,
            target: TargetResolver::ResolvedCollection("environment_read_projects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: GetSource::Topics,
            target: TargetResolver::ResolvedCollection("environment_read_topics"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: GetSource::Threads,
            target: TargetResolver::ResolvedCollection("environment_read_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The explicit inspect intent retains native Manage, independently of whether the row is removed.
pub const GET_INSPECT: Declaration<GetSource> = Declaration {
    operation: "EnvironmentGet.inspect_deleted",
    bindings: GET_READ.bindings,
};

/// Whole-view foreign content is required only when the native producer actually emits it.
pub const GET_READ_FACETS: [FacetSpec; 7] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Mount),
        ],
        TargetResolver::ResolvedCollection("environment_read_sources"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection("environment_read_groups"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("environment_read_orgs"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("environment_read_projects"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("environment_read_topics"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("environment_read_threads"),
    ),
];

/// Retained inspection is the same genuine privileged read as the native retained list.
pub const GET_INSPECT_FACETS: [FacetSpec; 7] = {
    let mut facets = GET_READ_FACETS;
    facets[0] = FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Path("id"),
    );
    facets
};

/// One row for the native environment feed's environment, Library and job members.
pub const SYNC: Declaration<GetSource> = Declaration {
    operation: "EnvironmentSyncGet",
    bindings: &[
        GET_READ.bindings[0],
        GET_READ.bindings[1],
        GET_READ.bindings[2],
        GET_READ.bindings[3],
        GET_READ.bindings[4],
        GET_READ.bindings[5],
        GET_READ.bindings[6],
        Binding {
            source: GetSource::SyncLibraryThreads,
            target: TargetResolver::ResolvedCollection("environment_sync_library_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: GetSource::SyncJobMachines,
            target: TargetResolver::ResolvedCollection("environment_sync_job_machines"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Additional members preserve their native privacy cuts and separately bound live labels.
pub const SYNC_FACETS: [FacetSpec; 9] = [
    GET_READ_FACETS[0],
    GET_READ_FACETS[1],
    GET_READ_FACETS[2],
    GET_READ_FACETS[3],
    GET_READ_FACETS[4],
    GET_READ_FACETS[5],
    GET_READ_FACETS[6],
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("environment_sync_library_threads"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::ResolvedCollection("environment_sync_job_machines"),
    ),
];

/// Actual native subjects of a privately stored operation audience.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipientSource {
    /// The native aggregate governor or individually selected business subjects.
    Subjects,
}

/// One stored-source row handles every native preview subtype and honest empty batches.
pub const RECIPIENTS: Declaration<RecipientSource> = Declaration {
    operation: "OperationRecipientsGet",
    bindings: &[Binding {
        source: RecipientSource::Subjects,
        target: TargetResolver::ResolvedCollection("operation_subjects"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }],
};

/// The preview UUID and recipient accounts never substitute for its real business subjects.
pub const RECIPIENTS_FACETS: [FacetSpec; 1] = [FacetSpec::required(
    KeyAction::Read,
    &[
        KeyTargetKind::Object(ObjectKind::Host),
        KeyTargetKind::Object(ObjectKind::Environment),
        KeyTargetKind::Object(ObjectKind::Provider),
        KeyTargetKind::Object(ObjectKind::Machine),
        KeyTargetKind::Object(ObjectKind::Thread),
    ],
    TargetResolver::ResolvedCollection("operation_subjects"),
)];

/// Actual stored configuration target, optional changed home and native response sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateSource {
    /// Existing Environment management and independently optional content.
    Environment,
    /// The actual new Node in the Environment creation namespace, or proven unchanged.
    Home,
    /// Native foreign resource context in full readback.
    Foreign,
    /// Group holder names.
    Groups,
    /// Org labels.
    Orgs,
    /// Project labels.
    Projects,
    /// Topic labels.
    Topics,
    /// Thread labels.
    Threads,
}

/// One prepared-effect declaration supplies the writer and its own native resource response.
pub const UPDATE: Declaration<UpdateSource> = Declaration {
    operation: "EnvironmentUpdate",
    bindings: &[
        Binding {
            source: UpdateSource::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: UpdateSource::Home,
            target: TargetResolver::ResolvedCollection("changed_home"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: UpdateSource::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: UpdateSource::Foreign,
            target: TargetResolver::ResolvedCollection("environment_update_sources"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: UpdateSource::Groups,
            target: TargetResolver::ResolvedCollection("environment_update_groups"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: UpdateSource::Orgs,
            target: TargetResolver::ResolvedCollection("environment_update_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: UpdateSource::Projects,
            target: TargetResolver::ResolvedCollection("environment_update_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: UpdateSource::Topics,
            target: TargetResolver::ResolvedCollection("environment_update_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: UpdateSource::Threads,
            target: TargetResolver::ResolvedCollection("environment_update_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Home changes keep their native Environment Write namespace and do not require invented rows.
pub const UPDATE_FACETS: [FacetSpec; 9] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("changed_home"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Template),
            KeyTargetKind::Object(ObjectKind::Host),
            KeyTargetKind::Object(ObjectKind::Machine),
            KeyTargetKind::Object(ObjectKind::Mount),
        ],
        TargetResolver::ResolvedCollection("environment_update_sources"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection("environment_update_groups"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("environment_update_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("environment_update_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("environment_update_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("environment_update_threads"),
    ),
];
