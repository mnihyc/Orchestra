//! Admitted TCP and per-sender UDP sockets served through the existing machine tunnel.

use crate::policy::PolicyStore;
use crate::tunnels::TunnelServer;
use async_trait::async_trait;
use bytes::Bytes;
use orch_api_contract::ports::{PortConnectionInside, PortSnapshotUdpDropped, PortTransport};
use orch_envhost::environment::{EnvHost, Environment};
use orch_envhost::ports_sockets;
use orch_link::ports::count;
use orch_link::tunnels::channel::{IncomingTunnel, Tunnel};
use orch_link_contract::ports::PortDatagram;
use orch_link_contract::tunnels::{OPEN_TIMEOUT_SECS, OpenErrorCode};
use orch_link_contract::version::PortLimits;
use orch_types::{Clock, Id};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::Semaphore;
use tokio::time::Instant;

/// Recheck current attachment and local ceiling at the ordinary carrier tick.
const RECHECK: Duration = Duration::from_millis(250);
struct Capacity {
    tcp: Arc<Semaphore>,
    waiting: Arc<Semaphore>,
    udp: Arc<Semaphore>,
}

struct ActivitySocket {
    socket: tokio::net::TcpStream,
    ports: orch_envhost::ports::Ports,
    clock: Arc<dyn Clock>,
}
impl ActivitySocket {
    fn used(&self) {
        self.ports.activity(orch_types::Time::new(self.clock.now()));
    }
}
impl AsyncRead for ActivitySocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.socket).poll_read(cx, buffer);
        if matches!(result, Poll::Ready(Ok(()))) && buffer.filled().len() > before {
            self.used();
        }
        result
    }
}
impl AsyncWrite for ActivitySocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.socket).poll_write(cx, bytes);
        if matches!(result,Poll::Ready(Ok(n)) if n>0) {
            self.used();
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.socket).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(cx)
    }
}
impl Capacity {
    /// An environment's capacities, within the port limits its session's gateway stated.
    fn new(limits: &PortLimits) -> Self {
        Self {
            tcp: Arc::new(Semaphore::new(count(limits.connections_max))),
            waiting: Arc::new(Semaphore::new(count(limits.waiting_max))),
            udp: Arc::new(Semaphore::new(count(limits.udp_senders_max))),
        }
    }
}

/// One carrying service shares capacities across all ingresses of each attached environment.
pub struct PortCarrier {
    host: EnvHost,
    policy: Arc<PolicyStore>,
    clock: Arc<dyn Clock>,
    capacity: Mutex<HashMap<Id, Arc<Capacity>>>,
    local_ports: Mutex<Option<std::sync::Weak<crate::local_ports::LocalPorts>>>,
}
impl std::fmt::Debug for PortCarrier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortCarrier").finish_non_exhaustive()
    }
}
impl PortCarrier {
    /// A carrier over the role's existing scoped host and local policy.
    #[must_use]
    pub fn new(host: EnvHost, policy: Arc<PolicyStore>, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            host,
            policy,
            clock,
            capacity: Mutex::new(HashMap::new()),
            local_ports: Mutex::new(None),
        })
    }
    /// Connects actual native socket registrations to this role's inside reader.
    pub(crate) fn local_reader(&self, reader: &Arc<crate::local_ports::LocalPorts>) {
        *self
            .local_ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::downgrade(reader));
    }
    fn track(
        &self,
        id: Id,
        claims: &orch_link_contract::ports::PortTicketClaims,
        environment: &Arc<Environment>,
        inside: PortConnectionInside,
        tunnel: &Tunnel,
    ) -> Option<crate::local_ports::Tracked> {
        self.local_ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()?
            .upgrade()?
            .track(id, claims, environment, inside, tunnel.closer())
    }
    fn local(&self, audience: Option<orch_api_contract::ports::PortOpeningKind>) -> bool {
        self.policy
            .policy()
            .is_none_or(|p| p.permits_port(audience))
    }
    fn current(
        &self,
        environment: &Arc<Environment>,
        generation: i64,
        audience: Option<orch_api_contract::ports::PortOpeningKind>,
    ) -> bool {
        self.local(audience)
            && environment.generation() == generation
            && self
                .host
                .environment(environment.id())
                .is_ok_and(|now| Arc::ptr_eq(&now, environment))
    }
    fn activity(&self, environment: &Environment) {
        environment
            .ports()
            .activity(orch_types::Time::new(self.clock.now()));
    }
}

#[async_trait]
impl TunnelServer for PortCarrier {
    async fn serve(&self, incoming: IncomingTunnel) {
        let connection_id = incoming.claims.jti;
        let Some(claims) = incoming.claims.port.clone() else {
            incoming.refuse(
                OpenErrorCode::BadTicket,
                "a port needs its signed admission facts",
            );
            return;
        };
        if claims.opening_id.is_some() != claims.audience.is_some() {
            incoming.refuse(
                OpenErrorCode::BadTicket,
                "a wider port opening needs its signed audience",
            );
            return;
        }
        if claims.host_template.is_some()
            && claims.audience != Some(orch_api_contract::ports::PortOpeningKind::Host)
        {
            incoming.refuse(
                OpenErrorCode::BadTicket,
                "a public listener template needs the Host audience",
            );
            return;
        }
        if !self.local(claims.audience) {
            incoming.refuse(
                OpenErrorCode::Refused,
                "ports (environment role): this audience is off in the machine operator's policy",
            );
            return;
        }
        let Ok(environment) = self.host.environment(claims.environment_id) else {
            incoming.refuse(OpenErrorCode::Refused, "the environment is not attached");
            return;
        };
        if !self.current(&environment, claims.generation, claims.audience) {
            incoming.refuse(
                OpenErrorCode::Refused,
                "the environment generation or current attachment changed",
            );
            return;
        }
        let limits = incoming.port_limits();
        let capacity = self
            .capacity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(claims.environment_id)
            .or_insert_with(|| Arc::new(Capacity::new(&limits)))
            .clone();
        match claims.transport {
            PortTransport::Tcp => {
                let Ok(waiting) = capacity.waiting.clone().try_acquire_owned() else {
                    incoming.refuse(
                        OpenErrorCode::TooManyChannels,
                        "the waiting TCP slots are full",
                    );
                    return;
                };
                let connection = tokio::time::timeout(
                    Duration::from_secs(u64::from(OPEN_TIMEOUT_SECS)),
                    async {
                        let permit = capacity.tcp.clone().acquire_owned().await.map_err(|_| ())?;
                        let socket = ports_sockets::tcp(environment.ports(), claims.port)
                            .await
                            .map_err(|_| ())?;
                        Ok::<_, ()>((permit, socket))
                    },
                )
                .await;
                let Ok(Ok((_permit, mut socket))) = connection else {
                    incoming.refuse(
                        OpenErrorCode::Refused,
                        "the current owned TCP socket is unavailable",
                    );
                    return;
                };
                if !self.current(&environment, claims.generation, claims.audience) {
                    incoming.refuse(
                        OpenErrorCode::Refused,
                        "the current attachment changed while connecting",
                    );
                    return;
                }
                let Ok(peer) = socket.local_addr() else {
                    incoming.refuse(OpenErrorCode::Refused, "the inside peer is unavailable");
                    return;
                };
                if claims
                    .host_template
                    .as_ref()
                    .is_some_and(|template| template.proxy_protocol)
                {
                    let outside = claims
                        .source
                        .address
                        .parse::<std::net::IpAddr>()
                        .ok()
                        .zip(socket.peer_addr().ok())
                        .map(|(ip, target)| {
                            (std::net::SocketAddr::new(ip, claims.source.port), target)
                        });
                    let Some((outside, target)) = outside else {
                        incoming.refuse(
                            OpenErrorCode::Refused,
                            "the actual host peer is unavailable",
                        );
                        return;
                    };
                    if socket
                        .write_all(&orch_link::ports::proxy_v2(outside, target))
                        .await
                        .is_err()
                    {
                        incoming.refuse(
                            OpenErrorCode::Refused,
                            "the receiver's PROXY prefix could not be written",
                        );
                        return;
                    }
                }
                let Ok(mut tunnel) = incoming.accept_port(PortConnectionInside {
                    address: peer.ip().to_string(),
                    port: peer.port(),
                }) else {
                    return;
                };
                drop(waiting);
                let _peer = self.track(
                    connection_id,
                    &claims,
                    &environment,
                    PortConnectionInside {
                        address: peer.ip().to_string(),
                        port: peer.port(),
                    },
                    &tunnel,
                );
                self.activity(&environment);
                let mut socket = ActivitySocket {
                    socket,
                    ports: environment.ports().clone(),
                    clock: Arc::clone(&self.clock),
                };
                let mut tick = tokio::time::interval(RECHECK);
                let copy = tokio::io::copy_bidirectional(&mut tunnel, &mut socket);
                tokio::pin!(copy);
                loop {
                    tokio::select! {
                        _=tick.tick()=>if !self.current(&environment,claims.generation,claims.audience) {break;},
                        _=&mut copy=>break,
                    }
                }
            }
            PortTransport::Udp => {
                let Ok(_permit) = capacity.udp.clone().try_acquire_owned() else {
                    environment.ports().udp_dropped(PortSnapshotUdpDropped {
                        senders: 1,
                        ..PortSnapshotUdpDropped::default()
                    });
                    incoming.refuse(
                        OpenErrorCode::TooManyChannels,
                        "the UDP sender slots are full",
                    );
                    return;
                };
                let Ok(socket) =
                    ports_sockets::udp(Arc::new(environment.ports().clone()), claims.port).await
                else {
                    incoming.refuse(
                        OpenErrorCode::Refused,
                        "the current owned UDP binding is unavailable",
                    );
                    return;
                };
                if !self.current(&environment, claims.generation, claims.audience) {
                    incoming.refuse(
                        OpenErrorCode::Refused,
                        "the current attachment changed while connecting",
                    );
                    return;
                }
                let Ok(peer) = socket.inside_peer() else {
                    incoming.refuse(OpenErrorCode::Refused, "the inside peer is unavailable");
                    return;
                };
                let Ok(tunnel) = incoming.accept_port(PortConnectionInside {
                    address: peer.ip().to_string(),
                    port: peer.port(),
                }) else {
                    return;
                };
                let _peer = self.track(
                    connection_id,
                    &claims,
                    &environment,
                    PortConnectionInside {
                        address: peer.ip().to_string(),
                        port: peer.port(),
                    },
                    &tunnel,
                );
                if let Some(budget) = tunnel.datagram_budget() {
                    environment
                        .ports()
                        .carrier_drops(Arc::new(move || budget.dropped()));
                }
                self.activity(&environment);
                let idle = Duration::from_secs(u64::from(limits.udp_idle_secs));
                self.udp(
                    &environment,
                    claims.generation,
                    claims.audience,
                    &socket,
                    &tunnel,
                    idle,
                )
                .await;
            }
        }
    }
}
impl PortCarrier {
    async fn udp(
        &self,
        environment: &Arc<Environment>,
        generation: i64,
        audience: Option<orch_api_contract::ports::PortOpeningKind>,
        socket: &ports_sockets::PortUdp,
        tunnel: &Tunnel,
        idle: Duration,
    ) {
        let mut buffer = vec![0; PortDatagram::MAX_BYTES + 1];
        let mut tick = tokio::time::interval(RECHECK);
        let mut last = Instant::now();
        loop {
            // Keep the native ownership proof/read in progress across the control tick.
            // Cancelling it each tick can starve packets when the scoped reader is busy.
            let bytes = {
                let received = socket.recv(&mut buffer);
                tokio::pin!(received);
                loop {
                    tokio::select! {
                        _=tokio::time::sleep_until(last+idle)=>return,
                        _=tick.tick()=>if !self.current(environment,generation,audience) || tunnel.is_closed() {return;},
                        packet=tunnel.recv_datagram()=>{
                            let Ok(packet)=packet else {return;};
                            last=Instant::now();
                            if socket.send(&packet).await.is_err() {environment.ports().udp_dropped(PortSnapshotUdpDropped {local_send:1,..PortSnapshotUdpDropped::default()});continue;}
                            self.activity(environment);
                        },
                        result=&mut received=>{
                            let Ok(bytes)=result else {return;};
                            break bytes;
                        },
                    }
                }
            };
            last = Instant::now();
            if bytes > PortDatagram::MAX_BYTES {
                environment.ports().udp_dropped(PortSnapshotUdpDropped {
                    oversized: 1,
                    ..PortSnapshotUdpDropped::default()
                });
                continue;
            }
            if tunnel
                .send_datagram(Bytes::copy_from_slice(&buffer[..bytes]))
                .is_err()
            {
                return;
            }
            self.activity(environment);
        }
    }
}
