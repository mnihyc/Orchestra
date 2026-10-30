//! Transient feed delivery, shared watch ownership and current whole lists.
//!
//! One database lease elects a watch coordinator. Processes advertise their live feeds on the
//! existing transient presence channel, and the coordinator keeps their union through a ten-second
//! departure grace. Measurements and lists travel as bounded notification chunks, never in a table.
//! Lists keep only their latest value in memory; everything else is forgotten after delivery.

use super::subscriptions::Subscription;
use super::{Fail, Inner, Realtime};
use crate::backplane::{Link, Report};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::{Stream, StreamExt};
use orch_db::Db;
use orch_db::queries::job_leases::Lease;
use orch_db::sequences;
use orch_realtime_contract::feeds::{EventData, Feed, ResourceKind};
use orch_realtime_contract::frames::{EventFrame, ServerFrame};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

/// The protocol keeps an unused underlying watch for ten seconds.
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// Every process renews its interests once per second, repairing a lost announcement promptly.
const INTEREST_EVERY: Duration = Duration::from_secs(1);
/// Three missed renewals mean the process went away; its watches still receive the departure grace.
const PEER_EXPIRES: Duration = Duration::from_secs(3);
/// Timers use a tenth of a second, finer than the shortest feed's half-second batch.
const TICK: Duration = Duration::from_millis(100);
/// Whole lists and resource invalidations have at most one update per second.
const LIST_GAP: Duration = Duration::from_secs(1);
/// Direct-child changes accumulate for at most half a second.
const DIR_GAP: Duration = Duration::from_millis(500);
/// Measurements normally arrive every five seconds.
const SAMPLE_GAP: Duration = Duration::from_secs(5);
/// A chunk's base64 and envelope must fit PostgreSQL's 7,900-byte notification bound.
const CHUNK_BYTES: usize = 5_000;
/// A transient message has the same bound as the backplane's largest frame.
pub(crate) const MESSAGE_BYTES: usize = crate::backplane::MAX_BACKPLANE_FRAME_BYTES;
/// Eight maximum-size messages bound incomplete assemblies during a burst or lost chunk.
const ASSEMBLY_BYTES: usize = 8 * MESSAGE_BYTES;
/// The mailbox has the same capacity as the backplane's measurement broadcast.
const MAILBOX: usize = 1_024;
/// Reuse the API's largest page for background liveness reads; this is a batch size, not a watch limit.
const LIVENESS_BATCH: usize = orch_api_contract::common::MAX_LIMIT as usize;
/// A node's physical identity stays live even when its public inventory uses its host facade.
const LIVE_MACHINES_SQL: &str = "SELECT id FROM machines WHERE id=ANY($1) AND deleted_at IS NULL";
/// The name of the lease that elects the watch coordinator, distinct from every scheduled job's.
pub const COORDINATOR: &str = "realtime.ephemeral.coordinator";
/// How long the coordinator's lease holds past its last renewal: ten seconds, ten passes of
/// [`INTEREST_EVERY`]. A process that stopped without giving the role up leaves it to another that
/// soon, the same span as a departed watch's grace.
const COORDINATOR_LEASE: chrono::Duration = chrono::Duration::seconds(10);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn live_machines(conn: &mut orch_db::Conn, ids: &[Id]) -> Result<HashSet<Id>, Fail> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(sqlx::query_scalar(LIVE_MACHINES_SQL)
        .bind(ids)
        .fetch_all(conn)
        .await
        .map_err(Fail::other)?
        .into_iter()
        .collect())
}

/// Looks only at the requested sources, yielding deleted feeds one bounded batch at a time.
fn deleted_batches<'a>(
    db: &'a Db,
    feeds: &'a [Feed],
) -> impl Stream<Item = Result<Vec<Feed>, Fail>> + 'a {
    futures::stream::iter(feeds.chunks(LIVENESS_BATCH)).then(move |feeds| async move {
        let ids: Vec<_> = feeds
            .iter()
            .filter_map(|feed| match feed {
                Feed::Resource {
                    kind: ResourceKind::Machine,
                    ..
                } => None,
                Feed::Resource { id, .. } => Some(*id),
                other => super::environment_watches::environment(other),
            })
            .collect();
        let threads: Vec<_> = feeds
            .iter()
            .filter_map(|feed| match feed {
                Feed::ThreadRaw(id) | Feed::ThreadBackground(id) | Feed::ThreadGit(id) => Some(*id),
                _ => None,
            })
            .collect();
        let mut conn = db.acquire().await?;
        let resources = if ids.is_empty() {
            Vec::new()
        } else {
            orch_db::queries::resources::resource_facts(&mut conn, &ids).await?
        };
        let resources: HashSet<_> = resources
            .into_iter()
            .filter(|row| !row.deleted)
            .map(|row| (row.id, row.kind))
            .collect();
        // Nodes use a Host facade in the inventory, but their physical identity
        // still has a page and certificate. That inventory's absence is not deletion.
        let machine_ids: Vec<_> = feeds
            .iter()
            .filter_map(|feed| match feed {
                Feed::Resource {
                    kind: ResourceKind::Machine,
                    id,
                } => Some(*id),
                _ => None,
            })
            .collect();
        let machines = live_machines(&mut conn, &machine_ids).await?;
        let gone: HashSet<Id> = if threads.is_empty() {
            HashSet::new()
        } else {
            orch_db::queries::feeds::gone_nodes(&mut conn, &threads)
                .await?
                .into_iter()
                .collect()
        };
        Ok(feeds
            .iter()
            .filter_map(|feed| {
                let deleted = match feed {
                    Feed::Job(_) | Feed::Fork(_) => false,
                    Feed::ThreadRaw(id) | Feed::ThreadBackground(id) | Feed::ThreadGit(id) => {
                        gone.contains(id)
                    }
                    Feed::Resource {
                        kind: ResourceKind::Machine,
                        id,
                    } => !machines.contains(id),
                    Feed::Resource { kind, id } => {
                        !resources.contains(&(*id, kind.as_str().to_owned()))
                    }
                    other => super::environment_watches::environment(other)
                        .is_some_and(|id| !resources.contains(&(id, "environment".to_owned()))),
                };
                deleted.then(|| feed.clone())
            })
            .collect())
    })
}

/// A transient message exchanged by API processes on the notification channel.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Message {
    Interests {
        process: String,
        feeds: Vec<Feed>,
    },
    List {
        feed: Feed,
        data: Json,
    },
    Event {
        feed: Feed,
        data: Json,
    },
    Initial {
        process: String,
        generation: u64,
        feed: Feed,
        data: Json,
    },
    Current {
        process: String,
        generation: u64,
        feed: Feed,
    },
    DirectoryCompleted {
        completion: Box<super::environment_watches::DirectoryCompletion>,
    },
    /// Already-masked observations whose segment identity survives the shared channel.
    Raw {
        batch: super::raw_traffic::Batch,
    },
    /// Commit/authority wake for currently observed raw projections, never a timer poll.
    RawChanged {
        thread: Option<Id>,
    },
    Report {
        machine: Id,
        eph: orch_link_contract::streams::Eph,
    },
    Machine {
        machine: Id,
        session: Option<Id>,
    },
    Refresh,
    ViewChanged {
        environment: Id,
    },
    ObjectChanged {
        feed: Feed,
    },
    DocumentTreeChanged {
        root: Option<Id>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Chunk {
    ephemeral: Id,
    part: usize,
    total: usize,
    bytes: String,
}

struct Assembly {
    at: Instant,
    pieces: Vec<Option<Vec<u8>>>,
    size: usize,
}

#[derive(Default)]
struct Assemblies {
    waiting: HashMap<Id, Assembly>,
    bytes: usize,
}

impl Assemblies {
    fn take(&mut self, text: &str) -> Option<Message> {
        let chunk: Chunk = serde_json::from_str(text).ok()?;
        let now = Instant::now();
        self.waiting
            .retain(|_, held| now.duration_since(held.at) < STOP_GRACE);
        self.bytes = self.waiting.values().map(|held| held.size).sum();
        if chunk.total == 0
            || chunk.total > MESSAGE_BYTES.div_ceil(CHUNK_BYTES)
            || chunk.part >= chunk.total
        {
            return None;
        }
        let bytes = STANDARD.decode(chunk.bytes).ok()?;
        if bytes.len() > CHUNK_BYTES || self.bytes + bytes.len() > ASSEMBLY_BYTES {
            return None;
        }
        let held = self
            .waiting
            .entry(chunk.ephemeral)
            .or_insert_with(|| Assembly {
                at: now,
                pieces: vec![None; chunk.total],
                size: 0,
            });
        if held.pieces.len() != chunk.total {
            return None;
        }
        if held.pieces[chunk.part].is_none() {
            held.size += bytes.len();
            self.bytes += bytes.len();
            held.pieces[chunk.part] = Some(bytes);
        }
        if held.size > MESSAGE_BYTES || held.pieces.iter().any(Option::is_none) {
            return None;
        }
        let held = self.waiting.remove(&chunk.ephemeral)?;
        self.bytes = self.bytes.saturating_sub(held.size);
        let bytes: Vec<u8> = held.pieces.into_iter().flatten().flatten().collect();
        serde_json::from_slice(&bytes).ok()
    }
}

/// Publishes a transient message without recording it in any data table.
pub(crate) async fn broadcast(db: &Db, message: &Message) -> Result<(), Fail> {
    let bytes = serde_json::to_vec(message).map_err(Fail::other)?;
    if bytes.len() > MESSAGE_BYTES {
        return Err(Fail::other("the transient message exceeds the frame limit"));
    }
    let id = orch_types::new_id();
    // One transaction makes the chunks contiguous and prevents a partially published message.
    let mut tx = db.begin().await?;
    for (part, piece) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let text = serde_json::to_string(&Chunk {
            ephemeral: id,
            part,
            total: bytes.len().div_ceil(CHUNK_BYTES),
            bytes: STANDARD.encode(piece),
        })
        .map_err(Fail::other)?;
        if !sequences::notify_presence(&mut tx, &text).await? {
            return Err(Fail::other(
                "the transient chunk exceeds the notification limit",
            ));
        }
    }
    tx.commit().await.map_err(Fail::other)
}

/// Carries a gateway's ephemeral report to subscribers on every API process.
pub async fn forward_report(db: &Db, report: &Report) -> Result<(), Fail> {
    broadcast(
        db,
        &Message::Report {
            machine: report.machine_id,
            eph: report.eph.clone(),
        },
    )
    .await
}

/// State held only in this API process for unreplayed feeds.
pub(crate) struct Feeds {
    document_tree_dirty: AtomicBool,
    raw_dirty: AtomicBool,
    subs: Mutex<HashMap<Feed, Vec<Arc<Subscription>>>>,
    lists: Mutex<HashMap<Feed, Json>>,
    publishers: Mutex<HashSet<Feed>>,
    offline: Mutex<HashSet<Feed>>,
    assemblies: Mutex<Assemblies>,
    incoming: mpsc::Sender<Message>,
    receiver: Mutex<Option<mpsc::Receiver<Message>>>,
    changed: Notify,
    /// The machine link used to issue shared watch commands.
    pub(crate) link: OnceLock<Link>,
    /// The latest session or outage announced for each machine.
    pub(crate) sessions: Mutex<HashMap<Id, Option<Id>>>,
}

impl Default for Feeds {
    fn default() -> Self {
        let (incoming, receiver) = mpsc::channel(MAILBOX);
        Self {
            document_tree_dirty: AtomicBool::new(false),
            raw_dirty: AtomicBool::new(false),
            subs: Mutex::default(),
            lists: Mutex::default(),
            publishers: Mutex::default(),
            offline: Mutex::default(),
            assemblies: Mutex::default(),
            incoming,
            receiver: Mutex::new(Some(receiver)),
            changed: Notify::new(),
            link: OnceLock::new(),
            sessions: Mutex::default(),
        }
    }
}

impl Feeds {
    /// Local tree/authority/reconnect wake; bounded-mailbox overflow requests one consolidated repair.
    pub(crate) fn document_tree_changed(&self, root: Option<Id>) {
        if !lock(&self.subs)
            .keys()
            .any(|feed| matches!(feed, Feed::DocumentTree(_)))
        {
            return;
        }
        if self
            .incoming
            .try_send(Message::DocumentTreeChanged { root })
            .is_err()
        {
            self.document_tree_dirty.store(true, Ordering::Release);
            self.changed.notify_one();
        }
    }

    /// Only watched native sources are woken; overflow consolidates one repair without retaining an unbounded ID set.
    pub(crate) fn raw_changed(&self, thread: Option<Id>) {
        if !lock(&self.subs).keys().any(
            |feed| matches!(feed,Feed::ThreadRaw(id) if thread.is_none_or(|thread|thread==*id)),
        ) {
            return;
        }
        if self
            .incoming
            .try_send(Message::RawChanged { thread })
            .is_err()
        {
            self.raw_dirty.store(true, Ordering::Release);
            self.changed.notify_one();
        }
    }

    pub(crate) fn raw_subscribers(&self, thread: Option<Id>) -> Vec<Arc<Subscription>> {
        lock(&self.subs).iter().filter(|(feed,_)|matches!(feed,Feed::ThreadRaw(id) if thread.is_none_or(|thread|thread==*id)))
            .flat_map(|(_,subs)|subs.iter().cloned()).collect()
    }

    pub(crate) fn document_tree_subscribers(&self) -> Vec<Arc<Subscription>> {
        lock(&self.subs)
            .iter()
            .filter(|(feed, _)| matches!(feed, Feed::DocumentTree(_)))
            .flat_map(|(_, subs)| subs.iter().cloned())
            .collect()
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<Message> {
        self.incoming.clone()
    }
    /// Closes a feed only when its stored source is actually deleted.
    pub(super) async fn close_deleted(&self, inner: &Arc<Inner>) -> Result<(), Fail> {
        let feeds = self.interests();
        if feeds.is_empty() {
            return Ok(());
        }
        // Only the active interest index is snapshotted. Database rows and identifier
        // arrays live for one page, independently of the number of stored resources.
        let batches = deleted_batches(&inner.db, &feeds);
        tokio::pin!(batches);
        while let Some(batch) = batches.next().await {
            for feed in batch? {
                for sub in self.subscribers(&feed) {
                    let mut st = sub.state.lock().await;
                    inner
                        .close_sub(
                            &sub,
                            &mut st,
                            orch_realtime_contract::frames::FeedClosedReason::Deleted,
                        )
                        .await;
                }
            }
        }
        Ok(())
    }

    /// The latest complete list held in this process.
    pub(crate) fn list(&self, feed: &Feed) -> Option<Json> {
        lock(&self.lists).get(feed).cloned()
    }

    /// Clears the cached outage when an environment returns.
    pub(crate) fn online(&self, feed: &Feed) {
        lock(&self.offline).remove(feed);
    }

    /// A committed row change wakes only the live views of that environment.
    pub(crate) fn environment_changed(&self, environment: Id) {
        let _ = self.incoming.try_send(Message::ViewChanged { environment });
    }

    pub(crate) fn object_changed(&self, feed: Feed) {
        let _ = self.incoming.try_send(Message::ObjectChanged { feed });
    }

    /// The announced session, distinguishing an unknown route from an outage.
    pub(crate) fn session(&self, machine: Id) -> Option<Option<Id>> {
        lock(&self.sessions).get(&machine).copied()
    }
    /// Adds a local reader and wakes the shared interest ledger.
    pub(crate) fn register(&self, sub: &Arc<Subscription>) {
        lock(&self.subs)
            .entry(sub.feed().clone())
            .or_default()
            .push(Arc::clone(sub));
        self.changed.notify_one();
    }

    /// Removes a local reader and announces the changed interests.
    pub(crate) fn unregister(&self, sub: &Arc<Subscription>) {
        let mut subs = lock(&self.subs);
        if let Some(held) = subs.get_mut(sub.feed()) {
            held.retain(|each| !Arc::ptr_eq(each, sub));
            if held.is_empty() {
                subs.remove(sub.feed());
            }
        }
        self.changed.notify_one();
    }

    /// The number of local transient subscriptions.
    pub(crate) fn len(&self) -> usize {
        lock(&self.subs).values().map(Vec::len).sum()
    }

    fn interests(&self) -> Vec<Feed> {
        lock(&self.subs).keys().cloned().collect()
    }

    fn subscribers(&self, feed: &Feed) -> Vec<Arc<Subscription>> {
        lock(&self.subs).get(feed).cloned().unwrap_or_default()
    }

    /// Assembles a notification and admits the complete message to the bounded mailbox.
    pub(crate) fn heard(&self, text: &str) {
        if let Some(message) = lock(&self.assemblies).take(text) {
            let _ = self.incoming.try_send(message);
        }
    }

    /// Seeds a reader from the current list or outage and says whether an outage was sent.
    pub(crate) async fn current(
        &self,
        inner: &Arc<Inner>,
        sub: &Arc<Subscription>,
        st: &mut super::subscriptions::SubState,
    ) -> Result<bool, Fail> {
        if matches!(sub.feed(), Feed::DocumentTree(_)) {
            super::document_tree::current(inner, sub, st).await?;
            return Ok(false);
        }
        if let Feed::Usage(scope) = sub.feed() {
            for visible in
                super::usage::project(inner, sub.connection(), scope, &mut st.usage_read).await?
            {
                if scope.ends_with(":health") {
                    st.health_read = Some(super::resource_health::fingerprint(&visible));
                }
                send(sub, &visible)?;
            }
            return Ok(false);
        }
        // A process that joined during an outage did not hear the earlier transition.
        let offline = if let Some(id) = super::environment_watches::environment(sub.feed()) {
            let mut conn = inner.db.acquire().await?;
            super::environment_watches::reachable(inner, &mut conn, id)
                .await?
                .is_none()
        } else {
            lock(&self.offline).contains(sub.feed())
        };
        let data = if offline {
            Some(serde_json::json!({"offline": true}))
        } else {
            lock(&self.lists).get(sub.feed()).cloned().or_else(|| {
                if matches!(sub.feed(), Feed::Resource { .. }) {
                    return Some(serde_json::json!({"what": "changed"}));
                }
                (super::machine_views::projected(sub.feed())
                    && !matches!(
                        sub.feed(),
                        Feed::ThreadGit(_) | Feed::EnvGit { .. } | Feed::EnvTerminals(_)
                    ))
                .then(|| serde_json::json!({}))
            })
        };
        if let Some(data) = data {
            // Availability reads can await the database too; the seed is authorized for
            // this reader at the same serialization boundary as a newly published list.
            let epoch = super::sync::read_epoch(inner).await?;
            if !inner.recheck(sub, st, epoch).await? {
                return Ok(offline);
            }
            if let Feed::ThreadBackground(thread) = sub.feed() {
                let visible =
                    super::background_watches::project(inner, sub.connection(), *thread, &data)
                        .await?;
                send(sub, &visible)?;
            } else if matches!(sub.feed(), Feed::Job(_) | Feed::Fork(_)) {
                match super::machine_views::progress(inner, sub.connection(), sub.feed()).await? {
                    Ok(visible) => {
                        let epoch = super::sync::read_epoch(inner).await?;
                        if inner.recheck(sub, st, epoch).await? {
                            send(sub, &visible)?;
                        }
                    }
                    Err(access) => {
                        inner
                            .close_sub(
                                sub,
                                st,
                                if access == super::subscriptions::Access::NotFound {
                                    orch_realtime_contract::frames::FeedClosedReason::Deleted
                                } else {
                                    orch_realtime_contract::frames::FeedClosedReason::Revoked
                                },
                            )
                            .await
                    }
                }
            } else if matches!(sub.feed(), Feed::Resource { .. }) && data["what"] == "changed" {
                send_resource(inner, sub, st, &data).await?;
            } else if super::machine_views::projected(sub.feed()) {
                send_machine(inner, sub, st, &data).await?;
            } else {
                send(sub, &data)?;
            }
            if matches!(sub.feed(), Feed::EnvDir { .. }) && data["error"].is_object() {
                broadcast(
                    &inner.db,
                    &Message::Current {
                        process: inner.process.clone(),
                        generation: sub.tag().generation,
                        feed: sub.feed().clone(),
                    },
                )
                .await?;
            }
        } else if is_list(sub.feed()) {
            broadcast(
                &inner.db,
                &Message::Current {
                    process: inner.process.clone(),
                    generation: sub.tag().generation,
                    feed: sub.feed().clone(),
                },
            )
            .await?;
        }
        Ok(offline)
    }
}

fn is_list(feed: &Feed) -> bool {
    matches!(
        feed,
        Feed::EnvBrowsers(_)
            | Feed::EnvDisplays(_)
            | Feed::ThreadBackground(_)
            | Feed::Usage(_)
            | Feed::EnvStats(_)
            | Feed::EnvProcs(_)
            | Feed::EnvTerminals(_)
            | Feed::EnvPorts(_)
            | Feed::EnvGit { .. }
            | Feed::ThreadGit(_)
            | Feed::EnvDir { .. }
            | Feed::Job(_)
            | Feed::Fork(_)
    )
}

pub(crate) fn send(sub: &Subscription, data: &Json) -> Result<(), Fail> {
    let data = EventData::decode(sub.feed(), data.clone()).map_err(Fail::other)?;
    let frame = EventFrame::new(sub.tag().id, sub.feed().clone(), &data).map_err(Fail::other)?;
    let text = ServerFrame::Event(frame).encode().map_err(Fail::other)?;
    sub.connection()
        .outbox
        .push_event(sub.tag(), sub.feed(), text);
    Ok(())
}

/// A deleted machine-backed source closes its own reader, including during
/// initial projection after the ACK. Changed authority reprojects before any
/// private field is published, just as a native resource replacement does.
async fn send_machine(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut super::subscriptions::SubState,
    data: &Json,
) -> Result<(), Fail> {
    loop {
        if inner.stop.is_cancelled() {
            return Ok(());
        }
        let epoch = super::sync::read_epoch(inner).await?;
        if !inner.recheck(sub, st, epoch).await? {
            return Ok(());
        }
        let cut = (
            sub.decided(),
            sub.connection().session(),
            sub.connection().as_admin(),
        );
        let projected =
            super::machine_views::project(inner, sub.connection(), sub.feed(), data).await?;
        let latest = super::sync::read_epoch(inner).await?;
        let expired = sub.decided() != cut.0;
        if !inner.recheck(sub, st, latest).await? {
            return Ok(());
        }
        if latest != epoch
            || expired
            || sub.connection().session() != cut.1
            || sub.connection().as_admin() != cut.2
        {
            continue;
        }
        match projected {
            Ok(Some(visible)) => send(sub, &visible)?,
            Ok(None) => {}
            Err(access) => {
                inner
                    .close_sub(
                        sub,
                        st,
                        if matches!(
                            access,
                            super::subscriptions::Access::NotFound
                                | super::subscriptions::Access::Gone
                        ) {
                            orch_realtime_contract::frames::FeedClosedReason::Deleted
                        } else {
                            orch_realtime_contract::frames::FeedClosedReason::Revoked
                        },
                    )
                    .await
            }
        }
        return Ok(());
    }
}

/// Project before the existing delivery fence; an identical visible row is no new event.
async fn send_resource(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut super::subscriptions::SubState,
    data: &Json,
) -> Result<(), Fail> {
    loop {
        if inner.stop.is_cancelled() {
            return Ok(());
        }
        let epoch = super::sync::read_epoch(inner).await?;
        if !inner.recheck(sub, st, epoch).await? {
            return Ok(());
        }
        let cut = (
            sub.decided(),
            sub.connection().session(),
            sub.connection().as_admin(),
        );
        let visible =
            super::resource_rows::project(inner, sub.connection(), sub.feed(), data).await?;
        #[cfg(test)]
        super::epoch_handoff_tests::after_projection(super::epoch_handoff_tests::Cut::Resource)
            .await;
        let latest = super::sync::read_epoch(inner).await?;
        let expired = sub.decided() != cut.0;
        if !inner.recheck(sub, st, latest).await? {
            return Ok(());
        }
        if latest != epoch
            || expired
            || sub.connection().session() != cut.1
            || sub.connection().as_admin() != cut.2
        {
            // Reproject a changed cut; a new View decision cannot validate an old
            // rich row that still carried private Manage fields.
            continue;
        }
        let Some(visible) = visible else {
            inner
                .close_sub(
                    sub,
                    st,
                    orch_realtime_contract::frames::FeedClosedReason::Revoked,
                )
                .await;
            return Ok(());
        };
        let fingerprint = (visible.get("row").is_some() || visible.get("view_revoked").is_some())
            .then(|| orch_types::Hash::of(visible.to_string().as_bytes()));
        if fingerprint.is_some() && st.resource_read == fingerprint {
            return Ok(());
        }
        if fingerprint.is_some() {
            let data = EventData::decode(sub.feed(), visible).map_err(Fail::other)?;
            let frame =
                EventFrame::new(sub.tag().id, sub.feed().clone(), &data).map_err(Fail::other)?;
            let text = ServerFrame::Event(frame).encode().map_err(Fail::other)?;
            sub.connection()
                .outbox
                .push_resource(sub.tag(), sub.feed(), text);
        } else {
            send(sub, &visible)?;
        }
        st.resource_read = fingerprint;
        return Ok(());
    }
}

impl Realtime {
    /// Publishes a whole browser/profile or display list, retaining only the latest in memory.
    ///
    /// Pass `Feed::EnvBrowsers(id)` with `{ browsers, profiles }`, or `Feed::EnvDisplays(id)` with
    /// `{ displays }`, after the change commits. Call on each change; updates within one second
    /// coalesce to the last list, reach every API process, and seed new subscribers immediately.
    pub async fn publish_list(&self, feed: Feed, data: Json) -> Result<(), Fail> {
        if !is_list(&feed) {
            return Err(Fail::other("this feed does not carry a whole list"));
        }
        EventData::decode(&feed, data.clone()).map_err(Fail::other)?;
        if data.get("offline") == Some(&Json::Bool(true)) {
            return Err(Fail::other(
                "a publisher supplies a list; the watch decides whether it is offline",
            ));
        }
        broadcast(
            &self.inner.db,
            &Message::List {
                feed: feed.clone(),
                data: data.clone(),
            },
        )
        .await?;
        lock(&self.inner.ephemeral.publishers).insert(feed.clone());
        lock(&self.inner.ephemeral.lists).insert(feed, data);
        Ok(())
    }

    /// Gives transient watches the machine link; the socket also calls this automatically.
    pub fn attach_link(&self, link: Link) {
        let _ = self.inner.ephemeral.link.set(link);
    }
}

struct Peer {
    seen: Instant,
    feeds: HashSet<Feed>,
}
struct Active {
    idle: Option<Instant>,
}
struct Pending {
    due: Instant,
    data: Json,
}

/// The shared lifecycle ledger and rate-limited pending events, held by the coordinator.
#[derive(Default)]
pub(crate) struct Coordinator {
    peers: HashMap<String, Peer>,
    active: HashMap<Feed, Active>,
    pending: HashMap<(Feed, String), Pending>,
    last: HashMap<(Feed, String), Instant>,
}

impl Coordinator {
    fn interests(&mut self, process: String, feeds: Vec<Feed>) {
        self.interests_at(process, feeds, Instant::now());
    }

    fn interests_at(&mut self, process: String, feeds: Vec<Feed>, now: Instant) {
        self.peers.insert(
            process,
            Peer {
                seen: now,
                feeds: feeds.into_iter().collect(),
            },
        );
    }

    /// The union of renewed interests and watches still in their departure grace.
    pub(crate) fn active(&mut self) -> HashSet<Feed> {
        self.active_at(Instant::now())
    }

    fn active_at(&mut self, now: Instant) -> HashSet<Feed> {
        self.peers
            .retain(|_, peer| now.duration_since(peer.seen) < PEER_EXPIRES);
        let wanted: HashSet<_> = self
            .peers
            .values()
            .flat_map(|peer| peer.feeds.iter().cloned())
            .collect();
        for feed in &wanted {
            self.active
                .entry(feed.clone())
                .or_insert(Active { idle: None })
                .idle = None;
        }
        self.active.retain(|feed, held| {
            if wanted.contains(feed) {
                return true;
            }
            let idle = held.idle.get_or_insert(now);
            now.duration_since(*idle) < STOP_GRACE
        });
        self.active.keys().cloned().collect()
    }

    /// Queues the latest event under the rate of its feed; process signals bypass the periodic gap.
    pub(crate) fn queue(&mut self, feed: Feed, data: Json, immediate: bool) {
        let what = data
            .get("what")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned();
        let gap = match &feed {
            Feed::EnvDir { .. } => DIR_GAP,
            Feed::EnvStats(_) => SAMPLE_GAP,
            Feed::EnvProcs(_) => Duration::ZERO,
            Feed::ThreadRaw(_) => Duration::ZERO,
            Feed::Resource { .. } if what == "login" => Duration::ZERO,
            _ => LIST_GAP,
        };
        let key = (feed, what);
        let now = Instant::now();
        let due = if immediate {
            now
        } else if let Some(last) = self.last.get(&key) {
            now.max(*last + gap)
        } else if is_list(&key.0) {
            now + gap
        } else {
            now
        };
        match self.pending.get_mut(&key) {
            Some(waiting) if matches!(key.0, Feed::EnvDir { .. }) => {
                merge_directory(&mut waiting.data, data)
            }
            Some(waiting) => {
                waiting.data = data;
                if immediate {
                    waiting.due = now;
                }
            }
            None => {
                self.pending.insert(key, Pending { due, data });
            }
        }
    }

    async fn flush(&mut self, inner: &Inner) -> Result<(), Fail> {
        let now = Instant::now();
        let ready: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.due <= now)
            .map(|(key, _)| key.clone())
            .collect();
        for key in ready {
            if let Some(pending) = self.pending.remove(&key) {
                broadcast(
                    &inner.db,
                    &Message::Event {
                        feed: key.0.clone(),
                        data: pending.data,
                    },
                )
                .await?;
                self.last.insert(key, Instant::now());
            }
        }
        Ok(())
    }
}

pub(crate) fn merge_directory(held: &mut Json, new: Json) {
    if new["reset"] == true || new["error"].is_object() {
        *held = new;
        return;
    }
    let reset = held["reset"] == true;
    let invalidated = new["invalidated"] == true || held["invalidated"] == true;
    let truncated = new
        .get("truncated")
        .and_then(Json::as_bool)
        .or_else(|| held["truncated"].as_bool())
        .unwrap_or(false);
    let mut changed: HashMap<String, Json> = held["changed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| Some((entry["name"].as_str()?.to_owned(), entry.clone())))
        .collect();
    let mut removed: HashSet<String> = held["removed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();
    for name in new["removed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
    {
        changed.remove(name);
        removed.insert(name.to_owned());
    }
    for entry in new["changed"].as_array().into_iter().flatten() {
        if let Some(name) = entry["name"].as_str() {
            removed.remove(name);
            changed.insert(name.to_owned(), entry.clone());
        }
    }
    *held = serde_json::json!({"as_of": new["as_of"], "changed": changed.into_values().collect::<Vec<_>>(),
        "removed": if reset { Vec::<String>::new() } else { removed.into_iter().collect::<Vec<_>>() },
        "reset":reset,"truncated":truncated,"invalidated":invalidated,
        "revision":new["revision"],"next_cursor":new["next_cursor"],"expires_at":new["expires_at"],
        "observed_from":new["observed_from"]});
}

async fn deliver(
    inner: &Arc<Inner>,
    feed: &Feed,
    data: &Json,
    generation: Option<u64>,
) -> Result<(), Fail> {
    // Raw delivery requires its typed source batch and admitted live floor; a generic event cannot supply that proof.
    if matches!(feed, Feed::ThreadRaw(_)) {
        return Ok(());
    }
    if data.get("offline") == Some(&Json::Bool(true)) {
        lock(&inner.ephemeral.offline).insert(feed.clone());
    } else {
        lock(&inner.ephemeral.offline).remove(feed);
    }
    for sub in inner.ephemeral.subscribers(feed) {
        if generation.is_some_and(|generation| generation != sub.tag().generation) {
            continue;
        }
        let mut st = sub.state.lock().await;
        if !st.open {
            continue;
        }
        // Each recipient pins its own identity and authority before the shared payload
        // is projected and encoded; an ended token is never another token of that user.
        let Some(_identity) = super::watches::delivery_guard(inner, sub.connection()).await? else {
            continue;
        };
        let epoch = super::sync::read_epoch(inner).await?;
        if !inner.recheck(&sub, &mut st, epoch).await? || !sub.is_live() {
            continue;
        }
        let offline = data.get("offline") == Some(&Json::Bool(true));
        if offline && st.offline {
            continue;
        }
        st.offline = offline;
        if matches!(feed, Feed::Resource { .. })
            && matches!(data["what"].as_str(), Some("login" | "job"))
            && !super::subscriptions::viewing(
                sub.connection(),
                super::resources::may_manage(inner, sub.connection().user, feed),
            )
            .await?
        {
            continue;
        }
        // Browser lists are published canonically and cut down for the person receiving them.
        if let Feed::Usage(scope) = feed {
            for visible in
                super::usage::project(inner, sub.connection(), scope, &mut st.usage_read).await?
            {
                if scope.ends_with(":health") {
                    let hash = super::resource_health::fingerprint(&visible);
                    let epoch = super::sync::read_epoch(inner).await?;
                    if inner.recheck(&sub, &mut st, epoch).await? && st.health_read != Some(hash) {
                        send(&sub, &visible)?;
                        st.health_read = Some(hash);
                    }
                } else {
                    send(&sub, &visible)?;
                }
            }
        } else if let Feed::ThreadBackground(thread) = feed {
            let visible =
                super::background_watches::project(inner, sub.connection(), *thread, data).await?;
            send(&sub, &visible)?;
        } else if matches!(feed, Feed::Job(_) | Feed::Fork(_)) {
            match super::machine_views::progress(inner, sub.connection(), feed).await? {
                Ok(visible) => {
                    let epoch = super::sync::read_epoch(inner).await?;
                    if inner.recheck(&sub, &mut st, epoch).await? {
                        send(&sub, &visible)?;
                    }
                }
                Err(access) => {
                    inner
                        .close_sub(
                            &sub,
                            &mut st,
                            if access == super::subscriptions::Access::NotFound {
                                orch_realtime_contract::frames::FeedClosedReason::Deleted
                            } else {
                                orch_realtime_contract::frames::FeedClosedReason::Revoked
                            },
                        )
                        .await
                }
            }
        } else if matches!(feed, Feed::Resource { .. }) && data["what"] == "changed" {
            send_resource(inner, &sub, &mut st, data).await?;
        } else if super::machine_views::projected(feed) {
            send_machine(inner, &sub, &mut st, data).await?;
        } else {
            send(&sub, data)?;
        }
    }
    Ok(())
}

/// Starts the single transient-feed actor for this API process.
pub(crate) fn start(inner: &Arc<Inner>) {
    if let Some(receiver) = lock(&inner.ephemeral.receiver).take() {
        let inner = Arc::clone(inner);
        inner.tasks.clone().spawn(async move {
            run(inner, receiver).await;
        });
    }
}

async fn run(inner: Arc<Inner>, mut receiver: mpsc::Receiver<Message>) {
    let mut coordinator = Coordinator::default();
    let mut sources = super::environment_watches::Sources::default();
    let mut background = super::background_watches::Sources::default();
    let mut usage = super::usage::Sources::default();
    let mut prior_active = HashSet::new();
    let mut resources = super::resource_changes::Resources::default();
    // A lease renewed at each pass, not a lock kept in a transaction held open for as long as the
    // process coordinates.
    let lease = Lease::new(COORDINATOR, inner.process.clone(), COORDINATOR_LEASE);
    let mut coordinating = false;
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut renewed = Instant::now() - INTEREST_EVERY;
    loop {
        let message = tokio::select! {
            () = inner.stop.cancelled() => break,
            () = inner.ephemeral.changed.notified() => Some(Message::Refresh),
            _ = interval.tick() => None,
            message = receiver.recv() => message,
        };
        let result = async {
            if renewed.elapsed() >= INTEREST_EVERY || matches!(message, Some(Message::Refresh)) {
                inner.ephemeral.close_deleted(&inner).await?;
                let feeds = inner.ephemeral.interests();
                coordinator.interests(inner.process.clone(), feeds.clone());
                broadcast(
                    &inner.db,
                    &Message::Interests {
                        process: inner.process.clone(),
                        feeds,
                    },
                )
                .await?;
                if inner.ephemeral.link.get().is_some() {
                    // Taken where it is free, renewed while held. A process whose renewal fails stops
                    // coordinating until it holds the lease again; nobody else takes it before it lapses.
                    let held = lease.hold(&inner.db, inner.clock.now()).await;
                    coordinating = held.as_ref().is_ok_and(|held| *held);
                    held?;
                }
                renewed = Instant::now();
            }
            if inner.ephemeral.raw_dirty.swap(false,Ordering::AcqRel) {
                super::raw_traffic::refresh(&inner,None).await?;
            }
            match message {
                Some(Message::Raw {batch})=>super::raw_traffic::deliver(&inner,batch).await?,
                Some(Message::RawChanged {thread})=>super::raw_traffic::refresh(&inner,thread).await?,
                Some(Message::DocumentTreeChanged {root})=>super::document_tree::refresh(&inner,root).await?,
                Some(Message::Refresh) if inner.ephemeral.document_tree_dirty.swap(false,Ordering::AcqRel)=>{
                    super::document_tree::refresh(&inner,None).await?;
                },
                Some(Message::DirectoryCompleted { completion }) if coordinating => {
                    sources.completed(&inner, *completion, &mut coordinator)?;
                }
                Some(Message::Interests { process, feeds }) => {
                    coordinator.interests(process, feeds)
                }
                Some(Message::List { feed, data }) => {
                    lock(&inner.ephemeral.lists).insert(feed.clone(), data.clone());
                    if coordinating {
                        coordinator.queue(feed, data, false);
                    }
                }
                Some(Message::Event { feed, data }) => {
                    if data.get("offline") != Some(&Json::Bool(true)) && is_list(&feed) {
                        let mut lists = lock(&inner.ephemeral.lists);
                        if matches!(feed, Feed::EnvDir { .. }) {
                            if data["reset"] == true || data["error"].is_object() {
                                lists.insert(feed.clone(), data.clone());
                            } else if let Some(held) = lists.get_mut(&feed) {
                                merge_directory(held, data.clone());
                            }
                        } else { lists.insert(feed.clone(), data.clone()); }
                    }
                    if let Feed::EnvStats(environment) = &feed
                        && let Some(app) = inner.app()
                        && let Ok(sample) = serde_json::from_value::<orch_api_contract::processes::ResourceSample>(data.clone())
                    { app.cache.samples().push(*environment, sample); }
                    resources.observed(&feed, &data);
                    deliver(&inner, &feed, &data, None).await?;
                }
                Some(Message::Initial {
                    process,
                    generation,
                    feed,
                    data,
                }) if process == inner.process => {
                    deliver(&inner, &feed, &data, Some(generation)).await?;
                }
                Some(Message::Current {
                    process,
                    generation,
                    feed,
                }) => {
                    if coordinating && sources.retry(&feed) {
                        lock(&inner.ephemeral.lists).remove(&feed);
                    }
                    let data = lock(&inner.ephemeral.lists).get(&feed).cloned();
                    if (lock(&inner.ephemeral.publishers).contains(&feed)
                        || (coordinating && is_list(&feed)))
                        && let Some(data) = data
                    {
                        broadcast(
                            &inner.db,
                            &Message::Initial {
                                process,
                                generation,
                                feed,
                                data,
                            },
                        )
                        .await?;
                    }
                }
                Some(Message::Report { machine, eph }) if coordinating => {
                    sources.report(&inner, machine, eph, &mut coordinator).await?
                }
                Some(Message::Machine { machine, session }) => {
                    lock(&inner.ephemeral.sessions).insert(machine, session);
                    sources.changed();
                }
                Some(Message::ViewChanged { environment }) => {
                    sources.changed();
                    if coordinating {
                        for feed in coordinator.active() {
                            if matches!(&feed, Feed::EnvTerminals(id) | Feed::EnvPorts(id) | Feed::EnvBrowsers(id) | Feed::EnvDisplays(id) if *id == environment) {
                                let data = lock(&inner.ephemeral.lists).get(&feed).cloned().unwrap_or_else(|| serde_json::json!({}));
                                coordinator.queue(feed, data, false);
                            }
                        }
                    }
                }
                Some(Message::ObjectChanged { feed }) => {
                    if coordinating && coordinator.active().contains(&feed) {
                        coordinator.queue(feed, serde_json::json!({}), false);
                    }
                }
                Some(Message::Report { .. } | Message::DirectoryCompleted { .. })
                | Some(Message::Initial { .. } | Message::Refresh)
                | None => {}
            }
            if coordinating {
                let active = coordinator.active();
                if active != prior_active {
                    sources.changed();
                    prior_active = active.clone();
                }
                sources.reconcile(&inner, &active, &mut coordinator).await?;
                background.reconcile(&inner, &active).await?;
                usage.reconcile(&inner,&active).await?;
                lock(&inner.ephemeral.lists).retain(|feed, _| {
                    (!matches!(feed, Feed::ThreadBackground(_) | Feed::ThreadGit(_) | Feed::EnvGit { .. }
                        | Feed::Job(_) | Feed::Fork(_)
                        | Feed::EnvProcs(_) | Feed::EnvStats(_) | Feed::EnvTerminals(_) | Feed::EnvPorts(_) | Feed::EnvDir { .. }))
                        || active.contains(feed)
                });
                resources
                    .reconcile(&inner, &active, &mut coordinator)
                    .await?;
                coordinator
                    .pending
                    .retain(|(feed, _), _| active.contains(feed) && sources.available(feed));
                coordinator.flush(&inner).await?;
            }
            Ok::<_, Fail>(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "a transient feed pass failed");
        }
    }
    // A process that stops gives the role up at once; one that cannot leaves the lease to lapse.
    if let Err(error) = lease.release(&inner.db).await {
        tracing::debug!(%error, "the watch coordinator's lease is left to lapse");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn watching_a_thousand_machines_among_a_million_reads_only_indexed_batches() {
        let db = orch_testkit::database::TestDb::fresh().await;
        let mut conn = db.pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO machines (id,name,placement,platform_os,platform_arch,connector_version,
                 protocol_version,cert_serial,cert_fingerprint,cert_not_after,deleted_at)
             SELECT md5('feed-scale-'||g)::uuid,'Synthetic machine','connected','linux','x86_64',
                 'test',1,g::text,decode(md5('feed-scale-'||g),'hex'),
                 '2099-01-01'::timestamptz,
                 CASE WHEN g%10=0 THEN '2026-01-01'::timestamptz END
             FROM generate_series(1,1000000) AS g",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("ANALYZE machines")
            .execute(&mut *conn)
            .await
            .unwrap();
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM machines")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(stored, 1_000_000);
        let ids: Vec<Id> = sqlx::query_scalar(
            "SELECT md5('feed-scale-'||g)::uuid FROM generate_series(1,1000) AS g",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        let expected: HashSet<Id> = sqlx::query_scalar(
            "SELECT md5('feed-scale-'||g)::uuid FROM generate_series(1,1000) AS g WHERE g%10=0",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap()
        .into_iter()
        .collect();
        let feeds: Vec<_> = ids
            .iter()
            .map(|id| Feed::Resource {
                kind: ResourceKind::Machine,
                id: *id,
            })
            .collect();
        let pool = Db::from_pool(db.pool.clone());
        let batches = deleted_batches(&pool, &feeds);
        tokio::pin!(batches);
        let mut reads = 0;
        let mut deleted = HashSet::new();
        let mut largest = 0;
        while let Some(batch) = batches.next().await {
            let batch = batch.unwrap();
            reads += 1;
            largest = largest.max(batch.len());
            for feed in batch {
                let Feed::Resource {
                    kind: ResourceKind::Machine,
                    id,
                } = feed
                else {
                    panic!("an unrelated feed was returned");
                };
                assert!(deleted.insert(id), "one watched identity appears only once");
            }
        }
        assert_eq!(reads, 1_000_usize.div_ceil(LIVENESS_BATCH));
        assert_eq!(
            deleted, expected,
            "only the 100 deleted sources close; 900 stay live"
        );
        assert!(largest <= LIVENESS_BATCH);
        let plan: Json = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {LIVE_MACHINES_SQL}"
        )))
        .bind(&ids[..LIVENESS_BATCH])
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let text = plan.to_string();
        assert!(text.contains("machines_pkey"), "{text}");
        assert!(!text.contains("Seq Scan"), "{text}");
        assert!(plan[0]["Plan"]["Actual Rows"].as_u64().unwrap() <= LIVENESS_BATCH as u64);
    }

    #[test]
    fn a_departed_watch_stays_for_ten_seconds_and_a_return_cancels_its_departure() {
        let feed = Feed::EnvProcs(orch_types::new_id());
        let mut coordinator = Coordinator::default();
        let now = Instant::now();
        coordinator.interests_at("one".to_owned(), vec![feed.clone()], now);
        assert!(coordinator.active_at(now).contains(&feed));
        coordinator.interests_at("one".to_owned(), Vec::new(), now);
        assert!(coordinator.active_at(now).contains(&feed));
        let returning = now + STOP_GRACE - TICK;
        assert!(coordinator.active_at(returning).contains(&feed));
        coordinator.interests_at("two".to_owned(), vec![feed.clone()], returning);
        assert!(coordinator.active_at(returning).contains(&feed));
        coordinator.interests_at("two".to_owned(), Vec::new(), returning);
        assert!(coordinator.active_at(returning).contains(&feed));
        assert!(
            !coordinator
                .active_at(returning + STOP_GRACE)
                .contains(&feed)
        );
    }

    #[test]
    fn ten_lists_in_one_second_leave_only_the_last_one_to_be_sent() {
        let feed = Feed::EnvDisplays(orch_types::new_id());
        let mut coordinator = Coordinator::default();
        for n in 0..10 {
            coordinator.queue(feed.clone(), serde_json::json!({"displays": [n]}), false);
        }
        assert_eq!(coordinator.pending.len(), 1);
        let pending = coordinator.pending.values().next().unwrap();
        assert_eq!(pending.data["displays"], serde_json::json!([9]));
        assert!(pending.due.duration_since(Instant::now()) <= LIST_GAP);
    }

    #[test]
    fn partial_directory_invalidation_retains_rows_but_cannot_reuse_a_snapshot_cursor() {
        let mut held = serde_json::json!({"as_of":"a","reset":true,"truncated":true,
            "observed_from":"before-a","revision":"frozen","next_cursor":"old","expires_at":"later","changed":[{"name":"first"}],"removed":[]});
        merge_directory(
            &mut held,
            serde_json::json!({"as_of":"b","invalidated":true,"truncated":true,"changed":[],"removed":[]}),
        );
        assert_eq!(held["changed"], serde_json::json!([{"name":"first"}]));
        assert_eq!(held["invalidated"], true);
        assert_eq!(held["truncated"], true);
        assert!(held["revision"].is_null());
        assert!(held["next_cursor"].is_null());
        assert!(held["expires_at"].is_null());
        assert!(held["observed_from"].is_null());
        let reset = serde_json::json!({"as_of":"c","reset":true,"truncated":false,"revision":"fresh",
            "observed_from":"before-c","next_cursor":null,"changed":[{"name":"current"}],"removed":[]});
        merge_directory(&mut held, reset.clone());
        assert_eq!(held, reset);
    }

    #[test]
    fn a_removed_file_cancels_an_earlier_change_and_a_recreated_file_cancels_its_removal() {
        let mut held =
            serde_json::json!({"as_of": "a", "changed": [{"name": "gone"}], "removed": ["back"]});
        merge_directory(
            &mut held,
            serde_json::json!({"as_of": "b", "changed": [{"name": "back"}], "removed": ["gone"]}),
        );
        assert_eq!(held["removed"], serde_json::json!(["gone"]));
        assert_eq!(held["changed"], serde_json::json!([{"name": "back"}]));
    }
}
