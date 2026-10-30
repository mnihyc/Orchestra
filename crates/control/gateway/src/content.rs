//! Authenticated content proxy to API storage.
//!
//! Enrollment, renewal and content move over HTTPS beside the session, to the
//! same host and port, and the gateway is where they arrive. It does not
//! serve them: it finds out who is asking, as far as it can see it, and hands
//! the request to the system over the same kind of connection the backplane
//! uses, with the answer to who is asking in headers that only a gateway's
//! certificate can send. Whatever a machine puts in those headers is thrown
//! away; the system trusts only what the gateway wrote.
//!
//! What the gateway can see depends on the port. On the pinned port the
//! connection has already shown the machine's certificate and it is checked;
//! behind the public edge the machine has a token that its session was given,
//! and the gateway knows which session that was. Enrollment is asked by
//! machines that are nobody yet, and is handed over as it is.

use crate::Gateway;
use crate::authentication::{Identified, Unidentified, identify};
use crate::listeners::{Body, Kind, Tls, client_ip, host_of, problem};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use orch_link_contract::content::{BLOBS_PATH, COMMAND_HEADER, MAX_CHUNK_BYTES};
use orch_link_contract::enrollment::{ENROLL_PATH, RENEW_PATH, REQUEST_TIMEOUT_SECS};
use orch_link_contract::version::PATH_PREFIX;
use rustls::pki_types::ServerName;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// Where the system serves what the gateway hands over.
const UPSTREAM_PREFIX: &str = "/internal/link";
/// The header in which the gateway gives the address a machine connected from.
pub const CLIENT_IP_HEADER: &str = "x-orch-client-ip";
/// The header in which the gateway gives the certificate a machine showed in
/// the connection it checked, as unpadded base64url of its DER.
pub const CLIENT_CERTIFICATE_HEADER: &str = "x-orch-client-certificate";
/// The header in which the gateway gives the host the machine connected to.
pub const FORWARDED_HOST_HEADER: &str = "x-orch-forwarded-host";
/// The header in which the gateway gives the machine it found the request to
/// be from.
pub const MACHINE_HEADER: &str = "x-orch-machine";
/// The header in which the gateway gives the fingerprint of the certificate
/// that machine holds its session with.
pub const MACHINE_FINGERPRINT_HEADER: &str = "x-orch-machine-fingerprint";
/// What a machine that renews through the public edge sends to prove it holds
/// the key of its certificate: the certificate, the signature and the moment.
const RENEW_HEADERS: [&str; 3] = [
    "x-orch-renew-certificate",
    "x-orch-renew-signature",
    "x-orch-renew-sent-at",
];
/// The headers of a machine's request that are passed on. Nothing else is: in
/// particular not the headers that say who the machine is.
const PASSED_ON: [&str; 6] = [
    "content-type",
    "content-length",
    "accept",
    "range",
    "user-agent",
    COMMAND_HEADER,
];
/// The headers of the system's answer that are not passed on: they describe
/// the connection to the system, not the one to the machine.
const NOT_PASSED_BACK: [&str; 4] = ["connection", "keep-alive", "transfer-encoding", "upgrade"];
/// How large a request to enroll or renew may be. A certificate request is
/// a few hundred bytes.
const SMALL_BODY_BYTES: usize = 64 * 1024;
/// How large a question which content is missing may be: a thousand hashes
/// of seventy characters, and room.
const MISSING_BODY_BYTES: usize = 128 * 1024;
/// What an upload's body may exceed a piece's size by: nothing is meant to,
/// and a request line's worth of room is not a way to carry more.
const PIECE_SLACK_BYTES: usize = 4 * 1024;
/// How long the gateway waits to reach the system.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

type ProxyError = Box<dyn std::error::Error + Send + Sync + 'static>;

// Concrete conversions keep the spawned request future independent of the lifetime of a borrowed error.
fn proxy_error(error: hyper::Error) -> ProxyError {
    Box::new(error)
}

fn no_body_error(never: std::convert::Infallible) -> ProxyError {
    match never {}
}

/// Whether the path is one that is handed to the system.
#[must_use]
pub fn serves(path: &str) -> bool {
    path.starts_with("/api/v1/downloads/")
        || path == ENROLL_PATH
        || path == RENEW_PATH
        || path == BLOBS_PATH
        || path
            .strip_prefix(BLOBS_PATH)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// What is asked, and what it takes to be allowed to ask.
struct Kinded {
    /// Whether the request has to be from a machine the gateway can tell.
    needs_machine: bool,
    /// The most bytes of body.
    body_limit: usize,
    /// How long all of it may take, where it is bounded.
    patience: Option<Duration>,
}

fn kinded(method: &Method, path: &str) -> Option<Kinded> {
    let request_time = Some(Duration::from_secs(u64::from(REQUEST_TIMEOUT_SECS)));
    if path.starts_with("/api/v1/downloads/") && matches!(*method, Method::GET | Method::HEAD) {
        return Some(Kinded {
            needs_machine: false,
            body_limit: 0,
            patience: None,
        });
    }
    if path == ENROLL_PATH && method == Method::POST {
        return Some(Kinded {
            needs_machine: false,
            body_limit: SMALL_BODY_BYTES,
            patience: request_time,
        });
    }
    if path == RENEW_PATH && method == Method::POST {
        return Some(Kinded {
            needs_machine: false,
            body_limit: SMALL_BODY_BYTES,
            patience: request_time,
        });
    }
    if serves(path) && path.starts_with(BLOBS_PATH) {
        let body_limit = if method == Method::PUT {
            usize::try_from(MAX_CHUNK_BYTES).unwrap_or(usize::MAX) + PIECE_SLACK_BYTES
        } else if method == Method::POST {
            MISSING_BODY_BYTES
        } else if method == Method::GET || method == Method::HEAD {
            0
        } else {
            return None;
        };
        return Some(Kinded {
            needs_machine: true,
            body_limit,
            patience: None,
        });
    }
    None
}

fn insert(headers: &mut HeaderMap, name: &str, value: &str) {
    if let (Ok(name), Ok(value)) = (
        HeaderName::from_bytes(name.as_bytes()),
        HeaderValue::from_str(value),
    ) {
        headers.insert(name, value);
    }
}

/// Hands a request to the system and gives its answer back.
pub(crate) async fn forward(
    gateway: &Gateway,
    kind: Kind,
    tls: &Tls,
    remote: IpAddr,
    request: Request<Incoming>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let Some(route) = kinded(&parts.method, parts.uri.path()) else {
        return problem(
            StatusCode::METHOD_NOT_ALLOWED,
            "request.invalid",
            "that request is not made here",
        );
    };
    let identified: Option<Identified> = match identify(gateway, kind, tls, &parts.headers) {
        Ok(who) => Some(who),
        Err(Unidentified::NothingShown) if !route.needs_machine => None,
        Err(Unidentified::NotGood) if !route.needs_machine => None,
        Err(Unidentified::NothingShown) => {
            return problem(
                StatusCode::UNAUTHORIZED,
                "auth.required",
                "show who you are",
            );
        }
        Err(Unidentified::NotGood) => {
            return problem(
                StatusCode::UNAUTHORIZED,
                "auth.invalid_token",
                "what was shown is not good",
            );
        }
    };

    let path = parts.uri.path();
    let rest = path.strip_prefix(PATH_PREFIX).unwrap_or(path);
    let forwarded = if path.starts_with("/api/v1/downloads/") {
        path.to_owned()
    } else {
        format!("{UPSTREAM_PREFIX}{rest}")
    };
    let target = match parts.uri.query() {
        Some(query) => format!("{forwarded}?{query}"),
        None => forwarded,
    };
    let mut headers = HeaderMap::new();
    for name in PASSED_ON.iter().chain(RENEW_HEADERS.iter()) {
        if let Some(value) = parts.headers.get(*name) {
            headers.insert(HeaderName::from_static(name), value.clone());
        }
    }
    insert(
        &mut headers,
        CLIENT_IP_HEADER,
        &client_ip(
            kind,
            remote,
            &parts.headers,
            &gateway.inner.settings.trusted_proxy_cidrs,
            gateway.inner.clock.now().timestamp(),
        )
        .to_string(),
    );
    if let Some(host) = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        insert(&mut headers, FORWARDED_HOST_HEADER, &host_of(host));
    }
    if let Some(certificate) = &tls.client_certificate {
        insert(
            &mut headers,
            CLIENT_CERTIFICATE_HEADER,
            &URL_SAFE_NO_PAD.encode(certificate.der()),
        );
    }
    if let Some(who) = &identified {
        insert(&mut headers, MACHINE_HEADER, &who.machine_id.to_string());
        insert(&mut headers, MACHINE_FINGERPRINT_HEADER, &who.fingerprint);
    }

    let work = relay_request(
        gateway,
        parts.method,
        &target,
        headers,
        Limited::new(body, route.body_limit).boxed(),
    );
    let outcome = match route.patience {
        Some(patience) => tokio::time::timeout(patience, work)
            .await
            .unwrap_or(Err(Upstream::TimedOut)),
        None => work.await,
    };
    match outcome {
        Ok(response) => response,
        Err(Upstream::TimedOut) => problem(
            StatusCode::GATEWAY_TIMEOUT,
            "internal",
            "the system did not answer in time",
        ),
        Err(Upstream::Unreachable) => problem(
            StatusCode::BAD_GATEWAY,
            "internal",
            "the system cannot be reached just now",
        ),
    }
}

enum Upstream {
    Unreachable,
    TimedOut,
}

/// Sends `body` as JSON to the system's internal `path` for the gateway itself (its certificate's renewal), on
/// a connection of its own, and gives the status and the answer, of at most `limit` bytes.
pub(crate) async fn ask_system(
    gateway: &Gateway,
    path: &str,
    body: Vec<u8>,
    limit: usize,
) -> Result<(StatusCode, bytes::Bytes), String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let body: Body = http_body_util::Full::new(bytes::Bytes::from(body))
        .map_err(no_body_error)
        .boxed();
    let response = relay_request(gateway, Method::POST, path, headers, body)
        .await
        .map_err(|fault| match fault {
            Upstream::Unreachable => "the system could not be reached".to_owned(),
            Upstream::TimedOut => "the system did not take the connection in time".to_owned(),
        })?;
    let status = response.status();
    // Read frame by frame rather than through `Limited`: its boxed-error conversion makes this future fail the
    // `Send` check of the task that spawns it (rustc's higher-ranked lifetimes; the renewal loop is spawned).
    let mut body = response.into_body();
    let mut answer = bytes::BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| format!("the answer could not be read: {error}"))?;
        if let Ok(data) = frame.into_data() {
            if answer.len() + data.len() > limit {
                return Err(format!("the answer is longer than {limit} bytes"));
            }
            answer.extend_from_slice(&data);
        }
    }
    Ok((status, answer.freeze()))
}

/// Sends one request to the system over a connection of its own and returns
/// its answer, whose body is passed on as it arrives.
async fn relay_request(
    gateway: &Gateway,
    method: Method,
    target: &str,
    headers: HeaderMap,
    body: Body,
) -> Result<Response<Body>, Upstream> {
    let url = url::Url::parse(&gateway.inner.settings.backplane_url)
        .map_err(|_| Upstream::Unreachable)?;
    let host = url.host_str().ok_or(Upstream::Unreachable)?.to_owned();
    let port = url.port_or_known_default().ok_or(Upstream::Unreachable)?;
    let name = ServerName::try_from(host.clone()).map_err(|_| Upstream::Unreachable)?;
    let socket = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host.clone(), port)))
        .await
        .map_err(|_| Upstream::TimedOut)?
        .map_err(|_| Upstream::Unreachable)?;
    let _ = socket.set_nodelay(true);
    let tls = TlsConnector::from(Arc::clone(&gateway.inner.upstream_tls))
        .connect(name, socket)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "the system refused the gateway's connection");
            Upstream::Unreachable
        })?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(tls))
            .await
            .map_err(|_| Upstream::Unreachable)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut upstream = Request::new(body);
    *upstream.method_mut() = method;
    *upstream.uri_mut() = target.parse().map_err(|_| Upstream::Unreachable)?;
    *upstream.headers_mut() = headers;
    insert(upstream.headers_mut(), "host", &format!("{host}:{port}"));
    let response = sender.send_request(upstream).await.map_err(|error| {
        tracing::warn!(%error, "a request could not be handed to the system");
        Upstream::Unreachable
    })?;
    let (mut parts, incoming) = response.into_parts();
    for name in NOT_PASSED_BACK {
        parts.headers.remove(name);
    }
    Ok(Response::from_parts(
        parts,
        incoming.map_err(proxy_error).boxed(),
    ))
}
