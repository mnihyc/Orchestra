//! Control and binary framing.
//!
//! A session runs over anything that carries whole messages in order in both
//! directions, which is what a WebSocket does. This module says what that is
//! ([`Connection`]: a half that sends, a half that receives, and what is
//! known of the other end), reads frames off it and writes frames onto it.
//!
//! **Data does not hold up control.** What is to be sent waits in three
//! queues, and the writer always takes from the first that has something:
//! control frames (acknowledgements, results, credit, pings), then events,
//! then tunnel data. Data frames are sized to what the link has been seen to
//! carry, about a quarter of a second of it, so that a control frame waits
//! for at most one small data frame and never for a long one.
//!
//! The link is measured as it is used: the round trip from pings, and the
//! rate in each direction from what moved. The figures size the data frames
//! and are reported as facts of the machine.

use crate::ports::{Datagram, UdpBudget};
use async_trait::async_trait;
use bytes::Bytes;
use orch_link_contract::frames::{self, DataFrame, Frame, FrameError, Text};
use orch_link_contract::ports::PortDatagram;
use orch_link_contract::session::MachineFactsRates;
use orch_link_contract::tunnels::data_frame_bytes;
use orch_pki::certificates::Certificate;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// How long the figures of the link are looked back over, in seconds: long
/// enough to see a burst through, short enough to follow a link that slowed
/// down.
const METER_WINDOW_SECS: u64 = 10;
/// The size of a data frame before anything has been seen of the link: the
/// least a frame carries. A thin line is not held up by it for more than
/// a moment, a fast one is seen to be fast within the first second, and
/// from then on frames are as large as the line allows.
pub const FIRST_DATA_FRAME_BYTES: usize = orch_link_contract::tunnels::MIN_DATA_FRAME_BYTES;
/// The weight of the newest round trip in the average: a round trip is a
/// noisy figure, and one slow ping is not a slow link.
const RTT_WEIGHT: f64 = 0.25;

/// Why a message could not be sent or received.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// The connection is closed.
    #[error("the connection is closed")]
    Closed,
    /// The transport failed.
    #[error("{0}")]
    Io(String),
}

/// One message of a connection: text or bytes, as a WebSocket has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// A text frame: one JSON control message.
    Text(String),
    /// A binary frame: tunnel data.
    Binary(Bytes),
}

impl Message {
    /// How many bytes it takes on the link, apart from the framing of the
    /// transport.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Binary(bytes) => bytes.len(),
        }
    }

    /// Whether it carries nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The half of a connection that sends.
#[async_trait]
pub trait MessageSink: Send {
    /// Returns when the transport can take a message without making the
    /// sender wait. The writer asks before it chooses what to send next, so
    /// that a message of a higher priority that comes while the transport is
    /// busy goes before a lower one that was waiting earlier: what has
    /// already been handed to the transport cannot be overtaken, so as little
    /// as possible is handed over ahead of time. A transport that cannot say
    /// leaves this as it is, and is always ready.
    async fn ready(&mut self) {}

    /// Sends one message, returning when the transport has taken it.
    async fn send(&mut self, message: Message) -> Result<(), TransportError>;

    /// Ends the connection politely.
    async fn close(&mut self);
}

/// The half of a connection that receives.
#[async_trait]
pub trait MessageSource: Send {
    /// The next message, `None` when the other end closed the connection.
    async fn recv(&mut self) -> Result<Option<Message>, TransportError>;
}

/// What is known of the other end of a connection before a word is said.
#[derive(Debug, Clone, Default)]
pub struct PeerInfo {
    /// The address it connected from; on the machine's side, `None`.
    pub remote_ip: Option<IpAddr>,
    /// The host name the machine connected to, without a port: the
    /// gateway's reading from the request, the machine's own from the
    /// address it dialled.
    pub host: Option<String>,
    /// The certificate the other end showed in the TLS handshake, where
    /// there was one and the handshake accepted it.
    pub client_certificate: Option<Certificate>,
}

/// A connection that carries whole messages, in two halves.
pub struct Connection {
    /// The half that sends.
    pub sink: Box<dyn MessageSink>,
    /// The half that receives.
    pub source: Box<dyn MessageSource>,
    /// What is known of the other end.
    pub peer: PeerInfo,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

struct MemorySink(Option<mpsc::UnboundedSender<Message>>);

#[async_trait]
impl MessageSink for MemorySink {
    async fn send(&mut self, message: Message) -> Result<(), TransportError> {
        self.0
            .as_ref()
            .ok_or(TransportError::Closed)?
            .send(message)
            .map_err(|_| TransportError::Closed)
    }

    async fn close(&mut self) {
        self.0 = None;
    }
}

struct MemorySource(mpsc::UnboundedReceiver<Message>);

#[async_trait]
impl MessageSource for MemorySource {
    async fn recv(&mut self) -> Result<Option<Message>, TransportError> {
        Ok(self.0.recv().await)
    }
}

/// Two connections joined in memory: what one sends the other receives. Each
/// is given what it knows of the other end.
#[must_use]
pub fn memory_pair(first_sees: PeerInfo, second_sees: PeerInfo) -> (Connection, Connection) {
    let (to_second, from_first) = mpsc::unbounded_channel();
    let (to_first, from_second) = mpsc::unbounded_channel();
    (
        Connection {
            sink: Box::new(MemorySink(Some(to_second))),
            source: Box::new(MemorySource(from_second)),
            peer: first_sees,
        },
        Connection {
            sink: Box::new(MemorySink(Some(to_first))),
            source: Box::new(MemorySource(from_first)),
            peer: second_sees,
        },
    )
}

/// How many seconds the busiest stretch is that a rate is read from: long
/// enough that a frame landing on either side of a boundary is a small share
/// of it, short enough to follow a line that changes.
const RATE_SECONDS: u64 = 4;

/// What moved, second by second.
#[derive(Debug, Default)]
struct RateWindow {
    started: Option<Instant>,
    /// For each second in which bytes moved: its number from the first, and how
    /// many bytes.
    seconds: VecDeque<(u64, u64)>,
}

impl RateWindow {
    fn observe(&mut self, bytes: usize, now: Instant) {
        let started = *self.started.get_or_insert(now);
        let second = now.duration_since(started).as_secs();
        match self.seconds.back_mut() {
            Some((last, total)) if *last == second => *total += bytes as u64,
            _ => self.seconds.push_back((second, bytes as u64)),
        }
        while self
            .seconds
            .front()
            .is_some_and(|(first, _)| second.saturating_sub(*first) > METER_WINDOW_SECS)
        {
            self.seconds.pop_front();
        }
    }

    /// Bytes per second, as the busiest stretch of [`RATE_SECONDS`] seconds
    /// that is over carried them. The second still going is left out, because
    /// it says less than it will; a link that has gone quiet keeps the figure
    /// it had.
    fn rate(&self, now: Instant) -> Option<u64> {
        let started = self.started?;
        let current = now.duration_since(started).as_secs();
        let finished: Vec<(u64, u64)> = self
            .seconds
            .iter()
            .copied()
            .filter(|(second, _)| *second < current)
            .collect();
        finished
            .iter()
            .map(|(last, _)| {
                finished
                    .iter()
                    .filter(|(second, _)| *second + RATE_SECONDS > *last && *second <= *last)
                    .map(|(_, bytes)| *bytes)
                    .sum::<u64>()
                    / RATE_SECONDS
            })
            .max()
    }
}

#[derive(Debug, Default)]
struct MeterState {
    rtt: Option<f64>,
    up: RateWindow,
    down: RateWindow,
}

/// What is seen of a link: its round trip and the rate in each direction.
#[derive(Debug, Default)]
pub struct LinkMeter {
    state: Mutex<MeterState>,
}

/// The figures of a link, where there are any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Measurements {
    /// The round trip in milliseconds.
    pub round_trip_ms: Option<i64>,
    /// Bytes per second sent.
    pub up_bps: Option<i64>,
    /// Bytes per second received.
    pub down_bps: Option<i64>,
}

impl Measurements {
    /// The figures as a machine reports them in its facts, zero where
    /// nothing has been seen yet.
    #[must_use]
    pub fn as_facts(&self) -> MachineFactsRates {
        MachineFactsRates {
            up_bps: self.up_bps.unwrap_or(0).saturating_mul(BITS_PER_BYTE),
            down_bps: self.down_bps.unwrap_or(0).saturating_mul(BITS_PER_BYTE),
            round_trip_ms: self.round_trip_ms.unwrap_or(0),
        }
    }
}

/// Facts state a rate in bits a second, as links are spoken of.
const BITS_PER_BYTE: i64 = 8;

impl LinkMeter {
    /// A meter that has seen nothing.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MeterState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Takes the round trip of a ping.
    pub fn observe_round_trip(&self, round_trip: Duration) {
        let millis = round_trip.as_secs_f64() * 1_000.0;
        let mut state = self.lock();
        state.rtt = Some(match state.rtt {
            Some(before) => before + RTT_WEIGHT * (millis - before),
            None => millis,
        });
    }

    /// Takes bytes that the transport accepted for sending.
    pub fn observe_sent(&self, bytes: usize, now: Instant) {
        self.lock().up.observe(bytes, now);
    }

    /// Takes bytes that arrived.
    pub fn observe_received(&self, bytes: usize, now: Instant) {
        self.lock().down.observe(bytes, now);
    }

    /// What has been seen.
    #[must_use]
    pub fn measurements(&self) -> Measurements {
        let state = self.lock();
        // Rounded to whole units: a figure is a fact, not an instrument reading.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a round trip in milliseconds is far below i64::MAX"
        )]
        let round_trip_ms = state.rtt.map(|rtt| rtt.round() as i64);
        let now = Instant::now();
        Measurements {
            round_trip_ms,
            up_bps: state
                .up
                .rate(now)
                .map(|rate| i64::try_from(rate).unwrap_or(i64::MAX)),
            down_bps: state
                .down
                .rate(now)
                .map(|rate| i64::try_from(rate).unwrap_or(i64::MAX)),
        }
    }

    /// How many bytes of payload to put in the next data frame, which may
    /// carry `max_payload` at most.
    #[must_use]
    pub fn data_frame_bytes(&self, max_payload: usize) -> usize {
        match self.lock().up.rate(Instant::now()) {
            Some(rate) => data_frame_bytes(rate, max_payload),
            None => FIRST_DATA_FRAME_BYTES.min(max_payload),
        }
    }
}

/// What the writer is given, by urgency.
#[derive(Debug, Clone)]
pub struct Outgoing {
    control: mpsc::UnboundedSender<Message>,
    events: mpsc::UnboundedSender<Message>,
    data: mpsc::UnboundedSender<Message>,
    datagrams: mpsc::UnboundedSender<Datagram>,
}

/// The queues the writer takes from.
#[derive(Debug)]
pub struct Queues {
    control: mpsc::UnboundedReceiver<Message>,
    events: mpsc::UnboundedReceiver<Message>,
    data: mpsc::UnboundedReceiver<Message>,
    datagrams: mpsc::UnboundedReceiver<Datagram>,
}

/// A set of three queues, for a writer.
#[must_use]
pub fn queues() -> (Outgoing, Queues) {
    let (control_tx, control) = mpsc::unbounded_channel();
    let (events_tx, events) = mpsc::unbounded_channel();
    let (data_tx, data) = mpsc::unbounded_channel();
    let (datagram_tx, datagrams) = mpsc::unbounded_channel();
    (
        Outgoing {
            control: control_tx,
            events: events_tx,
            data: data_tx,
            datagrams: datagram_tx,
        },
        Queues {
            control,
            events,
            data,
            datagrams,
        },
    )
}

/// The queue is closed: the session it belonged to is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the session is over")]
pub struct SessionOver;

impl Outgoing {
    /// Queues a leased UDP packet without waiting or consuming stream credit.
    pub fn datagram(&self, packet: Datagram) -> Result<(), SessionOver> {
        self.datagrams.send(packet).map_err(|_| SessionOver)
    }
    /// Queues a control frame: it goes before anything else.
    pub fn control(&self, frame: &Frame, max_frame_bytes: usize) -> Result<(), SessionOver> {
        self.queue(&self.control, frame, max_frame_bytes)
    }

    /// Queues a frame of events.
    pub fn events(&self, frame: &Frame, max_frame_bytes: usize) -> Result<(), SessionOver> {
        self.queue(&self.events, frame, max_frame_bytes)
    }

    /// Queues a frame of tunnel data.
    pub fn data(&self, frame: &DataFrame, max_frame_bytes: usize) -> Result<(), SessionOver> {
        let bytes = frame.encode(max_frame_bytes).map_err(|_| SessionOver)?;
        self.data
            .send(Message::Binary(bytes))
            .map_err(|_| SessionOver)
    }

    /// Queues a text frame that has to arrive in order with the data before
    /// it: the end of a stream, a clean close.
    pub fn ordered(&self, frame: &Frame, max_frame_bytes: usize) -> Result<(), SessionOver> {
        self.queue(&self.data, frame, max_frame_bytes)
    }

    /// Queues a binary frame that is already written.
    pub fn data_bytes(&self, bytes: Bytes) -> Result<(), SessionOver> {
        self.data
            .send(Message::Binary(bytes))
            .map_err(|_| SessionOver)
    }

    /// Queues text that is already written, as a control frame.
    pub fn control_text(&self, text: String) -> Result<(), SessionOver> {
        self.control
            .send(Message::Text(text))
            .map_err(|_| SessionOver)
    }

    fn queue(
        &self,
        queue: &mpsc::UnboundedSender<Message>,
        frame: &Frame,
        max_frame_bytes: usize,
    ) -> Result<(), SessionOver> {
        let text = frames::encode(frame, max_frame_bytes).map_err(|error| {
            tracing::error!(%error, "a frame was not sent");
            SessionOver
        })?;
        queue.send(Message::Text(text)).map_err(|_| SessionOver)
    }

    /// Whether the writer is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.control.is_closed()
    }
}

/// Writes what is queued until every sender is gone and the queues are
/// empty, the session is ended, or the transport fails; then closes the
/// transport. Control goes before events and events before data, whenever
/// there is a choice. When the session is ended, what is still waiting as
/// control (a `bye` among it) is written first.
pub async fn write_loop(
    mut sink: Box<dyn MessageSink>,
    mut queues: Queues,
    meter: Arc<LinkMeter>,
    ended: CancellationToken,
) -> Result<(), TransportError> {
    let mut result = Ok(());
    let mut udp_next = false;
    'writing: loop {
        // The next message is chosen only when the transport can take it: a
        // command that arrives while a data frame is on the wire goes next,
        // not after the data frame that was chosen before it came.
        tokio::select! {
            biased;
            () = ended.cancelled() => break,
            () = sink.ready() => {}
        }
        // Alternate ready TCP and UDP after control/events; neither data kind
        // can starve the other or hold a control frame behind its full store.
        let (message, packet) = if udp_next {
            tokio::select! {
                biased;
                Some(message) = queues.control.recv() => (message,None),
                () = ended.cancelled() => break,
                Some(message) = queues.events.recv() => (message,None),
                Some(packet) = queues.datagrams.recv() => match packet.frame.encode() {Ok(bytes)=>(Message::Binary(bytes),Some(packet)),Err(_)=>continue},
                Some(message) = queues.data.recv() => (message,None),
                else => break,
            }
        } else {
            tokio::select! {
                biased;
                Some(message) = queues.control.recv() => (message,None),
                () = ended.cancelled() => break,
                Some(message) = queues.events.recv() => (message,None),
                Some(message) = queues.data.recv() => (message,None),
                Some(packet) = queues.datagrams.recv() => match packet.frame.encode() {Ok(bytes)=>(Message::Binary(bytes),Some(packet)),Err(_)=>continue},
                else => break,
            }
        };
        udp_next = packet.is_none();
        let bytes = message.len();
        if let Err(error) = sink.send(message).await {
            result = Err(error);
            break 'writing;
        }
        if let Some(packet) = packet {
            packet.delivered();
        }
        meter.observe_sent(bytes, Instant::now());
    }
    if result.is_ok() {
        while let Ok(message) = queues.control.try_recv() {
            if let Err(error) = sink.send(message).await {
                result = Err(error);
                break;
            }
        }
    }
    sink.close().await;
    result
}

/// What came off the connection.
#[derive(Debug)]
pub enum Inbound {
    /// A text frame of a known type.
    Frame(Box<Frame>),
    /// A text frame of a type this build does not know, which is passed
    /// over.
    Unknown(String),
    /// A frame of tunnel data.
    Data(DataFrame),
    /// A complete leased port datagram, with no byte-stream offset.
    Datagram(Datagram),
    /// The other end broke the protocol; the session is ended.
    Broken(FrameError),
    /// The connection ended. `None` is the other end closing it.
    Lost(Option<TransportError>),
}

/// Reads frames until the connection ends or the other end breaks the
/// protocol, and passes each on.
pub async fn read_loop(
    mut source: Box<dyn MessageSource>,
    inbound: mpsc::UnboundedSender<Inbound>,
    max_frame_bytes: usize,
    ports: orch_link_contract::version::PortLimits,
    meter: Arc<LinkMeter>,
) {
    // This additional session boundary is conservative for a machine with
    // several environments; no unbounded inbox can precede their own stores.
    let udp = Arc::new(UdpBudget::for_limits(&ports));
    loop {
        let message = match source.recv().await {
            Ok(Some(message)) => message,
            Ok(None) => {
                let _ = inbound.send(Inbound::Lost(None));
                return;
            }
            Err(error) => {
                let _ = inbound.send(Inbound::Lost(Some(error)));
                return;
            }
        };
        meter.observe_received(message.len(), Instant::now());
        let item = match message {
            Message::Text(text) => match frames::decode::<Frame>(&text, max_frame_bytes) {
                Ok(Text::Known(frame)) => Inbound::Frame(Box::new(frame)),
                Ok(Text::Unknown(kind)) => Inbound::Unknown(kind),
                Err(error) => Inbound::Broken(error),
            },
            Message::Binary(bytes) if bytes.first() == Some(&PortDatagram::KIND) => {
                match PortDatagram::decode(bytes) {
                    Ok(frame) => match udp.hold(frame) {
                        Some(packet) => Inbound::Datagram(packet),
                        None => continue,
                    },
                    Err(FrameError::TooLarge { .. }) => {
                        udp.oversized_dropped();
                        continue;
                    }
                    Err(error) => Inbound::Broken(error),
                }
            }
            Message::Binary(bytes) => match DataFrame::decode(bytes, max_frame_bytes) {
                Ok(frame) => Inbound::Data(frame),
                Err(error) => Inbound::Broken(error),
            },
        };
        let broken = matches!(item, Inbound::Broken(_));
        if inbound.send(item).is_err() || broken {
            return;
        }
    }
}

/// Sends one frame straight on a sink, as the handshake does before the
/// queues exist.
pub async fn send_frame(
    sink: &mut dyn MessageSink,
    frame: &Frame,
    max_frame_bytes: usize,
) -> Result<(), TransportError> {
    let text = frames::encode(frame, max_frame_bytes)
        .map_err(|error| TransportError::Io(error.to_string()))?;
    sink.send(Message::Text(text)).await
}
