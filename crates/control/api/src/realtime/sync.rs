//! Sequenced fan-out, gap repair and database notification sweep.
//!
//! The database says that a feed moved by an announcement that is queued in
//! the transaction that moved it, so that it arrives when, and only when, the
//! transaction committed. Every API process listens; a process that has
//! subscribers to the feed reads the rows stamped after the last number it
//! read for each of them, using a per-credential snapshot for thread projections, and
//! sends each subscriber one `sync` frame whose `prev` is the `seq` of the
//! frame before it. Announcements are a wake-up and not the data: one that is
//! lost is made up for by the next, and by a sweep every few seconds that
//! compares every subscribed feed with the number the database now holds.
//!
//! One task at a time reads for the subscribers of a source. It reads at once
//! for the first announcement and then at most once in the gap (a fifth of a
//! second, or a second while text grows in a thread), with everything that was
//! announced meanwhile, so that a text that grows and is stored with each
//! stretch is not read for every one of them.
//!
//! A reader that says where it is gets what it missed: `head` answers a
//! subscription, and the first read for it takes it from there to the number
//! the feed holds. A reader that is too far behind is told to load again, and
//! one whose frames were dropped is told to read again; neither is a reason
//! to close a connection.

use crate::realtime::deltas::Held;
use crate::realtime::subscriptions::{SubState, Subscription};
use crate::realtime::{
    Entry, Fail, Inner, STREAMING_WINDOW, Source, chats, documents, environments, threads, topics,
    tree, user,
};
use orch_db::queries::documents::DOC_CHANNEL;
use orch_db::queries::jobs::ENVIRONMENT_CHANNEL;
use orch_db::sequences::{self, Notice};
use orch_realtime_contract::feeds::{EventData, Feed, SyncData, UserEvent};
use orch_realtime_contract::frames::{
    EventFrame, FeedClosedReason, ResyncFrame, ServerFrame, SyncFrame,
};
use orch_realtime_contract::sync::FeedHead;
use orch_types::{Id, Seq};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::Instant;

/// How long the listener waits before it listens again after it failed to:
/// the database is down or restarting, and the sweep that follows its return
/// makes up for what it did not hear.
const LISTEN_RETRY: Duration = Duration::from_secs(1);

/// Where a feed stands when somebody subscribes: its sequence, and how many
/// rows are stamped above the number they hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Head {
    pub(crate) feed: FeedHead,
    pub(crate) rows_after: i64,
}

/// Reusable source rows for feeds whose personalized projection does not change their cut.
/// Thread frames deliberately do not enter this cache.
#[derive(Debug, Default)]
pub(crate) struct Cache {
    pub(crate) documents: HashMap<(Seq, Seq), Arc<documents::Shared>>,
}

/// What a read found for one subscriber.
#[derive(Debug)]
pub(crate) enum Rows {
    /// A numeric sequence belongs to a different execution projection and cannot be merged.
    ProjectionChanged,
    /// Nothing the subscriber may see lies in the range.
    Nothing,
    /// More changed than a frame holds: the client reads again.
    TooMany,
    /// What the feed is about is gone.
    Gone,
    /// Rows to send.
    Frame(Box<Frame>),
}

/// The rows of one `sync` frame.
#[derive(Debug)]
pub(crate) struct Frame {
    pub(crate) data: SyncData,
    /// The highest stamp among them.
    pub(crate) highest: Seq,
    /// What they hold of the fields that grow.
    pub(crate) held: Vec<Held>,
}

/// Where `feed` stands for `user`, and how many rows lie above `since`. None
/// where what it is about is not there.
pub(crate) async fn head(
    inner: &Inner,
    feed: &Feed,
    user: Id,
    since: Option<Seq>,
) -> Result<Option<Head>, Fail> {
    match feed {
        Feed::Thread(id) => threads::head(inner, *id, since).await,
        Feed::Topic(id) => topics::head(inner, *id, since).await,
        Feed::Org(id) => tree::head(inner, *id, since).await,
        Feed::User => user::head(inner, user, since).await,
        Feed::Chat(id) => chats::head(inner, *id, since).await,
        Feed::Doc(id) => documents::head(inner, *id, since).await,
        Feed::Env(id) => environments::head(inner, *id, since).await,
        _ => Ok(None),
    }
}

/// The authorization epoch now; connections are woken when it moved.
pub(crate) async fn read_epoch(inner: &Inner) -> Result<i64, Fail> {
    let mut conn = inner.db.acquire().await?;
    let epoch = orch_db::queries::access::epoch(&mut conn).await?;
    let moved = inner.epoch.send_if_modified(|held| {
        let moved = *held != epoch;
        *held = epoch;
        moved
    });
    if moved {
        inner.ephemeral.document_tree_changed(None);
        inner.ephemeral.raw_changed(None);
    }
    Ok(epoch)
}

// ---------------------------------------------------------------- reading for a subscriber

impl Inner {
    /// Reads the rows in `(after, upto]` for one subscriber.
    async fn read_rows(
        self: &Arc<Self>,
        sub: &Arc<Subscription>,
        after: Seq,
        upto: Seq,
        cache: &mut Cache,
        thread_scope: threads::ReadScope,
    ) -> Result<Rows, Fail> {
        let person = sub.connection().user;
        super::subscriptions::viewing(sub.connection(), async {
            match sub.feed() {
                Feed::Thread(id) => {
                    threads::read_scoped(self, person, *id, after, upto, cache, thread_scope).await
                }
                Feed::Topic(id) => topics::read(self, person, *id, after, upto).await,
                Feed::Org(id) => tree::read(self, person, *id, after, upto).await,
                Feed::User => user::read(self, person, after, upto).await,
                Feed::Chat(id) => chats::read(self, person, *id, after, upto).await,
                Feed::Doc(id) => documents::read(self, person, *id, after, upto, cache).await,
                Feed::Env(id) => environments::read(self, person, *id, after, upto).await,
                _ => Ok(Rows::Nothing),
            }
        })
        .await
    }

    /// Sends a subscriber what is owed it up to `target`: one frame, with the
    /// rows stamped after the last it was sent. The subscription's lock is
    /// held by the caller.
    pub(crate) async fn deliver(
        self: &Arc<Self>,
        sub: &Arc<Subscription>,
        st: &mut SubState,
        target: Seq,
        cache: &mut Cache,
    ) -> Result<(), Fail> {
        let Some((after, upto)) = st.cursor.owed(target) else {
            return Ok(());
        };
        // The current credential and shared authority gate stay pinned through projection
        // and serialization; authority writers cannot commit halfway through this frame.
        let Some(_identity) = super::watches::delivery_guard(self, sub.connection()).await? else {
            return Ok(());
        };
        let rows = self
            .read_rows(sub, after, upto, cache, sub.thread_scope(st))
            .await?;
        // A personalized snapshot may have advanced beyond the wake-up that started this pass.
        // Its complete frame and cursor use that actual cut together, including a topic's jobs
        // and announcements; skipping an older wake-up must not acknowledge its undelivered rows.
        let upto = match &rows {
            Rows::Frame(frame)
                if matches!(&frame.data, SyncData::Thread(_) | SyncData::Topic(_)) =>
            {
                upto.max(frame.highest)
            }
            _ => upto,
        };
        // Projection reads can await the database. Recheck at the delivery boundary too,
        // so a grant ending or a committed permission change during that read sends no data.
        let epoch = read_epoch(self).await?;
        if !self.recheck(sub, st, epoch).await? {
            return Ok(());
        }
        let connection = sub.connection();
        match rows {
            Rows::ProjectionChanged => {
                self.invalidate_thread_projection(sub, st);
                return Ok(());
            }
            Rows::Gone => {
                self.close_sub(sub, st, FeedClosedReason::Deleted).await;
                return Ok(());
            }
            Rows::TooMany => {
                // Everything up to here is taken as sent: the client reads it
                // over HTTP, which answers whether it can be read from where
                // the client stands or has to be loaded again.
                let _ = st.cursor.read(upto, Some(upto));
                self.ask_resync(sub, false);
            }
            Rows::Nothing => {
                let _ = st.cursor.read(upto, None);
            }
            Rows::Frame(frame) => {
                if let Some((prev, seq)) = st.cursor.read(upto, Some(frame.highest)) {
                    let sync =
                        SyncFrame::new(sub.tag().id, sub.feed().clone(), prev, seq, &frame.data)
                            .map_err(Fail::other)?;
                    let text = ServerFrame::Sync(sync).encode().map_err(Fail::other)?;
                    // One frame must fit the connection's whole queue budget. A large row is
                    // read over HTTP instead of defeating the socket's memory bound by itself.
                    if i64::try_from(text.len()).unwrap_or(i64::MAX) > self.tuning.buffer_max_bytes
                    {
                        self.ask_resync(sub, false);
                    } else {
                        if let SyncData::Thread(data) = &frame.data {
                            st.thread_projection = data.projection.clone();
                        }
                        connection.outbox.push_sync(sub.tag(), text, &frame.held);
                    }
                }
            }
        }
        st.read = upto;
        sub.set_read(upto);
        Ok(())
    }

    fn ask_resync(&self, sub: &Subscription, reload: bool) {
        let frame = ServerFrame::Resync(ResyncFrame {
            id: sub.tag().id,
            feed: sub.feed().clone(),
            reload: reload.then_some(true),
        });
        if let Ok(text) = frame.encode() {
            sub.connection().outbox.push_frame(sub.tag(), text);
        }
    }

    /// Purge old queued rows/deltas before announcing a new read scope; no more old work is sent.
    pub(crate) fn invalidate_thread_projection(&self, sub: &Subscription, state: &mut SubState) {
        if state.thread_reset {
            return;
        }
        state.thread_reset = true;
        sub.set_live(false);
        sub.connection().outbox.purge(sub.tag().generation);
        self.ask_resync(sub, true);
    }

    /// One pass for the subscribers of `source`: the access of each is asked
    /// again if the epoch moved, and each is sent what it is owed up to
    /// `target`. A subscriber that could not be served is left where it was,
    /// to be served by the next pass.
    async fn pass(self: &Arc<Self>, source: Source, target: Seq) -> Result<(), Fail> {
        let subs = self.subscribers_of(source);
        if subs.is_empty() {
            return Ok(());
        }
        let epoch = read_epoch(self).await?;
        let mut cache = Cache::default();
        for sub in subs {
            let mut st = sub.state.lock().await;
            if !st.open {
                continue;
            }
            match self.recheck(&sub, &mut st, epoch).await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    tracing::warn!(%error, ?source, "the access of a subscription could not be asked again");
                    continue;
                }
            }
            if let Err(error) = self.deliver(&sub, &mut st, target, &mut cache).await {
                tracing::warn!(%error, ?source, "rows could not be read for a subscription; the next pass reads them");
            }
        }
        Ok(())
    }

    /// The work of one source: passes, spaced by the gap, for as long as
    /// something is announced.
    async fn work(self: Arc<Self>, source: Source) {
        loop {
            // The first read after a quiet time is at once; the next waits for
            // the gap to be over, whichever task made the last. While text grows
            // in a thread the gap is the second a row may be behind.
            let wait = {
                let registry = self.registry();
                registry.sources.get(&source).and_then(|entry| {
                    let growing = entry
                        .streaming
                        .is_some_and(|at| at.elapsed() < STREAMING_WINDOW);
                    let gap = if growing {
                        self.tuning.fanout_gap.max(self.tuning.stream_gap)
                    } else {
                        self.tuning.fanout_gap
                    };
                    entry.last.map(|at| at + gap)
                })
            };
            if let Some(until) = wait {
                tokio::select! {
                    () = tokio::time::sleep_until(until) => {}
                    () = self.stop.cancelled() => {}
                }
            }
            let target = {
                let mut registry = self.registry();
                let Some(entry) = registry.sources.get_mut(&source) else {
                    return;
                };
                if entry.subs.is_empty() {
                    registry.sources.remove(&source);
                    return;
                }
                entry.dirty = false;
                entry.last = Some(Instant::now());
                entry.notified
            };
            if let Err(error) = self.pass(source, target).await {
                tracing::warn!(%error, ?source, "a pass over the subscribers of a feed failed");
            }
            let mut registry = self.registry();
            match registry.sources.get_mut(&source) {
                Some(entry) if entry.dirty && !self.stop.is_cancelled() => {}
                Some(entry) => {
                    entry.running = false;
                    if entry.subs.is_empty() {
                        registry.sources.remove(&source);
                    }
                    return;
                }
                None => return,
            }
        }
    }

    fn behind(entry: &Entry) -> bool {
        entry.subs.iter().any(|sub| sub.read() < entry.notified)
    }

    /// The feed of `source` moved up to `seq` (or was found there): its
    /// subscribers are read for, unless they all have read that far.
    pub(crate) fn announce(self: &Arc<Self>, source: Source, seq: Seq) {
        let mut registry = self.registry();
        let Some(entry) = registry.sources.get_mut(&source) else {
            return;
        };
        entry.notified = entry.notified.max(seq);
        self.kick(entry, source);
    }

    /// Looks again whether anybody has not read as far as was announced, for a
    /// source whose subscriber was just set up.
    pub(crate) fn poke(self: &Arc<Self>, source: Source) {
        let mut registry = self.registry();
        let Some(entry) = registry.sources.get_mut(&source) else {
            return;
        };
        self.kick(entry, source);
    }

    fn kick(self: &Arc<Self>, entry: &mut Entry, source: Source) {
        if !Self::behind(entry) {
            return;
        }
        entry.dirty = true;
        if entry.running {
            return;
        }
        entry.running = true;
        let inner = Arc::clone(self);
        self.tasks.spawn(inner.work(source));
    }
}

// ---------------------------------------------------------------- listening and sweeping

/// Starts what the server does beside answering a socket: it listens to the
/// database, compares every subscribed feed with it every few seconds, and
/// watches the authorization epoch.
pub(crate) fn start(inner: &Arc<Inner>) {
    inner.tasks.spawn(Arc::clone(inner).listen_loop());
    inner.tasks.spawn(Arc::clone(inner).sweep_loop());
    inner.tasks.spawn(Arc::clone(inner).epoch_loop());
    inner.start_presence();
}

impl Inner {
    async fn listen_loop(self: Arc<Self>) {
        let channels = [
            sequences::THREAD_CHANNEL,
            sequences::TOPIC_CHANNEL,
            sequences::TREE_CHANNEL,
            sequences::USER_CHANNEL,
            sequences::AUTHZ_CHANNEL,
            sequences::IDENTITY_CHANNEL,
            sequences::PRESENCE_CHANNEL,
            orch_db::queries::chats::CHAT_CHANNEL,
            orch_db::queries::chats::TYPING_CHANNEL,
            DOC_CHANNEL,
            ENVIRONMENT_CHANNEL,
            super::machine_views::CHANNEL,
        ];
        while !self.stop.is_cancelled() {
            let mut notices = match sequences::listen(&self.db, &channels).await {
                Ok(notices) => notices,
                Err(error) => {
                    tracing::warn!(%error, "the realtime server cannot listen to the database");
                    self.rest(LISTEN_RETRY).await;
                    continue;
                }
            };
            // Whatever was announced before this was listening is found by
            // the sweep.
            self.listening.send_replace(true);
            self.sweep_soon();
            self.tell_meta_changed(None);
            loop {
                let heard = tokio::select! {
                    () = self.stop.cancelled() => return,
                    heard = notices.next() => heard,
                };
                match heard {
                    Ok(Notice::Heard { channel, payload }) => self.heard(&channel, &payload),
                    Ok(Notice::Reconnected) => {
                        self.sweep_soon();
                        self.tell_meta_changed(None);
                        self.ephemeral.document_tree_changed(None);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "the realtime server lost its listening connection");
                        self.listening.send_replace(false);
                        self.rest(LISTEN_RETRY).await;
                        break;
                    }
                }
            }
        }
    }

    async fn rest(&self, how_long: Duration) {
        tokio::select! {
            () = tokio::time::sleep(how_long) => {}
            () = self.stop.cancelled() => {}
        }
    }

    /// What an announcement says.
    fn heard(self: &Arc<Self>, channel: &str, payload: &str) {
        match channel {
            sequences::THREAD_CHANNEL => {
                if let Some((thread, seq)) = sequences::parse_payload(payload) {
                    // Raw projection repair is driven by actual commits, never the periodic durable-feed sweep.
                    self.ephemeral.raw_changed(Some(thread));
                    self.announce(Source::Thread(thread), seq);
                }
            }
            sequences::TOPIC_CHANNEL => {
                if let Some((topic, seq)) = sequences::parse_payload(payload) {
                    self.announce(Source::Topic(topic), seq);
                }
            }
            sequences::TREE_CHANNEL => {
                if let Some((org, seq)) = sequences::parse_payload(payload) {
                    self.announce(Source::Tree(org), seq);
                    self.ephemeral.document_tree_changed(Some(org));
                    // A topic's feed moves with a node's stamp, and is not
                    // announced by it.
                    self.look_at_topics();
                }
            }
            sequences::USER_CHANNEL => match sequences::parse_user_payload(payload) {
                Some(sequences::UserNotice::Inbox { user, seq }) => {
                    self.announce(Source::Inbox(user), seq);
                }
                Some(sequences::UserNotice::MetaChanged { request_id }) => {
                    self.tell_meta_changed(request_id)
                }
                Some(sequences::UserNotice::Event { users, event }) => {
                    self.tell_connections(&users, &event);
                }
                None => {}
            },
            sequences::AUTHZ_CHANNEL => {
                let inner = Arc::clone(self);
                self.tasks.spawn(async move {
                    if let Err(error) = read_epoch(&inner).await {
                        tracing::warn!(%error, "the authorization epoch could not be read");
                    }
                });
            }
            sequences::IDENTITY_CHANNEL => {
                if let Ok(user) = payload.parse::<Id>() {
                    let inner = Arc::clone(self);
                    self.tasks.spawn(async move {
                        let connections: Vec<_> = inner.registry().connections.values()
                            .filter(|connection| connection.user == user).cloned().collect();
                        for connection in connections {
                            let result = async {
                                if !super::watches::credential_current(&inner, &connection).await? {
                                    return Ok::<(), Fail>(());
                                }
                                let epoch = read_epoch(&inner).await?;
                                for sub in connection.subscriptions() {
                                    let mut state = sub.state.lock().await;
                                    // Shared permissions did not change, but this session's
                                    // second-step hold or credential may have done so.
                                    state.epoch = -1;
                                    inner.recheck(&sub, &mut state, epoch).await?;
                                }
                                Ok(())
                            }.await;
                            if let Err(error) = result {
                                tracing::warn!(%error, "a changed personal identity could not be rechecked");
                                connection.close(orch_realtime_contract::version::CloseCode::Unauthenticated,
                                    "the changed identity could not be checked");
                            }
                        }
                    });
                }
            }
            sequences::PRESENCE_CHANNEL => {
                self.ephemeral.heard(payload);
                self.heard_presence(payload);
            }
            orch_db::queries::chats::CHAT_CHANNEL => {
                if let Some((chat, seq)) = sequences::parse_payload(payload) {
                    self.announce(Source::Chat(chat), seq);
                }
            }
            orch_db::queries::chats::TYPING_CHANNEL => {
                if let Some((chat, user)) = orch_db::queries::chats::parse_typing(payload) {
                    let inner = Arc::clone(self);
                    self.tasks.spawn(async move {
                        chats::deliver_typing(&inner, chat, user).await;
                    });
                }
            }
            DOC_CHANNEL => documents::heard(self, payload),
            ENVIRONMENT_CHANNEL => {
                if let Some((environment, seq)) = sequences::parse_payload(payload) {
                    self.announce(Source::Environment(environment), seq);
                    self.ephemeral.environment_changed(environment);
                }
            }
            super::machine_views::CHANNEL => {
                if let Ok(environment) = payload.parse::<Id>() {
                    self.ephemeral.environment_changed(environment);
                } else if let Ok(feed @ (Feed::Job(_) | Feed::Fork(_))) = payload.parse::<Feed>() {
                    self.ephemeral.object_changed(feed);
                }
            }
            _ => {}
        }
    }

    /// Hands news that is no row to the connections of some people that
    /// follow their own feed.
    pub(super) fn tell_connections(self: &Arc<Self>, users: &[Id], event: &serde_json::Value) {
        let Ok(event) = serde_json::from_value::<UserEvent>(event.clone()) else {
            return;
        };
        for person in users {
            for sub in self.subscribers_of(Source::Inbox(*person)) {
                let inner = Arc::clone(self);
                let event = event.clone();
                self.tasks.spawn(async move {
                    if let Err(error) = inner.personal_event(&sub, event).await {
                        tracing::warn!(%error, "a personal event could not be authorized for its reader");
                    }
                });
            }
        }
    }

    /// One committed hint fans out only to existing personal-feed subscriptions. The normal
    /// personal-event path retakes session identity and authority before publishing it.
    pub(super) fn tell_meta_changed(self: &Arc<Self>, request_id: Option<String>) {
        let subscriptions: Vec<_> = self
            .registry()
            .sources
            .iter()
            .filter(|(source, _)| matches!(source, Source::Inbox(_)))
            .flat_map(|(_, entry)| entry.subs.clone())
            .collect();
        for sub in subscriptions {
            let inner = Arc::clone(self);
            let request_id = request_id.clone();
            self.tasks.spawn(async move {
                if let Err(error) = inner
                    .personal_event(&sub, UserEvent::MetaChanged { request_id })
                    .await
                {
                    tracing::warn!(%error, "a metadata hint could not reach its current reader");
                }
            });
        }
    }

    /// News is not a sequenced Inbox row: a pending row repair does not end
    /// this listener. Current identity, authority and projection still apply.
    pub(crate) async fn personal_event(
        self: &Arc<Self>,
        sub: &Arc<Subscription>,
        event: UserEvent,
    ) -> Result<(), Fail> {
        let Some(_identity) = super::watches::delivery_guard(self, sub.connection()).await? else {
            return Ok(());
        };
        let mut st = sub.state.lock().await;
        loop {
            if self.stop.is_cancelled() {
                return Ok(());
            }
            let epoch = read_epoch(self).await?;
            if !self.recheck_personal(sub, &mut st, epoch).await? {
                return Ok(());
            }
            let cut = (
                sub.decided(),
                sub.connection().session(),
                sub.connection().as_admin(),
            );
            // The state lock orders projection after the reset, even when the epoch
            // watcher arrived first. Keep only this unprojected notice across a cut change.
            let projected =
                super::personal_changes::project(self, sub.connection(), event.clone()).await?;
            #[cfg(test)]
            super::epoch_handoff_tests::after_projection(super::epoch_handoff_tests::Cut::Personal)
                .await;
            let latest = read_epoch(self).await?;
            let expired = sub.decided() != cut.0;
            if !self.recheck_personal(sub, &mut st, latest).await? {
                return Ok(());
            }
            if latest != epoch
                || expired
                || sub.connection().session() != cut.1
                || sub.connection().as_admin() != cut.2
            {
                // Retry only after an actual authority/identity/deadline transition;
                // no timer, idle poll, stored DTO or additional notification task.
                continue;
            }
            let frame = EventFrame::new(sub.tag().id, Feed::User, &EventData::User(projected))
                .map_err(Fail::other)?;
            let text = ServerFrame::Event(frame).encode().map_err(Fail::other)?;
            sub.connection()
                .outbox
                .push_event(sub.tag(), &Feed::User, text);
            return Ok(());
        }
    }

    /// Compares every subscribed feed with the database, soon: on the task
    /// that is already doing it, if there is one.
    fn sweep_soon(self: &Arc<Self>) {
        if self.tuning.sweep_every.is_zero() {
            return;
        }
        let inner = Arc::clone(self);
        self.tasks.spawn(async move {
            if let Err(error) = inner.sweep_once().await {
                tracing::warn!(%error, "a sweep of the feeds failed");
            }
        });
    }

    /// The topics that are subscribed are compared with the database.
    fn look_at_topics(self: &Arc<Self>) {
        // One look at a time is enough: it reads what the database holds when
        // it reads, which includes what was announced while the last was
        // under way.
        let inner = Arc::clone(self);
        if self.looking.swap(true, Ordering::AcqRel) {
            return;
        }
        self.tasks.spawn(async move {
            tokio::time::sleep(inner.tuning.fanout_gap).await;
            inner.looking.store(false, Ordering::Release);
            let topics: Vec<Id> = inner
                .sources()
                .into_iter()
                .filter_map(|source| match source {
                    Source::Topic(id) => Some(id),
                    _ => None,
                })
                .collect();
            if topics.is_empty() {
                return;
            }
            let found = async {
                let mut conn = inner.db.acquire().await?;
                orch_db::queries::feeds::topic_seqs(&mut conn, &topics).await
            }
            .await;
            match found {
                Ok(found) => {
                    for (topic, seq) in found {
                        inner.announce(Source::Topic(topic), seq);
                    }
                }
                Err(error) => tracing::warn!(%error, "the topics could not be looked at"),
            }
        });
    }

    fn sources(&self) -> Vec<Source> {
        self.registry().sources.keys().copied().collect()
    }

    /// One comparison of every subscribed feed with what the database holds:
    /// a feed that moved without an announcement being heard is read for, and
    /// a subscription to something that was deleted ends.
    pub(crate) async fn sweep_once(self: &Arc<Self>) -> Result<(), Fail> {
        read_epoch(self).await?;
        let sources = self.sources();
        let mut threads = Vec::new();
        let mut topics = Vec::new();
        let mut trees = Vec::new();
        let mut inboxes = Vec::new();
        let mut chat_ids = Vec::new();
        let mut documents = Vec::new();
        let mut environment_ids = Vec::new();
        for source in &sources {
            match source {
                Source::Thread(id) => threads.push(*id),
                Source::Topic(id) => topics.push(*id),
                Source::Tree(id) => trees.push(*id),
                Source::Inbox(id) => inboxes.push(*id),
                Source::Chat(id) => chat_ids.push(*id),
                Source::Document(id) => documents.push(*id),
                Source::Environment(id) => environment_ids.push(*id),
            }
        }
        let mut conn = self.db.acquire().await?;
        for (id, seq) in orch_db::queries::feeds::thread_seqs(&mut conn, &threads).await? {
            self.announce(Source::Thread(id), seq);
        }
        for (id, seq) in orch_db::queries::feeds::topic_seqs(&mut conn, &topics).await? {
            self.announce(Source::Topic(id), seq);
        }
        for (id, seq) in orch_db::queries::feeds::tree_seqs(&mut conn, &trees).await? {
            self.announce(Source::Tree(id), seq);
        }
        for (id, seq) in orch_db::queries::feeds::inbox_seqs(&mut conn, &inboxes).await? {
            self.announce(Source::Inbox(id), seq);
        }
        let found = orch_db::queries::chats::seqs(&mut conn, &chat_ids).await?;
        for (id, seq) in &found {
            self.announce(Source::Chat(*id), *seq);
        }
        let deleted_chats: Vec<Id> = chat_ids
            .iter()
            .copied()
            .filter(|id| !found.iter().any(|(each, _)| each == id))
            .collect();
        for (id, seq) in orch_db::queries::documents::sequences(&mut conn, &documents).await? {
            self.announce(Source::Document(id), seq);
        }
        let gone_documents: HashSet<Id> = orch_db::queries::documents::gone(&mut conn, &documents)
            .await?
            .into_iter()
            .collect();
        for (id, seq) in
            orch_db::queries::feeds::environment_seqs(&mut conn, &environment_ids).await?
        {
            self.announce(Source::Environment(id), seq);
        }
        let gone_environments: HashSet<Id> =
            orch_db::queries::feeds::gone_environments(&mut conn, &environment_ids)
                .await?
                .into_iter()
                .collect();
        let mut nodes = threads;
        nodes.extend(&topics);
        nodes.extend(&trees);
        let gone: HashSet<Id> = orch_db::queries::feeds::gone_nodes(&mut conn, &nodes)
            .await?
            .into_iter()
            .collect();
        drop(conn);
        for id in deleted_chats {
            self.end_source(Source::Chat(id), FeedClosedReason::Deleted)
                .await;
        }
        if !gone.is_empty() || !gone_documents.is_empty() || !gone_environments.is_empty() {
            for source in sources {
                let (node, ended) = match source {
                    Source::Thread(id) | Source::Topic(id) | Source::Tree(id) => (id, &gone),
                    Source::Document(id) => (id, &gone_documents),
                    Source::Environment(id) => (id, &gone_environments),
                    Source::Inbox(_) | Source::Chat(_) => continue,
                };
                if ended.contains(&node) {
                    self.end_source(source, FeedClosedReason::Deleted).await;
                }
            }
        }
        Ok(())
    }

    /// Ends every subscription to a source, with `reason`.
    async fn end_source(self: &Arc<Self>, source: Source, reason: FeedClosedReason) {
        for sub in self.subscribers_of(source) {
            let mut st = sub.state.lock().await;
            self.close_sub(&sub, &mut st, reason).await;
        }
    }

    async fn sweep_loop(self: Arc<Self>) {
        // Explicitly disabling this repair source lets notification delivery be observed
        // without inferring its source from a wall-clock deadline.
        if self.tuning.sweep_every.is_zero() {
            self.stop.cancelled().await;
            return;
        }
        loop {
            self.rest(self.tuning.sweep_every).await;
            if self.stop.is_cancelled() {
                return;
            }
            if let Err(error) = self.sweep_once().await {
                tracing::warn!(%error, "a sweep of the feeds failed");
            }
        }
    }

    async fn epoch_loop(self: Arc<Self>) {
        if self.tuning.epoch_every.is_zero() {
            self.stop.cancelled().await;
            return;
        }
        loop {
            self.rest(self.tuning.epoch_every).await;
            if self.stop.is_cancelled() {
                return;
            }
            if let Err(error) = read_epoch(&self).await {
                tracing::warn!(%error, "the authorization epoch could not be read");
            }
        }
    }
}
