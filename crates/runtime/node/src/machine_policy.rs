//! Canonical machine policy read from the connector configuration.

use crate::settings::{NodeSettings, StorageSetting};
use orch_connector_core::{
    enrollment::{DataDir, EnrollError},
    policy::{PolicyFault, PolicyStore},
};
use orch_link_contract::policy::MachinePorts;

/// Opening the machine's single policy file failed.
#[derive(Debug, thiserror::Error)]
pub enum PolicyOpenError {
    /// The data directory or configuration could not be read.
    #[error("{0}")]
    Data(#[from] EnrollError),
    /// A policy value could not be validated or persisted.
    #[error("{0}")]
    Policy(#[from] PolicyFault),
}

/// Opens the single canonical machine policy without importing another configuration.
pub fn open(settings: &NodeSettings) -> Result<PolicyStore, PolicyOpenError> {
    let data = DataDir::open(&settings.data_dir)?;
    Ok(PolicyStore::host(
        data.config(|_| None)?,
        Some(data.config_path()),
    )?)
}

/// Driver setup and storage facts take their paths from the effective machine policy.
pub fn configure(settings: &mut NodeSettings) -> Result<(), PolicyOpenError> {
    if let Some(policy) = open(settings)?.policy() {
        if !policy.drivers.allowed.iter().any(|driver| driver == "*") {
            settings
                .external_drivers
                .retain(|driver| policy.drivers.offers(driver));
            settings.drivers = policy
                .drivers
                .allowed
                .iter()
                .filter(|driver| !settings.external_drivers.contains(driver))
                .cloned()
                .collect();
        }
        settings.driver_configurations = policy.drivers.configurations.clone();
        settings.storages = policy
            .storages
            .iter()
            .map(|storage| StorageSetting {
                name: storage.name.clone(),
                path: storage.path.clone().into(),
                default: storage.default,
            })
            .collect();
        settings.ports = limits(&policy.ports);
        settings.hooks = crate::hooks::HookSettings::from_policy(&policy.hooks);
    }
    Ok(())
}

/// Driver connection settings are separate from live permission rules in MachineDrivers.
#[must_use]
pub fn docker_options(settings: &orch_config::server::NodeDocker) -> orch_types::Json {
    serde_json::json!(settings)
}

/// Host connection settings use the same canonical operational representation.
#[must_use]
pub fn host_options(settings: &orch_config::server::NodeHost) -> orch_types::Json {
    serde_json::json!(settings)
}

/// Optional operator constraints for checking a proposed declaration; no parallel orch.toml read.
#[must_use]
pub fn limits(ports: &MachinePorts) -> orch_config::server::Ports {
    orch_connector_core::host_ports::policy_limits(ports)
}

/// Existing listeners are held to the current declaration and its audience, including withdrawal.
#[must_use]
pub fn listeners(ports: &MachinePorts) -> orch_config::server::Ports {
    orch_connector_core::host_ports::policy_listeners(ports)
}

#[cfg(test)]
mod tests {
    use orch_link_contract::policy::PolicyKey;
    #[test]
    fn opening_the_canonical_policy_never_imports_settings_or_rewrites_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let settings = NodeSettings::in_dir(directory.path());
        let data = DataDir::open(&settings.data_dir).unwrap();
        let store =
            PolicyStore::host(data.config(|_| None).unwrap(), Some(data.config_path())).unwrap();
        store
            .set_here(
                PolicyKey::Storages,
                &serde_json::json!([{"name":"kept","path":"/data","default":true}]),
            )
            .unwrap();
        let before = std::fs::read(data.config_path()).unwrap();
        let policy = open(&settings).unwrap().policy().unwrap();
        assert_eq!(policy.storages[0].name, "kept");
        assert_eq!(std::fs::read(data.config_path()).unwrap(), before);
    }

    use super::*;

    #[test]
    fn saving_local_and_handed_driver_options_never_drops_their_opaque_nulls() {
        let mut config = orch_config::connector::ConnectorConfig::default();
        let local = serde_json::json!({"unset":null,"nested":[null,{"unset":null}]});
        let handed = serde_json::json!({"source":"server","unset":null});
        config.drivers.allowed = vec!["cluster-test".into()];
        config
            .drivers
            .configurations
            .insert("cluster-test".into(), local.clone());
        config.policy.values.insert("drivers".into(), serde_json::json!({"allowed":["cluster-test"],"configurations":{"cluster-test":handed}}));
        let written =
            orch_connector_core::policy::sparse_config(&config, &["drivers".into()]).unwrap();
        let reloaded: orch_config::connector::ConnectorConfig = toml::from_str(&written).unwrap();
        assert_eq!(reloaded.drivers.allowed, ["cluster-test"]);
        assert_eq!(reloaded.drivers.configurations["cluster-test"], local);
        assert_eq!(
            reloaded.policy.values["drivers"]["configurations"]["cluster-test"],
            handed
        );
    }
}
