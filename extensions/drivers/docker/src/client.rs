//! Docker socket API access.
//!
//! One [`Daemon`] is the driver's connection to the daemon and what it learned of it when it connected
//! ([`DaemonFacts`]): which runtimes it has, whether it enforces limits, whether its kernel has overlayfs. The
//! lookups here answer `None` for what is not there, so that the callers can tell absence from failure, and every
//! error of the client library is turned into the driver contract's [`DriverError`] in one place ([`map_error`]).

use std::sync::Arc;
use std::time::Duration;

use bollard::Docker;
use bollard::errors::Error as DockerError;
use bollard::models::{
    ContainerInspectResponse, ContainerSummary, ImageInspect, NetworkInspect, SystemInfo,
    SystemInfoCgroupVersionEnum, Volume,
};
use bollard::query_parameters::{
    DownloadFromContainerOptionsBuilder, InspectContainerOptionsBuilder,
    ListContainersOptionsBuilder,
};
use futures::TryStreamExt;
use orch_driver_contract::driver::DriverError;

use crate::config::DockerConfig;

/// What the driver knows of the daemon it talks to, learned once when it connected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonFacts {
    /// The daemon's version, such as `29.5.2`.
    pub server_version: String,
    /// The API version the daemon speaks.
    pub api_version: String,
    /// The operating system the daemon runs on: `linux` for every daemon this driver supports.
    pub os_type: String,
    /// The architecture of the host, as the daemon names it (`x86_64`).
    pub architecture: String,
    /// The storage driver or snapshotter that keeps images and layers.
    pub storage_driver: String,
    /// Whether images are kept by the daemon's containerd (an image's identifier is then its manifest digest).
    pub containerd_store: bool,
    /// The version of the control groups: 1 or 2, 0 when the daemon does not say.
    pub cgroup_version: u8,
    /// Whether the daemon itself runs without root.
    pub rootless: bool,
    /// The names of the container runtimes the daemon has, sorted.
    pub runtimes: Vec<String>,
    /// The runtime a container gets that names none.
    pub default_runtime: String,
    /// Whether the kernel can limit memory.
    pub memory_limit: bool,
    /// Whether the kernel can limit memory plus swap.
    pub swap_limit: bool,
    /// Whether the kernel can limit processor time by quota.
    pub cpu_quota: bool,
    /// Whether the kernel can limit the number of processes.
    pub pids_limit: bool,
    /// The memory of the host, in bytes.
    pub memory_total_bytes: i64,
    /// The number of processors of the host.
    pub cpus: i64,
    /// Whether the storage driver can limit the disk a container's own layer may fill.
    pub disk_quota: bool,
    /// Whether the kernel of the host has overlayfs, which a private view is made of.
    pub kernel_overlay: bool,
    /// The directory the daemon keeps its data in.
    pub data_root: String,
    /// Whether this daemon was found, by trying it, to make a network whose bridge has no address on the host
    /// (`gateway_mode_ipv4` `isolated`) for a restricted unit and its proxy. Never assumed from a version.
    pub restricted_egress: bool,
}

impl DaemonFacts {
    /// Whether the limits of a unit are enforced here: memory, processor time and processes.
    #[must_use]
    pub fn limits_enforced(&self) -> bool {
        self.memory_limit && self.cpu_quota && self.pids_limit
    }

    /// Whether a unit's processes can be held still: the freezer of the control groups is there, which is the
    /// case with the second version and, for a daemon with root, with the first.
    #[must_use]
    pub fn can_pause(&self) -> bool {
        self.os_type == "linux"
            && (self.cgroup_version == 2 || (self.cgroup_version == 1 && !self.rootless))
    }

    /// Whether a private view can be made on this host: the daemon mounts an overlay for it, which needs the
    /// kernel to have one and the daemon to be allowed to mount.
    #[must_use]
    pub fn private_views(&self) -> bool {
        self.os_type == "linux" && self.kernel_overlay && !self.rootless
    }

    /// What the facts of a daemon come to, from its answer to `info` and `version`.
    #[must_use]
    pub fn from_info(info: &SystemInfo, server_version: &str, api_version: &str) -> Self {
        let mut runtimes: Vec<String> = info
            .runtimes
            .as_ref()
            .map(|runtimes| runtimes.keys().cloned().collect())
            .unwrap_or_default();
        runtimes.sort();
        let storage_driver = info.driver.clone().unwrap_or_default();
        let containerd_store = info.driver_status.iter().flatten().any(|pair| {
            pair.first().is_some_and(|key| key == "driver-type")
                && pair
                    .get(1)
                    .is_some_and(|value| value.starts_with("io.containerd.snapshotter"))
        });
        let cgroup_version = match info.cgroup_version {
            Some(SystemInfoCgroupVersionEnum::_1) => 1,
            Some(SystemInfoCgroupVersionEnum::_2) => 2,
            _ => 0,
        };
        let rootless = info
            .security_options
            .iter()
            .flatten()
            .any(|option| option.split(',').any(|part| part == "name=rootless"));
        Self {
            server_version: server_version.to_owned(),
            api_version: api_version.to_owned(),
            os_type: info.os_type.clone().unwrap_or_default(),
            architecture: info.architecture.clone().unwrap_or_default(),
            // Only these storage drivers give a container's own layer a size limit (`--storage-opt size`):
            // overlay2 needs an xfs file system with project quotas, which the daemon does not report.
            disk_quota: matches!(storage_driver.as_str(), "zfs" | "btrfs" | "devicemapper"),
            storage_driver,
            containerd_store,
            cgroup_version,
            rootless,
            runtimes,
            default_runtime: info.default_runtime.clone().unwrap_or_default(),
            memory_limit: info.memory_limit.unwrap_or(false),
            swap_limit: info.swap_limit.unwrap_or(false),
            cpu_quota: info.cpu_cfs_quota.unwrap_or(false),
            pids_limit: info.pids_limit.unwrap_or(false),
            memory_total_bytes: info.mem_total.unwrap_or(0),
            cpus: info.ncpu.unwrap_or(0),
            kernel_overlay: kernel_has_overlayfs(),
            data_root: info.docker_root_dir.clone().unwrap_or_default(),
            // Found by trying, when the driver starts (`networks::probe_restricted`).
            restricted_egress: false,
        }
    }
}

/// Whether the running kernel lists `overlay` among its file systems. The list is the kernel's, whoever reads
/// it, so a node in a container sees the host's.
fn kernel_has_overlayfs() -> bool {
    std::fs::read_to_string("/proc/filesystems")
        .map(|text| {
            text.lines()
                .any(|line| line.split_whitespace().last() == Some("overlay"))
        })
        .unwrap_or(false)
}

/// The driver's connection to a daemon.
#[derive(Debug, Clone)]
pub struct Daemon {
    docker: Docker,
    facts: Arc<DaemonFacts>,
}

impl Daemon {
    /// Connects to the socket of the configuration, settles on an API version both sides speak, and asks the
    /// daemon what it is.
    pub async fn connect(config: &DockerConfig) -> Result<Self, DriverError> {
        let docker = Docker::connect_with_socket(
            &config.socket,
            config.request_timeout_secs,
            bollard::API_DEFAULT_VERSION,
        )
        .map_err(map_error)?
        .negotiate_version()
        .await
        .map_err(map_error)?;
        let (info, version) =
            futures::try_join!(docker.info(), docker.version()).map_err(map_error)?;
        let facts = DaemonFacts::from_info(
            &info,
            version.version.as_deref().unwrap_or_default(),
            version.api_version.as_deref().unwrap_or_default(),
        );
        tracing::info!(
            version = %facts.server_version,
            storage = %facts.storage_driver,
            runtimes = ?facts.runtimes,
            "connected to the Docker daemon"
        );
        Ok(Self {
            docker,
            facts: Arc::new(facts),
        })
    }

    /// The client library's handle, for the requests that need no special care.
    #[must_use]
    pub fn docker(&self) -> &Docker {
        &self.docker
    }

    /// Container metadata remains available when a disappearing writable layer prevents the daemon
    /// measuring the host-wide list. That snapshot has unknown sizes and retains every dependency.
    pub async fn inventory_containers(&self) -> Result<Vec<ContainerSummary>, DriverError> {
        match self
            .docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .size(true)
                    .build(),
            ))
            .await
        {
            Ok(containers) => Ok(containers),
            Err(error) if status_of(&error) == Some(404) => self
                .docker
                .list_containers(Some(
                    ListContainersOptionsBuilder::default()
                        .all(true)
                        .size(false)
                        .build(),
                ))
                .await
                .map_err(map_error),
            Err(error) => Err(map_error(error)),
        }
    }

    /// A handle whose requests may take `timeout`: for what is long by nature.
    #[must_use]
    pub fn patient(&self, timeout: Duration) -> Docker {
        self.docker.clone().with_timeout(timeout)
    }

    /// What was learned of the daemon.
    #[must_use]
    pub fn facts(&self) -> &DaemonFacts {
        &self.facts
    }

    /// The same connection, with what trying found of restricted units' networks.
    #[must_use]
    pub fn with_restricted_egress(mut self, found: bool) -> Self {
        let mut facts = (*self.facts).clone();
        facts.restricted_egress = found;
        self.facts = Arc::new(facts);
        self
    }

    /// The container named or numbered `name`, or `None` when there is none.
    pub async fn container(
        &self,
        name: &str,
    ) -> Result<Option<ContainerInspectResponse>, DriverError> {
        absent_is_none(
            self.docker
                .inspect_container(
                    name,
                    Some(InspectContainerOptionsBuilder::default().build()),
                )
                .await,
        )
    }

    /// The volume called `name`, or `None` when there is none.
    pub async fn volume(&self, name: &str) -> Result<Option<Volume>, DriverError> {
        absent_is_none(self.docker.inspect_volume(name).await)
    }

    /// The network called `name`, or `None` when there is none.
    pub async fn network(&self, name: &str) -> Result<Option<NetworkInspect>, DriverError> {
        absent_is_none(self.docker.inspect_network(name, None).await)
    }

    /// The image `reference` names (a tag, a digest or an identifier), or `None` when the host has none.
    pub async fn image(&self, reference: &str) -> Result<Option<ImageInspect>, DriverError> {
        absent_is_none(self.docker.inspect_image(reference).await)
    }

    /// Puts the files of a tar archive into a container, below the directory `into`. The container need not run.
    pub async fn put_archive(
        &self,
        container: &str,
        into: &str,
        archive: Vec<u8>,
    ) -> Result<(), DriverError> {
        let options = bollard::query_parameters::UploadToContainerOptionsBuilder::default()
            .path(into)
            .build();
        self.docker
            .upload_to_container(
                container,
                Some(options),
                bollard::body_full(bytes::Bytes::from(archive)),
            )
            .await
            .map_err(map_error)
    }

    /// The content of the file `path` inside a container, or `None` when there is no such file. The container
    /// need not run.
    pub async fn read_file(
        &self,
        container: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, DriverError> {
        let options = DownloadFromContainerOptionsBuilder::default()
            .path(path)
            .build();
        let stream = self
            .docker
            .download_from_container(container, Some(options));
        let archive: Result<Vec<bytes::Bytes>, DockerError> = stream.try_collect().await;
        match archive {
            Ok(pieces) => {
                let all: Vec<u8> = pieces
                    .iter()
                    .flat_map(|piece| piece.iter().copied())
                    .collect();
                Ok(crate::archive::first_file(&all))
            }
            Err(error) if status_of(&error) == Some(404) => Ok(None),
            Err(error) => Err(map_error(error)),
        }
    }
}

/// The HTTP status of an answer of the daemon, when the error is one.
#[must_use]
pub fn status_of(error: &DockerError) -> Option<u16> {
    match error {
        DockerError::DockerResponseServerError { status_code, .. } => Some(*status_code),
        _ => None,
    }
}

/// `Ok(None)` for what the daemon says is not there; every other failure stays one.
fn absent_is_none<T>(result: Result<T, DockerError>) -> Result<Option<T>, DriverError> {
    match result {
        Ok(found) => Ok(Some(found)),
        Err(error) if status_of(&error) == Some(404) => Ok(None),
        Err(error) => Err(map_error(error)),
    }
}

/// What a failed request means in the terms of the driver contract: the daemon is not there or does not
/// answer, the request took too long, the daemon refused it as wrong, or the daemon failed.
#[must_use]
pub fn map_error(error: DockerError) -> DriverError {
    match &error {
        DockerError::RequestTimeoutError => DriverError::Timeout,
        DockerError::SocketNotFoundError(_)
        | DockerError::HyperLegacyError { .. }
        | DockerError::HyperResponseError { .. } => DriverError::Unavailable(error.to_string()),
        DockerError::IOError { err } if is_connection_failure(err.kind()) => {
            DriverError::Unavailable(error.to_string())
        }
        DockerError::DockerResponseServerError {
            status_code,
            message,
        } => match *status_code {
            _ if daemon_capacity_failure(message) => DriverError::NoCapacity(message.clone()),
            409 => DriverError::Busy(message.clone()),
            404 => DriverError::NotFound(message.clone()),
            400 | 422 => DriverError::InvalidSpec(message.clone()),
            503 => DriverError::Unavailable(message.clone()),
            _ => DriverError::Internal(error.to_string()),
        },
        _ => DriverError::Internal(error.to_string()),
    }
}

/// Docker communicates native storage and memory exhaustion in its error body, not a separate enum.
pub(crate) fn daemon_capacity_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "no space left on device",
        "cannot allocate memory",
        "out of memory",
        "disk quota exceeded",
    ]
    .iter()
    .any(|native| message.contains(native))
}

fn is_connection_failure(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind::{
        BrokenPipe, ConnectionAborted, ConnectionRefused, ConnectionReset, NotFound,
        PermissionDenied,
    };
    matches!(
        kind,
        ConnectionRefused
            | ConnectionReset
            | ConnectionAborted
            | BrokenPipe
            | NotFound
            | PermissionDenied
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    struct InventorySocket {
        requests: Arc<Mutex<Vec<String>>>,
        task: JoinHandle<()>,
    }

    impl Drop for InventorySocket {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn inventory_socket(replies: Vec<(u16, &'static str)>) -> (Daemon, InventorySocket) {
        let mut listening = None;
        for port in 41000..42000 {
            if let Ok(listener) = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
                listening = Some(listener);
                break;
            }
        }
        let listener = listening.expect("a loopback port for the controlled daemon");
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                    request.push(byte[0]);
                    // These controlled GETs have no body or authentication and fit one header page.
                    assert!(request.len() <= 8192);
                }
                observed
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                let response = format!(
                    "HTTP/1.1 {status} controlled\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (
            Daemon {
                docker: Docker::connect_with_http(
                    &format!("http://{address}"),
                    2,
                    bollard::API_DEFAULT_VERSION,
                )
                .unwrap(),
                facts: Arc::new(DaemonFacts::default()),
            },
            InventorySocket { requests, task },
        )
    }

    #[tokio::test]
    async fn a_disappearing_layer_keeps_the_live_inventory_and_its_ownership_and_dependencies() {
        let (daemon, server) = inventory_socket(vec![
            (404, r#"{"message":"rw layer snapshot not found for container removed"}"#),
            (200, r#"[{"Id":"live","Names":["/workspace"],"State":"running","Labels":{"orch.system":"selected","orch.driver":"docker"},"Mounts":[{"Name":"owned-volume"}]}]"#),
        ]).await;
        let listed = daemon.inventory_containers().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id.as_deref(), Some("live"));
        assert_eq!(
            listed[0].state.as_ref().map(ToString::to_string).as_deref(),
            Some("running")
        );
        assert_eq!(
            listed[0]
                .labels
                .as_ref()
                .unwrap()
                .get("orch.system")
                .map(String::as_str),
            Some("selected")
        );
        assert_eq!(
            listed[0].mounts.as_ref().unwrap()[0].name.as_deref(),
            Some("owned-volume")
        );
        assert_eq!(
            listed[0].size_rw, None,
            "a lost measurement is unknown, never zero"
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("size=true"));
        assert!(requests[1].contains("size=false"));
    }

    #[tokio::test]
    async fn a_successful_inventory_keeps_its_real_size_without_an_extra_request() {
        let (daemon, server) =
            inventory_socket(vec![(200, r#"[{"Id":"measured","SizeRw":7}]"#)]).await;
        let listed = daemon.inventory_containers().await.unwrap();
        assert_eq!(listed[0].size_rw, Some(7));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_daemon_failure_does_not_become_a_successful_or_empty_inventory() {
        let (daemon, server) = inventory_socket(vec![
            (503, r#"{"message":"daemon is unavailable"}"#),
            (200, "[]"),
        ])
        .await;
        assert!(matches!(
            daemon.inventory_containers().await,
            Err(DriverError::Unavailable(_))
        ));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    fn server_error(status_code: u16, message: &str) -> DockerError {
        DockerError::DockerResponseServerError {
            status_code,
            message: message.to_owned(),
        }
    }

    #[test]
    fn what_the_daemon_refuses_is_told_apart_from_what_it_fails_at() {
        assert_eq!(
            map_error(server_error(404, "No such container: x")),
            DriverError::NotFound("No such container: x".into())
        );
        assert!(matches!(
            map_error(server_error(400, "invalid mount config")),
            DriverError::InvalidSpec(_)
        ));
        assert!(matches!(
            map_error(server_error(409, "name in use")),
            DriverError::Busy(_)
        ));
        assert!(matches!(
            map_error(server_error(503, "starting")),
            DriverError::Unavailable(_)
        ));
        assert_eq!(
            map_error(DockerError::RequestTimeoutError),
            DriverError::Timeout
        );
        assert!(matches!(
            map_error(DockerError::SocketNotFoundError("/x".into())),
            DriverError::Unavailable(_)
        ));
        assert!(map_error(server_error(503, "x")).retryable());
    }

    #[test]
    fn the_facts_of_a_daemon_are_read_from_what_it_reports() {
        let info: SystemInfo = serde_json::from_value(serde_json::json!({
            "Driver": "overlayfs",
            "DriverStatus": [["driver-type", "io.containerd.snapshotter.v1"]],
            "MemoryLimit": true, "SwapLimit": false, "CpuCfsQuota": true, "PidsLimit": true,
            "CgroupVersion": "2", "OSType": "linux", "Architecture": "x86_64",
            "SecurityOptions": ["name=apparmor", "name=seccomp,profile=builtin", "name=cgroupns"],
            "Runtimes": { "runc": {"path": "runc"}, "io.containerd.runc.v2": {"path": "runc"} },
            "DefaultRuntime": "runc", "MemTotal": 16_000_000_000_i64, "NCPU": 10,
            "DockerRootDir": "/var/lib/docker"
        }))
        .unwrap();
        let facts = DaemonFacts::from_info(&info, "29.5.2", "1.54");
        assert!(facts.containerd_store && facts.limits_enforced() && facts.can_pause());
        assert!(!facts.rootless && !facts.disk_quota && !facts.swap_limit);
        assert_eq!(facts.runtimes, ["io.containerd.runc.v2", "runc"]);
        assert_eq!(facts.cgroup_version, 2);
        assert_eq!(facts.default_runtime, "runc");
        let rootless: SystemInfo = serde_json::from_value(serde_json::json!({
            "OSType": "linux", "CgroupVersion": "1",
            "SecurityOptions": ["name=rootless", "name=seccomp,profile=builtin"]
        }))
        .unwrap();
        let facts = DaemonFacts::from_info(&rootless, "", "");
        assert!(facts.rootless && !facts.can_pause() && !facts.private_views());
        assert!(!facts.limits_enforced());
    }
}
