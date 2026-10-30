//! Pinned mutual-TLS and public-trust edge listeners.
//!
//! Machines reach a gateway in two ways. On the **pinned** port they connect
//! under the system's own authority: the gateway shows its certificate, the
//! machine verifies it against that authority, and a machine that has a
//! certificate shows it. A machine that has none yet is not turned away at
//! the connection, because the first thing it does is enroll; what it may do
//! without a certificate is checked request by request. On the **edge** port,
//! behind the public front, the connection is TLS under the same authority
//! between the front and the gateway, and shows no machine at all: a machine
//! there proves itself in the handshake of its session, or by the token the
//! session gave it.
//!
//! Both ports speak HTTP/1.1. A request for the session path that asks to be
//! upgraded becomes a session; a request for enrollment, renewal or content
//! is handed to [`crate::content`]; the rest is not found.

use crate::Gateway;
use crate::content;
use bytes::Bytes;
use http::{HeaderMap, Method, Request, Response, StatusCode, header};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use orch_link::framing::PeerInfo;
use orch_link_contract::version::{SESSION_PATH, SUBPROTOCOL};
use orch_pki::PkiError;
use orch_pki::certificates::Certificate;
use orch_pki::service_renew::LiveIdentity;
use orch_pki::verification::{ClockTime, OrchClientVerifier, crypto, presented_certificate};
use orch_types::Clock;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    DigitallySignedStruct, DistinguishedName, Error as TlsError, ServerConfig, SignatureScheme,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_util::task::TaskTracker;

/// How long a connection has to finish its TLS handshake: as long as a
/// machine allows itself to connect.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a connection has to send the head of a request once it begins.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long what is under way is let finish when the listener is told to
/// stop.
const DRAIN: Duration = Duration::from_secs(5);
/// The most a single message may have on a session's socket, whatever limit
/// the session states later: the same bound the link's own sockets have.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What every answer of the gateway is made of.
pub(crate) type Body = BoxBody<Bytes, BoxError>;

/// A body of text.
pub(crate) fn full(text: impl Into<Bytes>) -> Body {
    Full::new(text.into())
        .map_err(|never| match never {})
        .boxed()
}

/// An answer that is a problem, in the form the system answers them.
pub(crate) fn problem(status: StatusCode, code: &str, detail: &str) -> Response<Body> {
    let body = format!(
        "{{\"type\":\"orch:error:{code}\",\"code\":\"{code}\",\"status\":{},\"detail\":\"{detail}\"}}",
        status.as_u16()
    );
    let mut response = Response::new(full(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/problem+json"),
    );
    response
}

/// Which port a connection came in on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The pinned port: under the system's authority, asking for the
    /// machine's certificate.
    Pinned,
    /// The port behind the public edge: no machine is shown.
    Edge,
}

/// What the TLS handshake of a connection found out.
#[derive(Debug, Clone, Default)]
pub struct Tls {
    /// The certificate the client showed, which the handshake accepted.
    pub client_certificate: Option<Certificate>,
    /// The name the client asked for in the handshake.
    pub server_name: Option<String>,
}

// ---------------------------------------------------------------- the pinned port's TLS

/// Takes a client certificate when there is one, judged as the pinned path
/// judges it, and takes a client without one as well: it may enroll.
#[derive(Debug)]
struct CertificateIfAny {
    inner: Arc<OrchClientVerifier>,
}

impl ClientCertVerifier for CertificateIfAny {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        self.inner
            .verify_client_cert(end_entity, intermediates, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// The TLS configuration of the pinned port: the gateway's certificate as it
/// stands at each handshake (a renewed one from the next), and a certificate
/// of the system asked of every client, one that ran out less than
/// `grace_days` ago included, so that its request to renew is heard.
pub fn pinned_server_config(
    identity: Arc<LiveIdentity>,
    authority: &Certificate,
    grace_days: u64,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    /// Seconds in a day.
    const DAY_SECS: u64 = 86_400;
    let provider = crypto();
    let inner = OrchClientVerifier::new(
        authority,
        Duration::from_secs(grace_days.saturating_mul(DAY_SECS)),
        &provider,
    )?;
    let config =
        ServerConfig::builder_with_details(Arc::clone(&provider), ClockTime::provider(clock))
            .with_safe_default_protocol_versions()
            .map_err(|error| PkiError::Certificate(error.to_string()))?
            .with_client_cert_verifier(Arc::new(CertificateIfAny { inner }))
            .with_cert_resolver(identity);
    Ok(Arc::new(config))
}

// ---------------------------------------------------------------- serving

/// Takes connections on `listener` until the gateway is told to stop.
pub(crate) async fn serve(gateway: Gateway, listener: TcpListener, kind: Kind) {
    let tls = match kind {
        Kind::Pinned => Arc::clone(&gateway.inner.pinned_tls),
        Kind::Edge => Arc::clone(&gateway.inner.edge_tls),
    };
    let acceptor = TlsAcceptor::from(tls);
    let tracker = TaskTracker::new();
    loop {
        let accepted = tokio::select! {
            () = gateway.inner.shutdown.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let Ok((socket, remote)) = accepted else {
            continue;
        };
        let (acceptor, gateway) = (acceptor.clone(), gateway.clone());
        tracker.spawn(async move { connection(gateway, acceptor, kind, socket, remote).await });
    }
    drop(listener);
    tracker.close();
    let _ = tokio::time::timeout(DRAIN, tracker.wait()).await;
}

async fn connection(
    gateway: Gateway,
    acceptor: TlsAcceptor,
    kind: Kind,
    socket: tokio::net::TcpStream,
    remote: SocketAddr,
) {
    // Small frames (acknowledgements, credit) must not wait to be joined by
    // others.
    let _ = socket.set_nodelay(true);
    let handshake = tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(socket)).await;
    let stream = match handshake {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            tracing::debug!(%remote, %error, "a TLS handshake failed");
            return;
        }
        Err(_) => return,
    };
    let session = stream.get_ref().1;
    let tls = Arc::new(Tls {
        client_certificate: presented_certificate(session.peer_certificates()),
        server_name: session.server_name().map(str::to_ascii_lowercase),
    });
    let shutdown = gateway.inner.shutdown.clone();
    let service = hyper::service::service_fn(move |request: Request<Incoming>| {
        let (gateway, tls) = (gateway.clone(), Arc::clone(&tls));
        async move {
            Ok::<_, std::convert::Infallible>(
                handle(gateway, kind, tls, remote.ip(), request).await,
            )
        }
    });
    let connection = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    tokio::pin!(connection);
    tokio::select! {
        result = connection.as_mut() => {
            if let Err(error) = result {
                tracing::debug!(%remote, %error, "a connection ended badly");
            }
        }
        () = shutdown.cancelled() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

async fn handle(
    gateway: Gateway,
    kind: Kind,
    tls: Arc<Tls>,
    remote: IpAddr,
    request: Request<Incoming>,
) -> Response<Body> {
    let path = request.uri().path();
    if path == SESSION_PATH {
        return upgrade(gateway, kind, &tls, remote, request);
    }
    if content::serves(path) {
        return content::forward(&gateway, kind, &tls, remote, request).await;
    }
    problem(
        StatusCode::NOT_FOUND,
        "not_found",
        "there is nothing at that path",
    )
}

/// The caller's address, resolved by the same proxy rule as the API.
pub(crate) fn client_ip(
    kind: Kind,
    remote: IpAddr,
    headers: &HeaderMap,
    trusted_proxy_cidrs: &[String],
    now: i64,
) -> IpAddr {
    let proxies = if kind == Kind::Edge {
        trusted_proxy_cidrs
    } else {
        &[]
    };
    if kind == Kind::Edge {
        orch_config::client_address::resolve_observed(remote, headers, proxies, now)
    } else {
        orch_config::client_address::resolve(remote, headers, proxies)
    }
}

/// The host of a `Host` header, without a port, in lowercase.
pub(crate) fn host_of(header: &str) -> String {
    let without_port = match header.strip_prefix('[') {
        // An IPv6 literal: keep what is inside the brackets.
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        None => header.split(':').next().unwrap_or(header),
    };
    without_port.to_ascii_lowercase()
}

/// Whether the header `name` lists `token`.
fn lists(headers: &HeaderMap, name: header::HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|item| item.trim().eq_ignore_ascii_case(token))
}

/// Turns a request for the session path into a session, or says why not.
fn upgrade(
    gateway: Gateway,
    kind: Kind,
    tls: &Arc<Tls>,
    remote: IpAddr,
    mut request: Request<Incoming>,
) -> Response<Body> {
    let headers = request.headers();
    let key = headers
        .get("sec-websocket-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let is_upgrade = request.method() == Method::GET
        && lists(headers, header::CONNECTION, "upgrade")
        && lists(headers, header::UPGRADE, "websocket")
        && headers
            .get("sec-websocket-version")
            .is_some_and(|version| version == "13");
    let Some(key) = key.filter(|_| is_upgrade) else {
        return problem(
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "a session is a WebSocket",
        );
    };
    if !lists(headers, header::SEC_WEBSOCKET_PROTOCOL, SUBPROTOCOL) {
        return problem(
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "the subprotocol orch-link.v1 is required",
        );
    }
    if kind == Kind::Pinned && tls.client_certificate.is_none() {
        return problem(
            StatusCode::FORBIDDEN,
            "authz.denied",
            "a client certificate is required here",
        );
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(host_of)
        .or_else(|| tls.server_name.clone());
    let peer = PeerInfo {
        remote_ip: Some(client_ip(
            kind,
            remote,
            headers,
            &gateway.inner.settings.trusted_proxy_cidrs,
            gateway.inner.clock.now().timestamp(),
        )),
        host,
        client_certificate: tls.client_certificate.clone(),
    };
    let accept = derive_accept_key(key.as_bytes());
    let upgraded = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let upgraded = match upgraded.await {
            Ok(upgraded) => upgraded,
            Err(error) => {
                tracing::debug!(%error, "a session could not be upgraded");
                return;
            }
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let socket =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, Some(config))
                .await;
        let connection = orch_link::websocket::connection_of(socket, peer);
        crate::sessions::serve_connection(gateway, connection).await;
    });
    let mut response = Response::new(full(Bytes::new()));
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
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
