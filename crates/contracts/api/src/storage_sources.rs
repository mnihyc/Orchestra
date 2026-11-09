//! Actual retained-storage governors and independently disclosed live reference names.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver as Target};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind as I, KeyTargetKind as T};
use orch_access_contract::permissions::ObjectKind as K;

/// Native source roles, produced before storage rows are formatted or counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual requested account, system, node or resource namespace.
    Namespace,
    /// Selected retained records' real governors.
    Records,
    /// Current reference names emitted by the native formatter.
    Names,
    /// Actual physical publishers used by the native cleanup target.
    Physical,
    /// Existing selected metadata returned after the completed write.
    Content,
}

/// Every actual storage governor, including identities with no permission-page UUID.
pub const KINDS: &[T] = &[
    T::Object(K::Org),
    T::Object(K::Project),
    T::Object(K::Topic),
    T::Object(K::Thread),
    T::Object(K::Document),
    T::Object(K::DocumentContainer),
    T::Object(K::Environment),
    T::Object(K::Provider),
    T::Object(K::Host),
    T::Object(K::Machine),
    T::Object(K::Mount),
    T::Object(K::Skill),
    T::Object(K::Template),
    T::Object(K::Agent),
    T::Object(K::McpServer),
    T::Identity(I::Account),
    T::Identity(I::System),
    T::Identity(I::Export),
    T::Identity(I::Import),
    T::Identity(I::Upload),
];
/// Management remains the report's native question; live reference names have their own Read.
pub const REPORT_FACETS: [FacetSpec; 3] = [
    FacetSpec::required(
        KeyAction::Manage,
        KINDS,
        Target::Resolved("storage_namespace"),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        KINDS,
        Target::ResolvedCollection("storage_records"),
    ),
    FacetSpec::response_content(KINDS, Target::ResolvedCollection("storage_names")),
];
/// The same row feeds route admission, native selection and final publication.
pub const REPORT: Declaration<Source> = Declaration {
    operation: "StorageGet",
    bindings: &[
        Binding {
            source: Source::Namespace,
            target: Target::Resolved("storage_namespace"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Records,
            target: Target::ResolvedCollection("storage_records"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: Source::Names,
            target: Target::ResolvedCollection("storage_names"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Native cleanup writes require their exact selected governors and physical destinations.
pub const CLEANUP_FACETS: [FacetSpec; 5] = [
    REPORT_FACETS[0],
    REPORT_FACETS[1],
    FacetSpec::required(
        KeyAction::Manage,
        KINDS,
        Target::ResolvedCollection("cleanup_destinations"),
    ),
    FacetSpec::response_content(KINDS, Target::ResolvedCollection("cleanup_content")),
    REPORT_FACETS[2],
];
const CLEANUP_BINDINGS: &[Binding<Source>] = &[
    REPORT.bindings[0],
    REPORT.bindings[1],
    Binding {
        source: Source::Physical,
        target: Target::ResolvedCollection("cleanup_destinations"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Content,
        target: Target::ResolvedCollection("cleanup_content"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    REPORT.bindings[2],
];
/// The stored preview selects the same actual native operands that confirmation consumes.
pub const PREFLIGHT: Declaration<Source> = Declaration {
    operation: "StorageCleanupPreflight",
    bindings: CLEANUP_BINDINGS,
};

/// Confirmation and retained forward work use the same native selected-source roles.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "StorageCleanupCreate",
    bindings: CLEANUP_BINDINGS,
};
