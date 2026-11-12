//! Current Provider projections behind the same operation's declared content facets.
//! S02 owns the shared source walkers/binder and ordinary bounded name projections.

use crate::routes::access::operations::NameRoles;

pub use orch_access_contract::permissions::ObjectKind;
pub const NAMES: NameRoles = NameRoles {
    groups: Some("provider_groups"),
    nodes: [
        (ObjectKind::Org, "provider_names_orgs"),
        (ObjectKind::Project, "provider_names_projects"),
        (ObjectKind::Topic, "provider_names_topics"),
        (ObjectKind::Thread, "provider_names_threads"),
    ],
    containers: None,
};
