//! The session over a real socket: WebSocket frames on TLS, on either end.
//!
//! The mechanisms of the link know only [`Connection`]. This module makes
//! one from a WebSocket: for a machine, by dialling an endpoint and
//! checking the gateway under the trust path the endpoint names; for a
//! gateway, by taking a socket that was accepted, finishing its TLS
//! handshake, and upgrading it. What the TLS handshake found out about the
//! other end (the certificate a machine showed, the host it asked for) is
//! carried along in [`PeerInfo`], so that the session handshake can use it.
//!
//! The writer of a session chooses what to send next only when the previous
//! message has been taken, but a socket takes a good deal before it blocks: on
//! a thin line, control frames can wait behind data that the operating system
//! already holds. A deployment that cares about that sets a low
//! `TCP_NOTSENT_LOWAT` on its sockets, about a quarter of a second of the
//! line's rate; the sizes of data frames and the priority of the queues do the
//! rest.

use crate::framing::{Connection, Message, MessageSink, MessageSource, PeerInfo, TransportError};
use crate::outward::OutwardProxy;
use crate::reconnect::{DialError, Dialer};
use async_trait::async_trait;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use orch_link_contract::enrollment::{Endpoint, Trust};
use orch_link_contract::session::CONNECT_TIMEOUT_SECS;
use orch_link_contract::version::{SESSION_PATH, SUBPROTOCOL};
use orch_pki::verification::presented_certificate;
use rustls::pki_types::ServerName;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message as WsMessage};

/// The header that names the subprotocol a WebSocket speaks.
const PROTOCOL_HEADER: &str = "Sec-WebSocket-Protocol";
/// The most a single message may have on a real socket, whatever a session
/// agrees later: the handshake is read before limits are agreed, and no
/// frame of the protocol is larger than the largest limit a gateway sends.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

fn transport(error: &WsError) -> TransportError {
    match error {
        WsError::ConnectionClosed | WsError::AlreadyClosed => TransportError::Closed,
        other => TransportError::Io(other.to_string()),
    }
}

/// The sending half of a WebSocket.
struct WsSink<S>(SplitSink<WebSocketStream<S>, WsMessage>);

#[async_trait]
impl<S> MessageSink for WsSink<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn send(&mut self, message: Message) -> Result<(), TransportError> {
        let message = match message {
            Message::Text(text) => WsMessage::text(text),
            Message::Binary(bytes) => WsMessage::binary(bytes),
        };
        self.0.send(message).await.map_err(|e| transport(&e))
    }

    async fn close(&mut self) {
        let _ = self.0.close().await;
    }
}

/// The receiving half of a WebSocket.
struct WsSource<S>(SplitStream<WebSocketStream<S>>);

#[async_trait]
impl<S> MessageSource for WsSource<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn recv(&mut self) -> Result<Option<Message>, TransportError> {
        loop {
            match self.0.next().await {
                None | Some(Ok(WsMessage::Close(_))) => return Ok(None),
                Some(Ok(WsMessage::Text(text))) => {
                    return Ok(Some(Message::Text(text.as_str().to_owned())));
                }
                Some(Ok(WsMessage::Binary(bytes))) => return Ok(Some(Message::Binary(bytes))),
                // The library answers pings by itself.
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
                Some(Err(error)) => return Err(transport(&error)),
            }
        }
    }
}

/// A connection made of a WebSocket.
pub fn connection_of<S>(stream: WebSocketStream<S>, peer: PeerInfo) -> Connection
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, source) = stream.split();
    Connection {
        sink: Box::new(WsSink(sink)),
        source: Box::new(WsSource(source)),
        peer,
    }
}

/// The host of a `Host` header, without a port, in lowercase.
fn host_of(header: &str) -> String {
    let without_port = match header.strip_prefix('[') {
        // An IPv6 literal: keep what is inside the brackets.
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        None => header.split(':').next().unwrap_or(header),
    };
    without_port.to_ascii_lowercase()
}

/// Upgrades a socket, TLS or not, that a gateway accepted, and checks that
/// the machine asked for the session path and speaks the subprotocol of this
/// protocol. `peer` is what the TLS handshake found out; the host the
/// machine asked for is added to it.
pub async fn accept<S>(stream: S, mut peer: PeerInfo) -> Result<Connection, TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let asked_host: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&asked_host);
    #[expect(
        clippy::result_large_err,
        reason = "the refusal is the library's own type, built once per connection that is refused"
    )]
    let callback =
        move |request: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
            let refuse = |status: StatusCode, why: &str| {
                let mut refusal = ErrorResponse::new(Some(why.to_owned()));
                *refusal.status_mut() = status;
                refusal
            };
            if request.uri().path() != SESSION_PATH {
                return Err(refuse(StatusCode::NOT_FOUND, "no such path"));
            }
            let speaks = request
                .headers()
                .get_all(PROTOCOL_HEADER)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .flat_map(|value| value.split(','))
                .any(|protocol| protocol.trim() == SUBPROTOCOL);
            if !speaks {
                return Err(refuse(
                    StatusCode::BAD_REQUEST,
                    "the subprotocol orch-link.v1 is required",
                ));
            }
            response
                .headers_mut()
                .insert(PROTOCOL_HEADER, HeaderValue::from_static(SUBPROTOCOL));
            if let Some(host) = request
                .headers()
                .get("host")
                .and_then(|value| value.to_str().ok())
            {
                *seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host_of(host));
            }
            Ok(response)
        };
    let websocket =
        tokio_tungstenite::accept_hdr_async_with_config(stream, callback, Some(config()))
            .await
            .map_err(|e| transport(&e))?;
    if let Some(host) = asked_host
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        peer.host = Some(host);
    }
    Ok(connection_of(websocket, peer))
}

/// Finishes the TLS handshake of a socket a gateway accepted, and upgrades
/// it. What the handshake found out about the machine goes into the
/// connection: the address, and the certificate it showed.
pub async fn accept_tls(
    acceptor: &TlsAcceptor,
    socket: TcpStream,
) -> Result<Connection, TransportError> {
    let remote_ip: Option<IpAddr> = socket.peer_addr().ok().map(|address| address.ip());
    // Small frames (acknowledgements, pongs, credit) must not wait to be joined
    // by others: the dialling side does the same.
    let _ = socket.set_nodelay(true);
    let stream = acceptor
        .accept(socket)
        .await
        .map_err(|e| TransportError::Io(format!("TLS: {e}")))?;
    let client_certificate = presented_certificate(stream.get_ref().1.peer_certificates());
    let sni = stream
        .get_ref()
        .1
        .server_name()
        .map(str::to_ascii_lowercase);
    accept(
        stream,
        PeerInfo {
            remote_ip,
            host: sni,
            client_certificate,
        },
    )
    .await
}

/// Dials endpoints over TCP and TLS and upgrades them to a session socket.
#[derive(Clone)]
pub struct TlsDialer {
    pinned: Option<Arc<rustls::ClientConfig>>,
    public: Option<Arc<rustls::ClientConfig>>,
    connect_timeout: Duration,
    proxy: Option<OutwardProxy>,
}

impl std::fmt::Debug for TlsDialer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsDialer").finish_non_exhaustive()
    }
}

impl TlsDialer {
    /// A dialer with the TLS configuration of each trust path it may meet:
    /// `pinned` for endpoints under the system's authority, `public` for
    /// those behind a public edge.
    #[must_use]
    pub fn new(
        pinned: Option<Arc<rustls::ClientConfig>>,
        public: Option<Arc<rustls::ClientConfig>>,
    ) -> Self {
        Self {
            pinned,
            public,
            connect_timeout: Duration::from_secs(u64::from(CONNECT_TIMEOUT_SECS)),
            proxy: None,
        }
    }

    /// The same dialer with another limit on how long connecting may take.
    #[must_use]
    pub const fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// The same dialer reaching endpoints through `proxy` (a tunnel per connection, the TLS inside it the
    /// dialer's own), where the proxy applies to the endpoint's host.
    #[must_use]
    pub fn with_proxy(mut self, proxy: Option<OutwardProxy>) -> Self {
        self.proxy = proxy;
        self
    }

    async fn connect(&self, endpoint: &Endpoint) -> Result<Connection, DialError> {
        let url = url::Url::parse(
            &endpoint
                .session_url()
                .ok_or_else(|| DialError::Address(endpoint.url.clone()))?,
        )
        .map_err(|e| DialError::Address(e.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| DialError::Address(endpoint.url.clone()))?
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| DialError::Address(endpoint.url.clone()))?;
        let tls = match endpoint.trust {
            Trust::Pinned => self.pinned.clone(),
            Trust::Public => self.public.clone(),
        }
        .ok_or(DialError::NoTrust(endpoint.trust))?;
        let socket = crate::outward::connect(&host, port, true, self.proxy.as_ref())
            .await
            .map_err(|e| DialError::Connect(e.to_string()))?;
        socket
            .set_nodelay(true)
            .map_err(|e| DialError::Connect(e.to_string()))?;
        let name = ServerName::try_from(host.clone())
            .map_err(|_| DialError::Address(format!("`{host}` is not a name to verify")))?;
        let stream = TlsConnector::from(tls)
            .connect(name, socket)
            .await
            .map_err(|e| DialError::Tls(e.to_string()))?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| DialError::Address(e.to_string()))?;
        request
            .headers_mut()
            .insert(PROTOCOL_HEADER, HeaderValue::from_static(SUBPROTOCOL));
        let (websocket, response) =
            tokio_tungstenite::client_async_with_config(request, stream, Some(config()))
                .await
                .map_err(|e| DialError::Upgrade(e.to_string()))?;
        let agreed = response
            .headers()
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok())
            == Some(SUBPROTOCOL);
        if !agreed {
            return Err(DialError::Upgrade(
                "the gateway does not speak orch-link.v1".to_owned(),
            ));
        }
        Ok(connection_of(
            websocket,
            PeerInfo {
                host: Some(host.to_ascii_lowercase()),
                ..PeerInfo::default()
            },
        ))
    }
}

#[async_trait]
impl Dialer for TlsDialer {
    async fn dial(&self, endpoint: &Endpoint) -> Result<Connection, DialError> {
        tokio::time::timeout(self.connect_timeout, self.connect(endpoint))
            .await
            .map_err(|_| DialError::TimedOut)?
    }
}
