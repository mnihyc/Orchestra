//! Canonical machine policy, with a one-time import of legacy operator-owned node settings.

use crate::settings::{NodeSettings, StorageSetting};
use orch_connector_core::{
    enrollment::{DataDir, EnrollError},
    policy::{PolicyFault, PolicyStore},
};
use orch_link_contract::policy::{LocalPolicy, MachinePorts, PolicyDefaults, PolicyKey};

/// Import version two adds the documented hooks table to the original ports/storage/driver import.
const IMPORT_VERSION: u32 = 2;

/// Opening or importing the machine's single policy file failed.
#[derive(Debug, thiserror::Error)]
pub enum PolicyOpenError {
    /// A legacy configuration or imported driver options could not be decoded.
    #[error("drivers (node role): {0}")]
    Driver(String),
    /// The data directory or configuration could not be read.
    #[error("{0}")]
    Data(#[from] EnrollError),
    /// A policy value could not be validated or persisted.
    #[error("{0}")]
    Policy(#[from] PolicyFault),
}

/// Import legacy local declarations once; every later read and command uses config.toml alone.
pub fn open(settings: &NodeSettings) -> Result<PolicyStore, PolicyOpenError> {
    let data = DataDir::open(&settings.data_dir)?;
    let mut config = data.config(|_| None)?;
    let imported = config.connector.node_policy_imported;
    let import_version = config.connector.node_policy_import_version;
    // Before this policy existed, an enrolled node's environment/build switches were controlled
    // by Orch. A persisted local_keys marker identifies deliberate new-policy choices instead.
    let legacy_handover = !imported
        && data.is_enrolled()
        && std::fs::read_to_string(data.config_path())
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
            .and_then(|written| {
                written
                    .get("policy")
                    .and_then(|policy| policy.get("local_keys"))
                    .cloned()
            })
            .is_none();
    if legacy_handover {
        config.policy.local_keys = Some(Vec::new());
        config.connector.roles = vec!["node".into()];
    }
    let store = PolicyStore::host(config, Some(data.config_path()))?;
    if legacy_handover {
        store.delegate(&["*".into()])?;
        eprintln!(
            "moved: now `orch-node policy show`; this enrolled node keeps its previous Orch-controlled settings, with local ports, storage, drivers and hooks imported below"
        );
    }
    if !imported || import_version < IMPORT_VERSION {
        if let Some(path) = &settings.config_file {
            let text = std::fs::read_to_string(path)
                .map_err(|error| PolicyOpenError::Driver(error.to_string()))?;
            let written: toml::Value = toml::from_str(&text)
                .map_err(|error| PolicyOpenError::Driver(error.to_string()))?;
            let node = written.get("node");
            if !imported
                && node.is_some_and(|node| {
                    node.get("drivers").is_some()
                        || node.get("docker").is_some()
                        || node.get("host").is_some()
                })
                && !store
                    .policy()
                    .is_some_and(|policy| policy.local_values.contains_key("drivers"))
            {
                let mut drivers = orch_link_contract::policy::MachineDrivers::managed();
                drivers.allowed = settings
                    .drivers
                    .iter()
                    .chain(&settings.external_drivers)
                    .cloned()
                    .collect();
                drivers.images = orch_link_contract::policy::ImageRules {
                    images: settings.docker.allowed_images.clone(),
                    registries: settings.docker.allowed_registries.clone(),
                    capabilities: settings.docker.allowed_capabilities.clone(),
                    runtime_classes: settings.docker.runtime_classes.clone(),
                    privileged: settings.docker.allow_privileged,
                    builds: settings.docker.builds,
                    archives: settings.docker.allow_archives,
                };
                drivers.processes = orch_link_contract::policy::ProcessRules {
                    templates: settings.host.allowed_templates.clone(),
                    accounts: settings.host.allowed_accounts.clone(),
                    programs: settings.host.allowed_programs.clone(),
                };
                drivers
                    .configurations
                    .insert("docker".into(), docker_options(&settings.docker));
                drivers
                    .configurations
                    .insert("host".into(), host_options(&settings.host));
                keep_legacy(&store, PolicyKey::Drivers, &serde_json::json!(drivers))?;
            }
            if node.is_some_and(|node| node.get("hooks").is_some()) {
                import_hooks(&store, settings)?;
            }
        } else if settings.hooks.any() {
            import_hooks(&store, settings)?;
        }
        if !imported
            && (settings.ports.incoming_address.is_some()
                || settings.ports.listen.is_some()
                || settings.ports.ranges.is_some())
        {
            let mut ports = LocalPolicy::installed(PolicyDefaults::Managed).ports;
            ports.limits = Some(crate::host_ports::limits(&settings.ports));
            ports.publish = match (&settings.ports.incoming_address, &settings.ports.ranges) {
                (Some(address), Some(ranges)) if !ranges.is_empty() => {
                    Some(orch_api_contract::ports::HostPorts {
                        incoming_address: address.clone(),
                        listen: settings
                            .ports
                            .listen
                            .clone()
                            .unwrap_or_else(|| "all".into()),
                        ranges: ranges
                            .iter()
                            .map(|range| orch_api_contract::ports::PortRange {
                                from: range.from,
                                to: range.to,
                            })
                            .collect(),
                    })
                }
                _ => None,
            };
            if !store
                .policy()
                .is_some_and(|policy| policy.local_values.contains_key("ports"))
            {
                keep_legacy(&store, PolicyKey::Ports, &serde_json::json!(ports))?;
            }
        }
        if !imported
            && !settings.storages.is_empty()
            && !store
                .policy()
                .is_some_and(|policy| policy.local_values.contains_key("storages"))
        {
            let values: Vec<_> = settings
                .storages
                .iter()
                .map(|storage| orch_link_contract::policy::MachineStorage {
                    name: storage.name.clone(),
                    path: storage.path.to_string_lossy().into_owned(),
                    default: storage.default,
                })
                .collect();
            keep_legacy(&store, PolicyKey::Storages, &serde_json::json!(values))?;
        }
        data.update_config(|config| {
            config.connector.node_policy_imported = true;
            config.connector.node_policy_import_version = IMPORT_VERSION;
            if !data.is_enrolled() {
                config.connector.roles = vec!["node".into()];
            }
        })?;
        return Ok(PolicyStore::host(
            data.config(|_| None)?,
            Some(data.config_path()),
        )?);
    }
    Ok(store)
}

fn keep_legacy(
    store: &PolicyStore,
    key: PolicyKey,
    value: &orch_types::Json,
) -> Result<(), PolicyFault> {
    store.set_here(key, value)?;
    // Configuration is opened before the program installs its tracing subscriber; stderr is its service log.
    eprintln!(
        "moved: now `orch-node policy set {} …`; view the imported local value with `orch-node policy show`",
        key.as_str()
    );
    Ok(())
}

fn import_hooks(store: &PolicyStore, settings: &NodeSettings) -> Result<(), PolicyFault> {
    if !store
        .policy()
        .is_some_and(|policy| policy.local_values.contains_key("hooks"))
    {
        let hooks = orch_link_contract::policy::MachineHooks {
            pre_up: settings
                .hooks
                .pre_up
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            post_up: settings
                .hooks
                .post_up
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            pre_down: settings
                .hooks
                .pre_down
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            post_down: settings
                .hooks
                .post_down
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            timeout_secs: settings.hooks.timeout.as_secs(),
        };
        keep_legacy(store, PolicyKey::LifecycleHooks, &serde_json::json!(hooks))?;
    }
    Ok(())
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

/// Connection settings exclude permission switches: those have one live source in MachineDrivers.
#[must_use]
pub fn docker_options(settings: &orch_config::server::NodeDocker) -> orch_types::Json {
    let mut options = serde_json::json!(settings);
    if let Some(options) = options.as_object_mut() {
        for key in [
            "allowed_images",
            "allowed_registries",
            "allowed_capabilities",
            "runtime_classes",
            "allow_privileged",
            "builds",
            "allow_archives",
        ] {
            options.remove(key);
        }
    }
    options
}

/// Host connection settings likewise exclude its live template, account and program rules.
#[must_use]
pub fn host_options(settings: &orch_config::server::NodeHost) -> orch_types::Json {
    let mut options = serde_json::json!(settings);
    if let Some(options) = options.as_object_mut() {
        for key in ["allowed_templates", "allowed_accounts", "allowed_programs"] {
            options.remove(key);
        }
    }
    options
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

    #[test]
    fn a_legacy_port_and_storage_declaration_is_kept_once_and_is_the_reported_policy() {
        let directory = tempfile::tempdir().unwrap();
        let mut settings = NodeSettings::in_dir(directory.path());
        settings.ports = orch_config::server::Ports {
            incoming_address: Some("vm.example.test".into()),
            listen: Some("all".into()),
            ranges: Some(vec![orch_config::server::PortsRange {
                from: 41000,
                to: 41010,
            }]),
        };
        settings.storages = vec![StorageSetting {
            name: "fast".into(),
            path: directory.path().join("fast"),
            default: true,
        }];
        let policy = open(&settings).unwrap().policy().unwrap();
        assert!(policy.local_values.contains_key("ports"));
        assert!(policy.local_values.contains_key("storages"));
        assert_eq!(
            policy.reported()["ports"]["publish"]["incoming_address"],
            serde_json::json!("vm.example.test")
        );
        assert_eq!(policy.storages[0].name, "fast");
        settings.ports.incoming_address = Some("changed-in-the-old-file.example".into());
        assert_eq!(
            open(&settings).unwrap().policy().unwrap(),
            policy,
            "the old file is no second source after import"
        );
    }

    #[test]
    fn an_earlier_import_receives_missing_hooks_without_reimporting_other_keys() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("old-node.toml");
        std::fs::write(
            &file,
            "[node]\ndrivers=['docker']\n[node.hooks]\npre_up='/bin/true'\ntimeout_secs=17\n",
        )
        .unwrap();
        let config: orch_config::ServerConfig = orch_config::load_text(
            &std::fs::read_to_string(&file).unwrap(),
            "previous-release node",
            |_| None,
        )
        .unwrap();
        let mut settings = NodeSettings::from_config(&config);
        settings.data_dir = directory.path().join("data");
        settings.config_file = Some(file.clone());
        let data = DataDir::open(&settings.data_dir).unwrap();
        data.update_config(|config| {
            config.connector.node_policy_imported = true;
            config.policy.delegated = vec!["*".into()];
        })
        .unwrap();
        let policy = open(&settings).unwrap().policy().unwrap();
        assert_eq!(policy.hooks.pre_up.as_deref(), Some("/bin/true"));
        assert_eq!(policy.hooks.timeout_secs, 17);
        assert!(policy.local_values.contains_key("hooks"));
        assert!(
            !policy.local_values.contains_key("drivers"),
            "an earlier handover is preserved"
        );
        assert_eq!(
            data.config(|_| None)
                .unwrap()
                .connector
                .node_policy_import_version,
            IMPORT_VERSION
        );
        std::fs::write(file, "[node.hooks]\npre_up='/bin/false'\n").unwrap();
        settings.hooks.pre_up = Some("/bin/false".into());
        assert_eq!(
            open(&settings).unwrap().policy().unwrap(),
            policy,
            "legacy input is retired after import"
        );
    }
}
