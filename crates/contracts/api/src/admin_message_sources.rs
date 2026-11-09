//! Native governing scopes for administrator messages, without separate recipient permissions.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual organization or the installation being addressed.
    Scope,
    /// Same-operation optional reading of the accepted message.
    Content,
}

macro_rules! row {
    ($operation:literal, $target:expr) => {
        Declaration {
            operation: $operation,
            bindings: &[
                Binding {
                    source: Source::Scope,
                    target: $target,
                    usage: FacetUsage::Required,
                    cardinality: Cardinality::One,
                },
                Binding {
                    source: Source::Content,
                    target: $target,
                    usage: FacetUsage::ResponseContent,
                    cardinality: Cardinality::One,
                },
            ],
        }
    };
}

pub const ORG_CREATE: Declaration<Source> = row!("OrgMessageCreate", TargetResolver::Path("id"));
pub const SYSTEM_CREATE: Declaration<Source> = row!("AdminMessageCreate", TargetResolver::System);

/// One native scope, selected from the accepted message rather than its recipient roster.
pub const GET: Declaration<Source> = Declaration {
    operation: "MessageGet",
    bindings: &[Binding {
        source: Source::Scope,
        target: TargetResolver::Resolved("message_scope"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
pub const ORG_LIST: Declaration<Source> = Declaration {
    operation: "OrgMessageList",
    bindings: &[Binding {
        source: Source::Scope,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
pub const SYSTEM_LIST: Declaration<Source> = Declaration {
    operation: "AdminMessageList",
    bindings: &[Binding {
        source: Source::Scope,
        target: TargetResolver::System,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
