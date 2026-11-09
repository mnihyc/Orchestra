//! Native source-job facts beside the canonical source and restore contracts.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Actual native operands and protected problem sources of source preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual managed workspace.
    Environment,
    /// The actual stored Thread whose frozen source is used.
    Thread,
    /// Existing workspace bytes copied into an own worktree.
    WorkspaceFiles,
    /// Existing live job references in a native conflict.
    ProblemEnvironment,
}
/// Checkout/directory preparation needs its actual configuration source.
pub const SOURCE_PREPARE: Declaration<Source> = Declaration {
    operation: "EnvironmentSourcePrepare.prepare",
    bindings: &[
        Binding {
            source: Source::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Thread,
            target: TargetResolver::Resolved("bound_thread"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ProblemEnvironment,
            target: TargetResolver::Resolved("source_problem_environment"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};
/// A native worktree additionally reads existing workspace content.
pub const SOURCE_WORKTREE: Declaration<Source> = Declaration {
    operation: "EnvironmentSourcePrepare.worktree",
    bindings: &[
        Binding {
            source: Source::Environment,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Thread,
            target: TargetResolver::Resolved("bound_thread"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::WorkspaceFiles,
            target: TargetResolver::Resolved("worktree_environment"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::ProblemEnvironment,
            target: TargetResolver::Resolved("source_problem_environment"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};
