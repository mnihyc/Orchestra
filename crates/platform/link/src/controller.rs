//! The system's end of one machine's link.
//!
//! Between the system and a machine stands a gateway, which relays text
//! frames and keeps no state that has to last. What the system does with the
//! frames is here, for one machine: it takes the machine's events in, once
//! each and in order, and answers with how far it has come only when its
//! caller says that the events were committed; it issues the machine's
//! commands and lets its caller wait for their results; it tells its caller
//! when the machine has a session and when it has not.
//!
//! The link speaks in the forms of the backplane: [`UpFrame`] in from the
//! gateway, [`DownFrame`] out to it. A caller wires the two to wherever the
//! gateway is, a WebSocket in a deployment, a channel in a test.

use crate::commands::CommandIssuer;
use crate::streams::{Delivery, Received, ResetNotice, StreamReceiver};
use orch_link_contract::backplane::{DownFrame, UpFrame};
use orch_link_contract::session::Role;
use orch_link_contract::streams::Eph;
use orch_types::{Clock, Id, Seq};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc;

/// What the system's caller is told by a machine's link.
#[derive(Debug)]
pub enum Notice {
    /// New events to apply in one transaction. When it commits, the caller
    /// calls [`ControllerLink::committed`]; when it cannot,
    /// [`ControllerLink::abandoned`].
    Deliver(Delivery),
    /// The machine says that events of a stream are gone. The caller
    /// records it, stores the new cursor, and, if [`ResetNotice::resync`], has
    /// the machine's adapter report the session again.
    Reset(ResetNotice),
    /// A measurement that is not kept, for whoever is watching.
    Report(Eph),
    /// The machine has a session.
    Online {
        /// The session.
        session_id: Id,
    },
    /// The session ended.
    Offline {
        /// The session.
        session_id: Id,
    },
}

struct State {
    receiver: StreamReceiver,
    session: Option<Id>,
}

struct Shared {
    machine_id: Id,
    state: Mutex<State>,
    down: mpsc::UnboundedSender<DownFrame>,
    notices: mpsc::UnboundedSender<Notice>,
    issuer: CommandIssuer,
}

/// The system's end of one machine's link.
#[derive(Clone)]
pub struct ControllerLink {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for ControllerLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControllerLink")
            .field("machine", &self.shared.machine_id)
            .finish_non_exhaustive()
    }
}

/// The ends of a controller link that its caller reads from.
#[derive(Debug)]
pub struct ControllerOutputs {
    /// Frames for the machine, which the gateway carries.
    pub down: mpsc::UnboundedReceiver<DownFrame>,
    /// What the caller is told.
    pub notices: mpsc::UnboundedReceiver<Notice>,
}

impl ControllerLink {
    /// The link of machine `machine_id`, which has `roles`, and whose streams
    /// the system has taken in as far as `cursors` say.
    #[must_use]
    pub fn new(
        machine_id: Id,
        roles: Vec<Role>,
        cursors: BTreeMap<String, Seq>,
        clock: Arc<dyn Clock>,
    ) -> (Self, ControllerOutputs) {
        let (down_tx, down_rx) = mpsc::unbounded_channel();
        let (notices_tx, notices_rx) = mpsc::unbounded_channel();
        let issuer = CommandIssuer::new(down_tx.clone(), clock);
        let shared = Arc::new(Shared {
            machine_id,
            state: Mutex::new(State {
                receiver: StreamReceiver::new(cursors, roles),
                session: None,
            }),
            down: down_tx,
            notices: notices_tx,
            issuer,
        });
        (
            Self { shared },
            ControllerOutputs {
                down: down_rx,
                notices: notices_rx,
            },
        )
    }

    /// The machine.
    #[must_use]
    pub fn machine_id(&self) -> Id {
        self.shared.machine_id
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The commands of this machine.
    #[must_use]
    pub fn commands(&self) -> &CommandIssuer {
        &self.shared.issuer
    }

    /// How far each stream was taken in: what a gateway tells the machine
    /// when its session becomes ready.
    #[must_use]
    pub fn cursors(&self) -> BTreeMap<String, Seq> {
        self.state().receiver.cursors()
    }

    /// The machine has a session. Commands that have not ended are sent.
    pub fn session_up(&self, session_id: Id) {
        self.state().session = Some(session_id);
        let _ = self.shared.notices.send(Notice::Online { session_id });
        self.shared.issuer.session_ready();
    }

    /// A session ended. One that is not the machine's current is ignored: a
    /// new session takes the old one's place before the old one is
    /// reported gone.
    pub fn session_down(&self, session_id: Id) {
        {
            let mut state = self.state();
            if state.session != Some(session_id) {
                return;
            }
            state.session = None;
        }
        self.shared.issuer.session_down();
        let _ = self.shared.notices.send(Notice::Offline { session_id });
    }

    /// A text frame that the machine sent on `session_id`.
    pub fn up(&self, session_id: Id, frame: UpFrame) {
        let received = {
            let mut state = self.state();
            if state.session != Some(session_id) {
                return;
            }
            match frame {
                // The API's machine-request adapter handles this before stream/command dispatch.
                UpFrame::McpImportAsk(_) | UpFrame::PortLocalAsk(_) => None,
                UpFrame::PortConnectionsReply(_) => None,
                UpFrame::Ev(ev) => Some(state.receiver.on_ev(ev)),
                UpFrame::Evb(evb) => Some(state.receiver.on_evb(evb)),
                UpFrame::Reset(reset) => {
                    match state.receiver.on_reset(&reset) {
                        Ok(notice) => {
                            let _ = self.shared.notices.send(Notice::Reset(notice));
                        }
                        Err(why) => {
                            tracing::warn!(machine = %self.shared.machine_id, why, "a reset was not taken")
                        }
                    }
                    None
                }
                UpFrame::Eph(eph) => {
                    match state.receiver.on_eph(eph) {
                        Ok(eph) => {
                            let _ = self.shared.notices.send(Notice::Report(eph));
                        }
                        Err(why) => {
                            tracing::warn!(machine = %self.shared.machine_id, why, "a report was not taken")
                        }
                    }
                    None
                }
                UpFrame::CmdAck(ack) => {
                    self.shared.issuer.on_cmd_ack(&ack);
                    None
                }
                UpFrame::CmdRes(result) => {
                    self.shared.issuer.on_cmd_res(&result);
                    None
                }
            }
        };
        if let Some(received) = received {
            self.handle(received);
        }
    }

    fn handle(&self, received: Received) {
        match received {
            Received::Apply(delivery) => {
                let _ = self.shared.notices.send(Notice::Deliver(delivery));
            }
            Received::Answer(frame) => {
                if let Ok(down) = into_down(*frame) {
                    let _ = self.shared.down.send(down);
                }
            }
            Received::Queued => {}
            Received::Refused(why) => {
                tracing::warn!(machine = %self.shared.machine_id, why, "events were not taken");
            }
        }
    }

    /// The events of a delivery were applied and committed. The machine is
    /// told how far the stream has come, and the next frame of that stream,
    /// if one was waiting, is dealt with.
    pub fn committed(&self, delivery: &Delivery) {
        let (answer, next) = self.state().receiver.committed(delivery);
        if let Ok(down) = into_down(answer) {
            let _ = self.shared.down.send(down);
        }
        if let Some(next) = next {
            self.handle(next);
        }
    }

    /// The events of a delivery could not be applied. Nothing is answered,
    /// and the machine sends them again.
    pub fn abandoned(&self, delivery: &Delivery) {
        let next = self.state().receiver.abandoned(delivery);
        if let Some(next) = next {
            self.handle(next);
        }
    }
}

fn into_down(frame: orch_link_contract::frames::Frame) -> Result<DownFrame, ()> {
    use orch_link_contract::frames::Frame;
    match frame {
        Frame::Ack(ack) => Ok(DownFrame::Ack(ack)),
        Frame::Nak(nak) => Ok(DownFrame::Nak(nak)),
        _ => Err(()),
    }
}
