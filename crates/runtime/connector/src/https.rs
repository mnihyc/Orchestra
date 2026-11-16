//! A small HTTPS client for the few requests a connector makes beside its session: enrolling, renewing, moving content.
//!
//! Connections use the caller's TLS configuration: the system's authority, public roots, or the
//! authority's pin at first contact. File requests reuse verified connections and wait for silence
//! only; short control requests keep their finite allowance. The system trust store is never
//! consulted for the system's own certificates.
//! Enrollment alone also accepts explicit loopback HTTP, with a direct loopback peer and no proxy.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{HeaderMap, Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;
use url::Url;

/// Metadata replies are bounded independently of file ranges, whose requested length is their bound.
pub const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
/// How long connecting may take before the attempt is given up: the link's own figure.
pub const CONNECT_TIMEOUT: Duration =
    Duration::from_secs(orch_link_contract::session::CONNECT_TIMEOUT_SECS as u64);

/// Why a request did not get its reply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpsError {
    /// The address or its transport is not allowed for this request.
    #[error("not an address that can be reached: {0}")]
    Address(String),
    /// The socket could not be connected.
    #[error("could not connect: {0}")]
    Connect(String),
    /// The TLS handshake failed: the server is not the system's, or does not know this machine.
    #[error("TLS: {0}")]
    Tls(String),
    /// The exchange failed after the handshake.
    #[error("HTTP: {0}")]
    Http(String),
    /// The request took longer than it may.
    #[error("the request took too long")]
    TimedOut,
}

/// A reply, whole.
#[derive(Debug, Clone)]
pub struct Reply {
    /// Its status.
    pub status: StatusCode,
    /// Its headers.
    pub headers: HeaderMap,
    /// Its body.
    pub body: Bytes,
}

/// The base address, over HTTPS, of an address given as `https://`, `wss://` or `host:port`: the scheme,
/// the host and the port, without a path.
pub fn https_base(address: &str) -> Result<Url, HttpsError> {
    let with_scheme = if address.contains("://") {
        address.to_owned()
    } else {
        format!("https://{address}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|e| HttpsError::Address(e.to_string()))?;
    if matches!(url.scheme(), "wss" | "https") {
        let _ = url.set_scheme("https");
    } else {
        return Err(HttpsError::Address(format!(
            "{address}: only https:// and wss:// addresses are used"
        )));
    }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    if url.host_str().is_none() {
        return Err(HttpsError::Address(format!("{address} names no host")));
    }
    Ok(url)
}

fn loopback_http(url: &Url) -> bool {
    url.scheme() == "http"
        && url.username().is_empty()
        && url.password().is_none()
        && match url.host() {
            Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        }
}

/// Enrollment may reach a development system through HTTP on this machine; other uses stay TLS-only.
pub fn enrollment_base(address: &str) -> Result<Url, HttpsError> {
    if let Ok(mut url) = Url::parse(address)
        && loopback_http(&url)
    {
        url.set_path("");
        url.set_query(None);
        url.set_fragment(None);
        return Ok(url);
    }
    https_base(address)
}

/// Accepted file-body and connection limits, independent of small control requests.
pub(crate) struct ContentOptions {
    pub(crate) idle: Duration,
    pub(crate) maximum_reply: usize,
    pub(crate) pooled_connections: usize,
}

/// A client over one TLS configuration, with an explicit local-enrollment exception.
#[derive(Clone)]
pub struct HttpsClient {
    tls: Arc<rustls::ClientConfig>,
    loopback_enrollment: bool,
    content_pool: Arc<Mutex<Vec<(String, ContentConnection)>>>,
}

impl std::fmt::Debug for HttpsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpsClient").finish_non_exhaustive()
    }
}

impl HttpsClient {
    /// A client that verifies servers as `tls` says.
    #[must_use]
    pub fn new(tls: Arc<rustls::ClientConfig>) -> Self {
        Self {
            tls,
            loopback_enrollment: false,
            content_pool: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The enrollment caller alone opts into loopback HTTP for the enrollment POST.
    #[must_use]
    pub(crate) fn for_enrollment(mut self) -> Self {
        self.loopback_enrollment = true;
        self
    }

    /// Makes one request and reads the whole reply, within `limit`.
    pub async fn send(
        &self,
        base: &Url,
        method: Method,
        path_and_query: &str,
        headers: &[(&str, String)],
        body: Bytes,
        limit: Duration,
    ) -> Result<Reply, HttpsError> {
        tokio::time::timeout(
            limit,
            self.exchange(base, method, path_and_query, headers, body),
        )
        .await
        .map_err(|_| HttpsError::TimedOut)?
    }

    /// File requests reuse verified connections and renew patience on positive socket I/O.
    pub(crate) async fn send_content(
        &self,
        base: &Url,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        body: Bytes,
        limits: ContentOptions,
    ) -> Result<Reply, HttpsError> {
        let key = base.origin().ascii_serialization();
        let held = {
            let mut pool = self
                .content_pool
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            pool.retain(|(_, connection)| !connection.0.sender.is_closed());
            pool.iter()
                .position(|(origin, _)| origin == &key)
                .map(|index| pool.swap_remove(index).1)
        };
        let mut connection = match held {
            Some(connection) => connection,
            None => ContentConnection(
                tokio::time::timeout(CONNECT_TIMEOUT, self.connection(base, false))
                    .await
                    .map_err(|_| HttpsError::TimedOut)??,
            ),
        };
        let mut activity = connection.0.activity.clone();
        activity.borrow_and_update();
        let initial = tokio::time::Instant::now();
        let response = {
            let work = connection.0.send_bounded_with_refusal(
                method,
                path,
                headers,
                body,
                limits.maximum_reply,
                MAX_REPLY_BYTES,
            );
            tokio::pin!(work);
            loop {
                let last = (*activity.borrow_and_update()).max(initial);
                tokio::select! {
                    result=&mut work=>break result,
                    changed=activity.changed()=>{if changed.is_err(){break Err(HttpsError::Http("the connection ended".into()));}},
                    ()=tokio::time::sleep_until(last+limits.idle)=>break Err(HttpsError::TimedOut),
                }
            }
        };
        if response.is_ok() && !connection.0.sender.is_closed() {
            let mut pool = self
                .content_pool
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if pool.iter().filter(|(origin, _)| origin == &key).count() < limits.pooled_connections
            {
                pool.push((key, connection));
            }
        }
        response
    }

    /// Makes one `GET` and writes its body to `path` as it arrives, at most `max` bytes, within `limit`; gives
    /// the status, the headers, the bytes written and their SHA-256 in lowercase hexadecimal. A reply that is
    /// not 200 writes nothing; a body larger than `max` is an error, and what was written of it is removed.
    pub async fn download(
        &self,
        base: &Url,
        path_and_query: &str,
        path: &std::path::Path,
        max: u64,
        limit: Duration,
    ) -> Result<(StatusCode, HeaderMap, u64, String), HttpsError> {
        let written = tokio::time::timeout(limit, async {
            let reply = self
                .respond(base, Method::GET, path_and_query, &[], Bytes::new())
                .await?;
            let (parts, mut incoming) = reply.into_parts();
            if parts.status != StatusCode::OK {
                return Ok::<_, HttpsError>((parts.status, parts.headers, 0, String::new()));
            }
            let (count, digest) = write_body(&mut incoming, path, max).await?;
            Ok::<_, HttpsError>((parts.status, parts.headers, count, digest))
        })
        .await
        .map_err(|_| HttpsError::TimedOut)?;
        if written.is_err() {
            let _ = tokio::fs::remove_file(path).await;
        }
        written
    }

    async fn exchange(
        &self,
        base: &Url,
        method: Method,
        path_and_query: &str,
        headers: &[(&str, String)],
        body: Bytes,
    ) -> Result<Reply, HttpsError> {
        let reply = self
            .respond(base, method, path_and_query, headers, body)
            .await?;
        let (parts, incoming) = reply.into_parts();
        let body = Limited::new(incoming, MAX_REPLY_BYTES)
            .collect()
            .await
            .map_err(|e| HttpsError::Http(e.to_string()))?
            .to_bytes();
        Ok(Reply {
            status: parts.status,
            headers: parts.headers,
            body,
        })
    }

    /// Connects, sends one request and gives the reply with its body still to be read.
    async fn respond(
        &self,
        base: &Url,
        method: Method,
        path_and_query: &str,
        headers: &[(&str, String)],
        body: Bytes,
    ) -> Result<hyper::Response<hyper::body::Incoming>, HttpsError> {
        let plain = self.loopback_enrollment
            && loopback_http(base)
            && ((method == Method::POST
                && path_and_query == orch_link_contract::enrollment::ENROLL_PATH)
                || (method == Method::GET
                    && path_and_query == orch_link_contract::gateways::DISCOVERY_PATH));
        self.connection(base, plain)
            .await?
            .respond(method, path_and_query, headers, body)
            .await
    }

    /// One verified HTTPS connection for consecutive small RTT probes, excluding TLS setup.
    pub(crate) async fn connect_https(&self, base: &Url) -> Result<HttpConnection, HttpsError> {
        self.connection(base, false).await
    }

    async fn connection(&self, base: &Url, plain: bool) -> Result<HttpConnection, HttpsError> {
        let hostname = base
            .host()
            .ok_or_else(|| HttpsError::Address(base.to_string()))?;
        let host = match &hostname {
            url::Host::Domain(name) => (*name).to_owned(),
            url::Host::Ipv4(ip) => ip.to_string(),
            url::Host::Ipv6(ip) => ip.to_string(),
        };
        if !plain && base.scheme() != "https" {
            return Err(HttpsError::Address("HTTP is allowed only for enrollment on localhost or a loopback IP; use HTTPS otherwise.".into()));
        }
        let port = base
            .port_or_known_default()
            .ok_or_else(|| HttpsError::Address(base.to_string()))?;
        let socket = tokio::time::timeout(CONNECT_TIMEOUT, async {
            if plain {
                tokio::net::TcpStream::connect((host.as_str(), port)).await
            } else {
                // TLS requests keep the restricted unit's proxy. Local enrollment must never
                // send an unencrypted token through an external proxy.
                let proxy = orch_link::outward::OutwardProxy::system();
                orch_link::outward::connect(&host, port, true, Some(&proxy)).await
            }
        })
        .await
        .map_err(|_| HttpsError::TimedOut)?
        .map_err(|e| HttpsError::Connect(e.to_string()))?;
        socket
            .set_nodelay(true)
            .map_err(|e| HttpsError::Connect(e.to_string()))?;
        let authority = match base.port() {
            Some(port) => format!("{hostname}:{port}"),
            None => hostname.to_string(),
        };
        if plain {
            if !socket
                .peer_addr()
                .map_err(|e| HttpsError::Connect(e.to_string()))?
                .ip()
                .is_loopback()
            {
                return Err(HttpsError::Address(
                    "The HTTP enrollment peer is not loopback; use HTTPS.".into(),
                ));
            }
            connection_on(socket, authority).await
        } else {
            let name =
                ServerName::try_from(host).map_err(|_| HttpsError::Address(base.to_string()))?;
            let stream = TlsConnector::from(Arc::clone(&self.tls))
                .connect(name, socket)
                .await
                .map_err(|e| HttpsError::Tls(e.to_string()))?;
            connection_on(stream, authority).await
        }
    }
}

async fn connection_on<S>(stream: S, authority: String) -> Result<HttpConnection, HttpsError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (activity, observed) = tokio::sync::watch::channel(tokio::time::Instant::now());
    let stream = MovingIo {
        inner: stream,
        activity,
    };
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| HttpsError::Http(e.to_string()))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    })
    .abort_handle();
    Ok(HttpConnection {
        sender,
        authority,
        activity: observed,
        driver,
    })
}

// File requests own the driver through cancellation and pooling. Short callers keep their body owner.
struct ContentConnection(HttpConnection);
impl Drop for ContentConnection {
    fn drop(&mut self) {
        self.0.driver.abort();
    }
}
struct MovingIo<S> {
    inner: S,
    activity: tokio::sync::watch::Sender<tokio::time::Instant>,
}
impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for MovingIo<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            self.activity.send_replace(tokio::time::Instant::now());
        }
        result
    }
}
impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for MovingIo<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let result = std::pin::Pin::new(&mut self.inner).poll_write(cx, buf);
        if matches!(result,std::task::Poll::Ready(Ok(n)) if n>0) {
            self.activity.send_replace(tokio::time::Instant::now());
        }
        result
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A verified HTTP/1 connection. Body reads remain explicitly bounded by the caller.
pub(crate) struct HttpConnection {
    sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    authority: String,
    activity: tokio::sync::watch::Receiver<tokio::time::Instant>,
    driver: tokio::task::AbortHandle,
}

impl HttpConnection {
    async fn respond(
        &mut self,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        body: Bytes,
    ) -> Result<hyper::Response<hyper::body::Incoming>, HttpsError> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::HOST, &self.authority);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let request = request
            .body(Full::new(body))
            .map_err(|error| HttpsError::Http(error.to_string()))?;
        self.sender
            .send_request(request)
            .await
            .map_err(|error| HttpsError::Http(error.to_string()))
    }

    pub(crate) async fn send_bounded(
        &mut self,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        body: Bytes,
        max: usize,
    ) -> Result<Reply, HttpsError> {
        self.send_bounded_with_refusal(method, path, headers, body, max, max)
            .await
    }

    async fn send_bounded_with_refusal(
        &mut self,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        body: Bytes,
        max: usize,
        error_max: usize,
    ) -> Result<Reply, HttpsError> {
        let response = self.respond(method, path, headers, body).await?;
        let (parts, incoming) = response.into_parts();
        let limit = if parts.status.is_success() {
            max
        } else {
            error_max
        };
        let body = Limited::new(incoming, limit)
            .collect()
            .await
            .map_err(|error| HttpsError::Http(error.to_string()))?
            .to_bytes();
        Ok(Reply {
            status: parts.status,
            headers: parts.headers,
            body,
        })
    }
}

/// Writes the body `incoming` to `path`, at most `max` bytes; gives the bytes written and their SHA-256.
async fn write_body(
    incoming: &mut hyper::body::Incoming,
    path: &std::path::Path,
    max: u64,
) -> Result<(u64, String), HttpsError> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;
    let local = |e: std::io::Error| HttpsError::Http(format!("{}: {e}", path.display()));
    let mut file = tokio::fs::File::create(path).await.map_err(local)?;
    let (mut count, mut digest) = (0_u64, Sha256::new());
    while let Some(frame) = incoming.frame().await {
        let frame = frame.map_err(|e| HttpsError::Http(e.to_string()))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        count += data.len() as u64;
        if count > max {
            return Err(HttpsError::Http(format!(
                "the body is larger than the {max} bytes it may have"
            )));
        }
        digest.update(&data);
        file.write_all(&data).await.map_err(local)?;
    }
    file.flush().await.map_err(local)?;
    file.sync_all().await.map_err(local)?;
    Ok((count, hex::encode(digest.finalize())))
}

/// The shared public and machine roots used for every public HTTPS or WebSocket path.
pub use orch_pki::verification::trusted_roots as public_roots;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_reduced_to_its_scheme_host_and_port() {
        for (given, wanted) in [
            ("https://orch.example.com", "https://orch.example.com/"),
            (
                "https://orch.example.com/some/path?x=1",
                "https://orch.example.com/",
            ),
            (
                "wss://gw.orch.internal:9443/link/v1/session",
                "https://gw.orch.internal:9443/",
            ),
            ("orch.example.com:8443", "https://orch.example.com:8443/"),
        ] {
            assert_eq!(https_base(given).unwrap().as_str(), wanted, "{given}");
        }
    }

    #[test]
    fn an_address_that_is_not_https_is_refused() {
        assert!(https_base("http://orch.example.com").is_err());
        assert!(
            https_base("http://localhost:3000").is_err(),
            "renewal and content still require TLS"
        );
        assert!(https_base("ftp://x").is_err());
    }

    #[test]
    fn enrollment_alone_accepts_explicit_loopback_http_without_promoting_other_hosts()
    -> Result<(), HttpsError> {
        for address in [
            "http://localhost:3000",
            "http://LOCALHOST",
            "http://127.0.0.1:3000",
            "http://127.45.1.7",
            "http://[::1]:3000",
            "http://[0:0:0:0:0:0:0:1]",
        ] {
            let base = enrollment_base(address)?;
            assert_eq!(base.scheme(), "http");
            assert!(loopback_http(&base), "{address}");
        }
        for address in [
            "http://orch.example",
            "http://10.0.0.1",
            "http://128.0.0.1",
            "http://[::2]",
            "http://localhost.example",
            "http://localhost@orch.example",
            "http://person@localhost",
        ] {
            assert!(enrollment_base(address).is_err(), "{address}");
        }
        Ok(())
    }

    #[test]
    fn the_public_roots_are_not_empty() {
        assert!(!public_roots().roots.is_empty());
    }
}

#[cfg(test)]
mod content_tests {
    use super::*;
    use hyper::service::service_fn;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn client() -> Result<HttpsClient, rustls::Error> {
        Ok(HttpsClient::new(Arc::new(
            rustls::ClientConfig::builder_with_provider(orch_pki::verification::crypto())
                .with_safe_default_protocol_versions()?
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        )))
    }
    async fn pooled(
        client: &HttpsClient,
        stream: tokio::io::DuplexStream,
        base: &Url,
    ) -> Result<(), HttpsError> {
        let connection = connection_on(stream, "content.test".to_owned()).await?;
        client
            .content_pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((
                base.origin().ascii_serialization(),
                ContentConnection(connection),
            ));
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn file_progress_outlasts_the_silence_limit_and_reuses_the_same_connection()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = client()?;
        let base = Url::parse("https://content.test")?;
        let (send, receive) = tokio::io::duplex(64);
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let serving = tokio::spawn(async move {
            hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(receive),
                    service_fn(move |mut request: Request<hyper::body::Incoming>| {
                        let seen = Arc::clone(&seen);
                        async move {
                            seen.fetch_add(1, Ordering::SeqCst);
                            let mut bytes = 0;
                            while let Some(frame) = request.body_mut().frame().await {
                                bytes += frame
                                    .map_err(std::io::Error::other)?
                                    .data_ref()
                                    .map_or(0, Bytes::len);
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                            Ok::<_, std::io::Error>(hyper::Response::new(Full::new(Bytes::from(
                                bytes.to_string(),
                            ))))
                        }
                    }),
                )
                .await
        });
        pooled(&client, send, &base).await?;
        let began = tokio::time::Instant::now();
        for _ in 0..2 {
            let reply = client
                .send_content(
                    &base,
                    Method::PUT,
                    "/piece",
                    &[],
                    Bytes::from(vec![7; 1024]),
                    ContentOptions {
                        idle: Duration::from_millis(50),
                        maximum_reply: 64,
                        pooled_connections: 5,
                    },
                )
                .await?;
            assert_eq!(reply.body, Bytes::from_static(b"1024"));
        }
        assert!(began.elapsed() > Duration::from_millis(100));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        drop(client);
        serving.abort();
        let _ = serving.await;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn a_file_request_is_aborted_after_actual_silence()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = client()?;
        let base = Url::parse("https://content.test")?;
        let (send, receive) = tokio::io::duplex(64);
        let serving = tokio::spawn(async move {
            hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(receive),
                    service_fn(|_request: Request<hyper::body::Incoming>| async move {
                        std::future::pending::<()>().await;
                        Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::new())))
                    }),
                )
                .await
        });
        pooled(&client, send, &base).await?;
        let reply = client
            .send_content(
                &base,
                Method::PUT,
                "/piece",
                &[],
                Bytes::from(vec![7; 1024]),
                ContentOptions {
                    idle: Duration::from_millis(50),
                    maximum_reply: 64,
                    pooled_connections: 5,
                },
            )
            .await;
        assert!(matches!(reply, Err(HttpsError::TimedOut)));
        assert!(
            client
                .content_pool
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        );
        serving.abort();
        let _ = serving.await;
        Ok(())
    }

    #[tokio::test]
    async fn a_file_reply_is_bounded_by_its_requested_range_rather_than_the_metadata_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = client()?;
        let base = Url::parse("https://content.test")?;
        let (send, receive) = tokio::io::duplex(64 * 1024);
        let bytes = MAX_REPLY_BYTES + 17;
        let serving = tokio::spawn(async move {
            hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(receive),
                    service_fn(move |_request: Request<hyper::body::Incoming>| async move {
                        Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(vec![
                            7;
                            bytes
                        ]))))
                    }),
                )
                .await
        });
        pooled(&client, send, &base).await?;
        let reply = client
            .send_content(
                &base,
                Method::GET,
                "/piece",
                &[],
                Bytes::new(),
                ContentOptions {
                    idle: Duration::from_secs(1),
                    maximum_reply: bytes,
                    pooled_connections: 5,
                },
            )
            .await?;
        assert_eq!(reply.body.len(), bytes);
        assert!(
            client
                .send_content(
                    &base,
                    Method::GET,
                    "/piece",
                    &[],
                    Bytes::new(),
                    ContentOptions {
                        idle: Duration::from_secs(1),
                        maximum_reply: bytes - 1,
                        pooled_connections: 5
                    }
                )
                .await
                .is_err()
        );
        serving.abort();
        let _ = serving.await;
        Ok(())
    }
    #[tokio::test]
    async fn a_one_byte_file_tail_preserves_its_structured_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = client()?;
        let base = Url::parse("https://content.test")?;
        let (send, receive) = tokio::io::duplex(1024);
        let serving = tokio::spawn(async move {
            hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(receive),
                    service_fn(move |_request: Request<hyper::body::Incoming>| async move {
                        let mut response = hyper::Response::new(Full::new(Bytes::from_static(
                            b"the requested bytes are not permitted",
                        )));
                        *response.status_mut() = StatusCode::FORBIDDEN;
                        Ok::<_, Infallible>(response)
                    }),
                )
                .await
        });
        pooled(&client, send, &base).await?;
        let reply = client
            .send_content(
                &base,
                Method::GET,
                "/piece",
                &[],
                Bytes::new(),
                ContentOptions {
                    idle: Duration::from_secs(1),
                    maximum_reply: 1,
                    pooled_connections: 5,
                },
            )
            .await?;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(reply.body.len() > 1);
        serving.abort();
        let _ = serving.await;
        Ok(())
    }
}
