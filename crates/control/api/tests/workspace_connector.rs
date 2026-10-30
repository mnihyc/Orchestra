//! An environment's workspace through the API, on a real connector started as a program: a file written over HTTP is on the machine's disk, a file changed on the disk is read back over HTTP, a file larger than one piece goes both ways, a path outside the workspace is refused, and a snapshot is taken and one of its files read.
//!
//! **What is real:** the API's router over a real database with every bound request; the commands that
//! those requests store and the dispatcher that waits for them; the content routes of the internal
//! listener; the connector as a process (enrolled over HTTPS with a token, holding a session over TLS
//! and a WebSocket, hosting a directory of its machine as an environment under the policy of its owner)
//! and the content it moves in pieces over HTTPS. **What stands in:** the gateway, which is a front on
//! the loopback that hands the session to the testkit's hub and the requests for content to the
//! internal routes with the machine it found in the certificate (as a gateway does), and the link
//! between the API's stored commands and that hub, which is a few lines here that send each stored
//! command over the hub and write the machine's answer back.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
// The program is stopped the way a service manager stops it, with a termination signal.
#![cfg(unix)]

mod places;
mod support;
mod workspace;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use http::StatusCode;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, header};
use hyper_util::rt::TokioIo;
use orch_api_contract::common::{ErrorCode as ApiCode, PageQuery};
use orch_api_contract::nodes::JobState;
use orch_api_contract::workspace::{
    FetchStatus, FetchUnavailable, Job, JobGet, SnapshotFileContentGet, SnapshotFileList,
    SnapshotGet, SnapshotPathQuery, SnapshotState, ThreadSnapshotCreate, ThreadSnapshotCreateBody,
    ThreadSnapshotList,
};
use orch_api_core::backplane::Link;
use orch_api_core::registry::Registry;
use orch_api_core::state::AppState;
use orch_api_core::testing::{Machine, TestApp};
use orch_build_info::BuildInfo;
use orch_db::Db;
use orch_db::queries::resources::{NewMachine, insert_machine};
use orch_link::commands::Progress;
use orch_link::controller::{ControllerLink, Notice};
use orch_link::framing::{Connection, MessageSink, MessageSource, PeerInfo};
use orch_link_contract::commands::{Cmd, CommandError, ErrorCode};
use orch_link_contract::content::BLOBS_PATH;
use orch_link_contract::enrollment::{
    ENROLL_PATH, Endpoint, EnrollReply, EnrollReplySystem, EnrollRequest, Trust,
};
use orch_link_contract::session::Role;
use orch_link_contract::version::{SESSION_PATH, SUBPROTOCOL};
use orch_pki::ca::Holder;
use orch_pki::certificates::{Certificate, ServiceName};
use orch_pki::enrollment::{MemoryLedger, TokenGrant, TokenPurpose, issue_token};
use orch_pki::verification::{
    TlsIdentity, pinned_server_config, plain_server_config, presented_certificate,
};
use orch_testkit::doubles::link::{Hub, MachineRecord, TestPki};
use orch_types::{Clock, Hash, Id, SystemClock, new_id};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role as SocketRole, WebSocketConfig};
use workspace::{Content, Placed, Site, ask};

/// How long the test waits for something that has to happen, in real time.
const PATIENCE: Duration = Duration::from_secs(90);

/// The most a message of a session may have: the same bound the link's own sockets have.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Bytes that do not repeat inside a piece, so that a piece in the wrong place is noticed.
fn bytes_of(len: usize, seed: usize) -> Vec<u8> {
    (0..len)
        .map(|at| u8::try_from((at.wrapping_mul(31) + seed + at / 251) % 256).unwrap())
        .collect()
}

// ---------------------------------------------------------------- the system on the loopback

/// The machine that enrolled: what the system keeps of it to talk to it.
struct Arrival {
    machine: Id,
    roles: Vec<Role>,
    link: ControllerLink,
    notices: Option<mpsc::UnboundedReceiver<Notice>>,
}

/// The system's end of the sockets: an authority, the hub behind it, the tokens it gave, the machine
/// it enrolled, and the routes content is handed to.
struct System {
    clock: Arc<dyn Clock>,
    pki: TestPki,
    hub: Hub,
    ledger: MemoryLedger,
    gateway_port: u16,
    arrival: Mutex<Option<Arrival>>,
    machine: Mutex<Option<Id>>,
    content: Mutex<Option<Content>>,
}

impl System {
    fn token(&self) -> String {
        let issued = issue_token(
            self.pki.authority(),
            Duration::from_secs(900),
            self.clock.as_ref(),
        );
        self.ledger.record(
            &issued,
            TokenGrant {
                purpose: TokenPurpose::Machine,
                created_by: new_id(),
            },
        );
        issued.token.to_string()
    }

    /// A certificate for the loopback address, which both listeners show.
    fn server_identity(&self) -> TlsIdentity {
        let (key, certificate) = self
            .pki
            .authority()
            .issue_with_new_key(
                &Holder::Service {
                    service: ServiceName::Gateway,
                    names: vec!["localhost".to_owned(), "127.0.0.1".to_owned()],
                },
                365,
                self.clock.as_ref(),
            )
            .unwrap();
        TlsIdentity {
            chain: vec![certificate, self.pki.certificate()],
            key,
        }
    }

    /// The answer to a request to enroll: the authority signs, the machine is added to the hub, and the
    /// endpoint named is the listener of the front.
    fn enroll(&self, request: &EnrollRequest) -> Result<EnrollReply, StatusCode> {
        let enrolled = orch_pki::enrollment::enroll(
            self.pki.authority(),
            &self.ledger,
            request,
            365,
            self.clock.as_ref(),
        )
        .map_err(|error| match error {
            orch_pki::enrollment::EnrollError::Refused(refusal) => {
                StatusCode::from_u16(refusal.status()).unwrap()
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
        let (link, notices) = self.hub.add_machine(
            enrolled.machine_id,
            request.roles.clone(),
            BTreeMap::new(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        *lock(&self.machine) = Some(enrolled.machine_id);
        *lock(&self.arrival) = Some(Arrival {
            machine: enrolled.machine_id,
            roles: request.roles.clone(),
            link,
            notices: Some(notices),
        });
        Ok(EnrollReply {
            machine_id: enrolled.machine_id,
            certificate_pem: enrolled.certificate.pem(),
            ca_pem: self.pki.authority().certificate_pem(),
            endpoints: vec![Endpoint {
                url: format!("wss://127.0.0.1:{}", self.gateway_port),
                trust: Trust::Pinned,
            }],
            system: EnrollReplySystem {
                name: "Orch".to_owned(),
                public_url: "https://orch.example.com".to_owned(),
            },
            protocol_version: enrolled.protocol_version,
        })
    }
}

/// Carries the messages of one half of a connection to the other, and lets the other half go when this
/// one has ended: a sink that is dropped is how the end of a connection reaches the hub.
async fn pump(mut from: Box<dyn MessageSource>, mut to: Box<dyn MessageSink>) {
    while let Ok(Some(message)) = from.recv().await {
        if to.send(message).await.is_err() {
            break;
        }
    }
}

type Reply = Response<Full<Bytes>>;

fn plain(status: StatusCode) -> Reply {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

/// Turns a request for the session path into a session on the hub.
fn upgrade(
    system: &Arc<System>,
    certificate: Option<Certificate>,
    remote: SocketAddr,
    request: &mut Request<Incoming>,
) -> Reply {
    let Some(key) = request
        .headers()
        .get("sec-websocket-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return plain(StatusCode::BAD_REQUEST);
    };
    let accept = derive_accept_key(key.as_bytes());
    let upgraded = hyper::upgrade::on(&mut *request);
    let system = Arc::clone(system);
    tokio::spawn(async move {
        let Ok(upgraded) = upgraded.await else {
            return;
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let socket = WebSocketStream::from_raw_socket(
            TokioIo::new(upgraded),
            SocketRole::Server,
            Some(config),
        )
        .await;
        let peer = PeerInfo {
            remote_ip: Some(remote.ip()),
            host: Some("127.0.0.1".to_owned()),
            client_certificate: certificate.clone(),
        };
        let outside = orch_link::websocket::connection_of(socket, peer);
        let Ok(inside) = system.hub.dialer(certificate).dial(&Hub::endpoint()).await else {
            return;
        };
        let Connection {
            sink: outside_sink,
            source: outside_source,
            ..
        } = outside;
        let Connection {
            sink: inside_sink,
            source: inside_source,
            ..
        } = inside;
        tokio::spawn(pump(outside_source, inside_sink));
        pump(inside_source, outside_sink).await;
    });
    let mut response = plain(StatusCode::SWITCHING_PROTOCOLS);
    let headers = response.headers_mut();
    headers.insert(
        header::CONNECTION,
        header::HeaderValue::from_static("Upgrade"),
    );
    headers.insert(
        header::UPGRADE,
        header::HeaderValue::from_static("websocket"),
    );
    headers.insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        header::HeaderValue::from_static(SUBPROTOCOL),
    );
    if let Ok(value) = header::HeaderValue::from_str(&accept) {
        headers.insert(header::SEC_WEBSOCKET_ACCEPT, value);
    }
    response
}

/// Hands a request for content to the routes of the internal listener, for the machine whose
/// certificate the connection showed, and gives their answer back.
async fn forward(system: &System, request: Request<Incoming>) -> Reply {
    let Some(content) = lock(&system.content).clone() else {
        return plain(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Some(machine) = *lock(&system.machine) else {
        return plain(StatusCode::UNAUTHORIZED);
    };
    let (parts, body) = request.into_parts();
    let bytes = body
        .collect()
        .await
        .map(|all| all.to_bytes())
        .unwrap_or_default();
    let rest = parts
        .uri
        .path()
        .strip_prefix("/link/v1")
        .unwrap_or(parts.uri.path());
    let target = match parts.uri.query() {
        Some(query) => format!("/internal/link{rest}?{query}"),
        None => format!("/internal/link{rest}"),
    };
    let mut extra: Vec<(&str, String)> = Vec::new();
    for name in ["x-orch-command", "range", "content-type"] {
        if let Some(value) = parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
        {
            extra.push((name, value.to_owned()));
        }
    }
    let internal = content.request(parts.method, &target, Some(machine), None, &extra, bytes);
    let answer = content.send(internal).await;
    let mut response = Response::builder().status(answer.status);
    for (name, value) in &answer.headers {
        response = response.header(name, value);
    }
    response.body(Full::new(answer.body)).unwrap()
}

async fn handle(
    system: Arc<System>,
    certificate: Option<Certificate>,
    remote: SocketAddr,
    mut request: Request<Incoming>,
) -> Reply {
    let path = request.uri().path().to_owned();
    if path == SESSION_PATH {
        return upgrade(&system, certificate, remote, &mut request);
    }
    if path == BLOBS_PATH || path.starts_with(&format!("{BLOBS_PATH}/")) {
        return forward(&system, request).await;
    }
    plain(StatusCode::NOT_FOUND)
}

/// The front on the gateway's port: TLS under the system's authority with the machine's certificate
/// asked for, then HTTP, whose requests are a session (upgraded) or content.
async fn serve_front(system: Arc<System>, listener: TcpListener) {
    let tls = pinned_server_config(
        &system.server_identity(),
        &system.pki.certificate(),
        90,
        Arc::clone(&system.clock),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(tls);
    loop {
        let Ok((socket, remote)) = listener.accept().await else {
            return;
        };
        let (acceptor, system) = (acceptor.clone(), Arc::clone(&system));
        tokio::spawn(async move {
            let _ = socket.set_nodelay(true);
            let Ok(stream) = acceptor.accept(socket).await else {
                return;
            };
            let certificate = presented_certificate(stream.get_ref().1.peer_certificates());
            let service = service_fn(move |request: Request<Incoming>| {
                let (system, certificate) = (Arc::clone(&system), certificate.clone());
                async move {
                    Ok::<_, std::convert::Infallible>(
                        handle(system, certificate, remote, request).await,
                    )
                }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await;
        });
    }
}

/// The listener for enrollment: HTTPS under the system's authority, answering `POST /link/v1/enroll`.
async fn serve_enrollment(system: Arc<System>, listener: TcpListener) {
    let acceptor = TlsAcceptor::from(
        plain_server_config(&system.server_identity(), Arc::clone(&system.clock)).unwrap(),
    );
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        let (acceptor, system) = (acceptor.clone(), Arc::clone(&system));
        tokio::spawn(async move {
            let Ok(stream) = acceptor.accept(socket).await else {
                return;
            };
            let service = service_fn(move |request: Request<Incoming>| {
                let system = Arc::clone(&system);
                async move {
                    let reply = |status: StatusCode, body: Vec<u8>| {
                        Response::builder()
                            .status(status)
                            .header("content-type", "application/json")
                            .body(Full::new(Bytes::from(body)))
                            .unwrap()
                    };
                    if request.method() != http::Method::POST || request.uri().path() != ENROLL_PATH
                    {
                        return Ok::<_, std::convert::Infallible>(reply(
                            StatusCode::NOT_FOUND,
                            Vec::new(),
                        ));
                    }
                    let body = request.into_body().collect().await.unwrap().to_bytes();
                    let Ok(asked) = serde_json::from_slice::<EnrollRequest>(&body) else {
                        return Ok(reply(StatusCode::UNPROCESSABLE_ENTITY, Vec::new()));
                    };
                    Ok(match system.enroll(&asked) {
                        Ok(enrolled) => {
                            reply(StatusCode::CREATED, serde_json::to_vec(&enrolled).unwrap())
                        }
                        Err(status) => reply(status, Vec::new()),
                    })
                }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

/// A system on two loopback sockets; the second is the port the connector is told to enroll at.
async fn start_system() -> (Arc<System>, u16) {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let enrollment = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let system = Arc::new(System {
        clock,
        pki,
        hub,
        ledger: MemoryLedger::new(),
        gateway_port: gateway.local_addr().unwrap().port(),
        arrival: Mutex::new(None),
        machine: Mutex::new(None),
        content: Mutex::new(None),
    });
    let enrollment_port = enrollment.local_addr().unwrap().port();
    tokio::spawn(serve_front(Arc::clone(&system), gateway));
    tokio::spawn(serve_enrollment(Arc::clone(&system), enrollment));
    (system, enrollment_port)
}

// ---------------------------------------------------------------- the stored commands, sent over the hub

/// A machine that is reached over the hub: each command the API stored for it is sent on its session,
/// and what it answers is what the stored command ends with.
struct OverTheLink {
    link: ControllerLink,
}

#[async_trait]
impl Machine for OverTheLink {
    async fn run(&self, command: &Cmd) -> Result<serde_json::Value, CommandError> {
        let mut handle = self.link.commands().issue(command.clone());
        match handle.wait().await {
            Progress::Succeeded(result) => Ok(result),
            Progress::Failed(error) => Err(error),
            other => Err(CommandError::new(
                ErrorCode::Timeout,
                format!("the command did not end well: {other:?}"),
            )),
        }
    }
}

/// What the API does with a machine's events: applies them in a transaction that moves the stream's
/// cursor, and tells the link they are committed (or that they are not).
fn spawn_applier(
    app: TestApp,
    machine: Id,
    link: ControllerLink,
    mut notices: mpsc::UnboundedReceiver<Notice>,
) {
    tokio::spawn(async move {
        while let Some(notice) = notices.recv().await {
            match notice {
                Notice::Deliver(delivery) => {
                    app.deliver_machine_events(machine, &link, delivery).await;
                }
                Notice::Online { .. } => {
                    set_online(&app.app.db, machine, true).await;
                    let _ = app.app.engine.machine_online(machine).await;
                }
                Notice::Offline { .. } => {
                    set_online(&app.app.db, machine, false).await;
                    let _ = app.app.engine.machine_offline(machine).await;
                }
                _ => {}
            }
        }
    });
}

/// What the system's link writes when a machine's session comes or goes: the presence and the moment of
/// it, by which one session is told from the next and an environment on the machine is attached in it.
async fn set_online(pool: &Db, machine: Id, online: bool) {
    let mut conn = pool.acquire().await.unwrap();
    orch_db::queries::resources::set_presence(&mut conn, machine, online, None, chrono::Utc::now())
        .await
        .unwrap();
}

// ---------------------------------------------------------------- the program

/// Where the connector's binary is: next to the test's own, in the target directory. A binary of another
/// package is not built for a test, so it is built here when it is not there.
fn connector_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap().to_owned();
    let binary = profile_dir.join("orch-connector");
    if !binary.exists() {
        let mut build = std::process::Command::new(env!("CARGO"));
        build
            .args(["build", "-p", "orch-connector"])
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        if profile_dir.file_name().and_then(|name| name.to_str()) == Some("release") {
            build.arg("--release");
        }
        let status = build.status().unwrap();
        assert!(status.success(), "the connector could not be built");
    }
    assert!(binary.exists(), "no connector at {}", binary.display());
    binary
}

/// The program, as a command with the data directory it is to work in.
fn program(binary: &Path, data_dir: &Path) -> Command {
    let mut command = Command::new(binary);
    command.arg("--data-dir").arg(data_dir);
    // Nothing of the environment of whoever runs the test is the program's configuration.
    command
        .env_remove("ORCH_DATA_DIR")
        .env_remove("ORCH_LOG")
        .env_remove("RUST_MIN_STACK");
    command
}

/// Runs the program to its end and returns what it printed on standard output; the test fails if it fails.
async fn succeeds(mut command: Command) -> String {
    let output = command.stdin(Stdio::null()).output().await.unwrap();
    assert!(
        output.status.success(),
        "{command:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Looks every few milliseconds whether something holds, for as long as `PATIENCE`.
async fn eventually<F, Fut>(what: &str, mut holds: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let began = Instant::now();
    while began.elapsed() < PATIENCE {
        if holds().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("gave up waiting for: {what}");
}

/// A machine row, for a machine the system knows of.
async fn machine_row(db: &Db, machine: Id, name: &str, roles: &[String]) {
    let mut conn = db.acquire().await.unwrap();
    let mut fingerprint = [0_u8; 32];
    fingerprint[..16].copy_from_slice(machine.as_bytes());
    fingerprint[16..].copy_from_slice(machine.as_bytes());
    let serial = machine.simple().to_string();
    insert_machine(
        &mut conn,
        &NewMachine {
            id: machine,
            name,
            owner_id: None,
            placement: "connected",
            roles,
            platform_os: "linux",
            platform_arch: "x86_64",
            connector_version: "0.1.0",
            protocol_version: 1,
            cert_serial: &serial,
            cert_fingerprint: &fingerprint,
            cert_not_after: Utc::now() + chrono::Duration::days(300),
            public_key: "",
        },
        Utc::now(),
    )
    .await
    .unwrap();
}

fn role_words(roles: &[Role]) -> Vec<String> {
    roles
        .iter()
        .map(|role| {
            serde_json::to_value(role)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// Everything a test stands on: the system with a real connector enrolled and running on a directory,
/// and a site whose environment is that directory.
struct Cluster {
    site: Site,
    system: Arc<System>,
    work: tempfile::TempDir,
    _data: tempfile::TempDir,
    _server_data: tempfile::TempDir,
    _child: Child,
}

impl Cluster {
    /// A connector whose owner lets files be written from outside, or does not.
    async fn start(remote_write: bool) -> Self {
        Self::with_store(remote_write, false, &[], true).await
    }

    /// Production uses a filesystem store from the default storage section. A test only moves
    /// its root into an owned temporary directory; no recording or restore setting is supplied.
    async fn with_store(
        remote_write: bool,
        persistent: bool,
        files: &[(&str, &[u8])],
        make_web: bool,
    ) -> Self {
        Self::with_root_alias(remote_write, persistent, files, make_web, false).await
    }

    async fn with_root_alias(
        remote_write: bool,
        persistent: bool,
        files: &[(&str, &[u8])],
        make_web: bool,
        root_alias: bool,
    ) -> Self {
        let world = support::World::new().await;
        let old = world.app.app.clone();
        let db = old.db.clone();
        let server_data = tempfile::tempdir().unwrap();
        let mut config = (*old.config).clone();
        config.storage.root = server_data.path().display().to_string();
        let config = Arc::new(config);
        // The program keeps the time of the machine it runs on, and the commands it is sent carry
        // deadlines it reads, so the application's clock is that time too.
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let link = Link::detached(db.clone(), Arc::clone(&clock), Arc::clone(&config))
            .expect("a link without gateways can be made");
        let engine = orch_api_core::thread_engine::detached(&db, &clock, &config, &link);
        let blobs = if persistent {
            let store = orch_blob_local::LocalBlobStore::open(&config.storage.root)
                .await
                .unwrap();
            orch_api_core::blobs::Blobs::new(Arc::new(store), Path::new(&config.storage.root))
        } else {
            orch_api_core::blobs::Blobs::in_memory()
        };
        engine.attach_blobs(blobs.store());
        link.set_applier(Arc::new(orch_api_core::thread_engine::EngineApplier::new(
            engine.clone(),
        )));
        let state = Arc::new(AppState {
            host_ports: old.host_ports.clone(),
            builtin_sources: old.builtin_sources.clone(),
            db: db.clone(),
            config: Arc::clone(&config),
            clock: Arc::clone(&clock),
            link,
            engine: engine.clone(),
            realtime: orch_api_core::realtime::Realtime::new(&db, &clock, &old.config),
            build: BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs,
            cache: orch_api_core::cache::Cache::new(&old.config),
        });
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let mut world = world;
        world.app = TestApp::over(Arc::clone(&state), world.app.clock.clone(), registry);

        let (system, enrollment_port) = start_system().await;
        *lock(&system.content) = Some(Content::over(&state));

        // The connector is enrolled with a token, as a person would, and told what its owner allows.
        let binary = connector_binary();
        let data = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let registered_root = if root_alias {
            let alias = work.path().join("registered-root");
            std::os::unix::fs::symlink(".", &alias).unwrap();
            alias
        } else {
            work.path().to_path_buf()
        };
        if make_web {
            std::fs::create_dir_all(work.path().join("web")).unwrap();
        }
        // Fixture files exist before attachment and its initial index. The first-recording
        // assertion must not accidentally depend on an unrelated watcher debounce after attach.
        for (path, bytes) in files {
            let full = work.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, bytes).unwrap();
        }
        let said = succeeds({
            let mut enroll = program(&binary, data.path());
            enroll
                .args([
                    "enroll",
                    "--url",
                    &format!("https://127.0.0.1:{enrollment_port}"),
                ])
                .args(["--token", &system.token(), "--name", "laptop"]);
            enroll
        })
        .await;
        assert!(said.contains("Enrolled laptop"), "{said}");
        for (key, value) in [
            ("environment.roots", work.path().display().to_string()),
            ("environment.remote_write", remote_write.to_string()),
        ] {
            succeeds({
                let mut set = program(&binary, data.path());
                set.args(["policy", "set", key, &value]);
                set
            })
            .await;
        }

        // The system knows the machine, and keeps its link; the events it sends are applied by the engine.
        let arrival = lock(&system.arrival).take().expect("no machine enrolled");
        machine_row(&db, arrival.machine, "laptop", &role_words(&arrival.roles)).await;
        spawn_applier(
            world.app.clone(),
            arrival.machine,
            arrival.link.clone(),
            arrival.notices.expect("the notices of the machine"),
        );
        engine.start();

        // The program runs, and the system knows that.
        let log = std::fs::File::create(data.path().join("run.log")).unwrap();
        let mut run = program(&binary, data.path());
        run.arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .kill_on_drop(true);
        let child = run.spawn().unwrap();
        let (hub, machine) = (system.hub.clone(), arrival.machine);
        eventually("the program holds a session", || {
            let up = hub.session_of(machine).is_some();
            async move { up }
        })
        .await;
        eventually("the system knows the machine is online", || async {
            sqlx::query_scalar("SELECT online FROM machines WHERE id = $1")
                .bind(machine)
                .fetch_one(db.pool())
                .await
                .unwrap()
        })
        .await;

        eventually(
            "the CLI-selected physical write policy is committed",
            || async {
                let facts: serde_json::Value =
                    sqlx::query_scalar("SELECT facts FROM machines WHERE id=$1")
                        .bind(machine)
                        .fetch_one(db.pool())
                        .await
                        .unwrap();
                facts
                    .get("policy")
                    .and_then(|policy| policy.get("environment.remote_write"))
                    .and_then(serde_json::Value::as_bool)
                    == Some(remote_write)
            },
        )
        .await;

        let mut site = Site::in_world(
            world,
            Some(Placed {
                machine,
                root: registered_root.display().to_string(),
            }),
        )
        .await;
        site.power_on_with(Arc::new(OverTheLink { link: arrival.link }));
        // The engine attaches a directory in each session of its machine; nothing here does.
        let environment = site.environment;
        eventually("the engine to attach the directory in the session of the machine", || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT COALESCE(e.status = 'running' AND e.attached_session = m.last_seen_at, false)
                   FROM environments e JOIN machines m ON m.id = e.machine_id WHERE e.id = $1",
            )
            .bind(environment)
            .fetch_one(db.pool())
            .await
            .unwrap()
        })
        .await;
        Self {
            site,
            system,
            work,
            _data: data,
            _server_data: server_data,
            _child: child,
        }
    }
}

async fn job_of(site: &Site, id: Id) -> Job {
    site.app()
        .call(JobGet, &[&id])
        .token(&site.ada.token)
        .send()
        .await
        .ok()
}

// ---------------------------------------------------------------- the tests

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn default_recording_uses_the_deployments_disk_store_and_the_restore_routes_put_back_its_files()
 {
    recording_and_restore(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registered_root_alias_keeps_recording_and_restore_identity() {
    recording_and_restore(true).await;
}

async fn recording_and_restore(root_alias: bool) {
    use orch_api_contract::source::{
        EnvironmentFilesRestore, EnvironmentFilesRestoreBody, EnvironmentRestorePreflight,
        EnvironmentRestorePreflightBody,
    };
    use orch_api_contract::workspace::{SnapshotGet, SnapshotStorage};
    use orch_db::queries::{source, storage};
    use orch_link_contract::commands::SerialKey;
    use orch_link_contract::source::{FsRecord, FsRecordResult, RecordKind, RecordState};
    use orch_settings_contract::registry::{self, keys};
    let cluster = Cluster::with_root_alias(
        true,
        true,
        &[("web/default-restore.txt", b"before the turn\n")],
        true,
        root_alias,
    )
    .await;
    let site = &cluster.site;
    let app = &site.app().app;
    let days = registry::get(keys::WORKSPACE_RECORD_DAYS)
        .unwrap()
        .default_value()
        .unwrap()
        .as_i64()
        .unwrap();
    let max_bytes = registry::get(keys::WORKSPACE_RECORD_MAX_BYTES)
        .unwrap()
        .default_value()
        .unwrap()
        .as_i64()
        .unwrap();
    assert!(
        days > 0 && max_bytes > 0,
        "a fresh install records without a settings edit"
    );
    assert!(Path::new(&app.config.storage.root).join("blobs").is_dir());
    let file = cluster.work.path().join("web/default-restore.txt");
    let snapshot = new_id();
    let command = orch_engine::ids::derived("fs.record.before-turn", &[snapshot.as_bytes()]);
    let dispatcher = app.link.dispatcher();
    dispatcher
        .issue_as(
            command,
            site.machine,
            &SerialKey::Environment(site.environment),
            orch_types::Time::new(app.clock.now() + chrono::Duration::seconds(90)),
            &FsRecord {
                environment_id: site.environment,
                snapshot_id: snapshot,
                directory: "web".into(),
                kind: RecordKind::Turn,
                record_days: days,
                max_bytes,
            },
        )
        .await
        .unwrap();
    let captured = dispatcher.wait_for::<FsRecord>(command).await.unwrap();
    eventually("the original recording result is retained", || async {
        let mut conn = site.world.conn().await;
        orch_db::queries::link::command(&mut conn, command)
            .await
            .unwrap()
            .is_some_and(|row| row.state == "succeeded" && row.result.is_some())
    })
    .await;
    assert_eq!(
        captured.state,
        RecordState::Complete,
        "the first native request: {captured:?}"
    );
    let hash = captured.manifest_blob.unwrap();
    let bytes = app.blobs.read_whole(&hash, 1024 * 1024).await.unwrap();
    let manifest = orch_engine::manifest::Manifest::parse(&bytes).unwrap();
    let mut conn = site.world.conn().await;
    let env = orch_db::queries::environments::workspace::environment(&mut conn, site.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(manifest.header().generation, i64::from(env.generation));
    let registered_root = if root_alias {
        cluster.work.path().join("registered-root")
    } else {
        cluster.work.path().to_path_buf()
    };
    assert_eq!(
        env.workspace_root,
        registered_root.display().to_string(),
        "a native report does not change the owner's registered path"
    );
    assert_eq!(
        env.facts
            .get("workspace_root")
            .and_then(serde_json::Value::as_str),
        Some(cluster.work.path().to_str().unwrap()),
        "the same connector reports its resolved workspace root"
    );
    assert_eq!(
        env.facts
            .get("generation")
            .and_then(serde_json::Value::as_i64),
        Some(i64::from(env.generation))
    );
    assert_eq!(
        manifest.header().root,
        cluster.work.path().join("web").display().to_string()
    );
    let now = app.clock.now();
    source::begin_recorded(
        &mut conn,
        &storage::NewSnapshot {
            id: snapshot,
            thread_id: Some(site.thread),
            environment_id: site.environment,
            env_generation: env.generation,
            label: "Before the turn",
            selection: &serde_json::json!({"include":["web"],"exclude":[]}),
            created_by: Some(site.ada.id()),
            at: now,
        },
        "web",
        "turn",
    )
    .await
    .unwrap();
    // The original native result is good, but old server validation could freeze only a
    // generic unavailable fallback. Reading it must revalidate that original, never recapture.
    let fallback = FsRecordResult {
        state: RecordState::Unavailable,
        tree_ref: None,
        repository_head: None,
        manifest_blob: None,
        files: 0,
        bytes: 0,
        expires_at: captured.expires_at,
        reason: Some("store_unavailable".into()),
    };
    source::finish_recorded(&mut conn, snapshot, &fallback, now)
        .await
        .unwrap();
    // File management is enough to restore; observing idle writers must not require terminal
    // management or deserialize its native response as an unrelated bare array.
    for permission in ["files.view", "files.manage"] {
        orch_db::queries::access::insert_entry(
            &mut conn,
            &orch_db::queries::access::NewEntry {
                id: new_id(),
                page: &orch_db::queries::access::PageKey::new(
                    "environment",
                    site.environment,
                    "live",
                ),
                profile: "any",
                permission,
                holder_kind: "user",
                holder_id: Some(site.vic.id()),
                holder_role: None,
                options: &serde_json::json!({}),
                expires_at: None,
                note: "",
                source: "page",
                created_by: Some(site.ada.id()),
                at: now,
            },
        )
        .await
        .unwrap();
    }
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let shown = site
        .app()
        .call(SnapshotGet, &[&snapshot])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(shown.state, SnapshotState::Complete);
    assert_eq!(shown.storage, SnapshotStorage::Environment);
    assert_eq!(shown.unavailable_reason, None);
    let recordings: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='fs.record' AND body->>'snapshot_id'=$1",
    )
    .bind(snapshot.to_string())
    .fetch_one(app.db.pool())
    .await
    .unwrap();
    assert_eq!(
        recordings, 1,
        "the footer read must use the retained capture, not today's files"
    );
    std::fs::write(&file, "after the turn\n").unwrap();
    let later = cluster.work.path().join("web/new-after-default.txt");
    std::fs::write(&later, "made after the recording\n").unwrap();
    // Bare leases leave one slot for successive short phases. Keeping the request transaction
    // over dispatch/upload would prevent the native plan from ever answering.
    let mut reserved = Vec::new();
    for _ in 1..site.world.db.pool.options().get_max_connections() {
        reserved.push(site.world.db.pool.acquire().await.unwrap());
    }
    let plan = tokio::time::timeout(
        PATIENCE,
        site.app()
            .call(EnvironmentRestorePreflight, &[&site.environment])
            .token(&site.vic.token)
            .body(&EnvironmentRestorePreflightBody {
                snapshot_id: snapshot,
                options: None,
            })
            .send(),
    )
    .await
    .expect("restore planning held the only pool slot while asking the connector or content store")
    .ok();
    drop(reserved);
    assert!(plan.blockers.is_empty(), "{plan:?}");
    assert!(plan.undo_available);
    assert!(
        plan.paths
            .iter()
            .any(|path| path.path == "default-restore.txt")
    );
    let accepted = site
        .app()
        .call(EnvironmentFilesRestore, &[&site.environment])
        .token(&site.vic.token)
        .body(&EnvironmentFilesRestoreBody {
            preflight_id: plan.preflight_id,
        })
        .send()
        .await
        .ok();
    let started = Instant::now();
    let finished = loop {
        let job = job_of(site, accepted.id).await;
        if matches!(
            job.state,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        ) {
            break job;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "the restore job did not finish: {job:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    assert_eq!(std::fs::read(&file).unwrap(), b"before the turn\n");
    assert!(!later.exists());
    let undo = finished.result.as_ref().unwrap()["undo_snapshot_id"]
        .as_str()
        .unwrap()
        .parse::<Id>()
        .unwrap();
    let undo = site
        .app()
        .call(SnapshotGet, &[&undo])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(undo.state, SnapshotState::Complete);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_full_index_reaches_the_server_and_file_name_search_answers_an_empty_result() {
    use orch_api_contract::workspace::{EnvironmentFileSearch, EnvironmentFileSearchReply};
    let cluster = Cluster::with_store(true, true, &[], false).await;
    let site = &cluster.site;
    eventually(
        "the connector's initial complete empty manifest is stored",
        || async {
            let mut conn = site.world.conn().await;
            orch_db::queries::environments::workspace::environment(&mut conn, site.environment)
                .await
                .unwrap()
                .is_some_and(|environment| {
                    environment.manifest_blob.is_some() && environment.manifest_at.is_some()
                })
        },
    )
    .await;
    let answer = site
        .app()
        .call(EnvironmentFileSearch, &[&site.environment])
        .token(&site.ada.token)
        .raw_query("q=absent&in=names")
        .send()
        .await
        .ok();
    assert!(
        matches!(answer, EnvironmentFileSearchReply::Names(entries) if entries.is_empty()),
        "an indexed empty tree answers no matches, not a missing-list failure"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_written_over_http_is_on_the_machines_disk_and_one_changed_there_is_read_back() {
    let cluster = Cluster::start(true).await;
    let (site, work) = (&cluster.site, cluster.work.path());

    // Written through the API, it is on the disk of the machine.
    let stat = ask::write(
        site,
        &site.ada,
        "web/hello.txt",
        Some("none"),
        b"written over HTTP\n",
    )
    .await
    .ok();
    assert_eq!(stat.hash, Some(Hash::of(b"written over HTTP\n")));
    assert_eq!(
        std::fs::read(work.join("web/hello.txt")).unwrap(),
        b"written over HTTP\n"
    );
    let listed = ask::list(site, &site.ada, "web", None).await.ok();
    assert!(
        listed
            .entries
            .iter()
            .any(|entry| entry.name == "hello.txt" && entry.size == 18)
    );
    let attached = site
        .played
        .as_ref()
        .unwrap()
        .seen_of(orch_link_contract::commands::CommandKind::EnvAttach);
    assert_eq!(attached.len(), 1, "the environment was attached once");
    assert_eq!(
        attached[0].body["workspace_root"],
        serde_json::json!(work.display().to_string()),
        "a directory of a person's machine is attached with its root"
    );

    // Changed on the disk, it is read back through the API: a fetch, then the content by its hash.
    std::fs::write(work.join("web/hello.txt"), "changed on the machine\n").unwrap();
    let fetched = ask::fetch(site, &site.ada, "web/hello.txt", Some(30))
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready, "{fetched:?}");
    assert_eq!(fetched.hash, Some(Hash::of(b"changed on the machine\n")));
    let read = ask::content(
        site,
        &site.ada,
        "web/hello.txt",
        &fetched.hash.unwrap(),
        None,
        false,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.raw.as_ref(), b"changed on the machine\n");

    // A write on the version that was seen is refused once the file has changed since.
    std::fs::write(work.join("web/hello.txt"), "changed once more\n").unwrap();
    let late = ask::write(
        site,
        &site.ada,
        "web/hello.txt",
        Some(&fetched.hash.unwrap().to_string()),
        b"mine\n",
    )
    .await
    .problem();
    assert_eq!(late.code, ApiCode::ConflictStaleRevision);
    assert_eq!(late.extra["current"]["size"], 18);
    assert_eq!(
        std::fs::read(work.join("web/hello.txt")).unwrap(),
        b"changed once more\n"
    );

    // Renamed, copied and deleted: each is on the disk.
    ask::mkdir(site, &site.ada, "web/docs").await.ok();
    ask::rename(site, &site.ada, "web/hello.txt", "web/docs/hello.txt")
        .await
        .ok();
    ask::copy(site, &site.ada, "web/docs/hello.txt", "web/docs/copy.txt")
        .await
        .ok();
    assert!(work.join("web/docs/hello.txt").exists() && work.join("web/docs/copy.txt").exists());
    ask::delete(site, &site.ada, "web/docs", true).await.ok();
    assert!(!work.join("web/docs").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_larger_than_one_piece_goes_both_ways() {
    let cluster = Cluster::start(true).await;
    let (site, work) = (&cluster.site, cluster.work.path());

    // Up from the API to the disk: the connector takes it in pieces, each a request for a range.
    let down = bytes_of(6 * 1024 * 1024 + 123, 1);
    ask::write(site, &site.ada, "web/down.bin", Some("none"), &down)
        .await
        .ok();
    let on_disk = std::fs::read(work.join("web/down.bin")).unwrap();
    assert_eq!(on_disk.len(), down.len());
    assert!(
        on_disk == down,
        "the file on the disk is whole and in order"
    );

    // From the disk to the API: the connector puts it up in pieces, each at the offset where the last ended.
    let up = bytes_of(5 * 1024 * 1024 + 7, 2);
    std::fs::write(work.join("web/up.bin"), &up).unwrap();
    let fetched = ask::fetch(site, &site.ada, "web/up.bin", Some(60))
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready, "{fetched:?}");
    assert_eq!(fetched.hash, Some(Hash::of(&up)));
    let read = ask::content(
        site,
        &site.ada,
        "web/up.bin",
        &fetched.hash.unwrap(),
        None,
        false,
    )
    .await;
    assert_eq!(read.raw.len(), up.len());
    assert!(
        read.raw.as_ref() == up.as_slice(),
        "the content is whole and in order"
    );
    // A range of it, as a browser asks for one.
    let part = ask::content(
        site,
        &site.ada,
        "web/up.bin",
        &fetched.hash.unwrap(),
        Some("bytes=4194304-4194403"),
        false,
    )
    .await;
    assert_eq!(part.raw.as_ref(), &up[4_194_304..4_194_404]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_outside_the_workspace_is_refused_and_so_is_a_link_that_leads_out_of_it() {
    let cluster = Cluster::start(true).await;
    let (site, work) = (&cluster.site, cluster.work.path());
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "not yours").unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret.txt"), work.join("web/leak")).unwrap();

    // By its path: the API answers as if there were nothing there.
    for path in ["../secret.txt", "web/../../secret.txt", "/etc/hostname"] {
        assert_eq!(
            ask::stat(site, &site.ada, path).await.problem().code,
            ApiCode::NotFound,
            "{path}"
        );
        assert_eq!(
            ask::write(site, &site.ada, path, None, b"x")
                .await
                .problem()
                .code,
            ApiCode::NotFound,
            "{path}"
        );
    }
    // By a link inside the workspace: the machine leads nowhere, and nothing is written through it.
    let fetched = ask::fetch(site, &site.ada, "web/leak", Some(10)).await.ok();
    assert_eq!(fetched.state, FetchStatus::Unavailable);
    assert_eq!(fetched.reason, Some(FetchUnavailable::NotFound));
    let through = ask::write(site, &site.ada, "web/leak", None, b"overwritten").await;
    assert!(!through.status.is_success(), "{:?}", through.body);
    assert_eq!(
        std::fs::read(outside.path().join("secret.txt")).unwrap(),
        b"not yours"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_the_machines_owner_forbids_is_refused_as_the_problem_it_is() {
    let cluster = Cluster::start(false).await;
    let (site, work) = (&cluster.site, cluster.work.path());
    std::fs::write(work.join("web/keep.txt"), "as it was").unwrap();
    let refused = ask::write(site, &site.ada, "web/keep.txt", None, b"overwritten")
        .await
        .problem();
    assert_eq!(refused.code, ApiCode::PolicyRemoteWriteDisabled);
    assert_eq!(
        std::fs::read(work.join("web/keep.txt")).unwrap(),
        b"as it was"
    );
    // Reading is another matter.
    let fetched = ask::fetch(site, &site.ada, "web/keep.txt", Some(30))
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready);
    assert_eq!(fetched.hash, Some(Hash::of(b"as it was")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_environments_own_write_switch_refuses_without_overriding_its_machines_permission() {
    let cluster = Cluster::start(true).await;
    let site = &cluster.site;
    let file = cluster.work.path().join("web/keep.txt");
    std::fs::write(&file, b"as it was").unwrap();
    site.set_remote_write(false).await;
    let refused = ask::write(site, &site.ada, "web/keep.txt", None, b"overwritten")
        .await
        .problem();
    assert_eq!(refused.code, ApiCode::WorkspaceRemoteWriteDisabled);
    assert_eq!(std::fs::read(&file).unwrap(), b"as it was");
    let fetched = ask::fetch(site, &site.ada, "web/keep.txt", Some(30))
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready);
    assert_eq!(fetched.hash, Some(Hash::of(b"as it was")));
    let mut conn = site.world.conn().await;
    assert!(
        orch_db::queries::environment_facts::permits(
            &mut conn,
            site.environment,
            "environment.remote_write"
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn an_unreported_physical_write_policy_is_refused_before_any_file_mutation() {
    // This negative fixture has no live facts producer that could restore the intentionally absent policy.
    let site = Site::on_a_directory().await;
    site.disk.put("web/keep.txt", b"as it was");
    sqlx::query("UPDATE machines SET facts=facts-'policy' WHERE id=$1")
        .bind(site.machine)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    let refused = ask::write(&site, &site.ada, "web/keep.txt", None, b"overwritten")
        .await
        .problem();
    assert_eq!(refused.code, ApiCode::PolicyRemoteWriteDisabled);
    assert_eq!(
        site.disk.read("web/keep.txt").as_deref(),
        Some(b"as it was".as_slice())
    );
    assert_eq!(
        site.sent(orch_link_contract::commands::CommandKind::FsWrite),
        0
    );
    let fetched = ask::fetch(&site, &site.ada, "web/keep.txt", Some(30))
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready);
    assert_eq!(fetched.hash, Some(Hash::of(b"as it was")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_legacy_create_attaches_the_registered_root_and_reads_its_existing_bytes() {
    use orch_api_contract::environment::{MachineEnvironmentCreate, MachineEnvironmentCreateBody};
    use orch_api_contract::workspace::{
        EnvironmentFileFetch, EnvironmentFileFetchQuery, FilePathBody,
    };
    let cluster = Cluster::start(true).await;
    let site = &cluster.site;
    let root = cluster.work.path().join("registered");
    std::fs::create_dir_all(root.join("web")).unwrap();
    std::fs::write(root.join("web/keep.txt"), b"registered bytes").unwrap();
    {
        let mut conn = site.world.conn().await;
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("machine", site.machine, "live"),
            "any",
            "machine.manage",
            ("user", Some(site.ada.id()), None),
            None,
            site.world.now(),
        )
        .await
        .unwrap();
    }
    let request = MachineEnvironmentCreateBody {
        data_sources: None,
        home_id: site.org,
        name: "Legacy directory".to_owned(),
        root: root.display().to_string(),
        home_node_id: Some(site.topic),
        remote_write: Some(true),
        terminal: Some(false),
        access: None,
    };
    let outside = tempfile::tempdir().unwrap();
    let refused = site
        .app()
        .call(MachineEnvironmentCreate, &[&site.machine])
        .token(&site.ada.token)
        .body(&MachineEnvironmentCreateBody {
            root: outside.path().display().to_string(),
            ..request.clone()
        })
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ApiCode::RequestInvalid,
        "the existing machine roots policy still applies"
    );
    let made = site
        .app()
        .call(MachineEnvironmentCreate, &[&site.machine])
        .token(&site.ada.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert!(
        made.config.get("env.data_sources").is_none(),
        "the legacy request supplies no own data-source choice"
    );
    eventually(
        "the legacy-created directory is attached at its actual registered root",
        || async {
            let found: Option<(String, Option<String>)> = sqlx::query_as(
                "SELECT status,facts->>'workspace_root' FROM environments WHERE id=$1",
            )
            .bind(made.base.id)
            .fetch_optional(&site.world.db.pool)
            .await
            .unwrap();
            found.is_some_and(|(status, actual)| {
                status == "running" && actual.as_deref() == Some(request.root.as_str())
            })
        },
    )
    .await;
    let fetched = site
        .app()
        .call(EnvironmentFileFetch, &[&made.base.id])
        .token(&site.ada.token)
        .query(&EnvironmentFileFetchQuery {
            thread: None,
            mount: None,
            wait: Some(30),
        })
        .body(&FilePathBody {
            path: "web/keep.txt".to_owned(),
        })
        .send()
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready);
    assert_eq!(fetched.hash, Some(Hash::of(b"registered bytes")));
    assert_eq!(
        std::fs::read(root.join("web/keep.txt")).unwrap(),
        b"registered bytes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_snapshot_is_taken_by_the_machine_and_one_of_its_files_is_read() {
    let cluster = Cluster::start(true).await;
    let (site, work) = (&cluster.site, cluster.work.path());
    std::fs::create_dir_all(work.join("web/src")).unwrap();
    std::fs::write(work.join("web/src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(work.join("web/README.md"), "# Web\n").unwrap();
    std::fs::write(work.join("web/big.bin"), bytes_of(300_000, 3)).unwrap();

    let begun = site
        .app()
        .call(ThreadSnapshotCreate, &[&site.thread])
        .token(&site.ada.token)
        .body(&ThreadSnapshotCreateBody {
            label: Some("Before the change".to_owned()),
            selection: None,
        })
        .send()
        .await
        .ok();
    assert!(matches!(begun.state, JobState::Queued | JobState::Running));
    eventually("the snapshot job to end", || async {
        !matches!(
            job_of(site, begun.id).await.state,
            JobState::Queued | JobState::Running
        )
    })
    .await;
    let ended = job_of(site, begun.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{ended:?}");
    assert_eq!(ended.progress.percent, 100);
    let id: Id = ended.result.unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let snapshot = site
        .app()
        .call(SnapshotGet, &[&id])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(snapshot.state, SnapshotState::Complete);
    assert_eq!(snapshot.label, "Before the change");
    assert_eq!(snapshot.file_count, 3);
    assert_eq!(snapshot.missing_count, 0);
    let listed = site
        .app()
        .call(SnapshotFileList, &[&id])
        .token(&site.vic.token)
        .query(&SnapshotPathQuery {
            path: String::new(),
        })
        .send()
        .await
        .ok();
    let names: Vec<_> = listed
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["src", "big.bin", "README.md"]);

    // One of its files is read from the store of content, with the machine no longer needed.
    let main = site
        .app()
        .call(SnapshotFileContentGet, &[&id])
        .token(&site.ada.token)
        .query(&SnapshotPathQuery {
            path: "src/main.rs".to_owned(),
        })
        .send_bytes()
        .await;
    assert_eq!(main.raw.as_ref(), b"fn main() {}\n");
    // The disk moved on since; the snapshot did not.
    std::fs::write(work.join("web/src/main.rs"), "fn main() { changed }\n").unwrap();
    let again = site
        .app()
        .call(SnapshotFileContentGet, &[&id])
        .token(&site.ada.token)
        .query(&SnapshotPathQuery {
            path: "src/main.rs".to_owned(),
        })
        .send_bytes()
        .await;
    assert_eq!(again.raw.as_ref(), b"fn main() {}\n");
    let page = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(&site.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    drop(cluster.system);
}
