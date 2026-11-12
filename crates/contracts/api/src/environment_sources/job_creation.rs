//! The governing resource of native singleton update jobs and their current replies.
use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Governor,
    Content,
}
const BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::Governor,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Content,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
];
pub const ENVIRONMENT_UPDATE: Declaration<Source> = Declaration {
    operation: "EnvironmentUpdateCreate",
    bindings: BINDINGS,
};
pub const PROVIDER_UPDATE: Declaration<Source> = Declaration {
    operation: "ProviderRuntimeUpdateCreate",
    bindings: BINDINGS,
};
