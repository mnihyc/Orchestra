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
