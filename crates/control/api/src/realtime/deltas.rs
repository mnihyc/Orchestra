//! Streaming delivery bounded in bytes and in time, with deltas merged for a reader that is behind and no connection closed for being slow.
//!
//! Everything a connection is to be sent waits in its [`Outbox`] until the
//! socket takes it, one frame at a time. Rows (`sync` frames and the answers
//! to a subscription) are never dropped and leave in the order they were
//! queued. Growing text is the one thing that is kept in a different shape
//! while it waits: a `delta` that finds another of the same item and field
//! waiting, and continuing exactly where that one ends, joins it, so that a
//! reader on a slow link is sent fewer, larger deltas and loses nothing; and
//! a delta that a row queued behind it makes pointless is dropped, since the
//! row carries the text. A connection whose socket takes frames as fast as
//! they come has nothing waiting, and its deltas leave one by one as they
//! arrived.
//!
//! A connection is closed for not keeping up only by the rule of the two
//! buffer keys: when more than `buffer_max_bytes` has been waiting for it for
//! `buffer_max_secs`, it is closed with 4408. Before that, as soon as the
//! limit is passed, what can be made up for is shed: the deltas and events
//! that wait are dropped and a `resync` is queued for each subscription they
//! belonged to, which costs that reader one request and loses nothing.
//!
//! [`Sink`] is what the thread engine hands growing text to: it queues the
//! delta for every subscription that follows the thread.

use crate::realtime::{Inner, Realtime, Source};
use orch_engine::live::{LiveDelta, LiveSink};
use orch_link_contract::events::DeltaField as LinkField;
use orch_realtime_contract::delta::{BehindRow, DeltaField, PendingDelta, wanted};
use orch_realtime_contract::feeds::Feed;
use orch_realtime_contract::frames::{DeltaFrame, ResyncFrame, ServerFrame, SubId};
use orch_realtime_contract::version::{Backlog, BacklogAction, CloseCode};
use orch_types::{Clock, Id};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::Notify;

/// What a frame of JSON adds to the text it carries, for a delta: the member
/// names, the feed's name, an item's identifier and the numbers. A frame is
/// counted as its text plus this, so that many tiny deltas weigh what they
/// will weigh on the wire.
const DELTA_OVERHEAD_BYTES: i64 = 192;

/// A subscription as the queue knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tag {
    /// The number by which the client names it; what every frame about it
    /// says.
    pub id: SubId,
    /// A number that no other subscription of the process has had, because a
    /// client may name a new subscription as an ended one was named: this is
    /// what tells their frames apart when a queue is cleared of one's.
    pub generation: u64,
}

/// What a row that is queued holds of one growing field of one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    /// The item.
    pub item: Id,
    /// The field.
    pub field: DeltaField,
    /// How much of it the row holds, in the field's unit.
    pub len: i64,
}

/// A stretch of text that was added to a field of an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The field.
    pub field: DeltaField,
    /// Where it begins.
    pub offset: i64,
    /// How long it is, in the field's unit.
    pub len: i64,
    /// What it reads.
    pub data: String,
}

/// What the writer of a connection takes next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// A text frame to write.
    Frame(String),
    /// The connection is to be closed with this code, and this reason.
    Close(u16, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    generation: u64,
    item: Id,
    field: DeltaField,
}

#[derive(Debug)]
enum What {
    /// A frame that is never dropped.
    Row(String),
    /// A pending reset must survive replacement by another authority reset.
    Resync(String),
    /// A frame that is dropped when the queue is shed.
    Event {
        id: SubId,
        feed: Feed,
        text: String,
        resource: bool,
    },
    /// Growing text, which may take in the text that grows after it.
    Delta {
        id: SubId,
        feed: Feed,
        item: Id,
        pending: PendingDelta,
    },
    /// What was here and no longer is.
    Gone,
}

#[derive(Debug)]
struct Entry {
    generation: u64,
    size: i64,
    what: What,
}

#[derive(Debug)]
struct Queue {
    entries: VecDeque<Entry>,
    /// The number of the entry at the front: entries are numbered one after
    /// the other, and only the front is ever removed, so that an entry's
    /// place is its number less this.
    front: u64,
    /// The entries that can still take in text, by what they are of.
    open: HashMap<Key, u64>,
    /// What leaves before the rest: answers to a ping.
    urgent: VecDeque<String>,
    /// How many of the entries are deltas or events, which a shed drops.
    droppable: usize,
    /// Bytes of what waits in the entries.
    bytes: i64,
    /// Bytes of the frame the socket is taking now.
    in_flight: i64,
    backlog: Backlog,
    closing: Option<(u16, String)>,
}

impl Queue {
    fn next_number(&self) -> u64 {
        self.front + self.entries.len() as u64
    }

    fn entry_mut(&mut self, number: u64) -> Option<&mut Entry> {
        let place = usize::try_from(number.checked_sub(self.front)?).ok()?;
        self.entries.get_mut(place)
    }

    fn waiting(&self) -> i64 {
        self.bytes.saturating_add(self.in_flight)
    }

    fn push(&mut self, generation: u64, size: i64, what: What) -> u64 {
        let number = self.next_number();
        if matches!(what, What::Event { .. } | What::Delta { .. }) {
            self.droppable += 1;
        }
        self.bytes = self.bytes.saturating_add(size);
        self.entries.push_back(Entry {
            generation,
            size,
            what,
        });
        number
    }

    /// Takes an entry out of the queue where it stands, leaving a mark in its
    /// place.
    fn remove(&mut self, number: u64) {
        if let Some(entry) = self.entry_mut(number) {
            let size = std::mem::replace(&mut entry.size, 0);
            let was = std::mem::replace(&mut entry.what, What::Gone);
            self.bytes = self.bytes.saturating_sub(size);
            if matches!(was, What::Event { .. } | What::Delta { .. }) {
                self.droppable = self.droppable.saturating_sub(1);
            }
        }
    }

    /// Stops the delta at `number` from taking in more.
    fn seal(&mut self, key: &Key) {
        self.open.remove(key);
    }
}

/// What waits for one connection, and what it is to be sent next.
#[derive(Debug)]
pub struct Outbox {
    queue: Mutex<Queue>,
    wake: Notify,
    clock: Arc<dyn Clock>,
}

/// Queue classes removed by an authority reset, without inspecting frame JSON.
#[derive(Default)]
pub(crate) struct Purged {
    pub(crate) resource: bool,
    pub(crate) resync: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The frame a delta waiting in the queue is written as.
fn delta_text(id: SubId, feed: &Feed, item: Id, pending: &PendingDelta) -> Option<String> {
    ServerFrame::Delta(DeltaFrame {
        id,
        feed: feed.clone(),
        item_id: item,
        field: pending.field(),
        offset: pending.offset(),
        len: pending.len(),
        data: pending.data().to_owned(),
    })
    .encode()
    .ok()
}

fn bytes_of(text: &str) -> i64 {
    i64::try_from(text.len()).unwrap_or(i64::MAX)
}

impl Outbox {
    /// An empty outbox for a connection, which is closed when more than
    /// `max_bytes` has waited for `max_millis` milliseconds.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, max_bytes: i64, max_millis: i64) -> Self {
        Self {
            queue: Mutex::new(Queue {
                entries: VecDeque::new(),
                front: 0,
                open: HashMap::new(),
                urgent: VecDeque::new(),
                droppable: 0,
                bytes: 0,
                in_flight: 0,
                backlog: Backlog::new(max_bytes, max_millis),
                closing: None,
            }),
            wake: Notify::new(),
            clock,
        }
    }

    /// Queues a frame that is never dropped: an answer to a subscription, the
    /// end of one, a presence list.
    pub fn push_frame(&self, tag: Tag, text: String) {
        let mut queue = lock(&self.queue);
        if queue.closing.is_some() {
            return;
        }
        let size = bytes_of(&text);
        queue.push(tag.generation, size, What::Row(text));
        self.after_push(&mut queue);
    }

    pub(crate) fn push_resync(&self, tag: Tag, text: String) {
        let mut queue = lock(&self.queue);
        if queue.closing.is_some() {
            return;
        }
        let size = bytes_of(&text);
        queue.push(tag.generation, size, What::Resync(text));
        self.after_push(&mut queue);
    }

    /// Queues a `sync` frame, with what its rows hold of the fields that
    /// grow. A delta of one of those items that waits ahead of it and says
    /// no more than the row is dropped; one that says a little more is sealed,
    /// so that it leaves first and what grows after it is a delta of its own.
    pub fn push_sync(&self, tag: Tag, text: String, held: &[Held]) {
        let mut queue = lock(&self.queue);
        if queue.closing.is_some() {
            return;
        }
        for each in held {
            let key = Key {
                generation: tag.generation,
                item: each.item,
                field: each.field,
            };
            let Some(number) = queue.open.get(&key).copied() else {
                continue;
            };
            let verdict = match queue.entry_mut(number).map(|entry| &entry.what) {
                Some(What::Delta { pending, .. }) => Some(pending.behind_row(each.len)),
                _ => None,
            };
            match verdict {
                Some(BehindRow::Drop) => {
                    queue.remove(number);
                    queue.seal(&key);
                }
                Some(BehindRow::SendFirst) => queue.seal(&key),
                Some(BehindRow::Keep) | None => {}
            }
        }
        let size = bytes_of(&text);
        queue.push(tag.generation, size, What::Row(text));
        self.after_push(&mut queue);
    }

    /// Queues news that is no row and is not kept: it is dropped, and the
    /// reader told to read again, when the queue is shed.
    pub fn push_event(&self, tag: Tag, feed: &Feed, text: String) {
        self.push_event_class(tag, feed, text, false);
    }

    /// A whole native resource replacement, distinguished from private progress
    /// so discarding an unsent replacement also invalidates its cached fingerprint.
    pub(crate) fn push_resource(&self, tag: Tag, feed: &Feed, text: String) {
        self.push_event_class(tag, feed, text, true);
    }

    fn push_event_class(&self, tag: Tag, feed: &Feed, text: String, resource: bool) {
        let mut queue = lock(&self.queue);
        if queue.closing.is_some() {
            return;
        }
        let size = bytes_of(&text);
        queue.push(
            tag.generation,
            size,
            What::Event {
                id: tag.id,
                feed: feed.clone(),
                text,
                resource,
            },
        );
        self.after_push(&mut queue);
    }

    /// Queues growing text. If the text that grew before it still waits, and
    /// this continues exactly where that ends, the two become one delta.
    pub fn push_delta(&self, tag: Tag, feed: &Feed, item: Id, chunk: Chunk) {
        let Chunk {
            field,
            offset,
            len,
            data,
        } = chunk;
        let Ok(fresh) = PendingDelta::new(field, offset, len, data) else {
            // Text that does not say what it is cannot be told apart from
            // text that was lost; the row of the item carries what is right.
            return;
        };
        let key = Key {
            generation: tag.generation,
            item,
            field,
        };
        let mut queue = lock(&self.queue);
        if queue.closing.is_some() {
            return;
        }
        if let Some(number) = queue.open.get(&key).copied() {
            let mut added = None;
            if let Some(Entry {
                what: What::Delta { pending, .. },
                size,
                ..
            }) = queue.entry_mut(number)
                && pending
                    .absorb(fresh.offset(), fresh.len(), fresh.data())
                    .is_ok()
            {
                let more = bytes_of(fresh.data());
                *size = size.saturating_add(more);
                added = Some(more);
            }
            if let Some(more) = added {
                if more > 0 {
                    metrics::counter!("orch_ws_merged_deltas_total").increment(1);
                }
                queue.bytes = queue.bytes.saturating_add(more);
                self.after_push(&mut queue);
                return;
            }
            // It does not continue what waits: that stays as it is, and this
            // goes behind it.
            queue.seal(&key);
        }
        let size = bytes_of(fresh.data()).saturating_add(DELTA_OVERHEAD_BYTES);
        let number = queue.push(
            tag.generation,
            size,
            What::Delta {
                id: tag.id,
                feed: feed.clone(),
                item,
                pending: fresh,
            },
        );
        queue.open.insert(key, number);
        self.after_push(&mut queue);
    }

    /// Queues a frame that leaves before everything that waits, such as the
    /// answer to a ping, whose delay would be taken for the line's.
    pub fn push_urgent(&self, text: String) {
        let mut queue = lock(&self.queue);
        if queue.closing.is_some() {
            return;
        }
        queue.bytes = queue.bytes.saturating_add(bytes_of(&text));
        queue.urgent.push_back(text);
        drop(queue);
        self.wake.notify_one();
    }

    /// Takes everything that waits for one subscription out of the queue: it
    /// ended, and what was read for it is of no use to anyone.
    pub fn purge(&self, generation: u64) {
        self.purge_projection(generation);
    }

    /// Purges the generation and reports which complete projections/reset were unsent.
    pub(crate) fn purge_projection(&self, generation: u64) -> Purged {
        let mut queue = lock(&self.queue);
        let mut purged = Purged::default();
        for entry in &queue.entries {
            if entry.generation == generation {
                purged.resource |= matches!(&entry.what, What::Event { resource: true, .. });
                purged.resync |= matches!(&entry.what, What::Resync(_));
            }
        }
        let numbers: Vec<u64> = queue
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.generation == generation && !matches!(entry.what, What::Gone)
            })
            .map(|(place, _)| queue.front + place as u64)
            .collect();
        for number in numbers {
            queue.remove(number);
        }
        queue.open.retain(|key, _| key.generation != generation);
        purged
    }

    /// Looks at what waits against the limits: what can be made up for is shed
    /// once the limit is passed, and the connection is to be closed when it
    /// has been passed for as long as is allowed. Called whenever something is
    /// queued, and from time to time while the socket takes nothing.
    pub fn tick(&self) -> BacklogAction {
        let mut queue = lock(&self.queue);
        self.judge(&mut queue)
    }

    fn after_push(&self, queue: &mut Queue) {
        self.judge(queue);
        self.wake.notify_one();
    }

    fn judge(&self, queue: &mut Queue) -> BacklogAction {
        if queue.closing.is_some() {
            return BacklogAction::Keep;
        }
        let action = queue.backlog.update(queue.waiting(), self.clock.time());
        match action {
            BacklogAction::Keep => {}
            BacklogAction::Shed => {
                Self::shed(queue);
            }
            BacklogAction::Close => {
                Self::close_in(
                    queue,
                    CloseCode::NotKeepingUp.code(),
                    "the connection did not keep up",
                );
            }
        }
        action
    }

    /// Drops the deltas and events that wait, and queues a `resync` for each
    /// subscription they belonged to.
    fn shed(queue: &mut Queue) {
        if queue.droppable == 0 {
            return;
        }
        let mut told: HashSet<u64> = HashSet::new();
        let mut resyncs: Vec<(u64, SubId, Feed)> = Vec::new();
        let droppable: Vec<(u64, u64, SubId, Feed)> = queue
            .entries
            .iter()
            .enumerate()
            .filter_map(|(place, entry)| {
                let (id, feed) = match &entry.what {
                    What::Event { id, feed, .. } | What::Delta { id, feed, .. } => {
                        (*id, feed.clone())
                    }
                    _ => return None,
                };
                Some((queue.front + place as u64, entry.generation, id, feed))
            })
            .collect();
        for (number, generation, id, feed) in droppable {
            queue.remove(number);
            if told.insert(generation) {
                resyncs.push((generation, id, feed));
            }
        }
        queue.open.clear();
        for (generation, id, feed) in resyncs {
            if let Ok(text) = ServerFrame::Resync(ResyncFrame {
                id,
                feed,
                reload: None,
            })
            .encode()
            {
                let size = bytes_of(&text);
                queue.push(generation, size, What::Resync(text));
            }
        }
    }

    fn close_in(queue: &mut Queue, code: u16, reason: &str) {
        if queue.closing.is_some() {
            return;
        }
        if code == CloseCode::NotKeepingUp.code() {
            metrics::counter!("orch_ws_slow_closes_total").increment(1);
        }
        queue.entries.clear();
        queue.urgent.clear();
        queue.open.clear();
        queue.droppable = 0;
        queue.bytes = 0;
        queue.closing = Some((code, reason.to_owned()));
    }

    /// Ends the connection with `code` after the frame in the socket's hands:
    /// whatever waits is let go.
    pub fn close(&self, code: u16, reason: &str) {
        let mut queue = lock(&self.queue);
        Self::close_in(&mut queue, code, reason);
        drop(queue);
        self.wake.notify_one();
    }

    /// Why and with what code the connection is to be closed, where it is.
    #[must_use]
    pub fn closing(&self) -> Option<(u16, String)> {
        lock(&self.queue).closing.clone()
    }

    /// How many bytes wait for the socket, the frame in its hands among them.
    #[must_use]
    pub fn waiting_bytes(&self) -> i64 {
        lock(&self.queue).waiting()
    }

    /// The writer is done with the frame it was given.
    pub fn done(&self) {
        lock(&self.queue).in_flight = 0;
    }

    fn take(&self) -> Option<Next> {
        let mut queue = lock(&self.queue);
        if let Some((code, reason)) = queue.closing.clone() {
            return Some(Next::Close(code, reason));
        }
        if let Some(text) = queue.urgent.pop_front() {
            let size = bytes_of(&text);
            queue.bytes = queue.bytes.saturating_sub(size);
            queue.in_flight = size;
            return Some(Next::Frame(text));
        }
        while let Some(entry) = queue.entries.pop_front() {
            queue.front += 1;
            queue.bytes = queue.bytes.saturating_sub(entry.size);
            if matches!(entry.what, What::Event { .. } | What::Delta { .. }) {
                queue.droppable = queue.droppable.saturating_sub(1);
            }
            let text = match entry.what {
                What::Gone => continue,
                What::Row(text) | What::Resync(text) | What::Event { text, .. } => text,
                What::Delta {
                    id,
                    feed,
                    item,
                    pending,
                } => {
                    let key = Key {
                        generation: entry.generation,
                        item,
                        field: pending.field(),
                    };
                    // It leaves: nothing can be added to it any more.
                    if queue.open.get(&key) == Some(&(queue.front - 1)) {
                        queue.open.remove(&key);
                    }
                    match delta_text(id, &feed, item, &pending) {
                        Some(text) => text,
                        None => continue,
                    }
                }
            };
            queue.in_flight = bytes_of(&text);
            return Some(Next::Frame(text));
        }
        None
    }

    /// The next thing for the socket, waiting for it. The caller tells the
    /// outbox when it is done with a frame ([`Outbox::done`]).
    pub async fn next(&self) -> Next {
        loop {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(next) = self.take() {
                return next;
            }
            notified.await;
        }
    }
}

// ---------------------------------------------------------------- passing growing text on

/// The most stretches of text that wait to be passed on for one thread. They
/// wait while the access engine's epoch is read, which is a moment; more than
/// this means the database does not answer, and a stretch that is let go is
/// not lost: the row of its item carries the text.
const MOST_WAITING: usize = 4_096;

impl Inner {
    /// Growing text for a thread arrives from the engine. It is passed on to the
    /// subscribers by a task that first reads the authorization epoch: a
    /// subscriber whose access was decided under an older one is asked about
    /// again before it is sent anything, since text that no longer is theirs to
    /// read must not leave before the word that it is not.
    pub(crate) fn hand_over(self: &Arc<Self>, delta: &LiveDelta) {
        let source = Source::Thread(delta.thread_id);
        let mut registry = self.registry();
        let Some(entry) = registry.sources.get_mut(&source) else {
            return;
        };
        entry.streaming = Some(tokio::time::Instant::now());
        if entry.waiting.len() < MOST_WAITING {
            entry.waiting.push(delta.clone());
        }
        if entry.carrying {
            return;
        }
        entry.carrying = true;
        let inner = Arc::clone(self);
        self.tasks.spawn(inner.carry(source));
    }

    async fn carry(self: Arc<Self>, source: Source) {
        loop {
            let (batch, subs) = {
                let mut registry = self.registry();
                let Some(entry) = registry.sources.get_mut(&source) else {
                    return;
                };
                if entry.waiting.is_empty() {
                    entry.carrying = false;
                    return;
                }
                (std::mem::take(&mut entry.waiting), entry.subs.clone())
            };
            if let Err(error) = crate::realtime::sync::read_epoch(&self).await {
                tracing::warn!(%error, "growing text could not be passed on");
                continue;
            }
            for sub in subs {
                if !sub.is_live() {
                    continue;
                }
                // Growing text has the same per-reader identity/authority boundary as a
                // stored sync frame; its shared text is never a shared access decision.
                let Some(_identity) = crate::realtime::watches::delivery_guard(
                    &self,
                    sub.connection(),
                )
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "growing text could not pin its reader's identity");
                    None
                }) else {
                    continue;
                };
                let current_epoch = match crate::realtime::sync::read_epoch(&self).await {
                    Ok(epoch) => epoch,
                    Err(error) => {
                        tracing::warn!(%error, "growing text could not read current authority");
                        continue;
                    }
                };
                if sub.decided() != current_epoch {
                    // The epoch moved since this subscriber's access was decided:
                    // it is asked again before the text is sent, and the text waits
                    // for the answer instead of leaving a hole in what the reader has.
                    let mut st = sub.state.lock().await;
                    match self.recheck(&sub, &mut st, current_epoch).await {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(error) => {
                            tracing::warn!(%error, "the access of a subscription could not be asked again");
                            continue;
                        }
                    }
                }
                if !sub.is_live() {
                    continue;
                }
                // The state lock orders live appends with scope reset and queued-row purging.
                // One identity-only batch query suffices, including for an explicit inactive view.
                let mut state = sub.state.lock().await;
                if !sub.is_live() || state.thread_reset {
                    continue;
                }
                let Feed::Thread(thread) = sub.feed() else {
                    continue;
                };
                let Some(projection) = state.thread_projection.as_ref() else {
                    self.invalidate_thread_projection(&sub, &mut state);
                    continue;
                };
                let ids: Vec<_> = batch
                    .iter()
                    .map(|delta| delta.item_id)
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect();
                let visible = match super::threads::live_items(
                    &self,
                    *thread,
                    sub.thread_scope(&state),
                    projection,
                    &ids,
                )
                .await
                {
                    Ok(Some(visible)) => visible.into_iter().collect::<HashSet<_>>(),
                    Ok(None) => {
                        self.invalidate_thread_projection(&sub, &mut state);
                        continue;
                    }
                    Err(error) => {
                        tracing::warn!(%error,"growing text could not verify its history projection");
                        continue;
                    }
                };
                for delta in &batch {
                    if !visible.contains(&delta.item_id)
                        || !wanted(&delta.stream, sub.sub_streams())
                    {
                        continue;
                    }
                    sub.connection().outbox.push_delta(
                        sub.tag(),
                        sub.feed(),
                        delta.item_id,
                        Chunk {
                            field: field_of(delta.field),
                            offset: delta.offset,
                            len: delta.len,
                            data: delta.data.clone(),
                        },
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------- what the engine hands over

fn field_of(field: LinkField) -> DeltaField {
    match field {
        LinkField::Text => DeltaField::Text,
        LinkField::Summary => DeltaField::Summary,
        LinkField::Output => DeltaField::Output,
    }
}

/// Where the thread engine hands growing text to: every subscription that
/// follows the thread is sent it, as it arrives.
///
/// What is handed over was applied, or is being applied in the transaction
/// that will make the same text durable. A client that sees a stretch twice
/// ignores the second, and one that misses a stretch reads the row, which
/// the feed sends within a moment after it is stored.
#[derive(Debug, Clone)]
pub struct Sink {
    realtime: Realtime,
}

impl Sink {
    pub(crate) fn new(realtime: Realtime) -> Self {
        Self { realtime }
    }
}

impl LiveSink for Sink {
    fn job_is_watched(&self, job: Id) -> bool {
        !self
            .realtime
            .inner
            .presence
            .list(&Feed::Job(job))
            .is_empty()
    }
    fn delta(&self, delta: &LiveDelta) {
        self.realtime.inner.hand_over(delta);
    }

    fn approvals_changed(&self, deciders: &[Id]) {
        self.realtime.tell_users(
            deciders,
            orch_realtime_contract::feeds::UserEvent::Approvals { summary: None },
        );
    }

    fn document_readers(&self, document: Id) -> usize {
        self.realtime
            .inner
            .presence
            .list(&Feed::Doc(document))
            .len()
    }

    fn thread_is_visible(&self, thread: Id, person: Id) -> bool {
        self.realtime
            .inner
            .presence
            .list(&Feed::Thread(thread))
            .iter()
            .any(|(reader, _)| *reader == person)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use orch_types::{ManualClock, new_id};

    fn clock() -> ManualClock {
        ManualClock::new(
            Utc.with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
                .single()
                .unwrap(),
        )
    }

    fn outbox(clock: &ManualClock, max_bytes: i64, max_secs: i64) -> Outbox {
        Outbox::new(Arc::new(clock.clone()), max_bytes, max_secs * 1_000)
    }

    fn tag(id: SubId) -> Tag {
        Tag {
            id,
            generation: u64::try_from(id).unwrap(),
        }
    }

    fn text(offset: i64, data: &str) -> Chunk {
        Chunk {
            field: DeltaField::Text,
            offset,
            len: i64::try_from(data.chars().count()).unwrap(),
            data: data.to_owned(),
        }
    }

    /// Everything the writer would take, in order, as (op, member) pairs.
    fn drain(outbox: &Outbox) -> Vec<serde_json::Value> {
        let mut taken = Vec::new();
        while let Some(next) = outbox.take() {
            match next {
                Next::Frame(text) => {
                    outbox.done();
                    taken.push(serde_json::from_str(&text).unwrap());
                }
                Next::Close(..) => break,
            }
        }
        taken
    }

    fn row(sequence: i64) -> String {
        format!(
            "{{\"op\":\"sync\",\"id\":1,\"feed\":\"user\",\"prev\":{},\"seq\":{sequence},\"data\":{{}}}}",
            sequence - 1
        )
    }

    #[test]
    fn deltas_that_wait_for_a_slow_reader_become_one_larger_delta_and_lose_nothing() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        outbox.push_delta(tag(1), &feed, item, text(0, "I'll start by read"));
        outbox.push_delta(tag(1), &feed, item, text(18, "ing the "));
        outbox.push_delta(tag(1), &feed, item, text(26, "handler."));
        let taken = drain(&outbox);
        assert_eq!(taken.len(), 1, "one delta left, not three");
        assert_eq!(taken[0]["offset"], 0);
        assert_eq!(taken[0]["len"], 34);
        assert_eq!(taken[0]["data"], "I'll start by reading the handler.");
    }

    #[test]
    fn a_reader_who_takes_each_frame_as_it_comes_is_sent_each_delta_as_it_grew() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        outbox.push_delta(tag(1), &feed, item, text(0, "I'll start by read"));
        let first = drain(&outbox);
        outbox.push_delta(tag(1), &feed, item, text(18, "ing the handler."));
        let second = drain(&outbox);
        assert_eq!((first.len(), second.len()), (1, 1));
        assert_eq!(second[0]["offset"], 18);
        assert_eq!(second[0]["data"], "ing the handler.");
    }

    #[test]
    fn the_deltas_of_two_items_are_not_mixed() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let feed = Feed::Thread(new_id());
        let (one, two) = (new_id(), new_id());
        outbox.push_delta(tag(1), &feed, one, text(0, "a"));
        outbox.push_delta(tag(1), &feed, two, text(0, "b"));
        outbox.push_delta(tag(1), &feed, one, text(1, "c"));
        let taken = drain(&outbox);
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0]["item_id"], one.to_string());
        assert_eq!(taken[0]["data"], "ac");
        assert_eq!(taken[1]["data"], "b");
    }

    #[test]
    fn a_delta_that_does_not_continue_the_one_that_waits_goes_behind_it() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        outbox.push_delta(tag(1), &feed, item, text(0, "abc"));
        // A stretch was skipped: the client reads the item when it sees the gap.
        outbox.push_delta(tag(1), &feed, item, text(10, "xyz"));
        let taken = drain(&outbox);
        assert_eq!(taken.len(), 2);
        assert_eq!(
            (taken[0]["offset"].as_i64(), taken[1]["offset"].as_i64()),
            (Some(0), Some(10))
        );
    }

    #[test]
    fn a_row_that_holds_all_that_a_waiting_delta_says_makes_the_delta_pointless() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        outbox.push_delta(tag(1), &feed, item, text(0, "I'll start"));
        outbox.push_sync(
            tag(1),
            row(5),
            &[Held {
                item,
                field: DeltaField::Text,
                len: 10,
            }],
        );
        let taken = drain(&outbox);
        assert_eq!(taken.len(), 1, "only the row");
        assert_eq!(taken[0]["op"], "sync");
    }

    #[test]
    fn a_row_that_holds_only_the_start_of_a_waiting_delta_goes_behind_it_and_what_grows_next_is_a_delta_of_its_own()
     {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        outbox.push_delta(tag(1), &feed, item, text(0, "I'll start"));
        outbox.push_sync(
            tag(1),
            row(5),
            &[Held {
                item,
                field: DeltaField::Text,
                len: 4,
            }],
        );
        outbox.push_delta(tag(1), &feed, item, text(10, " by"));
        let taken = drain(&outbox);
        let ops: Vec<&str> = taken
            .iter()
            .map(|frame| frame["op"].as_str().unwrap())
            .collect();
        assert_eq!(ops, ["delta", "sync", "delta"]);
        assert_eq!(taken[0]["data"], "I'll start");
        assert_eq!(taken[2]["offset"], 10);
    }

    #[test]
    fn a_row_older_than_a_waiting_delta_leaves_it_in_place_to_grow() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        outbox.push_delta(tag(1), &feed, item, text(10, "abc"));
        outbox.push_sync(
            tag(1),
            row(5),
            &[Held {
                item,
                field: DeltaField::Text,
                len: 10,
            }],
        );
        outbox.push_delta(tag(1), &feed, item, text(13, "def"));
        let taken = drain(&outbox);
        let ops: Vec<&str> = taken
            .iter()
            .map(|frame| frame["op"].as_str().unwrap())
            .collect();
        assert_eq!(ops, ["delta", "sync"]);
        assert_eq!(taken[0]["data"], "abcdef");
    }

    #[test]
    fn rows_are_never_dropped_and_leave_in_the_order_they_were_queued() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        for sequence in 1..=4 {
            outbox.push_delta(tag(1), &feed, item, text((sequence - 1) * 2, "ab"));
            outbox.push_sync(tag(1), row(sequence), &[]);
        }
        let taken = drain(&outbox);
        let rows: Vec<i64> = taken
            .iter()
            .filter(|frame| frame["op"] == "sync")
            .map(|frame| frame["seq"].as_i64().unwrap())
            .collect();
        assert_eq!(rows, [1, 2, 3, 4]);
    }

    #[test]
    fn an_answer_to_a_ping_leaves_before_what_waits() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        outbox.push_sync(tag(1), row(1), &[]);
        outbox.push_urgent("{\"op\":\"pong\",\"n\":3}".to_owned());
        let taken = drain(&outbox);
        assert_eq!(taken[0]["op"], "pong");
        assert_eq!(taken[1]["op"], "sync");
    }

    #[test]
    fn what_an_ended_subscription_left_in_the_queue_is_taken_out_and_the_others_are_not() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        outbox.push_delta(tag(1), &feed, item, text(0, "one"));
        outbox.push_sync(tag(1), row(1), &[]);
        outbox.push_sync(tag(2), row(2), &[]);
        outbox.purge(1);
        let taken = drain(&outbox);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0]["seq"], 2);
        assert_eq!(outbox.waiting_bytes(), 0);
    }

    #[test]
    fn past_the_limit_deltas_and_events_are_shed_and_each_subscription_is_told_to_read_again_once()
    {
        let clock = clock();
        let outbox = outbox(&clock, 2_000, 30);
        let feed = Feed::Thread(new_id());
        let (one, two) = (new_id(), new_id());
        outbox.push_sync(tag(1), row(1), &[]);
        outbox.push_delta(tag(1), &feed, one, text(0, &"x".repeat(700)));
        outbox.push_event(tag(1), &feed, "{\"op\":\"event\",\"id\":1}".to_owned());
        outbox.push_delta(tag(1), &feed, two, text(0, &"y".repeat(700)));
        // Under the limit nothing was dropped.
        assert_eq!(outbox.closing(), None);
        outbox.push_event(
            tag(1),
            &feed,
            format!(
                "{{\"op\":\"event\",\"id\":1,\"pad\":\"{}\"}}",
                "p".repeat(300)
            ),
        );
        let taken = drain(&outbox);
        let ops: Vec<&str> = taken
            .iter()
            .map(|frame| frame["op"].as_str().unwrap())
            .collect();
        assert_eq!(
            ops,
            ["sync", "resync"],
            "the row stays, the text goes, one word to read again"
        );
        assert_eq!(taken[1]["id"], 1);
    }

    #[test]
    fn the_limit_is_for_what_waits_and_a_reader_that_keeps_taking_frames_is_never_over_it() {
        let clock = clock();
        let outbox = outbox(&clock, 1_000, 30);
        let (feed, item) = (Feed::Thread(new_id()), new_id());
        for round in 0..50 {
            outbox.push_delta(tag(1), &feed, item, text(round * 400, &"z".repeat(400)));
            clock.advance(Duration::seconds(5));
            assert_eq!(drain(&outbox).len(), 1);
        }
        assert_eq!(outbox.closing(), None);
    }

    #[test]
    fn a_connection_that_takes_nothing_is_closed_with_4408_after_the_time_and_not_before() {
        let clock = clock();
        let outbox = outbox(&clock, 1_000, 30);
        for sequence in 1..=3 {
            outbox.push_sync(tag(1), format!("{}{}", row(sequence), "x".repeat(600)), &[]);
        }
        assert_eq!(outbox.closing(), None);
        clock.advance(Duration::seconds(29));
        assert_eq!(outbox.tick(), BacklogAction::Shed);
        assert_eq!(outbox.closing(), None);
        clock.advance(Duration::seconds(2));
        assert_eq!(outbox.tick(), BacklogAction::Close);
        let (code, _) = outbox.closing().expect("it is closed");
        assert_eq!(code, 4408);
        assert_eq!(outbox.waiting_bytes(), 0, "what waited is let go");
    }

    #[test]
    fn a_frame_in_the_sockets_hands_counts_as_waiting() {
        let clock = clock();
        let outbox = outbox(&clock, 1_000, 30);
        outbox.push_sync(tag(1), format!("{}{}", row(1), "x".repeat(2_000)), &[]);
        assert!(matches!(outbox.take(), Some(Next::Frame(_))));
        assert!(
            outbox.waiting_bytes() > 2_000,
            "the socket has not taken it yet"
        );
        clock.advance(Duration::seconds(31));
        assert_eq!(outbox.tick(), BacklogAction::Close);
        outbox.done();
    }

    #[test]
    fn frames_queued_after_the_connection_was_closed_are_not_kept() {
        let clock = clock();
        let outbox = outbox(&clock, 1 << 20, 30);
        outbox.close(CloseCode::GoingAway.code(), "stopping");
        outbox.push_sync(tag(1), row(1), &[]);
        assert_eq!(outbox.waiting_bytes(), 0);
        assert_eq!(
            outbox.take(),
            Some(Next::Close(1001, "stopping".to_owned()))
        );
    }

    #[tokio::test]
    async fn the_writer_waits_for_a_frame_and_is_woken_by_it() {
        let clock = clock();
        let outbox = Arc::new(outbox(&clock, 1 << 20, 30));
        let waiting = {
            let outbox = Arc::clone(&outbox);
            tokio::spawn(async move { outbox.next().await })
        };
        tokio::task::yield_now().await;
        outbox.push_sync(tag(1), row(1), &[]);
        let next = tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(next, Next::Frame(_)));
    }
}
