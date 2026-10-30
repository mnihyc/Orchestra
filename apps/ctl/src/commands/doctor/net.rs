//! The connections the checks make: to an address, under TLS or without, with a request or a WebSocket on them.
//!
//! A check says what it tried and what came back, so every failure here is a
//! sentence about the step that failed (the name did not resolve, nothing
//! answered, the certificate was refused), not an error code.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use rustls::pki_types::ServerName;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// How long to wait for a connection, and for the handshake that follows it.
/// A host that does not answer in this time is not going to: the longest a
/// healthy path takes is a few round trips of a slow line.
const CONNECT_SECS: u64 = 10;
/// How long to wait for the answer to a request. The 8 MiB body is sent inside
/// it, so it is long enough for that to cross a thin line.
const REQUEST_SECS: u64 = 120;

/// Anything a request can be made over.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// A connection, under TLS or not.
pub type Stream = Box<dyn Io>;

/// Names that are reached at an address of the operator's choosing: some of
/// what a system advertises resolves only inside its own network.
#[derive(Debug, Clone, Default)]
pub struct Resolve(BTreeMap<String, IpAddr>);

impl Resolve {
    /// Reads `NAME=ADDRESS` pairs.
    pub fn parse(pairs: &[String]) -> Result<Self, String> {
        let mut names = BTreeMap::new();
        for pair in pairs {
            let (name, address) = pair
                .split_once('=')
                .ok_or_else(|| format!("--resolve wants NAME=ADDRESS, and {pair:?} is not"))?;
            let address: IpAddr = address
                .parse()
                .map_err(|_| format!("--resolve {pair:?}: {address:?} is not an IP address"))?;
            names.insert(name.to_ascii_lowercase(), address);
        }
        Ok(Self(names))
    }

    fn lookup(&self, host: &str) -> Option<IpAddr> {
        self.0.get(&host.to_ascii_lowercase()).copied()
    }
}

/// Where to connect, from an address as an operator writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// Whether the connection is under TLS (`https`, `wss`).
    pub secure: bool,
    /// The name or address to connect to.
    pub host: String,
    /// The port.
    pub port: u16,
    /// The path the address ends in, without a final `/`; empty for the root.
    pub base: String,
}

impl Address {
    /// Reads `http`, `https`, `ws` and `wss` addresses.
    pub fn parse(text: &str) -> Result<Self, String> {
        let url = url::Url::parse(text)
            .map_err(|error| format!("{text:?} is not an address: {error}"))?;
        let secure = match url.scheme() {
            "http" | "ws" => false,
            "https" | "wss" => true,
            other => return Err(format!("{text:?}: {other} is not http, https, ws or wss")),
        };
        let host = url
            .host_str()
            .ok_or_else(|| format!("{text:?} names no host"))?
            .trim_matches(['[', ']'])
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| format!("{text:?} names no port"))?;
        Ok(Self {
            secure,
            host,
            port,
            base: url.path().trim_end_matches('/').to_owned(),
        })
    }

    /// The address of `path` below this one.
    #[must_use]
    pub fn path(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The `Host` header of a request to this address.
    #[must_use]
    pub fn authority(&self) -> String {
        let default = if self.secure { 443 } else { 80 };
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == default {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// The WebSocket address of `path` below this one.
    #[must_use]
    pub fn websocket(&self, path: &str) -> String {
        format!(
            "{}://{}{}",
            if self.secure { "wss" } else { "ws" },
            self.authority(),
            self.path(path)
        )
    }
}

/// Connects to `address`. Under TLS the certificate is judged by `tls` and is
/// good for `server_name`, which is the host unless another is given.
pub async fn connect(
    address: &Address,
    resolve: &Resolve,
    tls: Option<&Arc<rustls::ClientConfig>>,
    server_name: Option<&str>,
) -> Result<Stream, String> {
    let wait = Duration::from_secs(CONNECT_SECS);
    let place = format!("{}:{}", address.host, address.port);
    let connecting = async {
        match resolve.lookup(&address.host) {
            Some(ip) => TcpStream::connect((ip, address.port)).await,
            None => TcpStream::connect((address.host.as_str(), address.port)).await,
        }
    };
    let socket = tokio::time::timeout(wait, connecting)
        .await
        .map_err(|_| format!("{place} did not answer in {CONNECT_SECS} s"))?
        .map_err(|error| format!("cannot connect to {place}: {error}"))?;
    // A failed option only makes small writes wait a little; nothing depends on it.
    let _ = socket.set_nodelay(true);
    if !address.secure {
        return Ok(Box::new(socket));
    }
    let config = tls.ok_or_else(|| format!("{place} is under TLS and no trust was given"))?;
    let name = server_name.unwrap_or(&address.host).to_owned();
    let server = ServerName::try_from(name.clone())
        .map_err(|_| format!("{name:?} is not a name a certificate can be checked for"))?;
    let secured = tokio::time::timeout(
        wait,
        TlsConnector::from(Arc::clone(config)).connect(server, socket),
    )
    .await
    .map_err(|_| format!("{place} did not finish the TLS handshake in {CONNECT_SECS} s"))?
    .map_err(|error| format!("TLS to {place} as {name}: {error}"))?;
    Ok(Box::new(secured))
}

/// What came back to a request.
#[derive(Debug)]
pub struct Reply {
    /// The status.
    pub status: u16,
    /// The headers.
    pub headers: http::HeaderMap,
    /// The body.
    pub body: Vec<u8>,
}

impl Reply {
    /// The value of a header, where there is one.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// Makes one request over `stream` and reads the whole answer.
pub async fn request(
    stream: Stream,
    method: http::Method,
    authority: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Result<Reply, String> {
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(|error| format!("the HTTP exchange could not begin: {error}"))?;
    tokio::spawn(connection);
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header(http::header::HOST, authority)
        .header(http::header::CONNECTION, "close");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Full::new(body))
        .map_err(|error| format!("the request is not well formed: {error}"))?;
    let wait = Duration::from_secs(REQUEST_SECS);
    let response = tokio::time::timeout(wait, sender.send_request(request))
        .await
        .map_err(|_| format!("no answer in {REQUEST_SECS} s"))?
        .map_err(|error| format!("the connection ended before an answer: {error}"))?;
    let (parts, incoming) = response.into_parts();
    let body = tokio::time::timeout(wait, incoming.collect())
        .await
        .map_err(|_| format!("the answer did not finish in {REQUEST_SECS} s"))?
        .map_err(|error| format!("the answer was cut off: {error}"))?
        .to_bytes()
        .to_vec();
    Ok(Reply {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_gives_its_host_port_and_path_as_a_browser_reads_them() {
        let address = Address::parse("https://orch.example.com/").expect("an address");
        assert_eq!(
            (
                address.secure,
                address.host.as_str(),
                address.port,
                address.base.as_str()
            ),
            (true, "orch.example.com", 443, "")
        );
        assert_eq!(address.authority(), "orch.example.com");
        let local = Address::parse("http://127.0.0.1:41201").expect("an address");
        assert_eq!(local.authority(), "127.0.0.1:41201");
        assert_eq!(
            local.websocket("/api/v1/ws"),
            "ws://127.0.0.1:41201/api/v1/ws"
        );
    }

    #[test]
    fn a_path_is_added_below_the_address_the_operator_gave() {
        let behind = Address::parse("https://example.com/orch/").expect("an address");
        assert_eq!(behind.path("/health"), "/orch/health");
    }

    #[test]
    fn an_address_that_is_not_web_is_refused() {
        assert!(Address::parse("ftp://example.com").is_err());
        assert!(Address::parse("example.com").is_err());
    }

    #[test]
    fn a_name_can_be_sent_to_an_address_of_the_operators_choosing() {
        let resolve =
            Resolve::parse(&["Gateway.Orch.Internal=172.17.0.1".to_owned()]).expect("pairs");
        assert_eq!(
            resolve.lookup("gateway.orch.internal"),
            Some("172.17.0.1".parse().expect("an address"))
        );
        assert!(Resolve::parse(&["no-equals".to_owned()]).is_err());
        assert!(Resolve::parse(&["name=not-an-address".to_owned()]).is_err());
    }
}
