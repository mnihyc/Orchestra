//! Native notification subject declarations, shared by context and effect producers.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;

/// Subject kinds of the existing native notification subject switch.
/// A declared kind still passes the native producer's lifecycle and capability rules.
pub const SUBJECT_KINDS: &[KeyTargetKind] = &[
    KeyTargetKind::Object(ObjectKind::Org),
    KeyTargetKind::Object(ObjectKind::Project),
    KeyTargetKind::Object(ObjectKind::Topic),
    KeyTargetKind::Object(ObjectKind::Thread),
    KeyTargetKind::Object(ObjectKind::Document),
    KeyTargetKind::Object(ObjectKind::DocumentContainer),
    KeyTargetKind::Identity(KeyIdentityKind::Chat),
    KeyTargetKind::Object(ObjectKind::Provider),
    KeyTargetKind::Object(ObjectKind::Agent),
    KeyTargetKind::Object(ObjectKind::Environment),
    KeyTargetKind::Object(ObjectKind::Host),
    KeyTargetKind::Object(ObjectKind::Machine),
    KeyTargetKind::Object(ObjectKind::Template),
    KeyTargetKind::Object(ObjectKind::McpServer),
    KeyTargetKind::Object(ObjectKind::Mount),
    KeyTargetKind::Object(ObjectKind::Skill),
];

/// Native sources of a notification operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual node, document, chat or resource selected by the native producer.
    Subject,
}

/// A context read is bound to the same actual subject as its native capability projection.
pub const PERSON_CONTEXT: Declaration<Source> = Declaration {
    operation: "PersonNotifyContextGet",
    bindings: &[Binding {
        source: Source::Subject,
        target: TargetResolver::Resolved("subject"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
