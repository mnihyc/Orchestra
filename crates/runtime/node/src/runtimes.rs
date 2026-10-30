//! Driver-neutral runtime commands for both pools, serialized lifecycle, successors and retired units.
//!
//! Everything Orch runs on a host for people is a **runtime**, a unit that a driver makes: an environment
//! that threads work in, or a provider's runtime where its credential is kept. The node carries out the
//! commands about them the same way for both pools and for every driver: `node.runtime.create`, `.start`,
//! `.stop` and `.remove`.
//!
//! **Create** opens the enrollment token that was sealed to this machine, checks what the connector inside the
//! unit will need ([`crate::managed`]), makes the image available first (pulled, built or loaded) and has the
//! driver make the unit, idempotently by name: asked again for a runtime it has, it answers the unit it has,
//! and a unit that is not running is given the new request's token. **Start** and **stop** act on the unit
//! the node recorded for the runtime (or finds by the system's label when it has no record), and tell the system
//! the state the driver then finds. **Remove** takes the unit and never its volumes; removing what is not
//! there is done.
//!
//! **Commands for one runtime run one at a time**, in the order they came: the link queues them behind each
//! other by their serial key, and the node holds a lock per runtime besides, because it acts on a runtime on its
//! own account too (a unit whose connector never enrolled). Commands for two runtimes run side by side.
//!
//! **Successors.** A recreate gives an environment a new unit on the same volumes, made as a runtime of its own
//! while the old unit is kept until the system says to remove it. The node keeps two units off one set of
//! volumes whatever the driver does: a unit is not started while another that stands on one of its volumes runs
//! ([`orch_driver_contract::units::successor_may_start`]). The old unit is a **retired** one once its
//! successor has taken over, and the system removes it with `node.runtime.remove`; its volumes stay with the
//! successor.
//!
//! **A connector that does not enroll.** A unit that was started and whose connector has not enrolled is
//! watched for `[node] enroll_timeout_secs`. If the unit ends before it has, or the time is up and the driver
//! says the connector has not enrolled, the runtime is reported failed with the reason and its unit is
//! removed (never its volumes). A driver that cannot tell whether a connector has enrolled is not judged.
//!
//! **The operator's hooks** ([`crate::hooks`]) run around a start that brings a unit up (`pre_up` once its network
//! exists, `post_up` once it runs; either failing keeps it from running, and the start fails with what the hook
//! printed first) and around a stop or a removal that takes it down (`pre_down`, `post_down`; told and never
//! holding it), each told why: the reason a stop carries, or for a unit's first start the reason it was made for.
//! A starting node runs `post_up` again for the units it finds running, and a unit that came up and ended by
//! itself is given its `post_down` at the next look.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::FutureExt;

use orch_connector_core::roles::Keys;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::network::{EgressMode, NetworkPolicy, UnitNetwork};
use orch_driver_contract::runtime::LABEL_SYSTEM;
use orch_driver_contract::sources::Source;
use orch_driver_contract::units::{UnitRef, UnitStateKind, successor_may_start};
use orch_link_contract::commands::{
    CommandError, ErrorCode, NodeNetworkApply, NodeRuntimeCreateResult,
};
use orch_types::{Clock, Hash, Id, Json};
use tokio_util::sync::CancellationToken;

use crate::drivers::{DriverHost, HostDriver, NO_CAPACITY, failure};
use crate::facts::{Reports, capacity, declared, detail_of, state_name};
use crate::hooks::{
    DOCUMENT_VERSION, HookDocument, HookEvent, HookHost, HookNetwork, HookPhase, HookRun,
    HookRuntime, HookUnit, Hooks, PublishedPorts, UnitAbout, UnitReason,
};
use crate::images::Images;
use crate::ledger::{Ledger, Record};
use crate::managed;
use crate::reconciliation::locate;
use crate::settings::NodeSettings;
use crate::storages::Storages;
use crate::tools::ToolCache;
use crate::volumes::Volumes;

mod kept_images;
mod live;
mod mount_copies;
mod mounts;
mod retained;
mod volume_moves;

/// How often a unit that has not enrolled is looked at: two seconds, which costs a file read and finds the
/// failure of a unit within the time it takes to be sorry.
pub const ENROLL_LOOK_EVERY: Duration = Duration::from_secs(2);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The runtimes a command is being carried out for. The reconciliation leaves these alone: the command
/// tells the system what became of them when it ends.
#[derive(Debug, Default)]
pub struct Busy {
    running: Mutex<BTreeMap<Id, usize>>,
}

/// A runtime counts as busy until this is dropped.
pub struct BusyGuard {
    busy: Arc<Busy>,
    runtime_id: Id,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        let mut running = lock(&self.busy.running);
        if let Some(count) = running.get_mut(&self.runtime_id) {
            *count -= 1;
            if *count == 0 {
                running.remove(&self.runtime_id);
            }
        }
    }
}

impl Busy {
    /// Nothing is busy.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Counts the runtime as busy until the guard is dropped.
    #[must_use]
    pub fn enter(self: &Arc<Self>, runtime_id: Id) -> BusyGuard {
        *lock(&self.running).entry(runtime_id).or_default() += 1;
        BusyGuard {
            busy: Arc::clone(self),
            runtime_id,
        }
    }

    /// The runtimes that are busy now.
    #[must_use]
    pub fn ids(&self) -> BTreeSet<Id> {
        lock(&self.running).keys().copied().collect()
    }
}

/// Everything a runtime command needs, and the state the commands share.
struct Inner {
    drivers: Arc<DriverHost>,
    ledger: Arc<Ledger>,
    volumes: Arc<Volumes>,
    storages: Arc<Storages>,
    images: Arc<Images>,
    tools: Arc<ToolCache>,
    reports: Arc<Reports>,
    keys: Arc<Keys>,
    clock: Arc<dyn Clock>,
    settings: NodeSettings,
    busy: Arc<Busy>,
    copy_tasks: tokio_util::task::TaskTracker,
    locks: Mutex<HashMap<Id, Arc<tokio::sync::Mutex<()>>>>,
    /// Existing ledgers are checked once for an interrupted live mount transaction.
    cold_mounts: Mutex<BTreeSet<Id>>,
    watches: Mutex<HashMap<Id, (u64, CancellationToken)>>,
    watch_count: std::sync::atomic::AtomicU64,
    /// The tokens of requests that are being carried out, by the runtime they are for: a token is claimed
    /// before the image is pulled, so that two requests with one token cannot both get past the check.
    claims: Arc<Mutex<HashMap<Hash, Id>>>,
    stop: CancellationToken,
    /// The operator's lifecycle hooks.
    hooks: Hooks,
    /// The host openings the node holds, which the hooks are told of.
    ports: Option<Arc<dyn PublishedPorts>>,
    /// How the node puts up and fetches content for a command: a runtime's files on their way to another host.
    content: Option<Arc<orch_connector_core::content::Content>>,
    policy: Mutex<Option<Arc<orch_connector_core::policy::PolicyStore>>>,
    resource_changes: tokio::sync::RwLock<()>,
    link: Mutex<Option<orch_link::machine::MachineLink>>,
}

/// A token is claimed for a runtime until this is dropped; from then on the ledger holds it.
struct Claim {
    claims: Arc<Mutex<HashMap<Hash, Id>>>,
    token: Hash,
}

impl Drop for Claim {
    fn drop(&mut self) {
        lock(&self.claims).remove(&self.token);
    }
}

/// The runtimes of the host.
#[derive(Clone)]
pub struct Runtimes {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Runtimes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtimes").finish_non_exhaustive()
    }
}

/// What the node needs to carry out the commands about runtimes.
pub struct RuntimeParts {
    /// The drivers.
    pub drivers: Arc<DriverHost>,
    /// What is recorded of the runtimes.
    pub ledger: Arc<Ledger>,
    /// The volumes.
    pub volumes: Arc<Volumes>,
    /// The storages.
    pub storages: Arc<Storages>,
    /// The images.
    pub images: Arc<Images>,
    /// The tool cache.
    pub tools: Arc<ToolCache>,
    /// Where events are made.
    pub reports: Arc<Reports>,
    /// The machine's key, which opens what the system sealed to it.
    pub keys: Arc<Keys>,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// How the node is configured.
    pub settings: NodeSettings,
    /// The runtimes that are busy.
    pub busy: Arc<Busy>,
    /// The host openings the node holds, which the operator's hooks are told of; none where it holds none.
    pub ports: Option<Arc<dyn PublishedPorts>>,
    /// How the node puts up and fetches content for a command; none where it moves no files.
    pub content: Option<Arc<orch_connector_core::content::Content>>,
}

fn network_state(
    network: &UnitNetwork,
) -> Result<orch_api_contract::environment::NetworkState, CommandError> {
    let mut value = serde_json::to_value(network)
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
    value["network_mode"] = serde_json::json!("bridge");
    value["sources"] = serde_json::json!([]);
    value["always_source"] = serde_json::json!("orch_configuration");
    serde_json::from_value(value)
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))
}

impl Runtimes {
    /// Reports this installation's objects across every configured driver and the tool cache.
    pub async fn resources(
        &self,
    ) -> Result<orch_driver_contract::resources::ResourceInventory, CommandError> {
        let _read = self.inner.resource_changes.read().await;
        crate::resources::list(
            &self.inner.drivers,
            &self.inner.tools,
            &self.inner.ledger,
            self.inner.clock.as_ref(),
        )
        .await
    }

    /// Removes a freshly verified unused object while no unit can be created or started over it.
    pub async fn remove_resource(
        &self,
        body: &orch_link_contract::commands::NodeResourcesRemove,
    ) -> Result<(), CommandError> {
        let _change = self.inner.resource_changes.try_write().map_err(|_| {
            crate::drivers::failure(&orch_driver_contract::driver::DriverError::Busy(
                "the host is using its runtimes or caches".into(),
            ))
        })?;
        if body.stop_first {
            crate::resources::stop_for_purge(
                &self.inner.drivers,
                &self.inner.tools,
                &self.inner.ledger,
                &body.reference(),
            )
            .await?;
        }
        let runtime = crate::resources::remove_with_runtime(
            &self.inner.drivers,
            &self.inner.tools,
            &self.inner.ledger,
            self.inner.clock.as_ref(),
            &body.reference(),
            body.expected_last_used_at,
        )
        .await?;
        if let Some(runtime) = runtime {
            self.unwatch(runtime);
            self.forget(runtime).await;
        }
        Ok(())
    }

    /// Keeps cache use and its durable timestamp indivisible with respect to cleanup.
    pub(crate) async fn hold_resources(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.inner.resource_changes.read().await
    }

    /// The runtimes of a host made of these parts.
    #[must_use]
    pub fn new(parts: RuntimeParts) -> Self {
        let cold_mounts = parts
            .ledger
            .all()
            .into_iter()
            .map(|record| record.runtime_id)
            .collect();
        Self {
            inner: Arc::new(Inner {
                drivers: parts.drivers,
                ledger: parts.ledger,
                volumes: parts.volumes,
                storages: parts.storages,
                images: parts.images,
                tools: parts.tools,
                reports: parts.reports,
                keys: parts.keys,
                clock: parts.clock,
                hooks: Hooks::new(parts.settings.hooks.clone()),
                settings: parts.settings,
                busy: parts.busy,
                copy_tasks: tokio_util::task::TaskTracker::new(),
                locks: Mutex::new(HashMap::new()),
                cold_mounts: Mutex::new(cold_mounts),
                watches: Mutex::new(HashMap::new()),
                watch_count: std::sync::atomic::AtomicU64::new(0),
                claims: Arc::new(Mutex::new(HashMap::new())),
                stop: CancellationToken::new(),
                ports: parts.ports,
                content: parts.content,
                policy: Mutex::new(None),
                resource_changes: tokio::sync::RwLock::new(()),
                link: Mutex::new(None),
            }),
        }
    }

    /// Share the connector's live, owner-controlled environment.roots policy; no host-only list.
    pub fn set_policy(&self, policy: Arc<orch_connector_core::policy::PolicyStore>) {
        self.inner.hooks.set_policy(Arc::clone(&policy));
        *lock(&self.inner.policy) = Some(policy);
    }

    /// Shares the node's authenticated connection with unit creation.
    pub fn set_link(&self, link: orch_link::machine::MachineLink) {
        *lock(&self.inner.link) = Some(link);
    }

    /// The addresses a unit must always be able to reach.
    #[must_use]
    pub fn gateway_addresses(&self) -> Vec<String> {
        if !self.inner.settings.managed_endpoint.is_empty() {
            return vec![crate::enrollment::endpoint(&self.inner.settings.managed_endpoint).url];
        }
        lock(&self.inner.link)
            .as_ref()
            .map(|link| {
                link.hosted_endpoints()
                    .into_iter()
                    .map(|endpoint| endpoint.url)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Reapplies each retained restriction against the node's newly authenticated address list.
    pub async fn refresh_control_addresses(&self) {
        for record in self.inner.ledger.all() {
            let serial = self.lock_of(record.runtime_id);
            let _one_at_a_time = serial.lock().await;
            let Some(policy) = self
                .inner
                .ledger
                .network(record.runtime_id)
                .filter(|policy| policy.restricted())
            else {
                continue;
            };
            if let Some(driver) = self.inner.drivers.get(&record.driver) {
                let network = self
                    .install_network(record.runtime_id, &driver, &record.unit, &policy)
                    .await;
                if !network.is_ready_for(&policy) {
                    self.unwatch(record.runtime_id);
                    let _ = driver
                        .contract()
                        .stop(&record.unit, self.inner.settings.stop_grace)
                        .await;
                    self.tell(record.runtime_id, &driver, &record.unit).await;
                }
            }
        }
    }

    fn check_data_directories(
        &self,
        spec: &mut orch_driver_contract::runtime::RuntimeSpec,
    ) -> Result<(), CommandError> {
        for (role, volume) in &mut spec.volumes {
            let Some(path) = &volume.directory else {
                continue;
            };
            if *role == orch_driver_contract::runtime::VolumeRole::State {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "the private connector state must be made by Orch",
                ));
            }
            let policy = lock(&self.inner.policy)
                .as_ref()
                .and_then(|store| store.policy())
                .ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::PolicyRootNotAllowed,
                        "this machine has not offered any environment.roots",
                    )
                })?;
            volume.directory = Some(orch_connector_core::policy::inspect_environment_directory(
                &policy,
                std::path::Path::new(&path.path),
            )?);
        }
        Ok(())
    }

    fn check_mount_paths(
        &self,
        mounts: &mut [orch_driver_contract::mounts::MountSpec],
    ) -> Result<(), CommandError> {
        // A physical machine can offer directory environments and unit drivers together. Its owned
        // directory data is one location, so unit drivers bind the very same recorded bytes.
        for mount in mounts
            .iter_mut()
            .filter(|mount| mount.kind == orch_driver_contract::mounts::MountKind::Volume)
        {
            if let Some(id) = mount
                .source
                .strip_prefix("orch-mount-")
                .and_then(|id| id.parse::<Id>().ok())
                && let Some(policy) = lock(&self.inner.policy)
                    .as_ref()
                    .and_then(|store| store.policy())
                && let Some(path) = orch_connector_core::storages::recorded_directory(
                    &self.inner.settings.data_dir,
                    &policy,
                    id,
                    "mount",
                )?
            {
                mount.kind = orch_driver_contract::mounts::MountKind::Path;
                mount.source = path.to_string_lossy().into_owned();
            }
        }
        for mount in mounts
            .iter_mut()
            .filter(|mount| mount.kind == orch_driver_contract::mounts::MountKind::Path)
        {
            let policy = lock(&self.inner.policy)
                .as_ref()
                .and_then(|store| store.policy())
                .ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::PolicyRootNotAllowed,
                        "this machine has not offered any environment.roots",
                    )
                })?;
            mount.source = orch_connector_core::policy::inspect_environment_path(
                &policy,
                std::path::Path::new(&mount.source),
                false,
            )?
            .path;
        }
        Ok(())
    }

    /// Stops watching the connectors of units that have not enrolled.
    pub fn stop_watching(&self) {
        self.inner.stop.cancel();
    }

    /// Abandons controller-side enrollment watches without requesting native copy cleanup.
    pub(crate) fn abandon_watches(&self) {
        for (_, (_, watch)) in std::mem::take(&mut *lock(&self.inner.watches)) {
            watch.cancel();
        }
    }

    /// Stops and joins data commands even when their original waiter was dropped.
    pub async fn stop_copy_work(&self) {
        self.inner.stop.cancel();
        self.inner.copy_tasks.close();
        self.inner.copy_tasks.wait().await;
    }

    /// Reports changed proxy state without applying stale policy or opening outward traffic.
    pub async fn report_networks(&self) {
        for record in self.inner.ledger.all() {
            if self.inner.busy.ids().contains(&record.runtime_id) {
                continue;
            }
            let serial = self.lock_of(record.runtime_id);
            let _one_at_a_time = serial.lock().await;
            let Some(policy) = self
                .inner
                .ledger
                .network(record.runtime_id)
                .filter(NetworkPolicy::restricted)
            else {
                continue;
            };
            let Some(host) = self.inner.drivers.get(&record.driver) else {
                continue;
            };
            let Ok(mut network) = host.contract().inspect_network(&record.unit).await else {
                continue;
            };
            network.required_mode = policy.mode;
            if !network.is_ready_for(&policy) {
                network.state = orch_driver_contract::network::NetworkReadiness::Unavailable;
                network.reason.get_or_insert_with(|| {
                    "the requested network is not currently verified".to_owned()
                });
            }
            if network.mode == EgressMode::Open {
                let _ = host
                    .contract()
                    .stop(&record.unit, self.inner.settings.stop_grace)
                    .await;
                self.tell(record.runtime_id, &host, &record.unit).await;
            }
            if let Ok(state) = network_state(&network) {
                self.inner.reports.network(record.runtime_id, state).await;
            }
        }
    }

    fn lock_of(&self, runtime_id: Id) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(lock(&self.inner.locks).entry(runtime_id).or_default())
    }

    async fn hold_volumes(
        &self,
        driver: &str,
        names: impl IntoIterator<Item = String>,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<Vec<tokio::sync::OwnedMutexGuard<()>>, CommandError> {
        let names: BTreeSet<_> = names.into_iter().collect();
        let mut held = Vec::with_capacity(names.len());
        for name in names {
            let guard = tokio::select! {
                biased;
                () = work.cancelled() => return Err(crate::work::cancelled()),
                guard = self.inner.volumes.hold(driver, &name) => guard,
            };
            held.push(guard);
        }
        crate::work::check(work)?;
        Ok(held)
    }

    /// Serializes the shared-name check with other starts and the actual volume lifecycle.
    async fn hold_start_volumes(&self, record: &Record) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        let names: BTreeSet<_> = record.volumes.iter().collect();
        let mut held = Vec::with_capacity(names.len() * 2);
        // Every start takes all shared-name locks first, then the driver-volume locks in the same order.
        for name in &names {
            held.push(self.inner.volumes.hold_start(name).await);
        }
        for name in names {
            held.push(self.inner.volumes.hold(&record.driver, name).await);
        }
        held
    }

    async fn volume_driver<T>(
        &self,
        driver: &HostDriver,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
        operation: impl std::future::Future<Output = Result<T, DriverError>>,
    ) -> Result<T, CommandError> {
        // The caller holds runtime and volume serialization outside this future, including on panic.
        let outcome = std::panic::AssertUnwindSafe(operation)
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(DriverError::Internal("the volume driver failed".into())));
        if outcome.is_err() || work.is_cancelled() {
            let command = work.command_id().ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Internal,
                    "the volume worker has no command identity",
                )
            })?;
            loop {
                match driver.extras().stop_volume_work(command).await {
                    Ok(()) => break,
                    Err(error) => {
                        tracing::warn!(%error, %command, "still stopping the volume helper");
                        tokio::time::sleep(self.inner.settings.reconcile_every).await;
                    }
                }
            }
        }
        crate::work::check(work)?;
        outcome.map_err(|error| failure(&error))
    }

    fn driver(&self, name: &str) -> Result<Arc<HostDriver>, CommandError> {
        self.inner.drivers.get(name).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                format!("this host has no driver {name:?}"),
            )
        })
    }

    /// The driver and unit of a runtime, from what the node recorded or else from what the drivers hold.
    async fn find(
        &self,
        runtime_id: Id,
    ) -> Result<Option<(Arc<HostDriver>, Record)>, CommandError> {
        if let Some(record) = self.inner.ledger.get(runtime_id) {
            let Some(driver) = self.inner.drivers.get(&record.driver) else {
                // The unit is there and the driver that holds it is not loaded now: nothing is done to the unit,
                // and nothing is forgotten about it.
                let why = self
                    .inner
                    .drivers
                    .failures()
                    .remove(&record.driver)
                    .unwrap_or_else(|| "it is not among the drivers of this node".to_owned());
                return Err(CommandError {
                    code: "unavailable".to_owned(),
                    detail: format!(
                        "the driver {} that holds the unit of the runtime {runtime_id} is not loaded: {why}",
                        record.driver
                    ),
                    retryable: true,
                    details: None,
                });
            };
            return Ok(Some((driver, record)));
        }
        let mut systems = self.inner.ledger.systems();
        if !systems.contains(&self.inner.settings.system_name) {
            systems.push(self.inner.settings.system_name.clone());
        }
        let found = locate(&self.inner.drivers, &systems, runtime_id).await;
        Ok(found.and_then(|(driver, unit)| {
            let host = self.inner.drivers.get(&driver)?;
            Some((
                host,
                Record {
                    runtime_id,
                    kind: unit.kind,
                    driver,
                    unit,
                    volumes: Vec::new(),
                    token: None,
                    enrolled: true,
                    resources: orch_driver_contract::runtime::Resources::default(),
                    image: None,
                    created_at: self.inner.clock.time(),
                    started_at: None,
                    enroll_timeout_secs: None,
                },
            ))
        }))
    }

    /// Tells the system the state the driver finds a runtime's unit in now, if it is not what it was last
    /// told.
    async fn tell(&self, runtime_id: Id, host: &HostDriver, unit: &UnitRef) {
        let state = match host.contract().inspect(unit).await {
            Ok(state) => state,
            Err(error) => {
                tracing::warn!(%runtime_id, %error, "the state of a unit could not be read");
                return;
            }
        };
        self.inner
            .reports
            .runtime_if_changed(
                runtime_id,
                state_name(state.state),
                self.inner
                    .ledger
                    .mount_detail(runtime_id, detail_of(&state)),
            )
            .await;
    }

    // ------------------------------------------------------------------------------------ create

    /// Carries out `node.runtime.create` for `command`, whose body is `body`.
    pub async fn create(
        &self,
        command: Id,
        body: Json,
        cancel: &CancellationToken,
    ) -> Result<NodeRuntimeCreateResult, CommandError> {
        let _resources = self.inner.resource_changes.read().await;
        let mut spec = managed::unseal(&self.inner.keys, command, body)?;
        let requested_driver = self.driver(&spec.driver)?;
        if let Some(link) = lock(&self.inner.link).as_ref() {
            let endpoint = if self.inner.settings.managed_endpoint.is_empty() {
                link.connected_endpoint()
            } else {
                Some(crate::enrollment::endpoint(
                    &self.inner.settings.managed_endpoint,
                ))
            };
            spec.gateway_url = endpoint
                .ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::OfflinePeer,
                        "the node has no verified connection address",
                    )
                })?
                .url;
        }
        if let Some(policy) = lock(&self.inner.policy)
            .as_ref()
            .and_then(|store| store.policy())
        {
            let template = spec.labels.get("orch.template_id").cloned().or_else(|| {
                spec.subject
                    .as_ref()
                    .and_then(|subject| subject.environment.as_ref())
                    .and_then(|environment| environment.template_id)
                    .map(|id| id.to_string())
            });
            policy
                .drivers
                .check(
                    &requested_driver.describe(),
                    &spec.spec,
                    template.as_deref(),
                    &spec.capabilities,
                )
                .map_err(|reason| CommandError::new(ErrorCode::Unsupported, reason))?;
            self.inner.drivers.check_configuration(
                &spec.driver,
                policy.drivers.configurations.get(&spec.driver),
            )?;
            spec.resources = policy
                .bounded_resources(spec.resources)
                .map_err(|reason| CommandError::new(ErrorCode::Unsupported, reason))?;
        }
        self.check_data_directories(&mut spec)?;
        self.check_mount_paths(&mut spec.mounts)?;
        let checked_roots: BTreeMap<_, _> = spec
            .volumes
            .values()
            .filter_map(|volume| {
                volume
                    .directory
                    .as_ref()
                    .map(|directory| (volume.name.clone(), directory.clone()))
            })
            .collect();
        if self.inner.ledger.get(spec.runtime_id).is_some()
            && self.inner.ledger.directories(spec.runtime_id) != checked_roots
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "an existing data directory changed; recreate with an allowed directory instead of changing a unit in place",
            ));
        }
        let _busy = self.inner.busy.enter(spec.runtime_id);
        let serial = self.lock_of(spec.runtime_id);
        let _one_at_a_time = serial.lock().await;
        let host = self.driver(&spec.driver)?;
        if self
            .inner
            .ledger
            .network(spec.runtime_id)
            .is_some_and(|policy| policy.restricted())
            && !spec.network_policy.restricted()
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "kept stopped: the retained restriction requires a verified replacement or an explicit open plan",
            ));
        }
        self.inner
            .ledger
            .set_network(spec.runtime_id, spec.network_policy.clone())
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        let descriptor = host.describe();
        if spec
            .volumes
            .values()
            .any(|volume| volume.directory.is_some())
            && !descriptor.data_storage.existing_directory
        {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                descriptor.data_storage.reason.clone().unwrap_or_else(|| {
                    "this driver has not offered existing directories; choose data made by Orch"
                        .into()
                }),
            ));
        }
        if !descriptor.makes_units_for(spec.kind) {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                format!(
                    "the driver {} makes no units for a runtime of kind {}",
                    spec.driver,
                    spec.kind.as_str()
                ),
            ));
        }
        if spec.kept_image.is_none()
            && let Ok(source) = Source::from_spec(&spec.spec)
            && !descriptor.sources.is_empty()
            && !source.is_offered(&descriptor.sources)
        {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                format!(
                    "this host does not offer {:?} as the source of a unit's file system",
                    source.kind()
                ),
            ));
        }
        if spec.kept_image.is_none() {
            host.contract()
                .validate(spec.kind, &spec.spec)
                .await
                .map_err(|error| failure(&error))?;
        }
        let checked = managed::check(&spec, &self.inner.tools)?;
        if spec
            .labels
            .get("orch.restore")
            .is_some_and(|value| value == "true")
        {
            self.verify_kept(&spec).await?;
        }
        for volume in spec
            .volumes
            .values()
            .filter(|volume| volume.directory.is_none())
        {
            self.inner
                .volumes
                .check(&host, &volume.name, volume.storage.as_deref())?;
        }
        // What one environment wrote in a private view is a volume of its own, on a storage of the host.
        for layer in spec.mounts.iter().filter_map(|mount| mount.layer.as_ref()) {
            self.inner
                .volumes
                .check(&host, &layer.name, layer.storage.as_deref())?;
        }
        let _claimed = self.claim_token(checked.token, spec.runtime_id)?;
        if let Some(system) = spec.labels.get(LABEL_SYSTEM)
            && let Err(error) = self.inner.ledger.adopt_system(system)
        {
            tracing::warn!(%error, "the system a unit belongs to could not be noted");
        }
        // The image first: pulled, built or loaded, so that the unit is made from what is there.
        if spec.kept_image.is_none() {
            self.inner
                .images
                .ensure(command, &spec.driver, &spec.spec, None, cancel)
                .await?;
        }
        spec.capabilities.check().map_err(|error| failure(&error))?;
        let unit = host
            .contract()
            .create(&spec)
            .await
            .map_err(|error| self.failed_for_room(&error))?;
        if let Some(image) = &spec.kept_image {
            self.inner
                .ledger
                .set_kept_for(spec.runtime_id, image.clone())
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        }
        if self.inner.ledger.capabilities(spec.runtime_id).is_none() {
            self.inner
                .ledger
                .set_capabilities(
                    spec.runtime_id,
                    crate::ledger::CapabilityConfiguration {
                        desired: spec.capabilities.clone(),
                        applied: spec.capabilities.clone(),
                        changing: false,
                        applied_started_at: None,
                    },
                )
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        }
        let before = self.inner.ledger.get(spec.runtime_id);
        self.inner
            .ledger
            .set_directories(
                spec.runtime_id,
                spec.volumes
                    .values()
                    .filter_map(|volume| {
                        volume
                            .directory
                            .as_ref()
                            .map(|directory| (volume.name.clone(), directory.clone()))
                    })
                    .collect(),
            )
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        let first_record = before.is_none();
        let record = Record {
            runtime_id: spec.runtime_id,
            kind: spec.kind,
            driver: spec.driver.clone(),
            unit: unit.clone(),
            volumes: spec
                .volumes
                .values()
                .map(|volume| volume.name.clone())
                .chain(
                    spec.mounts
                        .iter()
                        .filter_map(|mount| mount.layer.as_ref())
                        .map(|layer| layer.name.clone()),
                )
                .collect(),
            token: Some(checked.token),
            enrolled: before.as_ref().is_some_and(|before| before.enrolled),
            resources: spec.resources,
            image: Images::use_of(&spec.spec),
            created_at: before
                .as_ref()
                .map_or_else(|| self.inner.clock.time(), |before| before.created_at),
            started_at: before.and_then(|before| before.started_at),
            // The runtime a unit is made under may need longer to enroll than a plain container does.
            enroll_timeout_secs: host
                .describe()
                .runtime_class(spec.spec.get("runtime").and_then(Json::as_str))
                .and_then(|class| class.enrol_timeout_secs),
        };
        if let Err(error) = self.inner.ledger.insert(record) {
            // A unit the node could not record is a unit nobody could find again: it is not kept.
            let _ = host.contract().remove(&unit).await;
            return Err(CommandError::new(ErrorCode::Internal, error.to_string()));
        }
        // What the unit serves and why it was made, for the operator's hooks at each of its moments.
        if let Err(error) = self
            .inner
            .ledger
            .set_about(spec.runtime_id, UnitAbout::of(&spec))
        {
            tracing::warn!(runtime = %spec.runtime_id, %error, "what a unit serves could not be noted for its hooks");
        }
        if first_record {
            let reserved = ["workspace_mount", "home_mount", "state_path"]
                .iter()
                .zip(["/workspace", "/home/agent", "/var/lib/orch"])
                .map(|(key, default)| {
                    spec.spec
                        .get(*key)
                        .and_then(Json::as_str)
                        .unwrap_or(default)
                        .to_owned()
                })
                .collect();
            self.inner
                .ledger
                .set_mounts(
                    spec.runtime_id,
                    crate::ledger::MountConfiguration {
                        base: spec.mounts.clone(),
                        wanted: spec.mounts.clone(),
                        reserved,
                        applied: spec.mounts.clone(),
                        applied_started_at: None,
                        pending_reason: None,
                        retry_pending: false,
                    },
                )
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        }
        self.tell(spec.runtime_id, &host, &unit).await;
        Ok(NodeRuntimeCreateResult { unit })
    }

    /// Claims the enrollment token for the runtime, or says who has it. A token is single-use: it is given to
    /// one unit, and a new unit gets a token of its own.
    fn claim_token(&self, token: Hash, runtime_id: Id) -> Result<Claim, CommandError> {
        let taken = |who: &str| {
            CommandError::new(
                ErrorCode::Conflict,
                format!(
                    "this enrollment token was given to {who} already; a token is single-use, and a new unit gets one of its own"
                ),
            )
        };
        if let Some(holder) = self.inner.ledger.holder_of_token(runtime_id, token) {
            return Err(taken(&format!("the unit {}", holder.unit.name)));
        }
        let mut claims = lock(&self.inner.claims);
        match claims.get(&token) {
            Some(other) if *other != runtime_id => {
                Err(taken(&format!("the request for the runtime {other}")))
            }
            _ => {
                claims.insert(token, runtime_id);
                Ok(Claim {
                    claims: Arc::clone(&self.inner.claims),
                    token,
                })
            }
        }
    }

    /// The failure of a command for a failure of a driver. When the host says it is out of room, the units that
    /// are there are said to declare what they do, so that whoever reads it can see why.
    fn failed_for_room(&self, error: &DriverError) -> CommandError {
        let mut failed = failure(error);
        if failed.code == NO_CAPACITY {
            let records = self.inner.ledger.all();
            let started: std::collections::HashSet<Id> = records
                .iter()
                .filter(|record| record.started_at.is_some())
                .map(|record| record.runtime_id)
                .collect();
            let sum = declared(&records, &started);
            let host = capacity(&self.inner.storages);
            failed.detail = format!(
                "{} (the units on this host declare {} MiB of memory and {} GB of disk; the host has {} MiB and {} GB)",
                failed.detail, sum.memory_mb, sum.disk_gb, host.memory_mb, host.disk_gb
            );
        }
        failed
    }

    // ------------------------------------------------------------------------------------ start, stop, remove

    /// Carries out `node.runtime.start`.
    pub async fn start(&self, runtime_id: Id) -> Result<(), CommandError> {
        let _resources = self.inner.resource_changes.read().await;
        let _busy = self.inner.busy.enter(runtime_id);
        let serial = self.lock_of(runtime_id);
        let _one_at_a_time = serial.lock().await;
        let (host, record) = self.require(runtime_id).await?;
        crate::resources::runtime_occupancy(
            &self.inner.drivers,
            &self.inner.tools,
            &self.inner.ledger,
            runtime_id,
            self.inner.clock.time(),
        )
        .await?;
        for directory in self.inner.ledger.directories(runtime_id).values() {
            let policy = lock(&self.inner.policy)
                .as_ref()
                .and_then(|store| store.policy())
                .ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::PolicyRootNotAllowed,
                        "this machine has no environment.roots policy",
                    )
                })?;
            if orch_connector_core::policy::inspect_environment_directory(
                &policy,
                std::path::Path::new(&directory.path),
            )? != *directory
            {
                return Err(CommandError::new(
                    ErrorCode::PolicyRootNotAllowed,
                    "the existing directory changed since this unit was made; recreate with an allowed directory",
                ));
            }
        }
        self.recover_interrupted_mounts(runtime_id, &host, &record)
            .await?;
        let _volume_holds = self.hold_start_volumes(&record).await;
        self.keep_off_shared_volumes(&record).await?;
        let policy = self.inner.ledger.network(runtime_id).unwrap_or_default();
        let network = self
            .install_network(runtime_id, &host, &record.unit, &policy)
            .await;
        if !network.is_ready_for(&policy) {
            self.unwatch(runtime_id);
            let _ = host
                .contract()
                .stop(&record.unit, self.inner.settings.stop_grace)
                .await;
            self.tell(runtime_id, &host, &record.unit).await;
            return Err(CommandError {
                code: "network.restricted_unavailable".to_owned(),
                detail: network
                    .reason
                    .unwrap_or_else(|| "the current network policy is not verified".to_owned()),
                retryable: true,
                details: None,
            });
        }
        // The operator's up hooks run around a start that brings the unit up, never around one that finds it
        // running; they fail closed.
        let hooked = self.inner.hooks.any() && !self.runs(&host, &record).await;
        let reason = UnitAbout::up_reason(
            self.inner.ledger.about(runtime_id).as_ref(),
            record.started_at.is_none(),
        );
        if hooked
            && let Some(run) = self
                .hook(HookEvent::PreUp, reason, false, &host, &record)
                .await
            && run.failed
        {
            self.tell(runtime_id, &host, &record.unit).await;
            return Err(hook_failure(&run));
        }
        self.start_capabilities(&host, &record).await?;
        crate::resources::runtime_occupancy(
            &self.inner.drivers,
            &self.inner.tools,
            &self.inner.ledger,
            runtime_id,
            self.inner.clock.time(),
        )
        .await?;
        self.restore_live_mounts(runtime_id, &host, &record).await;
        if hooked {
            if let Some(run) = self
                .hook(HookEvent::PostUp, reason, false, &host, &record)
                .await
                && run.failed
            {
                self.close_after_failed_up(&host, &record).await;
                self.tell(runtime_id, &host, &record.unit).await;
                return Err(hook_failure(&run));
            }
            self.note_phase(runtime_id, HookPhase::Up);
        }
        let now = self.inner.clock.time();
        let _ = self
            .inner
            .ledger
            .update(runtime_id, |record| record.started_at = Some(now));
        self.report_addresses(runtime_id, &host, &record, &policy)
            .await;
        self.tell(runtime_id, &host, &record.unit).await;
        if !record.enrolled {
            self.watch(runtime_id, self.enroll_time(&record));
        }
        Ok(())
    }

    /// Carries out `node.runtime.stop`: the unit is given `[node] stop_grace_secs` to end by itself. The operator's
    /// down hooks run around the stop of a unit that came up, told `reason` (an ordinary stop where none is given),
    /// and never hold the stop.
    pub async fn stop(
        &self,
        runtime_id: Id,
        reason: Option<UnitReason>,
    ) -> Result<(), CommandError> {
        let _resources = self.inner.resource_changes.read().await;
        let _busy = self.inner.busy.enter(runtime_id);
        let serial = self.lock_of(runtime_id);
        let _one_at_a_time = serial.lock().await;
        let (host, record) = self.require(runtime_id).await?;
        let occupied = crate::resources::runtime_occupancy(
            &self.inner.drivers,
            &self.inner.tools,
            &self.inner.ledger,
            runtime_id,
            self.inner.clock.time(),
        )
        .await?;
        self.unwatch(runtime_id);
        let reason = reason.unwrap_or(UnitReason::Stop);
        let running = self.inner.hooks.any() && self.runs(&host, &record).await;
        let came_up = running || self.came_up(runtime_id);
        if running {
            let _ = self
                .hook(HookEvent::PreDown, reason, false, &host, &record)
                .await;
        }
        host.contract()
            .stop(&record.unit, self.inner.settings.stop_grace)
            .await
            .map_err(|error| failure(&error))?;
        let actual = host
            .contract()
            .inspect(&record.unit)
            .await
            .map_err(|error| failure(&error))?;
        if actual.state != UnitStateKind::Stopped {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the host did not confirm that the unit stopped",
            ));
        }
        self.inner
            .ledger
            .resource_occupancy_ended(&occupied, self.inner.clock.time())
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        if came_up {
            let _ = self
                .hook(HookEvent::PostDown, reason, false, &host, &record)
                .await;
            self.note_phase(runtime_id, HookPhase::Down);
        }
        self.tell(runtime_id, &host, &record.unit).await;
        Ok(())
    }

    /// Carries out `node.runtime.remove`: the unit goes and its volumes stay. A runtime the node has no unit
    /// for is already removed. A unit removed while it runs, or that came up and ended by itself, is given the
    /// operator's down hooks, told of a removal; `post_down` is told the networks the unit stood on, which go with it.
    pub async fn remove(&self, runtime_id: Id) -> Result<(), CommandError> {
        let _resources = self.inner.resource_changes.read().await;
        let _busy = self.inner.busy.enter(runtime_id);
        let serial = self.lock_of(runtime_id);
        let _one_at_a_time = serial.lock().await;
        self.unwatch(runtime_id);
        let Some((host, record)) = self.find(runtime_id).await? else {
            self.forget(runtime_id).await;
            return Ok(());
        };
        let occupied = crate::resources::runtime_occupancy(
            &self.inner.drivers,
            &self.inner.tools,
            &self.inner.ledger,
            runtime_id,
            self.inner.clock.time(),
        )
        .await?;
        let running = self.inner.hooks.any() && self.runs(&host, &record).await;
        let came_up = running || self.came_up(runtime_id);
        if running {
            let _ = self
                .hook(
                    HookEvent::PreDown,
                    UnitReason::Delete,
                    false,
                    &host,
                    &record,
                )
                .await;
        }
        // Built before the unit and its networks go, so that the hook is told what it is to undo.
        let after = if came_up && self.inner.hooks.wants(HookEvent::PostDown) {
            Some(
                self.hook_document(
                    HookEvent::PostDown,
                    UnitReason::Delete,
                    false,
                    &host,
                    &record,
                )
                .await,
            )
        } else {
            None
        };
        match host.contract().remove(&record.unit).await {
            Ok(()) | Err(DriverError::NotFound(_)) => {}
            Err(error) => return Err(failure(&error)),
        }
        self.inner
            .ledger
            .resource_occupancy_ended(&occupied, self.inner.clock.time())
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        if let Some(mut document) = after {
            document.at = self.inner.clock.time();
            let _ = self.run_hook(record.runtime_id, &document).await;
        }
        self.forget(runtime_id).await;
        Ok(())
    }

    // ------------------------------------------------------------------------------------ files that move with a unit

    /// The node's means of moving content, or why it has none.
    fn content(&self) -> Result<&Arc<orch_connector_core::content::Content>, CommandError> {
        self.inner.content.as_ref().ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "this node moves no files between hosts",
            )
        })
    }

    /// Carries out `node.runtime.export`: the files a stopped unit keeps below `path` of its state are put up as one
    /// archive for `command`, with their owners and modes; nothing is put up where it keeps none.
    pub async fn export(
        &self,
        command: Id,
        body: &orch_link_contract::commands::NodeRuntimeExport,
    ) -> Result<orch_link_contract::commands::NodeRuntimeExportResult, CommandError> {
        if !orch_link_contract::commands::state_path_is_inside(&body.path) {
            return Err(outside_state());
        }
        let _busy = self.inner.busy.enter(body.runtime_id);
        let serial = self.lock_of(body.runtime_id);
        let _one_at_a_time = serial.lock().await;
        let (host, record) = self.require(body.runtime_id).await?;
        if self.runs(&host, &record).await {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the unit runs; its files are taken only once it has stopped, so that they stand still",
            ));
        }
        let content = self.content()?;
        let directory = content.directory_of(command);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| {
                CommandError::new(
                    ErrorCode::Internal,
                    format!("no room for the archive: {error}"),
                )
            })?;
        let archive = directory.join(MOVE_ARCHIVE);
        let outcome = async {
            let kept = host
                .extras()
                .export_state(&record.unit, &body.path, &archive)
                .await
                .map_err(|error| failure(&error))?;
            if !kept {
                return Ok(None);
            }
            content.put(command, &archive).await.map(Some)
        }
        .await;
        content.finish(command).await;
        Ok(orch_link_contract::commands::NodeRuntimeExportResult { archive: outcome? })
    }

    /// Carries out `node.runtime.import`: the archive is fetched for `command` and its files put into the unit's
    /// state, with their owners and modes, before the unit first starts.
    pub async fn import(
        &self,
        command: Id,
        body: &orch_link_contract::commands::NodeRuntimeImport,
    ) -> Result<(), CommandError> {
        if !orch_link_contract::commands::state_path_is_inside(&body.path) {
            return Err(outside_state());
        }
        let _busy = self.inner.busy.enter(body.runtime_id);
        let serial = self.lock_of(body.runtime_id);
        let _one_at_a_time = serial.lock().await;
        let (host, record) = self.require(body.runtime_id).await?;
        if record.started_at.is_some() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the unit has run already; files are given to a unit only before its first start",
            ));
        }
        let content = self.content()?;
        let outcome = async {
            let archive = content
                .fetch(command, body.archive.hash, body.archive.size, MOVE_ARCHIVE)
                .await?;
            host.extras()
                .import_state(&record.unit, &archive)
                .await
                .map_err(|error| failure(&error))
        }
        .await;
        content.finish(command).await;
        outcome
    }

    // ------------------------------------------------------------------------------------ the operator's hooks

    /// Whether a runtime's unit runs now; a unit the driver cannot say of is taken as not running.
    async fn runs(&self, host: &HostDriver, record: &Record) -> bool {
        host.contract()
            .inspect(&record.unit)
            .await
            .is_ok_and(|state| state.state == UnitStateKind::Running)
    }

    /// Whether the operator's hooks last saw a runtime's unit come up.
    fn came_up(&self, runtime_id: Id) -> bool {
        self.inner.ledger.hook_phase(runtime_id) == Some(HookPhase::Up)
    }

    /// Notes that a unit came up or went down, where the operator named hooks.
    fn note_phase(&self, runtime_id: Id, phase: HookPhase) {
        if !self.inner.hooks.any() {
            return;
        }
        if let Err(error) = self.inner.ledger.set_hook_phase(runtime_id, phase) {
            tracing::warn!(%runtime_id, %error, "whether a unit came up could not be noted for its hooks");
        }
    }

    /// What the operator's hook of `event` is told of a runtime's unit: what it serves, its networks and addresses
    /// now, and the host openings the node holds for its environment.
    async fn hook_document(
        &self,
        event: HookEvent,
        reason: UnitReason,
        replay: bool,
        host: &HostDriver,
        record: &Record,
    ) -> HookDocument {
        let about = self.inner.ledger.about(record.runtime_id);
        let network = host.contract().inspect_network(&record.unit).await.ok();
        let subject = about.as_ref().and_then(|about| about.subject.clone());
        let ports = match (
            about.as_ref().and_then(|about| about.environment_id),
            &self.inner.ports,
        ) {
            (Some(environment), Some(ports)) => ports.ports_of(environment),
            _ => Vec::new(),
        };
        HookDocument {
            version: DOCUMENT_VERSION,
            event,
            reason,
            replay,
            at: self.inner.clock.time(),
            host: HookHost {
                system: self.inner.settings.system_name.clone(),
                name: sysinfo::System::host_name().unwrap_or_default(),
            },
            runtime: HookRuntime {
                id: record.runtime_id,
                kind: record.kind,
                driver: record.driver.clone(),
            },
            environment: subject
                .as_ref()
                .and_then(|subject| subject.environment.clone()),
            provider: subject.and_then(|subject| subject.provider),
            unit: HookUnit {
                name: record.unit.name.clone(),
                id: record.unit.reference.clone(),
            },
            network: HookNetwork {
                mode: about
                    .as_ref()
                    .and_then(|about| about.network_mode.clone())
                    .unwrap_or_else(|| "bridge".to_owned()),
                egress: network
                    .as_ref()
                    .map_or(EgressMode::Open, |network| network.mode),
                networks: network.map(|network| network.networks).unwrap_or_default(),
            },
            ports,
        }
    }

    /// Runs a hook with its document and reports the run.
    async fn run_hook(&self, runtime_id: Id, document: &HookDocument) -> Option<HookRun> {
        let run = self
            .inner
            .hooks
            .run(document, self.inner.clock.as_ref())
            .await?;
        self.inner.reports.hook(run.report(runtime_id)).await;
        Some(run)
    }

    /// Runs the operator's hook of `event` for a runtime's unit and reports the run; none when the operator named no
    /// hook for that moment.
    async fn hook(
        &self,
        event: HookEvent,
        reason: UnitReason,
        replay: bool,
        host: &HostDriver,
        record: &Record,
    ) -> Option<HookRun> {
        if !self.inner.hooks.wants(event) {
            return None;
        }
        let document = self
            .hook_document(event, reason, replay, host, record)
            .await;
        self.run_hook(record.runtime_id, &document).await
    }

    /// A unit whose up hook failed does not stay up: its down hooks run around its stop, as for any stop, and the
    /// start that brought it up fails whatever they do.
    async fn close_after_failed_up(&self, host: &HostDriver, record: &Record) {
        let _ = self
            .hook(HookEvent::PreDown, UnitReason::Stop, false, host, record)
            .await;
        if let Err(error) = host
            .contract()
            .stop(&record.unit, self.inner.settings.stop_grace)
            .await
        {
            tracing::warn!(runtime = %record.runtime_id, %error, "a unit whose up hook failed could not be stopped");
        }
        let _ = self
            .hook(HookEvent::PostDown, UnitReason::Stop, false, host, record)
            .await;
        self.note_phase(record.runtime_id, HookPhase::Down);
    }

    /// Tells the system an open unit's networks once it runs, with its address there; a restricted unit's are told
    /// with its proxy's state. The revisions in force are the policy's, so that nothing else is taken as changed.
    async fn report_addresses(
        &self,
        runtime_id: Id,
        host: &HostDriver,
        record: &Record,
        policy: &NetworkPolicy,
    ) {
        let Ok(mut network) = host.contract().inspect_network(&record.unit).await else {
            return;
        };
        if network.mode != EgressMode::Open || policy.mode != EgressMode::Open {
            return;
        }
        network.required_mode = policy.mode;
        network.revision.clone_from(&policy.revision);
        network.always_revision.clone_from(&policy.always_revision);
        if let Ok(state) = network_state(&network) {
            self.inner.reports.network(runtime_id, state).await;
        }
    }

    /// Runs the operator's hooks again for the units a starting node finds: `post_up` for each that runs, as a
    /// replay (a failure stops it, closed, as at a start), and `post_down` for each that came up and ended while the
    /// node was away.
    pub async fn replay_hooks(&self) {
        if !self.inner.hooks.any() {
            return;
        }
        for record in self.inner.ledger.all() {
            let serial = self.lock_of(record.runtime_id);
            let _one_at_a_time = serial.lock().await;
            let Some(host) = self.inner.drivers.get(&record.driver) else {
                continue;
            };
            if self.runs(&host, &record).await {
                if let Some(run) = self
                    .hook(HookEvent::PostUp, UnitReason::Start, true, &host, &record)
                    .await
                    && run.failed
                {
                    tracing::warn!(runtime = %record.runtime_id, "{}", run.why());
                    self.close_after_failed_up(&host, &record).await;
                    self.inner
                        .reports
                        .runtime(
                            record.runtime_id,
                            state_name(UnitStateKind::Stopped),
                            Some(run.why()),
                        )
                        .await;
                    continue;
                }
                self.note_phase(record.runtime_id, HookPhase::Up);
            } else if self.came_up(record.runtime_id) {
                let _ = self
                    .hook(HookEvent::PostDown, UnitReason::Stop, true, &host, &record)
                    .await;
                self.note_phase(record.runtime_id, HookPhase::Down);
            }
        }
    }

    /// Gives `post_down` to each unit that came up and has ended by itself since; units a command is being carried
    /// out for are left to it.
    pub async fn settle_hooks(&self) {
        if !self.inner.hooks.any() {
            return;
        }
        for record in self.inner.ledger.all() {
            if !self.came_up(record.runtime_id)
                || self.inner.busy.ids().contains(&record.runtime_id)
            {
                continue;
            }
            let serial = self.lock_of(record.runtime_id);
            let _one_at_a_time = serial.lock().await;
            let Some(host) = self.inner.drivers.get(&record.driver) else {
                continue;
            };
            if self.came_up(record.runtime_id) && !self.runs(&host, &record).await {
                let _ = self
                    .hook(HookEvent::PostDown, UnitReason::Stop, false, &host, &record)
                    .await;
                self.note_phase(record.runtime_id, HookPhase::Down);
            }
        }
    }

    /// Takes a runtime out of the node's memory and tells the system its unit is gone.
    async fn forget(&self, runtime_id: Id) {
        if let Err(error) = self.inner.ledger.remove(runtime_id) {
            tracing::warn!(%runtime_id, %error, "a removed runtime could not be taken out of the ledger");
        }
        lock(&self.inner.locks).remove(&runtime_id);
        self.inner
            .reports
            .runtime_if_changed(runtime_id, state_name(UnitStateKind::Absent), None)
            .await;
    }

    async fn require(&self, runtime_id: Id) -> Result<(Arc<HostDriver>, Record), CommandError> {
        self.find(runtime_id).await?.ok_or_else(|| {
            CommandError::new(
                ErrorCode::NotFound,
                format!("this host has no unit for the runtime {runtime_id}"),
            )
        })
    }

    async fn install_network(
        &self,
        runtime: Id,
        host: &HostDriver,
        unit: &UnitRef,
        policy: &NetworkPolicy,
    ) -> UnitNetwork {
        let network = match host.contract().apply_network(unit, policy).await {
            Ok(network) => network,
            Err(error) => {
                let mut network = UnitNetwork::unavailable(
                    policy,
                    "the requested network could not be verified; kept stopped",
                );
                // A requested restriction never changes an old open unit's actual mode.
                if matches!(error, DriverError::Unsupported(_)) {
                    network.mode = EgressMode::Open;
                }
                network
            }
        };
        if let Ok(state) = network_state(&network) {
            self.inner.reports.network(runtime, state).await;
        }
        network
    }

    /// Installs the current policy, retaining its requirement before trying the driver.
    pub async fn apply_network(
        &self,
        body: &NodeNetworkApply,
    ) -> Result<orch_api_contract::environment::NetworkState, CommandError> {
        let _busy = self.inner.busy.enter(body.runtime_id);
        let serial = self.lock_of(body.runtime_id);
        let _one_at_a_time = serial.lock().await;
        let (host, record) = self.require(body.runtime_id).await?;
        let policy = NetworkPolicy {
            mode: match body.required_mode {
                orch_api_contract::environment::EgressMode::Open => EgressMode::Open,
                orch_api_contract::environment::EgressMode::Restricted => EgressMode::Restricted,
            },
            allowed_hosts: body.allowed_hosts.clone(),
            // A caller's transport list is never authority for the node's trusted endpoint set.
            always: Vec::new(),
            always_revision: String::new(),
            revision: body.revision.clone(),
        };
        self.inner
            .ledger
            .set_network(body.runtime_id, policy.clone())
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        let network = self
            .install_network(body.runtime_id, &host, &record.unit, &policy)
            .await;
        if !network.is_ready_for(&policy) {
            self.unwatch(body.runtime_id);
            host.contract()
                .stop(&record.unit, self.inner.settings.stop_grace)
                .await
                .map_err(|error| failure(&error))?;
            self.tell(body.runtime_id, &host, &record.unit).await;
        }
        network_state(&network)
    }

    /// Two units never run on one set of volumes: a unit that stands on a volume another unit runs on is not
    /// started. That is the order of a recreate, in which the old unit is stopped before its successor runs, and
    /// of its undoing, in which the successor is removed before the old unit is started again.
    async fn keep_off_shared_volumes(&self, record: &Record) -> Result<(), CommandError> {
        for other in self
            .inner
            .ledger
            .sharing_volumes(record.runtime_id, &record.volumes)
        {
            let Some(host) = self.inner.drivers.get(&other.driver) else {
                return Err(failure(&DriverError::Unavailable(format!(
                    "the driver {} of the unit {} is unavailable, so its volumes cannot be safely reused",
                    other.driver, other.unit.name
                ))));
            };
            let state = host
                .contract()
                .inspect(&other.unit)
                .await
                .map_err(|error| failure(&error))?;
            if !successor_may_start(true, state.state) {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    format!(
                        "the unit {} is running on the volumes of {}; two units never run on one set of volumes, so it is stopped first",
                        other.unit.name, record.unit.name
                    ),
                ));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------ enrollment

    fn unwatch(&self, runtime_id: Id) {
        if let Some((_, watch)) = lock(&self.inner.watches).remove(&runtime_id) {
            watch.cancel();
        }
    }

    /// Starts watching a unit for its connector to enroll within `time`.
    pub fn watch(&self, runtime_id: Id, time: Duration) {
        let cancel = self.inner.stop.child_token();
        let mine = self
            .inner
            .watch_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some((_, before)) =
            lock(&self.inner.watches).insert(runtime_id, (mine, cancel.clone()))
        {
            before.cancel();
        }
        let runtimes = self.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + time;
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(ENROLL_LOOK_EVERY) => {}
                }
                match runtimes
                    .look_for_enrollment(runtime_id, deadline, &cancel)
                    .await
                {
                    Look::Again => {}
                    Look::Done => break,
                }
            }
            let mut watches = lock(&runtimes.inner.watches);
            if watches
                .get(&runtime_id)
                .is_some_and(|(theirs, _)| *theirs == mine)
            {
                watches.remove(&runtime_id);
            }
        });
    }

    /// How long the connector of a unit has to enroll: what its runtime needs where that is more than the node's own
    /// figure, else the node's.
    fn enroll_time(&self, record: &Record) -> Duration {
        record
            .enroll_timeout_secs
            .map_or(self.inner.settings.enroll_timeout, |secs| {
                Duration::from_secs(secs).max(self.inner.settings.enroll_timeout)
            })
    }

    /// Takes up the watch of the units that were started and whose connectors had not enrolled when the node
    /// went away.
    pub fn rewatch(&self) {
        for record in self.inner.ledger.all() {
            if !record.enrolled && record.started_at.is_some() {
                self.watch(record.runtime_id, self.enroll_time(&record));
            }
        }
    }

    async fn look_for_enrollment(
        &self,
        runtime_id: Id,
        deadline: tokio::time::Instant,
        cancel: &CancellationToken,
    ) -> Look {
        let serial = self.lock_of(runtime_id);
        let _one_at_a_time = serial.lock().await;
        // A command that stops or removes the unit cancels the watch before it acts, and holds the lock
        // while it does: what is seen after the lock is the unit as that command left it.
        if cancel.is_cancelled() {
            return Look::Done;
        }
        let Some(record) = self.inner.ledger.get(runtime_id) else {
            return Look::Done;
        };
        if record.enrolled {
            return Look::Done;
        }
        let Some(host) = self.inner.drivers.get(&record.driver) else {
            return Look::Done;
        };
        let enrolled = host.extras().enrolled(&record.unit).await;
        if cancel.is_cancelled() {
            return Look::Done;
        }
        if enrolled == Some(true) {
            let _ = self
                .inner
                .ledger
                .update(runtime_id, |record| record.enrolled = true);
            return Look::Done;
        }
        let state = match host.contract().inspect(&record.unit).await {
            Ok(state) => state,
            Err(error) => {
                tracing::warn!(%runtime_id, %error, "a unit could not be looked at");
                return Look::Again;
            }
        };
        if cancel.is_cancelled() {
            return Look::Done;
        }
        if state.state != UnitStateKind::Running {
            // A failed restriction intentionally stops the unit; it must not turn into data removal after restart.
            if let Some(policy) = self
                .inner
                .ledger
                .network(runtime_id)
                .filter(NetworkPolicy::restricted)
                && !host
                    .contract()
                    .inspect_network(&record.unit)
                    .await
                    .is_ok_and(|network| network.is_ready_for(&policy))
            {
                return Look::Done;
            }
            let why = format!(
                "its unit ended before its connector enrolled{}",
                detail_of(&state).map_or_else(String::new, |detail| format!(": {detail}"))
            );
            self.give_up(runtime_id, &host, &record, &why).await;
            return Look::Done;
        }
        if enrolled.is_none() {
            // The driver cannot tell, and the node does not judge what it cannot see.
            return Look::Done;
        }
        if tokio::time::Instant::now() >= deadline {
            let why = format!(
                "its connector did not enroll within {} seconds",
                self.enroll_time(&record).as_secs()
            );
            self.give_up(runtime_id, &host, &record, &why).await;
            return Look::Done;
        }
        Look::Again
    }

    /// A unit whose connector will not enroll is reported failed, with why, and removed: its volumes stay.
    async fn give_up(&self, runtime_id: Id, host: &HostDriver, record: &Record, why: &str) {
        tracing::warn!(%runtime_id, unit = %record.unit.name, "{why}");
        let _ = host
            .contract()
            .stop(&record.unit, self.inner.settings.stop_grace)
            .await;
        if let Err(error) = host.contract().remove(&record.unit).await
            && !matches!(error, DriverError::NotFound(_))
        {
            tracing::warn!(%runtime_id, %error, "a unit that did not enroll could not be removed");
        }
        if let Err(error) = self.inner.ledger.remove(runtime_id) {
            tracing::warn!(%runtime_id, %error, "a unit that did not enroll could not be taken out of the ledger");
        }
        self.inner
            .reports
            .runtime_if_changed(
                runtime_id,
                state_name(UnitStateKind::Failed),
                Some(why.to_owned()),
            )
            .await;
    }
}

enum Look {
    Again,
    Done,
}

/// The name of the file a moving unit's archive is kept in while it is put up or fetched.
const MOVE_ARCHIVE: &str = "state.tar";

/// The refusal of a path that does not stay inside a unit's state.
fn outside_state() -> CommandError {
    CommandError {
        code: crate::drivers::INVALID.to_owned(),
        detail: "the path stays inside the unit's state: plain names, nothing absolute, no `..`"
            .to_owned(),
        retryable: false,
        details: None,
    }
}

/// The code of a start an up hook of the operator's kept from running.
pub const HOOK_FAILED: &str = "hook.failed";

/// The failure of a start whose up hook failed: what the hook did and the first lines it printed, for whoever manages
/// the environment. It is not tried again by itself: the operator's hook needs a look first.
fn hook_failure(run: &HookRun) -> CommandError {
    CommandError {
        code: HOOK_FAILED.to_owned(),
        detail: run.why(),
        retryable: false,
        details: None,
    }
}
