//! A durable creation-intent and native-UID journal for claims.
//!
//! The caller gives this store a private, node-owned directory and a verified
//! cluster/namespace scope. A recorded UID fences native retries; it does not
//! supply the missing immutable identity in an ordinary volume-removal command.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::{LABEL_KIND, LABEL_RUNTIME_ID, LABEL_SYSTEM, RuntimeKind};
use orch_driver_contract::volumes::VolumeRequest;
use orch_types::{Hash, Id};
use serde::{Deserialize, Serialize};

use crate::authenticated_control::{GuardKey, VerifiedReply};
use crate::control_protocol::MAX_FRAME_BYTES;
use crate::platform::WorkerIdentity;
use crate::pod_identity::{ContainerIdentities, StageIdentity};
use crate::process_scope::container_id;

/// A future incompatible format is refused instead of being treated as an empty journal.
const VERSION: u32 = 1;
/// Metadata for many thousands of claims fits here; a damaged file cannot grow recovery without bound.
const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
const JOURNAL_NAME: &str = "claims.json";
const LOCK_NAME: &str = "claims.lock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    intent: Id,
    uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pv_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pv_uid: Option<String>,
    storage: Option<String>,
    labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    version: u32,
    scope: String,
    claims: BTreeMap<String, Entry>,
    #[serde(default)]
    units: BTreeMap<Id, UnitEntry>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
struct StoredKey([u8; 32]);

impl std::fmt::Debug for StoredKey {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("[secret]")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitEntry {
    intent: Id,
    name: String,
    kind: RuntimeKind,
    labels: BTreeMap<String, String>,
    pod_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worker_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worker_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage_id: Option<String>,
    workload_id: Option<String>,
    guard_id: Option<String>,
    key: StoredKey,
    issued_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<PendingSequence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed: Option<PendingSequence>,
    quarantined: bool,
    #[serde(default)]
    prepared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_request: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admitted: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage_header: Option<crate::stage_protocol::StageHeader>,
    #[serde(default)]
    removed: bool,
}

/// One exact unit, read from its own journal rather than a growing host inventory.
pub(crate) struct UnitRecord {
    pub unit: orch_driver_contract::units::UnitRef,
    pub labels: BTreeMap<String, String>,
    pub worker: Option<RecordedWorker>,
    pub stage_id: Option<String>,
    pub containers: Option<ContainerIdentities>,
    pub key: GuardKey,
    pub prepared: bool,
    pub pending_request: Option<Vec<u8>>,
    pub admitted: Option<serde_json::Value>,
    pub stage_header: Option<crate::stage_protocol::StageHeader>,
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingSequence {
    sequence: u64,
    request_hash: Hash,
}

struct State {
    data: Data,
    healthy: bool,
}

/// A planned create and its native identity, if the API response has been durably recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimIntent {
    pub intent: Id,
    pub uid: Option<String>,
    pub pv_name: Option<String>,
    pub pv_uid: Option<String>,
}

/// A unit's durable creation intent and, once the API answered, the Pod it made. The rest of what is
/// recorded is read back through [`ReceiptStore::unit`].
#[derive(Debug, Clone)]
pub(crate) struct UnitIntent {
    pub intent: Id,
    pub pod_uid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedWorker {
    pub name: String,
    pub uid: String,
}

/// One process owns the journal lock for its lifetime; methods serialize its durable changes.
pub(crate) struct ReceiptStore {
    directory: PathBuf,
    _lock: File,
    state: Mutex<State>,
}

fn unavailable() -> DriverError {
    DriverError::Unavailable("the Kubernetes claim journal is unavailable or damaged".into())
}

fn conflict() -> DriverError {
    DriverError::InvalidSpec("the claim does not match its recorded creation intent".into())
}

/// Serializes no more than the journal's recovery limit, including while constructing it.
struct BoundedBytes(Vec<u8>);

impl Write for BoundedBytes {
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        let length = self
            .0
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| std::io::Error::other("the claim journal is too large"))?;
        let length = u64::try_from(length)
            .map_err(|_| std::io::Error::other("the claim journal is too large"))?;
        if length > MAX_JOURNAL_BYTES {
            return Err(std::io::Error::other("the claim journal is too large"));
        }
        self.0.extend_from_slice(chunk);
        Ok(chunk.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
fn private_directory(directory: &Path) -> Result<(), DriverError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if !directory.is_absolute() {
        return Err(DriverError::InvalidSpec(
            "the claim journal directory must be absolute".into(),
        ));
    }
    let parent = directory.parent().ok_or_else(unavailable)?;
    let parent_meta = fs::symlink_metadata(parent).map_err(|_| unavailable())?;
    if !parent_meta.is_dir()
        || parent_meta.file_type().is_symlink()
        || parent_meta.uid() != rustix::process::geteuid().as_raw()
        || parent_meta.permissions().mode() & 0o022 != 0
    {
        return Err(unavailable());
    }
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(directory) {
        Ok(()) => File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|_| unavailable())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(unavailable()),
    }
    let meta = fs::symlink_metadata(directory).map_err(|_| unavailable())?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(unavailable());
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_directory(_directory: &Path) -> Result<(), DriverError> {
    Err(DriverError::Unsupported(
        "the claim journal requires a private Unix directory".into(),
    ))
}

#[cfg(unix)]
fn open_options() -> OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    options
}

#[cfg(not(unix))]
fn open_options() -> OpenOptions {
    OpenOptions::new()
}

#[cfg(unix)]
fn check_file(file: &File) -> Result<(), DriverError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = file.metadata().map_err(|_| unavailable())?;
    if !meta.is_file()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(unavailable());
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_file(_file: &File) -> Result<(), DriverError> {
    Err(DriverError::Unsupported(
        "the claim journal requires private Unix files".into(),
    ))
}

impl ReceiptStore {
    /// Opens a private journal and refuses concurrent owners, corruption or a different cluster scope.
    pub(crate) fn open(directory: &Path, scope: &str) -> Result<Self, DriverError> {
        if scope.trim().is_empty() {
            return Err(DriverError::InvalidSpec(
                "the Kubernetes cluster scope is missing".into(),
            ));
        }
        private_directory(directory)?;
        let lock = open_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(LOCK_NAME))
            .map_err(|_| unavailable())?;
        check_file(&lock)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(DriverError::Busy(
                    "the Kubernetes claim journal is already in use".into(),
                ));
            }
            Err(TryLockError::Error(_)) => return Err(unavailable()),
        }
        let path = directory.join(JOURNAL_NAME);
        let (data, initial) = match open_options().read(true).open(&path) {
            Ok(mut file) => {
                check_file(&file)?;
                if file.metadata().map_err(|_| unavailable())?.len() > MAX_JOURNAL_BYTES {
                    return Err(unavailable());
                }
                let mut bytes = Vec::new();
                Read::by_ref(&mut file)
                    .take(MAX_JOURNAL_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| unavailable())?;
                if u64::try_from(bytes.len()).map_err(|_| unavailable())? > MAX_JOURNAL_BYTES {
                    return Err(unavailable());
                }
                let data: Data = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
                if data.version != VERSION || data.scope != scope {
                    return Err(unavailable());
                }
                (data, false)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                Data {
                    version: VERSION,
                    scope: scope.to_owned(),
                    claims: BTreeMap::new(),
                    units: BTreeMap::new(),
                },
                true,
            ),
            Err(_) => return Err(unavailable()),
        };
        let store = Self {
            directory: directory.to_owned(),
            _lock: lock,
            state: Mutex::new(State {
                data,
                healthy: true,
            }),
        };
        if initial {
            // The scope must be durable before a create may be attempted.
            store.persist(&store.lock()?.data)?;
        }
        Ok(store)
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, DriverError> {
        // A panic between a durable write and the in-memory update is ambiguous.
        self.state.lock().map_err(|_| unavailable())
    }

    fn persist(&self, data: &Data) -> Result<(), DriverError> {
        let mut bytes = BoundedBytes(Vec::new());
        serde_json::to_writer(&mut bytes, data).map_err(|_| unavailable())?;
        let temporary = self
            .directory
            .join(format!(".claims-{}.writing", orch_types::new_id()));
        let mut file = open_options()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| unavailable())?;
        if file
            .write_all(&bytes.0)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(&temporary);
            return Err(unavailable());
        }
        if fs::rename(&temporary, self.directory.join(JOURNAL_NAME)).is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(unavailable());
        }
        // A failed directory sync is ambiguous; the caller must stop using this store.
        File::open(&self.directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| unavailable())
    }

    fn commit(&self, state: &mut State, next: Data) -> Result<(), DriverError> {
        if !state.healthy {
            return Err(unavailable());
        }
        if let Err(error) = self.persist(&next) {
            state.healthy = false;
            return Err(error);
        }
        state.data = next;
        Ok(())
    }

    /// Persists a creation intent before any Kubernetes create request is made.
    pub(crate) fn begin(&self, volume: &VolumeRequest) -> Result<ClaimIntent, DriverError> {
        if volume.name.trim().is_empty()
            || volume
                .labels
                .get(LABEL_SYSTEM)
                .is_none_or(|owner| owner.trim().is_empty())
        {
            return Err(DriverError::InvalidSpec(
                "a claim needs a name and owning system".into(),
            ));
        }
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        if let Some(entry) = state.data.claims.get(&volume.name) {
            if entry.storage != volume.storage
                || entry.labels != volume.labels
                || entry.pv_name.is_some() != entry.pv_uid.is_some()
            {
                return Err(conflict());
            }
            return Ok(ClaimIntent {
                intent: entry.intent,
                uid: entry.uid.clone(),
                pv_name: entry.pv_name.clone(),
                pv_uid: entry.pv_uid.clone(),
            });
        }
        let intent = orch_types::new_id();
        let mut next = state.data.clone();
        next.claims.insert(
            volume.name.clone(),
            Entry {
                intent,
                uid: None,
                pv_name: None,
                pv_uid: None,
                storage: volume.storage.clone(),
                labels: volume.labels.clone(),
            },
        );
        self.commit(&mut state, next)?;
        Ok(ClaimIntent {
            intent,
            uid: None,
            pv_name: None,
            pv_uid: None,
        })
    }

    /// Reads ownership of one already-created claim without creating a new intent.
    pub(crate) fn volume(&self, name: &str) -> Result<Option<VolumeRequest>, DriverError> {
        let state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let Some(entry) = state.data.claims.get(name) else {
            return Ok(None);
        };
        if entry.uid.is_none() {
            return Err(unavailable());
        }
        Ok(Some(VolumeRequest {
            name: name.to_owned(),
            storage: entry.storage.clone(),
            labels: entry.labels.clone(),
        }))
    }

    /// Binds the UID returned by the API; a later UID can never replace it in this journal.
    pub(crate) fn bind(&self, name: &str, intent: Id, uid: &str) -> Result<(), DriverError> {
        if uid.is_empty() {
            return Err(conflict());
        }
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.claims.get(name).ok_or_else(conflict)?;
        if entry.intent != intent || entry.uid.as_deref().is_some_and(|old| old != uid) {
            return Err(conflict());
        }
        if entry.uid.as_deref() == Some(uid) {
            return Ok(());
        }
        let mut next = state.data.clone();
        next.claims.get_mut(name).ok_or_else(conflict)?.uid = Some(uid.to_owned());
        self.commit(&mut state, next)
    }

    /// Forgets a claim after its exact recorded incarnation was deleted, so that the name can be made anew.
    pub(crate) fn release(&self, name: &str, intent: Id, uid: &str) -> Result<(), DriverError> {
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let Some(entry) = state.data.claims.get(name) else {
            return Ok(());
        };
        if entry.intent != intent || entry.uid.as_deref() != Some(uid) {
            return Err(conflict());
        }
        let mut next = state.data.clone();
        next.claims.remove(name);
        self.commit(&mut state, next)
    }

    /// Once a PV has bound, never substitute another PV under the same claim incarnation.
    pub(crate) fn bind_pv(
        &self,
        name: &str,
        intent: Id,
        claim_uid: &str,
        pv_name: &str,
        pv_uid: &str,
    ) -> Result<(), DriverError> {
        if claim_uid.is_empty() || pv_name.is_empty() || pv_uid.is_empty() {
            return Err(conflict());
        }
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.claims.get(name).ok_or_else(conflict)?;
        if entry.intent != intent
            || entry.uid.as_deref() != Some(claim_uid)
            || entry.pv_name.is_some() != entry.pv_uid.is_some()
            || entry.pv_name.as_deref().is_some_and(|old| old != pv_name)
            || entry.pv_uid.as_deref().is_some_and(|old| old != pv_uid)
        {
            return Err(conflict());
        }
        if entry.pv_name.as_deref() == Some(pv_name) && entry.pv_uid.as_deref() == Some(pv_uid) {
            return Ok(());
        }
        let mut next = state.data.clone();
        let claim = next.claims.get_mut(name).ok_or_else(conflict)?;
        claim.pv_name = Some(pv_name.to_owned());
        claim.pv_uid = Some(pv_uid.to_owned());
        self.commit(&mut state, next)
    }

    /// Records a unit intent and fresh guard key before submitting its Pod.
    pub(crate) fn begin_unit(
        &self,
        runtime: Id,
        kind: RuntimeKind,
        name: &str,
        labels: &BTreeMap<String, String>,
    ) -> Result<UnitIntent, DriverError> {
        if name.trim().is_empty()
            || labels
                .get(LABEL_SYSTEM)
                .is_none_or(|system| system.trim().is_empty())
            || labels.get(LABEL_RUNTIME_ID) != Some(&runtime.to_string())
            || labels.get(LABEL_KIND).map(String::as_str) != Some(kind.as_str())
        {
            return Err(DriverError::InvalidSpec(
                "the unit ownership is incomplete".into(),
            ));
        }
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        if let Some(entry) = state.data.units.get(&runtime) {
            if entry.quarantined
                || entry.name != name
                || entry.kind != kind
                || &entry.labels != labels
                || entry.worker_name.is_some() != entry.worker_uid.is_some()
                || entry.worker_name.is_some() && entry.pod_uid.is_none()
                || entry.stage_id.is_some() && entry.worker_name.is_none()
            {
                return Err(conflict());
            }
            return Ok(UnitIntent {
                intent: entry.intent,
                pod_uid: entry.pod_uid.clone(),
            });
        }
        let intent = orch_types::new_id();
        let key = GuardKey::generate()?;
        let mut next = state.data.clone();
        next.units.insert(
            runtime,
            UnitEntry {
                intent,
                name: name.to_owned(),
                kind,
                labels: labels.clone(),
                pod_uid: None,
                worker_name: None,
                worker_uid: None,
                stage_id: None,
                workload_id: None,
                guard_id: None,
                key: StoredKey(*key.expose()),
                issued_sequence: 0,
                pending: None,
                completed: None,
                quarantined: false,
                prepared: false,
                pending_request: None,
                admitted: None,
                stage_header: None,
                removed: false,
            },
        );
        self.commit(&mut state, next)?;
        Ok(UnitIntent {
            intent,
            pod_uid: None,
        })
    }

    /// Pin the worker incarnation before trusting either container in the Pod.
    pub(crate) fn bind_worker(
        &self,
        runtime: Id,
        pod_uid: &str,
        worker: &WorkerIdentity,
    ) -> Result<(), DriverError> {
        if worker.name().is_empty() || worker.uid().is_empty() {
            return Err(conflict());
        }
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined
            || entry.pod_uid.as_deref() != Some(pod_uid)
            || entry.worker_name.is_some() != entry.worker_uid.is_some()
            || entry
                .worker_name
                .as_deref()
                .is_some_and(|name| name != worker.name())
            || entry
                .worker_uid
                .as_deref()
                .is_some_and(|uid| uid != worker.uid())
        {
            return Err(conflict());
        }
        if entry.worker_name.as_deref() == Some(worker.name())
            && entry.worker_uid.as_deref() == Some(worker.uid())
        {
            return Ok(());
        }
        let mut next = state.data.clone();
        let unit = next.units.get_mut(&runtime).ok_or_else(conflict)?;
        unit.worker_name = Some(worker.name().to_owned());
        unit.worker_uid = Some(worker.uid().to_owned());
        self.commit(&mut state, next)
    }

    /// Record the init container before streaming any trusted artifacts into it.
    pub(crate) fn bind_stage(
        &self,
        runtime: Id,
        pod_uid: &str,
        stage: &StageIdentity,
    ) -> Result<(), DriverError> {
        let id = stage.reference();
        container_id(id)?;
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined
            || entry.pod_uid.as_deref() != Some(pod_uid)
            || entry.worker_name.is_none()
            || entry.worker_uid.is_none()
            || entry
                .stage_id
                .as_deref()
                .is_some_and(|expected| expected != id)
        {
            return Err(conflict());
        }
        if entry.stage_id.as_deref() == Some(id) {
            return Ok(());
        }
        let mut next = state.data.clone();
        next.units.get_mut(&runtime).ok_or_else(conflict)?.stage_id = Some(id.to_owned());
        self.commit(&mut state, next)
    }

    /// Records exactly the Pod returned by create; the intent cannot adopt a later UID.
    pub(crate) fn bind_pod(
        &self,
        runtime: Id,
        intent: Id,
        pod_uid: &str,
    ) -> Result<(), DriverError> {
        if pod_uid.is_empty() {
            return Err(conflict());
        }
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined
            || entry.intent != intent
            || entry.pod_uid.as_deref().is_some_and(|uid| uid != pod_uid)
        {
            return Err(conflict());
        }
        if entry.pod_uid.as_deref() == Some(pod_uid) {
            return Ok(());
        }
        let mut next = state.data.clone();
        next.units.get_mut(&runtime).ok_or_else(conflict)?.pod_uid = Some(pod_uid.to_owned());
        self.commit(&mut state, next)
    }

    /// Pins both complete container IDs once; restarted or replaced containers are not adopted.
    pub(crate) fn bind_containers(
        &self,
        runtime: Id,
        pod_uid: &str,
        identities: &ContainerIdentities,
    ) -> Result<(), DriverError> {
        container_id(&identities.workload)?;
        container_id(&identities.guard)?;
        if identities.workload == identities.guard {
            return Err(conflict());
        }
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined
            || entry.pod_uid.as_deref() != Some(pod_uid)
            || entry.worker_name.is_none()
            || entry.worker_uid.is_none()
            || entry.stage_id.is_none()
            || entry.stage_id.as_deref() == Some(identities.workload.as_str())
            || entry.stage_id.as_deref() == Some(identities.guard.as_str())
            || entry.workload_id.is_some() != entry.guard_id.is_some()
            || entry
                .workload_id
                .as_ref()
                .is_some_and(|id| id != &identities.workload)
            || entry
                .guard_id
                .as_ref()
                .is_some_and(|id| id != &identities.guard)
        {
            return Err(conflict());
        }
        if entry.workload_id.as_deref() == Some(identities.workload.as_str())
            && entry.guard_id.as_deref() == Some(identities.guard.as_str())
        {
            return Ok(());
        }
        let mut next = state.data.clone();
        let entry = next.units.get_mut(&runtime).ok_or_else(conflict)?;
        entry.workload_id = Some(identities.workload.clone());
        entry.guard_id = Some(identities.guard.clone());
        self.commit(&mut state, next)
    }

    /// Durably reserves the next sequence, or reuses the one held for this exact pending request.
    pub(crate) fn reserve_sequence(
        &self,
        runtime: Id,
        pod_uid: &str,
        request: &[u8],
    ) -> Result<u64, DriverError> {
        if request.is_empty() || request.len() > MAX_FRAME_BYTES {
            return Err(conflict());
        }
        let request_hash = Hash::of(request);
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined
            || entry.pod_uid.as_deref() != Some(pod_uid)
            || entry.worker_name.is_none()
            || entry.worker_uid.is_none()
            || entry.stage_id.is_none()
            || entry.workload_id.is_none()
            || entry.guard_id.is_none()
        {
            return Err(conflict());
        }
        if let Some(pending) = &entry.pending {
            return if pending.request_hash == request_hash {
                Ok(pending.sequence)
            } else {
                Err(DriverError::Busy(
                    "a previous unit command is unresolved".into(),
                ))
            };
        }
        let sequence = entry
            .issued_sequence
            .checked_add(1)
            .ok_or_else(unavailable)?;
        let mut next = state.data.clone();
        let unit = next.units.get_mut(&runtime).ok_or_else(conflict)?;
        unit.issued_sequence = sequence;
        unit.pending = Some(PendingSequence {
            sequence,
            request_hash,
        });
        unit.pending_request = Some(request.to_vec());
        self.commit(&mut state, next)?;
        Ok(sequence)
    }

    /// Clear only the exact pending request after its authenticated result is durably retained by the caller.
    pub(crate) fn complete_sequence(
        &self,
        runtime: Id,
        pod_uid: &str,
        sequence: u64,
        request: &[u8],
        reply: &VerifiedReply,
    ) -> Result<(), DriverError> {
        if request.is_empty() || request.len() > MAX_FRAME_BYTES {
            return Err(conflict());
        }
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        let command = PendingSequence {
            sequence,
            request_hash: Hash::of(request),
        };
        if entry.quarantined
            || entry.pod_uid.as_deref() != Some(pod_uid)
            || entry.worker_name.is_none()
            || entry.worker_uid.is_none()
            || entry.stage_id.is_none()
            || !reply.matches(pod_uid, sequence, request, &entry.key.0)
        {
            return Err(conflict());
        }
        if entry.completed.as_ref() == Some(&command) {
            return Ok(());
        }
        if entry.pending.as_ref() != Some(&command) {
            return Err(conflict());
        }
        let mut next = state.data.clone();
        let unit = next.units.get_mut(&runtime).ok_or_else(conflict)?;
        unit.pending = None;
        unit.pending_request = None;
        unit.completed = Some(command);
        if matches!(
            serde_json::from_slice::<crate::control_protocol::GuardRequest>(request),
            Ok(crate::control_protocol::GuardRequest::Prepare { .. })
        ) && matches!(
            reply.response(),
            crate::control_protocol::GuardReply::State {
                phase: crate::control_protocol::GuardPhase::Stopped
            }
        ) {
            unit.prepared = true;
        }
        self.commit(&mut state, next)
    }

    /// Reads a single recorded incarnation; uncertainty never becomes an absent or stopped fact.
    pub(crate) fn unit(&self, runtime: Id) -> Result<Option<UnitRecord>, DriverError> {
        let state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let Some(entry) = state.data.units.get(&runtime) else {
            return Ok(None);
        };
        if entry.quarantined {
            return Err(unavailable());
        }
        let pod_uid = entry.pod_uid.clone().ok_or_else(unavailable)?;
        let worker = entry
            .worker_name
            .as_ref()
            .zip(entry.worker_uid.as_ref())
            .map(|(name, uid)| RecordedWorker {
                name: name.clone(),
                uid: uid.clone(),
            });
        let containers =
            entry
                .workload_id
                .as_ref()
                .zip(entry.guard_id.as_ref())
                .map(|(workload, guard)| ContainerIdentities {
                    workload: workload.clone(),
                    guard: guard.clone(),
                });
        Ok(Some(UnitRecord {
            unit: orch_driver_contract::units::UnitRef {
                reference: pod_uid,
                name: entry.name.clone(),
                runtime_id: runtime,
                kind: entry.kind,
            },
            labels: entry.labels.clone(),
            worker,
            stage_id: entry.stage_id.clone(),
            containers,
            key: GuardKey::from_bytes(entry.key.0),
            prepared: entry.prepared,
            pending_request: entry.pending_request.clone(),
            admitted: entry.admitted.clone(),
            stage_header: entry.stage_header.clone(),
            removed: entry.removed,
        }))
    }

    /// The admitted request is frozen before any private bootstrap or workload start.
    pub(crate) fn bind_admission(
        &self,
        runtime: Id,
        expected: &serde_json::Value,
    ) -> Result<(), DriverError> {
        let mut state = self.lock()?;
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined || entry.admitted.as_ref().is_some_and(|old| old != expected) {
            return Err(conflict());
        }
        if entry.admitted.is_some() {
            return Ok(());
        }
        let mut next = state.data.clone();
        next.units.get_mut(&runtime).ok_or_else(conflict)?.admitted = Some(expected.clone());
        self.commit(&mut state, next)
    }

    pub(crate) fn bind_stage_header(
        &self,
        runtime: Id,
        header: &crate::stage_protocol::StageHeader,
    ) -> Result<(), DriverError> {
        let mut state = self.lock()?;
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined
            || entry.pod_uid.as_deref() != Some(header.pod_uid.as_str())
            || entry.stage_header.as_ref().is_some_and(|old| old != header)
        {
            return Err(conflict());
        }
        if entry.stage_header.is_some() {
            return Ok(());
        }
        let mut next = state.data.clone();
        next.units
            .get_mut(&runtime)
            .ok_or_else(conflict)?
            .stage_header = Some(header.clone());
        self.commit(&mut state, next)
    }

    /// Called only after a fresh successful stop and an exact UID-preconditioned delete.
    pub(crate) fn removed(&self, runtime: Id) -> Result<(), DriverError> {
        let mut state = self.lock()?;
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined || entry.pending.is_some() {
            return Err(conflict());
        }
        let mut next = state.data.clone();
        next.units.get_mut(&runtime).ok_or_else(conflict)?.removed = true;
        self.commit(&mut state, next)
    }

    /// Called when the unit's Pod is known to be gone: it is absent, or its workload container ended and the Pod
    /// has just been deleted by its exact UID. A command whose answer never came went to a Pod that no longer
    /// exists and can never be answered, so it is let go with the unit, where `removed` would refuse it.
    pub(crate) fn removed_with_pod(&self, runtime: Id) -> Result<(), DriverError> {
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined {
            return Err(conflict());
        }
        let mut next = state.data.clone();
        let unit = next.units.get_mut(&runtime).ok_or_else(conflict)?;
        unit.pending = None;
        unit.pending_request = None;
        unit.removed = true;
        self.commit(&mut state, next)
    }

    /// A bootstrap identity race makes this key unusable until an explicit recovery procedure.
    pub(crate) fn quarantine_unit(&self, runtime: Id) -> Result<(), DriverError> {
        let mut state = self.lock()?;
        if !state.healthy {
            return Err(unavailable());
        }
        let entry = state.data.units.get(&runtime).ok_or_else(conflict)?;
        if entry.quarantined {
            return Ok(());
        }
        let mut next = state.data.clone();
        next.units
            .get_mut(&runtime)
            .ok_or_else(conflict)?
            .quarantined = true;
        self.commit(&mut state, next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::runtime::unit_labels;
    fn private_parent() -> tempfile::TempDir {
        let parent = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        parent
    }

    fn signed_reply(
        guard: &mut crate::authenticated_control::ReplayGuard,
        key: &GuardKey,
        sequence: u64,
        request: &[u8],
    ) -> VerifiedReply {
        use crate::authenticated_control::{Admission, open_response, seal_request};
        use crate::control_protocol::{GuardPhase, GuardReply};
        let envelope = seal_request(key, "pod-one", sequence, request).unwrap();
        let Admission::Execute {
            request: decoded, ..
        } = guard.begin(sequence, &envelope).unwrap()
        else {
            panic!("first execution")
        };
        assert_eq!(decoded, request);
        guard
            .finish(
                sequence,
                request,
                GuardReply::State {
                    phase: GuardPhase::Stopped,
                },
            )
            .unwrap();
        let sealed = guard.sealed_reply(sequence, request).unwrap();
        open_response(key, "pod-one", sequence, request, &sealed).unwrap()
    }

    fn request() -> VolumeRequest {
        VolumeRequest {
            name: "workspace".into(),
            storage: Some("local-path".into()),
            labels: BTreeMap::from([(LABEL_SYSTEM.into(), "fixture".into())]),
        }
    }

    fn fixture_worker(uid: &str) -> WorkerIdentity {
        let pod = serde_json::from_value(serde_json::json!({"metadata":{"name":"fixture"},
            "spec":{"nodeName":"worker-one","containers":[{"name":"workload","image":"example"}]}}))
        .unwrap();
        let node = serde_json::from_value(
            serde_json::json!({"metadata":{"name":"worker-one","uid":uid,
            "labels":{"kubernetes.io/os":"linux","kubernetes.io/arch":"amd64"}},
            "status":{"conditions":[{"type":"Ready","status":"True"}]}}),
        )
        .unwrap();
        let selector = crate::platform::worker_selector("linux", "x86_64").unwrap();
        crate::platform::worker_for_pod(&pod, &node, &selector, true).unwrap()
    }

    fn fixture_stage(runtime: Id, digit: char) -> StageIdentity {
        let unit = orch_driver_contract::units::UnitRef {
            reference: "pod-one".into(),
            name: "work".into(),
            runtime_id: runtime,
            kind: RuntimeKind::Environment,
        };
        let labels = unit_labels("fixture", RuntimeKind::Environment, &runtime);
        let pod = serde_json::from_value(serde_json::json!({"metadata":{"name":"work","namespace":"ns","uid":"pod-one","annotations":labels},
            "spec":{"restartPolicy":"Never","containers":[{"name":"workload","image":"example"}]},
            "status":{"phase":"Pending","initContainerStatuses":[{"name":"stage","ready":false,"restartCount":0,
                "containerID":format!("containerd://{}", digit.to_string().repeat(64)),"state":{"running":{}}}]}})).unwrap();
        crate::pod_identity::read_stage(&pod, "ns", "fixture", &unit, false).unwrap()
    }

    #[test]
    fn intent_and_uid_survive_reopen_and_cannot_be_rebound() {
        let parent = private_parent();
        let directory = parent.path().join("receipts");
        let store = ReceiptStore::open(&directory, "cluster-and-namespace").unwrap();
        let first = store.begin(&request()).unwrap();
        assert_eq!(first.uid, None);
        assert_eq!(store.begin(&request()).unwrap(), first);
        assert!(
            store
                .bind_pv("workspace", first.intent, "uid-one", "pv-name", "pv-one")
                .is_err()
        );
        drop(store);
        // A restart before the API result was recorded keeps the same intent, without inventing a UID.
        let store = ReceiptStore::open(&directory, "cluster-and-namespace").unwrap();
        assert_eq!(store.begin(&request()).unwrap(), first);
        store.bind("workspace", first.intent, "uid-one").unwrap();
        store.bind("workspace", first.intent, "uid-one").unwrap();
        let legacy = std::fs::read_to_string(directory.join(JOURNAL_NAME)).unwrap();
        assert!(
            !legacy.contains("pv_uid") && !legacy.contains("pv_name"),
            "unbound claims keep the older journal shape"
        );
        store
            .bind_pv("workspace", first.intent, "uid-one", "pv-name", "pv-one")
            .unwrap();
        store
            .bind_pv("workspace", first.intent, "uid-one", "pv-name", "pv-one")
            .unwrap();
        assert!(
            store
                .bind_pv("workspace", first.intent, "uid-one", "pv-name", "pv-two")
                .is_err()
        );
        assert!(
            store
                .bind_pv("workspace", first.intent, "uid-one", "other-name", "pv-one")
                .is_err()
        );
        assert!(store.bind("workspace", first.intent, "uid-two").is_err());
        assert!(
            store
                .bind("workspace", orch_types::new_id(), "uid-one")
                .is_err()
        );
        drop(store);
        let recovered = ReceiptStore::open(&directory, "cluster-and-namespace").unwrap();
        assert_eq!(
            recovered.begin(&request()).unwrap(),
            ClaimIntent {
                intent: first.intent,
                uid: Some("uid-one".into()),
                pv_name: Some("pv-name".into()),
                pv_uid: Some("pv-one".into()),
            }
        );
        let mut changed = request();
        changed
            .labels
            .insert("orch.system".into(), "another".into());
        assert!(recovered.begin(&changed).is_err());
        drop(recovered);
        assert!(ReceiptStore::open(&directory, "a-different-cluster").is_err());
    }

    #[test]
    fn the_lock_refuses_a_second_owner_and_a_damaged_journal_is_not_overwritten() {
        let parent = private_parent();
        let directory = parent.path().join("receipts");
        let store = ReceiptStore::open(&directory, "scope").unwrap();
        assert!(matches!(
            ReceiptStore::open(&directory, "scope"),
            Err(DriverError::Busy(_))
        ));
        drop(store);
        let path = directory.join(JOURNAL_NAME);
        fs::write(&path, b"not a journal").unwrap();
        assert!(ReceiptStore::open(&directory, "scope").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not a journal");
    }

    #[cfg(unix)]
    #[test]
    fn a_shared_directory_is_not_used_for_receipts() {
        use std::os::unix::fs::PermissionsExt;
        let parent = private_parent();
        let directory = parent.path().join("shared");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ReceiptStore::open(&directory, "scope").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_substitute_the_journal_file() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let parent = private_parent();
        let directory = parent.path().join("receipts");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let target = parent.path().join("elsewhere");
        fs::write(&target, b"do not change").unwrap();
        symlink(&target, directory.join(JOURNAL_NAME)).unwrap();
        assert!(ReceiptStore::open(&directory, "scope").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"do not change");
    }

    #[test]
    fn an_ambiguous_panic_refuses_later_mutation() {
        let parent = private_parent();
        let directory = parent.path().join("receipts");
        let store = ReceiptStore::open(&directory, "scope").unwrap();
        let result = std::panic::catch_unwind(|| {
            let _held = store.state.lock().unwrap();
            panic!("simulate an interruption while journaling");
        });
        assert!(result.is_err());
        assert!(store.begin(&request()).is_err());
    }

    #[test]
    fn unit_keys_identities_and_sequences_are_durable_and_fenced() {
        use crate::authenticated_control::ReplayGuard;
        use crate::control_protocol::GuardRequest;
        use orch_plugin_contract::Secret;
        let parent = private_parent();
        let directory = parent.path().join("receipts");
        let runtime = orch_types::new_id();
        let request = serde_json::to_vec(&GuardRequest::Prepare {
            pod_uid: "pod-one".into(),
            ca: "fixture-ca".into(),
            enrollment: Secret::new("synthetic-enrollment-value"),
            environment: Secret::new("{}"),
            mounts: None,
        })
        .unwrap();
        let inspect = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "pod-one".into(),
        })
        .unwrap();
        let stop = serde_json::to_vec(&GuardRequest::Stop {
            pod_uid: "pod-one".into(),
            grace_millis: 0,
        })
        .unwrap();
        let labels = unit_labels("fixture", RuntimeKind::Environment, &runtime);
        let store = ReceiptStore::open(&directory, "scope").unwrap();
        let first = store
            .begin_unit(runtime, RuntimeKind::Environment, "work", &labels)
            .unwrap();
        assert!(first.pod_uid.is_none());
        assert!(
            store.unit(runtime).is_err(),
            "a unit with no Pod recorded yet is not readable"
        );
        assert!(
            store
                .reserve_sequence(runtime, "pod-one", &request)
                .is_err()
        );
        store.bind_pod(runtime, first.intent, "pod-one").unwrap();
        assert!(store.bind_pod(runtime, first.intent, "pod-two").is_err());
        let pair = ContainerIdentities {
            workload: format!("containerd://{}", "a".repeat(64)),
            guard: format!("containerd://{}", "b".repeat(64)),
        };
        assert!(store.bind_containers(runtime, "pod-one", &pair).is_err());
        let stage = fixture_stage(runtime, 'd');
        assert!(store.bind_stage(runtime, "pod-one", &stage).is_err());
        assert!(
            store
                .reserve_sequence(runtime, "pod-one", &request)
                .is_err()
        );
        let worker = fixture_worker("worker-uid");
        store.bind_worker(runtime, "pod-one", &worker).unwrap();
        store.bind_worker(runtime, "pod-one", &worker).unwrap();
        let replaced_worker = fixture_worker("replacement");
        assert!(
            store
                .bind_worker(runtime, "pod-one", &replaced_worker)
                .is_err()
        );
        assert!(store.bind_containers(runtime, "pod-one", &pair).is_err());
        store.bind_stage(runtime, "pod-one", &stage).unwrap();
        store.bind_stage(runtime, "pod-one", &stage).unwrap();
        assert!(
            store
                .bind_stage(runtime, "pod-one", &fixture_stage(runtime, 'e'))
                .is_err()
        );
        let mut reused_stage = pair.clone();
        reused_stage.guard = stage.reference().into();
        assert!(
            store
                .bind_containers(runtime, "pod-one", &reused_stage)
                .is_err()
        );
        store.bind_containers(runtime, "pod-one", &pair).unwrap();
        let mut replaced = pair.clone();
        replaced.guard = format!("containerd://{}", "c".repeat(64));
        assert!(
            store
                .bind_containers(runtime, "pod-one", &replaced)
                .is_err()
        );
        assert_eq!(
            store
                .reserve_sequence(runtime, "pod-one", &request)
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .reserve_sequence(runtime, "pod-one", &request)
                .unwrap(),
            1
        );
        assert!(matches!(
            store.reserve_sequence(runtime, "pod-one", &inspect),
            Err(DriverError::Busy(_))
        ));
        let persisted = fs::read_to_string(directory.join(JOURNAL_NAME)).unwrap();
        assert!(!persisted.contains("synthetic-enrollment-value"));
        let recorded = store.unit(runtime).unwrap().unwrap();
        assert!(
            format!("{:?}", recorded.key).contains("[secret]"),
            "the guard key is redacted when printed"
        );
        let key = recorded.key;
        let mut guard = ReplayGuard::new("pod-one", key.clone()).unwrap();
        let reply = signed_reply(&mut guard, &key, 1, &request);
        let other_key = GuardKey::from_bytes([3; 32]);
        let mut other_guard = ReplayGuard::new("pod-one", other_key.clone()).unwrap();
        let forged = signed_reply(&mut other_guard, &other_key, 1, &request);
        assert!(
            store
                .complete_sequence(runtime, "pod-one", 1, &request, &forged)
                .is_err()
        );
        drop(store);
        let recovered = ReceiptStore::open(&directory, "scope").unwrap();
        let again = recovered
            .begin_unit(runtime, RuntimeKind::Environment, "work", &labels)
            .unwrap();
        assert_eq!(again.intent, first.intent);
        assert_eq!(again.pod_uid.as_deref(), Some("pod-one"));
        let kept = recovered.unit(runtime).unwrap().unwrap();
        assert_eq!(kept.stage_id.as_deref(), Some(stage.reference()));
        assert_eq!(
            kept.worker,
            Some(RecordedWorker {
                name: "worker-one".into(),
                uid: "worker-uid".into()
            })
        );
        assert_eq!(kept.key.expose(), key.expose());
        assert_eq!(
            recovered
                .reserve_sequence(runtime, "pod-one", &request)
                .unwrap(),
            1
        );
        assert!(
            recovered
                .complete_sequence(runtime, "pod-one", 1, b"other", &reply)
                .is_err()
        );
        recovered
            .complete_sequence(runtime, "pod-one", 1, &request, &reply)
            .unwrap();
        recovered
            .complete_sequence(runtime, "pod-one", 1, &request, &reply)
            .unwrap();
        assert_eq!(
            recovered
                .reserve_sequence(runtime, "pod-one", &inspect)
                .unwrap(),
            2
        );
        recovered
            .complete_sequence(runtime, "pod-one", 1, &request, &reply)
            .unwrap();
        let observed = signed_reply(&mut guard, &key, 2, &inspect);
        recovered
            .complete_sequence(runtime, "pod-one", 2, &inspect, &observed)
            .unwrap();
        assert!(
            recovered
                .complete_sequence(runtime, "pod-one", 1, &request, &reply)
                .is_err()
        );
        assert_eq!(
            recovered
                .reserve_sequence(runtime, "pod-one", &stop)
                .unwrap(),
            3
        );
        recovered.quarantine_unit(runtime).unwrap();
        assert!(
            recovered
                .reserve_sequence(runtime, "pod-one", &stop)
                .is_err()
        );
        drop(recovered);
        let recovered = ReceiptStore::open(&directory, "scope").unwrap();
        assert!(
            recovered
                .begin_unit(runtime, RuntimeKind::Environment, "work", &labels)
                .is_err()
        );
    }

    #[test]
    fn a_unit_whose_pod_is_gone_is_removed_with_the_command_that_was_never_answered() {
        use crate::control_protocol::GuardRequest;
        let parent = private_parent();
        let directory = parent.path().join("receipts");
        let runtime = orch_types::new_id();
        let inspect = serde_json::to_vec(&GuardRequest::Inspect {
            pod_uid: "pod-one".into(),
        })
        .unwrap();
        let labels = unit_labels("fixture", RuntimeKind::Environment, &runtime);
        let store = ReceiptStore::open(&directory, "scope").unwrap();
        let first = store
            .begin_unit(runtime, RuntimeKind::Environment, "work", &labels)
            .unwrap();
        store.bind_pod(runtime, first.intent, "pod-one").unwrap();
        store
            .bind_worker(runtime, "pod-one", &fixture_worker("worker-uid"))
            .unwrap();
        store
            .bind_stage(runtime, "pod-one", &fixture_stage(runtime, 'd'))
            .unwrap();
        let pair = ContainerIdentities {
            workload: format!("containerd://{}", "a".repeat(64)),
            guard: format!("containerd://{}", "b".repeat(64)),
        };
        store.bind_containers(runtime, "pod-one", &pair).unwrap();
        assert_eq!(
            store
                .reserve_sequence(runtime, "pod-one", &inspect)
                .unwrap(),
            1
        );
        // The unit ended before its answer came: an ordinary removal does not let the command go...
        assert!(store.removed(runtime).is_err());
        assert!(!store.unit(runtime).unwrap().unwrap().removed);
        // ... and one for a Pod that is gone does, and is remembered.
        store.removed_with_pod(runtime).unwrap();
        let kept = store.unit(runtime).unwrap().unwrap();
        assert!(kept.removed);
        assert!(kept.pending_request.is_none());
        drop(store);
        let recovered = ReceiptStore::open(&directory, "scope").unwrap();
        assert!(recovered.unit(runtime).unwrap().unwrap().removed);
    }
}
