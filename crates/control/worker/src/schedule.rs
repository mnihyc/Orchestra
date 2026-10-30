//! Worker-owned schedule and per-job database locks.
//!
//! Work that happens because time passed is split by one rule: the worker
//! needs only the database and the outside world, and what needs a machine or
//! the engine is the API's. These are the worker's jobs, each with the interval
//! it ships with (an operator may name another in `[schedule]`, by the job's
//! key, in seconds) and the lock it runs under.
//!
//! Each run of a job takes a lock named after the job, in the transaction of the
//! run ([`take`]), so that however many workers are started one runs the job at
//! a time; a run that finds the lock taken does nothing. The delivery of the
//! outbox is the one job that several workers share, since its rows are
//! claimed one by one.

use crate::channels::{self, Notifiers, Surroundings};
use crate::heartbeat::{self, Identity, Metrics};
use crate::outbox::{Outbox, OutboxParts};
use crate::{
    bot_links, catalogue, exports, provider_renewals, purge, reconciliation, reminders, retention,
    rollup, routes, side_threads,
};
use orch_config::ServerConfig;
use orch_db::queries::notifications::upkeep;
use orch_db::secrets::MasterKey;
use orch_db::{Db, DbError, Tx};
use orch_types::Clock;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The jobs of the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Job {
    /// Removes tokens, codes, browsers, counts of wrong tries and answers kept
    /// for a day that have run out.
    PurgeExpired,
    /// Claims and sends the rows that are due.
    Outbox,
    /// Sends what waits for a route's unread delay or frequency limit.
    Routes,
    /// Asks a bot who followed a link, while a code waits.
    BotLinks,
    /// Removes rows that were soft-deleted long enough ago.
    SoftDeletePurge,
    /// Removes raw events past the days they are kept.
    RawEvents,
    /// Writes the measurements the worker holds.
    MetricsSample,
    /// Removes measurements past the days they are kept.
    MetricsRetention,
    /// Tells the people who own machines whose certificate is about to end.
    CertReminders,
    /// Keeps each person's recent visits to their share.
    RecentsTrim,
    /// Reports that the worker is alive.
    Heartbeat,
    /// Fetches the rate catalogue from the address an admin set, when it is due.
    RateCatalogue,
    /// Checks the configured release source only when an administrator enabled its interval.
    ReleaseCheck,
    /// Folds the hours of metric samples that have ended into figures that are kept.
    MetricsRollup,
    /// Closes the side threads that stood idle for as long as their place allows.
    SideThreads,
    /// Queues the start of a runtime that has been stopped so long that the sign-in in it needs renewing.
    ProviderRenew,
    /// Removes the archives of a place whose time has run out or whose person is gone, what unfinished archives left
    /// in the scratch directory, and forgets the rows of those long gone.
    ExportPurge,
    /// Cuts a version of changed documents that have been left alone.
    DocumentVersions,
    /// Prunes old versions, removed blocks and unused document-file pins.
    DocumentPurge,
    /// Removes the commands to machines that ended a day ago, but for those still read back as a record.
    CommandPurge,
}

impl Job {
    /// Every job.
    pub const ALL: [Self; 20] = [
        Self::PurgeExpired,
        Self::Outbox,
        Self::Routes,
        Self::BotLinks,
        Self::SoftDeletePurge,
        Self::RawEvents,
        Self::MetricsSample,
        Self::MetricsRetention,
        Self::CertReminders,
        Self::RecentsTrim,
        Self::Heartbeat,
        Self::RateCatalogue,
        Self::ReleaseCheck,
        Self::MetricsRollup,
        Self::SideThreads,
        Self::ProviderRenew,
        Self::ExportPurge,
        Self::DocumentVersions,
        Self::DocumentPurge,
        Self::CommandPurge,
    ];

    /// The key of the job in `[schedule]` and in the name of its lock.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::PurgeExpired => "purge_expired",
            Self::Outbox => "outbox",
            Self::Routes => "routes",
            Self::BotLinks => "bot_links",
            Self::SoftDeletePurge => "soft_delete_purge",
            Self::RawEvents => "raw_events",
            Self::MetricsSample => "metrics_sample",
            Self::MetricsRetention => "metrics_retention",
            Self::CertReminders => "cert_reminders",
            Self::RecentsTrim => "recents_trim",
            Self::Heartbeat => "heartbeat",
            Self::RateCatalogue => "rate_catalogue",
            Self::ReleaseCheck => "release_check",
            Self::MetricsRollup => "metrics_rollup",
            Self::SideThreads => "side_threads",
            Self::ProviderRenew => "provider_renew",
            Self::ExportPurge => "export_purge",
            Self::DocumentVersions => "document_versions",
            Self::DocumentPurge => "document_purge",
            Self::CommandPurge => "command_purge",
        }
    }

    /// The interval the job ships with. A bot's link is good for ten minutes, so
    /// that job looks every few seconds whether a code waits, and asks no one
    /// when none does.
    #[must_use]
    pub const fn default_interval(self) -> Duration {
        let secs = match self {
            Self::PurgeExpired | Self::ExportPurge => 10 * 60,
            Self::Outbox | Self::BotLinks => 5,
            Self::Routes | Self::MetricsSample => 60,
            Self::SoftDeletePurge => 60 * 60,
            Self::RawEvents | Self::MetricsRetention | Self::CertReminders | Self::RecentsTrim => {
                24 * 60 * 60
            }
            Self::Heartbeat => 15,
            Self::MetricsRollup | Self::ProviderRenew => 60 * 60,
            Self::RateCatalogue => 24 * 60 * 60,
            Self::ReleaseCheck => 60 * 60,
            Self::SideThreads => 5 * 60,
            Self::DocumentVersions => 60,
            Self::DocumentPurge => 24 * 60 * 60,
            // A row is kept a day, so an hourly run keeps none more than an hour past it.
            Self::CommandPurge => 60 * 60,
        };
        Duration::from_secs(secs)
    }

    /// The job with this key.
    #[must_use]
    pub fn named(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|job| job.key() == key)
    }
}

/// The interval of a job: the one the operator named for its key, which is
/// never less than a second, or the one it ships with.
#[must_use]
pub fn interval(job: Job, named: &BTreeMap<String, u64>) -> Duration {
    named.get(job.key()).map_or_else(
        || job.default_interval(),
        |secs| Duration::from_secs((*secs).max(1)),
    )
}

/// The keys of the jobs that are not the worker's own, which `[schedule]` may
/// name too: the API's, and the two of the documents.
const OTHER_JOBS: [&str; 10] = [
    "approval_expiry",
    "idle_stop",
    "machine_liveness",
    "quota_release",
    "retention",
    "shell_detached",
    "summaries",
    "cache_eviction",
    "blob_gc",
    "provider_expiry",
];

/// Keys in `[schedule]` that name no job of the system: the operator's
/// misspellings, for a start to warn about.
#[must_use]
pub fn unknown_keys(config: &ServerConfig) -> Vec<String> {
    config
        .schedule
        .keys()
        .filter(|key| Job::named(key).is_none() && !OTHER_JOBS.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// Begins the transaction of a run of `job` and takes the job's lock in it.
/// None where another run holds the lock: this one has nothing to do. The lock
/// goes with the transaction, whether it commits or not.
pub async fn take(db: &Db, job: Job) -> Result<Option<Tx>, DbError> {
    if let Some(mut tx) = take_latch(db, job).await? {
        // The scheduling latch is nonblocking and protects no content: an already running job
        // must be skipped without waiting on its authority gate. Take authority before the run
        // can read or lock any object. Nobody waits on this latch, so it creates no reverse wait.
        // The jobs that wait on the network (the rate catalogue's fetch, the readings of who followed a
        // bot's link) or commit page by page hold a lease instead ([`take_lease`]).
        match job {
            Job::PurgeExpired | Job::SoftDeletePurge | Job::SideThreads | Job::ProviderRenew => {
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            }
            _ => orch_db::queries::transaction_guard::lock_action(&mut tx).await?,
        }
        Ok(Some(tx))
    } else {
        Ok(None)
    }
}

/// How long a run's lease on its job holds before the run renews it: ten minutes, far longer than one
/// page of a paged job takes, so that the run renews it between pages, and a worker that died leaves it
/// to another that soon.
pub(crate) const RUN_LEASE: chrono::Duration = chrono::Duration::minutes(10);

/// Takes the lease of `job` for one run whose pages each commit on their own: none where another run holds
/// it. No transaction is held open meanwhile; each page takes its own authority gate before it reads or locks
/// anything, and the run releases the lease at its end.
pub(crate) async fn take_lease(
    db: &Db,
    job: Job,
    clock: &dyn Clock,
) -> Result<Option<orch_db::queries::job_leases::Lease>, DbError> {
    let lease = orch_db::queries::job_leases::Lease::for_run(job.key(), RUN_LEASE);
    Ok(lease.hold(db, clock.now()).await?.then_some(lease))
}

/// Begins the transaction of a run that does all its work in it, holding the job's nonblocking latch, which
/// [`take`] then gates.
pub(crate) async fn take_latch(db: &Db, job: Job) -> Result<Option<Tx>, DbError> {
    let mut tx = orch_db::queries::transaction_guard::scope(false, db.begin()).await?;
    if upkeep::try_lock_job(&mut tx, job.key()).await? {
        Ok(Some(tx))
    } else {
        tx.rollback().await?;
        Ok(None)
    }
}

/// Runs `step` now and then every `every`, until `stop` is asked for. A step
/// that takes longer than the interval is followed by the next at once, not by
/// a pile of them, and one that fails is logged and tried at the next turn. A
/// stop does not interrupt a step that is under way.
pub async fn run_every<F, Fut>(job: Job, every: Duration, stop: CancellationToken, mut step: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut ticks = tokio::time::interval(every);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = ticks.tick() => {}
        }
        if let Err(error) = step().await {
            tracing::warn!(job = job.key(), %error, "a run of a job failed");
        }
    }
}

/// Everything the jobs run on.
#[derive(Clone)]
pub struct Services {
    /// The database.
    pub db: Db,
    /// Where time comes from.
    pub clock: Arc<dyn Clock>,
    /// The configuration of the deployment.
    pub config: Arc<ServerConfig>,
    /// The notifiers there are.
    pub notifiers: Notifiers,
    /// The key that opens sealed secrets.
    pub master: MasterKey,
    /// Which process this is.
    pub who: Identity,
    /// What is counted.
    pub metrics: Arc<Metrics>,
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services")
            .field("notifiers", &self.notifiers)
            .finish_non_exhaustive()
    }
}

/// What a step of a job reports when it did something: it is logged, and a step
/// that did nothing is not.
fn report(job: Job, did: &str) {
    tracing::info!(job = job.key(), "{did}");
}

/// One run of a job.
async fn step(
    job: Job,
    services: &Services,
    outbox: &Outbox,
    stop: &CancellationToken,
) -> Result<(), String> {
    let (db, clock, config) = (&services.db, &*services.clock, &*services.config);
    let failed = |error: &dyn std::fmt::Display| error.to_string();
    match job {
        Job::DocumentVersions => {
            if let Some(cuts) = crate::document_versions::run(db, clock, config)
                .await
                .map_err(|e| failed(&e))?
                && cuts > 0
            {
                report(job, &format!("{cuts} idle document versions cut"));
            }
        }
        Job::DocumentPurge => {
            if let Some(done) = crate::document_purge::run(db, clock, config, stop)
                .await
                .map_err(|e| failed(&e))?
                && done != crate::document_purge::Done::default()
            {
                report(job, &format!("{done:?}"));
            }
        }
        Job::CommandPurge => {
            if let Some(removed) = crate::command_purge::purge(db, clock)
                .await
                .map_err(|e| failed(&e))?
                && removed > 0
            {
                report(job, &format!("{removed} finished commands removed"));
            }
        }
        Job::ReleaseCheck => {
            crate::releases::check(
                db,
                clock,
                &config.release,
                &services.who.version,
                crate::releases::Wanted::WhenDue,
            )
            .await
            .map_err(|e| failed(&e))?;
        }
        Job::Outbox => {
            let done = outbox.run_until(stop).await.map_err(|e| failed(&e))?;
            if done.total() > 0 {
                report(job, &format!("{done:?}"));
            }
        }
        Job::Routes => {
            let served = routes::serve(db, clock).await.map_err(|e| failed(&e))?;
            if served.notifications > 0 {
                report(job, &format!("{served:?}"));
            }
        }
        Job::BotLinks => {
            let followed = bot_links::follow(db, clock, &services.notifiers, &services.master)
                .await
                .map_err(|e| failed(&e))?;
            if followed.linked > 0 {
                report(job, &format!("{followed:?}"));
            }
        }
        Job::PurgeExpired => {
            if let Some(removed) = purge::expired(db, clock, config)
                .await
                .map_err(|e| failed(&e))?
            {
                report(job, &format!("{removed:?}"));
            }
            let ended = reconciliation::end_stale_claims(db, reconciliation::STALE_CLAIM_IDLE)
                .await
                .map_err(|e| failed(&e))?;
            if ended > 0 {
                report(
                    job,
                    &format!("{ended} sessions that held claims were ended"),
                );
            }
        }
        Job::SoftDeletePurge => {
            if let Some(removed) = purge::soft_deleted(db, clock, config)
                .await
                .map_err(|e| failed(&e))?
                && !removed.is_empty()
            {
                report(job, &format!("{removed:?}"));
            }
        }
        Job::RawEvents => {
            if let Some(done) = retention::raw_events(db, clock)
                .await
                .map_err(|e| failed(&e))?
            {
                report(job, &format!("{done:?}"));
            }
        }
        Job::MetricsSample => {
            crate::gauges::storage_free(std::path::Path::new(&config.storage.root));
            heartbeat::sample(
                db,
                clock,
                &services.metrics,
                &services.notifiers,
                config.metrics.store,
            )
            .await
            .map_err(|e| failed(&e))?;
        }
        Job::MetricsRetention => {
            if let Some(done) = retention::metric_minutes(db, clock, config)
                .await
                .map_err(|e| failed(&e))?
            {
                report(job, &format!("{done:?}"));
            }
        }
        Job::CertReminders => {
            let shared = {
                let mut conn = db.acquire().await.map_err(|e| failed(&e))?;
                channels::mail_may_be_shared(&mut conn)
                    .await
                    .map_err(|e| failed(&e))?
            };
            if let Some(done) = reminders::certificates(db, clock, config, shared)
                .await
                .map_err(|e| failed(&e))?
                && done.machines > 0
            {
                report(job, &format!("{done:?}"));
            }
        }
        Job::SideThreads => {
            if let Some(closed) = side_threads::close_idle(db, clock)
                .await
                .map_err(|e| failed(&e))?
                && closed > 0
            {
                report(job, &format!("{closed} side threads closed"));
            }
        }
        Job::ExportPurge => {
            if let Some(done) = exports::purge(db, clock, &config.storage.scratch_dir())
                .await
                .map_err(|e| failed(&e))?
                && (done.removed > 0 || done.forgotten > 0 || done.scratch > 0)
            {
                report(job, &format!("{done:?}"));
            }
        }
        Job::RecentsTrim => {
            if let Some(removed) = retention::recents(db, config)
                .await
                .map_err(|e| failed(&e))?
                && removed > 0
            {
                report(job, &format!("{removed} recent visits trimmed"));
            }
        }
        Job::Heartbeat => {
            heartbeat::beat(db, clock, &services.who)
                .await
                .map_err(|e| failed(&e))?;
        }
        Job::RateCatalogue => {
            if let Some(catalogue::Outcome::Loaded(loaded)) = catalogue::scheduled(db, clock)
                .await
                .map_err(|e| failed(&e))?
            {
                report(job, &format!("{loaded:?}"));
            }
        }
        Job::MetricsRollup => {
            if let Some(done) = rollup::metric_hours(db, clock)
                .await
                .map_err(|e| failed(&e))?
                && done.hours > 0
            {
                report(job, &format!("{done:?}"));
            }
        }
        Job::ProviderRenew => {
            if let Some(done) = provider_renewals::scheduled(db, clock)
                .await
                .map_err(|e| failed(&e))?
                && done.queued > 0
            {
                report(job, &format!("{done:?}"));
            }
        }
    }
    Ok(())
}

/// Runs the worker: brings back what a worker that is gone left, reports that
/// this one is alive, and then runs every job at its interval until `stop` is
/// asked for. When it is, a step that is under way is finished, and this
/// returns when all are.
pub async fn run(services: Services, stop: CancellationToken) -> Result<(), DbError> {
    for key in unknown_keys(&services.config) {
        tracing::warn!(key, "[schedule] names a job that does not exist");
    }
    let recovered = reconciliation::at_start(&services.db, &*services.clock).await?;
    tracing::info!(?recovered, "the worker starts");
    heartbeat::beat(&services.db, &*services.clock, &services.who).await?;
    let outbox = Outbox::new(OutboxParts {
        db: services.db.clone(),
        clock: Arc::clone(&services.clock),
        notifiers: services.notifiers.clone(),
        master: services.master.clone(),
        surroundings: Surroundings {
            system_name: services.config.system.name.clone(),
            public_url: services.config.system.public_url.clone(),
        },
        retry: services
            .config
            .worker
            .outbox_retry_secs
            .iter()
            .map(|secs| Duration::from_secs(*secs))
            .collect(),
        metrics: Arc::clone(&services.metrics),
    });
    let mut jobs = tokio::task::JoinSet::new();
    for job in Job::ALL {
        let every = interval(job, &services.config.schedule);
        let (services, outbox, stopping) = (services.clone(), outbox.clone(), stop.clone());
        jobs.spawn(async move {
            let again = stopping.clone();
            run_every(job, every, stopping, move || {
                let (services, outbox, stopping) =
                    (services.clone(), outbox.clone(), again.clone());
                async move { step(job, &services, &outbox, &stopping).await }
            })
            .await;
        });
    }
    while let Some(ended) = jobs.join_next().await {
        if let Err(error) = ended {
            tracing::error!(%error, "a job ended abnormally");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::database::TestDb;

    #[test]
    fn every_job_has_a_key_and_the_interval_the_schedule_of_the_data_model_gives_it() {
        let intervals: Vec<_> = Job::ALL
            .iter()
            .map(|job| (job.key(), job.default_interval().as_secs()))
            .collect();
        assert_eq!(
            intervals,
            [
                ("purge_expired", 600),
                ("outbox", 5),
                ("routes", 60),
                ("bot_links", 5),
                ("soft_delete_purge", 3600),
                ("raw_events", 86_400),
                ("metrics_sample", 60),
                ("metrics_retention", 86_400),
                ("cert_reminders", 86_400),
                ("recents_trim", 86_400),
                ("heartbeat", 15),
                ("rate_catalogue", 86_400),
                ("release_check", 3600),
                ("metrics_rollup", 3600),
                ("side_threads", 300),
                ("provider_renew", 3600),
                ("export_purge", 600),
                ("document_versions", 60),
                ("document_purge", 86400),
                ("command_purge", 3600),
            ]
        );
        for job in Job::ALL {
            assert_eq!(Job::named(job.key()), Some(job));
        }
        assert_eq!(Job::named("no_such_job"), None);
    }

    #[test]
    fn an_operator_names_another_interval_by_the_key_of_a_job_and_never_below_a_second() {
        let named = BTreeMap::from([("routes".to_owned(), 300), ("outbox".to_owned(), 0)]);
        assert_eq!(interval(Job::Routes, &named), Duration::from_secs(300));
        assert_eq!(interval(Job::Outbox, &named), Duration::from_secs(1));
        assert_eq!(interval(Job::Heartbeat, &named), Duration::from_secs(15));
    }

    #[test]
    fn a_key_that_names_no_job_of_the_system_is_reported_but_the_apis_are_not() {
        let mut config = ServerConfig::default();
        config.schedule.insert("routes".into(), 30);
        config.schedule.insert("blob_gc".into(), 7_200);
        config.schedule.insert("routs".into(), 30);
        assert_eq!(unknown_keys(&config), ["routs"]);
    }

    #[tokio::test]
    async fn one_run_of_a_job_holds_its_lock_and_another_finds_it_taken_until_the_first_ends() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        // This is a latch proof, not a wait behind an authority-changing job. Both runs below
        // use the shared action gate; PurgeExpired correctly waits for that gate exclusively.
        const LOCK_PROOF_WITHIN: Duration = Duration::from_secs(25);
        tokio::time::timeout(LOCK_PROOF_WITHIN, async {
            let first = take(&db, Job::Routes)
                .await
                .unwrap()
                .expect("the first run takes the lock");
            assert!(
                take(&db, Job::Routes).await.unwrap().is_none(),
                "the second has nothing to do"
            );
            assert!(
                take(&db, Job::Heartbeat).await.unwrap().is_some(),
                "another job has a lock of its own"
            );
            first.commit().await.unwrap();
            assert!(
                take(&db, Job::Routes).await.unwrap().is_some(),
                "and the lock is free again"
            );
        })
        .await
        .expect("the nonblocking job-latch proof did not finish");
    }

    #[tokio::test(start_paused = true)]
    async fn a_job_runs_at_once_and_then_at_its_interval_until_it_is_stopped() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        // Cover the failed second step and successful steps on both sides of it.
        const FOLLOWING_INTERVALS: u32 = 3;
        // The runtime's timers distinguish milliseconds; stop immediately before
        // a tick to show that an interval causes exactly one following run.
        const BEFORE_TICK: Duration = Duration::from_millis(1);
        let runs = Arc::new(AtomicU32::new(0));
        let (finished, mut completion) = tokio::sync::mpsc::unbounded_channel();
        let started = tokio::time::Instant::now();
        let every = Job::Heartbeat.default_interval();
        let stop = CancellationToken::new();
        let counted = Arc::clone(&runs);
        let job = tokio::spawn(run_every(Job::Heartbeat, every, stop.clone(), move || {
            let counted = Arc::clone(&counted);
            let finished = finished.clone();
            async move {
                let run = counted.fetch_add(1, Ordering::SeqCst) + 1;
                finished.send(run).unwrap();
                if run == 2 {
                    return Err("one failed".to_owned());
                }
                Ok(())
            }
        }));
        assert_eq!(completion.recv().await, Some(1));
        assert_eq!(tokio::time::Instant::now(), started);
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the first run is at once, not after an interval"
        );
        for following in 1..=FOLLOWING_INTERVALS {
            tokio::time::advance(every - BEFORE_TICK).await;
            tokio::task::yield_now().await;
            assert_eq!(runs.load(Ordering::SeqCst), following);
            tokio::time::advance(BEFORE_TICK).await;
            assert_eq!(completion.recv().await, Some(following + 1));
            assert_eq!(runs.load(Ordering::SeqCst), following + 1);
        }
        stop.cancel();
        job.await.unwrap();
        tokio::time::advance(every * FOLLOWING_INTERVALS).await;
        assert_eq!(
            runs.load(Ordering::SeqCst),
            FOLLOWING_INTERVALS + 1,
            "no run after it is stopped"
        );
    }
}
