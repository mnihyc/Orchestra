//! Native Mount usage sources connected to their canonical operation facets.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Primary stored subject and actual native selected usage sources, including counted rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseSource {
    /// The actual live Mount that the native gate opens.
    Mount,
    /// Node settings that enable this Mount, with explicit native node kinds.
    Scopes,
    /// Enabled Environment sources supplying usage metadata or the native hidden count.
    Environments,
}

const PRIMARY: TargetResolver = TargetResolver::TypedPath {
    parameter: "id",
    kind: KeyTargetKind::Object(ObjectKind::Mount),
};

/// The native usage page reads each selected source under its own operation.
pub const USE_FACETS: [FacetSpec; 3] = [
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Mount)],
        PRIMARY,
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
            KeyTargetKind::Object(ObjectKind::Topic),
            KeyTargetKind::Object(ObjectKind::Thread),
        ],
        TargetResolver::ResolvedCollection("mount_use_scopes"),
    ),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::ResolvedCollection("mount_use_environments"),
    ),
];

/// One selected-row declaration, shared by the native mounting adapter and producer.
pub const USES: Declaration<UseSource> = Declaration {
    operation: "MountUsesGet",
    bindings: &[
        Binding {
            source: UseSource::Mount,
            target: PRIMARY,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: UseSource::Scopes,
            target: TargetResolver::ResolvedCollection("mount_use_scopes"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: UseSource::Environments,
            target: TargetResolver::ResolvedCollection("mount_use_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
