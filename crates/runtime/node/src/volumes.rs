//! Persistent volume lifecycle on a named storage, for runtimes, mounts and private views.
//!
//! An environment's workspace, its home and the state of its connector are volumes that outlive any unit; so
//! is a mount's volume location and the layer that keeps what one environment wrote in a private view. The
//! system names them (`orch-ws-…`, `orch-home-…`, `orch-state-…`, `orch-mount-…`, `orch-over-…`, the same on
//! every host) and says on which storage of the host each is made; the node makes sure the driver has it there.
//!
//! **A storage the host does not have is refused** with the reason, before the driver is asked, so that a
//! template that names `fast` is never quietly given the default. **A volume that a running unit stands on is
//! not removed**: the unit would lose the ground it works on. Removing what is not there is done.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::LABEL_SYSTEM;
use orch_driver_contract::units::UnitStateKind;
use orch_driver_contract::volumes::VolumeRequest;
use orch_link_contract::commands::{CommandError, ErrorCode, NodeVolumeEnsure, NodeVolumeRemove};

use crate::drivers::{DriverHost, HostDriver, INVALID, failure};
use crate::ledger::Ledger;
use crate::storages::Storages;

/// The longest a volume's name may be: what a file name may be, since a driver may keep a volume as a
/// directory of that name.
const NAME_LONGEST: usize = 255;

/// `None` fences the name across drivers; `Some` holds a particular driver's volume.
type VolumeKey = (Option<String>, String);

/// Reclaim a small page on each lookup instead of scanning all concurrent volume locks.
/// One lookup adds at most one entry, so a 32-entry sweep also catches up with retired names.
const LOCK_SWEEP_BATCH: usize = 32;

#[derive(Default)]
struct VolumeHolds {
    entries: BTreeMap<VolumeKey, Weak<tokio::sync::Mutex<()>>>,
    after: Option<VolumeKey>,
}

impl VolumeHolds {
    fn serial(&mut self, key: VolumeKey) -> Arc<tokio::sync::Mutex<()>> {
        use std::ops::Bound::{Excluded, Unbounded};
        let mut page: Vec<_> = self
            .entries
            .range((self.after.as_ref().map_or(Unbounded, Excluded), Unbounded))
            .take(LOCK_SWEEP_BATCH)
            .map(|(key, _)| key.clone())
            .collect();
        if page.is_empty() {
            page = self
                .entries
                .keys()
                .take(LOCK_SWEEP_BATCH)
                .cloned()
                .collect();
        }
        self.after = page.last().cloned();
        for old in page {
            if self
                .entries
                .get(&old)
                .is_some_and(|serial| serial.strong_count() == 0)
            {
                self.entries.remove(&old);
            }
        }
        if let Some(serial) = self.entries.get(&key).and_then(Weak::upgrade) {
            return serial;
        }
        let serial = Arc::new(tokio::sync::Mutex::new(()));
        self.entries.insert(key, Arc::downgrade(&serial));
        serial
    }
}

/// Whether `name` can be the name of a volume: letters, digits and `_`, `.`, `-`, starting with a letter or a
/// digit. That is what the Docker daemon takes, and it keeps a name from reaching outside a directory.
#[must_use]
pub fn is_volume_name(name: &str) -> bool {
    name.len() <= NAME_LONGEST
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && !name.contains("..")
}

/// The volumes of the host.
pub struct Volumes {
    policy: std::sync::Mutex<Option<Arc<orch_connector_core::policy::PolicyStore>>>,
    drivers: Arc<DriverHost>,
    storages: Arc<Storages>,
    ledger: Arc<Ledger>,
    locks: Mutex<VolumeHolds>,
}

impl std::fmt::Debug for Volumes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Volumes").finish_non_exhaustive()
    }
}

impl Volumes {
    /// The volumes of a host whose drivers, storages and units are these.
    #[must_use]
    pub fn new(drivers: Arc<DriverHost>, storages: Arc<Storages>, ledger: Arc<Ledger>) -> Self {
        Self {
            policy: std::sync::Mutex::new(None),
            drivers,
            storages,
            ledger,
            locks: Mutex::new(VolumeHolds::default()),
        }
    }

    /// Holds a physical volume through data work, helper cleanup and lifecycle changes.
    pub(crate) async fn hold(&self, driver: &str, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.hold_in(Some(driver), name).await
    }

    async fn hold_in(&self, driver: Option<&str>, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let serial = {
            let mut locks = self
                .locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = (driver.map(str::to_owned), name.to_owned());
            locks.serial(key)
        };
        serial.lock_owned().await
    }

    /// Holds a volume name while a unit checks its predecessors and starts.
    pub(crate) async fn hold_start(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.hold_in(None, name).await
    }

    /// The same live machine policy used by image and runtime admission.
    pub fn set_policy(&self, policy: Arc<orch_connector_core::policy::PolicyStore>) {
        *self
            .policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(policy);
    }

    fn driver(&self, name: &str) -> Result<Arc<HostDriver>, CommandError> {
        self.drivers.get(name).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                format!("this host has no driver {name:?}"),
            )
        })
    }

    /// Checks that the volume can be made where it is asked to be: the name is one, the driver keeps volumes,
    /// and the storage, if one is named, is the host's and one the driver can make a volume on.
    pub fn check(
        &self,
        driver: &HostDriver,
        name: &str,
        storage: Option<&str>,
    ) -> Result<(), CommandError> {
        if let Some(policy) = self
            .policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|store| store.policy())
        {
            self.drivers.check_configuration(
                &driver.describe().name,
                policy.drivers.configurations.get(&driver.describe().name),
            )?;
            if !policy.drivers.offers(&driver.describe().name) {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "drivers (node role): this driver is off in the machine operator's policy",
                ));
            }
            if !self.storages.policy_paths_match(&policy.storages) {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "storages (node role): storage paths changed; restart orch-node before making data on them",
                ));
            }
        }
        if !is_volume_name(name) {
            return Err(CommandError {
                code: INVALID.to_owned(),
                detail: format!("{name:?} is not a name a volume can have"),
                retryable: false,
                details: None,
            });
        }
        let descriptor = driver.describe();
        if !descriptor.volumes {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                format!("the driver {} keeps no volumes", descriptor.name),
            ));
        }
        if let Some(storage) = storage {
            if !descriptor.storages {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    format!(
                        "the driver {} cannot make a volume on a named storage, and {storage:?} is named",
                        descriptor.name
                    ),
                ));
            }
            self.storages
                .resolve(Some(storage))
                .map_err(|fault| CommandError::new(ErrorCode::NotFound, fault.to_string()))?;
        }
        Ok(())
    }

    /// Makes sure the volume is there, on the storage named or the host's default.
    pub async fn ensure(&self, request: &NodeVolumeEnsure) -> Result<(), CommandError> {
        let _held = self.hold(&request.driver, &request.name).await;
        let driver = self.driver(&request.driver)?;
        self.check(&driver, &request.name, request.storage.as_deref())?;
        if let Some(system) = request.labels.get(LABEL_SYSTEM)
            && let Err(error) = self.ledger.adopt_system(system)
        {
            tracing::warn!(%error, "the system a volume belongs to could not be noted");
        }
        if let Some(job) = request.labels.get("orch.recovery_job_id") {
            let volume = VolumeRequest {
                name: request.name.clone(),
                storage: request.storage.clone(),
                labels: request.labels.clone(),
            };
            if let Some(identity) = driver
                .contract()
                .volume_identity(&volume)
                .await
                .map_err(|error| failure(&error))?
                && identity.labels.get("orch.recovery_job_id") != Some(job)
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "restored data appeared after review; it is not this recovery's scratch volume",
                ));
            }
        }
        driver
            .contract()
            .ensure_volume(&VolumeRequest {
                name: request.name.clone(),
                storage: request.storage.clone(),
                labels: request.labels.clone(),
            })
            .await
            .map_err(|error| failure(&error))
    }

    /// Verifies both system and environment ownership before restored data can be reused.
    pub async fn inspect(
        &self,
        request: &orch_link_contract::commands::NodeVolumeInspect,
        system: &str,
    ) -> Result<orch_link_contract::commands::NodeVolumeInspectResult, CommandError> {
        let driver = self.driver(&request.driver)?;
        self.check(&driver, &request.name, request.storage.as_deref())?;
        let volume = VolumeRequest {
            name: request.name.clone(),
            storage: request.storage.clone(),
            labels: std::collections::BTreeMap::from([
                (LABEL_SYSTEM.into(), system.into()),
                (
                    "orch.environment_id".into(),
                    request.environment_id.to_string(),
                ),
            ]),
        };
        let Some(identity) = driver
            .contract()
            .volume_identity(&volume)
            .await
            .map_err(|error| failure(&error))?
        else {
            return Ok(orch_link_contract::commands::NodeVolumeInspectResult {
                exists: false,
                bytes: None,
            });
        };
        if volume
            .labels
            .iter()
            .any(|(key, value)| identity.labels.get(key) != Some(value))
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the restored volume belongs to another system or environment",
            ));
        }
        let bytes = driver
            .extras()
            .measure_volume(&volume)
            .await
            .ok()
            .map(|measure| measure.total_bytes);
        Ok(orch_link_contract::commands::NodeVolumeInspectResult {
            exists: true,
            bytes,
        })
    }

    /// Removes the volume and its data, unless a unit that runs stands on it.
    pub async fn remove(
        &self,
        request: &NodeVolumeRemove,
        system: &str,
    ) -> Result<(), CommandError> {
        let _start = self.hold_start(&request.name).await;
        let _held = self.hold(&request.driver, &request.name).await;
        let driver = self.driver(&request.driver)?;
        if !is_volume_name(&request.name) {
            return Err(CommandError {
                code: INVALID.to_owned(),
                detail: format!("{:?} is not a name a volume can have", request.name),
                retryable: false,
                details: None,
            });
        }
        if !request.labels.is_empty() {
            let expected = VolumeRequest {
                name: request.name.clone(),
                storage: request.storage.clone(),
                labels: request.labels.clone(),
            };
            let identity = driver
                .contract()
                .volume_identity(&expected)
                .await
                .map_err(|error| failure(&error))?;
            let Some(identity) = identity else {
                return Ok(());
            };
            if request
                .labels
                .iter()
                .any(|(key, value)| identity.labels.get(key) != Some(value))
                || identity
                    .labels
                    .get("orch.system")
                    .is_none_or(|name| name != system)
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the volume's protected ownership does not match this removal",
                ));
            }
        }
        for record in self.ledger.using_volume(&request.name) {
            let Some(holder) = self.drivers.get(&record.driver) else {
                return Err(failure(&DriverError::Unavailable(format!(
                    "the driver {} of the unit {} is unavailable, so its volume cannot be safely removed",
                    record.driver, record.unit.name
                ))));
            };
            let state = holder
                .contract()
                .inspect(&record.unit)
                .await
                .map_err(|error| failure(&error))?;
            if state.state == UnitStateKind::Running {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    format!(
                        "the unit {} is running on the volume {}; it is stopped before the volume is removed",
                        record.unit.name, request.name
                    ),
                ));
            }
        }
        match driver.contract().remove_volume(&request.name).await {
            Ok(()) | Err(DriverError::NotFound(_)) => Ok(()),
            Err(error) => Err(failure(&error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_has_a_name_a_directory_could_have_and_none_that_leaves_one() {
        for good in ["orch-ws-0199abcd-1234", "orch-test-node-1-state", "a.b_c-d"] {
            assert!(is_volume_name(good), "{good}");
        }
        for bad in [
            "",
            "-x",
            "../x",
            "a/b",
            "a b",
            "a..b",
            ".hidden",
            &"x".repeat(256),
        ] {
            assert!(!is_volume_name(bad), "{bad:?}");
        }
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn a_million_volume_lock_names_keep_only_a_bounded_sweep_page_beyond_live_locks() {
        const LIVE: usize = 1000;
        const NAMES: usize = 1_000_000;
        let mut table = VolumeHolds::default();
        let live: Vec<_> = (0..LIVE)
            .map(|index| table.serial((None, format!("live-{index:04}"))))
            .collect();
        let bound = LIVE + LIVE.div_ceil(LOCK_SWEEP_BATCH - 1) + LOCK_SWEEP_BATCH + 2;
        let mut peak = 0;
        for index in 0..NAMES {
            drop(table.serial((None, format!("retired-{index:07}"))));
            peak = peak.max(table.entries.len());
            assert!(
                table.entries.len() <= bound,
                "dead lock names grew to {}",
                table.entries.len()
            );
        }
        for (index, held) in live.iter().enumerate() {
            let again = table.serial((None, format!("live-{index:04}")));
            assert!(Arc::ptr_eq(held, &again), "a live fence was replaced");
        }
        assert!(peak <= bound);
    }
}
