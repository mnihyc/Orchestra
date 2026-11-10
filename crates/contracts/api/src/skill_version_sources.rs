//! Native version identities and a draft slot share their actual keeping Skill scope.
use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
};
/// The current native resource that keeps the immutable version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The existing native skill, not a new version key kind.
    Skill,
}
const BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::Skill,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Skill,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
];
/// Existing-skill version acceptance and optional same-operation content.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "SkillVersionCreate",
    bindings: BINDINGS,
};
/// Draft publication and optional same-operation version content.
pub const PUBLISH: Declaration<Source> = Declaration {
    operation: "SkillDraftPublish",
    bindings: BINDINGS,
};
/// The native Skill's single draft slot and its optional current file metadata.
pub const DRAFT: Declaration<Source> = Declaration {
    operation: "SkillDraftCreate",
    bindings: BINDINGS,
};
