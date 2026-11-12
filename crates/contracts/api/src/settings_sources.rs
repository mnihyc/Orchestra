//! Native resolved-setting provenance, separate from configuration values.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Facts reported by the environment settings resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual selected environment.
    Environment,
    /// Actual ordinary value/lock/model governors selected by native setting validation.
    SettingSubjects,
    /// The real node whose accepted setting response is optionally read.
    SettingResult,
    /// Actual optional org provenance names.
    OrgNames,
    /// Actual optional project provenance names.
    ProjectNames,
    /// Actual optional topic provenance names.
    TopicNames,
    /// Actual optional thread provenance names.
    ThreadNames,
}

/// Resolve the environment's values and independently bound provenance labels.
pub const ENVIRONMENT: Declaration<Source> = Declaration {
    operation: "EnvironmentSettingList",
    bindings: &[
        Binding {
            source: Source::Environment,
            target: TargetResolver::TypedPath {
                parameter: "id",
                kind: KeyTargetKind::Object(ObjectKind::Environment),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::OrgNames,
            target: TargetResolver::ResolvedCollection("environment_setting_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ProjectNames,
            target: TargetResolver::ResolvedCollection("environment_setting_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::TopicNames,
            target: TargetResolver::ResolvedCollection("environment_setting_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::ThreadNames,
            target: TargetResolver::ResolvedCollection("environment_setting_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The native setting operation, independent of its transport or later application.
pub const NODE_EDIT_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
            KeyTargetKind::Object(ObjectKind::Environment),
            KeyTargetKind::Object(ObjectKind::Provider),
        ],
        TargetResolver::ResolvedCollection("setting_subjects"),
    ),
    FacetSpec::response_content(
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::Resolved("setting_result"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("setting_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("setting_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("setting_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("setting_threads"),
    ),
];
const NODE_EDIT_BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::SettingSubjects,
        target: TargetResolver::ResolvedCollection("setting_subjects"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::SettingResult,
        target: TargetResolver::Resolved("setting_result"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::OrgNames,
        target: TargetResolver::ResolvedCollection("setting_orgs"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::ProjectNames,
        target: TargetResolver::ResolvedCollection("setting_projects"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::TopicNames,
        target: TargetResolver::ResolvedCollection("setting_topics"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::ThreadNames,
        target: TargetResolver::ResolvedCollection("setting_threads"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
pub const NODE_SET: Declaration<Source> = Declaration {
    operation: "NodeSettingSet",
    bindings: NODE_EDIT_BINDINGS,
};
pub const NODE_CLEAR: Declaration<Source> = Declaration {
    operation: "NodeSettingDelete",
    bindings: NODE_EDIT_BINDINGS,
};

/// Both actual subjects of the backend read are declared, including its governing place.
pub const CAPABILITY_BACKEND_FACETS: &[FacetSpec] = &[
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Provider)],
        TargetResolver::TypedPath {
            parameter: "id",
            kind: KeyTargetKind::Object(ObjectKind::Provider),
        },
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::Resolved("capability_backend_place"),
    ),
];
/// This read uses native source admission for the provider and the query's real place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityBackendSource {
    Provider,
    Place,
}
pub const CAPABILITY_BACKENDS: Declaration<CapabilityBackendSource> = Declaration {
    operation: "ProviderCapabilityBackendList",
    bindings: &[
        Binding {
            source: CapabilityBackendSource::Provider,
            target: TargetResolver::TypedPath {
                parameter: "id",
                kind: KeyTargetKind::Object(ObjectKind::Provider),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: CapabilityBackendSource::Place,
            target: TargetResolver::Resolved("capability_backend_place"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};
