//! The edge in front of the stack, tried from wherever the operator is.
//!
//! What the HTTPS front an operator runs must do is easy to get slightly wrong and shows
//! only under use: a page that comes without its headers, a WebSocket that is
//! cut when idle, a body that is refused at some size, an upgrade that is not
//! passed. Each is tried as a browser or a machine would try it, and the line
//! says what was sent and what came back.

use super::net::{self, Address, Resolve};
use super::{Line, Status};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use orch_link_contract::enrollment::{Endpoint, Trust};
use orch_link_contract::version::SUBPROTOCOL as LINK_SUBPROTOCOL;
use orch_pki::certificates::{Certificate, ServiceName};
use orch_pki::verification::{ServerTrust, machine_client_config};
use orch_realtime_contract::frames::{AuthFrame, ClientFrame};
use orch_realtime_contract::version::{SILENCE_SECS, SUBPROTOCOL as SOCKET_SUBPROTOCOL};
use orch_types::SystemClock;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{self, Message, client::IntoClientRequest};

/// The size of the body that must pass: the most that any one request of the
/// product carries. Larger content is sent in pieces.
pub const BODY_BYTES: usize = 8 * 1024 * 1024;
/// How long a WebSocket must be held without a word from either side: an edge
/// that keeps idle connections for less cuts the product's sockets.
pub const IDLE_SECS: u64 = 60;
/// How much earlier than asked a server may end a held socket and still have
/// held it: its clock and ours do not start together.
const IDLE_SLACK_SECS: u64 = 2;
/// How long the answer to a WebSocket upgrade may take.
const UPGRADE_SECS: u64 = 15;
/// The address the 8 MiB body is sent to: a path of the stack's own web edge that
/// answers 204 after it has read the whole body, so that whatever stands in front
/// of the edge has to carry the body whole to get an answer. The API is no part of
/// it: it closes the connection on a body it will not take, which says nothing
/// about the front.
const BODY_PATH: &str = "/edge-check";
/// The headers every response carries.
const SECURITY_HEADERS: [&str; 5] = [
    "content-security-policy",
    "x-content-type-options",
    "x-frame-options",
    "referrer-policy",
    "permissions-policy",
];
/// How long the API lets a realtime socket stay without signing in: what ends
/// an unsigned socket, whatever the edge does.
const UNSIGNED_SOCKET_SECS: u64 = 10;

/// What the edge checks are given.
pub struct Edge<'a> {
    /// The address of the edge, as the operator wrote it.
    pub address: Address,
    /// Names that resolve elsewhere than the name system says.
    pub resolve: &'a Resolve,
    /// Where the authority's certificate may be read, when this runs where the
    /// stack's secrets are.
    pub secrets_dir: &'a std::path::Path,
    /// An access token of a signed-in browser session, to hold the realtime
    /// socket idle as a person's is.
    pub access_token: Option<&'a str>,
    /// How long the socket is held idle.
    pub idle: Duration,
    /// Which checks are wanted.
    pub wanted: &'a (dyn Fn(&str) -> bool + Sync),
    /// The configuration's `[edge] ports_domain`, where it names one.
    pub ports_domain: Option<String>,
}

/// The cookie a port's stable name keeps its entry in, which is the only one a name of the ports domain
/// may set (the system's own sign-in never reaches it).
const PORT_ENTRY_COOKIE: &str = "__Host-orch-port";

fn said(id: &str, status: Status, said: String) -> Line {
    Line {
        id: id.to_owned(),
        status,
        said,
    }
}

fn fail(id: &str, text: String) -> Line {
    said(id, Status::Fail, text)
}

/// The roots every browser has, or the ones this machine has.
fn public_tls(edge: &Edge<'_>) -> Result<Arc<rustls::ClientConfig>, String> {
    let authority = std::fs::read_to_string(edge.secrets_dir.join("orch_ca_cert.pem"))
        .ok()
        .and_then(|pem| Certificate::from_pem(&pem).ok());
    machine_client_config(
        authority.map(|certificate| ServerTrust::Authority(Box::new(certificate))),
        ServiceName::Frontend,
        None,
        Arc::new(SystemClock),
    )
    .map_err(|error| error.to_string())
}

/// Makes one request to the edge.
async fn fetch(
    edge: &Edge<'_>,
    method: http::Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Result<net::Reply, String> {
    let tls = if edge.address.secure {
        Some(public_tls(edge)?)
    } else {
        None
    };
    let stream = net::connect(&edge.address, edge.resolve, tls.as_ref(), None).await?;
    net::request(
        stream,
        method,
        &edge.address.authority(),
        &edge.address.path(path),
        headers,
        body,
    )
    .await
}

/// Runs the checks that are wanted, all at once, since one of them waits a minute.
pub async fn checks(edge: &Edge<'_>) -> Vec<Line> {
    let one = |id: &'static str| (edge.wanted)(id);
    let (page, api, body, socket, link, endpoints) = tokio::join!(
        async {
            if one("edge-page") {
                vec![page(edge).await]
            } else {
                Vec::new()
            }
        },
        async {
            if one("edge-api") {
                vec![api(edge).await]
            } else {
                Vec::new()
            }
        },
        async {
            if one("edge-body") {
                vec![body(edge).await]
            } else {
                Vec::new()
            }
        },
        async {
            if one("edge-ws") {
                vec![realtime_socket(edge).await]
            } else {
                Vec::new()
            }
        },
        async {
            if one("edge-link") {
                vec![link_upgrade(edge).await]
            } else {
                Vec::new()
            }
        },
        async {
            if one("edge-endpoints") {
                endpoints(edge).await
            } else {
                Vec::new()
            }
        },
    );
    let ports = if one("edge-ports") {
        vec![ports_names(edge).await]
    } else {
        Vec::new()
    };
    [page, api, body, socket, link, endpoints, ports].concat()
}

/// A port's stable name under the ports domain reaches the system and lets nobody in by the name alone: the
/// wildcard name resolves, its certificate holds (on HTTPS), the name of an environment that does not exist is
/// answered by Orch itself with no entry, and nothing of the system's own sign-in is set there. Streaming and
/// the end of an entry by revocation need a real opening, and are not tried here.
async fn ports_names(edge: &Edge<'_>) -> Line {
    const ID: &str = "edge-ports";
    let Some(domain) = edge
        .ports_domain
        .as_deref()
        .map(str::trim)
        .filter(|domain| !domain.is_empty())
    else {
        return said(
            ID,
            Status::Skip,
            "no [edge] ports_domain: ports get no browser names, and the helper needs none"
                .to_owned(),
        );
    };
    // The name of an environment of zeros, which no environment has.
    let name = format!("e{}-p1.{domain}", "0".repeat(32));
    let scheme = if edge.address.secure { "https" } else { "http" };
    let address = match Address::parse(&format!("{scheme}://{name}")) {
        Ok(address) => address,
        Err(error) => return fail(ID, format!("{name} is not an address: {error}")),
    };
    let tls = if address.secure {
        match public_tls(edge) {
            Ok(tls) => Some(tls),
            Err(error) => return fail(ID, error),
        }
    } else {
        None
    };
    let stream = match net::connect(&address, edge.resolve, tls.as_ref(), None).await {
        Ok(stream) => stream,
        Err(error) => {
            return fail(
                ID,
                format!(
                    "{name} does not answer: {error}; the wildcard name, or a certificate that covers it, is missing"
                ),
            );
        }
    };
    let reply = match net::request(
        stream,
        http::Method::GET,
        &address.authority(),
        &address.path("/"),
        &[],
        Bytes::new(),
    )
    .await
    {
        Ok(reply) => reply,
        Err(error) => return fail(ID, format!("GET / at {name} failed: {error}")),
    };
    if let Some(cookie) = reply.header("set-cookie")
        && !cookie.starts_with(PORT_ENTRY_COOKIE)
    {
        return fail(
            ID,
            format!(
                "GET / at {name} sets a cookie that is not a port's own entry: the name shares the system's sign-in"
            ),
        );
    }
    match reply.status {
        401 | 404 => said(
            ID,
            Status::Part,
            format!(
                "{name} reaches the system and its certificate holds; a name without an entry is answered {} and sets no sign-in. Streaming and revocation need a real opening and are not tried here",
                reply.status
            ),
        ),
        other => fail(
            ID,
            format!(
                "GET / at {name} -> {other}; Orch's own answer to a name without an entry was expected (401 or 404): the wildcard name does not reach the system's web edge"
            ),
        ),
    }
}

/// A page, with the headers every response must carry.
async fn page(edge: &Edge<'_>) -> Line {
    const ID: &str = "edge-page";
    let reply = match fetch(edge, http::Method::GET, "/", &[], Bytes::new()).await {
        Ok(reply) => reply,
        Err(error) => return fail(ID, format!("GET / failed: {error}")),
    };
    let kind = reply.header("content-type").unwrap_or("none").to_owned();
    if reply.status != 200 || !kind.starts_with("text/html") {
        return fail(
            ID,
            format!(
                "GET / -> {} {kind}; the application's page was expected (200 text/html)",
                reply.status
            ),
        );
    }
    let missing: Vec<&str> = SECURITY_HEADERS
        .into_iter()
        .filter(|name| reply.header(name).is_none())
        .collect();
    if missing.is_empty() {
        said(
            ID,
            Status::Ok,
            format!(
                "GET / -> 200 {kind}, with the content policy and the four other security headers"
            ),
        )
    } else {
        fail(
            ID,
            format!(
                "GET / -> 200 {kind}, without {}: the edge in front removes them, or the request did not reach the stack's own web edge",
                missing.join(", ")
            ),
        )
    }
}

/// The meta of the system, as the edge passes it on.
async fn meta(edge: &Edge<'_>) -> Result<Value, String> {
    let reply = fetch(edge, http::Method::GET, "/api/v1/meta", &[], Bytes::new()).await?;
    if reply.status != 200 {
        return Err(format!("GET /api/v1/meta -> {}", reply.status));
    }
    serde_json::from_slice(&reply.body)
        .map_err(|error| format!("GET /api/v1/meta -> 200, and the body is not JSON: {error}"))
}

/// An API call, and the health address.
async fn api(edge: &Edge<'_>) -> Line {
    const ID: &str = "edge-api";
    let health = match fetch(edge, http::Method::GET, "/health", &[], Bytes::new()).await {
        Ok(reply) => reply,
        Err(error) => return fail(ID, format!("GET /health failed: {error}")),
    };
    if health.status != 200 {
        return fail(
            ID,
            format!(
                "GET /health -> {}; the API does not answer through the edge",
                health.status
            ),
        );
    }
    match meta(edge).await {
        Err(error) => fail(ID, error),
        Ok(meta) => {
            let version = meta.get("version").and_then(Value::as_str).unwrap_or("?");
            let build = meta.get("build").and_then(Value::as_str).unwrap_or("?");
            let setup = meta
                .get("needs_setup")
                .and_then(Value::as_bool)
                .map_or("?", |needed| if needed { "yes" } else { "no" });
            said(
                ID,
                Status::Ok,
                format!(
                    "GET /health -> 200; GET /api/v1/meta -> 200, release {version} (build {build}), first administrator still to make: {setup}"
                ),
            )
        }
    }
}

/// A request body of the largest size the product sends.
async fn body(edge: &Edge<'_>) -> Line {
    const ID: &str = "edge-body";
    let payload = Bytes::from(vec![b'x'; BODY_BYTES]);
    let sent = fetch(
        edge,
        http::Method::POST,
        BODY_PATH,
        &[("content-type", "application/octet-stream")],
        payload,
    )
    .await;
    match sent {
        Err(error) => fail(
            ID,
            format!(
                "POST {BODY_PATH} with {BODY_BYTES} bytes (8 MiB) failed: {error}; an edge that cuts the connection refuses a body this size"
            ),
        ),
        Ok(reply) if reply.status == 413 => fail(
            ID,
            format!(
                "POST {BODY_PATH} with {BODY_BYTES} bytes (8 MiB) -> 413: allow request bodies of 8 MiB (apache2 LimitRequestBody, nginx client_max_body_size)"
            ),
        ),
        Ok(reply) if reply.status >= 500 => fail(
            ID,
            format!(
                "POST {BODY_PATH} with {BODY_BYTES} bytes (8 MiB) -> {}: the edge or what is behind it failed on a body of this size",
                reply.status
            ),
        ),
        Ok(reply) if (200..300).contains(&reply.status) => said(
            ID,
            Status::Ok,
            format!(
                "POST {BODY_PATH} with {BODY_BYTES} bytes (8 MiB) -> {}: the whole body went through the edge and was read",
                reply.status
            ),
        ),
        Ok(reply) if reply.status == 404 || reply.status == 405 => said(
            ID,
            Status::Part,
            format!(
                "POST {BODY_PATH} with {BODY_BYTES} bytes (8 MiB) -> {}: this web edge has no such path (a release before it), so nothing waited for the body and a front's limit on it cannot be told",
                reply.status
            ),
        ),
        Ok(reply) => fail(
            ID,
            format!(
                "POST {BODY_PATH} with {BODY_BYTES} bytes (8 MiB) -> {}: not an answer of the stack's web edge",
                reply.status
            ),
        ),
    }
}

/// How a held socket ended.
#[derive(Debug)]
enum Ended {
    /// It was still open when the time was up.
    Held,
    /// The other end closed it with a frame.
    Closed {
        after: Duration,
        code: u16,
        reason: String,
    },
    /// The connection went without a close frame.
    Cut { after: Duration, why: String },
}

/// Why a socket was not held.
#[derive(Debug)]
enum Fault {
    /// The upgrade was answered with something else than 101.
    Refused(u16),
    /// The connection or the exchange failed.
    Other(String),
}

struct Socket {
    protocol: Option<String>,
    ended: Ended,
}

/// What a held WebSocket is asked to be.
struct Hold<'a> {
    address: &'a Address,
    resolve: &'a Resolve,
    tls: Option<&'a Arc<rustls::ClientConfig>>,
    path: &'a str,
    subprotocol: &'a str,
    /// The first frame to send, when there is one.
    first: Option<String>,
    /// How long to hold the socket without another word.
    time: Duration,
}

/// Opens the WebSocket, says the first frame if there is one, and holds it.
async fn hold(ask: Hold<'_>) -> Result<Socket, Fault> {
    let stream = net::connect(ask.address, ask.resolve, ask.tls, None)
        .await
        .map_err(Fault::Other)?;
    let mut request = ask
        .address
        .websocket(ask.path)
        .into_client_request()
        .map_err(|error| Fault::Other(error.to_string()))?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        http::HeaderValue::from_str(ask.subprotocol)
            .map_err(|error| Fault::Other(error.to_string()))?,
    );
    let upgrading = tokio::time::timeout(
        Duration::from_secs(UPGRADE_SECS),
        tokio_tungstenite::client_async(request, stream),
    )
    .await
    .map_err(|_| Fault::Other(format!("no answer to the upgrade in {UPGRADE_SECS} s")))?;
    let (mut socket, response) = match upgrading {
        Ok(pair) => pair,
        Err(tungstenite::Error::Http(response)) => {
            return Err(Fault::Refused(response.status().as_u16()));
        }
        Err(error) => return Err(Fault::Other(error.to_string())),
    };
    let protocol = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if let Some(frame) = ask.first {
        socket
            .send(Message::text(frame))
            .await
            .map_err(|error| Fault::Other(error.to_string()))?;
    }
    let started = Instant::now();
    let deadline = started + ask.time;
    let ended = loop {
        match tokio::time::timeout_at(deadline, socket.next()).await {
            Err(_) => break Ended::Held,
            Ok(None) => {
                break Ended::Cut {
                    after: started.elapsed(),
                    why: "the connection was closed without a close frame".to_owned(),
                };
            }
            Ok(Some(Err(error))) => {
                break Ended::Cut {
                    after: started.elapsed(),
                    why: error.to_string(),
                };
            }
            Ok(Some(Ok(Message::Close(frame)))) => {
                let (code, reason) = frame.map_or((1005, String::new()), |frame| {
                    (u16::from(frame.code), frame.reason.to_string())
                });
                break Ended::Closed {
                    after: started.elapsed(),
                    code,
                    reason,
                };
            }
            // Anything else is a frame of the protocol; what is said is not the point.
            Ok(Some(Ok(_))) => {}
        }
    };
    if matches!(ended, Ended::Held) {
        let _ = socket.close(None).await;
    }
    Ok(Socket { protocol, ended })
}

/// The realtime socket, held idle.
async fn realtime_socket(edge: &Edge<'_>) -> Line {
    const ID: &str = "edge-ws";
    let path = "/api/v1/ws";
    let tls = match edge.address.secure.then(|| public_tls(edge)).transpose() {
        Ok(tls) => tls,
        Err(error) => return fail(ID, error),
    };
    let first = edge.access_token.map(|token| {
        serde_json::to_string(&ClientFrame::Auth(AuthFrame {
            token: token.to_owned(),
        }))
        .unwrap_or_default()
    });
    let signed_in = first.is_some();
    let socket = hold(Hold {
        address: &edge.address,
        resolve: edge.resolve,
        tls: tls.as_ref(),
        path,
        subprotocol: SOCKET_SUBPROTOCOL,
        first,
        time: edge.idle,
    })
    .await;
    let secs = |d: Duration| format!("{:.1} s", d.as_secs_f64());
    let asked = edge.idle.as_secs();
    match socket {
        Err(Fault::Refused(status)) => fail(
            ID,
            format!(
                "GET {path} asking to upgrade -> {status}: the edge does not pass WebSocket upgrades (nginx needs the Upgrade and Connection headers, apache2 mod_proxy_wstunnel)"
            ),
        ),
        Err(Fault::Other(error)) => {
            fail(ID, format!("GET {path} asking to upgrade failed: {error}"))
        }
        Ok(Socket { protocol, .. }) if protocol.as_deref() != Some(SOCKET_SUBPROTOCOL) => fail(
            ID,
            format!(
                "the upgrade at {path} was answered without the subprotocol {SOCKET_SUBPROTOCOL}: something else than the API answers there"
            ),
        ),
        Ok(Socket {
            ended: Ended::Held, ..
        }) => said(
            ID,
            Status::Ok,
            format!(
                "upgraded {path} and held it idle for {asked} s, {}",
                if signed_in { "signed in" } else { "unsigned" }
            ),
        ),
        Ok(Socket {
            ended: Ended::Cut { after, why },
            ..
        }) => fail(
            ID,
            format!(
                "upgraded {path}, and the connection went after {} with no close frame ({why}): the edge's idle limit is below {asked} s; raise it (apache2 ProxyTimeout, nginx proxy_read_timeout)",
                secs(after)
            ),
        ),
        Ok(Socket {
            ended:
                Ended::Closed {
                    after,
                    code,
                    reason,
                },
            ..
        }) => {
            // The API itself ends a socket that is silent for a minute, so a front
            // that has held it until then has held it as long as it can be.
            let wanted = edge.idle.min(Duration::from_secs(u64::from(SILENCE_SECS)));
            let held_long_enough = after + Duration::from_secs(IDLE_SLACK_SECS) >= wanted;
            if held_long_enough {
                said(
                    ID,
                    Status::Ok,
                    format!(
                        "upgraded {path}; the API ended the idle socket itself after {} (code {code}), so the edge held it for {asked} s",
                        secs(after)
                    ),
                )
            } else if !signed_in {
                said(
                    ID,
                    Status::Part,
                    format!(
                        "upgraded {path} and held it until the API closed it after {} (code {code}: {reason}), its rule for a socket that does not sign in ({UNSIGNED_SOCKET_SECS} s); to try the full {asked} s give an access token of a signed-in session (ORCH_ACCESS_TOKEN)",
                        secs(after)
                    ),
                )
            } else {
                fail(
                    ID,
                    format!(
                        "upgraded {path}, signed in, and the API closed it after {} (code {code}: {reason})",
                        secs(after)
                    ),
                )
            }
        }
    }
}

/// The machines' link, as far as an upgrade.
async fn link_upgrade(edge: &Edge<'_>) -> Line {
    const ID: &str = "edge-link";
    let path = orch_link_contract::version::SESSION_PATH;
    let tls = match edge.address.secure.then(|| public_tls(edge)).transpose() {
        Ok(tls) => tls,
        Err(error) => return fail(ID, error),
    };
    let socket = hold(Hold {
        address: &edge.address,
        resolve: edge.resolve,
        tls: tls.as_ref(),
        path,
        subprotocol: LINK_SUBPROTOCOL,
        first: None,
        time: Duration::ZERO,
    })
    .await;
    match socket {
        Err(Fault::Refused(status)) => fail(
            ID,
            format!(
                "GET {path} asking to upgrade -> {status}: the edge does not carry /link/v1/session to the gateway"
            ),
        ),
        Err(Fault::Other(error)) => {
            fail(ID, format!("GET {path} asking to upgrade failed: {error}"))
        }
        Ok(Socket { protocol, .. }) if protocol.as_deref() == Some(LINK_SUBPROTOCOL) => said(
            ID,
            Status::Ok,
            format!("upgraded {path}: the gateway answers with the subprotocol {LINK_SUBPROTOCOL}"),
        ),
        Ok(Socket { protocol, .. }) => fail(
            ID,
            format!(
                "the upgrade at {path} was answered with the subprotocol {protocol:?}, and the gateway speaks {LINK_SUBPROTOCOL}"
            ),
        ),
    }
}

/// One connection to each advertised endpoint, with the trust it is listed with.
async fn endpoints(edge: &Edge<'_>) -> Vec<Line> {
    const ID: &str = "edge-endpoints";
    let advertised = match meta(edge).await {
        Ok(meta) => meta,
        Err(error) => {
            return vec![fail(
                ID,
                format!("the advertised endpoints are in the meta: {error}"),
            )];
        }
    };
    let listed: Vec<Endpoint> = advertised
        .get("link_endpoints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let url = entry.get("url")?.as_str()?.to_owned();
            let trust = match entry.get("trust")?.as_str()? {
                "pinned" => Trust::Pinned,
                "public" => Trust::Public,
                _ => return None,
            };
            Some(Endpoint { url, trust })
        })
        .collect();
    if listed.is_empty() {
        return vec![fail(
            ID,
            "the system advertises no endpoint, so no machine can connect".to_owned(),
        )];
    }
    let authority = fetch_authority(edge).await;
    let mut lines = Vec::new();
    for (index, endpoint) in listed.iter().enumerate() {
        let id = format!("edge-endpoint-{}", index + 1);
        lines.push(try_endpoint(edge, &id, endpoint, authority.as_ref()).await);
    }
    lines
}

/// The authority of the system: the file of the secrets where this runs next to
/// them, else as the system itself gives it out.
async fn fetch_authority(edge: &Edge<'_>) -> Result<Certificate, String> {
    if let Ok(pem) = std::fs::read_to_string(edge.secrets_dir.join("orch_ca_cert.pem"))
        && let Ok(certificate) = Certificate::from_pem(&pem)
    {
        return Ok(certificate);
    }
    let reply = fetch(
        edge,
        http::Method::GET,
        "/api/v1/pki/ca.pem",
        &[],
        Bytes::new(),
    )
    .await?;
    if reply.status != 200 {
        return Err(format!("GET /api/v1/pki/ca.pem -> {}", reply.status));
    }
    let pem = String::from_utf8_lossy(&reply.body);
    Certificate::from_pem(&pem)
        .map_err(|error| format!("GET /api/v1/pki/ca.pem is not a certificate: {error}"))
}

async fn try_endpoint(
    edge: &Edge<'_>,
    id: &str,
    endpoint: &Endpoint,
    authority: Result<&Certificate, &String>,
) -> Line {
    let Some(session_url) = endpoint.session_url() else {
        return fail(
            id,
            format!("{} is not an address with a host", endpoint.url),
        );
    };
    let address = match Address::parse(&session_url) {
        Ok(address) => address,
        Err(error) => return fail(id, error),
    };
    let (tls, how) = match endpoint.trust {
        Trust::Pinned => {
            let authority = match authority {
                Ok(authority) => authority.clone(),
                Err(error) => {
                    return fail(
                        id,
                        format!(
                            "{} (pinned): the system's authority could not be fetched to check it by: {error}",
                            endpoint.url
                        ),
                    );
                }
            };
            let fingerprint = authority.fingerprint();
            let config = machine_client_config(
                Some(ServerTrust::Authority(Box::new(authority))),
                ServiceName::Gateway,
                None,
                Arc::new(SystemClock),
            )
            .map_err(|error| error.to_string());
            (
                config,
                format!(
                    "TLS under the system's authority {}",
                    &fingerprint[..fingerprint.len().min(16)]
                ),
            )
        }
        Trust::Public => (
            machine_client_config(
                authority
                    .ok()
                    .cloned()
                    .map(|certificate| ServerTrust::Authority(Box::new(certificate))),
                ServiceName::Frontend,
                None,
                Arc::new(SystemClock),
            )
            .map_err(|error| error.to_string()),
            "TLS under device roots or the Orch web edge identity".to_owned(),
        ),
    };
    let tls = match tls {
        Ok(tls) => tls,
        Err(error) => {
            return fail(
                id,
                format!("{} ({:?}): {error}", endpoint.url, endpoint.trust),
            );
        }
    };
    let socket = hold(Hold {
        address: &address,
        resolve: edge.resolve,
        tls: Some(&tls),
        // The address is the session socket's already.
        path: "",
        subprotocol: LINK_SUBPROTOCOL,
        first: None,
        time: Duration::ZERO,
    })
    .await;
    let trust = match endpoint.trust {
        Trust::Pinned => "pinned",
        Trust::Public => "public",
    };
    match socket {
        Ok(Socket { protocol, .. }) if protocol.as_deref() == Some(LINK_SUBPROTOCOL) => said(
            id,
            Status::Ok,
            format!(
                "{} ({trust}): {how}, upgraded to the link, subprotocol {LINK_SUBPROTOCOL}",
                endpoint.url
            ),
        ),
        Ok(_) => fail(
            id,
            format!(
                "{} ({trust}): upgraded, without the subprotocol {LINK_SUBPROTOCOL}",
                endpoint.url
            ),
        ),
        // The pinned listener wants the client certificate of a machine for a
        // session, and says so with 403 to a caller that has none: it was reached,
        // under the system's authority, and it spoke as it should.
        Err(Fault::Refused(403)) if endpoint.trust == Trust::Pinned => said(
            id,
            Status::Ok,
            format!(
                "{} ({trust}): {how}; the gateway answered the upgrade with 403, which is how it asks a caller that is no enrolled machine for a certificate",
                endpoint.url
            ),
        ),
        Err(Fault::Refused(status)) => fail(
            id,
            format!(
                "{} ({trust}): {how}, and the upgrade -> {status}",
                endpoint.url
            ),
        ),
        Err(Fault::Other(error)) => fail(id, format!("{} ({trust}): {error}", endpoint.url)),
    }
}
