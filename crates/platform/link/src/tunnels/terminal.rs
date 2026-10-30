//! API-terminated terminal tunnels.
//!
//! A shell is a tunnel whose other end is a person's browser, so the system
//! opens it and the gateway relays only the machine's end. The frames of such
//! a tunnel reach the system over the backplane: `tun_open`, `tun_credit`,
//! `tun_eof`, `tun_close` and the text frames for a terminal's size and its
//! exit, and the data as binary frames that name the machine and the channel.
//!
//! A terminal tunnel is one attachment, not the shell: the shell belongs to
//! the connector and lives until its process ends. So the tunnel is never
//! resumed. If the session at either end drops, it ends, and the browser
//! attaches again, to be sent the last stretch of output the connector kept.
//!
//! [`TerminalHub`] is the system's side. It opens terminal tunnels with a
//! ticket whose source is the system, and gives each as a [`Tunnel`], the same
//! socket a machine has; flow control is the same as on any tunnel.

use crate::ports::{Datagram, UdpBudget};
use crate::tunnels::channel::{
    Channel, ChannelFrame, ChannelOptions, Ended, OpenError, Tunnel, TunnelEvent, Wire,
};
use crate::tunnels::flow::Gauge;
use bytes::Bytes;
use orch_link_contract::backplane::{
    BackplaneData, BackplaneFrame, ChannelText, TunClose, TunCredit, TunEof, TunOpen, TunText,
};
use orch_link_contract::ports::{BackplanePortDatagram, PortDatagram};
use orch_link_contract::tunnels::{
    OPEN_TIMEOUT_SECS, PtyExit, PtyResize, REPLAY_BUFFER_BYTES, Ticket,
};
use orch_link_contract::version::TUNNEL_WINDOW_BYTES;
use orch_types::Id;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the system's end sends the gateway.
#[derive(Debug)]
pub enum ApiOut {
    /// A text frame.
    Text(Box<BackplaneFrame>),
    /// A frame of data.
    Data(BackplaneData),
    /// One leased UDP packet on the same admitted backplane.
    Datagram {
        /// The enrolled environment machine.
        machine: Id,
        /// The bounded whole packet.
        packet: Datagram,
    },
}

struct BackplaneWire {
    machine: Id,
    out: mpsc::UnboundedSender<ApiOut>,
    frame_bytes: usize,
}

impl Wire for BackplaneWire {
    fn datagram(&self, packet: Datagram) -> bool {
        self.out
            .send(ApiOut::Datagram {
                machine: self.machine,
                packet,
            })
            .is_ok()
    }
    fn control(&self, channel: u32, frame: ChannelFrame) -> bool {
        let machine_id = self.machine;
        let text = match frame {
            ChannelFrame::Credit(n) => BackplaneFrame::TunCredit(TunCredit {
                machine_id,
                channel,
                n,
            }),
            ChannelFrame::Eof => BackplaneFrame::TunEof(TunEof {
                machine_id,
                channel,
            }),
            ChannelFrame::Close { code, detail } => BackplaneFrame::TunClose(TunClose {
                machine_id,
                channel,
                code,
                detail,
            }),
            ChannelFrame::PtyResize { cols, rows } => BackplaneFrame::TunText(TunText {
                machine_id,
                channel,
                frame: ChannelText::PtyResize(PtyResize {
                    channel,
                    cols,
                    rows,
                }),
            }),
            ChannelFrame::PtyExit { code } => BackplaneFrame::TunText(TunText {
                machine_id,
                channel,
                frame: ChannelText::PtyExit(PtyExit { channel, code }),
            }),
            // A terminal is never resumed, so nothing is acknowledged.
            ChannelFrame::Ack(_) => return true,
        };
        self.out.send(ApiOut::Text(Box::new(text))).is_ok()
    }

    fn data(&self, channel: u32, offset: u64, payload: Bytes) -> bool {
        self.out
            .send(ApiOut::Data(BackplaneData {
                machine_id: self.machine,
                channel,
                offset,
                payload,
            }))
            .is_ok()
    }

    fn data_frame_bytes(&self) -> usize {
        self.frame_bytes
    }
}

/// A terminal that was asked for, and the channel through which its answer
/// is waited for.
type Waiting = (Arc<Mutex<Channel>>, oneshot::Sender<Result<Id, OpenError>>);

struct State {
    udp: HashMap<Id, Arc<UdpBudget>>,
    channels: HashMap<(Id, u32), Arc<Mutex<Channel>>>,
    pending: HashMap<(Id, u32), Waiting>,
    next: u32,
}

/// The system's end of the terminals of all machines.
#[derive(Clone)]
pub struct TerminalHub {
    out: mpsc::UnboundedSender<ApiOut>,
    state: Arc<Mutex<State>>,
    window: u64,
    frame_bytes: usize,
    ports: orch_link_contract::version::PortLimits,
    gauge: Option<Arc<Gauge>>,
}

impl std::fmt::Debug for TerminalHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalHub").finish_non_exhaustive()
    }
}

impl TerminalHub {
    /// Releases locally closed port handles independently of a peer's final frame.
    pub fn prune_ports(&self) {
        lock(&self.state).channels.retain(|_, channel| {
            let channel = lock(channel);
            !channel.is_port() || !channel.is_closed()
        });
    }
    /// Ends the channels of an opening, including pending connects, without ending another door.
    pub fn close_port_opening(&self, opening: Id) {
        let channels = lock(&self.state)
            .channels
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for channel in channels {
            let mut channel = lock(&channel);
            if channel.belongs_to_opening(opening) {
                channel.close_with(Ended::Reset, true);
            }
        }
    }
    /// Ends current port connections for a runtime transition, retaining durable openings.
    pub fn close_environment_ports(&self, environment: Id) {
        let channels = lock(&self.state)
            .channels
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for channel in channels {
            let mut channel = lock(&channel);
            if channel.belongs_to_environment(environment) {
                channel.close_with(Ended::Reset, true);
            }
        }
    }
    /// A hub whose frames go to `out`, which the backplane carries.
    #[must_use]
    pub fn new(out: mpsc::UnboundedSender<ApiOut>) -> Self {
        Self {
            out,
            state: Arc::new(Mutex::new(State {
                udp: HashMap::new(),
                channels: HashMap::new(),
                pending: HashMap::new(),
                next: 1,
            })),
            window: u64::from(TUNNEL_WINDOW_BYTES),
            frame_bytes: orch_link_contract::version::MAX_FRAME_BYTES as usize,
            ports: orch_link_contract::version::PortLimits::SHIPPED,
            gauge: None,
        }
    }

    /// The same hub, carrying ports within the deployment's limits.
    #[must_use]
    pub fn with_port_limits(mut self, ports: orch_link_contract::version::PortLimits) -> Self {
        self.ports = ports;
        self
    }

    /// The same hub, counting the bytes its tunnels hold.
    #[must_use]
    pub fn with_gauge(mut self, gauge: Arc<Gauge>) -> Self {
        self.gauge = Some(gauge);
        self
    }

    /// Opens a terminal tunnel to `machine` with a ticket the system signed
    /// for itself.
    pub async fn open(&self, machine: Id, ticket: &str) -> Result<Tunnel, OpenError> {
        let claims = Ticket::parse(ticket)
            .map_err(|error| OpenError::Refused {
                code: orch_link_contract::tunnels::OpenErrorCode::BadTicket,
                detail: error.to_string(),
            })?
            .claims()
            .clone();
        let (channel, events) = Channel::make(
            claims.kind,
            claims.service.clone(),
            true,
            ChannelOptions {
                window_bytes: self.window,
                replay_bytes: REPLAY_BUFFER_BYTES,
                resumable: false,
            }
            .for_port(claims.port.as_ref(), &self.ports),
            self.gauge.clone(),
        );
        let (answer_tx, answer_rx) = oneshot::channel();
        let budget = {
            let mut state = lock(&self.state);
            claims
                .port
                .as_ref()
                .map(|port| {
                    state
                        .udp
                        .entry(port.environment_id)
                        .or_insert_with(|| Arc::new(UdpBudget::for_limits(&self.ports)))
                        .clone()
                })
                .unwrap_or_else(|| Arc::new(UdpBudget::for_limits(&self.ports)))
        };
        lock(&channel).configure_port(claims.port.clone(), None, budget);
        let number = {
            let mut state = lock(&self.state);
            let mut number = state.next;
            while state.channels.contains_key(&(machine, number)) {
                number = number.wrapping_add(1).max(1);
            }
            state.next = number.wrapping_add(1).max(1);
            state
                .channels
                .insert((machine, number), Arc::clone(&channel));
            state
                .pending
                .insert((machine, number), (Arc::clone(&channel), answer_tx));
            number
        };
        let sent = self
            .out
            .send(ApiOut::Text(Box::new(BackplaneFrame::TunOpen(TunOpen {
                machine_id: machine,
                channel: number,
                ticket: ticket.to_owned(),
            }))));
        if sent.is_err() {
            self.forget(machine, number);
            return Err(OpenError::NoSession);
        }
        let waited = Duration::from_secs(u64::from(OPEN_TIMEOUT_SECS) + 2);
        match tokio::time::timeout(waited, answer_rx).await {
            Ok(Ok(Ok(cid))) => {
                let wire: Arc<dyn Wire> = Arc::new(BackplaneWire {
                    machine,
                    out: self.out.clone(),
                    frame_bytes: self.frame_bytes,
                });
                lock(&channel).attach(wire, number, cid);
                Ok(Tunnel::new(channel, events))
            }
            Ok(Ok(Err(error))) => {
                self.forget(machine, number);
                Err(error)
            }
            Ok(Err(_)) => {
                self.forget(machine, number);
                Err(OpenError::Interrupted)
            }
            Err(_) => {
                self.forget(machine, number);
                Err(OpenError::TimedOut)
            }
        }
    }

    fn forget(&self, machine: Id, number: u32) {
        let mut state = lock(&self.state);
        state.channels.remove(&(machine, number));
        state.pending.remove(&(machine, number));
    }

    fn channel(&self, machine: Id, number: u32) -> Option<Arc<Mutex<Channel>>> {
        lock(&self.state).channels.get(&(machine, number)).cloned()
    }

    /// A text frame from the gateway.
    pub fn on_frame(&self, frame: BackplaneFrame) {
        match frame {
            BackplaneFrame::TunOpenOk(ok) => {
                let pending = lock(&self.state)
                    .pending
                    .remove(&(ok.machine_id, ok.channel));
                if let Some((channel, answer)) = pending {
                    lock(&channel).set_inside(ok.inside);
                    let _ = answer.send(Ok(ok.cid));
                }
            }
            BackplaneFrame::TunOpenErr(err) => {
                let pending = lock(&self.state)
                    .pending
                    .remove(&(err.machine_id, err.channel));
                if let Some((channel, answer)) = pending {
                    lock(&self.state)
                        .channels
                        .remove(&(err.machine_id, err.channel));
                    lock(&channel).close_with(Ended::Reset, false);
                    let _ = answer.send(Err(OpenError::Refused {
                        code: err.code,
                        detail: err.detail,
                    }));
                }
            }
            BackplaneFrame::TunCredit(c) => {
                if let Some(channel) = self.channel(c.machine_id, c.channel) {
                    lock(&channel).on_credit(c.n);
                }
            }
            BackplaneFrame::TunEof(e) => {
                if let Some(channel) = self.channel(e.machine_id, e.channel) {
                    lock(&channel).on_eof();
                }
            }
            BackplaneFrame::TunClose(c) => {
                if let Some(channel) = self.channel(c.machine_id, c.channel) {
                    lock(&channel).on_peer_close(c.code, &c.detail);
                    lock(&self.state)
                        .channels
                        .remove(&(c.machine_id, c.channel));
                }
            }
            BackplaneFrame::TunText(t) => {
                if let Some(channel) = self.channel(t.machine_id, t.channel) {
                    let event = match t.frame {
                        ChannelText::PtyExit(e) => TunnelEvent::Exit { code: e.code },
                        ChannelText::PtyResize(r) => TunnelEvent::Resize {
                            cols: r.cols,
                            rows: r.rows,
                        },
                    };
                    lock(&channel).tell(event);
                }
            }
            _ => {}
        }
    }

    /// A frame of data from the gateway.
    pub fn on_data(&self, data: BackplaneData) {
        if let Some(channel) = self.channel(data.machine_id, data.channel) {
            lock(&channel).on_data(data.offset, data.payload);
        }
    }

    /// Delivers one complete UDP packet to the already admitted API association.
    pub fn on_datagram(&self, data: BackplanePortDatagram) {
        if let Some(channel) = self.channel(data.machine_id, data.datagram.channel) {
            let packet = Arc::new(UdpBudget::for_limits(&self.ports)).hold(PortDatagram {
                channel: data.datagram.channel,
                payload: data.datagram.payload,
            });
            if let Some(packet) = packet {
                lock(&channel).on_datagram(packet);
            }
        }
    }
}
