//! How often a side may speak: reports that are merged, and notices that are dropped.
//!
//! The protocol limits how often some things are said. Two rules cover
//! them. A [`Throttle`] merges: whatever changes while a report is not yet
//! allowed is reported once the interval has passed, so that nothing is
//! lost and a burst becomes one report (who is looking at a thread, a
//! resource that changed). A [`RateLimit`] drops: a notice that comes too
//! soon after the last is not worth saying later (somebody is typing).
//!
//! Both are told the moment, and neither reads a clock.

use orch_types::Time;

/// Milliseconds from `earlier` to `later`; negative if the clock went back.
pub(crate) fn millis_between(earlier: Time, later: Time) -> i64 {
    later.unix_millis().saturating_sub(earlier.unix_millis())
}

/// Lets a report through at most once in an interval, and reports a change
/// that had to wait as soon as the interval is over.
///
/// The first change is reported at once. A change that comes before the
/// interval has passed since the last report is not reported but
/// remembered; any number of them are one. [`Throttle::wait_millis`] says
/// how long until that one is due, and [`Throttle::poll`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Throttle {
    interval_millis: i64,
    last_sent: Option<Time>,
    waiting: bool,
}

impl Throttle {
    /// A throttle that allows one report in `interval_millis`.
    #[must_use]
    pub const fn new(interval_millis: i64) -> Self {
        Self {
            interval_millis,
            last_sent: None,
            waiting: false,
        }
    }

    fn open(&self, now: Time) -> bool {
        self.last_sent
            .is_none_or(|last| millis_between(last, now) >= self.interval_millis)
    }

    /// Something changed at `now`. `true` when it is to be reported now;
    /// otherwise it waits, and [`Throttle::poll`] reports it when it is due.
    pub fn changed(&mut self, now: Time) -> bool {
        if self.open(now) {
            self.last_sent = Some(now);
            self.waiting = false;
            true
        } else {
            self.waiting = true;
            false
        }
    }

    /// Whether a change that had to wait is due at `now`, and is to be
    /// reported.
    pub fn poll(&mut self, now: Time) -> bool {
        if self.waiting && self.open(now) {
            self.last_sent = Some(now);
            self.waiting = false;
            true
        } else {
            false
        }
    }

    /// How many milliseconds from `now` until the change that waits is
    /// due; `None` when none waits.
    #[must_use]
    pub fn wait_millis(&self, now: Time) -> Option<i64> {
        if !self.waiting {
            return None;
        }
        let since = self
            .last_sent
            .map_or(self.interval_millis, |last| millis_between(last, now));
        Some((self.interval_millis - since).max(0))
    }
}

/// Lets a notice through if enough time has passed since the last that
/// was let through, and drops the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    min_gap_millis: i64,
    last_admitted: Option<Time>,
}

impl RateLimit {
    /// A limit of one notice in `min_gap_millis`.
    #[must_use]
    pub const fn new(min_gap_millis: i64) -> Self {
        Self {
            min_gap_millis,
            last_admitted: None,
        }
    }

    /// Whether a notice at `now` is let through. A notice that is not is
    /// dropped and does not count: it does not postpone the next.
    pub fn admit(&mut self, now: Time) -> bool {
        let allowed = self
            .last_admitted
            .is_none_or(|last| millis_between(last, now) >= self.min_gap_millis);
        if allowed {
            self.last_admitted = Some(now);
        }
        allowed
    }
}

/// Keeps something running for those who use it, and for a while after the
/// last of them has gone, so that a screen that is left and opened again
/// does not stop and start it.
///
/// A server keeps a watch (of a directory, of what an environment uses) for
/// the subscribers of a feed, and a client keeps a subscription for the
/// components that show a feed. Both let go ten seconds after the last
/// user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Linger {
    linger_millis: i64,
    users: u32,
    running: bool,
    idle_since: Option<Time>,
}

impl Linger {
    /// Something that is kept for `linger_millis` after its last user.
    #[must_use]
    pub const fn new(linger_millis: i64) -> Self {
        Self {
            linger_millis,
            users: 0,
            running: false,
            idle_since: None,
        }
    }

    /// How many are using it.
    #[must_use]
    pub const fn users(&self) -> u32 {
        self.users
    }

    /// A user arrives. `true` when it has to be started: nothing was
    /// running, which is not so when the last user left a moment ago.
    pub fn join(&mut self) -> bool {
        self.users = self.users.saturating_add(1);
        self.idle_since = None;
        !std::mem::replace(&mut self.running, true)
    }

    /// A user leaves at `now`.
    pub fn leave(&mut self, now: Time) {
        self.users = self.users.saturating_sub(1);
        if self.users == 0 && self.running && self.idle_since.is_none() {
            self.idle_since = Some(now);
        }
    }

    /// Whether, at `now`, it has gone unused for long enough to be stopped.
    /// `true` once; it then has to be started again by the next user.
    pub fn poll(&mut self, now: Time) -> bool {
        let Some(since) = self.idle_since else {
            return false;
        };
        if millis_between(since, now) >= self.linger_millis {
            self.idle_since = None;
            self.running = false;
            true
        } else {
            false
        }
    }
}

/// The moment `millis` after 17:00:00 on 8 October 2026, written out so that
/// no calendar library is needed; good for up to six hours.
#[cfg(test)]
pub(crate) fn moment(millis: i64) -> Time {
    let (hours, minutes, seconds, ms) = (
        17 + millis / 3_600_000,
        (millis / 60_000) % 60,
        (millis / 1_000) % 60,
        millis % 1_000,
    );
    format!("2026-10-08T{hours:02}:{minutes:02}:{seconds:02}.{ms:03}Z")
        .parse()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::moment as at;
    use super::*;

    #[test]
    fn the_first_change_is_reported_at_once_and_the_next_waits_for_the_interval() {
        let mut throttle = Throttle::new(1_000);
        assert!(throttle.changed(at(0)));
        assert!(!throttle.changed(at(300)));
        assert_eq!(throttle.wait_millis(at(300)), Some(700));
        assert!(!throttle.poll(at(999)));
        assert!(throttle.poll(at(1_000)));
        assert_eq!(throttle.wait_millis(at(1_000)), None);
    }

    #[test]
    fn a_burst_of_changes_is_one_report_and_none_is_lost() {
        let mut throttle = Throttle::new(1_000);
        assert!(throttle.changed(at(0)));
        for millis in [100, 200, 450, 800] {
            assert!(!throttle.changed(at(millis)));
        }
        assert!(throttle.poll(at(1_000)));
        // One report was owed, and it was made: nothing more is due.
        assert!(!throttle.poll(at(2_000)));
    }

    #[test]
    fn a_change_after_a_quiet_interval_is_reported_at_once() {
        let mut throttle = Throttle::new(1_000);
        assert!(throttle.changed(at(0)));
        assert!(throttle.changed(at(1_500)));
        assert!(!throttle.changed(at(1_600)));
    }

    #[test]
    fn a_change_that_comes_when_it_is_due_is_reported_by_that_change_and_not_again() {
        let mut throttle = Throttle::new(1_000);
        assert!(throttle.changed(at(0)));
        assert!(!throttle.changed(at(500)));
        assert!(throttle.changed(at(1_200)));
        assert!(!throttle.poll(at(5_000)));
    }

    #[test]
    fn nothing_waits_when_nothing_changed() {
        let throttle = Throttle::new(1_000);
        assert_eq!(throttle.wait_millis(at(0)), None);
    }

    #[test]
    fn something_is_started_by_its_first_user_and_stopped_ten_seconds_after_its_last() {
        let mut watch = Linger::new(10_000);
        assert!(watch.join());
        assert!(!watch.join());
        assert_eq!(watch.users(), 2);
        watch.leave(at(1_000));
        assert!(!watch.poll(at(60_000)), "one user is left");
        watch.leave(at(2_000));
        assert!(!watch.poll(at(11_999)));
        assert!(watch.poll(at(12_000)));
        // Once: it is stopped now and the next user starts it again.
        assert!(!watch.poll(at(13_000)));
        assert!(watch.join());
    }

    #[test]
    fn a_user_who_returns_within_the_ten_seconds_finds_it_running() {
        let mut watch = Linger::new(10_000);
        assert!(watch.join());
        watch.leave(at(0));
        assert!(!watch.join(), "it is still running");
        watch.leave(at(5_000));
        // The ten seconds count from the later departure.
        assert!(!watch.poll(at(14_999)));
        assert!(watch.poll(at(15_000)));
    }

    #[test]
    fn a_notice_that_comes_too_soon_is_dropped_and_does_not_postpone_the_next() {
        let mut limit = RateLimit::new(3_000);
        assert!(limit.admit(at(0)));
        assert!(!limit.admit(at(1_000)));
        assert!(!limit.admit(at(2_999)));
        // The dropped ones did not count: three seconds after the one that was let through is enough.
        assert!(limit.admit(at(3_000)));
        assert!(!limit.admit(at(5_000)));
        assert!(limit.admit(at(6_000)));
    }
}
