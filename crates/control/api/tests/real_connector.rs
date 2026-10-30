//! A thread made over HTTP runs its turn through a real connector, started as a program: it is enrolled with a token, is given a provider's key sealed to its certificate, opens and closes the lease of the turn, and the reply is read back over HTTP; in the second test the model call is let through its forwarder with a key the harness never had, and one row of the usage ledger counts it.
//!
//! **What is real in both:** the API's router over a real database; the thread engine; the connector as a
//! process (its own binary, enrolled over HTTPS, holding a session over TLS and a WebSocket to a listener on
//! the loopback); the provider role in it, which opens the key the system sealed to it and keeps it sealed;
//! its leases and its forwarder; the scripted model server as the provider's upstream. **What stands in:** the
//! gateway and the link of the API are the testkit's hub behind the same kind of sockets (as the connector's
//! own tests have it), and the commands of the engine are sent by a double that seals a secret to the
//! machine's key and signs a ticket the way the link does when a frame is written.
//!
//! Machine reports use the application's backplane transaction: the facts and cursor come from the
//! connector's actual stream and are acknowledged only after they commit.
//!
//! **The first test** puts the harness on the connector: the scripted harness that the program hosts answers
//! the turn. That harness (kind `fake`) makes no call through the connector's listener on the forwarded path
//! (its manual says so), and the program carries no other harness that runs without its own program; and a
//! harness reaches the listener with the token of its process and the key of its session, which exist only
//! inside the connector's process. So this test shows the key's whole journey up to the point where a
//! harness would use it, not the call.
//!
//! **The second test** puts the harness on a machine of its own (a scripted one) and the provider on the
//! connector: the key stays with the connector and the harness is beside its tools. The engine asks the
//! connector to start the provider (the key sealed to the connector's certificate) and to open the lease, and
//! the harness's machine to open the session with the ticket of the lease; the harness calls the connector's
//! forwarder through an `llm` tunnel with that ticket, never having the key, and says what the model said.
//! One row of the usage ledger counts the call.
//!
//! **The third test** puts everything on the connector: a profile of the scripted harness that never ran and a
//! subscription whose sign-in is the harness's own, with the environment beside them. The thread is made over
//! HTTP, which has the program start the harness once to say what it is; the turn then starts it there on its
//! own sign-in, with no key, no forwarder and no lease on any machine, and is counted from the totals the
//! harness reports.
//!
//! **The fourth test** has a person type the key into the program (`provider set-key`): the system never holds
//! it, `provider.start` carries none, and the provider role opens it from the sealed store when a request needs
//! it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
// The program is stopped the way a service manager stops it, with a termination signal.
#![cfg(unix)]

mod places;
mod rig;
mod support;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_api_contract::share::{
    ShareAgentCreate, ShareDocumentsLevel, ShareParts, ShareTo, ShareToNode, ShareWhat,
    ShareWhatDocument, ShareWithAgents,
};
use orch_api_contract::threads::{
    ContentPart, Delivery, ShareMode, ThreadInputCreate, ThreadInputCreateBody, ThreadState,
    ThreadSyncGet, ThreadSyncGetQuery,
};
use orch_api_core::backplane::Link;
use orch_api_core::dispatch::{SecretSource, StoredSecrets};
use orch_api_core::registry::Registry;
use orch_api_core::state::AppState;
use orch_api_core::testing::TestApp;
use orch_api_core::thread_access::AuthzAccess;
use orch_api_core::tickets::Tickets;
use orch_build_info::BuildInfo;
use orch_db::Db;
use orch_db::queries::resources::{NewMachine, insert_machine};
use orch_db::secrets::MasterKey;
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing, Stage};
use orch_engine::live::Discard;
use orch_engine::{Engine, EngineParts};
use orch_link::commands::{CommandHandle, Progress};
use orch_link::controller::{ControllerLink, Notice};
use orch_link::framing::{Connection, MessageSink, MessageSource};
use orch_link::machine::MachineLink;
use orch_link::websocket::accept_tls;
use orch_link_contract::commands::{
    AgentStartResult, AgentStop, Cmd, Command as LinkCommand, CommandError, CommandKind,
    CommandResult, Empty, ErrorCode, ProviderDiscoverModels, Reference, SerialKey,
    SessionOpenProvider, SessionOpenResult, TicketSlot, TicketWanted, TurnStart,
    resolve_references,
};
use orch_link_contract::enrollment::{
    ENROLL_PATH, Endpoint, EnrollReply, EnrollReplySystem, EnrollRequest, Trust,
};
use orch_link_contract::events::{Event, TurnOutcome};
use orch_link_contract::policy::{LocalPolicy, PolicyHarness};
use orch_link_contract::session::{
    MachineFacts, MachineFactsAdapter, MachineFactsHarness, MachineFactsRates, Role,
};
use orch_link_contract::streams::Stream;
use orch_pki::ca::Holder;
use orch_pki::certificates::{Certificate, ServiceName};
use orch_pki::enrollment::{MemoryLedger, TokenGrant, TokenPurpose, issue_token};
use orch_pki::sealing::{RecipientKey, info_for_command, seal};
use orch_pki::verification::{TlsIdentity, pinned_server_config, plain_server_config};
use orch_testkit::doubles::link::{Hub, MachineRecord, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_testkit::doubles::model::{MockConfig, MockLlm, Verdict};
use orch_types::{Clock, Id, Json, SystemClock, Time, new_id};
use places::{admin, make_org, make_project, make_topic};
use rig::{descriptor, engine_config, said};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

/// How long the test waits for something that has to happen, in real time.
const PATIENCE: Duration = Duration::from_secs(60);

/// The key of the provider, which nothing but the system's store, the sealed command and the connector's
/// provider role may ever hold.
const KEY: &str = "sk-orch-real-connector-9d3f51c8a07e42b6";

/// The model the scripted harness is asked to run.
const MODEL: &str = "mock-large";

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
    certificate: Certificate,
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
            certificate: enrolled.certificate.clone(),
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

struct Sent {
    cmd: Cmd,
    machine: Id,
    handle: CommandHandle,
    at: DateTime<Utc>,
}

/// The engine's end of the machine's commands, as the link makes them when a frame is written: a secret is
/// replaced by its value sealed to the machine's key (for the command's own identifier), and a ticket by one
/// signed now. A command asked for twice under one identifier is made once. What the machine is sent is kept,
/// so that a test can look at it.
struct SealingCommands {
    machines: Mutex<HashMap<Id, (ControllerLink, RecipientKey)>>,
    secrets: StoredSecrets,
    tickets: Tickets,
    sent: Mutex<HashMap<Id, Sent>>,
    order: Mutex<Vec<(Id, String)>>,
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

impl SealingCommands {
    /// A machine has enrolled: commands for it are sent over its link and sealed to its key.
    fn arrived(&self, machine: Id, link: ControllerLink, key: RecipientKey) {
        lock(&self.machines).insert(machine, (link, key));
    }

    /// The kinds of the commands that were sent, in the order they were asked for, each with the machine it
    /// was for.
    fn kinds(&self) -> Vec<(Id, String)> {
        lock(&self.order).clone()
    }

    /// How far each command stands, for a failure to be read.
    fn progress(&self) -> Vec<String> {
        lock(&self.sent)
            .values()
            .map(|sent| format!("{}: {:?}", sent.cmd.kind, sent.handle.progress()))
            .collect()
    }

    /// What was sent of a kind, as the machine got it.
    fn bodies(&self, kind: &str) -> Vec<Json> {
        lock(&self.sent)
            .values()
            .filter(|sent| sent.cmd.kind == kind)
            .map(|sent| sent.cmd.body.clone())
            .collect()
    }

    /// The result the machine answered a command with.
    fn result_of(&self, kind: &str) -> Option<Json> {
        lock(&self.sent)
            .values()
            .find(|sent| sent.cmd.kind == kind)
            .and_then(|sent| match sent.handle.progress() {
                Progress::Succeeded(result) => Some(result),
                _ => None,
            })
    }
}

#[async_trait]
impl Commands for SealingCommands {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        if lock(&self.sent).contains_key(&command.id) {
            return Ok(());
        }
        let (link, key) = lock(&self.machines)
            .get(&command.machine)
            .cloned()
            .ok_or(CommandsError::UnknownMachine)?;
        let mut body = command.body.clone();
        let mut wanted: Vec<Id> = Vec::new();
        let mut scan = body.clone();
        let Ok(()) = resolve_references(&mut scan, &mut |reference| {
            if let Reference::Secret(id) = reference {
                wanted.push(id);
            }
            Ok::<_, Infallible>(String::new())
        });
        let mut sealed: HashMap<Id, String> = HashMap::new();
        for id in wanted {
            let value = self
                .secrets
                .value(id)
                .await
                .map_err(|error| CommandsError::Other(error.to_string()))?
                .ok_or_else(|| CommandsError::Other("a secret is gone".to_owned()))?;
            let text = seal(&key, &info_for_command(command.id), &value)
                .map_err(|error| CommandsError::Other(error.to_string()))?;
            sealed.insert(id, text);
        }
        resolve_references(&mut body, &mut |reference| match reference {
            Reference::Secret(id) => sealed
                .get(&id)
                .cloned()
                .ok_or_else(|| "a secret was not sealed".to_owned()),
            Reference::Ticket(more) => {
                let wanted: TicketWanted = serde_json::from_value(Json::Object(more.clone()))
                    .map_err(|error| error.to_string())?;
                self.tickets
                    .for_request(&wanted, command.machine)
                    .map_err(|error| error.to_string())
            }
        })
        .map_err(CommandsError::Other)?;
        let now = self.clock.now();
        let deadline = command.deadline.unwrap_or_else(|| {
            Time::new(now + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())))
        });
        let cmd = Cmd {
            id: command.id,
            kind: command.kind.as_str().to_owned(),
            key: command.key.wire(),
            deadline,
            body,
        };
        lock(&self.order).push((command.machine, cmd.kind.clone()));
        let handle = link.commands().issue(cmd.clone());
        lock(&self.sent).insert(
            command.id,
            Sent {
                cmd,
                machine: command.machine,
                handle,
                at: now,
            },
        );
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
        let (machine, open) = match lock(&self.sent).get(&id) {
            Some(sent) => (Some(sent.machine), !sent.handle.progress().is_final()),
            None => (None, false),
        };
        if let (true, Some(machine)) = (open, machine)
            && let Some((link, _)) = lock(&self.machines).get(&machine).cloned()
        {
            link.commands().cancel(id);
        }
        Ok(open)
    }
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

/// Every file under `root` whose bytes hold `needle`.
fn files_holding(root: &Path, needle: &str) -> Vec<PathBuf> {
    fn walk(directory: &Path, needle: &[u8], found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                walk(&path, needle, found);
            } else if kind.is_file()
                && std::fs::read(&path)
                    .is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, needle.as_bytes(), &mut found);
    found
}

// ---------------------------------------------------------------- the descriptor

/// What the tests' harness declares of itself: it can run beside its tools and call a forwarder.
fn key_separated_agent() -> Json {
    let mut declared = descriptor();
    declared["arrangements"] = json!([
        { "type": "key_separated" },
        { "type": "together" }
    ]);
    declared
}

/// A recognizable rate, so that the credits of a row can be worked out by hand.
fn rate() -> Json {
    json!({ "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15" })
}

// ---------------------------------------------------------------- the cluster

/// The engine, the application and a database, a system on the loopback, a scripted model server, and a
/// connector that has enrolled (and is not running until it is told to).
struct Cluster {
    world: support::World,
    db: Db,
    clock: Arc<dyn Clock>,
    engine: Engine,
    commands: Arc<SealingCommands>,
    system: Arc<System>,
    upstream: MockLlm,
    master: MasterKey,
    binary: PathBuf,
    data: tempfile::TempDir,
    work: tempfile::TempDir,
    connector: Id,
}

/// The people and the tree of a test: one person owns it all.
struct Tree {
    owner: places::Who,
    topic: Id,
    org: Id,
}

/// The rows a thread runs on.
struct Resources {
    agent: Id,
    provider: Id,
    environment: Id,
}

/// Where the harness runs. The provider is on the connector in both: the system keeps its key and seals it
/// to the connector when it asks for the provider to be started.
enum Shape {
    /// The harness is on the connector: the scripted harness the program hosts does the work.
    OnTheConnector,
    /// The harness is on another machine, beside its tools, and reaches the connector's forwarder through a
    /// tunnel.
    HarnessElsewhere { harness: Id },
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

impl Cluster {
    /// Everything but the resources a thread runs on and the running of the program.
    async fn new() -> Self {
        // The application, its database, and an engine whose commands are sealed to the machines.
        let mut world = support::World::new().await;
        let old = world.app.app.clone();
        let db = old.db.clone();
        // The program keeps the time of the machine it runs on, so the engine, whose commands carry
        // deadlines the program reads, does too; the application's own clock is the world's and stands
        // still.
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let master = MasterKey::new([5; 32]);
        let commands = Arc::new(SealingCommands {
            machines: Mutex::new(HashMap::new()),
            secrets: StoredSecrets::new(db.clone(), master.clone()),
            tickets: Tickets::generate(Arc::clone(&clock)),
            sent: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
            clock: Arc::clone(&clock),
        });
        let engine = Engine::new(EngineParts {
            db: db.clone(),
            clock: Arc::clone(&clock),
            config: engine_config(),
            commands: commands.clone(),
            access: Arc::new(AuthzAccess::new(Arc::clone(&clock))),
            live: Arc::new(Discard),
        });
        engine.start();
        let state = Arc::new(AppState {
            host_ports: old.host_ports.clone(),
            builtin_sources: old.builtin_sources.clone(),
            db: db.clone(),
            config: Arc::clone(&old.config),
            clock: Arc::clone(&old.clock),
            link: Link::detached(db.clone(), Arc::clone(&old.clock), Arc::clone(&old.config))
                .expect("a link without gateways can be made"),
            engine: engine.clone(),
            realtime: orch_api_core::realtime::Realtime::new(&db, &old.clock, &old.config),
            build: BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs: orch_api_core::blobs::Blobs::in_memory(),
            cache: orch_api_core::cache::Cache::detached(),
        });
        state
            .link
            .set_applier(Arc::new(orch_api_core::thread_engine::EngineApplier::new(
                engine.clone(),
            )));
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        world.app = TestApp::over(state, world.app.clock.clone(), registry);

        // The scripted model server is the upstream of the provider, and insists on the key.
        let upstream = MockLlm::start(MockConfig::default().with_credential(KEY))
            .await
            .unwrap();

        // The system on the loopback; its relay believes the signatures of the tickets the engine's
        // commands carry.
        let (system, enrollment_port) = start_system().await;
        system.hub.set_ticket_verifier(commands.tickets.verifier());

        // The connector is enrolled with a token, as a person would, and told what its owner allows.
        let binary = connector_binary();
        let data = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("web")).unwrap();
        // This machine keeps a key as well as hosts a harness and its workspace: the role of a provider
        // is switched on in the configuration it enrolls with (a connector plays an agent and an
        // environment unless told more).
        std::fs::write(
            data.path().join("config.toml"),
            "[connector]\nroles = [\"agent\", \"environment\", \"provider\"]\n",
        )
        .unwrap();
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
            ("environment.roots", work.path().display().to_string()),
            // The fake harness is compiled into this connector. Its real binary is the installed
            // program listed by local policy and reported by the connector's own facts gathering.
            (
                "agent.harnesses",
                json!([{ "kind": "fake", "path": binary }]).to_string(),
            ),
            // The machine's own policy lists where model calls may be forwarded to: exactly the
            // scripted server.
            (
                "provider.upstreams",
                json!([upstream.responses_base_url()]).to_string(),
            ),
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
        assert!(words.iter().any(|role| role == "provider"), "{words:?}");
        machine_row(&db, arrival.machine, "keeper", &words).await;
        commands.arrived(
            arrival.machine,
            arrival.link.clone(),
            RecipientKey::from_certificate(&arrival.certificate),
        );
        spawn_applier(
            world.app.clone(),
            arrival.machine,
            arrival.link,
            arrival.notices.expect("the notices of the machine"),
        );
        Self {
            world,
            db,
            clock,
            engine,
            commands,
            system,
            upstream,
            master,
            binary,
            data,
            work,
            connector: arrival.machine,
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
        self.known_online(machine).await;
        self.reported(machine).await;
        child
    }

    async fn reported(&self, machine: Id) {
        eventually("the machine's own facts are committed", || async {
            sqlx::query_scalar("SELECT facts ? 'adapters' FROM machines WHERE id = $1")
                .bind(machine)
                .fetch_one(self.db.pool())
                .await
                .unwrap()
        })
        .await;
    }

    async fn known_online(&self, machine: Id) {
        eventually("the system knows the machine is online", || async {
            sqlx::query_scalar("SELECT online FROM machines WHERE id = $1")
                .bind(machine)
                .fetch_one(self.db.pool())
                .await
                .unwrap()
        })
        .await;
    }

    /// A person who owns an org with a project and a topic, made through HTTP.
    async fn tree(&self) -> Tree {
        let owner = admin(&self.world, "root").await;
        let org = make_org(&self.world, &owner, "Acme", "acme").await;
        let project = make_project(&self.world, &owner, org.base.id, "Web").await;
        let topic = make_topic(&self.world, &owner, project.base.id, "Login").await;
        Tree {
            owner,
            topic: topic.base.id,
            org: org.base.id,
        }
    }

    /// What the thread runs on, as rows: the requests that make a provider on a machine belong to other
    /// parts. The provider is an API provider on the key, sealed in the store as the requests seal it.
    async fn resources(&self, tree: &Tree, shape: &Shape) -> Resources {
        let (agent, provider, environment, template) = (new_id(), new_id(), new_id(), new_id());
        let owner = tree.owner.id();
        let (environment_machine, root_dir, colocated) = match shape {
            Shape::OnTheConnector => (
                self.connector,
                self.work.path().display().to_string(),
                json!({}),
            ),
            Shape::HarnessElsewhere { harness } => (
                *harness,
                "/workspace".to_owned(),
                json!({ "colocated_harnesses": ["fake"] }),
            ),
        };
        let mut conn = self.db.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
             VALUES ($1, 'Fake agent', $2, $3, $3, 'fake', $4, $5)",
        )
        .bind(agent)
        .bind(owner)
        .bind(tree.org)
        .bind(json!({ "harness_version": null, "config": {}, "env": {} }))
        .bind(key_separated_agent())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, upstreams, protocols,
                                    machine_id, key_machine_id, arrangements, status)
             VALUES ($1, 'Mock key', $2, $3, $3, 'api', $4, ARRAY['openai-responses'], $5, NULL,
                     ARRAY['separated','key_separated'], 'ready')",
        )
        .bind(provider)
        .bind(owner)
        .bind(tree.org)
        .bind(json!([{ "protocol": "openai-responses", "base_url": self.upstream.responses_base_url(), "options": {} }]))
        .bind(self.connector)
        .execute(&mut *conn)
        .await
        .unwrap();
        // The key is kept by the system, sealed in its store as the requests that make a provider seal it;
        // it leaves the store only sealed again, to the key of the machine that starts the provider.
        let sealed = self
            .master
            .seal(
                KEY.as_bytes(),
                format!("secrets/provider/{provider}/api_key").as_bytes(),
            )
            .unwrap();
        orch_db::queries::settings::put_secret(
            &mut conn,
            "provider",
            provider,
            "api_key",
            &sealed,
            None,
            Utc::now(),
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
             VALUES ($1, $2, $3, 'Mock Large', $4, true, 'manual', $5)",
        )
        .bind(new_id())
        .bind(provider)
        .bind(MODEL)
        .bind(json!({ "input_images": true, "rate": rate() }))
        .bind(owner)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'template', 'test', $2)",
        )
        .bind(template)
        .bind(colocated)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id,
                                       template_id, generation, status)
             VALUES ($1, 'Workspace', $2, $3, 'managed', $4, $5, $6, $7, 1, 'running')",
        )
        .bind(environment)
        .bind(owner)
        .bind(tree.org)
        .bind(tree.topic)
        .bind(root_dir)
        .bind(environment_machine)
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
        for (kind, id) in [
            ("agent", agent),
            ("provider", provider),
            ("environment", environment),
        ] {
            orch_db::queries::access::insert_entry(
                &mut conn,
                &orch_db::queries::access::NewEntry {
                    id: new_id(),
                    page: &orch_db::queries::access::PageKey::new(kind, id, "live"),
                    profile: "any",
                    permission: "access.own",
                    holder_kind: "user",
                    holder_id: Some(owner),
                    holder_role: None,
                    options: &json!({}),
                    expires_at: None,
                    note: "",
                    source: "creation",
                    created_by: Some(owner),
                    at: Utc::now(),
                },
            )
            .await
            .unwrap();
        }
        Resources {
            agent,
            provider,
            environment,
        }
    }

    /// A thread is made over HTTP and told something; the answer is read back over HTTP when it has come.
    async fn converse(&self, tree: &Tree, on: &Resources, say: &str) -> (Id, Vec<String>) {
        let thread = self
            .world
            .app
            .call(ThreadCreate, &[&tree.topic])
            .token(&tree.owner.token)
            .body(&CreateThread {
                name: Some("Through a real connector".to_owned()),
                binding: Some(CreateThreadBinding {
                    agent_id: on.agent,
                    provider_id: on.provider,
                    arrangement: None,
                }),
                model: Some(MODEL.to_owned()),
                environment: Some(EnvironmentChoice::Use(on.environment)),
                directory: Some("web".to_owned()),
                ..CreateThread::default()
            })
            .send()
            .await
            .ok();
        let id = thread.base.id;
        assert_eq!(thread.state, ThreadState::Idle);
        let sent = self
            .world
            .app
            .call(ThreadInputCreate, &[&id])
            .token(&tree.owner.token)
            .body(&ThreadInputCreateBody {
                plan: false,
                content: vec![ContentPart::Text {
                    text: say.to_owned(),
                }],
                delivery: Delivery::Queue,
                client_id: "first".to_owned(),
                bypass: None,
                native_action: None,
            })
            .send()
            .await;
        assert_eq!(sent.status, 201);
        // If the reply does not come, what the system and the program had to say is told, so that the
        // failure can be read.
        let watch = {
            let (db, commands, log) = (self.db.clone(), Arc::clone(&self.commands), self.log());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(20)).await;
                let state: (String, Option<String>, Option<String>, Option<String>) =
                    sqlx::query_as(
                        "SELECT state, blocked_reason, blocked_detail, starting_step FROM threads WHERE node_id = $1",
                    )
                    .bind(id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
                eprintln!(
                    "STILL WAITING after 20 s\nthread: {state:?}\ncommands: {:#?}\nconnector log:\n{}",
                    commands.progress(),
                    std::fs::read_to_string(log).unwrap_or_default()
                );
            })
        };
        let heard: Arc<Mutex<Vec<String>>> = Arc::default();
        eventually("the reply is read back", || {
            let (app, token, heard) = (
                &self.world.app,
                tree.owner.token.clone(),
                Arc::clone(&heard),
            );
            async move {
                let synced = app
                    .call(ThreadSyncGet, &[&id])
                    .token(&token)
                    .query(&ThreadSyncGetQuery::default())
                    .send()
                    .await
                    .ok();
                let found = said(&synced.items);
                let done = synced.thread.state == ThreadState::Idle && found.len() == 2;
                *lock(&heard) = found;
                done
            }
        })
        .await;
        watch.abort();
        let reply = lock(&heard).clone();
        (id, reply)
    }

    /// Lease completion and end observation use separate serial keys; either may finish last.
    async fn wait_for_lease_close(&self) {
        eventually("the lease is closed", || {
            let closed = self.commands.result_of("lease.close").is_some();
            async move { closed }
        })
        .await;
        self.wait_for_file_commands().await;
    }

    async fn wait_for_file_commands(&self) {
        eventually(
            "the turn's recording and end observation are issued",
            || {
                let kinds = self.commands.kinds();
                async move {
                    ["fs.record", "workspace.observe"]
                        .iter()
                        .all(|wanted| kinds.iter().any(|(_, kind)| kind == wanted))
                }
            },
        )
        .await;
    }

    /// Assert the exact new bookkeeping independently of the existing provider/session order.
    fn turn_commands(&self, environment: Id, machine: Id) -> Vec<(Id, String)> {
        let kinds = self.commands.kinds();
        let positions = |wanted: &str| {
            kinds
                .iter()
                .enumerate()
                .filter(|(_, (_, kind))| kind == wanted)
                .map(|(at, _)| at)
                .collect::<Vec<_>>()
        };
        let record = positions("fs.record");
        let observe = positions("workspace.observe");
        let turn = positions("turn.start");
        assert_eq!((record.len(), observe.len(), turn.len()), (1, 1, 1));
        assert!(record[0] < turn[0] && turn[0] < observe[0]);
        assert!(
            positions("lease.open").into_iter().all(|at| record[0] < at),
            "recording precedes every lease"
        );
        for (kind, at) in [("fs.record", record[0]), ("workspace.observe", observe[0])] {
            assert_eq!(
                kinds[at].0, machine,
                "file work goes to the environment's machine"
            );
            assert_eq!(
                self.commands.bodies(kind)[0]["environment_id"],
                json!(environment)
            );
        }
        kinds
            .into_iter()
            .filter(|(_, kind)| kind != "fs.record" && kind != "workspace.observe")
            .collect()
    }

    /// The key is where it belongs and nowhere else: not in a file of the connector, not in its log, not in
    /// a row of the system.
    async fn the_key_is_nowhere_in_the_clear(&self) {
        assert!(
            files_holding(self.data.path(), KEY).is_empty(),
            "the connector keeps the key sealed, never in the clear"
        );
        assert!(
            !std::fs::read_to_string(self.log())
                .unwrap_or_default()
                .contains(KEY)
        );
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT table_name::text FROM information_schema.tables
              WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY 1",
        )
        .fetch_all(self.db.pool())
        .await
        .unwrap();
        for table in tables {
            let sql =
                format!("SELECT count(*) FROM \"{table}\" t WHERE t::text LIKE '%' || $1 || '%'");
            let hits: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(KEY)
                .fetch_one(self.db.pool())
                .await
                .unwrap();
            assert_eq!(hits, 0, "the key is in the clear in {table}");
        }
    }

    /// A machine of the scripted kind, on the same hub as the connector, which plays a harness: asked to
    /// start a turn, it calls the model through the forwarder of the machine that has the key, through an
    /// `llm` tunnel and with the ticket of the lease, and says what the model answered.
    async fn add_harness_machine(&self) -> Harness {
        let id = new_id();
        let roles = vec![Role::Agent, Role::Environment];
        let identity = self.system.pki.machine(id);
        let (link, notices) = self.system.hub.add_machine(
            id,
            roles.clone(),
            BTreeMap::new(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        machine_row(&self.db, id, "harness", &role_words(&roles)).await;
        self.commands.arrived(
            id,
            link.clone(),
            RecipientKey::from_certificate(&identity.certificate),
        );
        spawn_applier(self.world.app.clone(), id, link, notices);
        let machine = ScriptedMachine::start(
            &self.system.hub,
            &self.clock,
            &identity,
            &MachineSpec {
                roles,
                ..MachineSpec::default()
            },
        );
        let told: Arc<Mutex<Vec<String>>> = Arc::default();
        let session: Arc<Mutex<Option<(Id, Id, String)>>> = Arc::default();
        let link = machine.link.clone();
        machine.handler.on(CommandKind::AgentStart, {
            let told = Arc::clone(&told);
            move |command| {
                lock(&told).push(serde_json::to_string(command).unwrap());
                Ok(CommandResult::AgentStart(AgentStartResult {
                    reused: false,
                    harness_version: "1.0.0".to_owned(),
                    descriptor: serde_json::from_value(key_separated_agent()).unwrap(),
                }))
            }
        });
        machine.handler.on(CommandKind::SessionOpen, {
            let (told, session) = (Arc::clone(&told), Arc::clone(&session));
            move |command| {
                lock(&told).push(serde_json::to_string(command).unwrap());
                let LinkCommand::SessionOpen(open) = command else {
                    unreachable!()
                };
                let SessionOpenProvider::Forwarded {
                    ticket: TicketSlot::Signed(ticket),
                    ..
                } = &open.provider
                else {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "this harness reaches its model through the forwarder",
                    ));
                };
                *lock(&session) = Some((open.thread_id, open.segment_id, ticket.clone()));
                Ok(CommandResult::SessionOpen(SessionOpenResult {
                    native_session_ref: format!("native-{}", open.segment_id),
                    resumed: false,
                    import_report: None,
                    instruction_sources: Vec::new(),
                    approval_reviewer: None,
                }))
            }
        });
        machine.handler.on(CommandKind::TurnStart, {
            let (told, session) = (Arc::clone(&told), Arc::clone(&session));
            move |command| {
                lock(&told).push(serde_json::to_string(command).unwrap());
                let LinkCommand::TurnStart(turn) = command else {
                    unreachable!()
                };
                let session = lock(&session).clone().expect("a session is open");
                tokio::spawn(harness_turn(link.clone(), session, turn.clone()));
                Ok(CommandResult::TurnStart(Empty {}))
            }
        });
        let hub = self.system.hub.clone();
        eventually("the harness machine connects", || {
            let up = hub.session_of(id).is_some();
            async move { up }
        })
        .await;
        self.known_online(id).await;
        // This machine runs its fake harness in the scripted handler above. It reports that
        // installation itself over its durable machine stream, before any thread uses it.
        let handler_program = std::env::current_exe().unwrap().display().to_string();
        machine
            .link
            .publish(
                &Stream::Machine,
                &Event::MachineFacts(MachineFacts {
                    role_management: None,
                    programs: None,
                    installation: None,
                    provider_runtime_descriptor: None,
                    build_fetch: None,
                    storages: Vec::new(),
                    environment_descriptor: None,
                    hostname: "harness".to_owned(),
                    os_version: "scripted Linux".to_owned(),
                    cpus: 1,
                    memory_bytes: 0,
                    harnesses: vec![MachineFactsHarness {
                        kind: "fake".to_owned(),
                        path: handler_program.clone(),
                        version: "1.0.0".to_owned(),
                        daemon: false,
                    }],
                    adapters: vec![MachineFactsAdapter {
                        needs_binary: Some(false),
                        kind: "fake".to_owned(),
                        adapter_version: 1,
                        builtin: true,
                        default_home_owned: true,
                        manifest: None,
                    }],
                    plugins: vec![],
                    policy: Some(LocalPolicy {
                        agent_harnesses: vec![PolicyHarness {
                            kind: "fake".to_owned(),
                            path: handler_program,
                        }],
                        ..LocalPolicy::default()
                    }),
                    rates: MachineFactsRates {
                        up_bps: 0,
                        down_bps: 0,
                        round_trip_ms: 0,
                    },
                    clock_offset_ms: 0,
                }),
            )
            .await
            .unwrap();
        self.reported(id).await;
        Harness {
            id,
            told,
            _machine: machine,
        }
    }
}

/// A scripted machine that plays a harness.
struct Harness {
    id: Id,
    /// Every command it was sent, as text.
    told: Arc<Mutex<Vec<String>>>,
    _machine: ScriptedMachine,
}

/// The turn of the scripted harness: it calls the model through the tunnel its ticket opens, which has
/// no key in it and needs none, and says what the model answered.
async fn harness_turn(link: MachineLink, session: (Id, Id, String), turn: TurnStart) {
    let (thread_id, segment_id, ticket) = session;
    let stream = Stream::Session {
        thread_id,
        segment_id,
    };
    link.publish(&stream, &rig::turn_began(turn.turn_id))
        .await
        .unwrap();
    let said = match model_call(&link, &ticket, &turn).await {
        Ok(text) => text,
        Err(refused) => format!("The model could not be reached: {refused}"),
    };
    for event in [
        rig::message_began(turn.turn_id, "m1"),
        rig::message_done("m1", &said),
        rig::turn_ended(turn.turn_id, TurnOutcome::Completed),
    ] {
        link.publish(&stream, &event).await.unwrap();
    }
}

/// One request of the Responses format through an `llm` tunnel, and the text of the answer.
async fn model_call(link: &MachineLink, ticket: &str, turn: &TurnStart) -> Result<String, String> {
    let tunnel = link
        .open_tunnel(ticket)
        .await
        .map_err(|error| format!("the tunnel was refused: {error:?}"))?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(tunnel))
            .await
            .map_err(|error| error.to_string())?;
    tokio::spawn(connection);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/responses")
        .header("host", "forwarder")
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(
            json!({ "model": turn.model, "input": turn.text }).to_string(),
        )))
        .map_err(|error| error.to_string())?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|error| error.to_string())?
        .to_bytes();
    let reply: Json = serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    if status != StatusCode::OK {
        return Err(format!("{status}: {reply}"));
    }
    reply["output"][0]["content"][0]["text"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("no text in {reply}"))
}

// ---------------------------------------------------------------- the tests

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_made_over_http_runs_its_turn_on_a_real_connector_that_was_given_the_key_sealed() {
    let began = Instant::now();
    let step = |what: &str| eprintln!("[{:>6.2} s] {what}", began.elapsed().as_secs_f64());
    let cluster = Cluster::new().await;
    step("the connector is enrolled");
    let tree = cluster.tree().await;
    let on = cluster.resources(&tree, &Shape::OnTheConnector).await;
    let mut child = cluster.run_connector().await;
    step("the program holds the session");

    let (_, reply) = cluster.converse(&tree, &on, "Say it plainly").await;
    assert_eq!(
        reply,
        ["user: Say it plainly", "agent: You said: Say it plainly"]
    );
    step("the reply is read back");

    // What the machine was asked, in this order, and which of it carried the key.
    cluster.wait_for_lease_close().await;
    let kinds: Vec<String> = cluster
        .turn_commands(on.environment, cluster.connector)
        .into_iter()
        .map(|(machine, kind)| {
            assert_eq!(machine, cluster.connector);
            kind
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "provider.start",
            "fs.mkdir",
            "agent.start",
            "session.open",
            "lease.open",
            "turn.start",
            "lease.close"
        ]
    );
    let started = &cluster.commands.bodies("provider.start")[0];
    assert!(
        started["credential"]["$sealed"].is_string(),
        "the key went sealed to the machine's key: {started}"
    );
    assert!(!started.to_string().contains(KEY));
    let closed = cluster.commands.result_of("lease.close").unwrap();
    assert_eq!(
        closed["requests"], 0,
        "the scripted harness made no model call"
    );

    // The provider role opened the key: asked for the models of the provider, it goes to the upstream with
    // the key, which the scripted model server accepts. (While a harness of the machine can hold the
    // provider's sign-in, the harness answers that question; the scripted harness is stopped first, so
    // that the provider role does.)
    let stopped = cluster
        .ask(
            SerialKey::Harness {
                provider_id: on.provider,
                agent_id: on.agent,
            },
            &AgentStop {
                provider_id: on.provider,
                agent_id: on.agent,
                logout: None,
            },
        )
        .await;
    assert!(matches!(stopped, Outcome::Succeeded(_)), "{stopped:?}");
    let Outcome::Succeeded(found) = cluster
        .ask(
            SerialKey::Provider(on.provider),
            &ProviderDiscoverModels {
                provider_id: on.provider,
                models: None,
            },
        )
        .await
    else {
        panic!("the provider did not list its models");
    };
    let listed: Vec<String> = found["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|suggestion| suggestion["model_name"].as_str().unwrap().to_owned())
        .collect();
    assert!(listed.contains(&MODEL.to_owned()), "{listed:?}");
    let requests = cluster.upstream.requests();
    assert!(!requests.is_empty());
    assert!(
        requests
            .iter()
            .all(|request| request.credential.verdict == Verdict::Accepted),
        "the key the connector opened is the one the upstream insists on: {requests:?}"
    );
    cluster.the_key_is_nowhere_in_the_clear().await;

    // The program stops when it is told to.
    let status = terminate(&mut child).await;
    assert!(status.success(), "{status:?}");
    cluster.engine.stop();
    step("done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_call_of_a_harness_on_another_machine_passes_the_forwarder_of_a_real_connector_and_is_one_row_of_the_ledger()
 {
    let began = Instant::now();
    let step = |what: &str| eprintln!("[{:>6.2} s] {what}", began.elapsed().as_secs_f64());
    let cluster = Cluster::new().await;
    let tree = cluster.tree().await;
    let mut child = cluster.run_connector().await;
    let harness = cluster.add_harness_machine().await;
    let on = cluster
        .resources(
            &tree,
            &Shape::HarnessElsewhere {
                harness: harness.id,
            },
        )
        .await;
    step("the provider is on the connector, the harness is elsewhere");

    // A thread is made over HTTP, one input is sent, and the reply is the model's, read back over HTTP.
    let (thread, reply) = cluster.converse(&tree, &on, "Say it plainly").await;
    let starts = cluster.commands.bodies("turn.start");
    let composed = starts[0]["text"].as_str().unwrap();
    assert!(composed.starts_with("<orch>\n"));
    assert_eq!(
        composed.split_once("</orch>\n\n").unwrap().1,
        "Say it plainly"
    );
    assert_eq!(
        reply,
        [
            "user: Say it plainly".to_owned(),
            format!("agent: You said: {composed}")
        ],
        "the model echoes the exact composed input through the forwarder"
    );
    cluster.wait_for_lease_close().await;
    step("the reply is read back");

    // Each command went to the machine it is for, in this order: the key's machine starts the provider and
    // holds the lease, the harness's machine makes the directory of the thread and has the session and the turn.
    let keeper = cluster.connector;
    let kinds: Vec<(bool, String)> = cluster
        .turn_commands(on.environment, harness.id)
        .into_iter()
        .map(|(machine, kind)| (machine == keeper, kind))
        .collect();
    assert_eq!(
        kinds,
        [
            (true, "provider.start".to_owned()),
            (false, "fs.mkdir".to_owned()),
            (false, "agent.start".to_owned()),
            (false, "session.open".to_owned()),
            (true, "lease.open".to_owned()),
            (false, "turn.start".to_owned()),
            (true, "lease.close".to_owned()),
        ]
    );
    let started = &cluster.commands.bodies("provider.start")[0];
    assert!(
        started["credential"]["$sealed"].is_string(),
        "the key went to the connector sealed to its key: {started}"
    );
    assert!(!started.to_string().contains(KEY));

    // The model was called once, with the key the connector has and the harness's machine never had.
    let requests = cluster.upstream.requests();
    let calls: Vec<_> = requests
        .iter()
        .filter(|request| request.method == "POST")
        .collect();
    assert_eq!(calls.len(), 1, "{requests:?}");
    assert_eq!(calls[0].credential.verdict, Verdict::Accepted);
    assert_eq!(calls[0].model.as_deref(), Some(MODEL));
    for told in lock(&harness.told).iter() {
        assert!(
            !told.contains(KEY),
            "the harness's machine was sent the key: {told}"
        );
    }
    assert_eq!(
        lock(&harness.told).len(),
        3,
        "agent.start, session.open and turn.start"
    );
    cluster.the_key_is_nowhere_in_the_clear().await;

    // The lease counted one request, and the ledger has one row for it, with the rate and the credits.
    let closed = cluster.commands.result_of("lease.close").unwrap();
    assert_eq!(closed["requests"], 1);
    let turn: Id = sqlx::query_scalar("SELECT id FROM turns WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(cluster.db.pool())
        .await
        .unwrap();
    let mut rows = Vec::new();
    for _ in 0..200 {
        let mut conn = cluster.db.acquire().await.unwrap();
        rows = orch_db::usage::ledger::events_of_turn(&mut conn, turn)
            .await
            .unwrap();
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The lease's close records its cut once, a zero remainder too: the model call is the one row
    // that names the upstream's request.
    let cut =
        |row: &&orch_db::usage::ledger::EventRow| row.request_ref.as_deref() == Some("lease.close");
    assert!(
        rows.iter().filter(cut).all(|row| row.total_tokens == 0),
        "{rows:?}"
    );
    let calls: Vec<_> = rows.iter().filter(|row| !cut(row)).collect();
    assert_eq!(calls.len(), 1, "{rows:?}");
    let row = calls[0];
    assert!(row.counted);
    assert_eq!(
        (row.origin.as_str(), row.scope.as_str()),
        ("forwarder", "request")
    );
    assert_eq!(row.model, MODEL);
    assert_eq!(row.provider_id, on.provider);
    assert_eq!(row.thread_id, Some(thread));
    assert_eq!(row.user_id, Some(tree.owner.id()));
    assert_eq!(row.arrangement.as_deref(), Some("key_separated"));
    assert_eq!(row.environment_id, Some(on.environment));
    assert!(row.input_tokens > 0 && row.output_tokens > 0, "{row:?}");
    assert_eq!(row.total_tokens, row.input_tokens + row.output_tokens);
    assert_eq!(row.rate_source.as_deref(), Some("provider"));
    assert_eq!(row.rate, Some(rate()));
    // Worked out by hand: the input at 2.5 and the output at 15 for a million, to six places, a half going up.
    let doubled = row.input_tokens * 5 + row.output_tokens * 30;
    let micro = (doubled + 1) / 2;
    let expected = format!("{}.{:06}", micro / 1_000_000, micro % 1_000_000);
    assert_eq!(row.credits.as_deref(), Some(expected.as_str()));
    let usage: Json = sqlx::query_scalar("SELECT usage FROM turns WHERE id = $1")
        .bind(turn)
        .fetch_one(cluster.db.pool())
        .await
        .unwrap();
    assert_eq!(usage["total_tokens"], row.total_tokens);

    let status = terminate(&mut child).await;
    assert!(status.success(), "{status:?}");
    cluster.engine.stop();
    step("done");
}

impl Cluster {
    /// Sends the connector a command of its own, as the engine would, and waits for its end.
    async fn ask<B: orch_link_contract::commands::CommandBody>(
        &self,
        key: SerialKey,
        body: &B,
    ) -> Outcome {
        let command = Outgoing::new(new_id(), self.connector, key, body).unwrap();
        let id = command.id;
        self.commands.issue(command).await.unwrap();
        tokio::time::timeout(PATIENCE, self.commands.wait(id))
            .await
            .unwrap()
            .unwrap()
    }
}

/// What the API does with a machine's events: applies them in a transaction that moves the stream's cursor,
/// and tells the link they are committed (or that they are not).
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

/// What the system's link writes when a machine's session comes or goes: the presence and the moment of it,
/// by which one session is told from the next.
async fn set_online(pool: &Db, machine: Id, online: bool) {
    let mut conn = pool.acquire().await.unwrap();
    orch_db::queries::resources::set_presence(&mut conn, machine, online, None, Utc::now())
        .await
        .unwrap();
}

// ---------------------------------------------------------------- one machine for everything; a key typed on the machine

impl Cluster {
    /// Runs the program to its end with `input` on its standard input.
    async fn succeeds_with_input(&self, mut command: Command, input: &str) -> String {
        use tokio::io::AsyncWriteExt;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).await.unwrap();
        drop(stdin);
        let output = child.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// The rows of a harness that never ran, a subscription whose sign-in the harness holds on the connector,
    /// and an environment on the connector too: harness, sign-in and tools are on one machine.
    async fn everything_on_the_connector(&self, tree: &Tree) -> Resources {
        let (agent, provider, environment, template) = (new_id(), new_id(), new_id(), new_id());
        let owner = tree.owner.id();
        let mut conn = self.db.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
             VALUES ($1, 'Fake agent', $2, $3, $3, 'fake', $4, '{}')",
        )
        .bind(agent)
        .bind(owner)
        .bind(tree.org)
        .bind(json!({ "harness_version": null, "config": {}, "env": {} }))
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                    machine_id, arrangements, status)
             VALUES ($1, 'The login here', $2, $3, $3, 'subscription', 'fake', $4, $5,
                     ARRAY['separated'], 'ready')",
        )
        .bind(provider)
        .bind(owner)
        .bind(tree.org)
        .bind(agent)
        .bind(self.connector)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
             VALUES ($1, $2, $3, 'Mock Large', $4, true, 'manual', $5)",
        )
        .bind(new_id())
        .bind(provider)
        .bind(MODEL)
        .bind(json!({ "input_images": true }))
        .bind(owner)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'template', 'test', '{}')")
            .bind(template)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id,
                                       template_id, generation, status)
             VALUES ($1, 'Workspace', $2, $3, 'managed', $4, $5, $6, $7, 1, 'running')",
        )
        .bind(environment)
        .bind(owner)
        .bind(tree.org)
        .bind(tree.topic)
        .bind(self.work.path().display().to_string())
        .bind(self.connector)
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
        for (kind, id) in [
            ("agent", agent),
            ("provider", provider),
            ("environment", environment),
        ] {
            orch_db::queries::access::insert_entry(
                &mut conn,
                &orch_db::queries::access::NewEntry {
                    id: new_id(),
                    page: &orch_db::queries::access::PageKey::new(kind, id, "live"),
                    profile: "any",
                    permission: "access.own",
                    holder_kind: "user",
                    holder_id: Some(owner),
                    holder_role: None,
                    options: &json!({}),
                    expires_at: None,
                    note: "",
                    source: "creation",
                    created_by: Some(owner),
                    at: Utc::now(),
                },
            )
            .await
            .unwrap();
        }
        Resources {
            agent,
            provider,
            environment,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_on_a_harness_that_never_ran_is_made_and_its_turn_needs_no_key_and_no_lease_on_a_real_connector_that_holds_everything()
 {
    let cluster = Cluster::new().await;
    let tree = cluster.tree().await;
    let on = cluster.everything_on_the_connector(&tree).await;
    let mut child = cluster.run_connector().await;

    // The profile says nothing of its harness; making the thread has the program start the harness once.
    let script = "#!fake\nsay Counted\nusage 1200 80";
    let (thread, reply) = cluster.converse(&tree, &on, script).await;
    assert_eq!(
        reply,
        [format!("user: {script}"), "agent: Counted".to_owned()]
    );
    let declared: Json = sqlx::query_scalar("SELECT descriptor FROM agents WHERE id = $1")
        .bind(on.agent)
        .fetch_one(cluster.db.pool())
        .await
        .unwrap();
    assert_eq!(
        declared["kind"], "fake",
        "the program's own adapter said what it is"
    );
    let arrangement: String =
        sqlx::query_scalar("SELECT arrangement FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(cluster.db.pool())
            .await
            .unwrap();
    assert_eq!(arrangement, "together");

    // The machine was asked the harness's start (once: the turn found it up), the directory of the thread, its
    // session on its own sign-in, and the turn; no provider was started, no lease was opened, no key went anywhere.
    cluster.wait_for_file_commands().await;
    let kinds: Vec<String> = cluster
        .turn_commands(on.environment, cluster.connector)
        .into_iter()
        .map(|(machine, kind)| {
            assert_eq!(machine, cluster.connector);
            kind
        })
        .collect();
    assert_eq!(
        kinds,
        ["agent.start", "fs.mkdir", "session.open", "turn.start"]
    );
    let leases: i64 = sqlx::query_scalar("SELECT count(*) FROM leases WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(cluster.db.pool())
        .await
        .unwrap();
    assert_eq!(leases, 0);

    // What the harness reported of the turn is what the ledger counts.
    let turn: Id = sqlx::query_scalar("SELECT id FROM turns WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(cluster.db.pool())
        .await
        .unwrap();
    let mut rows = Vec::new();
    for _ in 0..200 {
        let mut conn = cluster.db.acquire().await.unwrap();
        rows = orch_db::usage::ledger::events_of_turn(&mut conn, turn)
            .await
            .unwrap();
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(rows.len(), 1, "{rows:?}");
    // The harness reported the turn's request, and a request it reports replaces the turn's
    // cumulative fallback.
    assert_eq!(
        (
            rows[0].origin.as_str(),
            rows[0].scope.as_str(),
            rows[0].counted
        ),
        ("harness", "request", true)
    );
    assert_eq!(rows[0].provider_id, on.provider);
    assert_eq!(rows[0].arrangement.as_deref(), Some("together"));
    assert_eq!(
        (
            rows[0].input_tokens,
            rows[0].output_tokens,
            rows[0].total_tokens
        ),
        (1_200, 80, 1_280),
        "the totals the scripted harness reported"
    );

    let status = terminate(&mut child).await;
    assert!(status.success(), "{status:?}");
    cluster.engine.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_typed_on_the_machine_serves_a_turn_without_the_system_ever_holding_it_or_sending_it()
{
    let cluster = Cluster::new().await;
    let tree = cluster.tree().await;
    let on = cluster.resources(&tree, &Shape::OnTheConnector).await;
    // The key is moved from the system's store to the machine: the provider is marked as keeping it there,
    // the system forgets its copy, and the person types the key into the program.
    sqlx::query("UPDATE providers SET key_machine_id = $2 WHERE id = $1")
        .bind(on.provider)
        .bind(cluster.connector)
        .execute(cluster.db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1")
        .bind(on.provider)
        .execute(cluster.db.pool())
        .await
        .unwrap();
    let said = cluster
        .succeeds_with_input(
            {
                let mut set = program(&cluster.binary, cluster.data.path());
                set.args([
                    "provider",
                    "set-key",
                    "--provider",
                    &on.provider.to_string(),
                ]);
                set
            },
            &format!("{KEY}\n"),
        )
        .await;
    assert!(!said.contains(KEY) && said.contains("not sent"), "{said}");
    let mut child = cluster.run_connector().await;

    let (_, reply) = cluster.converse(&tree, &on, "Say it plainly").await;
    assert_eq!(
        reply,
        ["user: Say it plainly", "agent: You said: Say it plainly"]
    );
    cluster.wait_for_lease_close().await;
    let kinds: Vec<String> = cluster
        .turn_commands(on.environment, cluster.connector)
        .into_iter()
        .map(|(_, kind)| kind)
        .collect();
    assert_eq!(
        kinds,
        [
            "provider.start",
            "fs.mkdir",
            "agent.start",
            "session.open",
            "lease.open",
            "turn.start",
            "lease.close"
        ]
    );
    let started = &cluster.commands.bodies("provider.start")[0];
    assert!(
        started["credential"].is_null(),
        "the key is on the machine already, and none is sent: {started}"
    );

    // The provider role opens the key from its store: asked for the models, it goes to the upstream with it,
    // which accepts only that key. (The scripted harness is stopped first, as it would answer instead.)
    let stopped = cluster
        .ask(
            SerialKey::Harness {
                provider_id: on.provider,
                agent_id: on.agent,
            },
            &AgentStop {
                provider_id: on.provider,
                agent_id: on.agent,
                logout: None,
            },
        )
        .await;
    assert!(matches!(stopped, Outcome::Succeeded(_)), "{stopped:?}");
    let Outcome::Succeeded(found) = cluster
        .ask(
            SerialKey::Provider(on.provider),
            &ProviderDiscoverModels {
                provider_id: on.provider,
                models: None,
            },
        )
        .await
    else {
        panic!("the provider did not list its models");
    };
    assert!(
        found["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|suggestion| suggestion["model_name"] == MODEL),
        "{found}"
    );
    let requests = cluster.upstream.requests();
    assert!(!requests.is_empty());
    assert!(
        requests
            .iter()
            .all(|request| request.credential.verdict == Verdict::Accepted),
        "the key the program opened is the one the upstream insists on: {requests:?}"
    );
    cluster.the_key_is_nowhere_in_the_clear().await;

    let status = terminate(&mut child).await;
    assert!(status.success(), "{status:?}");
    cluster.engine.stop();
}

// ---------------------------------------------------------------- a directory of the connector's machine

impl Cluster {
    /// A directory of the connector's machine, registered for the owner as an environment of theirs.
    async fn directory(&self, tree: &Tree) -> Id {
        use orch_db::queries::environments::routes::{NewConnected, insert_connected};
        let (id, owner) = (new_id(), tree.owner.id());
        let root = self.work.path().display().to_string();
        let mut conn = self.db.acquire().await.unwrap();
        let made = insert_connected(
            &mut conn,
            &NewConnected {
                id,
                name: "Laptop",
                owner_id: owner,
                machine_id: self.connector,
                home_node_id: tree.topic,
                workspace_root: &root,
                remote_write: Some(false),
                terminal: Some(false),
            },
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(made, "the topic is there");
        orch_db::queries::access::insert_entry(
            &mut conn,
            &orch_db::queries::access::NewEntry {
                id: new_id(),
                page: &orch_db::queries::access::PageKey::new("environment", id, "live"),
                profile: "any",
                permission: "access.own",
                holder_kind: "user",
                holder_id: Some(owner),
                holder_role: None,
                options: &json!({}),
                expires_at: None,
                note: "",
                source: "creation",
                created_by: Some(owner),
                at: Utc::now(),
            },
        )
        .await
        .unwrap();
        id
    }

    /// Where the system holds an environment: its status, what its connector said it is, and its root.
    async fn standing(&self, environment: Id) -> (String, Option<String>, String) {
        sqlx::query_as(
            "SELECT status, facts ->> 'os', workspace_root FROM environments WHERE id = $1",
        )
        .bind(environment)
        .fetch_one(self.db.pool())
        .await
        .unwrap()
    }

    async fn wait_for_status(&self, environment: Id, wanted: &str) {
        eventually(&format!("the directory to be {wanted}"), || async {
            self.standing(environment).await.0 == wanted
        })
        .await;
    }

    /// How many times the connector was asked for `kind`.
    fn asked(&self, kind: &str) -> usize {
        self.commands
            .kinds()
            .iter()
            .filter(|(_, each)| each == kind)
            .count()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_directory_of_a_real_connector_is_attached_in_each_session_and_a_turn_that_needed_it_goes_on_when_it_is_up()
 {
    let began = Instant::now();
    let step = |what: &str| eprintln!("[{:>6.2} s] {what}", began.elapsed().as_secs_f64());
    let cluster = Cluster::new().await;
    // An offline machine remains known by its last report. Let this connector report its actual
    // installation before it goes away; the first binding then has the facts a real server keeps.
    let mut first = cluster.run_connector().await;
    assert!(terminate(&mut first).await.success());
    eventually("the reported machine goes offline", || async {
        !sqlx::query_scalar::<_, bool>("SELECT online FROM machines WHERE id = $1")
            .bind(cluster.connector)
            .fetch_one(cluster.db.pool())
            .await
            .unwrap()
    })
    .await;
    let tree = cluster.tree().await;
    let resources = cluster.resources(&tree, &Shape::OnTheConnector).await;
    let directory = cluster.directory(&tree).await;
    let on = Resources {
        environment: directory,
        ..resources
    };
    assert_eq!(
        cluster.standing(directory).await.0,
        "none",
        "as registering leaves it"
    );
    step("the directory is registered, and the program is not running");

    // The thread is made and told something while the machine is away. The program starts a little later:
    // the system attaches the directory to it by itself, and the turn that waited for that goes on.
    let later = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        cluster.run_connector().await
    };
    let ((_, reply), mut child) =
        tokio::join!(cluster.converse(&tree, &on, "Say it plainly"), later);
    assert_eq!(
        reply,
        ["user: Say it plainly", "agent: You said: Say it plainly"]
    );
    step("the reply is read back");
    let (status, os, root) = cluster.standing(directory).await;
    assert_eq!(status, "running");
    assert_eq!(os.as_deref(), Some("linux"), "what the program said it is");
    assert_eq!(
        root,
        cluster.work.path().display().to_string(),
        "the root it was registered with"
    );
    let kinds: Vec<String> = cluster
        .commands
        .kinds()
        .into_iter()
        .map(|(_, kind)| kind)
        .collect();
    let attached = kinds
        .iter()
        .position(|kind| kind == "env.attach")
        .expect("the directory was attached");
    let started = kinds
        .iter()
        .position(|kind| kind == "turn.start")
        .expect("the turn was started");
    assert!(
        attached < started,
        "the turn waited for the attach: {kinds:?}"
    );
    assert_eq!(cluster.asked("env.attach"), 1);

    // The program stops: the directory is offline.
    let status = terminate(&mut child).await;
    assert!(status.success(), "{status:?}");
    cluster.wait_for_status(directory, "offline").await;
    step("the directory is offline with its machine");

    // A new process keeps nothing of the last: the directory is attached in it again, which the program
    // shows by listing it.
    let mut again = cluster.run_connector().await;
    cluster.wait_for_status(directory, "running").await;
    assert_eq!(cluster.asked("env.attach"), 2);
    let listed = cluster
        .ask(
            SerialKey::Environment(directory),
            &orch_link_contract::commands::FsList {
                environment_id: directory,
                mount: None,
                path: "web".to_owned(),
                watch_id: None,
                watch_instance: None,
                page: None,
            },
        )
        .await;
    assert!(matches!(listed, Outcome::Succeeded(_)), "{listed:?}");
    step("attached again in the second process");

    let status = terminate(&mut again).await;
    assert!(status.success(), "{status:?}");
    cluster.engine.stop();
    step("done");
}

// ---------------------------------------------------------------- Orch's tools, through a real connector

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_on_a_real_connector_announces_and_another_thread_of_its_topic_is_started_by_it_as_the_topic_says()
 {
    // The scripted harness hosted by the program makes the call of its directive `announce`: an event of its
    // session that the program carries to the system, which answers it once, and the program hands the answer
    // back to the harness. What the call did to the other thread is read from the system. (The listener's own
    // server of these tools, with a harness that speaks the protocol to it, is shown against the connector in
    // process in `orch_server.rs` of the connector: the scripted harness is no client of it.)
    let cluster = Cluster::new().await;
    let tree = cluster.tree().await;
    let on = cluster.everything_on_the_connector(&tree).await;
    // Announcements wake idle threads. Port exposure is explicitly off in this fixture.
    {
        let mut conn = cluster.db.acquire().await.unwrap();
        for (target, key, value) in [
            (tree.topic, "broadcast.wake_idle", true),
            (tree.org, "ports.enabled", false),
        ] {
            orch_settings::write::write(
                &mut conn,
                &orch_settings::write::Write {
                    actor: orch_db::audit::Actor::User(tree.owner.id()),
                    target: orch_settings::Target::Node(target),
                    key,
                    value: json!(value),
                    locked: None,
                    expected_revision: None,
                    may_lock: true,
                    origin: &orch_db::audit::Origin::default(),
                },
                Utc::now(),
            )
            .await
            .unwrap();
        }
    }
    let mut child = cluster.run_connector().await;
    // The thread that will hear: made, and told nothing.
    let hearer = cluster
        .world
        .app
        .call(ThreadCreate, &[&tree.topic])
        .token(&tree.owner.token)
        .body(&CreateThread {
            name: Some("Validate".to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: on.agent,
                provider_id: on.provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(on.environment)),
            directory: Some("web".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .base
        .id;

    let script = "#!fake\nannounce The build moved | It is in ./out now\nsay Told them";
    let (sender, reply) = cluster.converse(&tree, &on, script).await;
    assert_eq!(
        reply,
        [format!("user: {script}"), "agent: Told them".to_owned()]
    );

    // The call was an item of the sender's thread, completed with what the system answered.
    let call: Json = sqlx::query_scalar(
        "SELECT payload FROM items WHERE thread_id = $1 AND kind = 'tool_call' AND payload #>> '{tool,tool}' = 'announce'",
    )
    .bind(sender)
    .fetch_one(cluster.db.pool())
    .await
    .unwrap();
    assert_eq!(call["tool"]["kind"], "orch");
    assert_eq!(call["tool"]["result"]["category"], "notice");
    assert_eq!(call["tool"]["result"]["audience"], "topic");
    assert_eq!(call["tool"]["result"]["applied"]["wake_idle"], true);
    assert_eq!(call["tool"]["result"]["delivered"]["queued"], 1);

    // Browser access on this connected machine remains off until its owner enables it locally.
    let environment = cluster
        .world
        .app
        .call(
            orch_api_contract::environment::EnvironmentGet,
            &[&on.environment],
        )
        .token(&tree.owner.token)
        .send()
        .await
        .ok();
    assert!(!environment.browser);
    assert!(!environment.vnc);
    // Every eligible tool is offered; disabled browser, display and port doors are not advertised.
    let opened = cluster.commands.bodies("session.open");
    let own = opened
        .iter()
        .find(|body| body["thread_id"] == json!(sender))
        .unwrap_or_else(|| panic!("no session was opened for the sender: {opened:?}"));
    let mut offered: Vec<String> = serde_json::from_value(own["orch_tools"].clone()).unwrap();
    offered.sort();
    let unavailable = [
        "use_browser",
        "use_display",
        "list_ports",
        "open_port",
        "list_port_connections",
    ];
    assert!(
        unavailable
            .iter()
            .all(|tool| !offered.iter().any(|offered| offered == tool)),
        "{offered:?}"
    );
    assert!(
        offered.iter().any(|tool| tool == "close_port"),
        "closing earlier exposure stays available"
    );
    let mut all: Vec<String> = orch_engine::tools::definitions::BUILT
        .iter()
        .filter(|tool| !unavailable.contains(&tool.name()))
        .map(|tool| tool.name().to_owned())
        .collect();
    all.sort();
    assert_eq!(offered, all);

    // The other thread had the announcement as the input of a turn that the program ran for it.
    let announced_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT created_at FROM announcements WHERE from_thread_id = $1")
            .bind(sender)
            .fetch_one(cluster.db.pool())
            .await
            .unwrap();
    let wanted = format!(
        "You said: <relayed from-agent=\"Through a real connector\" kind=\"announcement\" at=\"{}\">\nThe build moved\nIt is in ./out now\n</relayed>",
        Time::new(announced_at)
    );
    let observed = Mutex::new(Json::Null);
    eventually(
        "the other thread's turn runs on the announcement",
        || async {
            let said: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'agent_message'
              AND payload ->> 'text' = $2",
            )
            .bind(hearer)
            .bind(&wanted)
            .fetch_one(cluster.db.pool())
            .await
            .unwrap();
            if said != 1 {
                let state: (String, Option<String>, Option<String>, Json) = sqlx::query_as(
                    "SELECT state, blocked_reason, blocked_detail,
                        COALESCE((SELECT jsonb_agg(payload ->> 'text') FROM items
                                  WHERE thread_id = $1 AND kind = 'agent_message'), '[]'::jsonb)
                   FROM threads WHERE node_id = $1",
                )
                .bind(hearer)
                .fetch_one(cluster.db.pool())
                .await
                .unwrap();
                let trace = json!(state);
                let mut previous = lock(&observed);
                if *previous != trace {
                    eprintln!("announcement receiver: {trace}");
                    *previous = trace;
                }
            }
            said == 1
        },
    )
    .await;
    let lines: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'announcement'",
    )
    .bind(hearer)
    .fetch_one(cluster.db.pool())
    .await
    .unwrap();
    assert_eq!(lines, 1, "its history shows the announcement once");

    let status = terminate(&mut child).await;
    assert!(status.success(), "{status:?}");
    cluster.engine.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_on_a_real_connector_deletes_a_document_it_was_shared_to_manage_by_a_plan_and_then_the_word_of_the_plan()
 {
    // The scripted harness makes the first call of `manage_shared` with its directive `manage` and the second with
    // `confirm`, which carries the word the system gave the first. The first changes nothing and is answered with a
    // plan; the second does what the plan listed. What it did is read from the system.
    let cluster = Cluster::new().await;
    let tree = cluster.tree().await;
    let on = cluster.everything_on_the_connector(&tree).await;
    let document = cluster
        .world
        .app
        .call(NodeDocumentCreate, &[&tree.topic])
        .token(&tree.owner.token)
        .body(&NodeDocumentCreateBody {
            title: "Plan".to_owned(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id;
    // The document is shared to manage with the agents of its topic, so the thread that will ask need not exist yet.
    let shared = cluster
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&tree.owner.token)
        .body(&ShareWithAgents {
            read_first: None,
            what: ShareWhat::Document(ShareWhatDocument {
                document_id: document,
            }),
            inside: None,
            parts: Some(ShareParts {
                documents: Some(ShareDocumentsLevel::Manage),
                ..ShareParts::default()
            }),
            mode: ShareMode::Live,
            to: ShareTo::Nodes(vec![ShareToNode {
                node_id: tree.topic,
            }]),
            comment: None,
            expires_at: None,
            client_id: "to-manage".to_owned(),
            download: None,
        })
        .send()
        .await;
    assert_eq!(shared.status, 201);
    let mut child = cluster.run_connector().await;

    let script = format!("#!fake\nmanage delete id:{document}\nconfirm\nsay Gone");
    let (sender, reply) = cluster.converse(&tree, &on, &script).await;
    assert_eq!(reply, [format!("user: {script}"), "agent: Gone".to_owned()]);

    // Two calls of the tool were items of the sender's thread: the plan, and what the word of the plan did.
    let calls: Vec<Json> = sqlx::query_scalar(
        "SELECT payload FROM items WHERE thread_id = $1 AND kind = 'tool_call' AND payload #>> '{tool,tool}' = 'manage_shared' ORDER BY seq_created",
    )
    .bind(sender)
    .fetch_all(cluster.db.pool())
    .await
    .unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0]["tool"]["result"]["done"], false);
    let word = calls[0]["tool"]["result"]["confirm"].as_str().unwrap();
    assert_eq!(word.len(), 32, "128 bits as hexadecimal");
    let listed = &calls[0]["tool"]["result"]["affects"];
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["action"], "delete");
    assert_eq!(
        listed[0]["id"],
        json!(document),
        "the plan names the document asked for by its identifier"
    );
    assert_eq!(
        listed[0]["path"], "/acme/Web/Login/docs/Plan.md",
        "and lists it by its path in the tree"
    );
    assert_eq!(calls[1]["tool"]["arguments"]["confirm"], word);
    assert_eq!(
        calls[1]["tool"]["result"]["done"], true,
        "{}",
        calls[1]["tool"]["result"]
    );

    // The document is deleted as any is, kept until the purge, with the agent's name on it in the history.
    let (deleted, by): (bool, Option<Id>) =
        sqlx::query_as("SELECT deleted_at IS NOT NULL, deleted_by FROM documents WHERE id = $1")
            .bind(document)
            .fetch_one(cluster.db.pool())
            .await
            .unwrap();
    assert!(deleted && by.is_none(), "an agent's delete names no person");
    let lines: Vec<(String, Option<Id>, bool)> = sqlx::query_as(
        "SELECT actor_kind, actor_id, history FROM audit_log WHERE action = 'document.deleted' AND object_id = $1",
    )
    .bind(document)
    .fetch_all(cluster.db.pool())
    .await
    .unwrap();
    assert_eq!(lines, [("agent".to_owned(), Some(sender), true)]);

    let status = terminate(&mut child).await;
    assert!(status.success(), "{status:?}");
    cluster.engine.stop();
}
