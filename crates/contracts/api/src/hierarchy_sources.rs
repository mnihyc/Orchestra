//! Native hierarchy source declarations shared by its creation and tree producers.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver as Target};
/// Actual hierarchy source roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Existing account or parent to which creation is addressed.
    Destination,
    /// Newly allocated organization, resolved only after native creation.
    Created,
    /// Actual selected native tree rows, including its real continuation witness.
    Results,
    /// Optional protected context names/content of the same producer.
    Names,
    /// Native full-opening settings at the actual root; absent on incremental pages.
    Settings,
}
const NAMES: Binding<Source> = Binding {
    source: Source::Names,
    target: Target::ResolvedCollection("hierarchy_names"),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::Collection,
};
/// The actual authenticated account is the root creation destination.
pub const CREATE: Declaration<Source> = Declaration {
    operation: "OrgCreate",
    bindings: &[
        Binding {
            source: Source::Destination,
            target: Target::CurrentAccount,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Created,
            target: Target::Resolved("created_org"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        NAMES,
    ],
};
/// Parent organization plus the actual created child; General remains in the same native transaction.
pub const CHILD: Declaration<Source> = Declaration {
    operation: "OrgChildCreate",
    bindings: &[
        Binding {
            source: Source::Destination,
            target: Target::PermissionObject,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Created,
            target: Target::Resolved("created_resource"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        NAMES,
    ],
};
/// The native tree owns rows, incremental repairs, cursor and optional initial settings.
pub const TREE: Declaration<Source> = Declaration {
    operation: "OrgTreeGet",
    bindings: &[
        Binding {
            source: Source::Results,
            target: Target::ResolvedCollection("results"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        NAMES,
        Binding {
            source: Source::Settings,
            target: Target::ResolvedCollection("tree_settings"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
