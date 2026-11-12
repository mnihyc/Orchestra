//! Stored artifact identities and the native live place labels in their metadata.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Actual stored artifact and independently protected current place labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The selected native stored artifact.
    Artifact,
    /// Current org names.
    Orgs,
    /// Current project names.
    Projects,
    /// Current topic names.
    Topics,
    /// Current thread names.
    Threads,
}

/// One stored-source declaration for the creator's native export metadata.
pub const EXPORT_GET: Declaration<Source> = Declaration {
    operation: "ExportGet",
    bindings: &[
        Binding {
            source: Source::Artifact,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Orgs,
            target: TargetResolver::ResolvedCollection("artifact_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Projects,
            target: TargetResolver::ResolvedCollection("artifact_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Topics,
            target: TargetResolver::ResolvedCollection("artifact_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Threads,
            target: TargetResolver::ResolvedCollection("artifact_threads"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// The artifact keeps its genuine identity after purge, while live names stay separate.
pub const EXPORT_GET_FACETS: [FacetSpec; 5] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Export)],
        TargetResolver::Path("id"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection("artifact_orgs"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection("artifact_projects"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection("artifact_topics"),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection("artifact_threads"),
    ),
];

/// One selected page of the creator's native Export identities.
pub const EXPORT_LIST: Declaration<Source> = Declaration {
    operation: "MeExportList",
    bindings: &[
        Binding {
            source: Source::Artifact,
            target: TargetResolver::ResolvedCollection("results"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        EXPORT_GET.bindings[1],
        EXPORT_GET.bindings[2],
        EXPORT_GET.bindings[3],
        EXPORT_GET.bindings[4],
    ],
};

/// Real empty pages have no invented account or Export identity.
pub const EXPORT_LIST_FACETS: [FacetSpec; 5] = {
    let mut facets = EXPORT_GET_FACETS;
    facets[0] = FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Export)],
        TargetResolver::ResolvedCollection("results"),
    );
    facets
};

/// The native creator's stored Import metadata and independent live destination labels.
pub const IMPORT_GET: Declaration<Source> = Declaration {
    operation: "ImportGet",
    bindings: EXPORT_GET.bindings,
};

/// Import is its own real retained identity, not the destination or an Export at the same UUID.
pub const IMPORT_GET_FACETS: [FacetSpec; 5] = {
    let mut facets = EXPORT_GET_FACETS;
    facets[0] = FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Import)],
        TargetResolver::Path("id"),
    );
    facets
};

/// Stored source file names have one actual Import governor and no live name projection.
pub const IMPORT_FILES: Declaration<Source> = Declaration {
    operation: "ImportFileList",
    bindings: &[EXPORT_GET.bindings[0]],
};

/// Retained source file names are read through the genuine stored Import identity.
pub const IMPORT_FILES_FACETS: [FacetSpec; 1] = [IMPORT_GET_FACETS[0]];

/// The creator's actual selected narrow Import history and its live destination labels.
pub const IMPORT_LIST: Declaration<Source> = Declaration {
    operation: "MeImportList",
    bindings: EXPORT_LIST.bindings,
};

/// Native scope filtering precedes the result window, including a genuine empty selection.
pub const IMPORT_LIST_FACETS: [FacetSpec; 5] = {
    let mut facets = IMPORT_GET_FACETS;
    facets[0] = FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Import)],
        TargetResolver::ResolvedCollection("results"),
    );
    facets
};

/// The creator's persisted report records and separately protected stored place references.
pub const IMPORT_REPORT: Declaration<Source> = Declaration {
    operation: "ImportReportList",
    bindings: IMPORT_GET.bindings,
};

/// Report source names are retained Import content; populated destination labels stay separate.
pub const IMPORT_REPORT_FACETS: [FacetSpec; 5] = IMPORT_GET_FACETS;

/// The actual stored report header and each independently selected native row piece.
pub const IMPORT_REPORT_DOWNLOAD: Declaration<Source> = Declaration {
    operation: "ImportReportDownload",
    bindings: IMPORT_GET.bindings,
};

/// Streamed saved report content and any actually emitted destination reference names.
pub const IMPORT_REPORT_DOWNLOAD_FACETS: [FacetSpec; 5] = IMPORT_GET_FACETS;

/// Native archive removal governs its own associated writer cancellation and pin release.
pub const EXPORT_DELETE: Declaration<Source> = Declaration {
    operation: "ExportDelete",
    bindings: &[EXPORT_GET.bindings[0]],
};

/// The native empty acknowledgement requires no metadata Read.
pub const EXPORT_DELETE_FACETS: [FacetSpec; 1] = [FacetSpec::required(
    KeyAction::Manage,
    &[KeyTargetKind::Identity(KeyIdentityKind::Export)],
    TargetResolver::Path("id"),
)];

/// Native retained archive Read and its independently protected optional filename label.
pub const EXPORT_DOWNLOAD: Declaration<Source> = Declaration {
    operation: "ExportDownload",
    bindings: EXPORT_GET.bindings,
};

/// Archive custody remains independent of a live place; its label is optional content.
pub const EXPORT_DOWNLOAD_FACETS: [FacetSpec; 5] = EXPORT_GET_FACETS;
