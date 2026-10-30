//! Visible reader aggregation.
//!
//! On the feed of a thread a person is looking when at least one of their
//! connections says the thread is in front of them. This process knows its
//! own connections; the others say theirs through the database's
//! notifications, each process telling who reads what on its connections and
//! again every few seconds, so that one that died is forgotten after a while.
//! The list a subscriber is sent is the union of all of them, the person who
//! has looked longest first.
//!
//! The list is sent whole, when it changes, at most once a second for a feed
//! ([`Throttle`]): a tab switched back and forth many times a second is
//! reported once, as it then stands.

use crate::realtime::server::Connection;
use crate::realtime::{Fail, Inner, Source};
use chrono::DateTime;
use orch_realtime_contract::feeds::Feed;
use orch_realtime_contract::pacing::Throttle;
use orch_realtime_contract::presence::{PRESENCE_INTERVAL_MILLIS, PresenceUser, VisibleSet};
use orch_types::{Id, Time};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// How often a process says who reads what on its connections, again, so
/// that the others know it is still there. Ten seconds is a few round trips of
/// the database's notifications and a small share of a minute, and is short
/// enough that a list that lost a reader is repaired before anyone notices.
pub const REFRESH_SECS: i64 = 10;
/// How long what a process said is believed without being said again: three
/// times the refresh, so that one lost announcement does not make its readers
/// disappear.
pub const BELIEVED_SECS: i64 = 3 * REFRESH_SECS;

/// What another process said of one feed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Remote {
    readers: Vec<(Id, Time)>,
    heard: Time,
}

/// Whether and when a changed list is to be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Now.
    Now,
    /// Not yet: this is the first change that had to wait, and the list goes
    /// out after this many milliseconds, when [`Presence::poll`] says so.
    After(i64),
    /// Not yet: a change is waiting already, and is on its way.
    Waiting,
}

#[derive(Debug)]
struct Readers {
    local: VisibleSet,
    others: HashMap<String, Remote>,
    throttle: Throttle,
}

impl Readers {
    fn new() -> Self {
        Self {
            local: VisibleSet::new(),
            others: HashMap::new(),
            throttle: Throttle::new(PRESENCE_INTERVAL_MILLIS),
        }
    }

    /// Everyone who is looking, here and elsewhere, with since when: the
    /// earliest of the moments they began on any connection.
    fn merged(&self) -> Vec<(Id, Time)> {
        let mut by_person: BTreeMap<Id, Time> = BTreeMap::new();
        let elsewhere = self
            .others
            .values()
            .flat_map(|remote| remote.readers.iter());
        for (person, since) in self.local.readers().iter().chain(elsewhere) {
            by_person
                .entry(*person)
                .and_modify(|held| *held = (*held).min(*since))
                .or_insert(*since);
        }
        let mut list: Vec<(Id, Time)> = by_person.into_iter().collect();
        list.sort_by_key(|(person, since)| (*since, *person));
        list
    }

    fn is_empty(&self) -> bool {
        self.local.is_empty() && self.others.is_empty()
    }
}

/// Who reads what.
#[derive(Debug, Default)]
pub struct Presence {
    feeds: Mutex<HashMap<Feed, Readers>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Presence {
    /// Nobody reads anything.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The connection `connection` of `user` says that `feed` is, or is no
    /// longer, in front of them. `true` when the list of those who look
    /// changed by it.
    pub fn set_visible(
        &self,
        feed: &Feed,
        user: Id,
        connection: &str,
        visible: bool,
        now: Time,
    ) -> bool {
        let mut feeds = lock(&self.feeds);
        if !visible && !feeds.contains_key(feed) {
            return false;
        }
        let readers = feeds.entry(feed.clone()).or_insert_with(Readers::new);
        let before = readers.merged();
        readers.local.set_visible(user, connection, visible, now);
        let changed = readers.merged() != before;
        if readers.is_empty() && !readers.throttle_waits(now) {
            feeds.remove(feed);
        }
        changed
    }

    /// The connection `connection` ended. The feeds whose list changed by it.
    pub fn connection_gone(&self, connection: &str) -> Vec<Feed> {
        let mut feeds = lock(&self.feeds);
        let mut changed = Vec::new();
        for (feed, readers) in feeds.iter_mut() {
            let before = readers.merged();
            readers.local.connection_gone(connection);
            if readers.merged() != before {
                changed.push(feed.clone());
            }
        }
        changed
    }

    /// What the process `process` said of `feed`: those who read on its
    /// connections. `true` when the list changed by it.
    pub fn heard(&self, feed: &Feed, process: &str, readers: Vec<(Id, Time)>, now: Time) -> bool {
        let mut feeds = lock(&self.feeds);
        if readers.is_empty() && !feeds.contains_key(feed) {
            return false;
        }
        let entry = feeds.entry(feed.clone()).or_insert_with(Readers::new);
        let before = entry.merged();
        if readers.is_empty() {
            entry.others.remove(process);
        } else {
            entry.others.insert(
                process.to_owned(),
                Remote {
                    readers,
                    heard: now,
                },
            );
        }
        entry.merged() != before
    }

    /// Forgets what the processes that have not been heard from for as long as
    /// they are believed said. The feeds whose list changed by it.
    pub fn forget_the_silent(&self, now: Time) -> Vec<Feed> {
        let mut feeds = lock(&self.feeds);
        let mut changed = Vec::new();
        for (feed, readers) in feeds.iter_mut() {
            let before = readers.merged();
            readers
                .others
                .retain(|_, remote| seconds_between(remote.heard, now) < BELIEVED_SECS);
            if readers.merged() != before {
                changed.push(feed.clone());
            }
        }
        // A feed nobody reads and whose list is not waiting to be sent is not
        // kept in mind.
        feeds.retain(|_, readers| !readers.is_empty() || readers.throttle_waits(now));
        changed
    }

    /// Those who read `feed`, the longest first.
    #[must_use]
    pub fn list(&self, feed: &Feed) -> Vec<(Id, Time)> {
        lock(&self.feeds)
            .get(feed)
            .map(Readers::merged)
            .unwrap_or_default()
    }

    /// Who reads what on this process's own connections, for the others.
    #[must_use]
    pub fn local(&self) -> Vec<(Feed, Vec<(Id, Time)>)> {
        lock(&self.feeds)
            .iter()
            .filter(|(_, readers)| !readers.local.is_empty())
            .map(|(feed, readers)| (feed.clone(), readers.local.readers()))
            .collect()
    }

    /// Those who read `feed` on this process's own connections.
    #[must_use]
    pub fn local_of(&self, feed: &Feed) -> Vec<(Id, Time)> {
        lock(&self.feeds)
            .get(feed)
            .map(|readers| readers.local.readers())
            .unwrap_or_default()
    }

    /// The list of `feed` changed at `now`: whether to send it now.
    pub fn changed(&self, feed: &Feed, now: Time) -> Verdict {
        let mut feeds = lock(&self.feeds);
        let entry = feeds.entry(feed.clone()).or_insert_with(Readers::new);
        let was_waiting = entry.throttle.wait_millis(now).is_some();
        if entry.throttle.changed(now) {
            return Verdict::Now;
        }
        if was_waiting {
            Verdict::Waiting
        } else {
            Verdict::After(
                entry
                    .throttle
                    .wait_millis(now)
                    .unwrap_or(PRESENCE_INTERVAL_MILLIS),
            )
        }
    }

    /// Whether a change that had to wait is due at `now`, and is to be sent.
    pub fn poll(&self, feed: &Feed, now: Time) -> bool {
        lock(&self.feeds)
            .get_mut(feed)
            .is_some_and(|entry| entry.throttle.poll(now))
    }
}

impl Readers {
    fn throttle_waits(&self, now: Time) -> bool {
        self.throttle.wait_millis(now).is_some()
    }
}

fn seconds_between(earlier: Time, later: Time) -> i64 {
    later.unix_millis().saturating_sub(earlier.unix_millis()) / 1_000
}

// ---------------------------------------------------------------- what the server does with it

/// The bytes one reader takes in the list a process says, with some room for
/// the rest of what it says: a list that would not fit in one announcement is
/// cut here, so that the readers who come first are the ones who are told.
const BYTES_PER_READER: usize = 64;
/// What a process says besides its readers: its name, the feed's name, the
/// braces.
const BYTES_BESIDES_READERS: usize = 200;

impl Inner {
    /// The people who are looking at `feed`, with the names a frame gives them.
    pub(crate) async fn presence_users(&self, feed: &Feed) -> Result<Vec<PresenceUser>, Fail> {
        let list = self.presence.list(feed);
        let ids: Vec<Id> = list.iter().map(|(person, _)| *person).collect();
        let mut conn = self.db.acquire().await?;
        let people = crate::middleware::authorization::refusals::user_refs(&mut conn, &ids)
            .await
            .map_err(|error| Fail::other(error.problem.detail))?;
        Ok(list
            .into_iter()
            .filter_map(|(person, since)| {
                people.get(&person).map(|user| PresenceUser {
                    user: user.clone(),
                    since,
                })
            })
            .collect())
    }

    /// A connection says that a feed is, or is no longer, in front of its
    /// person. Ignored for a feed that does not say who is looking and for one
    /// the connection does not follow.
    pub(crate) fn set_visible(
        self: &Arc<Self>,
        connection: &Arc<Connection>,
        feed: &Feed,
        visible: bool,
    ) {
        if !feed.has_presence() || !connection.follows(feed) {
            return;
        }
        // An admin who only inspects a chat is left out of who looks at it.
        if connection
            .subscriptions()
            .iter()
            .any(|sub| sub.feed() == feed && sub.inspecting())
        {
            return;
        }
        let local_changed = connection.mark_visible(feed, visible);
        let changed = self.presence.set_visible(
            feed,
            connection.user,
            &connection.name,
            visible,
            self.clock.time(),
        );
        // Another process may already report this person with the same since time.
        // Our own contribution still has to be published, especially its final empty list.
        if changed || local_changed {
            self.presence_changed(feed);
        }
    }

    /// Job interest shares the existing presence leases without publishing a browser reader list.
    pub(crate) fn watch_job(self: &Arc<Self>, connection: &Arc<Connection>, feed: &Feed) {
        if !matches!(feed, Feed::Job(_)) || !connection.follows(feed) {
            return;
        }
        let local_changed = connection.mark_visible(feed, true);
        let changed = self.presence.set_visible(
            feed,
            connection.user,
            &connection.name,
            true,
            self.clock.time(),
        );
        if changed || local_changed {
            self.presence_changed(feed);
        }
    }

    /// A connection stopped following a feed: if it was the last way that its
    /// person was looking at it, they are not looking any more.
    pub(crate) fn leave_presence(self: &Arc<Self>, connection: &Arc<Connection>, feed: &Feed) {
        if (!feed.has_presence() && !matches!(feed, Feed::Job(_))) || connection.follows(feed) {
            return;
        }
        if connection.mark_visible(feed, false) {
            self.presence.set_visible(
                feed,
                connection.user,
                &connection.name,
                false,
                self.clock.time(),
            );
            // Leaving a local subscription also withdraws it from the other processes.
            // Their copies can keep the merged user list unchanged until this is announced.
            self.presence_changed(feed);
        }
    }

    /// A connection ended: whatever it was showing is not shown.
    pub(crate) fn presence_ended(self: &Arc<Self>, connection: &Connection) {
        let mut changed: std::collections::HashSet<_> = self
            .presence
            .connection_gone(&connection.name)
            .into_iter()
            .collect();
        changed.extend(connection.visible_feeds());
        for feed in changed {
            self.presence_changed(&feed);
        }
    }

    /// The list of `feed` changed: it is sent now, or when the second since the
    /// last is over.
    pub(crate) fn presence_changed(self: &Arc<Self>, feed: &Feed) {
        let inner = Arc::clone(self);
        let feed = feed.clone();
        match self.presence.changed(&feed, self.clock.time()) {
            Verdict::Now => {
                self.tasks
                    .spawn(async move { inner.broadcast_presence(feed).await });
            }
            Verdict::After(millis) => {
                self.tasks.spawn(async move {
                    let mut wait = Duration::from_millis(u64::try_from(millis).unwrap_or(0));
                    loop {
                        tokio::select! {
                            () = tokio::time::sleep(wait) => {}
                            () = inner.stop.cancelled() => return,
                        }
                        if inner.presence.poll(&feed, inner.clock.time()) {
                            inner.broadcast_presence(feed).await;
                            return;
                        }
                        // The second is not quite over by the clock of the
                        // application: look again a moment later.
                        wait = Duration::from_millis(25);
                    }
                });
            }
            Verdict::Waiting => {}
        }
    }

    /// Sends the list of `feed` to everyone who follows it, and says it to the
    /// other processes.
    async fn broadcast_presence(self: Arc<Self>, feed: Feed) {
        self.publish_presence(&feed).await;
        let source = match &feed {
            Feed::Thread(thread) => Source::Thread(*thread),
            Feed::Chat(chat) => Source::Chat(*chat),
            Feed::Doc(document) => Source::Document(*document),
            _ => return,
        };
        let subs = self.subscribers_of(source);
        if subs.iter().all(|sub| !sub.is_live()) {
            return;
        }
        let users = match self.presence_users(&feed).await {
            Ok(users) => users,
            Err(error) => {
                tracing::warn!(%error, "who is looking could not be told");
                return;
            }
        };
        for sub in subs {
            if !sub.is_live() {
                continue;
            }
            match super::watches::reader_guard(&self, &sub).await {
                Ok(Some(())) => {}
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error, "presence could not authorize its reader");
                    continue;
                }
            };
            let frame = orch_realtime_contract::frames::ServerFrame::Presence(
                orch_realtime_contract::frames::PresenceFrame {
                    id: sub.tag().id,
                    feed: feed.clone(),
                    users: users.clone(),
                },
            );
            if let Ok(text) = frame.encode() {
                sub.connection().outbox.push_frame(sub.tag(), text);
            }
        }
    }

    /// Says who reads `feed` on this process's connections, for the others.
    async fn publish_presence(&self, feed: &Feed) {
        let room = orch_db::sequences::MAX_PAYLOAD_BYTES.saturating_sub(BYTES_BESIDES_READERS)
            / BYTES_PER_READER;
        let readers: Vec<serde_json::Value> = self
            .presence
            .local_of(feed)
            .into_iter()
            .take(room)
            .map(|(person, since)| serde_json::json!([person, since.unix_millis()]))
            .collect();
        let text = serde_json::json!({ "p": self.process, "f": feed.to_string(), "r": readers })
            .to_string();
        let sent = async {
            let mut conn = self.db.acquire().await?;
            orch_db::sequences::notify_presence(&mut conn, &text).await
        }
        .await;
        if let Err(error) = sent {
            tracing::warn!(%error, "who is looking could not be told to the other processes");
        }
    }

    /// What another process said of who reads on its connections.
    pub(crate) fn heard_presence(self: &Arc<Self>, payload: &str) {
        let Ok(said) = serde_json::from_str::<serde_json::Value>(payload) else {
            return;
        };
        let (Some(process), Some(feed), Some(readers)) = (
            said.get("p").and_then(serde_json::Value::as_str),
            said.get("f")
                .and_then(serde_json::Value::as_str)
                .and_then(|feed| feed.parse::<Feed>().ok()),
            said.get("r").and_then(serde_json::Value::as_array),
        ) else {
            return;
        };
        if process == self.process {
            return;
        }
        let readers: Vec<(Id, Time)> = readers
            .iter()
            .filter_map(|reader| {
                let reader = reader.as_array()?;
                let person: Id = reader.first()?.as_str()?.parse().ok()?;
                let since = DateTime::from_timestamp_millis(reader.get(1)?.as_i64()?)?;
                Some((person, Time::new(since)))
            })
            .collect();
        if self
            .presence
            .heard(&feed, process, readers, self.clock.time())
        {
            self.presence_changed(&feed);
        }
    }

    /// Forgets the processes that stopped saying who reads on their
    /// connections, and says again who reads on this one's.
    async fn presence_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                () = tokio::time::sleep(Duration::from_secs(u64::try_from(REFRESH_SECS).unwrap_or(10))) => {}
                () = self.stop.cancelled() => return,
            }
            for feed in self.presence.forget_the_silent(self.clock.time()) {
                self.presence_changed(&feed);
            }
            for (feed, _) in self.presence.local() {
                self.publish_presence(&feed).await;
            }
        }
    }

    /// Starts the task that keeps the other processes' lists believed.
    pub(crate) fn start_presence(self: &Arc<Self>) {
        self.tasks.spawn(Arc::clone(self).presence_loop());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn at(second: i64) -> Time {
        format!("2026-10-08T17:{:02}:{:02}.000Z", second / 60, second % 60)
            .parse()
            .unwrap()
    }

    #[test]
    fn a_person_is_listed_from_their_first_connection_until_their_last_one_leaves() {
        let presence = Presence::new();
        let (feed, ada) = (Feed::Thread(new_id()), new_id());
        assert!(presence.set_visible(&feed, ada, "c1", true, at(1)));
        assert!(!presence.set_visible(&feed, ada, "c2", true, at(5)));
        assert_eq!(presence.list(&feed), vec![(ada, at(1))]);
        assert!(!presence.set_visible(&feed, ada, "c1", false, at(6)));
        assert!(presence.set_visible(&feed, ada, "c2", false, at(7)));
        assert!(presence.list(&feed).is_empty());
    }

    #[test]
    fn a_connection_that_ends_while_showing_a_feed_takes_its_reader_with_it() {
        let presence = Presence::new();
        let (one, two) = (Feed::Thread(new_id()), Feed::Thread(new_id()));
        let ada = new_id();
        presence.set_visible(&one, ada, "c1", true, at(1));
        presence.set_visible(&two, ada, "c1", true, at(1));
        let mut changed = presence.connection_gone("c1");
        changed.sort();
        let mut both = vec![one.clone(), two.clone()];
        both.sort();
        assert_eq!(changed, both);
        assert!(presence.list(&one).is_empty());
    }

    #[test]
    fn what_other_processes_say_is_listed_with_what_this_one_knows_and_the_longest_reader_comes_first()
     {
        let presence = Presence::new();
        let feed = Feed::Thread(new_id());
        let (ada, bo) = (new_id(), new_id());
        presence.set_visible(&feed, ada, "c1", true, at(10));
        assert!(presence.heard(&feed, "p2", vec![(bo, at(3)), (ada, at(8))], at(11)));
        // Ada began on the other process first.
        assert_eq!(presence.list(&feed), vec![(bo, at(3)), (ada, at(8))]);
        assert!(presence.heard(&feed, "p2", vec![], at(12)));
        assert_eq!(presence.list(&feed), vec![(ada, at(10))]);
        assert_eq!(presence.local_of(&feed), vec![(ada, at(10))]);
    }

    #[test]
    fn a_process_that_stops_saying_it_is_there_is_forgotten_after_three_refreshes() {
        let presence = Presence::new();
        let feed = Feed::Thread(new_id());
        let bo = new_id();
        presence.heard(&feed, "p2", vec![(bo, at(1))], at(1));
        assert!(presence.forget_the_silent(at(30)).is_empty());
        assert_eq!(presence.list(&feed).len(), 1);
        assert_eq!(presence.forget_the_silent(at(31)), vec![feed.clone()]);
        assert!(presence.list(&feed).is_empty());
        // Said again, it is believed again for as long.
        presence.heard(&feed, "p2", vec![(bo, at(1))], at(40));
        assert!(presence.forget_the_silent(at(60)).is_empty());
    }

    #[test]
    fn what_this_process_knows_for_the_others_is_only_what_its_own_connections_show() {
        let presence = Presence::new();
        let feed = Feed::Thread(new_id());
        let (ada, bo) = (new_id(), new_id());
        presence.set_visible(&feed, ada, "c1", true, at(1));
        presence.heard(&feed, "p2", vec![(bo, at(2))], at(2));
        assert_eq!(presence.local(), vec![(feed, vec![(ada, at(1))])]);
    }

    #[test]
    fn a_list_is_sent_at_once_the_first_time_and_then_at_most_once_a_second() {
        let presence = Presence::new();
        let feed = Feed::Thread(new_id());
        assert_eq!(presence.changed(&feed, at(10)), Verdict::Now);
        let first_wait = presence.changed(&feed, at(10));
        assert!(
            matches!(first_wait, Verdict::After(ms) if ms > 0 && ms <= 1_000),
            "{first_wait:?}"
        );
        assert_eq!(presence.changed(&feed, at(10)), Verdict::Waiting);
        assert!(!presence.poll(&feed, at(10)));
        assert!(presence.poll(&feed, at(11)));
        assert!(!presence.poll(&feed, at(12)), "it was sent once");
    }
}
