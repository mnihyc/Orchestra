//! Bounded port datagram storage and transient records of both connection ends.

use bytes::Bytes;
use orch_api_contract::ports::*;
use orch_link_contract::ports::{PortDatagram, PortTicketClaims};
use orch_link_contract::version::{
    PORT_BUFFER_BYTES, PORT_CALLER_TOTALS, PORT_CONNECTIONS_MAX, PORT_RECENT_CONNECTIONS,
    PORT_RECENT_SECS, PORT_RECENT_TOTAL, PORT_UDP_IDLE_SECS, PORT_UDP_SENDERS_MAX,
    PORT_WAITING_MAX, PortLimits,
};
use orch_types::{Id, Time};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Each TCP direction holds at most a quarter MiB of credit and replay, as shipped.
pub const BUFFER_BYTES: u64 = PORT_BUFFER_BYTES as u64;
/// Parallel TCP services and assets, across every ingress of an environment, as shipped.
pub const TCP_CONNECTIONS: usize = PORT_CONNECTIONS_MAX as usize;
/// Accepted TCP connections waiting for their ordinary open deadline, as shipped.
pub const TCP_WAITING: usize = PORT_WAITING_MAX as usize;
/// Each outside UDP sender has one inside socket, with this environment bound, as shipped.
pub const UDP_SENDERS: usize = PORT_UDP_SENDERS_MAX as usize;
/// A minute of silence ends a sender association, as shipped.
pub const UDP_IDLE_SECS: u64 = PORT_UDP_IDLE_SECS as u64;
/// Short debugging history per opening or own-use port, bounded independently, as shipped.
pub const RECENT_PER_TARGET: usize = PORT_RECENT_CONNECTIONS as usize;
/// A gateway takes half of the environment's 2048 retained ended rows, as shipped.
pub const RECENT_PER_OWNER: usize = (PORT_RECENT_TOTAL / 2) as usize;
/// A gateway takes half of the environment's 128 distinct caller groups, as shipped.
pub const CALLERS_PER_OWNER: usize = (PORT_CALLER_TOTALS / 2) as usize;
/// Ended rows leave after the fifteen-minute debugging loop, as shipped.
pub const RECENT_SECONDS: i64 = PORT_RECENT_SECS as i64;

/// The port bounds a deployment's `[link]` section sets, as the gateway states them to machines.
/// A figure larger than the wire's number is the largest it holds, and a datagram is never larger
/// than a frame carries, whatever the section says.
#[must_use]
pub fn limits_of(link: &orch_config::server::Link) -> PortLimits {
    let fit = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
    PortLimits {
        connections_max: fit(link.port_connections_max),
        waiting_max: fit(link.port_waiting_max),
        buffer_bytes: fit(link.port_buffer_bytes),
        udp_idle_secs: fit(link.port_udp_idle_secs),
        udp_senders_max: fit(link.port_udp_senders_max),
        udp_datagram_bytes: fit(link.port_udp_datagram_bytes)
            .min(u32::try_from(PortDatagram::MAX_BYTES).unwrap_or(u32::MAX)),
        udp_buffer_bytes: fit(link.port_udp_buffer_bytes),
        recent_connections: fit(link.port_recent_connections),
        recent_total: fit(link.port_recent_total),
        recent_secs: fit(link.port_recent_secs),
        caller_totals: fit(link.port_caller_totals),
    }
}

/// A bound of [`PortLimits`] as a count of things.
#[must_use]
pub fn count(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// A raw host caller's PROXY v2 prefix, written only at the environment's last TCP hop.
#[must_use]
pub fn proxy_v2(caller: std::net::SocketAddr, local: std::net::SocketAddr) -> Vec<u8> {
    use std::net::{IpAddr, SocketAddr};
    let plain = |address: SocketAddr| match address.ip() {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(address, |ip| {
            SocketAddr::new(IpAddr::V4(ip), address.port())
        }),
        IpAddr::V4(_) => address,
    };
    let (caller, local) = (plain(caller), plain(local));
    let mut bytes = b"\r\n\r\n\0\r\nQUIT\n".to_vec();
    bytes.push(0x21);
    match (caller.ip(), local.ip()) {
        (IpAddr::V4(from), IpAddr::V4(to)) => {
            bytes.push(0x11);
            bytes.extend_from_slice(&12u16.to_be_bytes());
            bytes.extend_from_slice(&from.octets());
            bytes.extend_from_slice(&to.octets());
        }
        (from, to) => {
            let six = |ip: IpAddr| match ip {
                IpAddr::V4(ip) => ip.to_ipv6_mapped(),
                IpAddr::V6(ip) => ip,
            };
            bytes.push(0x21);
            bytes.extend_from_slice(&36u16.to_be_bytes());
            bytes.extend_from_slice(&six(from).octets());
            bytes.extend_from_slice(&six(to).octets());
        }
    }
    bytes.extend_from_slice(&caller.port().to_be_bytes());
    bytes.extend_from_slice(&local.port().to_be_bytes());
    bytes
}

/// The share of an environment-wide total (ended rows, caller groups) that one carrier owner
/// keeps: a gateway the lower half and a node the rest, so that together they never pass the total
/// and neither has to ask the other.
#[must_use]
pub fn share(owner: PortConnectionOwner, total: u32) -> usize {
    let half = total / 2;
    count(match owner {
        PortConnectionOwner::Gateway => half,
        PortConnectionOwner::Node | PortConnectionOwner::Mixed => total - half,
    })
}

/// A shared nonblocking UDP budget, including headers so empty packets are bounded.
#[derive(Debug)]
pub struct UdpBudget {
    store: Arc<Semaphore>,
    max_datagram: usize,
    oversized: AtomicI64,
    full: AtomicI64,
    senders: AtomicI64,
    local_send: AtomicI64,
    link_lost: AtomicI64,
}

impl Default for UdpBudget {
    fn default() -> Self {
        Self::for_limits(&PortLimits::SHIPPED)
    }
}

impl UdpBudget {
    /// A store bounded by the configured environment bytes.
    #[must_use]
    pub fn new(bytes: usize) -> Self {
        Self {
            store: Arc::new(Semaphore::new(bytes)),
            max_datagram: PortDatagram::MAX_BYTES,
            oversized: AtomicI64::new(0),
            full: AtomicI64::new(0),
            senders: AtomicI64::new(0),
            local_send: AtomicI64::new(0),
            link_lost: AtomicI64::new(0),
        }
    }

    /// A store of the deployment's UDP bytes, taking no datagram larger than it allows.
    #[must_use]
    pub fn for_limits(limits: &PortLimits) -> Self {
        let mut budget = Self::new(count(limits.udp_buffer_bytes));
        budget.max_datagram = count(limits.udp_datagram_bytes).min(PortDatagram::MAX_BYTES);
        budget
    }

    /// Takes room immediately, or drops and counts; it never waits for credit.
    pub fn hold(self: &Arc<Self>, frame: PortDatagram) -> Option<Datagram> {
        if frame.payload.len() > self.max_datagram {
            self.oversized.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let bytes = frame.payload.len() + PortDatagram::HEADER_BYTES;
        let permit = match self.store.clone().try_acquire_many_owned(bytes as u32) {
            Ok(permit) => permit,
            Err(_) => {
                self.full.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        Some(Datagram {
            frame,
            held: Some(permit),
            budget: Arc::clone(self),
            delivered: false,
        })
    }

    /// Current numeric drop facts, with no caller or packet retained.
    #[must_use]
    pub fn dropped(&self) -> PortSnapshotUdpDropped {
        PortSnapshotUdpDropped {
            oversized: self.oversized.load(Ordering::Relaxed),
            full: self.full.load(Ordering::Relaxed),
            senders: self.senders.load(Ordering::Relaxed),
            local_send: self.local_send.load(Ordering::Relaxed),
            link_lost: self.link_lost.load(Ordering::Relaxed),
        }
    }

    /// Counts a refused new sender association.
    pub fn sender_dropped(&self) {
        self.senders.fetch_add(1, Ordering::Relaxed);
    }
    /// Counts an oversized packet observed at a framed transport boundary.
    pub fn oversized_dropped(&self) {
        self.oversized.fetch_add(1, Ordering::Relaxed);
    }
    /// Counts a failed last-hop send without keeping the packet.
    pub fn local_send_dropped(&self) {
        self.local_send.fetch_add(1, Ordering::Relaxed);
    }
}

/// One packet holds its room until emission or delivery; loss releases it and counts.
#[derive(Debug)]
pub struct Datagram {
    /// One whole admitted packet.
    pub frame: PortDatagram,
    held: Option<OwnedSemaphorePermit>,
    budget: Arc<UdpBudget>,
    delivered: bool,
}

impl Datagram {
    /// A queued packet can be retargeted only inside the same environment's negotiated store.
    #[must_use]
    pub fn belongs_to(&self, budget: &Arc<UdpBudget>) -> bool {
        Arc::ptr_eq(&self.budget, budget)
    }
    /// Marks emission/delivery and releases room, without replaying the packet.
    pub fn delivered(mut self) -> PortDatagram {
        self.delivered = true;
        self.held.take();
        self.frame.clone()
    }
}

impl Drop for Datagram {
    fn drop(&mut self) {
        if !self.delivered {
            self.budget.link_lost.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Records {
    peers: HashMap<(i64, u16, PortTransport, String, u16), Id>,
    rows: HashMap<Id, PortConnection>,
    ended: VecDeque<Id>,
    totals: HashMap<String, PortCallerTotal>,
    // An evicted active row must keep charging Other even if a later bounds increase admits
    // a new connection from that same caller. Entries live no longer than their bounded rows.
    row_groups: HashMap<Id, Option<String>>,
    other: PortCallerTotal,
    reset: Time,
    bytes_in: i64,
    bytes_out: i64,
    truncated: bool,
}

impl Records {
    fn new(now: Time) -> Self {
        Self {
            peers: HashMap::new(),
            rows: HashMap::new(),
            ended: VecDeque::new(),
            totals: HashMap::new(),
            row_groups: HashMap::new(),
            other: PortCallerTotal {
                caller: None,
                address: None,
                active: 0,
                connections: 0,
                bytes_in: 0,
                bytes_out: 0,
                http_requests: None,
                last_active: now,
                other: true,
            },
            reset: now,
            bytes_in: 0,
            bytes_out: 0,
            truncated: false,
        }
    }
    fn key(row: &PortConnection) -> String {
        match &row.outside.caller {
            PortCaller::Person { user } => format!("person:{}", user.id),
            PortCaller::Link { id, .. } => format!("link:{id}"),
            PortCaller::Address => format!("address:{}", row.outside.address),
        }
    }
    fn group(&mut self, row: &PortConnection, callers: usize) -> &mut PortCallerTotal {
        self.trim_callers(callers);
        let key = match self.row_groups.get(&row.id) {
            Some(key) => key.clone(),
            None => {
                let key = Self::key(row);
                let group =
                    (self.totals.contains_key(&key) || self.totals.len() < callers).then_some(key);
                self.row_groups.insert(row.id, group.clone());
                group
            }
        };
        if let Some(key) = key {
            self.totals.entry(key).or_insert_with(|| PortCallerTotal {
                caller: Some(row.outside.caller.clone()),
                address: Some(row.outside.address.clone()),
                active: 0,
                connections: 0,
                bytes_in: 0,
                bytes_out: 0,
                http_requests: row.http_requests.map(|_| 0),
                last_active: row.last_active,
                other: false,
            })
        } else {
            self.truncated = true;
            &mut self.other
        }
    }

    fn trim_callers(&mut self, callers: usize) {
        if self.totals.len() <= callers {
            return;
        }
        let mut keys: Vec<_> = self
            .totals
            .iter()
            .map(|(key, total)| (key.clone(), total.last_active))
            .collect();
        keys.sort_by(|(left, at), (right, other)| other.cmp(at).then_with(|| left.cmp(right)));
        for (key, _) in keys.into_iter().skip(callers) {
            if let Some(total) = self.totals.remove(&key) {
                self.other.active = self.other.active.saturating_add(total.active);
                self.other.connections = self.other.connections.saturating_add(total.connections);
                self.other.bytes_in = self.other.bytes_in.saturating_add(total.bytes_in);
                self.other.bytes_out = self.other.bytes_out.saturating_add(total.bytes_out);
                if let Some(count) = total.http_requests {
                    self.other.http_requests =
                        Some(self.other.http_requests.unwrap_or(0).saturating_add(count));
                }
                self.other.last_active = self.other.last_active.max(total.last_active);
            }
        }
        for group in self.row_groups.values_mut() {
            if group
                .as_ref()
                .is_some_and(|key| !self.totals.contains_key(key))
            {
                *group = None;
            }
        }
        self.truncated = true;
    }

    fn prune(&mut self, now: Time, bounds: &Bounds) {
        self.trim_callers(bounds.callers);
        let cutoff = now.get() - chrono::Duration::seconds(bounds.keep_secs);
        let mut groups: HashMap<(Option<Id>, u16), usize> = HashMap::new();
        let mut keep = VecDeque::new();
        // Newest wins within each opening and within the owner's fixed share of the total.
        for id in self.ended.iter().rev() {
            let Some(row) = self.rows.get(id) else {
                continue;
            };
            let count = groups.entry((row.opening_id, row.port)).or_default();
            if row.ended_at.is_none_or(|at| at.get() < cutoff)
                || *count >= bounds.per_target
                || keep.len() >= bounds.per_owner
            {
                self.rows.remove(id);
                self.row_groups.remove(id);
                self.truncated = true;
            } else {
                *count += 1;
                keep.push_front(*id);
            }
        }
        self.ended = keep;
    }
}

/// What one carrier owner keeps of ended connections and callers, from the deployment's limits.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    per_target: usize,
    per_owner: usize,
    callers: usize,
    keep_secs: i64,
}

impl Bounds {
    fn of(owner: PortConnectionOwner, limits: &PortLimits) -> Self {
        Self {
            per_target: count(limits.recent_connections),
            per_owner: share(owner, limits.recent_total),
            callers: share(owner, limits.caller_totals),
            keep_secs: i64::from(limits.recent_secs),
        }
    }
}

/// Each carrier owner's bounded memory, with exact active inside-peer lookup.
pub struct PortRecords {
    owner: PortConnectionOwner,
    bounds: Mutex<Bounds>,
    environments: Mutex<HashMap<Id, Records>>,
}

impl std::fmt::Debug for PortRecords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortRecords")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl PortRecords {
    /// A new transient registry for one carrier owner, with the shipped bounds; no rows are
    /// reconstructed.
    #[must_use]
    pub fn new(owner: PortConnectionOwner) -> Self {
        Self::with_limits(owner, &PortLimits::SHIPPED)
    }

    /// A new transient registry for one carrier owner, keeping its share of what the deployment's
    /// limits allow.
    #[must_use]
    pub fn with_limits(owner: PortConnectionOwner, limits: &PortLimits) -> Self {
        Self {
            owner,
            bounds: Mutex::new(Bounds::of(owner, limits)),
            environments: Mutex::new(HashMap::new()),
        }
    }

    /// Applies a newly negotiated lower deployment bound without reconstructing or copying rows.
    pub fn set_limits(&self, limits: &PortLimits) {
        let bounds = Bounds::of(self.owner, limits);
        *lock(&self.bounds) = bounds;
        for records in lock(&self.environments).values_mut() {
            records.trim_callers(bounds.callers);
        }
    }

    fn bounds(&self) -> Bounds {
        *lock(&self.bounds)
    }

    /// Expires ended rows even when nobody subscribes or asks for another page.
    pub fn prune(&self, now: Time) {
        for records in lock(&self.environments).values_mut() {
            records.prune(now, &self.bounds());
        }
    }

    /// Records an admitted use only after its actual inside end was reported.
    pub fn opened(
        &self,
        id: Id,
        claims: &PortTicketClaims,
        inside: PortConnectionInside,
        now: Time,
    ) {
        let mut environments = lock(&self.environments);
        let memory = environments
            .entry(claims.environment_id)
            .or_insert_with(|| Records::new(now));
        if memory.rows.contains_key(&id) {
            return;
        }
        let row = PortConnection {
            id,
            opening_id: claims.opening_id,
            environment_id: claims.environment_id,
            port: claims.port,
            transport: claims.transport,
            generation: claims.generation,
            outside: claims.source.clone(),
            inside,
            since: now,
            last_active: now,
            bytes_in: 0,
            bytes_out: 0,
            http_requests: None,
            ended_at: None,
        };
        let group = memory.group(&row, self.bounds().callers);
        group.active += 1;
        group.connections += 1;
        memory.peers.insert(
            (
                row.generation,
                row.port,
                row.transport,
                row.inside.address.clone(),
                row.inside.port,
            ),
            id,
        );
        memory.rows.insert(id, row);
    }

    /// Adds numeric use, with no content retained; idle presence adds no activity.
    pub fn traffic(
        &self,
        environment: Id,
        id: Id,
        incoming: bool,
        bytes: usize,
        http_request: bool,
        now: Time,
    ) {
        let mut environments = lock(&self.environments);
        let Some(memory) = environments.get_mut(&environment) else {
            return;
        };
        let Some(mut row) = memory.rows.remove(&id) else {
            return;
        };
        if row.ended_at.is_some() {
            memory.rows.insert(id, row);
            return;
        }
        let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
        row.last_active = now;
        if incoming {
            row.bytes_in = row.bytes_in.saturating_add(bytes);
            memory.bytes_in = memory.bytes_in.saturating_add(bytes);
        } else {
            row.bytes_out = row.bytes_out.saturating_add(bytes);
            memory.bytes_out = memory.bytes_out.saturating_add(bytes);
        }
        if http_request {
            row.http_requests = Some(row.http_requests.unwrap_or(0).saturating_add(1));
        }
        let group = memory.group(&row, self.bounds().callers);
        group.last_active = now;
        if incoming {
            group.bytes_in = group.bytes_in.saturating_add(bytes);
        } else {
            group.bytes_out = group.bytes_out.saturating_add(bytes);
        }
        if http_request {
            group.http_requests = Some(group.http_requests.unwrap_or(0).saturating_add(1));
        }
        memory.rows.insert(id, row);
    }

    /// Ends one row once, preserving only the bounded recent debugging facts.
    pub fn ended(&self, environment: Id, id: Id, now: Time) {
        let mut environments = lock(&self.environments);
        let Some(memory) = environments.get_mut(&environment) else {
            return;
        };
        let Some(mut row) = memory.rows.remove(&id) else {
            return;
        };
        if row.ended_at.is_none() {
            row.ended_at = Some(now);
            let key = (
                row.generation,
                row.port,
                row.transport,
                row.inside.address.clone(),
                row.inside.port,
            );
            if memory.peers.get(&key) == Some(&id) {
                memory.peers.remove(&key);
            }
            memory.group(&row, self.bounds().callers).active -= 1;
            memory.ended.push_back(id);
        }
        memory.rows.insert(id, row);
        memory.prune(now, &self.bounds());
    }

    /// Reads one bounded page; View alone gets only numeric counts.
    pub fn list(
        &self,
        environment: Id,
        generation: i64,
        query: &EnvironmentPortConnectionListQuery,
        callers: bool,
        now: Time,
    ) -> PortConnectionList {
        let mut environments = lock(&self.environments);
        let memory = environments
            .entry(environment)
            .or_insert_with(|| Records::new(now));
        memory.prune(now, &self.bounds());
        let mut rows: Vec<_> = memory
            .rows
            .values()
            .filter(|row| {
                (row.generation == generation || row.ended_at.is_some())
                    && query.opening_id.is_none_or(|id| row.opening_id == Some(id))
                    && query.port.is_none_or(|port| row.port == port)
                    && query.inside_port.is_none_or(|port| row.inside.port == port)
                    && query
                        .transport
                        .is_none_or(|transport| row.transport == transport)
            })
            .cloned()
            .collect();
        rows.sort_by_key(|row| row.id);
        let unfiltered = query.port.is_none()
            && query.opening_id.is_none()
            && query.inside_port.is_none()
            && query.transport.is_none();
        let counts = PortCounts {
            active: rows.iter().filter(|r| r.ended_at.is_none()).count() as i64,
            recent: rows.iter().filter(|r| r.ended_at.is_some()).count() as i64,
            http_requests: rows
                .iter()
                .filter_map(|r| r.http_requests)
                .reduce(i64::saturating_add),
            bytes_in: if unfiltered {
                memory.bytes_in
            } else {
                rows.iter().map(|r| r.bytes_in).fold(0, i64::saturating_add)
            },
            bytes_out: if unfiltered {
                memory.bytes_out
            } else {
                rows.iter()
                    .map(|r| r.bytes_out)
                    .fold(0, i64::saturating_add)
            },
            tracked_since: memory.reset,
            reset_at: memory.reset,
        };
        let after = query
            .cursor
            .as_ref()
            .and_then(|cursor| cursor.parse::<Id>().ok());
        rows.retain(|row| after.is_none_or(|after| row.id > after));
        const PAGE_DEFAULT: usize = 50;
        const PAGE_MAX: usize = 200;
        let limit = query
            .limit
            .map_or(PAGE_DEFAULT, |n| n as usize)
            .clamp(1, PAGE_MAX);
        let paged = rows.len() > limit;
        rows.truncate(limit);
        let cursor = if paged {
            rows.last().map(|row| row.id.to_string())
        } else {
            None
        };
        let mut totals: Vec<_> = memory.totals.values().cloned().collect();
        if memory.other.connections > 0 {
            totals.push(memory.other.clone());
        }
        totals.sort_by(|a, b| a.address.cmp(&b.address));
        PortConnectionList {
            as_of: now,
            owner: self.owner,
            counts,
            connections: callers.then_some(rows),
            totals: callers.then_some(totals),
            cursor,
            truncated: memory.truncated || paged,
        }
    }

    /// Returns only a current exact tuple, never an ended or reused peer.
    pub fn peer(
        &self,
        environment: Id,
        generation: i64,
        listener: u16,
        transport: PortTransport,
        inside: &PortConnectionInside,
    ) -> Option<PortConnection> {
        let environments = lock(&self.environments);
        let memory = environments.get(&environment)?;
        let id = memory.peers.get(&(
            generation,
            listener,
            transport,
            inside.address.clone(),
            inside.port,
        ))?;
        memory
            .rows
            .get(id)
            .filter(|row| row.ended_at.is_none())
            .cloned()
    }

    /// Drops all memory for a retired environment; a later registry starts honestly anew.
    pub fn forget(&self, environment: Id) {
        lock(&self.environments).remove(&environment);
    }
}

/// Makes a packet for a session-local channel without a destination in its payload.
#[must_use]
pub fn datagram(channel: u32, payload: Bytes) -> PortDatagram {
    PortDatagram { channel, payload }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deployment_s_port_bounds_come_from_its_link_section_and_a_datagram_never_outgrows_a_frame()
    {
        assert_eq!(
            limits_of(&orch_config::server::Link::default()),
            PortLimits::SHIPPED
        );
        let link = orch_config::server::Link {
            port_connections_max: 8,
            port_udp_datagram_bytes: 65_536,
            port_recent_total: 5,
            port_caller_totals: 3,
            ..orch_config::server::Link::default()
        };
        let limits = limits_of(&link);
        assert_eq!(limits.connections_max, 8);
        assert_eq!(limits.udp_datagram_bytes, 4_000);
        // The gateway keeps the lower half and the node the rest: together never more than the total.
        assert_eq!(share(PortConnectionOwner::Gateway, limits.recent_total), 2);
        assert_eq!(share(PortConnectionOwner::Node, limits.recent_total), 3);
        assert_eq!(share(PortConnectionOwner::Gateway, limits.caller_totals), 1);
        assert_eq!(share(PortConnectionOwner::Node, limits.caller_totals), 2);
    }

    #[test]
    fn a_datagram_larger_than_the_deployment_allows_is_dropped_and_counted() {
        let budget = Arc::new(UdpBudget::for_limits(&PortLimits {
            udp_datagram_bytes: 100,
            ..PortLimits::SHIPPED
        }));
        assert!(
            budget
                .hold(datagram(1, Bytes::from(vec![0; 101])))
                .is_none()
        );
        assert_eq!(budget.dropped().oversized, 1);
        let held = budget.hold(datagram(1, Bytes::from(vec![0; 100]))).unwrap();
        assert_eq!(held.delivered().payload.len(), 100);
        assert_eq!(budget.dropped().oversized, 1);
    }
}
