//! Native workspace subjects connected to the canonical operation facets.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

/// Factual identities supplied by the native Snapshot custody and Job readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Files authority follows the stored Snapshot's native current governor.
    SnapshotGovernor,
    /// The actual Environment selected from the Thread's current snapshot binding.
    SnapshotEnvironment,
    /// Current optional disclosure of a completed snapshot request's source references.
    SnapshotContent,
    /// The actual stored native Job identity.
    Job,
    /// Current optional disclosure of the completed cancellation's Job.
    JobReply,
    /// The native subject branch that admitted this Job, not every optional link.
    JobSubjects,
    /// Real current machine labels disclosed by the native progress formatter.
    JobMachineNames,
    /// The native copy's real source and destination Environment.
    CopyEnvironment,
    /// Current optional disclosure of the completed copy's metadata.
    CopyContent,
}

/// Deleting retained state needs its actual Files governor, with no existing-content reply.
pub const SNAPSHOT_DELETE: Declaration<Source> = Declaration {
    operation: "SnapshotDelete",
    bindings: &[Binding {
        source: Source::SnapshotGovernor,
        target: TargetResolver::Resolved("snapshot_environment"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// Ordinary native Job visibility, including a real empty creator-only subject set.
pub const JOB_GET_ORDINARY: Declaration<Source> = Declaration {
    operation: "JobGet.ordinary",
    bindings: &[
        Binding {
            source: Source::Job,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::JobSubjects,
            target: TargetResolver::ResolvedCollection("job_subjects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::JobMachineNames,
            target: TargetResolver::ResolvedCollection("job_machine_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The native provider-management alternative keeps its real required Manage action.
pub const JOB_GET_PROVIDER: Declaration<Source> = Declaration {
    operation: "JobGet.provider",
    bindings: JOB_GET_ORDINARY.bindings,
};

/// Cancellation retains its actual subject, independently of optional job disclosure.
pub const JOB_CANCEL_LIFECYCLE: Declaration<Source> = Declaration {
    operation: "JobCancel.lifecycle",
    bindings: &[
        Binding {
            source: Source::Job,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::JobSubjects,
            target: TargetResolver::ResolvedCollection("job_subjects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::JobReply,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::JobMachineNames,
            target: TargetResolver::ResolvedCollection("job_machine_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Continuation cancellation uses its existing Thread Interact source.
pub const JOB_CANCEL_CONTINUATION: Declaration<Source> = Declaration {
    operation: "JobCancel.continuation",
    bindings: JOB_CANCEL_LIFECYCLE.bindings,
};

/// Measurement reads the actual source and keeps the existing Files management gate.
pub const THREAD_SNAPSHOT_PREFLIGHT: Declaration<Source> = Declaration {
    operation: "ThreadSnapshotPreflight",
    bindings: &[Binding {
        source: Source::SnapshotEnvironment,
        target: TargetResolver::Resolved("environment"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// The measured source is also the source copied by the queued native snapshot job.
pub const THREAD_SNAPSHOT_CREATE: Declaration<Source> = Declaration {
    operation: "ThreadSnapshotCreate",
    bindings: &[
        Binding {
            source: Source::SnapshotEnvironment,
            target: TargetResolver::Resolved("environment"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::SnapshotContent,
            target: TargetResolver::Resolved("snapshot_reply_environment"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

/// Whole-workspace export retains its own native management operation and stored source.
pub const ENVIRONMENT_FILES_EXPORT: Declaration<Source> = Declaration {
    operation: "EnvironmentFilesExport",
    bindings: THREAD_SNAPSHOT_CREATE.bindings,
};

/// Native workspace copying reads and writes its actual Environment, with separate response Read.
pub const ENVIRONMENT_FILE_COPY: Declaration<Source> = Declaration {
    operation: "EnvironmentFileCopy",
    bindings: &[
        Binding {
            source: Source::CopyEnvironment,
            target: TargetResolver::Resolved("copy_source"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::CopyEnvironment,
            target: TargetResolver::Resolved("copy_destination"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::CopyContent,
            target: TargetResolver::Resolved("copy_reply_environment"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};
