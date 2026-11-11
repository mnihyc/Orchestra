//! Shared native resource Read roles, expanded into the ordinary operation catalogue.
//!
//! These declarations describe producer facts. They add no permission evaluator or wire
//! authority shape: each consumer still publishes its own ordinary operation/facet list.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::FacetUsage;
use crate::operations::{FacetSpec, TargetResolver};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, ResourceKind};

/// One distinct native content source, with its business family independent of its governor.
pub struct SourceRole {
    /// The consuming operation's declared resolved collection role.
    pub role: &'static str,
    /// The native business family of the actual emitted source identities.
    pub kind: ResourceKind,
}

/// Protected names emitted by the existing native permission/home projection.
pub struct NameRoles {
    /// Full Group payloads, when that producer emits them.
    pub groups: Option<&'static str>,
    /// Org, Project, Topic and Thread names, in that order.
    pub nodes: [&'static str; 4],
    /// Document-folder names, when that producer emits them.
    pub containers: Option<&'static str>,
}

/// Factual native source roles shared by individual, list and inventory consumers.
pub struct Family {
    /// The business resource family of the selected primary identities.
    pub kind: ResourceKind,
    /// Actual distinct content sources reported by this family's native preparation.
    pub sources: &'static [SourceRole],
    /// The native preparation's protected names.
    pub names: NameRoles,
}

/// Factual roles of the same native family at a transport or continuing boundary.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The selected native primary subject.
    Primary,
    /// A distinct native content-source collection, by declaration position.
    Content(usize),
    /// Selected access Group names.
    Groups,
    /// Org, Project, Topic or Thread names, by declaration position.
    Node(usize),
    /// Selected document-folder names.
    Containers,
}

/// The existing typed business family as a static facet declaration.
pub const fn resources(kind: ResourceKind) -> &'static [KeyTargetKind] {
    match kind {
        ResourceKind::Provider => &[KeyTargetKind::Object(ObjectKind::Provider)],
        ResourceKind::Agent => &[KeyTargetKind::Object(ObjectKind::Agent)],
        ResourceKind::Environment => &[KeyTargetKind::Object(ObjectKind::Environment)],
        ResourceKind::Host => &[KeyTargetKind::Object(ObjectKind::Host)],
        ResourceKind::Machine => &[KeyTargetKind::Object(ObjectKind::Machine)],
        ResourceKind::Template => &[KeyTargetKind::Object(ObjectKind::Template)],
        ResourceKind::McpServer => &[KeyTargetKind::Object(ObjectKind::McpServer)],
        ResourceKind::Mount => &[KeyTargetKind::Object(ObjectKind::Mount)],
        ResourceKind::Skill => &[KeyTargetKind::Object(ObjectKind::Skill)],
    }
}

impl Family {
    /// The same factual row used by a native continuation. Actions and business kinds
    /// still come exclusively from this family's canonical operation facets.
    pub const fn bindings<const N: usize>(&self, primary: TargetResolver) -> [Binding<Source>; N] {
        let count = 1
            + self.sources.len()
            + self.names.groups.is_some() as usize
            + self.names.nodes.len()
            + self.names.containers.is_some() as usize;
        assert!(N == count);
        let root = Binding {
            source: Source::Primary,
            target: primary,
            usage: FacetUsage::Required,
            cardinality: if matches!(primary, TargetResolver::ResolvedCollection(_)) {
                Cardinality::Collection
            } else {
                Cardinality::One
            },
        };
        let mut bindings = [root; N];
        let mut next = 1;
        let mut source = 0;
        while source < self.sources.len() {
            bindings[next] = Binding {
                source: Source::Content(source),
                target: TargetResolver::ResolvedCollection(self.sources[source].role),
                usage: FacetUsage::Required,
                cardinality: Cardinality::Collection,
            };
            source += 1;
            next += 1;
        }
        if let Some(role) = self.names.groups {
            bindings[next] = Binding {
                source: Source::Groups,
                target: TargetResolver::ResolvedCollection(role),
                usage: FacetUsage::Required,
                cardinality: Cardinality::Collection,
            };
            next += 1;
        }
        let mut node = 0;
        while node < self.names.nodes.len() {
            bindings[next] = Binding {
                source: Source::Node(node),
                target: TargetResolver::ResolvedCollection(self.names.nodes[node]),
                usage: FacetUsage::Required,
                cardinality: Cardinality::Collection,
            };
            node += 1;
            next += 1;
        }
        if let Some(role) = self.names.containers {
            bindings[next] = Binding {
                source: Source::Containers,
                target: TargetResolver::ResolvedCollection(role),
                usage: FacetUsage::Required,
                cardinality: Cardinality::Collection,
            };
        }
        bindings
    }

    /// Compose the primary resolver with these same native source/name roles. The exact
    /// array length is checked in const evaluation; no unresolved role becomes empty.
    pub const fn facets<const N: usize>(&self, primary: TargetResolver) -> [FacetSpec; N] {
        let count = 1
            + self.sources.len()
            + self.names.groups.is_some() as usize
            + self.names.nodes.len()
            + self.names.containers.is_some() as usize;
        assert!(N == count);
        let root = FacetSpec::required(KeyAction::Read, resources(self.kind), primary);
        let mut facets = [root; N];
        let mut next = 1;
        let mut source = 0;
        while source < self.sources.len() {
            let role = &self.sources[source];
            facets[next] = FacetSpec::required(
                KeyAction::Read,
                resources(role.kind),
                TargetResolver::ResolvedCollection(role.role),
            );
            next += 1;
            source += 1;
        }
        if let Some(role) = self.names.groups {
            facets[next] = FacetSpec::required(
                KeyAction::Read,
                &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
                TargetResolver::ResolvedCollection(role),
            );
            next += 1;
        }
        let node_kinds: [&'static [KeyTargetKind]; 4] = [
            &[KeyTargetKind::Object(ObjectKind::Org)],
            &[KeyTargetKind::Object(ObjectKind::Project)],
            &[KeyTargetKind::Object(ObjectKind::Topic)],
            &[KeyTargetKind::Object(ObjectKind::Thread)],
        ];
        let mut node = 0;
        while node < self.names.nodes.len() {
            facets[next] = FacetSpec::required(
                KeyAction::Read,
                node_kinds[node],
                TargetResolver::ResolvedCollection(self.names.nodes[node]),
            );
            next += 1;
            node += 1;
        }
        if let Some(role) = self.names.containers {
            facets[next] = FacetSpec::required(
                KeyAction::Read,
                &[KeyTargetKind::Object(ObjectKind::DocumentContainer)],
                TargetResolver::ResolvedCollection(role),
            );
        }
        facets
    }
}

/// Mount Get, List and Inventory use the same emitted location/kept-machine facts.
/// Lost-location historical names and attached-environment counts remain intrinsic Mount data.
pub const MOUNT: Family = Family {
    kind: ResourceKind::Mount,
    sources: &[SourceRole {
        role: "mount_machines",
        kind: ResourceKind::Machine,
    }],
    names: NameRoles {
        groups: Some("mount_groups"),
        nodes: [
            "mount_names_orgs",
            "mount_names_projects",
            "mount_names_topics",
            "mount_names_threads",
        ],
        containers: None,
    },
};

/// Host content includes only the retained Environment references selected by its
/// native projection. Its physical Machine is intrinsic to the same Host subject.
pub const HOST: Family = Family {
    kind: ResourceKind::Host,
    sources: &[SourceRole {
        role: "host_retained_environments",
        kind: ResourceKind::Environment,
    }],
    names: NameRoles {
        groups: Some("host_groups"),
        nodes: [
            "host_names_orgs",
            "host_names_projects",
            "host_names_topics",
            "host_names_threads",
        ],
        containers: None,
    },
};

/// Host Get and its once-only native ports proposal keep the same canonical roles.
pub const HOST_GET: Declaration<Source> = Declaration {
    operation: "HostGet",
    bindings: &HOST.bindings::<7>(TargetResolver::Path("id")),
};
