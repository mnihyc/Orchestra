//! A shell on a real connector process, with a real WebSocket client: a shell is made, `echo` typed into it comes back, the socket is dropped and attached again and the earlier output is there, a resize reaches the program, `exit 3` ends it with that code, and the list shows it gone; and a shell that its connector was restarted under is closed as lost by the link when the environment is attached again.
//!
//! **What is real:** the API's router over a real database, with the engine, the realtime server and the
//! terminal sockets served on a loopback socket; the connector as a process (its own binary, enrolled over
//! HTTPS, holding a session over TLS and a WebSocket to a listener on the loopback), with a pseudo-terminal
//! and the shell program of the machine it runs on; the browser's end, a WebSocket client that speaks the
//! protocol of the terminal sockets. **What stands in:** the gateway and the link of the API are the
//! testkit's hub behind the same kind of sockets (as the connector's own tests have it); the environment is
//! attached by the test, with the command that attaches one, because the step that does so in the system
//! belongs to another part; and the machine's local policy, which the connector reports to the system with
//! its facts, is written to the machine's row by the test for the same reason.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
// The program is stopped the way a service manager stops it, with a termination signal.
#![cfg(unix)]

mod places;
mod resources;
mod rig;
mod support;
mod terminals_support;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_api_contract::terminals::{
    EnvironmentTerminalCreate, EnvironmentTerminalList, TerminalState, TerminalTicketCreate,
};
use orch_api_core::backplane::Link;
use orch_api_core::registry::Registry;
use orch_api_core::state::{App, AppState};
use orch_api_core::testing::TestApp;
use orch_build_info::BuildInfo;
use orch_db::Db;
use orch_db::queries::link as link_queries;
use orch_db::queries::resources::{NewMachine, insert_machine};
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing, Stage};
use orch_engine::config::EngineConfig;
use orch_engine::live::Discard;
use orch_engine::{Engine, EngineParts};
use orch_link::commands::{CommandHandle, Progress};
use orch_link::controller::{ControllerLink, Notice};
use orch_link::framing::{Connection, MessageSink, MessageSource};
use orch_link::websocket::accept_tls;
use orch_link_contract::commands::{Cmd, EnvAttach, SerialKey};
use orch_link_contract::enrollment::{
    ENROLL_PATH, Endpoint, EnrollReply, EnrollReplySystem, EnrollRequest, Trust,
};
use orch_link_contract::session::Role;
use orch_pki::ca::Holder;
use orch_pki::certificates::ServiceName;
use orch_pki::enrollment::{MemoryLedger, TokenGrant, TokenPurpose, issue_token};
use orch_pki::verification::{TlsIdentity, pinned_server_config, plain_server_config};
use orch_testkit::doubles::link::{Hub, MachineRecord, TestPki};
use orch_types::{Clock, Id, SystemClock, new_id};
use places::{admin, make_org};
use resources::name_person;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use terminals_support::Screen;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

/// How long the test waits for something that has to happen, in real time.
const PATIENCE: Duration = Duration::from_secs(60);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------- the system on the loopback

/// The machine that enrolled: what the system keeps of it to talk to it.
struct Arrival {
    machine: Id,
    roles: Vec<Role>,
    link: ControllerLink,
    notices: Option<mpsc::UnboundedReceiver<Notice>>,
}

/// The system's end of the sockets: an authority, the hub behind it, the tokens it gave, and the machine
/// it enrolled.
struct System {
    clock: Arc<dyn Clock>,
    pki: TestPki,
    hub: Hub,
    ledger: MemoryLedger,
    gateway_port: u16,
    arrival: Mutex<Option<Arrival>>,
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
    /// endpoint named is the listener of the gateway.
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

/// Carries the messages of one half of a connection to the other, and lets the other half go when this one
/// has ended: a sink that is dropped is how the end of a connection reaches the hub.
async fn pump(mut from: Box<dyn MessageSource>, mut to: Box<dyn MessageSink>) {
    while let Ok(Some(message)) = from.recv().await {
        if to.send(message).await.is_err() {
            break;
        }
    }
}

/// The gateway's listener: TLS under the system's authority with the machine's certificate asked for, then
/// the WebSocket, then the hub, which holds the session as it would in memory.
async fn serve_gateway(system: Arc<System>, listener: TcpListener) {
    let tls = pinned_server_config(
        &system.server_identity(),
        &system.pki.certificate(),
        90,
        Arc::clone(&system.clock),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(tls);
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        let (acceptor, system) = (acceptor.clone(), Arc::clone(&system));
        tokio::spawn(async move {
            let Ok(outside) = accept_tls(&acceptor, socket).await else {
                return;
            };
            let certificate = outside.peer.client_certificate.clone();
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
                async move { Ok::<_, Infallible>(answer(&system, request).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn answer(system: &System, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let reply = |status: StatusCode, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body)))
            .unwrap()
    };
    if request.method() != Method::POST || request.uri().path() != ENROLL_PATH {
        return reply(StatusCode::NOT_FOUND, Vec::new());
    }
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let Ok(asked) = serde_json::from_slice::<EnrollRequest>(&body) else {
        return reply(StatusCode::UNPROCESSABLE_ENTITY, Vec::new());
    };
    match system.enroll(&asked) {
        Ok(enrolled) => reply(StatusCode::CREATED, serde_json::to_vec(&enrolled).unwrap()),
        Err(status) => reply(status, Vec::new()),
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
    });
    let enrollment_port = enrollment.local_addr().unwrap().port();
    tokio::spawn(serve_gateway(Arc::clone(&system), gateway));
    tokio::spawn(serve_enrollment(Arc::clone(&system), enrollment));
    (system, enrollment_port)
}

// ---------------------------------------------------------------- the engine's commands, sealed

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
    command.env_remove("ORCH_DATA_DIR").env_remove("ORCH_LOG");
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

/// Asks the process to stop as a service manager does, and waits for it to end.
async fn terminate(child: &mut Child) -> std::process::ExitStatus {
    let pid = child.id().expect("the program already ended");
    let sent = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .await
        .unwrap();
    assert!(sent.success());
    tokio::time::timeout(PATIENCE, child.wait())
        .await
        .expect("the program did not stop")
        .unwrap()
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

/// A machine row, for a machine the system knows of. The fingerprint and the serial of its certificate
/// are made from its id, as two machines never share one.
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

/// What the API does with a machine's events: applies them in a transaction that moves the stream's cursor,
/// and tells the link they are committed (or that they are not).
fn spawn_applier(
    pool: Db,
    engine: Engine,
    clock: Arc<dyn Clock>,
    machine: Id,
    link: ControllerLink,
    mut notices: mpsc::UnboundedReceiver<Notice>,
) {
    tokio::spawn(async move {
        while let Some(notice) = notices.recv().await {
            match notice {
                Notice::Deliver(delivery) => {
                    let applied = async {
                        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
                        engine
                            .apply_events(&mut tx, machine, &delivery)
                            .await
                            .map_err(|e| e.to_string())?;
                        let first = delivery.events.first().map_or(1, |event| event.seq);
                        let moved = link_queries::advance_cursor(
                            &mut tx,
                            machine,
                            &delivery.stream.to_string(),
                            first - 1,
                            delivery.last_seq,
                            clock.now(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        if !moved {
                            return Ok::<bool, String>(false);
                        }
                        tx.commit().await.map_err(|e| e.to_string())?;
                        Ok(true)
                    }
                    .await;
                    match applied {
                        Ok(true) => link.committed(&delivery),
                        Ok(false) => link.abandoned(&delivery),
                        Err(error) => {
                            eprintln!("events could not be applied: {error}");
                            link.abandoned(&delivery);
                        }
                    }
                }
                Notice::Online { .. } => {
                    set_online(&pool, machine, true).await;
                    let _ = engine.machine_online(machine).await;
                }
                Notice::Offline { .. } => {
                    set_online(&pool, machine, false).await;
                    let _ = engine.machine_offline(machine).await;
                }
                _ => {}
            }
        }
    });
}

async fn set_online(pool: &Db, machine: Id, online: bool) {
    sqlx::query("UPDATE machines SET online = $2 WHERE id = $1")
        .bind(machine)
        .bind(online)
        .execute(pool.pool())
        .await
        .unwrap();
}

// ---------------------------------------------------------------- the engine's commands, sent over the link

struct Sent {
    handle: CommandHandle,
    at: DateTime<Utc>,
}

/// The engine's end of the one machine's commands: each is put on the machine's link as it is issued, and a
/// command asked for twice under one identifier is made once.
struct LinkCommands {
    link: Mutex<Option<ControllerLink>>,
    sent: Mutex<HashMap<Id, Sent>>,
    clock: Arc<dyn Clock>,
}

fn stage_of(progress: &Progress) -> Stage {
    match progress {
        Progress::Queued => Stage::Queued,
        Progress::Sent => Stage::Sent,
        Progress::Accepted => Stage::Accepted,
        _ => Stage::Over,
    }
}

fn outcome_of(progress: &Progress) -> Option<Outcome> {
    match progress {
        Progress::Succeeded(body) => Some(Outcome::Succeeded(body.clone())),
        Progress::Failed(error) => Some(Outcome::Failed(error.clone())),
        Progress::Expired => Some(Outcome::Expired),
        Progress::Cancelled => Some(Outcome::Cancelled),
        _ => None,
    }
}

#[async_trait]
impl Commands for LinkCommands {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        if lock(&self.sent).contains_key(&command.id) {
            return Ok(());
        }
        let link = lock(&self.link)
            .clone()
            .ok_or(CommandsError::UnknownMachine)?;
        let now = self.clock.now();
        let deadline = command.deadline.unwrap_or_else(|| {
            orch_types::Time::new(
                now + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        });
        let handle = link.commands().issue(Cmd {
            id: command.id,
            kind: command.kind.as_str().to_owned(),
            key: command.key.wire(),
            deadline,
            body: command.body,
        });
        lock(&self.sent).insert(command.id, Sent { handle, at: now });
        Ok(())
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        Ok(lock(&self.sent).get(&id).map(|sent| {
            let progress = sent.handle.progress();
            Info {
                stage: stage_of(&progress),
                issued_at: sent.at,
                outcome: outcome_of(&progress),
            }
        }))
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        let handle = lock(&self.sent).get(&id).map(|sent| sent.handle.clone());
        let Some(mut handle) = handle else {
            return Err(CommandsError::Other("there is no such command".to_owned()));
        };
        let progress = handle.wait().await;
        outcome_of(&progress).ok_or(CommandsError::Stopping)
    }

    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        let (link, open) = match lock(&self.sent).get(&id) {
            Some(sent) => (lock(&self.link).clone(), !sent.handle.progress().is_final()),
            None => (None, false),
        };
        if let (true, Some(link)) = (open, link) {
            link.commands().cancel(id);
        }
        Ok(open)
    }
}

// ---------------------------------------------------------------- the plant

/// The API over a database, with its sockets on the loopback, the system's end of a machine, and a connector
/// that has enrolled (and is not running until it is told to).
struct Plant {
    world: support::World,
    db: Db,
    commands: Arc<LinkCommands>,
    system: Arc<System>,
    binary: PathBuf,
    data: tempfile::TempDir,
    work: tempfile::TempDir,
    connector: Id,
    owner: places::Who,
    environment: Id,
    addr: SocketAddr,
}

impl Plant {
    async fn new() -> Self {
        let mut world = support::World::new().await;
        let old = world.app.app.clone();
        let db = old.db.clone();
        // The program keeps the time of the machine it runs on, and so do the engine, whose commands carry
        // deadlines the program reads, and the link, whose tickets the relay checks.
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let commands = Arc::new(LinkCommands {
            link: Mutex::new(None),
            sent: Mutex::new(HashMap::new()),
            clock: Arc::clone(&clock),
        });
        let engine = Engine::new(EngineParts {
            db: db.clone(),
            clock: Arc::clone(&clock),
            config: EngineConfig::default(),
            commands: commands.clone(),
            access: Arc::new(orch_api_core::thread_access::AuthzAccess::new(Arc::clone(
                &clock,
            ))),
            live: Arc::new(Discard),
        });
        engine.start();
        let link = Link::detached(db.clone(), Arc::clone(&clock), Arc::clone(&old.config))
            .expect("a link without gateways can be made");
        let app: App = Arc::new(AppState {
            host_ports: old.host_ports.clone(),
            builtin_sources: old.builtin_sources.clone(),
            db: db.clone(),
            config: Arc::clone(&old.config),
            clock: Arc::clone(&clock),
            link,
            engine: engine.clone(),
            realtime: orch_api_core::realtime::Realtime::new(&db, &clock, &old.config),
            build: BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs: orch_api_core::blobs::Blobs::in_memory(),
            cache: orch_api_core::cache::Cache::detached(),
        });
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        world.app = TestApp::over(Arc::clone(&app), world.app.clock.clone(), registry);

        // The system on the loopback; its relay believes the signatures of the tickets the link makes, and
        // the terminal sockets reach machines through it.
        let (system, enrollment_port) = start_system().await;
        system
            .hub
            .set_ticket_verifier(app.link.tickets().verifier());
        app.realtime.terminals().through(system.hub.terminals());

        // The connector is enrolled with a token, as a person would, and told what its owner allows.
        let binary = connector_binary();
        let data = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let said_by_enroll = succeeds({
            let mut enroll = program(&binary, data.path());
            enroll
                .args([
                    "enroll",
                    "--url",
                    &format!("https://127.0.0.1:{enrollment_port}"),
                ])
                .args(["--token", &system.token(), "--name", "keeper"]);
            enroll
        })
        .await;
        assert!(
            said_by_enroll.contains("Enrolled keeper"),
            "{said_by_enroll}"
        );
        for (key, value) in [
            (
                "environment.roots",
                json!([work.path().display().to_string()]).to_string(),
            ),
            ("environment.terminal", "true".to_owned()),
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
        let words = role_words(&arrival.roles);
        machine_row(&db, arrival.machine, "keeper", &words).await;
        *lock(&commands.link) = Some(arrival.link.clone());
        spawn_applier(
            db.clone(),
            engine.clone(),
            Arc::clone(&clock),
            arrival.machine,
            arrival.link,
            arrival.notices.expect("the notices of the machine"),
        );
        // What the connector reports of its local policy with its facts.
        let policy = orch_link_contract::policy::LocalPolicy {
            environment_roots: vec![work.path().display().to_string()],
            environment_terminal: true,
            ..orch_link_contract::policy::LocalPolicy::default()
        };
        sqlx::query(
            "UPDATE machines SET facts = jsonb_build_object('policy', $2::jsonb) WHERE id = $1",
        )
        .bind(arrival.machine)
        .bind(serde_json::to_value(&policy).unwrap())
        .execute(db.pool())
        .await
        .unwrap();

        // A person who owns an org, and a directory of the machine registered as their environment.
        let owner = admin(&world, "root").await;
        let org = make_org(&world, &owner, "Acme", "acme").await.base.id;
        let environment = new_id();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id,
                                       terminal, status)
             VALUES ($1, 'Work', $2, $3, 'connected', $3, $4, $5, true, 'running')",
        )
        .bind(environment)
        .bind(owner.id())
        .bind(org)
        .bind(work.path().display().to_string())
        .bind(arrival.machine)
        .execute(db.pool())
        .await
        .unwrap();
        name_person(
            &world,
            "environment",
            environment,
            "environment.manage",
            owner.id(),
        )
        .await;

        // The sockets of the API on the loopback.
        app.realtime.start(engine);
        app.realtime.listening().await;
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let router = orch_api_core::server::router(Arc::clone(&app), registry);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ =
                orch_api_core::server::serve_plain(listener, router, std::future::pending()).await;
        });
        Self {
            world,
            db,
            commands,
            system,
            binary,
            data,
            work,
            connector: arrival.machine,
            owner,
            environment,
            addr,
        }
    }

    fn log(&self) -> PathBuf {
        self.data.path().join("logs").join("run.log")
    }

    /// Runs the program, and waits until it holds its session and the system knows that.
    async fn run_connector(&self) -> Child {
        let log = std::fs::File::create(self.log()).unwrap();
        let mut run = program(&self.binary, self.data.path());
        run.arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .kill_on_drop(true);
        let child = run.spawn().unwrap();
        let (hub, machine) = (self.system.hub.clone(), self.connector);
        eventually("the program holds a session", || {
            let up = hub.session_of(machine).is_some();
            async move { up }
        })
        .await;
        eventually("the system knows the machine is online", || async {
            sqlx::query_scalar("SELECT online FROM machines WHERE id = $1")
                .bind(machine)
                .fetch_one(self.db.pool())
                .await
                .unwrap()
        })
        .await;
        child
    }

    /// Attaches the environment to the connector, as the step that does so in the system does.
    async fn attach_environment(&self) {
        let body = EnvAttach {
            resources: None,
            data_generation: None,
            storage: None,
            data_sources: None,
            environment_id: self.environment,
            workspace_root: Some(self.work.path().display().to_string()),
            ignore: Vec::new(),
            index: orch_link_contract::commands::IndexMode::Full,
            mounts: Some(Vec::new()),
            variables: BTreeMap::new(),
            secrets: BTreeMap::new(),
        };
        let command = Outgoing::new(
            new_id(),
            self.connector,
            SerialKey::Environment(self.environment),
            &body,
        )
        .unwrap();
        let id = command.id;
        self.commands.issue(command).await.unwrap();
        let outcome = tokio::time::timeout(PATIENCE, self.commands.wait(id))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, Outcome::Succeeded(_)), "{outcome:?}");
        // The engine looks after the directories of a machine as well: it marked this one `offline` while its
        // machine was away and puts it back `running` when its own attach has been answered, which is not the
        // attach above and may come after it. A shell is opened from a directory that stands `running`.
        eventually("the environment to stand running", || async {
            let status: String =
                sqlx::query_scalar("SELECT status::text FROM environments WHERE id = $1")
                    .bind(self.environment)
                    .fetch_one(self.db.pool())
                    .await
                    .unwrap();
            status == "running"
        })
        .await;
    }

    /// The owner opens a shell from the environment's own page.
    async fn open(&self) -> orch_api_contract::terminals::EnvironmentTerminalCreateReply {
        self.world
            .app
            .call(EnvironmentTerminalCreate, &[&self.environment])
            .token(&self.owner.token)
            .raw_body(&json!({ "cols": 100, "rows": 30 }))
            .send()
            .await
            .ok()
    }

    async fn ticket(&self, shell: Id) -> String {
        self.world
            .app
            .call(TerminalTicketCreate, &[&shell])
            .token(&self.owner.token)
            .send()
            .await
            .ok()
            .ticket
    }

    async fn screen(&self, shell: Id, ticket: &str) -> Screen {
        Screen::connect(
            self.addr,
            &format!("/api/v1/ws/terminals/{shell}?ticket={ticket}"),
        )
        .await
    }

    async fn shells(&self, state: &str) -> Vec<orch_api_contract::terminals::TerminalSession> {
        self.world
            .app
            .call(EnvironmentTerminalList, &[&self.environment])
            .token(&self.owner.token)
            .raw_query(&format!("state={state}"))
            .send()
            .await
            .ok()
            .items
    }
}

// ---------------------------------------------------------------- the tests

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_on_a_real_connector_is_typed_into_left_taken_up_again_resized_and_ended_with_its_code()
 {
    let plant = Plant::new().await;
    let mut connector = plant.run_connector().await;
    plant.attach_environment().await;

    // A shell is made on the machine, and its person attaches.
    let made = plant.open().await;
    let shell = made.session.id;
    assert_eq!(made.session.state, TerminalState::Open);
    assert!(
        !made.session.shell.is_empty(),
        "the connector says which program it started"
    );
    let mut screen = plant.screen(shell, &made.ticket).await;
    let ready = screen.ready().await;
    assert_eq!(
        (ready["cols"].as_i64(), ready["rows"].as_i64()),
        (Some(100), Some(30))
    );

    // What is typed comes back from the program.
    screen.type_("echo $((6*7))\n").await;
    screen.until("42").await;

    // The socket is dropped, and the shell runs on: it is attached again, and the earlier output is there.
    screen.drop_it().await;
    eventually("the shell is left", || async {
        plant
            .shells("open")
            .await
            .first()
            .is_some_and(|session| !session.attached)
    })
    .await;
    let mut again = plant.screen(shell, &plant.ticket(shell).await).await;
    let ready = again.ready().await;
    assert!(ready["replay_bytes"].as_i64().unwrap() > 0, "{ready}");
    let replayed = String::from_utf8_lossy(&again.shown).into_owned();
    assert!(
        replayed.contains("42"),
        "the earlier output is there: {replayed:?}"
    );

    // A resize reaches the program.
    again
        .send(json!({ "op": "resize", "cols": 111, "rows": 37 }))
        .await;
    again.type_("stty size\n").await;
    again.until("37 111").await;

    // Foreground work stays alive until the test releases it, rather than relying on a timed sleep.
    again
        .type_("sh -c 'echo busy-start-$((6*7)); read release; echo busy-end-$((6*7))'\n")
        .await;
    again.until("busy-start-42").await;
    let sessions = plant.shells("open").await;
    assert!(
        sessions
            .iter()
            .find(|session| session.id == shell)
            .unwrap()
            .busy,
        "the real foreground child is shown busy"
    );
    again.type_("continue\n").await;
    again.until("busy-end-42").await;
    eventually("the foreground returns to the shell prompt", || async {
        plant
            .shells("open")
            .await
            .iter()
            .find(|session| session.id == shell)
            .is_some_and(|session| !session.busy)
    })
    .await;

    // The program ends with a code, which the screen is told; and the list shows the shell gone.
    again.type_("exit 3\n").await;
    assert_eq!(again.until_the_end().await, Some(1000));
    let told: Vec<_> = again
        .frames
        .iter()
        .filter(|frame| frame["op"] != "ready")
        .cloned()
        .collect();
    assert_eq!(
        told,
        [
            json!({ "op": "exit", "code": 3 }),
            json!({ "op": "closed", "reason": "exited" })
        ]
    );
    assert!(plant.shells("open").await.is_empty());
    let gone = plant.shells("closed").await;
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].state, TerminalState::Closed);
    assert_eq!(gone[0].exit_code, Some(3));
    assert_eq!(gone[0].close_reason.as_deref(), Some("exited"));
    terminate(&mut connector).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_screen_takes_a_shell_of_a_real_connector_and_the_first_is_told() {
    let plant = Plant::new().await;
    let mut connector = plant.run_connector().await;
    plant.attach_environment().await;
    let made = plant.open().await;
    let shell = made.session.id;
    let mut first = plant.screen(shell, &made.ticket).await;
    first.ready().await;
    first.type_("echo before\n").await;
    first.until("before").await;
    let mut second = plant.screen(shell, &plant.ticket(shell).await).await;
    second.ready().await;
    assert!(
        String::from_utf8_lossy(&second.shown).contains("before"),
        "the second screen finds the screen as it is"
    );
    assert_eq!(first.until_the_end().await, Some(1000));
    assert_eq!(first.closed_reason().as_deref(), Some("attached_elsewhere"));
    second.type_("echo after\n").await;
    second.until("after").await;
    assert_eq!(
        plant.shells("open").await.len(),
        1,
        "the shell is untouched"
    );
    terminate(&mut connector).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_that_its_connector_was_restarted_under_is_closed_as_lost_by_the_link() {
    let plant = Plant::new().await;
    let mut connector = plant.run_connector().await;
    plant.attach_environment().await;
    let made = plant.open().await;
    let shell = made.session.id;
    // The connector dies with the shell in it, without a word, as a machine that is switched off.
    connector.kill().await.unwrap();
    let (hub, machine) = (plant.system.hub.clone(), plant.connector);
    eventually("the session of the connector that died is gone", || {
        let gone = hub.session_of(machine).is_none();
        async move { gone }
    })
    .await;
    eventually("the system knows the machine is away", || async {
        !sqlx::query_scalar::<_, bool>("SELECT online FROM machines WHERE id = $1")
            .bind(machine)
            .fetch_one(plant.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert!(
        plant.shells("open").await.len() == 1,
        "the system cannot know yet"
    );
    let mut restarted = plant.run_connector().await;
    plant.attach_environment().await;
    // The connector no longer has the shell, and says so when the environment is attached again.
    eventually("the shell is closed", || async {
        plant
            .shells("closed")
            .await
            .iter()
            .any(|session| session.id == shell)
    })
    .await;
    let gone = plant.shells("closed").await;
    assert_eq!(gone[0].close_reason.as_deref(), Some("link_lost"));
    assert!(plant.shells("open").await.is_empty());
    terminate(&mut restarted).await;
}
