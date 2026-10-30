//! API connection, recovery and heartbeat reporting.
//!
//! The gateway dials the system and keeps one connection to it, under mutual
//! TLS with its own certificate, and the system dials nobody. Over it the
//! system says what it is (the key tickets are checked with, the limits and
//! the connector builds a session is stated), the gateway asks whether a
//! session may start, tells which machines it holds, passes up what they say
//! and down what is said to them, and reports its figures every few seconds,
//! which is also how the system knows it is alive.
//!
//! The connection is lost whenever the system is restarted, and then the
//! gateway does what its design asks: it answers new sessions with `busy`,
//! stops taking events in so that machines keep what they have, goes on
//! relaying tunnels, tries again with growing pauses, and, when the system is
//! back, tells it about every session it holds.

use crate::Gateway;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use orch_link::reconnect::Backoff;
use orch_link_contract::backplane::{
    ApiHello, AuthQuery, AuthReply, BackplaneData, BackplaneFrame, GwHello, STATS_EVERY_SECS, Stats,
};
use orch_link_contract::frames::{self, Text};
use orch_link_contract::session::CONNECT_TIMEOUT_SECS;
use orch_pki::tickets::TicketVerifier;
use rustls::pki_types::ServerName;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// The most bytes of a message of the backplane. The same bound the link's
/// own sockets have: a frame of a machine is at most a session's limit, and
/// the wrapper around it is small.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
/// How long the system has to answer the gateway's first word.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// How many of the system's pings may go unheard before it is taken to be
/// gone.
const DEAD_AFTER_BEATS: u32 = 3;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

enum Outbound {
    Frame(Box<BackplaneFrame>),
    Data(Bytes),
    Datagram(IdPacket),
}

struct IdPacket {
    machine: orch_types::Id,
    packet: orch_link::ports::Datagram,
}

struct Shared {
    out: Mutex<Option<mpsc::UnboundedSender<Outbound>>>,
    pending: Mutex<HashMap<i64, oneshot::Sender<AuthReply>>>,
    next_question: AtomicI64,
    up: watch::Sender<bool>,
}

/// The way to the system: cheap to clone, and always there, whether or not the
/// connection is up.
#[derive(Clone)]
pub struct BackplaneHandle {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for BackplaneHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackplaneHandle")
            .field("up", &self.is_up())
            .finish_non_exhaustive()
    }
}

impl Default for BackplaneHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl BackplaneHandle {
    /// Emits a bounded unreplayed datagram on the existing backplane.
    pub fn send_port_datagram(
        &self,
        machine: orch_types::Id,
        packet: orch_link::ports::Datagram,
    ) -> bool {
        lock(&self.shared.out).as_ref().is_some_and(|out| {
            out.send(Outbound::Datagram(IdPacket { machine, packet }))
                .is_ok()
        })
    }
    /// A handle on a connection that is not up yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                out: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                next_question: AtomicI64::new(1),
                up: watch::channel(false).0,
            }),
        }
    }

    /// Whether the connection is up.
    #[must_use]
    pub fn is_up(&self) -> bool {
        lock(&self.shared.out).is_some()
    }

    /// Waits until the connection is up.
    pub async fn wait_up(&self) {
        let mut up = self.shared.up.subscribe();
        loop {
            if *up.borrow_and_update() {
                return;
            }
            if up.changed().await.is_err() {
                return;
            }
        }
    }

    /// Sends a frame to the system. Returns whether it was handed to a
    /// connection: while there is none the frame is not kept.
    pub fn send(&self, frame: BackplaneFrame) -> bool {
        lock(&self.shared.out)
            .as_ref()
            .is_some_and(|out| out.send(Outbound::Frame(Box::new(frame))).is_ok())
    }

    /// Sends tunnel data to the system.
    pub fn send_data(&self, data: &BackplaneData) -> bool {
        let Ok(bytes) = data.encode() else {
            return false;
        };
        lock(&self.shared.out)
            .as_ref()
            .is_some_and(|out| out.send(Outbound::Data(bytes)).is_ok())
    }

    /// Asks the system whether a session may start. `None` when the system
    /// cannot be asked: there is no connection, it was lost meanwhile, or the
    /// answer did not come in time. The question is numbered here, because
    /// the numbers the sessions bring are moments in milliseconds and two
    /// of them can be the same.
    pub async fn ask(&self, mut query: AuthQuery) -> Option<AuthReply> {
        let n = self.shared.next_question.fetch_add(1, Ordering::Relaxed);
        query.n = n;
        let (answer, waiting) = oneshot::channel();
        lock(&self.shared.pending).insert(n, answer);
        if !self.send(BackplaneFrame::AuthQuery(query)) {
            lock(&self.shared.pending).remove(&n);
            return None;
        }
        let timeout = Duration::from_secs(u64::from(
            orch_link_contract::backplane::AUTH_QUERY_TIMEOUT_SECS,
        ));
        match tokio::time::timeout(timeout, waiting).await {
            Ok(Ok(reply)) => Some(reply),
            _ => {
                lock(&self.shared.pending).remove(&n);
                None
            }
        }
    }

    fn answered(&self, reply: AuthReply) {
        if let Some(waiting) = lock(&self.shared.pending).remove(&reply.n) {
            let _ = waiting.send(reply);
        }
    }

    fn lost(&self) {
        *lock(&self.shared.out) = None;
        self.shared.up.send_replace(false);
        // Everyone who is waiting for an answer is told there will be none.
        lock(&self.shared.pending).clear();
    }
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

/// Why the connection to the system could not be made or kept.
#[derive(Debug, thiserror::Error)]
enum Fault {
    #[error("the address of the backplane is not usable: {0}")]
    Address(String),
    #[error("connecting: {0}")]
    Connect(String),
    #[error("the system said something unexpected: {0}")]
    Protocol(String),
}

async fn dial(gateway: &Gateway) -> Result<Socket, Fault> {
    let url = url::Url::parse(&gateway.inner.settings.backplane_url)
        .map_err(|error| Fault::Address(error.to_string()))?;
    let host = url
        .host_str()
        .ok_or_else(|| Fault::Address("it has no host".to_owned()))?
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| Fault::Address("it has no port".to_owned()))?;
    let name = ServerName::try_from(host.clone())
        .map_err(|_| Fault::Address(format!("`{host}` is not a name to verify")))?;
    let request = url
        .as_str()
        .into_client_request()
        .map_err(|error| Fault::Address(error.to_string()))?;
    let work = async {
        let socket = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|error| Fault::Connect(error.to_string()))?;
        socket
            .set_nodelay(true)
            .map_err(|error| Fault::Connect(error.to_string()))?;
        let tls = TlsConnector::from(Arc::clone(&gateway.inner.upstream_tls))
            .connect(name, socket)
            .await
            .map_err(|error| Fault::Connect(format!("TLS: {error}")))?;
        let (socket, _) =
            tokio_tungstenite::client_async_with_config(request, tls, Some(websocket_config()))
                .await
                .map_err(|error| Fault::Connect(format!("WebSocket: {error}")))?;
        Ok(socket)
    };
    tokio::time::timeout(Duration::from_secs(u64::from(CONNECT_TIMEOUT_SECS)), work)
        .await
        .map_err(|_| Fault::Connect("it took too long".to_owned()))?
}

fn text_of(frame: &BackplaneFrame) -> Option<Message> {
    frames::encode(frame, MAX_MESSAGE_BYTES)
        .ok()
        .map(Message::text)
}

/// Reads the system's first word and takes in what it says.
async fn greet(gateway: &Gateway, socket: &mut Socket) -> Result<(), Fault> {
    let hello = BackplaneFrame::GwHello(GwHello {
        gateway_id: gateway.inner.settings.gateway_id.clone(),
        version: gateway.inner.settings.version.clone(),
        started_at: Some(gateway.inner.started_at),
    });
    let message =
        text_of(&hello).ok_or_else(|| Fault::Protocol("hello cannot be written".to_owned()))?;
    socket
        .send(message)
        .await
        .map_err(|error| Fault::Connect(error.to_string()))?;
    let answer = tokio::time::timeout(HELLO_TIMEOUT, async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    return match frames::decode::<BackplaneFrame>(text.as_str(), MAX_MESSAGE_BYTES)
                    {
                        Ok(Text::Known(BackplaneFrame::ApiHello(hello))) => Ok(hello),
                        _ => Err(Fault::Protocol(
                            "the system did not begin with its hello".to_owned(),
                        )),
                    };
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => {
                    return Err(Fault::Connect(
                        "the system closed the connection".to_owned(),
                    ));
                }
            }
        }
    })
    .await
    .map_err(|_| Fault::Protocol("the system did not answer".to_owned()))??;
    take_in(gateway, answer);
    Ok(())
}

/// Takes in what the system says about itself.
fn take_in(gateway: &Gateway, hello: ApiHello) {
    match TicketVerifier::from_text(&hello.ticket_public_key) {
        Ok(verifier) => gateway
            .inner
            .relay
            .with(|relay| relay.set_verifier(verifier)),
        Err(error) => tracing::error!(%error, "the key tickets are checked with cannot be read"),
    }
    let mut api = gateway
        .inner
        .api
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    api.limits = hello.limits;
    api.versions = hello.versions;
}

/// Keeps the connection to the system for as long as the gateway runs.
pub(crate) async fn keep(gateway: Gateway) {
    let mut backoff = Backoff::default();
    loop {
        if gateway.inner.shutdown.is_cancelled() {
            return;
        }
        let held = hold(&gateway).await;
        let pause = match held {
            Ok(()) => {
                backoff.succeeded();
                backoff.failed(None)
            }
            Err(fault) => {
                tracing::warn!(%fault, "the system cannot be reached");
                backoff.failed(None)
            }
        };
        tokio::select! {
            () = gateway.inner.shutdown.cancelled() => return,
            () = tokio::time::sleep(pause) => {}
        }
    }
}

/// One connection to the system, from the dial to its end.
async fn hold(gateway: &Gateway) -> Result<(), Fault> {
    let mut socket = dial(gateway).await?;
    greet(gateway, &mut socket).await?;
    tracing::info!("the backplane is up");
    let (mut sink, mut source) = socket.split();
    let (out, mut queued) = mpsc::unbounded_channel();
    {
        // Every session that stands is told to the system, and the way to it
        // is opened, as one step: a session that begins meanwhile is either in
        // the first or tells itself.
        let handle = &gateway.inner.backplane;
        let mut slot = lock(&handle.shared.out);
        for up in gateway.inner.sessions.announcements() {
            let _ = out.send(Outbound::Frame(Box::new(BackplaneFrame::MachineUp(up))));
        }
        *slot = Some(out.clone());
        handle.shared.up.send_replace(true);
    }
    let mut stats = tokio::time::interval(Duration::from_secs(u64::from(STATS_EVERY_SECS)));
    gateway.inner.relay.with(|relay| relay.port_control(true));
    stats.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let dead_after = Duration::from_secs(u64::from(STATS_EVERY_SECS)) * DEAD_AFTER_BEATS;
    let ended: Result<(), Fault> = loop {
        tokio::select! {
            () = gateway.inner.shutdown.cancelled() => break Ok(()),
            message = tokio::time::timeout(dead_after, source.next()) => match message {
                Err(_) => break Err(Fault::Connect("the system has been silent for too long".to_owned())),
                Ok(None | Some(Err(_) | Ok(Message::Close(_)))) => {
                    break Err(Fault::Connect("the system closed the connection".to_owned()));
                }
                Ok(Some(Ok(message))) => on_message(gateway, message),
            },
            outgoing = queued.recv() => {
                let Some(outgoing) = outgoing else { break Ok(()) };
                let message = match outgoing {
                    Outbound::Frame(frame) => match text_of(&frame) {
                        Some(message) => message,
                        None => continue,
                    },
                    Outbound::Data(bytes) => Message::Binary(bytes),
                    Outbound::Datagram(packet) => {
                        let data=orch_link_contract::ports::BackplanePortDatagram {machine_id:packet.machine,datagram:packet.packet.frame.clone()};
                        let Ok(bytes)=data.encode() else {continue;};
                        if sink.send(Message::Binary(bytes)).await.is_err() {break Err(Fault::Connect("the connection failed".into()));}
                        packet.packet.delivered();
                        continue;
                    },
                };
                if sink.send(message).await.is_err() {
                    break Err(Fault::Connect("the connection failed".to_owned()));
                }
            }
            _ = stats.tick() => {
                gateway.inner.sessions.sample_round_trips();
                for (kind,count) in gateway.inner.relay.with(|relay|relay.measured_routes()) {
                    metrics::gauge!("orch_tunnels_open","kind"=>kind).set(count as f64);
                }
                let figures = Stats {
                    sessions: i64::try_from(gateway.inner.sessions.len()).unwrap_or(i64::MAX),
                    channels: i64::try_from(gateway.inner.relay.with(|relay| relay.routes()))
                        .unwrap_or(i64::MAX),
                    bytes_relayed: i64::try_from(gateway.bytes_relayed()).unwrap_or(i64::MAX),
                    metrics: orch_telemetry::metrics::take_process_samples(),
                };
                gateway.inner.backplane.send(BackplaneFrame::Stats(figures));
            }
        }
    };
    gateway.inner.backplane.lost();
    gateway.inner.relay.with(|relay| relay.port_control(false));
    let _ = sink.close().await;
    tracing::info!("the backplane is down");
    ended
}

fn on_message(gateway: &Gateway, message: Message) {
    match message {
        Message::Text(text) => {
            match frames::decode::<BackplaneFrame>(text.as_str(), MAX_MESSAGE_BYTES) {
                Ok(Text::Known(frame)) => on_frame(gateway, frame),
                Ok(Text::Unknown(kind)) => {
                    tracing::debug!(kind, "a frame of an unknown type was passed over");
                }
                Err(error) => tracing::warn!(%error, "the system sent a frame that is not one"),
            }
        }
        Message::Binary(bytes)
            if bytes.first() == Some(&orch_link_contract::ports::PortDatagram::KIND) =>
        {
            if let Ok(data) = orch_link_contract::ports::BackplanePortDatagram::decode(bytes) {
                gateway
                    .inner
                    .relay
                    .with(|relay| relay.on_api_datagram(data));
            }
        }
        Message::Binary(bytes) => match BackplaneData::decode(bytes) {
            Ok(data) => {
                gateway
                    .inner
                    .bytes_relayed
                    .fetch_add(data.payload.len() as u64, Ordering::Relaxed);
                gateway.inner.relay.with(|relay| relay.on_api_data(data));
            }
            Err(error) => tracing::warn!(%error, "the system sent data that is not data"),
        },
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) | Message::Close(_) => {}
    }
}

fn on_frame(gateway: &Gateway, frame: BackplaneFrame) {
    match frame {
        BackplaneFrame::AuthReply(reply) => gateway.inner.backplane.answered(reply),
        BackplaneFrame::Down(down) => crate::routing::to_machine(gateway, down),
        BackplaneFrame::Kick(kick) => gateway.inner.sessions.kick(kick.machine_id, kick.code),
        BackplaneFrame::PortConnectionsQuery(query) => {
            let now = gateway.inner.clock.time();
            let connections = gateway.inner.relay.with(|relay| {
                relay.port_records().list(
                    query.environment_id,
                    query.generation,
                    &query.query,
                    true,
                    now,
                )
            });
            gateway
                .inner
                .backplane
                .send(BackplaneFrame::PortConnectionsReply(
                    orch_link_contract::ports::PortConnectionsReply {
                        request_ref: query.request_ref,
                        environment_id: query.environment_id,
                        connections,
                    },
                ));
        }
        frame @ (BackplaneFrame::TunOpen(_)
        | BackplaneFrame::TunCredit(_)
        | BackplaneFrame::TunEof(_)
        | BackplaneFrame::TunClose(_)
        | BackplaneFrame::TunText(_)) => {
            gateway.inner.relay.with(|relay| relay.on_api_frame(frame));
        }
        // The system's hello comes once, and the rest is for the system.
        _ => {}
    }
}
