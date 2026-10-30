//! In-memory environment driver.
//!
//! [`MemoryDriver`] is an [`EnvironmentDriver`] that keeps its units, volumes and images in memory, so that
//! whatever drives a driver (the node today, the engine's provisioning later) can be tested without a
//! daemon. It behaves like a conforming driver: a unit is made idempotently by name, kept apart by its
//! `orch.system` label, started and stopped, removed without its volumes, and found again by `list`. It
//! declares the least a driver can (nothing confined, no capabilities, nothing suspended, no sources, no
//! imports; units restart rather than being replaced), and a test widens that with the builders
//! ([`MemoryDriver::replacing`], [`MemoryDriver::with_storages`], [`MemoryDriver::with_sources`], …).
//!
//! What a test can ask of it: every call in the order it was made ([`MemoryDriver::calls`]), the units and
//! volumes it holds, and how many calls for one unit were in flight at once
//! ([`MemoryDriver::max_in_flight`]). What a test can make it do: fail the next call of an operation, delay or
//! hold an operation, let a unit die by itself ([`MemoryDriver::crash`]), plant a unit it was never asked for
//! ([`MemoryDriver::plant`]), move the digest a registry has behind a tag, and say whether the connector in a
//! unit has enrolled. It is `Send + Sync` and every method takes `&self`.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::imports::{ImportProposal, ImportRequest};
use orch_driver_contract::runtime::{LABEL_SYSTEM, RuntimeKind, RuntimeSpec};
use orch_driver_contract::sources::{BaseImage, BuildRecord, ImageEnsured, SourceKind};
use orch_driver_contract::units::{UnitRef, UnitState, UnitStateKind};
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest};
use orch_types::{Hash, Json};
use serde_json::json;
use tokio::sync::Notify;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a driver is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Operation {
    /// `validate`.
    Validate,
    /// `create`.
    Create,
    /// `start`.
    Start,
    /// `stop`.
    Stop,
    /// `remove`.
    Remove,
    /// `inspect`.
    Inspect,
    /// `list`.
    List,
    /// `stats`.
    Stats,
    /// `ensure_volume`.
    EnsureVolume,
    /// `remove_volume`.
    RemoveVolume,
    /// `ensure_image`.
    EnsureImage,
    /// `import`.
    Import,
    /// Holding a unit's processes still.
    Suspend,
    /// Letting them go again.
    Resume,
}

/// One call the driver received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// What it was asked.
    pub operation: Operation,
    /// What it was asked about: a unit's name, a volume's name, an image reference or a system.
    pub target: String,
}

/// A unit as the driver holds it.
#[derive(Debug, Clone)]
pub struct UnitRecord {
    /// The unit.
    pub unit: UnitRef,
    /// The request it was made from.
    pub spec: RuntimeSpec,
    /// Where it stands.
    pub state: UnitState,
    /// How many times it was started.
    pub starts: u32,
    /// How long it was given to stop by the last stop.
    pub last_grace: Option<Duration>,
}

impl UnitRecord {
    /// The names of the volumes the unit stands on.
    #[must_use]
    pub fn volumes(&self) -> Vec<String> {
        self.spec
            .volumes
            .values()
            .map(|volume| volume.name.clone())
            .collect()
    }
}

/// A volume as the driver holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeRecord {
    /// Its name.
    pub name: String,
    /// The storage it was made on; `None` is the host's default.
    pub storage: Option<String>,
    /// The labels it was made with.
    pub labels: BTreeMap<String, String>,
}

/// An image as the driver holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRecord {
    /// Its digest.
    pub digest: String,
    /// How many times it was made available.
    pub ensured: u32,
}

#[derive(Default)]
struct Inner {
    kept_images: BTreeMap<String, orch_driver_contract::kept::KeptImage>,
    units: BTreeMap<String, UnitRecord>,
    volumes: BTreeMap<String, VolumeRecord>,
    images: BTreeMap<String, ImageRecord>,
    storages: Vec<StorageInfo>,
    calls: Vec<Call>,
    failures: Vec<(Operation, DriverError)>,
    delays: BTreeMap<Operation, Duration>,
    held: HashSet<Operation>,
    in_flight: BTreeMap<String, usize>,
    max_in_flight: BTreeMap<String, usize>,
    registry: BTreeMap<String, String>,
    image_lines: Vec<String>,
    enrolled: BTreeMap<String, Option<bool>>,
    imports: BTreeMap<String, ImportProposal>,
    bases: Vec<BaseImage>,
    image_pause: Duration,
    image_failure: Option<DriverError>,
    exclusive: bool,
}

/// The driver. See the module.
pub struct MemoryDriver {
    descriptor: Mutex<DriverDescriptor>,
    inner: Mutex<Inner>,
    released: Notify,
}

impl std::fmt::Debug for MemoryDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryDriver")
            .field("units", &lock(&self.inner).units.len())
            .finish_non_exhaustive()
    }
}

/// Counts a call as in flight until it is dropped.
struct InFlight<'a> {
    driver: &'a MemoryDriver,
    target: String,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Some(count) = lock(&self.driver.inner).in_flight.get_mut(&self.target) {
            *count = count.saturating_sub(1);
        }
    }
}

fn base_descriptor(name: &str) -> DriverDescriptor {
    serde_json::from_value(json!({
        "name": name, "version": "1", "title": "Units in memory",
        "spec_schemas": { "environment": { "type": "object" }, "provider": { "type": "object" } },
        "confinement": "none", "recreate": "restart", "volumes": true,
        "host_ports": {"supported":true,"reason":null},
        "mounts": "none", "process_scope": "started", "resource_limits": {"cpu":"none","memory":"none","pids":"none","disk":"none"}
    }))
    .unwrap_or_else(|error| panic!("the descriptor of the in-memory driver is wrong: {error}"))
}

impl MemoryDriver {
    /// A driver called `name` that declares the least a driver can.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            descriptor: Mutex::new(base_descriptor(name)),
            inner: Mutex::new(Inner::default()),
            released: Notify::new(),
        }
    }

    /// The same driver, shared.
    #[must_use]
    pub fn shared(name: &str) -> Arc<Self> {
        Arc::new(Self::new(name))
    }

    /// A driver whose recreate makes a new unit on the same volumes (`replace`).
    #[must_use]
    pub fn replacing(self) -> Self {
        lock(&self.descriptor).recreate = orch_driver_contract::descriptor::Recreate::Replace;
        self
    }

    /// A driver that can make volumes on the named storages it is given, and reports them.
    #[must_use]
    pub fn with_storages(self, storages: Vec<StorageInfo>) -> Self {
        lock(&self.descriptor).storages = true;
        lock(&self.inner).storages = storages;
        self
    }

    /// A driver that declares the sources of a unit's file system it can have.
    #[must_use]
    pub fn with_sources(self, sources: Vec<SourceKind>) -> Self {
        lock(&self.descriptor).sources = sources;
        self
    }

    /// A driver that can hold a unit's processes still.
    #[must_use]
    pub fn with_suspend(self) -> Self {
        lock(&self.descriptor).suspend = true;
        self
    }

    /// A driver with another declaration than the least.
    #[must_use]
    pub fn with_descriptor(self, descriptor: DriverDescriptor) -> Self {
        *lock(&self.descriptor) = descriptor;
        self
    }

    /// A driver that refuses to start a unit while another unit on one of its volumes runs, as the Docker
    /// driver does. Without this the driver starts whatever it is asked to, and the node's own guard is what
    /// keeps two units off one volume.
    #[must_use]
    pub fn enforcing_exclusive_volumes(self) -> Self {
        lock(&self.inner).exclusive = true;
        self
    }

    // ------------------------------------------------------------------ what a test can ask

    /// Every call, in the order they were received.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.inner).calls.clone()
    }

    /// The targets of the calls of one operation, in order.
    #[must_use]
    pub fn targets_of(&self, operation: Operation) -> Vec<String> {
        lock(&self.inner)
            .calls
            .iter()
            .filter(|call| call.operation == operation)
            .map(|call| call.target.clone())
            .collect()
    }

    /// Forgets the calls so far.
    pub fn clear_calls(&self) {
        lock(&self.inner).calls.clear();
    }

    /// The units it holds.
    #[must_use]
    pub fn units(&self) -> Vec<UnitRecord> {
        lock(&self.inner).units.values().cloned().collect()
    }

    /// The unit of that name.
    #[must_use]
    pub fn unit(&self, name: &str) -> Option<UnitRecord> {
        lock(&self.inner).units.get(name).cloned()
    }

    /// Where the unit of that name stands; `Absent` when there is none.
    #[must_use]
    pub fn state_of(&self, name: &str) -> UnitStateKind {
        lock(&self.inner)
            .units
            .get(name)
            .map_or(UnitStateKind::Absent, |record| record.state.state)
    }

    /// The volumes it holds.
    #[must_use]
    pub fn volumes(&self) -> Vec<VolumeRecord> {
        lock(&self.inner).volumes.values().cloned().collect()
    }

    /// The volume of that name.
    #[must_use]
    pub fn volume(&self, name: &str) -> Option<VolumeRecord> {
        lock(&self.inner).volumes.get(name).cloned()
    }

    /// The images it holds, by reference.
    #[must_use]
    pub fn images(&self) -> BTreeMap<String, ImageRecord> {
        lock(&self.inner).images.clone()
    }
    /// A labelled image left by an earlier node run, or retained for an archived environment.
    pub fn remember_kept(&self, image: orch_driver_contract::kept::KeptImage) {
        lock(&self.inner)
            .kept_images
            .insert(image.reference.clone(), image);
    }
    /// Exact kept-image names left on this simulated daemon.
    pub fn kept_names(&self) -> Vec<String> {
        lock(&self.inner).kept_images.keys().cloned().collect()
    }

    /// The most calls for one target that were in flight at once.
    #[must_use]
    pub fn max_in_flight(&self, target: &str) -> usize {
        lock(&self.inner)
            .max_in_flight
            .get(target)
            .copied()
            .unwrap_or(0)
    }

    /// The storages it reports.
    #[must_use]
    pub fn storages(&self) -> Vec<StorageInfo> {
        lock(&self.inner).storages.clone()
    }

    /// The digest the registry has behind a reference, if a test set one.
    #[must_use]
    pub fn registry_digest(&self, reference: &str) -> Option<String> {
        lock(&self.inner).registry.get(reference).cloned()
    }

    /// Whether the connector in the unit `name` has enrolled, as a test says it has; `None` is "cannot tell".
    #[must_use]
    pub fn enrolled(&self, name: &str) -> Option<bool> {
        lock(&self.inner).enrolled.get(name).copied().flatten()
    }

    // ------------------------------------------------------------------ what a test can make it do

    /// The next call of `operation` fails with `error`; later calls are as before.
    pub fn fail_next(&self, operation: Operation, error: DriverError) {
        lock(&self.inner).failures.push((operation, error));
    }

    /// Every call of `operation` takes this long before it does its work.
    pub fn delay(&self, operation: Operation, by: Duration) {
        lock(&self.inner).delays.insert(operation, by);
    }

    /// Calls of `operation` wait, after they are counted, until [`MemoryDriver::release`].
    pub fn hold(&self, operation: Operation) {
        lock(&self.inner).held.insert(operation);
    }

    /// Lets the held calls of `operation` go on.
    pub fn release(&self, operation: Operation) {
        lock(&self.inner).held.remove(&operation);
        self.released.notify_waiters();
    }

    /// A unit dies by itself, as a container does when its main process fails.
    pub fn crash(&self, name: &str, exit_code: i64, detail: &str) {
        self.failed(name, Some(exit_code), detail);
    }

    /// A unit is definitely gone, while its process exit status is unknown.
    pub fn mark_failed(&self, name: &str, detail: &str) {
        self.failed(name, None, detail);
    }

    fn failed(&self, name: &str, exit_code: Option<i64>, detail: &str) {
        if let Some(record) = lock(&self.inner).units.get_mut(name) {
            record.state = UnitState {
                state: UnitStateKind::Failed,
                detail: Some(detail.to_owned()),
                exit_code,
                started_at: None,
                finished_at: None,
            };
        }
    }

    /// A unit appears that nobody asked this driver for, running, as one left from before would.
    pub fn plant(&self, spec: RuntimeSpec) {
        let unit = unit_of(&spec);
        let record = UnitRecord {
            unit,
            state: UnitState::of(UnitStateKind::Running),
            starts: 1,
            last_grace: None,
            spec,
        };
        lock(&self.inner)
            .units
            .insert(record.unit.name.clone(), record);
    }

    /// The registry has this digest behind the reference from now on.
    pub fn set_registry_digest(&self, reference: &str, digest: &str) {
        lock(&self.inner)
            .registry
            .insert(reference.to_owned(), digest.to_owned());
    }

    /// The lines of progress an image made available tells, in order.
    pub fn set_image_lines(&self, lines: &[&str]) {
        lock(&self.inner).image_lines = lines.iter().map(|line| (*line).to_owned()).collect();
    }

    /// Each line of progress is followed by a pause of this length, so that an operation takes time.
    pub fn set_image_pause(&self, pause: Duration) {
        lock(&self.inner).image_pause = pause;
    }

    /// The next image made available fails with `error` after it has told all its lines.
    pub fn fail_image_after_lines(&self, error: DriverError) {
        lock(&self.inner).image_failure = Some(error);
    }

    /// The base images a build is said to have started from, with the digests they had.
    pub fn set_build_bases(&self, bases: &[(&str, &str)]) {
        lock(&self.inner).bases = bases
            .iter()
            .map(|(reference, digest)| BaseImage {
                reference: (*reference).to_owned(),
                digest: (*digest).to_owned(),
            })
            .collect();
    }

    /// Says whether the connector in the unit `name` has enrolled; `None` is "cannot tell".
    pub fn set_enrolled(&self, name: &str, enrolled: Option<bool>) {
        lock(&self.inner).enrolled.insert(name.to_owned(), enrolled);
    }

    /// What an import of that format proposes.
    pub fn set_import(&self, format: &str, proposal: ImportProposal) {
        lock(&self.inner)
            .imports
            .insert(format.to_owned(), proposal);
    }

    // ------------------------------------------------------------------ what a node asks beyond the contract

    /// Makes the source of `spec` available and tells each line of progress to `line` as it goes.
    pub async fn ensure_image_reporting(
        &self,
        spec: &Json,
        line: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ImageEnsured, DriverError> {
        let reference = reference_of(spec);
        let _flight = self.enter(Operation::EnsureImage, &reference).await?;
        let (lines, pause) = {
            let inner = lock(&self.inner);
            (inner.image_lines.clone(), inner.image_pause)
        };
        for text in &lines {
            line(text);
            if !pause.is_zero() {
                tokio::time::sleep(pause).await;
            }
        }
        if let Some(error) = lock(&self.inner).image_failure.take() {
            return Err(error);
        }
        Ok(self.make_available(spec, &reference))
    }

    /// Holds a unit's processes still.
    pub async fn suspend(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _flight = self.enter(Operation::Suspend, &unit.name).await?;
        self.require(&unit.name).map(|_| ())
    }

    /// Lets go of the processes that [`MemoryDriver::suspend`] held.
    pub async fn resume(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _flight = self.enter(Operation::Resume, &unit.name).await?;
        self.require(&unit.name).map(|_| ())
    }

    // ------------------------------------------------------------------ inside

    fn require(&self, name: &str) -> Result<UnitRecord, DriverError> {
        lock(&self.inner)
            .units
            .get(name)
            .cloned()
            .ok_or_else(|| DriverError::NotFound(format!("there is no unit {name}")))
    }

    async fn enter(&self, operation: Operation, target: &str) -> Result<InFlight<'_>, DriverError> {
        let delay = {
            let mut inner = lock(&self.inner);
            inner.calls.push(Call {
                operation,
                target: target.to_owned(),
            });
            let now = {
                let count = inner.in_flight.entry(target.to_owned()).or_default();
                *count += 1;
                *count
            };
            let most = inner.max_in_flight.entry(target.to_owned()).or_default();
            *most = (*most).max(now);
            inner.delays.get(&operation).copied()
        };
        let flight = InFlight {
            driver: self,
            target: target.to_owned(),
        };
        loop {
            let released = self.released.notified();
            if !lock(&self.inner).held.contains(&operation) {
                break;
            }
            released.await;
        }
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let mut inner = lock(&self.inner);
        if let Some(place) = inner
            .failures
            .iter()
            .position(|(wanted, _)| *wanted == operation)
        {
            let (_, error) = inner.failures.remove(place);
            return Err(error);
        }
        drop(inner);
        Ok(flight)
    }

    fn make_available(&self, spec: &Json, reference: &str) -> ImageEnsured {
        if lock(&self.descriptor).sources.is_empty() {
            return ImageEnsured::nothing();
        }
        let mut inner = lock(&self.inner);
        let digest = inner
            .registry
            .get(reference)
            .cloned()
            .unwrap_or_else(|| format!("sha256:{}", Hash::of(reference.as_bytes())));
        let record = inner
            .images
            .entry(reference.to_owned())
            .or_insert_with(|| ImageRecord {
                digest: digest.clone(),
                ensured: 0,
            });
        record.ensured += 1;
        // A reference that is not pinned follows its tag: the registry's answer is what the host holds now.
        record.digest.clone_from(&digest);
        let bases = inner.bases.clone();
        let built = spec
            .get("build")
            .filter(|build| !build.is_null())
            .map(|_| BuildRecord {
                commit: None,
                bases,
            });
        ImageEnsured {
            programs: Default::default(),
            digest: Some(digest),
            built,
        }
    }
}

/// The name an image is held under: the reference of an `image` source, or a name made from the
/// definition of a `build` or an `archive`.
fn reference_of(spec: &Json) -> String {
    if let Some(reference) = spec.get("image").and_then(Json::as_str) {
        return reference.to_owned();
    }
    format!("built:{}", Hash::of(spec.to_string().as_bytes()))
}

fn unit_of(spec: &RuntimeSpec) -> UnitRef {
    UnitRef {
        reference: format!("mem-{}", spec.name),
        name: spec.name.clone(),
        runtime_id: spec.runtime_id,
        kind: spec.kind,
    }
}

#[async_trait]
impl EnvironmentDriver for MemoryDriver {
    async fn resources(
        &self,
        system: &str,
    ) -> Result<Vec<orch_driver_contract::resources::Resource>, DriverError> {
        use orch_driver_contract::resources::{Resource, ResourceKind, ResourceRef};
        if system.is_empty() {
            return Err(DriverError::InvalidSpec(
                "an installation label is required".into(),
            ));
        }
        let inner = lock(&self.inner);
        let mut found = Vec::new();
        for record in inner.units.values().filter(|record| {
            record
                .spec
                .labels
                .get(LABEL_SYSTEM)
                .is_some_and(|value| value == system)
        }) {
            let mut depends_on: Vec<_> = record
                .volumes()
                .into_iter()
                .map(|id| ResourceRef {
                    kind: ResourceKind::Volume,
                    id,
                })
                .collect();
            depends_on.extend(record.spec.tools.iter().map(|tool| ResourceRef {
                kind: ResourceKind::ToolVersion,
                id: format!("{}/{}", tool.name, tool.version),
            }));
            if let Some(kept) = &record.spec.kept_image {
                depends_on.push(ResourceRef {
                    kind: ResourceKind::KeptLayer,
                    id: kept.reference.clone(),
                });
            }
            found.push(Resource {
                resource: ResourceRef {
                    kind: ResourceKind::Unit,
                    id: record.unit.reference.clone(),
                },
                name: record.unit.name.clone(),
                size_bytes: None,
                labels: record.spec.labels.clone(),
                depends_on,
                aliases: Vec::new(),
                last_used_at: None,
                busy: record.state.state == UnitStateKind::Running,
            });
        }
        for volume in inner.volumes.values().filter(|volume| {
            volume
                .labels
                .get(LABEL_SYSTEM)
                .is_some_and(|value| value == system)
        }) {
            found.push(Resource {
                resource: ResourceRef {
                    kind: ResourceKind::Volume,
                    id: volume.name.clone(),
                },
                name: volume.name.clone(),
                size_bytes: None,
                labels: volume.labels.clone(),
                depends_on: Vec::new(),
                aliases: Vec::new(),
                last_used_at: None,
                busy: inner.units.values().any(|unit| {
                    unit.volumes().contains(&volume.name)
                        && unit
                            .spec
                            .labels
                            .get(LABEL_SYSTEM)
                            .is_none_or(|value| value != system)
                }),
            });
        }
        for kept in inner
            .kept_images
            .values()
            .filter(|kept| kept.system == system)
        {
            found.push(Resource {
                resource: ResourceRef {
                    kind: ResourceKind::KeptLayer,
                    id: kept.reference.clone(),
                },
                name: kept.reference.clone(),
                size_bytes: Some(kept.bytes),
                labels: BTreeMap::from([
                    (LABEL_SYSTEM.into(), system.into()),
                    ("orch.environment_id".into(), kept.environment.to_string()),
                ]),
                depends_on: Vec::new(),
                aliases: Vec::new(),
                last_used_at: None,
                busy: false,
            });
        }
        Ok(found)
    }

    async fn remove_resource(
        &self,
        system: &str,
        resource: &orch_driver_contract::resources::ResourceRef,
    ) -> Result<(), DriverError> {
        use orch_driver_contract::resources::ResourceKind;
        if system.is_empty() {
            return Err(DriverError::InvalidSpec(
                "an installation label is required".into(),
            ));
        }
        let mut inner = lock(&self.inner);
        match resource.kind {
            ResourceKind::Unit => {
                let record = inner
                    .units
                    .values()
                    .find(|unit| {
                        unit.unit.reference == resource.id
                            && unit
                                .spec
                                .labels
                                .get(LABEL_SYSTEM)
                                .is_some_and(|value| value == system)
                    })
                    .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
                if record.state.state == UnitStateKind::Running {
                    return Err(DriverError::Busy("the unit is running".into()));
                }
                let name = record.unit.name.clone();
                inner.units.remove(&name);
            }
            ResourceKind::Volume => {
                if inner
                    .units
                    .values()
                    .any(|unit| unit.volumes().contains(&resource.id))
                {
                    return Err(DriverError::Busy("the volume is in use".into()));
                }
                if !inner.volumes.get(&resource.id).is_some_and(|volume| {
                    volume
                        .labels
                        .get(LABEL_SYSTEM)
                        .is_some_and(|value| value == system)
                }) {
                    return Err(DriverError::NotFound("the owned volume is absent".into()));
                }
                inner.volumes.remove(&resource.id);
            }
            ResourceKind::KeptLayer => {
                if inner.units.values().any(|unit| {
                    unit.spec
                        .kept_image
                        .as_ref()
                        .is_some_and(|kept| kept.reference == resource.id)
                }) {
                    return Err(DriverError::Busy("the kept layer is in use".into()));
                }
                if !inner
                    .kept_images
                    .get(&resource.id)
                    .is_some_and(|kept| kept.system == system)
                {
                    return Err(DriverError::NotFound("the owned layer is absent".into()));
                }
                inner.kept_images.remove(&resource.id);
            }
            _ => return Err(DriverError::NotFound("the owned object is absent".into())),
        }
        Ok(())
    }

    async fn volume_identity(
        &self,
        volume: &VolumeRequest,
    ) -> Result<Option<orch_driver_contract::volumes::VolumeIdentity>, DriverError> {
        let Some(held) = self.volume(&volume.name) else {
            return Ok(None);
        };
        if held.storage != volume.storage {
            return Err(DriverError::InvalidSpec(
                "the existing volume is on another storage".into(),
            ));
        }
        Ok(Some(orch_driver_contract::volumes::VolumeIdentity {
            labels: held.labels,
        }))
    }
    async fn kept_images(
        &self,
        system: &str,
    ) -> Result<Vec<orch_driver_contract::kept::KeptImage>, DriverError> {
        Ok(lock(&self.inner)
            .kept_images
            .values()
            .filter(|image| image.system == system)
            .cloned()
            .collect())
    }
    async fn remove_kept_image(
        &self,
        image: &orch_driver_contract::kept::KeptImage,
    ) -> Result<(), DriverError> {
        let mut inner = lock(&self.inner);
        if inner.kept_images.get(&image.reference).is_some_and(|held| {
            held.capture == image.capture
                && held.system == image.system
                && held.environment == image.environment
        }) {
            inner.kept_images.remove(&image.reference);
        }
        Ok(())
    }
    fn describe(&self) -> DriverDescriptor {
        lock(&self.descriptor).clone()
    }

    async fn validate(&self, kind: RuntimeKind, spec: &Json) -> Result<(), DriverError> {
        let _flight = self.enter(Operation::Validate, kind.as_str()).await?;
        if spec.is_object() {
            Ok(())
        } else {
            Err(DriverError::InvalidSpec("a spec is an object".to_owned()))
        }
    }

    async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
        let _flight = self.enter(Operation::Create, &spec.name).await?;
        let mut inner = lock(&self.inner);
        if let Some(found) = inner.units.get(&spec.name) {
            if found.unit.runtime_id != spec.runtime_id {
                return Err(DriverError::Internal(format!(
                    "a unit named {} exists and is not the unit of this runtime",
                    spec.name
                )));
            }
            return Ok(found.unit.clone());
        }
        for volume in spec.volumes.values() {
            inner
                .volumes
                .entry(volume.name.clone())
                .or_insert_with(|| VolumeRecord {
                    name: volume.name.clone(),
                    storage: volume.storage.clone(),
                    labels: BTreeMap::new(),
                });
        }
        let unit = unit_of(spec);
        inner.units.insert(
            spec.name.clone(),
            UnitRecord {
                unit: unit.clone(),
                spec: spec.clone(),
                state: UnitState::of(UnitStateKind::Stopped),
                starts: 0,
                last_grace: None,
            },
        );
        Ok(unit)
    }

    async fn start(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _flight = self.enter(Operation::Start, &unit.name).await?;
        let mut inner = lock(&self.inner);
        let volumes = inner
            .units
            .get(&unit.name)
            .ok_or_else(|| DriverError::NotFound(format!("there is no unit {}", unit.name)))?
            .volumes();
        if inner.exclusive {
            for (other, record) in &inner.units {
                if other != &unit.name
                    && record.state.state == UnitStateKind::Running
                    && record
                        .volumes()
                        .iter()
                        .any(|volume| volumes.contains(volume))
                {
                    return Err(DriverError::Internal(format!(
                        "the unit {other} is running on the volume of this one; two units never run on one volume"
                    )));
                }
            }
        }
        if let Some(record) = inner.units.get_mut(&unit.name) {
            record.state = UnitState::of(UnitStateKind::Running);
            record.starts += 1;
        }
        Ok(())
    }

    async fn stop(&self, unit: &UnitRef, grace: Duration) -> Result<(), DriverError> {
        let _flight = self.enter(Operation::Stop, &unit.name).await?;
        let mut inner = lock(&self.inner);
        let record = inner
            .units
            .get_mut(&unit.name)
            .ok_or_else(|| DriverError::NotFound(format!("there is no unit {}", unit.name)))?;
        record.last_grace = Some(grace);
        if record.state.state == UnitStateKind::Running {
            record.state = UnitState::of(UnitStateKind::Stopped);
        }
        Ok(())
    }

    async fn remove(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _flight = self.enter(Operation::Remove, &unit.name).await?;
        lock(&self.inner).units.remove(&unit.name);
        Ok(())
    }

    async fn inspect(&self, unit: &UnitRef) -> Result<UnitState, DriverError> {
        let _flight = self.enter(Operation::Inspect, &unit.name).await?;
        Ok(lock(&self.inner)
            .units
            .get(&unit.name)
            .map_or_else(|| UnitState::of(UnitStateKind::Absent), |r| r.state.clone()))
    }

    async fn list(&self, system: &str) -> Result<Vec<UnitRef>, DriverError> {
        let _flight = self.enter(Operation::List, system).await?;
        Ok(lock(&self.inner)
            .units
            .values()
            .filter(|record| {
                record.spec.labels.get(LABEL_SYSTEM).map(String::as_str) == Some(system)
            })
            .map(|record| record.unit.clone())
            .collect())
    }

    async fn ensure_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        let _flight = self.enter(Operation::EnsureVolume, &volume.name).await?;
        let mut inner = lock(&self.inner);
        if let Some(storage) = &volume.storage
            && !inner.storages.iter().any(|known| &known.name == storage)
        {
            return Err(DriverError::InvalidSpec(format!(
                "the host has no storage {storage:?}"
            )));
        }
        inner
            .volumes
            .entry(volume.name.clone())
            .or_insert_with(|| VolumeRecord {
                name: volume.name.clone(),
                storage: volume.storage.clone(),
                labels: volume.labels.clone(),
            });
        Ok(())
    }

    async fn remove_volume(&self, name: &str) -> Result<(), DriverError> {
        let _flight = self.enter(Operation::RemoveVolume, name).await?;
        lock(&self.inner).volumes.remove(name);
        Ok(())
    }

    async fn ensure_image(&self, spec: &Json) -> Result<ImageEnsured, DriverError> {
        let reference = reference_of(spec);
        let _flight = self.enter(Operation::EnsureImage, &reference).await?;
        Ok(self.make_available(spec, &reference))
    }

    async fn import(&self, request: &ImportRequest) -> Result<ImportProposal, DriverError> {
        let _flight = self.enter(Operation::Import, &request.format).await?;
        lock(&self.inner)
            .imports
            .get(&request.format)
            .cloned()
            .ok_or_else(|| DriverError::Unsupported("import".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::runtime::unit_labels;
    use orch_types::new_id;

    /// A request for a unit named after `volume`: the name the identifier gives is its first twelve digits, which
    /// for identifiers made in one millisecond are the same, so a test names its units by itself.
    fn spec(system: &str, volume: &str) -> RuntimeSpec {
        let id = new_id();
        serde_json::from_value(json!({
            "runtime_id": id, "kind": "environment", "name": format!("orch-environment-{volume}"),
            "driver": "memory", "spec": {}, "resources": {}, "roles": ["environment"],
            "gateway_url": "wss://gateway.orch.internal:9443", "enroll_token": "t", "ca_cert": "",
            "tools": [],
            "volumes": {
                "workspace": { "name": format!("{volume}-ws"), "storage": null },
                "home": { "name": format!("{volume}-home"), "storage": null },
                "state": { "name": format!("{volume}-state"), "storage": null }
            },
            "mounts": [], "env": {}, "labels": unit_labels(system, RuntimeKind::Environment, &id)
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn a_unit_is_made_once_by_name_started_stopped_and_removed_without_its_volumes() {
        let driver = MemoryDriver::new("memory");
        let request = spec("sys", "v1");
        let unit = driver.create(&request).await.unwrap();
        assert_eq!(driver.create(&request).await.unwrap(), unit, "by name");
        assert_eq!(driver.units().len(), 1);
        driver.start(&unit).await.unwrap();
        assert_eq!(driver.state_of(&unit.name), UnitStateKind::Running);
        driver.stop(&unit, Duration::from_secs(3)).await.unwrap();
        assert_eq!(
            driver.unit(&unit.name).unwrap().last_grace,
            Some(Duration::from_secs(3))
        );
        driver.remove(&unit).await.unwrap();
        assert_eq!(driver.state_of(&unit.name), UnitStateKind::Absent);
        assert_eq!(driver.volumes().len(), 3, "a unit's volumes outlive it");
    }

    #[tokio::test]
    async fn only_the_units_of_the_system_asked_for_are_listed() {
        let driver = MemoryDriver::new("memory");
        driver.create(&spec("mine", "a")).await.unwrap();
        driver.create(&spec("yours", "b")).await.unwrap();
        assert_eq!(driver.list("mine").await.unwrap().len(), 1);
        assert!(driver.list("nobody").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_driver_that_enforces_it_starts_no_second_unit_on_a_volume_that_is_in_use() {
        let driver = MemoryDriver::new("memory").enforcing_exclusive_volumes();
        let (first, mut second) = (spec("s", "same"), spec("s", "same"));
        second.name = "orch-environment-second".to_owned();
        let (one, two) = (
            driver.create(&first).await.unwrap(),
            driver.create(&second).await.unwrap(),
        );
        driver.start(&one).await.unwrap();
        assert!(driver.start(&two).await.is_err());
        driver.stop(&one, Duration::ZERO).await.unwrap();
        driver.start(&two).await.unwrap();
    }

    #[tokio::test]
    async fn a_scripted_failure_happens_once_and_a_held_call_waits_for_its_release() {
        let driver = Arc::new(MemoryDriver::new("memory"));
        let request = spec("s", "x");
        driver.fail_next(
            Operation::Create,
            DriverError::Unavailable("no daemon".into()),
        );
        assert!(driver.create(&request).await.is_err());
        let unit = driver.create(&request).await.unwrap();
        driver.hold(Operation::Start);
        let running = {
            let (driver, unit) = (Arc::clone(&driver), unit.clone());
            tokio::spawn(async move { driver.start(&unit).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(driver.state_of(&unit.name), UnitStateKind::Stopped);
        driver.release(Operation::Start);
        running.await.unwrap().unwrap();
        assert_eq!(driver.state_of(&unit.name), UnitStateKind::Running);
        assert_eq!(driver.max_in_flight(&unit.name), 1);
    }

    #[tokio::test]
    async fn an_image_is_made_available_with_the_digest_the_registry_has_and_a_driver_without_sources_does_nothing()
     {
        let plain = MemoryDriver::new("plain");
        assert_eq!(
            plain
                .ensure_image(&json!({ "image": "x:1" }))
                .await
                .unwrap(),
            ImageEnsured::nothing()
        );
        let driver = MemoryDriver::new("memory").with_sources(vec![SourceKind::Image]);
        driver.set_registry_digest("x:1", "sha256:aaa");
        let mut told = Vec::new();
        driver.set_image_lines(&["pulling", "done"]);
        let ensured = driver
            .ensure_image_reporting(&json!({ "image": "x:1" }), &mut |line| {
                told.push(line.to_owned());
            })
            .await
            .unwrap();
        assert_eq!(ensured.digest.as_deref(), Some("sha256:aaa"));
        assert_eq!(told, ["pulling", "done"]);
        assert_eq!(driver.images()["x:1"].ensured, 1);
    }
}
