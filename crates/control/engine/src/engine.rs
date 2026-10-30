//! The engine: what it is made of, how it is started, and how work is found.
//!
//! An [`Engine`] is cheap to clone and holds the handles the requests of
//! every other module use. It keeps nothing in memory that a restart would
//! lose: the few things it does keep are hints (which threads somebody
//! thinks have work, which sessions it opened itself) and a missing hint is
//! made up for by the supervisor, which looks at the database again and
//! again. A process that is built anew on the same database goes on where
//! the last one stopped.

use crate::access::ResourceAccess;
use crate::commands::Commands;
use crate::config::EngineConfig;
use crate::error::EngineError;
use crate::harnesses::OpenSessions;
use crate::hosting::HostServices;
use crate::live::LiveSink;
use crate::tools::definitions::CallRate;
use crate::tools::manage::Plans;
use chrono::{DateTime, Utc};
use orch_blob_contract::store::BlobStore;
use orch_db::Db;
use orch_db::queries::threads;
use orch_types::{Clock, Id};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use tokio::sync::Notify;
use tokio::time::Instant;

/// What an engine is made from.
pub struct EngineParts {
    /// The database.
    pub db: Db,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// The waits of the engine.
    pub config: EngineConfig,
    /// The machine link, as far as commands go.
    pub commands: Arc<dyn Commands>,
    /// The resource side of the access engine.
    pub access: Arc<dyn ResourceAccess>,
    /// Where growing text is forwarded.
    pub live: Arc<dyn LiveSink>,
}

impl std::fmt::Debug for EngineParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineParts").finish_non_exhaustive()
    }
}

/// The shared state of an engine.
pub struct Inner {
    pub(crate) db: Db,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) config: EngineConfig,
    pub(crate) commands: Arc<dyn Commands>,
    pub(crate) access: Arc<dyn ResourceAccess>,
    pub(crate) live: Arc<dyn LiveSink>,
    pub(crate) sessions: Mutex<OpenSessions>,
    /// The threads whose follow-up work is being carried out, and whether
    /// more was asked for meanwhile.
    pub(crate) flights: Mutex<HashMap<Id, bool>>,
    /// The threads somebody thinks have work, and when that was thought.
    pub(crate) wakes: Mutex<HashMap<Id, Instant>>,
    /// The last active goal examined by the supervisor's bounded round-robin pass.
    pub(crate) goal_cursor: Mutex<Option<Id>>,
    pub(crate) signal: Notify,
    /// Tells the task that hands woken threads to their work ([`Engine::deliver_wakes`]) that one was woken.
    pub(crate) woken: Notify,
    pub(crate) stopped: AtomicBool,
    pub(crate) started: AtomicBool,
    /// When the indexer of search by meaning last began a turn here, whether one runs, and how many seconds the
    /// system's tuning puts between turns (as last read; the shipped default until then).
    pub(crate) indexed: Mutex<Option<Instant>>,
    pub(crate) indexing: AtomicBool,
    pub(crate) index_every: AtomicU64,
    /// What making a unit asks of the program the engine lives in, once that has been attached.
    pub(crate) host_services: RwLock<Option<Arc<dyn HostServices>>>,
    /// What answers the four port tools, once the program the engine lives in has attached it.
    pub(crate) port_tools: RwLock<Option<Arc<dyn crate::tools::ports::PortTools>>>,
    /// What keeps the files of copies shared with agents, once that has been attached.
    pub(crate) parcels: RwLock<Option<Arc<dyn crate::sharing::Parcels>>>,
    /// What writes the forms of a single export that need the API's views, once that has been attached (the
    /// export of a place).
    pub(crate) forms: RwLock<Option<Arc<dyn crate::exports::Forms>>>,
    /// When the schedule of environments last looked at what is due.
    pub(crate) reaped: Mutex<Option<Instant>>,
    /// When this process last examined age-based node retention.
    pub(crate) retained: Mutex<Option<Instant>>,
    /// Coalesced native cache work, independent of the supervisor's machine checks.
    pub(crate) agent_history: Mutex<crate::agent_history::Memory>,
    /// The environments on people's machines that this process is attaching now.
    pub(crate) attaching: Mutex<HashSet<Id>>,
    /// Serializes live mount resolution and acknowledgments; inactive entries own no locks.
    pub(crate) mount_changes: Mutex<HashMap<Id, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    pub(crate) harness_starts:
        Mutex<HashMap<crate::harnesses::HarnessKey, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    /// The calls the agents of threads made of Orch's tools in the last minute.
    pub(crate) call_rate: Mutex<CallRate>,
    /// What the agents of threads were told `manage_shared` would do, and have not yet confirmed.
    pub(crate) manage_plans: Mutex<Plans>,
    /// When the usage caps of a thread's running turn were last looked at (a hint that thins the looks out).
    pub(crate) cap_checks: Mutex<HashMap<Id, DateTime<Utc>>>,
    /// The caps, and the person for one counted per person, whose running turns are being stopped.
    pub(crate) cap_stops: Mutex<HashSet<(Id, Option<Id>)>>,
    /// When room in a cap may have been made and the threads that wait for it were not yet looked at.
    pub(crate) room_made: Mutex<Option<DateTime<Utc>>>,
    /// Whether the threads that wait for room are being looked at now.
    pub(crate) releasing: AtomicBool,
    /// The cap that a thread was last found to wait for: what a pass that releases waiting threads reads
    /// right after it looked at one, so that no thread a full cap covers overtakes it.
    pub(crate) cap_waits: Mutex<HashMap<Id, Id>>,
    /// The store of content, once the program the engine lives in has attached it.
    pub(crate) blobs: OnceLock<Arc<dyn BlobStore>>,
    /// The idle environments that this process is stopping now.
    pub(crate) stopping: Mutex<HashSet<Id>>,
    /// Current browser profiles, counts and the API's volatile transport.
    pub(crate) browser_memory: Arc<crate::browsers::Memory>,
    /// Display sealing and transient-list hooks supplied by the API.
    pub(crate) display_memory: Arc<crate::displays::Memory>,
    /// Native command observations and reviews are strictly volatile.
    pub(crate) background: crate::background::Memory,
    // The part that forks and switches: what writes the text of a summary handoff, once something
    // better than the transcript has been attached.
    pub(crate) handoff: RwLock<Option<Arc<dyn crate::fork_history::Handoff>>>,
    /// Additional summary producers; the built-in model is constructed per call with its host port.
    pub(crate) summarizers:
        RwLock<HashMap<String, Arc<dyn orch_summarizer_contract::summarizer::Summarizer>>>,
    /// What this process remembers of the credentials that left their providers (the part that keeps
    /// providers): the copies issued under each lease, and the returns under way.
    pub(crate) credentials: Mutex<crate::credentials::Issuing>,
    /// The pieces of growing text that are not written to their items yet ([`crate::streaming`]).
    pub(crate) growing: crate::streaming::Growing,
}

/// The thread engine.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Engine {
    /// An engine over `parts`. Nothing runs until [`Engine::start`].
    #[must_use]
    pub fn new(parts: EngineParts) -> Self {
        let EngineParts {
            db,
            clock,
            config,
            commands,
            access,
            live,
        } = parts;
        let plans_kept = config.manage_plan_kept;
        Self {
            inner: Arc::new(Inner {
                db,
                clock,
                config,
                commands,
                access,
                live,
                sessions: Mutex::new(OpenSessions::default()),
                flights: Mutex::new(HashMap::new()),
                wakes: Mutex::new(HashMap::new()),
                goal_cursor: Mutex::new(None),
                signal: Notify::new(),
                woken: Notify::new(),
                stopped: AtomicBool::new(false),
                started: AtomicBool::new(false),
                indexed: Mutex::new(None),
                indexing: AtomicBool::new(false),
                index_every: AtomicU64::new(INDEX_EVERY_SECS),
                host_services: RwLock::new(None),
                port_tools: RwLock::new(None),
                parcels: RwLock::new(None),
                forms: RwLock::new(None),
                reaped: Mutex::new(None),
                retained: Mutex::new(None),
                agent_history: Mutex::new(crate::agent_history::Memory::default()),
                attaching: Mutex::new(HashSet::new()),
                mount_changes: Mutex::new(HashMap::new()),
                harness_starts: Mutex::new(HashMap::new()),
                call_rate: Mutex::new(CallRate::default()),
                manage_plans: Mutex::new(Plans::new(plans_kept)),
                cap_checks: Mutex::new(HashMap::new()),
                cap_stops: Mutex::new(HashSet::new()),
                room_made: Mutex::new(None),
                releasing: AtomicBool::new(false),
                cap_waits: Mutex::new(HashMap::new()),
                growing: crate::streaming::Growing::default(),
                blobs: OnceLock::new(),
                stopping: Mutex::new(HashSet::new()),
                browser_memory: Arc::new(crate::browsers::Memory::default()),
                display_memory: Arc::new(crate::displays::Memory::default()),
                background: crate::background::Memory::default(),
                handoff: RwLock::new(None),
                summarizers: RwLock::new(HashMap::new()),
                credentials: Mutex::new(crate::credentials::Issuing::default()),
            }),
        }
    }

    /// Attaches the store of content: what the engine reads from it (the list of a snapshot) and
    /// writes to it (the list of files to be written into a workspace). Attached once; asked again
    /// it does nothing.
    pub fn attach_blobs(&self, blobs: Arc<dyn BlobStore>) {
        let _ = self.inner.blobs.set(blobs);
    }

    /// The store of content, if it was attached.
    #[must_use]
    pub fn blobs(&self) -> Option<Arc<dyn BlobStore>> {
        self.inner.blobs.get().cloned()
    }

    /// The database.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// The waits of the engine.
    #[must_use]
    pub fn config(&self) -> &EngineConfig {
        &self.inner.config
    }

    /// Attaches what making a unit asks of the program the engine lives in: the token its connector
    /// enrolls with, the address it dials and the programs it runs. What was attached before is
    /// replaced, by a program that knows better than the default it started with.
    pub fn attach_host_services(&self, services: Arc<dyn HostServices>) {
        *self
            .inner
            .host_services
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(services);
    }

    /// What making a unit asks of the program the engine lives in, if it was attached.
    #[must_use]
    pub fn host_services(&self) -> Option<Arc<dyn HostServices>> {
        self.inner
            .host_services
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether [`Engine::stop`] was called.
    pub(crate) fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    /// Stops what [`Engine::start`] started. Work in hand is left where it
    /// is in the database, for the next process to take up.
    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        self.stop_agent_history();
        self.inner.signal.notify_waiters();
        self.inner.woken.notify_waiters();
    }

    /// Asks that the work of `thread` be looked at soon, after the
    /// transaction that asked has had time to commit. A hint: the supervisor
    /// finds the same work by itself. The woken thread is handed to its work
    /// by the engine's own task for wakes, not by a sweep of everything else.
    pub fn wake(&self, thread: Id) {
        lock(&self.inner.wakes).insert(thread, Instant::now());
        self.inner.woken.notify_one();
    }
}

// ---------------------------------------------------------------- supervision

/// The seconds between turns of the indexer before the system's `search.semantic.tuning` is read: its shipped
/// `interval_secs`.
const INDEX_EVERY_SECS: u64 = 10;

impl Engine {
    /// Starts what runs beside requests: the supervisor, which looks at the
    /// database again and again for work that is due and carries it out. A
    /// process that is started on a database that another process left in
    /// the middle of its work finds that work by the same looking. Called
    /// once; asked again it does nothing.
    pub fn start(&self) {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move { engine.supervise().await });
        let engine = self.clone();
        tokio::spawn(async move { engine.deliver_wakes().await });
        let engine = self.clone();
        tokio::spawn(async move { engine.write_held_text().await });
        let engine = self.clone();
        tokio::spawn(async move {
            if let Err(error) = engine.repair_context_search().await {
                tracing::warn!(%error, "legacy conversation search text awaits repair; injected context stays excluded");
            }
        });
    }

    /// Hands every woken thread to its work ([`Engine::dispatch`]) once its wake is a grace old (so that the
    /// transaction that asked has committed), by itself and never behind a sweep: under load one sweep of
    /// everything else takes seconds, and every step of every turn waited for the next one (C-turns), while
    /// every wake also started a whole sweep. The supervisor's sweep, once a second, still finds the same
    /// work, and a thread has one task of work at a time whoever asks.
    async fn deliver_wakes(&self) {
        let grace = self.inner.config.post_commit_grace;
        while !self.is_stopped() {
            let next = lock(&self.inner.wakes)
                .values()
                .map(|at| (*at + grace).saturating_duration_since(Instant::now()))
                .min();
            match next {
                None => self.inner.woken.notified().await,
                Some(wait) if !wait.is_zero() => {
                    tokio::select! {
                        () = self.inner.woken.notified() => {}
                        () = tokio::time::sleep(wait) => {}
                    }
                }
                Some(_) => {}
            }
            let ripe: Vec<Id> = {
                let mut wakes = lock(&self.inner.wakes);
                let ripe: Vec<Id> = wakes
                    .iter()
                    .filter(|(_, at)| at.elapsed() >= grace)
                    .map(|(thread, _)| *thread)
                    .collect();
                for thread in &ripe {
                    wakes.remove(thread);
                }
                ripe
            };
            for thread in ripe {
                if self.is_stopped() {
                    return;
                }
                self.dispatch(thread);
            }
        }
    }

    async fn supervise(&self) {
        if let Err(error) = self.reconcile_startup().await {
            tracing::warn!(%error, "startup supervision could not be restored; ordinary sweeps continue recovery");
        }
        let mut last_recheck: Option<Instant> = None;
        while !self.is_stopped() {
            let recheck =
                last_recheck.is_none_or(|at| at.elapsed() >= self.inner.config.recheck_every);
            if recheck {
                last_recheck = Some(Instant::now());
            }
            if let Err(error) = self.sweep(recheck).await {
                tracing::warn!(%error, "a sweep of the engine failed; the next looks again");
            }
            let pause = if lock(&self.inner.wakes).is_empty() {
                self.inner.config.sweep_every
            } else {
                self.inner.config.post_commit_grace
            };
            tokio::select! {
                () = self.inner.signal.notified() => {}
                () = tokio::time::sleep(pause) => {}
            }
            // A wake is held back for as long as the transaction behind it may
            // take to commit.
            if !lock(&self.inner.wakes).is_empty() {
                tokio::time::sleep(self.inner.config.post_commit_grace).await;
            }
        }
    }

    /// One look at everything that may be due.
    async fn sweep(&self, recheck: bool) -> Result<(), EngineError> {
        let grace = self.inner.config.post_commit_grace;
        let ripe: Vec<Id> = {
            let mut wakes = lock(&self.inner.wakes);
            let ripe: Vec<Id> = wakes
                .iter()
                .filter(|(_, at)| at.elapsed() >= grace)
                .map(|(thread, _)| *thread)
                .collect();
            for thread in &ripe {
                wakes.remove(thread);
            }
            ripe
        };
        let batch = self.inner.config.sweep_batch;
        if let Err(error) = crate::tools::source::reap(self, None).await {
            tracing::warn!(%error, "admitted own-tool native commands await their durable outcome");
        }
        if crate::side_threads::history::reap_closed(self)
            .await
            .is_err()
        {
            tracing::warn!(
                "closed side history inputs await their original native completion receipt"
            );
        }
        if let Err(error) = self.sweep_running_requests(batch).await {
            tracing::warn!(%error,"abandoned model request observations await the next sweep");
        }
        if let Err(error) = self.reconcile_networks().await {
            tracing::warn!(%error, "runtime network policies could not be reconciled; the next sweep looks again");
        }
        // Providers, and the leases that no turn holds, come before the threads they decide.
        self.keep_providers().await;
        // Environments: the jobs that make and remove them, and what is due for removal or stopping.
        self.keep_environments().await;
        if let Err(error) = self.keep_live_mount_facts().await {
            tracing::warn!(%error,"completed mount facts could not be refreshed; the next sweep looks again");
        }
        if let Err(error) = self.keep_call_recoveries().await {
            tracing::warn!(%error,"model call recoveries await the next sweep");
        }
        // Summaries claim their own work and await providers in independent tasks.
        if let Err(error) = self.keep_summaries().await {
            tracing::warn!(%error, "summary triggers could not be read; the next sweep looks again");
        }
        // Search by meaning indexes in a task of its own, one turn at a time, never holding the sweep.
        self.keep_indexing_due();
        let mut due: Vec<Id> = ripe;
        {
            let mut conn = self.inner.db.acquire().await?;
            due.extend(
                threads::threads_in_state(&mut conn, threads::thread_state::STARTING, batch)
                    .await?,
            );
            due.extend(
                threads::delivering_inputs_of_running_turns(&mut conn, batch)
                    .await?
                    .into_iter()
                    .map(|input| input.thread_id),
            );
            due.extend(
                threads::interrupted_active_turns(&mut conn, batch)
                    .await?
                    .into_iter()
                    .map(|turn| turn.thread_id),
            );
            if recheck {
                due.extend(threads::threads_with_waiting_inputs(&mut conn, batch).await?);
                let after = *lock(&self.inner.goal_cursor);
                let (goals, next) = orch_db::queries::goals::threads_with_goal_work(
                    &mut conn,
                    self.inner.clock.now(),
                    after,
                    batch,
                )
                .await?;
                *lock(&self.inner.goal_cursor) = next;
                due.extend(goals);
                due.extend(
                    threads::threads_blocked_by_their_own_cause(
                        &mut conn,
                        crate::preflight::CAUSES_OF_THE_THREAD,
                        batch,
                    )
                    .await?,
                );
            }
            // A turn's end that is still to be observed, after the commit that ended it.
            due.extend(orch_db::queries::source::threads_with_end_due(&mut conn, batch).await?);
            due.extend(threads::threads_with_turn_end_work(&mut conn, batch).await?);
        }
        due.extend(self.threads_with_decisions_to_deliver().await?);
        due.extend(self.threads_with_leases_to_close().await?);
        self.sync_presence(batch).await?;
        if let Err(error) = self.expire_capability_requests().await {
            tracing::warn!(%error, "requests for a capability could not be looked at; the next sweep looks again");
        }
        if let Err(error) = self.expire_chat_memberships().await {
            tracing::warn!(%error,"timed group memberships await the next sweep");
        }
        if let Err(error) = self.expire_access_requests().await {
            tracing::warn!(%error, "requests for access could not be looked at; the next sweep looks again");
        }
        if let Err(error) = self.reconcile_source_requests().await {
            tracing::warn!(%error,"source requests could not be reconciled; the next sweep tries again");
        }
        // The threads that wait for room in a usage cap are looked at together, the one whose oldest input is
        // oldest first, so that a cap behaves as a queue: when room is made, the first of them it fits starts.
        let queue = if recheck || self.take_room_made() {
            self.threads_waiting_for_room().await?
        } else {
            Vec::new()
        };
        due.sort_unstable();
        due.dedup();
        due.retain(|thread| !queue.contains(thread));
        for thread in due {
            self.dispatch(thread);
        }
        if !queue.is_empty() {
            let engine = self.clone();
            tokio::spawn(async move {
                if let Err(error) = engine.release_waiting_threads().await {
                    tracing::warn!(%error, "the threads that wait for room in a cap could not be looked at; the next sweep looks again");
                }
            });
        }
        Ok(())
    }

    /// Blocks the threads whose turn runs on a machine that went away, and
    /// puts back those whose machine came back, from what the database
    /// knows of presence.
    async fn sync_presence(&self, batch: i64) -> Result<(), EngineError> {
        let (away, back) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                threads::running_threads_on_offline_machines(&mut conn, batch).await?,
                threads::offline_threads_on_online_machines(&mut conn, batch).await?,
            )
        };
        for thread in away {
            self.block_as_offline(thread).await?;
        }
        for thread in back {
            self.restore_after_offline(thread).await?;
            self.wake(thread);
        }
        Ok(())
    }

    /// Begins a turn of the indexer of search by meaning when its interval has passed and none runs here.
    fn keep_indexing_due(&self) {
        let every =
            std::time::Duration::from_secs(self.inner.index_every.load(Ordering::Relaxed).max(1));
        {
            let mut last = lock(&self.inner.indexed);
            if last.is_some_and(|at: Instant| at.elapsed() < every) {
                return;
            }
            *last = Some(Instant::now());
        }
        if self.inner.indexing.swap(true, Ordering::SeqCst) {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            if let Err(error) = engine.keep_indexing().await {
                tracing::warn!(%error, "search by meaning could not look for work; its next turn looks again");
            }
            engine.inner.indexing.store(false, Ordering::SeqCst);
        });
    }

    /// The threads that have a forwarded lease closed whose provider has not been told, a page at a time through
    /// the window, however many leases closed in it.
    async fn threads_with_leases_to_close(&self) -> Result<Vec<Id>, EngineError> {
        let mut after = (
            crate::config::decisions_since(self.inner.clock.now()),
            Id::nil(),
        );
        let mut due = Vec::new();
        loop {
            let page = {
                let mut conn = self.inner.db.acquire().await?;
                threads::leases_closed_after(&mut conn, None, after, self.inner.config.sweep_batch)
                    .await?
            };
            let full =
                i64::try_from(page.len()).unwrap_or(i64::MAX) >= self.inner.config.sweep_batch;
            for lease in page {
                after = (lease.closed_at.unwrap_or(after.0), lease.id);
                if self.lease_needs_close(&lease).await? {
                    due.push(lease.thread_id);
                }
            }
            if !full {
                return Ok(due);
            }
        }
    }

    /// Carries out the outstanding work of a thread in a task of its own. Of
    /// two asks for one thread at once the second makes the first look again
    /// when it is done, so a thread has one such task at a time.
    pub(crate) fn dispatch(&self, thread: Id) {
        tracing::debug!(target:"orch_engine::terminal_work",%thread,phase="dispatch",outcome="enter",stopped=self.is_stopped());
        {
            let mut flights = lock(&self.inner.flights);
            if let Some(again) = flights.get_mut(&thread) {
                *again = true;
                return;
            }
            flights.insert(thread, false);
        }
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                // Named for the development guard, which reports a transaction held long or a
                // pooled connection asked for while this task holds one.
                if !engine.is_stopped()
                    && let Err(error) =
                        orch_db::pool::serving("thread work", Box::pin(engine.reconcile(thread)))
                            .await
                {
                    tracing::warn!(%error, %thread, "the work of a thread could not be carried out; it is looked at again");
                }
                let mut flights = lock(&engine.inner.flights);
                match flights.get_mut(&thread) {
                    Some(again) if *again && !engine.is_stopped() => *again = false,
                    _ => {
                        flights.remove(&thread);
                        break;
                    }
                }
            }
        });
    }

    /// The providers' state changed: the threads that were blocked for
    /// want of it are looked at.
    pub async fn reevaluate_provider(&self, provider: Id) -> Result<usize, EngineError> {
        let blocked = {
            let mut conn = self.inner.db.acquire().await?;
            threads::blocked_threads_of_provider(&mut conn, provider, None).await?
        };
        for thread in &blocked {
            self.wake(*thread);
        }
        Ok(blocked.len())
    }

    /// An environment's state changed: the threads that were blocked for
    /// want of it are looked at.
    pub async fn reevaluate_environment(&self, environment: Id) -> Result<usize, EngineError> {
        let blocked = {
            let mut conn = self.inner.db.acquire().await?;
            threads::blocked_threads_of_environment(&mut conn, environment).await?
        };
        for thread in &blocked {
            self.wake(*thread);
        }
        Ok(blocked.len())
    }
}
