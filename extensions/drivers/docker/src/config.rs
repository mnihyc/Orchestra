//! The driver's own settings: where the daemon is, what the host offers, where its storages and tools are.
//!
//! These are the settings of the `docker` driver in the node's configuration, plus the few that only matter when a
//! node is not on the daemon's host (paths as the daemon sees them) or when the daemon is shared (labels and names
//! of everything the driver makes itself). Every member has the default a stock installation wants, so an empty
//! table is a working configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the daemon listens unless the configuration says otherwise.
pub const DEFAULT_SOCKET: &str = "/var/run/docker.sock";

/// What `gateway.orch.internal` resolves to inside a unit: the daemon's own name for the host the units run on.
pub const DEFAULT_GATEWAY_HOST: &str = "host-gateway";

/// The longest one build may take: half an hour, enough for a toolchain image built from nothing on a modest
/// host, and an end for a build that hangs.
pub const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 1_800;

/// Builds running at once: one, because a build takes the processors that agents are using.
pub const DEFAULT_BUILD_PARALLEL: u32 = 1;

/// The build cache kept on the host, in GB: a few toolchain images' worth of layers.
pub const DEFAULT_BUILD_CACHE_GB: u64 = 20;

/// The network of environments.
pub const DEFAULT_ENV_NETWORK: &str = "orch-env";

/// The network of providers' runtimes.
pub const DEFAULT_CTL_NETWORK: &str = "orch-ctl";

/// The repository the images the driver builds and loads are named in; the tag says which.
pub const DEFAULT_IMAGE_REPOSITORY: &str = "orch-build";

/// The largest a unit's log may grow before the oldest part is dropped: 10 MiB, as the services of a deployment
/// keep theirs.
pub const DEFAULT_LOG_MAX_SIZE: &str = "10m";

/// How many files of that size a unit's log is kept in.
pub const DEFAULT_LOG_MAX_FILES: u32 = 5;

/// The protocols a Git context may be fetched with: those that need no credential.
pub const DEFAULT_GIT_SCHEMES: [&str; 3] = ["https", "http", "git"];

/// How long a request to the daemon may take before it is given up, in seconds: two minutes, which is the client
/// library's own figure and longer than anything the daemon answers at once. Operations that are long by nature
/// (pulling, building, loading, stopping with a grace) set their own.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 120;

// SHA-256 image content IDs have 256 bits written in hexadecimal.
const IMAGE_DIGEST_HEX_BYTES: usize = 64;

fn default_socket() -> String {
    DEFAULT_SOCKET.to_owned()
}

fn default_gateway_host() -> String {
    DEFAULT_GATEWAY_HOST.to_owned()
}

fn default_true() -> bool {
    true
}

fn default_build_timeout() -> u64 {
    DEFAULT_BUILD_TIMEOUT_SECS
}

fn default_build_parallel() -> u32 {
    DEFAULT_BUILD_PARALLEL
}

fn default_build_cache() -> u64 {
    DEFAULT_BUILD_CACHE_GB
}

fn default_image_repository() -> String {
    DEFAULT_IMAGE_REPOSITORY.to_owned()
}

fn default_log_size() -> String {
    DEFAULT_LOG_MAX_SIZE.to_owned()
}

fn default_log_files() -> u32 {
    DEFAULT_LOG_MAX_FILES
}

fn default_git_schemes() -> Vec<String> {
    DEFAULT_GIT_SCHEMES
        .iter()
        .map(|scheme| (*scheme).to_owned())
        .collect()
}

fn default_request_timeout() -> u64 {
    DEFAULT_REQUEST_TIMEOUT_SECS
}

fn default_helper_cleanup_retry() -> u64 {
    1_000
}

/// A named place on the host where volumes are made; the first of the places a node declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Its name, which a template or a setting names.
    pub name: String,
    /// The directory below which the data of its volumes lives.
    pub path: PathBuf,
    /// Whether it is where a volume goes that names no storage.
    #[serde(default)]
    pub default: bool,
}

/// A directory of the node's file system, and the same directory as the daemon sees it. A node in a container sees
/// the host's directories under other names; whatever is handed to the daemon must be a name the daemon knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMap {
    /// The directory as the node sees it.
    pub node: PathBuf,
    /// The same directory as the daemon sees it.
    pub host: PathBuf,
}

/// Where the programs a unit is given are kept, as the daemon sees the directories.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCache {
    /// The directory with a directory `<name>/<version>` for each harness build.
    #[serde(default)]
    pub dir: Option<PathBuf>,
    /// The directory that holds `orch-connector`, which every unit finds at `/opt/orch/bin`.
    #[serde(default)]
    pub connector_dir: Option<PathBuf>,
}

/// What the daemon is told to log in when it pulls from a registry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryAuth {
    /// The registry's host name, as it appears in an image reference (`ghcr.io`, `registry.example.com:5000`).
    pub server: String,
    /// The user name.
    #[serde(default)]
    pub username: Option<String>,
    /// The password or access token.
    #[serde(default)]
    pub password: Option<String>,
    /// An identity token, where the registry gives one instead of a password.
    #[serde(default)]
    pub identity_token: Option<String>,
}

impl std::fmt::Debug for RegistryAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryAuth")
            .field("server", &self.server)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<hidden>"))
            .field(
                "identity_token",
                &self.identity_token.as_ref().map(|_| "<hidden>"),
            )
            .finish()
    }
}

/// The networks units are put on: every unit, open or restricted, has a network of its own cut from one pool,
/// so that an operator's hook sees one stable subnet for each unit and the daemon's own address pools and
/// configuration stay as they are. The two names are those of the common bridges units stood on before: a unit
/// still on one is moved to a network of its own at its next start. A shared daemon may have networks of the
/// default names that are somebody else's, so the names can be changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkNames {
    /// The common bridge providers' runtimes stood on before each had a network of its own.
    #[serde(default = "default_ctl_network")]
    pub ctl: String,
    /// The common bridge environments stood on before each had a network of its own.
    #[serde(default = "default_env_network")]
    pub env: String,
    /// The pool, as an IPv4 range, every unit's own network is cut from (and a restricted unit's proxy's way
    /// out too).
    #[serde(default = "default_network_pool")]
    pub pool: String,
    /// The prefix length of one unit's network.
    #[serde(default = "default_network_prefix")]
    pub prefix: u8,
}

impl Default for NetworkNames {
    fn default() -> Self {
        Self {
            ctl: default_ctl_network(),
            env: default_env_network(),
            pool: default_network_pool(),
            prefix: default_network_prefix(),
        }
    }
}

impl NetworkNames {
    /// The pool, read; none when it is no IPv4 range or cannot hold one network of the prefix.
    #[must_use]
    pub fn pool(&self) -> Option<ipnet::Ipv4Net> {
        let pool: ipnet::Ipv4Net = self.pool.parse().ok()?;
        (self.prefix >= pool.prefix_len() && self.prefix <= MOST_PREFIX).then_some(pool)
    }
}

/// The address pool every unit's own network is cut from, unless the operator names another: a block of the
/// private range that Docker's default pools (172.17.0.0/16 and up, 192.168.0.0/16) do not hand out.
pub const DEFAULT_NETWORK_POOL: &str = "10.199.0.0/16";

/// The size of one unit's network: sixteen addresses, of which the unit and a restricted unit's proxy need two
/// and the rest are the bridge's own and room to spare. A pool of a /16 then holds 4,096 of them.
pub const DEFAULT_NETWORK_PREFIX: u8 = 28;

/// The longest prefix a unit's network may have: a /29 holds the bridge's addresses, the unit and its proxy.
const MOST_PREFIX: u8 = 29;

fn default_ctl_network() -> String {
    DEFAULT_CTL_NETWORK.to_owned()
}

fn default_env_network() -> String {
    DEFAULT_ENV_NETWORK.to_owned()
}

fn default_network_pool() -> String {
    DEFAULT_NETWORK_POOL.to_owned()
}

fn default_network_prefix() -> u8 {
    DEFAULT_NETWORK_PREFIX
}

/// Where restricted units' proxies keep their orders and state. Their networks are cut from the pool every
/// unit's is ([`NetworkNames::pool`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressConfig {
    /// The operator's trusted sidecar image. Never taken from a runtime's requested image; none refuses restriction.
    #[serde(default)]
    pub proxy_image: Option<String>,
    /// The directory, as the node sees it, below which each restricted unit's proxy has a directory of its own:
    /// the node writes the proxy's orders and heartbeat there and reads its state. None: this host makes no
    /// restricted unit.
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

/// What the operator declares of one runtime of the daemon, which the daemon cannot tell.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSettings {
    /// What holds a unit in; a runtime that is not declared counts as a shared kernel.
    #[serde(default)]
    pub boundary: orch_driver_contract::isolation::Boundary,
    /// How it is called on screens; none takes the usual name.
    #[serde(default)]
    pub title: Option<String>,
    /// Whether software installed outside the volumes survives a stop; none takes what the runtime usually does.
    #[serde(default)]
    pub keeps_software_on_stop: Option<bool>,
    /// What each unit costs the host beyond its limits, in MiB; none takes the usual figure for the boundary.
    #[serde(default)]
    pub memory_overhead_mb: Option<i64>,
    /// Seconds a new unit's connector has to enrol; none takes the usual figure for the boundary.
    #[serde(default)]
    pub enrol_timeout_secs: Option<u64>,
}

/// The settings of the driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerConfig {
    /// The daemon's socket.
    #[serde(default = "default_socket")]
    pub socket: String,
    /// How long a request to the daemon may take, in seconds.
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    /// What `gateway.orch.internal` is made to resolve to inside every unit: an address, a name, or
    /// `host-gateway`, which the daemon turns into the address of the host.
    #[serde(default = "default_gateway_host")]
    pub gateway_host: String,
    /// Whether this host builds images from Dockerfiles. Off, it does not offer that source.
    #[serde(default = "default_true")]
    pub builds: bool,
    /// Nonsecret proxy and repository defaults, overridden by the template’s own build arguments.
    #[serde(default)]
    pub build_args: BTreeMap<String, String>,
    /// Carried-in image references mapped to verified immutable image IDs by the operator.
    #[serde(default)]
    pub offline_images: BTreeMap<String, String>,
    /// The longest one build may take, in seconds.
    #[serde(default = "default_build_timeout")]
    pub build_timeout_secs: u64,
    /// How many builds run at once.
    #[serde(default = "default_build_parallel")]
    pub build_parallel: u32,
    /// Accepted from older configuration files; cache removals are selected by the server's cleanup policy.
    #[serde(default = "default_build_cache")]
    pub build_cache_gb: u64,
    /// The pool every unit's own network is cut from, and the names of the common bridges units stood on before.
    #[serde(default)]
    pub networks: NetworkNames,
    /// The schemes of the addresses a Git context may be fetched from; only those that need no credential.
    #[serde(default = "default_git_schemes")]
    pub git_schemes: Vec<String>,
    /// The repository that built and loaded images are named in.
    #[serde(default = "default_image_repository")]
    pub image_repository: String,
    /// The named places for volumes. None declared means the one the daemon keeps volumes in.
    #[serde(default)]
    pub storages: Vec<StorageConfig>,
    /// Where the connector and the harness builds are.
    #[serde(default)]
    pub tool_cache: ToolCache,
    /// Directories whose names differ between the node and the daemon.
    #[serde(default)]
    pub path_map: Vec<PathMap>,
    /// Whether the node can see the directories it hands to the daemon, so that a missing one can be told from
    /// a present one before a unit is made. Off for a node in a container that sees none of them.
    #[serde(default = "default_true")]
    pub check_host_paths: bool,
    /// The registries the daemon logs in to when it pulls.
    #[serde(default)]
    pub registries: Vec<RegistryAuth>,
    /// Labels put on everything the driver makes by itself: its networks, its helpers, the images it builds and
    /// the volumes it creates. A shared daemon tells them from other people's by these.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// The size of a unit's log file before it rolls over.
    #[serde(default = "default_log_size")]
    pub log_max_size: String,
    /// How many log files of a unit are kept.
    #[serde(default = "default_log_files")]
    pub log_max_files: u32,
    /// A directory of seccomp profiles `<name>.json`, which a capability names by `seccomp=<name>`.
    #[serde(default)]
    pub seccomp_dir: Option<PathBuf>,
    /// Trusted operator image in which the static node helper is run; no unit-requested image is used.
    #[serde(default)]
    pub live_helper_image: Option<String>,
    /// The node's static binary as the node sees it; path_map gives its daemon-side name.
    #[serde(default)]
    pub live_helper_binary: Option<PathBuf>,
    /// Milliseconds between retries when a cancelled helper cannot yet be removed from its daemon.
    #[serde(default = "default_helper_cleanup_retry")]
    pub helper_cleanup_retry_ms: u64,
    /// Preloaded image used for isolated native harness checks.
    #[serde(default)]
    pub conformance_image: Option<String>,
    /// Restricted units: where their proxies keep their files, and the pool of their networks.
    #[serde(default)]
    pub egress: EgressConfig,
    /// What the operator declares of each runtime of the daemon, by the name the daemon knows it by.
    #[serde(default)]
    pub runtimes: BTreeMap<String, RuntimeSettings>,
    /// The size of the file Docker inside keeps its data in, in GB. The file is sparse: it takes the room its
    /// contents take, and this is how far they may grow.
    #[serde(default = "default_docker_inside_gb")]
    pub docker_inside_gb: u64,
    /// The name servers of a unit under a runtime with a kernel of its own, whose daemon's resolver at 127.0.0.11
    /// cannot be reached from inside. Empty takes the host's own, read from its resolver file.
    #[serde(default)]
    pub resolvers: Vec<String>,
}

/// How far the file of Docker inside may grow, in GB: a toolchain's images and a few builds; the file is sparse.
pub const DEFAULT_DOCKER_INSIDE_GB: u64 = 100;

fn default_docker_inside_gb() -> u64 {
    DEFAULT_DOCKER_INSIDE_GB
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket: default_socket(),
            request_timeout_secs: default_request_timeout(),
            gateway_host: default_gateway_host(),
            builds: true,
            build_args: BTreeMap::new(),
            offline_images: BTreeMap::new(),
            build_timeout_secs: default_build_timeout(),
            build_parallel: default_build_parallel(),
            build_cache_gb: default_build_cache(),
            networks: NetworkNames::default(),
            git_schemes: default_git_schemes(),
            image_repository: default_image_repository(),
            storages: Vec::new(),
            tool_cache: ToolCache::default(),
            path_map: Vec::new(),
            check_host_paths: true,
            registries: Vec::new(),
            labels: BTreeMap::new(),
            log_max_size: default_log_size(),
            log_max_files: default_log_files(),
            seccomp_dir: None,
            live_helper_image: None,
            live_helper_binary: None,
            helper_cleanup_retry_ms: default_helper_cleanup_retry(),
            conformance_image: None,
            egress: EgressConfig::default(),
            runtimes: BTreeMap::new(),
            docker_inside_gb: default_docker_inside_gb(),
            resolvers: Vec::new(),
        }
    }
}

/// What is wrong with a configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigFault {
    /// A storage has no name, or two have one.
    #[error("storage names are given and each is used once: {0:?}")]
    StorageName(String),
    /// A storage's path is not absolute.
    #[error("the path of storage {0:?} is absolute")]
    StoragePath(String),
    /// More than one storage is the default.
    #[error("at most one storage is the default")]
    SeveralDefaults,
    /// Storages are declared and none is the default.
    #[error("storages are declared and one of them is the default")]
    NoDefault,
    /// The two networks have one name, or a name is empty.
    #[error("the two networks have names of their own")]
    NetworkNames,
    /// The pool of the units' networks is no IPv4 range that holds one network of the prefix.
    #[error(
        "the pool of the units' networks is an IPv4 range that holds at least one network of the prefix, which is at most 29"
    )]
    NetworkPool,
    /// A directory given to the daemon is not absolute.
    #[error("{0} is an absolute path")]
    NotAbsolute(&'static str),
    /// No build may run, and building is on.
    #[error("build_parallel is at least 1")]
    NoBuilds,
    /// A carried-in reference must resolve to an immutable image content ID.
    #[error("carried-in images map to sha256 image content IDs")]
    OfflineImage,
}

impl DockerConfig {
    /// The immutable local ID for a carried-in reference, or the original reference for ordinary registry use.
    #[must_use]
    pub fn local_image<'a>(&'a self, reference: &'a str) -> &'a str {
        self.offline_images
            .get(reference)
            .map_or(reference, String::as_str)
    }

    /// Removes configured registry secrets before daemon text is shown in logs, progress or failures.
    #[must_use]
    pub fn redact_registry_text(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in self
            .registries
            .iter()
            .flat_map(|auth| [auth.password.as_deref(), auth.identity_token.as_deref()])
            .flatten()
            .filter(|secret| !secret.is_empty())
        {
            text = text.replace(secret, "<hidden>");
        }
        text
    }

    /// Checks that the configuration holds together. Every fault is returned.
    pub fn check(&self) -> Result<(), Vec<ConfigFault>> {
        let mut faults = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for storage in &self.storages {
            if storage.name.is_empty() || seen.contains(&storage.name.as_str()) {
                faults.push(ConfigFault::StorageName(storage.name.clone()));
            }
            seen.push(&storage.name);
            if !storage.path.is_absolute() {
                faults.push(ConfigFault::StoragePath(storage.name.clone()));
            }
        }
        let defaults = self
            .storages
            .iter()
            .filter(|storage| storage.default)
            .count();
        if defaults > 1 {
            faults.push(ConfigFault::SeveralDefaults);
        } else if defaults == 0 && !self.storages.is_empty() {
            faults.push(ConfigFault::NoDefault);
        }
        if self.networks.ctl.is_empty()
            || self.networks.env.is_empty()
            || self.networks.ctl == self.networks.env
        {
            faults.push(ConfigFault::NetworkNames);
        }
        if self.networks.pool().is_none() {
            faults.push(ConfigFault::NetworkPool);
        }
        for (what, path) in [
            ("the tool cache", &self.tool_cache.dir),
            ("the connector directory", &self.tool_cache.connector_dir),
        ] {
            if path.as_ref().is_some_and(|path| !path.is_absolute()) {
                faults.push(ConfigFault::NotAbsolute(what));
            }
        }
        if self.builds && self.build_parallel == 0 {
            faults.push(ConfigFault::NoBuilds);
        }
        if self.offline_images.values().any(|value| {
            value.strip_prefix("sha256:").is_none_or(|hex| {
                hex.len() != IMAGE_DIGEST_HEX_BYTES
                    || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        }) {
            faults.push(ConfigFault::OfflineImage);
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }

    /// The directory as the daemon names it: the first mapping whose node directory contains `path` rewrites it;
    /// a path none contains is the daemon's own name already.
    #[must_use]
    pub fn host_path(&self, path: &Path) -> PathBuf {
        for map in &self.path_map {
            if let Ok(rest) = path.strip_prefix(&map.node) {
                return map.host.join(rest);
            }
        }
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_table_is_a_working_configuration() {
        let config: DockerConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(config, DockerConfig::default());
        assert_eq!(config.check(), Ok(()));
        assert!(config.builds && config.build_parallel == 1);
        assert_eq!(config.gateway_host, "host-gateway");
        assert_eq!(config.networks.env, "orch-env");
    }

    #[test]
    fn a_misspelled_setting_is_refused() {
        let error = serde_json::from_value::<DockerConfig>(json!({ "gatewayhost": "x" }));
        assert!(error.is_err());
    }

    #[test]
    fn storages_have_names_absolute_paths_and_one_default() {
        let storage = |name: &str, path: &str, default| StorageConfig {
            name: name.into(),
            path: path.into(),
            default,
        };
        let mut config = DockerConfig {
            storages: vec![
                storage("fast", "/mnt/fast", true),
                storage("bulk", "/mnt/bulk", false),
            ],
            ..DockerConfig::default()
        };
        assert_eq!(config.check(), Ok(()));
        config.storages[1].default = true;
        assert_eq!(config.check(), Err(vec![ConfigFault::SeveralDefaults]));
        config.storages[1].default = false;
        config.storages[0].default = false;
        assert_eq!(config.check(), Err(vec![ConfigFault::NoDefault]));
        config.storages[0].default = true;
        config.storages[1] = storage("fast", "relative", false);
        let faults = config.check().unwrap_err();
        assert!(faults.contains(&ConfigFault::StorageName("fast".into())));
        assert!(faults.contains(&ConfigFault::StoragePath("fast".into())));
    }

    #[test]
    fn a_directory_of_the_node_is_handed_to_the_daemon_under_the_name_the_daemon_knows() {
        let config = DockerConfig {
            path_map: vec![PathMap {
                node: "/var/lib/orch-node".into(),
                host: "/srv/orch/runtime/node".into(),
            }],
            ..DockerConfig::default()
        };
        assert_eq!(
            config.host_path(Path::new("/var/lib/orch-node/tools/codex/1")),
            PathBuf::from("/srv/orch/runtime/node/tools/codex/1")
        );
        assert_eq!(
            config.host_path(Path::new("/var/lib/orch-node2/x")),
            PathBuf::from("/var/lib/orch-node2/x"),
            "a directory whose name only starts like the node's is not inside it"
        );
        assert_eq!(
            config.host_path(Path::new("/data/src")),
            PathBuf::from("/data/src")
        );
    }

    #[test]
    fn every_units_network_is_cut_from_one_pool_that_holds_one_of_the_prefix() {
        let config = DockerConfig::default();
        assert_eq!(config.networks.pool().unwrap().to_string(), "10.199.0.0/16");
        assert_eq!(config.networks.prefix, 28);
        let read: DockerConfig =
            serde_json::from_value(json!({ "networks": { "pool": "10.77.0.0/20", "prefix": 27 } }))
                .unwrap();
        assert_eq!(read.check(), Ok(()));
        assert_eq!(
            read.networks.env, "orch-env",
            "the old bridge's name keeps its default"
        );
        for (pool, prefix) in [
            ("10.77.0.0/20", 30),
            ("10.77.0.0/20", 19),
            ("fd00::/64", 28),
            ("nonsense", 28),
        ] {
            let config = DockerConfig {
                networks: NetworkNames {
                    pool: pool.to_owned(),
                    prefix,
                    ..NetworkNames::default()
                },
                ..DockerConfig::default()
            };
            assert_eq!(
                config.check(),
                Err(vec![ConfigFault::NetworkPool]),
                "{pool} /{prefix}"
            );
        }
        assert!(
            serde_json::from_value::<DockerConfig>(json!({ "egress": { "pool": "10.0.0.0/8" } }))
                .is_err(),
            "restricted units have no pool of their own any more"
        );
    }

    #[test]
    fn a_password_is_never_printed() {
        let auth = RegistryAuth {
            server: "ghcr.io".into(),
            username: Some("bot".into()),
            password: Some("hunter2".into()),
            identity_token: None,
        };
        assert!(!format!("{auth:?}").contains("hunter2"));
    }
}
