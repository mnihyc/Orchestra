//! Ticket-authorized machine tunnels and API terminal tunnels.
//!
//! The mechanism is the link's own [`Relay`]: it looks at the opening of a
//! tunnel (the ticket the system signed, who presents it, whether the
//! destination has a session) and then carries the tunnel's frames without
//! looking into them. It is a state machine that is told what happened, so
//! one holds it behind a lock and calls it, briefly, from wherever a frame
//! arrives; nothing in it waits.
//!
//! The one tunnel that does not end at a machine ends at the system, because
//! its other end is a person's browser: a terminal. Its frames cross the
//! backplane, and [`ApiBridge`] is where the relay hands them over.

use crate::backplane::BackplaneHandle;
use orch_link::tunnels::relay::{ApiPort, Relay};
use orch_link_contract::backplane::{BackplaneData, BackplaneFrame};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The relay of a gateway, behind the lock that serializes what happens to
/// it.
pub struct SharedRelay {
    inner: Mutex<Relay>,
}

impl std::fmt::Debug for SharedRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedRelay").finish_non_exhaustive()
    }
}

impl SharedRelay {
    /// Puts a relay behind its lock.
    #[must_use]
    pub fn new(relay: Relay) -> Self {
        Self {
            inner: Mutex::new(relay),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Relay> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `work` on the relay. The lock is held for as long as it takes, so
    /// `work` does nothing that waits.
    pub fn with<R>(&self, work: impl FnOnce(&mut Relay) -> R) -> R {
        work(&mut self.lock())
    }
}

/// Where the relay sends what is for the system's end of a terminal: up the
/// backplane. While the backplane is down there is no terminal, and what is
/// sent is lost with it; the browser attaches anew.
pub struct ApiBridge {
    backplane: BackplaneHandle,
}

impl ApiBridge {
    /// A bridge over `backplane`.
    #[must_use]
    pub fn new(backplane: BackplaneHandle) -> Self {
        Self { backplane }
    }
}

impl ApiPort for ApiBridge {
    fn datagram(&self, machine: orch_types::Id, packet: orch_link::ports::Datagram) {
        self.backplane.send_port_datagram(machine, packet);
    }
    fn text(&self, frame: BackplaneFrame) {
        self.backplane.send(frame);
    }

    fn data(&self, data: BackplaneData) {
        self.backplane.send_data(&data);
    }
}
