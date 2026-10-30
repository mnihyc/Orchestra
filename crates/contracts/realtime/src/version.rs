//! Protocol versions, limits and close codes.
//!
//! A browser tab holds one socket, and a deployed system serves one
//! version of its protocol: a page that was loaded before an update
//! offers the old one, is told so with a close code of its own, and
//! loads the application again. The limits here bound what one
//! connection may ask of a server and how long either side waits for
//! the other. They are counted in silence, never in slowness: a line
//! that is slow is still a line.

use crate::pacing::millis_between;
use orch_types::Time;
use std::time::Duration;

pub use orch_api_contract::terminals::TERMINAL_SUBPROTOCOL;

/// Milliseconds in a second, for the rules below that count in seconds.
const MILLIS_PER_SECOND: i64 = 1_000;

/// The WebSocket subprotocol of the realtime socket.
pub const SUBPROTOCOL: &str = "orch.v1";
/// The path of the realtime socket.
pub const SOCKET_PATH: &str = "/api/v1/ws";

/// How long a client may take to send its first frame, which says who
/// it is. Ten seconds are some tens of round trips of a slow link.
pub const AUTH_TIMEOUT_SECS: u32 = 10;
/// How often a client pings. Under half a minute, so that proxies that
/// close idle sockets after thirty seconds leave it alone.
pub const HEARTBEAT_SECS: u32 = 25;
/// How long either side hears nothing before it closes: two heartbeats
/// and some, which is hundreds of round trips even of a slow link.
pub const SILENCE_SECS: u32 = 60;
/// The most bytes of one frame from a client. No frame a client sends
/// carries more than a token or a feed's name.
pub const MAX_CLIENT_FRAME_BYTES: usize = 16 * 1024;
/// The most subscriptions one connection holds: more than any screen
/// shows at once, and a bound on what a tab can make a server watch.
pub const MAX_SUBSCRIPTIONS: u32 = 64;
/// The first pause before a client connects again.
pub const RECONNECT_FIRST: Duration = Duration::from_millis(500);
/// The longest pause before a client connects again.
pub const RECONNECT_LONGEST: Duration = Duration::from_secs(15);
/// How far a pause is moved at random, as a share of itself, so that
/// the tabs a restart dropped do not come back in one instant.
pub const RECONNECT_JITTER: f64 = 0.2;
/// How long a connection has to have lasted for the pauses to start
/// from the first again.
pub const RECONNECT_RESET_AFTER: Duration = Duration::from_secs(30);
/// How many bytes may wait for one connection before the server sheds
/// what it can, as a system ships.
pub const BUFFER_MAX_BYTES: i64 = 4 * 1024 * 1024;
/// For how long that much has to have been waiting before the
/// connection is closed as not keeping up, as a system ships.
pub const BUFFER_MAX_SECS: u32 = 30;
/// How often a server compares what it has sent on each feed with what
/// the feed now holds, to repair a wake-up that was lost.
pub const SWEEP_SECS: u32 = 5;
/// How long before its access token expires a client sends `reauth`. A new
/// token costs a few round trips, and a minute leaves room for dozens of
/// them on a slow line, and is a small part of the token's fifteen minutes.
pub const REAUTH_LEAD_SECS: u32 = 60;
/// A round trip above this many milliseconds is shown to the person, as
/// the status of the connection and beside a shell. Slowness is no event:
/// nothing else is said, and nothing closes for it.
pub const SLOW_ROUND_TRIP_MILLIS: i64 = 150;
/// How many of the latest round trips a client weighs: it uses the middle
/// one of the last three, so that one late packet changes nothing.
pub const ROUND_TRIPS_WEIGHED: usize = 3;

/// What a client does when its socket was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientAction {
    /// Connect again, after a pause.
    Reconnect,
    /// Report a defect, then connect again.
    ReportAndReconnect,
    /// Get a new access token, then connect again.
    RefreshThenReconnect,
    /// Sign out.
    SignOut,
    /// Connect again and load every feed anew.
    ReconnectAndReload,
    /// Load the application again: a new build is deployed.
    ReloadApplication,
}

/// Why a realtime socket was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseCode {
    /// The server is going away.
    GoingAway,
    /// The server is restarting.
    Restarting,
    /// The client sent a frame that is not one, or an unknown one.
    Malformed,
    /// The client said nothing about who it is, or its token is wrong or
    /// has run out.
    Unauthenticated,
    /// The person was suspended.
    Suspended,
    /// The client did not keep up: too much waited for it for too long.
    NotKeepingUp,
    /// The client offered a protocol the server does not speak.
    Unsupported,
}

impl CloseCode {
    /// Every code.
    pub const ALL: [CloseCode; 7] = [
        Self::GoingAway,
        Self::Restarting,
        Self::Malformed,
        Self::Unauthenticated,
        Self::Suspended,
        Self::NotKeepingUp,
        Self::Unsupported,
    ];

    /// The code as the socket carries it.
    #[must_use]
    pub const fn code(self) -> u16 {
        match self {
            Self::GoingAway => 1001,
            Self::Restarting => 1012,
            Self::Malformed => 4400,
            Self::Unauthenticated => 4401,
            Self::Suspended => 4403,
            Self::NotKeepingUp => 4408,
            Self::Unsupported => 4409,
        }
    }

    /// The reason a socket was closed with `code`, when it is one of
    /// the protocol's.
    #[must_use]
    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.code() == code)
    }

    /// What a client does about it.
    #[must_use]
    pub const fn action(self) -> ClientAction {
        match self {
            Self::GoingAway | Self::Restarting => ClientAction::Reconnect,
            Self::Malformed => ClientAction::ReportAndReconnect,
            Self::Unauthenticated => ClientAction::RefreshThenReconnect,
            Self::Suspended => ClientAction::SignOut,
            Self::NotKeepingUp => ClientAction::ReconnectAndReload,
            Self::Unsupported => ClientAction::ReloadApplication,
        }
    }
}

/// The subprotocol a server speaks with a client that offers `offered`,
/// or the code it closes with when the client offers none it speaks.
pub fn negotiate(offered: &[&str]) -> Result<&'static str, CloseCode> {
    if offered.contains(&SUBPROTOCOL) {
        Ok(SUBPROTOCOL)
    } else {
        Err(CloseCode::Unsupported)
    }
}

/// Whether a client that last pinged at `last_ping` owes another.
#[must_use]
pub fn ping_due(last_ping: Time, now: Time, heartbeat_secs: u32) -> bool {
    millis_between(last_ping, now) >= i64::from(heartbeat_secs) * MILLIS_PER_SECOND
}

/// Whether a side that last heard something at `last_heard` closes the
/// socket.
#[must_use]
pub fn silent(last_heard: Time, now: Time) -> bool {
    millis_between(last_heard, now) >= i64::from(SILENCE_SECS) * MILLIS_PER_SECOND
}

/// Whether a connection that opened at `connected_at` and has not yet sent
/// `auth` is closed as unauthenticated.
#[must_use]
pub fn auth_overdue(connected_at: Time, now: Time) -> bool {
    millis_between(connected_at, now) >= i64::from(AUTH_TIMEOUT_SECS) * MILLIS_PER_SECOND
}

/// Whether a connection has outlived the access token it was last
/// given, and is closed as unauthenticated.
#[must_use]
pub fn outlived(access_expires_at: Time, now: Time) -> bool {
    now >= access_expires_at
}

/// Whether a client sends `reauth` now, for the token it holds, which
/// expires at `access_expires_at`.
#[must_use]
pub fn reauth_due(access_expires_at: Time, now: Time) -> bool {
    millis_between(now, access_expires_at) <= i64::from(REAUTH_LEAD_SECS) * MILLIS_PER_SECOND
}

/// Whether a connection that holds `held` subscriptions may take another.
#[must_use]
pub const fn subscription_allowed(held: u32) -> bool {
    held < MAX_SUBSCRIPTIONS
}

/// What a client does when its socket was closed with `code`. A code that
/// is none of the protocol's, such as the one of a line that broke, is the
/// network and not the server's word: the client connects again after a
/// pause.
#[must_use]
pub fn client_action(code: u16) -> ClientAction {
    CloseCode::from_code(code).map_or(ClientAction::Reconnect, CloseCode::action)
}

/// How long a client waits before it connects again, having failed
/// `failures` times in a row: the first pause, doubled for each further
/// failure up to the longest, then moved by `jitter`, a number from -1
/// to 1 drawn at random, times [`RECONNECT_JITTER`] of itself. A jitter
/// that is no number moves nothing.
#[must_use]
pub fn reconnect_delay(failures: u32, jitter: f64) -> Duration {
    let doublings = failures.saturating_sub(1).min(u32::BITS - 1);
    let base = RECONNECT_FIRST
        .saturating_mul(1u32 << doublings)
        .min(RECONNECT_LONGEST);
    let jitter = if jitter.is_finite() {
        jitter.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    base.mul_f64(1.0 + RECONNECT_JITTER * jitter)
}

/// How many failures in a row a client counts after a connection that
/// lasted `connected_for` broke: one that held long enough starts the
/// pauses from the first again.
#[must_use]
pub fn failures_after(connected_for: Duration, failures_before: u32) -> u32 {
    if connected_for >= RECONNECT_RESET_AFTER {
        1
    } else {
        failures_before.saturating_add(1)
    }
}

/// What a server does about the frames that wait for one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BacklogAction {
    /// Nothing: little waits, or it has not waited long.
    Keep,
    /// More waits than the limit: drop the `delta` and `event` frames in
    /// the queue and send `resync` for the feeds they were of, which costs
    /// the reader one request and loses nothing. Rows are never dropped.
    Shed,
    /// More than the limit has waited for the whole time: close the
    /// connection with the code of [`CloseCode::NotKeepingUp`].
    Close,
}

/// How long, and how much, waits for one connection.
///
/// A reader that is merely slow never gets here, because deltas are merged
/// while they wait. A reader that takes nothing does: when more than the
/// limit stays queued for as long as the limit of time allows, the
/// connection is let go and loads again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backlog {
    max_bytes: i64,
    max_millis: i64,
    over_since: Option<Time>,
}

impl Backlog {
    /// A backlog that sheds above `max_bytes` and closes after `max_millis`
    /// of being above.
    #[must_use]
    pub const fn new(max_bytes: i64, max_millis: i64) -> Self {
        Self {
            max_bytes,
            max_millis,
            over_since: None,
        }
    }

    /// The limits a system ships with: [`BUFFER_MAX_BYTES`] for
    /// [`BUFFER_MAX_SECS`].
    #[must_use]
    pub fn shipped() -> Self {
        Self::new(
            BUFFER_MAX_BYTES,
            i64::from(BUFFER_MAX_SECS) * MILLIS_PER_SECOND,
        )
    }

    /// `waiting_bytes` wait for the connection at `now`.
    pub fn update(&mut self, waiting_bytes: i64, now: Time) -> BacklogAction {
        if waiting_bytes <= self.max_bytes {
            self.over_since = None;
            return BacklogAction::Keep;
        }
        let since = *self.over_since.get_or_insert(now);
        if millis_between(since, now) >= self.max_millis {
            BacklogAction::Close
        } else {
            BacklogAction::Shed
        }
    }
}

/// The round trip of a line as a client reports it: the middle one of the
/// last three measured, in milliseconds.
///
/// A round trip is measured when a socket opens and with every heartbeat,
/// as the time from a `ping` to the `pong` that repeats its `n`. The middle
/// of three is not moved by one late packet. A shell's socket is measured
/// the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoundTrips {
    latest: Vec<i64>,
}

impl RoundTrips {
    /// A line that has not been measured.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in a measurement, in milliseconds.
    pub fn record(&mut self, millis: i64) {
        self.latest.push(millis.max(0));
        if self.latest.len() > ROUND_TRIPS_WEIGHED {
            self.latest.remove(0);
        }
    }

    /// The middle one of the last three, in milliseconds; of fewer than
    /// three, the smaller of what there is; `None` before any was taken.
    #[must_use]
    pub fn estimate(&self) -> Option<i64> {
        let mut sorted = self.latest.clone();
        sorted.sort_unstable();
        sorted.get(sorted.len().checked_sub(1)? / 2).copied()
    }

    /// Whether the line is slow enough for the person to be told: the
    /// round trip is above [`SLOW_ROUND_TRIP_MILLIS`].
    #[must_use]
    pub fn is_slow(&self) -> bool {
        self.estimate()
            .is_some_and(|millis| millis > SLOW_ROUND_TRIP_MILLIS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(text: &str) -> Time {
        text.parse().unwrap()
    }

    #[test]
    fn every_close_code_says_what_a_client_does_about_it() {
        assert_eq!(
            CloseCode::from_code(1001).unwrap().action(),
            ClientAction::Reconnect
        );
        assert_eq!(
            CloseCode::from_code(1012).unwrap().action(),
            ClientAction::Reconnect
        );
        assert_eq!(
            CloseCode::from_code(4400).unwrap().action(),
            ClientAction::ReportAndReconnect
        );
        assert_eq!(
            CloseCode::from_code(4401).unwrap().action(),
            ClientAction::RefreshThenReconnect
        );
        assert_eq!(
            CloseCode::from_code(4403).unwrap().action(),
            ClientAction::SignOut
        );
        assert_eq!(
            CloseCode::from_code(4408).unwrap().action(),
            ClientAction::ReconnectAndReload
        );
        assert_eq!(
            CloseCode::from_code(4409).unwrap().action(),
            ClientAction::ReloadApplication
        );
        assert_eq!(CloseCode::from_code(1000), None);
    }

    #[test]
    fn a_client_that_does_not_offer_the_protocol_is_told_to_load_the_application_again() {
        assert_eq!(negotiate(&["orch.v1"]), Ok("orch.v1"));
        assert_eq!(negotiate(&["orch.v0", "orch.v1"]), Ok("orch.v1"));
        assert_eq!(negotiate(&["orch.v0"]), Err(CloseCode::Unsupported));
        assert_eq!(negotiate(&[]), Err(CloseCode::Unsupported));
        assert_eq!(
            negotiate(&[TERMINAL_SUBPROTOCOL]),
            Err(CloseCode::Unsupported)
        );
    }

    #[test]
    fn a_client_pings_every_heartbeat_and_a_minute_of_silence_closes() {
        let last = time("2026-10-08T17:04:00.000Z");
        assert!(!ping_due(
            last,
            time("2026-10-08T17:04:24.999Z"),
            HEARTBEAT_SECS
        ));
        assert!(ping_due(
            last,
            time("2026-10-08T17:04:25.000Z"),
            HEARTBEAT_SECS
        ));
        assert!(!silent(last, time("2026-10-08T17:04:59.999Z")));
        assert!(silent(last, time("2026-10-08T17:05:00.000Z")));
    }

    #[test]
    fn a_connection_that_outlives_its_token_is_closed() {
        let expires = time("2026-10-08T17:19:05.000Z");
        assert!(!outlived(expires, time("2026-10-08T17:19:04.999Z")));
        assert!(outlived(expires, expires));
    }

    #[test]
    fn pauses_before_connecting_again_double_from_half_a_second_to_fifteen() {
        let pauses: Vec<u128> = (1..=7)
            .map(|failures| reconnect_delay(failures, 0.0).as_millis())
            .collect();
        assert_eq!(pauses, [500, 1_000, 2_000, 4_000, 8_000, 15_000, 15_000]);
        assert_eq!(reconnect_delay(u32::MAX, 0.0), RECONNECT_LONGEST);
    }

    #[test]
    fn a_pause_is_moved_by_at_most_a_fifth_of_itself() {
        assert_eq!(reconnect_delay(2, 1.0), Duration::from_millis(1_200));
        assert_eq!(reconnect_delay(2, -1.0), Duration::from_millis(800));
        assert_eq!(reconnect_delay(2, -7.5), Duration::from_millis(800));
    }

    #[test]
    fn a_jitter_that_is_no_number_moves_nothing() {
        assert_eq!(reconnect_delay(2, f64::NAN), Duration::from_millis(1_000));
        assert_eq!(
            reconnect_delay(2, f64::INFINITY),
            Duration::from_millis(1_000)
        );
    }

    #[test]
    fn a_connection_that_held_for_half_a_minute_starts_the_pauses_over() {
        assert_eq!(failures_after(Duration::from_secs(29), 4), 5);
        assert_eq!(failures_after(Duration::from_secs(30), 4), 1);
    }

    #[test]
    fn a_connection_has_ten_seconds_to_say_who_it_is() {
        let opened = time("2026-10-08T17:04:00.000Z");
        assert!(!auth_overdue(opened, time("2026-10-08T17:04:09.999Z")));
        assert!(auth_overdue(opened, time("2026-10-08T17:04:10.000Z")));
    }

    #[test]
    fn a_client_sends_a_new_token_a_minute_before_the_old_one_runs_out() {
        let expires = time("2026-10-08T17:19:05.000Z");
        assert!(!reauth_due(expires, time("2026-10-08T17:18:04.999Z")));
        assert!(reauth_due(expires, time("2026-10-08T17:18:05.000Z")));
        // One that was missed is still due, and the server closes the connection after the token's end.
        assert!(reauth_due(expires, time("2026-10-08T17:30:00.000Z")));
    }

    #[test]
    fn a_connection_holds_sixty_four_subscriptions_and_not_a_sixty_fifth() {
        assert!(subscription_allowed(0));
        assert!(subscription_allowed(63));
        assert!(!subscription_allowed(64));
        assert!(!subscription_allowed(u32::MAX));
    }

    #[test]
    fn a_close_that_is_not_the_servers_word_is_the_network_and_the_client_connects_again() {
        assert_eq!(client_action(4408), ClientAction::ReconnectAndReload);
        assert_eq!(client_action(4403), ClientAction::SignOut);
        for code in [1000, 1006, 1011, 4000, 4500] {
            assert_eq!(client_action(code), ClientAction::Reconnect, "{code}");
        }
    }

    #[test]
    fn the_middle_of_the_last_three_round_trips_is_not_moved_by_one_late_packet() {
        let mut line = RoundTrips::new();
        assert_eq!(line.estimate(), None);
        assert!(!line.is_slow());
        line.record(300);
        assert_eq!(line.estimate(), Some(300));
        assert!(line.is_slow());
        // Of two, the smaller: one late packet does not make a quick line slow.
        line.record(40);
        assert_eq!(line.estimate(), Some(40));
        line.record(45);
        assert_eq!(line.estimate(), Some(45));
        // A packet that is late by a second.
        line.record(1_000);
        assert_eq!(line.estimate(), Some(45));
        assert!(!line.is_slow());
        // The first of the four has gone: 45, 1000 and 1100 now.
        line.record(1_100);
        assert_eq!(line.estimate(), Some(1_000));
        assert!(line.is_slow());
    }

    #[test]
    fn a_backlog_sheds_text_at_once_and_closes_the_connection_after_half_a_minute() {
        let mut backlog = Backlog::shipped();
        let mib = 1024 * 1024;
        let at = |seconds: u32| time(&format!("2026-10-08T17:04:{seconds:02}.000Z"));
        assert_eq!(backlog.update(4 * mib, at(0)), BacklogAction::Keep);
        assert_eq!(backlog.update(4 * mib + 1, at(1)), BacklogAction::Shed);
        // Still over, and shedding did not bring it under.
        assert_eq!(backlog.update(5 * mib, at(30)), BacklogAction::Shed);
        assert_eq!(backlog.update(5 * mib, at(31)), BacklogAction::Close);
    }

    #[test]
    fn a_backlog_that_falls_under_the_limit_starts_its_half_minute_again() {
        let mut backlog = Backlog::new(100, 30_000);
        let at = |seconds: u32| time(&format!("2026-10-08T17:04:{seconds:02}.000Z"));
        assert_eq!(backlog.update(500, at(0)), BacklogAction::Shed);
        assert_eq!(backlog.update(50, at(20)), BacklogAction::Keep);
        assert_eq!(backlog.update(500, at(25)), BacklogAction::Shed);
        // Thirty seconds after the first time it went over, but only five after the second.
        assert_eq!(backlog.update(500, at(30)), BacklogAction::Shed);
        assert_eq!(backlog.update(500, at(55)), BacklogAction::Close);
    }

    #[test]
    fn a_line_is_slow_above_a_hundred_and_fifty_milliseconds_and_not_at_it() {
        let mut line = RoundTrips::new();
        line.record(150);
        assert!(!line.is_slow());
        line.record(151);
        line.record(151);
        assert!(line.is_slow());
    }
}
