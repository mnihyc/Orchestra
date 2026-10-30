//! What the node records of the runtimes it made, kept on disk.
//!
//! A command to start, stop or remove a runtime names its identifier and nothing else, and a node that is
//! restarted still has to know which unit that is, which volumes it stands on, whether its connector has
//! enrolled and which enrollment token it was given. The ledger is that memory: one file in the node's data
//! directory, written whole and atomically whenever it changes, so that it is either as it was or as it is
//! now. It is not the system's record of the runtimes (the system has its own rows and the node never reads
//! them); it is only what this node needs to answer the commands it is given, and the driver's own listing of
//! units is what it is checked against ([`crate::reconciliation`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use orch_connector_core::policy::write_atomically;
use orch_driver_contract::network::NetworkPolicy;
use orch_driver_contract::runtime::{Resources, RuntimeKind};
use orch_driver_contract::sources::BaseImage;
use orch_driver_contract::units::UnitRef;
use orch_types::{Hash, Id, Time};
use serde::{Deserialize, Serialize};

/// The file of the ledger, in the `state` directory of the node's data directory.
pub const LEDGER_FILE: &str = "runtimes.json";

/// The version of the file's format.
const LEDGER_VERSION: u32 = 1;

/// Why the ledger could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// The file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// The file is not a ledger. It is left as it is, and the node does not start over it: what it holds is
    /// the only record of which units the node made.
    #[error("{path} is damaged and is left as it is: {why}")]
    Damaged {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        why: String,
    },
}

/// What a runtime's unit was made from, so that the node can tell when the image behind a tag has moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageUse {
    /// The source key within the owning driver's image namespace: a reference or build definition.
    pub key: String,
    /// The registry reference, for a spec that names one.
    pub reference: Option<String>,
    /// Whether a digest pins it: a pinned image never falls behind its tag.
    pub pinned: bool,
}

/// What the node knows of one runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The runtime.
    pub runtime_id: Id,
    /// Its pool.
    pub kind: RuntimeKind,
    /// The driver that made its unit.
    pub driver: String,
    /// The unit, as the driver named it.
    pub unit: UnitRef,
    /// The names of the volumes its unit stands on. Two units on one volume never run at once.
    pub volumes: Vec<String>,
    /// The hash of the enrollment token its unit was given. The token itself is never kept.
    pub token: Option<Hash>,
    /// Whether its connector is known to have enrolled.
    pub enrolled: bool,
    /// What the unit was made with as its limits.
    pub resources: Resources,
    /// What the image of the unit was made from.
    pub image: Option<ImageUse>,
    /// When the node made the unit.
    pub created_at: Time,
    /// When the unit was last started, if it was.
    pub started_at: Option<Time>,
    /// How long its connector has to enroll, in seconds, where its runtime needs another time than the node's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enroll_timeout_secs: Option<u64>,
}

/// What the node noted when it last made an image available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageNote {
    /// The digest then.
    pub digest: Option<String>,
    /// For a build: the base images it started from, with the digests they had.
    pub bases: Vec<BaseImage>,
}

/// The ordinary mounts in Docker's config and the approved live set reapplied after every start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MountConfiguration {
    /// What a new namespace initially has from the unit's ordinary mount configuration.
    pub base: Vec<orch_driver_contract::mounts::MountSpec>,
    /// The approved complete set, including a change that awaits recreate.
    pub wanted: Vec<orch_driver_contract::mounts::MountSpec>,
    /// The unit's workspace, home and other protected roots.
    pub reserved: Vec<String>,
    /// Last completely accepted set, for recovery while the same namespace is still running.
    #[serde(default)]
    pub applied: Vec<orch_driver_contract::mounts::MountSpec>,
    /// Its driver-observed start identity, rather than a PID that could be reused.
    #[serde(default)]
    pub applied_started_at: Option<Time>,
    /// Kept for runtime reports too, so a failed replay does not disappear after a node restart.
    #[serde(default)]
    pub pending_reason: Option<String>,
    /// Retry a transient failure in the same namespace; permanent refusals await a new start or request.
    #[serde(default)]
    pub retry_pending: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    resource_uses: BTreeMap<String, Time>,
    #[serde(default)]
    resource_occupied: BTreeSet<String>,
    #[serde(default)]
    kept_epoch: u64,
    #[serde(default)]
    kept_runtime: BTreeMap<Id, orch_driver_contract::kept::KeptImage>,
    /// Every value of the `orch.system` label the node has been given: it lists the units of each.
    #[serde(default)]
    systems: BTreeSet<String>,
    #[serde(default)]
    runtimes: BTreeMap<Id, Record>,
    #[serde(default)]
    images: BTreeMap<String, ImageNote>,
    /// Policies acknowledged by this node, including failed requests that keep an old open unit stopped.
    #[serde(default)]
    networks: BTreeMap<Id, NetworkPolicy>,
    /// Template boundary and granted live state, containing no launch credentials.
    #[serde(default)]
    capabilities: BTreeMap<Id, CapabilityConfiguration>,
    #[serde(default)]
    mounts: BTreeMap<Id, MountConfiguration>,
    /// Borrowed roots checked by this node, keyed by their stable data-root name.
    #[serde(default)]
    directories: BTreeMap<Id, BTreeMap<String, orch_driver_contract::runtime::DirectoryBinding>>,
    /// What each unit's request said it serves, for the operator's hooks.
    #[serde(default)]
    about: BTreeMap<Id, crate::hooks::UnitAbout>,
    /// Whether each unit came up or went down, as far as the operator's hooks were run for it.
    #[serde(default)]
    hooks: BTreeMap<Id, crate::hooks::HookPhase>,
    /// Source units paused by a whole-volume export; interrupted exports resume them on recovery.
    #[serde(default)]
    volume_copy_holds: BTreeSet<Id>,
}

/// Durable desired and verified capability sets for one prepared unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityConfiguration {
    /// What the controller has authorized and the next start must replay.
    pub desired: orch_driver_contract::capabilities::CapabilityState,
    /// The last complete helper result.
    pub applied: orch_driver_contract::capabilities::CapabilityState,
    /// An interrupted operation is reconciled before readiness is reported.
    pub changing: bool,
    /// Namespace start at which the last replay completed; a daemon restart must replay again.
    #[serde(default)]
    pub applied_started_at: Option<Time>,
}

/// The ledger of one node.
#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    state: Mutex<File>,
}

impl Ledger {
    /// The latest observed use of an opaque inventory object.
    #[must_use]
    pub fn resource_last_used(&self, id: &str) -> Option<Time> {
        self.lock().resource_uses.get(id).copied()
    }

    /// Records explicit cache use before its command's success is acknowledged.
    pub fn resource_used(&self, id: String, at: Time) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let at = state
            .resource_uses
            .get(&id)
            .copied()
            .map_or(at, |before| before.max(at));
        let before = state.resource_uses.insert(id.clone(), at);
        if let Err(error) = self.save(&state) {
            match before {
                Some(before) => {
                    state.resource_uses.insert(id, before);
                }
                None => {
                    state.resource_uses.remove(&id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    /// Records observed use in the existing ledger, starting an unknown cache's age at its first observation.
    pub fn resource_uses(
        &self,
        all: &BTreeSet<String>,
        used: &BTreeSet<String>,
        at: Time,
    ) -> Result<BTreeMap<String, Time>, LedgerError> {
        let mut state = self.lock();
        let before = state.resource_uses.clone();
        let occupied_before = state.resource_occupied.clone();
        state.resource_uses.retain(|id, _| all.contains(id));
        for id in all {
            let last = state.resource_uses.entry(id.clone()).or_insert(at);
            // A dependency disappearing between polls must age from its end, never its old start.
            // Keeping the previous occupied set also covers a crash after native removal.
            if used.contains(id) || occupied_before.contains(id) {
                *last = (*last).max(at);
            }
        }
        state.resource_occupied = used.clone();
        if let Err(error) = self.save(&state) {
            state.resource_uses = before;
            state.resource_occupied = occupied_before;
            return Err(error);
        }
        Ok(state.resource_uses.clone())
    }

    /// Persists a unit's dependencies before it can disappear, so recovery observes their end.
    pub fn resource_occupying(&self, ids: &BTreeSet<String>, at: Time) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let before = state.resource_uses.clone();
        let occupied_before = state.resource_occupied.clone();
        for id in ids {
            let last = state.resource_uses.entry(id.clone()).or_insert(at);
            *last = (*last).max(at);
            state.resource_occupied.insert(id.clone());
        }
        if let Err(error) = self.save(&state) {
            state.resource_uses = before;
            state.resource_occupied = occupied_before;
            return Err(error);
        }
        Ok(())
    }

    /// Stamps confirmed occupancy end without counting a reported use or releasing other holds.
    pub fn resource_occupancy_ended(
        &self,
        ids: &BTreeSet<String>,
        at: Time,
    ) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let before = state.resource_uses.clone();
        let occupied_before = state.resource_occupied.clone();
        for id in ids {
            let last = state.resource_uses.entry(id.clone()).or_insert(at);
            *last = (*last).max(at);
            state.resource_occupied.remove(id);
        }
        if let Err(error) = self.save(&state) {
            state.resource_uses = before;
            state.resource_occupied = occupied_before;
            return Err(error);
        }
        Ok(())
    }

    /// Canonical borrowed roots and identities retained across node restarts.
    pub fn directories(
        &self,
        runtime: Id,
    ) -> BTreeMap<String, orch_driver_contract::runtime::DirectoryBinding> {
        self.lock()
            .directories
            .get(&runtime)
            .cloned()
            .unwrap_or_default()
    }
    /// Persist the checked binding before a start or a source export can use it.
    pub fn set_directories(
        &self,
        runtime: Id,
        roots: BTreeMap<String, orch_driver_contract::runtime::DirectoryBinding>,
    ) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let previous = state.directories.insert(runtime, roots);
        if let Err(error) = self.save(&state) {
            match previous {
                Some(previous) => {
                    state.directories.insert(runtime, previous);
                }
                None => {
                    state.directories.remove(&runtime);
                }
            }
            return Err(error);
        }
        Ok(())
    }
    /// Units whose export briefly holds their processes still.
    pub fn volume_copy_holds(&self) -> BTreeSet<Id> {
        self.lock().volume_copy_holds.clone()
    }

    /// Journals before pause and clears only after resume, so a node crash cannot strand a source.
    pub fn set_volume_copy_hold(&self, runtime: Id, held: bool) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let previous = state.volume_copy_holds.contains(&runtime);
        if held {
            state.volume_copy_holds.insert(runtime);
        } else {
            state.volume_copy_holds.remove(&runtime);
        }
        if let Err(error) = self.save(&state) {
            if previous {
                state.volume_copy_holds.insert(runtime);
            } else {
                state.volume_copy_holds.remove(&runtime);
            }
            return Err(error);
        }
        Ok(())
    }
    /// Current inventory epoch; every accepted image mutation changes it before its daemon effect.
    #[must_use]
    pub fn kept_epoch(&self) -> u64 {
        self.lock().kept_epoch
    }

    /// Invalidates a sweep prepared before this image operation, durably.
    pub fn begin_kept_change(&self) -> Result<u64, LedgerError> {
        let mut state = self.lock();
        let old = state.kept_epoch;
        state.kept_epoch = old.saturating_add(1);
        if let Err(error) = self.save(&state) {
            state.kept_epoch = old;
            return Err(error);
        }
        Ok(state.kept_epoch)
    }

    /// The installed-files source of a current unit, which a local sweep must protect while it runs.
    pub fn kept_for(&self, runtime: Id) -> Option<orch_driver_contract::kept::KeptImage> {
        self.lock().kept_runtime.get(&runtime).cloned()
    }
    pub fn set_kept_for(
        &self,
        runtime: Id,
        image: orch_driver_contract::kept::KeptImage,
    ) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let old = state.kept_runtime.insert(runtime, image);
        if let Err(error) = self.save(&state) {
            match old {
                Some(old) => {
                    state.kept_runtime.insert(runtime, old);
                }
                None => {
                    state.kept_runtime.remove(&runtime);
                }
            }
            return Err(error);
        }
        Ok(())
    }
    /// This runtime's frozen boundary and live grants, retained across node restarts.
    #[must_use]
    pub fn capabilities(&self, runtime: Id) -> Option<CapabilityConfiguration> {
        self.lock().capabilities.get(&runtime).cloned()
    }

    /// Retains a capability intent atomically, restoring memory too when the journal cannot be written.
    pub fn set_capabilities(
        &self,
        runtime: Id,
        capabilities: CapabilityConfiguration,
    ) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let old = state.capabilities.insert(runtime, capabilities);
        if let Err(error) = self.save(&state) {
            match old {
                Some(old) => {
                    state.capabilities.insert(runtime, old);
                }
                None => {
                    state.capabilities.remove(&runtime);
                }
            }
            return Err(error);
        }
        Ok(())
    }
    /// Opens the ledger of the node whose data directory is `data_dir`; there is none yet on a node's first
    /// start. A file that is not a ledger is an error and is not touched.
    pub fn open(data_dir: &Path) -> Result<Self, LedgerError> {
        let path = data_dir.join("state").join(LEDGER_FILE);
        let state = match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<File>(&bytes).map_err(|error| LedgerError::Damaged {
                    path: path.clone(),
                    why: error.to_string(),
                })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => File {
                version: LEDGER_VERSION,
                ..File::default()
            },
            Err(source) => return Err(LedgerError::Io { path, source }),
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    fn lock(&self) -> MutexGuard<'_, File> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn save(&self, state: &File) -> Result<(), LedgerError> {
        let bytes = serde_json::to_vec_pretty(state).map_err(|error| LedgerError::Damaged {
            path: self.path.clone(),
            why: error.to_string(),
        })?;
        write_atomically(&self.path, &bytes).map_err(|source| LedgerError::Io {
            path: self.path.clone(),
            source,
        })
    }

    /// The record of a runtime.
    #[must_use]
    pub fn get(&self, runtime_id: Id) -> Option<Record> {
        self.lock().runtimes.get(&runtime_id).cloned()
    }

    /// The record of the runtime whose unit has this name.
    #[must_use]
    pub fn by_unit_name(&self, name: &str) -> Option<Record> {
        self.lock()
            .runtimes
            .values()
            .find(|record| record.unit.name == name)
            .cloned()
    }

    /// Every record.
    #[must_use]
    pub fn all(&self) -> Vec<Record> {
        self.lock().runtimes.values().cloned().collect()
    }

    /// The last policy requested of this runtime, kept across restarts even when installation failed.
    #[must_use]
    pub fn network(&self, runtime: Id) -> Option<NetworkPolicy> {
        self.lock().networks.get(&runtime).cloned()
    }

    /// The complete desired set and its last accepted namespace identity.
    #[must_use]
    pub fn mounts(&self, runtime: Id) -> Option<MountConfiguration> {
        self.lock().mounts.get(&runtime).cloned()
    }

    /// Runtime observations retain the mount-specific pending reason alongside the driver's own detail.
    #[must_use]
    pub fn mount_detail(&self, runtime: Id, own: Option<String>) -> Option<String> {
        match self
            .mounts(runtime)
            .and_then(|mounts| mounts.pending_reason)
        {
            Some(reason) => Some(match own {
                Some(own) => format!("mount changes pending: {reason}; {own}"),
                None => format!("mount changes pending: {reason}"),
            }),
            None => own,
        }
    }

    /// Keeps the approved change before the first namespace operation, atomically with rollback on disk failure.
    pub fn set_mounts(&self, runtime: Id, mounts: MountConfiguration) -> Result<(), LedgerError> {
        let mut state = self.lock();
        if state.mounts.get(&runtime) == Some(&mounts) {
            return Ok(());
        }
        let old = state.mounts.insert(runtime, mounts);
        if let Err(error) = self.save(&state) {
            match old {
                Some(old) => {
                    state.mounts.insert(runtime, old);
                }
                None => {
                    state.mounts.remove(&runtime);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    /// Retains a policy before any driver operation can stop or start its unit.
    pub fn set_network(&self, runtime: Id, policy: NetworkPolicy) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let previous = state.networks.insert(runtime, policy);
        if let Err(error) = self.save(&state) {
            match previous {
                Some(previous) => {
                    state.networks.insert(runtime, previous);
                }
                None => {
                    state.networks.remove(&runtime);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    /// Puts a record in the ledger, in place of the one the runtime had.
    pub fn insert(&self, record: Record) -> Result<(), LedgerError> {
        let mut state = self.lock();
        state.runtimes.insert(record.runtime_id, record);
        self.save(&state)
    }

    /// Changes a record. Answers whether there was one.
    pub fn update(
        &self,
        runtime_id: Id,
        change: impl FnOnce(&mut Record),
    ) -> Result<bool, LedgerError> {
        let mut state = self.lock();
        let Some(record) = state.runtimes.get_mut(&runtime_id) else {
            return Ok(false);
        };
        change(record);
        self.save(&state)?;
        Ok(true)
    }

    /// Takes a record out of the ledger.
    pub fn remove(&self, runtime_id: Id) -> Result<Option<Record>, LedgerError> {
        let mut state = self.lock();
        let removed = state.runtimes.remove(&runtime_id);
        if removed.is_some() {
            state.mounts.remove(&runtime_id);
            state.directories.remove(&runtime_id);
            state.capabilities.remove(&runtime_id);
            state.kept_runtime.remove(&runtime_id);
            state.about.remove(&runtime_id);
            state.hooks.remove(&runtime_id);
            self.save(&state)?;
        }
        Ok(removed)
    }

    /// What the request of a runtime's unit said it serves, for the operator's hooks.
    #[must_use]
    pub fn about(&self, runtime: Id) -> Option<crate::hooks::UnitAbout> {
        self.lock().about.get(&runtime).cloned()
    }

    /// Keeps what a runtime's unit serves, as its latest request says it.
    pub fn set_about(
        &self,
        runtime: Id,
        about: crate::hooks::UnitAbout,
    ) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let previous = state.about.insert(runtime, about);
        if let Err(error) = self.save(&state) {
            match previous {
                Some(previous) => {
                    state.about.insert(runtime, previous);
                }
                None => {
                    state.about.remove(&runtime);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    /// Whether a runtime's unit last came up or went down, as far as its hooks were run.
    #[must_use]
    pub fn hook_phase(&self, runtime: Id) -> Option<crate::hooks::HookPhase> {
        self.lock().hooks.get(&runtime).copied()
    }

    /// Notes that a runtime's unit came up or went down; a note that is already so writes nothing.
    pub fn set_hook_phase(
        &self,
        runtime: Id,
        phase: crate::hooks::HookPhase,
    ) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let previous = state.hooks.insert(runtime, phase);
        if previous == Some(phase) {
            return Ok(());
        }
        if let Err(error) = self.save(&state) {
            match previous {
                Some(previous) => {
                    state.hooks.insert(runtime, previous);
                }
                None => {
                    state.hooks.remove(&runtime);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    /// The records of the runtimes, other than `runtime_id`, whose units stand on a volume of that name.
    #[must_use]
    pub fn sharing_volumes(&self, runtime_id: Id, volumes: &[String]) -> Vec<Record> {
        self.lock()
            .runtimes
            .values()
            .filter(|record| {
                record.runtime_id != runtime_id
                    && record.volumes.iter().any(|volume| volumes.contains(volume))
            })
            .cloned()
            .collect()
    }

    /// The records of the runtimes whose units stand on the volume `name`.
    #[must_use]
    pub fn using_volume(&self, name: &str) -> Vec<Record> {
        self.lock()
            .runtimes
            .values()
            .filter(|record| record.volumes.iter().any(|volume| volume == name))
            .cloned()
            .collect()
    }

    /// The record, other than that of `runtime_id`, whose unit was given the enrollment token with this
    /// hash.
    #[must_use]
    pub fn holder_of_token(&self, runtime_id: Id, token: Hash) -> Option<Record> {
        self.lock()
            .runtimes
            .values()
            .find(|record| record.runtime_id != runtime_id && record.token == Some(token))
            .cloned()
    }

    /// Every value of the system label the node has been given.
    #[must_use]
    pub fn systems(&self) -> Vec<String> {
        self.lock().systems.iter().cloned().collect()
    }

    /// Notes a value of the system label. Answers whether it was new.
    pub fn adopt_system(&self, system: &str) -> Result<bool, LedgerError> {
        let mut state = self.lock();
        if !state.systems.insert(system.to_owned()) {
            return Ok(false);
        }
        self.save(&state)?;
        Ok(true)
    }

    /// What the node noted about the image under `key`.
    #[must_use]
    pub fn image(&self, key: &str) -> Option<ImageNote> {
        self.lock().images.get(key).cloned()
    }

    /// Every image the node noted.
    #[must_use]
    pub fn images(&self) -> BTreeMap<String, ImageNote> {
        self.lock().images.clone()
    }

    /// Notes what an image was made available as.
    pub fn note_image(&self, key: &str, note: ImageNote) -> Result<(), LedgerError> {
        let mut state = self.lock();
        if state.images.get(key) == Some(&note) {
            return Ok(());
        }
        state.images.insert(key.to_owned(), note);
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn record(volumes: &[&str]) -> Record {
        let id = new_id();
        Record {
            runtime_id: id,
            kind: RuntimeKind::Environment,
            driver: "memory".to_owned(),
            unit: UnitRef {
                reference: "r".to_owned(),
                name: format!("orch-environment-{}", &id.simple().to_string()[..12]),
                runtime_id: id,
                kind: RuntimeKind::Environment,
            },
            volumes: volumes.iter().map(|volume| (*volume).to_owned()).collect(),
            token: Some(Hash::of(id.as_bytes())),
            enrolled: false,
            resources: Resources::default(),
            image: None,
            created_at: "2026-10-11T12:00:00.000Z".parse().unwrap(),
            started_at: None,
            enroll_timeout_secs: None,
        }
    }

    #[test]
    fn what_was_recorded_is_there_after_a_restart_and_a_removed_runtime_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let (kept, gone) = (record(&["a"]), record(&["b"]));
        ledger.insert(kept.clone()).unwrap();
        ledger.insert(gone.clone()).unwrap();
        ledger.adopt_system("prod").unwrap();
        ledger.remove(gone.runtime_id).unwrap();
        drop(ledger);
        let again = Ledger::open(dir.path()).unwrap();
        assert_eq!(again.get(kept.runtime_id), Some(kept.clone()));
        assert_eq!(again.get(gone.runtime_id), None);
        assert_eq!(again.systems(), ["prod"]);
        assert_eq!(again.by_unit_name(&kept.unit.name), Some(kept));
    }

    #[test]
    fn runtimes_on_one_volume_are_found_by_it() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let (old, successor, other) = (
            record(&["ws", "state"]),
            record(&["ws", "state"]),
            record(&["elsewhere"]),
        );
        for record in [&old, &successor, &other] {
            ledger.insert(record.clone()).unwrap();
        }
        let sharing = ledger.sharing_volumes(old.runtime_id, &old.volumes);
        assert_eq!(sharing.len(), 1);
        assert_eq!(sharing[0].runtime_id, successor.runtime_id);
        assert_eq!(ledger.using_volume("ws").len(), 2);
        assert!(ledger.using_volume("nowhere").is_empty());
    }

    #[test]
    fn a_token_is_found_on_the_other_runtime_that_holds_it_and_never_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let first = record(&["a"]);
        ledger.insert(first.clone()).unwrap();
        let token = first.token.unwrap();
        assert!(ledger.holder_of_token(first.runtime_id, token).is_none());
        assert_eq!(
            ledger
                .holder_of_token(new_id(), token)
                .map(|r| r.runtime_id),
            Some(first.runtime_id)
        );
    }

    #[test]
    fn a_file_that_is_not_a_ledger_is_left_alone_and_the_node_does_not_start_over_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        let path = dir.path().join("state").join(LEDGER_FILE);
        std::fs::write(&path, "{ not json").unwrap();
        let error = Ledger::open(dir.path()).unwrap_err();
        assert!(matches!(error, LedgerError::Damaged { .. }), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn a_change_to_a_record_is_kept_and_a_change_to_none_is_reported_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let one = record(&["a"]);
        ledger.insert(one.clone()).unwrap();
        assert!(
            ledger
                .update(one.runtime_id, |r| r.enrolled = true)
                .unwrap()
        );
        assert!(!ledger.update(new_id(), |r| r.enrolled = true).unwrap());
        assert!(
            Ledger::open(dir.path())
                .unwrap()
                .get(one.runtime_id)
                .unwrap()
                .enrolled
        );
    }
}
