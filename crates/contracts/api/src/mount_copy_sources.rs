//! A mount data copy binds its own mount and both real machine operands.
use crate::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetSpec, FacetUsage, TargetResolver},
};
use orch_access_contract::{
    key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Mount,
    Reader,
    Writer,
    Job,
}

pub const READER: &str = "mount_copy_reader";
pub const WRITER: &str = "mount_copy_writer";
const MOUNT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Mount);
const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);

pub const FACETS: [FacetSpec; 6] = [
    FacetSpec::required(KeyAction::Write, &[MOUNT], TargetResolver::Path("id")),
    FacetSpec::required(
        KeyAction::Manage,
        &[MACHINE],
        TargetResolver::Resolved(READER),
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[MACHINE],
        TargetResolver::Resolved(WRITER),
    ),
    FacetSpec::response_content(&[MOUNT], TargetResolver::Path("id")),
    FacetSpec::response_content(&[MACHINE], TargetResolver::Resolved(READER)),
    FacetSpec::response_content(&[MACHINE], TargetResolver::Resolved(WRITER)),
];

const BINDINGS: [Binding<Source>; 6] = [
    Binding {
        source: Source::Mount,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Reader,
        target: TargetResolver::Resolved(READER),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Writer,
        target: TargetResolver::Resolved(WRITER),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Mount,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Reader,
        target: TargetResolver::Resolved(READER),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Source::Writer,
        target: TargetResolver::Resolved(WRITER),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::One,
    },
];

pub const PREFLIGHT: Declaration<Source> = Declaration {
    operation: "MountCopyPreflight",
    bindings: &BINDINGS,
};
pub const CREATE: Declaration<Source> = Declaration {
    operation: "MountCopyCreate",
    bindings: &BINDINGS,
};

pub const JOBS: &str = "mount_copy_jobs";
pub const READ_FACETS: [FacetSpec; 2] = [
    FacetSpec::required(KeyAction::Read, &[MOUNT], TargetResolver::Path("id")),
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Job)],
        TargetResolver::ResolvedCollection(JOBS),
    ),
];
pub const READ: Declaration<Source> = Declaration {
    operation: "MountCopyLatest",
    bindings: &[
        Binding {
            source: Source::Mount,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Job,
            target: TargetResolver::ResolvedCollection(JOBS),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};
