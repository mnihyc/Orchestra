//! What a gateway does with the channels of the sessions it holds.
//!
//! A gateway carries a tunnel between two machines without looking into
//! it. What it does look at is the opening: a machine presents a ticket the
//! system signed, and the gateway checks the signature, that the ticket is
//! the presenter's and not past its time, that the destination has a session,
//! and that the session has channels to spare. Then it offers the channel to
//! the destination, and carries on the answer.
//!
//! Each end numbers its channels by itself, machines with odd numbers and the
//! gateway, towards a destination, with even ones; the relay keeps one route
//! for each tunnel and translates the number of every frame that passes
//! through. Data is passed on as it comes, in the queue of the session it is
//! meant for. Credit, acknowledgements and end-of-stream travel end to end;
//! the gateway holds no bytes of a tunnel beyond the frames in a queue, which
//! credit keeps within a window.
//!
//! A route outlives the sessions at its ends. When a session drops, the other
//! end is left to wait, and the route is kept for a minute; if the opener
//! asks, on a new session, to carry the tunnel on, the request is passed to
//! the destination, as soon as it has a session. If the minute passes, both
//! ends are told that the tunnel is reset. A tunnel that cannot be resumed
//! (a terminal) ends at once when a session at either end drops.
//!
//! The other end of a terminal is the system, over the backplane: it is a
//! route end like a machine, reached through an [`ApiPort`].

use crate::ports::{Datagram, PortRecords, UdpBudget};
use crate::session::SessionSender;
use bytes::Bytes;
use orch_api_contract::ports::{PortConnectionInside, PortConnectionOwner};
use orch_link_contract::backplane::{
    BackplaneData, BackplaneFrame, ChannelText, TunClose, TunCredit, TunEof, TunOpenErr, TunOpenOk,
    TunText,
};
use orch_link_contract::frames::{DataFrame, Frame};
use orch_link_contract::ports::{BackplanePortDatagram, PortControl, PortDatagram};
use orch_link_contract::tunnels::{
    ChAck, Close, CloseCode, Credit, Eof, OPEN_TIMEOUT_SECS, Open, OpenErr, OpenErrorCode,
    OpenOffer, OpenOk, PtyExit, PtyResize, RESUME_GRACE_SECS, Resume, ResumeOk, Side, TicketClaims,
    TicketSource, TunnelKind,
};
use orch_link_contract::version::Limits;
use orch_pki::tickets::TicketVerifier;
use orch_types::{Clock, Id};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// Where a gateway sends what is for the system's end of a terminal.
pub trait ApiPort: Send + Sync {
    /// A text frame of a terminal's channel.
    fn text(&self, frame: BackplaneFrame);
    /// A frame of a terminal's data.
    fn data(&self, data: BackplaneData);
    /// One leased UDP packet through the same backplane, never the stream queue.
    fn datagram(&self, _machine: Id, _packet: Datagram) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RouteId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Machine { id: Id, channel: Option<u32> },
    Api { machine: Id, channel: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Opener,
    Destination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Opening(Instant),
    Open,
    Suspended(Instant),
}

struct Route {
    claims: TicketClaims,
    /// The original signed publication ticket; each admitted host socket has its own claims.jti.
    host_template_jti: Option<Id>,
    cid: Option<Id>,
    opener: End,
    destination: End,
    state: State,
    /// A request to carry the tunnel on that waits for the destination to
    /// have a session.
    waiting_resume: Option<Resume>,
}

struct MachineSession {
    session_id: Id,
    sender: SessionSender,
    next_even: u32,
}

/// The routes of a gateway.
pub struct Relay {
    port_available: bool,
    port_records: Arc<PortRecords>,
    udp: HashMap<Id, Arc<UdpBudget>>,
    port_uses: HashMap<Id, i64>,
    limits: Limits,
    grace: Duration,
    clock: Arc<dyn Clock>,
    verifier: Option<TicketVerifier>,
    api: Option<Arc<dyn ApiPort>>,
    sessions: HashMap<Id, MachineSession>,
    routes: HashMap<RouteId, Route>,
    by_channel: HashMap<(Id, u32), (RouteId, Role)>,
    by_api_channel: HashMap<(Id, u32), RouteId>,
    by_cid: HashMap<Id, RouteId>,
    next_route: u64,
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay")
            .field("routes", &self.routes.len())
            .finish_non_exhaustive()
    }
}

impl Relay {
    /// A relay with no routes, for sessions with `limits`; tickets are
    /// judged by `clock`.
    #[must_use]
    pub fn new(limits: Limits, clock: Arc<dyn Clock>) -> Self {
        Self {
            limits,
            port_available: false,
            port_records: Arc::new(PortRecords::with_limits(
                PortConnectionOwner::Gateway,
                &limits.ports,
            )),
            udp: HashMap::new(),
            port_uses: HashMap::new(),
            grace: Duration::from_secs(u64::from(RESUME_GRACE_SECS)),
            clock,
            verifier: None,
            api: None,
            sessions: HashMap::new(),
            routes: HashMap::new(),
            by_channel: HashMap::new(),
            by_api_channel: HashMap::new(),
            by_cid: HashMap::new(),
            next_route: 1,
        }
    }

    /// How long a route waits for its tunnel to be resumed.
    pub fn set_grace(&mut self, grace: Duration) {
        self.grace = grace;
    }

    /// The public key tickets are checked with, as the system gave it over
    /// the backplane.
    pub fn set_verifier(&mut self, verifier: TicketVerifier) {
        self.verifier = Some(verifier);
    }

    /// The through-Orch live record, shared with bounded current-query readers.
    #[must_use]
    pub fn port_records(&self) -> Arc<PortRecords> {
        Arc::clone(&self.port_records)
    }

    /// Fences only port admission and channels when current control is lost.
    pub fn port_control(&mut self, available: bool) {
        self.port_available = available;
        for session in self.sessions.values() {
            let _ = session
                .sender
                .send(Frame::PortControl(PortControl { available }));
        }
        if !available {
            let ports = self
                .routes
                .iter()
                .filter(|(_, r)| r.claims.kind == TunnelKind::Port)
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            for id in ports {
                if let Some(route) = self.routes.get(&id) {
                    for end in [route.opener, route.destination] {
                        if let Some(channel) = Self::channel_of(end) {
                            self.end_text(
                                end,
                                Frame::Close(Close {
                                    channel,
                                    code: CloseCode::Reset,
                                    detail: "current port control was lost".into(),
                                }),
                            );
                        }
                    }
                }
                self.remove(id);
            }
            self.udp.clear();
        }
    }

    /// Where the system's end of a terminal is reached.
    pub fn set_api(&mut self, api: Arc<dyn ApiPort>) {
        self.api = Some(api);
    }

    /// How many routes are held.
    #[must_use]
    pub fn routes(&self) -> usize {
        self.routes.len()
    }

    /// Live or resumable routes by transport purpose, for the gateway's bounded operator gauges.
    pub fn measured_routes(&self) -> Vec<(&'static str, usize)> {
        use orch_link_contract::tunnels::TunnelKind;
        [
            TunnelKind::Exec,
            TunnelKind::Llm,
            TunnelKind::Mcp,
            TunnelKind::Pty,
            TunnelKind::Browser,
            TunnelKind::Rfb,
            TunnelKind::Port,
        ]
        .into_iter()
        .map(|kind| {
            (
                kind.as_str(),
                self.routes
                    .values()
                    .filter(|route| {
                        route.claims.kind == kind && !matches!(route.state, State::Opening(_))
                    })
                    .count(),
            )
        })
        .collect()
    }

    /// How many routes a machine's session is an end of.
    #[must_use]
    pub fn channels_of(&self, machine: Id) -> usize {
        self.by_channel
            .keys()
            .filter(|(id, _)| *id == machine)
            .count()
    }

    fn end_text(&self, end: End, frame: Frame) {
        match end {
            End::Machine {
                id,
                channel: Some(_),
            } => {
                if let Some(session) = self.sessions.get(&id) {
                    // What ends a stream stays behind the data before it.
                    let ordered = matches!(
                        &frame,
                        Frame::Eof(_)
                            | Frame::Close(Close {
                                code: CloseCode::Ok,
                                ..
                            })
                    );
                    let _ = if ordered {
                        session.sender.send_ordered(frame)
                    } else {
                        session.sender.send(frame)
                    };
                }
            }
            End::Machine { channel: None, .. } => {}
            End::Api { machine, channel } => {
                let Some(api) = &self.api else {
                    return;
                };
                let text = match frame {
                    Frame::OpenOk(OpenOk { cid, inside, .. }) => {
                        BackplaneFrame::TunOpenOk(TunOpenOk {
                            machine_id: machine,
                            channel,
                            cid,
                            inside,
                        })
                    }
                    Frame::OpenErr(OpenErr { code, detail, .. }) => {
                        BackplaneFrame::TunOpenErr(TunOpenErr {
                            machine_id: machine,
                            channel,
                            code,
                            detail,
                        })
                    }
                    Frame::Credit(Credit { n, .. }) => BackplaneFrame::TunCredit(TunCredit {
                        machine_id: machine,
                        channel,
                        n,
                    }),
                    Frame::Eof(_) => BackplaneFrame::TunEof(TunEof {
                        machine_id: machine,
                        channel,
                    }),
                    Frame::Close(Close { code, detail, .. }) => {
                        BackplaneFrame::TunClose(TunClose {
                            machine_id: machine,
                            channel,
                            code,
                            detail,
                        })
                    }
                    Frame::PtyExit(PtyExit { code, .. }) => BackplaneFrame::TunText(TunText {
                        machine_id: machine,
                        channel,
                        frame: ChannelText::PtyExit(PtyExit { channel, code }),
                    }),
                    Frame::PtyResize(PtyResize { cols, rows, .. }) => {
                        BackplaneFrame::TunText(TunText {
                            machine_id: machine,
                            channel,
                            frame: ChannelText::PtyResize(PtyResize {
                                channel,
                                cols,
                                rows,
                            }),
                        })
                    }
                    _ => return,
                };
                api.text(text);
            }
        }
    }

    fn end_data(&self, end: End, offset: u64, payload: Bytes) {
        match end {
            End::Machine {
                id,
                channel: Some(channel),
            } => {
                let Some(session) = self.sessions.get(&id) else {
                    return;
                };
                // A frame is sized to the link it goes over: one that came off
                // a fast link is cut into pieces a thin one carries in a
                // quarter of a second, so that control frames never wait for a
                // long one.
                let piece = session.sender.data_frame_bytes().max(1);
                let mut at = 0;
                while at < payload.len() {
                    let n = piece.min(payload.len() - at);
                    let _ = session.sender.send_data(&DataFrame {
                        channel,
                        offset: offset + at as u64,
                        payload: payload.slice(at..at + n),
                    });
                    at += n;
                }
            }
            End::Machine { channel: None, .. } => {}
            End::Api { machine, channel } => {
                if let Some(api) = &self.api {
                    api.data(BackplaneData {
                        machine_id: machine,
                        channel,
                        offset,
                        payload,
                    });
                }
            }
        }
    }

    /// Re-labels `frame` for the channel number `to`.
    fn relabel(frame: Frame, to: u32) -> Frame {
        match frame {
            Frame::Credit(c) => Frame::Credit(Credit { channel: to, ..c }),
            Frame::Eof(_) => Frame::Eof(Eof { channel: to }),
            Frame::Close(c) => Frame::Close(Close { channel: to, ..c }),
            Frame::ChAck(a) => Frame::ChAck(ChAck { channel: to, ..a }),
            Frame::PtyResize(r) => Frame::PtyResize(PtyResize { channel: to, ..r }),
            Frame::PtyExit(e) => Frame::PtyExit(PtyExit { channel: to, ..e }),
            Frame::OpenOk(o) => Frame::OpenOk(OpenOk { channel: to, ..o }),
            Frame::OpenErr(e) => Frame::OpenErr(OpenErr { channel: to, ..e }),
            Frame::ResumeOk(r) => Frame::ResumeOk(ResumeOk { channel: to, ..r }),
            other => other,
        }
    }

    fn channel_of(end: End) -> Option<u32> {
        match end {
            End::Machine { channel, .. } => channel,
            End::Api { channel, .. } => Some(channel),
        }
    }

    fn lookup(&self, machine: Id, channel: u32) -> Option<(RouteId, Role)> {
        self.by_channel.get(&(machine, channel)).copied()
    }

    fn remove(&mut self, id: RouteId) {
        let Some(route) = self.routes.remove(&id) else {
            return;
        };
        if let Some(port) = &route.claims.port {
            self.port_records
                .ended(port.environment_id, route.claims.jti, self.clock.time());
        }
        for end in [route.opener, route.destination] {
            match end {
                End::Machine {
                    id: machine,
                    channel: Some(channel),
                } => {
                    self.by_channel.remove(&(machine, channel));
                }
                End::Machine { channel: None, .. } => {}
                End::Api { machine, channel } => {
                    self.by_api_channel.remove(&(machine, channel));
                }
            }
        }
        if let Some(cid) = route.cid {
            self.by_cid.remove(&cid);
        }
    }

    /// A machine's session stands.
    pub fn session_up(&mut self, machine: Id, session_id: Id, sender: SessionSender) {
        let _ = sender.send(Frame::PortControl(PortControl {
            available: self.port_available,
        }));
        self.sessions.insert(
            machine,
            MachineSession {
                session_id,
                sender,
                next_even: Side::Gateway.first(),
            },
        );
        // Requests to carry a tunnel on that waited for this machine.
        let waiting: Vec<RouteId> = self
            .routes
            .iter()
            .filter(|(_, route)| {
                route.waiting_resume.is_some()
                    && matches!(route.destination, End::Machine { id, .. } if id == machine)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in waiting {
            self.forward_resume(id);
        }
    }

    /// A machine's session ended.
    pub fn session_gone(&mut self, machine: Id, session_id: Id) {
        if self
            .sessions
            .get(&machine)
            .map(|session| session.session_id)
            != Some(session_id)
        {
            return;
        }
        self.sessions.remove(&machine);
        let now = Instant::now();
        let touched: Vec<RouteId> = self
            .routes
            .iter()
            .filter(|(_, route)| {
                matches!(route.opener, End::Machine { id, .. } if id == machine)
                    || matches!(route.destination, End::Machine { id, .. } if id == machine)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in touched {
            let Some(route) = self.routes.get(&id) else {
                continue;
            };
            let opener_gone = matches!(route.opener, End::Machine { id, .. } if id == machine);
            let kind = route.claims.kind;
            let state = route.state;
            let (opener, destination) = (route.opener, route.destination);
            // Forget the numbers of the ended session; the route stays.
            for end in [opener, destination] {
                if let End::Machine {
                    id: owner,
                    channel: Some(channel),
                } = end
                    && owner == machine
                {
                    self.by_channel.remove(&(owner, channel));
                }
            }
            if let Some(route) = self.routes.get_mut(&id) {
                for end in [&mut route.opener, &mut route.destination] {
                    if let End::Machine { id: owner, channel } = end
                        && *owner == machine
                    {
                        *channel = None;
                    }
                }
            }
            let reset = |relay: &Self, end: End, channel_of_other: Option<u32>| {
                if let Some(channel) = channel_of_other {
                    relay.end_text(
                        end,
                        Frame::Close(Close {
                            channel,
                            code: CloseCode::Reset,
                            detail: "the session at the other end ended".to_owned(),
                        }),
                    );
                }
            };
            match (kind, state) {
                (TunnelKind::Pty | TunnelKind::Browser | TunnelKind::Rfb, _) => {
                    let survivor = if opener_gone { destination } else { opener };
                    reset(self, survivor, Self::channel_of(survivor));
                    self.remove(id);
                }
                (_, State::Opening(_)) => {
                    if opener_gone {
                        let survivor = destination;
                        reset(self, survivor, Self::channel_of(survivor));
                    } else if let Some(channel) = Self::channel_of(opener) {
                        self.end_text(
                            opener,
                            Frame::OpenErr(OpenErr {
                                channel,
                                code: OpenErrorCode::PeerOffline,
                                detail: "the destination's session ended".to_owned(),
                            }),
                        );
                    }
                    self.remove(id);
                }
                (_, _) => {
                    if let Some(route) = self.routes.get_mut(&id)
                        && !matches!(route.state, State::Suspended(_))
                    {
                        route.state = State::Suspended(now);
                    }
                }
            }
        }
    }

    /// A text frame of a tunnel from a machine.
    pub fn on_frame(&mut self, from: Id, session_id: Id, frame: Frame) {
        if self.sessions.get(&from).map(|session| session.session_id) != Some(session_id) {
            return;
        }
        match frame {
            Frame::Open(Open::Request(request)) => {
                self.open(from, request.channel, &request.ticket, request.host_peer)
            }
            Frame::OpenOk(ok) => self.answer(from, ok.channel, Ok((ok.cid, ok.inside))),
            Frame::OpenErr(err) => self.answer(from, err.channel, Err(err)),
            Frame::Resume(resume) => self.resume(from, resume),
            Frame::ResumeOk(ok) => {
                if let Some((id, Role::Destination)) = self.lookup(from, ok.channel)
                    && let Some(route) = self.routes.get_mut(&id)
                {
                    route.state = State::Open;
                    let opener = route.opener;
                    if let Some(channel) = Self::channel_of(opener) {
                        self.end_text(
                            opener,
                            Frame::ResumeOk(ResumeOk {
                                channel,
                                recv: ok.recv,
                            }),
                        );
                    }
                }
            }
            frame @ (Frame::Credit(_)
            | Frame::Eof(_)
            | Frame::Close(_)
            | Frame::ChAck(_)
            | Frame::PtyResize(_)
            | Frame::PtyExit(_)) => self.pass(from, frame),
            _ => {}
        }
    }

    /// A frame of tunnel data from a machine.
    pub fn on_data(&mut self, from: Id, session_id: Id, data: DataFrame) {
        if self.sessions.get(&from).map(|session| session.session_id) != Some(session_id) {
            return;
        }
        if let Some((id, role)) = self.lookup(from, data.channel)
            && let Some(route) = self.routes.get(&id)
        {
            if let Some(port) = &route.claims.port {
                if port.transport != orch_api_contract::ports::PortTransport::Tcp {
                    return;
                }
                self.port_records.traffic(
                    port.environment_id,
                    route.claims.jti,
                    role == Role::Opener,
                    data.payload.len(),
                    false,
                    self.clock.time(),
                );
            }
            let other = match role {
                Role::Opener => route.destination,
                Role::Destination => route.opener,
            };
            metrics::counter!("orch_tunnel_bytes_total","kind"=>route.claims.kind.as_str())
                .increment(data.payload.len() as u64);
            self.end_data(other, data.offset, data.payload);
        }
    }

    /// Relays a complete UDP packet only on its authenticated admitted association.
    pub fn on_datagram(&mut self, from: Id, session: Id, packet: Datagram) {
        if self.sessions.get(&from).map(|s| s.session_id) != Some(session) {
            return;
        }
        let Some((id, role)) = self.lookup(from, packet.frame.channel) else {
            return;
        };
        self.pass_datagram(id, role, packet.delivered());
    }

    fn pass_datagram(&mut self, id: RouteId, role: Role, frame: PortDatagram) {
        let Some(route) = self.routes.get(&id) else {
            return;
        };
        let Some(port) = &route.claims.port else {
            return;
        };
        if !self.port_available
            || route.state != State::Open
            || port.transport != orch_api_contract::ports::PortTransport::Udp
        {
            return;
        }
        let other = if role == Role::Opener {
            route.destination
        } else {
            route.opener
        };
        self.port_records.traffic(
            port.environment_id,
            route.claims.jti,
            role == Role::Opener,
            frame.payload.len(),
            false,
            self.clock.time(),
        );
        let budget = self
            .udp
            .entry(port.environment_id)
            .or_insert_with(|| Arc::new(UdpBudget::for_limits(&self.limits.ports)))
            .clone();
        let Some(channel) = Self::channel_of(other) else {
            return;
        };
        let Some(packet) = budget.hold(PortDatagram {
            channel,
            payload: frame.payload,
        }) else {
            return;
        };
        metrics::counter!("orch_tunnel_bytes_total","kind"=>route.claims.kind.as_str())
            .increment(packet.frame.payload.len() as u64);
        match other {
            End::Machine { id, .. } => {
                if let Some(session) = self.sessions.get(&id) {
                    let _ = session.sender.send_datagram(packet);
                }
            }
            End::Api { machine, .. } => {
                if let Some(api) = &self.api {
                    api.datagram(machine, packet);
                }
            }
        }
    }

    /// Relays one complete API packet through the same machine association.
    pub fn on_api_datagram(&mut self, data: BackplanePortDatagram) {
        if let Some(id) = self
            .by_api_channel
            .get(&(data.machine_id, data.datagram.channel))
            .copied()
        {
            self.pass_datagram(id, Role::Opener, data.datagram);
        }
    }

    fn refuse(&self, to: Id, channel: u32, code: OpenErrorCode, detail: &str) {
        if let Some(session) = self.sessions.get(&to) {
            let _ = session.sender.send(Frame::OpenErr(OpenErr {
                channel,
                code,
                detail: detail.to_owned(),
            }));
        }
    }

    /// A machine presents a ticket to open a tunnel.
    fn open(
        &mut self,
        from: Id,
        channel: u32,
        ticket: &str,
        host_peer: Option<orch_link_contract::ports::HostPortPeer>,
    ) {
        let mut claims = match self.check_ticket(ticket, TicketSource::Machine(from)) {
            Ok(claims) => claims,
            Err(code) => return self.refuse(from, channel, code, "the ticket is not accepted"),
        };
        let template = claims
            .port
            .as_ref()
            .is_some_and(|port| port.host_template.is_some());
        let host_template_jti = template.then_some(claims.jti);
        if template {
            let Some(peer) = host_peer else {
                return self.refuse(
                    from,
                    channel,
                    OpenErrorCode::BadTicket,
                    "a host accept needs its actual outside peer",
                );
            };
            let Some(port) = claims.port.as_mut() else {
                return;
            };
            if port.allocation_revision.is_none()
                || port.opening_id.is_none()
                || !matches!(
                    &port.source.caller,
                    orch_api_contract::ports::PortCaller::Address
                )
                || peer.use_id == claims.jti
                || peer.peer.address.parse::<std::net::IpAddr>().is_err()
            {
                return self.refuse(
                    from,
                    channel,
                    OpenErrorCode::BadTicket,
                    "the host socket facts do not match a publication template",
                );
            }
            port.source.address = peer.peer.address;
            port.source.port = peer.peer.port;
            // The signature delegates raw accepts only to its named node; every socket gets a
            // distinct reference, never a chosen person/link identity or a different target.
            claims.jti = peer.use_id;
            claims.service = orch_link_contract::tunnels::Service::Port {
                use_id: peer.use_id,
            }
            .to_string();
        } else if host_peer.is_some() {
            return self.refuse(
                from,
                channel,
                OpenErrorCode::BadTicket,
                "only a host publication delegates outside peer observation",
            );
        }
        if !Side::Machine.owns(channel) || self.lookup(from, channel).is_some() {
            return self.refuse(
                from,
                channel,
                OpenErrorCode::BadTicket,
                "that channel number cannot be used",
            );
        }
        if self.channels_of(from) >= self.limits.max_channels as usize {
            return self.refuse(
                from,
                channel,
                OpenErrorCode::TooManyChannels,
                "too many channels",
            );
        }
        let opener = End::Machine {
            id: from,
            channel: Some(channel),
        };
        self.start(opener, claims, host_template_jti);
    }

    /// The system presents a ticket to open a tunnel that ends at a terminal.
    pub fn api_open(&mut self, machine: Id, channel: u32, ticket: &str) {
        let refuse = |relay: &Self, code: OpenErrorCode, detail: &str| {
            relay.end_text(
                End::Api { machine, channel },
                Frame::OpenErr(OpenErr {
                    channel,
                    code,
                    detail: detail.to_owned(),
                }),
            );
        };
        let claims = match self.check_ticket(ticket, TicketSource::Api) {
            Ok(claims)
                if claims.dst == machine
                    && claims
                        .port
                        .as_ref()
                        .is_none_or(|port| port.host_template.is_none()) =>
            {
                claims
            }
            Ok(_) => {
                return refuse(
                    self,
                    OpenErrorCode::BadTicket,
                    "the ticket is for another machine",
                );
            }
            Err(code) => return refuse(self, code, "the ticket is not accepted"),
        };
        if self.by_api_channel.contains_key(&(machine, channel)) {
            return refuse(
                self,
                OpenErrorCode::BadTicket,
                "that channel number is in use",
            );
        }
        self.start(End::Api { machine, channel }, claims, None);
    }

    fn check_ticket(
        &self,
        ticket: &str,
        presenter: TicketSource,
    ) -> Result<TicketClaims, OpenErrorCode> {
        let verifier = self.verifier.as_ref().ok_or(OpenErrorCode::BadTicket)?;
        verifier.verify(ticket, presenter, self.clock.time())
    }

    fn start(&mut self, opener: End, claims: TicketClaims, host_template_jti: Option<Id>) {
        let refuse = |relay: &Self, code: OpenErrorCode, detail: &str| {
            if let Some(channel) = Self::channel_of(opener) {
                relay.end_text(
                    opener,
                    Frame::OpenErr(OpenErr {
                        channel,
                        code,
                        detail: detail.to_owned(),
                    }),
                );
            }
        };
        if let Some(port) = &claims.port {
            let now = self.clock.time().unix_millis().div_euclid(1_000);
            self.port_uses.retain(|_, expires| *expires > now);
            if port.host_template.is_none() && self.port_uses.contains_key(&claims.jti) {
                return refuse(
                    self,
                    OpenErrorCode::BadTicket,
                    "this single-use port admission was already consumed",
                );
            }
            if !self.port_available {
                return refuse(
                    self,
                    OpenErrorCode::Refused,
                    "current port control is unavailable",
                );
            }
            let current = self
                .routes
                .values()
                .filter(|r| {
                    r.claims.port.as_ref().is_some_and(|p| {
                        p.environment_id == port.environment_id && p.transport == port.transport
                    })
                })
                .collect::<Vec<_>>();
            let bounds = self.limits.ports;
            let max = if port.transport == orch_api_contract::ports::PortTransport::Udp {
                crate::ports::count(bounds.udp_senders_max)
            } else {
                crate::ports::count(bounds.connections_max)
                    .saturating_add(crate::ports::count(bounds.waiting_max))
            };
            if current.len() >= max || current.iter().any(|r| r.claims.jti == claims.jti) {
                return refuse(
                    self,
                    OpenErrorCode::TooManyChannels,
                    "the admitted environment transport is full or this use was already taken",
                );
            }
        }
        let destination_machine = claims.dst;
        if claims
            .port
            .as_ref()
            .is_some_and(|port| port.host_template.is_none())
        {
            self.port_uses.insert(claims.jti, claims.exp);
        }
        if self.channels_of(destination_machine) >= self.limits.max_channels as usize {
            return refuse(
                self,
                OpenErrorCode::TooManyChannels,
                "the destination has too many channels",
            );
        }
        let Some(first_try) = self
            .sessions
            .get(&destination_machine)
            .map(|session| session.next_even)
        else {
            return refuse(
                self,
                OpenErrorCode::PeerOffline,
                "the destination is not connected",
            );
        };
        let mut channel = first_try;
        while self
            .by_channel
            .contains_key(&(destination_machine, channel))
        {
            channel = Side::Gateway.next(channel);
        }
        if let Some(session) = self.sessions.get_mut(&destination_machine) {
            session.next_even = Side::Gateway.next(channel);
        }
        let id = RouteId(self.next_route);
        self.next_route += 1;
        let destination = End::Machine {
            id: destination_machine,
            channel: Some(channel),
        };
        let offer = Frame::Open(Open::Offer(Box::new(OpenOffer {
            channel,
            kind: claims.kind,
            service: claims.service.clone(),
            from: claims.src,
            claims: claims.clone(),
        })));
        self.routes.insert(
            id,
            Route {
                claims,
                host_template_jti,
                cid: None,
                opener,
                destination,
                state: State::Opening(Instant::now()),
                waiting_resume: None,
            },
        );
        self.by_channel
            .insert((destination_machine, channel), (id, Role::Destination));
        match opener {
            End::Machine {
                id: machine,
                channel: Some(number),
            } => {
                self.by_channel
                    .insert((machine, number), (id, Role::Opener));
            }
            End::Api { machine, channel } => {
                self.by_api_channel.insert((machine, channel), id);
            }
            End::Machine { channel: None, .. } => {}
        }
        self.end_text(destination, offer);
    }

    /// The destination answers an offer.
    fn answer(
        &mut self,
        from: Id,
        channel: u32,
        answer: Result<(Id, Option<PortConnectionInside>), OpenErr>,
    ) {
        let Some((id, Role::Destination)) = self.lookup(from, channel) else {
            // The opener may have cancelled while its receiver established the owned socket.
            // A late acknowledgement must retire that socket, not leave an unreachable channel.
            self.end_text(
                End::Machine {
                    id: from,
                    channel: Some(channel),
                },
                Frame::Close(Close {
                    channel,
                    code: CloseCode::Reset,
                    detail: "the opening no longer has a caller".to_owned(),
                }),
            );
            return;
        };
        let Some(route) = self.routes.get_mut(&id) else {
            return;
        };
        let opener = route.opener;
        let Some(opener_channel) = Self::channel_of(opener) else {
            return;
        };
        match answer {
            Ok((cid, inside)) => {
                if let Some(port) = &route.claims.port {
                    let Some(peer) = inside.clone() else {
                        let destination = route.destination;
                        self.end_text(
                            opener,
                            Frame::OpenErr(OpenErr {
                                channel: opener_channel,
                                code: OpenErrorCode::Refused,
                                detail:
                                    "the port receiver did not establish its actual inside peer"
                                        .into(),
                            }),
                        );
                        self.end_text(
                            destination,
                            Frame::Close(Close {
                                channel,
                                code: CloseCode::Reset,
                                detail: "the inside port peer was not established".into(),
                            }),
                        );
                        self.remove(id);
                        return;
                    };
                    if port.host_template.is_none() {
                        self.port_records
                            .opened(route.claims.jti, port, peer, self.clock.time());
                    }
                }
                route.cid = Some(cid);
                route.state = State::Open;
                self.by_cid.insert(cid, id);
                self.end_text(
                    opener,
                    Frame::OpenOk(OpenOk {
                        channel: opener_channel,
                        cid,
                        inside,
                    }),
                );
            }
            Err(err) => {
                self.end_text(
                    opener,
                    Frame::OpenErr(OpenErr {
                        channel: opener_channel,
                        ..err
                    }),
                );
                self.remove(id);
            }
        }
    }

    /// A frame that travels end to end: credit, end of stream, close,
    /// acknowledgement, a terminal's size or exit.
    fn pass(&mut self, from: Id, frame: Frame) {
        let channel = match &frame {
            Frame::Credit(Credit { channel, .. })
            | Frame::Eof(Eof { channel })
            | Frame::Close(Close { channel, .. })
            | Frame::ChAck(ChAck { channel, .. })
            | Frame::PtyResize(PtyResize { channel, .. })
            | Frame::PtyExit(PtyExit { channel, .. }) => *channel,
            _ => return,
        };
        let closing = matches!(frame, Frame::Close(_));
        let Some((id, role)) = self.lookup(from, channel) else {
            return;
        };
        let Some(route) = self.routes.get(&id) else {
            return;
        };
        let other = match role {
            Role::Opener => route.destination,
            Role::Destination => route.opener,
        };
        if let Some(number) = Self::channel_of(other) {
            self.end_text(other, Self::relabel(frame, number));
        }
        if closing {
            self.remove(id);
        }
    }

    /// The opener asks to carry a tunnel on over its new session.
    fn resume(&mut self, from: Id, resume: Resume) {
        let reset = |relay: &Self, why: &str| {
            if let Some(session) = relay.sessions.get(&from) {
                let _ = session.sender.send(Frame::Close(Close {
                    channel: resume.channel,
                    code: CloseCode::Reset,
                    detail: why.to_owned(),
                }));
            }
        };
        let Ok(claims) = self.check_ticket(&resume.ticket, TicketSource::Machine(from)) else {
            return reset(self, "the ticket is not accepted");
        };
        let Some(&id) = self.by_cid.get(&resume.jti_channel) else {
            return reset(self, "no such tunnel");
        };
        let Some(route) = self.routes.get(&id) else {
            return reset(self, "no such tunnel");
        };
        let opener_is_from =
            matches!(route.opener, End::Machine { id: owner, .. } if owner == from);
        // Host routes retain their observed socket and use reference across a session cut. The
        // signed parent template belongs only to this opener; a different publication cannot resume it.
        let same_service = match route.host_template_jti {
            Some(parent) => {
                parent == claims.jti
                    && claims
                        .port
                        .as_ref()
                        .is_some_and(|port| port.host_template.is_some())
            }
            None => route.claims.jti == claims.jti && route.claims.service == claims.service,
        };
        if !opener_is_from || route.claims.dst != claims.dst || !same_service {
            return reset(self, "that ticket is not the tunnel's");
        }
        if self.by_channel.contains_key(&(from, resume.channel)) {
            return reset(self, "that channel number is in use");
        }
        if let Some(route) = self.routes.get_mut(&id) {
            if let End::Machine { channel: old, .. } = &mut route.opener {
                if let Some(old) = old.take() {
                    self.by_channel.remove(&(from, old));
                }
                *old = Some(resume.channel);
            }
            if !matches!(route.state, State::Suspended(_)) {
                route.state = State::Suspended(Instant::now());
            }
            route.waiting_resume = Some(resume.clone());
        }
        self.by_channel
            .insert((from, resume.channel), (id, Role::Opener));
        self.forward_resume(id);
    }

    /// Passes a request to carry a tunnel on to the destination, if it has a
    /// session; otherwise it waits for one.
    fn forward_resume(&mut self, id: RouteId) {
        let Some(route) = self.routes.get(&id) else {
            return;
        };
        let Some(resume) = route.waiting_resume.clone() else {
            return;
        };
        let End::Machine {
            id: destination,
            channel: held,
        } = route.destination
        else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&destination) else {
            return;
        };
        let channel = match held {
            Some(channel) => channel,
            None => {
                let mut channel = session.next_even;
                while self.by_channel.contains_key(&(destination, channel)) {
                    channel = Side::Gateway.next(channel);
                }
                if let Some(session) = self.sessions.get_mut(&destination) {
                    session.next_even = Side::Gateway.next(channel);
                }
                self.by_channel
                    .insert((destination, channel), (id, Role::Destination));
                channel
            }
        };
        if let Some(route) = self.routes.get_mut(&id) {
            route.destination = End::Machine {
                id: destination,
                channel: Some(channel),
            };
            route.waiting_resume = None;
        }
        if let Some(session) = self.sessions.get(&destination) {
            let _ = session.sender.send(Frame::Resume(Resume {
                channel,
                ticket: resume.ticket,
                jti_channel: resume.jti_channel,
                recv: resume.recv,
            }));
        }
    }

    /// A frame from the system's end of a terminal.
    pub fn on_api_frame(&mut self, frame: BackplaneFrame) {
        match frame {
            BackplaneFrame::TunOpen(open) => {
                self.api_open(open.machine_id, open.channel, &open.ticket)
            }
            BackplaneFrame::TunCredit(TunCredit {
                machine_id,
                channel,
                n,
            }) => {
                self.api_pass(machine_id, channel, Frame::Credit(Credit { channel, n }));
            }
            BackplaneFrame::TunEof(TunEof {
                machine_id,
                channel,
            }) => {
                self.api_pass(machine_id, channel, Frame::Eof(Eof { channel }));
            }
            BackplaneFrame::TunClose(TunClose {
                machine_id,
                channel,
                code,
                detail,
            }) => {
                self.api_pass(
                    machine_id,
                    channel,
                    Frame::Close(Close {
                        channel,
                        code,
                        detail,
                    }),
                );
            }
            BackplaneFrame::TunText(TunText {
                machine_id,
                channel,
                frame,
            }) => {
                let frame = match frame {
                    ChannelText::PtyResize(r) => Frame::PtyResize(PtyResize {
                        channel,
                        cols: r.cols,
                        rows: r.rows,
                    }),
                    ChannelText::PtyExit(e) => Frame::PtyExit(PtyExit {
                        channel,
                        code: e.code,
                    }),
                };
                self.api_pass(machine_id, channel, frame);
            }
            _ => {}
        }
    }

    fn api_pass(&mut self, machine: Id, channel: u32, frame: Frame) {
        let Some(&id) = self.by_api_channel.get(&(machine, channel)) else {
            return;
        };
        let Some(route) = self.routes.get(&id) else {
            return;
        };
        let destination = route.destination;
        let closing = matches!(frame, Frame::Close(_));
        if let Some(number) = Self::channel_of(destination) {
            self.end_text(destination, Self::relabel(frame, number));
        }
        if closing {
            self.remove(id);
        }
    }

    /// Data from the system's end of a terminal.
    pub fn on_api_data(&mut self, data: BackplaneData) {
        let Some(&id) = self.by_api_channel.get(&(data.machine_id, data.channel)) else {
            return;
        };
        if let Some(route) = self.routes.get(&id) {
            self.end_data(route.destination, data.offset, data.payload);
        }
    }

    /// Looks at the clocks: a tunnel that has waited out its grace is reset,
    /// and an opening nobody answered is refused.
    pub fn tick(&mut self, now: Instant) {
        self.port_records.prune(self.clock.time());
        let open_wait = Duration::from_secs(u64::from(OPEN_TIMEOUT_SECS));
        let expired: Vec<(RouteId, bool)> = self
            .routes
            .iter()
            .filter_map(|(id, route)| match route.state {
                State::Suspended(since) if now.duration_since(since) >= self.grace => {
                    Some((*id, false))
                }
                State::Opening(since) if now.duration_since(since) >= open_wait => {
                    Some((*id, true))
                }
                _ => None,
            })
            .collect();
        for (id, opening) in expired {
            let Some(route) = self.routes.get(&id) else {
                continue;
            };
            let (opener, destination) = (route.opener, route.destination);
            if opening && let Some(channel) = Self::channel_of(opener) {
                self.end_text(
                    opener,
                    Frame::OpenErr(OpenErr {
                        channel,
                        code: OpenErrorCode::PeerOffline,
                        detail: "nobody answered the opening".to_owned(),
                    }),
                );
            }
            for end in [opener, destination] {
                if let Some(channel) = Self::channel_of(end)
                    && (!opening || end == destination)
                {
                    self.end_text(
                        end,
                        Frame::Close(Close {
                            channel,
                            code: CloseCode::Reset,
                            detail: if opening {
                                "the opening was not answered".to_owned()
                            } else {
                                "the tunnel was not resumed in time".to_owned()
                            },
                        }),
                    );
                }
            }
            self.remove(id);
        }
    }
}
