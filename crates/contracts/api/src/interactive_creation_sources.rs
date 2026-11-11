//! Actual native browser/display reservation governors and protected thread labels.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver as Target};
/// Native sources in the existing session projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Actual environment containing the reserved browser or display.
    Environment,
    /// Native thread labels emitted by this current individual session.
    Threads,
}
const BINDINGS: &[Binding<Source>] = &[
    Binding {
        source: Source::Environment,
        target: Target::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Environment,
        target: Target::PermissionObject,
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Threads,
        target: Target::ResolvedCollection("close_threads"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
/// Browser creation retains its native environment management act.
pub const BROWSER: Declaration<Source> = Declaration {
    operation: "EnvironmentBrowserCreate",
    bindings: BINDINGS,
};
/// Display creation retains its native environment management act.
pub const DISPLAY: Declaration<Source> = Declaration {
    operation: "EnvironmentDisplayCreate",
    bindings: BINDINGS,
};

/// Profile metadata follows its actual native Environment and profile name.
pub const PROFILE: Declaration<Source> = Declaration {
    operation: "BrowserProfileCreate",
    bindings: &[
        Binding {
            source: Source::Environment,
            target: Target::PermissionObject,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Environment,
            target: Target::PermissionObject,
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};
