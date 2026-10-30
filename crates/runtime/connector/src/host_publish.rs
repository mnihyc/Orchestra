//! A host opening on its node: the reviewed number bound for exactly the transports asked, and each caller
//! carried to the environment on a channel of its own.
//!
//! `port.publish` binds the host number on the declared address (`all` is every address of both families, by
//! one dual-stack socket where the machine has IPv6, else IPv4 alone; or one literal IP) for TCP, UDP or both,
//! and reports open only when every transport asked is bound: a binding that fails leaves nothing of the opening
//! bound. Each accepted TCP connection opens a `port` channel to the environment's machine with the ticket the
//! system signed for this node, reports its observed outside peer for the last-hop PROXY prefix where the
//! program asks for it (a host caller has its address and no invented identity), and carries bytes both ways
//! until either end closes. Each UDP sender gets an association of its own, which ends after a minute without a
//! datagram. The same opening at the same revision binds nothing twice and only takes the new tickets; an older
//! revision never replaces or closes a newer one. No firewall or NAT rule is changed.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::tunnels::Tunnels;
use bytes::Bytes;
use orch_api_contract::ports::PortTransport;
use orch_link_contract::ports::{
    HostPortPeer, PortClose, PortCloseResult, PortPublish, PortPublishResult, PortPublishState,
};
use orch_link_contract::tunnels::Ticket;
use orch_types::{Clock, Id};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

mod record;

/// What `listen` is written as for every address of both families.
const ALL: &str = "all";
/// How many datagrams of one sender wait for its channel to take them before more are dropped.
const SENDER_QUEUE: usize = 64;
/// How long a listener waits after a failed accept before it accepts again.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);
/// How often a node holding host openings reads its `[ports]` ceiling again: an operator who narrows it sees
/// what it forbids end within this, and reading one small file this often costs nothing.
pub const CEILING_LOOK_EVERY: Duration = Duration::from_millis(250);

/// One opening as its node holds it.
struct Held {
    revision: i64,
    environment_id: Id,
    port: u16,
    host_port: u16,
    incoming_address: String,
    listen: String,
    transports: Vec<PortTransport>,
    tickets: Arc<Mutex<HashMap<PortTransport, String>>>,
    stop: CancellationToken,
    tasks: TaskTracker,
}

/// The host openings a node holds.
pub struct Publisher {
    tunnels: Arc<Tunnels>,
    held: Mutex<HashMap<Id, Held>>,
    capture: record::Capture,
}

impl std::fmt::Debug for Publisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Publisher").finish_non_exhaustive()
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        for held in self
            .held
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            held.stop.cancel();
            held.tasks.close();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The addresses `listen` binds at `port`: one dual-stack address for `all` (and IPv4 alone where IPv6 is
/// missing, tried after it), or the one literal IP.
fn addresses(listen: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    if listen == ALL {
        return Ok(vec![
            SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
        ]);
    }
    listen
        .parse::<IpAddr>()
        .map(|ip| vec![SocketAddr::new(ip, port)])
        .map_err(|_| format!("{listen} is neither all nor one IP address"))
}

/// A bound listener of one transport.
enum Bound {
    Tcp(tokio::net::TcpListener),
    Udp(tokio::net::UdpSocket),
}

/// How often a number still held by this node's own previous binding of the same opening is tried again, and
/// how long between: an ended listener lets its socket go at the next turn of its task, which is soon.
const REBIND_TRIES: u32 = 20;
/// The pause between two tries of a number this node's own ended binding still holds.
const REBIND_PAUSE: Duration = Duration::from_millis(50);

/// Binds `transport` on the first of `candidates` that takes it: every address of both families by the
/// dual-stack one, else the next. A number that is in use is tried again for a second, the time this node's
/// own replaced binding of the opening takes to let it go; a native process holding it is never moved.
async fn bind(transport: PortTransport, candidates: &[SocketAddr]) -> Result<Bound, String> {
    let mut last = String::from("no address to bind");
    for address in candidates {
        let mut tries = 0;
        loop {
            let bound = match transport {
                PortTransport::Tcp => tokio::net::TcpListener::bind(address).await.map(Bound::Tcp),
                PortTransport::Udp => tokio::net::UdpSocket::bind(address).await.map(Bound::Udp),
            };
            match bound {
                Ok(bound) => return Ok(bound),
                Err(error)
                    if error.kind() == std::io::ErrorKind::AddrInUse && tries < REBIND_TRIES =>
                {
                    tries += 1;
                    tokio::time::sleep(REBIND_PAUSE).await;
                }
                Err(error) => {
                    last = format!("{address}: {error}");
                    break;
                }
            }
        }
    }
    Err(last)
}

/// The PROXY protocol v2 header of a TCP connection from `caller` to `local`: both of one family (an IPv4
/// caller of a dual-stack socket is written as IPv4), and no TLV, since a host caller is known by its address
/// alone.
#[must_use]
pub fn proxy_v2(caller: SocketAddr, local: SocketAddr) -> Vec<u8> {
    orch_link::ports::proxy_v2(caller, local)
}

impl Publisher {
    /// A node's openings, carried over `tunnels`.
    #[must_use]
    pub fn new(tunnels: Arc<Tunnels>) -> Self {
        Self::with_clock(tunnels, Arc::new(orch_types::SystemClock))
    }

    /// A node's own transient record uses the same clock as its other live facts.
    #[must_use]
    pub fn with_clock(tunnels: Arc<Tunnels>, clock: Arc<dyn Clock>) -> Self {
        Self {
            tunnels,
            held: Mutex::new(HashMap::new()),
            capture: record::Capture::new(clock),
        }
    }

    /// The node half of each environment's bounded live record, never journalled or reconstructed.
    #[must_use]
    pub fn records(&self) -> Arc<orch_link::ports::PortRecords> {
        Arc::clone(&self.capture.records)
    }

    /// Expires ended debugging rows even while there are no listeners or readers.
    pub fn prune_records(&self) {
        self.capture.records.prune(self.capture.clock.time());
    }

    /// Ends owned listeners and child sockets before a node stops.
    pub async fn close_all(&self) {
        let held: Vec<_> = lock(&self.held).drain().map(|(_, held)| held).collect();
        for held in &held {
            held.stop.cancel();
            held.tasks.close();
        }
        for held in held {
            held.tasks.wait().await;
        }
    }

    /// Carries out `port.publish`.
    pub async fn publish(&self, body: PortPublish) -> PortPublishResult {
        let failed = |reason: String| PortPublishResult {
            state: PortPublishState::Failed,
            host_port: body.host_port,
            transports: Vec::new(),
            reason: Some(reason),
        };
        let tickets: HashMap<PortTransport, String> = body
            .tickets
            .iter()
            .map(|each| (each.transport, each.ticket.clone()))
            .collect();
        if body.transports.is_empty() || body.transports.iter().any(|t| !tickets.contains_key(t)) {
            return failed("every transport to bind needs a ticket of its own".to_owned());
        }
        for (transport, ticket) in &tickets {
            let valid = Ticket::parse(ticket).ok().is_some_and(|ticket| {
                let claims = ticket.claims();
                claims.dst == body.environment_machine_id
                    && claims.port.as_ref().is_some_and(|port| {
                        port.environment_id == body.environment_id
                            && port.generation == body.generation
                            && port.port == body.port
                            && port.transport == *transport
                            && port.opening_id == Some(body.opening_id)
                            && port.audience
                                == Some(orch_api_contract::ports::PortOpeningKind::Host)
                            && port.allocation_revision == Some(body.allocation_revision)
                            && port
                                .host_template
                                .as_ref()
                                .is_some_and(|host| host.proxy_protocol == body.proxy_protocol)
                    })
            });
            if !valid {
                return failed(
                    "the publication's signed transport template does not match its target"
                        .to_owned(),
                );
            }
        }
        self.capture.records.set_limits(&self.tunnels.port_limits());
        self.capture.records.prune(self.capture.clock.time());
        let older = {
            let mut held = lock(&self.held);
            if let Some(current) = held.get(&body.opening_id) {
                if current.revision > body.revision {
                    return failed("a newer revision of the opening holds it".to_owned());
                }
                if current.revision == body.revision
                    && current.host_port == body.host_port
                    && current.listen == body.listen
                    && current.transports == body.transports
                {
                    // Bound already: only the tickets are new.
                    *lock(&current.tickets) = tickets;
                    return PortPublishResult {
                        state: PortPublishState::Open,
                        host_port: body.host_port,
                        transports: body.transports.clone(),
                        reason: None,
                    };
                }
            }
            held.remove(&body.opening_id)
        };
        if let Some(older) = older {
            older.stop.cancel();
            older.tasks.close();
            older.tasks.wait().await;
        }
        let candidates = match addresses(&body.listen, body.host_port) {
            Ok(candidates) => candidates,
            Err(reason) => return failed(reason),
        };
        let mut bound = Vec::with_capacity(body.transports.len());
        for transport in &body.transports {
            match bind(*transport, &candidates).await {
                Ok(listener) => bound.push(listener),
                // What was bound is dropped here, so nothing of the opening stays bound.
                Err(reason) => {
                    return failed(format!(
                        "{} {} could not be bound: {reason}",
                        match transport {
                            PortTransport::Tcp => "TCP",
                            PortTransport::Udp => "UDP",
                        },
                        body.host_port
                    ));
                }
            }
        }
        let stop = CancellationToken::new();
        let tasks = TaskTracker::new();
        let tickets = Arc::new(Mutex::new(tickets));
        for listener in bound {
            let tunnels = Arc::clone(&self.tunnels);
            let tickets = Arc::clone(&tickets);
            let stop = stop.clone();
            match listener {
                Bound::Tcp(listener) => {
                    tasks.spawn(serve_tcp(
                        listener,
                        tunnels,
                        tickets,
                        self.capture.clone(),
                        tasks.clone(),
                        stop,
                    ));
                }
                Bound::Udp(socket) => {
                    tasks.spawn(serve_udp(
                        Arc::new(socket),
                        tunnels,
                        tickets,
                        body.environment_id,
                        self.capture.clone(),
                        tasks.clone(),
                        stop,
                    ));
                }
            }
        }
        lock(&self.held).insert(
            body.opening_id,
            Held {
                revision: body.revision,
                environment_id: body.environment_id,
                port: body.port,
                host_port: body.host_port,
                incoming_address: body.incoming_address.clone(),
                listen: body.listen.clone(),
                transports: body.transports.clone(),
                tickets,
                stop,
                tasks,
            },
        );
        tracing::info!(opening = %body.opening_id, host_port = body.host_port, "a host opening is bound");
        PortPublishResult {
            state: PortPublishState::Open,
            host_port: body.host_port,
            transports: body.transports,
            reason: None,
        }
    }

    /// Carries out `port.close`: the opening's listeners and connections end, for that revision or an
    /// older one; a newer publication stays.
    pub async fn close(&self, body: &PortClose) -> PortCloseResult {
        let ended = {
            let mut held = lock(&self.held);
            if held
                .get(&body.opening_id)
                .is_some_and(|current| current.revision <= body.revision)
            {
                held.remove(&body.opening_id)
            } else {
                None
            }
        };
        if let Some(ended) = ended {
            ended.stop.cancel();
            ended.tasks.close();
            ended.tasks.wait().await;
            PortCloseResult { closed: true }
        } else {
            PortCloseResult { closed: false }
        }
    }

    /// Ends the host openings that the ceiling `ports` forbids now, as its operator narrowed it: their listeners
    /// and connections close and their numbers are free. Returns each with the revision it held and why it ended.
    pub async fn close_forbidden(
        &self,
        ports: &orch_config::server::Ports,
    ) -> Vec<(Id, i64, String)> {
        let limits = crate::host_ports::limits(ports);
        let forbidden: Vec<(Id, i64, String)> = lock(&self.held)
            .iter()
            .filter_map(|(opening, current)| {
                crate::host_ports::check(
                    &orch_api_contract::ports::HostPorts {
                        incoming_address: current.incoming_address.clone(),
                        listen: current.listen.clone(),
                        ranges: vec![orch_api_contract::ports::PortRange {
                            from: current.host_port,
                            to: current.host_port,
                        }],
                    },
                    &limits,
                )
                .err()
                .map(|reason| (*opening, current.revision, reason))
            })
            .collect();
        for (opening, revision, reason) in &forbidden {
            let _ = self
                .close(&PortClose {
                    opening_id: *opening,
                    environment_id: Id::nil(),
                    generation: 0,
                    revision: *revision,
                })
                .await;
            tracing::info!(%opening, %reason, "a host opening ends: the node's ceiling forbids it now");
        }
        forbidden
    }

    /// The host numbers the node holds now, which a proposal leaves out.
    #[must_use]
    pub fn held_ports(&self) -> Vec<u16> {
        lock(&self.held)
            .values()
            .map(|held| held.host_port)
            .collect()
    }
}

impl crate::lifecycle::PublishedPorts for Publisher {
    fn ports_of(&self, environment: Id) -> Vec<crate::lifecycle::HookPort> {
        let mut ports: Vec<crate::lifecycle::HookPort> = lock(&self.held)
            .iter()
            .filter(|(_, held)| held.environment_id == environment)
            .map(|(opening, held)| crate::lifecycle::HookPort {
                opening_id: *opening,
                host_port: held.host_port,
                port: held.port,
                transports: held.transports.clone(),
            })
            .collect();
        ports.sort_by_key(|port| port.host_port);
        ports
    }
}

/// The ticket of `transport` as it stands now.
fn ticket_of(
    tickets: &Mutex<HashMap<PortTransport, String>>,
    transport: PortTransport,
) -> Option<String> {
    lock(tickets).get(&transport).cloned()
}

/// Accepts the TCP callers of one opening until it ends, each on a channel of its own, at most as many at once
/// as a port carries.
async fn serve_tcp(
    listener: tokio::net::TcpListener,
    tunnels: Arc<Tunnels>,
    tickets: Arc<Mutex<HashMap<PortTransport, String>>>,
    capture: record::Capture,
    tasks: TaskTracker,
    stop: CancellationToken,
) {
    let room = Arc::new(Semaphore::new(orch_link::ports::count(
        tunnels.port_limits().connections_max,
    )));
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((caller, peer)) = accepted else {
            // Accepting fails mostly when the process has no descriptor left: a pause lets connections end
            // instead of spinning on the same failure.
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(ACCEPT_PAUSE) => {}
            }
            continue;
        };
        // A caller beyond the bound is refused at once rather than queued without end.
        let Ok(permit) = Arc::clone(&room).try_acquire_owned() else {
            continue;
        };
        let Some(ticket) = ticket_of(&tickets, PortTransport::Tcp) else {
            continue;
        };
        let tunnels = Arc::clone(&tunnels);
        let stop = stop.clone();
        let capture = capture.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let use_id=orch_types::new_id();
            let accepted=HostPortPeer{use_id,peer:orch_api_contract::ports::PortConnectionInside{address:peer.ip().to_string(),port:peer.port()}};
            let opened=tokio::select! {()=stop.cancelled()=>return,opened=tunnels.open_host_port(&ticket,accepted)=>opened};
            let tunnel = match opened {
                Ok(tunnel) => tunnel,
                Err(error) => {
                    tracing::debug!(%error, "a host caller's channel was not opened");
                    return;
                }
            };
            let Some(inside)=tunnel.inside_peer() else {return;};
            let Some(record)=capture.accepted(&ticket,use_id,peer,inside) else {return;};
            let mut caller=record::Socket{socket:caller,record};
            let mut tunnel = Box::pin(tunnel);
            tokio::select! {
                () = stop.cancelled() => {}
                _ = tokio::io::copy_bidirectional(&mut caller, &mut tunnel) => {}
            }
        });
    }
}

/// Serves the UDP senders of one opening until it ends: each sender an association of its own, ended after a
/// minute without a datagram either way.
async fn serve_udp(
    socket: Arc<tokio::net::UdpSocket>,
    tunnels: Arc<Tunnels>,
    tickets: Arc<Mutex<HashMap<PortTransport, String>>>,
    environment: Id,
    capture: record::Capture,
    tasks: TaskTracker,
    stop: CancellationToken,
) {
    let mut senders: HashMap<SocketAddr, mpsc::Sender<orch_link::ports::Datagram>> = HashMap::new();
    let (ended, mut ending) = mpsc::unbounded_channel::<SocketAddr>();
    let limits = tunnels.port_limits();
    let max = orch_link::ports::count(limits.udp_datagram_bytes)
        .min(orch_link_contract::ports::PortDatagram::MAX_BYTES);
    let mut buffer = vec![0u8; max.saturating_add(1)];
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            gone = ending.recv() => {
                if let Some(peer) = gone {
                    senders.remove(&peer);
                }
            }
            received = socket.recv_from(&mut buffer) => {
                let Ok((length, peer)) = received else { continue };
                let Some(datagram)=tunnels.reserve_port_datagram(environment,Bytes::copy_from_slice(&buffer[..length])) else {continue;};
                if let Some(queue) = senders.get(&peer) {
                    // A sender whose channel is behind loses datagrams, as UDP may.
                    let _ = queue.try_send(datagram);
                    continue;
                }
                if senders.len() >= orch_link::ports::count(limits.udp_senders_max) {
                    continue;
                }
                let Some(ticket) = ticket_of(&tickets, PortTransport::Udp) else { continue };
                let (queue, datagrams) = mpsc::channel(SENDER_QUEUE);
                let _ = queue.try_send(datagram);
                senders.insert(peer, queue);
                tasks.spawn(associate(Association{
                    socket:Arc::clone(&socket),peer,tunnels:Arc::clone(&tunnels),ticket,
                    stop:stop.clone(),ended:ended.clone(),capture:capture.clone(),
                    idle:Duration::from_secs(u64::from(limits.udp_idle_secs)),
                },datagrams));
            }
        }
    }
}

struct Association {
    socket: Arc<tokio::net::UdpSocket>,
    peer: SocketAddr,
    tunnels: Arc<Tunnels>,
    ticket: String,
    stop: CancellationToken,
    ended: mpsc::UnboundedSender<SocketAddr>,
    capture: record::Capture,
    idle: Duration,
}

/// One UDP sender's association: datagrams until its negotiated idle time, channel or opening ends.
async fn associate(
    association: Association,
    mut datagrams: mpsc::Receiver<orch_link::ports::Datagram>,
) {
    let Association {
        socket,
        peer,
        tunnels,
        ticket,
        stop,
        ended,
        capture,
        idle,
    } = association;
    let use_id = orch_types::new_id();
    let accepted = HostPortPeer {
        use_id,
        peer: orch_api_contract::ports::PortConnectionInside {
            address: peer.ip().to_string(),
            port: peer.port(),
        },
    };
    let opened = tokio::select! {()=stop.cancelled()=>{let _=ended.send(peer);return;},opened=tunnels.open_host_port(&ticket,accepted)=>opened};
    if let Ok(tunnel) = opened {
        let record = tunnel
            .inside_peer()
            .and_then(|inside| capture.accepted(&ticket, use_id, peer, inside));
        let Some(record) = record else {
            tunnel.closer().close();
            let _ = ended.send(peer);
            return;
        };
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                () = tokio::time::sleep(idle) => break,
                next = datagrams.recv() => match next {
                    Some(datagram) => {
                        let bytes=datagram.frame.payload.len();
                        match tunnel.send_reserved_datagram(datagram) {Ok(true)=>record.bytes(true,bytes),Ok(false)=>{},Err(_)=>break}
                    }
                    None => break,
                },
                answer = tunnel.recv_datagram() => match answer {
                    Ok(datagram) => {
                        if let Ok(bytes)=socket.send_to(&datagram, peer).await {record.bytes(false,bytes);}
                    }
                    Err(_) => break,
                },
            }
        }
        tunnel.closer().close();
    }
    let _ = ended.send(peer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_header_names_the_caller_in_its_own_family() {
        let caller: SocketAddr = "[::ffff:203.0.113.7]:51000".parse().unwrap();
        let local: SocketAddr = "[::ffff:192.0.2.10]:50000".parse().unwrap();
        let header = proxy_v2(caller, local);
        assert_eq!(&header[..12], b"\r\n\r\n\0\r\nQUIT\n");
        assert_eq!(header[12], 0x21);
        assert_eq!(
            header[13], 0x11,
            "an IPv4 caller of a dual-stack socket is written as IPv4"
        );
        assert_eq!(&header[14..16], &12u16.to_be_bytes());
        assert_eq!(&header[16..20], &[203, 0, 113, 7]);
        assert_eq!(&header[20..24], &[192, 0, 2, 10]);
        assert_eq!(&header[24..26], &51000u16.to_be_bytes());
        assert_eq!(&header[26..28], &50000u16.to_be_bytes());
        assert_eq!(header.len(), 28);

        let six = proxy_v2(
            "[2001:db8::7]:51000".parse().unwrap(),
            "[2001:db8::1]:50000".parse().unwrap(),
        );
        assert_eq!(six[13], 0x21);
        assert_eq!(&six[14..16], &36u16.to_be_bytes());
        assert_eq!(six.len(), 16 + 36);
    }

    #[test]
    fn all_is_the_dual_stack_address_first_and_ipv4_after_and_a_literal_is_itself() {
        let all = addresses("all", 50_000).unwrap();
        assert_eq!(all[0], "[::]:50000".parse::<SocketAddr>().unwrap());
        assert_eq!(all[1], "0.0.0.0:50000".parse::<SocketAddr>().unwrap());
        assert_eq!(
            addresses("192.0.2.10", 50_000).unwrap(),
            vec!["192.0.2.10:50000".parse::<SocketAddr>().unwrap()]
        );
        assert!(addresses("somewhere", 1).is_err());
    }
}
