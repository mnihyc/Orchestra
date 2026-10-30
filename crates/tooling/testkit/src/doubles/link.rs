//! In-memory hub with link fault injection.
//!
//! The hub stands for the two things a machine's link meets: the gateway,
//! which holds its session, and the system behind the gateway, which takes
//! its events in and sends it commands. It runs the real handshake of
//! `orch-link` on in-memory connections, relays frames between a session and
//! the [`ControllerLink`] of that machine, and relays tunnels between the
//! sessions of two machines.
//!
//! Between the machine and the hub sits a pipe in which a test can make
//! things go wrong: drop, repeat or swap frames of a kind, cut the
//! connection after some frames or at once, refuse the next connections, or
//! shape the pipe to a thin line (delay, rate, and loss that costs a stall
//! as TCP's retransmission does). Nothing goes wrong unless the test says
//! so.

use async_trait::async_trait;
use orch_link::controller::{ControllerLink, ControllerOutputs, Notice};
use orch_link::framing::{
    Connection, Message, MessageSink, MessageSource, PeerInfo, TransportError,
};
use orch_link::reconnect::{DialError, Dialer};
use orch_link::session::{GatewayConfig, SessionAuthority, SessionEvent, SessionSender, accept};
use orch_link::tunnels::relay::{ApiPort, Relay};
use orch_link::tunnels::terminal::{ApiOut, TerminalHub};
use orch_link_contract::backplane::{AuthQuery, AuthReply, DownFrame, UpFrame};
use orch_link_contract::enrollment::Endpoint;
use orch_link_contract::frames::Frame;
use orch_link_contract::session::Role;
use orch_link_contract::version::{ByeCode, ConnectorVersions, Limits, RejectCode};
use orch_pki::certificates::Certificate;
use orch_types::{Clock, Id, Seq};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Which way a frame travels through the pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the machine to the gateway.
    Up,
    /// From the gateway to the machine.
    Down,
}

/// What happens to a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// It is lost.
    Drop,
    /// It arrives twice.
    Duplicate,
    /// It arrives after the frame that follows it.
    SwapWithNext,
    /// The connection is cut, and the frame is lost with it.
    Cut,
}

/// One thing that goes wrong, for some of the frames of a kind.
#[derive(Debug, Clone)]
struct Rule {
    direction: Direction,
    /// The `t` of a text frame, or `data` for tunnel data; any frame when
    /// `None`.
    frame: Option<String>,
    /// How many matching frames pass untouched first.
    skip: usize,
    /// How many matching frames suffer the action after that.
    times: usize,
    action: Action,
}

/// The shape of one direction of the pipe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// How long a frame takes to get there once it is sent.
    pub delay: Duration,
    /// Bytes a second the pipe carries.
    pub bytes_per_second: u64,
    /// The share of packets lost, which TCP makes up with a stall.
    pub loss: f64,
}

/// The size of a packet, for turning a loss rate into a loss of frames.
const PACKET_BYTES: usize = 1_460;
/// How long a lost packet stalls the pipe: a retransmission timeout, at least
/// twice the delay and at least this.
const MIN_STALL: Duration = Duration::from_millis(200);

#[derive(Debug)]
struct FaultsInner {
    rules: Mutex<Vec<Rule>>,
    shapes: Mutex<[Option<Shape>; 2]>,
    epoch: watch::Sender<u64>,
    refuse: AtomicUsize,
    connections: AtomicUsize,
    fired: AtomicUsize,
    lost: Mutex<u64>,
}

/// Where a test makes things go wrong.
#[derive(Debug, Clone)]
pub struct Faults {
    inner: Arc<FaultsInner>,
}

impl Default for Faults {
    fn default() -> Self {
        Self::new()
    }
}

impl Faults {
    /// A pipe where nothing goes wrong.
    #[must_use]
    pub fn new() -> Self {
        let (epoch, _) = watch::channel(0);
        Self {
            inner: Arc::new(FaultsInner {
                rules: Mutex::new(Vec::new()),
                shapes: Mutex::new([None, None]),
                epoch,
                refuse: AtomicUsize::new(0),
                connections: AtomicUsize::new(0),
                fired: AtomicUsize::new(0),
                lost: Mutex::new(0),
            }),
        }
    }

    fn add(
        &self,
        direction: Direction,
        frame: Option<&str>,
        skip: usize,
        times: usize,
        action: Action,
    ) {
        lock(&self.inner.rules).push(Rule {
            direction,
            frame: frame.map(str::to_owned),
            skip,
            times,
            action,
        });
    }

    /// Loses `times` frames of kind `frame` after `skip` have passed.
    pub fn drop_frames(&self, direction: Direction, frame: &str, skip: usize, times: usize) {
        self.add(direction, Some(frame), skip, times, Action::Drop);
    }

    /// Repeats `times` frames of kind `frame` after `skip` have passed.
    pub fn duplicate_frames(&self, direction: Direction, frame: &str, skip: usize, times: usize) {
        self.add(direction, Some(frame), skip, times, Action::Duplicate);
    }

    /// Swaps `times` frames of kind `frame` with the frame after them, after
    /// `skip` have passed.
    pub fn swap_frames(&self, direction: Direction, frame: &str, skip: usize, times: usize) {
        self.add(direction, Some(frame), skip, times, Action::SwapWithNext);
    }

    /// Cuts the connection when the frame after the first `skip` of kind
    /// `frame` (any kind when `None`) would pass.
    pub fn cut_after(&self, direction: Direction, frame: Option<&str>, skip: usize) {
        self.add(direction, frame, skip, 1, Action::Cut);
    }

    /// Cuts every connection now.
    pub fn sever_now(&self) {
        self.inner.epoch.send_modify(|epoch| *epoch += 1);
    }

    /// Refuses the next `count` connection attempts.
    pub fn refuse_connections(&self, count: usize) {
        self.inner.refuse.store(count, Ordering::SeqCst);
    }

    /// Shapes one direction of the pipe to a thin line.
    pub fn shape(&self, direction: Direction, shape: Option<Shape>) {
        lock(&self.inner.shapes)[Self::slot(direction)] = shape;
    }

    /// Forgets every rule that has not fired, and the shaping.
    pub fn clear(&self) {
        lock(&self.inner.rules).clear();
        *lock(&self.inner.shapes) = [None, None];
    }

    /// How many connections were made.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.inner.connections.load(Ordering::SeqCst)
    }

    /// How many times a rule made something go wrong.
    #[must_use]
    pub fn fired(&self) -> usize {
        self.inner.fired.load(Ordering::SeqCst)
    }

    /// How many frames the shaped pipe lost and made up.
    #[must_use]
    pub fn losses(&self) -> u64 {
        *lock(&self.inner.lost)
    }

    const fn slot(direction: Direction) -> usize {
        match direction {
            Direction::Up => 0,
            Direction::Down => 1,
        }
    }

    fn shape_of(&self, direction: Direction) -> Option<Shape> {
        lock(&self.inner.shapes)[Self::slot(direction)]
    }

    fn decide(&self, direction: Direction, kind: &str) -> Option<Action> {
        let mut rules = lock(&self.inner.rules);
        for rule in rules.iter_mut() {
            if rule.direction != direction || rule.frame.as_deref().is_some_and(|only| only != kind)
            {
                continue;
            }
            if rule.skip > 0 {
                rule.skip -= 1;
                continue;
            }
            if rule.times == 0 {
                continue;
            }
            rule.times -= 1;
            self.inner.fired.fetch_add(1, Ordering::SeqCst);
            return Some(rule.action);
        }
        None
    }
}

/// The kind of a message, as rules name it: the `t` of a text frame, `data`
/// for binary.
fn kind_of(message: &Message) -> String {
    match message {
        Message::Binary(_) => "data".to_owned(),
        Message::Text(text) => serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| value.get("t").and_then(|t| t.as_str().map(str::to_owned)))
            .unwrap_or_default(),
    }
}

struct PipeSink {
    tx: mpsc::Sender<Message>,
    dead: CancellationToken,
    /// Whether the forwarder has nothing in hand: the pipe can then carry the
    /// next message at once, and nothing is waiting in it.
    idle: Arc<watch::Sender<bool>>,
}

#[async_trait]
impl MessageSink for PipeSink {
    /// Ready when the pipe has no message in it, so that what is sent next is
    /// chosen at the last moment, as it is when a socket's buffer is small.
    async fn ready(&mut self) {
        let mut idle = self.idle.subscribe();
        tokio::select! {
            () = self.dead.cancelled() => {}
            _ = idle.wait_for(|idle| *idle) => {}
        }
    }

    async fn send(&mut self, message: Message) -> Result<(), TransportError> {
        if self.dead.is_cancelled() {
            return Err(TransportError::Closed);
        }
        self.idle.send_replace(false);
        tokio::select! {
            () = self.dead.cancelled() => Err(TransportError::Closed),
            sent = self.tx.send(message) => sent.map_err(|_| TransportError::Closed),
        }
    }

    async fn close(&mut self) {
        // Drop this direction's sender so its forwarder drains and the peer sees EOF.
        // Waiting for the receiver to close while retaining its sender keeps both sides alive forever.
        let (closed, receive) = mpsc::channel(1);
        drop(receive);
        self.tx = closed;
    }
}

struct PipeSource {
    rx: mpsc::UnboundedReceiver<Message>,
    dead: CancellationToken,
}

#[async_trait]
impl MessageSource for PipeSource {
    async fn recv(&mut self) -> Result<Option<Message>, TransportError> {
        tokio::select! {
            biased;
            () = self.dead.cancelled() => Ok(None),
            message = self.rx.recv() => Ok(message),
        }
    }
}

/// A small deterministic generator, so that a lossy pipe loses the same
/// frames on every run.
struct Dice(u64);

impl Dice {
    fn roll(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

/// Carries the messages of one direction from one end to the other, doing to
/// them what the faults say.
async fn forward(
    mut from: mpsc::Receiver<Message>,
    to: mpsc::UnboundedSender<Message>,
    direction: Direction,
    faults: Faults,
    dead: CancellationToken,
    born: u64,
    idle: Arc<watch::Sender<bool>>,
) {
    let mut epoch = faults.inner.epoch.subscribe();
    let (timed_tx, mut timed_rx) = mpsc::unbounded_channel::<(Instant, Message)>();
    let deliver_dead = dead.clone();
    let deliver = tokio::spawn(async move {
        while let Some((at, message)) = timed_rx.recv().await {
            if deliver_dead.is_cancelled() {
                return;
            }
            tokio::time::sleep_until(at).await;
            if deliver_dead.is_cancelled() || to.send(message).is_err() {
                return;
            }
        }
    });
    let mut held: Option<Message> = None;
    let mut link_free_at = Instant::now();
    let mut dice = Dice(0x9E37_79B9_7F4A_7C15 ^ (born << 1) ^ (direction as u64));
    loop {
        // Nothing is in hand: the sender may hand over the next message.
        idle.send_replace(true);
        let message = tokio::select! {
            () = dead.cancelled() => break,
            changed = epoch.changed() => {
                if changed.is_err() || *epoch.borrow() > born {
                    dead.cancel();
                    break;
                }
                continue;
            }
            message = from.recv() => match message {
                Some(message) => message,
                None => break,
            },
        };
        let action = faults.decide(direction, &kind_of(&message));
        let mut to_deliver: Vec<Message> = Vec::new();
        match action {
            Some(Action::Cut) => {
                dead.cancel();
                break;
            }
            Some(Action::Drop) => {}
            Some(Action::Duplicate) => {
                to_deliver.push(message.clone());
                to_deliver.push(message);
            }
            Some(Action::SwapWithNext) => {
                match held.replace(message) {
                    Some(previous) => to_deliver.push(previous),
                    None => continue,
                }
                continue;
            }
            None => {
                to_deliver.push(message);
                if let Some(previous) = held.take() {
                    to_deliver.push(previous);
                }
            }
        }
        for message in to_deliver {
            let now = Instant::now();
            let (arrive, free) = match faults.shape_of(direction) {
                None => (now, link_free_at.max(now)),
                Some(shape) => {
                    let start = link_free_at.max(now);
                    let transmit = Duration::from_secs_f64(
                        message.len() as f64 / shape.bytes_per_second.max(1) as f64,
                    );
                    let mut done = start + transmit;
                    let packets = message.len().div_ceil(PACKET_BYTES).max(1);
                    let survive =
                        (1.0 - shape.loss).powi(i32::try_from(packets).unwrap_or(i32::MAX));
                    if dice.roll() > survive {
                        *lock(&faults.inner.lost) += 1;
                        done += MIN_STALL.max(shape.delay * 2);
                    }
                    (done + shape.delay, done)
                }
            };
            link_free_at = free;
            // The sender is held up until the pipe is free to carry on: this
            // is what makes the rate show in how fast a sender can send.
            if link_free_at > now {
                tokio::select! {
                    () = dead.cancelled() => break,
                    () = tokio::time::sleep_until(link_free_at) => {}
                }
            }
            if timed_tx.send((arrive, message)).is_err() {
                break;
            }
        }
    }
    drop(timed_tx);
    // Let what was already sent arrive unless the connection was cut.
    if !dead.is_cancelled() {
        let _ = deliver.await;
    }
}

/// A connection between a machine and the hub, with the faults in between.
fn faulty_pair(
    machine_sees: PeerInfo,
    hub_sees: PeerInfo,
    faults: &Faults,
) -> (Connection, Connection) {
    /// Room for one frame in the pipe, as a socket buffer has room for
    /// little more: a sender that is faster than the pipe is held back.
    const PIPE_ROOM: usize = 1;
    let born = *faults.inner.epoch.borrow();
    let dead = CancellationToken::new();
    let (up_tx, up_rx) = mpsc::channel(PIPE_ROOM);
    let (down_tx, down_rx) = mpsc::channel(PIPE_ROOM);
    let (to_hub, hub_inbox) = mpsc::unbounded_channel();
    let (to_machine, machine_inbox) = mpsc::unbounded_channel();
    let (up_idle, down_idle) = (
        Arc::new(watch::channel(true).0),
        Arc::new(watch::channel(true).0),
    );
    tokio::spawn(forward(
        up_rx,
        to_hub,
        Direction::Up,
        faults.clone(),
        dead.clone(),
        born,
        Arc::clone(&up_idle),
    ));
    tokio::spawn(forward(
        down_rx,
        to_machine,
        Direction::Down,
        faults.clone(),
        dead.clone(),
        born,
        Arc::clone(&down_idle),
    ));
    let machine = Connection {
        sink: Box::new(PipeSink {
            tx: up_tx,
            dead: dead.clone(),
            idle: up_idle,
        }),
        source: Box::new(PipeSource {
            rx: machine_inbox,
            dead: dead.clone(),
        }),
        peer: machine_sees,
    };
    let hub = Connection {
        sink: Box::new(PipeSink {
            tx: down_tx,
            dead: dead.clone(),
            idle: down_idle,
        }),
        source: Box::new(PipeSource {
            rx: hub_inbox,
            dead,
        }),
        peer: hub_sees,
    };
    (machine, hub)
}

/// What the system knows of a machine.
#[derive(Debug, Clone)]
pub struct MachineRecord {
    /// Whether it may have a session.
    pub active: bool,
    /// The fingerprint of the certificate it is known by; any when `None`.
    pub fingerprint: Option<String>,
}

struct Slot {
    record: MachineRecord,
    controller: ControllerLink,
    session: Option<(Id, SessionSender)>,
}

struct HubInner {
    advertised: Mutex<Option<Vec<Endpoint>>>,
    clock: Arc<dyn Clock>,
    gateway: GatewayConfig,
    machines: Mutex<HashMap<Id, Slot>>,
    faults: Faults,
    relay: Mutex<Relay>,
    api_down: AtomicBool,
    terminals: TerminalHub,
}

/// The system's end of terminals, as the relay reaches it.
struct HubApi {
    terminals: TerminalHub,
}

impl ApiPort for HubApi {
    fn datagram(&self, machine: Id, packet: orch_link::ports::Datagram) {
        self.terminals
            .on_datagram(orch_link_contract::ports::BackplanePortDatagram {
                machine_id: machine,
                datagram: packet.delivered(),
            });
    }
    fn text(&self, frame: orch_link_contract::backplane::BackplaneFrame) {
        self.terminals.on_frame(frame);
    }

    fn data(&self, data: orch_link_contract::backplane::BackplaneData) {
        self.terminals.on_data(data);
    }
}

/// The gateway and the system, in memory.
#[derive(Clone)]
pub struct Hub {
    inner: Arc<HubInner>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").finish_non_exhaustive()
    }
}

/// The hub as the authority a gateway asks.
struct HubAuthority {
    hub: Arc<HubInner>,
}

#[async_trait]
impl SessionAuthority for HubAuthority {
    async fn authorize(&self, query: AuthQuery) -> AuthReply {
        let refuse = |code| AuthReply {
            endpoints: None,
            update_permit: None,
            n: query.n,
            ok: false,
            code: Some(code),
            cursors: None,
            placement: None,
        };
        if self.hub.api_down.load(Ordering::SeqCst) {
            return refuse(RejectCode::Busy);
        }
        let machines = lock(&self.hub.machines);
        let Some(slot) = machines.get(&query.machine_id) else {
            return refuse(RejectCode::UnknownMachine);
        };
        if !slot.record.active {
            return refuse(RejectCode::Revoked);
        }
        if slot
            .record
            .fingerprint
            .as_ref()
            .is_some_and(|known| known != &query.fingerprint)
        {
            return refuse(RejectCode::Revoked);
        }
        AuthReply {
            endpoints: lock(&self.hub.advertised).clone(),
            update_permit: None,
            n: query.n,
            ok: true,
            code: None,
            cursors: Some(slot.controller.cursors()),
            placement: Some("connected".to_owned()),
        }
    }
}

/// The address the hub's endpoint is known by in a machine's configuration.
pub const HUB_URL: &str = "wss://hub.test:9443";
/// The host the machines connect to.
pub const HUB_HOST: &str = "hub.test";

impl Hub {
    /// A hub whose gateway trusts machine certificates issued by `authority`.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, authority: Certificate) -> Self {
        Self::assemble(Self::default_gateway(authority), clock)
    }

    fn assemble(gateway: GatewayConfig, clock: Arc<dyn Clock>) -> Self {
        let (api_tx, mut api_rx) = mpsc::unbounded_channel::<ApiOut>();
        let terminals = TerminalHub::new(api_tx);
        let mut relay = Relay::new(gateway.limits, Arc::clone(&clock));
        relay.set_api(Arc::new(HubApi {
            terminals: terminals.clone(),
        }));
        let inner = Arc::new(HubInner {
            advertised: Mutex::new(None),
            relay: Mutex::new(relay),
            clock,
            gateway,
            machines: Mutex::new(HashMap::new()),
            faults: Faults::new(),
            api_down: AtomicBool::new(false),
            terminals,
        });
        let carrying = Arc::downgrade(&inner);
        tokio::spawn(async move {
            while let Some(out) = api_rx.recv().await {
                let Some(hub) = carrying.upgrade() else {
                    return;
                };
                let mut relay = lock(&hub.relay);
                match out {
                    ApiOut::Text(frame) => relay.on_api_frame(*frame),
                    ApiOut::Data(data) => relay.on_api_data(data),
                    ApiOut::Datagram { machine, packet } => {
                        relay.on_api_datagram(orch_link_contract::ports::BackplanePortDatagram {
                            machine_id: machine,
                            datagram: packet.delivered(),
                        })
                    }
                }
            }
        });
        let ticking = Arc::downgrade(&inner);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(250));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let Some(hub) = ticking.upgrade() else {
                    return;
                };
                lock(&hub.relay).tick(Instant::now());
            }
        });
        Self { inner }
    }

    /// The same hub with other limits in its sessions.
    #[must_use]
    pub fn with_limits(
        clock: Arc<dyn Clock>,
        authority: Certificate,
        limits: Limits,
        heartbeat_secs: u32,
    ) -> Self {
        let mut gateway = Self::default_gateway(authority);
        gateway.limits = limits;
        gateway.heartbeat_secs = heartbeat_secs;
        Self::assemble(gateway, clock)
    }

    fn default_gateway(authority: Certificate) -> GatewayConfig {
        GatewayConfig {
            gateway_id: "test-gateway".to_owned(),
            limits: Limits::default(),
            versions: ConnectorVersions {
                min: "0.1.0".to_owned(),
                recommended: "0.1.0".to_owned(),
            },
            heartbeat_secs: orch_link_contract::session::HEARTBEAT_SECS,
            authority,
        }
    }

    /// Where a test makes things go wrong.
    #[must_use]
    pub fn faults(&self) -> &Faults {
        &self.inner.faults
    }

    /// The system's record of a machine, and its link. Replaces what was
    /// there. The returned outputs are what the system's caller reads: the
    /// notices of the machine, and nothing else, because the hub carries the
    /// frames for the gateway by itself.
    pub fn add_machine(
        &self,
        machine_id: Id,
        roles: Vec<Role>,
        cursors: BTreeMap<String, Seq>,
        record: MachineRecord,
    ) -> (ControllerLink, mpsc::UnboundedReceiver<Notice>) {
        let (controller, outputs) =
            ControllerLink::new(machine_id, roles, cursors, Arc::clone(&self.inner.clock));
        let ControllerOutputs { down, notices } = outputs;
        let previous = lock(&self.inner.machines).insert(
            machine_id,
            Slot {
                record,
                controller: controller.clone(),
                session: None,
            },
        );
        if let Some(Slot {
            session: Some((_, sender)),
            ..
        }) = previous
        {
            sender.end();
        }
        tokio::spawn(carry_down(Arc::clone(&self.inner), machine_id, down));
        (controller, notices)
    }

    /// Replaces the system's link of a machine by a new one that starts from
    /// `cursors`, as if the system had restarted and read its cursors from
    /// its database. The sessions stay up.
    pub fn restart_system(
        &self,
        machine_id: Id,
        roles: Vec<Role>,
        cursors: BTreeMap<String, Seq>,
    ) -> (ControllerLink, mpsc::UnboundedReceiver<Notice>) {
        let (record, session) = {
            let machines = lock(&self.inner.machines);
            match machines.get(&machine_id) {
                Some(slot) => (slot.record.clone(), slot.session.clone()),
                None => (
                    MachineRecord {
                        active: true,
                        fingerprint: None,
                    },
                    None,
                ),
            }
        };
        let (controller, notices) = self.add_machine(machine_id, roles, cursors, record);
        if let Some((session_id, sender)) = session {
            // The old link's frames were lost with it; the machine is told its
            // session stands.
            if let Some(slot) = lock(&self.inner.machines).get_mut(&machine_id) {
                slot.session = Some((session_id, sender));
            }
            controller.session_up(session_id);
        }
        (controller, notices)
    }

    /// Makes the system unable to answer the gateway's question.
    pub fn set_api_down(&self, down: bool) {
        self.inner.api_down.store(down, Ordering::SeqCst);
        lock(&self.inner.relay).port_control(!down);
    }

    /// Changes the authenticated server list received on each subsequent connection.
    pub fn advertise_endpoints(&self, endpoints: Vec<Endpoint>) {
        *lock(&self.inner.advertised) = Some(endpoints);
    }

    /// Drops only this machine's session, leaving its identity and controller unchanged.
    pub fn disconnect(&self, machine: Id) {
        if let Some((_, sender)) = lock(&self.inner.machines)
            .get(&machine)
            .and_then(|slot| slot.session.clone())
        {
            sender.end();
        }
    }

    /// Marks a machine revoked, and ends its session.
    pub fn revoke(&self, machine_id: Id) {
        if let Some(slot) = lock(&self.inner.machines).get_mut(&machine_id) {
            slot.record.active = false;
            if let Some((_, sender)) = slot.session.take() {
                sender.bye(ByeCode::Revoked);
            }
        }
    }

    /// The session a machine has now, if any.
    #[must_use]
    pub fn session_of(&self, machine_id: Id) -> Option<Id> {
        lock(&self.inner.machines)
            .get(&machine_id)
            .and_then(|slot| slot.session.as_ref().map(|(id, _)| *id))
    }

    /// A dialer for a machine that shows `certificate` in the TLS handshake
    /// of its connection; when it shows none, it proves itself in the
    /// handshake of the session.
    #[must_use]
    pub fn dialer(&self, certificate: Option<Certificate>) -> Arc<dyn Dialer> {
        Arc::new(HubDialer {
            hub: self.clone(),
            certificate,
        })
    }

    /// The endpoint to put in a machine's configuration.
    #[must_use]
    pub fn endpoint() -> Endpoint {
        Endpoint {
            url: HUB_URL.to_owned(),
            trust: orch_link_contract::enrollment::Trust::Pinned,
        }
    }

    /// The system's end of the terminals of all machines.
    #[must_use]
    pub fn terminals(&self) -> TerminalHub {
        self.inner.terminals.clone()
    }

    /// Tells the relay whose signatures on tickets to believe.
    pub fn set_ticket_verifier(&self, verifier: orch_pki::tickets::TicketVerifier) {
        let mut relay = lock(&self.inner.relay);
        relay.set_verifier(verifier);
        relay.port_control(true);
    }

    /// How long the relay waits for a tunnel to be resumed.
    pub fn set_relay_grace(&self, grace: Duration) {
        lock(&self.inner.relay).set_grace(grace);
    }

    /// The relay that carries tunnels between sessions.
    pub fn with_relay<R>(&self, work: impl FnOnce(&mut Relay) -> R) -> R {
        work(&mut lock(&self.inner.relay))
    }
}

struct HubDialer {
    hub: Hub,
    certificate: Option<Certificate>,
}

#[async_trait]
impl Dialer for HubDialer {
    async fn dial(&self, endpoint: &Endpoint) -> Result<Connection, DialError> {
        let faults = &self.hub.inner.faults;
        let refused = faults
            .inner
            .refuse
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if refused {
            return Err(DialError::Connect(
                "the hub is refusing connections".to_owned(),
            ));
        }
        faults.inner.connections.fetch_add(1, Ordering::SeqCst);
        let (machine, gateway) = faulty_pair(
            PeerInfo {
                host: Some(HUB_HOST.to_owned()),
                ..PeerInfo::default()
            },
            PeerInfo {
                remote_ip: Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
                host: Some(HUB_HOST.to_owned()),
                client_certificate: self.certificate.clone(),
            },
            faults,
        );
        let _ = endpoint;
        tokio::spawn(serve(Arc::clone(&self.hub.inner), gateway));
        Ok(machine)
    }
}

/// Carries what the system sends a machine to the machine's session.
async fn carry_down(
    hub: Arc<HubInner>,
    machine_id: Id,
    mut down: mpsc::UnboundedReceiver<DownFrame>,
) {
    while let Some(frame) = down.recv().await {
        let sender = lock(&hub.machines)
            .get(&machine_id)
            .and_then(|slot| slot.session.as_ref().map(|(_, sender)| sender.clone()));
        if let Some(sender) = sender {
            let _ = match frame {
                DownFrame::Cmd(cmd) => sender.send(Frame::Cmd(cmd)),
                DownFrame::CmdCancel(cancel) => sender.send(Frame::CmdCancel(cancel)),
                DownFrame::Ack(ack) => sender.send(Frame::Ack(ack)),
                DownFrame::Nak(nak) => sender.send(Frame::Nak(nak)),
                DownFrame::McpImportAnswer(answer) => sender.send(Frame::McpImportAnswer(answer)),
                DownFrame::PortLocalAnswer(answer) => sender.send(Frame::PortLocalAnswer(answer)),
                DownFrame::PortConnectionsQuery(query) => {
                    sender.send(Frame::PortConnectionsQuery(query))
                }
            };
        }
    }
}

/// The gateway's side of one connection: the handshake, then relaying until
/// the session ends.
async fn serve(hub: Arc<HubInner>, connection: Connection) {
    let authority = HubAuthority {
        hub: Arc::clone(&hub),
    };
    let accepted = match accept(connection, &hub.gateway, &authority, hub.clock.as_ref()).await {
        Ok(accepted) => accepted,
        Err(error) => {
            tracing::debug!(%error, "the hub did not accept a session");
            return;
        }
    };
    let machine_id = accepted.auth.machine_id;
    let session_id = accepted.session_id;
    let sender = accepted.sender.clone();
    let mut inbox = accepted.inbox;
    let controller = {
        let mut machines = lock(&hub.machines);
        let Some(slot) = machines.get_mut(&machine_id) else {
            sender.end();
            return;
        };
        if let Some((old_id, old)) = slot.session.replace((session_id, sender.clone())) {
            old.bye(ByeCode::Replaced);
            lock(&hub.relay).session_gone(machine_id, old_id);
        }
        slot.controller.clone()
    };
    lock(&hub.relay).session_up(machine_id, session_id, sender.clone());
    controller.session_up(session_id);
    while let Some(event) = inbox.next().await {
        match event {
            SessionEvent::Frame(frame) => match UpFrame::try_from(*frame) {
                Ok(up) => controller.up(session_id, up),
                Err(other) => lock(&hub.relay).on_frame(machine_id, session_id, *other),
            },
            SessionEvent::Data(data) => lock(&hub.relay).on_data(machine_id, session_id, data),
            SessionEvent::Datagram(packet) => {
                lock(&hub.relay).on_datagram(machine_id, session_id, packet)
            }
            SessionEvent::Ended(_) => break,
        }
    }
    {
        let mut machines = lock(&hub.machines);
        if let Some(slot) = machines.get_mut(&machine_id)
            && slot.session.as_ref().map(|(id, _)| *id) == Some(session_id)
        {
            slot.session = None;
        }
    }
    lock(&hub.relay).session_gone(machine_id, session_id);
    controller.session_down(session_id);
}

/// A queue of messages, to be used by the doubles that need to look at
/// what passed.
#[derive(Debug, Default)]
pub struct Sightings {
    seen: Mutex<VecDeque<String>>,
}

impl Sightings {
    /// Notes a sighting.
    pub fn note(&self, what: impl Into<String>) {
        lock(&self.seen).push_back(what.into());
    }

    /// Takes everything noted so far.
    pub fn take(&self) -> Vec<String> {
        lock(&self.seen).drain(..).collect()
    }
}

/// A machine's key and certificate, as a test needs them.
#[derive(Debug)]
pub struct TestIdentity {
    /// The machine.
    pub machine_id: Id,
    /// Its key.
    pub key: orch_pki::certificates::LeafKey,
    /// Its certificate.
    pub certificate: Certificate,
}

/// An authority for tests, which issues machine certificates.
pub struct TestPki {
    authority: orch_pki::ca::CertificateAuthority,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for TestPki {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestPki").finish_non_exhaustive()
    }
}

impl TestPki {
    /// A new authority, good for ten years from the clock's moment.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        let authority = orch_pki::ca::CertificateAuthority::create(
            "Test",
            orch_pki::ca::CA_VALIDITY_DAYS,
            clock.as_ref(),
        )
        .unwrap_or_else(|error| panic!("a test authority could not be made: {error}"));
        Self { authority, clock }
    }

    /// The authority's certificate.
    #[must_use]
    pub fn certificate(&self) -> Certificate {
        self.authority.certificate().clone()
    }

    /// The authority itself.
    #[must_use]
    pub const fn authority(&self) -> &orch_pki::ca::CertificateAuthority {
        &self.authority
    }

    /// A key and a certificate for machine `machine_id`.
    #[must_use]
    pub fn machine(&self, machine_id: Id) -> TestIdentity {
        let (key, certificate) = self
            .authority
            .issue_with_new_key(
                &orch_pki::ca::Holder::Machine(machine_id),
                orch_link_contract::enrollment::LEAF_VALIDITY_DAYS.unsigned_abs(),
                self.clock.as_ref(),
            )
            .unwrap_or_else(|error| panic!("a test certificate could not be made: {error}"));
        TestIdentity {
            machine_id,
            key,
            certificate,
        }
    }
}

// ---------------------------------------------------------------- content, over a link of some shape

/// The content a system holds, and the rules it applies to uploads and
/// downloads: a request belongs to a command that is open, an upload goes on
/// where the last piece ended, and the whole is checked against its hash.
#[derive(Debug, Default)]
pub struct BlobServer {
    state: Mutex<BlobState>,
}

#[derive(Debug, Default)]
struct BlobState {
    open: std::collections::HashSet<Id>,
    uploads: HashMap<orch_types::Hash, Vec<u8>>,
    complete: HashMap<orch_types::Hash, bytes::Bytes>,
    puts: usize,
    cut_puts: Vec<usize>,
    requests: Vec<(String, usize)>,
}

impl BlobServer {
    /// A server with no content.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Opens a command: requests that name it are served.
    pub fn open_command(&self, command: Id) {
        lock(&self.state).open.insert(command);
    }

    /// Makes the `nth` upload piece (from 0) fail on its way, after the piece
    /// has been sent and before the server has taken it.
    pub fn cut_put(&self, nth: usize) {
        lock(&self.state).cut_puts.push(nth);
    }

    /// Puts content in the store.
    pub fn insert(&self, content: bytes::Bytes) -> orch_types::Hash {
        let hash = orch_types::Hash::of(&content);
        lock(&self.state).complete.insert(hash, content);
        hash
    }

    /// The content of a hash, if the server holds it whole.
    #[must_use]
    pub fn content(&self, hash: &orch_types::Hash) -> Option<bytes::Bytes> {
        lock(&self.state).complete.get(hash).cloned()
    }

    /// The requests served so far: their kind and the bytes they carried.
    #[must_use]
    pub fn requests(&self) -> Vec<(String, usize)> {
        lock(&self.state).requests.clone()
    }
}

/// How a link is shaped for content: the round trip, and bytes a second each
/// way.
#[derive(Debug, Clone, Copy)]
pub struct LinkShape {
    /// The round trip.
    pub round_trip: Duration,
    /// Bytes a second towards the system.
    pub up_bytes_per_second: u64,
    /// Bytes a second from the system.
    pub down_bytes_per_second: u64,
}

/// The server as a machine reaches it across a shaped link.
#[derive(Debug, Clone)]
pub struct ShapedBlobs {
    server: Arc<BlobServer>,
    shape: Arc<Mutex<Option<LinkShape>>>,
    max_blob_bytes: i64,
}

impl ShapedBlobs {
    /// The endpoint over `shape`; `None` is a link that is not slow.
    #[must_use]
    pub fn new(server: Arc<BlobServer>, shape: Option<LinkShape>, max_blob_bytes: i64) -> Self {
        Self {
            server,
            shape: Arc::new(Mutex::new(shape)),
            max_blob_bytes,
        }
    }

    /// Changes the shape of the link, as a line that gets better or worse.
    pub fn set_shape(&self, shape: Option<LinkShape>) {
        *lock(&self.shape) = shape;
    }

    async fn travel(&self, up: usize, down: usize) {
        let shape = *lock(&self.shape);
        if let Some(shape) = shape {
            let seconds = up as f64 / shape.up_bytes_per_second.max(1) as f64
                + down as f64 / shape.down_bytes_per_second.max(1) as f64;
            tokio::time::sleep(shape.round_trip + Duration::from_secs_f64(seconds)).await;
        }
    }

    fn require_open(&self, command: Id) -> Result<(), orch_link::content::PutRefusal> {
        if lock(&self.server.state).open.contains(&command) {
            Ok(())
        } else {
            Err(orch_link::content::PutRefusal::Refused(
                "the command is not open".to_owned(),
            ))
        }
    }
}

#[async_trait]
impl orch_link::content::BlobEndpoint for ShapedBlobs {
    async fn head(
        &self,
        command: Id,
        hash: &orch_types::Hash,
    ) -> Result<Option<orch_link_contract::content::BlobProgress>, orch_link::content::PutRefusal>
    {
        self.travel(0, 0).await;
        self.require_open(command)?;
        let state = lock(&self.server.state);
        if let Some(whole) = state.complete.get(hash) {
            return Ok(Some(orch_link_contract::content::BlobProgress {
                received: whole.len() as i64,
                complete: true,
            }));
        }
        Ok(state
            .uploads
            .get(hash)
            .map(|held| orch_link_contract::content::BlobProgress {
                received: held.len() as i64,
                complete: false,
            }))
    }

    async fn put(
        &self,
        command: Id,
        hash: &orch_types::Hash,
        query: orch_link_contract::content::PutQuery,
        body: bytes::Bytes,
    ) -> Result<orch_link_contract::content::BlobProgress, orch_link::content::PutRefusal> {
        use orch_link::content::PutRefusal;
        self.travel(body.len(), 0).await;
        self.require_open(command)?;
        let mut state = lock(&self.server.state);
        let nth = state.puts;
        state.puts += 1;
        if state.cut_puts.contains(&nth) {
            return Err(PutRefusal::Failed("the connection was cut".to_owned()));
        }
        state.requests.push(("put".to_owned(), body.len()));
        let received = state.uploads.get(hash).map_or(0, |held| held.len() as i64);
        let progress = orch_link_contract::content::accept_chunk(
            received,
            query,
            body.len() as i64,
            self.max_blob_bytes,
        )
        .map_err(|error| match error {
            orch_link_contract::content::ChunkRefusal::Offset { received } => {
                PutRefusal::Offset { received }
            }
            other => PutRefusal::Refused(other.to_string()),
        })?;
        state
            .uploads
            .entry(*hash)
            .or_default()
            .extend_from_slice(&body);
        if progress.complete {
            let whole = bytes::Bytes::from(state.uploads.remove(hash).unwrap_or_default());
            if orch_types::Hash::of(&whole) != *hash {
                return Err(PutRefusal::HashMismatch);
            }
            state.complete.insert(*hash, whole);
        }
        Ok(progress)
    }

    async fn get(
        &self,
        command: Id,
        hash: &orch_types::Hash,
        offset: i64,
        len: i64,
    ) -> Result<bytes::Bytes, orch_link::content::PutRefusal> {
        self.require_open(command)?;
        let piece = {
            let mut state = lock(&self.server.state);
            let whole = state.complete.get(hash).cloned().ok_or_else(|| {
                orch_link::content::PutRefusal::Refused("no such content".to_owned())
            })?;
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(whole.len());
            let end = usize::try_from(offset.saturating_add(len))
                .unwrap_or(usize::MAX)
                .min(whole.len());
            let piece = whole.slice(start..end);
            state.requests.push(("get".to_owned(), piece.len()));
            piece
        };
        self.travel(0, piece.len()).await;
        Ok(piece)
    }

    async fn missing(
        &self,
        command: Id,
        hashes: &[orch_types::Hash],
    ) -> Result<Vec<orch_types::Hash>, orch_link::content::PutRefusal> {
        self.travel(hashes.len() * 64, 0).await;
        self.require_open(command)?;
        let state = lock(&self.server.state);
        Ok(hashes
            .iter()
            .filter(|hash| !state.complete.contains_key(hash))
            .copied()
            .collect())
    }
}
