//! Real connector restarts retain each diagnostic frame once in PostgreSQL and never resend a consumed queue.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
// The program is stopped the way a service manager stops it, with a termination signal.

#[cfg(unix)]
use std::convert::Infallible;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use bytes::Bytes;
#[cfg(unix)]
use http_body_util::{BodyExt, Full};
#[cfg(unix)]
use hyper::body::Incoming;
#[cfg(unix)]
use hyper::service::service_fn;
#[cfg(unix)]
use hyper::{Method, Request, Response, StatusCode};
#[cfg(unix)]
use hyper_util::rt::TokioIo;
#[cfg(unix)]
use orch_db::queries::{link as cursors, raw_events};
#[cfg(unix)]
use orch_link::controller::{ControllerLink, Notice};
#[cfg(unix)]
use orch_link::framing::{Connection, MessageSink, MessageSource};
#[cfg(unix)]
use orch_link::spool::{Spool, SpoolEvent, SpoolOptions};
#[cfg(unix)]
use orch_link::websocket::accept_tls;
#[cfg(unix)]
use orch_link_contract::enrollment::{
    ENROLL_PATH, Endpoint, EnrollReply, EnrollReplySystem, EnrollRequest, Trust,
};
#[cfg(unix)]
use orch_link_contract::streams::Stream;
#[cfg(unix)]
use orch_pki::ca::Holder;
#[cfg(unix)]
use orch_pki::certificates::ServiceName;
#[cfg(unix)]
use orch_pki::enrollment::{MemoryLedger, TokenGrant, TokenPurpose, issue_token};
#[cfg(unix)]
use orch_pki::verification::{TlsIdentity, pinned_server_config, plain_server_config};
#[cfg(unix)]
use orch_testkit::database::TestDb;
#[cfg(unix)]
use orch_testkit::doubles::link::MachineRecord;
#[cfg(unix)]
use orch_testkit::doubles::link::{Hub, TestPki};
#[cfg(unix)]
use orch_types::{Clock, Id, SystemClock, Time, new_id};
#[cfg(unix)]
use serde_json::json;
#[cfg(unix)]
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::process::{Child, Command};
#[cfg(unix)]
use tokio_rustls::TlsAcceptor;

/// How long the test waits for something that has to happen, in real time.
#[cfg(unix)]
const PATIENCE: Duration = Duration::from_secs(60);
/// Four runs with three frames each exercise three service restarts and fill the frame allowance.
#[cfg(unix)]
const FRAME_ALLOWANCE: i64 = 12;
/// Gives the ACK time to cross the local socket before the termination signal.
#[cfg(unix)]
const ACK_SETTLE: Duration = Duration::from_millis(250);
/// Use zstd's default level, as the engine does for diagnostics.
#[cfg(unix)]
const COMPRESSION_LEVEL: i32 = 3;

/// The system: an authority, the hub behind it, the tokens it gave, and the machine it enrolled.
#[cfg(unix)]
struct System {
    clock: Arc<dyn Clock>,
    pki: TestPki,
    hub: Hub,
    ledger: MemoryLedger,
    gateway_port: u16,
    enrolled: Mutex<Option<(Id, ControllerLink)>>,
    database: sqlx::PgPool,
    received: Arc<Mutex<Vec<i64>>>,
    allow_spool_loss: Arc<AtomicBool>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

#[cfg(unix)]
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

    fn machine(&self) -> (Id, ControllerLink) {
        self.enrolled
            .lock()
            .unwrap()
            .clone()
            .expect("no machine enrolled")
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

    /// The answer to a request to enroll: the authority signs, a machine of the hub is made for it, and
    /// the endpoint named is the gateway's listener.
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
        let (controller, notices) = self.hub.add_machine(
            enrolled.machine_id,
            request.roles.clone(),
            Default::default(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        self.tasks.lock().unwrap().push(tokio::spawn(keep_events(
            self.database.clone(),
            enrolled.machine_id,
            controller.clone(),
            notices,
            Arc::clone(&self.received),
            Arc::clone(&self.allow_spool_loss),
        )));
        *self.enrolled.lock().unwrap() = Some((enrolled.machine_id, controller));
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
#[cfg(unix)]
async fn pump(mut from: Box<dyn MessageSource>, mut to: Box<dyn MessageSink>) {
    while let Ok(Some(message)) = from.recv().await {
        if to.send(message).await.is_err() {
            break;
        }
    }
}

/// The gateway's listener: TLS under the system's authority with the machine's certificate asked for,
/// then the WebSocket, then the hub, which holds the session as it would in memory.
#[cfg(unix)]
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
#[cfg(unix)]
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

#[cfg(unix)]
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

/// A system on two loopback sockets.
#[cfg(unix)]
async fn start_system(database: sqlx::PgPool) -> (Arc<System>, u16) {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let gateway = listener().await;
    let enrollment = listener().await;
    let system = Arc::new(System {
        clock,
        pki,
        hub,
        ledger: MemoryLedger::new(),
        gateway_port: gateway.local_addr().unwrap().port(),
        enrolled: Mutex::new(None),
        database,
        received: Arc::default(),
        allow_spool_loss: Arc::default(),
        tasks: Mutex::new(Vec::new()),
    });
    let enrollment_port = enrollment.local_addr().unwrap().port();
    system
        .tasks
        .lock()
        .unwrap()
        .push(tokio::spawn(serve_gateway(Arc::clone(&system), gateway)));
    system
        .tasks
        .lock()
        .unwrap()
        .push(tokio::spawn(serve_enrollment(
            Arc::clone(&system),
            enrollment,
        )));
    (system, enrollment_port)
}

/// The program, as a command with the data directory it is to work in.
#[cfg(unix)]
fn program(data_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orch-connector"));
    command.arg("--data-dir").arg(data_dir);
    // Nothing of the environment of whoever runs the test is the program's configuration.
    command.env_remove("ORCH_DATA_DIR").env_remove("ORCH_LOG");
    command
}

/// Runs the program to its end and returns what it printed on standard output; the test fails if it fails.
#[cfg(unix)]
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
#[cfg(unix)]
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

// All listeners stay in the test-port range. Random choices avoid collisions with other workers.
#[cfg(unix)]
async fn listener() -> TcpListener {
    const FIRST: u16 = 41_000;
    const PORTS: u128 = 1_000;
    // This range belongs to the shared development stack, including its temporarily unused ports.
    const SHARED_START: u16 = 41_280;
    const SHARED_END: u16 = 41_299;
    const ATTEMPTS: usize = 64; // A generous bound for a range used by a few dozen concurrent tests.
    for _ in 0..ATTEMPTS {
        let port = FIRST + u16::try_from(new_id().as_u128() % PORTS).unwrap();
        if (SHARED_START..=SHARED_END).contains(&port) {
            continue;
        }
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
            return listener;
        }
    }
    panic!("no free test listener");
}

#[cfg(unix)]
async fn keep_events(
    pool: sqlx::PgPool,
    machine: Id,
    link: ControllerLink,
    mut notices: tokio::sync::mpsc::UnboundedReceiver<Notice>,
    received: Arc<Mutex<Vec<i64>>>,
    allow_spool_loss: Arc<AtomicBool>,
) {
    sqlx::query(
        "INSERT INTO machines (id, name, placement, roles, platform_os, platform_arch,
        connector_version, protocol_version, cert_serial, cert_fingerprint, cert_not_after)
        VALUES ($1, 'raw-restart', 'managed', '{agent,provider,environment}', 'linux', 'x86_64',
            '0.1.0', 1, 'test', $2, '2099-01-01')",
    )
    .bind(machine)
    .bind(machine.as_bytes().as_slice())
    .execute(&pool)
    .await
    .unwrap();
    while let Some(notice) = notices.recv().await {
        match notice {
            Notice::Deliver(delivery) => {
                let mut tx = pool.begin().await.unwrap();
                let mut frames = Vec::new();
                if let Stream::Session { segment_id, .. } = delivery.stream {
                    for event in &delivery.events {
                        if event.kind == "raw.frame" {
                            let text = serde_json::to_vec(&event.body["payload"]).unwrap();
                            let compressed =
                                zstd::stream::encode_all(text.as_slice(), COMPRESSION_LEVEL)
                                    .unwrap();
                            raw_events::append(
                                &mut tx,
                                &raw_events::NewRawEvent {
                                    segment: segment_id,
                                    n: event.body["n"].as_i64().unwrap(),
                                    at: event.at.get(),
                                    direction: raw_events::FROM_HARNESS,
                                    payload: &compressed,
                                    at_most: FRAME_ALLOWANCE,
                                },
                            )
                            .await
                            .unwrap();
                            frames.push(event.seq);
                        }
                    }
                }
                cursors::set_cursor(
                    &mut tx,
                    machine,
                    &delivery.stream.to_string(),
                    delivery.last_seq,
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
                received.lock().unwrap().extend(frames);
                link.committed(&delivery);
            }
            Notice::Reset(reset) => {
                assert!(
                    allow_spool_loss.load(Ordering::SeqCst),
                    "a preserved spool must not reset: {reset:?}"
                );
                assert_eq!(
                    reset.reason,
                    orch_link_contract::streams::ResetReason::SpoolLost
                );
                let mut conn = pool.acquire().await.unwrap();
                cursors::set_cursor(
                    &mut conn,
                    machine,
                    &reset.stream.to_string(),
                    reset.from - 1,
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
            }
            _ => {}
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn three_sigterm_restarts_keep_each_raw_frame_once_in_order_in_a_real_database() {
    let database = TestDb::fresh().await;
    let (system, enrollment_port) = start_system(database.pool.clone()).await;
    let data = tempfile::tempdir().unwrap();
    let mut enroll = program(data.path());
    enroll.args([
        "enroll",
        "--url",
        &format!("https://127.0.0.1:{enrollment_port}"),
        "--token",
        &system.token(),
        "--name",
        "raw-restart",
    ]);
    succeeds(enroll).await;
    let (machine, _) = system.machine();
    let (thread, segment) = (new_id(), new_id());
    seed_segment(&database.pool, thread, segment).await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    }
    .to_string();
    let directory = orch_connector_core::enrollment::DataDir::open(data.path()).unwrap();
    let options = SpoolOptions::from(&directory.config(|_| None).unwrap().spool);
    let mut expected = Vec::new();
    let mut expected_sources = Vec::new();
    for wave in 0..4 {
        // These are the real spool records an adapter publishes. Add more only while its owner is down.
        let mut spool = Spool::open_for_machine(data.path(), options).unwrap().0;
        let at = Time::new(system.clock.now());
        spool
            .append(
                &stream,
                &SpoolEvent {
                    at,
                    kind: "session.title".to_owned(),
                    body: json!({ "title": format!("Wave {wave}") }),
                },
            )
            .unwrap();
        for frame in 0..3 {
            let payload = json!({ "wave": wave, "frame": frame });
            expected.push(payload.clone());
            expected_sources.push(wave * 4 + frame + 2);
            spool
                .append(
                    &stream,
                    &SpoolEvent {
                        at,
                        kind: "raw.frame".to_owned(),
                        body: json!({ "direction": "in", "payload": payload }),
                    },
                )
                .unwrap();
        }
        spool.sync().unwrap();
        drop(spool);
        let log_path = data.path().join("logs").join(format!("run-{wave}.log"));
        let mut run = program(data.path());
        run.arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log_path).unwrap())
            .kill_on_drop(true);
        let mut child = run.spawn().unwrap();
        tokio::time::timeout(PATIENCE, async {
            loop {
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM raw_events WHERE segment_id = $1")
                        .bind(segment)
                        .fetch_one(&database.pool)
                        .await
                        .unwrap();
                if count >= i64::try_from(expected.len()).unwrap() {
                    break;
                }
                if let Some(status) = child.try_wait().unwrap() {
                    panic!(
                        "connector stopped {status}: {}",
                        std::fs::read_to_string(&log_path).unwrap()
                    );
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("raw frames never reached the database");
        // Wait for the committed ACK to reach the machine, then stop it as a service manager does.
        tokio::time::sleep(ACK_SETTLE).await;
        assert!(terminate(&mut child).await.success());
        let mut conn = database.pool.acquire().await.unwrap();
        let mut rows = raw_events::newest(&mut conn, segment, None, 100)
            .await
            .unwrap();
        rows.reverse();
        let kept: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                serde_json::from_slice(&zstd::stream::decode_all(row.payload.as_slice()).unwrap())
                    .unwrap()
            })
            .collect();
        assert_eq!(
            kept, expected,
            "each distinct frame is retained once after restart {wave}"
        );
        assert_eq!(
            rows.iter().map(|row| row.n).collect::<Vec<_>>(),
            (1..=i64::try_from(expected.len()).unwrap()).collect::<Vec<_>>()
        );
        assert_eq!(
            *system.received.lock().unwrap(),
            expected_sources,
            "acknowledged frames are never delivered again"
        );
        tokio::time::timeout(PATIENCE, async {
            while system.hub.session_of(machine).is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the connector's session outlived its process");
        let spool = Spool::open_for_machine(data.path(), options).unwrap().0;
        assert_eq!(
            spool.diagnostics_waiting(&stream),
            0,
            "no consumed diagnostic is queued again"
        );
    }
    for task in system.tasks.lock().unwrap().drain(..) {
        task.abort();
    }
}

#[cfg(unix)]
async fn seed_segment(pool: &sqlx::PgPool, thread: Id, segment: Id) {
    let user = new_id();
    sqlx::query("INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, $2, 'Raw tester', 0)")
        .bind(user).bind(user.simple().to_string()).execute(pool).await.unwrap();
    let (org, project, topic) = (new_id(), new_id(), new_id());
    for (node, kind, parent, path) in [
        (org, "org", None, vec![org]),
        (project, "project", Some(org), vec![org, project]),
        (topic, "topic", Some(project), vec![org, project, topic]),
        (
            thread,
            "thread",
            Some(topic),
            vec![org, project, topic, thread],
        ),
    ] {
        sqlx::query(
            "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by)
                     VALUES ($1, $2, $3, $4, $5, 'Raw frames', $6, $6)",
        )
        .bind(node)
        .bind(kind)
        .bind(parent)
        .bind(org)
        .bind(path)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO threads (node_id, topic_id) VALUES ($1, $2)")
        .bind(thread)
        .bind(topic)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO segments (id, thread_id, ordinal, agent_id, provider_id, environment_id,
                    env_generation, arrangement, credential_path, seq_updated)
                VALUES ($1, $2, 1, $3, $3, $3, 1, 'together', 'native', 1)",
    )
    .bind(segment)
    .bind(thread)
    .bind(new_id())
    .execute(pool)
    .await
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_session_open_installs_the_server_raw_floor_before_a_real_connector_restart() {
    use orch_link::commands::CommandFailure;
    use orch_link_contract::commands::{ErrorCode, SerialKey, SessionOpen};
    let database = TestDb::fresh().await;
    let (system, enrollment_port) = start_system(database.pool.clone()).await;
    let data = tempfile::tempdir().unwrap();
    let mut enroll = program(data.path());
    enroll.args([
        "enroll",
        "--url",
        &format!("https://127.0.0.1:{enrollment_port}"),
        "--token",
        &system.token(),
        "--name",
        "raw-floor",
    ]);
    succeeds(enroll).await;
    let (machine, link) = system.machine();
    let (thread, segment) = (new_id(), new_id());
    seed_segment(&database.pool, thread, segment).await;
    // The earlier bytes have expired, but their segment cursor remains.
    const FLOOR: i64 = 5000;
    sqlx::query("UPDATE segments SET raw_last_n = $2 WHERE id = $1")
        .bind(segment)
        .bind(FLOOR)
        .execute(&database.pool)
        .await
        .unwrap();
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    }
    .to_string();
    let mut run = program(data.path());
    run.arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = run.spawn().unwrap();
    tokio::time::timeout(PATIENCE, async {
        while system.hub.session_of(machine).is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let open: SessionOpen = serde_json::from_value(json!({
        "agent_id": new_id(), "thread_id": thread, "segment_id": segment, "mode": "new",
        "native_session_ref": null, "archive": null, "history": null, "upto_turn_ordinal": null,
        "workspace_root": data.path().display().to_string(), "instruction_root_marker": false,
        "directory": data.path().display().to_string(), "cwd": data.path().display().to_string(),
        "mounts": [], "model": "unconfigured", "effort": null, "params": {},
        "approval_mode": "on_request", "sandbox_mode": "workspace", "instructions": "", "memory": null,
        "arrangement": "separated", "provider": { "path": "native", "provider_id": new_id() },
        "exec": { "executor": "codex-exec-server", "tool_set": null, "environment_id": new_id(), "generation": 1, "ticket": null },
        "mcp": [], "orch_tools": [], "raw_events": true, "raw_after": FLOOR
    })).unwrap();
    let deadline = Time::new(
        system.clock.now() + chrono::Duration::seconds(i64::try_from(PATIENCE.as_secs()).unwrap()),
    );
    let mut issued = link
        .commands()
        .issue_typed(new_id(), &SerialKey::Thread(thread), deadline, &open)
        .unwrap();
    let result = tokio::time::timeout(PATIENCE, issued.wait_for::<SessionOpen>())
        .await
        .unwrap();
    // No model harness is configured or launched. The floor is installed before that lookup fails.
    assert!(
        matches!(&result, Err(CommandFailure::Failed(error)) if error.code == ErrorCode::HarnessUnavailable.as_str()),
        "{result:?}"
    );
    assert!(terminate(&mut child).await.success());
    tokio::time::timeout(PATIENCE, async {
        while system.hub.session_of(machine).is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Losing every spool segment must not lose the allocator installed by the real command.
    system.allow_spool_loss.store(true, Ordering::SeqCst);
    std::fs::remove_dir_all(data.path().join("spool")).unwrap();
    let directory = orch_connector_core::enrollment::DataDir::open(data.path()).unwrap();
    let options = SpoolOptions::from(&directory.config(|_| None).unwrap().spool);
    let mut spool = Spool::open_for_machine(data.path(), options).unwrap().0;
    spool
        .append(
            &stream,
            &SpoolEvent {
                at: Time::new(system.clock.now()),
                kind: "raw.frame".to_owned(),
                body: json!({"direction": "in", "payload": "new capture"}),
            },
        )
        .unwrap();
    spool.sync().unwrap();
    drop(spool);
    let mut run = program(data.path());
    run.arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = run.spawn().unwrap();
    let retained = tokio::time::timeout(PATIENCE, async {
        loop {
            let n: Option<i64> =
                sqlx::query_scalar("SELECT n FROM raw_events WHERE segment_id = $1")
                    .bind(segment)
                    .fetch_optional(&database.pool)
                    .await
                    .unwrap();
            if n == Some(FLOOR + 1) {
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("the connector ended {status}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if retained.is_err() {
        let numbers: Vec<i64> =
            sqlx::query_scalar("SELECT n FROM raw_events WHERE segment_id = $1 ORDER BY n")
                .bind(segment)
                .fetch_all(&database.pool)
                .await
                .unwrap();
        let allocator = std::fs::read_to_string(data.path().join("spool-meta/RAW_CURSORS"));
        panic!(
            "the new capture did not use the installed floor: rows={numbers:?}, allocator={allocator:?}, connected={}, received={:?}",
            system.hub.session_of(machine).is_some(),
            system.received.lock().unwrap()
        );
    }
    tokio::time::sleep(ACK_SETTLE).await;
    assert!(terminate(&mut child).await.success());
    let last: i64 = sqlx::query_scalar("SELECT raw_last_n FROM segments WHERE id = $1")
        .bind(segment)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(last, FLOOR + 1);
    for task in system.tasks.lock().unwrap().drain(..) {
        task.abort();
    }
}
