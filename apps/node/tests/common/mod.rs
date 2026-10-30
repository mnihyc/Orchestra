//! A whole system on loopback and the Docker bridge, and a node on a real daemon: a database, the secrets of a deployment, the API's listener, a gateway, a node enrolled with a host token, and the daemon's own containers.
//!
//! Everything this makes on the daemon is named `orch-test-node-<token>…`, carries the label `orch.test=<token>`,
//! and is removed by its exact name when the lab is dropped, also when the test fails; nothing is ever removed by
//! a filter, and nothing that the lab did not make is touched. Where no daemon answers, or the static connector is
//! not built, [`Lab::start`] says so and the test returns.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "a test fails by panicking; each test file uses some of what is here"
)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use bollard::Docker;
use orch_api_core::backplane::{ApplyError, EventApplier, Link, LinkParts};
use orch_api_core::certificates::{TokenFor, TokenRequest};
use orch_api_core::server;
use orch_api_core::state::{App, AppState};
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_connector_core::connector::Connector;
use orch_connector_core::enrollment::{DataDir, HttpsEnroll};
use orch_connector_core::session::tls_dialer;
use orch_db::queries::resources::testing as rows;
use orch_db::secrets::MasterKey;
use orch_db::{Conn, Db, DbError};
use orch_gateway_core::{Gateway, GatewayParts, Settings};
use orch_link::streams::Delivery;
use orch_link_contract::enrollment::Trust;
use orch_node::docker;
use orch_node_core::drivers::{DriverHost, Registrations};
use orch_node_core::enrollment::{HostEnrollment, Wiring, builder, enroll_host};
use orch_node_core::images::BlobDesk;
use orch_node_core::role::Prepared;
use orch_node_core::settings::NodeSettings;
use orch_pki::ca::{CA_CERT_FILE, CaFiles, CertificateAuthority, Holder};
use orch_pki::certificates::{Certificate, LeafKey, ServiceName, write_file, write_private_file};
use orch_pki::verification::TlsIdentity;
use orch_testkit::database::TestDb;
use orch_types::{Clock, Id, Json, SystemClock, new_id};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The small image the units are made from: pulled once on this machine.
pub const BUSYBOX: &str = "busybox:latest";

/// How long a test waits for something that has to happen.
pub const PATIENCE: Duration = Duration::from_secs(90);

/// Where a test may listen on the bridge: the range this machine allows for tests.
const PORT_FIRST: u16 = 41_000;

/// How many ports there are in that range.
const PORT_COUNT: u16 = 1_000;

/// Waits until `check` holds, for `PATIENCE`.
pub async fn eventually<F, Fut>(what: &str, check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    eventually_for(PATIENCE, what, check).await;
}

/// Waits until `check` holds, for `patience`.
pub async fn eventually_for<F, Fut>(patience: Duration, what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let waited = tokio::time::timeout(patience, async {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "gave up waiting for {what}");
}

/// A port of the range for tests, chosen at random.
pub fn random_port() -> u16 {
    let id = new_id();
    PORT_FIRST + u16::from_be_bytes([id.as_bytes()[14], id.as_bytes()[15]]) % PORT_COUNT
}

/// Listens on `address` at a port of the range for tests that is free.
pub async fn listen_in_range(address: &str) -> TcpListener {
    for _ in 0..500 {
        if let Ok(listener) = TcpListener::bind((address, random_port())).await {
            return listener;
        }
    }
    panic!("no port of the range for tests is free on {address}");
}

// ---------------------------------------------------------------------------------------------- the daemon

/// What a lab made, by exact name.
#[derive(Debug, Default, Clone)]
struct Made {
    containers: Vec<String>,
    volumes: Vec<String>,
    networks: Vec<String>,
}

/// The daemon of this machine, and the names and labels of one test on it.
pub struct Lab {
    pub token: String,
    pub system: String,
    pub docker: Docker,
    pub socket: String,
    pub bridge: String,
    /// The static connector the units run.
    pub connector: PathBuf,
    made: Mutex<Made>,
}

fn socket_of_tests() -> String {
    std::env::var("ORCH_TEST_DOCKER_SOCKET").unwrap_or_else(|_| "/var/run/docker.sock".to_owned())
}

/// The static connector: named by `ORCH_TEST_CONNECTOR_BINARY`, or built for the musl target in the target
/// directory this test runs from.
fn static_connector() -> Option<PathBuf> {
    if let Ok(named) = std::env::var("ORCH_TEST_CONNECTOR_BINARY") {
        return Some(PathBuf::from(named)).filter(|path| path.is_file());
    }
    // `<target>/debug/deps/<test>`: the target directory is three levels up.
    let exe = std::env::current_exe().ok()?;
    let target = exe.parent()?.parent()?.parent()?.to_owned();
    let program = target
        .join("x86_64-unknown-linux-musl")
        .join("debug")
        .join("orch-connector");
    if !program.is_file() {
        eprintln!(
            "building the static connector: cargo build -p orch-connector --target x86_64-unknown-linux-musl"
        );
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let built = std::process::Command::new(cargo)
            .args([
                "build",
                "-p",
                "orch-connector",
                "--target",
                "x86_64-unknown-linux-musl",
            ])
            .status()
            .ok()?;
        if !built.success() {
            return None;
        }
    }
    Some(program).filter(|path| path.is_file())
}

/// The address of the host on the daemon's default bridge: what `host-gateway` stands for, and an address the
/// units can reach and that is on this machine alone.
pub async fn bridge_address(docker: &Docker) -> Option<String> {
    let network = docker.inspect_network("bridge", None).await.ok()?;
    network
        .ipam?
        .config?
        .into_iter()
        .find_map(|config| config.gateway)
}

impl Lab {
    /// A lab, or `None` (with a word on why) where there is no daemon, no bridge or no static connector to test
    /// with.
    pub async fn start() -> Option<Self> {
        let socket = socket_of_tests();
        let docker =
            Docker::connect_with_socket(&socket, 120, bollard::API_DEFAULT_VERSION).ok()?;
        if docker.ping().await.is_err() {
            eprintln!("skipped: no Docker daemon is reachable at {socket}");
            return None;
        }
        let Some(bridge) = bridge_address(&docker).await else {
            eprintln!("skipped: the daemon's default bridge has no address");
            return None;
        };
        let Some(connector) = static_connector() else {
            eprintln!(
                "skipped: the static connector is not built (cargo build -p orch-connector --target x86_64-unknown-linux-musl)"
            );
            return None;
        };
        let id = new_id().simple().to_string();
        let token = id[id.len() - 12..].to_owned();
        if docker.inspect_image(BUSYBOX).await.is_err() {
            eprintln!("skipped: the image {BUSYBOX} is not on this daemon");
            return None;
        }
        Some(Self {
            system: format!("orch-test-node-{token}"),
            token,
            docker,
            socket,
            bridge,
            connector,
            made: Mutex::new(Made::default()),
        })
    }

    /// A name that begins with `orch-test-node-` and the lab's token.
    pub fn name(&self, what: &str) -> String {
        format!("orch-test-node-{}-{what}", self.token)
    }

    /// The settings the driver is given on a shared daemon: its networks, the repository of what it builds and
    /// the labels of everything it makes by itself carry the lab's names.
    pub fn adjust(
        &self,
    ) -> impl Fn(&mut orch_driver_docker::config::DockerConfig) + Send + Sync + 'static {
        let (token, system, env, ctl) = (
            self.token.clone(),
            self.system.clone(),
            self.name("env"),
            self.name("ctl"),
        );
        move |config| {
            config.labels.insert("orch.test".to_owned(), token.clone());
            config
                .labels
                .insert("orch.system".to_owned(), system.clone());
            config.image_repository = format!("orch-test-node-{token}-build");
            config.networks.env.clone_from(&env);
            config.networks.ctl.clone_from(&ctl);
            config.build_cache_gb = 0;
        }
    }

    /// Records what a unit named `unit` with these volumes makes, so that it is removed whatever becomes of the
    /// test.
    pub fn track(&self, unit: &str, volumes: &[String]) {
        let mut made = self.made.lock().unwrap();
        made.containers.push(unit.to_owned());
        made.containers.push(format!("{unit}-prep"));
        made.volumes.push(format!("{unit}-run"));
        made.volumes.extend(volumes.iter().cloned());
        made.networks.extend([
            format!("{unit}-net"),
            format!("{unit}-out"),
            self.name("env"),
            self.name("ctl"),
            format!("{}-internal", self.name("env")),
            format!("{}-internal", self.name("ctl")),
        ]);
    }

    /// Whether the daemon has a container of this name.
    pub async fn has_container(&self, name: &str) -> bool {
        self.docker.inspect_container(name, None).await.is_ok()
    }

    /// Whether the daemon has a volume of this name.
    pub async fn has_volume(&self, name: &str) -> bool {
        self.docker.inspect_volume(name).await.is_ok()
    }

    /// Puts a file in a directory of a container, which need not be running (a volume mounted there is filled).
    pub async fn put_file(&self, container: &str, directory: &str, name: &str, content: &str) {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o666);
        header.set_uid(1000);
        header.set_gid(1000);
        builder
            .append_data(&mut header, name, content.as_bytes())
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let options = bollard::query_parameters::UploadToContainerOptionsBuilder::default()
            .path(directory)
            .build();
        self.docker
            .upload_to_container(container, Some(options), bollard::body_full(archive.into()))
            .await
            .expect("a file could not be put in the container");
    }

    /// The text of a file in a container, if it is there.
    pub async fn read_file(&self, container: &str, path: &str) -> Option<String> {
        use futures::TryStreamExt;
        use std::io::Read;
        let options = bollard::query_parameters::DownloadFromContainerOptionsBuilder::default()
            .path(path)
            .build();
        let pieces: Result<Vec<bytes::Bytes>, _> = self
            .docker
            .download_from_container(container, Some(options))
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

    /// What a container printed.
    pub async fn logs(&self, container: &str) -> String {
        use futures::TryStreamExt;
        let options = bollard::query_parameters::LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .build();
        let lines: Result<Vec<_>, _> = self
            .docker
            .logs(container, Some(options))
            .try_collect()
            .await;
        lines
            .map(|lines| lines.iter().map(ToString::to_string).collect::<String>())
            .unwrap_or_default()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let made = self
            .made
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (docker, token, system) =
            (self.docker.clone(), self.token.clone(), self.system.clone());
        let mut config = orch_driver_docker::config::DockerConfig {
            socket: self.socket.clone(),
            ..orch_driver_docker::config::DockerConfig::default()
        };
        self.adjust()(&mut config);
        // A runtime of its own on a thread of its own: the test's runtime may be shutting down, and whatever
        // the test ended with, this has to run.
        let outcome = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(async {
                remove_all(&docker, &token, made).await;
                // Fixture cleanup selects labelled cache objects; no local quota chooses their fate.
                if let Ok(driver) =
                    orch_driver_docker::DockerDriver::connect_for_inventory(config).await
                {
                    use orch_driver_contract::driver::EnvironmentDriver;
                    let mut pending = driver
                        .resources(&system)
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
                                .remove_resource(&system, &item.resource)
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
                }
            });
        })
        .join();
        if outcome.is_err() {
            eprintln!("the cleanup of the lab {} failed", self.token);
        }
    }
}

/// Removes what a lab made, by exact name, and only when it carries the lab's label.
async fn remove_all(docker: &Docker, token: &str, made: Made) {
    use bollard::query_parameters::{RemoveContainerOptionsBuilder, RemoveVolumeOptionsBuilder};
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
}

// ---------------------------------------------------------------------------------------------- the system

/// The secrets of a deployment, laid out in a directory as the program that composes them lays them out.
pub struct Secrets {
    pub dir: tempfile::TempDir,
}

impl Secrets {
    /// Composes them, with service certificates good for the given names.
    pub fn compose(clock: &dyn Clock, names: &[String]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let files = CaFiles::in_dir(dir.path());
        let (authority, _) = files.create_if_missing("Test", 3_650, clock).unwrap();
        for service in [ServiceName::Api, ServiceName::Gateway, ServiceName::Worker] {
            let (key, certificate) = authority
                .issue_with_new_key(
                    &Holder::Service {
                        service,
                        names: names.to_vec(),
                    },
                    365,
                    clock,
                )
                .unwrap();
            write_private_file(
                &dir.path().join(format!("service_{service}_key.pem")),
                &key.to_pem(),
            )
            .unwrap();
            write_file(
                &dir.path().join(format!("service_{service}_cert.pem")),
                &certificate.pem(),
            )
            .unwrap();
        }
        Self { dir }
    }

    pub fn authority(&self) -> CertificateAuthority {
        CaFiles::in_dir(self.dir.path()).load().unwrap()
    }

    pub fn ca_certificate(&self) -> Certificate {
        Certificate::from_pem(&std::fs::read_to_string(self.dir.path().join(CA_CERT_FILE)).unwrap())
            .unwrap()
    }

    pub fn identity(&self, service: ServiceName) -> TlsIdentity {
        let read = |name: String| std::fs::read_to_string(self.dir.path().join(name)).unwrap();
        TlsIdentity {
            chain: vec![
                Certificate::from_pem(&read(format!("service_{service}_cert.pem"))).unwrap(),
            ],
            key: LeafKey::from_pem(&read(format!("service_{service}_key.pem"))).unwrap(),
        }
    }
}

/// Applies events under unique request keys in the initial fixture user's canonical ledger, in the transaction that moves the cursor, so that what the
/// machines told can be read back.
pub struct RecordingApplier {
    pub applied: AtomicUsize,
}

#[async_trait]
impl EventApplier for RecordingApplier {
    async fn apply(
        &self,
        tx: &mut Conn,
        machine: Id,
        delivery: &Delivery,
    ) -> Result<(), ApplyError> {
        for event in &delivery.events {
            sqlx::query(
                "INSERT INTO idempotency_keys (user_id,key,method,path,status,response) SELECT id,$1::uuid::text || '/' || $2::text || '/' || $3::bigint::text,'LINK_EVENT',$2,200,jsonb_build_object('machine_id',$1::uuid,'seq',$3::bigint,'kind',$4::text,'body',$5::jsonb) FROM users ORDER BY created_at,id LIMIT 1",
            )
            .bind(machine)
            .bind(delivery.stream.to_string())
            .bind(event.seq)
            .bind(&event.kind)
            .bind(&event.body)
            .execute(&mut *tx)
            .await
            .map_err(|error| ApplyError::Db(DbError::from(error)))?;
            self.applied.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// The API's internal listener over a link, as the program composes it: all a gateway reaches.
pub struct Api {
    pub link: Link,
    pub app: App,
    pub port: u16,
    pub applier: Arc<RecordingApplier>,
    /// A controlled pause after the first real download piece of a selected upload.
    pub download: Arc<DownloadGate>,
    stop: CancellationToken,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

/// Holds later ranges of one real native download, after its first piece was written.
pub struct DownloadGate {
    hash: Mutex<Option<orch_types::Hash>>,
    entered: tokio::sync::Semaphore,
    released: CancellationToken,
}

impl DownloadGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            hash: Mutex::new(None),
            entered: tokio::sync::Semaphore::new(0),
            released: CancellationToken::new(),
        })
    }

    /// Selects the upload whose later download pieces this fixture holds.
    pub fn hold(&self, hash: orch_types::Hash) {
        let mut selected = self.hash.lock().unwrap();
        assert!(selected.is_none(), "one controlled transfer per lab");
        *selected = Some(hash);
    }

    /// Waits for a range after a completed first piece, using the lab's original patience.
    pub async fn entered(&self) {
        tokio::time::timeout(PATIENCE, self.entered.acquire())
            .await
            .expect("the native writer never asked for its next download piece")
            .unwrap()
            .forget();
    }

    /// Releases only this lab's held source requests, including during shutdown.
    pub fn release(&self) {
        self.released.cancel();
    }

    fn offset(&self, request: &axum::extract::Request) -> Option<i64> {
        if request.method() != axum::http::Method::GET {
            return None;
        }
        let hash = request
            .uri()
            .path()
            .rsplit('/')
            .next()?
            .parse::<orch_types::Hash>()
            .ok()?;
        if self.hash.lock().unwrap().as_ref() != Some(&hash) {
            return None;
        }
        request
            .headers()
            .get(axum::http::header::RANGE)?
            .to_str()
            .ok()?
            .strip_prefix("bytes=")?
            .split('-')
            .next()?
            .parse()
            .ok()
    }

    async fn pause(&self, offset: i64) {
        if offset == 0 {
            // Let the normal one-second report cadence elapse before the real first chunk lands.
            tokio::time::sleep(orch_envhost::snapshot::PROGRESS_INTERVAL).await;
        } else {
            self.entered.add_permits(1);
            self.released.cancelled().await;
        }
    }
}

impl Api {
    pub async fn stop(&mut self) {
        self.download.release();
        self.stop.cancel();
        self.link.shutdown();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(15), task).await;
        }
    }
}

/// A gateway on two ports.
pub struct Running {
    pub gateway: Gateway,
    pub pinned: SocketAddr,
    task: Option<JoinHandle<()>>,
}

impl Running {
    pub async fn stop(&mut self) {
        self.gateway.shutdown();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(15), task).await;
        }
    }
}

/// Everything a test stands on.
pub struct World {
    pub db: Db,
    pub test_db: TestDb,
    pub clock: Arc<dyn Clock>,
    pub secrets: Secrets,
    pub config: Arc<ServerConfig>,
    pub api: Api,
    pub gateway: Running,
    pub user: Id,
    pub master: [u8; 32],
    /// The address the host reaches the gateway at, and the units reach it at as `gateway.orch.internal`.
    pub bridge: String,
}

impl World {
    /// A system whose gateway listens on the Docker bridge, where units reach it as `gateway.orch.internal`,
    /// and whose API listens on loopback.
    pub async fn start(bridge: &str) -> Self {
        Self::start_on(bridge, "gateway.orch.internal").await
    }

    /// A system whose gateway listens at `bind`, and tells the machines that enroll to reach it as `advertised`.
    pub async fn start_on(bind: &str, advertised: &str) -> Self {
        let bridge = bind;
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let names: Vec<String> = [
            "api",
            "gateway",
            "worker",
            "gateway.orch.internal",
            "localhost",
            "127.0.0.1",
            bridge,
        ]
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
        let secrets = Secrets::compose(clock.as_ref(), &names);
        let test_db = TestDb::fresh().await;
        let db = Db::from_pool(test_db.pool.clone());
        let user = {
            let mut conn = db.acquire().await.unwrap();
            rows::insert_person_with_home(&mut conn, "ada", clock.now())
                .await
                .unwrap()
        };
        // The gateway's port is known before the API is, because the API tells machines about it when they
        // enroll; what it tells is the name units resolve to the bridge.
        let listener = listen_in_range(bridge).await;
        let pinned = listener.local_addr().unwrap();
        let mut config = ServerConfig::default();
        config.gateway.endpoints = vec![orch_config::server::GatewayEndpoint {
            url: format!("wss://{advertised}:{}", pinned.port()),
            trust: "pinned".to_owned(),
        }];
        let config = Arc::new(config);
        let mut master = [0u8; 32];
        rand::fill(&mut master[..]);
        let api = start_api(&db, &clock, &secrets, &config, master).await;
        let gateway = start_gateway(&clock, &secrets, api.port, listener).await;
        gateway.gateway.wait_for_backplane().await;
        Self {
            db,
            test_db,
            clock,
            secrets,
            config,
            api,
            gateway,
            user,
            master,
            bridge: bridge.to_owned(),
        }
    }

    /// The address the host's own machines (the node) reach the gateway at.
    pub fn gateway_url(&self) -> String {
        format!("wss://{}", self.gateway.pinned)
    }

    /// A token as the system makes one, for a host or for a machine.
    pub async fn token(&self, purpose: TokenFor, hint: &str) -> String {
        self.api
            .link
            .certificates()
            .issue_token(&TokenRequest {
                created_by: self.user,
                purpose,
                name_hint: Some(hint.to_owned()),
            })
            .await
            .unwrap()
            .token
    }

    /// Stores a value as a secret of an environment, sealed under the system's key; answers its identifier,
    /// which a command refers to.
    pub async fn secret(&self, environment: Id, name: &str, value: &str) -> Id {
        let binding = format!("secrets/environment/{environment}/{name}").into_bytes();
        let sealed = MasterKey::new(self.master)
            .seal(value.as_bytes(), &binding)
            .unwrap();
        let mut conn = self.db.acquire().await.unwrap();
        orch_db::queries::settings::put_secret(
            &mut conn,
            "environment",
            environment,
            name,
            &sealed,
            None,
            chrono::Utc::now(),
        )
        .await
        .unwrap()
        .id
    }

    /// The machine a token enrolled, once it has.
    pub async fn machine_of_token(&self, hint: &str) -> Option<Id> {
        sqlx::query_scalar::<_, Option<Id>>(
            "SELECT machine_id FROM enrollment_tokens WHERE name_hint = $1",
        )
        .bind(hint)
        .fetch_optional(&self.test_db.pool)
        .await
        .unwrap()
        .flatten()
    }

    /// Waits for the connector whose token carries `hint` to enroll, and answers the machine it became.
    pub async fn wait_enrolled(&self, hint: &str) -> Id {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            if let Some(machine) = self.machine_of_token(hint).await {
                return machine;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "gave up waiting for the connector of {hint} to enroll"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Waits until a machine has a session at the API (or has none).
    pub async fn wait_online(&self, machine: Id, online: bool) {
        let api = self.api.link.clone();
        eventually(
            &format!("machine {machine} to be online: {online}"),
            move || {
                let api = api.clone();
                async move { api.is_online(machine) == online }
            },
        )
        .await;
    }

    /// What a machine told, on a stream, in order, as `(kind, body)`.
    pub async fn told(&self, machine: Id, stream: &str) -> Vec<(String, Json)> {
        sqlx::query_as::<_, (String, Json)>(
            "SELECT response->>'kind', response->'body' FROM idempotency_keys WHERE method='LINK_EVENT' AND response->>'machine_id'=$1::uuid::text AND path=$2 ORDER BY (response->>'seq')::bigint",
        )
        .bind(machine)
        .bind(stream)
        .fetch_all(&self.test_db.pool)
        .await
        .unwrap()
    }

    /// Stops the gateway and the API, and closes the pool.
    pub async fn stop(&mut self) {
        self.gateway.stop().await;
        self.api.stop().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.db.pool().close().await;
    }
}

async fn start_api(
    db: &Db,
    clock: &Arc<dyn Clock>,
    secrets: &Secrets,
    config: &Arc<ServerConfig>,
    master: [u8; 32],
) -> Api {
    let link = Link::new(LinkParts {
        db: db.clone(),
        clock: Arc::clone(clock),
        config: Arc::clone(config),
        release_version: "0.1.0".to_owned(),
        authority: Arc::new(secrets.authority()),
        master_key: MasterKey::new(master),
    });
    let applier = Arc::new(RecordingApplier {
        applied: AtomicUsize::new(0),
    });
    link.set_applier(applier.clone());
    link.start().await.unwrap();
    let app: App = Arc::new(AppState {
        builtin_sources: Default::default(),
        host_ports: Default::default(),
        db: db.clone(),
        config: Arc::clone(config),
        clock: Arc::clone(clock),
        link: link.clone(),
        engine: orch_api_core::thread_engine::detached(db, clock, config, &link),
        realtime: orch_api_core::realtime::Realtime::new(db, clock, config),
        build: BuildInfo::new("api", "0.0.0-test", "", "1"),
        blobs: orch_api_core::blobs::Blobs::in_memory(),
        cache: orch_api_core::cache::Cache::detached(),
    });
    let download = DownloadGate::new();
    let gate = Arc::clone(&download);
    let content = orch_api_core::content::routes().layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let gate = Arc::clone(&gate);
            async move {
                let offset = gate.offset(&request);
                if let Some(offset) = offset {
                    gate.pause(offset).await;
                }
                next.run(request).await
            }
        },
    ));
    let router = server::internal_router(Arc::clone(&app), content);
    let tls = server::internal_tls_config(
        &secrets.identity(ServiceName::Api),
        &secrets.ca_certificate(),
        Arc::clone(clock),
    )
    .unwrap();
    let listener = listen_in_range("127.0.0.1").await;
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let task = tokio::spawn(server::serve_tls(listener, tls, router, stop.clone()));
    Api {
        link,
        app,
        port,
        applier,
        download,
        stop,
        task: Some(task),
    }
}

async fn start_gateway(
    clock: &Arc<dyn Clock>,
    secrets: &Secrets,
    api_port: u16,
    pinned: TcpListener,
) -> Running {
    let settings = Settings {
        gateway_id: "test-gateway".to_owned(),
        version: "0.0.0-test".to_owned(),
        backplane_url: format!("wss://127.0.0.1:{api_port}/internal/backplane"),
        heartbeat_secs: 20,
        limits: orch_link_contract::version::Limits::default(),
        tunnel_resume_grace: Duration::from_secs(60),
        expired_renew_grace_days: 90,
        trusted_proxy_cidrs: Vec::new(),
    };
    let gateway = Gateway::new(GatewayParts {
        settings,
        identity: secrets.identity(ServiceName::Gateway),
        authority: secrets.ca_certificate(),
        clock: Arc::clone(clock),
    })
    .unwrap();
    // The edge listener is the public one; nothing in these tests reaches it, and it listens on loopback.
    let edge = listen_in_range("127.0.0.1").await;
    let pinned_addr = pinned.local_addr().unwrap();
    let running = gateway.clone();
    let task = tokio::spawn(async move { running.run(pinned, edge).await });
    Running {
        gateway,
        pinned: pinned_addr,
        task: Some(task),
    }
}

// ---------------------------------------------------------------------------------------------- the node

/// A node on the real daemon, enrolled in the world's system with a host token and holding its session.
pub struct NodeUnderTest {
    pub machine_id: Id,
    pub data_dir: tempfile::TempDir,
    pub settings: NodeSettings,
    pub drivers: Arc<DriverHost>,
    pub connector: Option<Connector>,
    wiring: Wiring,
}

impl NodeUnderTest {
    /// Enrolls a host and starts its node, over the Docker driver with the lab's names.
    pub async fn start(world: &World, lab: &Lab) -> Self {
        Self::start_over(
            world,
            lab,
            Registrations::new().register(docker::NAME, docker::registration(lab.adjust())),
        )
        .await
    }

    /// As [`NodeUnderTest::start`], over these registrations.
    pub async fn start_over(world: &World, lab: &Lab, registrations: Registrations) -> Self {
        Self::start_with(
            world,
            &lab.token,
            &lab.system,
            registrations,
            |settings| {
                settings.drivers = vec![docker::NAME.to_owned()];
                settings.docker.socket.clone_from(&lab.socket);
            },
            &[docker::NAME],
        )
        .await
    }

    /// A node over these registrations whose settings `adjust` completes, expecting exactly the drivers named
    /// in `expected` to load. `token` names the host in the system; `system` labels what the node makes.
    pub async fn start_with(
        world: &World,
        token: &str,
        system: &str,
        registrations: Registrations,
        adjust: impl FnOnce(&mut NodeSettings),
        expected: &[&str],
    ) -> Self {
        let data_dir = tempfile::tempdir().unwrap();
        let mut settings = NodeSettings::in_dir(data_dir.path());
        settings.system_name = system.to_owned();
        let clock = Arc::clone(&world.clock);
        let data = DataDir::open(data_dir.path()).unwrap();
        let host_token = world
            .token(
                TokenFor::Host { home: None },
                &format!("orch-test-node-{token}-host"),
            )
            .await;
        let enrolled = enroll_host(
            &data,
            &HostEnrollment {
                delegate_policy: false,
                address: world.gateway_url(),
                token: host_token,
                name: format!("orch-test-node-{token}-host"),
            },
            "0.1.0",
            &HttpsEnroll::new(Arc::clone(&clock), Some(Trust::Pinned)),
            clock.as_ref(),
        )
        .await
        .expect("the host could not enroll");
        // The system tells machines to reach the gateway as `gateway.orch.internal`, which only the units
        // resolve; the node, on the host, reaches it at the address it listens on.
        let reach = world.gateway_url();
        data.update_config(move |config| {
            config.endpoints = vec![orch_config::connector::ConnectorEndpoint {
                url: reach,
                trust: "pinned".to_owned(),
            }];
        })
        .unwrap();
        // The machine is enrolled: its policy can be written now, as the settings that follow it are.
        adjust(&mut settings);
        let prepared = Prepared::open(&settings).unwrap();
        let desk = Arc::new(BlobDesk::new());
        let drivers = DriverHost::load(&settings, registrations, Arc::clone(&desk)).await;
        assert_eq!(drivers.names(), expected, "{:?}", drivers.failures());
        let wiring = Wiring::new(
            "0.1.0",
            Arc::clone(&clock),
            prepared,
            Arc::clone(&drivers),
            desk,
        );
        let mut node = Self {
            machine_id: enrolled.record.machine_id,
            data_dir,
            settings,
            drivers,
            connector: None,
            wiring,
        };
        node.run(&data).await;
        node
    }

    async fn run(&mut self, data: &DataDir) {
        let clock = Arc::clone(&self.wiring.clock);
        let identity = data.identity().unwrap();
        let dialer = Arc::new(tls_dialer(&identity, clock).unwrap());
        let connector = builder(&self.wiring, data, identity, dialer, &|_| None)
            .unwrap()
            .start()
            .await
            .expect("the node could not start");
        tokio::time::timeout(PATIENCE, connector.link().connected())
            .await
            .expect("the node never got a session");
        self.connector = Some(connector);
    }

    /// Stops the node as a program that is told to stop.
    pub async fn stop(&mut self) {
        if let Some(connector) = self.connector.take() {
            connector.stop().await;
        }
    }

    /// Starts it again over the identity it kept.
    pub async fn restart(&mut self) {
        let data = DataDir::open(self.data_dir.path()).unwrap();
        self.run(&data).await;
    }
}

// ---------------------------------------------------------------------------------------------- the connector's program

/// A server on loopback that serves one program, for `node.tool.ensure` to download.
pub struct ProgramServer {
    pub url: String,
    pub sha256: String,
    task: JoinHandle<()>,
}

impl ProgramServer {
    /// Serves the file at `program` as `/<name>`.
    pub async fn serve(program: &Path, name: &str) -> Self {
        let bytes = bytes::Bytes::from(std::fs::read(program).unwrap());
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let app: Router = Router::new().route(
            &format!("/{name}"),
            axum::routing::get(move || {
                let bytes = bytes.clone();
                async move { bytes }
            }),
        );
        let listener = listen_in_range("127.0.0.1").await;
        let url = format!("http://{}/{name}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { url, sha256, task }
    }
}

impl Drop for ProgramServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ---------------------------------------------------------------------------------------------- what the system sends

/// A runtime a test makes through the node.
#[derive(Debug, Clone)]
pub struct Unit {
    /// The runtime.
    pub runtime_id: Id,
    /// The environment kept through a recreate.
    pub environment_id: Id,
    /// The template the environment was made from.
    pub template_id: Id,
    /// The previous runtime, while a recreate is prepared.
    pub successor_of: Option<Id>,
    /// The name of its unit.
    pub name: String,
    /// Its volumes, by role.
    pub volumes: Vec<(String, String)>,
    /// What its enrollment token is called in the system, by which the machine it becomes is found.
    pub hint: String,
}

/// The system's side of one node: what the engine's provisioning sends, written out.
pub struct Fleet<'a> {
    pub world: &'a World,
    pub lab: &'a Lab,
    pub host: Id,
}

impl<'a> Fleet<'a> {
    /// The system's side of the node `node` in `world`.
    pub fn new(world: &'a World, lab: &'a Lab, node: &NodeUnderTest) -> Self {
        Self {
            world,
            lab,
            host: node.machine_id,
        }
    }

    /// Sends a command and waits for its result.
    pub async fn run<B: orch_link_contract::commands::CommandBody>(
        &self,
        key: &orch_link_contract::commands::SerialKey,
        body: &B,
    ) -> Result<B::Result, orch_api_core::dispatch::RunError> {
        let value = serde_json::to_value(body).unwrap();
        if let Some(job) = value
            .get("job_id")
            .and_then(Json::as_str)
            .map(|id| id.parse::<Id>().unwrap())
        {
            self.record_job(job).await;
        }
        self.world
            .api
            .link
            .dispatcher()
            .run(self.host, key, body)
            .await
    }

    /// The real controller records placement before a node may report that runtime's state.
    async fn record_runtime(
        &self,
        runtime: Id,
        name: &str,
        spec: &Json,
        volumes: &Json,
    ) -> (Id, Id) {
        let mut tx = self.world.db.begin().await.unwrap();
        let host = orch_db::queries::resources::hosting::host_of_machine(&mut tx, self.host)
            .await
            .unwrap()
            .expect("the node has its registered host");
        let existing: Option<(Id, Id, String)> =
            sqlx::query_as("SELECT host_id,environment_id,unit_name FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_optional(&mut *tx)
                .await
                .unwrap();
        if let Some((assigned, environment, unit)) = existing {
            assert_eq!((assigned, unit.as_str()), (host, name));
            tx.commit().await.unwrap();
            return (host, environment);
        }
        let home: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
            .bind(self.world.user)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let environment = orch_db::queries::resources::access_fixtures::insert_environment(
            &mut tx,
            self.world.user,
            Some(home),
            name,
            "managed",
        )
        .await
        .unwrap();
        let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        sqlx::query("UPDATE env_templates SET driver='docker',spec=$2 WHERE id=$1")
            .bind(template)
            .bind(spec)
            .execute(&mut *tx)
            .await
            .unwrap();
        orch_db::queries::environments::hosting::insert_runtime(
            &mut tx,
            &orch_db::queries::environments::hosting::NewRuntime {
                id: runtime,
                host_id: host,
                unit_name: name,
                environment_id: environment,
                driver: "docker",
                template_id: template,
                spec_hash: &[0; 32],
                generation: 1,
                volumes,
            },
            self.world.clock.now(),
        )
        .await
        .unwrap();
        assert!(
            orch_db::queries::machine_reports::owns_runtime(&mut tx, self.host, runtime)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        (host, environment)
    }

    /// Build progress has the same host/job ownership proof as an ordinary provisioning job.
    async fn record_job(&self, job: Id) {
        let mut conn = self.world.db.acquire().await.unwrap();
        let existing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=$1)")
            .bind(job)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        if existing {
            assert!(
                orch_db::queries::machine_reports::owns_host_job(&mut conn, self.host, job)
                    .await
                    .unwrap()
            );
            return;
        }
        drop(conn);
        let (host, environment) = self
            .record_runtime(
                new_id(),
                &self.lab.name(&format!("job-{job}")),
                &serde_json::json!({"image":BUSYBOX}),
                &serde_json::json!({}),
            )
            .await;
        let mut tx = self.world.db.begin().await.unwrap();
        let runtime: Id = sqlx::query_scalar("SELECT id FROM runtimes WHERE environment_id=$1")
            .bind(environment)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        orch_db::queries::jobs::insert(
            &mut tx,
            &orch_db::queries::jobs::NewJob {
                id: job,
                kind: "environment_provision",
                node_id: None,
                environment_id: Some(environment),
                provider_id: None,
                plan: &serde_json::json!({"host":host,"runtime":runtime}),
                progress: &serde_json::json!({}),
                created_by: Some(self.world.user),
            },
            self.world.clock.now(),
        )
        .await
        .unwrap();
        assert!(
            orch_db::queries::machine_reports::owns_host_job(&mut tx, self.host, job)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
    }

    /// The program the units run, put in the node's tool cache from a server on loopback, and the small image
    /// made available: the first two phases that a provisioning sends. Answers the server, which must outlive
    /// the download.
    pub async fn prepare(&self) -> ProgramServer {
        use orch_link_contract::commands::{NodeImageEnsure, NodeToolEnsure, SerialKey};
        let server = ProgramServer::serve(&self.lab.connector, "orch-connector").await;
        self.run(
            &SerialKey::Tool("orch-connector".into()),
            &NodeToolEnsure {
                job_id: None,
                name: "orch-connector".into(),
                version: "0.1.0".into(),
                url: server.url.clone(),
                sha256: server.sha256.clone(),
            },
        )
        .await
        .expect("the connector could not be put in the cache");
        self.run(
            &SerialKey::Image(BUSYBOX.into()),
            &NodeImageEnsure {
                driver: "docker".into(),
                spec: serde_json::json!({ "image": BUSYBOX }),
                job_id: None,
            },
        )
        .await
        .expect("the image could not be made available");
        server
    }

    /// A new runtime, named by the lab and tracked for cleanup.
    pub fn unit(&self, label: &str) -> Unit {
        let unit = Unit {
            runtime_id: new_id(),
            environment_id: new_id(),
            template_id: new_id(),
            successor_of: None,
            name: self.lab.name(label),
            volumes: ["workspace", "home", "state"]
                .iter()
                .map(|role| {
                    (
                        (*role).to_owned(),
                        self.lab.name(&format!("{label}-{role}")),
                    )
                })
                .collect(),
            hint: self.lab.name(&format!("{label}-token")),
        };
        self.lab.track(
            &unit.name,
            &unit
                .volumes
                .iter()
                .map(|(_, name)| name.clone())
                .collect::<Vec<_>>(),
        );
        unit
    }

    /// A runtime that takes over from `old`: its own name and token, the volumes of `old`.
    pub fn successor(&self, old: &Unit, label: &str) -> Unit {
        let unit = Unit {
            runtime_id: new_id(),
            environment_id: old.environment_id,
            template_id: old.template_id,
            successor_of: Some(old.runtime_id),
            name: self.lab.name(label),
            volumes: old.volumes.clone(),
            hint: self.lab.name(&format!("{label}-token")),
        };
        self.lab.track(&unit.name, &[]);
        unit
    }

    async fn record_unit_runtime(&self, unit: &Unit) {
        // The API accepts runtime reports only from the node of their canonical host.
        // These direct-dispatch tests still create the rows that provisioning normally commits first.
        let mut tx = self.world.db.begin().await.unwrap();
        let home: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
            .bind(self.world.user)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO env_templates(id,name,driver,spec) VALUES($1,$2,'docker','{}') ON CONFLICT (id) DO NOTHING")
            .bind(unit.template_id)
            .bind(&unit.name)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,home_node_id,placement,template_id) VALUES($1,$2,$3,$4,$4,'managed',$5) ON CONFLICT (id) DO NOTHING")
            .bind(unit.environment_id).bind(&unit.name).bind(self.world.user).bind(home).bind(unit.template_id)
            .execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state,successor_of) SELECT $1,'environment',id,$2,$3,'docker',$4,$5,'creating',$7 FROM hosts WHERE machine_id=$6")
            .bind(unit.runtime_id).bind(&unit.name).bind(unit.environment_id).bind(unit.template_id).bind(vec![0u8;32]).bind(self.host).bind(unit.successor_of)
            .execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Makes the volumes of a unit, then the unit, as the phases `volumes` and `create` do. `gateway` is the
    /// address its connector enrolls at; the system's own, as units resolve it, when none is given.
    pub async fn make(
        &self,
        unit: &Unit,
        gateway: Option<&str>,
    ) -> Result<
        orch_link_contract::commands::NodeRuntimeCreateResult,
        orch_api_core::dispatch::RunError,
    > {
        self.make_with(unit, gateway, |_| {}).await
    }

    /// Makes a unit with the same enrollment and ownership proof, over a caller's frozen definition.
    pub async fn make_with(
        &self,
        unit: &Unit,
        gateway: Option<&str>,
        change: impl FnOnce(&mut Json),
    ) -> Result<
        orch_link_contract::commands::NodeRuntimeCreateResult,
        orch_api_core::dispatch::RunError,
    > {
        use orch_link_contract::commands::{NodeRuntimeCreate, NodeVolumeEnsure, SerialKey};
        self.record_unit_runtime(unit).await;
        for (_, name) in &unit.volumes {
            self.run(
                &SerialKey::Volume(name.clone()),
                &NodeVolumeEnsure {
                    driver: "docker".into(),
                    name: name.clone(),
                    storage: None,
                    labels: std::collections::BTreeMap::from([(
                        "orch.test".into(),
                        self.lab.token.clone(),
                    )]),
                },
            )
            .await?;
        }
        // The token is made as the system makes one, and stored as a secret that the command refers to.
        let token = self.world.token(TokenFor::Machine, &unit.hint).await;
        let secret = self
            .world
            .secret(unit.runtime_id, "enroll_token", &token)
            .await;
        let mut volumes = serde_json::Map::new();
        for (role, name) in &unit.volumes {
            volumes.insert(
                role.clone(),
                serde_json::json!({ "name": name, "storage": null }),
            );
        }
        let gateway_url = gateway.map_or_else(
            || {
                format!(
                    "wss://gateway.orch.internal:{}",
                    self.world.gateway.pinned.port()
                )
            },
            str::to_owned,
        );
        let mut spec = serde_json::json!({
            "runtime_id": unit.runtime_id, "kind": "environment", "name": unit.name, "driver": "docker",
            "spec": {
                "image": BUSYBOX, "user": { "uid": 1000, "gid": 1000 },
                "network": { "mode": "bridge", "egress": "open", "dns": [] },
                "security": { "read_only_rootfs": false, "no_new_privileges": true, "cap_add": [], "cap_drop": ["NET_RAW"] },
                "mounts": []
            },
            "resources": {}, "roles": ["environment"], "gateway_url": gateway_url,
            "enroll_token": { "$secret": secret },
            "ca_cert": self.world.secrets.ca_certificate().pem(),
            "tools": [], "volumes": volumes, "mounts": [], "env": {},
            "labels": {
                "orch.system": self.lab.system, "orch.runtime_id": unit.runtime_id,
                "orch.kind": "environment", "orch.test": self.lab.token
            }
        });
        change(&mut spec);
        self.record_runtime(unit.runtime_id, &unit.name, &spec["spec"], &spec["volumes"])
            .await;
        self.run(
            &SerialKey::Runtime(unit.runtime_id),
            &NodeRuntimeCreate(serde_json::from_value(spec).unwrap()),
        )
        .await
    }

    /// `node.runtime.start`.
    pub async fn start(&self, unit: &Unit) -> Result<(), orch_api_core::dispatch::RunError> {
        use orch_link_contract::commands::{NodeRuntimeStart, SerialKey};
        let runtime_id = unit.runtime_id;
        self.run(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeStart { runtime_id },
        )
        .await
        .map(|_| ())
    }

    /// `node.runtime.stop`.
    pub async fn stop(&self, unit: &Unit) -> Result<(), orch_api_core::dispatch::RunError> {
        use orch_link_contract::commands::{NodeRuntimeStop, SerialKey};
        let runtime_id = unit.runtime_id;
        self.run(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeStop {
                runtime_id,
                reason: None,
            },
        )
        .await
        .map(|_| ())
    }

    /// `node.runtime.remove`.
    pub async fn remove(&self, unit: &Unit) -> Result<(), orch_api_core::dispatch::RunError> {
        use orch_link_contract::commands::{NodeRuntimeRemove, SerialKey};
        let runtime_id = unit.runtime_id;
        self.run(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeRemove {
                runtime_id,
                keep_volumes: true,
                volume_refs: vec![],
            },
        )
        .await
        .map(|_| ())
    }

    /// `node.volume.remove`.
    pub async fn remove_volume(&self, name: &str) -> Result<(), orch_api_core::dispatch::RunError> {
        use orch_link_contract::commands::{NodeVolumeRemove, SerialKey};
        self.run(
            &SerialKey::Volume(name.to_owned()),
            &NodeVolumeRemove {
                driver: "docker".into(),
                name: name.to_owned(),
                storage: None,
                labels: std::collections::BTreeMap::new(),
            },
        )
        .await
        .map(|_| ())
    }
}

/// The code a command failed with.
pub fn code_of(error: &orch_api_core::dispatch::RunError) -> String {
    match error {
        orch_api_core::dispatch::RunError::Command(
            orch_link::commands::CommandFailure::Failed(error),
        ) => error.code.clone(),
        other => format!("{other}"),
    }
}
