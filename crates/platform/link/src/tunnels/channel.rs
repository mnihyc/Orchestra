//! Ordered channel lifecycle and local sockets.
//!
//! A tunnel is an ordered byte stream between two machines, or between the
//! system and a machine, and to the program at either end it is a socket: it
//! reads and writes bytes as it would a TCP connection. [`Tunnel`] is that
//! socket, an `AsyncRead` and `AsyncWrite`. Under it lies the state of a
//! channel: what has been written and not yet acknowledged, so that it can be
//! sent again; how much the other end has allowed; what has arrived and not
//! yet been read; and whether the tunnel is open, waiting for a new session,
//! or over.
//!
//! A channel lives longer than a session. When the session drops, a tunnel
//! that can be resumed waits for a minute with its local socket open; the end
//! that opened it asks, on the new session, to carry it on, each end says how
//! many bytes it has, and each sends again from there. If the minute passes,
//! or an end no longer holds what the other needs, the tunnel is closed with
//! a reset and the program sees its socket reset.
//!
//! [`TunnelHub`] is a machine's tunnels: opening one with a ticket, taking
//! up those that others open to it, and following them across sessions. The
//! frames themselves go through a [`Wire`], which is a session for a machine
//! and the backplane for the system's end of a terminal.

use crate::framing::SessionOver;
use crate::ports::{Datagram, UdpBudget};
use crate::session::SessionSender;
use crate::tunnels::flow::{Gauge, credit_due};
use crate::tunnels::replay::ReplayBuffer;
use bytes::Bytes;
use orch_api_contract::ports::{PortConnectionInside, PortTransport};
use orch_link_contract::frames::{DataFrame, Frame};
use orch_link_contract::ports::{PortDatagram, PortTicketClaims};
use orch_link_contract::tunnels::{
    ChAck, Close, CloseCode, Credit, Eof, OPEN_TIMEOUT_SECS, Open, OpenErr, OpenErrorCode, OpenOk,
    OpenRequest, PtyExit, PtyResize, REPLAY_BUFFER_BYTES, RESUME_GRACE_SECS, RecvWindow, Resume,
    ResumeOk, SendWindow, Side, Ticket, TicketClaims, TicketSource, TunnelKind,
};
use orch_link_contract::version::PortLimits;
use orch_types::{Id, new_id};
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How often the hub looks at its tunnels for acknowledgements that are due
/// and graces that ran out: a quarter of the second an acknowledgement may
/// wait.
const TICK: Duration = Duration::from_millis(250);
/// The most a single write to a tunnel takes in: a window's worth is as much
/// as can be on its way, and a bounded take keeps one writer from holding
/// the lock of a channel for long.
const MAX_WRITE_BYTES: usize = 1024 * 1024;
/// How long a machine waits for the answer to an opening beyond the time the
/// gateway itself allows, so that the gateway's own answer is the one that
/// arrives.
const OPEN_MARGIN: Duration = Duration::from_secs(2);

/// What the frames of a channel are sent through.
pub trait Wire: Send + Sync {
    /// Sends a control frame of a channel, which goes before any data.
    fn control(&self, channel: u32, frame: ChannelFrame) -> bool;
    /// Sends data of a channel. `false` when the wire is gone.
    fn data(&self, channel: u32, offset: u64, payload: Bytes) -> bool;
    /// How many bytes of payload the next data frame should carry.
    fn data_frame_bytes(&self) -> usize;
    /// Emits one leased complete UDP packet; unsupported wires refuse it.
    fn datagram(&self, _packet: Datagram) -> bool {
        false
    }
}

/// A control frame of a channel, without its channel number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelFrame {
    /// More credit for the other end's sending.
    Credit(u64),
    /// This end sends nothing more.
    Eof,
    /// The channel is over.
    Close {
        /// How it ended.
        code: CloseCode,
        /// A sentence for the log.
        detail: String,
    },
    /// This end has received this many bytes in all.
    Ack(u64),
    /// A terminal's new size.
    PtyResize {
        /// The width in characters.
        cols: u16,
        /// The height in lines.
        rows: u16,
    },
    /// The process in the terminal ended.
    PtyExit {
        /// What it exited with.
        code: i64,
    },
}

/// A session as the wire of the channels on it.
#[derive(Debug, Clone)]
pub struct SessionWire(pub SessionSender);

impl Wire for SessionWire {
    fn datagram(&self, packet: Datagram) -> bool {
        self.0.send_datagram(packet).is_ok()
    }
    fn control(&self, channel: u32, frame: ChannelFrame) -> bool {
        // What ends a stream goes in the queue of the data, so that it comes
        // after every byte sent before it; a control frame would overtake them.
        let ordered = matches!(
            frame,
            ChannelFrame::Eof
                | ChannelFrame::Close {
                    code: CloseCode::Ok,
                    ..
                }
        );
        let frame = match frame {
            ChannelFrame::Credit(n) => Frame::Credit(Credit { channel, n }),
            ChannelFrame::Eof => Frame::Eof(Eof { channel }),
            ChannelFrame::Close { code, detail } => Frame::Close(Close {
                channel,
                code,
                detail,
            }),
            ChannelFrame::Ack(offset) => Frame::ChAck(ChAck { channel, offset }),
            ChannelFrame::PtyResize { cols, rows } => Frame::PtyResize(PtyResize {
                channel,
                cols,
                rows,
            }),
            ChannelFrame::PtyExit { code } => Frame::PtyExit(PtyExit { channel, code }),
        };
        if ordered {
            self.0.send_ordered(frame).is_ok()
        } else {
            self.0.send(frame).is_ok()
        }
    }

    fn data(&self, channel: u32, offset: u64, payload: Bytes) -> bool {
        self.0
            .send_data(&DataFrame {
                channel,
                offset,
                payload,
            })
            .is_ok()
    }

    fn data_frame_bytes(&self) -> usize {
        self.0.data_frame_bytes()
    }
}

/// What the program at the end of a tunnel is told besides bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelEvent {
    /// The terminal's new size.
    Resize {
        /// The width in characters.
        cols: u16,
        /// The height in lines.
        rows: u16,
    },
    /// The process in the terminal ended.
    Exit {
        /// What it exited with.
        code: i64,
    },
}

/// How a closed channel ended, for the program that reads and writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ended {
    /// As it should.
    Cleanly,
    /// Something failed.
    Failed(String),
    /// It could not be resumed.
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Asked for, not answered.
    Opening,
    /// Open.
    Open,
    /// The session is gone; waiting for a new one since then.
    Suspended(Instant),
    /// Over.
    Closed,
}

/// What a channel is made with.
#[derive(Debug, Clone, Copy)]
pub struct ChannelOptions {
    /// The credit each end starts with.
    pub window_bytes: u64,
    /// How many unacknowledged bytes are kept for a resumption.
    pub replay_bytes: u64,
    /// Whether the tunnel can be taken up again on a new session. A shell
    /// is attached anew each time, and so is not.
    pub resumable: bool,
}

impl ChannelOptions {
    /// Applies the smaller common credit/replay bounds of the session's port limits to an
    /// admitted TCP port.
    pub(crate) fn for_port(mut self, port: Option<&PortTicketClaims>, limits: &PortLimits) -> Self {
        if let Some(port) = port {
            let buffer = u64::from(limits.buffer_bytes);
            self.window_bytes = self.window_bytes.min(buffer);
            self.replay_bytes = self.replay_bytes.min(buffer);
            // TCP retains the ordinary authorized-use replay grace; datagrams end with their session.
            self.resumable &= port.transport == PortTransport::Tcp;
        }
        self
    }
}

struct SendSide {
    window: SendWindow,
    buffer: ReplayBuffer,
    eof_wanted: bool,
    eof_sent: bool,
}

struct RecvSide {
    window: RecvWindow,
    queue: std::collections::VecDeque<Bytes>,
    eof: bool,
    credit_owed: u64,
    last_ack: Instant,
}

/// The state of one channel.
pub(crate) struct Channel {
    cid: Id,
    kind: TunnelKind,
    service: String,
    opener: bool,
    ticket: Option<String>,
    number: Option<u32>,
    wire: Option<Arc<dyn Wire>>,
    options: ChannelOptions,
    phase: Phase,
    ended: Option<Ended>,
    send: SendSide,
    recv: RecvSide,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
    gauge: Option<Arc<Gauge>>,
    events: mpsc::UnboundedSender<TunnelEvent>,
    closed_signal: Option<oneshot::Sender<()>>,
    /// The program let go of the tunnel: what it wrote is still to be
    /// delivered, and the tunnel closes when that is done.
    lingering: bool,
    port: Option<PortTicketClaims>,
    inside: Option<PortConnectionInside>,
    datagrams: std::collections::VecDeque<Datagram>,
    udp: Option<Arc<UdpBudget>>,
}

impl Channel {
    pub(crate) fn is_port(&self) -> bool {
        self.port.is_some()
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }
    pub(crate) fn belongs_to_opening(&self, opening: Id) -> bool {
        self.port
            .as_ref()
            .is_some_and(|port| port.opening_id == Some(opening))
    }
    pub(crate) fn belongs_to_environment(&self, environment: Id) -> bool {
        self.port
            .as_ref()
            .is_some_and(|port| port.environment_id == environment)
    }
    pub(crate) fn set_inside(&mut self, inside: Option<PortConnectionInside>) {
        self.inside = inside;
    }
    pub(crate) fn configure_port(
        &mut self,
        port: Option<PortTicketClaims>,
        inside: Option<PortConnectionInside>,
        udp: Arc<UdpBudget>,
    ) {
        if port
            .as_ref()
            .is_some_and(|p| p.transport == PortTransport::Udp)
        {
            self.udp = Some(udp);
        }
        self.port = port;
        self.inside = inside;
    }

    pub(crate) fn on_datagram(&mut self, packet: Datagram) {
        if self.phase != Phase::Open {
            return;
        }
        if let Some(budget) = &self.udp
            && let Some(packet) = budget.hold(packet.delivered())
        {
            self.datagrams.push_back(packet);
            if let Some(waker) = self.read_waker.take() {
                waker.wake();
            }
        }
    }
    /// A channel, and what its program is told besides bytes.
    pub(crate) fn make(
        kind: TunnelKind,
        service: String,
        opener: bool,
        options: ChannelOptions,
        gauge: Option<Arc<Gauge>>,
    ) -> (Arc<Mutex<Self>>, mpsc::UnboundedReceiver<TunnelEvent>) {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let channel = Self::new(new_id(), kind, service, opener, options, gauge, events_tx);
        (Arc::new(Mutex::new(channel)), events_rx)
    }

    /// The channel is open as `number` over `wire`.
    pub(crate) fn attach(&mut self, wire: Arc<dyn Wire>, number: u32, cid: Id) {
        self.wire = Some(wire);
        self.number = Some(number);
        self.cid = cid;
        self.phase = Phase::Open;
        self.wake_all();
    }

    /// What the program is told besides bytes.
    pub(crate) fn tell(&self, event: TunnelEvent) {
        let _ = self.events.send(event);
    }

    fn new(
        cid: Id,
        kind: TunnelKind,
        service: String,
        opener: bool,
        options: ChannelOptions,
        gauge: Option<Arc<Gauge>>,
        events: mpsc::UnboundedSender<TunnelEvent>,
    ) -> Self {
        Self {
            cid,
            kind,
            service,
            opener,
            ticket: None,
            number: None,
            wire: None,
            options,
            phase: Phase::Opening,
            ended: None,
            send: SendSide {
                window: SendWindow::new(options.window_bytes, options.replay_bytes),
                buffer: ReplayBuffer::new(),
                eof_wanted: false,
                eof_sent: false,
            },
            recv: RecvSide {
                window: RecvWindow::new(options.window_bytes),
                queue: std::collections::VecDeque::new(),
                eof: false,
                credit_owed: 0,
                last_ack: Instant::now(),
            },
            read_waker: None,
            write_waker: None,
            gauge,
            events,
            closed_signal: None,
            lingering: false,
            port: None,
            inside: None,
            datagrams: std::collections::VecDeque::new(),
            udp: None,
        }
    }

    /// The program lets go of the tunnel. What it wrote is delivered first,
    /// as a socket delivers what was written before it is closed; what the
    /// other end sends from now on is taken and dropped, so that it is not
    /// held up. The tunnel is closed when the other end has everything.
    pub(crate) fn linger_close(&mut self) {
        if self.phase == Phase::Closed {
            return;
        }
        self.lingering = true;
        self.send.eof_wanted = true;
        self.read_waker = None;
        self.write_waker = None;
        // Whatever arrived and was not read will not be.
        let unread: u64 = self.recv.queue.iter().map(|chunk| chunk.len() as u64).sum();
        self.recv.queue.clear();
        if unread > 0 && self.recv.window.deliver(unread).is_ok() {
            self.held_gauge(0, unread);
            self.give_credit();
        }
        self.finish_lingering();
    }

    fn finish_lingering(&mut self) {
        let delivered =
            self.send.window.offset() >= self.send.buffer.end() && self.send.buffer.is_empty();
        let nothing_to_wait_for = delivered || self.phase == Phase::Opening;
        if self.lingering && nothing_to_wait_for {
            self.close_with(Ended::Cleanly, true);
        }
    }

    fn wake_all(&mut self) {
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
        if let Some(waker) = self.write_waker.take() {
            waker.wake();
        }
    }

    fn held_gauge(&self, add: u64, sub: u64) {
        if let Some(gauge) = &self.gauge {
            gauge.add(add);
            gauge.sub(sub);
        }
    }

    /// Sends as much of what was written as the credit and the wire allow.
    fn pump(&mut self) {
        let (Some(wire), Some(number)) = (self.wire.clone(), self.number) else {
            return;
        };
        if self.phase != Phase::Open {
            return;
        }
        loop {
            let pending = self
                .send
                .buffer
                .end()
                .saturating_sub(self.send.window.offset());
            if pending == 0 {
                break;
            }
            let frame_bytes = u64::try_from(wire.data_frame_bytes()).unwrap_or(u64::MAX);
            let n = pending.min(self.send.window.sendable()).min(frame_bytes);
            if n == 0 {
                break;
            }
            let offset = self.send.window.offset();
            let Some(payload) = self
                .send
                .buffer
                .slice(offset, usize::try_from(n).unwrap_or(usize::MAX))
            else {
                break;
            };
            let sent = payload.len() as u64;
            if self.send.window.send(sent).is_err() {
                break;
            }
            if !wire.data(number, offset, payload) {
                break;
            }
            if !self.options.resumable {
                // Nothing can be sent again, so nothing is kept.
                let _ = self.send.window.acknowledge(offset + sent);
                let freed = self.send.buffer.release_to(offset + sent);
                self.held_gauge(0, freed);
            }
        }
        if self.send.eof_wanted
            && !self.send.eof_sent
            && self.send.window.offset() >= self.send.buffer.end()
            && wire.control(number, ChannelFrame::Eof)
        {
            self.send.eof_sent = true;
        }
        // Room may have opened for a writer.
        if let Some(waker) = self.write_waker.take() {
            waker.wake();
        }
        self.finish_lingering();
    }

    pub(crate) fn on_credit(&mut self, more: u64) {
        if self.send.window.credit(more).is_err() {
            self.close_with(Ended::Failed("credit overflowed".to_owned()), true);
            return;
        }
        self.pump();
    }

    pub(crate) fn on_ack(&mut self, offset: u64) {
        match self.send.window.acknowledge(offset) {
            Ok(_) => {
                let freed = self.send.buffer.release_to(offset);
                self.held_gauge(0, freed);
                self.pump();
            }
            Err(_) => self.close_with(
                Ended::Failed("the other end acknowledged bytes that were not sent".to_owned()),
                true,
            ),
        }
    }

    pub(crate) fn on_data(&mut self, offset: u64, payload: Bytes) {
        if self.udp.is_some() {
            self.close_with(
                Ended::Failed("UDP requires one kind3 datagram frame".into()),
                true,
            );
            return;
        }
        if self.phase == Phase::Closed {
            return;
        }
        match self.recv.window.accept(offset, payload.len() as u64) {
            Ok(accepted) => {
                let skip = usize::try_from(accepted.skip).unwrap_or(usize::MAX);
                if skip < payload.len() {
                    let fresh = payload.slice(skip..);
                    if self.lingering {
                        // Nobody will read it; it is taken so that the other
                        // end is not held up.
                        let taken = fresh.len() as u64;
                        if self.recv.window.deliver(taken).is_ok() {
                            self.give_credit();
                        }
                    } else {
                        self.held_gauge(fresh.len() as u64, 0);
                        self.recv.queue.push_back(fresh);
                    }
                }
                if let Some(waker) = self.read_waker.take() {
                    waker.wake();
                }
                if self.options.resumable && self.recv.window.ack_due(Duration::ZERO) {
                    self.maybe_ack(true);
                }
            }
            Err(error) => self.close_with(Ended::Failed(error.to_string()), true),
        }
    }

    /// Sends an acknowledgement if one is due; `by_bytes_only` skips the one
    /// that is due by time.
    fn maybe_ack(&mut self, by_bytes_only: bool) {
        let (Some(wire), Some(number)) = (self.wire.clone(), self.number) else {
            return;
        };
        if !self.options.resumable || self.phase != Phase::Open {
            return;
        }
        let since = if by_bytes_only {
            Duration::ZERO
        } else {
            self.recv.last_ack.elapsed()
        };
        let received = self.recv.window.received();
        if self.recv.window.ack_due(since) && received > 0 {
            let offset = self.recv.window.take_ack();
            self.recv.last_ack = Instant::now();
            let _ = wire.control(number, ChannelFrame::Ack(offset));
        }
    }

    /// Gives credit back for what the program has read.
    fn give_credit(&mut self) {
        self.recv.credit_owed += self.recv.window.take_credit();
        let drained = self.recv.queue.is_empty();
        if credit_due(self.recv.credit_owed, self.options.window_bytes, drained)
            && let (Some(wire), Some(number)) = (self.wire.clone(), self.number)
            && self.phase == Phase::Open
            && wire.control(number, ChannelFrame::Credit(self.recv.credit_owed))
        {
            self.recv.credit_owed = 0;
        }
    }

    pub(crate) fn on_eof(&mut self) {
        self.recv.eof = true;
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
    }

    /// The channel is over. `tell` says the other end is to be told.
    pub(crate) fn close_with(&mut self, how: Ended, tell: bool) {
        self.datagrams.clear();
        if self.phase == Phase::Closed {
            return;
        }
        if tell && let (Some(wire), Some(number)) = (self.wire.clone(), self.number) {
            let (code, detail) = match &how {
                Ended::Cleanly => (CloseCode::Ok, String::new()),
                Ended::Failed(why) => (CloseCode::Error, why.clone()),
                Ended::Reset => (CloseCode::Reset, String::new()),
            };
            let _ = wire.control(number, ChannelFrame::Close { code, detail });
        }
        self.phase = Phase::Closed;
        self.ended = Some(how);
        self.wire = None;
        let held = self.send.buffer.len();
        self.send.buffer.release_to(self.send.buffer.end());
        self.held_gauge(0, held);
        if let Some(signal) = self.closed_signal.take() {
            let _ = signal.send(());
        }
        self.wake_all();
    }

    pub(crate) fn on_peer_close(&mut self, code: CloseCode, detail: &str) {
        let how = match code {
            CloseCode::Ok => Ended::Cleanly,
            CloseCode::Error => Ended::Failed(detail.to_owned()),
            CloseCode::Reset => Ended::Reset,
        };
        self.close_with(how, false);
    }

    /// The session is gone: the tunnel waits for a new one, or, if it cannot
    /// be resumed, is over.
    fn suspend(&mut self, now: Instant) {
        match self.phase {
            Phase::Closed | Phase::Suspended(_) => {}
            Phase::Open if self.options.resumable => {
                self.phase = Phase::Suspended(now);
                self.wire = None;
                self.number = None;
            }
            Phase::Open | Phase::Opening => {
                self.wire = None;
                self.close_with(Ended::Reset, false);
            }
        }
    }

    /// The tunnel goes on over `wire` as channel `number`, the other end
    /// having received `peer_received` bytes. Returns the number of bytes this
    /// end has received, for the other end to start from.
    fn resume(
        &mut self,
        wire: Arc<dyn Wire>,
        number: u32,
        peer_received: u64,
    ) -> Result<u64, Ended> {
        let window = self.options.window_bytes;
        let from = self
            .send
            .window
            .resume(peer_received, window)
            .map_err(|_| Ended::Reset)?;
        let freed = self.send.buffer.release_to(from);
        self.held_gauge(0, freed);
        let have = self.recv.window.resume(window);
        self.recv.credit_owed = 0;
        self.send.eof_sent = false;
        self.wire = Some(wire);
        self.number = Some(number);
        self.phase = Phase::Open;
        Ok(have)
    }

    fn start_pumping_after_resume(&mut self) {
        self.pump();
        self.maybe_ack(false);
    }
}

/// The program's end of a tunnel: a socket.
pub struct Tunnel {
    channel: Arc<Mutex<Channel>>,
    events: Option<mpsc::UnboundedReceiver<TunnelEvent>>,
    closed: Option<oneshot::Receiver<()>>,
}

/// A retirement handle that holds no socket owner alive.
#[derive(Clone)]
pub struct TunnelCloser {
    channel: Weak<Mutex<Channel>>,
}
impl std::fmt::Debug for TunnelCloser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelCloser").finish_non_exhaustive()
    }
}
impl TunnelCloser {
    /// Ends both directions immediately, independently of an idle consumer.
    pub fn close(&self) {
        if let Some(channel) = self.channel.upgrade() {
            lock(&channel).close_with(Ended::Reset, true);
        }
    }
    /// Whether no current channel remains behind this handle.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.channel
            .upgrade()
            .is_none_or(|channel| lock(&channel).phase == Phase::Closed)
    }
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let channel = lock(&self.channel);
        f.debug_struct("Tunnel")
            .field("cid", &channel.cid)
            .field("service", &channel.service)
            .finish_non_exhaustive()
    }
}

impl Tunnel {
    /// A weak retirement handle for current-authority and lifecycle reconciliation.
    #[must_use]
    pub fn closer(&self) -> TunnelCloser {
        TunnelCloser {
            channel: Arc::downgrade(&self.channel),
        }
    }
    /// The actually reported peer, present only after a port's inside socket opened.
    #[must_use]
    pub fn inside_peer(&self) -> Option<PortConnectionInside> {
        lock(&self.channel).inside.clone()
    }

    /// The association's shared environment store and live numeric losses.
    #[must_use]
    pub fn datagram_budget(&self) -> Option<Arc<UdpBudget>> {
        lock(&self.channel).udp.clone()
    }

    /// Emits one packet immediately or returns false when the bounded UDP store dropped it.
    pub fn send_datagram(&self, payload: Bytes) -> io::Result<bool> {
        let channel = lock(&self.channel);
        if channel.phase != Phase::Open {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the association is closed",
            ));
        }
        let budget = channel.udp.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "this channel carries TCP bytes",
            )
        })?;
        let number = channel
            .number
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "no current session"))?;
        let Some(packet) = budget.hold(PortDatagram {
            channel: number,
            payload,
        }) else {
            return Ok(false);
        };
        let wire = channel
            .wire
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "no current wire"))?;
        if wire.datagram(packet) {
            Ok(true)
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the link is closed",
            ))
        }
    }

    /// Emits an already reserved packet without releasing and reacquiring its shared byte budget.
    pub fn send_reserved_datagram(&self, mut packet: Datagram) -> io::Result<bool> {
        let channel = lock(&self.channel);
        if channel.phase != Phase::Open {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the association is closed",
            ));
        }
        let budget = channel.udp.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "this channel carries TCP bytes",
            )
        })?;
        if !packet.belongs_to(budget) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the packet belongs to another environment",
            ));
        }
        packet.frame.channel = channel
            .number
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "no current session"))?;
        let wire = channel
            .wire
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "no current wire"))?;
        if wire.datagram(packet) {
            Ok(true)
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the link is closed",
            ))
        }
    }

    /// Receives exactly one packet, preserving an empty datagram and never replaying it.
    pub async fn recv_datagram(&self) -> io::Result<Bytes> {
        std::future::poll_fn(|cx| {
            let mut channel = lock(&self.channel);
            if channel.udp.is_none() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "this channel carries TCP bytes",
                )));
            }
            if let Some(packet) = channel.datagrams.pop_front() {
                return Poll::Ready(Ok(packet.delivered().payload));
            }
            if channel.phase == Phase::Closed {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "the association ended",
                )));
            }
            channel.read_waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }
    pub(crate) fn new(
        channel: Arc<Mutex<Channel>>,
        events: mpsc::UnboundedReceiver<TunnelEvent>,
    ) -> Self {
        let (closed_tx, closed_rx) = oneshot::channel();
        lock(&channel).closed_signal = Some(closed_tx);
        Self {
            channel,
            events: Some(events),
            closed: Some(closed_rx),
        }
    }

    /// The name the tunnel keeps across sessions.
    #[must_use]
    pub fn cid(&self) -> Id {
        lock(&self.channel).cid
    }

    /// What the tunnel carries.
    #[must_use]
    pub fn kind(&self) -> TunnelKind {
        lock(&self.channel).kind
    }

    /// The service it ends at.
    #[must_use]
    pub fn service(&self) -> String {
        lock(&self.channel).service.clone()
    }

    /// Whether the tunnel is over.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        lock(&self.channel).phase == Phase::Closed
    }

    /// Whether the tunnel is waiting for a new session.
    #[must_use]
    pub fn is_suspended(&self) -> bool {
        matches!(lock(&self.channel).phase, Phase::Suspended(_))
    }

    /// What the program is told besides bytes: a terminal's size, the end of
    /// its process. Taken once.
    pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<TunnelEvent>> {
        self.events.take()
    }

    /// Waits until the tunnel is over.
    pub async fn closed(&mut self) {
        if let Some(signal) = self.closed.take() {
            let _ = signal.await;
        }
    }

    /// Tells the other end of a terminal its new size.
    pub fn resize(&self, cols: u16, rows: u16) {
        let channel = lock(&self.channel);
        if let (Some(wire), Some(number)) = (channel.wire.clone(), channel.number) {
            let _ = wire.control(number, ChannelFrame::PtyResize { cols, rows });
        }
    }

    /// Tells the other end that the process in the terminal ended.
    pub fn exited(&self, code: i64) {
        let channel = lock(&self.channel);
        if let (Some(wire), Some(number)) = (channel.wire.clone(), channel.number) {
            let _ = wire.control(number, ChannelFrame::PtyExit { code });
        }
    }

    /// Lets go of the tunnel: what was written is delivered, then the other
    /// end is told that the tunnel ended as it should.
    pub fn close(&self) {
        lock(&self.channel).linger_close();
    }

    /// Ends the tunnel with a failure.
    pub fn fail(&self, why: &str) {
        lock(&self.channel).close_with(Ended::Failed(why.to_owned()), true);
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        lock(&self.channel).linger_close();
    }
}

fn ended_error(ended: &Ended) -> io::Error {
    match ended {
        Ended::Cleanly => io::Error::new(io::ErrorKind::BrokenPipe, "the tunnel is closed"),
        Ended::Failed(why) => io::Error::new(io::ErrorKind::ConnectionAborted, why.clone()),
        Ended::Reset => io::Error::new(io::ErrorKind::ConnectionReset, "the tunnel was reset"),
    }
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut channel = lock(&self.channel);
        if channel.recv.queue.is_empty() {
            if channel.udp.is_some() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "UDP is received as whole datagrams",
                )));
            }
            return match (&channel.ended, channel.recv.eof) {
                (Some(Ended::Cleanly), _) | (None, true) => Poll::Ready(Ok(())),
                (Some(ended), _) => Poll::Ready(Err(ended_error(ended))),
                (None, false) => {
                    channel.read_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            };
        }
        let mut taken = 0usize;
        while buf.remaining() > 0 {
            let Some(front) = channel.recv.queue.front_mut() else {
                break;
            };
            let n = front.len().min(buf.remaining());
            buf.put_slice(&front[..n]);
            *front = front.slice(n..);
            if front.is_empty() {
                channel.recv.queue.pop_front();
            }
            taken += n;
        }
        if channel.recv.window.deliver(taken as u64).is_ok() {
            channel.held_gauge(0, taken as u64);
            channel.give_credit();
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut channel = lock(&self.channel);
        if let Some(ended) = &channel.ended {
            return Poll::Ready(Err(ended_error(ended)));
        }
        if channel.udp.is_some() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "UDP is sent as whole datagrams",
            )));
        }
        if channel.send.eof_wanted {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the tunnel was shut down for writing",
            )));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let kept = channel.send.buffer.len();
        let room = channel.options.replay_bytes.saturating_sub(kept);
        let n = usize::try_from(room)
            .unwrap_or(usize::MAX)
            .min(data.len())
            .min(MAX_WRITE_BYTES);
        if n == 0 || channel.phase == Phase::Opening {
            channel.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        channel.send.buffer.push(Bytes::copy_from_slice(&data[..n]));
        channel.held_gauge(n as u64, 0);
        channel.pump();
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut channel = lock(&self.channel);
        if channel.ended.is_some() {
            return Poll::Ready(Ok(()));
        }
        channel.send.eof_wanted = true;
        channel.pump();
        Poll::Ready(Ok(()))
    }
}

/// Why a tunnel could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// No session stands.
    #[error("no session stands")]
    NoSession,
    /// The gateway or the other end refused.
    #[error("the tunnel was refused ({code:?}): {detail}")]
    Refused {
        /// Why.
        code: OpenErrorCode,
        /// A sentence for the log.
        detail: String,
    },
    /// Nobody answered in time.
    #[error("nobody answered the opening in time")]
    TimedOut,
    /// The session ended before the tunnel was open.
    #[error("the session ended before the tunnel was open")]
    Interrupted,
    /// A session has as many channels as it may.
    #[error("the session has as many channels as it may")]
    TooMany,
}

/// A tunnel that another machine, or the system, asks to open here.
pub struct IncomingTunnel {
    /// What the tunnel carries.
    pub kind: TunnelKind,
    /// The service it is to end at.
    pub service: String,
    /// Who opens it.
    pub from: TicketSource,
    /// What its ticket allows.
    pub claims: TicketClaims,
    number: u32,
    hub: Weak<HubShared>,
    answered: bool,
}

impl std::fmt::Debug for IncomingTunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingTunnel")
            .field("service", &self.service)
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

impl IncomingTunnel {
    /// The port bounds of the session this tunnel came on, as its gateway stated them; the
    /// shipped ones once that session is gone.
    #[must_use]
    pub fn port_limits(&self) -> PortLimits {
        self.hub
            .upgrade()
            .and_then(|hub| {
                lock(&hub.state)
                    .session
                    .as_ref()
                    .map(|(session, _)| session.limits().ports)
            })
            .unwrap_or_default()
    }

    /// Takes the tunnel up: the other end is told it is open, and the tunnel
    /// is returned as a socket.
    pub fn accept(mut self) -> Result<Tunnel, OpenError> {
        self.answered = true;
        let hub = self.hub.upgrade().ok_or(OpenError::NoSession)?;
        hub.accept(
            self.number,
            self.kind,
            &self.service,
            self.claims.port.clone(),
            None,
        )
    }

    /// Accepts a port only after its actual inside socket peer was established.
    pub fn accept_port(mut self, inside: PortConnectionInside) -> Result<Tunnel, OpenError> {
        self.answered = true;
        let hub = self.hub.upgrade().ok_or(OpenError::NoSession)?;
        hub.accept(
            self.number,
            self.kind,
            &self.service,
            self.claims.port.clone(),
            Some(inside),
        )
    }

    /// Refuses the tunnel.
    pub fn refuse(mut self, code: OpenErrorCode, detail: &str) {
        self.answered = true;
        if let Some(hub) = self.hub.upgrade() {
            hub.refuse(self.number, code, detail);
        }
    }
}

impl Drop for IncomingTunnel {
    fn drop(&mut self) {
        if !self.answered
            && let Some(hub) = self.hub.upgrade()
        {
            hub.refuse(
                self.number,
                OpenErrorCode::Refused,
                "nothing took the tunnel up",
            );
        }
    }
}

/// The tunnels others open to this machine.
#[derive(Debug)]
pub struct IncomingTunnels {
    rx: mpsc::UnboundedReceiver<IncomingTunnel>,
}

impl IncomingTunnels {
    /// The next tunnel that is asked for.
    pub async fn next(&mut self) -> Option<IncomingTunnel> {
        self.rx.recv().await
    }
}

/// How a machine's tunnels behave.
#[derive(Debug, Clone, Copy)]
pub struct TunnelOptions {
    /// How long a tunnel waits for a new session.
    pub grace: Duration,
    /// How many unacknowledged bytes are kept for a resumption.
    pub replay_bytes: u64,
}

impl Default for TunnelOptions {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(u64::from(RESUME_GRACE_SECS)),
            replay_bytes: REPLAY_BUFFER_BYTES,
        }
    }
}

struct PendingOpen {
    channel: Arc<Mutex<Channel>>,
    answer: oneshot::Sender<Result<Id, OpenError>>,
}

struct HubState {
    port_available: bool,
    udp_budgets: HashMap<Id, Weak<UdpBudget>>,
    session: Option<(SessionSender, Arc<dyn Wire>)>,
    channels: HashMap<u32, Arc<Mutex<Channel>>>,
    tunnels: HashMap<Id, Arc<Mutex<Channel>>>,
    pending: HashMap<u32, PendingOpen>,
    next_number: u32,
    incoming_tx: Option<mpsc::UnboundedSender<IncomingTunnel>>,
    incoming_rx: Option<mpsc::UnboundedReceiver<IncomingTunnel>>,
}

pub(crate) struct HubShared {
    port_control: tokio::sync::watch::Sender<bool>,
    options: TunnelOptions,
    gauge: Option<Arc<Gauge>>,
    state: Mutex<HubState>,
    weak: Weak<HubShared>,
}

/// A machine's tunnels.
#[derive(Clone)]
pub struct TunnelHub {
    shared: Arc<HubShared>,
}

struct OpeningGuard {
    hub: Arc<HubShared>,
    channel: Arc<Mutex<Channel>>,
    number: u32,
    complete: bool,
}
impl Drop for OpeningGuard {
    fn drop(&mut self) {
        if !self.complete {
            lock(&self.channel).close_with(Ended::Reset, true);
            self.hub.forget(self.number);
        }
    }
}

impl Default for TunnelHub {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TunnelHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelHub").finish_non_exhaustive()
    }
}

impl TunnelHub {
    /// A hub with no tunnels. Must be called inside a Tokio runtime.
    #[must_use]
    pub fn new() -> Self {
        Self::with_options(TunnelOptions::default(), None)
    }

    /// A hub with other options, and a gauge of the bytes its tunnels hold.
    #[must_use]
    pub fn with_options(options: TunnelOptions, gauge: Option<Arc<Gauge>>) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        let shared = Arc::new_cyclic(|weak| HubShared {
            port_control: tokio::sync::watch::channel(false).0,
            options,
            gauge,
            state: Mutex::new(HubState {
                port_available: false,
                udp_budgets: HashMap::new(),
                session: None,
                channels: HashMap::new(),
                tunnels: HashMap::new(),
                pending: HashMap::new(),
                next_number: Side::Machine.first(),
                incoming_tx: Some(incoming_tx),
                incoming_rx: Some(incoming_rx),
            }),
            weak: weak.clone(),
        });
        let ticking = Arc::downgrade(&shared);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(TICK);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let Some(shared) = ticking.upgrade() else {
                    return;
                };
                shared.tick(Instant::now());
            }
        });
        Self { shared }
    }

    /// The tunnels others open to this machine. Taken once.
    #[must_use]
    pub fn incoming(&self) -> IncomingTunnels {
        let rx = lock(&self.shared.state)
            .incoming_rx
            .take()
            .unwrap_or_else(|| mpsc::unbounded_channel().1);
        IncomingTunnels { rx }
    }

    /// A session stands: tunnels that waited are asked to be taken up
    /// again.
    pub fn session_up(&self, sender: SessionSender) {
        self.shared.session_up(sender);
    }

    /// The session is gone.
    pub fn session_down(&self) {
        self.shared.session_down(Instant::now());
    }

    /// A text frame of a channel.
    pub fn on_frame(&self, frame: Frame) {
        self.shared.on_frame(frame);
    }

    /// The admitted gateway's port control signal, including loss on disconnect.
    #[must_use]
    pub fn port_control(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shared.port_control.subscribe()
    }

    /// A frame of tunnel data.
    pub fn on_data(&self, data: DataFrame) {
        let channel = lock(&self.shared.state)
            .channels
            .get(&data.channel)
            .cloned();
        if let Some(channel) = channel {
            lock(&channel).on_data(data.offset, data.payload);
        }
    }

    /// Delivers a leased complete datagram only to its already admitted UDP channel.
    pub fn on_datagram(&self, packet: Datagram) {
        let channel = lock(&self.shared.state)
            .channels
            .get(&packet.frame.channel)
            .cloned();
        if let Some(channel) = channel {
            lock(&channel).on_datagram(packet);
        }
    }

    /// Opens a tunnel with a ticket the system signed.
    pub async fn open(&self, ticket: &str) -> Result<Tunnel, OpenError> {
        self.open_with_host_peer(ticket, None).await
    }

    /// Opens one raw host socket under the node's signed, transport-specific publication template.
    pub async fn open_host_port(
        &self,
        ticket: &str,
        peer: orch_link_contract::ports::HostPortPeer,
    ) -> Result<Tunnel, OpenError> {
        self.open_with_host_peer(ticket, Some(peer)).await
    }

    /// Current negotiated environment bounds, or the shipped bounds before a session stands.
    #[must_use]
    pub fn port_limits(&self) -> PortLimits {
        lock(&self.shared.state)
            .session
            .as_ref()
            .map_or(PortLimits::SHIPPED, |(session, _)| session.limits().ports)
    }

    /// Reserves a host sender's packet in the very store its future tunnel will use.
    pub fn reserve_port_datagram(&self, environment: Id, payload: Bytes) -> Option<Datagram> {
        let budget = HubShared::udp_budget_for(&mut lock(&self.shared.state), Some(environment));
        budget.hold(PortDatagram {
            channel: 1,
            payload,
        })
    }

    async fn open_with_host_peer(
        &self,
        ticket: &str,
        host_peer: Option<orch_link_contract::ports::HostPortPeer>,
    ) -> Result<Tunnel, OpenError> {
        // The signature is for the gateway to check; the claims say what the
        // tunnel is.
        let claims = Ticket::parse(ticket)
            .map_err(|error| OpenError::Refused {
                code: OpenErrorCode::BadTicket,
                detail: error.to_string(),
            })?
            .claims()
            .clone();
        let (answer_tx, answer_rx) = oneshot::channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (number, channel, wire) = {
            let mut state = lock(&self.shared.state);
            if claims.kind == TunnelKind::Port && !state.port_available {
                return Err(OpenError::NoSession);
            }
            let Some((session, wire)) = state.session.clone() else {
                return Err(OpenError::NoSession);
            };
            let limits = session.limits();
            if state.channels.len() >= limits.max_channels as usize {
                return Err(OpenError::TooMany);
            }
            let mut number = state.next_number;
            while state.channels.contains_key(&number) || number == 0 {
                number = Side::Machine.next(number);
            }
            state.next_number = Side::Machine.next(number);
            let options = ChannelOptions {
                window_bytes: u64::from(limits.tunnel_window_bytes),
                replay_bytes: self.shared.options.replay_bytes,
                resumable: !matches!(
                    claims.kind,
                    TunnelKind::Pty | TunnelKind::Browser | TunnelKind::Rfb
                ),
            }
            .for_port(claims.port.as_ref(), &limits.ports);
            let mut channel = Channel::new(
                new_id(),
                claims.kind,
                claims.service.clone(),
                true,
                options,
                self.shared.gauge.clone(),
                events_tx,
            );
            channel.ticket = Some(ticket.to_owned());
            let budget = HubShared::udp_budget(&mut state, claims.port.as_ref());
            channel.configure_port(claims.port.clone(), None, budget);
            channel.number = Some(number);
            channel.wire = Some(Arc::clone(&wire));
            let channel = Arc::new(Mutex::new(channel));
            state.channels.insert(number, Arc::clone(&channel));
            state.pending.insert(
                number,
                PendingOpen {
                    channel: Arc::clone(&channel),
                    answer: answer_tx,
                },
            );
            (number, channel, session)
        };
        let mut opening = OpeningGuard {
            hub: Arc::clone(&self.shared),
            channel: Arc::clone(&channel),
            number,
            complete: false,
        };
        if wire
            .send(Frame::Open(Open::Request(OpenRequest {
                channel: number,
                ticket: ticket.to_owned(),
                host_peer,
            })))
            .is_err()
        {
            self.shared.forget(number);
            return Err(OpenError::Interrupted);
        }
        let waited = Duration::from_secs(u64::from(OPEN_TIMEOUT_SECS)) + OPEN_MARGIN;
        let answer = tokio::time::timeout(waited, answer_rx).await;
        match answer {
            Ok(Ok(Ok(cid))) => {
                let mut state = lock(&channel);
                state.cid = cid;
                state.phase = Phase::Open;
                drop(state);
                lock(&self.shared.state)
                    .tunnels
                    .insert(cid, Arc::clone(&channel));
                opening.complete = true;
                Ok(Tunnel::new(channel, events_rx))
            }
            Ok(Ok(Err(error))) => {
                self.shared.forget(number);
                Err(error)
            }
            Ok(Err(_)) => {
                self.shared.forget(number);
                Err(OpenError::Interrupted)
            }
            Err(_) => {
                self.shared.forget(number);
                Err(OpenError::TimedOut)
            }
        }
    }

    /// How many tunnels the hub holds, open or waiting.
    #[must_use]
    pub fn count(&self) -> usize {
        lock(&self.shared.state).tunnels.len()
    }
}

impl HubShared {
    fn udp_budget(state: &mut HubState, port: Option<&PortTicketClaims>) -> Arc<UdpBudget> {
        Self::udp_budget_for(state, port.map(|port| port.environment_id))
    }

    fn udp_budget_for(state: &mut HubState, environment: Option<Id>) -> Arc<UdpBudget> {
        // The session's port limits, as its gateway stated them; the shipped ones without one.
        let limits = state
            .session
            .as_ref()
            .map_or(PortLimits::SHIPPED, |(session, _)| session.limits().ports);
        let Some(environment) = environment else {
            return Arc::new(UdpBudget::for_limits(&limits));
        };
        if let Some(budget) = state.udp_budgets.get(&environment).and_then(Weak::upgrade) {
            return budget;
        }
        let budget = Arc::new(UdpBudget::for_limits(&limits));
        state
            .udp_budgets
            .insert(environment, Arc::downgrade(&budget));
        budget
    }
    fn tick(&self, now: Instant) {
        let channels: Vec<Arc<Mutex<Channel>>> =
            lock(&self.state).channels.values().cloned().collect();
        let waiting: Vec<Arc<Mutex<Channel>>> = {
            let state = lock(&self.state);
            state.tunnels.values().cloned().collect()
        };
        for channel in channels {
            lock(&channel).maybe_ack(false);
        }
        for channel in waiting {
            let mut inner = lock(&channel);
            if let Phase::Suspended(since) = inner.phase
                && now.duration_since(since) >= self.options.grace
            {
                inner.close_with(Ended::Reset, false);
            }
        }
        let closed: Vec<Id> = lock(&self.state)
            .tunnels
            .iter()
            .filter(|(_, channel)| lock(channel).phase == Phase::Closed)
            .map(|(cid, _)| *cid)
            .collect();
        if !closed.is_empty() {
            let mut state = lock(&self.state);
            for cid in closed {
                state.tunnels.remove(&cid);
            }
            state
                .channels
                .retain(|_, channel| lock(channel).phase != Phase::Closed);
        }
    }

    fn forget(&self, number: u32) {
        let mut state = lock(&self.state);
        state.channels.remove(&number);
        state.pending.remove(&number);
    }

    fn session_up(&self, sender: SessionSender) {
        let wire: Arc<dyn Wire> = Arc::new(SessionWire(sender.clone()));
        let mut resumes = Vec::new();
        {
            let mut state = lock(&self.state);
            state.session = Some((sender.clone(), Arc::clone(&wire)));
            state.channels.clear();
            state.next_number = Side::Machine.first();
            let waiting: Vec<Arc<Mutex<Channel>>> = state.tunnels.values().cloned().collect();
            for channel in waiting {
                let (opener, suspended) = {
                    let inner = lock(&channel);
                    (inner.opener, matches!(inner.phase, Phase::Suspended(_)))
                };
                if opener && suspended {
                    let number = state.next_number;
                    state.next_number = Side::Machine.next(number);
                    state.channels.insert(number, Arc::clone(&channel));
                    resumes.push((number, channel));
                }
            }
        }
        for (number, channel) in resumes {
            let inner = lock(&channel);
            if let Some(ticket) = inner.ticket.clone() {
                let _ = sender.send(Frame::Resume(Resume {
                    channel: number,
                    ticket,
                    jti_channel: inner.cid,
                    recv: inner.recv.window.received(),
                }));
            }
        }
    }

    fn session_down(&self, now: Instant) {
        self.port_control.send_replace(false);
        let (pending, tunnels) = {
            let mut state = lock(&self.state);
            state.session = None;
            state.port_available = false;
            let pending: Vec<PendingOpen> =
                state.pending.drain().map(|(_, pending)| pending).collect();
            state.channels.clear();
            let tunnels: Vec<Arc<Mutex<Channel>>> = state.tunnels.values().cloned().collect();
            (pending, tunnels)
        };
        for waiting in pending {
            let _ = waiting.answer.send(Err(OpenError::Interrupted));
            lock(&waiting.channel).close_with(Ended::Reset, false);
        }
        for channel in tunnels {
            lock(&channel).suspend(now);
        }
    }

    fn on_frame(&self, frame: Frame) {
        match frame {
            Frame::Open(Open::Offer(offer)) => self.on_offer(*offer),
            Frame::PortControl(control) => {
                self.port_control.send_replace(control.available);
                let channels = {
                    let mut state = lock(&self.state);
                    state.port_available = control.available;
                    state.channels.values().cloned().collect::<Vec<_>>()
                };
                if !control.available {
                    for channel in channels {
                        let mut channel = lock(&channel);
                        if channel.kind == TunnelKind::Port {
                            channel.close_with(Ended::Reset, true);
                        }
                    }
                }
            }
            Frame::OpenOk(OpenOk {
                channel,
                cid,
                inside,
            }) => {
                let pending = lock(&self.state).pending.remove(&channel);
                if let Some(pending) = pending {
                    {
                        let mut inner = lock(&pending.channel);
                        inner.cid = cid;
                        inner.inside = inside;
                    }
                    let _ = pending.answer.send(Ok(cid));
                }
            }
            Frame::OpenErr(OpenErr {
                channel,
                code,
                detail,
            }) => {
                let pending = lock(&self.state).pending.remove(&channel);
                if let Some(pending) = pending {
                    lock(&self.state).channels.remove(&channel);
                    lock(&pending.channel).close_with(Ended::Reset, false);
                    let _ = pending
                        .answer
                        .send(Err(OpenError::Refused { code, detail }));
                }
            }
            Frame::Credit(Credit { channel, n }) => self.with_channel(channel, |c| c.on_credit(n)),
            Frame::Eof(Eof { channel }) => self.with_channel(channel, Channel::on_eof),
            Frame::ChAck(ChAck { channel, offset }) => {
                self.with_channel(channel, |c| c.on_ack(offset))
            }
            Frame::Close(Close {
                channel,
                code,
                detail,
            }) => {
                self.with_channel(channel, |c| c.on_peer_close(code, &detail));
                lock(&self.state).channels.remove(&channel);
            }
            Frame::PtyResize(PtyResize {
                channel,
                cols,
                rows,
            }) => {
                self.with_channel(channel, |c| {
                    let _ = c.events.send(TunnelEvent::Resize { cols, rows });
                });
            }
            Frame::PtyExit(PtyExit { channel, code }) => {
                self.with_channel(channel, |c| {
                    let _ = c.events.send(TunnelEvent::Exit { code });
                });
            }
            Frame::Resume(resume) => self.on_resume(&resume),
            Frame::ResumeOk(ResumeOk { channel, recv }) => self.on_resume_ok(channel, recv),
            _ => {}
        }
    }

    fn with_channel(&self, number: u32, work: impl FnOnce(&mut Channel)) {
        let channel = lock(&self.state).channels.get(&number).cloned();
        if let Some(channel) = channel {
            work(&mut lock(&channel));
        }
    }

    fn on_offer(&self, offer: orch_link_contract::tunnels::OpenOffer) {
        if offer.kind == TunnelKind::Port && !lock(&self.state).port_available {
            self.refuse(
                offer.channel,
                OpenErrorCode::Refused,
                "current port control is unavailable",
            );
            return;
        }
        let incoming = {
            let state = lock(&self.state);
            state.incoming_tx.clone()
        };
        let Some(incoming) = incoming else {
            self.refuse(
                offer.channel,
                OpenErrorCode::NoSuchService,
                "nothing here takes tunnels",
            );
            return;
        };
        let tunnel = IncomingTunnel {
            kind: offer.kind,
            service: offer.service,
            from: offer.from,
            claims: offer.claims,
            number: offer.channel,
            hub: self.weak.clone(),
            answered: false,
        };
        if let Err(rejected) = incoming.send(tunnel) {
            let mut tunnel = rejected.0;
            tunnel.answered = true;
            self.refuse(
                offer.channel,
                OpenErrorCode::NoSuchService,
                "nothing here takes tunnels",
            );
        }
    }

    fn accept(
        &self,
        number: u32,
        kind: TunnelKind,
        service: &str,
        port: Option<PortTicketClaims>,
        inside: Option<PortConnectionInside>,
    ) -> Result<Tunnel, OpenError> {
        if kind == TunnelKind::Port
            && (inside.is_none() || port.is_none() || !lock(&self.state).port_available)
        {
            return Err(OpenError::NoSession);
        }
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (sender, wire) = lock(&self.state)
            .session
            .clone()
            .ok_or(OpenError::NoSession)?;
        let limits = sender.limits();
        let options = ChannelOptions {
            window_bytes: u64::from(limits.tunnel_window_bytes),
            replay_bytes: self.options.replay_bytes,
            resumable: !matches!(
                kind,
                TunnelKind::Pty | TunnelKind::Browser | TunnelKind::Rfb
            ),
        }
        .for_port(port.as_ref(), &limits.ports);
        let cid = new_id();
        let mut channel = Channel::new(
            cid,
            kind,
            service.to_owned(),
            false,
            options,
            self.gauge.clone(),
            events_tx,
        );
        channel.number = Some(number);
        channel.wire = Some(Arc::clone(&wire));
        channel.phase = Phase::Open;
        let budget = {
            let mut state = lock(&self.state);
            Self::udp_budget(&mut state, port.as_ref())
        };
        channel.configure_port(port, inside.clone(), budget);
        let channel = Arc::new(Mutex::new(channel));
        {
            let mut state = lock(&self.state);
            state.channels.insert(number, Arc::clone(&channel));
            state.tunnels.insert(cid, Arc::clone(&channel));
        }
        sender
            .send(Frame::OpenOk(OpenOk {
                channel: number,
                cid,
                inside,
            }))
            .map_err(|SessionOver| OpenError::Interrupted)?;
        Ok(Tunnel::new(channel, events_rx))
    }

    fn refuse(&self, number: u32, code: OpenErrorCode, detail: &str) {
        if let Some((sender, _)) = lock(&self.state).session.clone() {
            let _ = sender.send(Frame::OpenErr(OpenErr {
                channel: number,
                code,
                detail: detail.to_owned(),
            }));
        }
    }

    /// The other end asks to carry a tunnel on over this session.
    fn on_resume(&self, resume: &Resume) {
        let (sender, wire, channel) = {
            let state = lock(&self.state);
            let Some((sender, wire)) = state.session.clone() else {
                return;
            };
            (
                sender,
                wire,
                state.tunnels.get(&resume.jti_channel).cloned(),
            )
        };
        let Some(channel) = channel else {
            let _ = sender.send(Frame::Close(Close {
                channel: resume.channel,
                code: CloseCode::Reset,
                detail: "no such tunnel".to_owned(),
            }));
            return;
        };
        let outcome = {
            let mut inner = lock(&channel);
            if inner.phase == Phase::Closed {
                Err(Ended::Reset)
            } else {
                // The old number, if the session did not change, is replaced.
                inner.resume(Arc::clone(&wire), resume.channel, resume.recv)
            }
        };
        match outcome {
            Ok(have) => {
                {
                    let mut state = lock(&self.state);
                    state
                        .channels
                        .retain(|_, held| !Arc::ptr_eq(held, &channel));
                    state.channels.insert(resume.channel, Arc::clone(&channel));
                }
                let _ = sender.send(Frame::ResumeOk(ResumeOk {
                    channel: resume.channel,
                    recv: have,
                }));
                lock(&channel).start_pumping_after_resume();
            }
            Err(_) => {
                lock(&channel).close_with(Ended::Reset, false);
                let _ = sender.send(Frame::Close(Close {
                    channel: resume.channel,
                    code: CloseCode::Reset,
                    detail: "the bytes needed are no longer held".to_owned(),
                }));
            }
        }
    }

    /// The destination answered a request to carry a tunnel on.
    fn on_resume_ok(&self, number: u32, peer_received: u64) {
        let (wire, channel) = {
            let state = lock(&self.state);
            let Some((_, wire)) = state.session.clone() else {
                return;
            };
            (wire, state.channels.get(&number).cloned())
        };
        let Some(channel) = channel else {
            return;
        };
        let mut inner = lock(&channel);
        if !matches!(inner.phase, Phase::Suspended(_)) {
            return;
        }
        match inner.resume(wire, number, peer_received) {
            Ok(_) => inner.start_pumping_after_resume(),
            Err(_) => inner.close_with(Ended::Reset, true),
        }
    }
}

/// Copies bytes both ways between a tunnel and a local socket until both
/// directions end. Used by the programs that connect a harness to its
/// executor.
pub async fn bridge<A, B>(a: &mut A, b: &mut B) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    tokio::io::copy_bidirectional(a, b).await
}
