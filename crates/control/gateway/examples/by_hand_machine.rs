//! A machine for trying a running deployment by hand.
//!
//! `enroll` presents a token to a gateway's pinned port, trusting nothing but
//! the pin the token carries, and keeps what the system gives back in a
//! directory. `hold` connects with that identity as a connector would, says
//! what the machine is, answers the commands it is sent as a scripted machine
//! does (it runs nothing), and reports every change of its link until the
//! time it was given is up.
//!
//! ```text
//! by_hand_machine enroll --token-file FILE --address 127.0.0.1:PORT --dir DIR [--name NAME]
//! by_hand_machine hold --dir DIR [--seconds N] [--trust pinned|public]
//! ```

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use orch_link::commands::CommandHandler;
use orch_link::machine::{LinkStatus, MachineConfig, MachineLink};
use orch_link::spool::SpoolOptions;
use orch_link::tunnels::channel::TunnelOptions;
use orch_link::websocket::TlsDialer;
use orch_link_contract::enrollment::{
    ENROLL_PATH, Endpoint, EnrollReply, EnrollRequest, EnrollmentToken, Trust,
};
use orch_link_contract::events::Event;
use orch_link_contract::session::{Arch, MachineFacts, Os, Platform, Role};
use orch_link_contract::streams::Stream;
use orch_pki::certificates::ServiceName;
use orch_pki::enrollment::{MachineIdentity, PendingEnrollment};
use orch_pki::verification::{ServerTrust, pinned_client_config, public_client_config, root_store};
use orch_testkit::doubles::machine::ScriptedHandler;
use orch_types::{Clock, SystemClock};
use rustls::pki_types::ServerName;
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

type Failure = Box<dyn Error>;

const ENDPOINTS_FILE: &str = "endpoints.json";
/// How long `hold` holds when it is not told, in seconds: long enough to see
/// the link come up and a waiting command run.
const DEFAULT_HOLD_SECS: &str = "20";
/// How often `hold` looks for commands that ran: they do not change the
/// link's status.
const LOOK_EVERY: Duration = Duration::from_millis(500);
/// What the spool of the trial machine may hold: a trial sends a few events.
const SPOOL_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// How long the spool keeps what has not been taken.
const SPOOL_MAX_AGE: Duration = Duration::from_secs(7 * 86_400);
/// The size of one file of the spool.
const SPOOL_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
/// How long the journal remembers a command, so that one sent again is not
/// run again.
const JOURNAL_KEEP: Duration = Duration::from_secs(24 * 3_600);
const PLATFORM: Platform = Platform {
    os: Os::Linux,
    arch: Arch::X86_64,
};
const VERSION: &str = "0.1.0";

#[tokio::main]
async fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments.first().map(String::as_str) {
        Some("enroll") => enroll(&Flags::parse(&arguments[1..])).await,
        Some("hold") => hold(&Flags::parse(&arguments[1..])).await,
        _ => Err("usage: by_hand_machine enroll|hold --dir DIR [...]".into()),
    };
    if let Err(error) = outcome {
        eprintln!("by_hand_machine: {error}");
        std::process::exit(1);
    }
}

/// `--name value` pairs.
struct Flags(Vec<(String, String)>);

impl Flags {
    fn parse(arguments: &[String]) -> Self {
        Self(
            arguments
                .chunks(2)
                .filter_map(|pair| match pair {
                    [name, value] => {
                        Some((name.trim_start_matches("--").to_owned(), value.clone()))
                    }
                    _ => None,
                })
                .collect(),
        )
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn need(&self, name: &str) -> Result<&str, Failure> {
        self.get(name)
            .ok_or_else(|| format!("--{name} is needed").into())
    }
}

async fn enroll(flags: &Flags) -> Result<(), Failure> {
    let token_text = std::fs::read_to_string(flags.need("token-file")?)?
        .trim()
        .to_owned();
    let address: SocketAddr = flags.need("address")?.parse()?;
    let dir = PathBuf::from(flags.need("dir")?);
    let token: EnrollmentToken = token_text.parse()?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let pending = PendingEnrollment::start()?;
    let request = EnrollRequest {
        machine_id: None,
        token: token_text,
        csr_pem: pending.csr_pem().to_owned(),
        name: flags.get("name").unwrap_or("by-hand").to_owned(),
        platform: PLATFORM,
        connector_version: VERSION.to_owned(),
        protocol_versions: vec![1],
        roles: vec![Role::Agent, Role::Environment],
    };
    let tls = pinned_client_config(
        ServerTrust::Pin(*token.pin()),
        Some(ServiceName::Gateway),
        None,
        Arc::clone(&clock),
    )?;
    let (status, body) =
        post_json(address, tls, ENROLL_PATH, &serde_json::to_string(&request)?).await?;
    if status != 201 {
        return Err(format!(
            "the system answered {status}: {}",
            String::from_utf8_lossy(&body)
        )
        .into());
    }
    let reply: EnrollReply = serde_json::from_slice(&body)?;
    let identity = pending.complete(&token, &reply, clock.as_ref())?;
    std::fs::create_dir_all(&dir)?;
    identity.save(&dir)?;
    std::fs::write(
        dir.join(ENDPOINTS_FILE),
        serde_json::to_vec_pretty(&reply.endpoints)?,
    )?;
    println!(
        "enrolled as machine {} into {}; endpoints: {}",
        identity.machine_id,
        reply.system.name,
        reply
            .endpoints
            .iter()
            .map(|endpoint| format!("{} ({:?})", endpoint.url, endpoint.trust))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

async fn hold(flags: &Flags) -> Result<(), Failure> {
    let dir = PathBuf::from(flags.need("dir")?);
    let seconds: u64 = flags.get("seconds").unwrap_or(DEFAULT_HOLD_SECS).parse()?;
    let wanted = match flags.get("trust").unwrap_or("pinned") {
        "pinned" => Trust::Pinned,
        "public" => Trust::Public,
        other => return Err(format!("--trust is pinned or public, not {other}").into()),
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let identity = MachineIdentity::load(&dir)?;
    let endpoints: Vec<Endpoint> =
        serde_json::from_slice(&std::fs::read(dir.join(ENDPOINTS_FILE))?)?;
    let endpoints: Vec<Endpoint> = endpoints
        .into_iter()
        .filter(|endpoint| endpoint.trust == wanted)
        .collect();
    if endpoints.is_empty() {
        return Err(format!("the system gave no {wanted:?} endpoint").into());
    }

    let pinned = pinned_client_config(
        ServerTrust::Authority(Box::new(identity.authority.clone())),
        Some(ServiceName::Gateway),
        Some(&identity.tls()?),
        Arc::clone(&clock),
    )?;
    let public = public_client_config(
        root_store(std::slice::from_ref(&identity.authority))?,
        Arc::clone(&clock),
    )?;
    let data_dir = dir.join("link");
    std::fs::create_dir_all(&data_dir)?;
    let link = MachineLink::start(
        MachineConfig {
            hosted_endpoints: None,
            follow_system_endpoints: false,
            machine_id: identity.machine_id,
            connector_version: VERSION.to_owned(),
            platform: PLATFORM,
            roles: vec![Role::Agent, Role::Environment],
            proof: Some((identity.key.duplicate()?, identity.certificate.clone())),
            endpoints,
            data_dir,
            spool: SpoolOptions {
                max_bytes: SPOOL_MAX_BYTES,
                max_age: SPOOL_MAX_AGE,
                segment_bytes: SPOOL_SEGMENT_BYTES,
            },
            journal_keep: JOURNAL_KEEP,
            connected: true,
            tunnels: TunnelOptions::default(),
            tunnel_gauge: None,
        },
        Arc::new(TlsDialer::new(Some(pinned), Some(public))),
        Arc::clone(&clock),
    )?;
    let handler = Arc::new(ScriptedHandler::default());
    link.serve(Arc::clone(&handler) as Arc<dyn CommandHandler>);
    println!("machine {} starts its link", identity.machine_id);

    let mut status = link.status();
    let mut said_facts = false;
    let end = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let mut ran = 0;
    'holding: loop {
        let now = status.borrow_and_update().clone();
        println!("link: {}", describe(&now));
        if matches!(now, LinkStatus::Connected { .. }) && !said_facts {
            said_facts = true;
            link.publish(&Stream::Machine, &Event::MachineFacts(facts()?))
                .await?;
            println!("facts published");
        }
        // The commands that arrive do not change the link's status, so
        // between two changes of it look for them twice a second.
        loop {
            report_runs(&handler, &mut ran);
            let now = tokio::time::Instant::now();
            if now >= end {
                break 'holding;
            }
            let next = (now + LOOK_EVERY).min(end);
            match tokio::time::timeout_at(next, status.changed()).await {
                Err(_) => {}
                Ok(Ok(())) => break,
                Ok(Err(_)) => break 'holding,
            }
        }
    }
    report_runs(&handler, &mut ran);
    link.shutdown().await;
    println!(
        "machine {} stops; {ran} command(s) ran",
        identity.machine_id
    );
    Ok(())
}

/// Says what the scripted handler ran since the last look.
fn report_runs(handler: &ScriptedHandler, seen: &mut usize) {
    let executions = handler.executions();
    for run in executions.iter().skip(*seen) {
        println!("command {} ({}) ran", run.id, run.kind);
    }
    *seen = executions.len();
}

fn describe(status: &LinkStatus) -> String {
    match status {
        LinkStatus::Connected {
            session_id,
            endpoint,
        } => format!("connected, session {session_id}, through {endpoint}"),
        other => format!("{other:?}"),
    }
}

fn facts() -> Result<MachineFacts, Failure> {
    Ok(serde_json::from_value(serde_json::json!({
        "hostname": "by-hand-machine", "os_version": "Debian 13", "cpus": 4,
        "memory_bytes": 8_000_000_000u64, "harnesses": [], "adapters": [], "plugins": [],
        "policy": null,
        "rates": { "up_bps": 1_000_000, "down_bps": 4_000_000, "round_trip_ms": 3 },
        "clock_offset_ms": 0
    }))?)
}

/// One HTTPS request with a JSON body, over a connection of its own.
async fn post_json(
    address: SocketAddr,
    tls: Arc<rustls::ClientConfig>,
    path: &str,
    body: &str,
) -> Result<(u16, Vec<u8>), Failure> {
    let socket = TcpStream::connect(address).await?;
    let name = ServerName::try_from(address.ip().to_string())?;
    let stream = TlsConnector::from(tls).connect(name, socket).await?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream)).await?;
    tokio::spawn(connection);
    let request = hyper::Request::builder()
        .method("POST")
        .uri(path)
        .header("host", address.to_string())
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.to_owned())))?;
    let response = sender.send_request(request).await?;
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await?.to_bytes();
    Ok((status, bytes.to_vec()))
}
