//! How long a plugin may take, whether it is alive, and which requests are still waiting.
//!
//! The host starts, or connects to, a plugin at its own start and keeps it. A plugin that exits, disconnects
//! or misses a `ping` for ten seconds is started or connected to again, with a pause that doubles from a
//! second to a minute ([`crate::registration::backoff`]). While it is down, whatever depends on it reports
//! `unavailable` with the plugin's name.
//!
//! A request with no answer within the method's deadline fails as `timeout`, retryable; progress messages
//! do not extend it. The deadlines are those of the link's commands for adapters, and 60 seconds otherwise,
//! unless the link states a longer one for the operation a method stands for ([`deadline`]).
//!
//! These rules are pure: the time is passed in. [`Liveness`] says when to ping and when to give up, and
//! [`Requests`] numbers the requests, matches the answers, and finds the ones that ran out of time.

use std::collections::BTreeMap;
use std::time::Duration;

use orch_types::Time;

use crate::messages::{HELLO_ID, RequestId};
use crate::registration::PluginKind;

/// How long a line from the plugin may be missing before the host pings it: thirty seconds.
pub const PING_AFTER_IDLE: Duration = Duration::from_secs(30);
/// How long the host waits for the answer to a ping before it treats the plugin as gone: ten seconds.
pub const PING_REPLY_WITHIN: Duration = Duration::from_secs(10);
/// How long a plugin has to finish and exit after `shutdown`: ten seconds, after which the host ends it.
pub const SHUTDOWN_WITHIN: Duration = Duration::from_secs(10);
/// The deadline of a method that states none: sixty seconds.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(60);
/// How long a provider source has to say what it is (`describe`): ten seconds.
pub const PROVIDER_DESCRIBE_DEADLINE: Duration = Duration::from_secs(10);
/// How long a provider source has to read facts or present a key (`facts.read`, `credential.present`): eight
/// seconds, so that a Refresh is answered within the API's ten.
pub const PROVIDER_READ_DEADLINE: Duration = Duration::from_secs(8);
/// How long a provider source has to list models or spend a reset (`models.list`, `resets.spend`): thirty
/// seconds, as the link's command for either.
pub const PROVIDER_LIST_DEADLINE: Duration = Duration::from_secs(30);

const fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// The deadline of `method` of a plugin of `kind`.
///
/// For an adapter these are the deadlines of the link's agent-role commands. For a driver and an executor
/// the transport has no table of its own, so a method has the deadline of the link command it serves where
/// the link states one (creating a unit 10 minutes, starting, stopping or removing one 2 minutes, making an
/// image available 60 minutes, starting an executor 2 minutes). A provider source's methods have the deadlines
/// of its contract, the same for a program and an address ([`PROVIDER_DESCRIBE_DEADLINE`],
/// [`PROVIDER_READ_DEADLINE`], [`PROVIDER_LIST_DEADLINE`]). Every other method has [`DEFAULT_DEADLINE`].
#[must_use]
pub fn deadline(kind: PluginKind, method: &str) -> Duration {
    match (kind, method) {
        (PluginKind::Adapter, "agent.start" | "session.open") => secs(120),
        (
            PluginKind::Adapter,
            "agent.stop"
            | "turn.start"
            | "provider.login.start"
            | "provider.credential.accept"
            | "agent.credential.return",
        ) => secs(60),
        (
            PluginKind::Adapter,
            "agent.discover_sessions"
            | "session.close"
            | "turn.steer"
            | "turn.interrupt"
            | "approval.resolve"
            | "orch.tool_result"
            | "provider.login.input"
            | "provider.login.cancel"
            | "provider.logout"
            | "provider.refresh"
            | "provider.discover_models"
            | "provider.credential.issue"
            | "agent.credential.set"
            | "mcp.import",
        ) => secs(30),
        (PluginKind::Adapter, "session.export") => secs(10 * 60),
        (PluginKind::Adapter, "session.resync") => secs(5 * 60),
        (PluginKind::Driver, "create") => secs(10 * 60),
        (PluginKind::Driver, "start" | "stop" | "remove") => secs(120),
        (
            PluginKind::Driver,
            "ensure_image"
            | "rebuild_image"
            | "keep_files"
            | "export_kept_image"
            | "import_kept_image"
            | "sweep_kept_metadata",
        ) => secs(60 * 60),
        (PluginKind::Executor, "start") => secs(120),
        (PluginKind::Provider, "describe") => PROVIDER_DESCRIBE_DEADLINE,
        (PluginKind::Provider, "facts.read" | "credential.present") => PROVIDER_READ_DEADLINE,
        (PluginKind::Provider, "models.list" | "resets.spend") => PROVIDER_LIST_DEADLINE,
        _ => DEFAULT_DEADLINE,
    }
}

/// What the host does about a plugin's silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessAction {
    /// Nothing yet.
    Wait,
    /// Send a `ping`.
    Ping,
    /// The ping was not answered in time: restart or reconnect.
    Restart,
}

/// When a plugin last spoke, and whether a ping is waiting for its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liveness {
    last_heard: Time,
    ping_sent: Option<Time>,
}

impl Liveness {
    /// A plugin that was heard from at `now`: just now, at its handshake.
    #[must_use]
    pub fn new(now: Time) -> Self {
        Self {
            last_heard: now,
            ping_sent: None,
        }
    }

    /// A line arrived from the plugin: it is alive. An answer is not needed to count as traffic, but only
    /// the answer to the ping ends the wait for it ([`Liveness::ping_answered`]).
    pub fn heard(&mut self, now: Time) {
        self.last_heard = now;
    }

    /// A ping was sent at `now`.
    pub fn ping_sent(&mut self, now: Time) {
        self.ping_sent = Some(now);
    }

    /// The answer to the ping arrived.
    pub fn ping_answered(&mut self, now: Time) {
        self.ping_sent = None;
        self.last_heard = now;
    }

    /// What to do at `now`: ping after [`PING_AFTER_IDLE`] without a line from the plugin, and give up when a
    /// ping has waited [`PING_REPLY_WITHIN`].
    #[must_use]
    pub fn next(&self, now: Time) -> LivenessAction {
        let millis = |duration: Duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        if let Some(sent) = self.ping_sent {
            return if now.unix_millis() - sent.unix_millis() >= millis(PING_REPLY_WITHIN) {
                LivenessAction::Restart
            } else {
                LivenessAction::Wait
            };
        }
        if now.unix_millis() - self.last_heard.unix_millis() >= millis(PING_AFTER_IDLE) {
            LivenessAction::Ping
        } else {
            LivenessAction::Wait
        }
    }
}

#[derive(Debug, Clone)]
struct Open {
    method: String,
    due_millis: i64,
}

/// The requests a host has sent and not seen answered. It numbers them (the handshake is 0, the first
/// request after it is 1), matches answers by id, and finds those that ran out of time.
#[derive(Debug, Clone)]
pub struct Requests {
    next: RequestId,
    open: BTreeMap<RequestId, Open>,
}

impl Default for Requests {
    fn default() -> Self {
        Self::new()
    }
}

impl Requests {
    /// No request is open and the next id is the first after the handshake.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: HELLO_ID + 1,
            open: BTreeMap::new(),
        }
    }

    /// Opens a request for `method` sent at `sent_at`, due after `deadline`, and returns its id.
    pub fn open(&mut self, method: &str, sent_at: Time, deadline: Duration) -> RequestId {
        let id = self.next;
        self.next += 1;
        let due_millis = sent_at
            .unix_millis()
            .saturating_add(i64::try_from(deadline.as_millis()).unwrap_or(i64::MAX));
        self.open.insert(
            id,
            Open {
                method: method.to_owned(),
                due_millis,
            },
        );
        id
    }

    /// Whether request `id` is open. Progress for a request that is not open is ignored.
    #[must_use]
    pub fn is_open(&self, id: RequestId) -> bool {
        self.open.contains_key(&id)
    }

    /// An answer to `id` arrived: the request is closed, and its method returned. `None` for an answer to a
    /// request that is not open (a late or repeated answer), which is ignored.
    pub fn answered(&mut self, id: RequestId) -> Option<String> {
        self.open.remove(&id).map(|open| open.method)
    }

    /// Closes the requests that ran out of time by `now`, oldest id first, and returns them with their
    /// methods. Each fails as `timeout`, retryable. Progress does not move a request's deadline.
    pub fn expired(&mut self, now: Time) -> Vec<(RequestId, String)> {
        let ids: Vec<RequestId> = self
            .open
            .iter()
            .filter(|(_, open)| now.unix_millis() >= open.due_millis)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| self.open.remove(&id).map(|open| (id, open.method)))
            .collect()
    }

    /// How long until the first request runs out of time, from `now`; zero if one has. `None` when no request
    /// is open.
    #[must_use]
    pub fn next_due_in(&self, now: Time) -> Option<Duration> {
        let soonest = self.open.values().map(|open| open.due_millis).min()?;
        Some(Duration::from_millis(
            u64::try_from(soonest - now.unix_millis()).unwrap_or(0),
        ))
    }

    /// How many requests are open.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u32, millis: u32) -> Time {
        format!(
            "2026-10-11T10:{:02}:{:02}.{millis:03}Z",
            seconds / 60,
            seconds % 60
        )
        .parse()
        .unwrap()
    }

    #[test]
    fn a_plugin_that_is_silent_for_thirty_seconds_is_pinged_and_one_that_does_not_answer_in_ten_is_restarted()
     {
        let mut live = Liveness::new(at(0, 0));
        assert_eq!(live.next(at(29, 999)), LivenessAction::Wait);
        assert_eq!(live.next(at(30, 0)), LivenessAction::Ping);
        live.ping_sent(at(30, 0));
        assert_eq!(live.next(at(39, 999)), LivenessAction::Wait);
        assert_eq!(live.next(at(40, 0)), LivenessAction::Restart);
        // An answer in time ends the wait, and the silence starts over.
        live.ping_answered(at(31, 0));
        assert_eq!(live.next(at(45, 0)), LivenessAction::Wait);
        assert_eq!(live.next(at(59, 999)), LivenessAction::Wait);
        assert_eq!(live.next(at(61, 0)), LivenessAction::Ping);
    }

    #[test]
    fn any_line_from_the_plugin_counts_as_traffic_but_only_the_answer_ends_the_wait_for_a_ping() {
        let mut live = Liveness::new(at(0, 0));
        live.heard(at(20, 0));
        assert_eq!(
            live.next(at(49, 0)),
            LivenessAction::Wait,
            "thirty seconds from the last line"
        );
        assert_eq!(live.next(at(50, 0)), LivenessAction::Ping);
        live.ping_sent(at(50, 0));
        live.heard(at(55, 0));
        assert_eq!(
            live.next(at(60, 0)),
            LivenessAction::Restart,
            "events are not the answer to the ping"
        );
    }

    #[test]
    fn a_request_that_is_not_answered_by_its_deadline_fails_and_progress_does_not_extend_it() {
        let mut requests = Requests::new();
        let create = requests.open("create", at(0, 0), Duration::from_secs(60));
        let list = requests.open("list", at(1, 0), Duration::from_secs(5));
        assert_eq!((create, list), (1, 2), "the handshake was 0");
        assert_eq!(requests.in_flight(), 2);
        assert!(requests.is_open(create));
        assert_eq!(requests.next_due_in(at(2, 0)), Some(Duration::from_secs(4)));
        assert!(requests.expired(at(5, 999)).is_empty());
        assert_eq!(requests.expired(at(6, 0)), vec![(2, "list".to_owned())]);
        assert_eq!(requests.answered(create).as_deref(), Some("create"));
        assert_eq!(
            requests.answered(create),
            None,
            "a repeated answer is ignored"
        );
        assert_eq!(requests.next_due_in(at(6, 0)), None);
        let slow = requests.open(
            "ensure_image",
            at(0, 0),
            deadline(PluginKind::Driver, "ensure_image"),
        );
        assert!(
            requests.expired(at(59, 0)).is_empty(),
            "an hour for an image"
        );
        assert!(requests.is_open(slow));
    }

    #[test]
    fn requests_that_ran_out_of_time_are_found_oldest_first() {
        let mut requests = Requests::new();
        for method in ["a", "b", "c"] {
            requests.open(method, at(0, 0), Duration::from_secs(1));
        }
        let gone = requests.expired(at(2, 0));
        assert_eq!(
            gone.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(requests.in_flight(), 0);
    }

    #[test]
    fn the_deadlines_are_those_of_the_links_commands_for_adapters_and_sixty_seconds_otherwise() {
        assert_eq!(
            deadline(PluginKind::Adapter, "agent.start"),
            Duration::from_secs(120)
        );
        assert_eq!(
            deadline(PluginKind::Adapter, "turn.start"),
            Duration::from_secs(60)
        );
        assert_eq!(
            deadline(PluginKind::Adapter, "turn.interrupt"),
            Duration::from_secs(30)
        );
        assert_eq!(
            deadline(PluginKind::Adapter, "session.export"),
            Duration::from_secs(600)
        );
        assert_eq!(
            deadline(PluginKind::Adapter, "session.resync"),
            Duration::from_secs(300)
        );
        assert_eq!(
            deadline(PluginKind::Driver, "create"),
            Duration::from_secs(600)
        );
        assert_eq!(deadline(PluginKind::Driver, "inspect"), DEFAULT_DEADLINE);
        assert_eq!(deadline(PluginKind::Notifier, "deliver"), DEFAULT_DEADLINE);
        assert_eq!(
            deadline(PluginKind::Summarizer, "summarize"),
            DEFAULT_DEADLINE
        );
        assert_eq!(deadline(PluginKind::Adapter, "x_unknown"), DEFAULT_DEADLINE);
        assert_eq!(PING_AFTER_IDLE, Duration::from_secs(30));
        assert_eq!(SHUTDOWN_WITHIN, Duration::from_secs(10));
    }
}
