//! What the node is configured with: its data directory, the drivers it loads, its storages and its intervals.
//!
//! The operator's `[node]` table of `orch.toml` ([`orch_config::server::Node`]) is read once into
//! [`NodeSettings`], with the durations as durations, so that nothing else in the node handles a bare number of
//! seconds. Where the node keeps its own files is fixed here: the tool cache under `tools/`, and the
//! connector that new units are given at `tools/orch-connector/current`, which points at the version the system
//! named last.

use std::path::{Path, PathBuf};
use std::time::Duration;

use orch_config::ServerConfig;
use orch_config::server::NodeDocker;

/// The name of the tool that is the connector's own program. The system names it in `node.tool.ensure` like any
/// other tool, and every unit is started with the version that was named last.
pub const CONNECTOR_TOOL: &str = "orch-connector";

/// The directory of the tool cache in the node's data directory.
pub const TOOLS_DIR: &str = "tools";

/// The link, in the directory of the connector tool, to the version that new units are given.
pub const CURRENT_LINK: &str = "current";

/// A named place on the host where volumes are made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSetting {
    /// Its name, which a template or a setting names.
    pub name: String,
    /// The directory below which the data of its volumes lives.
    pub path: PathBuf,
    /// Whether it is where a volume goes that names no storage.
    pub default: bool,
}

/// What an operator configured for the node.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSettings {
    /// Where the node keeps its identity, its spool, its ledger and its tool cache.
    pub data_dir: PathBuf,
    /// The gateway it enrolls at when it is not told another address.
    pub gateway_url: String,
    /// An explicit address for this node's units; empty follows the node's own list.
    pub managed_endpoint: String,
    /// The built-in drivers it loads.
    pub drivers: Vec<String>,
    /// The names of the external drivers the operator registered.
    pub external_drivers: Vec<String>,
    /// How long a new runtime's connector has to enroll before its creation fails.
    pub enroll_timeout: Duration,
    /// How often the node compares its units with what the system says.
    pub reconcile_every: Duration,
    /// Time between asking a unit to stop and killing it.
    pub stop_grace: Duration,
    /// How often the node looks up the digest behind each image tag in use.
    pub image_check_every: Duration,
    /// The named places for volumes; none means the one the driver keeps its volumes in.
    pub storages: Vec<StorageSetting>,
    /// The Docker driver's own settings, which the program gives the driver of that name.
    pub docker: NodeDocker,
    /// Operator-owned host-process driver configuration.
    pub host: orch_config::server::NodeHost,
    /// Driver-owned connection values captured when drivers are opened.
    pub driver_configurations: std::collections::BTreeMap<String, orch_types::Json>,
    /// What the system is called, which is the value of `orch.system` until the node is given another.
    pub system_name: String,
    /// The operator's ceiling for host ports (`[ports]`), which no page of the host can widen.
    pub ports: orch_config::server::Ports,
    /// The previous configuration file, read only for the one-time local-policy import.
    pub config_file: Option<PathBuf>,
    /// Initial hooks for a standalone node; a running connector supplies its live machine policy.
    pub hooks: crate::hooks::HookSettings,
}

/// Seconds in an hour: `image_check_hours` is written in hours.
const SECS_PER_HOUR: u64 = 3_600;

impl NodeSettings {
    /// The settings of the `[node]` table, and the system's name from `[system]`.
    #[must_use]
    pub fn from_config(config: &ServerConfig) -> Self {
        let node = &config.node;
        Self {
            data_dir: PathBuf::from(&node.data_dir),
            gateway_url: node.gateway_url.clone(),
            managed_endpoint: config.gateway.managed_endpoint.clone(),
            drivers: node.drivers.clone(),
            external_drivers: config
                .plugin
                .iter()
                .filter(|plugin| plugin.kind == "driver")
                .map(|plugin| plugin.name.clone())
                .collect(),
            enroll_timeout: Duration::from_secs(node.enroll_timeout_secs),
            reconcile_every: Duration::from_secs(node.reconcile_secs.max(1)),
            stop_grace: Duration::from_secs(node.stop_grace_secs),
            image_check_every: Duration::from_secs(
                node.image_check_hours.saturating_mul(SECS_PER_HOUR),
            ),
            storages: node
                .storage
                .iter()
                .map(|storage| StorageSetting {
                    name: storage.name.clone(),
                    path: PathBuf::from(&storage.path),
                    default: storage.default,
                })
                .collect(),
            docker: node.docker.clone(),
            host: node.host.clone(),
            driver_configurations: std::collections::BTreeMap::new(),
            system_name: config.system.name.clone(),
            ports: config.ports.clone(),
            config_file: None,
            hooks: crate::hooks::HookSettings::from_policy(&node.hooks),
        }
    }

    /// The initial imported ceiling. Running roles read their live PolicyStore instead.
    #[must_use]
    pub fn ports_now(&self) -> orch_config::server::Ports {
        self.ports.clone()
    }

    /// Settings for a node whose files are in `data_dir`, with the defaults of everything else.
    #[must_use]
    pub fn in_dir(data_dir: &Path) -> Self {
        let mut settings = Self::from_config(&ServerConfig::default());
        settings.data_dir = data_dir.to_owned();
        settings
    }

    /// The tool cache.
    #[must_use]
    pub fn tools_dir(&self) -> PathBuf {
        self.data_dir.join(TOOLS_DIR)
    }

    /// The directory that holds `orch-connector`, which every new unit is given: a link to the version the
    /// system named last. The driver resolves it when it makes a unit, so a unit stays on the build it was made
    /// with.
    #[must_use]
    pub fn connector_dir(&self) -> PathBuf {
        self.tools_dir().join(CONNECTOR_TOOL).join(CURRENT_LINK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shipped_node_never_schedules_registry_lookups() {
        assert!(
            NodeSettings::from_config(&ServerConfig::default())
                .image_check_every
                .is_zero()
        );
    }

    #[test]
    fn the_node_table_is_read_with_its_durations_as_durations() {
        let mut config = ServerConfig::default();
        config.node.data_dir = "/srv/node".to_owned();
        config.node.image_check_hours = 2;
        config.node.stop_grace_secs = 7;
        config.node.storage = vec![orch_config::server::NodeStorage {
            name: "fast".to_owned(),
            path: "/mnt/fast".to_owned(),
            default: true,
        }];
        let settings = NodeSettings::from_config(&config);
        assert_eq!(settings.image_check_every, Duration::from_secs(7_200));
        assert_eq!(settings.stop_grace, Duration::from_secs(7));
        assert_eq!(settings.storages[0].path, PathBuf::from("/mnt/fast"));
        assert_eq!(
            settings.connector_dir(),
            PathBuf::from("/srv/node/tools/orch-connector/current")
        );
        assert_eq!(settings.enroll_timeout, Duration::from_secs(120));
    }

    #[test]
    fn only_the_plugins_of_kind_driver_are_external_drivers() {
        let mut config = ServerConfig::default();
        let plugin = |kind: &str, name: &str| orch_config::server::Plugin {
            kind: kind.to_owned(),
            name: name.to_owned(),
            ..orch_config::server::Plugin::default()
        };
        config.plugin = vec![plugin("driver", "firecracker"), plugin("notifier", "sms")];
        assert_eq!(
            NodeSettings::from_config(&config).external_drivers,
            ["firecracker"]
        );
    }
}
