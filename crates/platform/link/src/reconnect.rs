//! Endpoint selection, retry and backoff.
//!
//! A machine holds its session for as long as it lives, and every session
//! ends: a gateway is restarted, a line is cut, a laptop sleeps. What a
//! machine does about it is the same each time. It tries its endpoints, the
//! one that worked last first and the others in the order enrollment gave;
//! if none answers it waits, a second at first and twice as long after each
//! further failure up to half a minute, moved a little at random so that
//! machines that lost a gateway together do not come back together; and it
//! tries again, for ever.
//!
//! A refusal from the gateway changes the wait only when the refusal says
//! so: `busy` names a delay of its own. The rest of what a refusal means
//! (renew the certificate, update the connector) is for the machine to act
//! on, and is reported to it by the session; this module only keeps time.

use crate::framing::Connection;
use async_trait::async_trait;
use orch_link_contract::enrollment::{Endpoint, Trust, endpoint_order};
use orch_link_contract::session::reconnect_delay;
use std::time::Duration;

/// Why an endpoint could not be dialled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DialError {
    /// The address is not one that can be dialled.
    #[error("not an address that can be dialled: {0}")]
    Address(String),
    /// There is no way to trust an endpoint of that kind.
    #[error("no way to verify an endpoint of trust {0:?}")]
    NoTrust(Trust),
    /// The socket could not be connected.
    #[error("could not connect: {0}")]
    Connect(String),
    /// The TLS handshake failed: the server is not the system's, or the
    /// system does not know this machine.
    #[error("TLS: {0}")]
    Tls(String),
    /// The upgrade to a WebSocket failed.
    #[error("WebSocket: {0}")]
    Upgrade(String),
    /// Connecting took longer than it may.
    #[error("connecting took too long")]
    TimedOut,
}

/// Something that makes connections to endpoints.
#[async_trait]
pub trait Dialer: Send + Sync {
    /// Connects to an endpoint, under the trust path it names.
    async fn dial(&self, endpoint: &Endpoint) -> Result<Connection, DialError>;
}

/// The endpoints of a machine and which of them worked last.
#[derive(Debug, Clone, Default)]
pub struct Endpoints {
    list: Vec<Endpoint>,
    last_worked: Option<String>,
}

impl Endpoints {
    /// The endpoints enrollment gave, in order.
    #[must_use]
    pub fn new(list: Vec<Endpoint>) -> Self {
        Self {
            list,
            last_worked: None,
        }
    }

    /// The order to try them in: the one that worked last first.
    #[must_use]
    pub fn order(&self) -> Vec<Endpoint> {
        endpoint_order(&self.list, self.last_worked.as_deref())
            .into_iter()
            .cloned()
            .collect()
    }

    /// Takes note that a session was held through `endpoint`.
    pub fn worked(&mut self, endpoint: &Endpoint) {
        self.last_worked = Some(endpoint.url.clone());
    }

    /// Replaces the list, keeping the memory of what worked if it is still
    /// there.
    pub fn replace(&mut self, list: Vec<Endpoint>) {
        self.list = list;
    }

    /// Whether there is anything to try.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// Dials the endpoints in order and returns the first connection that
/// opens, with the endpoint it opened on; or every failure, in order.
pub async fn dial_in_order(
    dialer: &dyn Dialer,
    endpoints: &Endpoints,
) -> Result<(Connection, Endpoint), Vec<(Endpoint, DialError)>> {
    let mut failures = Vec::new();
    for endpoint in endpoints.order() {
        match dialer.dial(&endpoint).await {
            Ok(connection) => return Ok((connection, endpoint)),
            Err(error) => failures.push((endpoint, error)),
        }
    }
    Err(failures)
}

/// Where the random movement of a pause comes from: a number from -1 to 1.
pub trait Jitter: Send {
    /// The next number.
    fn next(&mut self) -> f64;
}

/// Jitter drawn at random.
#[derive(Debug, Default, Clone, Copy)]
pub struct RandomJitter;

impl Jitter for RandomJitter {
    fn next(&mut self) -> f64 {
        rand::random_range(-1.0..=1.0)
    }
}

/// No jitter at all, for tests that count seconds.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoJitter;

impl Jitter for NoJitter {
    fn next(&mut self) -> f64 {
        0.0
    }
}

/// The pauses between attempts to connect.
pub struct Backoff {
    failures: u32,
    jitter: Box<dyn Jitter>,
}

impl std::fmt::Debug for Backoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backoff")
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

impl Backoff {
    /// A backoff that has not failed yet.
    #[must_use]
    pub fn new(jitter: Box<dyn Jitter>) -> Self {
        Self {
            failures: 0,
            jitter,
        }
    }

    /// Takes note of a failure and returns how long to wait before trying
    /// again. A gateway that names a delay of its own is waited for at
    /// least that long.
    pub fn failed(&mut self, named_by_gateway: Option<Duration>) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let pause = reconnect_delay(self.failures, self.jitter.next());
        named_by_gateway.map_or(pause, |named| pause.max(named))
    }

    /// Takes note that a session was held: the next failure begins again at
    /// the first pause.
    pub fn succeeded(&mut self) {
        self.failures = 0;
    }

    /// How many attempts in a row failed.
    #[must_use]
    pub const fn failures(&self) -> u32 {
        self.failures
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Box::new(RandomJitter))
    }
}
