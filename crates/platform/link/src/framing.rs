//! Control and binary framing.
//!
//! A session runs over anything that carries whole messages in order in both
//! directions, which is what a WebSocket does. This module says what that is
//! ([`Connection`](crate::framing::Connection): a half that sends, a half that receives, and what is
//! known of the other end), reads frames off it and writes frames onto it.
//!
//! **Data does not hold up control.** What is to be sent waits in queues, and
//! the writer always takes from the first that has something: control frames
//! (acknowledgements, results, credit, pings), then events, then tunnel data.
//! Data frames are sized to what the link has been seen to carry, about a
//! quarter of a second of it, so that a control frame waits for at most one
//! small data frame and never for a long one.
//!
//! **A message is as large as the session says, not as a frame.** A text frame
//! larger than a frame goes in a queue of its own, from which the writer cuts
//! it into parts, sized as data frames are, and writes one part each time it is
//! the queue's turn, which it shares with the events: nothing waits behind a
//! large message longer than behind a frame. The reader puts the parts together
//! and passes the message on as it would a frame that came whole.
//!
//! The link is measured as it is used: the round trip from pings, and the
//! rate in each direction from what moved. The figures size the data frames
//! and are reported as facts of the machine.

use crate::ports::{Datagram, UdpBudget};
use async_trait::async_trait;
use bytes::Bytes;
use orch_link_contract::backplane::{BackplaneFrame, Down, DownFrame, Up, UpFrame};
use orch_link_contract::commands::CommandKind;
use orch_link_contract::frames::{self, DataFrame, Frame, FrameError, PartFrame, Text};
use orch_link_contract::ports::PortDatagram;
use orch_link_contract::session::MachineFactsRates;
use orch_link_contract::tunnels::data_frame_bytes;
use orch_link_contract::version::Limits;
use orch_pki::certificates::Certificate;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
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

    /// A transport heartbeat, where the transport has one of its own.
    async fn ping(&mut self) -> Result<(), TransportError> {
        Ok(())
    }

    /// Ends the connection politely.
    async fn close(&mut self);
}

/// The half of a connection that receives.
#[async_trait]
pub trait MessageSource: Send {
    #[cfg(feature = "test-observers")]
    fn inbound_observer(&self) -> Option<Arc<crate::test_observers::QueueGauge>> {
        None
    }
    /// The next message, `None` when the other end closed the connection.
    async fn recv(&mut self) -> Result<Option<Message>, TransportError>;
}

/// What is known of the other end of a connection before a word is said.
#[derive(Debug, Clone, Default)]
pub struct PeerInfo {
    /// Purpose negotiated by the WebSocket subprotocol, before machine authentication.
    pub purpose: orch_link_contract::gateways::SessionPurpose,
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
    /// The figures as reported in machine facts, retaining each sample's presence.
    #[must_use]
    pub fn as_facts(&self) -> MachineFactsRates {
        MachineFactsRates {
            up_bps: self.up_bps.map(|rate| rate.saturating_mul(BITS_PER_BYTE)),
            down_bps: self.down_bps.map(|rate| rate.saturating_mul(BITS_PER_BYTE)),
            round_trip_ms: self.round_trip_ms,
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

/// Why a message could not be queued to be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// The session it was for is over.
    #[error("the session is over")]
    SessionOver,
    /// The message is larger than the session carries: it is not sent, whatever
    /// the state of the link.
    #[error("a message of {bytes} bytes where this session carries {limit} at most")]
    TooLarge {
        /// How many bytes it would take.
        bytes: usize,
        /// The most bytes of a message the session carries.
        limit: usize,
    },
    /// The message cannot be written.
    #[error("the message cannot be written")]
    Unwritable,
}

impl From<SessionOver> for Refused {
    fn from(_: SessionOver) -> Self {
        Self::SessionOver
    }
}

/// What the writer is given, by urgency.
///
/// A text frame larger than a frame may be does not wait in the queue of its
/// kind. It waits in a fourth queue, the bulk queue, from which the writer cuts
/// it into parts as it writes: one part, then whatever else is due, so that
/// nothing waits longer behind it than behind any frame. Whatever has to stay
/// behind such a message in order, the events of one stream and the commands,
/// goes through the bulk queue as well while the message is there.
#[derive(Debug, Clone)]
pub struct Outgoing {
    control: mpsc::UnboundedSender<Queued>,
    events: mpsc::UnboundedSender<Queued>,
    bulk: mpsc::UnboundedSender<Bulk>,
    data: mpsc::UnboundedSender<Queued>,
    datagrams: mpsc::UnboundedSender<Datagram>,
    order: Arc<Order>,
    frame_bytes: usize,
    message_bytes: usize,
    #[cfg(feature = "test-observers")]
    observed: [Arc<crate::test_observers::QueueGauge>; 5],
}

/// The queues the writer takes from.
#[derive(Debug)]
pub struct Queues {
    control: mpsc::UnboundedReceiver<Queued>,
    events: mpsc::UnboundedReceiver<Queued>,
    bulk: mpsc::UnboundedReceiver<Bulk>,
    data: mpsc::UnboundedReceiver<Queued>,
    datagrams: mpsc::UnboundedReceiver<Datagram>,
    order: Arc<Order>,
    #[cfg(feature = "test-observers")]
    observed: [Arc<crate::test_observers::QueueGauge>; 5],
}

/// A whole frame and, when requested, a receipt completed only after the transport takes it.
#[derive(Debug)]
struct Queued {
    message: Message,
    sent: Option<oneshot::Sender<()>>,
    keep_alive: bool,
}

#[cfg(feature = "test-observers")]
impl Drop for Queues {
    fn drop(&mut self) {
        for queue in &self.observed {
            drop(queue.receiver());
        }
    }
}

/// What is kept between those who queue and the writer so that frames that
/// have to keep their order keep it when one of them travels in parts.
#[derive(Debug, Default)]
struct Order {
    /// How many frames were put in the events queue, and how many of them the
    /// writer has written.
    queued: AtomicU64,
    written: AtomicU64,
    /// The number the next message cut in parts is given.
    next_message: AtomicU32,
    /// For each domain of order, how many frames wait in the bulk queue.
    waiting: Mutex<HashMap<Domain, usize>>,
}

impl Order {
    fn waiting(&self) -> MutexGuard<'_, HashMap<Domain, usize>> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A frame of `domain` was written from the bulk queue.
    fn finished(&self, domain: Option<&Domain>) {
        let Some(domain) = domain else {
            return;
        };
        let mut waiting = self.waiting();
        let empty = match waiting.get_mut(domain) {
            Some(count) => {
                *count = count.saturating_sub(1);
                *count == 0
            }
            None => false,
        };
        if empty {
            waiting.remove(domain);
        }
    }
}

/// Separate namespaces keep a command key from colliding with an event stream name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Domain {
    Events(String),
    Reports(String),
    Commands(String),
    BackplaneCommands(orch_types::Id, String),
    BackplaneEvents(orch_types::Id, orch_types::Id, String),
    BackplaneReports(orch_types::Id, orch_types::Id, String),
}

/// What a frame has to keep its order with, when some of the frames of that
/// kind are too large for a frame and take a queue of their own: the events of
/// one stream arrive in the order they were sent, and commands keep the order
/// of their serial key. Commands without a key may run independently.
fn domain_of(frame: &Frame) -> Option<Domain> {
    match frame {
        Frame::Ev(event) => Some(Domain::Events(event.stream.clone())),
        Frame::Evb(batch) => Some(Domain::Events(batch.stream.clone())),
        Frame::Eph(report) => Some(Domain::Reports(report.stream.clone())),
        Frame::Cmd(cmd) => cmd.key.clone().map(Domain::Commands),
        _ => None,
    }
}

/// A message in the bulk queue.
#[derive(Debug)]
struct Bulk {
    body: BulkBody,
    /// What it keeps its order with.
    domain: Option<Domain>,
    /// How many frames of the events queue were queued before it: it is not
    /// written before the writer has written that many.
    not_before: u64,
    sent: Option<oneshot::Sender<()>>,
}

#[derive(Debug)]
enum BulkBody {
    /// A message that fits a frame, kept behind the large one before it.
    Whole(Message),
    /// The text of a frame that does not, which is cut in parts as it is
    /// written.
    Large(TextParts),
}

/// The existing parts encoder, also used by the authenticated API/gateway backplane.
#[derive(Debug)]
pub struct TextParts {
    message: u32,
    text: Bytes,
    sent: usize,
    paged: bool,
}

impl TextParts {
    /// A logical text value, sent in order without allocating a second full copy of its bytes.
    #[must_use]
    pub fn new(message: u32, text: String, paged: bool) -> Self {
        Self {
            message,
            text: Bytes::from(text),
            sent: 0,
            paged,
        }
    }

    /// One bounded piece, or none after the logical value is complete.
    pub fn next_part(
        &mut self,
        frame_bytes: usize,
        payload_bytes: usize,
    ) -> Result<Option<Bytes>, FrameError> {
        if self.sent == self.text.len() {
            return Ok(None);
        }
        let room = if self.paged {
            PartFrame::max_paged_payload(frame_bytes)
        } else {
            PartFrame::max_payload(frame_bytes)
        };
        let end = self
            .sent
            .saturating_add(payload_bytes.min(room).max(1))
            .min(self.text.len());
        let part = PartFrame {
            message: self.message,
            paged: self.paged,
            offset: u64::try_from(self.sent)
                .map_err(|_| FrameError::Part("needs a wider offset than the representation"))?,
            total: u64::try_from(self.text.len())
                .map_err(|_| FrameError::Part("needs a wider length than the representation"))?,
            payload: self.text.slice(self.sent..end),
        };
        let bytes = part.encode(frame_bytes)?;
        self.sent = end;
        Ok(Some(bytes))
    }
}

#[cfg(feature = "test-observers")]
impl Bulk {
    fn observed_bytes(&self) -> usize {
        match &self.body {
            BulkBody::Whole(message) => message.len(),
            BulkBody::Large(parts) => parts.text.len(),
        }
    }
}

/// A set of queues, for a writer, that carries messages of up to the limits.
#[must_use]
pub fn queues(limits: Limits) -> (Outgoing, Queues) {
    bounded_queues(limits.frame_bytes(), limits.message_bytes())
}

/// The same priority and ordering queues, with room for the backplane's envelope.
#[must_use]
pub fn backplane_queues(limits: Limits) -> (Outgoing, Queues) {
    bounded_queues(limits.frame_bytes(), limits.backplane_bytes())
}

fn bounded_queues(frame_bytes: usize, message_bytes: usize) -> (Outgoing, Queues) {
    let (control_tx, control) = mpsc::unbounded_channel();
    let (events_tx, events) = mpsc::unbounded_channel();
    let (bulk_tx, bulk) = mpsc::unbounded_channel();
    let (data_tx, data) = mpsc::unbounded_channel();
    let (datagram_tx, datagrams) = mpsc::unbounded_channel();
    let order = Arc::new(Order::default());
    #[cfg(feature = "test-observers")]
    let observed = [
        "link.control",
        "link.events",
        "link.data",
        "link.datagrams",
        "link.bulk",
    ]
    .map(crate::test_observers::QueueGauge::new);
    (
        Outgoing {
            control: control_tx,
            events: events_tx,
            bulk: bulk_tx,
            data: data_tx,
            datagrams: datagram_tx,
            order: Arc::clone(&order),
            frame_bytes,
            message_bytes,
            #[cfg(feature = "test-observers")]
            observed: observed.clone(),
        },
        Queues {
            control,
            events,
            bulk,
            data,
            datagrams,
            order,
            #[cfg(feature = "test-observers")]
            observed,
        },
    )
}

/// The queue is closed: the session it belonged to is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the session is over")]
pub struct SessionOver;

/// What a refused frame was, for the words that tell who sent it.
fn refusal(error: FrameError) -> Refused {
    match error {
        FrameError::TooLarge { bytes, max } => Refused::TooLarge { bytes, limit: max },
        error => {
            tracing::error!(%error, "a frame was not sent");
            Refused::Unwritable
        }
    }
}

impl Outgoing {
    /// A backplane value follows the same serial-key order, scoped to its machine.
    pub fn backplane(&self, frame: &BackplaneFrame) -> Result<oneshot::Receiver<()>, Refused> {
        let paged = frame.is_paged();
        let text = frames::encode(
            frame,
            if paged {
                usize::MAX
            } else {
                self.message_bytes
            },
        )
        .map_err(refusal)?;
        let domain = match frame {
            BackplaneFrame::Down(Down {
                machine_id,
                frame: DownFrame::Cmd(cmd),
            }) => cmd
                .key
                .clone()
                .map(|key| Domain::BackplaneCommands(*machine_id, key)),
            BackplaneFrame::Up(Up {
                machine_id,
                session_id,
                frame,
                ..
            }) => match frame {
                UpFrame::Ev(event) => Some(Domain::BackplaneEvents(
                    *machine_id,
                    *session_id,
                    event.stream.clone(),
                )),
                UpFrame::Evb(batch) => Some(Domain::BackplaneEvents(
                    *machine_id,
                    *session_id,
                    batch.stream.clone(),
                )),
                UpFrame::Eph(report) => Some(Domain::BackplaneReports(
                    *machine_id,
                    *session_id,
                    report.stream.clone(),
                )),
                _ => None,
            },
            _ => None,
        };
        let (sent, receipt) = oneshot::channel();
        self.queue_text(&self.control, 0, text, domain, Some(sent))?;
        Ok(receipt)
    }

    /// A socket heartbeat goes through the control lane without becoming a protocol text frame.
    pub fn keep_alive(&self) -> Result<(), SessionOver> {
        #[cfg(feature = "test-observers")]
        self.observed[0].enqueue(Some(0));
        self.control
            .send(Queued {
                message: Message::Text(String::new()),
                sent: None,
                keep_alive: true,
            })
            .map_err(|_| {
                #[cfg(feature = "test-observers")]
                self.observed[0].failed_enqueue(Some(0));
                SessionOver
            })
    }

    /// Raw backplane data retains a receipt for its actual write and any datagram lease.
    pub fn data_bytes_sent(&self, bytes: Bytes) -> Result<oneshot::Receiver<()>, SessionOver> {
        let (sent, receipt) = oneshot::channel();
        self.send_to(&self.data, 2, Message::Binary(bytes), Some(sent))?;
        Ok(receipt)
    }
    /// Queues a leased UDP packet without waiting or consuming stream credit.
    pub fn datagram(&self, packet: Datagram) -> Result<(), SessionOver> {
        #[cfg(feature = "test-observers")]
        let bytes = Some(packet.frame.payload.len() + PortDatagram::HEADER_BYTES);
        #[cfg(feature = "test-observers")]
        self.observed[3].enqueue(bytes);
        self.datagrams.send(packet).map_err(|_| {
            #[cfg(feature = "test-observers")]
            self.observed[3].failed_enqueue(bytes);
            SessionOver
        })
    }

    /// Queues a control frame: it goes before anything else. A frame that does
    /// not fit a frame goes in parts, in its turn among the others.
    pub fn control(&self, frame: &Frame) -> Result<(), Refused> {
        let paged = matches!(frame, Frame::Cmd(cmd) if cmd.kind.parse::<CommandKind>().is_ok_and(CommandKind::is_paged));
        self.queue(&self.control, 0, frame, None, paged)
    }

    /// Sends the result of a kind whose logical value can span message allowances.
    pub fn control_paged(&self, frame: &Frame) -> Result<(), Refused> {
        self.queue(&self.control, 0, frame, None, true)
    }

    /// Queues a control frame with a receipt for its complete transport write.
    /// Dropping the writer without finishing the message drops the receipt too.
    pub fn control_sent(&self, frame: &Frame) -> Result<oneshot::Receiver<()>, Refused> {
        let (sent, receipt) = oneshot::channel();
        let paged = matches!(frame, Frame::Cmd(cmd) if cmd.kind.parse::<CommandKind>().is_ok_and(CommandKind::is_paged));
        self.queue(&self.control, 0, frame, Some(sent), paged)?;
        Ok(receipt)
    }

    /// Queues a frame of events.
    pub fn events(&self, frame: &Frame) -> Result<(), Refused> {
        self.queue(&self.events, 1, frame, None, false)
    }

    /// Queues a frame of tunnel data.
    pub fn data(&self, frame: &DataFrame) -> Result<(), Refused> {
        let bytes = frame.encode(self.frame_bytes).map_err(refusal)?;
        self.send_to(&self.data, 2, Message::Binary(bytes), None)
            .map_err(Refused::from)
    }

    /// Queues a text frame that has to arrive in order with the data before
    /// it: the end of a stream, a clean close.
    pub fn ordered(&self, frame: &Frame) -> Result<(), Refused> {
        self.queue(&self.data, 2, frame, None, false)
    }

    /// Queues a binary frame that is already written.
    pub fn data_bytes(&self, bytes: Bytes) -> Result<(), SessionOver> {
        self.send_to(&self.data, 2, Message::Binary(bytes), None)
    }

    /// Queues text that is already written, as a control frame.
    pub fn control_text(&self, text: String) -> Result<(), SessionOver> {
        self.send_to(&self.control, 0, Message::Text(text), None)
    }

    /// The most bytes of a message this session carries.
    #[must_use]
    pub const fn message_bytes(&self) -> usize {
        self.message_bytes
    }

    /// Queues a text frame in `queue`, or, when it is larger than a frame, or
    /// has to stay behind one that is, in the bulk queue.
    fn queue(
        &self,
        queue: &mpsc::UnboundedSender<Queued>,
        lane: usize,
        frame: &Frame,
        sent: Option<oneshot::Sender<()>>,
        paged: bool,
    ) -> Result<(), Refused> {
        let text = frames::encode(
            frame,
            if paged {
                usize::MAX
            } else {
                self.message_bytes
            },
        )
        .map_err(refusal)?;
        let domain = domain_of(frame);
        self.queue_text(queue, lane, text, domain, sent)
    }

    fn queue_text(
        &self,
        queue: &mpsc::UnboundedSender<Queued>,
        lane: usize,
        text: String,
        domain: Option<Domain>,
        sent: Option<oneshot::Sender<()>>,
    ) -> Result<(), Refused> {
        if lane == 0 && domain.is_none() && text.len() <= self.frame_bytes {
            // The ordinary case: an acknowledgement, a ping, a small answer.
            return self
                .send_to(queue, lane, Message::Text(text), sent)
                .map_err(Refused::from);
        }
        let mut waiting = self.order.waiting();
        let large = text.len() > self.frame_bytes;
        let behind = domain
            .as_ref()
            .is_some_and(|domain| waiting.contains_key(domain));
        if !large && !behind {
            if lane == 1 {
                self.order.queued.fetch_add(1, Ordering::SeqCst);
            }
            return self
                .send_to(queue, lane, Message::Text(text), sent)
                .map_err(Refused::from);
        }
        let body = if large {
            let wide = text.len() > self.message_bytes;
            BulkBody::Large(TextParts::new(
                self.order.next_message.fetch_add(1, Ordering::Relaxed),
                text,
                wide,
            ))
        } else {
            BulkBody::Whole(Message::Text(text))
        };
        let not_before = if domain.is_some() {
            self.order.queued.load(Ordering::SeqCst)
        } else {
            0
        };
        let item = Bulk {
            body,
            domain: domain.clone(),
            not_before,
            sent,
        };
        #[cfg(feature = "test-observers")]
        let bytes = Some(item.observed_bytes());
        #[cfg(feature = "test-observers")]
        self.observed[4].enqueue(bytes);
        if self.bulk.send(item).is_err() {
            #[cfg(feature = "test-observers")]
            self.observed[4].failed_enqueue(bytes);
            return Err(Refused::SessionOver);
        }
        if let Some(domain) = domain {
            *waiting.entry(domain).or_insert(0) += 1;
        }
        Ok(())
    }

    fn send_to(
        &self,
        queue: &mpsc::UnboundedSender<Queued>,
        _lane: usize,
        message: Message,
        sent: Option<oneshot::Sender<()>>,
    ) -> Result<(), SessionOver> {
        #[cfg(feature = "test-observers")]
        let bytes = Some(message.len());
        #[cfg(feature = "test-observers")]
        self.observed[_lane].enqueue(bytes);
        queue
            .send(Queued {
                message,
                sent,
                keep_alive: false,
            })
            .map_err(|_| {
                #[cfg(feature = "test-observers")]
                self.observed[_lane].failed_enqueue(bytes);
                SessionOver
            })
    }

    /// Whether the writer is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.control.is_closed()
    }
}

/// A message of the bulk queue that the writer has begun.
struct Active {
    body: BulkBody,
    domain: Option<Domain>,
    not_before: u64,
    receipt: Option<oneshot::Sender<()>>,
    #[cfg(feature = "test-observers")]
    flight: crate::test_observers::Flight,
}

impl Active {
    /// The next message to write for it, and whether it is the last.
    fn next(
        &mut self,
        meter: &LinkMeter,
        frame_bytes: usize,
    ) -> Result<(Message, bool), FrameError> {
        match &mut self.body {
            BulkBody::Whole(message) => Ok((
                std::mem::replace(message, Message::Text(String::new())),
                true,
            )),
            BulkBody::Large(parts) => {
                let room = if parts.paged {
                    PartFrame::max_paged_payload(frame_bytes)
                } else {
                    PartFrame::max_payload(frame_bytes)
                }
                .max(1);
                let piece = meter.data_frame_bytes(room).clamp(1, room);
                let bytes = parts
                    .next_part(frame_bytes, piece)?
                    .ok_or(FrameError::Empty)?;
                Ok((Message::Binary(bytes), parts.sent == parts.text.len()))
            }
        }
    }
}

/// What the writer chose to do next.
enum Picked {
    /// Write a message of a queue.
    Message(Queued, usize),
    /// Write a leased UDP packet.
    Packet(Message, Datagram),
    /// Take the next message of the bulk queue in hand.
    Begin(Bulk),
    /// Write the next part of the message in hand.
    Part,
}

/// Writes what is queued until every sender is gone and the queues are
/// empty, the session is ended, or the transport fails; then closes the
/// transport. Control goes before events and events before data, whenever
/// there is a choice. A large message in hand is written one part at a time,
/// and after each part the next frame is chosen as ever, whatever it is
/// (control, then events, then data), and only the frame after that is the
/// large message's part again: it takes every other turn when anything else
/// waits, and all of them when nothing does, so that nothing waits behind it
/// longer than behind a frame. When the session is ended, what is still
/// waiting as control (a `bye` among it) is written first.
pub async fn write_loop(
    mut sink: Box<dyn MessageSink>,
    mut queues: Queues,
    meter: Arc<LinkMeter>,
    frame_bytes: usize,
    ended: CancellationToken,
) -> Result<(), TransportError> {
    let mut result = Ok(());
    let mut udp_next = false;
    // Whether the large message in hand is due its next part before anything but control.
    let mut bulk_turn = false;
    let mut active: Option<Active> = None;
    'writing: loop {
        // The next message is chosen only when the transport can take it: a
        // command that arrives while a data frame is on the wire goes next,
        // not after the data frame that was chosen before it came.
        tokio::select! {
            biased;
            () = ended.cancelled() => break,
            () = sink.ready() => {}
        }
        // A message of the bulk queue that has to stay behind frames of the
        // events queue waits until they are written.
        let eligible = active
            .as_ref()
            .is_some_and(|active| queues.order.written.load(Ordering::SeqCst) >= active.not_before);
        let part_first = eligible && bulk_turn;
        let part_next = eligible && !bulk_turn;
        // Alternate ready TCP and UDP after control/events; neither data kind
        // can starve the other or hold a control frame behind its full store.
        let picked = if udp_next {
            tokio::select! {
                biased;
                Some(message) = queues.control.recv() => Picked::Message(message, 0),
                () = ended.cancelled() => break,
                Some(next) = queues.bulk.recv(), if active.is_none() => Picked::Begin(next),
                () = std::future::ready(()), if part_first => Picked::Part,
                Some(message) = queues.events.recv() => Picked::Message(message, 1),
                Some(packet) = queues.datagrams.recv() => match packet.frame.encode() {
                    Ok(bytes) => Picked::Packet(Message::Binary(bytes), packet),
                    Err(_) => {
                        #[cfg(feature = "test-observers")]
                        drop(queues.observed[3].selected(Some(packet.frame.payload.len() + PortDatagram::HEADER_BYTES)));
                        continue;
                    }
                },
                Some(message) = queues.data.recv() => Picked::Message(message, 2),
                () = std::future::ready(()), if part_next => Picked::Part,
                else => break,
            }
        } else {
            tokio::select! {
                biased;
                Some(message) = queues.control.recv() => Picked::Message(message, 0),
                () = ended.cancelled() => break,
                Some(next) = queues.bulk.recv(), if active.is_none() => Picked::Begin(next),
                () = std::future::ready(()), if part_first => Picked::Part,
                Some(message) = queues.events.recv() => Picked::Message(message, 1),
                Some(message) = queues.data.recv() => Picked::Message(message, 2),
                Some(packet) = queues.datagrams.recv() => match packet.frame.encode() {
                    Ok(bytes) => Picked::Packet(Message::Binary(bytes), packet),
                    Err(_) => {
                        #[cfg(feature = "test-observers")]
                        drop(queues.observed[3].selected(Some(packet.frame.payload.len() + PortDatagram::HEADER_BYTES)));
                        continue;
                    }
                },
                () = std::future::ready(()), if part_next => Picked::Part,
                else => break,
            }
        };
        let mut last_part = false;
        let (message, packet, lane, receipt) = match picked {
            Picked::Message(queued, _lane) if queued.keep_alive => {
                #[cfg(feature = "test-observers")]
                let observed_send = queues.observed[_lane].selected(Some(0));
                if let Err(error) = sink.ping().await {
                    result = Err(error);
                    break 'writing;
                }
                #[cfg(feature = "test-observers")]
                observed_send.complete();
                continue;
            }
            Picked::Message(queued, lane) => (queued.message, None, lane, queued.sent),
            Picked::Packet(message, packet) => (message, Some(packet), 3, None),
            Picked::Begin(next) => {
                #[cfg(feature = "test-observers")]
                let flight = queues.observed[4].selected(Some(next.observed_bytes()));
                active = Some(Active {
                    body: next.body,
                    domain: next.domain,
                    not_before: next.not_before,
                    receipt: next.sent,
                    #[cfg(feature = "test-observers")]
                    flight,
                });
                continue;
            }
            Picked::Part => {
                let Some(current) = active.as_mut() else {
                    continue;
                };
                match current.next(&meter, frame_bytes) {
                    Ok((message, last)) => {
                        last_part = last;
                        (message, None, 4, None)
                    }
                    Err(error) => {
                        tracing::error!(%error, "a part of a large message could not be written");
                        result = Err(TransportError::Io(error.to_string()));
                        break 'writing;
                    }
                }
            }
        };
        udp_next = packet.is_none();
        let bytes = message.len();
        // A large message is observed whole, when it is begun and when it is done, not part by part.
        #[cfg(feature = "test-observers")]
        let observed_send = (lane != 4).then(|| queues.observed[lane].selected(Some(bytes)));
        if let Err(error) = sink.send(message).await {
            result = Err(error);
            break 'writing;
        }
        if let Some(packet) = packet {
            packet.delivered();
        }
        if let Some(receipt) = receipt {
            let _ = receipt.send(());
        }
        meter.observe_sent(bytes, Instant::now());
        #[cfg(feature = "test-observers")]
        if let Some(observed_send) = observed_send {
            observed_send.complete();
        }
        match lane {
            // Whatever is written but control gives the large message its turn next.
            1..=3 => {
                if lane == 1 {
                    queues.order.written.fetch_add(1, Ordering::SeqCst);
                }
                bulk_turn = true;
            }
            4 => {
                bulk_turn = false;
                if last_part && let Some(done) = active.take() {
                    queues.order.finished(done.domain.as_ref());
                    if let Some(receipt) = done.receipt {
                        let _ = receipt.send(());
                    }
                    #[cfg(feature = "test-observers")]
                    done.flight.complete();
                }
            }
            _ => {}
        }
    }
    if result.is_ok() {
        while let Ok(queued) = queues.control.try_recv() {
            #[cfg(feature = "test-observers")]
            let observed_send = queues.observed[0].selected(Some(queued.message.len()));
            let sent = if queued.keep_alive {
                sink.ping().await
            } else {
                sink.send(queued.message).await
            };
            if let Err(error) = sent {
                result = Err(error);
                break;
            }
            if let Some(receipt) = queued.sent {
                let _ = receipt.send(());
            }
            #[cfg(feature = "test-observers")]
            observed_send.complete();
        }
    }
    sink.close().await;
    result
}

/// A large message that is being put together.
#[derive(Debug)]
struct Assembly {
    /// The number its sender gave it.
    message: u32,
    paged: bool,
    /// How many bytes it is to have.
    total: usize,
    /// What has come of it.
    text: Vec<u8>,
    /// When the last part came.
    last_part: Instant,
}

/// Takes in a part of a large message. Returns the text of the message when this
/// was its last part, and `None` while parts are still to come. A part that is
/// not one that can follow what came before, or that announces a message the
/// session does not carry, is an error: the other end broke the protocol.
fn take_part(
    open: &mut Option<Assembly>,
    bytes: Bytes,
    limits: Limits,
    now: Instant,
) -> Result<Option<String>, FrameError> {
    take_part_bounded(
        open,
        bytes,
        limits.frame_bytes(),
        limits.message_bytes(),
        now,
    )
}

fn take_part_bounded(
    open: &mut Option<Assembly>,
    bytes: Bytes,
    frame_bytes: usize,
    message_bytes: usize,
    now: Instant,
) -> Result<Option<String>, FrameError> {
    let part = PartFrame::decode(bytes, frame_bytes)?;
    let total = usize::try_from(part.total)
        .map_err(|_| FrameError::Part("does not fit the receiver's address space"))?;
    if open.is_none() {
        if part.offset != 0 {
            return Err(FrameError::Part("does not begin its message"));
        }
        if !part.paged && total > message_bytes {
            return Err(FrameError::TooLarge {
                bytes: total,
                max: message_bytes,
            });
        }
        *open = Some(Assembly {
            message: part.message,
            paged: part.paged,
            total,
            text: Vec::with_capacity(part.payload.len()),
            last_part: now,
        });
    }
    let Some(current) = open.as_mut() else {
        return Ok(None);
    };
    if part.offset == 0 && !current.text.is_empty() {
        return Err(FrameError::Part(
            "begins while another message is still open",
        ));
    }
    if part.message != current.message || total != current.total || part.paged != current.paged {
        return Err(FrameError::Part(
            "belongs to another message than the open one",
        ));
    }
    if usize::try_from(part.offset).ok() != Some(current.text.len()) {
        return Err(FrameError::Part("does not follow the part before it"));
    }
    current.text.extend_from_slice(&part.payload);
    current.last_part = now;
    if current.text.len() < current.total {
        return Ok(None);
    }
    let Some(done) = open.take() else {
        return Ok(None);
    };
    String::from_utf8(done.text)
        .map(Some)
        .map_err(|_| FrameError::Part("is not text"))
}

/// Per-connection reassembly shared by machine sessions and their authenticated backplane.
#[derive(Debug, Default)]
pub struct MessageAssembly {
    open: Option<Assembly>,
}

impl MessageAssembly {
    /// Adds one part; a complete UTF-8 logical value is returned exactly once.
    pub fn take(
        &mut self,
        bytes: Bytes,
        limits: Limits,
        now: Instant,
    ) -> Result<Option<String>, FrameError> {
        take_part(&mut self.open, bytes, limits, now)
    }

    /// The authenticated wrapper has its own envelope allowance, while parts still fit a frame.
    pub fn take_backplane(
        &mut self,
        bytes: Bytes,
        limits: Limits,
        now: Instant,
    ) -> Result<Option<String>, FrameError> {
        take_part_bounded(
            &mut self.open,
            bytes,
            limits.frame_bytes(),
            limits.backplane_bytes(),
            now,
        )
    }

    /// A partial value keeps its own deadline even while other traffic continues.
    #[must_use]
    pub fn deadline(&self, wait: Duration) -> Option<Instant> {
        self.open.as_ref().map(|open| open.last_part + wait)
    }
}

/// What came off the connection.
#[derive(Debug)]
pub enum Inbound {
    /// A text frame of a known type, as one frame or put together from the
    /// parts of a large message.
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
/// protocol, and passes each on. The parts of a large message are put
/// together as they come, between the other frames, and the message is passed
/// on as one; one that stops coming for `assembly_wait` ends the session as a
/// broken protocol, and is said so.
pub async fn read_loop(
    mut source: Box<dyn MessageSource>,
    inbound: mpsc::UnboundedSender<Inbound>,
    limits: Limits,
    meter: Arc<LinkMeter>,
    assembly_wait: Duration,
) {
    #[cfg(feature = "test-observers")]
    let observed = source.inbound_observer();
    let send = |item: Inbound| {
        #[cfg(feature = "test-observers")]
        let bytes = crate::test_observers::inbound_bytes(&item);
        #[cfg(feature = "test-observers")]
        if let Some(queue) = &observed {
            queue.enqueue(bytes);
        }
        let result = inbound.send(item);
        #[cfg(feature = "test-observers")]
        if result.is_err()
            && let Some(queue) = &observed
        {
            queue.failed_enqueue(bytes);
        }
        result
    };
    let max_frame_bytes = limits.frame_bytes();
    let max_message_bytes = limits.message_bytes();
    // This additional session boundary is conservative for a machine with
    // several environments; no unbounded inbox can precede their own stores.
    let udp = Arc::new(UdpBudget::for_limits(&limits.ports));
    let mut open: Option<Assembly> = None;
    loop {
        let received = match &open {
            Some(assembly) => {
                match tokio::time::timeout_at(assembly.last_part + assembly_wait, source.recv())
                    .await
                {
                    Ok(received) => received,
                    Err(_) => {
                        tracing::warn!(
                            received = assembly.text.len(),
                            total = assembly.total,
                            "a large message was begun and its parts stopped coming; the session is given up"
                        );
                        metrics::counter!("orch_link_messages_given_up_total").increment(1);
                        let _ = send(Inbound::Broken(FrameError::Unfinished {
                            received: assembly.text.len() as u64,
                            total: assembly.total as u64,
                        }));
                        return;
                    }
                }
            }
            None => source.recv().await,
        };
        let message = match received {
            Ok(Some(message)) => message,
            Ok(None) => {
                let _ = send(Inbound::Lost(None));
                return;
            }
            Err(error) => {
                let _ = send(Inbound::Lost(Some(error)));
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
            Message::Binary(bytes)
                if bytes.first().is_some_and(|kind| PartFrame::is_kind(*kind)) =>
            {
                match take_part(&mut open, bytes, limits, Instant::now()) {
                    Ok(Some(text)) => match frames::decode::<Frame>(&text, usize::MAX) {
                        Ok(Text::Known(frame)) => {
                            let paged = matches!(&frame, Frame::CmdRes(_))
                                || matches!(&frame,
                                Frame::Cmd(cmd) if cmd.kind.parse::<CommandKind>().is_ok_and(CommandKind::is_paged));
                            if text.len() > max_message_bytes && !paged {
                                Inbound::Broken(FrameError::TooLarge {
                                    bytes: text.len(),
                                    max: max_message_bytes,
                                })
                            } else {
                                Inbound::Frame(Box::new(frame))
                            }
                        }
                        Ok(Text::Unknown(kind)) => Inbound::Unknown(kind),
                        Err(error) => Inbound::Broken(error),
                    },
                    Ok(None) => continue,
                    Err(error) => Inbound::Broken(error),
                }
            }
            Message::Binary(bytes) => match DataFrame::decode(bytes, max_frame_bytes) {
                Ok(frame) => Inbound::Data(frame),
                Err(error) => Inbound::Broken(error),
            },
        };
        let broken = matches!(item, Inbound::Broken(_));
        if send(item).is_err() || broken {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test assertions")]
mod measurement_presence_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn facts_keep_absence_zero_and_independent_direction_samples() {
        for (measured, expected) in [
            (Measurements::default(), json!({})),
            (
                Measurements {
                    round_trip_ms: Some(16),
                    up_bps: None,
                    down_bps: Some(0),
                },
                json!({"round_trip_ms":16,"down_bps":0}),
            ),
            (
                Measurements {
                    round_trip_ms: Some(0),
                    up_bps: Some(5),
                    down_bps: None,
                },
                json!({"round_trip_ms":0,"up_bps":40}),
            ),
            (
                Measurements {
                    round_trip_ms: None,
                    up_bps: Some(i64::MAX),
                    down_bps: Some(1),
                },
                json!({"up_bps":i64::MAX,"down_bps":8}),
            ),
        ] {
            assert_eq!(serde_json::to_value(measured.as_facts()).unwrap(), expected);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test assertions")]
mod large_message_tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::watch;

    /// A sink that records what it is given, and takes no more than it was allowed: the writer
    /// chooses its next message only when the sink says it is ready, so that a test can queue
    /// something between two writes, as a frame that arrives while another is on the wire.
    struct Gated {
        written: Arc<Mutex<Vec<Message>>>,
        allowed: watch::Receiver<usize>,
    }

    #[async_trait]
    impl MessageSink for Gated {
        async fn ready(&mut self) {
            let written = Arc::clone(&self.written);
            let _ = self
                .allowed
                .wait_for(|allowed| *allowed > written.lock().unwrap().len())
                .await;
        }

        async fn send(&mut self, message: Message) -> Result<(), TransportError> {
            self.written.lock().unwrap().push(message);
            Ok(())
        }

        async fn close(&mut self) {}
    }

    /// A source that gives what a test puts in, and then waits.
    struct Fed(mpsc::UnboundedReceiver<Message>);

    #[async_trait]
    impl MessageSource for Fed {
        async fn recv(&mut self) -> Result<Option<Message>, TransportError> {
            Ok(self.0.recv().await)
        }
    }

    struct Rig {
        out: Outgoing,
        written: Arc<Mutex<Vec<Message>>>,
        allow: watch::Sender<usize>,
        limits: Limits,
    }

    /// A small frame and a message of a megabyte: parts of 4 KiB, which is what a link not yet measured gets.
    fn small_frames() -> Limits {
        Limits {
            max_frame_bytes: 8_192,
            max_message_bytes: 1_048_576,
            ..Limits::default()
        }
    }

    fn rig(limits: Limits) -> Rig {
        let (out, queued) = queues(limits);
        let written = Arc::new(Mutex::new(Vec::new()));
        let (allow, allowed) = watch::channel(0_usize);
        tokio::spawn(write_loop(
            Box::new(Gated {
                written: Arc::clone(&written),
                allowed,
            }),
            queued,
            LinkMeter::new(),
            limits.frame_bytes(),
            CancellationToken::new(),
        ));
        Rig {
            out,
            written,
            allow,
            limits,
        }
    }

    impl Rig {
        /// Lets the writer write until `total` messages are written, and waits until they are.
        async fn write_until(&self, total: usize) {
            self.allow.send_replace(total);
            tokio::time::timeout(Duration::from_secs(10), async {
                while self.written.lock().unwrap().len() < total {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("the writer wrote what it was allowed to");
        }

        /// Lets the writer write whatever is queued, and waits until it has nothing more to write.
        async fn write_all(&self) {
            self.allow.send_replace(usize::MAX);
            let mut before = usize::MAX;
            loop {
                tokio::time::sleep(Duration::from_millis(30)).await;
                let now = self.written.lock().unwrap().len();
                if now == before {
                    return;
                }
                before = now;
            }
        }

        fn written(&self) -> Vec<Message> {
            self.written.lock().unwrap().clone()
        }

        /// The frames the reader would hand on, in the order it would.
        fn delivered(&self) -> Vec<Frame> {
            delivered(&self.written(), self.limits)
        }
    }

    fn delivered(messages: &[Message], limits: Limits) -> Vec<Frame> {
        let mut open = None;
        let mut frames = Vec::new();
        let known = |text: &str, bound: usize| match frames::decode::<Frame>(text, bound).unwrap() {
            Text::Known(frame) => frame,
            Text::Unknown(kind) => panic!("a frame of the unknown type {kind}"),
        };
        for message in messages {
            match message {
                Message::Text(text) => frames.push(known(text, limits.frame_bytes())),
                Message::Binary(bytes)
                    if bytes.first().is_some_and(|kind| PartFrame::is_kind(*kind)) =>
                {
                    if let Some(text) =
                        take_part(&mut open, bytes.clone(), limits, Instant::now()).unwrap()
                    {
                        frames.push(known(&text, usize::MAX));
                    }
                }
                Message::Binary(_) => {}
            }
        }
        frames
    }

    fn kinds(messages: &[Message]) -> Vec<&'static str> {
        messages
            .iter()
            .map(|message| match message {
                Message::Text(_) => "text",
                Message::Binary(bytes)
                    if bytes.first().is_some_and(|kind| PartFrame::is_kind(*kind)) =>
                {
                    "part"
                }
                Message::Binary(_) => "data",
            })
            .collect()
    }

    fn reply(size: usize) -> Frame {
        serde_json::from_value(json!({
            "t": "cmd_res", "id": "0199c1a0-0000-7000-8000-00000000000a", "ok": true,
            "b": { "text": "x".repeat(size) }
        }))
        .unwrap()
    }

    fn event(stream: &str, seq: i64, size: usize) -> Frame {
        serde_json::from_value(json!({
            "t": "ev", "s": stream, "q": seq, "at": "2026-10-08T17:04:05.000Z", "k": "item.delta",
            "b": { "data": "x".repeat(size) }
        }))
        .unwrap()
    }

    fn ping(n: i64) -> Frame {
        serde_json::from_value(json!({ "t": "ping", "n": n })).unwrap()
    }

    fn data(offset: u64) -> DataFrame {
        DataFrame {
            channel: 1,
            offset,
            payload: Bytes::from(vec![7_u8; 100]),
        }
    }

    fn seqs(frames: &[Frame]) -> Vec<(String, i64)> {
        frames
            .iter()
            .filter_map(|frame| match frame {
                Frame::Ev(event) => Some((event.stream.clone(), event.seq)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_large_message_is_written_in_parts_that_fit_a_frame_and_comes_out_whole() {
        let rig = rig(small_frames());
        let big = reply(300_000);
        rig.out.control(&big).unwrap();
        rig.write_all().await;
        let written = rig.written();
        assert!(
            written.len() > 30,
            "parts of a few kilobytes: {}",
            written.len()
        );
        assert!(
            written
                .iter()
                .all(|message| matches!(message, Message::Binary(_))
                    && message.len() <= rig.limits.frame_bytes()),
            "every part fits a frame"
        );
        assert_eq!(rig.delivered(), vec![big]);
    }

    #[tokio::test]
    async fn a_transport_receipt_waits_for_the_last_part_of_its_message() {
        let rig = rig(small_frames());
        let big = reply(40_000);
        let mut receipt = rig.out.control_sent(&big).unwrap();
        rig.write_until(2).await;
        assert!(matches!(
            receipt.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(
            rig.delivered().is_empty(),
            "only the first parts were written"
        );
        rig.write_all().await;
        receipt.await.unwrap();
        assert_eq!(rig.delivered(), vec![big]);
    }

    #[tokio::test]
    async fn small_replies_finish_while_two_large_replies_are_still_incomplete() {
        let rig = rig(small_frames());
        let first = reply(60_000);
        let second = reply(90_000);
        rig.out.control(&first).unwrap();
        rig.out.control(&second).unwrap();
        rig.write_until(2).await;
        rig.out.control(&reply(1)).unwrap();
        rig.write_until(3).await;
        assert_eq!(
            rig.delivered(),
            vec![reply(1)],
            "neither large reply is complete"
        );
        rig.write_all().await;
        assert_eq!(rig.delivered(), vec![reply(1), first, second]);
    }

    #[tokio::test]
    async fn backplane_pages_keep_each_machines_command_key_in_order_while_other_controls_pass() {
        let rig = rig(small_frames());
        let machine = orch_types::new_id();
        let other_machine = orch_types::new_id();
        let Frame::Cmd(mut large) = orch_testkit::doubles::link::large_messages::example(
            CommandKind::ProviderComplete,
            rig.limits.message_bytes() + 1,
        ) else {
            panic!("a command")
        };
        large.key = Some("machine".into());
        let mut same_key = large.clone();
        same_key.id = orch_types::new_id();
        same_key.body["input"] = json!("small");
        let mut independent = same_key.clone();
        independent.id = orch_types::new_id();
        let wrap = |machine_id, cmd| {
            BackplaneFrame::Down(Down {
                machine_id,
                frame: DownFrame::Cmd(cmd),
            })
        };
        let first = large.id;
        let second = same_key.id;
        let third = independent.id;
        drop(rig.out.backplane(&wrap(machine, large)).unwrap());
        rig.write_until(2).await;
        drop(rig.out.backplane(&wrap(machine, same_key)).unwrap());
        drop(
            rig.out
                .backplane(&wrap(other_machine, independent))
                .unwrap(),
        );
        rig.write_until(3).await;
        let written = rig.written();
        let Message::Text(text) = &written[2] else {
            panic!("the independent control is next")
        };
        let Text::Known(BackplaneFrame::Down(Down {
            frame: DownFrame::Cmd(cmd),
            ..
        })) = frames::decode::<BackplaneFrame>(text, usize::MAX).unwrap()
        else {
            panic!("a command")
        };
        assert_eq!(cmd.id, third);
        rig.write_all().await;
        let mut assembly = MessageAssembly::default();
        let mut order = Vec::new();
        for message in rig.written() {
            let text = match message {
                Message::Text(text) => Some(text),
                Message::Binary(bytes) => assembly
                    .take_backplane(bytes, rig.limits, Instant::now())
                    .unwrap(),
            };
            if let Some(text) = text {
                let Text::Known(BackplaneFrame::Down(Down {
                    frame: DownFrame::Cmd(cmd),
                    ..
                })) = frames::decode::<BackplaneFrame>(&text, usize::MAX).unwrap()
                else {
                    panic!("a command")
                };
                order.push(cmd.id);
            }
        }
        assert_eq!(order, [third, first, second]);
    }

    #[tokio::test]
    async fn control_and_data_are_written_between_the_parts_of_a_large_message() {
        let rig = rig(small_frames());
        let big = reply(64 * 1024);
        rig.out.control(&big).unwrap();
        for offset in 0..3 {
            rig.out.data(&data(offset * 100)).unwrap();
        }
        rig.write_until(6).await;
        assert_eq!(
            kinds(&rig.written()),
            ["data", "part", "data", "part", "data", "part"],
            "the large message takes every other turn while anything else waits"
        );
        rig.out.control(&ping(7)).unwrap();
        rig.write_until(7).await;
        assert!(
            matches!(&rig.written()[6], Message::Text(text) if text.contains("\"ping\"")),
            "a control frame goes before the next part"
        );
        rig.write_all().await;
        let frames = rig.delivered();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], ping(7));
        assert_eq!(frames[1], big);
    }

    #[tokio::test]
    async fn a_ping_waits_behind_the_part_in_flight_and_no_more() {
        let rig = rig(small_frames());
        rig.out.control(&reply(64 * 1024)).unwrap();
        rig.write_until(2).await;
        rig.out.control(&ping(1)).unwrap();
        rig.write_until(3).await;
        let written = rig.written();
        assert_eq!(kinds(&written)[..3], ["part", "part", "text"]);
    }

    #[tokio::test]
    async fn a_small_event_stays_behind_a_large_one_of_its_stream_and_another_stream_does_not_wait()
    {
        let rig = rig(small_frames());
        rig.out.events(&event("one", 1, 40_000)).unwrap();
        rig.out.events(&event("one", 2, 10)).unwrap();
        rig.out.events(&event("two", 1, 10)).unwrap();
        rig.write_all().await;
        let order = seqs(&rig.delivered());
        let place = |stream: &str, seq: i64| {
            order
                .iter()
                .position(|(name, number)| name == stream && *number == seq)
                .unwrap()
        };
        assert_eq!(order.len(), 3);
        assert!(place("one", 1) < place("one", 2), "{order:?}");
        assert!(place("two", 1) < place("one", 2), "{order:?}");
    }

    #[tokio::test]
    async fn a_large_event_does_not_overtake_the_events_of_its_stream_sent_before_it() {
        // A frame of 4 KiB: the large event is two parts, which it would be done with after two turns
        // among ten small events, if it were let go before them.
        let rig = rig(Limits {
            max_frame_bytes: 4_096,
            max_message_bytes: 1_048_576,
            ..Limits::default()
        });
        for seq in 1..=10 {
            rig.out.events(&event("one", seq, 10)).unwrap();
        }
        rig.out.events(&event("one", 11, 5_000)).unwrap();
        for seq in 12..=14 {
            rig.out.events(&event("one", seq, 10)).unwrap();
        }
        rig.write_all().await;
        let numbers: Vec<i64> = seqs(&rig.delivered())
            .into_iter()
            .map(|(_, seq)| seq)
            .collect();
        assert_eq!(numbers, (1..=14).collect::<Vec<i64>>());
    }

    #[tokio::test]
    async fn a_message_over_the_bound_is_refused_with_its_size_and_the_limit_and_nothing_is_written()
     {
        let rig = rig(Limits {
            max_frame_bytes: 8_192,
            max_message_bytes: 65_536,
            ..Limits::default()
        });
        let refused = rig.out.control(&reply(100_000)).unwrap_err();
        assert!(
            matches!(refused, Refused::TooLarge { bytes, limit } if bytes > 100_000 && limit == 65_536),
            "{refused:?}"
        );
        assert!(
            refused.to_string().contains("65536"),
            "the words name the limit: {refused}"
        );
        let refused = rig.out.events(&event("one", 1, 100_000)).unwrap_err();
        assert!(matches!(refused, Refused::TooLarge { .. }), "{refused:?}");
        // The session is up: a frame that fits is written, and it is the only thing written.
        rig.out.control(&ping(1)).unwrap();
        rig.write_until(1).await;
        rig.write_all().await;
        assert_eq!(rig.written().len(), 1);
    }

    #[tokio::test]
    async fn parts_are_put_together_in_order_and_what_cannot_follow_is_refused() {
        let limits = small_frames();
        let text = serde_json::to_string(&reply(15_000)).unwrap();
        let bytes = Bytes::from(text.clone());
        let part = |message: u32, offset: usize, end: usize, total: usize| {
            PartFrame {
                paged: false,
                message,
                offset: u64::try_from(offset).unwrap(),
                total: u64::try_from(total).unwrap(),
                payload: bytes.slice(offset..end),
            }
            .encode(limits.frame_bytes())
            .unwrap()
        };
        let total = bytes.len();
        let now = Instant::now();

        let mut open = None;
        assert_eq!(
            take_part(&mut open, part(1, 0, 4_000, total), limits, now).unwrap(),
            None
        );
        assert_eq!(
            take_part(&mut open, part(1, 4_000, 8_000, total), limits, now).unwrap(),
            None
        );
        assert_eq!(
            take_part(&mut open, part(1, 8_000, total, total), limits, now)
                .unwrap()
                .as_deref(),
            Some(text.as_str())
        );
        assert!(open.is_none(), "the message is done");

        let mut open = None;
        take_part(&mut open, part(1, 0, 4_000, total), limits, now).unwrap();
        assert!(
            matches!(
                take_part(&mut open, part(1, 5_000, 8_000, total), limits, now),
                Err(FrameError::Part(_))
            ),
            "a part that leaves a hole"
        );
        let mut open = None;
        take_part(&mut open, part(1, 0, 4_000, total), limits, now).unwrap();
        assert!(
            matches!(
                take_part(&mut open, part(2, 0, 4_000, total), limits, now),
                Err(FrameError::Part(_))
            ),
            "a second message begins while one is open"
        );
        let mut open = None;
        take_part(&mut open, part(1, 0, 4_000, total), limits, now).unwrap();
        assert!(
            matches!(
                take_part(&mut open, part(2, 4_000, 8_000, total), limits, now),
                Err(FrameError::Part(_))
            ),
            "a part of another message"
        );
        let mut open = None;
        assert!(
            matches!(
                take_part(&mut open, part(1, 4_000, 8_000, total), limits, now),
                Err(FrameError::Part(_))
            ),
            "a part that does not begin its message"
        );
        let mut open = None;
        let huge = PartFrame {
            paged: false,
            message: 1,
            offset: 0,
            total: u64::try_from(limits.message_bytes() + 1).unwrap(),
            payload: Bytes::from_static(b"{"),
        }
        .encode(limits.frame_bytes())
        .unwrap();
        assert!(
            matches!(
                take_part(&mut open, huge, limits, now),
                Err(FrameError::TooLarge { max, .. }) if max == limits.message_bytes()
            ),
            "a part that announces more than a message may have"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_message_whose_parts_stop_coming_is_given_up_after_the_wait_whatever_else_arrives() {
        let limits = small_frames();
        let (feed, source) = mpsc::unbounded_channel();
        let (inbound, mut read) = mpsc::unbounded_channel();
        tokio::spawn(read_loop(
            Box::new(Fed(source)),
            inbound,
            limits,
            LinkMeter::new(),
            Duration::from_secs(5),
        ));
        let text = serde_json::to_string(&reply(20_000)).unwrap();
        let bytes = Bytes::from(text);
        let total = u64::try_from(bytes.len()).unwrap();
        let begun = Instant::now();
        feed.send(Message::Binary(
            PartFrame {
                paged: false,
                message: 1,
                offset: 0,
                total,
                payload: bytes.slice(..4_000),
            }
            .encode(limits.frame_bytes())
            .unwrap(),
        ))
        .unwrap();
        // Other traffic goes on, and does not make the message any less late.
        tokio::time::sleep(Duration::from_secs(3)).await;
        feed.send(Message::Text(
            frames::encode(&ping(1), limits.frame_bytes()).unwrap(),
        ))
        .unwrap();
        assert!(matches!(read.recv().await, Some(Inbound::Frame(frame)) if *frame == ping(1)));
        let ended = read.recv().await;
        assert!(
            matches!(
                ended,
                Some(Inbound::Broken(FrameError::Unfinished { received: 4_000, total: wanted }))
                    if wanted == total
            ),
            "{ended:?}"
        );
        let waited = begun.elapsed();
        assert!(
            waited >= Duration::from_secs(5) && waited < Duration::from_secs(6),
            "given up five seconds after the last part: {waited:?}"
        );
    }

    #[tokio::test]
    async fn a_reader_hands_on_a_message_put_together_from_parts_as_one_frame() {
        let limits = small_frames();
        let rig = rig(limits);
        let big = reply(50_000);
        rig.out.control(&big).unwrap();
        rig.out.control(&ping(2)).unwrap();
        rig.write_all().await;
        let (feed, source) = mpsc::unbounded_channel();
        let (inbound, mut read) = mpsc::unbounded_channel();
        tokio::spawn(read_loop(
            Box::new(Fed(source)),
            inbound,
            limits,
            LinkMeter::new(),
            Duration::from_secs(60),
        ));
        for message in rig.written() {
            feed.send(message).unwrap();
        }
        assert!(matches!(read.recv().await, Some(Inbound::Frame(frame)) if *frame == ping(2)));
        assert!(matches!(read.recv().await, Some(Inbound::Frame(frame)) if *frame == big));
    }
}
