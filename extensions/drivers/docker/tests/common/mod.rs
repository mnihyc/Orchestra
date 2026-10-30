//! Shared fixtures of the tests that run against a real Docker daemon.
//!
//! A [`Lab`] is a driver of its own with names and labels that cannot meet anybody else's: every name starts with
//! `orch-test-<token>`, every object carries the label `orch.test=<token>`, and the system the units belong to is
//! `orch-test-<token>`. Whatever a test makes through the lab is recorded by its exact name, and is removed when the
//! lab is dropped, also when the test fails; nothing is ever removed by a filter, and nothing the lab did not make
//! is touched. Where no daemon answers, [`Lab::start`] says so and the test returns.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    dead_code,
    reason = "a test fails by panicking; each test file uses some of this"
)]

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bytes::Bytes;
use futures::TryStreamExt;
use orch_blob_contract::store::{BlobError, ByteStream, stream_of};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::RuntimeSpec;
use orch_driver_contract::units::{UnitRef, UnitStateKind};
use orch_driver_docker::config::{DockerConfig, ToolCache};
use orch_driver_docker::{BlobSource, DockerDriver};
use orch_types::{Hash, Json};
use serde_json::json;

/// The small image most tests run: pulled once on this machine.
pub const BUSYBOX: &str = "busybox:latest";

/// A second small image, for the tests that change the image of a unit.
pub const ALPINE: &str = "alpine:latest";

/// How many labs may exist at once: each makes up to two networks, and the daemon's pool of addresses is finite.
const LABS_AT_ONCE: usize = 3;

static PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(LABS_AT_ONCE);

/// What stands for the connector: it records what it was started with, does what the test asks, and waits. Started
/// as `proxy` (a restricted unit's proxy), it stands for the proxy program: a tunnel to the trusted gateway's exact
/// name and port of its orders, and a refusal of everything else, so that the tests prove the network around it.
pub const CONNECTOR_SCRIPT: &str = r#"#!/bin/sh
if [ "$1" = proxy ]; then
  dir="$3"
  cat > "$dir/handler.sh" <<'HANDLER'
#!/bin/sh
port=$(sed -n 's/.*"port":\([0-9]*\).*/\1/p' /run/orch-egress/orders.json | head -1)
read -r method target version
while read -r line; do
  line=$(printf '%s' "$line" | tr -d '\r')
  [ -z "$line" ] && break
done
if [ "$method" = CONNECT ] && [ "$target" = "gateway.orch.internal:$port" ]; then
  printf 'HTTP/1.1 200 Connection established\r\n\r\n'
  exec nc "${target%:*}" "${target##*:}"
fi
printf 'HTTP/1.1 403 Forbidden\r\n\r\n'
HANDLER
  chmod +x "$dir/handler.sh"
  exec nc -lk -p 3128 -e "$dir/handler.sh"
fi
out="$ORCH_DATA_DIR"
rm -f "$out/done.txt" "$out/run.txt"
{
  echo "args=$*"
  echo "uid=$(id -u) gid=$(id -g)"
  for name in ORCH_ROLES ORCH_GATEWAY_URL ORCH_CA_FILE ORCH_ENROLL_FILE ORCH_DATA_DIR ORCH_TOOLS_DIR ORCH_WORKSPACE_DIR ORCH_HOME_DIR ORCH_MOUNTS_FILE; do
    eval "value=\${$name-}"
    echo "$name=$value"
  done
  echo "ca_file=$(head -c 200 "$ORCH_CA_FILE" 2>/dev/null | tr '\n' '|')"
  echo "enroll_file=$(cat "$ORCH_ENROLL_FILE" 2>/dev/null)"
  echo "enroll_mode=$(stat -c '%a %u:%g' "$ORCH_ENROLL_FILE" 2>/dev/null)"
  echo "run_dir=$(stat -c '%a %u:%g' /run/orch 2>/dev/null)"
  echo "mounts_file=$(cat "$ORCH_MOUNTS_FILE" 2>/dev/null)"
  echo "tools=$(ls "$ORCH_TOOLS_DIR" 2>/dev/null | tr '\n' ' ')"
  echo "tool_marker=$(cat "$ORCH_TOOLS_DIR/codex/marker" 2>/dev/null)"
  echo "tools_dir_writable=$(touch "$ORCH_TOOLS_DIR/.probe" 2>/dev/null && echo yes || echo no)"
  echo "workspace_writable=$(touch "$ORCH_WORKSPACE_DIR/.probe" 2>/dev/null && echo yes || echo no)"
  echo "data_writable=$(touch "$ORCH_DATA_DIR/.probe" 2>/dev/null && echo yes || echo no)"
  echo "cgroup_memory=$(cat /sys/fs/cgroup/memory.max 2>/dev/null)"
  echo "cgroup_pids=$(cat /sys/fs/cgroup/pids.max 2>/dev/null)"
  echo "cgroup_cpu=$(cat /sys/fs/cgroup/cpu.max 2>/dev/null)"
  echo "hosts=$(grep gateway.orch.internal /etc/hosts | head -1)"
  echo "root_writable=$(touch /lab-probe 2>/dev/null && echo yes || echo no)"
} > "$out/launch.txt" 2>&1
if [ -n "$LAB_RUN" ]; then eval "$LAB_RUN" > "$out/run.txt" 2>&1; fi
echo done > "$out/done.txt"
if [ -n "$LAB_EXIT" ]; then exit "$LAB_EXIT"; fi
exec sleep 3600
"#;

/// What a lab made, by exact name.
#[derive(Debug, Default, Clone)]
struct Made {
    containers: Vec<String>,
    volumes: Vec<String>,
    networks: Vec<String>,
    images: Vec<String>,
}

/// A driver with names of its own, and the means to look at what it did.
pub struct Lab {
    pub token: String,
    pub system: String,
    pub docker: Docker,
    pub driver: DockerDriver,
    pub dir: tempfile::TempDir,
    pub config: DockerConfig,
    pub blobs: Arc<MemoryBlobs>,
    made: Mutex<Made>,
    _permit: tokio::sync::SemaphorePermit<'static>,
}

/// Blobs in memory, and how often each was opened.
#[derive(Default)]
pub struct MemoryBlobs {
    blobs: Mutex<HashMap<Hash, Bytes>>,
    opened: Mutex<HashMap<Hash, usize>>,
}

impl MemoryBlobs {
    pub fn put(&self, content: impl Into<Bytes>) -> Hash {
        let content = content.into();
        let hash = Hash::of(&content);
        self.blobs.lock().unwrap().insert(hash, content);
        hash
    }

    pub fn opened(&self, hash: &Hash) -> usize {
        self.opened.lock().unwrap().get(hash).copied().unwrap_or(0)
    }
}

#[async_trait]
impl BlobSource for MemoryBlobs {
    async fn open(&self, hash: &Hash) -> Result<ByteStream, BlobError> {
        *self.opened.lock().unwrap().entry(*hash).or_default() += 1;
        let content = self
            .blobs
            .lock()
            .unwrap()
            .get(hash)
            .cloned()
            .ok_or(BlobError::NotFound(*hash))?;
        // In pieces, so that a stream of pieces is what the driver reads.
        let pieces: Vec<Bytes> = content
            .chunks(64 * 1024)
            .map(Bytes::copy_from_slice)
            .collect();
        Ok(stream_of(pieces))
    }
}

/// A random token for the names of one lab: twelve hexadecimal digits from the random end of an identifier.
fn new_token() -> String {
    let id = orch_types::new_id().simple().to_string();
    id[id.len() - 12..].to_owned()
}

impl Lab {
    /// A lab with the driver's default settings, or `None` when there is no daemon to test against.
    pub async fn start() -> Option<Self> {
        Self::with(|_, _| {}).await
    }

    /// A lab whose settings are adjusted by `adjust` before the driver connects.
    pub async fn with(adjust: impl FnOnce(&mut DockerConfig, &std::path::Path)) -> Option<Self> {
        let permit = PERMITS
            .acquire()
            .await
            .expect("the permits are never closed");
        let token = new_token();
        let system = format!("orch-test-{token}");
        let dir = tempfile::tempdir().expect("a directory for the lab");
        let connector = dir.path().join("connector");
        std::fs::create_dir_all(&connector).unwrap();
        let script = connector.join("orch-connector");
        std::fs::write(&script, CONNECTOR_SCRIPT).unwrap();
        make_executable(&script);
        let tools = dir.path().join("tools");
        std::fs::create_dir_all(tools.join("codex/1.0.0")).unwrap();
        std::fs::write(tools.join("codex/1.0.0/marker"), "codex 1.0.0\n").unwrap();
        let mut config = DockerConfig {
            labels: [
                ("orch.test".to_owned(), token.clone()),
                ("orch.system".to_owned(), system.clone()),
            ]
            .into(),
            image_repository: format!("orch-test-{token}-build"),
            tool_cache: ToolCache {
                dir: Some(tools),
                connector_dir: Some(connector),
            },
            build_cache_gb: 0,
            ..DockerConfig::default()
        };
        config.networks.env = format!("orch-test-{token}-env");
        config.egress.proxy_image = Some(BUSYBOX.to_owned());
        config.networks.ctl = format!("orch-test-{token}-ctl");
        // Another daemon than the usual one, such as a rootless one, is named by this variable.
        if let Ok(socket) = std::env::var("ORCH_TEST_DOCKER_SOCKET") {
            config.socket = socket;
        }
        adjust(&mut config, dir.path());
        let driver = match DockerDriver::connect(config.clone()).await {
            Ok(driver) => driver,
            Err(DriverError::Unavailable(why)) => {
                eprintln!(
                    "skipped: no Docker daemon is reachable at {} ({why})",
                    config.socket
                );
                return None;
            }
            Err(error) => panic!("the driver could not connect: {error}"),
        };
        let docker = Docker::connect_with_socket(&config.socket, 120, bollard::API_DEFAULT_VERSION)
            .expect("a second handle on the daemon");
        let blobs = Arc::new(MemoryBlobs::default());
        let driver = driver.with_blob_source(blobs.clone());
        let lab = Self {
            token,
            system,
            docker,
            driver,
            dir,
            config,
            blobs,
            made: Mutex::new(Made::default()),
            _permit: permit,
        };
        lab.have(BUSYBOX).await;
        Some(lab)
    }

    /// Makes sure the host has an image, pulling it through the driver when it has not (once for the machine).
    pub async fn have(&self, image: &str) {
        if self.docker.inspect_image(image).await.is_err() {
            self.driver
                .ensure_image(&json!({ "image": image }))
                .await
                .unwrap_or_else(|error| panic!("{image} could not be pulled: {error}"));
        }
    }

    /// A name that begins with `orch-test-` and the lab's token.
    pub fn name(&self, what: &str) -> String {
        format!("orch-test-{}-{what}", self.token)
    }

    /// The request to make a unit of the pool `kind`, named after `label`, with the default spec of a small
    /// environment (or provider) and the labels of the lab's system. `change` may alter the request before it is
    /// made.
    pub fn request(&self, kind: &str, label: &str, change: impl FnOnce(&mut Json)) -> RuntimeSpec {
        let id = orch_types::new_id();
        let name = self.name(label);
        let state = self.name(&format!("{label}-state"));
        let mut request = json!({
            "runtime_id": id, "kind": kind, "name": name, "driver": "docker",
            "spec": {
                "image": BUSYBOX, "user": { "uid": 1000, "gid": 1000 },
                "network": { "mode": "bridge", "egress": "open", "dns": [] },
                "security": { "read_only_rootfs": false, "no_new_privileges": true, "cap_add": [], "cap_drop": ["NET_RAW"] },
                "mounts": []
            },
            "resources": {},
            "roles": [ if kind == "environment" { "environment" } else { "provider" } ],
            "gateway_url": "wss://gateway.orch.internal:9443",
            "enroll_token": "enroll-token-of-a-test",
            "ca_cert": "-----BEGIN CERTIFICATE-----\nMIIBtestcertificate\n-----END CERTIFICATE-----\n",
            "tools": [ { "name": "codex", "version": "1.0.0" } ],
            "volumes": { "state": { "name": state, "storage": null } },
            "mounts": [], "env": {},
            "labels": { "orch.system": self.system, "orch.runtime_id": id, "orch.kind": kind, "orch.test": self.token }
        });
        if kind == "environment" {
            request["volumes"]["workspace"] =
                json!({ "name": self.name(&format!("{label}-ws")), "storage": null });
            request["volumes"]["home"] =
                json!({ "name": self.name(&format!("{label}-home")), "storage": null });
        }
        change(&mut request);
        serde_json::from_value(request).expect("a request")
    }

    /// Records what making `spec` makes, so that it is removed whatever becomes of the test.
    pub fn track(&self, spec: &RuntimeSpec) {
        let mut made = self.made.lock().unwrap();
        made.containers.push(spec.name.clone());
        made.containers.push(format!("{}-prep", spec.name));
        // A restricted unit's proxy and its host address observer, which go before its networks.
        made.containers.push(format!("{}-proxy", spec.name));
        made.containers.push(format!("{}-addresses", spec.name));
        made.volumes.push(format!("{}-run", spec.name));
        made.volumes
            .extend(spec.volumes.values().map(|volume| volume.name.clone()));
        for mount in &spec.mounts {
            if let Some(layer) = &mount.layer {
                made.volumes.push(format!("{}-view", layer.name));
                made.volumes.push(layer.name.clone());
            }
        }
        made.networks.extend([
            format!("{}-net", spec.name),
            format!("{}-out", spec.name),
            self.config.networks.env.clone(),
            self.config.networks.ctl.clone(),
            format!("{}-internal", self.config.networks.env),
            format!("{}-internal", self.config.networks.ctl),
        ]);
    }

    /// Records a volume the test made by itself.
    pub fn track_volume(&self, name: &str) {
        self.made.lock().unwrap().volumes.push(name.to_owned());
    }

    /// Records a container the test made by itself.
    pub fn track_container(&self, name: &str) {
        self.made.lock().unwrap().containers.push(name.to_owned());
    }

    /// Records an image tag the test made or the driver made for it.
    pub fn track_image(&self, tag: &str) {
        self.made.lock().unwrap().images.push(tag.to_owned());
    }

    /// Makes a unit through the driver and records what it made.
    pub async fn create(&self, spec: &RuntimeSpec) -> UnitRef {
        self.track(spec);
        self.driver
            .create(spec)
            .await
            .unwrap_or_else(|error| panic!("{} could not be made: {error}", spec.name))
    }

    /// Makes and starts a unit, and waits until its connector has done what it was asked.
    pub async fn run(&self, spec: &RuntimeSpec) -> UnitRef {
        let unit = self.create(spec).await;
        self.driver
            .start(&unit)
            .await
            .unwrap_or_else(|error| panic!("{} could not start: {error}", spec.name));
        self.wait_done(&unit).await;
        unit
    }

    /// The text of a file inside a unit's container, running or not; `None` when there is no such file.
    pub async fn read(&self, unit: &UnitRef, path: &str) -> Option<String> {
        let options = bollard::query_parameters::DownloadFromContainerOptionsBuilder::default()
            .path(path)
            .build();
        let pieces: Result<Vec<Bytes>, _> = self
            .docker
            .download_from_container(&unit.reference, Some(options))
            .try_collect()
            .await;
        let all: Vec<u8> = pieces
            .ok()?
            .iter()
            .flat_map(|piece| piece.iter().copied())
            .collect();
        let mut archive = tar::Archive::new(all.as_slice());
        for entry in archive.entries().ok()? {
            let mut entry = entry.ok()?;
            if entry.header().entry_type().is_file() {
                let mut text = String::new();
                entry.read_to_string(&mut text).ok()?;
                return Some(text);
            }
        }
        None
    }

    /// Waits for the file a connector writes when it has done what it was asked.
    pub async fn wait_done(&self, unit: &UnitRef) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if self.read(unit, "/var/lib/orch/done.txt").await.as_deref() == Some("done\n") {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{} did not finish; log:\n{}",
                unit.name,
                self.logs(unit).await
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Waits until a unit is no longer running.
    pub async fn wait_stopped(&self, unit: &UnitRef) -> orch_driver_contract::units::UnitState {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let state = self.driver.inspect(unit).await.unwrap();
            if state.state != UnitStateKind::Running {
                return state;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{} is still running",
                unit.name
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// What the unit printed.
    pub async fn logs(&self, unit: &UnitRef) -> String {
        let options = bollard::query_parameters::LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .build();
        let lines: Result<Vec<_>, _> = self
            .docker
            .logs(&unit.reference, Some(options))
            .try_collect()
            .await;
        lines
            .map(|lines| lines.iter().map(ToString::to_string).collect::<String>())
            .unwrap_or_default()
    }

    /// The facts the connector recorded, as `name -> value`.
    pub async fn launch(&self, unit: &UnitRef) -> HashMap<String, String> {
        let text = self
            .read(unit, "/var/lib/orch/launch.txt")
            .await
            .expect("the connector recorded what it found");
        text.lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect()
    }

    /// The container of a unit as the daemon describes it.
    pub async fn inspect(&self, unit: &UnitRef) -> bollard::models::ContainerInspectResponse {
        self.docker
            .inspect_container(&unit.reference, None)
            .await
            .expect("the container is there")
    }

    /// Whether the daemon has a container of this name.
    pub async fn has_container(&self, name: &str) -> bool {
        self.docker.inspect_container(name, None).await.is_ok()
    }

    /// Whether the daemon has a network of this name.
    pub async fn has_network(&self, name: &str) -> bool {
        self.docker.inspect_network(name, None).await.is_ok()
    }

    /// Whether the daemon has a volume of this name.
    pub async fn has_volume(&self, name: &str) -> bool {
        self.docker.inspect_volume(name).await.is_ok()
    }

    /// The path of the directory a test can put files in for the daemon to find (the daemon is on this machine).
    pub fn path(&self, what: &str) -> PathBuf {
        let path = self.dir.path().join(what);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The numbers of the user running the tests, for files that the lab's units write and that must be removable.
pub fn my_ids(path: &std::path::Path) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).unwrap();
    (meta.uid(), meta.gid())
}

impl Drop for Lab {
    fn drop(&mut self) {
        let made = self
            .made
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let docker = self.docker.clone();
        let driver = self.driver.clone();
        let token = self.token.clone();
        let system = self.system.clone();
        // A fresh runtime on a thread of its own: the test's runtime may be shutting down, and whatever the test
        // ended with, this has to run.
        let outcome = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(remove_all(&docker, &driver, &token, &system, made));
        })
        .join();
        if outcome.is_err() {
            eprintln!("the cleanup of the lab {} failed", self.token);
        }
    }
}

/// Removes what a lab made, by exact name, and only when it carries the lab's label.
async fn remove_all(docker: &Docker, driver: &DockerDriver, token: &str, system: &str, made: Made) {
    use bollard::query_parameters::{
        RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder, RemoveVolumeOptionsBuilder,
    };
    let mine = |labels: Option<&HashMap<String, String>>| {
        labels
            .and_then(|labels| labels.get("orch.test"))
            .map(String::as_str)
            == Some(token)
    };
    for name in &made.containers {
        if let Ok(container) = docker.inspect_container(name, None).await {
            let labels = container
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref());
            if mine(labels) {
                let options = RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(true)
                    .build();
                if let Err(error) = docker.remove_container(name, Some(options)).await {
                    eprintln!("lab {token}: container {name} could not be removed: {error}");
                }
            } else {
                eprintln!("lab {token}: container {name} is not the lab's and is left alone");
            }
        }
    }
    for name in &made.volumes {
        if let Ok(volume) = docker.inspect_volume(name).await {
            if mine(Some(&volume.labels)) {
                let options = RemoveVolumeOptionsBuilder::default().force(true).build();
                if let Err(error) = docker.remove_volume(name, Some(options)).await {
                    eprintln!("lab {token}: volume {name} could not be removed: {error}");
                }
            } else {
                eprintln!("lab {token}: volume {name} is not the lab's and is left alone");
            }
        }
    }
    for name in &made.networks {
        if let Ok(network) = docker.inspect_network(name, None).await {
            if mine(network.labels.as_ref()) {
                if let Err(error) = docker.remove_network(name).await {
                    eprintln!("lab {token}: network {name} could not be removed: {error}");
                }
            } else {
                eprintln!("lab {token}: network {name} is not the lab's and is left alone");
            }
        }
    }
    // Select only the fixture's labelled cache objects, through the production inventory seam.
    let mut pending = driver
        .resources(system)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|item| {
            matches!(
                item.resource.kind,
                orch_driver_contract::resources::ResourceKind::Image
                    | orch_driver_contract::resources::ResourceKind::Build
                    | orch_driver_contract::resources::ResourceKind::KeptLayer
            )
        })
        .collect::<Vec<_>>();
    while !pending.is_empty() {
        let before = pending.len();
        let mut retry = Vec::new();
        for item in pending {
            if driver
                .remove_resource(system, &item.resource)
                .await
                .is_err()
            {
                retry.push(item);
            }
        }
        if retry.len() == before {
            eprintln!(
                "lab {token}: {} dependent cache objects remain",
                retry.len()
            );
            break;
        }
        pending = retry;
    }
    for tag in &made.images {
        // Kept snapshots have canonical environment/generation names. Their copied lab label
        // proves ownership just as it does for units; an unrelated image is still left alone.
        let labelled = match docker.inspect_image(tag).await {
            Ok(image) => mine(
                image
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref()),
            ),
            Err(_) => false,
        };
        if !tag.contains(token) && !labelled {
            eprintln!("lab {token}: image {tag} does not carry the lab's token and is left alone");
            continue;
        }
        // An explicitly recorded extra alias is removed alone; its unselected parents remain.
        let options = RemoveImageOptionsBuilder::default().noprune(true).build();
        if let Err(error) = docker.remove_image(tag, Some(options), None).await
            && !error.to_string().contains("No such image")
        {
            eprintln!("lab {token}: image {tag} could not be removed: {error}");
        }
    }
}

/// The address of the host on the daemon's default bridge: what `host-gateway` stands for, and an address the
/// units of a lab can reach and that is on this machine alone.
pub async fn bridge_address(docker: &Docker) -> Option<String> {
    let network = docker.inspect_network("bridge", None).await.ok()?;
    network
        .ipam?
        .config?
        .into_iter()
        .find_map(|config| config.gateway)
}

/// A listener that tells whoever connects that it is the gateway, on an address given and a port from the range
/// this machine allows for tests. It runs until the value is dropped.
pub struct Gateway {
    pub port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// How many connections it took: what proves that somebody did not reach it.
    pub accepted: Arc<std::sync::atomic::AtomicUsize>,
}

impl Gateway {
    /// Listens on `address`, on a port chosen at random in the allowed range.
    pub fn listen(address: &str) -> Self {
        (0..200)
            .find_map(|_| Self::listen_on(address, random_port()))
            .expect("a free port in the range for tests")
    }

    /// Listens on `address` and `port`, when they are free.
    pub fn listen_on(address: &str, port: u16) -> Option<Self> {
        use std::io::Write;
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind((address, port)).ok()?;
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&accepted);
        std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, from)) => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        let _ = writeln!(stream, "gateway-hello from {}", from.ip());
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
        Some(Self {
            port,
            stop,
            accepted,
        })
    }
}

/// A port of the range this machine allows for tests, chosen at random.
pub fn random_port() -> u16 {
    let id = orch_types::new_id();
    41_000 + u16::from_be_bytes([id.as_bytes()[14], id.as_bytes()[15]]) % 1000
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The address a container has on a network.
pub async fn address_on(docker: &Docker, container: &str, network: &str) -> String {
    let inspected = docker.inspect_container(container, None).await.unwrap();
    inspected
        .network_settings
        .and_then(|settings| settings.networks)
        .and_then(|networks| {
            networks
                .get(network)
                .and_then(|endpoint| endpoint.ip_address.clone())
        })
        .filter(|address| !address.is_empty())
        .expect("an address on the network")
}
