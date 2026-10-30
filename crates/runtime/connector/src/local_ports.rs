//! Environment-local read-only ports, exact active peer lookup and coalesced assertion refresh without a private signer.

use crate::loopback::{Body, text};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use orch_api_contract::ports::{
    EnvironmentPortListQuery, PortAssertionKeys, PortConnectionInside, PortPeer, PortTransport,
};
use orch_envhost::environment::{EnvHost, Environment};
use orch_link::{machine::MachineLink, tunnels::channel::TunnelCloser};
use orch_link_contract::{
    local_ports::{
        LOCAL_CONCURRENT, LOCAL_PER_MINUTE, PEER_REPLY_BYTES, PortLocalAsk, PortLocalData,
        PortLocalRead,
    },
    ports::PortTicketClaims,
};
use orch_plugin_contract::Secret;
use orch_types::{Clock, Id, Time, new_id};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use subtle::ConstantTimeEq as _;
use tokio::{net::TcpListener, sync::Semaphore};
use tokio_util::sync::CancellationToken;

// Ten seconds for a control read plus room for HTTP parsing/serialization; no connection waits forever.
const HTTP_WITHIN: Duration = Duration::from_secs(15);
const WINDOW_MILLIS: i64 = 60_000;
const KEY_CACHE_MILLIS: i64 = 30_000;
const REFRESH_BEFORE_MILLIS: i64 = 5_000;
const FAILED_REFRESH_MILLIS: i64 = 1_000;
const TOKEN_BYTES: usize = 32;
// A complete canonical page, bounded independently of a peer's stricter 4,000-byte reply.
const CLIENT_REPLY_BYTES: usize = 1024 * 1024;
// Idle headers get one extra connection per active lookup, never an unbounded socket queue.
const HEADER_CONNECTIONS: usize = LOCAL_CONCURRENT * 2;

fn locked<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

fn admit_read(times: &mut VecDeque<i64>, now: i64) -> Result<(), u64> {
    while times
        .front()
        .is_some_and(|then| now.saturating_sub(*then) >= WINDOW_MILLIS)
    {
        times.pop_front();
    }
    if times.len() >= LOCAL_PER_MINUTE as usize {
        let wait = times.front().map_or(WINDOW_MILLIS, |then| {
            then.saturating_add(WINDOW_MILLIS).saturating_sub(now)
        });
        return Err(u64::try_from(wait.max(1).saturating_add(999) / 1000).unwrap_or(1));
    }
    times.push_back(now);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PeerKey {
    port: u16,
    transport: PortTransport,
    address: IpAddr,
    peer_port: u16,
}
impl PeerKey {
    fn new(port: u16, transport: PortTransport, inside: &PortConnectionInside) -> Option<Self> {
        Some(Self {
            port,
            transport,
            address: inside.address.parse::<IpAddr>().ok()?.to_canonical(),
            peer_port: inside.port,
        })
    }
}
struct Cached {
    until: i64,
    value: Result<Option<PortPeer>, ()>,
}
struct Peer {
    id: Id,
    generation: i64,
    attachment: std::sync::Weak<Environment>,
    closer: TunnelCloser,
    inside: PortConnectionInside,
    cached: tokio::sync::Mutex<Option<Cached>>,
}

/// A live native socket's registration; dropping it forgets only that exact use, not a reused tuple.
pub(crate) struct Tracked {
    scope: Arc<Scope>,
    key: PeerKey,
    id: Id,
}
impl Drop for Tracked {
    fn drop(&mut self) {
        let mut peers = locked(&self.scope.peers);
        if let Some(uses) = peers.get_mut(&self.key) {
            uses.remove(&self.id);
            if uses.is_empty() {
                peers.remove(&self.key);
            }
        }
    }
}

struct Scope {
    id: Id,
    port: u16,
    managed: bool,
    token: Secret,
    host: EnvHost,
    link: MachineLink,
    clock: Arc<dyn Clock>,
    alive: AtomicBool,
    stop: CancellationToken,
    room: Arc<Semaphore>,
    reads: Mutex<VecDeque<i64>>,
    peers: Mutex<HashMap<PeerKey, HashMap<Id, Arc<Peer>>>>,
    keys: Mutex<Option<(i64, PortAssertionKeys)>>,
    key_refresh: tokio::sync::Mutex<()>,
}
impl Scope {
    fn time(&self) -> Time {
        Time::new(self.clock.now() + chrono::Duration::milliseconds(self.link.clock_offset_ms()))
    }
    fn generation(&self) -> Option<i64> {
        self.alive
            .load(Ordering::Acquire)
            .then(|| {
                self.host
                    .environment(self.id)
                    .ok()
                    .map(|env| env.generation())
            })
            .flatten()
    }
    fn rate(&self) -> Result<(), u64> {
        admit_read(&mut locked(&self.reads), self.clock.time().unix_millis())
    }
    async fn ask(&self, generation: i64, read: PortLocalRead) -> Result<PortLocalData, ()> {
        let answer = self
            .link
            .local_ports(PortLocalAsk {
                request_id: new_id(),
                environment_id: self.id,
                generation,
                read,
            })
            .await
            .map_err(|_| ())?;
        match (answer.data, answer.failure) {
            (Some(data), None) => Ok(data),
            _ => Err(()),
        }
    }
    fn remember_keys(&self, keys: PortAssertionKeys) {
        *locked(&self.keys) = Some((
            self.time().unix_millis().saturating_add(KEY_CACHE_MILLIS),
            keys,
        ));
    }
    async fn keys(&self) -> Result<PortAssertionKeys, ()> {
        let current = || {
            locked(&self.keys)
                .as_ref()
                .filter(|(until, _)| *until > self.time().unix_millis())
                .map(|(_, keys)| keys.clone())
        };
        if let Some(keys) = current() {
            return Ok(keys);
        }
        let _refresh = self.key_refresh.lock().await;
        if let Some(keys) = current() {
            return Ok(keys);
        }
        let PortLocalData::Keys { keys } = self
            .ask(self.generation().unwrap_or(0), PortLocalRead::Keys)
            .await?
        else {
            return Err(());
        };
        self.remember_keys(keys.clone());
        Ok(keys)
    }
    fn current(&self, key: &PeerKey, peer: &Peer) -> bool {
        if self.generation() != Some(peer.generation) || peer.closer.is_closed() {
            return false;
        }
        let Some(attached) = peer.attachment.upgrade() else {
            return false;
        };
        if !self
            .host
            .environment(self.id)
            .is_ok_and(|current| Arc::ptr_eq(&current, &attached))
        {
            return false;
        }
        locked(&self.peers)
            .get(key)
            .is_some_and(|uses| uses.len() == 1 && uses.contains_key(&peer.id))
    }
    async fn whois(&self, key: PeerKey) -> Result<Option<PortPeer>, ()> {
        let peer = {
            let peers = locked(&self.peers);
            match peers.get(&key) {
                None => return Ok(None),
                Some(uses) if uses.len() != 1 => return Err(()),
                Some(uses) => uses.values().next().cloned().ok_or(())?,
            }
        };
        if !self.current(&key, &peer) {
            return Ok(None);
        }
        let mut cached = peer.cached.lock().await;
        let now = self.time();
        if let Some(cache) = cached
            .as_ref()
            .filter(|cache| cache.until > now.unix_millis())
        {
            return if self.current(&key, &peer) {
                cache.value.clone()
            } else {
                Ok(None)
            };
        }
        let answer = self
            .ask(
                peer.generation,
                PortLocalRead::Peer {
                    connection_id: peer.id,
                    port: key.port,
                    transport: key.transport,
                    inside: peer.inside.clone(),
                },
            )
            .await;
        let result = match answer {
            Ok(PortLocalData::Peer { peer: answer, keys }) => {
                self.remember_keys(keys.clone());
                match answer {
                    None => Ok(None),
                    Some(answer) => {
                        let verified = keys.keys.iter().find_map(|public| {
                            orch_pki::port_assertions::verify(
                                &answer.assertion,
                                &public.x,
                                &keys.issuer,
                                self.id,
                                key.port,
                                self.time(),
                            )
                            .ok()
                        });
                        if let Some(claims) = verified.filter(|claims| {
                            claims["exp"]
                                .as_i64()
                                .is_some_and(|expires| expires > self.time().get().timestamp())
                                && claims["connection_id"] == peer.id.to_string()
                                && claims["generation"] == peer.generation
                                && claims["transport"]
                                    == match key.transport {
                                        PortTransport::Tcp => "tcp",
                                        PortTransport::Udp => "udp",
                                    }
                        }) {
                            if answer.connection.id != peer.id
                                || answer.connection.environment_id != self.id
                                || answer.connection.generation != peer.generation
                                || answer.connection.port != key.port
                                || answer.connection.transport != key.transport
                                || answer.connection.ended_at.is_some()
                                || PeerKey::new(key.port, key.transport, &answer.connection.inside)
                                    .as_ref()
                                    != Some(&key)
                            {
                                Err(())
                            } else {
                                let until = claims["exp"]
                                    .as_i64()
                                    .unwrap_or(0)
                                    .saturating_mul(1000)
                                    .saturating_sub(REFRESH_BEFORE_MILLIS);
                                *cached = Some(Cached {
                                    until,
                                    value: Ok(Some(answer.clone())),
                                });
                                return if self.current(&key, &peer) {
                                    Ok(Some(answer))
                                } else {
                                    Ok(None)
                                };
                            }
                        } else {
                            Err(())
                        }
                    }
                }
            }
            _ => Err(()),
        };
        *cached = Some(Cached {
            until: self
                .time()
                .unix_millis()
                .saturating_add(FAILED_REFRESH_MILLIS),
            value: result.clone(),
        });
        if self.current(&key, &peer) {
            result
        } else {
            Ok(None)
        }
    }
}

/// One role's listeners; tokens exist only in memory and in that environment's started-process variables.
pub struct LocalPorts {
    host: EnvHost,
    link: MachineLink,
    clock: Arc<dyn Clock>,
    managed: bool,
    scopes: Mutex<HashMap<Id, Arc<Scope>>>,
    stopped: AtomicBool,
}
impl std::fmt::Debug for LocalPorts {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("LocalPorts").finish_non_exhaustive()
    }
}
impl LocalPorts {
    /// Uses the role's existing environment owner and authenticated link, with no separate network route.
    #[must_use]
    pub fn new(
        host: EnvHost,
        link: MachineLink,
        clock: Arc<dyn Clock>,
        managed: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            host,
            link,
            clock,
            managed,
            scopes: Mutex::new(HashMap::new()),
            stopped: AtomicBool::new(false),
        })
    }
    /// Creates/reuses the environment's listener and returns only its protected process variables.
    pub async fn variables(&self, environment: Id) -> std::io::Result<BTreeMap<String, String>> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(std::io::Error::other("the environment role is stopping"));
        }
        let known = locked(&self.scopes).get(&environment).cloned();
        let scope = if let Some(scope) = known {
            scope
        } else {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let scope = Arc::new(Scope {
                id: environment,
                port: listener.local_addr()?.port(),
                managed: self.managed,
                token: Secret::new(hex::encode(rand::random::<[u8; TOKEN_BYTES]>())),
                host: self.host.clone(),
                link: self.link.clone(),
                clock: Arc::clone(&self.clock),
                alive: AtomicBool::new(true),
                stop: CancellationToken::new(),
                room: Arc::new(Semaphore::new(LOCAL_CONCURRENT)),
                reads: Mutex::new(VecDeque::new()),
                peers: Mutex::new(HashMap::new()),
                keys: Mutex::new(None),
                key_refresh: tokio::sync::Mutex::new(()),
            });
            let (chosen, start) = {
                let mut scopes = locked(&self.scopes);
                if self.stopped.load(Ordering::Acquire) {
                    return Err(std::io::Error::other("the environment role is stopping"));
                }
                match scopes.entry(environment) {
                    std::collections::hash_map::Entry::Occupied(entry) => {
                        (Arc::clone(entry.get()), false)
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(Arc::clone(&scope));
                        (scope, true)
                    }
                }
            };
            if start {
                tokio::spawn(serve(listener, Arc::clone(&chosen)));
            }
            chosen
        };
        let mut variables = BTreeMap::from([(
            "ORCH_PORTS_URL".into(),
            format!("http://127.0.0.1:{}", scope.port),
        )]);
        if !self.managed {
            variables.insert("ORCH_PORTS_TOKEN".into(), scope.token.expose().to_owned());
        }
        Ok(variables)
    }
    /// Retires the token and every read for a detached environment; a later attach gets fresh random bytes.
    pub fn detach(&self, environment: Id) {
        if let Some(scope) = locked(&self.scopes).remove(&environment) {
            scope.alive.store(false, Ordering::Release);
            scope.stop.cancel();
        }
    }
    /// Retires every listener before the role stops.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        for (_, scope) in locked(&self.scopes).drain() {
            scope.alive.store(false, Ordering::Release);
            scope.stop.cancel();
        }
    }
    pub(crate) fn track(
        &self,
        id: Id,
        claims: &PortTicketClaims,
        environment: &Arc<Environment>,
        inside: PortConnectionInside,
        closer: TunnelCloser,
    ) -> Option<Tracked> {
        let scope = locked(&self.scopes).get(&claims.environment_id)?.clone();
        if environment.generation() != claims.generation
            || !scope
                .host
                .environment(scope.id)
                .is_ok_and(|current| Arc::ptr_eq(&current, environment))
        {
            return None;
        }
        let key = PeerKey::new(claims.port, claims.transport, &inside)?;
        let peer = Arc::new(Peer {
            id,
            generation: claims.generation,
            attachment: Arc::downgrade(environment),
            closer,
            inside,
            cached: tokio::sync::Mutex::new(None),
        });
        locked(&scope.peers)
            .entry(key.clone())
            .or_default()
            .insert(id, peer);
        Some(Tracked { scope, key, id })
    }
}
impl Drop for LocalPorts {
    fn drop(&mut self) {
        self.stop();
    }
}

fn json<T: Serialize>(status: StatusCode, value: &T, maximum: Option<usize>) -> Response<Body> {
    let Ok(bytes) = serde_json::to_vec(value) else {
        return text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the port reply could not be written",
        );
    };
    if maximum.is_some_and(|maximum| bytes.len() > maximum) {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "ports.unavailable");
    }
    let mut reply = Response::new(
        Full::new(Bytes::from(bytes))
            .map_err(|never: Infallible| match never {})
            .boxed(),
    );
    *reply.status_mut() = status;
    reply.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    reply.headers_mut().insert(
        hyper::header::CACHE_CONTROL,
        hyper::header::HeaderValue::from_static("no-store"),
    );
    reply
}
fn failure(status: StatusCode, code: &str) -> Response<Body> {
    json(status, &serde_json::json!({"code":code}), None)
}
fn limited(seconds: u64) -> Response<Body> {
    let mut reply = failure(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    if let Ok(value) = hyper::header::HeaderValue::from_str(&seconds.to_string()) {
        reply
            .headers_mut()
            .insert(hyper::header::RETRY_AFTER, value);
    }
    reply
}

async fn serve(listener: TcpListener, scope: Arc<Scope>) {
    // Bound idle headers separately from the sixty-four active lookups.
    let connections = Arc::new(Semaphore::new(HEADER_CONNECTIONS));
    loop {
        let accepted =
            tokio::select! {()=scope.stop.cancelled()=>return,accepted=listener.accept()=>accepted};
        let Ok((socket, _)) = accepted else {
            return;
        };
        let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else {
            drop(socket);
            continue;
        };
        let scope = Arc::clone(&scope);
        tokio::spawn(async move {
            let _permit = permit;
            let handler = Arc::clone(&scope);
            let service = service_fn(move |request| {
                let scope = Arc::clone(&handler);
                async move { Ok::<_, Infallible>(handle(request, &scope).await) }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .keep_alive(false)
                .serve_connection(TokioIo::new(socket), service);
            let _ = tokio::time::timeout(HTTP_WITHIN, connection).await;
        });
    }
}

/// A local read's safe failure; neither a bearer nor a server-supplied body is printed.
#[derive(Debug, thiserror::Error)]
pub enum LocalReadError {
    /// Only the environment's literal loopback listener is a credential destination.
    #[error("ORCH_PORTS_URL must name the environment's HTTP listener on 127.0.0.1")]
    Address,
    /// The endpoint is read-only and accepts relative paths only.
    #[error("the local port read's path is invalid")]
    Path,
    /// A missing listener, malformed answer or expired transport has the same safe report.
    #[error("the environment's local ports reader is unavailable")]
    Unavailable,
    /// Preserve a useful refusal without echoing untrusted text or a credential.
    #[error("the environment's local ports reader refused the read (HTTP {0})")]
    Refused(u16),
}

struct ConnectionTask(tokio::task::JoinHandle<()>);
impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Reads a canonical JSON projection directly from loopback, without proxies, redirects or cookies.
/// The whole exchange is bounded; a cancellation aborts its connection task as well.
pub async fn read_local(
    base: &str,
    token: Option<&Secret>,
    path: &str,
) -> Result<serde_json::Value, LocalReadError> {
    let url = url::Url::parse(base).map_err(|_| LocalReadError::Address)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(LocalReadError::Address);
    }
    if !path.starts_with('/') || path.starts_with("//") || path.contains(['\r', '\n', '#']) {
        return Err(LocalReadError::Path);
    }
    let port = url.port().ok_or(LocalReadError::Address)?;
    tokio::time::timeout(HTTP_WITHIN, async {
        let socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| LocalReadError::Unavailable)?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(socket))
            .await
            .map_err(|_| LocalReadError::Unavailable)?;
        let _connection = ConnectionTask(tokio::spawn(async move {
            let _ = connection.await;
        }));
        let mut request = Request::builder()
            .method(hyper::Method::GET)
            .uri(path)
            .header(hyper::header::HOST, format!("127.0.0.1:{port}"));
        if let Some(token) = token {
            request = request.header(
                hyper::header::AUTHORIZATION,
                format!("Bearer {}", token.expose()),
            );
        }
        let request = request
            .body(Full::new(Bytes::new()))
            .map_err(|_| LocalReadError::Path)?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| LocalReadError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(LocalReadError::Refused(response.status().as_u16()));
        }
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        // Read frames directly: Limited's boxed-error conversion is not Send-general enough
        // for some callers which spawn the complete connector future.
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| LocalReadError::Unavailable)?;
            if let Ok(data) = frame.into_data() {
                if data.len() > CLIENT_REPLY_BYTES.saturating_sub(bytes.len()) {
                    return Err(LocalReadError::Unavailable);
                }
                bytes.extend_from_slice(&data);
            }
        }
        serde_json::from_slice(&bytes).map_err(|_| LocalReadError::Unavailable)
    })
    .await
    .map_err(|_| LocalReadError::Unavailable)?
}

async fn handle(request: Request<Incoming>, scope: &Arc<Scope>) -> Response<Body> {
    if !request
        .headers()
        .get(hyper::header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| orch_agent_contract::loopback::host_is_allowed(host, scope.port))
    {
        return failure(StatusCode::MISDIRECTED_REQUEST, "wrong_host");
    }
    if request.method() != hyper::Method::GET {
        return failure(StatusCode::METHOD_NOT_ALLOWED, "read_only");
    }
    let path = request.uri().path();
    let public = path == "/ports/assertion-keys";
    if !scope.managed && !public {
        let token = request
            .headers()
            .get(hyper::header::AUTHORIZATION)
            .and_then(|header| header.to_str().ok())
            .and_then(|header| header.strip_prefix("Bearer "))
            .unwrap_or("");
        if !bool::from(token.as_bytes().ct_eq(scope.token.expose().as_bytes())) {
            return failure(StatusCode::UNAUTHORIZED, "environment_token_required");
        }
    }
    if let Err(wait) = scope.rate() {
        return limited(wait);
    }
    let Ok(_room) = Arc::clone(&scope.room).try_acquire_owned() else {
        return limited(1);
    };
    if public {
        return match scope.keys().await {
            Ok(keys) => json(StatusCode::OK, &keys, None),
            Err(()) => failure(StatusCode::SERVICE_UNAVAILABLE, "ports.unavailable"),
        };
    }
    let Some(generation) = scope.generation() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "ports.unavailable");
    };
    let fields = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .into_owned()
        .collect::<Vec<_>>();
    let mut query = BTreeMap::new();
    for (key, value) in fields {
        if query.insert(key, value).is_some() {
            return failure(StatusCode::BAD_REQUEST, "duplicate_parameter");
        }
    }
    if path == "/whois" {
        let parsed = (|| {
            let peer = query.remove("peer")?.parse::<SocketAddr>().ok()?;
            let port = query
                .remove("listener_port")?
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)?;
            let transport = match query.remove("transport")?.as_str() {
                "tcp" => PortTransport::Tcp,
                "udp" => PortTransport::Udp,
                _ => return None,
            };
            if !query.is_empty() || peer.port() == 0 {
                return None;
            }
            Some(PeerKey {
                port,
                transport,
                address: peer.ip().to_canonical(),
                peer_port: peer.port(),
            })
        })();
        let Some(key) = parsed else {
            return failure(StatusCode::BAD_REQUEST, "invalid_peer");
        };
        return match scope.whois(key).await {
            Ok(peer) => json(StatusCode::OK, &peer, Some(PEER_REPLY_BYTES)),
            Err(()) => failure(StatusCode::SERVICE_UNAVAILABLE, "ports.unavailable"),
        };
    }
    let read = if path == "/ports" {
        let parsed = (|| {
            let port = query
                .remove("port")
                .map(|value| value.parse::<u16>())
                .transpose()
                .ok()?;
            let limit = query
                .remove("limit")
                .map(|value| value.parse::<u32>())
                .transpose()
                .ok()?;
            let look = query
                .remove("look")
                .map(|value| value.parse::<bool>())
                .transpose()
                .ok()?;
            let cursor = query.remove("cursor");
            query.is_empty().then_some(EnvironmentPortListQuery {
                port,
                limit,
                look,
                cursor,
            })
        })();
        let Some(query) = parsed else {
            return failure(StatusCode::BAD_REQUEST, "invalid_query");
        };
        PortLocalRead::List { query }
    } else if let Some(port) = path
        .strip_prefix("/ports/")
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port > 0)
    {
        if !query.is_empty() {
            return failure(StatusCode::BAD_REQUEST, "invalid_query");
        }
        PortLocalRead::Address { port }
    } else {
        return failure(StatusCode::NOT_FOUND, "not_found");
    };
    let listing = matches!(&read, PortLocalRead::List { .. });
    let result = scope.ask(generation, read).await;
    if scope.generation() != Some(generation) {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "ports.unavailable");
    }
    match result {
        Ok(PortLocalData::List { ports }) if listing => json(StatusCode::OK, &ports, None),
        Ok(PortLocalData::Address { target }) if !listing => json(StatusCode::OK, &target, None),
        _ => failure(StatusCode::SERVICE_UNAVAILABLE, "ports.unavailable"),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod tests {
    use super::*;

    #[test]
    fn a_rolling_minute_refuses_excess_reads_and_says_when_room_returns() {
        let mut times = VecDeque::new();
        for _ in 0..LOCAL_PER_MINUTE {
            assert_eq!(admit_read(&mut times, 0), Ok(()));
        }
        assert_eq!(admit_read(&mut times, 0), Err(60));
        assert_eq!(admit_read(&mut times, 59_999), Err(1));
        assert_eq!(times.len(), LOCAL_PER_MINUTE as usize);
        assert_eq!(admit_read(&mut times, 60_000), Ok(()));
        assert_eq!(times.len(), 1);
    }

    #[tokio::test]
    async fn local_credentials_never_go_to_a_nonlocal_url_and_a_refusal_never_echoes_its_body() {
        let secret = Secret::new("private-environment-token");
        for base in [
            "https://127.0.0.1:4000",
            "http://localhost:4000",
            "http://example.test:4000",
            "http://127.0.0.1:4000/other",
            "http://name@127.0.0.1:4000",
        ] {
            assert!(matches!(
                read_local(base, Some(&secret), "/ports").await,
                Err(LocalReadError::Address)
            ));
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let service = service_fn(move |request: Request<Incoming>| async move {
                assert_eq!(request.method(), hyper::Method::GET);
                assert_eq!(request.uri(), "/ports");
                assert_eq!(
                    request.headers()[hyper::header::HOST],
                    format!("127.0.0.1:{port}")
                );
                assert_eq!(
                    request.headers()[hyper::header::AUTHORIZATION],
                    "Bearer private-environment-token"
                );
                Ok::<_, Infallible>(text(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "private-environment-token must never be echoed",
                ))
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(socket), service)
                .await;
        });
        let error = read_local(&format!("http://127.0.0.1:{port}"), Some(&secret), "/ports")
            .await
            .unwrap_err();
        assert!(matches!(error, LocalReadError::Refused(503)));
        assert!(!error.to_string().contains(secret.expose()));
        server.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_local_server_that_never_sends_headers_cannot_hold_the_cli_forever() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(HTTP_WITHIN * 2).await;
            drop(socket);
        });
        assert!(matches!(
            read_local(&format!("http://127.0.0.1:{port}"), None, "/ports").await,
            Err(LocalReadError::Unavailable)
        ));
        server.abort();
    }
}
