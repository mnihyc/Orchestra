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

#[cfg(feature = "test-observers")]
impl Notice {
    /// Logical payload bytes only; no payload is retained by the observer.
    pub fn observed_bytes(&self) -> Option<usize> {
        let bytes = match self {
            Self::Deliver(delivery) => {
                let events: Vec<_> = delivery
                    .events
                    .iter()
                    .map(|event| (&event.seq, &event.at, &event.kind, &event.body))
                    .collect();
                serde_json::to_vec(&(delivery.stream.to_string(), &events, delivery.last_seq))
            }
            Self::Reset(notice) => serde_json::to_vec(&(
                notice.stream.to_string(),
                notice.from,
                &notice.reason,
                notice.resync,
            )),
            Self::Report(eph) => serde_json::to_vec(eph),
            Self::Online { session_id } | Self::Offline { session_id } => {
                serde_json::to_vec(session_id)
            }
        };
        bytes.ok().map(|bytes| bytes.len())
    }
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
    #[cfg(feature = "test-observers")]
    observed_down: Arc<crate::test_observers::QueueGauge>,
    #[cfg(feature = "test-observers")]
    observed_notices: Arc<crate::test_observers::QueueGauge>,
}

impl Shared {
    fn send_down(&self, frame: DownFrame) {
        #[cfg(feature = "test-observers")]
        let bytes = orch_link_contract::frames::encode(&frame, usize::MAX)
            .ok()
            .map(|text| text.len());
        #[cfg(feature = "test-observers")]
        let _ = self.observed_down.send(&self.down, frame, bytes);
        #[cfg(not(feature = "test-observers"))]
        let _ = self.down.send(frame);
    }

    fn send_notice(&self, notice: Notice) {
        #[cfg(feature = "test-observers")]
        let bytes = notice.observed_bytes();
        #[cfg(feature = "test-observers")]
        let _ = self.observed_notices.send(&self.notices, notice, bytes);
        #[cfg(not(feature = "test-observers"))]
        let _ = self.notices.send(notice);
    }
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
    /// QA lifetime follows this exact receiver into its actual consumer.
    #[cfg(feature = "test-observers")]
    pub observed_down: crate::test_observers::ReceiverLifetime,
    /// QA lifetime follows this exact receiver into its actual consumer.
    #[cfg(feature = "test-observers")]
    pub observed_notices: crate::test_observers::ReceiverLifetime,
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
        #[cfg(feature = "test-observers")]
        let observed_down = crate::test_observers::QueueGauge::new("controller.down");
        #[cfg(feature = "test-observers")]
        let observed_notices = crate::test_observers::QueueGauge::new("controller.notices");
        #[cfg(feature = "test-observers")]
        let issuer = issuer.with_observer(Arc::clone(&observed_down));
        let shared = Arc::new(Shared {
            machine_id,
            state: Mutex::new(State {
                receiver: StreamReceiver::new(cursors, roles),
                session: None,
            }),
            down: down_tx,
            notices: notices_tx,
            issuer,
            #[cfg(feature = "test-observers")]
            observed_down: Arc::clone(&observed_down),
            #[cfg(feature = "test-observers")]
            observed_notices: Arc::clone(&observed_notices),
        });
        (
            Self { shared },
            ControllerOutputs {
                down: down_rx,
                notices: notices_rx,
                #[cfg(feature = "test-observers")]
                observed_down: observed_down.receiver(),
                #[cfg(feature = "test-observers")]
                observed_notices: observed_notices.receiver(),
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
        self.shared.send_notice(Notice::Online { session_id });
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
        self.shared.send_notice(Notice::Offline { session_id });
    }

    /// The gateway finished writing a command on the session it still holds.
    pub fn command_sent(&self, session_id: Id, id: Id) {
        let state = self.state();
        if state.session == Some(session_id) {
            self.shared.issuer.on_sent(id);
        }
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
                UpFrame::TunnelFailed(_) => None,
                UpFrame::TunnelRouteAsk(_)
                | UpFrame::TunnelPrepareAsk(_)
                | UpFrame::TunnelAttachResult(_)
                | UpFrame::TunnelRelease(_)
                | UpFrame::TunnelSelected(_) => None,
                UpFrame::Ev(ev) => Some(state.receiver.on_ev(ev)),
                UpFrame::Evb(evb) => Some(state.receiver.on_evb(evb)),
                UpFrame::Reset(reset) => {
                    match state.receiver.on_reset(&reset) {
                        Ok(notice) => {
                            self.shared.send_notice(Notice::Reset(notice));
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
                            self.shared.send_notice(Notice::Report(eph));
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
                self.shared.send_notice(Notice::Deliver(delivery));
            }
            Received::Answer(frame) => {
                if let Ok(down) = into_down(*frame) {
                    self.shared.send_down(down);
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
            self.shared.send_down(down);
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

#[cfg(all(test, feature = "test-observers"))]
#[allow(clippy::unwrap_used, reason = "observer controls fail by panicking")]
mod observer_tests {
    use super::*;
    use orch_link_contract::commands::{Cmd, CmdCancel};
    use orch_types::{Clock, ManualClock, Time, new_id};

    #[tokio::test]
    async fn actual_controller_and_command_issuer_share_their_exact_queue_observations() {
        let clock = ManualClock::new(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap());
        let (controller, mut out) =
            ControllerLink::new(new_id(), vec![], BTreeMap::new(), Arc::new(clock.clone()));
        let session = new_id();
        controller.session_up(session);
        let notice = out.notices.recv().await.unwrap();
        assert!(matches!(&notice, Notice::Online { session_id } if *session_id == session));
        out.observed_notices
            .selected(notice.observed_bytes())
            .processed();
        let id = new_id();
        let _ = controller.commands().issue(Cmd {
            id,
            kind: "machine.probe".into(),
            key: None,
            deadline: Time::new(clock.now() + chrono::Duration::seconds(30)),
            body: serde_json::json!({}),
        });
        let first = out.down.recv().await.unwrap();
        assert!(matches!(&first, DownFrame::Cmd(command) if command.id == id));
        out.observed_down
            .selected(
                orch_link_contract::frames::encode(&first, usize::MAX)
                    .ok()
                    .map(|bytes| bytes.len()),
            )
            .complete();
        controller.commands().cancel(id);
        let cancelled = out.down.recv().await.unwrap();
        assert_eq!(cancelled, DownFrame::CmdCancel(CmdCancel { id }));
        out.observed_down
            .selected(
                orch_link_contract::frames::encode(&cancelled, usize::MAX)
                    .ok()
                    .map(|bytes| bytes.len()),
            )
            .complete();
        let down = controller.shared.observed_down.snapshot();
        let notices = controller.shared.observed_notices.snapshot();
        assert_eq!((down.enqueue_attempts, down.selected, down.sent), (2, 2, 2));
        assert_eq!(
            (notices.enqueue_attempts, notices.processed, notices.sent),
            (1, 1, 0)
        );
        assert_eq!((down.queued_messages, down.in_flight_messages), (0, 0));
        assert!(!down.receiver_closed && !down.inconsistent && down.peak_payload_bytes > 0);
        drop(out.down);
        controller.commands().cancel(new_id());
        assert_eq!(
            controller.shared.observed_down.snapshot().failed_enqueues,
            1
        );
        drop(out.observed_down);
        assert!(controller.shared.observed_down.snapshot().receiver_closed);
    }
}
