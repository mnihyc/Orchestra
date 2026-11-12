//! Release observations use the packaged executable over the process fixture's real loopback sockets.
use super::*;
use anyhow::{Context, Result, ensure};
use orch_api_contract::machine_programs::{
    MachineProgramKind, MachineProgramRegistration, MachineProgramsCommand,
};
use orch_link::framing::{Message, PeerInfo};
use orch_link_contract::commands::{AgentStart, CommandBody, SerialKey};
use orch_link_contract::enrollment::{RENEW_PATH, RenewReply, RenewRequest};
use orch_pki::certificates::{Certificate, CertificateRequest, Identity};
use orch_pki::enrollment::MachineIdentity;
use orch_pki::renewal::{MachineRecord, RenewalPolicy};
use orch_pki::verification::presented_certificate;
use orch_types::{Json, ManualClock, Time};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};

// A native release observation may take as long as the existing process fixture's real deadline.
const WITHIN: Duration = PATIENCE;

struct Observed {
    system: Arc<System>,
    evidence: Mutex<Vec<Json>>,
    fingerprints: Mutex<BTreeMap<Id, Vec<String>>>,
    revoked: Mutex<BTreeSet<Id>>,
}
impl Observed {
    fn note(&self, mut value: Json) {
        value["at"] = json!(self.system.clock.time());
        self.evidence.lock().unwrap().push(value);
    }
    fn has(&self, kind: &str, fingerprint: &str) -> bool {
        self.evidence
            .lock()
            .unwrap()
            .iter()
            .any(|line| line["event"] == kind && line["fingerprint"] == fingerprint)
    }
}

async fn carry_observed(
    observed: Arc<Observed>,
    fingerprint: String,
    mut from: Box<dyn MessageSource>,
    mut to: Box<dyn MessageSink>,
) {
    while let Ok(Some(message)) = from.recv().await {
        if let Message::Text(text) = &message
            && let Ok(frame) = serde_json::from_str::<Json>(text)
        {
            match frame["t"].as_str() {
                Some("ready") => observed.note(json!({"event":"session_ready", "fingerprint":fingerprint, "session_id":frame["session_id"]})),
                Some("reject") => observed.note(json!({"event":"session_rejected", "fingerprint":fingerprint, "code":frame["code"]})),
                _ => {}
            }
        }
        if to.send(message).await.is_err() {
            break;
        }
    }
}

async fn renewal(
    observed: &Observed,
    presented: Option<&Certificate>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>> {
    let certificate = presented.context("renewal did not present its TLS certificate")?;
    ensure!(
        request.method() == Method::POST && request.uri().path() == RENEW_PATH,
        "unexpected renewal route"
    );
    let [Identity::Machine(machine)] = certificate.identities() else {
        anyhow::bail!("renewal did not identify a machine");
    };
    let body = request.into_body().collect().await?.to_bytes();
    let asked: RenewRequest = serde_json::from_slice(&body)?;
    let csr = CertificateRequest::from_pem(&asked.csr_pem)?;
    let record = MachineRecord {
        active: !observed.revoked.lock().unwrap().contains(machine),
        fingerprints: observed
            .fingerprints
            .lock()
            .unwrap()
            .get(machine)
            .cloned()
            .unwrap_or_default(),
    };
    let renewed = orch_pki::renewal::renew(
        observed.system.pki.authority(),
        certificate,
        Some(&record),
        &csr,
        &RenewalPolicy {
            validity_days: 365,
            grace_days: 90,
            service_names: Vec::new(),
        },
        observed.system.clock.as_ref(),
    );
    match renewed {
        Ok(renewed) => {
            observed
                .fingerprints
                .lock()
                .unwrap()
                .entry(*machine)
                .or_default()
                .push(renewed.fingerprint());
            observed.note(json!({"event":"renewal_issued", "machine_id":machine,
                "fingerprint":renewed.fingerprint(), "previous_fingerprint":certificate.fingerprint(),
                "new_public_key":renewed.public_point()!=certificate.public_point()}));
            let reply = RenewReply {
                certificate_pem: renewed.pem(),
                ca_pem: observed.system.pki.authority().certificate_pem(),
            };
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(serde_json::to_vec(&reply)?)))?)
        }
        Err(_) => {
            observed.note(json!({"event":"renewal_refused","machine_id":machine,"fingerprint":certificate.fingerprint()}));
            Ok(Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(Full::new(Bytes::new()))?)
        }
    }
}

/// The existing pinned gateway socket also serves the production renewal payload and PKI check.
async fn gateway(observed: Arc<Observed>, listener: TcpListener) {
    let tls = pinned_server_config(
        &observed.system.server_identity(),
        &observed.system.pki.certificate(),
        90,
        Arc::clone(&observed.system.clock),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(tls);
    loop {
        let Ok((socket, address)) = listener.accept().await else {
            return;
        };
        let (acceptor, observed) = (acceptor.clone(), Arc::clone(&observed));
        tokio::spawn(async move {
            let Ok(stream) = acceptor.accept(socket).await else {
                return;
            };
            let certificate = presented_certificate(stream.get_ref().1.peer_certificates());
            let peer = PeerInfo {
                remote_ip: Some(address.ip()),
                host: stream.get_ref().1.server_name().map(str::to_owned),
                client_certificate: certificate.clone(),
                ..PeerInfo::default()
            };
            let mut stream = BufReader::new(stream);
            let Ok(prefix) = stream.fill_buf().await else {
                return;
            };
            if prefix.first() == Some(&b'P') {
                let service = service_fn(move |request| {
                    let (observed, certificate) = (Arc::clone(&observed), certificate.clone());
                    async move {
                        let reply = renewal(&observed, certificate.as_ref(), request)
                            .await
                            .unwrap_or_else(|_| {
                                Response::builder()
                                    .status(StatusCode::BAD_REQUEST)
                                    .body(Full::new(Bytes::new()))
                                    .unwrap()
                            });
                        Ok::<_, Infallible>(reply)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
                return;
            }
            let Ok(outside) = orch_link::websocket::accept(stream, peer).await else {
                return;
            };
            let fingerprint = certificate
                .as_ref()
                .map(Certificate::fingerprint)
                .unwrap_or_default();
            let Ok(inside) = observed
                .system
                .hub
                .dialer(certificate)
                .dial(&Hub::endpoint())
                .await
            else {
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
            carry_observed(observed, fingerprint, inside_source, outside_sink).await;
        });
    }
}

async fn endpoint() -> Result<(Arc<Observed>, u16)> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let gateway_socket = TcpListener::bind("127.0.0.1:0").await?;
    let enrollment = TcpListener::bind("127.0.0.1:0").await?;
    let port = enrollment.local_addr()?.port();
    let system = Arc::new(System {
        clock,
        pki,
        hub,
        ledger: MemoryLedger::new(),
        gateway_port: gateway_socket.local_addr()?.port(),
        enrolled: Mutex::new(None),
    });
    let observed = Arc::new(Observed {
        system: Arc::clone(&system),
        evidence: Mutex::new(Vec::new()),
        fingerprints: Mutex::new(BTreeMap::new()),
        revoked: Mutex::new(BTreeSet::new()),
    });
    tokio::spawn(gateway(Arc::clone(&observed), gateway_socket));
    tokio::spawn(serve_enrollment(system, enrollment));
    Ok((observed, port))
}

fn candidate(directory: &Path) -> Result<Command> {
    let binary = std::env::var_os("ORCH_PLATFORM_CONNECTOR")
        .context("the packaged candidate connector is required")?;
    let mut command = Command::new(binary);
    command
        .arg("--data-dir")
        .arg(directory)
        .env_remove("ORCH_DATA_DIR")
        .env_remove("ORCH_PLUGINS")
        .stdin(Stdio::null());
    Ok(command)
}
async fn cli(directory: &Path, args: &[&str]) -> Result<()> {
    let output = tokio::time::timeout(WITHIN, candidate(directory)?.args(args).output()).await??;
    ensure!(
        output.status.success(),
        "candidate {} exited {}",
        args.first().unwrap_or(&"command"),
        output.status
    );
    Ok(())
}
fn running(directory: &Path) -> Result<tokio::process::Child> {
    Ok(candidate(directory)?
        .arg("run")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?)
}
async fn wait(mut ready: impl FnMut() -> bool) -> Result<()> {
    let began = Instant::now();
    while began.elapsed() < WITHIN {
        if ready() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    ensure!(
        ready(),
        "the native observation did not arrive within the process fixture deadline"
    );
    Ok(())
}
async fn issue<B: CommandBody>(
    controller: &ScriptedController,
    key: &SerialKey,
    body: &B,
) -> Result<Json> {
    let id = controller.issue(
        key,
        Time::new(chrono::Utc::now() + chrono::Duration::from_std(WITHIN)?),
        body,
    );
    match tokio::time::timeout(WITHIN, controller.wait_command(id)).await? {
        orch_link::commands::Progress::Succeeded(value) => Ok(value),
        _ => anyhow::bail!("the native command did not succeed"),
    }
}
async fn enrolled(
    observed: &Observed,
    port: u16,
    directory: &Path,
    name: &str,
) -> Result<(MachineIdentity, Arc<ScriptedController>)> {
    cli(
        directory,
        &[
            "enroll",
            "--url",
            &format!("https://127.0.0.1:{port}"),
            "--token",
            &observed.system.token(),
            "--name",
            name,
            "--roles",
            "agent",
        ],
    )
    .await?;
    let identity = MachineIdentity::load(&directory.join("identity"))?;
    let (_, controller) = observed
        .system
        .enrolled
        .lock()
        .unwrap()
        .clone()
        .context("the endpoint did not enroll the candidate")?;
    observed.fingerprints.lock().unwrap().insert(
        identity.machine_id,
        vec![identity.certificate.fingerprint()],
    );
    Ok((identity, controller))
}

fn scripted_program(directory: &Path) -> Result<Vec<String>> {
    let descriptor = json!({"kind":"platform-observed","adapter_version":1,
        "harness":{"name":"Platform observed","version":"1.0.0","supported_range":">=1.0.0","status":"supported"},
        "arrangements":[{"type":"together"}],"approval_modes":["on_request"],"sandbox_modes":["open"]});
    let mut responses = json!({"describe":{"result":descriptor},"agent.start":{"result":{"harness_version":"1.0.0","descriptor":descriptor}},"agent.stop":{"result":{}}});
    // Required transport methods are declared by the same conformance program used by the process tests.
    let mut methods: Vec<_> =
        orch_plugin_contract::negotiation::methods(orch_plugin_contract::PluginKind::Adapter)
            .iter()
            .filter(|m| m.required)
            .map(|m| m.name.to_owned())
            .collect();
    methods.extend(
        responses
            .as_object_mut()
            .context("scripted replies are an object")?
            .keys()
            .cloned(),
    );
    methods.sort();
    methods.dedup();
    let script = directory.join("scripted-program.json");
    std::fs::write(
        &script,
        serde_json::to_vec(
            &json!({"kind":"adapter","name":"platform-observed","version":"1.0.0","methods":methods,"descriptor":descriptor,"responses":responses}),
        )?,
    )?;
    Ok(vec![
        std::env::var("ORCH_PLATFORM_PYTHON")
            .context("the native Python executable is required")?,
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/platform/plugin/tests/support/scripted_plugin.py")
            .display()
            .to_string(),
        "--script".into(),
        script.display().to_string(),
        "--log".into(),
        directory.join("scripted-program.log").display().to_string(),
    ])
}

async fn observe(report: &mut Json) -> Result<()> {
    let (observed, port) = endpoint().await?;
    let directory = tempfile::tempdir()?;
    let began = Instant::now();
    let (identity, controller) =
        enrolled(&observed, port, directory.path(), "platform-attachment").await?;
    cli(directory.path(), &["policy", "delegate", "all"]).await?;
    let mut child = running(directory.path())?;
    let fingerprint = identity.certificate.fingerprint();
    wait(|| observed.has("session_ready", &fingerprint))
        .await
        .context("sign-in")?;
    let signed_in = began.elapsed().as_millis();
    let registered = issue(
        &controller,
        &SerialKey::Machine,
        &MachineProgramsCommand::Register(MachineProgramRegistration {
            kind: MachineProgramKind::Agent,
            name: "platform-observed".into(),
            command: scripted_program(directory.path())?,
        }),
    )
    .await?;
    ensure!(
        registered["programs"]["entries"]
            .as_array()
            .is_some_and(|entries| entries
                .iter()
                .any(|row| row["name"] == "platform-observed" && row["ready"] == true)),
        "the candidate did not report the attached program ready"
    );
    let (provider_id, agent_id) = (new_id(), new_id());
    let start: AgentStart = serde_json::from_value(
        json!({"provider_id":provider_id,"agent_id":agent_id,"harness_kind":"platform-observed","profile":{"config":{},"env":{}},"home":null}),
    )?;
    let started = issue(
        &controller,
        &SerialKey::Harness {
            provider_id,
            agent_id,
        },
        &start,
    )
    .await?;
    ensure!(
        started["descriptor"]["harness"]["name"] == "Platform observed",
        "the candidate did not start the attached program"
    );
    let calls = std::fs::read_to_string(directory.path().join("scripted-program.log"))?;
    ensure!(
        calls
            .lines()
            .filter_map(|line| serde_json::from_str::<Json>(line).ok())
            .any(|line| line["received"]["method"] == "agent.start"),
        "the attached process did not receive agent.start"
    );
    let described = registered["programs"]["entries"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|row| row["name"] == "platform-observed")
        })
        .context("the described program disappeared from its acknowledgement")?;
    report["observations"][0] = json!({"name":"connected_program_attachment","status":"passed","elapsed_ms":began.elapsed().as_millis(),"evidence":[
        {"event":"enrolled","machine_id":identity.machine_id,"fingerprint":fingerprint},
        {"event":"scripted_program_described","ready":described["ready"],"version":described["version"],"reported_name":described["descriptor"]["harness"]["name"]},
        {"event":"scripted_program_started","method":"agent.start","harness_version":started["harness_version"],"reported_name":started["descriptor"]["harness"]["name"]}]});
    observed.revoked.lock().unwrap().insert(identity.machine_id);
    observed.system.hub.revoke(identity.machine_id);
    // Restart the exact old pair, so refusal is observed at authentication, beyond a dropped session.
    let _ = child.kill().await;
    let _ = child.wait().await;
    let mut old = running(directory.path())?;
    wait(|| observed.has("session_rejected", &fingerprint))
        .await
        .context("revoked credential refusal")?;
    ensure!(
        observed
            .evidence
            .lock()
            .unwrap()
            .iter()
            .any(|row| row["event"] == "session_rejected"
                && row["fingerprint"] == fingerprint
                && row["code"] == "revoked"),
        "the native rejection did not name revocation"
    );
    let revoked_at = began.elapsed().as_millis();
    let _ = old.kill().await;
    let _ = old.wait().await;

    // Renewal is a separate eligible identity: revocation is never undone to make renewal pass.
    let renewal_directory = tempfile::tempdir()?;
    let (mut renewal_identity, _) = enrolled(
        &observed,
        port,
        renewal_directory.path(),
        "platform-renewal",
    )
    .await?;
    // A certificate in the final third of its native 365-day life exercises automatic startup renewal.
    let aged = ManualClock::new(chrono::Utc::now() - chrono::Duration::days(250));
    let csr = CertificateRequest::from_pem(&renewal_identity.key.request_pem()?)?;
    renewal_identity.certificate = observed.system.pki.authority().issue(
        &csr,
        &Holder::Machine(renewal_identity.machine_id),
        365,
        &aged,
    )?;
    renewal_identity.save(&renewal_directory.path().join("identity"))?;
    let before = renewal_identity.certificate.fingerprint();
    observed
        .fingerprints
        .lock()
        .unwrap()
        .insert(renewal_identity.machine_id, vec![before.clone()]);
    ensure!(
        orch_pki::renewal::due(&renewal_identity.certificate, observed.system.clock.time()),
        "the native renewal premise is not due"
    );
    let mut renewed = running(renewal_directory.path())?;
    wait(|| {
        MachineIdentity::load(&renewal_directory.path().join("identity"))
            .ok()
            .is_some_and(|now| {
                now.certificate.fingerprint() != before
                    && observed.has("session_ready", &now.certificate.fingerprint())
            })
    })
    .await
    .context("automatic renewal and renewed sign-in")?;
    let now = MachineIdentity::load(&renewal_directory.path().join("identity"))?;
    ensure!(
        now.machine_id == renewal_identity.machine_id
            && now.certificate.public_point() != renewal_identity.certificate.public_point(),
        "renewal did not retain the machine over a new key"
    );
    ensure!(
        observed.has("renewal_issued", &now.certificate.fingerprint()),
        "the candidate did not use the native HTTPS renewal"
    );
    let _ = renewed.kill().await;
    let _ = renewed.wait().await;
    report["observations"][1] = json!({"name":"connector_credential_lifecycle","status":"passed","elapsed_ms":began.elapsed().as_millis(),
        "sign_in_ms":signed_in,"revocation_refused_ms":revoked_at,
        "revoked_machine_id":identity.machine_id,"renewed_machine_id":now.machine_id,
        "evidence":observed.evidence.lock().unwrap().clone()});
    Ok(())
}

/// The release script always selects this ignored case explicitly and requires its two result records.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release observation requires the packaged candidate executable"]
async fn observe_candidate_platform() {
    let path = PathBuf::from(
        std::env::var_os("ORCH_PLATFORM_REPORT").expect("a native evidence path is required"),
    );
    let mut report = json!({"native_os":std::env::consts::OS,"native_arch":std::env::consts::ARCH,
        "observations":[{"name":"connected_program_attachment","status":"failed","reason":"the native observation did not complete"},
        {"name":"connector_credential_lifecycle","status":"failed","reason":"the native observation did not complete"}]});
    let result = observe(&mut report).await;
    if let Err(error) = &result {
        for row in report["observations"].as_array_mut().unwrap() {
            if row["status"] != "passed" {
                row["reason"] = json!(format!("{error:#}"));
            }
        }
    }
    std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    result.expect("a native platform observation failed");
}
