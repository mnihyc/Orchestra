//! The node's trusted parent for already admitted destinations, through the existing CONNECT client.

use orch_link::outward::{OutwardProxy, Via};
use std::{io, time::Duration};
use tokio::net::TcpStream;

/// Same total bound as a direct proxy connection, including parent resolution and its CONNECT answer.
const CONNECT_WAIT: Duration = Duration::from_secs(15);

/// Trusted node configuration. Debug never includes a proxy credential.
#[derive(Clone)]
pub struct Parent {
    address: String,
    proxy: OutwardProxy,
}
impl std::fmt::Debug for Parent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ParentProxy([private])")
    }
}
impl PartialEq for Parent {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}
impl Eq for Parent {}
impl Parent {
    /// Parse only the node's HTTP parent; templates and request headers never enter here.
    pub fn parse(address: &str) -> Result<Self, &'static str> {
        let url =
            url::Url::parse(address.trim()).map_err(|_| "the parent proxy address is invalid")?;
        if url.scheme() != "http"
            || url.host().is_none()
            || url.port_or_known_default().is_none_or(|port| port == 0)
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("the parent proxy must name an HTTP CONNECT endpoint");
        }
        let proxy = OutwardProxy::from_values(Some(url.as_str()), None)
            .ok_or("the parent proxy address is invalid")?;
        let route = proxy
            .route("parent-probe.invalid", 443, true)
            .ok_or("the parent proxy address is invalid")?;
        if route.via != Via::Tunnel {
            return Err("the parent proxy must speak HTTP CONNECT");
        }
        Ok(Self {
            address: url.to_string(),
            proxy,
        })
    }
    /// Only canonical admitted names or exact trusted transport pairs reach this connector.
    pub async fn connect(&self, host: &str, port: u16) -> io::Result<TcpStream> {
        let route = self
            .proxy
            .route(host, port, true)
            .filter(|route| route.via == Via::Tunnel)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the parent proxy route is unavailable",
                )
            })?;
        tokio::time::timeout(CONNECT_WAIT, route.connect_tunnel(host, port))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "the parent proxy did not answer")
            })?
    }
}

/// Restricted support uses the same parent path to the node's configured Orch transport.
pub async fn probe(parent: Option<&str>, transport: Option<&str>) -> bool {
    let Some(parent) = parent else {
        return true;
    };
    let Ok(parent) = Parent::parse(parent) else {
        return false;
    };
    let Some(transport) = transport else {
        return false;
    };
    let Ok(url) = url::Url::parse(transport) else {
        return false;
    };
    if !matches!(url.scheme(), "wss" | "https" | "ws" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return false;
    };
    parent.connect(host, port).await.is_ok()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn head(stream: &mut TcpStream) -> Vec<u8> {
        let mut head = Vec::new();
        let mut byte = [0];
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 8192);
            stream.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
        }
        head
    }
    #[test]
    fn only_http_parent_endpoints_parse_and_debug_never_exposes_credentials() {
        let parent = Parent::parse("http://operator:private-password@proxy.invalid:3128").unwrap();
        assert!(!format!("{parent:?}").contains("private-password"));
        for address in [
            "https://proxy.invalid",
            "socks5://proxy.invalid:1080",
            "http://proxy.invalid/private",
            "http://proxy.invalid?secret=value",
            "http://proxy.invalid/#fragment",
            "",
        ] {
            assert!(Parent::parse(address).is_err(), "{address}");
        }
    }
    #[tokio::test]
    async fn parent_resolves_the_admitted_name_and_connect_header_never_consumes_destination_bytes()
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let parent = Parent::parse(&format!("http://operator:private-password@{address}")).unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let asked = String::from_utf8(head(&mut socket).await).unwrap();
            assert!(asked.starts_with("CONNECT approved.invalid:443 HTTP/1.1\r\n"));
            assert!(asked.contains("Proxy-Authorization: Basic "));
            socket
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\nDATA")
                .await
                .unwrap();
        });
        let mut stream = parent.connect("approved.invalid", 443).await.unwrap();
        let mut bytes = [0; 4];
        stream.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"DATA");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn parent_refusal_never_downgrades_and_the_support_probe_uses_the_same_connect_path() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for allowed in [false, true] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let asked = String::from_utf8(head(&mut socket).await).unwrap();
                assert!(asked.starts_with("CONNECT gateway.invalid:9443 HTTP/1.1\r\n"));
                socket
                    .write_all(if allowed {
                        b"HTTP/1.1 200 OK\r\n\r\n"
                    } else {
                        b"HTTP/1.1 403 Forbidden\r\n\r\n"
                    })
                    .await
                    .unwrap();
            }
        });
        let url = format!("http://{address}");
        assert!(!probe(Some(&url), Some("wss://gateway.invalid:9443")).await);
        assert!(probe(Some(&url), Some("wss://gateway.invalid:9443")).await);
        assert!(!probe(Some(&url), None).await);
        assert!(probe(None, None).await);
        server.await.unwrap();
    }
}
