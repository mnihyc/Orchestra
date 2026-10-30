//! Credit and bounded backpressure.
//!
//! A sender never has more bytes outstanding than its receiver gave it
//! credit for, and the receiver gives more only as its consumer takes the
//! bytes it has. So a consumer that stops reading stops the sender within one
//! window, and nobody's memory fills: the most a channel holds at an end is a
//! window of received bytes and, for resumption, a replay buffer of sent ones.
//!
//! The arithmetic of a window is the contract's ([`SendWindow`] and
//! [`RecvWindow`]); what is decided here is when credit is worth a frame.
//! Giving it back a byte at a time would double the frames of a tunnel, and
//! holding it back until the window is empty would stop the sender for a
//! round trip each time, so it goes back in steps, and at once when the
//! consumer has taken everything there was.
//!
//! [`Gauge`] counts the bytes a set of channels hold, so that a test, or an
//! operator, can see that the bound holds.
//!
//! [`SendWindow`]: orch_link_contract::tunnels::SendWindow
//! [`RecvWindow`]: orch_link_contract::tunnels::RecvWindow

use std::sync::atomic::{AtomicU64, Ordering};

/// Credit goes back in steps of this share of the window: a quarter keeps a
/// sender busy while a credit frame is on its way, and costs a handful of
/// frames for each window of data.
pub const CREDIT_STEP_DIVISOR: u64 = 4;

/// The least credit worth a frame, for a window of `window` bytes.
#[must_use]
pub fn credit_step(window: u64) -> u64 {
    (window / CREDIT_STEP_DIVISOR).max(1)
}

/// Whether `owed` bytes of credit are to be given back now: a step has been
/// earned, or the consumer has taken everything that arrived and anything is
/// owed.
#[must_use]
pub fn credit_due(owed: u64, window: u64, drained: bool) -> bool {
    owed >= credit_step(window) || (drained && owed > 0)
}

/// A count of bytes held, with the most it ever was.
#[derive(Debug, Default)]
pub struct Gauge {
    current: AtomicU64,
    peak: AtomicU64,
}

impl Gauge {
    /// A gauge at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// More bytes are held.
    pub fn add(&self, bytes: u64) {
        let now = self.current.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak.fetch_max(now, Ordering::Relaxed);
    }

    /// Bytes are let go.
    pub fn sub(&self, bytes: u64) {
        self.current.fetch_sub(bytes, Ordering::Relaxed);
    }

    /// How many bytes are held now.
    #[must_use]
    pub fn current(&self) -> u64 {
        self.current.load(Ordering::Relaxed)
    }

    /// The most that were ever held at once.
    #[must_use]
    pub fn peak(&self) -> u64 {
        self.peak.load(Ordering::Relaxed)
    }
}
