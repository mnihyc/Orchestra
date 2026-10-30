//! The program itself, run as a process: it enrolls with a token over HTTPS, takes the owner's limits from its command line, holds a session to a gateway over TLS and a WebSocket, hosts the scripted harness for a turn, and stops when it is told to. The system is in memory (the testkit's hub) behind sockets on the loopback address that the operating system chose; to run it by hand, `cargo test -p orch-connector --test program -- --nocapture`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::convert::Infallible;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_link::framing::{Connection, MessageSink, MessageSource};
use orch_link::websocket::accept_tls;
#[cfg(unix)]
use orch_link_contract::commands::{AgentStart, SerialKey, SessionClose, SessionOpen, TurnStart};
use orch_link_contract::enrollment::{
    ENROLL_PATH, Endpoint, EnrollReply, EnrollReplySystem, EnrollRequest, Trust,
};
#[cfg(unix)]
use orch_link_contract::streams::Stream;
use orch_pki::ca::Holder;
use orch_pki::certificates::ServiceName;
use orch_pki::enrollment::{MemoryLedger, TokenGrant, TokenPurpose, issue_token};
use orch_pki::verification::{TlsIdentity, pinned_server_config, plain_server_config};
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
#[cfg(unix)]
use orch_types::Time;
use orch_types::{Clock, Id, SystemClock, new_id};
#[cfg(unix)]
use serde_json::json;
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::process::Child;
use tokio::process::Command;
use tokio_rustls::TlsAcceptor;

/// How long the test waits for something that has to happen, in real time.
const PATIENCE: Duration = Duration::from_secs(60);

/// The system: an authority, the hub behind it, the tokens it gave, and the machine it enrolled.
struct System {
    clock: Arc<dyn Clock>,
    pki: TestPki,
    hub: Hub,
    ledger: MemoryLedger,
    gateway_port: u16,
    enrolled: Mutex<Option<(Id, Arc<ScriptedController>)>>,
}

impl System {
    fn token(&self) -> String {
        self.token_choices(None)
    }

    fn token_choices(
        &self,
        choices: Option<&orch_link_contract::enrollment_choices::EnrollmentChoices>,
    ) -> String {
        let mut issued = issue_token(
            self.pki.authority(),
            Duration::from_secs(900),
            self.clock.as_ref(),
        );
        if let Some(choices) = choices {
            issued.token = issued.token.with_choices(choices).unwrap();
            issued.digest = orch_pki::enrollment::token_digest(issued.token.secret());
        }
        self.ledger.record(
            &issued,
            TokenGrant {
                purpose: TokenPurpose::Machine,
                created_by: new_id(),
            },
        );
        issued.token.to_string()
    }

    #[cfg(unix)]
    fn machine(&self) -> (Id, Arc<ScriptedController>) {
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
        let controller = ScriptedController::attach(
            &self.hub,
            Arc::clone(&self.clock),
            enrolled.machine_id,
            request.roles.clone(),
        );
        *self.enrolled.lock().unwrap() = Some((enrolled.machine_id, Arc::new(controller)));
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

/// The gateway's listener: TLS under the system's authority with the machine's certificate asked for,
/// then the WebSocket, then the hub, which holds the session as it would in memory.
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

/// A system on two loopback sockets.
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
        enrolled: Mutex::new(None),
    });
    let enrollment_port = enrollment.local_addr().unwrap().port();
    tokio::spawn(serve_gateway(Arc::clone(&system), gateway));
    tokio::spawn(serve_enrollment(Arc::clone(&system), enrollment));
    (system, enrollment_port)
}

/// The program, as a command with the data directory it is to work in.
fn program(data_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orch-connector"));
    command.arg("--data-dir").arg(data_dir);
    // Nothing of the environment of whoever runs the test is the program's configuration.
    command
        .env_remove("ORCH_DATA_DIR")
        .env_remove("ORCH_LOG")
        .env_remove("ORCH_PLUGINS");
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

#[cfg(unix)]
fn deadline() -> Time {
    Time::new(chrono::Utc::now() + chrono::Duration::minutes(5))
}

#[cfg(unix)]
async fn command<B: orch_link_contract::commands::CommandBody>(
    controller: &ScriptedController,
    key: &SerialKey,
    body: &B,
) -> serde_json::Value {
    let id = controller.issue(key, deadline(), body);
    match tokio::time::timeout(PATIENCE, controller.wait_command(id))
        .await
        .unwrap()
    {
        orch_link::commands::Progress::Succeeded(result) => result,
        other => panic!("the command did not succeed: {other:?}"),
    }
}

/// Looks every few milliseconds whether something holds, for as long as `PATIENCE`.
#[cfg(unix)]
async fn eventually(mut holds: impl FnMut() -> bool) -> bool {
    let began = std::time::Instant::now();
    while began.elapsed() < PATIENCE {
        if holds() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    holds()
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

/// Says, when the test is run with `--nocapture`, how far it has come and how long that took.
#[cfg(unix)]
fn step(began: std::time::Instant, what: &str) {
    eprintln!("[{:>6.2} s] {what}", began.elapsed().as_secs_f64());
}

/// A real supervised program speaking the public contract, rather than a registry-only double.
#[cfg(unix)]
fn program_fixture(
    directory: &Path,
    kind: orch_plugin_contract::PluginKind,
    name: &str,
    descriptor: serde_json::Value,
    mut responses: serde_json::Value,
) -> Vec<String> {
    responses["describe"] = json!({"result":descriptor.clone()});
    let mut methods = orch_plugin_contract::negotiation::methods(kind)
        .iter()
        .filter(|method| method.required)
        .map(|method| method.name.to_owned())
        .collect::<Vec<_>>();
    methods.extend(responses.as_object().unwrap().keys().cloned());
    methods.sort();
    methods.dedup();
    let script = directory.join(format!("{name}.json"));
    std::fs::write(
        &script,
        json!({"kind":kind.as_str(),"name":name,"version":"1.0.0","methods":methods,
        "descriptor":descriptor,"responses":responses})
        .to_string(),
    )
    .unwrap();
    vec![
        "python3".into(),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/platform/plugin/tests/support/scripted_plugin.py")
            .display()
            .to_string(),
        "--script".into(),
        script.display().to_string(),
        "--log".into(),
        directory.join(format!("{name}.log")).display().to_string(),
    ]
}

#[cfg(unix)]
fn run_program(data: &Path) -> Child {
    let log = std::fs::File::create(data.join("programs-test.log")).unwrap();
    program(data)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn programs_register_describe_run_survive_restart_remove_and_reload_with_the_local_policy_enforced()
 {
    use orch_api_contract::machine_programs::{
        MachineProgramKey, MachineProgramKind as Kind, MachineProgramRegistration,
        MachineProgramsCommand as Request,
    };
    use orch_connector_core::enrollment::DataDir;
    use orch_plugin_contract::PluginKind;
    let (system, port) = start_system().await;
    let directory = tempfile::tempdir().unwrap();
    let mut enroll = program(directory.path());
    enroll.args([
        "enroll",
        "--url",
        &format!("https://127.0.0.1:{port}"),
        "--token",
        &system.token(),
        "--name",
        "programs-machine",
    ]);
    succeeds(enroll).await;
    let mut handover = program(directory.path());
    handover.args(["policy", "delegate", "all"]);
    succeeds(handover).await;
    let (machine, controller) = system.machine();
    let mut child = run_program(directory.path());
    assert!(eventually(|| system.hub.session_of(machine).is_some()).await);
    let provider = program_fixture(
        directory.path(),
        PluginKind::Provider,
        "gpu-source",
        json!({
        "name":"gpu-source","title":"GPU source","summary":"A local model","contract":1,
        "facts":null,"models":{},"presents":null,"fields":[]}),
        json!({"models.list":{"result":{"models":[{"model_name":"gpu-model","display_name":"GPU model"}]}}}),
    );
    let request = MachineProgramRegistration {
        kind: Kind::ProviderSource,
        name: "gpu-source".into(),
        command: provider.clone(),
    };
    let registered = command(
        &controller,
        &SerialKey::Machine,
        &Request::Register(request.clone()),
    )
    .await;
    assert_eq!(
        registered["programs"]["entries"][0]["descriptor"]["title"],
        "GPU source"
    );
    assert_eq!(
        registered["programs"]["entries"][0]["models"]["models"][0]["display_name"], "GPU model",
        "registration returns the source's actual preview without a separate Test"
    );
    let calls = std::fs::read_to_string(directory.path().join("gpu-source.log")).unwrap();
    assert!(
        calls
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .any(|call| call["received"]["method"] == "describe"),
        "Register runs the actual describe call before it persists"
    );
    let preview = calls
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|call| call["received"]["method"] == "models.list")
        .unwrap();
    assert_eq!(preview["received"]["params"]["secrets"], json!({}));
    assert_eq!(preview["received"]["params"]["endpoints"], json!([]));
    assert!(preview["received"]["params"].get("credential").is_none());
    assert!(
        preview["received"]["params"].get("provider").is_none(),
        "a preview never invents a provider"
    );
    assert_eq!(
        registered["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "gpu-source")
            .unwrap()["state"],
        "running"
    );
    let identity = MachineProgramKey {
        kind: Kind::ProviderSource,
        name: "gpu-source".into(),
    };
    let tested = command(
        &controller,
        &SerialKey::Machine,
        &Request::Test(identity.clone()),
    )
    .await;
    assert_eq!(
        tested["programs"]["entries"][0]["models"]["models"][0]["model_name"],
        "gpu-model"
    );
    let descriptor = json!({"kind":"tiny-live","adapter_version":1,
        "harness":{"name":"Tiny live","version":"1.0.0","supported_range":">=1.0.0","status":"supported"},
        "arrangements":[{"type":"together"}],"approval_modes":["on_request"],"sandbox_modes":["open"]});
    let adapter = program_fixture(
        directory.path(),
        PluginKind::Adapter,
        "tiny-live",
        descriptor.clone(),
        json!({"agent.start":{"result":{"harness_version":"1.0.0","descriptor":descriptor}},"agent.stop":{"result":{}}}),
    );
    let registered = command(
        &controller,
        &SerialKey::Machine,
        &Request::Register(MachineProgramRegistration {
            kind: Kind::Agent,
            name: "tiny-live".into(),
            command: adapter,
        }),
    )
    .await;
    let adapter = registered["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "tiny-live")
        .unwrap();
    assert_eq!(
        adapter["needs_binary"], false,
        "the supervised program provides its own harness"
    );
    let (provider_id, agent_id) = (new_id(), new_id());
    let start: AgentStart = serde_json::from_value(
        json!({"provider_id":provider_id,"agent_id":agent_id,"harness_kind":"tiny-live",
        "profile":{"harness_version":null,"config":{},"env":{}},"home":null}),
    )
    .unwrap();
    let started = command(
        &controller,
        &SerialKey::Harness {
            provider_id,
            agent_id,
        },
        &start,
    )
    .await;
    assert_eq!(started["descriptor"]["harness"]["name"], "Tiny live");
    let reports_before = controller.applied("machine").len();
    assert!(terminate(&mut child).await.success());
    assert!(eventually(|| system.hub.session_of(machine).is_none()).await);
    child = run_program(directory.path());
    assert!(eventually(|| system.hub.session_of(machine).is_some()).await);
    assert!(
        controller
            .wait_until(PATIENCE, |controller| controller
                .applied("machine")
                .iter()
                .skip(reports_before)
                .any(|event| event.body["programs"]["entries"]
                    .as_array()
                    .is_some_and(|entries| entries
                        .iter()
                        .any(|entry| entry["name"] == "gpu-source"
                            && entry["models"]["models"][0]["display_name"]
                                == "GPU model"))))
            .await,
        "configured sources regain their metadata preview at startup, without a page/native read"
    );
    let reloaded = command(&controller, &SerialKey::Machine, &Request::Reload).await;
    assert_eq!(
        reloaded["programs"]["entries"].as_array().unwrap().len(),
        2,
        "screen registrations survive a native restart"
    );
    let removed = command(
        &controller,
        &SerialKey::Machine,
        &Request::Remove(identity.clone()),
    )
    .await;
    assert!(
        !removed["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "gpu-source")
    );
    let data = DataDir::open(directory.path()).unwrap();
    assert!(
        !data
            .config(|_| None)
            .unwrap()
            .plugin
            .iter()
            .any(|entry| entry.name == "gpu-source")
    );
    // A locally kept value wins even under '*'; an arbitrary command cannot run through Test or Reload.
    let mut keep = program(directory.path());
    keep.args(["policy", "set", "provider.programs", "false"]);
    succeeds(keep).await;
    let id = controller.issue(&SerialKey::Machine, deadline(), &Request::Register(request));
    let refusal = tokio::time::timeout(PATIENCE, controller.wait_command(id))
        .await
        .unwrap();
    let orch_link::commands::Progress::Failed(refusal) = refusal else {
        panic!("registration did not refuse: {refusal:?}")
    };
    assert!(refusal.detail.contains("provider.programs"));
    assert!(refusal.detail.contains("provider"));
    assert!(
        !data
            .config(|_| None)
            .unwrap()
            .plugin
            .iter()
            .any(|entry| entry.name == "gpu-source")
    );
    // The same kept machine can load a program its operator explicitly configured there.
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(data.config_path())
        .unwrap();
    writeln!(
        file,
        "\n[[plugin]]\nkind = 'provider'\nname = 'gpu-source'\ncommand = {}\n",
        serde_json::to_string(&provider).unwrap()
    )
    .unwrap();
    drop(file);
    let local = command(&controller, &SerialKey::Machine, &Request::Reload).await;
    let reloaded = local.clone();
    let local = local["programs"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "gpu-source")
        .unwrap_or_else(|| panic!("the configured program is missing after reload: {reloaded}"));
    assert_eq!(local["origin"], "local");
    assert_eq!(local["ready"], true);
    let id = controller.issue(
        &SerialKey::Machine,
        deadline(),
        &Request::Remove(identity.clone()),
    );
    let refused = tokio::time::timeout(PATIENCE, controller.wait_command(id))
        .await
        .unwrap();
    assert!(
        matches!(refused, orch_link::commands::Progress::Failed(_)),
        "{refused:?}"
    );
    let tested = command(&controller, &SerialKey::Machine, &Request::Test(identity)).await;
    assert!(
        tested["programs"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "gpu-source"
                && entry["models"]["models"][0]["model_name"] == "gpu-model")
    );
    assert!(terminate(&mut child).await.success());
}

#[tokio::test]
async fn role_enrollment_and_local_commands_preserve_handover_and_keep_node_separate() {
    use orch_connector_core::enrollment::DataDir;
    use orch_link_contract::{
        enrollment_choices::{CONNECTOR_NODE_REFUSAL, EnrollmentChoices},
        session::Role,
    };
    tokio::time::timeout(PATIENCE, async {
        let (system, port) = start_system().await;
        let directory = tempfile::tempdir().unwrap();
        let choices = EnrollmentChoices::new(
            vec![Role::Environment, Role::Provider],
            vec!["environment".into()],
            true,
        )
        .unwrap();
        let mut enroll = program(directory.path());
        enroll.kill_on_drop(true).args([
            "enroll",
            "--url",
            &format!("https://127.0.0.1:{port}"),
            "--token",
            &system.token_choices(Some(&choices)),
            "--roles",
            "environment,provider",
            "--delegate",
            "environment",
            "--dedicated",
        ]);
        assert!(succeeds(enroll).await.contains("Enrolled"));
        let data = DataDir::open(directory.path()).unwrap();
        let config = data.config(|_| None).unwrap();
        assert_eq!(config.connector.roles, ["environment", "provider"]);
        assert_eq!(config.policy.delegated, ["environment.*"]);
        assert!(config.connector.dedicated);
        let id = data.record().unwrap().machine_id;

        let mut change = program(directory.path());
        change
            .kill_on_drop(true)
            .args(["roles", "set", "agent,provider"]);
        assert!(succeeds(change).await.contains("Restart the connector"));
        let changed = data.config(|_| None).unwrap();
        assert_eq!(changed.connector.roles, ["agent", "provider"]);
        assert!(changed.connector.dedicated);
        assert_eq!(
            changed.policy.delegated, config.policy.delegated,
            "a role change does not change the local handover"
        );
        assert_eq!(
            data.record().unwrap().machine_id,
            id,
            "a local role change keeps the enrolled identity"
        );
        let before = std::fs::read(data.config_path()).unwrap();
        let mut refused = program(directory.path());
        refused.kill_on_drop(true).args(["roles", "set", "node"]);
        let output = refused.output().await.unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(CONNECTOR_NODE_REFUSAL));
        assert_eq!(std::fs::read(data.config_path()).unwrap(), before);

        // Refuse Node before any enrollment request, even when the address cannot be reached.
        let other = tempfile::tempdir().unwrap();
        let mut refused = program(other.path());
        refused.kill_on_drop(true).args([
            "enroll",
            "--url",
            "https://127.0.0.1:1",
            "--token",
            "unused",
            "--roles",
            "node",
        ]);
        let output = refused.output().await.unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(CONNECTOR_NODE_REFUSAL));
        assert!(!DataDir::enrolled_at(other.path()));
    })
    .await
    .expect(
        "role enrollment and local CLI commands must finish within the protocol's test deadline",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_source_that_needs_provider_context_can_register_even_when_its_metadata_preview_refuses()
{
    use orch_api_contract::machine_programs::{
        MachineProgramKind as Kind, MachineProgramRegistration, MachineProgramsCommand as Request,
    };
    use orch_plugin_contract::PluginKind;
    let (system, port) = start_system().await;
    let directory = tempfile::tempdir().unwrap();
    let mut enroll = program(directory.path());
    enroll.args([
        "enroll",
        "--url",
        &format!("https://127.0.0.1:{port}"),
        "--token",
        &system.token(),
    ]);
    succeeds(enroll).await;
    let mut handover = program(directory.path());
    handover.args(["policy", "delegate", "provider.programs"]);
    succeeds(handover).await;
    let (machine, controller) = system.machine();
    let mut child = run_program(directory.path());
    assert!(eventually(|| system.hub.session_of(machine).is_some()).await);
    let source = program_fixture(
        directory.path(),
        PluginKind::Provider,
        "configured-source",
        json!({"name":"configured-source","title":"Configured source","summary":"Needs a provider.","contract":1,"facts":null,"models":{},"presents":null,"fields":[]}),
        json!({"models.list":{"error":{"code":"unavailable","message":"Choose the provider's endpoint first.","retryable":false}}}),
    );
    let registered = command(
        &controller,
        &SerialKey::Machine,
        &Request::Register(MachineProgramRegistration {
            kind: Kind::ProviderSource,
            name: "configured-source".into(),
            command: source,
        }),
    )
    .await;
    let entry = &registered["programs"]["entries"][0];
    assert_eq!(
        entry["ready"], true,
        "an optional model preview cannot prevent use with a real provider"
    );
    assert!(entry["models"].is_null());
    assert!(entry["detail"].as_str().unwrap().contains("provider"));
    assert_eq!(registered["plugins"][0]["state"], "running");
    assert!(terminate(&mut child).await.success());
}

#[cfg(unix)]
#[tokio::test]
async fn a_delegated_role_change_reconnects_the_real_connector_and_makes_its_new_agent_usable() {
    use orch_connector_core::enrollment::DataDir;
    use orch_link_contract::commands::{MachineProbe, MachineRolesSet};
    tokio::time::timeout(PATIENCE,async {
        let (system,port)=start_system().await;let directory=tempfile::tempdir().unwrap();
        let mut enroll=program(directory.path());enroll.args(["enroll","--url",&format!("https://127.0.0.1:{port}"),"--token",&system.token(),"--roles","environment"]);
        succeeds(enroll).await;
        let mut handover=program(directory.path());handover.args(["policy","delegate","machine.roles"]);succeeds(handover).await;
        let (machine,controller)=system.machine();let mut child=run_program(directory.path());
        assert!(eventually(||system.hub.session_of(machine).is_some()).await);
        let session=system.hub.session_of(machine);
        let initial=command(&controller,&SerialKey::Machine,&MachineProbe {}).await;
        assert_eq!(initial["policy"]["policy.roles"],json!(["environment"]));
        assert_eq!(initial["role_management"]["supported"],true);
        let request=MachineRolesSet {roles:vec!["environment".into(),"agent".into(),"provider".into()]};
        let acknowledged=command(&controller,&SerialKey::Machine,&request).await;
        assert_eq!(acknowledged["roles"],json!(["environment","agent","provider"]));assert_eq!(acknowledged["reconnecting"],true);
        assert!(eventually(||system.hub.session_of(machine).is_some_and(|next|Some(next)!=session)).await);
        let active=command(&controller,&SerialKey::Machine,&MachineProbe {}).await;
        assert_eq!(active["policy"]["policy.roles"],json!(["environment","agent","provider"]));
        assert!(active["adapters"].as_array().unwrap().iter().any(|adapter|adapter["kind"]=="fake"));
        let (provider_id,agent_id)=(new_id(),new_id());
        let start:AgentStart=serde_json::from_value(json!({"provider_id":provider_id,"agent_id":agent_id,"harness_kind":"fake",
            "profile":{"harness_version":null,"config":{},"env":{}},"home":null})).unwrap();
        let started=command(&controller,&SerialKey::Harness {provider_id,agent_id},&start).await;
        assert_eq!(started["descriptor"]["kind"],"fake","the new role is an actual agent handler, not just a reported tag");
        let data=DataDir::open(directory.path()).unwrap();assert_eq!(data.record().unwrap().machine_id,machine);
        assert_eq!(data.config(|_|None).unwrap().policy.delegated,["machine.roles"]);
        let mut local=program(directory.path());local.args(["policy","set","machine.roles","false"]);succeeds(local).await;
        let before=std::fs::read(data.config_path()).unwrap();
        let denied=controller.issue(&SerialKey::Machine,deadline(),&MachineRolesSet {roles:vec!["provider".into()]});
        let refusal=controller.wait_command(denied).await;
        assert!(matches!(refusal,orch_link::commands::Progress::Failed(ref error) if error.detail.contains("machine.roles")),"{refusal:?}");
        assert_eq!(std::fs::read(data.config_path()).unwrap(),before);
        assert!(terminate(&mut child).await.success());
    }).await.expect("delegated role changes must acknowledge and reconnect within the native protocol budget");
}

#[cfg(unix)]
#[tokio::test]
async fn the_program_enrolls_takes_its_limits_holds_a_session_hosts_a_turn_and_stops_when_told() {
    let began = std::time::Instant::now();
    let (system, enrollment_port) = start_system().await;
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();

    // Enrolling: a token and an address, and nothing else.
    let mut enroll = program(data.path());
    enroll
        .args([
            "enroll",
            "--url",
            &format!("https://127.0.0.1:{enrollment_port}"),
        ])
        .args(["--token", &system.token(), "--name", "by-hand"]);
    let said = succeeds(enroll).await;
    step(
        began,
        &format!("enroll: {}", said.lines().next().unwrap_or_default()),
    );
    assert!(said.contains("Enrolled by-hand"), "{said}");
    let (machine_id, controller) = system.machine();
    assert!(said.contains(&machine_id.to_string()));

    // A supplied token is checked remotely, without letting it replace an unrelated saved identity.
    let saved = orch_connector_core::enrollment::DataDir::open(data.path()).unwrap();
    let certificate = saved.identity().unwrap().certificate.pem().to_owned();
    let record = std::fs::read(
        saved
            .identity_dir()
            .join(orch_connector_core::enrollment::RECORD_FILE),
    )
    .unwrap();
    let mut again = program(data.path());
    again
        .args([
            "enroll",
            "--url",
            &format!("https://127.0.0.1:{enrollment_port}"),
        ])
        .args(["--token", &system.token()]);
    let refused = again.stdin(Stdio::null()).output().await.unwrap();
    assert!(!refused.status.success());
    let error = String::from_utf8_lossy(&refused.stderr);
    assert!(error.contains("This token enrolled another machine. The existing identity was kept; use Enroll again on this machine's page."), "{error}");
    assert_eq!(saved.identity().unwrap().certificate.pem(), certificate);
    assert_eq!(
        std::fs::read(
            saved
                .identity_dir()
                .join(orch_connector_core::enrollment::RECORD_FILE)
        )
        .unwrap(),
        record
    );
    step(
        began,
        "a second enrollment into the same directory is refused",
    );

    // The owner's limits: the harness may work where the test's directory is, and the system may change
    // the roots later.
    let roots = std::env::temp_dir();
    let mut set = program(data.path());
    set.args([
        "policy",
        "set",
        "environment.roots",
        &roots.display().to_string(),
    ]);
    let shown = succeeds(set).await;
    assert!(shown.contains("environment.roots"), "{shown}");
    let mut delegate = program(data.path());
    delegate.args(["policy", "delegate", "environment.roots"]);
    let delegated = succeeds(delegate).await;
    assert!(
        !delegated.contains("environment.roots *"),
        "a locally kept root is still kept: {delegated}"
    );
    let saved = orch_connector_core::enrollment::DataDir::open(data.path()).unwrap();
    let policy =
        orch_connector_core::policy::PolicyStore::connected(saved.config(|_| None).unwrap(), None)
            .unwrap()
            .policy()
            .unwrap();
    assert_eq!(policy.policy_delegated, ["environment.roots"]);
    assert_eq!(policy.environment_roots, [roots.display().to_string()]);
    assert!(policy.local_values.contains_key("environment.roots"));
    assert!(!policy.delegates(orch_link_contract::policy::PolicyKey::EnvironmentRoots));
    let mut status = program(data.path());
    status.arg("status");
    let status = succeeds(status).await;
    step(began, "policy set, delegate and status done");
    assert!(status.contains("by-hand"), "{status}");
    assert!(
        status.contains("Adapters     claude-code, codex, dsh, fake"),
        "{status}"
    );
    assert!(
        status.contains(&format!("wss://127.0.0.1:{} (pinned)", system.gateway_port)),
        "{status}"
    );

    // Running: the program holds the session the hub sees.
    let log = std::fs::File::create(data.path().join("logs").join("run.log")).unwrap();
    let mut run = program(data.path());
    run.arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .kill_on_drop(true);
    let mut child = run.spawn().unwrap();
    step(began, "run started");
    let up = eventually(|| system.hub.session_of(machine_id).is_some()).await;
    assert!(
        up,
        "no session; the program said:\n{}",
        std::fs::read_to_string(data.path().join("logs").join("run.log")).unwrap_or_default()
    );

    step(began, "the hub holds the session");
    // It says what it offers: the harness kinds it can host, and the owner's limits.
    let facts_in = controller
        .wait_until(PATIENCE, |c| {
            c.applied("machine")
                .iter()
                .any(|e| e.kind == "machine.facts")
        })
        .await;
    assert!(facts_in);
    step(began, "machine.facts arrived");
    let facts = controller
        .applied("machine")
        .into_iter()
        .find(|e| e.kind == "machine.facts")
        .unwrap()
        .body;
    let kinds: Vec<_> = facts["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|adapter| adapter["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["claude-code", "codex", "dsh", "fake"]);
    assert_eq!(
        facts["policy"]["environment.roots"],
        json!([roots.display().to_string()])
    );
    assert_eq!(
        facts["policy"]["policy.delegated"],
        json!(["environment.roots"])
    );

    // A turn of the scripted harness, as the system would send it.
    let (provider, agent, thread, segment) = (new_id(), new_id(), new_id(), new_id());
    let started: AgentStart = serde_json::from_value(json!({
        "provider_id": provider, "agent_id": agent, "harness_kind": "fake",
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": null
    }))
    .unwrap();
    let key = SerialKey::Harness {
        provider_id: provider,
        agent_id: agent,
    };
    let answered = command(&controller, &key, &started).await;
    step(began, "agent.start answered");
    assert_eq!(answered["descriptor"]["kind"], "fake");
    let open: SessionOpen = serde_json::from_value(json!({
        "agent_id": agent, "thread_id": thread, "segment_id": segment, "mode": "new",
        "native_session_ref": null, "archive": null, "history": null, "upto_turn_ordinal": null,
        "workspace_root": work.path(), "instruction_root_marker": false,
        "directory": work.path(), "cwd": work.path(), "mounts": [],
        "model": "fake-large", "effort": "medium", "params": {},
        "approval_mode": "on_request", "sandbox_mode": "workspace",
        "instructions": "You are working inside Orch.", "memory": null, "arrangement": "together",
        "provider": { "path": "native", "provider_id": provider },
        "exec": { "executor": null, "environment_id": new_id(), "generation": 1 },
        "mcp": [], "orch_tools": [], "raw_events": false
    }))
    .unwrap();
    let thread_key = SerialKey::Thread(thread);
    command(&controller, &thread_key, &open).await;
    let turn: TurnStart = serde_json::from_value(json!({
        "thread_id": thread, "turn_id": new_id(), "text": "by hand", "images": [],
        "model": "fake-large", "effort": "high", "params": {},
        "approval_mode": "on_request", "sandbox_mode": "workspace"
    }))
    .unwrap();
    command(&controller, &thread_key, &turn).await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    }
    .to_string();
    let ended = controller
        .wait_until(PATIENCE, |c| {
            c.applied(&stream)
                .iter()
                .any(|e| e.kind == "turn.completed")
        })
        .await;
    assert!(ended);
    step(began, "the turn ended");
    let events = controller.applied(&stream);
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.len() >= 9, "{kinds:?}");
    assert_eq!(
        &kinds[..5],
        [
            "session.opened",
            "turn.started",
            "model.request",
            "item.started",
            "model.request"
        ]
    );
    // Adjacent text deltas may be coalesced by the transport; the completed text is invariant.
    let delta_end = kinds.len() - 3;
    assert!(!kinds[5..delta_end].is_empty());
    assert!(kinds[5..delta_end].iter().all(|kind| *kind == "item.delta"));
    assert_eq!(
        &kinds[delta_end..],
        ["item.completed", "model.request", "turn.completed"]
    );
    let reports: [orch_link_contract::events::ModelRequest; 3] = [2, 4, delta_end + 1]
        .map(|index| serde_json::from_value(events[index].body.clone()).unwrap());
    for report in &reports {
        assert_eq!(report.turn_id, turn.turn_id);
        assert_eq!(report.request.id, reports[0].request.id);
        assert_eq!(report.request.model.as_deref(), Some("fake-large"));
        report.request.check().unwrap();
    }
    assert!(reports[0].request.asked.is_some());
    assert!(reports[0].request.first_output.is_none());
    assert!(reports[0].request.outcome.is_none());
    assert!(reports[1].request.first_output.is_some());
    assert!(reports[1].request.outcome.is_none());
    assert!(reports[2].request.end.is_some());
    assert_eq!(
        reports[2]
            .request
            .outcome
            .as_ref()
            .map(|outcome| outcome.code()),
        Some("done")
    );
    assert_eq!(
        events[delta_end].body["payload"]["text"],
        "You said: by hand"
    );
    assert!(
        controller.violations().is_empty(),
        "{:?}",
        controller.violations()
    );
    command(
        &controller,
        &thread_key,
        &SessionClose { thread_id: thread },
    )
    .await;

    // Told to stop, it ends the session and exits well.
    let status = terminate(&mut child).await;
    step(began, &format!("the program stopped: {status}"));
    assert!(status.success(), "{status:?}");
    let gone = eventually(|| system.hub.session_of(machine_id).is_none()).await;
    assert!(gone, "the session outlived the program");
    step(began, "the hub has no session");
}

// ---------------------------------------------------------------- a key given to the machine

/// Runs the program to its end with `input` on its standard input, and returns what it did.
async fn runs_with_input(mut command: Command, input: &str) -> std::process::Output {
    use tokio::io::AsyncWriteExt;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).await.unwrap();
    drop(stdin);
    child.wait_with_output().await.unwrap()
}

#[tokio::test]
async fn a_key_given_to_the_program_is_kept_sealed_on_the_machine_and_is_shown_and_sent_nowhere() {
    use orch_connector_core::credentials::{SecretStore, local_key_name};
    use orch_connector_core::enrollment::DataDir;
    use orch_connector_core::roles::Keys;
    let (system, enrollment_port) = start_system().await;
    let data = tempfile::tempdir().unwrap();
    let mut enroll = program(data.path());
    enroll
        .args([
            "enroll",
            "--url",
            &format!("https://127.0.0.1:{enrollment_port}"),
        ])
        .args(["--token", &system.token(), "--name", "key-keeper"]);
    succeeds(enroll).await;
    let provider = new_id();
    let key = "sk-orch-kept-on-this-machine-5f21b8c3";

    // The key comes on standard input, with the newline a shell adds, and is kept for the provider.
    let mut set = program(data.path());
    set.args(["provider", "set-key", "--provider", &provider.to_string()]);
    let done = runs_with_input(set, &format!("{key}\n")).await;
    let said = String::from_utf8_lossy(&done.stdout).into_owned();
    assert!(
        done.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&done.stderr)
    );
    assert!(
        said.contains(&provider.to_string()) && said.contains("not sent"),
        "{said}"
    );
    assert!(
        !said.contains(key) && !String::from_utf8_lossy(&done.stderr).contains(key),
        "the key is not shown"
    );

    // The file does not hold the key; the machine's own key opens it, to the key without the newline.
    let file = data
        .path()
        .join("secrets")
        .join(format!("provider-{provider}.key"));
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(!on_disk.contains(key), "the store holds the key sealed");
    let identity = DataDir::open(data.path()).unwrap().identity().unwrap();
    let store = SecretStore::open(data.path(), Arc::new(Keys::new(identity.key))).unwrap();
    assert_eq!(
        store.get(&local_key_name(provider)).unwrap().as_deref(),
        Some(key)
    );

    // A key kept again replaces the one before.
    let mut again = program(data.path());
    again.args(["provider", "set-key", "--provider", &provider.to_string()]);
    assert!(
        runs_with_input(again, "sk-the-next-one")
            .await
            .status
            .success()
    );
    assert_eq!(
        store.get(&local_key_name(provider)).unwrap().as_deref(),
        Some("sk-the-next-one")
    );

    // What is not a key, or not a provider, is refused, and nothing is kept.
    let mut empty = program(data.path());
    empty.args(["provider", "set-key", "--provider", &new_id().to_string()]);
    let refused = runs_with_input(empty, "\n").await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("no key was given"));
    let mut nameless = program(data.path());
    nameless.args(["provider", "set-key", "--provider", "my-provider"]);
    let refused = runs_with_input(nameless, key).await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("not a provider's identifier"));
    assert_eq!(
        store.names().unwrap().len(),
        1,
        "only the one key is kept: {:?}",
        store.names()
    );

    // A directory that is not enrolled has no key of the machine to seal with, and is left as it was.
    let empty_dir = tempfile::tempdir().unwrap();
    let mut elsewhere = program(empty_dir.path());
    elsewhere.args(["provider", "set-key", "--provider", &provider.to_string()]);
    let refused = runs_with_input(elsewhere, key).await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("holds no identity"));
    assert_eq!(std::fs::read_dir(empty_dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn local_http_enrollment_finishes_and_its_machine_uses_the_pinned_tls_gateway() {
    let (system, _tls_enrollment_port) = start_system().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = Arc::clone(&system);
    let local_enrollment = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let server = Arc::clone(&server);
                    async move { Ok::<_, Infallible>(answer(&server, request).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    for host in ["127.0.0.1", "localhost"] {
        let directory = tempfile::tempdir().unwrap();
        let mut enroll = program(directory.path());
        enroll.args([
            "enroll",
            "--url",
            &format!("http://{host}:{port}"),
            "--token",
            &system.token(),
            "--name",
            "Local development machine",
        ]);
        assert!(succeeds(enroll).await.contains("Enrolled"));
        let (machine, _controller) = system.machine();
        let mut child = run_program(directory.path());
        assert!(
            eventually(|| system.hub.session_of(machine).is_some()).await,
            "a local HTTP enrollment still joins the authenticated TLS gateway"
        );
        let stopped = terminate(&mut child).await;
        assert!(
            stopped.success(),
            "{stopped}: {}",
            std::fs::read_to_string(directory.path().join("programs-test.log")).unwrap()
        );
        assert!(eventually(|| system.hub.session_of(machine).is_none()).await);
    }
    local_enrollment.abort();
}
