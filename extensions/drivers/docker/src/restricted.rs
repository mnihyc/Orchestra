//! A restricted unit's own network and its proxy: the network made isolated for the unit alone, the proxy's container beside it, the orders the node gives the proxy, and its heartbeat.
//!
//! A unit whose template says `network.egress: restricted`, or `network.mode: none`, is put on a bridge network of
//! its own: internal (no route out), with the gateway mode `isolated` (the bridge has no address on the host, so no
//! service of the host is reachable on it), and with two members, the unit and its proxy. The proxy is a container
//! of the node's, made from the operator's trusted sidecar image with the connector build of the tool cache as its program
//! (`orch-connector proxy`), as the node's user, without any capability, with a read-only file system; it is the one
//! member that also goes out, on a network of its own cut from the same pool ([`crate::networks`]), and the only
//! listener the unit can reach. The unit is
//! given the proxy as its name server and as `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY`, loopback alone in
//! `NO_PROXY`, and none of the capabilities that could change its network (`NET_ADMIN`, `NET_RAW`, `SYS_ADMIN`).
//!
//! The node writes the proxy's orders (what it admits) and beats its heartbeat into a directory of its own for each
//! unit, which is bound into the proxy's container; the proxy writes its state back there. The network, the proxy
//! and the directory are made with the unit, started before it, stopped after it and removed with it. A unit that
//! was made before this, on the shared internal network every unit without a way out once stood on, is made again on
//! a network of its own the next time it is started, with everything else it had kept.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, EndpointIpamConfig, EndpointSettings,
    HostConfig, Ipam, IpamConfig, NetworkConnectRequest, NetworkCreateRequest, NetworkInspect,
    NetworkingConfig, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use ipnet::{IpNet, Ipv4Net};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::network::{
    EgressMode, NetworkPolicy, NetworkReadiness, NetworkTransport, TransportPurpose, UnitNetwork,
};
use orch_driver_contract::runtime::{LABEL_RUNTIME_ID, LABEL_SYSTEM, RuntimeSpec};
use orch_driver_contract::units::UnitRef;
use orch_types::{Clock, Id};

use crate::client::{Daemon, map_error, status_of};
use crate::config::DockerConfig;
use crate::driver::DockerDriver;
use crate::egress::admission::{Admission, canonical_name};
use crate::egress::files::{self, Orders, State};
use crate::egress::program::{HEARTBEAT_EVERY, HTTP_PORT};
use crate::networks::LABEL_NETWORK;
use crate::spec::layout;
use crate::units::driver_labels;

/// The label on a unit that says how it reaches out: `restricted` (other names its place allows) or `none` (Orch's
/// transport alone).
pub const LABEL_EGRESS: &str = "orch.egress";

/// The label on a proxy's container that names the unit it serves.
pub const LABEL_PROXY_OF: &str = "orch.proxy_of";

/// What the label of a restricted unit's own network says it is.
pub const NETWORK_ROLE: &str = "restricted";

/// The option of a bridge that decides whether the host has an address on it.
const GATEWAY_MODE: &str = "com.docker.network.bridge.gateway_mode_ipv4";

/// The gateway mode in which the bridge has no address on the host.
const ISOLATED: &str = "isolated";

/// The option of a bridge that lets its members reach each other: on, since the unit reaches its proxy.
const OPTION_ICC: &str = "com.docker.network.bridge.enable_icc";

/// Where the proxy's directory is inside its container.
const PROXY_DIR: &str = "/run/orch-egress";

/// The setting of a network namespace below which ports are privileged: zero, so that the proxy's resolver binds
/// port 53 without being given any capability.
const UNPRIVILEGED_PORTS: &str = "net.ipv4.ip_unprivileged_port_start";

/// How long a change of the list may take to be in force before it is reported unavailable: inside the half
/// minute a command to apply it is given.
pub const APPLY_WAIT: Duration = Duration::from_secs(25);

/// How often the proxy's state is read while a change is awaited.
const APPLY_POLL: Duration = Duration::from_millis(250);

/// How long a proxy is given to stop.
const PROXY_STOP_SECS: i32 = 5;

/// What `NO_PROXY` holds in a restricted unit: loopback alone, so that a unit's own local services work.
pub const NO_PROXY: &str = "localhost,127.0.0.1,::1";

/// The capabilities a restricted unit never has, whatever its template or a capability asks: each can change its
/// network or make packets that go around the proxy.
pub const BYPASS_CAPABILITIES: [&str; 3] = ["NET_ADMIN", "NET_RAW", "SYS_ADMIN"];

/// The name of a restricted unit's own network.
#[must_use]
pub fn network_name(unit: &str) -> String {
    format!("{unit}-net")
}

/// The name of a restricted unit's proxy's container.
#[must_use]
pub fn proxy_name(unit: &str) -> String {
    format!("{unit}-proxy")
}

fn observer_name(unit: &str) -> String {
    format!("{unit}-addresses")
}

// Only kernel networking tables are read. The interpreter belongs to the operator's trusted image,
// and neither the runtime's image nor any caller-supplied executable enters the host network.
const HOST_INVENTORY_SCRIPT: &str = r#"set -eu
inventory_dir=$1
while :; do
  cat /proc/net/fib_trie > "$inventory_dir/host-fib-trie.pending"
  cat /proc/net/route > "$inventory_dir/host-routes.pending"
  if [ -r /proc/net/if_inet6 ]; then cat /proc/net/if_inet6 > "$inventory_dir/host-if-inet6.pending"; else : > "$inventory_dir/host-if-inet6.pending"; fi
  if [ -r /proc/net/ipv6_route ]; then cat /proc/net/ipv6_route > "$inventory_dir/host-routes6.pending"; else : > "$inventory_dir/host-routes6.pending"; fi
  mv "$inventory_dir/host-fib-trie.pending" "$inventory_dir/host-fib-trie"
  mv "$inventory_dir/host-routes.pending" "$inventory_dir/host-routes"
  mv "$inventory_dir/host-if-inet6.pending" "$inventory_dir/host-if-inet6"
  mv "$inventory_dir/host-routes6.pending" "$inventory_dir/host-routes6"
  date +%s > "$inventory_dir/host-addresses-at.pending"
  mv "$inventory_dir/host-addresses-at.pending" "$inventory_dir/host-addresses-at"
  sleep "$2"
done"#;

/// How a unit reaches out, as its template says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// On an ordinary network of the driver's.
    Open,
    /// Through its proxy: Orch's transport and the other names its place allows.
    Restricted,
    /// Through its proxy: Orch's transport alone, whatever a place allows.
    OnlyOrch,
}

impl Reach {
    /// The value of [`LABEL_EGRESS`] for it; none for an open unit.
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::Open => None,
            Self::Restricted => Some("restricted"),
            Self::OnlyOrch => Some("none"),
        }
    }

    /// Read back from a unit's label.
    #[must_use]
    pub fn of_label(label: Option<&str>) -> Self {
        match label {
            Some("restricted") => Self::Restricted,
            Some("none") => Self::OnlyOrch,
            _ => Self::Open,
        }
    }
}

/// A restricted unit's network as made: its name, its subnet and the address of its proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Private {
    /// The network's name.
    pub network: String,
    /// Its subnet.
    pub subnet: Ipv4Net,
    /// The proxy's address on it.
    pub proxy: IpAddr,
}

impl Private {
    /// Where the unit's programs find the proxy.
    #[must_use]
    pub fn proxy_url(&self) -> String {
        format!("http://{}:{HTTP_PORT}", self.proxy)
    }

    /// The variables that name the proxy to the unit's programs, in both cases, which no template or secret may
    /// replace.
    #[must_use]
    pub fn variables(&self) -> Vec<(String, String)> {
        let url = self.proxy_url();
        let mut out = Vec::new();
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            out.push((name.to_owned(), url.clone()));
            out.push((name.to_ascii_lowercase(), url.clone()));
        }
        out.push(("NO_PROXY".to_owned(), NO_PROXY.to_owned()));
        out.push(("no_proxy".to_owned(), NO_PROXY.to_owned()));
        out.push(("ORCH_EGRESS_PROXY".to_owned(), url));
        out
    }
}

/// The address the network's gateway is given in its subnet: the first, which an isolated bridge never holds.
fn gateway_address(subnet: Ipv4Net) -> Option<Ipv4Addr> {
    subnet.hosts().next()
}

/// The address of the proxy in a subnet: the second.
#[must_use]
pub fn proxy_address(subnet: Ipv4Net) -> Option<IpAddr> {
    subnet.hosts().nth(1).map(IpAddr::V4)
}

fn overlaps(taken: &IpNet, subnet: &Ipv4Net) -> bool {
    match taken {
        IpNet::V4(taken) => taken.contains(&subnet.network()) || subnet.contains(&taken.network()),
        IpNet::V6(_) => false,
    }
}

/// The subnet a runtime's network is tried at, the `attempt`th choice: from the pool, beginning at the place the
/// runtime's identifier gives (so that two runtimes seldom try one subnet at once), and skipping those taken.
#[must_use]
pub fn subnet_choice(
    pool: Ipv4Net,
    prefix: u8,
    seed: u128,
    taken: &[IpNet],
    attempt: usize,
) -> Option<Ipv4Net> {
    let all: Vec<Ipv4Net> = pool.subnets(prefix).ok()?.collect();
    if all.is_empty() {
        return None;
    }
    let count = u128::try_from(all.len()).ok()?;
    let start = usize::try_from(seed % count).ok()?;
    all.iter()
        .cycle()
        .skip(start)
        .take(all.len())
        .filter(|subnet| !taken.iter().any(|taken| overlaps(taken, subnet)))
        .nth(attempt)
        .copied()
}

/// The trusted Orch endpoint a unit's connector dials: the gateway of its request, by its exact name and port.
pub fn gateway_transport(gateway_url: &str) -> Result<NetworkTransport, DriverError> {
    let invalid = || {
        DriverError::InvalidSpec("network.restricted_unavailable: the trusted gateway must be a valid secure websocket endpoint".to_owned())
    };
    let url = url::Url::parse(gateway_url).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "wss" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let authority = match url.host().ok_or_else(invalid)? {
        url::Host::Domain(host) => canonical_name(host).map_err(|_| invalid())?,
        url::Host::Ipv4(address) => address.to_string(),
        url::Host::Ipv6(address) => address.to_string(),
    };
    let port = url.port_or_known_default().ok_or_else(invalid)?;
    Ok(NetworkTransport {
        authority,
        protocol: "wss".to_owned(),
        port,
        purpose: TransportPurpose::Gateway,
    })
}

/// The revision of the trusted endpoints: a hash of their canonical text.
#[must_use]
pub fn always_revision(always: &[NetworkTransport]) -> String {
    let mut sorted = always.to_vec();
    sorted.sort();
    let text = serde_json::to_string(&sorted).unwrap_or_default();
    let hex = blake3::hash(text.as_bytes()).to_hex();
    hex.as_str().chars().take(REVISION_DIGITS).collect()
}

/// How many hexadecimal digits of the hash a revision of the trusted endpoints keeps: enough that two sets never
/// meet by chance on one host.
const REVISION_DIGITS: usize = 16;

/// What the proxy of a unit admits under `policy`: the gateway always; the other names only for a unit whose
/// template allows any; and never an address of the ranges `refused`.
pub fn admission_of(
    reach: Reach,
    gateway_url: &str,
    policy: &NetworkPolicy,
    refused: Vec<IpNet>,
) -> Result<Admission, DriverError> {
    let always = vec![gateway_transport(gateway_url)?];
    let mut allowed_hosts: Vec<String> = match reach {
        Reach::Restricted => policy
            .allowed_hosts
            .iter()
            .filter_map(|host| canonical_name(host).ok())
            .collect(),
        Reach::Open | Reach::OnlyOrch => Vec::new(),
    };
    allowed_hosts.sort();
    allowed_hosts.dedup();
    Ok(Admission {
        revision: policy.revision.clone(),
        always_revision: always_revision(&always),
        always,
        allowed_hosts,
        refused,
    })
}

/// The subnets of every network of the daemon: the host's own addresses there and every other unit's, which no
/// other name may resolve into, and which a new network may not overlap.
pub async fn taken_subnets(daemon: &Daemon) -> Result<Vec<IpNet>, DriverError> {
    let networks = daemon
        .docker()
        .list_networks(None::<bollard::query_parameters::ListNetworksOptions>)
        .await
        .map_err(map_error)?;
    Ok(networks
        .iter()
        .filter_map(|network| network.ipam.as_ref())
        .filter_map(|ipam| ipam.config.as_ref())
        .flatten()
        .filter_map(|config| config.subnet.as_deref())
        .filter_map(|subnet| subnet.parse::<IpNet>().ok())
        .collect())
}

/// The request that makes a unit's own network at `subnet`.
#[must_use]
pub fn create_request(
    name: &str,
    subnet: Ipv4Net,
    config: &DockerConfig,
    system: Option<&str>,
) -> NetworkCreateRequest {
    let mut labels: HashMap<String, String> = driver_labels(config).into_iter().collect();
    labels.insert(LABEL_NETWORK.to_owned(), NETWORK_ROLE.to_owned());
    if let Some(system) = system {
        labels.insert(LABEL_SYSTEM.to_owned(), system.to_owned());
    }
    NetworkCreateRequest {
        name: name.to_owned(),
        driver: Some("bridge".to_owned()),
        internal: Some(true),
        attachable: Some(false),
        enable_ipv6: Some(false),
        ipam: Some(Ipam {
            driver: Some("default".to_owned()),
            config: Some(vec![IpamConfig {
                subnet: Some(subnet.to_string()),
                gateway: gateway_address(subnet).map(|address| address.to_string()),
                ..IpamConfig::default()
            }]),
            options: None,
        }),
        options: Some(HashMap::from([
            (GATEWAY_MODE.to_owned(), ISOLATED.to_owned()),
            (OPTION_ICC.to_owned(), "true".to_owned()),
        ])),
        labels: Some(labels),
        ..NetworkCreateRequest::default()
    }
}

/// The network as found, when it is a restricted unit's network of the driver's that is as it must be: internal,
/// its bridge without an address on the host, with one IPv4 subnet.
fn as_made(found: &NetworkInspect, name: &str) -> Result<Private, DriverError> {
    let ours = found
        .labels
        .as_ref()
        .is_some_and(|labels| labels.get(LABEL_NETWORK).map(String::as_str) == Some(NETWORK_ROLE));
    if !ours {
        return Err(DriverError::Internal(format!(
            "a network named {name} exists and is not a restricted unit's network of the driver's"
        )));
    }
    let isolated = found
        .options
        .as_ref()
        .is_some_and(|options| options.get(GATEWAY_MODE).map(String::as_str) == Some(ISOLATED));
    if found.internal != Some(true) || !isolated {
        return Err(DriverError::Unsupported(format!(
            "network.restricted_unavailable: the daemon did not make {name} internal with a bridge that has no address on the host"
        )));
    }
    let subnet = found
        .ipam
        .as_ref()
        .and_then(|ipam| ipam.config.as_ref())
        .and_then(|configs| configs.iter().find_map(|config| config.subnet.clone()))
        .and_then(|subnet| subnet.parse::<Ipv4Net>().ok())
        .ok_or_else(|| DriverError::Internal(format!("the network {name} has no IPv4 subnet")))?;
    let proxy = proxy_address(subnet).ok_or_else(|| {
        DriverError::Internal(format!("the subnet of {name} is too small for a proxy"))
    })?;
    Ok(Private {
        network: name.to_owned(),
        subnet,
        proxy,
    })
}

/// Makes sure the unit's own network exists and is as it must be; makes it when it does not.
pub async fn ensure_network(
    daemon: &Daemon,
    config: &DockerConfig,
    unit: &str,
    runtime: Id,
    system: Option<&str>,
) -> Result<Private, DriverError> {
    let name = network_name(unit);
    crate::networks::cut_from_pool(
        daemon,
        config,
        &name,
        runtime.as_u128(),
        |subnet| create_request(&name, subnet, config, system),
        as_made,
    )
    .await
    .map_err(|error| match error {
        // A pool that cannot hold the network refuses restriction with its own code.
        DriverError::Unsupported(why) if !why.starts_with("network.restricted_unavailable") => {
            DriverError::Unsupported(format!("network.restricted_unavailable: {why}"))
        }
        other => other,
    })
}

/// Makes sure the network a restricted unit's proxy goes out on exists, cut from the pool like every unit's.
pub async fn ensure_outward(
    daemon: &Daemon,
    config: &DockerConfig,
    unit: &str,
    runtime: Id,
    system: Option<&str>,
) -> Result<crate::networks::Own, DriverError> {
    crate::networks::ensure_open(
        daemon,
        config,
        &crate::networks::egress_network_name(unit),
        crate::networks::ROLE_EGRESS,
        runtime,
        system,
    )
    .await
}

/// Tries, once, whether this daemon makes a network as a restricted unit needs one: internal, its bridge without
/// an address on the host. The network is removed again whatever was found.
pub async fn probe(daemon: &Daemon, config: &DockerConfig) -> bool {
    if daemon.facts().rootless || config.egress.dir.is_none() {
        return false;
    }
    let name = format!("orch-egress-probe-{}", orch_types::new_id().simple());
    let Some(pool) = config.networks.pool() else {
        return false;
    };
    let Ok(taken) = taken_subnets(daemon).await else {
        return false;
    };
    let Some(subnet) = subnet_choice(
        pool,
        config.networks.prefix,
        orch_types::new_id().as_u128(),
        &taken,
        0,
    ) else {
        return false;
    };
    let made = daemon
        .docker()
        .create_network(create_request(&name, subnet, config, None))
        .await;
    let passed = match made {
        Ok(_) => match daemon.network(&name).await {
            Ok(Some(found)) => as_made(&found, &name).is_ok(),
            _ => false,
        },
        Err(error) => {
            tracing::info!(%error, "this daemon cannot make a restricted unit's network");
            false
        }
    };
    if let Err(error) = daemon.docker().remove_network(&name).await
        && status_of(&error) != Some(404)
    {
        tracing::warn!(network = %name, %error, "the probe's network could not be removed");
    }
    tracing::info!(passed, "probed restricted networks on this daemon");
    passed
}

/// How long the probe of a runtime waits for its unit to start, try the outside and end: a virtual machine boots
/// first, which took seconds on a host that is itself a virtual machine.
const RUNTIME_PROBE_WAIT: Duration = Duration::from_secs(150);

/// What the probe's unit does: it tries a connection to an address outside, which must fail on the unit's internal
/// network, and a name lookup through its own resolver file, which must not be answered. `nc` is busybox's. Exit
/// status 0 means nothing got out; any other means something did, or the unit could not try.
const RUNTIME_PROBE_SCRIPT: &str = "command -v nc >/dev/null 2>&1 || exit 4; \
if nc -w 3 1.1.1.1 80 </dev/null >/dev/null 2>&1; then exit 3; fi; exit 0";

/// Tries, once, whether a unit made under `runtime` on a restricted unit's kind of network reaches nothing outside:
/// it is made from the operator's trusted sidecar image, put on an internal network with the gateway mode `isolated`
/// and asked to connect to an address outside, which must fail. A runtime whose units have a kernel of their own is
/// offered restricted units only where this passed. The unit and the network are removed again whatever was found.
pub async fn probe_runtime(daemon: &Daemon, config: &DockerConfig, runtime: &str) -> bool {
    use bollard::query_parameters::{StartContainerOptionsBuilder, WaitContainerOptionsBuilder};
    use futures::StreamExt;
    let Some(image) = config.egress.proxy_image.as_deref() else {
        return false;
    };
    let Some(pool) = config.networks.pool() else {
        return false;
    };
    let Ok(taken) = taken_subnets(daemon).await else {
        return false;
    };
    let Some(subnet) = subnet_choice(
        pool,
        config.networks.prefix,
        orch_types::new_id().as_u128(),
        &taken,
        0,
    ) else {
        return false;
    };
    let token = orch_types::new_id().simple().to_string();
    let network = format!("orch-egress-probe-{token}");
    let container = format!("orch-egress-probe-{token}");
    if daemon
        .docker()
        .create_network(create_request(&network, subnet, config, None))
        .await
        .is_err()
    {
        return false;
    }
    let body = ContainerCreateBody {
        image: Some(image.to_owned()),
        entrypoint: Some(vec!["/bin/sh".to_owned()]),
        cmd: Some(vec!["-c".to_owned(), RUNTIME_PROBE_SCRIPT.to_owned()]),
        labels: Some(driver_labels(config).into_iter().collect()),
        host_config: Some(HostConfig {
            network_mode: Some(network.clone()),
            runtime: Some(runtime.to_owned()),
            cap_drop: Some(vec!["ALL".to_owned()]),
            ..HostConfig::default()
        }),
        ..ContainerCreateBody::default()
    };
    let passed = async {
        daemon
            .docker()
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&container)
                        .build(),
                ),
                body,
            )
            .await
            .ok()?;
        daemon
            .docker()
            .start_container(
                &container,
                Some(StartContainerOptionsBuilder::default().build()),
            )
            .await
            .ok()?;
        let docker = daemon.docker();
        let mut waiting = Box::pin(docker.wait_container(
            &container,
            Some(WaitContainerOptionsBuilder::default().build()),
        ));
        match tokio::time::timeout(RUNTIME_PROBE_WAIT, waiting.next()).await {
            Ok(Some(Ok(done))) => Some(done.status_code == 0),
            Ok(Some(Err(bollard::errors::Error::DockerContainerWaitError { code, .. }))) => {
                Some(code == 0)
            }
            _ => None,
        }
    }
    .await
    .unwrap_or(false);
    if let Err(error) = daemon
        .docker()
        .remove_container(
            &container,
            Some(
                RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(true)
                    .build(),
            ),
        )
        .await
        && status_of(&error) != Some(404)
    {
        tracing::warn!(%container, %error, "the probe's unit could not be removed");
    }
    if let Err(error) = daemon.docker().remove_network(&network).await
        && status_of(&error) != Some(404)
    {
        tracing::warn!(%network, %error, "the probe's network could not be removed");
    }
    tracing::info!(runtime, passed, "probed a restricted unit under a runtime");
    passed
}

/// The directory, as the node sees it, of a unit's proxy's files.
pub fn unit_dir(config: &DockerConfig, unit: &str) -> Result<PathBuf, DriverError> {
    config
        .egress
        .dir
        .as_ref()
        .map(|dir| dir.join(unit))
        .ok_or_else(|| {
            DriverError::Unsupported(
                "network.restricted_unavailable: this host keeps no directory for restricted units' proxies"
                    .to_owned(),
            )
        })
}

/// Writes the proxy's orders and a first heartbeat, making its directory when it is not there.
pub async fn write_orders(
    dir: &Path,
    admission: Admission,
    clock: &dyn Clock,
) -> Result<(), DriverError> {
    let io = |error: std::io::Error| {
        DriverError::Internal(format!(
            "the orders of a unit's proxy could not be written: {error}"
        ))
    };
    tokio::fs::create_dir_all(dir).await.map_err(io)?;
    files::write_json(
        dir,
        files::ORDERS,
        &Orders {
            admission,
            open: true,
        },
    )
    .await
    .map_err(io)?;
    files::beat(dir, clock.now().timestamp()).await.map_err(io)
}

/// The creation options of a unit's proxy's container.
#[must_use]
pub fn proxy_body(
    private: &Private,
    image: &str,
    connector_dir: &str,
    proxy_dir: &str,
    gateway_entry: &str,
    labels: HashMap<String, String>,
    user: (u32, u32),
) -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(image.to_owned()),
        user: Some(format!("{}:{}", user.0, user.1)),
        working_dir: Some("/".to_owned()),
        entrypoint: Some(vec![layout::CONNECTOR_PROGRAM.to_owned()]),
        cmd: Some(vec![
            "proxy".to_owned(),
            "--dir".to_owned(),
            PROXY_DIR.to_owned(),
            "--listen".to_owned(),
            private.proxy.to_string(),
            "--host-inventory".to_owned(),
        ]),
        labels: Some(labels),
        host_config: Some(HostConfig {
            init: Some(true),
            mounts: Some(vec![
                crate::mounts::bind(connector_dir, layout::CONNECTOR_DIR, true),
                crate::mounts::bind(proxy_dir, PROXY_DIR, false),
            ]),
            network_mode: Some(private.network.clone()),
            extra_hosts: Some(vec![gateway_entry.to_owned()]),
            cap_drop: Some(vec!["ALL".to_owned()]),
            security_opt: Some(vec!["no-new-privileges".to_owned()]),
            readonly_rootfs: Some(true),
            sysctls: Some(HashMap::from([
                (UNPRIVILEGED_PORTS.to_owned(), "0".to_owned()),
                ("net.ipv4.ip_forward".to_owned(), "0".to_owned()),
                ("net.ipv6.conf.all.forwarding".to_owned(), "0".to_owned()),
            ])),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::NO),
                maximum_retry_count: None,
            }),
            ..HostConfig::default()
        }),
        networking_config: Some(NetworkingConfig {
            endpoints_config: Some(HashMap::from([(
                private.network.clone(),
                EndpointSettings {
                    ipam_config: Some(EndpointIpamConfig {
                        ipv4_address: Some(private.proxy.to_string()),
                        ..EndpointIpamConfig::default()
                    }),
                    ..EndpointSettings::default()
                },
            )])),
        }),
        ..ContainerCreateBody::default()
    }
}

/// What a unit's proxy is made with, besides its network.
#[derive(Debug, Clone)]
pub struct ProxyMaking<'a> {
    /// The unit it serves.
    pub unit: &'a str,
    /// The unit's image, retained for caller compatibility; sidecars use only the operator's trusted image.
    pub image: &'a str,
    /// The directory of the connector, as the daemon names it.
    pub connector_dir: &'a str,
    /// The network of its own, cut from the pool, it goes out on: its one way out.
    pub outward: &'a str,
    /// The host entry by which it finds the gateway, as an open unit does.
    pub gateway_entry: &'a str,
    /// The system the unit belongs to.
    pub system: Option<&'a str>,
}

/// Makes sure the unit's proxy's container exists; makes it when it does not, on the unit's network at its fixed
/// address and on the ordinary network.
pub async fn ensure_proxy(
    daemon: &Daemon,
    config: &DockerConfig,
    private: &Private,
    making: &ProxyMaking<'_>,
) -> Result<(), DriverError> {
    let configured = config.egress.proxy_image.as_deref().ok_or_else(|| DriverError::Unsupported(
        "network.restricted_unavailable: the node has no trusted sidecar image; a runtime's image is never used for a host-owned proxy".to_owned()))?;
    let image = daemon
        .docker()
        .inspect_image(configured)
        .await
        .map_err(map_error)?
        .id
        .ok_or_else(|| {
            DriverError::Unavailable("the trusted proxy image has no immutable identity".to_owned())
        })?;
    let dir = unit_dir(config, making.unit)?;
    let host_dir = config.host_path(&dir);
    let observer = observer_name(making.unit);
    if let Some(found) = daemon.container(&observer).await? {
        if found
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .and_then(|labels| labels.get(LABEL_PROXY_OF))
            .is_none_or(|unit| unit != making.unit)
        {
            return Err(DriverError::Unavailable(
                "the address observer's name belongs to another container".to_owned(),
            ));
        }
    } else {
        let mut labels: HashMap<String, String> = driver_labels(config).into_iter().collect();
        labels.insert(LABEL_PROXY_OF.to_owned(), making.unit.to_owned());
        if let Some(system) = making.system {
            labels.insert(LABEL_SYSTEM.to_owned(), system.to_owned());
        }
        let user = (
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw(),
        );
        let body = ContainerCreateBody {
            image: Some(image.clone()),
            user: Some(format!("{}:{}", user.0, user.1)),
            entrypoint: Some(vec!["/bin/sh".to_owned()]),
            cmd: Some(vec![
                "-c".to_owned(),
                HOST_INVENTORY_SCRIPT.to_owned(),
                "orch-address-observer".to_owned(),
                PROXY_DIR.to_owned(),
                crate::egress::program::POLL.as_secs_f64().to_string(),
            ]),
            labels: Some(labels),
            host_config: Some(HostConfig {
                network_mode: Some("host".to_owned()),
                readonly_rootfs: Some(true),
                mounts: Some(vec![crate::mounts::bind(
                    &host_dir.to_string_lossy(),
                    PROXY_DIR,
                    false,
                )]),
                cap_drop: Some(vec!["ALL".to_owned()]),
                security_opt: Some(vec!["no-new-privileges".to_owned()]),
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::NO),
                    maximum_retry_count: None,
                }),
                ..HostConfig::default()
            }),
            ..ContainerCreateBody::default()
        };
        daemon
            .docker()
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&observer)
                        .build(),
                ),
                body,
            )
            .await
            .map_err(map_error)?;
    }
    let name = proxy_name(making.unit);
    if let Some(found) = daemon.container(&name).await? {
        let serves = found
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .and_then(|labels| labels.get(LABEL_PROXY_OF))
            .is_some_and(|unit| unit == making.unit);
        if !serves {
            return Err(DriverError::Internal(format!(
                "a container named {name} exists and is not the proxy of {}",
                making.unit
            )));
        }
        if found.image.as_deref() == Some(image.as_str()) {
            return Ok(());
        }
        // An older proxy made from the caller's image is never retained as a trusted endpoint.
        daemon
            .docker()
            .remove_container(
                &name,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await
            .map_err(map_error)?;
    }
    let dir = unit_dir(config, making.unit)?;
    let host_dir = config.host_path(&dir);
    let mut labels: HashMap<String, String> = driver_labels(config).into_iter().collect();
    labels.insert(LABEL_PROXY_OF.to_owned(), making.unit.to_owned());
    if let Some(system) = making.system {
        labels.insert(LABEL_SYSTEM.to_owned(), system.to_owned());
    }
    let user = (
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw(),
    );
    let body = proxy_body(
        private,
        &image,
        making.connector_dir,
        &host_dir.to_string_lossy(),
        making.gateway_entry,
        labels,
        user,
    );
    match daemon
        .docker()
        .create_container(
            Some(CreateContainerOptionsBuilder::default().name(&name).build()),
            body,
        )
        .await
    {
        Ok(_) => {}
        Err(error) if status_of(&error) == Some(409) => {}
        Err(error) => return Err(map_error(error)),
    }
    match daemon
        .docker()
        .connect_network(
            making.outward,
            NetworkConnectRequest {
                container: name.clone(),
                endpoint_config: None,
            },
        )
        .await
    {
        Ok(()) => Ok(()),
        // Already on it: a command for the same unit got here first.
        Err(error) if error.to_string().contains("already exists") => Ok(()),
        Err(error) => Err(map_error(error)),
    }
}

/// Starts the unit's proxy when it does not run, with a fresh heartbeat, before the unit is started.
pub async fn start_proxy(
    daemon: &Daemon,
    config: &DockerConfig,
    unit: &str,
    clock: &dyn Clock,
) -> Result<(), DriverError> {
    let name = proxy_name(unit);
    let Some(found) = daemon.container(&name).await? else {
        return Err(DriverError::Unavailable(format!(
            "network.restricted_unavailable: the proxy of {unit} is gone; the unit is made again"
        )));
    };
    let dir = unit_dir(config, unit)?;
    let observer = observer_name(unit);
    let sensor = daemon.container(&observer).await?.ok_or_else(|| {
        DriverError::Unavailable("the host address observer is missing".to_owned())
    })?;
    if sensor.state.as_ref().and_then(|state| state.running) != Some(true) {
        files::write(&dir, "host-addresses-at", b"0")
            .await
            .map_err(|_| {
                DriverError::Unavailable("the host address inventory could not be reset".to_owned())
            })?;
        daemon
            .docker()
            .start_container(
                &observer,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(map_error)?;
    }
    if let Err(error) = files::beat(&dir, clock.now().timestamp()).await {
        tracing::warn!(%unit, %error, "the heartbeat of a unit's proxy could not be written");
    }
    let running = found.state.as_ref().and_then(|state| state.running) == Some(true);
    if running {
        return Ok(());
    }
    // A stopped proxy's old Ready file is no acknowledgment from the new process.
    files::write_json(
        &dir,
        files::STATE,
        &State {
            reason: Some(
                "the proxy is starting and must acknowledge its current policy".to_owned(),
            ),
            ..State::default()
        },
    )
    .await
    .map_err(|_| {
        DriverError::Unavailable("the restarted proxy's state could not be reset".to_owned())
    })?;
    daemon
        .docker()
        .start_container(
            &name,
            None::<bollard::query_parameters::StartContainerOptions>,
        )
        .await
        .map_err(map_error)
}

/// Stops the unit's proxy, after the unit.
pub async fn stop_proxy(daemon: &Daemon, unit: &str) -> Result<(), DriverError> {
    for name in [proxy_name(unit), observer_name(unit)] {
        match daemon
            .docker()
            .stop_container(
                &name,
                Some(
                    StopContainerOptionsBuilder::default()
                        .t(PROXY_STOP_SECS)
                        .build(),
                ),
            )
            .await
        {
            Ok(()) => {}
            Err(error) if matches!(status_of(&error), Some(304 | 404)) => {}
            Err(error) => return Err(map_error(error)),
        }
    }
    Ok(())
}

/// Removes the unit's proxy, its network and its directory, after the unit; what is not there is done.
pub async fn remove_all(
    daemon: &Daemon,
    config: &DockerConfig,
    unit: &str,
) -> Result<(), DriverError> {
    for name in [proxy_name(unit), observer_name(unit)] {
        match daemon
            .docker()
            .remove_container(
                &name,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await
        {
            Ok(()) => {}
            Err(error) if status_of(&error) == Some(404) => {}
            Err(error) => return Err(map_error(error)),
        }
    }
    // The unit's own network, open or restricted, and its proxy's way out are released with it.
    crate::networks::remove(daemon, &network_name(unit)).await?;
    crate::networks::remove(daemon, &crate::networks::egress_network_name(unit)).await?;
    if let Ok(dir) = unit_dir(config, unit) {
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%unit, %error, "the directory of a unit's proxy could not be removed");
            }
        }
    }
    Ok(())
}

/// What is in force for a unit's network, from its proxy's orders and state.
pub async fn unit_network(
    config: &DockerConfig,
    unit: &str,
    reach: Reach,
    private: Option<&Private>,
) -> UnitNetwork {
    let required = match reach {
        Reach::Open => EgressMode::Open,
        Reach::Restricted | Reach::OnlyOrch => EgressMode::Restricted,
    };
    let Ok(dir) = unit_dir(config, unit) else {
        return UnitNetwork::unavailable(
            &NetworkPolicy {
                mode: required,
                ..NetworkPolicy::default()
            },
            "this host keeps no directory for restricted units' proxies",
        );
    };
    let orders: Option<Orders> = files::read_json(&dir, files::ORDERS).await;
    let state: Option<State> = files::read_json(&dir, files::STATE).await;
    let admission = orders.map(|orders| orders.admission).unwrap_or_default();
    let flowing = state.as_ref().is_some_and(|state| {
        state.flowing && state.revision.as_deref() == Some(admission.revision.as_str())
    });
    UnitNetwork {
        mode: required,
        required_mode: required,
        allowed_hosts: admission.allowed_hosts,
        always: admission.always,
        always_revision: admission.always_revision,
        revision: state
            .as_ref()
            .and_then(|state| state.revision.clone())
            .unwrap_or_default(),
        state: if flowing {
            NetworkReadiness::Ready
        } else {
            NetworkReadiness::Unavailable
        },
        reason: if flowing {
            None
        } else {
            Some(
                state
                    .as_ref()
                    .and_then(|state| state.reason.clone())
                    .unwrap_or_else(|| "the proxy has not answered yet".to_owned()),
            )
        },
        last_denied: state.and_then(|state| state.last_denied),
        proxy_url: private.map(Private::proxy_url),
        networks: Vec::new(),
    }
}

/// Puts a new admission in force for a running unit's proxy and waits until the proxy says it is (it closes what
/// the new one no longer admits before it says so), or until [`APPLY_WAIT`] has passed.
pub async fn apply(
    config: &DockerConfig,
    unit: &str,
    reach: Reach,
    admission: Admission,
    clock: &dyn Clock,
) -> Result<UnitNetwork, DriverError> {
    let dir = unit_dir(config, unit)?;
    let revision = admission.revision.clone();
    write_orders(&dir, admission, clock).await?;
    let deadline = tokio::time::Instant::now() + APPLY_WAIT;
    loop {
        let state: Option<State> = files::read_json(&dir, files::STATE).await;
        if state.as_ref().is_some_and(|state| {
            state.flowing && state.revision.as_deref() == Some(revision.as_str())
        }) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(APPLY_POLL).await;
    }
    Ok(unit_network(config, unit, reach, None).await)
}

/// Whether a unit stands on the shared internal network units without a way out once stood on: the network mode of
/// its container names it.
#[must_use]
pub fn on_old_network(container: &ContainerInspectResponse, config: &DockerConfig) -> bool {
    let mode = container
        .host_config
        .as_ref()
        .and_then(|host| host.network_mode.as_deref())
        .unwrap_or_default();
    [&config.networks.env, &config.networks.ctl]
        .iter()
        .any(|base| mode == format!("{base}{OLD_INTERNAL_SUFFIX}"))
}

/// What followed the name of a network of the driver's to make the name of its old internal variant.
const OLD_INTERNAL_SUFFIX: &str = "-internal";

/// The creation options of a unit made again on its own network from what it was made with: the same image,
/// program, user, labels, mounts, limits and everything else, with the unit's network, its proxy as name server
/// and in its variables, no host entry and none of the capabilities that could go around the proxy.
#[must_use]
pub fn remade_body(
    container: &ContainerInspectResponse,
    private: &Private,
    reach: Reach,
) -> ContainerCreateBody {
    let config = container.config.clone().unwrap_or_default();
    let mut host = container.host_config.clone().unwrap_or_default();
    let mut env: Vec<String> = config
        .env
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            let name = entry.split('=').next().unwrap_or_default();
            !private
                .variables()
                .iter()
                .any(|(variable, _)| variable == name)
        })
        .collect();
    env.extend(
        private
            .variables()
            .into_iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    let mut labels = config.labels.unwrap_or_default();
    if let Some(label) = reach.label() {
        labels.insert(LABEL_EGRESS.to_owned(), label.to_owned());
    }
    restrict_host(&mut host, private, false);
    ContainerCreateBody {
        hostname: config.hostname,
        user: config.user,
        env: Some(env),
        cmd: config.cmd,
        image: config.image,
        working_dir: config.working_dir,
        entrypoint: config.entrypoint,
        labels: Some(labels),
        stop_signal: config.stop_signal,
        host_config: Some(host),
        ..ContainerCreateBody::default()
    }
}

/// What a restricted unit's host options must say: its own network, its proxy as name server, no host entry, and
/// none of the capabilities that could go around the proxy.
pub fn restrict_host(host: &mut HostConfig, private: &Private, own_kernel: bool) {
    host.network_mode = Some(private.network.clone());
    host.dns = Some(vec![private.proxy.to_string()]);
    host.dns_search = None;
    host.dns_options = None;
    host.extra_hosts = None;
    host.port_bindings = None;
    host.publish_all_ports = Some(false);
    if own_kernel {
        // What a guest changes of its own network stays inside it: its traffic leaves through the host's bridge, which
        // is internal and which the guest cannot change, so no capability has to be taken from it.
        return;
    }
    let mut drop = host.cap_drop.take().unwrap_or_default();
    for capability in BYPASS_CAPABILITIES {
        if !drop
            .iter()
            .any(|dropped| dropped.eq_ignore_ascii_case(capability))
        {
            drop.push(capability.to_owned());
        }
    }
    host.cap_drop = Some(drop);
    if let Some(add) = host.cap_add.as_mut() {
        add.retain(|added| {
            !BYPASS_CAPABILITIES.iter().any(|bypass| {
                added.eq_ignore_ascii_case(bypass)
                    || added.eq_ignore_ascii_case(&format!("CAP_{bypass}"))
            })
        });
    }
}

/// Why a restricted unit cannot be made with these host options: a privileged unit, the host's network or another
/// container's, or a capability that could go around the proxy. None when it can.
#[must_use]
pub fn refusal(host: &HostConfig, own_kernel: bool) -> Option<String> {
    if host.privileged == Some(true) {
        return Some("a privileged unit can change its own network".to_owned());
    }
    if let Some(mode) = host.network_mode.as_deref()
        && (mode == "host" || mode.starts_with("container:"))
    {
        return Some(format!("the network mode {mode:?} goes around the proxy"));
    }
    if own_kernel {
        // A capability of the guest's own kernel does not reach the host's bridge, which holds the unit in.
        return None;
    }
    let added = host.cap_add.clone().unwrap_or_default();
    BYPASS_CAPABILITIES
        .iter()
        .find(|bypass| {
            added.iter().any(|added| {
                added.eq_ignore_ascii_case(bypass)
                    || added.eq_ignore_ascii_case(&format!("CAP_{bypass}"))
            })
        })
        .map(|bypass| format!("the capability {bypass} could go around the proxy"))
}

/// The heartbeat the driver beats for every restricted unit's proxy of this host, while the node's link to the
/// system stands. A proxy whose heartbeat stops closes its unit's network.
#[derive(Debug, Clone)]
pub struct Heartbeat {
    link_up: Arc<AtomicBool>,
    reconciled: Arc<Mutex<Option<BTreeSet<String>>>>,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self {
            link_up: Arc::new(AtomicBool::new(true)),
            reconciled: Arc::new(Mutex::new(None)),
        }
    }
}

impl Heartbeat {
    /// Says whether the node's link to the system stands: while it does not, nothing beats.
    pub fn set_link(&self, up: bool) {
        if !up {
            *self
                .reconciled
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(BTreeSet::new());
        }
        self.link_up.store(up, Ordering::SeqCst);
    }

    /// Marks only this unit's newly supplied policy current on the new control link.
    pub fn reconcile(&self, unit: &str) {
        if let Some(current) = self
            .reconciled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            current.insert(unit.to_owned());
        }
    }

    /// Whether this unit has a current policy on the present authenticated control link.
    #[must_use]
    pub fn current(&self, unit: &str) -> bool {
        self.link_up.load(Ordering::SeqCst)
            && self
                .reconciled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .is_none_or(|units| units.contains(unit))
    }

    /// Beats for every unit's proxy directory below `dir`, every [`HEARTBEAT_EVERY`], for as long as the program
    /// runs.
    pub fn spawn(&self, dir: PathBuf, clock: Arc<dyn Clock>) {
        let link_up = self.link_up.clone();
        let reconciled = self.reconciled.clone();
        tokio::spawn(async move {
            loop {
                if link_up.load(Ordering::SeqCst)
                    && let Ok(mut entries) = tokio::fs::read_dir(&dir).await
                {
                    let now = clock.now().timestamp();
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let current = reconciled
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .as_ref()
                            .is_none_or(|units| {
                                units.contains(&entry.file_name().to_string_lossy().into_owned())
                            });
                        if !current {
                            continue;
                        }
                        if let Err(error) = files::beat(&entry.path(), now).await {
                            tracing::debug!(%error, "a proxy's heartbeat could not be written");
                        }
                    }
                }
                tokio::time::sleep(HEARTBEAT_EVERY).await;
            }
        });
    }
}

/// The runtime a container of the driver's says it is for, from its labels.
#[must_use]
pub fn runtime_of(container: &ContainerInspectResponse) -> Option<Id> {
    container
        .config
        .as_ref()?
        .labels
        .as_ref()?
        .get(LABEL_RUNTIME_ID)?
        .parse()
        .ok()
}

/// The labels of a restricted unit's network and proxy that name its system, when the unit's do.
#[must_use]
pub fn system_of(labels: &BTreeMap<String, String>) -> Option<&str> {
    labels.get(LABEL_SYSTEM).map(String::as_str)
}

/// The value of a variable a container was made with.
fn variable_of(container: &ContainerInspectResponse, name: &str) -> Option<String> {
    container
        .config
        .as_ref()?
        .env
        .as_ref()?
        .iter()
        .find_map(|entry| entry.strip_prefix(&format!("{name}=")).map(str::to_owned))
}

/// The labels a container was made with.
fn labels_of(container: &ContainerInspectResponse) -> BTreeMap<String, String> {
    container
        .config
        .as_ref()
        .and_then(|config| config.labels.clone())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// How far a unit reaches, as its container's label says.
#[must_use]
pub fn reach_of(container: &ContainerInspectResponse) -> Reach {
    Reach::of_label(labels_of(container).get(LABEL_EGRESS).map(String::as_str))
}

impl DockerDriver {
    fn control_admission(
        &self,
        reach: Reach,
        gateway_url: &str,
        policy: &NetworkPolicy,
        refused: Vec<IpNet>,
    ) -> Result<Admission, DriverError> {
        let mut admission = admission_of(reach, gateway_url, policy, refused)?;
        for address in self
            .control_endpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
        {
            let transport = gateway_transport(address)?;
            if !admission.always.contains(&transport) {
                admission.always.push(transport);
            }
        }
        admission.always_revision = always_revision(&admission.always);
        Ok(admission)
    }

    /// Observes the proxy's state without reopening it, including a safe last denial.
    pub(crate) async fn observe_unit_network(
        &self,
        unit: &UnitRef,
    ) -> Result<UnitNetwork, DriverError> {
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the runtime unit is gone".to_owned()))?;
        let reach = reach_of(&container);
        let addresses = crate::networks::addresses(&self.daemon, &unit.name, &container).await;
        if reach == Reach::Open {
            let mut network = UnitNetwork::open(&NetworkPolicy::default());
            network.networks = addresses;
            return Ok(network);
        }
        let mut network = unit_network(&self.config, &unit.name, reach, None).await;
        network.proxy_url = variable_of(&container, "ORCH_EGRESS_PROXY");
        network.networks = addresses;
        if !self.heartbeat.current(&unit.name) {
            network.state = NetworkReadiness::Unavailable;
            network.reason =
                Some("the host's current control link has not reconciled this policy".to_owned());
        }
        Ok(network)
    }

    /// Puts a restricted unit's proxy on its own way out, the network cut for it from the pool, and takes it off the
    /// common bridge a proxy made before stood on; a proxy already so is left as it is.
    async fn proxy_outward(&self, unit: &UnitRef, system: Option<&str>) -> Result<(), DriverError> {
        let outward = ensure_outward(
            &self.daemon,
            &self.config,
            &unit.name,
            unit.runtime_id,
            system,
        )
        .await?;
        let Some(proxy) = self.daemon.container(&proxy_name(&unit.name)).await? else {
            return Ok(());
        };
        let proxy_container = proxy_name(&unit.name);
        match crate::networks::on_common_bridge(&proxy, &self.config) {
            Some(bridge) => {
                crate::networks::move_off(&self.daemon, &proxy_container, &outward.name, &bridge)
                    .await?;
                tracing::info!(unit = %unit.name, %bridge, "moved a unit's proxy off a common bridge onto a way out of its own");
            }
            None if !crate::networks::endpoints_of(&proxy).contains_key(&outward.name) => {
                crate::networks::connect(&self.daemon, &proxy_container, &outward.name).await?;
            }
            None => {}
        }
        Ok(())
    }

    /// Whether this host may make restricted units at all: it was found, by trying, able to make their networks.
    fn restricted_available(&self) -> Result<(), DriverError> {
        if self.daemon.facts().restricted_egress
            && !self.daemon.facts().rootless
            && self.config.egress.proxy_image.is_some()
        {
            Ok(())
        } else {
            Err(DriverError::Unsupported(
                "network.restricted_unavailable: this host was not found able to make a network of a unit's own whose bridge has no address on the host"
                    .to_owned(),
            ))
        }
    }

    /// The ranges no other name may resolve into for a unit whose own network is `private`: every other network of
    /// the daemon.
    async fn refused_for(&self, private: &Private) -> Result<Vec<IpNet>, DriverError> {
        let own = IpNet::V4(private.subnet);
        let mut ranges: Vec<IpNet> = taken_subnets(&self.daemon)
            .await?
            .into_iter()
            .filter(|subnet| *subnet != own)
            .collect();
        if let Some(pool) = self.config.networks.pool() {
            ranges.push(IpNet::V4(pool));
        }
        Ok(ranges)
    }

    /// Makes a restricted unit's own network and writes its proxy's orders, before the unit and its proxy are made.
    pub(crate) async fn prepare_egress(
        &self,
        spec: &RuntimeSpec,
        reach: Reach,
    ) -> Result<Private, DriverError> {
        self.restricted_available()?;
        let dir = unit_dir(&self.config, &spec.name)?;
        let private = ensure_network(
            &self.daemon,
            &self.config,
            &spec.name,
            spec.runtime_id,
            system_of(&spec.labels),
        )
        .await?;
        let refused = self.refused_for(&private).await?;
        let admission =
            self.control_admission(reach, &spec.gateway_url, &spec.network_policy, refused)?;
        write_orders(&dir, admission, self.clock.as_ref()).await?;
        self.heartbeat.reconcile(&spec.name);
        Ok(private)
    }

    /// Makes a unit that stands on the old shared network again on a network of its own, with its proxy, keeping
    /// everything else it was made with; answers the new container.
    pub(crate) async fn remake_restricted(
        &self,
        unit: &UnitRef,
        old: &ContainerInspectResponse,
    ) -> Result<ContainerInspectResponse, DriverError> {
        self.restricted_available()?;
        let labels = labels_of(old);
        let private = ensure_network(
            &self.daemon,
            &self.config,
            &unit.name,
            unit.runtime_id,
            system_of(&labels),
        )
        .await?;
        let gateway_url = variable_of(old, "ORCH_GATEWAY_URL").ok_or_else(|| {
            DriverError::Internal(format!(
                "the unit {} names no gateway, so its proxy cannot be told it",
                unit.name
            ))
        })?;
        let refused = self.refused_for(&private).await?;
        let policy = NetworkPolicy {
            mode: EgressMode::Restricted,
            ..NetworkPolicy::default()
        };
        let admission = self.control_admission(Reach::OnlyOrch, &gateway_url, &policy, refused)?;
        let dir = unit_dir(&self.config, &unit.name)?;
        write_orders(&dir, admission, self.clock.as_ref()).await?;
        let image = old
            .config
            .as_ref()
            .and_then(|config| config.image.clone())
            .ok_or_else(|| DriverError::Internal(format!("the unit {} has no image", unit.name)))?;
        let connector_dir = old
            .host_config
            .as_ref()
            .and_then(|host| host.mounts.as_ref())
            .and_then(|mounts| {
                mounts
                    .iter()
                    .find(|mount| mount.target.as_deref() == Some(layout::CONNECTOR_DIR))
            })
            .and_then(|mount| mount.source.clone())
            .ok_or_else(|| {
                DriverError::Internal(format!("the unit {} has no connector bound", unit.name))
            })?;
        let outward = ensure_outward(
            &self.daemon,
            &self.config,
            &unit.name,
            unit.runtime_id,
            system_of(&labels),
        )
        .await?;
        let gateway_entry = crate::networks::gateway_entry(&self.config);
        ensure_proxy(
            &self.daemon,
            &self.config,
            &private,
            &ProxyMaking {
                unit: &unit.name,
                image: &image,
                connector_dir: &connector_dir,
                outward: &outward.name,
                gateway_entry: &gateway_entry,
                system: system_of(&labels),
            },
        )
        .await?;
        let body = remade_body(old, &private, Reach::OnlyOrch);
        let old_id = old.id.clone().unwrap_or_default();
        match self
            .daemon
            .docker()
            .remove_container(
                &old_id,
                Some(
                    RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .v(true)
                        .build(),
                ),
            )
            .await
        {
            Ok(()) => {}
            Err(error) if status_of(&error) == Some(404) => {}
            Err(error) => return Err(map_error(error)),
        }
        self.daemon
            .docker()
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&unit.name)
                        .build(),
                ),
                body,
            )
            .await
            .map_err(map_error)?;
        tracing::info!(unit = %unit.name, "made a unit without a way out again on a network of its own");
        self.daemon.container(&unit.name).await?.ok_or_else(|| {
            DriverError::Internal(format!(
                "the unit {} vanished after it was made again",
                unit.name
            ))
        })
    }

    /// Puts `policy` in force for a unit; see `EnvironmentDriver::apply_network`. A unit's mode is what it was made
    /// with: an open unit is made restricted, and a restricted one open, only by making it again.
    pub(crate) async fn apply_unit_network(
        &self,
        unit: &UnitRef,
        policy: &NetworkPolicy,
    ) -> Result<UnitNetwork, DriverError> {
        let mut container = self.container_of(unit).await?.ok_or_else(|| {
            DriverError::NotFound(format!("there is no unit {} of this driver", unit.name))
        })?;
        if on_old_network(&container, &self.config) {
            container = self.remake_restricted(unit, &container).await?;
        }
        let reach = reach_of(&container);
        match (reach, policy.mode) {
            (Reach::Open, EgressMode::Open) => {
                // An open unit made on a common bridge is moved to a network of its own before it next starts.
                let container = self.settle_own_network(unit, container).await?;
                let mut network = UnitNetwork::open(policy);
                network.networks =
                    crate::networks::addresses(&self.daemon, &unit.name, &container).await;
                Ok(network)
            }
            (Reach::Open, EgressMode::Restricted) => Err(DriverError::Unsupported(
                "network.restricted_unavailable: this unit was made open; a restricted one is made by making it again"
                    .to_owned(),
            )),
            (_, EgressMode::Open) => Err(DriverError::Unsupported(
                "this unit was made restricted; an open one is made only by a planned change that chooses it"
                    .to_owned(),
            )),
            (reach, EgressMode::Restricted) => {
                let gateway_url = variable_of(&container, "ORCH_GATEWAY_URL").ok_or_else(|| {
                    DriverError::Internal(format!("the unit {} names no gateway", unit.name))
                })?;
                let labels = labels_of(&container);
                let private = ensure_network(
                    &self.daemon,
                    &self.config,
                    &unit.name,
                    unit.runtime_id,
                    system_of(&labels),
                )
                .await?;
                // A proxy made before proxies had a way out of their own is moved onto one.
                self.proxy_outward(unit, system_of(&labels)).await?;
                let refused = self.refused_for(&private).await?;
                let admission = self.control_admission(reach, &gateway_url, policy, refused)?;
                // Only a current authenticated request renews this unit's heartbeat after a reconnect.
                let dir = unit_dir(&self.config, &unit.name)?;
                write_orders(&dir, admission.clone(), self.clock.as_ref()).await?;
                self.heartbeat.reconcile(&unit.name);
                start_proxy(&self.daemon, &self.config, &unit.name, self.clock.as_ref()).await?;
                let mut applied =
                    apply(&self.config, &unit.name, reach, admission, self.clock.as_ref()).await?;
                applied.proxy_url = Some(private.proxy_url());
                applied.networks =
                    crate::networks::addresses(&self.daemon, &unit.name, &container).await;
                Ok(applied)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private() -> Private {
        let subnet: Ipv4Net = "10.199.4.16/28".parse().unwrap();
        Private {
            network: network_name("orch-environment-0199abcd1234"),
            subnet,
            proxy: proxy_address(subnet).unwrap(),
        }
    }

    #[test]
    fn a_unit_network_is_internal_isolated_and_its_proxy_is_its_second_address() {
        let private = private();
        assert_eq!(private.proxy.to_string(), "10.199.4.18");
        assert_eq!(private.proxy_url(), "http://10.199.4.18:3128");
        let request = create_request(
            &private.network,
            private.subnet,
            &DockerConfig::default(),
            Some("sys"),
        );
        assert_eq!(request.internal, Some(true));
        let options = request.options.unwrap();
        assert_eq!(options[GATEWAY_MODE], ISOLATED);
        assert_eq!(options[OPTION_ICC], "true");
        let ipam = request.ipam.unwrap().config.unwrap();
        assert_eq!(ipam[0].subnet.as_deref(), Some("10.199.4.16/28"));
        assert_eq!(ipam[0].gateway.as_deref(), Some("10.199.4.17"));
        assert_eq!(request.labels.unwrap()[LABEL_NETWORK], NETWORK_ROLE);
    }

    #[test]
    fn a_subnet_is_chosen_from_the_pool_past_those_taken() {
        let pool: Ipv4Net = "10.199.0.0/16".parse().unwrap();
        let first = subnet_choice(pool, 28, 7, &[], 0).unwrap();
        assert_eq!(first.to_string(), "10.199.0.112/28");
        let taken: Vec<IpNet> = vec![
            "10.199.0.112/28".parse().unwrap(),
            "10.199.0.0/25".parse().unwrap(),
        ];
        let next = subnet_choice(pool, 28, 7, &taken, 0).unwrap();
        assert!(!taken.iter().any(|taken| overlaps(taken, &next)), "{next}");
        let tiny: Ipv4Net = "10.199.0.0/28".parse().unwrap();
        assert!(subnet_choice(tiny, 28, 7, &[IpNet::V4(tiny)], 0).is_none());
    }

    #[test]
    fn the_trusted_gateway_is_admitted_by_its_exact_configured_authority_and_port() {
        let transport = gateway_transport("wss://Gateway.Orch.Internal:9443").unwrap();
        assert_eq!(transport.authority, "gateway.orch.internal");
        assert_eq!((transport.protocol.as_str(), transport.port), ("wss", 9443));
        for (url, authority) in [
            ("wss://10.0.0.1:9443", "10.0.0.1"),
            ("wss://[fd00::1]:9443", "fd00::1"),
        ] {
            let transport = gateway_transport(url).unwrap();
            assert_eq!(transport.authority, authority);
            assert_eq!((transport.protocol.as_str(), transport.port), ("wss", 9443));
            assert_eq!(transport.purpose, TransportPurpose::Gateway);
        }
        for invalid in [
            "ws://10.0.0.1:9443",
            "wss://user@10.0.0.1:9443",
            "wss://10.0.0.1:9443?other=host",
            "wss://10.0.0.1:9443#other",
        ] {
            assert!(gateway_transport(invalid).is_err());
        }
        assert!(
            canonical_name("10.0.0.1").is_err(),
            "an ordinary allowed-host setting cannot create a trusted IP exception"
        );
    }

    #[test]
    fn a_unit_without_a_way_out_admits_only_orch_whatever_its_place_allows() {
        let policy = NetworkPolicy {
            mode: EgressMode::Restricted,
            allowed_hosts: vec!["PyPI.org".into(), "pypi.org".into(), "*.bad".into()],
            revision: "r1".into(),
            ..NetworkPolicy::default()
        };
        let only_orch = admission_of(
            Reach::OnlyOrch,
            "wss://gateway.orch.internal:9443",
            &policy,
            Vec::new(),
        )
        .unwrap();
        assert!(only_orch.allowed_hosts.is_empty());
        assert_eq!(only_orch.always.len(), 1);
        let listed = admission_of(
            Reach::Restricted,
            "wss://gateway.orch.internal:9443",
            &policy,
            Vec::new(),
        )
        .unwrap();
        assert_eq!(listed.allowed_hosts, vec!["pypi.org".to_owned()]);
        assert_eq!(listed.revision, "r1");
        assert_eq!(listed.always_revision, only_orch.always_revision);
    }

    #[test]
    fn a_restricted_unit_keeps_no_way_around_its_proxy() {
        let private = private();
        let mut host = HostConfig {
            network_mode: Some("orch-env-internal".into()),
            extra_hosts: Some(vec!["gateway.orch.internal:10.0.0.1".into()]),
            cap_add: Some(vec![
                "NET_ADMIN".into(),
                "SYS_PTRACE".into(),
                "cap_net_raw".into(),
            ]),
            ..HostConfig::default()
        };
        assert!(refusal(&host, false).is_some());
        restrict_host(&mut host, &private, false);
        assert_eq!(host.network_mode.as_deref(), Some(private.network.as_str()));
        assert_eq!(host.dns, Some(vec!["10.199.4.18".to_owned()]));
        assert_eq!(host.extra_hosts, None);
        assert_eq!(host.cap_add, Some(vec!["SYS_PTRACE".to_owned()]));
        for bypass in BYPASS_CAPABILITIES {
            assert!(host.cap_drop.as_ref().unwrap().contains(&bypass.to_owned()));
        }
        assert!(refusal(&host, false).is_none());
        let privileged = HostConfig {
            privileged: Some(true),
            ..HostConfig::default()
        };
        assert!(refusal(&privileged, false).is_some());
    }

    #[test]
    fn a_guest_with_a_kernel_of_its_own_keeps_its_capabilities_because_the_bridge_holds_it_in() {
        let private = private();
        let mut host = HostConfig {
            cap_add: Some(vec!["ALL".into()]),
            ..HostConfig::default()
        };
        assert!(
            refusal(&host, true).is_none(),
            "its capabilities act inside the guest"
        );
        restrict_host(&mut host, &private, true);
        assert_eq!(host.network_mode.as_deref(), Some(private.network.as_str()));
        assert_eq!(host.dns, Some(vec!["10.199.4.18".to_owned()]));
        assert_eq!(host.cap_add, Some(vec!["ALL".to_owned()]));
        assert!(host.cap_drop.is_none(), "nothing is taken from it");
        let privileged = HostConfig {
            privileged: Some(true),
            ..HostConfig::default()
        };
        assert!(
            refusal(&privileged, true).is_some(),
            "privileged still reaches the host"
        );
    }

    #[test]
    fn a_proxy_runs_as_the_node_without_capabilities_on_its_units_network() {
        let private = private();
        let body = proxy_body(
            &private,
            "busybox:latest",
            "/var/lib/orch-node/tools/orch-connector/current",
            "/var/lib/orch-node/egress/orch-environment-0199abcd1234",
            "gateway.orch.internal:host-gateway",
            HashMap::new(),
            (1001, 1001),
        );
        assert_eq!(body.user.as_deref(), Some("1001:1001"));
        assert_eq!(
            body.cmd.unwrap(),
            vec![
                "proxy",
                "--dir",
                PROXY_DIR,
                "--listen",
                "10.199.4.18",
                "--host-inventory"
            ]
        );
        let host = body.host_config.unwrap();
        assert_eq!(host.cap_drop, Some(vec!["ALL".to_owned()]));
        assert_eq!(host.readonly_rootfs, Some(true));
        assert_eq!(host.network_mode.as_deref(), Some(private.network.as_str()));
        let sysctls = host.sysctls.unwrap();
        assert_eq!(sysctls[UNPRIVILEGED_PORTS], "0");
        assert_eq!(sysctls["net.ipv4.ip_forward"], "0");
        assert_eq!(sysctls["net.ipv6.conf.all.forwarding"], "0");
        let endpoint = &body.networking_config.unwrap().endpoints_config.unwrap()[&private.network];
        assert_eq!(
            endpoint
                .ipam_config
                .as_ref()
                .unwrap()
                .ipv4_address
                .as_deref(),
            Some("10.199.4.18")
        );
    }

    #[test]
    fn the_variables_name_the_proxy_in_both_cases_and_keep_loopback_local() {
        let variables: BTreeMap<String, String> = private().variables().into_iter().collect();
        assert_eq!(variables["HTTPS_PROXY"], "http://10.199.4.18:3128");
        assert_eq!(variables["https_proxy"], "http://10.199.4.18:3128");
        assert_eq!(variables["NO_PROXY"], NO_PROXY);
    }
}
