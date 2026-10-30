//! Session heartbeat and connection shutdown.
//!
//! A session keeps itself alive: each side pings after a heartbeat of
//! sending nothing, and gives the other up after three of hearing nothing,
//! which is the link's own business and runs inside each session. What the
//! gateway adds is the clock of the tunnels it relays, which decides when a
//! tunnel whose session dropped has waited long enough and when an opening
//! nobody answered is refused, and the end of everything: when the gateway is
//! told to stop, every session is ended, so that the machines see their
//! connection close and connect again, here when the gateway is back or to
//! another.

use crate::Gateway;
use std::time::Duration;

/// How often the tunnels' clock is looked at. A second: the shortest wait a
/// tunnel has is the time an opening is waited for, ten seconds, and a
/// tunnel's grace is a minute.
const TICK_EVERY: Duration = Duration::from_secs(1);

/// Runs the clock of the tunnels until the gateway is told to stop, then ends
/// the sessions.
pub(crate) async fn run(gateway: Gateway) {
    let mut interval = tokio::time::interval(TICK_EVERY);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = gateway.inner.shutdown.cancelled() => break,
            _ = interval.tick() => {
                gateway
                    .inner
                    .relay
                    .with(|relay| relay.tick(tokio::time::Instant::now()));
            }
        }
    }
    gateway.inner.sessions.end_all();
}
