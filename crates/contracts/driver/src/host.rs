//! Scheduling scope, runtime delivery and aggregate observations supplied by a driver.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whose resources a host reports, independent of where its node agent runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostScope {
    /// The machine running the node agent.
    Node,
    /// A pool whose placement and observations belong to its driver.
    Driver,
}

/// How immutable verified connector and tool builds reach a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolDelivery {
    /// The driver exposes the node's cache read-only at its chosen unit paths.
    HostDirectory,
    /// The driver distributes requested builds without exposing node paths to units.
    Driver,
}

/// How a unit reaches the trusted gateway URL in its launch inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GatewayAccess {
    /// The node's address is reachable, possibly through a driver-owned alias.
    NodeAddress,
    /// The driver establishes a route inside its own network.
    DriverNetwork,
}

/// Who makes an owned volume's export consistent while its unit is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VolumeExportConsistency {
    /// The driver takes a snapshot or performs its own safe quiescing and recovery.
    Driver,
}

/// Aggregate schedulable capacity and current usage for one administrative host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostResources {
    /// Logical processors available across healthy machines.
    pub cpus: i64,
    /// Schedulable processor time in thousandths of a processor, when it differs from the logical count.
    #[serde(default)]
    pub cpu_millis: Option<i64>,
    /// Memory available for scheduling in MiB.
    pub memory_mb: i64,
    /// Storage capacity in decimal GB, counted once per backing store.
    pub disk_gb: i64,
    /// Processor use in percent of one logical processor.
    #[serde(default)]
    pub cpu_percent: Option<f64>,
    /// Current memory use in MiB.
    #[serde(default)]
    pub memory_used_mb: Option<i64>,
    /// Current disk use in decimal GB.
    #[serde(default)]
    pub disk_used_gb: Option<i64>,
    /// Available disk in decimal GB.
    #[serde(default)]
    pub disk_free_gb: Option<i64>,
}
