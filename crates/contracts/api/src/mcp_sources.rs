//! Native MCP preset creation and its optional individual response sources.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Actual native creation home and completed preset projection roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual existing organization or System namespace where creation is authorized.
    Home,
    /// The real stored preset produced by the completed native effect.
    Created,
    /// The stored preset, or actual environment permission subject for an owned server.
    Subject,
    /// The owning environment when the native change also requires Environment Manage.
    Parent,
    /// The actual environment whose harness reads this protected definition.
    ImportEnvironment,
    /// The actual physical recipient of the protected native copy.
    ImportMachine,
    /// Native environment and physical reference payloads.
    References,
    /// Group payloads in native permission metadata.
    Groups,
    /// Protected organization labels.
    Orgs,
    /// Protected project labels.
    Projects,
    /// Protected topic labels.
    Topics,
    /// Protected thread labels.
    Threads,
}

/// The native reference payload collection.
pub const REFERENCES: &str = "mcp_created_sources";
/// The native permission-group payload collection.
pub const GROUPS: &str = "mcp_created_groups";
/// Native protected node-label roles, in hierarchy kind order.
pub const NODES: [&str; 4] = [
    "mcp_created_orgs",
    "mcp_created_projects",
    "mcp_created_topics",
    "mcp_created_threads",
];

/// Creation keeps the existing management act; content remains same-operation Read.
pub const CREATE_FACETS: [FacetSpec; 8] = [
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::McpServer)],
        TargetResolver::Resolved("home"),
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
        TargetResolver::ResolvedCollection(REFERENCES),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection(GROUPS),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection(NODES[0]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection(NODES[1]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection(NODES[2]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection(NODES[3]),
    ),
];

/// One native E declaration, also consumed by the stored completed-resource reader.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "McpServerCreate",
    bindings: &[
        Binding {
            source: Source::Home,
            target: TargetResolver::Resolved("home"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Created,
            target: TargetResolver::Resolved("created_server"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::References,
            target: TargetResolver::ResolvedCollection(REFERENCES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Groups,
            target: TargetResolver::ResolvedCollection(GROUPS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection(NODES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection(NODES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection(NODES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection(NODES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native stored changes keep the environment-owned management branch explicit.
pub const UPDATE_FACETS: [FacetSpec; 9] = {
    let mut facets = CREATE_FACETS;
    facets[0] = FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::McpServer)],
        TargetResolver::Resolved("server"),
    );
    facets[1] = FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::McpServer)],
        TargetResolver::Resolved("server"),
    );
    [
        facets[0],
        facets[1],
        facets[2],
        facets[3],
        facets[4],
        facets[5],
        facets[6],
        facets[7],
        FacetSpec::required(
            KeyAction::Manage,
            &[KeyTargetKind::Object(ObjectKind::Environment)],
            TargetResolver::ResolvedCollection("mcp_owned_environment"),
        ),
    ]
};

/// The same stored native subject supplies the act and the independently optional Read.
pub const UPDATE: Declaration<Source> = Declaration {
    operation: "McpServerUpdate",
    bindings: &[
        Binding {
            source: Source::Subject,
            target: TargetResolver::Resolved("server"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Parent,
            target: TargetResolver::ResolvedCollection("mcp_owned_environment"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Subject,
            target: TargetResolver::Resolved("server"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::References,
            target: TargetResolver::ResolvedCollection(REFERENCES),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Groups,
            target: TargetResolver::ResolvedCollection(GROUPS),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection(NODES[0]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection(NODES[1]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection(NODES[2]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection(NODES[3]),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Existing-definition imports retain the same stored subject and optional native response.
pub const IMPORT_FACETS: [FacetSpec; 11] = [
    UPDATE_FACETS[0],
    UPDATE_FACETS[1],
    UPDATE_FACETS[2],
    UPDATE_FACETS[3],
    UPDATE_FACETS[4],
    UPDATE_FACETS[5],
    UPDATE_FACETS[6],
    UPDATE_FACETS[7],
    UPDATE_FACETS[8],
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("mcp_import_environment"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::ResolvedCollection("mcp_import_machine"),
    ),
];
const IMPORT_BINDINGS: &[Binding<Source>] = &[
    UPDATE.bindings[0],
    UPDATE.bindings[1],
    UPDATE.bindings[2],
    UPDATE.bindings[3],
    UPDATE.bindings[4],
    UPDATE.bindings[5],
    UPDATE.bindings[6],
    UPDATE.bindings[7],
    UPDATE.bindings[8],
    Binding {
        source: Source::ImportEnvironment,
        target: TargetResolver::ResolvedCollection("mcp_import_environment"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::ImportMachine,
        target: TargetResolver::ResolvedCollection("mcp_import_machine"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];
/// One declaration for taking the actual current native source again.
pub const REIMPORT: Declaration<Source> = Declaration {
    operation: "McpServerReimport",
    bindings: IMPORT_BINDINGS,
};
/// One declaration for selecting the particular offering a manager reviewed.
pub const OFFERING_USE: Declaration<Source> = Declaration {
    operation: "McpOfferingUse",
    bindings: IMPORT_BINDINGS,
};
