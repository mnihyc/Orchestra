//! Offline manifests, per-environment eviction and blob garbage collection.
//!
//! **What the API remembers about environments.** A machine uploads a list of its tree (a
//! manifest) whenever it has changed; the API keeps the latest and answers from it when the machine
//! is away: a listing, the names of files, how old it all is. The list is parsed into a tree held in
//! memory for the environments used most lately ([`Cache::manifest_of`]). The API also remembers
//! on which session of which machine it asked for the measurements of an environment, so that it asks
//! once, and the measurements of the environments it watches, six hours of them, which are telemetry
//! and are kept nowhere else ([`Samples`]).
//!
//! **What it keeps tidy.** Two scheduled jobs run here, each under a lease of its own in the database
//! so that however many API processes there are, one runs them: the eviction of an environment's
//! cache when it holds more than its quota ([`evict_caches`]), least lately used first and never
//! what something points at; and the collection of content nothing points at once its grace has
//! run out ([`collect_garbage`]), together with the files of uploads that were given up.

use crate::backplane::{Link, Report};
use crate::blobs::{Blobs, BlobsError};
use crate::state::AppState;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use orch_api_contract::processes::ResourceSample;
use orch_config::ServerConfig;
use orch_db::queries::environments::workspace as environment_rows;
use orch_db::queries::job_leases;
use orch_db::queries::storage;
use orch_db::{Db, DbError};
use orch_settings::Place;
use orch_types::{Clock, Hash, Id};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub use orch_engine::manifest::{Entry, EntryKind, Header, Manifest, ManifestError};

/// The most bytes of a compressed list the API reads into memory: the same bound, since a list
/// compresses well and a bound on what it expands to is kept while it is read.
pub const MANIFEST_MAX_STORED_BYTES: i64 = 512 * 1024 * 1024;

/// How long an upload of a machine may have had no piece before it is taken for dead: a day, as
/// long as an upload session of a person lives.
pub const STALE_UPLOAD_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// How many rows a run of a job looks at in one statement.
const BATCH: i64 = 500;

/// How many batches a run of the collection takes up at most, so that one run never holds the
/// store for long: what is left is found by the next.
const MAX_BATCHES_PER_RUN: usize = 20;

/// How often the eviction of caches runs when nobody named another interval: every ten minutes.
pub const EVICTION_EVERY: Duration = Duration::from_secs(10 * 60);

/// How often the collection of content runs when nobody named another interval: every hour.
pub const COLLECTION_EVERY: Duration = Duration::from_secs(60 * 60);

/// The key of the eviction in `[schedule]` and the name of its lease.
pub const EVICTION_JOB: &str = "cache_eviction";

/// The key of the collection in `[schedule]` and the name of its lease.
pub const COLLECTION_JOB: &str = "blob_gc";

/// The most an environment may cache when nobody set a quota: twenty gibibytes, as shipped.
pub const DEFAULT_CACHE_QUOTA_BYTES: i64 = 20 * 1024 * 1024 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------- what the API remembers

/// An environment whose measurements a session of a machine was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watching {
    /// The machine.
    pub machine: Id,
    /// The session of the machine that was asked, where the link of this process knows of one.
    pub session: Option<Id>,
}

/// Measurements of environments, as they were sent: the last hours of each.
#[derive(Debug)]
pub struct Samples {
    rings: Mutex<HashMap<Id, VecDeque<ResourceSample>>>,
    capacity: usize,
}

impl Samples {
    fn new(capacity: usize) -> Self {
        Self {
            rings: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    /// Keeps a measurement of an environment; the oldest goes when there are more than are kept.
    pub fn push(&self, environment: Id, sample: ResourceSample) {
        let mut rings = lock(&self.rings);
        let ring = rings.entry(environment).or_default();
        if ring
            .back()
            .is_some_and(|last| last.at.get() > sample.at.get())
        {
            // A measurement that arrives late takes its place in the order of the times.
            let at = ring.partition_point(|each| each.at.get() <= sample.at.get());
            ring.insert(at, sample);
        } else {
            ring.push_back(sample);
        }
        while ring.len() > self.capacity {
            ring.pop_front();
        }
    }

    /// The measurements of an environment taken since `since`, oldest first.
    #[must_use]
    pub fn since(&self, environment: Id, since: DateTime<Utc>) -> Vec<ResourceSample> {
        lock(&self.rings)
            .get(&environment)
            .map(|ring| {
                ring.iter()
                    .filter(|sample| sample.at.get() >= since)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Forgets the measurements of an environment.
    pub fn forget(&self, environment: Id) {
        lock(&self.rings).remove(&environment);
    }
}

struct CacheInner {
    ports: crate::ports_runtime::PortRuntime,
    manifests: Mutex<VecDeque<(Id, Hash, Arc<Manifest>)>>,
    manifests_kept: usize,
    watching: Mutex<HashMap<Id, Watching>>,
    samples: Samples,
}

/// What the API remembers in memory about environments: parsed lists of trees, which environments
/// it attached where, and measurements. Cheap to clone; every clone is the same memory.
#[derive(Clone)]
pub struct Cache {
    inner: Arc<CacheInner>,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache").finish_non_exhaustive()
    }
}

impl Cache {
    /// Current port measurements and ephemeral admissions, never durable caller rows.
    #[must_use]
    pub fn ports(&self) -> &crate::ports_runtime::PortRuntime {
        &self.inner.ports
    }
    /// A memory sized by the deployment's configuration: the trees of the most lately used
    /// environments (`[limits] manifest_trees`) and the hours of measurements
    /// (`[engine] stats_history_hours`, one in `stats_interval_secs`).
    #[must_use]
    pub fn new(config: &ServerConfig) -> Self {
        let seconds = config
            .engine
            .stats_history_hours
            .saturating_mul(SECONDS_PER_HOUR);
        let samples = seconds / config.engine.stats_interval_secs.max(1);
        Self {
            inner: Arc::new(CacheInner {
                ports: crate::ports_runtime::PortRuntime::default(),
                manifests: Mutex::new(VecDeque::new()),
                manifests_kept: usize::try_from(config.limits.manifest_trees)
                    .unwrap_or(usize::MAX)
                    .max(1),
                watching: Mutex::new(HashMap::new()),
                samples: Samples::new(usize::try_from(samples).unwrap_or(usize::MAX).max(1)),
            }),
        }
    }

    /// A memory sized by the default configuration: for tests, and for a program that has none of
    /// its own.
    #[must_use]
    pub fn detached() -> Self {
        Self::new(&ServerConfig::default())
    }

    /// The measurements.
    #[must_use]
    pub fn samples(&self) -> &Samples {
        &self.inner.samples
    }

    /// The tree of the list `manifest` of the environment `environment`, parsed once for as long as
    /// the environment is among those used most lately.
    pub async fn manifest_of(
        &self,
        blobs: &Blobs,
        environment: Id,
        manifest: &Hash,
    ) -> Result<Arc<Manifest>, BlobsError> {
        {
            let mut kept = lock(&self.inner.manifests);
            if let Some(at) = kept
                .iter()
                .position(|(id, hash, _)| *id == environment && hash == manifest)
            {
                let found = kept.remove(at);
                if let Some(found) = found {
                    let tree = Arc::clone(&found.2);
                    kept.push_front(found);
                    return Ok(tree);
                }
            }
        }
        let bytes = blobs
            .read_whole(manifest, MANIFEST_MAX_STORED_BYTES)
            .await?;
        let tree = tokio::task::spawn_blocking(move || Manifest::parse(&bytes))
            .await
            .map_err(|error| BlobsError::Body(error.to_string()))??;
        let tree = Arc::new(tree);
        let mut kept = lock(&self.inner.manifests);
        kept.retain(|(id, _, _)| *id != environment);
        kept.push_front((environment, *manifest, Arc::clone(&tree)));
        kept.truncate(self.inner.manifests_kept);
        Ok(tree)
    }

    /// Where the measurements of the environment were asked for, as far as this process knows.
    #[must_use]
    pub fn watching(&self, environment: Id) -> Option<Watching> {
        lock(&self.inner.watching).get(&environment).copied()
    }

    /// Remembers that the measurements of the environment were asked for.
    pub fn watching_on(&self, environment: Id, on: Watching) {
        lock(&self.inner.watching).insert(environment, on);
    }

    /// Forgets that the measurements of the environment were asked for: the session is gone, or the
    /// machine would not.
    pub fn forget_watching(&self, environment: Id) {
        lock(&self.inner.watching).remove(&environment);
    }

    /// Hears one measurement a machine sent: the ones of an environment that the machine was asked
    /// to measure are kept, and no other (a machine does not speak for what is not its own).
    pub fn hear(&self, report: &Report) {
        self.inner.ports.hear(report);
        if report.eph.kind != STATS_SAMPLE {
            return;
        }
        let Some(environment) = report
            .eph
            .stream
            .strip_prefix(ENV_STREAM_PREFIX)
            .and_then(|id| id.parse::<Id>().ok())
        else {
            return;
        };
        if self
            .watching(environment)
            .is_none_or(|on| on.machine != report.machine_id)
        {
            return;
        }
        if let Ok(sample) = serde_json::from_value::<ResourceSample>(report.eph.body.clone()) {
            self.inner.samples.push(environment, sample);
        }
    }

    /// Starts listening for the measurements machines send, until the link is gone.
    pub fn collect_samples(&self, link: &Link) -> tokio::task::JoinHandle<()> {
        let mut reports = link.reports();
        let cache = self.clone();
        tokio::spawn(async move {
            loop {
                match reports.recv().await {
                    Ok(report) => cache.hear(&report),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    }
}

/// The kind of event a machine measures an environment with.
const STATS_SAMPLE: &str = "stats.sample";

/// How a stream of an environment begins.
const ENV_STREAM_PREFIX: &str = "env:";

/// Seconds in an hour.
const SECONDS_PER_HOUR: u64 = 3_600;

// ---------------------------------------------------------------- eviction

/// What one run of the eviction did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Evicted {
    /// How many environments were over their quota.
    pub environments: u64,
    /// How many cached versions of files were forgotten.
    pub versions: u64,
}

/// The quota of the cache of an environment: what the setting says at its home.
async fn quota_of(conn: &mut orch_db::Conn, environment: Id) -> Result<i64, DbError> {
    let Some(row) = environment_rows::environment(conn, environment).await? else {
        return Ok(DEFAULT_CACHE_QUOTA_BYTES);
    };
    Ok(orch_settings::get::<i64>(
        conn,
        Place::Node(row.home_node_id),
        "storage.env_cache_quota_bytes",
    )
    .await
    .ok()
    .flatten()
    .unwrap_or(DEFAULT_CACHE_QUOTA_BYTES))
}

/// Forgets cached versions of files, the ones whose content was asked for least lately first, in
/// each environment that has more cached than its quota allows, and never a version whose content
/// something points at. The content itself goes later, with the collection, once its grace is out.
pub async fn evict_caches(db: &Db, clock: &dyn Clock) -> Result<Evicted, DbError> {
    let mut conn = db.acquire().await?;
    let usage = storage::cache_usage(&mut conn).await?;
    let mut done = Evicted::default();
    for (environment, used) in usage {
        let quota = quota_of(&mut conn, environment).await?;
        if used <= quota {
            continue;
        }
        done.environments += 1;
        let mut excess = used - quota;
        while excess > 0 {
            let candidates = storage::eviction_candidates(&mut conn, environment, BATCH).await?;
            if candidates.is_empty() {
                break;
            }
            let mut progressed = false;
            for (file, size) in candidates {
                if excess <= 0 {
                    break;
                }
                if storage::evict_cached(&mut conn, environment, &file, clock.now()).await? {
                    done.versions += 1;
                    excess -= size;
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
            // Two versions with one content free its bytes once: measure again.
            let now_used = storage::cache_usage(&mut conn)
                .await?
                .into_iter()
                .find(|(id, _)| *id == environment)
                .map_or(0, |(_, bytes)| bytes);
            excess = now_used - quota;
        }
    }
    Ok(done)
}

// ---------------------------------------------------------------- collection

/// What one run of the collection did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Collected {
    /// How many blobs went: content nothing pointed at for as long as its grace.
    pub blobs: u64,
    /// How many bytes that was.
    pub bytes: i64,
    /// How many uploads that were given up went with the files of their pieces.
    pub uploads: u64,
}

/// Removes the content nothing points at, no cached version of a file names, and nobody asked for
/// during `[retention] blob_grace_hours`; and the uploads that were given up: the sessions of
/// people that ran out, and the uploads of machines that no piece came to for a day.
pub async fn collect_garbage(
    db: &Db,
    blobs: &Blobs,
    clock: &dyn Clock,
    config: &ServerConfig,
) -> Result<Collected, DbError> {
    let now = clock.now();
    let grace = ChronoDuration::hours(
        i64::try_from(config.retention.blob_grace_hours).unwrap_or(i64::MAX / 4),
    );
    let idle_since = now - grace;
    {
        let mut conn = db.acquire().await?;
        sqlx::query(
            "DELETE FROM thread_attachment_uploads u WHERE blob IS NOT NULL AND
          (expires_at < $1 OR (expires_at IS NULL AND NOT EXISTS(
            SELECT 1 FROM drafts d CROSS JOIN LATERAL jsonb_array_elements(d.content) part
            WHERE d.thread_id=u.thread_id AND d.user_id=u.created_by AND
              (part->>'path'=u.path OR (part->>'type'='image' AND part->>'name'=u.name)))))",
        )
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    let mut done = Collected::default();
    for _ in 0..MAX_BATCHES_PER_RUN {
        let mut conn = db.acquire().await?;
        let candidates = storage::unreferenced(&mut conn, idle_since, BATCH).await?;
        if candidates.is_empty() {
            break;
        }
        for (hash, size) in candidates {
            // The conditions are looked at again where the row is deleted: a pin or a use that
            // came meanwhile keeps the blob.
            if storage::forget_blob(&mut conn, &hash, idle_since).await? {
                if let Err(error) = blobs.store().delete(&hash).await {
                    tracing::warn!(%error, %hash, "a blob that nothing wanted could not be removed");
                }
                done.blobs += 1;
                done.bytes = done.bytes.saturating_add(size);
            }
        }
    }
    let mut conn = db.acquire().await?;
    for session in storage::expired_sessions(&mut conn, now, BATCH).await? {
        crate::blobs::discard(&blobs.part_of_session(session)).await;
        storage::end_session(&mut conn, session).await?;
        done.uploads += 1;
    }
    let silent_since = now - ChronoDuration::from_std(STALE_UPLOAD_AGE).unwrap_or_default();
    for hash in storage::stale_uploads(&mut conn, silent_since, BATCH).await? {
        crate::blobs::discard(&blobs.part_of(&hash)).await;
        storage::end_upload(&mut conn, &hash).await?;
        done.uploads += 1;
    }
    Ok(done)
}

// ---------------------------------------------------------------- the schedule

/// How long the lease of a job run here holds: a minute, renewed every [`LEASE_RENEWED_EVERY`] while
/// the run goes on, so that a process that died holds its job back for a minute at most.
const JOB_LEASE: ChronoDuration = ChronoDuration::minutes(1);

/// How often a run renews its lease: four times a lease, so that only a database that answers no
/// renewal for most of a minute lets another process start the job beside the run. Every job here
/// bears that: each looks again, where it removes something, at whether it may.
const LEASE_RENEWED_EVERY: Duration = Duration::from_secs(15);

/// Runs `step` under the lease of the job `job` in the database, so that of however many processes
/// there are, one runs it at a time. The lease is a row renewed while `step` runs, not a lock kept in
/// a transaction: no transaction stays open across the step's waits on the store of content or files.
pub(crate) async fn locked<F, Fut>(
    db: &Db,
    clock: &dyn Clock,
    job: &str,
    step: F,
) -> Result<(), DbError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), DbError>>,
{
    let lease = job_leases::Lease::for_run(job, JOB_LEASE);
    if !lease.hold(db, clock.now()).await? {
        return Ok(());
    }
    let step = step();
    tokio::pin!(step);
    let mut renewals = tokio::time::interval(LEASE_RENEWED_EVERY);
    renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    renewals.tick().await;
    let outcome = loop {
        tokio::select! {
            outcome = &mut step => break outcome,
            _ = renewals.tick() => {
                if !lease.hold(db, clock.now()).await? {
                    tracing::warn!(job, "another process took the lease of a job that was running; this run ends beside it");
                    break (&mut step).await;
                }
            }
        }
    };
    lease.release(db).await?;
    outcome
}

/// The interval a job runs at: the one the operator named for its key, never less than a second, or
/// the one it ships with.
pub(crate) fn interval_of(config: &ServerConfig, job: &str, shipped: Duration) -> Duration {
    config
        .schedule
        .get(job)
        .map_or(shipped, |seconds| Duration::from_secs((*seconds).max(1)))
}

/// Starts the two jobs of this module on their schedules, with the ports' measurements and the
/// documents' jobs, until `stop` is asked for.
pub fn start_housekeeping(
    app: &Arc<AppState>,
    stop: &CancellationToken,
) -> Vec<tokio::task::JoinHandle<()>> {
    let eviction = {
        let (app, stop) = (Arc::clone(app), stop.clone());
        tokio::spawn(async move {
            let every = interval_of(&app.config, EVICTION_JOB, EVICTION_EVERY);
            run_every(every, &stop, || async {
                locked(&app.db, app.clock.as_ref(), EVICTION_JOB, || async {
                    let evicted = evict_caches(&app.db, app.clock.as_ref()).await?;
                    if evicted.versions > 0 {
                        tracing::info!(?evicted, "the caches of environments were trimmed");
                    }
                    Ok(())
                })
                .await
            })
            .await;
        })
    };
    let collection = {
        let (app, stop) = (Arc::clone(app), stop.clone());
        tokio::spawn(async move {
            let every = interval_of(&app.config, COLLECTION_JOB, COLLECTION_EVERY);
            run_every(every, &stop, || async {
                locked(&app.db, app.clock.as_ref(), COLLECTION_JOB, || async {
                    let collected =
                        collect_garbage(&app.db, &app.blobs, app.clock.as_ref(), &app.config)
                            .await?;
                    if collected.blobs > 0 || collected.uploads > 0 {
                        tracing::info!(?collected, "content nobody wanted was removed");
                    }
                    Ok(())
                })
                .await
            })
            .await;
        })
    };
    let ports = crate::ports_runtime::start(app, stop);
    // The documents' versions and purge run on the same kind of schedule, beside these.
    let mut started = vec![eviction, collection, ports];
    started.extend(crate::documents_upkeep::start(app, stop));
    started.push(crate::input_schedules::start(app, stop));
    started
}

/// Runs `step` every `every`, until `stop`: a step that fails is logged and tried again at the next
/// turn, and one that takes longer than the interval is followed by the next at once.
pub(crate) async fn run_every<F, Fut>(every: Duration, stop: &CancellationToken, mut step: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), DbError>>,
{
    let mut ticks = tokio::time::interval(every);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = ticks.tick() => {}
        }
        if let Err(error) = step().await {
            tracing::warn!(%error, "a run of a scheduled job failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::{clock, database::TestDb};
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn idle_in_transaction(db: &Db) -> i64 {
        let mut conn = db.acquire().await.unwrap();
        sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database()
              AND pid <> pg_backend_pid() AND state LIKE 'idle in transaction%'",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_job_runs_in_one_process_at_a_time_under_its_lease_and_keeps_no_transaction_open() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let clock = clock::manual();
        let other = job_leases::Lease::new(EVICTION_JOB, "another process", JOB_LEASE);
        assert!(other.hold(&db, clock.now()).await.unwrap());
        let ran = AtomicBool::new(false);
        locked(&db, &clock, EVICTION_JOB, || async {
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert!(
            !ran.load(Ordering::SeqCst),
            "the other process runs the job"
        );
        other.release(&db).await.unwrap();
        locked(&db, &clock, EVICTION_JOB, || async {
            let mut conn = db.acquire().await?;
            let holder = job_leases::holder(&mut conn, EVICTION_JOB, clock.now()).await?;
            drop(conn);
            assert!(holder.is_some(), "the run holds the lease");
            assert_eq!(
                idle_in_transaction(&db).await,
                0,
                "and keeps no transaction open for it"
            );
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert!(ran.load(Ordering::SeqCst));
        let mut conn = db.acquire().await.unwrap();
        let holder = job_leases::holder(&mut conn, EVICTION_JOB, clock.now())
            .await
            .unwrap();
        assert_eq!(holder, None, "a run that ended gave its lease up");
    }
}
