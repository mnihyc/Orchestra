//! Native resolved-setting provenance, separate from configuration values.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;

/// Facts reported by the environment settings resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual selected environment.
    Environment,
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
