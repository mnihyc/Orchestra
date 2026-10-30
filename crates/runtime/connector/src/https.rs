//! A small HTTPS client for the few requests a connector makes beside its session: enrolling, renewing, moving content.
//!
//! One request, one connection, over a TLS configuration the caller chose: the system's own authority
//! (pinned), the public roots, or the authority's pin at first contact. The system trust store is
//! never consulted for the system's own certificates. Requests are few and large, so there is no pool;
//! a connection is made, used and dropped.
//! Enrollment alone also accepts explicit loopback HTTP, with a direct loopback peer and no proxy.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{HeaderMap, Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;
use url::Url;

/// The most a reply may hold: a piece of content is at most 4 MiB and every other reply is a few lines of
/// JSON, so eight is generous and bounds what a bad server can make the connector hold.
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

/// A client over one TLS configuration, with an explicit local-enrollment exception.
#[derive(Clone)]
pub struct HttpsClient {
    tls: Arc<rustls::ClientConfig>,
    loopback_enrollment: bool,
}

impl std::fmt::Debug for HttpsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpsClient").finish_non_exhaustive()
    }
}

impl HttpsClient {
    /// A client that verifies servers as `tls` says.
    #[must_use]
    pub const fn new(tls: Arc<rustls::ClientConfig>) -> Self {
        Self {
            tls,
            loopback_enrollment: false,
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
        let hostname = base
            .host()
            .ok_or_else(|| HttpsError::Address(base.to_string()))?;
        let host = match &hostname {
            url::Host::Domain(name) => (*name).to_owned(),
            url::Host::Ipv4(ip) => ip.to_string(),
            url::Host::Ipv6(ip) => ip.to_string(),
        };
        let plain = self.loopback_enrollment
            && loopback_http(base)
            && method == Method::POST
            && path_and_query == orch_link_contract::enrollment::ENROLL_PATH;
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
        let mut request = Request::builder()
            .method(method)
            .uri(path_and_query)
            .header(hyper::header::HOST, authority);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let request = request
            .body(Full::new(body))
            .map_err(|e| HttpsError::Http(e.to_string()))?;
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
            request_on(socket, request).await
        } else {
            let name =
                ServerName::try_from(host).map_err(|_| HttpsError::Address(base.to_string()))?;
            let stream = TlsConnector::from(Arc::clone(&self.tls))
                .connect(name, socket)
                .await
                .map_err(|e| HttpsError::Tls(e.to_string()))?;
            request_on(stream, request).await
        }
    }
}

async fn request_on<S>(
    stream: S,
    request: Request<Full<Bytes>>,
) -> Result<hyper::Response<hyper::body::Incoming>, HttpsError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| HttpsError::Http(e.to_string()))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    sender
        .send_request(request)
        .await
        .map_err(|e| HttpsError::Http(e.to_string()))
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
