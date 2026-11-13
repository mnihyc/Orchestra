//! Native recorded restore sources beside the canonical source contracts.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Actual native operands and optional existing-content references of a restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual managed Environment destination.
    Environment,
    /// Existing recorded and current workspace bytes read by the native procedure.
    Files,
    /// Actual reviewed writers controlled through the Thread Interact alternative.
    Writers,
    /// The Environment whose existing metadata and file plan are returned.
    Content,
    /// Actual visible writer names, independent of controlling those writers.
    WriterNames,
    /// The actual already-created Job returned by a confirmation replay.
    ExistingJob,
    /// Actual machine names emitted by the live replayed Job.
    MachineNames,
}

const ENVIRONMENT: Binding<Source> = Binding {
    source: Source::Environment,
    target: TargetResolver::Path("id"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const FILES: Binding<Source> = Binding {
    source: Source::Files,
    target: TargetResolver::Resolved("restore_files"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const CONTENT: Binding<Source> = Binding {
    source: Source::Content,
    target: TargetResolver::Resolved("restore_content"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};

/// A measured review also saves its actual private native plan.
pub const RESTORE_PREFLIGHT: Declaration<Source> = Declaration {
    operation: "EnvironmentRestorePreflight",
    bindings: &[
        ENVIRONMENT,
        FILES,
        CONTENT,
        Binding {
            source: Source::WriterNames,
            target: TargetResolver::ResolvedCollection("restore_writer_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Waiting, or controlling writers through the native Environment management alternative.
pub const RESTORE_ENVIRONMENT: Declaration<Source> = Declaration {
    operation: "EnvironmentFilesRestore.environment",
    bindings: &[ENVIRONMENT, FILES, CONTENT],
};

/// Native Thread Interact requires the actual affected writer set too.
pub const RESTORE_THREADS: Declaration<Source> = Declaration {
    operation: "EnvironmentFilesRestore.threads",
    bindings: &[
        ENVIRONMENT,
        FILES,
        CONTENT,
        Binding {
            source: Source::Writers,
            target: TargetResolver::ResolvedCollection("restore_writers"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// An actual already-created Job replay performs no new file operation.
pub const RESTORE_EXISTING: Declaration<Source> = Declaration {
    operation: "EnvironmentFilesRestore.existing",
    bindings: &[
        ENVIRONMENT,
        Binding {
            source: Source::ExistingJob,
            target: TargetResolver::Resolved("restore_existing_job"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::MachineNames,
            target: TargetResolver::ResolvedCollection("restore_machine_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
