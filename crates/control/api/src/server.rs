//! TLS listeners, startup and shutdown.
//!
//! The router is the registry's requests under the API's base path, and
//! the health check at the root as well, where the front proxy asks for it.
//! Serving stops taking connections when it is told to and lets the
//! requests that are under way finish.
//!
//! Two listeners serve under the system's own authority. The browser API
//! shows the API's service certificate to whoever connects, which the front
//! proxy verifies, and asks for none. The internal listener asks for a
//! certificate and takes only one that the authority issued to a service
//! of the system: a machine's certificate, a certificate of another
//! authority and a certificate that ran out do not get as far as the first
//! request. What the gateway hands over (the backplane, enrollment,
//! renewal, content) is served there, and nowhere else.

use crate::certificates::{self, LinkFault};
use crate::registry::Registry;
use crate::state::App;
use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Extension, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use orch_api_contract::common::BASE_PATH;
use orch_pki::PkiError;
use orch_pki::certificates::{Certificate, Identity, ServiceName};
use orch_pki::verification::{
    ClockTime, OrchClientVerifier, TlsIdentity, crypto, plain_server_config, presented_certificate,
};
use orch_types::Clock;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error as TlsError, ServerConfig,
    SignatureScheme,
};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// How long a connection has to finish its TLS handshake. Ten seconds, as
/// long as a machine allows itself to connect.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a connection has to send the head of a request once it begins:
/// a thin line carries it in this, and a connection that does not is not
/// going to.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long connections that are under way are let finish after serving was
/// told to stop.
const DRAIN: Duration = Duration::from_secs(10);

/// The application's router: every bound request under the base path, and the
/// browser's socket beside them.
pub fn router(app: App, registry: Registry) -> Router {
    crate::machine_requests::install(&app);
    // Browser tools need their live transport even before the first HTTP request arrives.
    crate::realtime::browsers::install(&app);
    crate::realtime::displays::install(&app);
    crate::port_tools::install(&app);
    crate::realtime::browsers::recover(&app);
    crate::realtime::displays::recover(&app);
    let health = crate::routes::meta::health_router();
    Router::new()
        .route(
            &format!("{BASE_PATH}{}", crate::realtime::SOCKET_ROUTE),
            get(crate::realtime::server::socket),
        )
        .nest(
            BASE_PATH,
            registry
                .into_router()
                .route_layer(axum::middleware::from_fn(measured)),
        )
        .merge(health)
        // A request for a port's stable name goes to that port, whatever its path.
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            crate::ports_browser::dispatch,
        ))
        .with_state(app)
}

/// Counts a request of the API and the time it took to answer, by the route it matched (its
/// template, so that the series stay few) and the status it was answered with:
/// `orch_http_requests_total` and `orch_http_request_seconds`. A request that matches no route is
/// not counted.
async fn measured(
    matched: axum::extract::MatchedPath,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let route = matched.as_str().to_owned();
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();
    metrics::counter!(
        "orch_http_requests_total",
        "route" => route.clone(),
        "status" => status.clone()
    )
    .increment(1);
    metrics::histogram!(
        "orch_http_request_seconds",
        "route" => route,
        "status" => status
    )
    .record(started.elapsed().as_secs_f64());
    response
}

/// Serves `router` on `listener` without TLS until `shutdown` resolves.
/// For development and for tests over a socket: a deployment's API is
/// never reachable without TLS.
pub async fn serve_plain(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}

// ---------------------------------------------------------------- who is at the other end

/// Who a connection is, as its TLS handshake showed: the request that
/// arrives on it carries this as an extension.
#[derive(Debug, Clone)]
pub struct Peer {
    /// The service the certificate names, where it names one.
    pub service: Option<ServiceName>,
    /// The certificate the connection showed.
    pub certificate: Option<Certificate>,
    /// The address it came from.
    pub remote: SocketAddr,
}

/// The connection is not from the service that was required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("that is not the service this is for")]
pub struct NotThatService;

impl Peer {
    /// Whether the connection is that of `service`.
    pub fn require(&self, service: ServiceName) -> Result<(), NotThatService> {
        if self.service == Some(service) {
            Ok(())
        } else {
            Err(NotThatService)
        }
    }
}

// ---------------------------------------------------------------- TLS

/// Takes a client certificate only when it is one of the system's services,
/// good now: the internal listener is for the system's own parts.
#[derive(Debug)]
struct ServiceClientVerifier {
    inner: Arc<OrchClientVerifier>,
}

fn refused() -> TlsError {
    TlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
}

impl ClientCertVerifier for ServiceClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
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
            .verify_client_cert(end_entity, intermediates, now)?;
        let named = Certificate::from_der(end_entity).map_err(|_| refused())?;
        let only_a_service = !named.identities().is_empty()
            && named
                .identities()
                .iter()
                .all(|identity| matches!(identity, Identity::Service(_)));
        if only_a_service {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(refused())
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// The TLS configuration of the browser API: the service's own certificate,
/// and no certificate asked of the client.
pub fn browser_tls_config(
    identity: &TlsIdentity,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    plain_server_config(identity, clock)
}

/// The TLS configuration of the internal listener: the service's own
/// certificate, and a certificate of a service of the system asked of every
/// client.
pub fn internal_tls_config(
    identity: &TlsIdentity,
    authority: &Certificate,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    let provider = crypto();
    let inner = OrchClientVerifier::new(authority, Duration::ZERO, &provider)?;
    let config =
        ServerConfig::builder_with_details(Arc::clone(&provider), ClockTime::provider(clock))
            .with_safe_default_protocol_versions()
            .map_err(|e| PkiError::Certificate(e.to_string()))?
            .with_client_cert_verifier(Arc::new(ServiceClientVerifier { inner }))
            .with_single_cert(
                identity.chain.iter().map(Certificate::to_rustls).collect(),
                identity.key.to_rustls(),
            )
            .map_err(|e| PkiError::Certificate(e.to_string()))?;
    Ok(Arc::new(config))
}

/// The browser API's configuration as [`browser_tls_config`] makes it, showing the service's certificate as it
/// stands at each handshake, so that the API's renewed certificate is shown without starting it again.
pub fn browser_tls_config_live(
    identity: Arc<orch_pki::service_renew::LiveIdentity>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    orch_pki::verification::plain_server_config_live(identity, clock)
}

/// The internal listener's configuration as [`internal_tls_config`] makes it, showing the service's certificate
/// as it stands at each handshake.
pub fn internal_tls_config_live(
    identity: Arc<orch_pki::service_renew::LiveIdentity>,
    authority: &Certificate,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    let provider = crypto();
    let inner = OrchClientVerifier::new(authority, Duration::ZERO, &provider)?;
    let config =
        ServerConfig::builder_with_details(Arc::clone(&provider), ClockTime::provider(clock))
            .with_safe_default_protocol_versions()
            .map_err(|e| PkiError::Certificate(e.to_string()))?
            .with_client_cert_verifier(Arc::new(ServiceClientVerifier { inner }))
            .with_cert_resolver(identity);
    Ok(Arc::new(config))
}

// ---------------------------------------------------------------- serving

/// Serves `router` on `listener` under TLS until `shutdown` is cancelled;
/// the requests under way are then let finish for a few seconds. Each request
/// carries the [`Peer`] it came from.
pub async fn serve_tls(
    listener: TcpListener,
    tls: Arc<ServerConfig>,
    router: Router,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    use hyper_util::rt::{TokioIo, TokioTimer};
    use hyper_util::service::TowerToHyperService;
    let acceptor = TlsAcceptor::from(tls);
    let tracker = TaskTracker::new();
    loop {
        let accepted = tokio::select! {
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let (socket, remote) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                // A connection that was reset before it was taken is not a
                // reason to stop listening.
                tracing::debug!(%error, "a connection could not be accepted");
                continue;
            }
        };
        let (acceptor, router, shutdown) = (acceptor.clone(), router.clone(), shutdown.clone());
        tracker.spawn(async move {
            let _ = socket.set_nodelay(true);
            let handshake =
                tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(socket)).await;
            let stream = match handshake {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    tracing::debug!(%remote, %error, "a TLS handshake failed");
                    return;
                }
                Err(_) => return,
            };
            let certificate = presented_certificate(stream.get_ref().1.peer_certificates());
            let peer = Peer {
                service: certificate.as_ref().and_then(Certificate::service),
                certificate,
                remote,
            };
            let service = TowerToHyperService::new(
                router
                    .layer(Extension(peer))
                    .layer(Extension(ConnectInfo(remote))),
            );
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
        });
    }
    tracker.close();
    let _ = tokio::time::timeout(DRAIN, tracker.wait()).await;
    Ok(())
}

// ---------------------------------------------------------------- the internal listener

async fn backplane(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    upgrade: WebSocketUpgrade,
) -> Response {
    if peer.require(ServiceName::Gateway).is_err() {
        return LinkFault::forbidden("only a gateway holds the backplane").into_response();
    }
    let link = app.link.clone();
    upgrade
        .max_message_size(crate::backplane::MAX_BACKPLANE_FRAME_BYTES)
        .max_frame_size(crate::backplane::MAX_BACKPLANE_FRAME_BYTES)
        .on_upgrade(move |socket| async move { link.serve_gateway(socket).await })
}

/// The router of the internal listener. `content` answers what a gateway
/// forwards of the content requests of machines, under
/// `/internal/link/blobs`.
pub fn internal_router(app: App, content: Router<App>) -> Router {
    crate::machine_requests::install(&app);
    let mut downloads = Registry::new();
    crate::routes::downloads::mount(&mut downloads);
    Router::new()
        .route("/internal/backplane", get(backplane))
        .nest(BASE_PATH, downloads.into_router())
        .merge(certificates::internal_routes())
        .merge(content)
        .with_state(app)
}
