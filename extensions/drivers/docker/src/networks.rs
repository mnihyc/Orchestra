//! Gateway-only managed networks.
//!
//! Every unit is put on a network of its own, cut from one pool of the operator's
//! ([`crate::config::NetworkNames::pool`], a /28 each as shipped): an open unit on a bridge from which it reaches the
//! gateway and the internet, a restricted unit (and one whose `network.mode` is `none`) on an internal one whose only
//! other member is its proxy, and that proxy goes out on a network of its own as well ([`crate::restricted`]). So a
//! hook of the operator's sees one stable subnet for each unit and can route it as it likes; no two units share a
//! bridge, and the daemon keeps traffic between two of its bridges apart, so units cannot reach each other; and the
//! daemon's own address pools and configuration stay as they are, since nothing is taken from them. A unit's networks
//! are made with the unit and removed with it.
//!
//! Every unit that is not restricted gets the host entry `gateway.orch.internal`, which is how the connector finds
//! the gateway. A network of the name that exists and does not carry the driver's label for what it is for is
//! somebody else's and is never used. A unit made before units had networks of their own stands on one of the two
//! common bridges (`orch-env` for environments, `orch-ctl` for providers' runtimes): it is moved to a network of its
//! own at its next start, connected there and disconnected from the bridge, and its container, with everything in
//! it, stays.

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;

use bollard::models::{
    ContainerInspectResponse, Ipam, IpamConfig, NetworkConnectRequest, NetworkCreateRequest,
    NetworkDisconnectRequest, NetworkInspect,
};
use ipnet::Ipv4Net;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::network::{UnitNetworkAddress, UnitNetworkRole};
use orch_driver_contract::runtime::{LABEL_RUNTIME_ID, LABEL_SYSTEM, RuntimeKind};
use orch_types::Id;

use crate::client::{Daemon, map_error, status_of};
use crate::config::DockerConfig;
use crate::units::driver_labels;

/// The name a unit finds the gateway by.
pub const GATEWAY_NAME: &str = "gateway.orch.internal";

/// The label that says what a network of the driver is for: `unit` for an open unit's own network, `egress` for
/// the way out of a restricted unit's proxy, `restricted` for a restricted unit's own network.
pub const LABEL_NETWORK: &str = "orch.network";

/// What [`LABEL_NETWORK`] says of an open unit's own network.
pub const ROLE_UNIT: &str = "unit";

/// What [`LABEL_NETWORK`] says of the network a restricted unit's proxy goes out on.
pub const ROLE_EGRESS: &str = "egress";

/// The option of a bridge that lets containers on it talk to one another; `false` on a network with one unit.
const OPTION_ICC: &str = "com.docker.network.bridge.enable_icc";

/// The option of a bridge that decides whether the host has an address on it.
const GATEWAY_MODE: &str = "com.docker.network.bridge.gateway_mode_ipv4";

/// The gateway mode in which the bridge has no address on the host.
const ISOLATED: &str = "isolated";

/// How many free subnets of the pool are tried when making a network races another client of the daemon: two
/// units made at the same moment each take another, and sixteen leaves room for a burst of them.
const SUBNET_ATTEMPTS: usize = 16;

/// The name of a unit's own network, open or restricted.
#[must_use]
pub fn unit_network_name(unit: &str) -> String {
    format!("{unit}-net")
}

/// The name of the network a restricted unit's proxy goes out on.
#[must_use]
pub fn egress_network_name(unit: &str) -> String {
    format!("{unit}-out")
}

/// Which network a unit is put on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPlan {
    /// The network's name.
    pub name: String,
    /// What it is for, as its label says.
    pub role: String,
}

/// The common bridge units of the pool `kind` stood on before every unit had a network of its own; a unit still on
/// it is moved off it at its next start.
#[must_use]
pub fn plan(config: &DockerConfig, kind: RuntimeKind) -> NetworkPlan {
    let (base, role) = match kind {
        RuntimeKind::Environment => (&config.networks.env, "env"),
        RuntimeKind::Provider => (&config.networks.ctl, "ctl"),
    };
    NetworkPlan {
        name: base.clone(),
        role: role.to_owned(),
    }
}

/// A network of a unit's own as made: its name, its subnet and the host's address on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Own {
    /// The network's name.
    pub name: String,
    /// Its subnet.
    pub subnet: Ipv4Net,
    /// The host's address on it, the gateway of what goes out of it.
    pub gateway: Option<Ipv4Addr>,
}

/// The labels of a network of a unit's own: the driver's, what the network is for, and the system and runtime of the
/// unit, by which the network is told apart from anybody else's.
#[must_use]
pub fn labels_for(
    config: &DockerConfig,
    role: &str,
    system: Option<&str>,
    runtime: Option<Id>,
) -> HashMap<String, String> {
    let mut labels: HashMap<String, String> = driver_labels(config).into_iter().collect();
    labels.insert(LABEL_NETWORK.to_owned(), role.to_owned());
    if let Some(system) = system {
        labels.insert(LABEL_SYSTEM.to_owned(), system.to_owned());
    }
    if let Some(runtime) = runtime {
        labels.insert(LABEL_RUNTIME_ID.to_owned(), runtime.to_string());
    }
    labels
}

/// The request that makes an open network of a unit's own at `subnet`: a bridge with a way out, the host at its first
/// address, and no traffic between its members (it has one, but a member added by hand gains nothing).
#[must_use]
pub fn open_request(
    name: &str,
    role: &str,
    subnet: Ipv4Net,
    config: &DockerConfig,
    system: Option<&str>,
    runtime: Option<Id>,
) -> NetworkCreateRequest {
    NetworkCreateRequest {
        name: name.to_owned(),
        driver: Some("bridge".to_owned()),
        internal: Some(false),
        attachable: Some(false),
        enable_ipv6: Some(false),
        ipam: Some(Ipam {
            driver: Some("default".to_owned()),
            config: Some(vec![IpamConfig {
                subnet: Some(subnet.to_string()),
                gateway: subnet.hosts().next().map(|address| address.to_string()),
                ..IpamConfig::default()
            }]),
            options: None,
        }),
        options: Some(HashMap::from([(OPTION_ICC.to_owned(), "false".to_owned())])),
        labels: Some(labels_for(config, role, system, runtime)),
        ..NetworkCreateRequest::default()
    }
}

/// The IPv4 subnet of a network and the gateway its address management gives it.
fn subnet_of(found: &NetworkInspect) -> Option<(Ipv4Net, Option<Ipv4Addr>)> {
    found
        .ipam
        .as_ref()
        .and_then(|ipam| ipam.config.as_ref())
        .and_then(|configs| {
            configs.iter().find_map(|config| {
                let subnet = config.subnet.as_deref()?.parse::<Ipv4Net>().ok()?;
                let gateway = config
                    .gateway
                    .as_deref()
                    .and_then(|gateway| gateway.parse::<Ipv4Addr>().ok());
                Some((subnet, gateway))
            })
        })
}

/// The network as found, when it is an open network of the driver's for `role`: labelled so, with a way out and an
/// IPv4 subnet.
fn open_as_made(found: &NetworkInspect, name: &str, role: &str) -> Result<Own, DriverError> {
    let ours = found
        .labels
        .as_ref()
        .is_some_and(|labels| labels.get(LABEL_NETWORK).map(String::as_str) == Some(role));
    if !ours {
        return Err(DriverError::Internal(format!(
            "a network named {name} exists and is not one of the driver's; name the driver's networks otherwise"
        )));
    }
    if found.internal.unwrap_or(false) {
        return Err(DriverError::Internal(format!(
            "the network {name} was made internal; an open unit's network has a way out"
        )));
    }
    let (subnet, gateway) = subnet_of(found)
        .ok_or_else(|| DriverError::Internal(format!("the network {name} has no IPv4 subnet")))?;
    Ok(Own {
        name: name.to_owned(),
        subnet,
        gateway,
    })
}

/// Makes the network `name` at a free subnet of the pool, unless it exists: `request` says how it is made at a
/// subnet, `read` what it must be to be used. The subnets begin at the place `seed` gives (so that two runtimes
/// seldom try one at once), skip every subnet any network of the daemon has, and a subnet taken meanwhile by somebody
/// else is passed for the next. A network that is made and is not as it must be is removed again.
pub(crate) async fn cut_from_pool<T, M, R>(
    daemon: &Daemon,
    config: &DockerConfig,
    name: &str,
    seed: u128,
    request: M,
    read: R,
) -> Result<T, DriverError>
where
    M: Fn(Ipv4Net) -> NetworkCreateRequest + Send + Sync,
    R: Fn(&NetworkInspect, &str) -> Result<T, DriverError> + Send + Sync,
    T: Send,
{
    if let Some(found) = daemon.network(name).await? {
        return read(&found, name);
    }
    let pool = config.networks.pool().ok_or_else(|| {
        DriverError::Unsupported(format!(
            "the pool {:?} of the units' networks cannot hold one of /{}",
            config.networks.pool, config.networks.prefix
        ))
    })?;
    for attempt in 0..SUBNET_ATTEMPTS {
        let taken = crate::restricted::taken_subnets(daemon).await?;
        let Some(subnet) =
            crate::restricted::subnet_choice(pool, config.networks.prefix, seed, &taken, attempt)
        else {
            break;
        };
        match daemon.docker().create_network(request(subnet)).await {
            Ok(_) => {
                let found = daemon.network(name).await?.ok_or_else(|| {
                    DriverError::Internal(format!("the network {name} vanished after it was made"))
                })?;
                return match read(&found, name) {
                    Ok(made) => {
                        tracing::info!(network = %name, %subnet, "made a unit's network from the pool");
                        Ok(made)
                    }
                    Err(error) => {
                        if let Err(removal) = daemon.docker().remove_network(name).await {
                            tracing::warn!(network = %name, %removal, "a network not as it must be could not be removed");
                        }
                        Err(error)
                    }
                };
            }
            // The name was taken meanwhile: by a command for the same unit, whose network it is.
            Err(error) if status_of(&error) == Some(409) => {
                if let Some(found) = daemon.network(name).await? {
                    return read(&found, name);
                }
                return Err(map_error(error));
            }
            // Another network took the subnet meanwhile: the next one.
            Err(error) if error.to_string().to_ascii_lowercase().contains("overlap") => {}
            Err(error) => return Err(map_error(error)),
        }
    }
    Err(DriverError::Unavailable(format!(
        "no subnet of {pool} is free for the network {name}"
    )))
}

/// Makes sure the open network `name` of a unit, for `role`, exists and is the driver's; makes it from the pool when
/// it does not.
pub async fn ensure_open(
    daemon: &Daemon,
    config: &DockerConfig,
    name: &str,
    role: &str,
    runtime: Id,
    system: Option<&str>,
) -> Result<Own, DriverError> {
    cut_from_pool(
        daemon,
        config,
        name,
        runtime.as_u128(),
        |subnet| open_request(name, role, subnet, config, system, Some(runtime)),
        |found, name| open_as_made(found, name, role),
    )
    .await
}

/// Removes a network of a unit's own; one that is not there is removed already.
pub async fn remove(daemon: &Daemon, name: &str) -> Result<(), DriverError> {
    match daemon.docker().remove_network(name).await {
        Ok(()) => Ok(()),
        Err(error) if status_of(&error) == Some(404) => Ok(()),
        Err(error) => Err(map_error(error)),
    }
}

/// The networks a container stands on, with its address on each; `None` where it has none now (it does not run).
#[must_use]
pub fn endpoints_of(container: &ContainerInspectResponse) -> BTreeMap<String, Option<String>> {
    container
        .network_settings
        .as_ref()
        .and_then(|settings| settings.networks.as_ref())
        .map(|networks| {
            networks
                .iter()
                .map(|(name, endpoint)| {
                    (
                        name.clone(),
                        endpoint
                            .ip_address
                            .clone()
                            .filter(|address| !address.is_empty()),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The common bridge a unit made before units had networks of their own still stands on, if it does.
#[must_use]
pub fn on_common_bridge(
    container: &ContainerInspectResponse,
    config: &DockerConfig,
) -> Option<String> {
    let endpoints = endpoints_of(container);
    [&config.networks.env, &config.networks.ctl]
        .into_iter()
        .find(|bridge| endpoints.contains_key(bridge.as_str()))
        .cloned()
}

/// Puts a container on `network`; one already on it is as it should be.
pub async fn connect(daemon: &Daemon, container: &str, network: &str) -> Result<(), DriverError> {
    match daemon
        .docker()
        .connect_network(
            network,
            NetworkConnectRequest {
                container: container.to_owned(),
                endpoint_config: None,
            },
        )
        .await
    {
        Ok(()) => Ok(()),
        Err(error) if error.to_string().contains("already exists") => Ok(()),
        Err(error) => Err(map_error(error)),
    }
}

/// Puts a container on `network` and takes it off `bridge`, in that order, so that a failure between the two leaves
/// it with a way to the gateway and the next attempt finishes it. A container already on `network`, or no longer on
/// `bridge`, is as it should be.
pub async fn move_off(
    daemon: &Daemon,
    container: &str,
    network: &str,
    bridge: &str,
) -> Result<(), DriverError> {
    connect(daemon, container, network).await?;
    match daemon
        .docker()
        .disconnect_network(
            bridge,
            NetworkDisconnectRequest {
                container: container.to_owned(),
                force: Some(true),
            },
        )
        .await
    {
        Ok(()) => Ok(()),
        Err(error) if matches!(status_of(&error), Some(404)) => Ok(()),
        Err(error) if error.to_string().contains("is not connected") => Ok(()),
        Err(error) => Err(map_error(error)),
    }
}

/// What one network of the daemon is to a unit: its subnet, the host's address on it (none where its bridge has none
/// on the host), and the address `address` there.
async fn describe(
    daemon: &Daemon,
    name: &str,
    role: UnitNetworkRole,
    address: Option<String>,
) -> Option<UnitNetworkAddress> {
    let found = daemon.network(name).await.ok().flatten()?;
    let (subnet, gateway) = subnet_of(&found)?;
    let isolated = found
        .options
        .as_ref()
        .is_some_and(|options| options.get(GATEWAY_MODE).map(String::as_str) == Some(ISOLATED));
    Some(UnitNetworkAddress {
        name: name.to_owned(),
        role,
        subnet: subnet.to_string(),
        gateway: if isolated {
            None
        } else {
            gateway.map(|gateway| gateway.to_string())
        },
        address,
    })
}

/// The networks `container`, the unit named `unit`, stands on, and for a restricted unit the network its proxy goes
/// out on, each with its subnet, gateway and their address: what the operator's hooks and the environment's facts are
/// told. A network the daemon no longer describes is left out.
pub async fn addresses(
    daemon: &Daemon,
    unit: &str,
    container: &ContainerInspectResponse,
) -> Vec<UnitNetworkAddress> {
    let mut found = Vec::new();
    for (name, address) in endpoints_of(container) {
        if let Some(network) = describe(daemon, &name, UnitNetworkRole::Unit, address).await {
            found.push(network);
        }
    }
    let own = unit_network_name(unit);
    if let Ok(Some(proxy)) = daemon.container(&crate::restricted::proxy_name(unit)).await {
        for (name, address) in endpoints_of(&proxy) {
            if name == own {
                continue;
            }
            if let Some(network) = describe(daemon, &name, UnitNetworkRole::Egress, address).await {
                found.push(network);
            }
        }
    }
    found
}

/// The host entry that makes `gateway.orch.internal` resolve to the gateway: the configured target as it is
/// (`host-gateway` the daemon turns into the address of the host).
#[must_use]
pub fn gateway_entry(config: &DockerConfig) -> String {
    format!("{GATEWAY_NAME}:{}", config.gateway_host)
}

impl crate::driver::DockerDriver {
    /// Moves an open unit that stands on a common bridge, and does not run, to a network of its own from the pool,
    /// keeping its container and everything in it; answers the container as it is then. A running unit is left
    /// where it is until its next start, since moving it would cut what it has open.
    pub(crate) async fn settle_own_network(
        &self,
        unit: &orch_driver_contract::units::UnitRef,
        container: ContainerInspectResponse,
    ) -> Result<ContainerInspectResponse, DriverError> {
        let Some(bridge) = on_common_bridge(&container, &self.config) else {
            return Ok(container);
        };
        if container.state.as_ref().and_then(|state| state.running) == Some(true) {
            return Ok(container);
        }
        let labels: BTreeMap<String, String> = container
            .config
            .as_ref()
            .and_then(|config| config.labels.clone())
            .unwrap_or_default()
            .into_iter()
            .collect();
        let own = ensure_open(
            &self.daemon,
            &self.config,
            &unit_network_name(&unit.name),
            ROLE_UNIT,
            unit.runtime_id,
            labels.get(LABEL_SYSTEM).map(String::as_str),
        )
        .await?;
        let id = container.id.clone().unwrap_or_else(|| unit.name.clone());
        move_off(&self.daemon, &id, &own.name, &bridge).await?;
        tracing::info!(unit = %unit.name, %bridge, network = %own.name, "moved a unit off a common bridge onto a network of its own");
        self.daemon.container(&id).await?.ok_or_else(|| {
            DriverError::NotFound(format!("the unit {} went while it was moved", unit.name))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{EndpointSettings, NetworkSettings};

    #[test]
    fn each_pool_had_a_common_bridge_whose_units_are_moved_off_it() {
        let config = DockerConfig::default();
        let env = plan(&config, RuntimeKind::Environment);
        assert_eq!((env.name.as_str(), env.role.as_str()), ("orch-env", "env"));
        let ctl = plan(&config, RuntimeKind::Provider);
        assert_eq!((ctl.name.as_str(), ctl.role.as_str()), ("orch-ctl", "ctl"));
    }

    #[test]
    fn a_units_own_network_is_cut_from_the_pool_with_the_host_at_its_first_address_and_the_drivers_labels()
     {
        let mut config = DockerConfig::default();
        config.labels.insert("orch.test".into(), "abc".into());
        let runtime = orch_types::new_id();
        let subnet: Ipv4Net = "10.199.0.32/28".parse().unwrap();
        let name = unit_network_name("orch-environment-0199abcd1234");
        assert_eq!(name, "orch-environment-0199abcd1234-net");
        let request = open_request(
            &name,
            ROLE_UNIT,
            subnet,
            &config,
            Some("sys"),
            Some(runtime),
        );
        assert_eq!(request.driver.as_deref(), Some("bridge"));
        assert_eq!(request.internal, Some(false));
        assert_eq!(request.options.unwrap()[OPTION_ICC], "false");
        let ipam = request.ipam.unwrap().config.unwrap();
        assert_eq!(ipam[0].subnet.as_deref(), Some("10.199.0.32/28"));
        assert_eq!(ipam[0].gateway.as_deref(), Some("10.199.0.33"));
        let labels = request.labels.unwrap();
        assert_eq!(labels[LABEL_NETWORK], ROLE_UNIT);
        assert_eq!(labels["orch.driver"], "docker");
        assert_eq!(labels["orch.test"], "abc");
        assert_eq!(labels[LABEL_SYSTEM], "sys");
        assert_eq!(labels[LABEL_RUNTIME_ID], runtime.to_string());
        assert_eq!(
            egress_network_name("orch-environment-0199abcd1234"),
            "orch-environment-0199abcd1234-out"
        );
    }

    fn inspected(labels: &[(&str, &str)], internal: bool, subnet: Option<&str>) -> NetworkInspect {
        NetworkInspect {
            labels: Some(
                labels
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect(),
            ),
            internal: Some(internal),
            ipam: Some(Ipam {
                driver: None,
                config: Some(vec![IpamConfig {
                    subnet: subnet.map(str::to_owned),
                    gateway: subnet.map(|_| "10.199.0.33".to_owned()),
                    ..IpamConfig::default()
                }]),
                options: None,
            }),
            ..NetworkInspect::default()
        }
    }

    #[test]
    fn a_network_of_the_name_is_used_only_when_it_is_the_drivers_for_that_purpose_and_has_a_way_out()
     {
        let ours = inspected(&[(LABEL_NETWORK, ROLE_UNIT)], false, Some("10.199.0.32/28"));
        let own = open_as_made(&ours, "n", ROLE_UNIT).unwrap();
        assert_eq!(own.subnet.to_string(), "10.199.0.32/28");
        assert_eq!(own.gateway, Some("10.199.0.33".parse().unwrap()));
        assert!(
            open_as_made(&ours, "n", ROLE_EGRESS).is_err(),
            "a unit's network is no proxy's way out"
        );
        let foreign = inspected(&[], false, Some("10.199.0.32/28"));
        assert!(
            matches!(open_as_made(&foreign, "n", ROLE_UNIT), Err(DriverError::Internal(ref why)) if why.contains("is not one of the driver's"))
        );
        let sealed = inspected(&[(LABEL_NETWORK, ROLE_UNIT)], true, Some("10.199.0.32/28"));
        assert!(open_as_made(&sealed, "n", ROLE_UNIT).is_err());
        let bare = inspected(&[(LABEL_NETWORK, ROLE_UNIT)], false, None);
        assert!(open_as_made(&bare, "n", ROLE_UNIT).is_err());
    }

    fn on(networks: &[(&str, &str)]) -> ContainerInspectResponse {
        ContainerInspectResponse {
            network_settings: Some(NetworkSettings {
                networks: Some(
                    networks
                        .iter()
                        .map(|(name, address)| {
                            (
                                (*name).to_owned(),
                                EndpointSettings {
                                    ip_address: Some((*address).to_owned()),
                                    ..EndpointSettings::default()
                                },
                            )
                        })
                        .collect(),
                ),
                ..NetworkSettings::default()
            }),
            ..ContainerInspectResponse::default()
        }
    }

    #[test]
    fn a_unit_on_a_common_bridge_is_found_and_one_on_its_own_network_is_not() {
        let config = DockerConfig::default();
        assert_eq!(
            on_common_bridge(&on(&[("orch-env", "172.20.0.5")]), &config).as_deref(),
            Some("orch-env")
        );
        assert_eq!(
            on_common_bridge(&on(&[("orch-ctl", "")]), &config).as_deref(),
            Some("orch-ctl")
        );
        assert_eq!(
            on_common_bridge(
                &on(&[("orch-environment-0199abcd1234-net", "10.199.0.34")]),
                &config
            ),
            None
        );
        let stopped = endpoints_of(&on(&[("orch-environment-0199abcd1234-net", "")]));
        assert_eq!(
            stopped["orch-environment-0199abcd1234-net"], None,
            "a stopped unit has no address, and says so rather than an empty one"
        );
    }

    #[test]
    fn the_gateway_is_the_configured_host() {
        assert_eq!(
            gateway_entry(&DockerConfig::default()),
            "gateway.orch.internal:host-gateway"
        );
        let fixed = DockerConfig {
            gateway_host: "10.0.0.7".into(),
            ..DockerConfig::default()
        };
        assert_eq!(gateway_entry(&fixed), "gateway.orch.internal:10.0.0.7");
    }
}
