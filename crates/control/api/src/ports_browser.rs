//! The browser route of a port: a request to its stable name, let in by a person's entry or a link and carried to the program with who is calling.
//!
//! A port's stable name (`e<environment>-p<port>.<ports domain>`) reaches the API through the edge.
//! Nobody is let in by the name alone. A signed-in person enters with a single-use code from
//! `POST …/ports/{port}/enter` and is given a cookie for that name alone; a link's holder comes with
//! the link's proof in `orch_token`, which a webhook sends with each call and a browser exchanges for
//! the same cookie. Every request asks its admission again. Orch's cookie and token never reach the
//! program: the caller's forwarding and identity headers are replaced by Orch's, with the signed
//! assertion beside them. Each request is carried on an inside connection of its own, so that no two
//! callers ever share one, and an upgrade (a WebSocket) goes on over it. A stopped environment, or a
//! port nothing serves, shows Orch's page saying so.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::ports_runtime::{BROWSER_ENTRY_HOURS, BrowserEntry};
use crate::routes::port_entry::port_signer;
use crate::routes::ports::{environment, may_use, may_use_link, proof_matches};
use crate::state::App;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use hyper_util::rt::TokioIo;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ports::{
    PortCaller, PortDiscovery, PortHttpScheme, PortProgram, PortTransport,
};
use orch_db::queries::environments::routes::EnvironmentFull;
use orch_db::queries::ports as rows;
use orch_pki::port_assertions::{AssertedCaller, PortAssertion};
use orch_types::{Id, Time, new_id};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

/// The cookie that carries a browser's entry to one port's name: host-only, for that name alone.
pub const ENTRY_COOKIE: &str = "__Host-orch-port";
/// The path a person's browser enters by, with its single-use code.
pub const ENTER_PATH: &str = "/.orch/enter";
/// The query member that carries a link's proof to the stable name.
pub const LINK_TOKEN: &str = "orch_token";
/// What a caller may say about forwarding and identity, all of which Orch replaces.
const REPLACED: [&str; 10] = [
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-proto",
    "x-forwarded-host",
    "orch-user",
    "orch-user-id",
    "orch-user-name",
    "orch-link",
    "orch-opening",
    "orch-assertion",
];
/// Headers of one hop only, which are never passed on (an upgrade keeps its two).
const HOP_BY_HOP: [&str; 7] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Sends a request for a port's stable name to the port, and every other request on.
pub async fn dispatch(State(app): State<App>, request: Request, next: Next) -> Response {
    match target(&app, request.headers()) {
        Some((environment, port, host)) => serve(&app, environment, port, host, request).await,
        None => next.run(request).await,
    }
}

/// The environment and the port that a Host names under the ports domain, and the name itself.
fn target(app: &App, headers: &HeaderMap) -> Option<(Id, u16, String)> {
    let domain = app
        .config
        .edge
        .ports_domain
        .as_deref()?
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if domain.is_empty() {
        return None;
    }
    let host = headers
        .get(header::HOST)?
        .to_str()
        .ok()?
        .to_ascii_lowercase();
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host.as_str(),
    };
    let label = name.strip_suffix(&format!(".{domain}"))?;
    let (environment, port) = label.strip_prefix('e')?.split_once("-p")?;
    if environment.len() != 32 {
        return None;
    }
    let environment = Id::parse_str(environment).ok()?;
    let port = port.parse::<u16>().ok()?;
    Some((environment, port, name.to_owned()))
}

/// Who a request comes from: the connection's address, or the caller's where a named proxy says
/// it; the caller's own port only where the connection is theirs.
fn outside_of(app: &App, request: &Request) -> SocketAddr {
    let Some(peer) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
    };
    let address = orch_config::client_address::resolve_observed(
        peer.0.ip(),
        request.headers(),
        &app.config.api.trusted_proxy_cidrs,
        app.clock.now().timestamp(),
    );
    let port = if address == peer.0.ip().to_canonical() {
        peer.0.port()
    } else {
        0
    };
    SocketAddr::new(address, port)
}

async fn serve(app: &App, environment: Id, port: u16, host: String, request: Request) -> Response {
    let now = Time::new(app.clock.now());
    let query = request.uri().query().unwrap_or_default().to_owned();
    // A person's browser enters with its single-use code, once.
    if request.uri().path() == ENTER_PATH {
        let code = form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.into_owned());
        let Some(entry) =
            code.and_then(|code| app.cache.ports().redeem_code(&code, environment, port, now))
        else {
            return page(
                StatusCode::FORBIDDEN,
                "This way in has expired or was used already",
                "Open the port again from its page in Orch.",
            );
        };
        return remembered(app, entry, now, "/");
    }
    let proofs: Vec<String> = form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key == LINK_TOKEN)
        .map(|(_, value)| value.into_owned())
        .collect();
    if proofs.len() > 1 {
        return page(
            StatusCode::BAD_REQUEST,
            "This address holds more than one link",
            "Use the link as it was given.",
        );
    }
    let entry = if let Some(proof) = proofs.first() {
        let Some(entry) = link_entry(app, environment, port, proof, now).await else {
            return page(
                StatusCode::FORBIDDEN,
                "This link cannot be used",
                "It has lapsed, was closed or is for another address.",
            );
        };
        if matches!(*request.method(), Method::GET | Method::HEAD) {
            // A browser takes the link's cookie for this name and goes on at the clean address.
            return remembered(app, entry, now, &clean_path(request.uri()));
        }
        entry
    } else if let Some(entry) = cookie_of(request.headers())
        .and_then(|token| app.cache.ports().browser(&token, environment, port, now))
    {
        entry
    } else {
        return page(
            StatusCode::UNAUTHORIZED,
            "Open this port from its page in Orch",
            "A port's address lets nobody in by itself.",
        );
    };
    match admitted(app, &entry, now).await {
        Ok(admission) => carry(app, &entry, admission, host, request, now).await,
        Err(error) => refused(&error),
    }
}

/// The page of a refusal, by what refused it.
fn refused(error: &ApiError) -> Response {
    match error.code() {
        ErrorCode::PortsUnavailable => page(
            StatusCode::SERVICE_UNAVAILABLE,
            "This port's environment is stopped or out of reach",
            "It works again once the environment runs. Nothing starts it from here.",
        ),
        ErrorCode::PortsNotListening => page(
            StatusCode::BAD_GATEWAY,
            "Nothing serves this port now",
            "Start the program in the environment, then reload this page.",
        ),
        _ => page(
            StatusCode::FORBIDDEN,
            "You cannot use this port now",
            "Who may use it has changed, or its opening ended.",
        ),
    }
}

/// The link opening of this name whose proof `proof` is.
async fn link_entry(
    app: &App,
    environment: Id,
    port: u16,
    proof: &str,
    now: Time,
) -> Option<BrowserEntry> {
    let mut conn = app.db.acquire().await.ok()?;
    let row = rows::openings(&mut conn, environment)
        .await
        .ok()?
        .into_iter()
        .find(|row| {
            row.kind == "link" && row.port == i32::from(port) && proof_matches(row, proof)
        })?;
    Some(BrowserEntry {
        person: None,
        environment,
        port,
        opening: Some(row.id),
        expires: Time::new(now.get() + chrono::Duration::hours(BROWSER_ENTRY_HOURS)),
    })
}

/// What one admitted request carries: the environment, the caller, the program's options.
struct Admission {
    env: EnvironmentFull,
    caller: PortCaller,
    revision: i64,
    program: PortProgram,
}

/// Asks an entry's admission again, as every request does, and reads what carrying it needs.
async fn admitted(app: &App, entry: &BrowserEntry, now: Time) -> ApiResult<Admission> {
    let mut tx = app.db.begin().await?;
    let env = environment(&mut tx, entry.environment).await?;
    let (caller, revision) = match entry.person {
        Some(person) => {
            let principal = orch_authz::Principal::load(
                &mut tx,
                orch_authz::tokens::Caller::session(person),
                now,
            )
            .await
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::PortsNotAllowed,
                    "the person cannot use this port",
                )
            })?;
            may_use(
                &mut tx,
                &principal,
                &env,
                entry.port,
                PortTransport::Tcp,
                entry.opening,
            )
            .await?;
            let user = refusals::user_refs(&mut tx, &[person])
                .await?
                .remove(&person)
                .ok_or_else(ApiError::not_found)?;
            let revision = match entry.opening {
                Some(opening) => {
                    rows::opening(&mut tx, opening)
                        .await?
                        .ok_or_else(ApiError::not_found)?
                        .revision
                }
                None => env.revision,
            };
            (PortCaller::Person { user }, revision)
        }
        None => {
            let row = rows::opening(&mut tx, entry.opening.ok_or_else(ApiError::not_found)?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if !row.entry_methods.iter().any(|method| method == "browser") {
                return Err(ApiError::new(
                    ErrorCode::PortsNotAllowed,
                    "this link does not admit a browser",
                ));
            }
            let caller = may_use_link(&mut tx, &env, &row, PortTransport::Tcp, now).await?;
            (caller, row.revision)
        }
    };
    if let Some(snapshot) = app
        .cache
        .ports()
        .snapshot(env.id, i64::from(env.generation))
        && snapshot.discovery == PortDiscovery::Native
        && !snapshot.listeners.iter().any(|listener| {
            listener.port == entry.port
                && listener.transport == PortTransport::Tcp
                && !listener.internal
                && listener.unavailable_reason.is_none()
        })
    {
        return Err(ApiError::new(
            ErrorCode::PortsNotListening,
            "nothing serves this port now",
        ));
    }
    let (program, _) = rows::program(&mut tx, env.id, entry.port).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Admission {
        env,
        caller,
        revision,
        program,
    })
}

/// Carries one admitted request on an inside connection of its own.
async fn carry(
    app: &App,
    entry: &BrowserEntry,
    admission: Admission,
    host: String,
    mut request: Request,
    now: Time,
) -> Response {
    let outside = outside_of(app, &request);
    let connection = new_id();
    let upgrading = request
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|value| !value.is_empty());
    let incoming = upgrading.then(|| hyper::upgrade::on(&mut request));
    let (mut parts, body) = request.into_parts();
    rewrite_request(&mut parts, upgrading);
    let signed = port_signer().sign(
        &PortAssertion {
            issuer: app.config.system.public_url.clone(),
            environment_id: admission.env.id,
            port: entry.port,
            transport: "tcp",
            opening_id: entry.opening,
            connection_id: connection,
            generation: i64::from(admission.env.generation),
            outside_address: outside.ip().to_string(),
            outside_port: outside.port(),
            caller: match &admission.caller {
                PortCaller::Person { user } => AssertedCaller::Person {
                    id: user.id,
                    username: user.username.clone(),
                    display_name: user.display_name.clone(),
                },
                PortCaller::Link { id, label } => AssertedCaller::Link {
                    id: *id,
                    label: label.clone(),
                },
                PortCaller::Address => {
                    return page(StatusCode::FORBIDDEN, "You cannot use this port now", "");
                }
            },
        },
        now,
    );
    let Ok(assertion) = signed else {
        return page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Who is calling cannot be told to the program now",
            "Try again in a moment.",
        );
    };
    identify(
        &mut parts.headers,
        &admission,
        entry,
        &host,
        outside,
        &assertion,
    );
    if let Some(expected) = &admission.program.host_header
        && let Ok(value) = HeaderValue::from_str(expected)
    {
        parts.headers.insert(header::HOST, value);
    }
    let tunnel = match app
        .cache
        .ports()
        .open_browser(
            app,
            &admission.env,
            entry.port,
            entry.opening,
            admission.caller.clone(),
            entry.person,
            admission.revision,
            outside,
            connection,
        )
        .await
    {
        Ok(tunnel) => tunnel,
        Err(error) => return refused(&error),
    };
    let request = Request::from_parts(parts, body);
    let answered = if admission.program.http_scheme == PortHttpScheme::Https {
        let name = admission
            .program
            .host_header
            .as_deref()
            .and_then(|host| host.split(':').next())
            .unwrap_or("localhost")
            .to_owned();
        let Ok(name) = ServerName::try_from(name) else {
            return page(
                StatusCode::BAD_GATEWAY,
                "The program's address is not a name",
                "",
            );
        };
        let Some(tls) = backend_tls() else {
            return page(
                StatusCode::BAD_GATEWAY,
                "HTTPS to the program cannot be spoken here",
                "",
            );
        };
        match tokio_rustls::TlsConnector::from(tls)
            .connect(name, tunnel)
            .await
        {
            Ok(tls) => exchange(tls, request, incoming).await,
            Err(_) => Err(()),
        }
    } else {
        exchange(tunnel, request, incoming).await
    };
    match answered {
        Ok(response) => response,
        Err(()) => page(
            StatusCode::BAD_GATEWAY,
            "The program did not answer",
            "It may have stopped or refused the connection; reload to try again.",
        ),
    }
}

/// Takes out what a caller says of forwarding and identity, Orch's own cookie and the link's
/// token, and what belongs to one hop only.
fn rewrite_request(parts: &mut http::request::Parts, upgrading: bool) {
    for name in REPLACED {
        parts.headers.remove(name);
    }
    for name in HOP_BY_HOP {
        if upgrading && (name == "connection" || name == "upgrade") {
            continue;
        }
        parts.headers.remove(name);
    }
    strip_cookie(&mut parts.headers);
    if let Ok(uri) = clean_path(&parts.uri).parse() {
        parts.uri = uri;
    }
}

/// Sets the forwarding headers, the caller's identity and the assertion.
fn identify(
    headers: &mut HeaderMap,
    admission: &Admission,
    entry: &BrowserEntry,
    host: &str,
    outside: SocketAddr,
    assertion: &str,
) {
    let mut set = |name: &'static str, value: String| {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(HeaderName::from_static(name), value);
        }
    };
    let address = outside.ip().to_string();
    set("x-forwarded-for", address.clone());
    set("x-forwarded-proto", "https".to_owned());
    set("x-forwarded-host", host.to_owned());
    set(
        "forwarded",
        format!("for=\"{address}\";proto=https;host=\"{host}\""),
    );
    match &admission.caller {
        PortCaller::Person { user } => {
            set("orch-user", user.username.clone());
            set("orch-user-id", user.id.to_string());
            set("orch-user-name", percent(&user.display_name));
        }
        PortCaller::Link { label, .. } => set("orch-link", percent(label)),
        PortCaller::Address => {}
    }
    set(
        "orch-opening",
        entry.opening.map_or_else(
            || format!("own:{}:{}", admission.env.id, entry.port),
            |opening| opening.to_string(),
        ),
    );
    set("orch-assertion", assertion.to_owned());
}

/// Sends one request over an inside connection and answers what the program answered; an upgrade
/// goes on between the caller and the program until either ends.
async fn exchange<I>(
    io: I,
    request: Request,
    incoming: Option<hyper::upgrade::OnUpgrade>,
) -> Result<Response, ()>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .map_err(|_| ())?;
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let mut response = sender.send_request(request).await.map_err(|_| ())?;
    if response.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(incoming) = incoming
    {
        let outgoing = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            if let (Ok(caller), Ok(program)) = (incoming.await, outgoing.await) {
                let _ = tokio::io::copy_bidirectional(
                    &mut TokioIo::new(caller),
                    &mut TokioIo::new(program),
                )
                .await;
            }
        });
        let (parts, _) = response.into_parts();
        return Ok(Response::from_parts(parts, Body::empty()));
    }
    let (mut parts, body) = response.into_parts();
    for name in HOP_BY_HOP {
        parts.headers.remove(name);
    }
    Ok(Response::from_parts(parts, Body::new(body)))
}

/// Remembers a browser's entry under a cookie for this name alone and sends it on to `location`.
fn remembered(app: &App, entry: BrowserEntry, now: Time, location: &str) -> Response {
    let token = app.cache.ports().start_browser(entry, now);
    let lifetime = chrono::Duration::hours(BROWSER_ENTRY_HOURS).num_seconds();
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    if let Ok(cookie) = HeaderValue::from_str(&format!(
        "{ENTRY_COOKIE}={token}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={lifetime}"
    )) {
        headers.insert(header::SET_COOKIE, cookie);
    }
    if let Ok(location) = HeaderValue::from_str(location) {
        headers.insert(header::LOCATION, location);
    }
    quiet(headers);
    response
}

/// The value of Orch's entry cookie among the caller's cookies.
fn cookie_of(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == ENTRY_COOKIE)
        .map(|(_, value)| value.to_owned())
}

/// The caller's cookies without Orch's own.
fn strip_cookie(headers: &mut HeaderMap) {
    let kept: Vec<String> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .filter(|pair| {
            !pair.is_empty()
                && pair
                    .split_once('=')
                    .is_none_or(|(name, _)| name != ENTRY_COOKIE)
        })
        .map(str::to_owned)
        .collect();
    headers.remove(header::COOKIE);
    if !kept.is_empty()
        && let Ok(value) = HeaderValue::from_str(&kept.join("; "))
    {
        headers.insert(header::COOKIE, value);
    }
}

/// The path and query of `uri` without the link's token.
fn clean_path(uri: &http::Uri) -> String {
    let path = uri.path();
    let Some(query) = uri.query() else {
        return path.to_owned();
    };
    let kept: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key != LINK_TOKEN)
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        return path.to_owned();
    }
    let query = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(kept)
        .finish();
    format!("{path}?{query}")
}

/// UTF-8 text as percent-encoded ASCII, every byte but the unreserved ones encoded.
fn percent(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Headers that keep a page of Orch's, and an entry, out of caches and referrers.
fn quiet(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

/// Orch's own page at a port's name, which says what is so and what can be done.
fn page(status: StatusCode, title: &'static str, line: &'static str) -> Response {
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title>\
         <meta name=\"viewport\" content=\"width=device-width\"></head>\
         <body style=\"font-family:system-ui,sans-serif;max-width:36rem;margin:4rem auto;padding:0 1rem\">\
         <h1 style=\"font-size:1.25rem\">{title}</h1><p>{line}</p></body></html>"
    );
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    quiet(headers);
    response
}

/// A program inside answers HTTPS with whatever certificate it has: Orch does not check it, as the
/// program option says; the connection is inside the environment's own carrier.
#[derive(Debug)]
struct AnyCertificate(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// The TLS configuration toward a program that serves HTTPS: the system's own provider, any
/// certificate. None where the provider speaks no default version.
fn backend_tls() -> Option<Arc<rustls::ClientConfig>> {
    let provider = orch_pki::verification::crypto();
    let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .ok()?;
    Some(Arc::new(
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
            .with_no_client_auth(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orch_s_cookie_and_the_link_token_never_reach_the_program() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; __Host-orch-port=secret"),
        );
        headers.append(header::COOKIE, HeaderValue::from_static("lang=en"));
        assert_eq!(cookie_of(&headers).as_deref(), Some("secret"));
        strip_cookie(&mut headers);
        let left = headers.get(header::COOKIE).unwrap().to_str().unwrap();
        assert_eq!(left, "theme=dark; lang=en");
        let uri: http::Uri = "/hook?a=1&orch_token=proof&b=2".parse().unwrap();
        assert_eq!(clean_path(&uri), "/hook?a=1&b=2");
        let only: http::Uri = "/?orch_token=proof".parse().unwrap();
        assert_eq!(clean_path(&only), "/");
    }

    #[test]
    fn a_caller_s_own_identity_headers_are_taken_out_in_every_case_and_copy() {
        let request = http::Request::builder()
            .uri("/")
            .header("Orch-User", "mallory")
            .header("orch-user", "mallory")
            .header("X-Forwarded-For", "203.0.113.9")
            .header("Orch-Assertion", "forged")
            .header("Transfer-Encoding", "chunked")
            .body(())
            .unwrap();
        let (mut parts, ()) = request.into_parts();
        rewrite_request(&mut parts, false);
        for name in [
            "orch-user",
            "x-forwarded-for",
            "orch-assertion",
            "transfer-encoding",
        ] {
            assert!(parts.headers.get(name).is_none(), "{name}");
        }
    }

    #[test]
    fn a_display_name_travels_as_percent_text() {
        assert_eq!(percent("Ada Lovelace"), "Ada%20Lovelace");
        assert_eq!(percent("Zoë"), "Zo%C3%AB");
    }
}
