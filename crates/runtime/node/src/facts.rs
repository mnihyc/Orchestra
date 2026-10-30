//! Capacity, load, what the units there declare they need, storages, driver descriptors, units and tool facts.
//!
//! What the node tells the system about the host it is on, and how it tells it. [`Reports`] is the one place
//! the node's events are made: `node.runtime` for a unit, `node.facts` for the host, `node.load` for what it
//! uses and `job.progress` for what it does for a job. **`node.facts`** says what the host has (processors,
//! memory, disk), which drivers it loaded with what each declares, and its storages; it is told when it
//! changes in a way that matters, and the free space of a storage, which changes all the time, is not such a
//! change. **`node.load`** says what the host uses, every [`NODE_LOAD_EVERY_SECS`] seconds and at once when a
//! value has moved by more than [`NODE_LOAD_STEP`] of what the host has ([`LoadWatch`]). **`node.probe`**
//! answers both, and adds the units the drivers hold and the tools in the cache.
//!
//! **What the units declare they need** is the limits their runtimes were made with ([`declared`]): a sum of
//! statements, as the system's list of places makes it. Nothing on the host is reserved for it. The node uses
//! it to say, when a unit cannot be made for want of room, how much the units already there declare.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use orch_api_contract::workspace::JobProgress;
use orch_connector_core::events::Publisher;
use orch_connector_core::facts::Host;
use orch_driver_contract::units::{UnitState, UnitStateKind};
use orch_link_contract::commands::{NodeProbeResult, NodeProbeTool, NodeProbeUnit};
use orch_link_contract::events::{
    Event, JobProgress as JobProgressEvent, NODE_LOAD_EVERY_SECS, NODE_LOAD_STEP, NodeCapacity,
    NodeFacts, NodeLoad, NodeRuntime,
};
use orch_link_contract::streams::Stream;
use orch_types::Id;
use sysinfo::{MemoryRefreshKind, System};

use crate::ledger::Record;
use crate::storages::Storages;
use crate::tools::ToolCache;

/// Bytes in a mebibyte, which is what memory is written in.
const BYTES_PER_MB: u64 = 1 << 20;
/// One percent of one processor is ten thousandths of a processor.
const CPU_MILLIS_PER_PERCENT: f64 = 10.0;

/// How often the node looks at what the host uses to see whether it moved far enough to be told: five seconds,
/// which is cheap, and short enough that a value that moved is told within the time it takes a person to
/// notice the host is full.
pub const LOAD_LOOK_EVERY: Duration = Duration::from_secs(5);

/// The word a unit's state is told in.
#[must_use]
pub const fn state_name(state: UnitStateKind) -> &'static str {
    match state {
        UnitStateKind::Running => "running",
        UnitStateKind::Stopped => "stopped",
        UnitStateKind::Absent => "absent",
        UnitStateKind::Failed => "failed",
    }
}

/// What a runtime's unit was last said to be in: its state, and what there was to say about it.
type Told = (String, Option<String>);

/// What a driver said about a unit's state, as one sentence.
#[must_use]
pub fn detail_of(state: &UnitState) -> Option<String> {
    match (&state.detail, state.exit_code) {
        (Some(detail), Some(code)) => Some(format!("{detail} (exit status {code})")),
        (Some(detail), None) => Some(detail.clone()),
        (None, Some(code)) if state.state == UnitStateKind::Failed => {
            Some(format!("exit status {code}"))
        }
        _ => None,
    }
}

/// What the node has told of its runtimes: the last thing said of each, and which telling it was.
#[derive(Default)]
struct Account {
    /// How many tellings there have been; each is numbered by it.
    count: u64,
    /// The last telling of each runtime: what it said, and its number.
    last: HashMap<Id, (Told, u64)>,
}

impl Account {
    /// Notes that `state` and `detail` are being told of a runtime.
    fn note(&mut self, runtime_id: Id, state: &str, detail: Option<String>) {
        self.count += 1;
        self.last
            .insert(runtime_id, ((state.to_owned(), detail), self.count));
    }
}

/// Where the node's events are made.
pub struct Reports {
    publisher: Arc<dyn Publisher>,
    told: Mutex<Account>,
    networks: Mutex<HashMap<Id, orch_api_contract::environment::NetworkState>>,
}

impl std::fmt::Debug for Reports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reports").finish_non_exhaustive()
    }
}

impl Reports {
    /// Complete owned inventory, durable so an API/node restart can finish an interrupted cleanup.
    pub async fn kept_images(
        &self,
        epoch: u64,
        images: Vec<orch_driver_contract::kept::KeptImage>,
    ) {
        self.put(Event::NodeKeptImages(
            orch_link_contract::events::NodeKeptImages { epoch, images },
        ))
        .await;
    }
    /// Reports a runtime's verified network on the durable node stream, before its connector need enroll.
    pub async fn network(
        &self,
        runtime_id: Id,
        network: orch_api_contract::environment::NetworkState,
    ) {
        if self
            .networks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&runtime_id)
            == Some(&network)
        {
            return;
        }
        let event = Event::NodeNetwork(orch_link_contract::events::NodeNetwork {
            runtime_id,
            network: network.clone(),
        });
        if self.publisher.publish(Stream::Node, event).await.is_ok() {
            self.networks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(runtime_id, network);
        }
    }

    /// Events put on `publisher`, on the node's stream.
    #[must_use]
    pub fn new(publisher: Arc<dyn Publisher>) -> Self {
        Self {
            publisher,
            told: Mutex::new(Account::default()),
            networks: Mutex::new(HashMap::new()),
        }
    }

    fn account(&self) -> MutexGuard<'_, Account> {
        self.told.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn put(&self, event: Event) {
        let kind = event.kind();
        if let Err(error) = self.publisher.publish(Stream::Node, event).await {
            tracing::warn!(%kind, %error, "an event of the node could not be put in the spool");
        }
    }

    async fn put_runtime(&self, runtime_id: Id, state: &str, detail: Option<String>) {
        self.put(Event::NodeRuntime(NodeRuntime {
            runtime_id,
            state: state.to_owned(),
            detail,
        }))
        .await;
    }

    /// The state the driver finds a runtime's unit in, told whether or not it is what the system was last
    /// told.
    pub async fn runtime(&self, runtime_id: Id, state: &str, detail: Option<String>) {
        self.account().note(runtime_id, state, detail.clone());
        self.put_runtime(runtime_id, state, detail).await;
    }

    /// As [`Reports::runtime`], but only when the state or what is said of it differs from what the system
    /// was last told of that runtime. Answers whether it told.
    pub async fn runtime_if_changed(
        &self,
        runtime_id: Id,
        state: &str,
        detail: Option<String>,
    ) -> bool {
        self.tell_if_changed(None, runtime_id, state, detail).await
    }

    /// A point in the tellings, by which a look at the units is dated: taken before the look begins, and given
    /// back with what the look found to [`Reports::runtime_if_changed_since`].
    #[must_use]
    pub fn mark(&self) -> u64 {
        self.account().count
    }

    /// As [`Reports::runtime_if_changed`] for what a look found that began at `mark`: nothing is told of a
    /// runtime the node told of since, because what the look saw may be what that telling has changed. The
    /// next look sees the unit as it is.
    pub async fn runtime_if_changed_since(
        &self,
        mark: u64,
        runtime_id: Id,
        state: &str,
        detail: Option<String>,
    ) -> bool {
        self.tell_if_changed(Some(mark), runtime_id, state, detail)
            .await
    }

    async fn tell_if_changed(
        &self,
        mark: Option<u64>,
        runtime_id: Id,
        state: &str,
        detail: Option<String>,
    ) -> bool {
        {
            // What was told is looked at and noted in one hold, so that two tellers cannot both find a
            // change in the same thing.
            let mut account = self.account();
            if let Some(((last_state, last_detail), number)) = account.last.get(&runtime_id) {
                let newer = mark.is_some_and(|mark| *number > mark);
                if newer || (last_state == state && *last_detail == detail) {
                    return false;
                }
            }
            account.note(runtime_id, state, detail.clone());
        }
        self.put_runtime(runtime_id, state, detail).await;
        true
    }

    /// What the system was last told of a runtime.
    #[must_use]
    pub fn told_of(&self, runtime_id: Id) -> Option<(String, Option<String>)> {
        self.account()
            .last
            .get(&runtime_id)
            .map(|(told, _)| told.clone())
    }

    /// What the host has and can make.
    pub async fn facts(&self, facts: NodeFacts) {
        self.put(Event::NodeFacts(facts)).await;
    }

    /// What the host uses.
    pub async fn load(&self, load: NodeLoad) {
        self.put(Event::NodeLoad(load)).await;
    }

    /// One run of an operator's lifecycle hook for a runtime's unit: the moment, why, how it ended and how long it
    /// took, never what it printed.
    pub async fn hook(&self, run: orch_link_contract::events::NodeHook) {
        self.put(Event::NodeHook(run)).await;
    }

    /// That the node no longer serves the host opening `opening_id` it held at `revision`, though nobody
    /// closed it, and why: the system marks it unavailable and its number free.
    pub async fn port_ended(&self, opening_id: Id, revision: i64, reason: String) {
        self.put(Event::PortState(orch_link_contract::ports::PortState {
            opening_id,
            revision,
            state: orch_link_contract::ports::PortPublishState::Failed,
            reason: Some(reason),
        }))
        .await;
    }

    /// What the host does for a job, with the end of a build's log where there is one.
    pub async fn job(&self, job_id: Id, progress: &JobProgress, log: Option<String>) {
        self.job_command(job_id, None, progress, log).await;
    }

    /// Reports file work under its exact command, so buffered counters cannot advance a later copy.
    pub async fn job_command(
        &self,
        job_id: Id,
        command_id: Option<Id>,
        progress: &JobProgress,
        log: Option<String>,
    ) {
        self.put(Event::JobProgress(JobProgressEvent {
            command_id,
            job_id,
            progress: progress.clone(),
            log,
        }))
        .await;
    }
}

/// What the units declare they need.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Declared {
    /// Thousandths of a processor.
    pub cpu_millis: i64,
    /// Memory, in MiB.
    pub memory_mb: i64,
    /// Disk, in GB.
    pub disk_gb: i64,
}

/// The limits of the runtimes whose units run (the processors and the memory), and of every runtime that has
/// volumes here, running or not (the disk): a sum of statements. A unit that states no limit is counted at
/// none; the system counts it at its own limit, which the node does not know.
#[must_use]
pub fn declared(records: &[Record], running: &HashSet<Id>) -> Declared {
    let mut sum = Declared::default();
    for record in records {
        let limits = &record.resources;
        if running.contains(&record.runtime_id) {
            sum.cpu_millis = sum
                .cpu_millis
                .saturating_add(limits.cpu_millis.unwrap_or(0));
            sum.memory_mb = sum.memory_mb.saturating_add(limits.memory_mb.unwrap_or(0));
        }
        sum.disk_gb = sum.disk_gb.saturating_add(limits.disk_gb.unwrap_or(0));
    }
    sum
}

/// What the host has.
#[must_use]
pub fn capacity(storages: &Storages) -> NodeCapacity {
    if let Some(resources) = storages.pool_resources() {
        return resources
            .map(|resources| NodeCapacity {
                cpus: resources.cpus,
                cpu_millis: resources.cpu_millis,
                memory_mb: resources.memory_mb,
                disk_gb: resources.disk_gb,
            })
            .unwrap_or(NodeCapacity {
                cpus: 0,
                cpu_millis: None,
                memory_mb: 0,
                disk_gb: 0,
            });
    }
    let host = Host::detect();
    NodeCapacity {
        cpus: i64::from(host.cpus),
        cpu_millis: None,
        memory_mb: host.memory_bytes / i64::try_from(BYTES_PER_MB).unwrap_or(i64::MAX),
        disk_gb: storages.disk().0,
    }
}

/// Reads what the host uses.
pub struct LoadSampler {
    system: System,
    cpus: f64,
}

impl std::fmt::Debug for LoadSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadSampler").finish_non_exhaustive()
    }
}

impl Default for LoadSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadSampler {
    /// A sampler; the processors are measured between two looks, so the first answers nothing about them.
    #[must_use]
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let cpus = f64::from(u32::try_from(system.cpus().len().max(1)).unwrap_or(1));
        Self { system, cpus }
    }

    /// What the host uses now: the processors in percent of one processor, the memory, and the disk of
    /// `storages`.
    pub fn sample(&mut self, storages: &Storages) -> Option<NodeLoad> {
        if let Some(resources) = storages.pool_resources() {
            let resources = resources.ok()?;
            return Some(NodeLoad {
                cpu_percent: resources.cpu_percent?,
                memory_used_mb: resources.memory_used_mb?,
                disk_used_gb: resources.disk_used_gb?,
                disk_free_gb: resources.disk_free_gb?,
            });
        }
        self.system.refresh_cpu_usage();
        self.system
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let (capacity, free) = storages.disk();
        Some(NodeLoad {
            cpu_percent: f64::from(self.system.global_cpu_usage()) * self.cpus,
            memory_used_mb: i64::try_from(self.system.used_memory() / BYTES_PER_MB)
                .unwrap_or(i64::MAX),
            disk_used_gb: capacity.saturating_sub(free).max(0),
            disk_free_gb: free,
        })
    }
}

/// Decides when what the host uses is told: every [`NODE_LOAD_EVERY_SECS`] seconds, and at once when a value
/// has moved by more than [`NODE_LOAD_STEP`] of what the host has.
#[derive(Debug, Default)]
pub struct LoadWatch {
    last: Option<(NodeLoad, tokio::time::Instant)>,
}

impl LoadWatch {
    /// A watch that has told nothing yet, so that the first look is told.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `load`, looked at `now` on a host that has `capacity`, is to be told; if it is, it is noted as
    /// told.
    pub fn due(
        &mut self,
        load: NodeLoad,
        capacity: &NodeCapacity,
        now: tokio::time::Instant,
    ) -> bool {
        let told = match &self.last {
            None => true,
            Some((before, at)) => {
                let moved = |now: f64, then: f64, of: f64| (now - then).abs() > NODE_LOAD_STEP * of;
                now.duration_since(*at) >= Duration::from_secs(u64::from(NODE_LOAD_EVERY_SECS))
                    || moved(
                        load.cpu_percent,
                        before.cpu_percent,
                        capacity
                            .cpu_millis
                            .map_or(capacity.cpus as f64 * 100.0, |millis| {
                                millis as f64 / CPU_MILLIS_PER_PERCENT
                            }),
                    )
                    || moved(
                        load.memory_used_mb as f64,
                        before.memory_used_mb as f64,
                        capacity.memory_mb as f64,
                    )
                    || moved(
                        load.disk_used_gb as f64,
                        before.disk_used_gb as f64,
                        capacity.disk_gb as f64,
                    )
            }
        };
        if told {
            self.last = Some((load, now));
        }
        told
    }
}

/// The facts of the host.
pub struct Facts {
    drivers: Arc<crate::drivers::DriverHost>,
    storages: Arc<Storages>,
    tools: Arc<ToolCache>,
    reports: Arc<Reports>,
    told: Mutex<Option<NodeFacts>>,
}

impl std::fmt::Debug for Facts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Facts").finish_non_exhaustive()
    }
}

/// What of the facts is told again when it changes: all of it, but the free space of the storages.
fn lasting(facts: &NodeFacts) -> NodeFacts {
    let mut kept = facts.clone();
    for storage in &mut kept.storages {
        storage.free_gb = 0;
    }
    kept
}

impl Facts {
    /// The facts of a host with these drivers, storages and tools.
    #[must_use]
    pub fn new(
        drivers: Arc<crate::drivers::DriverHost>,
        storages: Arc<Storages>,
        tools: Arc<ToolCache>,
        reports: Arc<Reports>,
    ) -> Self {
        Self {
            drivers,
            storages,
            tools,
            reports,
            told: Mutex::new(None),
        }
    }

    /// What the host has, can make and keeps its volumes on, now.
    #[must_use]
    pub fn current(&self) -> NodeFacts {
        NodeFacts {
            capacity: capacity(&self.storages),
            drivers: self.drivers.descriptors(),
            driver_configuration_schemas: self.drivers.configuration_schemas(),
            storages: self.storages.report(),
            mount_sizes: Vec::new(),
        }
    }

    /// Tells the system the facts if they are not what it was last told. Answers whether it did.
    pub async fn report_if_changed(&self) -> bool {
        let now = self.current();
        {
            let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
            if told.as_ref().map(lasting) == Some(lasting(&now)) {
                return false;
            }
            *told = Some(now.clone());
        }
        self.reports.facts(now).await;
        true
    }

    /// The tools in the cache.
    #[must_use]
    pub fn tools(&self) -> Vec<NodeProbeTool> {
        self.tools
            .list()
            .into_iter()
            .map(|(name, version)| NodeProbeTool { name, version })
            .collect()
    }

    /// The answer to `node.probe`: the facts, the units the drivers hold and the tools in the cache.
    #[must_use]
    pub fn probe(&self, units: Vec<NodeProbeUnit>) -> NodeProbeResult {
        let facts = self.current();
        NodeProbeResult {
            capacity: facts.capacity,
            drivers: facts.drivers,
            storages: facts.storages,
            units,
            tools: self.tools(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::runtime::{Resources, RuntimeKind};
    use orch_driver_contract::units::UnitRef;
    use orch_types::new_id;

    fn record(memory_mb: i64, disk_gb: i64) -> Record {
        let id = new_id();
        Record {
            runtime_id: id,
            kind: RuntimeKind::Environment,
            driver: "memory".into(),
            unit: UnitRef {
                reference: String::new(),
                name: id.to_string(),
                runtime_id: id,
                kind: RuntimeKind::Environment,
            },
            volumes: Vec::new(),
            token: None,
            enrolled: true,
            resources: Resources {
                cpu_millis: Some(1000),
                memory_mb: Some(memory_mb),
                pids: None,
                disk_gb: Some(disk_gb),
                ..Resources::default()
            },
            image: None,
            created_at: "2026-10-11T12:00:00.000Z".parse().unwrap(),
            started_at: None,
            enroll_timeout_secs: None,
        }
    }

    #[test]
    fn the_units_declare_memory_while_they_run_and_disk_for_as_long_as_they_have_volumes() {
        let (running, stopped) = (record(2048, 10), record(4096, 50));
        let up: HashSet<Id> = [running.runtime_id].into();
        let sum = declared(&[running, stopped], &up);
        assert_eq!(
            sum,
            Declared {
                cpu_millis: 1000,
                memory_mb: 2048,
                disk_gb: 60
            }
        );
    }

    fn load(cpu: f64, memory: i64, disk: i64) -> NodeLoad {
        NodeLoad {
            cpu_percent: cpu,
            memory_used_mb: memory,
            disk_used_gb: disk,
            disk_free_gb: 1000 - disk,
        }
    }

    #[test]
    fn fractional_pool_capacity_sets_the_same_relative_load_change_threshold() {
        let capacity = NodeCapacity {
            cpus: 4,
            cpu_millis: Some(500),
            memory_mb: 10_000,
            disk_gb: 1000,
        };
        let mut watch = LoadWatch::new();
        let now = tokio::time::Instant::now();
        assert!(watch.due(load(0.0, 1000, 100), &capacity, now));
        // Six percent exceeds a tenth of a half processor, but not a tenth of four processors.
        assert!(watch.due(
            load(6.0, 1000, 100),
            &capacity,
            now + Duration::from_secs(1)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn load_is_told_at_first_every_minute_and_at_once_when_a_value_moves_by_a_tenth() {
        let capacity = NodeCapacity {
            cpus: 4,
            cpu_millis: None,
            memory_mb: 10_000,
            disk_gb: 1000,
        };
        let mut watch = LoadWatch::new();
        let start = tokio::time::Instant::now();
        assert!(
            watch.due(load(10.0, 1000, 100), &capacity, start),
            "the first look is told"
        );
        let soon = start + Duration::from_secs(5);
        assert!(
            !watch.due(load(40.0, 1500, 120), &capacity, soon),
            "small moves wait for the minute"
        );
        assert!(
            watch.due(load(40.0, 2100, 120), &capacity, soon),
            "memory moved by 11 % of the host's"
        );
        assert!(
            watch.due(load(450.0, 2100, 120), &capacity, soon),
            "so did the processors, by 41 % of four"
        );
        let later = soon + Duration::from_secs(u64::from(NODE_LOAD_EVERY_SECS));
        assert!(
            watch.due(load(450.0, 2100, 120), &capacity, later),
            "the minute is up"
        );
    }

    #[test]
    fn a_state_is_told_in_the_words_the_drivers_state_is_written_with() {
        for state in [
            UnitStateKind::Running,
            UnitStateKind::Stopped,
            UnitStateKind::Absent,
            UnitStateKind::Failed,
        ] {
            let written = serde_json::to_value(state).unwrap();
            assert_eq!(written, state_name(state));
        }
    }

    /// Somewhere events are put, which keeps them.
    #[derive(Default)]
    struct Kept(Mutex<Vec<Event>>);

    #[async_trait::async_trait]
    impl Publisher for Kept {
        async fn publish(
            &self,
            _stream: Stream,
            event: Event,
        ) -> Result<(), orch_link::streams::PublishError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }

    impl Kept {
        /// What was told of a runtime, in order.
        fn told_of(&self, runtime_id: Id) -> Vec<(String, Option<String>)> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|event| match event {
                    Event::NodeRuntime(told) if told.runtime_id == runtime_id => {
                        Some((told.state.clone(), told.detail.clone()))
                    }
                    _ => None,
                })
                .collect()
        }
    }

    #[tokio::test]
    async fn a_look_that_began_before_the_node_told_of_a_runtime_does_not_tell_of_it_after() {
        let kept = Arc::new(Kept::default());
        let reports = Reports::new(kept.clone());
        let (watched, other) = (new_id(), new_id());

        // A look at the units begins, and while it goes on the watch of an enrollment tells why a unit failed.
        let mark = reports.mark();
        reports
            .runtime(
                watched,
                "failed",
                Some("its connector did not enroll".into()),
            )
            .await;
        // What the look saw of that unit is older than the telling and is not told; what it saw of another is.
        assert!(
            !reports
                .runtime_if_changed_since(mark, watched, "failed", Some("exit status 1".into()))
                .await
        );
        assert!(
            reports
                .runtime_if_changed_since(mark, other, "running", None)
                .await
        );
        assert_eq!(
            kept.told_of(watched),
            [(
                "failed".to_owned(),
                Some("its connector did not enroll".to_owned())
            )]
        );

        // The next look begins after the telling, and is believed; the same thing is told once.
        let mark = reports.mark();
        assert!(
            reports
                .runtime_if_changed_since(mark, watched, "absent", None)
                .await
        );
        assert!(
            !reports
                .runtime_if_changed_since(mark, watched, "absent", None)
                .await
        );
        assert_eq!(reports.told_of(watched), Some(("absent".to_owned(), None)));
        assert_eq!(kept.told_of(watched).len(), 2);
    }
}
