//! Display grouping over the existing resource home and native machine availability.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, OperationSpec, TargetResolver};
use crate::{nodes::NodeRef, resources::MachineRef};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_types::{Id, Revision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A native picker's availability judgement for one execution machine.
/// It is separate from the caller's permission to use or manage the resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceMachineAvailability {
    /// The actual machine on which this option was judged.
    pub machine_id: Id,
    /// Whether the native source can supply this resource there.
    pub available: bool,
    /// Why it is unavailable; no definition, command, path or credential is included.
    pub reason: Option<String>,
}

/// The same display metadata on resource lists and contextual picker options.
/// An empty availability list means no machine judgement was made, not universal availability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceGrouping {
    /// Its stored sharing/settings home, through the existing protected reference producer.
    pub home: Option<NodeRef>,
    /// The same home's hierarchy path, with each protected component independently redacted.
    #[serde(default)]
    pub home_path: Vec<NodeRef>,
    /// Where it came from, independently of a manually chosen home.
    pub origin_machine: Option<MachineRef>,
    /// Actual per-machine judgements, including unavailable options and their reasons.
    pub availability: Vec<ResourceMachineAvailability>,
}

/// Metadata for a resource reference offered by a compound picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceChoiceGrouping {
    /// The actual resource family, as in the resource library.
    pub kind: String,
    /// The actual selected resource.
    pub id: Id,
    /// Its existing protected first-owner reference, or the system.
    pub owner: Option<crate::users::UserRef>,
    /// Home, protected origin and native machine judgement for this choice.
    #[serde(flatten)]
    pub grouping: ResourceGrouping,
}

/// Provenance beside a full resource's existing home and owner fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceProvenance {
    /// The protected originating machine, independent of a manager's home override.
    pub origin_machine: Option<MachineRef>,
    /// Native contextual verdicts; empty means no machine was judged.
    #[serde(default)]
    pub availability: Vec<ResourceMachineAvailability>,
}

/// Move within the existing root; ownership and operational machine bindings stay intact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceHomeSetBody {
    /// An org or project in the same root that the caller manages.
    pub home_id: Id,
    /// Refuse a resource changed since it was observed, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// Generated acknowledgement of the accepted home, without reading existing resource content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceHomeSetReceipt {
    pub projection: crate::common::ReceiptProjection,
    pub object_id: Id,
    pub kind: String,
    pub home_id: Id,
    pub changed: bool,
}

/// Replace display tags using the same place-defined catalogue as node tags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceTagsSetBody {
    /// The exact tag set accepted from now on; empty removes the assignments.
    pub tag_ids: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// Accepted input/outcome only; definition names and existing resource content are not returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceTagsSetReceipt {
    pub projection: crate::common::ReceiptProjection,
    pub object_id: Id,
    pub kind: String,
    pub tag_ids: Vec<Id>,
    pub changed: bool,
}

/// Real native sources of the resource location/tag writers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Object,
    Home,
}

const KINDS: &[KeyTargetKind] = &[
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

pub const HOME_FACETS: &[FacetSpec] = &[
    FacetSpec::required(KeyAction::Manage, KINDS, TargetResolver::Path("id")),
    FacetSpec::required(
        KeyAction::Manage,
        &[
            KeyTargetKind::Object(ObjectKind::Org),
            KeyTargetKind::Object(ObjectKind::Project),
        ],
        TargetResolver::Resolved("destination_home"),
    ),
];
pub const HOME: Declaration<Source> = Declaration {
    operation: "ObjectHomeSet",
    bindings: &[
        Binding {
            source: Source::Object,
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Home,
            target: TargetResolver::Resolved("destination_home"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
    ],
};
pub const TAGS: Declaration<Source> = Declaration {
    operation: "ObjectTagsSet",
    bindings: &[HOME.bindings[0]],
};

crate::endpoints! {
    /// Moves a resource within its root to a managed home and retains that manual choice.
    ObjectHomeSet: PUT "/{object}/{id}/home", crate::routes::Access::Rule("resource.manage and org.manage or project.manage on the target home"), none, json ResourceHomeSetBody, json ResourceHomeSetReceipt, 200,
        operation OperationSpec::receipt(&OperationSpec::facets(HOME_FACETS));
    /// Replaces the display tags of an existing resource using its home catalogue.
    ObjectTagsSet: PUT "/{object}/{id}/tags", crate::routes::Access::Rule("resource.manage"), none, json ResourceTagsSetBody, json ResourceTagsSetReceipt, 200,
        operation OperationSpec::receipt(&OperationSpec::single(KeyAction::Manage, KINDS, TargetResolver::Path("id")));
}
