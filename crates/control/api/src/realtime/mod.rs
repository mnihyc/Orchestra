//! Realtime components.
//!
//! One socket per browser tab carries every live update. The server's side of
//! it is made of these parts, and [`Realtime`] is what the rest of the
//! application holds of them.
//!
//! - [`server`] is the socket itself: who is speaking, how long a token
//!   lasts, what is read from the browser and what is written to it.
//! - [`subscriptions`] decides what a connection may follow, and again every
//!   time the authorization epoch moves.
//! - [`sync`] makes the durable feeds true: rows by sequence from the
//!   database, a read for every announcement and a sweep for the ones that
//!   were lost, so that any number of API processes serve the same feeds.
//! - [`deltas`] is what waits for a connection, and the growing text of an
//!   item that is sent at once and merged for a reader that is behind.
//! - [`presence`] is who is looking.
//! - The feeds: [`threads`], [`tree`] (an organization's tree), [`topics`],
//!   [`user`], [`chats`], [`documents`] and [`environments`] read their rows;
//!   [`resources`] and [`watches`] decide transient subscriptions, while [`ephemeral`]
//!   shares their watches and carries their events across processes. [`terminals`]
//!   serves the separate terminal sockets.
//!
//! **How another part publishes to a feed.** Inside the transaction that
//! changes a thread's rows: `orch_db::sequences::next` for each change, the
//! number stamped on the row, and once before the commit
//! `orch_db::sequences::notify` with the last. A node or topic:
//! `orch_db::queries::hierarchy::stamp`, which announces. A person's inbox:
//! `orch_db::queries::notifications::people::announce`. News that is no row
//! (a favourite changed on one of a person's devices):
//! `orch_db::sequences::notify_user_event`. Nothing here is called
//! directly: the database delivers the announcement to every API process
//! when the transaction commits, and a sweep finds what an announcement did
//! not. Growing text is the one thing handed over in process, by the thread
//! engine, to [`Realtime::sink`].

pub mod background_watches;
pub mod browsers;
pub mod chats;
pub mod deltas;
pub mod displays;
pub(crate) mod document_tree;
pub mod documents;
pub mod environment_watches;
pub mod environments;
pub mod ephemeral;
#[cfg(test)]
mod epoch_handoff_tests;
#[cfg(test)]
pub(crate) mod history_read_tests;
mod machine_views;
mod personal_changes;
pub mod presence;
pub mod raw_traffic;
pub mod resource_changes;
mod resource_health;
pub(crate) mod resource_rows;
pub mod resources;
pub mod rfb_filter;
pub mod server;
pub mod subscriptions;
pub mod sync;
pub mod terminals;
pub mod threads;
pub mod topics;
pub mod tree;
pub mod usage;
pub mod user;
pub mod watches;

use crate::realtime::presence::Presence;
use crate::realtime::server::Connection;
use crate::realtime::subscriptions::Subscription;
use crate::state::{App, AppState};
use orch_config::ServerConfig;
use orch_db::{Db, DbError};
use orch_engine::Engine;
use orch_engine::live::{LiveDelta, LiveSink};
use orch_realtime_contract::delta::PERSIST_LAG_MILLIS;
use orch_realtime_contract::feeds::{Feed, UserEvent};
use orch_realtime_contract::version::{
    AUTH_TIMEOUT_SECS, HEARTBEAT_SECS, SILENCE_SECS, SWEEP_SECS,
};
use orch_types::{Clock, Id, Seq};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Where the browser's socket is, under the API's base path.
pub const SOCKET_ROUTE: &str = "/ws";

/// How soon the authorization epoch is looked at again when nobody announced
/// that it moved: the announcement of it is not sent by every writer yet, and
/// one that is lost must not leave somebody holding what they no longer may
/// for long. A second is a small share of the work of a server and shorter
/// than anyone waits for a revoked person's tab to stop.
pub const EPOCH_EVERY: Duration = Duration::from_secs(1);
/// The least time between two reads of one feed for its subscribers. The
/// first change is read at once; changes that follow within this are read
/// together when it is over. A text that grows is stored with every stretch,
/// each of which moves its row, and reading the row for every stretch would
/// send a reader its whole text again and again: a fifth of a second is a
/// delay no one sees in a row, and bounds a streaming message to five rows a
/// second for each reader, deltas being what carries the text between them.
pub const FANOUT_GAP: Duration = Duration::from_millis(200);
/// The least time between two reads of a thread's feed while text is growing in
/// it: the second the protocol allows a stored row to be behind what was
/// forwarded. A growing text is stored with every stretch, and a row that is
/// sent for each would send its reader the whole text again many times a second;
/// the deltas carry the text between the rows.
pub const STREAM_GAP: Duration = Duration::from_millis(PERSIST_LAG_MILLIS as u64);
/// For how long after a stretch of text was handed over a thread's text is still
/// taken to be growing: twice the time a row may be behind, so that one pause in
/// a stream does not end it.
pub const STREAMING_WINDOW: Duration = Duration::from_millis(2 * PERSIST_LAG_MILLIS as u64);

/// How often a connection looks at its own clocks: how long it heard
/// nothing, how long its token is good, how much waits for it. A second is
/// finer than any of the limits it keeps, which are counted in tens of seconds.
pub const TICK: Duration = Duration::from_secs(1);
/// How long a connection that is to be closed is given to take the frame that
/// says why: a reader that is merely slow takes it in a moment, and one that
/// takes nothing is not waited for.
pub const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// The times and limits of the server, which a deployment sets (the two
/// buffer keys) or the protocol fixes, gathered so that a test can shorten
/// them.
#[derive(Debug, Clone)]
pub struct Tuning {
    /// How long a connection has to say who it is.
    pub auth_timeout: Duration,
    /// How long a connection may hear nothing before it is closed.
    pub silence: Duration,
    /// How often a client is told to ping.
    pub heartbeat_secs: u32,
    /// How many bytes may wait for one connection.
    pub buffer_max_bytes: i64,
    /// For how many milliseconds that many may have waited.
    pub buffer_max_millis: i64,
    /// How often subscribed feeds are compared with the database; zero disables automatic repair sweeps.
    pub sweep_every: Duration,
    /// How often the authorization epoch is read when nobody announced it; zero disables this poll.
    pub epoch_every: Duration,
    /// The least time between two reads of one feed.
    pub fanout_gap: Duration,
    /// The least time between two reads of a thread's feed while text grows in it.
    pub stream_gap: Duration,
    /// How often a connection looks at its own clocks.
    pub tick: Duration,
    /// How long a closing connection is given to take its last frame.
    pub close_grace: Duration,
}

impl Tuning {
    /// What the protocol fixes, and what the deployment's `[realtime]` keys
    /// say.
    #[must_use]
    pub fn from_config(config: &ServerConfig) -> Self {
        Self {
            auth_timeout: Duration::from_secs(u64::from(AUTH_TIMEOUT_SECS)),
            silence: Duration::from_secs(u64::from(SILENCE_SECS)),
            heartbeat_secs: HEARTBEAT_SECS,
            buffer_max_bytes: i64::try_from(config.realtime.buffer_max_bytes).unwrap_or(i64::MAX),
            buffer_max_millis: i64::try_from(config.realtime.buffer_max_secs.saturating_mul(1_000))
                .unwrap_or(i64::MAX),
            sweep_every: Duration::from_secs(u64::from(SWEEP_SECS)),
            epoch_every: EPOCH_EVERY,
            fanout_gap: FANOUT_GAP,
            stream_gap: STREAM_GAP,
            tick: TICK,
            close_grace: CLOSE_GRACE,
        }
    }
}

/// Why something the socket was doing could not be done: the database failed,
/// or the access engine could not answer, or something that has to be there
/// was not. The connection that met it is closed, and its client connects
/// again.
#[derive(Debug, thiserror::Error)]
pub enum Fail {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The access engine could not answer.
    #[error(transparent)]
    Authz(#[from] orch_authz::Error),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

impl Fail {
    /// A failure with `cause` as its words.
    pub fn other(cause: impl std::fmt::Display) -> Self {
        Self::Other(cause.to_string())
    }
}

/// Whose sequence counts the rows of a feed: what the database stamps and
/// announces. Several feeds can be of one source (a thread's feed and the
/// feeds of what it runs on), and the feed of a person's own is of that
/// person's inbox, which is why a feed is not a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// A thread.
    Thread(Id),
    /// A topic.
    Topic(Id),
    /// The tree of a root organization.
    Tree(Id),
    /// The inbox of a person.
    Inbox(Id),
    /// A chat.
    Chat(Id),
    /// A document.
    Document(Id),
    /// An environment.
    Environment(Id),
}

impl Source {
    /// The source of `feed` for the person `user`; none for a feed that has
    /// no sequence, or that this server does not serve.
    #[must_use]
    pub fn of(feed: &Feed, user: Id) -> Option<Self> {
        match feed {
            Feed::Thread(id) => Some(Self::Thread(*id)),
            Feed::Topic(id) => Some(Self::Topic(*id)),
            Feed::Org(id) => Some(Self::Tree(*id)),
            Feed::User => Some(Self::Inbox(user)),
            Feed::Chat(id) => Some(Self::Chat(*id)),
            Feed::Doc(id) => Some(Self::Document(*id)),
            Feed::Env(id) => Some(Self::Environment(*id)),
            _ => None,
        }
    }
}

/// The subscriptions to one source, and where the work of reading for them
/// stands.
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) subs: Vec<Arc<Subscription>>,
    /// The highest number that was announced, or found by a sweep.
    pub(crate) notified: Seq,
    /// Whether a task is reading for these subscriptions.
    pub(crate) running: bool,
    /// Whether something was announced since that task last began.
    pub(crate) dirty: bool,
    /// When a task last began to read for these subscriptions: the next
    /// begins no sooner than the gap after it, whichever task it is.
    pub(crate) last: Option<tokio::time::Instant>,
    /// When text was last handed over for a thread.
    pub(crate) streaming: Option<tokio::time::Instant>,
    /// Growing text handed over and not yet passed on to the subscribers.
    pub(crate) waiting: Vec<LiveDelta>,
    /// Whether a task is passing it on.
    pub(crate) carrying: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    pub(crate) connections: HashMap<u64, Arc<Connection>>,
    pub(crate) sources: HashMap<Source, Entry>,
}

/// What every part of the server shares.
pub(crate) struct Inner {
    pub(crate) db: Db,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) tuning: Tuning,
    /// What this process is called to the others, on the presence channel.
    pub(crate) process: String,
    pub(crate) presence: Presence,
    /// The authorization epoch as last read: connections wake when it moves.
    pub(crate) epoch: watch::Sender<i64>,
    pub(crate) stop: CancellationToken,
    pub(crate) tasks: TaskTracker,
    pub(crate) ephemeral: ephemeral::Feeds,
    engine: OnceLock<Engine>,
    registry: Mutex<Registry>,
    numbers: AtomicU64,
    /// Whether the topics are being compared with the database.
    pub(crate) looking: AtomicBool,
    /// Whether the listening connection stands.
    pub(crate) listening: watch::Sender<bool>,
    /// The tickets given and the screens held for shells and the agent's own terminal.
    pub(crate) terminals: terminals::Sockets,
    /// Browser tickets and shared live streams.
    pub(crate) browsers: browsers::Sockets,
    /// Display tickets and independently authenticated viewer streams.
    pub(crate) displays: displays::Sockets,
    /// The application this server is part of, which the socket hands over when it serves its
    /// first connection: what reads an environment's state is the request's projection, and that
    /// needs it. Held weakly, since the application holds this server.
    pub(crate) app: OnceLock<Weak<AppState>>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner").finish_non_exhaustive()
    }
}

impl Inner {
    pub(crate) fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A number that no other call of this process has had.
    pub(crate) fn number(&self) -> u64 {
        self.numbers.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub(crate) fn engine(&self) -> Result<&Engine, Fail> {
        self.engine
            .get()
            .ok_or_else(|| Fail::other("the realtime server was not given the thread engine"))
    }

    /// The application, while it stands and once the socket has handed it over.
    pub(crate) fn app(&self) -> Option<App> {
        self.app.get().and_then(Weak::upgrade)
    }

    /// The subscriptions to `source`, as they are now.
    pub(crate) fn subscribers_of(&self, source: Source) -> Vec<Arc<Subscription>> {
        self.registry()
            .sources
            .get(&source)
            .map(|entry| entry.subs.clone())
            .unwrap_or_default()
    }
}

/// The realtime server of one API process, which the application holds.
///
/// Making one costs nothing: it listens, sweeps and serves only once
/// [`Realtime::start`] has been called.
#[derive(Clone)]
pub struct Realtime {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Realtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Realtime").finish_non_exhaustive()
    }
}

impl Realtime {
    /// Compares subscribed feeds with committed rows once, independently of the periodic timer.
    pub async fn sweep(&self) -> Result<(), Fail> {
        self.inner.sweep_once().await
    }

    /// The tickets and live streams of environment browsers.
    #[must_use]
    pub fn browsers(&self) -> &browsers::Sockets {
        &self.inner.browsers
    }
    /// Tickets, live viewers and safe sittings of environment displays.
    pub fn displays(&self) -> &displays::Sockets {
        &self.inner.displays
    }

    /// A server over the database, with the times and limits the deployment's
    /// configuration gives.
    #[must_use]
    pub fn new(db: &Db, clock: &Arc<dyn Clock>, config: &ServerConfig) -> Self {
        Self::tuned(db, clock, Tuning::from_config(config))
    }

    /// A server with times and limits of its own.
    #[must_use]
    pub fn tuned(db: &Db, clock: &Arc<dyn Clock>, tuning: Tuning) -> Self {
        let (epoch, _) = watch::channel(0);
        Self {
            inner: Arc::new(Inner {
                db: db.clone(),
                clock: Arc::clone(clock),
                tuning,
                process: orch_types::new_id().simple().to_string(),
                presence: Presence::new(),
                epoch,
                stop: CancellationToken::new(),
                tasks: TaskTracker::new(),
                ephemeral: ephemeral::Feeds::default(),
                engine: OnceLock::new(),
                registry: Mutex::new(Registry::default()),
                numbers: AtomicU64::new(0),
                looking: AtomicBool::new(false),
                listening: watch::channel(false).0,
                terminals: terminals::Sockets::default(),
                browsers: browsers::Sockets::default(),
                displays: displays::Sockets::default(),
                app: OnceLock::new(),
            }),
        }
    }

    /// Hands the server the application it is part of; the socket does this when it serves a
    /// connection, and a second call changes nothing.
    pub fn attach_app(&self, app: &App) {
        let _ = self.inner.app.set(Arc::downgrade(app));
    }

    /// Where the thread engine hands growing text to. Give it to the engine
    /// when it is made.
    #[must_use]
    pub fn sink(&self) -> Arc<dyn LiveSink> {
        Arc::new(deltas::Sink::new(self.clone()))
    }

    /// Begins listening to the database and sweeping what it announced
    /// without being heard. `engine` is read for the rows of threads. Called
    /// once, inside a runtime; asked again it does nothing.
    pub fn start(&self, engine: Engine) {
        if self.inner.engine.set(engine).is_err() {
            return;
        }
        sync::start(&self.inner);
        ephemeral::start(&self.inner);
        terminals::start(&self.inner);
    }

    /// Waits until the connection that listens to the database stands, which
    /// it does a moment after [`Realtime::start`]. What was announced before
    /// that is found by the sweep; this is for whoever wants the first
    /// announcement not to depend on it.
    pub async fn listening(&self) {
        let mut standing = self.inner.listening.subscribe();
        let _ = standing.wait_for(|standing| *standing).await;
    }

    /// Closes every connection with the code of a server going away, and
    /// stops what [`Realtime::start`] began. The connections are given a
    /// moment to take that word.
    pub async fn stop(&self) {
        self.inner.stop.cancel();
        terminals::stop(&self.inner);
        let connections: Vec<Arc<Connection>> = self
            .inner
            .registry()
            .connections
            .values()
            .cloned()
            .collect();
        for connection in connections {
            connection.close(
                orch_realtime_contract::version::CloseCode::GoingAway,
                "the server is stopping",
            );
        }
        self.inner.tasks.close();
        let _ = tokio::time::timeout(self.inner.tuning.close_grace, self.inner.tasks.wait()).await;
    }

    /// Tells `users` that something changed for them that is no row, on every
    /// connection they hold on every API process, through the database.
    pub fn tell_users(&self, users: &[Id], event: UserEvent) {
        let inner = Arc::clone(&self.inner);
        let users = users.to_vec();
        self.inner.tasks.spawn(async move {
            let Ok(news) = serde_json::to_value(&event) else {
                return;
            };
            let mut conn = match inner.db.acquire().await {
                Ok(conn) => conn,
                Err(error) => {
                    tracing::warn!(%error, "news for a person could not be sent");
                    return;
                }
            };
            if let Err(error) =
                orch_db::sequences::notify_user_event(&mut conn, &users, &news).await
            {
                tracing::warn!(%error, "news for a person could not be sent");
            }
        });
    }

    /// The tickets and the screens of the terminal sockets of this process.
    #[must_use]
    pub fn terminals(&self) -> &terminals::Sockets {
        &self.inner.terminals
    }

    /// How many connections are open on this process.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.inner.registry().connections.len()
    }

    /// How many subscriptions are open on this process.
    #[must_use]
    pub fn subscriptions(&self) -> usize {
        self.inner
            .registry()
            .sources
            .values()
            .map(|entry| entry.subs.len())
            .sum::<usize>()
            + self.inner.ephemeral.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn every_feed_with_a_sequence_has_a_source_and_the_users_own_feed_is_theirs_alone() {
        let (id, ada, bo) = (new_id(), new_id(), new_id());
        assert_eq!(Source::of(&Feed::Thread(id), ada), Some(Source::Thread(id)));
        assert_eq!(Source::of(&Feed::Topic(id), ada), Some(Source::Topic(id)));
        assert_eq!(Source::of(&Feed::Org(id), ada), Some(Source::Tree(id)));
        assert_eq!(Source::of(&Feed::User, ada), Some(Source::Inbox(ada)));
        assert_eq!(Source::of(&Feed::Chat(id), ada), Some(Source::Chat(id)));
        assert_eq!(Source::of(&Feed::Doc(id), ada), Some(Source::Document(id)));
        assert_eq!(
            Source::of(&Feed::Env(id), ada),
            Some(Source::Environment(id))
        );
        assert_ne!(Source::of(&Feed::User, ada), Source::of(&Feed::User, bo));
    }

    #[test]
    fn a_feed_without_a_sequence_or_without_data_here_has_no_source() {
        let id = new_id();
        for feed in [Feed::ThreadRaw(id), Feed::EnvProcs(id), Feed::EnvStats(id)] {
            assert_eq!(Source::of(&feed, new_id()), None, "{feed}");
        }
    }

    #[test]
    fn the_times_a_deployment_does_not_set_are_the_protocols_and_the_two_buffer_keys_are_the_deployments()
     {
        let mut config = ServerConfig::default();
        config.realtime.buffer_max_bytes = 1_000;
        config.realtime.buffer_max_secs = 5;
        let tuning = Tuning::from_config(&config);
        assert_eq!(tuning.buffer_max_bytes, 1_000);
        assert_eq!(tuning.buffer_max_millis, 5_000);
        assert_eq!(tuning.auth_timeout, Duration::from_secs(10));
        assert_eq!(tuning.silence, Duration::from_secs(60));
        assert_eq!(tuning.heartbeat_secs, 25);
        assert_eq!(tuning.sweep_every, Duration::from_secs(5));
        assert_eq!(tuning.stream_gap, Duration::from_secs(1));
    }
}
