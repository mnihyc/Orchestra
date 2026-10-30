//! Connections out of a machine, directly or through the proxy the machine names: an HTTP proxy asked for a tunnel
//! with `CONNECT`, or a SOCKS proxy; the TLS above either is the caller's own.
//!
//! The proxy is the system's: the variables every program reads (`HTTPS_PROXY` for a destination spoken to over
//! TLS, `HTTP_PROXY` for a plain one, `ALL_PROXY` for both, `NO_PROXY` for the hosts reached directly, in upper or
//! lower case), and where none of them is set, the proxy settings of macOS or Windows. A restricted unit's driver
//! names its host's proxy in those variables, and an installed connector's service carries the ones of the shell
//! that installed it or those its installer was given. A connection of the machine's own (the link to the gateway,
//! an enrollment, a model call it forwards) asks the proxy for a way to its host and port and then speaks its own TLS
//! through it, so that a pinned certificate stays pinned end to end and the proxy sees nothing of what passes.
//!
//! An `http://` proxy is asked for a tunnel; a `socks5://` one is given an address resolved here, a `socks5h://` one
//! the name, resolved at the proxy. Credentials in the proxy's address are sent: as Basic to an HTTP proxy, as a
//! username and password (RFC 1929) to a SOCKS5 one. Each answer of a proxy is read exactly, so that nothing the
//! destination sends after it is taken. A proxy of another scheme (`https://`, a TLS connection to the proxy itself,
//! or SOCKS4) is not one this program speaks: it is said once, and the connection is made directly, as it was before
//! there was a proxy. Where no proxy is named, or `NO_PROXY` keeps the host from it, the connection is made
//! directly.

use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use http::{HeaderValue, Uri};
use hyper_util::client::proxy::matcher::{Intercept, Matcher};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The most of a proxy's answer to `CONNECT` that is read: a status line and a few headers.
const ANSWER_MAX: usize = 8 * 1024;
/// The port of a SOCKS proxy whose address names none.
const SOCKS_PORT: u16 = 1080;

/// Whether a proxy this program does not speak has been said already.
static UNSPOKEN_SAID: AtomicBool = AtomicBool::new(false);

/// The proxy outward connections go through, and the hosts they do not.
#[derive(Clone)]
pub struct OutwardProxy {
    matcher: Arc<Matcher>,
}

impl std::fmt::Debug for OutwardProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The matcher's own description would show credentials.
        f.debug_struct("OutwardProxy").finish_non_exhaustive()
    }
}

/// How a connection reaches its destination through a proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// An HTTP proxy, asked for a tunnel with `CONNECT`.
    Tunnel,
    /// A SOCKS5 proxy; `remote_names` when the proxy resolves the destination's name (`socks5h`).
    Socks5 {
        /// Whether the name goes to the proxy.
        remote_names: bool,
    },
    /// A proxy of a scheme this program does not speak.
    Unspoken(String),
}

/// The proxy one connection goes through: how, where it is, and what it is told of the caller.
#[derive(Clone)]
pub struct Route {
    /// How.
    pub via: Via,
    /// Where the proxy is, as its address was named (with a SOCKS port where it named none).
    proxy: Uri,
    /// The Basic credentials for an HTTP proxy.
    basic: Option<HeaderValue>,
    /// The username and password for a SOCKS5 proxy.
    login: Option<(String, String)>,
}

impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Route")
            .field("via", &self.via)
            .field("proxy", &self.shown())
            .finish_non_exhaustive()
    }
}

impl Route {
    /// The proxy as a person may be shown it: its scheme, host and port, without credentials.
    #[must_use]
    pub fn shown(&self) -> String {
        let scheme = self.proxy.scheme_str().unwrap_or("http");
        let host = self.proxy.host().unwrap_or_default();
        match self.proxy.port_u16() {
            Some(port) => format!("{scheme}://{host}:{port}"),
            None => format!("{scheme}://{host}"),
        }
    }

    /// Whether the proxy is given credentials.
    #[must_use]
    pub const fn with_credentials(&self) -> bool {
        self.basic.is_some() || self.login.is_some()
    }

    fn of(intercept: &Intercept) -> Self {
        let uri = intercept.uri().clone();
        let scheme = uri.scheme_str().unwrap_or("http").to_ascii_lowercase();
        let via = match scheme.as_str() {
            "http" => Via::Tunnel,
            "socks5" | "socks5h" => Via::Socks5 {
                remote_names: scheme == "socks5h",
            },
            other => Via::Unspoken(other.to_owned()),
        };
        let proxy = match (&via, uri.port_u16()) {
            (Via::Socks5 { .. }, None) => {
                let host = uri.host().unwrap_or_default();
                format!("{scheme}://{}", authority(host, SOCKS_PORT))
                    .parse()
                    .unwrap_or(uri)
            }
            _ => uri,
        };
        Self {
            via,
            proxy,
            basic: intercept.basic_auth().cloned(),
            login: intercept
                .raw_auth()
                .map(|(user, password)| (user.to_owned(), password.to_owned())),
        }
    }

    /// The proxy's host and port; an HTTP proxy that names no port is on 80.
    fn address(&self) -> (String, u16) {
        let host = self
            .proxy
            .host()
            .unwrap_or_default()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        (host, self.proxy.port_u16().unwrap_or(80))
    }
}

impl OutwardProxy {
    /// The system's proxy: the variables of this process's environment, and where none is set, the proxy settings
    /// of macOS or Windows. It routes nothing when nothing is named.
    #[must_use]
    pub fn system() -> Self {
        Self {
            matcher: Arc::new(Matcher::from_system()),
        }
    }

    /// The proxy `proxy` names for every destination (`http://host:port`, `socks5h://user:pass@host`, or
    /// `host:port` for HTTP), with the hosts `no_proxy` keeps from it (comma-separated names, each also covering
    /// the names below it, addresses and networks; `*` keeps every host); none when `proxy` names nothing.
    #[must_use]
    pub fn from_values(proxy: Option<&str>, no_proxy: Option<&str>) -> Option<Self> {
        let proxy = proxy?.trim();
        if proxy.is_empty() {
            return None;
        }
        let matcher = Matcher::builder()
            .all(proxy.to_owned())
            .no(no_proxy.unwrap_or_default().to_owned())
            .build();
        Some(Self {
            matcher: Arc::new(matcher),
        })
    }

    /// The proxy a connection to `host` at `port` goes through, spoken to over TLS when `secure`; none when it is
    /// made directly.
    #[must_use]
    pub fn route(&self, host: &str, port: u16, secure: bool) -> Option<Route> {
        let scheme = if secure { "https" } else { "http" };
        let destination: Uri = format!("{scheme}://{}", authority(host, port))
            .parse()
            .ok()?;
        self.matcher
            .intercept(&destination)
            .map(|intercept| Route::of(&intercept))
    }

    /// Whether a connection to `host` (over TLS) goes through the proxy.
    #[must_use]
    pub fn applies_to(&self, host: &str) -> bool {
        self.route(host, 443, true).is_some()
    }
}

/// The authority of `host` at `port` as a request line writes it, with an address of IPv6 in brackets.
fn authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// The status of a proxy's answer, from its first line (`HTTP/1.1 200 Connection established`).
fn status_of(answer: &[u8]) -> Option<u16> {
    let line = answer.split(|byte| *byte == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/1.") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// Connects to `host` at `port`, a destination spoken to over TLS when `secure`: through `proxy` where it routes
/// the host, otherwise directly. What comes back is a socket to the host, through which the caller speaks as it
/// would directly.
pub async fn connect(
    host: &str,
    port: u16,
    secure: bool,
    proxy: Option<&OutwardProxy>,
) -> io::Result<TcpStream> {
    let Some(route) = proxy.and_then(|proxy| proxy.route(host, port, secure)) else {
        return TcpStream::connect((host, port)).await;
    };
    match &route.via {
        Via::Tunnel => tunnel(&route, host, port).await,
        Via::Socks5 { remote_names } => socks5(&route, host, port, *remote_names).await,
        Via::Unspoken(scheme) => {
            if !UNSPOKEN_SAID.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    proxy = route.shown(),
                    scheme,
                    "the system names a proxy of a scheme this program does not speak; connections are made directly"
                );
            }
            TcpStream::connect((host, port)).await
        }
    }
}

fn refused(route: &Route, host: &str, port: u16, said: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionRefused,
        format!(
            "the proxy {} did not open the way to {}: {said}",
            route.shown(),
            authority(host, port)
        ),
    )
}

/// What a SOCKS5 proxy's status says (RFC 1928, section 6).
fn socks5_status(status: u8) -> &'static str {
    match status {
        1 => "general failure",
        2 => "not allowed by its rules",
        3 => "network unreachable",
        4 => "host unreachable",
        5 => "connection refused",
        6 => "time to live expired",
        7 => "command not supported",
        8 => "address type not supported",
        _ => "an unknown status",
    }
}

/// Asks a SOCKS5 proxy for a connection to `host` at `port`, with the name when the proxy resolves it
/// (`remote_names`), otherwise with an address resolved here.
async fn socks5(route: &Route, host: &str, port: u16, remote_names: bool) -> io::Result<TcpStream> {
    let invalid = |said: &str| io::Error::new(io::ErrorKind::InvalidInput, said.to_owned());
    let (proxy_host, proxy_port) = route.address();
    let mut stream = TcpStream::connect((proxy_host.as_str(), proxy_port)).await?;
    // One way of signing in is offered: a username and password where the address names them, else none.
    let method = if route.login.is_some() { 2 } else { 0 };
    stream.write_all(&[5, 1, method]).await?;
    let mut chosen = [0_u8; 2];
    stream.read_exact(&mut chosen).await?;
    if chosen != [5, method] {
        return Err(refused(
            route,
            host,
            port,
            "it takes none of the ways this program signs in",
        ));
    }
    if let Some((user, password)) = &route.login {
        let (user, password) = (user.as_bytes(), password.as_bytes());
        let (Ok(user_length), Ok(password_length)) =
            (u8::try_from(user.len()), u8::try_from(password.len()))
        else {
            return Err(invalid(
                "the proxy's username and password are each at most 255 bytes",
            ));
        };
        let mut login = vec![1, user_length];
        login.extend_from_slice(user);
        login.push(password_length);
        login.extend_from_slice(password);
        stream.write_all(&login).await?;
        let mut answer = [0_u8; 2];
        stream.read_exact(&mut answer).await?;
        if answer[1] != 0 {
            return Err(refused(
                route,
                host,
                port,
                "it refused the username and password",
            ));
        }
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let address = match bare.parse::<IpAddr>() {
        Ok(address) => Some(address),
        Err(_) if remote_names => None,
        Err(_) => Some(
            tokio::net::lookup_host((bare, port))
                .await?
                .next()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("`{bare}` resolves to no address"),
                    )
                })?
                .ip(),
        ),
    };
    let mut request = vec![5, 1, 0];
    match address {
        Some(IpAddr::V4(address)) => {
            request.push(1);
            request.extend_from_slice(&address.octets());
        }
        Some(IpAddr::V6(address)) => {
            request.push(4);
            request.extend_from_slice(&address.octets());
        }
        None => {
            let Ok(length) = u8::try_from(bare.len()) else {
                return Err(invalid("a host name is at most 255 bytes"));
            };
            request.push(3);
            request.push(length);
            request.extend_from_slice(bare.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;
    // The answer is read exactly: its head, then the address the proxy bound, whose length its kind says.
    let mut head = [0_u8; 4];
    stream.read_exact(&mut head).await?;
    if head[1] != 0 {
        return Err(refused(
            route,
            host,
            port,
            &format!("{} ({})", socks5_status(head[1]), head[1]),
        ));
    }
    let bound = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut length = [0_u8; 1];
            stream.read_exact(&mut length).await?;
            usize::from(length[0])
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the proxy answered with an address of an unknown kind ({other})"),
            ));
        }
    };
    let mut rest = vec![0_u8; bound + 2];
    stream.read_exact(&mut rest).await?;
    Ok(stream)
}

/// Asks an HTTP proxy for a tunnel to `host` at `port`.
async fn tunnel(route: &Route, host: &str, port: u16) -> io::Result<TcpStream> {
    let (proxy_host, proxy_port) = route.address();
    let mut stream = TcpStream::connect((proxy_host.as_str(), proxy_port)).await?;
    let target = authority(host, port);
    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(basic) = route.basic.as_ref().and_then(|value| value.to_str().ok()) {
        request.push_str(&format!("Proxy-Authorization: {basic}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    // Read the answer a byte at a time, so that nothing of what the tunnel carries afterwards is taken.
    let mut answer: Vec<u8> = Vec::with_capacity(128);
    let mut byte = [0_u8; 1];
    while !answer.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the proxy ended the connection before it answered",
            ));
        }
        answer.push(byte[0]);
        if answer.len() > ANSWER_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the proxy's answer is longer than an answer to CONNECT is",
            ));
        }
    }
    match status_of(&answer) {
        Some(200) => Ok(stream),
        _ => {
            let first =
                String::from_utf8_lossy(answer.split(|b| *b == b'\r').next().unwrap_or_default())
                    .into_owned();
            Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("the proxy refused the connection to {target}: {first}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn a_proxy_is_read_with_or_without_its_scheme_and_spoken_to_as_its_scheme_says() {
        let route = |proxy: &str| {
            OutwardProxy::from_values(Some(proxy), None)
                .unwrap()
                .route("gateway.orch.internal", 9443, true)
                .unwrap()
        };
        let http = route("http://10.199.0.2:3128");
        assert_eq!(http.via, Via::Tunnel);
        assert_eq!(http.shown(), "http://10.199.0.2:3128");
        assert_eq!(route("10.199.0.2:3128").shown(), http.shown());
        assert_eq!(
            route("socks5://10.0.0.1:1080").via,
            Via::Socks5 {
                remote_names: false
            }
        );
        let named = route("socks5h://ada:secret@proxy.example");
        assert_eq!(named.via, Via::Socks5 { remote_names: true });
        assert_eq!(
            named.shown(),
            "socks5h://proxy.example:1080",
            "a SOCKS proxy without a port is on 1080"
        );
        assert!(named.with_credentials());
        assert!(
            !format!("{named:?}").contains("secret"),
            "credentials are never shown"
        );
        assert_eq!(
            route("socks4a://proxy:1081").via,
            Via::Unspoken("socks4a".into())
        );
        assert_eq!(
            route("https://proxy:443").via,
            Via::Unspoken("https".into())
        );
        assert!(OutwardProxy::from_values(Some("  "), None).is_none());
        assert!(OutwardProxy::from_values(None, Some("localhost")).is_none());
    }

    #[test]
    fn no_proxy_keeps_its_hosts_and_the_names_below_them_local() {
        let proxy = OutwardProxy::from_values(
            Some("http://proxy:3128"),
            Some("localhost, 127.0.0.1,::1,.internal.example,10.1.0.0/16"),
        )
        .unwrap();
        assert!(!proxy.applies_to("localhost"));
        assert!(!proxy.applies_to("127.0.0.1"));
        assert!(!proxy.applies_to("[::1]"));
        assert!(!proxy.applies_to("db.internal.example"));
        assert!(!proxy.applies_to("10.1.2.3"));
        assert!(proxy.applies_to("gateway.orch.internal"));
        assert!(proxy.applies_to("pypi.org"));
        let none = OutwardProxy::from_values(Some("http://proxy:3128"), Some("*")).unwrap();
        assert!(!none.applies_to("pypi.org"));
    }

    #[tokio::test]
    async fn a_tunnel_is_asked_for_by_host_and_port_with_the_proxy_s_credentials_and_a_refusal_is_an_error()
     {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = tokio::spawn(async move {
            let mut asked = Vec::new();
            let answers: [&[u8]; 2] = [
                b"HTTP/1.1 200 Connection established\r\n\r\nhello",
                b"HTTP/1.1 403 Forbidden\r\n\r\n",
            ];
            for answer in answers {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut head = vec![0_u8; 512];
                let count = socket.read(&mut head).await.unwrap();
                asked.push(String::from_utf8_lossy(&head[..count]).into_owned());
                socket.write_all(answer).await.unwrap();
            }
            asked
        });
        let proxy =
            OutwardProxy::from_values(Some(&format!("http://ada:secret@127.0.0.1:{port}")), None)
                .unwrap();
        let mut tunnel = connect("gateway.orch.internal", 9443, true, Some(&proxy))
            .await
            .unwrap();
        let mut rest = [0_u8; 5];
        tunnel.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"hello", "what follows the answer is the tunnel's");
        let refused = connect("evil.example", 443, true, Some(&proxy))
            .await
            .unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
        let asked = served.await.unwrap();
        assert!(
            asked[0].starts_with("CONNECT gateway.orch.internal:9443 HTTP/1.1\r\n"),
            "{asked:?}"
        );
        assert!(
            asked[0].contains("\r\nProxy-Authorization: Basic YWRhOnNlY3JldA==\r\n"),
            "{asked:?}"
        );
        assert!(asked[1].starts_with("CONNECT evil.example:443 "));
    }

    /// A SOCKS5 proxy that wants a username and password, takes one connection, and says what it was asked.
    async fn socks5_proxy(listener: TcpListener) -> (Vec<u8>, Vec<u8>) {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut greeting = [0_u8; 2];
        socket.read_exact(&mut greeting).await.unwrap();
        let mut methods = vec![0_u8; usize::from(greeting[1])];
        socket.read_exact(&mut methods).await.unwrap();
        assert!(
            methods.contains(&2),
            "the username and password method is offered"
        );
        socket.write_all(&[5, 2]).await.unwrap();
        let mut login = vec![0_u8; 2];
        socket.read_exact(&mut login).await.unwrap();
        let mut user = vec![0_u8; usize::from(login[1])];
        socket.read_exact(&mut user).await.unwrap();
        let mut length = [0_u8; 1];
        socket.read_exact(&mut length).await.unwrap();
        let mut password = vec![0_u8; usize::from(length[0])];
        socket.read_exact(&mut password).await.unwrap();
        socket.write_all(&[1, 0]).await.unwrap();
        let mut request = vec![0_u8; 4];
        socket.read_exact(&mut request).await.unwrap();
        let rest = match request[3] {
            1 => 4 + 2,
            4 => 16 + 2,
            _ => {
                let mut length = [0_u8; 1];
                socket.read_exact(&mut length).await.unwrap();
                request.push(length[0]);
                usize::from(length[0]) + 2
            }
        };
        let mut address = vec![0_u8; rest];
        socket.read_exact(&mut address).await.unwrap();
        request.extend_from_slice(&address);
        socket
            .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        socket.write_all(b"hello").await.unwrap();
        let mut credentials = user;
        credentials.push(b':');
        credentials.extend_from_slice(&password);
        (credentials, request)
    }

    #[tokio::test]
    async fn a_socks5h_proxy_is_given_the_name_and_the_credentials_and_the_tunnel_carries_what_follows()
     {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = tokio::spawn(socks5_proxy(listener));
        let proxy = OutwardProxy::from_values(
            Some(&format!("socks5h://ada:secret@127.0.0.1:{port}")),
            None,
        )
        .unwrap();
        let mut tunnel = connect("gateway.orch.internal", 9443, true, Some(&proxy))
            .await
            .unwrap();
        let mut rest = [0_u8; 5];
        tunnel.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"hello");
        let (credentials, request) = served.await.unwrap();
        assert_eq!(credentials, b"ada:secret");
        let name = b"gateway.orch.internal";
        let mut expected = vec![5, 1, 0, 3, u8::try_from(name.len()).unwrap()];
        expected.extend_from_slice(name);
        expected.extend_from_slice(&9443_u16.to_be_bytes());
        assert_eq!(request, expected, "the proxy resolves the name");
    }

    #[tokio::test]
    async fn a_socks5_proxy_is_given_an_address_resolved_here() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = tokio::spawn(socks5_proxy(listener));
        let proxy =
            OutwardProxy::from_values(Some(&format!("socks5://u:p@127.0.0.1:{port}")), None)
                .unwrap();
        let mut tunnel = connect("localhost", 9443, false, Some(&proxy))
            .await
            .unwrap();
        let mut rest = [0_u8; 5];
        tunnel.read_exact(&mut rest).await.unwrap();
        let (_, request) = served.await.unwrap();
        assert!(
            matches!(request[3], 1 | 4),
            "an address, never the name: {request:?}"
        );
        assert_eq!(&request[request.len() - 2..], &9443_u16.to_be_bytes());
    }
}
