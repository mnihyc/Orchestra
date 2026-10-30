//! Driver-neutral inventory and exact removal of installation-owned objects.

use std::collections::BTreeMap;

use orch_types::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The kind of an object held on a node, independent of its runtime implementation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// An execution unit, including an owned helper.
    Unit,
    /// Persistent owned storage.
    Volume,
    /// An owned network.
    Network,
    /// An owned image made available for a unit.
    Image,
    /// Files retained from an earlier unit.
    KeptLayer,
    /// An owned build result or cached build object.
    Build,
    /// One verified tool version in the node's cache.
    ToolVersion,
}

/// An opaque identity within one inventory; callers never interpret its identifier.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct ResourceRef {
    /// The object's kind.
    pub kind: ResourceKind,
    /// Its exact opaque identifier, never a pattern or a path supplied by a caller.
    pub id: String,
}

/// One object and the dependencies that keep other objects in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Resource {
    /// Its identity in this inventory.
    pub resource: ResourceRef,
    /// A short display name.
    pub name: String,
    /// Its measured bytes; unknown is never represented as zero.
    pub size_bytes: Option<i64>,
    /// Its actual ownership and attribution labels.
    pub labels: BTreeMap<String, String>,
    /// Objects this object depends on, including while it is stopped.
    pub depends_on: Vec<ResourceRef>,
    /// Other immutable content identities that a successful ensure may report for this object.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Last observed use, where known.
    pub last_used_at: Option<Time>,
    /// Whether native use or a protected dependency prevents removal.
    pub busy: bool,
}

/// A driver whose inventory could not be read; its absence cannot prove that a cache is unused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceUnavailable {
    /// The configured driver or local cache source.
    pub driver: String,
    /// The safe reason it could not be read.
    pub reason: String,
}

/// A node's complete inventory, with unavailable parts stated explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceInventory {
    /// When the inventory was observed.
    pub observed_at: Time,
    /// Every object found, with identifiers scoped to this node.
    pub resources: Vec<Resource>,
    /// Parts whose contents remain unknown.
    pub unavailable: Vec<ResourceUnavailable>,
}
