//! Forward events, commands and ephemeral reports.
//!
//! What a machine says for the system goes up the backplane wrapped in the
//! machine and the session it came on; what the system says for a machine
//! comes down it, and goes into the machine's session. Tunnel frames are the
//! relay's and never leave the gateway.
//!
//! While the backplane is down nothing that is meant for the system is kept
//! or acknowledged: the events a machine sends then are simply not taken in,
//! so the machine keeps them in its spool and sends them again; a result
//! that is lost is asked for again when the system sends the command anew.
//! Tunnels go on being carried, because they never needed the system.

use crate::Gateway;
use orch_link_contract::backplane::{BackplaneFrame, Down, Up, UpFrame};
use orch_link_contract::frames::{DataFrame, Frame};
use orch_types::Id;
use std::sync::atomic::Ordering;

/// A frame a machine sent on its session.
pub(crate) fn from_machine(gateway: &Gateway, machine: Id, session: Id, frame: Frame) {
    match frame {
        frame @ (Frame::Ev(_)
        | Frame::Evb(_)
        | Frame::Eph(_)
        | Frame::Reset(_)
        | Frame::CmdAck(_)
        | Frame::CmdRes(_)
        | Frame::McpImportAsk(_)
        | Frame::PortLocalAsk(_)
        | Frame::PortConnectionsReply(_)) => {
            if let Ok(frame) = UpFrame::try_from(frame) {
                gateway.inner.backplane.send(BackplaneFrame::Up(Up {
                    machine_id: machine,
                    session_id: session,
                    frame,
                }));
            }
        }
        frame @ (Frame::Open(_)
        | Frame::OpenOk(_)
        | Frame::OpenErr(_)
        | Frame::Resume(_)
        | Frame::ResumeOk(_)
        | Frame::Credit(_)
        | Frame::Eof(_)
        | Frame::Close(_)
        | Frame::ChAck(_)
        | Frame::PtyResize(_)
        | Frame::PtyExit(_)) => {
            gateway
                .inner
                .relay
                .with(|relay| relay.on_frame(machine, session, frame));
        }
        // Whatever else a machine sends is no business of a gateway.
        _ => {}
    }
}

/// A frame of tunnel data a machine sent on its session.
pub(crate) fn data_from_machine(gateway: &Gateway, machine: Id, session: Id, data: DataFrame) {
    gateway
        .inner
        .bytes_relayed
        .fetch_add(data.payload.len() as u64, Ordering::Relaxed);
    gateway
        .inner
        .relay
        .with(|relay| relay.on_data(machine, session, data));
}

/// A frame the system sent for a machine. A machine that has no session here
/// is not told: the system sends again when it has.
pub(crate) fn to_machine(gateway: &Gateway, down: Down) {
    let Some(sender) = gateway.inner.sessions.sender_of(down.machine_id) else {
        return;
    };
    let _ = sender.send(Frame::from(down.frame));
}
