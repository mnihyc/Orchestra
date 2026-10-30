//! The HTTP proxy of a restricted unit: `CONNECT` tunnels and plain requests, each decided before any lookup or socket, and the flows a changed policy closes.
//!
//! A `CONNECT host:port` is decided by its authority; an absolute `http://` request by its address, whose host
//! header has to name the same host. A tunnel carries whatever its client sends without looking into it, which is
//! how the connector's link to the gateway keeps its own pinned TLS end to end. A plain request goes out with
//! `Connection: close`, so that one connection carries one decided request. A refusal answers the host as it can
//! be shown and why, and is remembered as the last one; nothing of a request (a path, a query, a header, a body)
//! is written anywhere. Every flow is registered with its host, and a policy that no longer admits that host, or
//! a gate that closes, ends it before the new policy is reported in force.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use orch_driver_contract::network::NetworkDenial;
use orch_types::{Clock, Time};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use super::admission::{Admission, Decision, canonical_name, reasons, shown};

/// The most a request's head may hold: a longer one is no request a program sends through a proxy.
const HEAD_MAX: usize = 16 * 1024;
/// An HTTP authority without an explicit port names the standard cleartext listener.
const HTTP_DEFAULT_PORT: u16 = 80;

/// How long a client has to send its request's head.
const HEAD_WAIT: Duration = Duration::from_secs(10);

/// How long looking a host up may take.
const LOOKUP_WAIT: Duration = Duration::from_secs(10);

/// How long connecting to one address of a host may take.
const CONNECT_WAIT: Duration = Duration::from_secs(15);

/// How long the proxy waits before it accepts again after accepting failed (out of descriptors, say).
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// The headers a proxy takes for itself and never passes on.
const HOP_HEADERS: [&str; 5] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "proxy-authenticate",
];

/// A flow the proxy carries: the host it was decided for, and what ends it.
#[derive(Debug)]
struct Flow {
    host: String,
    port: u16,
    end: CancellationToken,
}

/// The policy in force and the flows it admitted. Closed (no policy) refuses everything.
#[derive(Debug, Default)]
pub struct Gate {
    admission: RwLock<Option<Admission>>,
    flows: Mutex<HashMap<u64, Flow>>,
    next: AtomicU64,
    denied: Mutex<Option<NetworkDenial>>,
}

impl Gate {
    /// A closed gate.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts `admission` in force, or closes the gate with none, and ends every flow it no longer admits before
    /// it returns.
    pub fn set(&self, admission: Option<Admission>) {
        let previous = self.current();
        let transports_changed = previous
            .as_ref()
            .map(|current| (&current.always, &current.refused))
            != admission.as_ref().map(|next| (&next.always, &next.refused));
        let mut flows = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        for flow in flows.values() {
            let admitted = admission
                .as_ref()
                .is_some_and(|admission| admission.still_admits(&flow.host, flow.port));
            if !admitted || transports_changed {
                flow.end.cancel();
            }
        }
        flows.retain(|_, flow| !flow.end.is_cancelled());
        *self
            .admission
            .write()
            .unwrap_or_else(PoisonError::into_inner) = admission;
    }

    /// The policy in force; none while the gate is closed.
    #[must_use]
    pub fn current(&self) -> Option<Admission> {
        self.admission
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many flows run.
    #[must_use]
    pub fn flows(&self) -> usize {
        self.flows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Registers a flow to `host` at `port`, if the policy in force admits it at this moment: the decision and
    /// the registration are one step under the lock a change of policy takes.
    fn open_flow(&self, host: &str, port: u16) -> Option<(u64, CancellationToken)> {
        let mut flows = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        let admitted = self
            .admission
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|admission| admission.still_admits(host, port));
        if !admitted {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let end = CancellationToken::new();
        flows.insert(
            id,
            Flow {
                host: host.to_owned(),
                port,
                end: end.clone(),
            },
        );
        Some((id, end))
    }

    fn close_flow(&self, id: u64) {
        self.flows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }

    /// Remembers a refusal as the last one.
    pub fn deny(&self, host: String, reason: &str, at: Time) {
        *self.denied.lock().unwrap_or_else(PoisonError::into_inner) = Some(NetworkDenial {
            host,
            at,
            reason: reason.to_owned(),
        });
    }

    /// The last refusal.
    #[must_use]
    pub fn last_denied(&self) -> Option<NetworkDenial> {
        self.denied
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Serves the proxy on `listener` for as long as the program runs.
pub async fn serve(listener: TcpListener, gate: Arc<Gate>, clock: Arc<dyn Clock>) {
    loop {
        let client = match listener.accept().await {
            Ok((client, _)) => client,
            Err(error) => {
                tracing::warn!(%error, "the proxy could not accept a connection");
                tokio::time::sleep(ACCEPT_PAUSE).await;
                continue;
            }
        };
        let gate = gate.clone();
        let clock = clock.clone();
        tokio::spawn(async move {
            if let Err(error) = handle(client, &gate, clock.as_ref()).await {
                tracing::debug!(%error, "a proxied connection ended with an error");
            }
        });
    }
}

/// What a request asks of the proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// A tunnel to an authority.
    Connect {
        /// The host, as asked.
        host: String,
        /// The port.
        port: u16,
    },
    /// A plain request, to be passed on in origin form.
    Plain {
        /// The host of its address, as asked.
        host: String,
        /// The port of its address.
        port: u16,
        /// The head to send the host: the request line in origin form and the headers that are passed on.
        head: Vec<u8>,
    },
}

/// Why a request's head is refused before it is decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    /// The host it named, as it can be shown; empty when it named none that could be read.
    pub host: String,
    /// Why.
    pub reason: &'static str,
}

fn unreadable(host: &str, reason: &'static str) -> Unreadable {
    Unreadable {
        host: shown(host),
        reason,
    }
}

/// Splits an authority `host:port`, with an address of IPv6 in brackets, into its two parts.
fn authority(text: &str) -> Option<(String, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_owned(), port))
}

/// Reads a request's head (its request line and headers) into what it asks.
pub fn read_request(head: &[u8]) -> Result<Request, Unreadable> {
    let text = std::str::from_utf8(head).map_err(|_| unreadable("", reasons::MALFORMED))?;
    let mut lines = text.split("\r\n");
    let line = lines.next().unwrap_or_default();
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(unreadable("", reasons::MALFORMED));
    };
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || method.is_empty() {
        return Err(unreadable("", reasons::MALFORMED));
    }
    let headers: Vec<(&str, &str)> = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim(), value.trim()))
        .collect();
    if headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .count()
        > 1
    {
        return Err(unreadable("", reasons::MALFORMED));
    }
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) =
            authority(target).ok_or_else(|| unreadable(target, reasons::MALFORMED))?;
        if let Some((_, named)) = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        {
            let (named_host, named_port) =
                authority(named).ok_or_else(|| unreadable(&host, reasons::HOST_MISMATCH))?;
            let same = match (canonical_name(&named_host), canonical_name(&host)) {
                (Ok(left), Ok(right)) => left == right,
                _ => named_host.eq_ignore_ascii_case(&host),
            };
            if named_port != port || !same {
                return Err(unreadable(&host, reasons::HOST_MISMATCH));
            }
        }
        return Ok(Request::Connect { host, port });
    }
    let url = url::Url::parse(target).map_err(|_| unreadable(target, reasons::MALFORMED))?;
    if url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() {
        return Err(unreadable(target, reasons::MALFORMED));
    }
    let host = match url.host() {
        Some(url::Host::Domain(domain)) => domain.to_owned(),
        Some(url::Host::Ipv4(address)) => address.to_string(),
        Some(url::Host::Ipv6(address)) => address.to_string(),
        None => return Err(unreadable(target, reasons::MALFORMED)),
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| unreadable(&host, reasons::MALFORMED))?;
    // The host header names the same host as the address, or the request is refused.
    let named = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| *value);
    if let Some(named) = named {
        let (named_host, named_port) = match authority(named) {
            Some((host, port)) => (host, port),
            None => (named.to_owned(), HTTP_DEFAULT_PORT),
        };
        let same = named_port == port
            && match (canonical_name(&named_host), canonical_name(&host)) {
                (Ok(left), Ok(right)) => left == right,
                _ => named_host.eq_ignore_ascii_case(&host),
            };
        if !same {
            return Err(unreadable(&host, reasons::HOST_MISMATCH));
        }
    }
    let mut origin = url.path().to_owned();
    if let Some(query) = url.query() {
        origin.push('?');
        origin.push_str(query);
    }
    let mut out = format!("{method} {origin} {version}\r\n");
    for (name, value) in &headers {
        if !HOP_HEADERS.iter().any(|hop| name.eq_ignore_ascii_case(hop)) {
            out.push_str(name);
            out.push_str(": ");
            out.push_str(value);
            out.push_str("\r\n");
        }
    }
    if named.is_none() {
        out.push_str("Host: ");
        out.push_str(&url[url::Position::BeforeHost..url::Position::AfterPort]);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    Ok(Request::Plain {
        host,
        port,
        head: out.into_bytes(),
    })
}

/// Reads a request's head from `client`: what came up to and with the empty line, and what came after it.
/// None when the client ended or the head is longer than any request of a program.
async fn read_head<R: AsyncRead + Unpin>(client: &mut R) -> io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 2048];
    loop {
        let count = client.read(&mut chunk).await?;
        if count == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..count]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let rest = buffer.split_off(end + 4);
            return Ok(Some((buffer, rest)));
        }
        if buffer.len() > HEAD_MAX {
            return Ok(None);
        }
    }
}

/// Answers a client with a short text and ends the connection.
async fn respond(client: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    let text = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(text.as_bytes()).await?;
    client.shutdown().await
}

/// Refuses a request: remembers it as the last refusal and answers with the host and why.
async fn refuse(
    client: &mut TcpStream,
    gate: &Gate,
    clock: &dyn Clock,
    host: String,
    reason: &'static str,
    status: &str,
) -> io::Result<()> {
    let body = format!("orch: {host}: {reason}\n");
    gate.deny(host, reason, clock.time());
    respond(client, status, &body).await
}

/// The addresses of `host` at `port` the proxy may connect to: every address of a trusted endpoint; for an
/// other name, those its policy does not refuse.
async fn addresses(
    host: &str,
    port: u16,
    refusing: Option<&Admission>,
) -> Result<Vec<SocketAddr>, &'static str> {
    let found: Vec<SocketAddr> =
        match tokio::time::timeout(LOOKUP_WAIT, tokio::net::lookup_host((host, port))).await {
            Ok(Ok(found)) => found.collect(),
            _ => return Err(reasons::NO_ADDRESS),
        };
    if found.is_empty() {
        return Err(reasons::NO_ADDRESS);
    }
    let usable: Vec<SocketAddr> = match refusing {
        Some(admission) => found
            .into_iter()
            .filter(|address| !admission.refuses(address.ip()))
            .collect(),
        None => found,
    };
    if usable.is_empty() {
        return Err(reasons::REFUSED_ADDRESS);
    }
    Ok(usable)
}

/// Connects to the first of `addresses` that answers.
async fn connect(addresses: &[SocketAddr]) -> Option<TcpStream> {
    for address in addresses {
        if let Ok(Ok(stream)) =
            tokio::time::timeout(CONNECT_WAIT, TcpStream::connect(address)).await
        {
            return Some(stream);
        }
    }
    None
}

/// One client connection: its request read, decided, and carried or refused.
async fn handle(mut client: TcpStream, gate: &Gate, clock: &dyn Clock) -> io::Result<()> {
    let read = tokio::time::timeout(HEAD_WAIT, read_head(&mut client)).await;
    let Ok(Ok(Some((head, rest)))) = read else {
        return Ok(());
    };
    let request = match read_request(&head) {
        Ok(request) => request,
        Err(Unreadable { host, reason }) => {
            return refuse(&mut client, gate, clock, host, reason, "400 Bad Request").await;
        }
    };
    let (host, port) = match &request {
        Request::Connect { host, port } | Request::Plain { host, port, .. } => {
            (host.clone(), *port)
        }
    };
    let Some(admission) = gate.current() else {
        return refuse(
            &mut client,
            gate,
            clock,
            shown(&host),
            reasons::CLOSED,
            "503 Service Unavailable",
        )
        .await;
    };
    let (name, refusing) = match admission.decide(&host, port) {
        Decision::Denied { host, reason } => {
            return refuse(&mut client, gate, clock, host, reason, "403 Forbidden").await;
        }
        Decision::Trusted(name) => {
            let tunnel = matches!(&request, Request::Connect { .. });
            let protocol_allowed = admission.always.iter().any(|transport| {
                transport.authority == name
                    && transport.port == port
                    && if tunnel {
                        matches!(transport.protocol.as_str(), "https" | "wss")
                    } else {
                        transport.protocol == "http"
                    }
            });
            if !protocol_allowed {
                return refuse(
                    &mut client,
                    gate,
                    clock,
                    name,
                    "the trusted endpoint requires another protocol",
                    "403 Forbidden",
                )
                .await;
            }
            (name, None)
        }
        Decision::Other(name) => (name, Some(&admission)),
    };
    let found = match addresses(&name, port, refusing).await {
        Ok(found) => found,
        Err(reason) => {
            return refuse(&mut client, gate, clock, name, reason, "403 Forbidden").await;
        }
    };
    let Some(mut upstream) = connect(&found).await else {
        return refuse(
            &mut client,
            gate,
            clock,
            name,
            reasons::UNREACHABLE,
            "502 Bad Gateway",
        )
        .await;
    };
    let Some((flow, end)) = gate.open_flow(&name, port) else {
        return refuse(
            &mut client,
            gate,
            clock,
            name,
            reasons::WITHDRAWN,
            "403 Forbidden",
        )
        .await;
    };
    let carried = async {
        match &request {
            Request::Connect { .. } => {
                client
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .await?;
            }
            Request::Plain { head, .. } => upstream.write_all(head).await?,
        }
        if !rest.is_empty() {
            upstream.write_all(&rest).await?;
        }
        tokio::io::copy_bidirectional(&mut client, &mut upstream).await
    };
    let outcome = tokio::select! {
        () = end.cancelled() => Ok(()),
        carried = carried => carried.map(|_| ()),
    };
    gate.close_flow(flow);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::network::{NetworkTransport, TransportPurpose};

    #[test]
    fn a_malformed_authority_never_echoes_a_path_query_or_credential() {
        let refused =
            read_request(b"CONNECT user:password@private/path?token=hidden HTTP/1.1\r\n\r\n")
                .unwrap_err();
        assert_eq!(refused.host, "invalid_host");
        assert!(!refused.host.contains("password") && !refused.host.contains("hidden"));
        let refused = read_request(b"CONNECT pypi.org:443 HTTP/1.1\r\nHost: crates.io:443\r\n\r\n")
            .unwrap_err();
        assert_eq!(refused.reason, reasons::HOST_MISMATCH);
        assert!(read_request(b"GET http://pypi.org/private HTTP/1.1\r\nHost: pypi.org\r\nHost: crates.io\r\n\r\n").is_err());
    }

    fn admission(hosts: &[&str]) -> Admission {
        Admission {
            revision: "r1".into(),
            always_revision: "a1".into(),
            always: vec![NetworkTransport {
                authority: "gateway.orch.internal".into(),
                protocol: "wss".into(),
                port: 9443,
                purpose: TransportPurpose::Gateway,
            }],
            allowed_hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
            refused: Vec::new(),
        }
    }

    #[test]
    fn a_tunnel_is_asked_for_by_its_authority() {
        assert_eq!(
            read_request(b"CONNECT pypi.org:443 HTTP/1.1\r\nHost: pypi.org:443\r\n\r\n"),
            Ok(Request::Connect {
                host: "pypi.org".into(),
                port: 443
            })
        );
        assert_eq!(
            read_request(b"CONNECT [::1]:443 HTTP/1.1\r\n\r\n"),
            Ok(Request::Connect {
                host: "[::1]".into(),
                port: 443
            })
        );
        assert!(read_request(b"CONNECT pypi.org HTTP/1.1\r\n\r\n").is_err());
        assert!(read_request(b"CONNECT pypi.org:0 HTTP/1.1\r\n\r\n").is_err());
    }

    #[test]
    fn a_plain_request_goes_on_in_origin_form_without_the_proxys_headers_and_closes() {
        let request = read_request(
            b"GET http://Example.com/a/b?c=d HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: x\r\nConnection: keep-alive\r\nAccept: */*\r\n\r\n",
        )
        .unwrap();
        let Request::Plain { host, port, head } = request else {
            panic!("a plain request")
        };
        assert_eq!((host.as_str(), port), ("example.com", 80));
        let head = String::from_utf8(head).unwrap();
        assert!(head.starts_with("GET /a/b?c=d HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("Host: example.com\r\n"));
        assert!(head.contains("Accept: */*\r\n"));
        assert!(!head.to_ascii_lowercase().contains("proxy-authorization"));
        assert!(!head.contains("keep-alive"));
        assert!(head.ends_with("Connection: close\r\n\r\n"));
    }

    #[test]
    fn a_host_header_that_names_another_host_is_refused() {
        let refused = read_request(b"GET http://pypi.org/ HTTP/1.1\r\nHost: evil.example\r\n\r\n")
            .unwrap_err();
        assert_eq!(refused.reason, reasons::HOST_MISMATCH);
        let refused = read_request(b"GET http://pypi.org/ HTTP/1.1\r\nHost: pypi.org:8080\r\n\r\n")
            .unwrap_err();
        assert_eq!(refused.reason, reasons::HOST_MISMATCH);
        assert!(
            read_request(b"GET http://pypi.org:80/ HTTP/1.1\r\nHost: PYPI.org\r\n\r\n").is_ok()
        );
    }

    #[test]
    fn only_plain_http_is_taken_in_absolute_form() {
        assert!(read_request(b"GET https://pypi.org/ HTTP/1.1\r\n\r\n").is_err());
        assert!(read_request(b"GET /relative HTTP/1.1\r\nHost: pypi.org\r\n\r\n").is_err());
        assert!(read_request(b"BREW\r\n\r\n").is_err());
    }

    #[test]
    fn a_new_policy_ends_the_flows_it_no_longer_admits_and_a_closed_gate_ends_all() {
        let gate = Gate::new();
        assert!(gate.open_flow("pypi.org", 443).is_none(), "closed");
        gate.set(Some(admission(&["pypi.org", "crates.io"])));
        let (_, pypi) = gate.open_flow("pypi.org", 443).unwrap();
        let (_, crates) = gate.open_flow("crates.io", 443).unwrap();
        let (_, link) = gate.open_flow("gateway.orch.internal", 9443).unwrap();
        assert_eq!(gate.flows(), 3);
        gate.set(Some(admission(&["crates.io"])));
        assert!(pypi.is_cancelled(), "a host taken off the list is cut off");
        assert!(!crates.is_cancelled() && !link.is_cancelled());
        assert_eq!(gate.flows(), 2);
        assert!(gate.open_flow("pypi.org", 443).is_none());
        gate.set(None);
        assert!(
            crates.is_cancelled() && link.is_cancelled(),
            "closed ends Orch's own too"
        );
        assert_eq!(gate.flows(), 0);
    }

    #[tokio::test]
    async fn a_refused_request_is_answered_and_remembered_by_its_host_alone() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new(Gate::new());
        gate.set(Some(admission(&["pypi.org"])));
        let clock: Arc<dyn Clock> = Arc::new(orch_types::SystemClock);
        tokio::spawn(serve(listener, gate.clone(), clock));
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"CONNECT evil.example:443 HTTP/1.1\r\nX-Secret: token\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        client.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
        assert!(answer.contains("evil.example"));
        assert!(!answer.contains("token"));
        let denied = gate.last_denied().unwrap();
        assert_eq!(denied.host, "evil.example");
        assert_eq!(denied.reason, reasons::NOT_LISTED);
    }
}
