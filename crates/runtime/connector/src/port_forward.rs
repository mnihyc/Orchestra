//! The helper of a port: a listener on the caller's own loopback whose each connection, or each UDP sender, is carried to a port of an environment.
//!
//! `orch-connector port forward` is run by whoever may use a port: a signed-in person with their
//! token, or a link's holder with the link. Either comes on standard input, never as an argument,
//! and is never printed. The helper binds only its caller's loopback, prints the address it bound,
//! and stays in the foreground. For each local TCP connection, and for each local UDP sender, it
//! takes one use of the port at the installation's ordinary address and carries it over that use's
//! socket; its own end closes those uses, never the opening. A lost admission or a lapsed link ends
//! the uses; a helper started again takes fresh ones.

use crate::https::{HttpsClient, HttpsError, https_base};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use hyper::Method;
use orch_api_contract::ports::{
    EnvironmentPortUseCreateBody, PortLinkMethod, PortLinkUseCreateBody, PortTransport, PortUse,
};
use orch_types::{Id, SystemClock};
use rustls::pki_types::ServerName;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

/// TCP or UDP, as the helper is asked to carry.
pub use orch_api_contract::ports::PortTransport as Transport;

/// The WebSocket subprotocol of a port's raw socket, as the installation speaks it.
pub const PORT_SUBPROTOCOL: &str = "orch.port.v1";
/// The text message that ends one TCP direction while the other goes on.
pub const END_OF_DIRECTION: &str = "eof";
/// How long taking a use may take: an ordinary request over a slow link.
const REQUEST_LIMIT: Duration = Duration::from_secs(30);
/// The most bytes read from a local connection into one message: a few slow-link frames.
const CHUNK_BYTES: usize = 65_536;
/// The most bytes read of one local datagram: one more than a frame carries (4 000), so that a
/// larger one is seen and dropped here as the carrier would drop it.
const DATAGRAM_READ_BYTES: usize = 4_001;
/// The largest datagram carried, as the frame allows.
const DATAGRAM_BYTES: usize = 4_000;
/// The silence that ends a local UDP sender's association, as the carrier's own minute.
const UDP_IDLE: Duration = Duration::from_secs(60);
/// Datagrams of one sender waiting for its socket: a short burst, dropped beyond, as the carrier
/// drops rather than holding.
const SENDER_QUEUE: usize = 64;

/// How the caller proves they may use the port. Its secret is never printed.
pub enum Credential {
    /// A signed-in person's token, with the environment and the port it is used for.
    Token {
        /// The token.
        token: String,
        /// The environment.
        environment: Id,
        /// Its port.
        port: u16,
    },
    /// A link's whole address.
    Link(String),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token {
                environment, port, ..
            } => f
                .debug_struct("Token")
                .field("environment", environment)
                .field("port", port)
                .finish_non_exhaustive(),
            Self::Link(_) => f.write_str("Link(..)"),
        }
    }
}

/// What one helper carries, and where it asks.
#[derive(Debug)]
pub struct Forward {
    /// The installation's ordinary address.
    pub base: Url,
    /// How the caller proves they may.
    pub credential: Credential,
    /// TCP or UDP.
    pub transport: PortTransport,
}

/// Why a helper could not start, or could not carry one connection.
#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    /// An address could not be used.
    #[error("{0}")]
    Address(String),
    /// The loopback address could not be bound.
    #[error("{address} cannot be listened on: {source}")]
    Listen {
        /// The address.
        address: SocketAddr,
        /// Why not.
        source: std::io::Error,
    },
    /// The installation refused the use, with its words.
    #[error("the installation refused the use ({status}): {detail}")]
    Refused {
        /// The status of its answer.
        status: u16,
        /// What it said.
        detail: String,
    },
    /// A request could not be made.
    #[error(transparent)]
    Https(#[from] HttpsError),
    /// The use's socket could not be opened.
    #[error("the socket of the use: {0}")]
    Socket(String),
}

impl Forward {
    /// The installation's address that a link names: its scheme, host and port.
    pub fn base_of_link(link: &str) -> Result<Url, ForwardError> {
        let url = Url::parse(link.trim()).map_err(|_| {
            ForwardError::Address("that is not a link: give the whole address of the link".into())
        })?;
        https_base(url.as_str()).map_err(|error| ForwardError::Address(error.to_string()))
    }
}

/// A bound loopback listener.
#[derive(Debug)]
pub enum Listener {
    /// For TCP: each accepted connection is one use.
    Tcp(TcpListener),
    /// For UDP: each sender is one use.
    Udp(UdpSocket),
}

impl Listener {
    /// The address it actually listens on.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        match self {
            Self::Tcp(listener) => listener.local_addr(),
            Self::Udp(socket) => socket.local_addr(),
        }
    }
}

/// Binds the caller's loopback: the address given, which must be a loopback one and free, or a free
/// port of 127.0.0.1.
pub async fn bind(
    listen: Option<SocketAddr>,
    transport: PortTransport,
) -> Result<Listener, ForwardError> {
    let address = listen.unwrap_or_else(|| SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
    if !address.ip().is_loopback() {
        return Err(ForwardError::Address(format!(
            "{address} is not a loopback address: the helper listens on this machine alone"
        )));
    }
    let wrong = |source| ForwardError::Listen { address, source };
    Ok(match transport {
        PortTransport::Tcp => Listener::Tcp(TcpListener::bind(address).await.map_err(wrong)?),
        PortTransport::Udp => Listener::Udp(UdpSocket::bind(address).await.map_err(wrong)?),
    })
}

/// Carries until `stop` resolves: each local connection or sender gets a use and a socket of its
/// own. A failure of one is told on standard error and the others go on.
pub async fn carry(
    forward: Arc<Forward>,
    listener: Listener,
    stop: impl std::future::Future<Output = ()>,
) {
    tokio::pin!(stop);
    match listener {
        Listener::Tcp(listener) => loop {
            tokio::select! {
                () = &mut stop => return,
                accepted = listener.accept() => {
                    let Ok((local, _)) = accepted else { continue };
                    let forward = Arc::clone(&forward);
                    tokio::spawn(async move {
                        if let Err(error) = carry_tcp(&forward, local).await {
                            eprintln!("a connection was not carried: {error}");
                        }
                    });
                }
            }
        },
        Listener::Udp(socket) => carry_udp(forward, Arc::new(socket), stop).await,
    }
}

fn tls() -> Result<Arc<rustls::ClientConfig>, ForwardError> {
    orch_pki::verification::machine_client_config(
        None,
        orch_pki::certificates::ServiceName::Frontend,
        None,
        Arc::new(SystemClock),
    )
    .map_err(|error| ForwardError::Socket(error.to_string()))
}

/// Takes one use of the port, as the caller's credential allows.
async fn take_use(forward: &Forward) -> Result<PortUse, ForwardError> {
    let client = HttpsClient::new(tls()?);
    let json = ("content-type", "application/json".to_owned());
    let (path, headers, body) = match &forward.credential {
        Credential::Token {
            token,
            environment,
            port,
        } => (
            format!("/api/v1/environments/{environment}/ports/{port}/use"),
            vec![json, ("authorization", format!("Bearer {token}"))],
            serde_json::to_vec(&EnvironmentPortUseCreateBody {
                transport: forward.transport,
                opening_id: None,
            }),
        ),
        Credential::Link(link) => (
            "/api/v1/port-links/use".to_owned(),
            vec![json],
            serde_json::to_vec(&PortLinkUseCreateBody {
                link: link.trim().to_owned(),
                transport: forward.transport,
                entry_method: PortLinkMethod::Helper,
            }),
        ),
    };
    let body = body.map_err(|error| ForwardError::Socket(error.to_string()))?;
    let reply = client
        .send(
            &forward.base,
            Method::POST,
            &path,
            &headers,
            Bytes::from(body),
            REQUEST_LIMIT,
        )
        .await?;
    if !reply.status.is_success() {
        // The problem's own words; the credential is in neither.
        let detail = serde_json::from_slice::<serde_json::Value>(&reply.body)
            .ok()
            .and_then(|problem| {
                ["detail", "title"].iter().find_map(|key| {
                    problem
                        .get(*key)
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                })
            })
            .unwrap_or_default();
        return Err(ForwardError::Refused {
            status: reply.status.as_u16(),
            detail,
        });
    }
    serde_json::from_slice(&reply.body)
        .map_err(|error| ForwardError::Socket(format!("the use cannot be read: {error}")))
}

/// The socket of one use, at the installation's address over its ordinary trust.
type UseSocket = tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

async fn open_socket(forward: &Forward, taken: &PortUse) -> Result<UseSocket, ForwardError> {
    let host = forward
        .base
        .host_str()
        .ok_or_else(|| ForwardError::Address(forward.base.to_string()))?
        .to_owned();
    let port = forward
        .base
        .port_or_known_default()
        .ok_or_else(|| ForwardError::Address(forward.base.to_string()))?;
    let authority = match forward.base.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.clone(),
    };
    let socket = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|error| ForwardError::Socket(error.to_string()))?;
    let name = ServerName::try_from(host.clone())
        .map_err(|_| ForwardError::Address(format!("`{host}` is not a name to verify")))?;
    let stream = TlsConnector::from(tls()?)
        .connect(name, socket)
        .await
        .map_err(|error| ForwardError::Socket(error.to_string()))?;
    let mut request = format!(
        "wss://{authority}/api/v1/ws/ports/{}?ticket={}",
        taken.id, taken.ticket
    )
    .into_client_request()
    .map_err(|error| ForwardError::Socket(error.to_string()))?;
    request.headers_mut().insert(
        "sec-websocket-protocol",
        PORT_SUBPROTOCOL
            .parse()
            .map_err(|_| ForwardError::Socket("the subprotocol cannot be named".into()))?,
    );
    tokio_tungstenite::client_async(request, stream)
        .await
        .map(|(socket, _)| socket)
        .map_err(|error| ForwardError::Socket(error.to_string()))
}

/// One local TCP connection, carried both ways; each direction ends by itself.
async fn carry_tcp(forward: &Forward, local: TcpStream) -> Result<(), ForwardError> {
    let taken = take_use(forward).await?;
    let socket = open_socket(forward, &taken).await?;
    let (mut sink, mut stream) = socket.split();
    let (mut reading, mut writing) = local.into_split();
    let outbound = async {
        let mut buffer = vec![0u8; CHUNK_BYTES];
        loop {
            match reading.read(&mut buffer).await {
                Ok(0) | Err(_) => {
                    let _ = sink.send(Message::text(END_OF_DIRECTION)).await;
                    break;
                }
                Ok(read) => {
                    if sink
                        .send(Message::binary(Bytes::copy_from_slice(&buffer[..read])))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        sink
    };
    let inbound = async {
        while let Some(Ok(message)) = stream.next().await {
            match message {
                Message::Binary(bytes) => {
                    if writing.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Text(text) if text.as_str() == END_OF_DIRECTION => {
                    let _ = writing.shutdown().await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = writing.shutdown().await;
    };
    let (mut sink, ()) = tokio::join!(outbound, inbound);
    let _ = sink.close().await;
    Ok(())
}

/// The UDP side: every local sender gets a use and a socket of its own, which ends after a minute
/// of silence; a datagram larger than a frame carries is dropped here.
async fn carry_udp(
    forward: Arc<Forward>,
    local: Arc<UdpSocket>,
    stop: impl std::future::Future<Output = ()>,
) {
    tokio::pin!(stop);
    let mut senders: HashMap<SocketAddr, mpsc::Sender<Bytes>> = HashMap::new();
    let mut buffer = vec![0u8; DATAGRAM_READ_BYTES];
    loop {
        let (read, from) = tokio::select! {
            () = &mut stop => return,
            received = local.recv_from(&mut buffer) => match received {
                Ok(received) => received,
                Err(_) => continue,
            },
        };
        if read > DATAGRAM_BYTES {
            continue;
        }
        let datagram = Bytes::copy_from_slice(&buffer[..read]);
        senders.retain(|_, queue| !queue.is_closed());
        if let Some(queue) = senders.get(&from) {
            let _ = queue.try_send(datagram);
            continue;
        }
        let (queue, waiting) = mpsc::channel(SENDER_QUEUE);
        let _ = queue.try_send(datagram);
        senders.insert(from, queue);
        let (forward, local) = (Arc::clone(&forward), Arc::clone(&local));
        tokio::spawn(async move {
            if let Err(error) = carry_sender(&forward, &local, from, waiting).await {
                eprintln!("a UDP sender was not carried: {error}");
            }
        });
    }
}

/// One local UDP sender's association: its datagrams out, the answers back to it.
async fn carry_sender(
    forward: &Forward,
    local: &UdpSocket,
    sender: SocketAddr,
    mut waiting: mpsc::Receiver<Bytes>,
) -> Result<(), ForwardError> {
    let taken = take_use(forward).await?;
    let socket = open_socket(forward, &taken).await?;
    let (mut sink, mut stream) = socket.split();
    let outbound = async {
        while let Ok(Some(datagram)) = tokio::time::timeout(UDP_IDLE, waiting.recv()).await {
            if sink.send(Message::binary(datagram)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    };
    let inbound = async {
        while let Some(Ok(message)) = stream.next().await {
            match message {
                Message::Binary(datagram) => {
                    let _ = local.send_to(&datagram, sender).await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    tokio::select! {
        () = outbound => {}
        () = inbound => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_names_the_installation_it_is_used_at() {
        let base =
            Forward::base_of_link("https://orch.example.com:8443/api/v1/ports/links/abc#proof")
                .unwrap();
        assert_eq!(base.as_str(), "https://orch.example.com:8443/");
        assert!(Forward::base_of_link("not a link").is_err());
    }

    #[tokio::test]
    async fn the_helper_listens_on_loopback_alone_and_refuses_an_occupied_port() {
        let refused = bind(Some("0.0.0.0:0".parse().unwrap()), PortTransport::Tcp).await;
        assert!(matches!(refused, Err(ForwardError::Address(_))));
        let first = bind(None, PortTransport::Tcp).await.unwrap();
        let taken = first.local_addr().unwrap();
        assert!(taken.ip().is_loopback() && taken.port() != 0);
        assert!(matches!(
            bind(Some(taken), PortTransport::Tcp).await,
            Err(ForwardError::Listen { .. })
        ));
    }

    #[test]
    fn a_credential_is_never_written_out() {
        let token = Credential::Token {
            token: "ort_secret".into(),
            environment: orch_types::new_id(),
            port: 8080,
        };
        assert!(!format!("{token:?}").contains("ort_secret"));
        let link = Credential::Link("https://orch.example.com/api/v1/ports/links/x#proof".into());
        assert!(!format!("{link:?}").contains("proof"));
    }
}
