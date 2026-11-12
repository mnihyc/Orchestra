//! A native Environment copy keeps its workspace source and real creation dependencies.
use super::*;

/// The existing creator roles, selected for this distinct original operation.
pub const MANAGED: Declaration<CreateSource> = Declaration {
    operation: "EnvironmentDuplicate.managed",
    bindings: &[
        CREATE.bindings[0],
        CREATE.bindings[1],
        CREATE.bindings[2],
        CREATE.bindings[3],
        CREATE.bindings[4],
        CREATE.bindings[5],
    ],
};

/// Source View and Files View share its actual Environment Read; creation keeps its own operands.
pub const MANAGED_FACETS: [FacetSpec; 6] = [
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        TargetResolver::Resolved("home"),
    ),
    CREATE_FACETS[1],
    CREATE_FACETS[2],
    CREATE_FACETS[3],
    CREATE_FACETS[4],
    CREATE_FACETS[5],
];

/// A registered directory copy has an actual physical recipient and no managed template or unit.
pub const CONNECTED: Declaration<CreateSource> = Declaration {
    operation: "EnvironmentDuplicate.connected",
    bindings: &[
        CREATE.bindings[0],
        Binding {
            source: CreateSource::Machine,
            target: TargetResolver::Resolved("duplicate_machine"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CREATE.bindings[3],
        CREATE.bindings[4],
        CREATE.bindings[5],
    ],
};

/// One source environment and native owned-data creation on the selected machine.
pub const CONNECTED_FACETS: [FacetSpec; 5] = [
    MANAGED_FACETS[0],
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Machine)],
        TargetResolver::Resolved("duplicate_machine"),
    ),
    CREATE_FACETS[3],
    CREATE_FACETS[4],
    CREATE_FACETS[5],
];

/// Original source Manage is separate from the actual workspace byte Read.
const MOVE_SUBJECT: Binding<CreateSource> = Binding {
    source: CreateSource::MoveSubject,
    target: TargetResolver::TypedPath {
        parameter: "id",
        kind: KeyTargetKind::Object(ObjectKind::Environment),
    },
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const MOVE_PREVIEW: Binding<CreateSource> = Binding {
    source: CreateSource::MovePreview,
    target: TargetResolver::TypedPath {
        parameter: "id",
        kind: KeyTargetKind::Object(ObjectKind::Environment),
    },
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};
const MOVE_FACET: FacetSpec = FacetSpec::required(
    KeyAction::Manage,
    &[KeyTargetKind::Object(ObjectKind::Environment)],
    TargetResolver::TypedPath {
        parameter: "id",
        kind: KeyTargetKind::Object(ObjectKind::Environment),
    },
);
const PREVIEW_FACET: FacetSpec = FacetSpec::response_content(
    &[KeyTargetKind::Object(ObjectKind::Environment)],
    TargetResolver::TypedPath {
        parameter: "id",
        kind: KeyTargetKind::Object(ObjectKind::Environment),
    },
);
pub const MOVE_MANAGED: Declaration<CreateSource> = Declaration {
    operation: "EnvironmentMoveOut.managed",
    bindings: &[
        MANAGED.bindings[0],
        MANAGED.bindings[1],
        MANAGED.bindings[2],
        MANAGED.bindings[3],
        MANAGED.bindings[4],
        MANAGED.bindings[5],
        MOVE_SUBJECT,
    ],
};
pub const MOVE_CONNECTED: Declaration<CreateSource> = Declaration {
    operation: "EnvironmentMoveOut.connected",
    bindings: &[
        CONNECTED.bindings[0],
        CONNECTED.bindings[1],
        CONNECTED.bindings[2],
        CONNECTED.bindings[3],
        CONNECTED.bindings[4],
        MOVE_SUBJECT,
    ],
};
pub const MOVE_MANAGED_PREVIEW: Declaration<CreateSource> = Declaration {
    operation: "EnvironmentMoveOutPreview.managed",
    bindings: &[
        MANAGED.bindings[0],
        MANAGED.bindings[1],
        MANAGED.bindings[2],
        MANAGED.bindings[3],
        MANAGED.bindings[4],
        MANAGED.bindings[5],
        MOVE_SUBJECT,
        MOVE_PREVIEW,
    ],
};
pub const MOVE_CONNECTED_PREVIEW: Declaration<CreateSource> = Declaration {
    operation: "EnvironmentMoveOutPreview.connected",
    bindings: &[
        CONNECTED.bindings[0],
        CONNECTED.bindings[1],
        CONNECTED.bindings[2],
        CONNECTED.bindings[3],
        CONNECTED.bindings[4],
        MOVE_SUBJECT,
        MOVE_PREVIEW,
    ],
};
pub const MOVE_MANAGED_FACETS: [FacetSpec; 7] = [
    MANAGED_FACETS[0],
    MANAGED_FACETS[1],
    MANAGED_FACETS[2],
    MANAGED_FACETS[3],
    MANAGED_FACETS[4],
    MANAGED_FACETS[5],
    MOVE_FACET,
];
pub const MOVE_CONNECTED_FACETS: [FacetSpec; 6] = [
    CONNECTED_FACETS[0],
    CONNECTED_FACETS[1],
    CONNECTED_FACETS[2],
    CONNECTED_FACETS[3],
    CONNECTED_FACETS[4],
    MOVE_FACET,
];
pub const MOVE_MANAGED_PREVIEW_FACETS: [FacetSpec; 8] = [
    MOVE_MANAGED_FACETS[0],
    MOVE_MANAGED_FACETS[1],
    MOVE_MANAGED_FACETS[2],
    MOVE_MANAGED_FACETS[3],
    MOVE_MANAGED_FACETS[4],
    MOVE_MANAGED_FACETS[5],
    MOVE_MANAGED_FACETS[6],
    PREVIEW_FACET,
];
pub const MOVE_CONNECTED_PREVIEW_FACETS: [FacetSpec; 7] = [
    MOVE_CONNECTED_FACETS[0],
    MOVE_CONNECTED_FACETS[1],
    MOVE_CONNECTED_FACETS[2],
    MOVE_CONNECTED_FACETS[3],
    MOVE_CONNECTED_FACETS[4],
    MOVE_CONNECTED_FACETS[5],
    PREVIEW_FACET,
];
/// Exact native connected creator selections; preview is never a retained creator.
pub fn is_connected(operation: &str) -> bool {
    operation == CONNECTED.operation || operation == MOVE_CONNECTED.operation
}
/// Exact native managed copy selections, each admitted under its own declaration.
pub fn is_managed(operation: &str) -> bool {
    operation == MANAGED.operation || operation == MOVE_MANAGED.operation
}
/// The two native workspace copying classes, preserving their actual operation.
pub fn is_copy(operation: &str) -> bool {
    is_connected(operation) || is_managed(operation)
}
/// Actual copy-and-delete classes only, never a preview or an ordinary duplicate.
pub fn is_move(operation: &str) -> bool {
    operation == MOVE_MANAGED.operation || operation == MOVE_CONNECTED.operation
}
