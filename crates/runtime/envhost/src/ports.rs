//! Current native port bindings in an environment's process or private network scope.
//!
//! Discovery reads the native socket table and the existing scoped process reader. It never
//! probes a service. A connection resolves a fresh owned binding rather than keeping a PID
//! from an earlier observation; the temporary proof is checked again before bytes are handed on.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};

use orch_api_contract::ports::{
    PortBinding, PortDiscovery, PortObservation, PortObservationProcess, PortSnapshot,
    PortSnapshotUdpDropped, PortTransport,
};
use orch_types::{Clock, Id, Time};

use crate::processes::Processes;

/// Whether unknown socket owners can be admitted within a proved private network namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortScope {
    /// Only a socket held by the environment's currently scoped processes can be used.
    Processes,
    /// The caller established that this whole network namespace belongs to the environment.
    PrivateNetwork,
}

/// Why the current native port cannot be carried.
#[derive(Debug, thiserror::Error)]
pub enum PortReadError {
    /// The platform cannot establish native scoped ownership.
    #[error("scoped port discovery is unavailable: {0}")]
    Unsupported(String),
    /// No currently owned socket serves this port and transport.
    #[error("nothing owned by this environment is bound at that port and transport")]
    NotListening,
    /// The selected target is an Orch control endpoint.
    #[error("this is an internal Orch endpoint")]
    Internal,
    /// The connection raced a socket replacement and must obtain a fresh proof.
    #[error("the owned socket changed during connection admission")]
    Changed,
    /// A foreign binding could receive the bytes sent to this selected address.
    #[error("the target address is not exclusively owned by this environment")]
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeSocket {
    address: SocketAddr,
    transport: PortTransport,
    inode: u64,
}

#[derive(Debug, Default)]
struct State {
    activity_at: Option<Time>,
    udp_dropped: PortSnapshotUdpDropped,
    carrier_drops: Option<CarrierDrops>,
    first_seen: HashMap<String, Time>,
    internal: BTreeMap<(u16, PortTransport), usize>,
    watch: Option<tokio::sync::watch::Sender<Option<PortSnapshot>>>,
}

struct CarrierDrops(Arc<dyn Fn() -> PortSnapshotUdpDropped + Send + Sync>);
impl std::fmt::Debug for CarrierDrops {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CarrierDrops")
    }
}

/// The one native reader used for both a fresh look and its live watch.
#[derive(Clone)]
pub struct Ports {
    environment_id: Id,
    generation: i64,
    scope: PortScope,
    processes: Processes,
    clock: Arc<dyn Clock>,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for Ports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ports")
            .field("environment_id", &self.environment_id)
            .field("generation", &self.generation)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A control socket remains Internal only while its registration is held.
#[derive(Debug)]
pub struct InternalPort {
    state: Arc<Mutex<State>>,
    key: (u16, PortTransport),
}

impl Drop for InternalPort {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        if let Some(count) = state.internal.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.internal.remove(&self.key);
            }
        }
    }
}

/// A temporary proof for the current selected owned native socket.
#[derive(Debug, Clone)]
pub struct OwnedPort {
    socket: NativeSocket,
    observation_id: String,
    /// The deterministic current local destination; a wildcard uses its family's loopback.
    pub address: SocketAddr,
}

impl Ports {
    /// Records actual owned use or traffic, never an idle socket or opening.
    pub fn activity(&self, at: Time) {
        let mut state = lock(&self.state);
        state.activity_at = Some(state.activity_at.map_or(at, |old| old.max(at)));
    }
    /// Adds transient numeric UDP losses without retaining a packet or caller.
    pub fn udp_dropped(&self, drops: PortSnapshotUdpDropped) {
        let mut state = lock(&self.state);
        state.udp_dropped.oversized += drops.oversized;
        state.udp_dropped.full += drops.full;
        state.udp_dropped.senders += drops.senders;
        state.udp_dropped.local_send += drops.local_send;
        state.udp_dropped.link_lost += drops.link_lost;
    }
    /// Joins the carrying link's shared store counters into this native live measurement.
    pub fn carrier_drops(&self, read: Arc<dyn Fn() -> PortSnapshotUdpDropped + Send + Sync>) {
        lock(&self.state).carrier_drops = Some(CarrierDrops(read));
    }
    /// Reads sockets for one environment, using its existing process ownership and clock.
    #[must_use]
    pub fn new(
        environment_id: Id,
        generation: i64,
        scope: PortScope,
        processes: Processes,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            environment_id,
            generation,
            scope,
            processes,
            clock,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// The configured ownership boundary, never selected by a port's caller.
    #[must_use]
    pub fn scope(&self) -> PortScope {
        self.scope
    }

    /// Marks a carrier or other protected control number for its socket's lifetime.
    #[must_use]
    pub fn internal_port(&self, port: u16, transport: PortTransport) -> InternalPort {
        let key = (port, transport);
        *lock(&self.state).internal.entry(key).or_default() += 1;
        InternalPort {
            state: Arc::clone(&self.state),
            key,
        }
    }

    /// Shares one current five-second native watch until its last reader leaves.
    pub fn watch(&self) -> tokio::sync::watch::Receiver<Option<PortSnapshot>> {
        // Match the environment's ordinary live measurement cadence, retaining only the latest
        // snapshot so a slow observer cannot accumulate an unbounded queue.
        const INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
        let mut state = lock(&self.state);
        if let Some(sender) = &state.watch
            && sender.receiver_count() > 0
        {
            return sender.subscribe();
        }
        let (sender, receiver) = tokio::sync::watch::channel(None);
        state.watch = Some(sender.clone());
        let ports = self.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(INTERVAL);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = sender.closed() => break,
                    _ = ticks.tick() => {
                        let reader = ports.clone();
                        let measured = tokio::task::spawn_blocking(move || reader.look()).await;
                        let Ok(snapshot) = measured else { break };
                        if sender.send(Some(snapshot)).is_err() { break; }
                    }
                }
            }
        });
        receiver
    }

    /// Measures TCP listeners and bound UDP, preserving unsupported as an explicit state.
    #[must_use]
    pub fn look(&self) -> PortSnapshot {
        let now = Time::new(self.clock.now());
        let (activity_at, udp_dropped) = {
            let state = lock(&self.state);
            let mut dropped = state.udp_dropped.clone();
            if let Some(read) = &state.carrier_drops {
                let carrier = read.0();
                dropped.oversized = dropped.oversized.saturating_add(carrier.oversized);
                dropped.full = dropped.full.saturating_add(carrier.full);
                dropped.senders = dropped.senders.saturating_add(carrier.senders);
                dropped.local_send = dropped.local_send.saturating_add(carrier.local_send);
                dropped.link_lost = dropped.link_lost.saturating_add(carrier.link_lost);
            }
            (state.activity_at, dropped)
        };
        match self.observations(now) {
            Ok((_, listeners)) => PortSnapshot {
                environment_id: self.environment_id,
                generation: self.generation,
                as_of: now,
                activity_at,
                discovery: PortDiscovery::Native,
                reason: None,
                listeners,
                udp_dropped,
            },
            Err(error) => PortSnapshot {
                environment_id: self.environment_id,
                generation: self.generation,
                as_of: now,
                activity_at: None,
                discovery: PortDiscovery::Unsupported,
                reason: Some(error.to_string()),
                listeners: Vec::new(),
                udp_dropped: PortSnapshotUdpDropped::default(),
            },
        }
    }

    /// Resolves the same preferred current owned address for every caller of this port.
    pub fn owned(&self, port: u16, transport: PortTransport) -> Result<OwnedPort, PortReadError> {
        let (native, observations) = self.observations(Time::new(self.clock.now()))?;
        let mut candidates: Vec<_> = observations
            .iter()
            .filter(|row| row.port == port && row.transport == transport && !row.internal)
            .filter(|row| row.unavailable_reason.is_none())
            .collect();
        candidates.sort_by_key(|row| {
            row.local_address
                .parse::<IpAddr>()
                .map(preference)
                .unwrap_or((u8::MAX, u128::MAX))
        });
        let Some(row) = candidates.first() else {
            return Err(
                if observations
                    .iter()
                    .any(|row| row.port == port && row.transport == transport && row.internal)
                {
                    PortReadError::Internal
                } else {
                    PortReadError::NotListening
                },
            );
        };
        let ip: IpAddr = row
            .local_address
            .parse()
            .map_err(|_| PortReadError::Changed)?;
        let destination = SocketAddr::new(loopback_for_wildcard(ip), port);
        let socket = native
            .iter()
            .find(|socket| {
                socket.transport == transport
                    && socket.address == SocketAddr::new(ip, port)
                    && row.observation_id.starts_with(&socket_key(socket))
            })
            .cloned()
            .ok_or(PortReadError::Changed)?;
        // A shared-address foreign SO_REUSEPORT binding could receive this connect. An observed
        // owned process is insufficient when another matching native socket is unproved.
        if self.scope == PortScope::Processes
            && native.iter().any(|other| {
                other.transport == transport
                    && other.address.port() == port
                    && other.inode != socket.inode
                    && same_destination(other.address.ip(), destination.ip())
                    && !observations.iter().any(|known| {
                        !known.internal
                            && known.unavailable_reason.is_none()
                            && known.observation_id.starts_with(&socket_key(other))
                    })
            })
        {
            return Err(PortReadError::Foreign);
        }
        Ok(OwnedPort {
            socket,
            observation_id: row.observation_id.clone(),
            address: destination,
        })
    }

    /// Checks the temporary proof again after connecting and before handing caller bytes on.
    pub fn recheck(&self, proof: &OwnedPort) -> Result<(), PortReadError> {
        let current = self.owned(proof.socket.address.port(), proof.socket.transport)?;
        if current.socket == proof.socket && current.observation_id == proof.observation_id {
            Ok(())
        } else {
            Err(PortReadError::Changed)
        }
    }

    fn observations(
        &self,
        now: Time,
    ) -> Result<(Vec<NativeSocket>, Vec<PortObservation>), PortReadError> {
        let native = native_sockets()?;
        let listed = self.processes.list();
        let mut owners: BTreeMap<u64, PortObservationProcess> = BTreeMap::new();
        for process in listed {
            if !self
                .processes
                .is_current(process.info.pid, process.start_ms)
            {
                continue;
            }
            let inodes = process_socket_inodes(process.info.pid);
            if !self
                .processes
                .is_current(process.info.pid, process.start_ms)
            {
                continue;
            }
            for inode in inodes {
                owners
                    .entry(inode)
                    .or_insert_with(|| PortObservationProcess {
                        pid: process.info.pid,
                        start: process.info.started_at,
                        name: process.info.name.clone(),
                        thread_id: process.info.thread_id,
                        item_id: process.info.item_id,
                    });
            }
        }
        let protected: BTreeSet<u64> = self
            .processes
            .protected_pids()
            .into_iter()
            .flat_map(process_socket_inodes)
            .collect();
        let mut state = lock(&self.state);
        let mut present = BTreeSet::new();
        let mut out = Vec::new();
        for socket in &native {
            let process = owners.get(&socket.inode).cloned();
            let internal = protected.contains(&socket.inode)
                || state
                    .internal
                    .contains_key(&(socket.address.port(), socket.transport));
            if process.is_none() && !internal && self.scope == PortScope::Processes {
                continue;
            }
            let start = process
                .as_ref()
                .map_or(0, |process| process.start.unix_millis());
            let observation_id = format!("{}:{start}", socket_key(socket));
            let first_seen_at = *state
                .first_seen
                .entry(observation_id.clone())
                .or_insert(now);
            present.insert(observation_id.clone());
            out.push(PortObservation {
                port: socket.address.port(),
                transport: socket.transport,
                local_address: socket.address.ip().to_string(),
                generation: self.generation,
                observation_id,
                first_seen_at,
                binding: match socket.transport {
                    PortTransport::Tcp => PortBinding::Listening,
                    PortTransport::Udp => PortBinding::BoundUnknown,
                },
                process,
                internal,
                unavailable_reason: internal.then(|| "Internal Orch endpoint".to_owned()),
            });
        }
        state.first_seen.retain(|key, _| present.contains(key));
        out.sort_by(|a, b| {
            (a.port, a.transport, &a.local_address, &a.observation_id).cmp(&(
                b.port,
                b.transport,
                &b.local_address,
                &b.observation_id,
            ))
        });
        Ok((native, out))
    }
}

fn socket_key(socket: &NativeSocket) -> String {
    format!(
        "{:?}:{}:{}:",
        socket.transport, socket.address, socket.inode
    )
}

fn loopback_for_wildcard(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        _ => ip,
    }
}

fn preference(ip: IpAddr) -> (u8, u128) {
    // All surfaces prefer IPv4 loopback, then IPv6 loopback, then numeric owned addresses.
    const IPV4_LOOPBACK: u8 = 0;
    const IPV6_LOOPBACK: u8 = 1;
    const OTHER_OWNED: u8 = 2;
    match loopback_for_wildcard(ip) {
        IpAddr::V4(ip) if ip.is_loopback() => (IPV4_LOOPBACK, u128::from(u32::from(ip))),
        IpAddr::V6(ip) if ip.is_loopback() => (IPV6_LOOPBACK, u128::from(ip)),
        IpAddr::V4(ip) => (OTHER_OWNED, u128::from(u32::from(ip))),
        IpAddr::V6(ip) => (OTHER_OWNED, u128::from(ip)),
    }
}

fn same_destination(bound: IpAddr, selected: IpAddr) -> bool {
    bound == selected
        || (bound.is_unspecified() && bound.is_ipv4() == selected.is_ipv4())
        // A dual-stack wildcard can accept an IPv4 caller too. Refuse an unproved one rather
        // than guess whether that particular native socket selected IPV6_V6ONLY.
        || (bound == IpAddr::V6(Ipv6Addr::UNSPECIFIED) && selected.is_ipv4())
}

#[cfg(target_os = "linux")]
fn process_socket_inodes(pid: u32) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter_map(|target| {
            target
                .to_str()?
                .strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse()
                .ok()
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn process_socket_inodes(_: u32) -> Vec<u64> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn native_sockets() -> Result<Vec<NativeSocket>, PortReadError> {
    const TABLES: [(&str, PortTransport, bool); 4] = [
        ("tcp", PortTransport::Tcp, false),
        ("tcp6", PortTransport::Tcp, true),
        ("udp", PortTransport::Udp, false),
        ("udp6", PortTransport::Udp, true),
    ];
    // The TCP native state for a socket accepting connections, rather than an outgoing use.
    const LISTEN: &str = "0A";
    let mut out = Vec::new();
    for (table, transport, ipv6) in TABLES {
        let text = std::fs::read_to_string(format!("/proc/self/net/{table}")).map_err(|_| {
            PortReadError::Unsupported("the native socket table cannot be read".into())
        })?;
        for line in text.lines().skip(1) {
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            let (Some(local), Some(state), Some(inode)) =
                (fields.get(1), fields.get(3), fields.get(9))
            else {
                return Err(PortReadError::Unsupported(
                    "the native socket row cannot be read".into(),
                ));
            };
            if transport == PortTransport::Tcp && *state != LISTEN {
                continue;
            }
            let address = native_address(local, ipv6).ok_or_else(|| {
                PortReadError::Unsupported("the native socket address cannot be read".into())
            })?;
            if address.port() == 0 {
                continue;
            }
            let inode = inode.parse().map_err(|_| {
                PortReadError::Unsupported("the native socket identity cannot be read".into())
            })?;
            out.push(NativeSocket {
                address,
                transport,
                inode,
            });
        }
    }
    Ok(out)
}

#[cfg(not(target_os = "linux"))]
fn native_sockets() -> Result<Vec<NativeSocket>, PortReadError> {
    Err(PortReadError::Unsupported(
        "this platform has no verified scoped socket reader".into(),
    ))
}

#[cfg(target_os = "linux")]
fn native_address(text: &str, ipv6: bool) -> Option<SocketAddr> {
    // Linux prints native-endian 32-bit words, also for each of the four IPv6 words.
    const WORD_HEX: usize = 8;
    const IPV6_WORDS: usize = 4;
    let (address, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if ipv6 {
        if address.len() != WORD_HEX * IPV6_WORDS {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (word, chunk) in address.as_bytes().chunks_exact(WORD_HEX).enumerate() {
            let number = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
            bytes[word * 4..word * 4 + 4].copy_from_slice(&number.to_ne_bytes());
        }
        IpAddr::V6(Ipv6Addr::from(bytes))
    } else {
        if address.len() != WORD_HEX {
            return None;
        }
        IpAddr::V4(Ipv4Addr::from(
            u32::from_str_radix(address, 16).ok()?.to_ne_bytes(),
        ))
    };
    Some(SocketAddr::new(ip, port))
}
