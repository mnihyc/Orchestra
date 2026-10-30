//! Named storages of the host: configuration, capacity, free space and private-view support.
//!
//! A host often has more than one kind of disk, so which disk a volume lands on is a choice and not an accident
//! of where the driver keeps things. A **storage** is a named place on the host where volumes are made; the
//! operator declares them in the node's configuration (`[[node.storage]]`, one of them the default), and the
//! host reports each with its capacity, its free space and whether a private view can be made on it. A host
//! that declares none has one, `default`, where its driver keeps its volumes anyway.
//!
//! **Where the figures come from.** A driver that can say what its storages are ([`crate::drivers::DriverExtras::storages`])
//! is believed: it knows where it keeps the data and whether its kernel can lay one directory over another.
//! Without one the node measures the directories the operator declared, and says that private views cannot be
//! made, because nothing falls back to shared when they cannot. With no storage declared and no driver to say,
//! the one storage is measured where the node keeps its own files.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use orch_driver_contract::volumes::{
    StorageFault, StorageInfo, implicit_storages, resolve_storage,
};
use orch_link_contract::events::NodeStorage;

use crate::drivers::DriverHost;
use crate::settings::NodeSettings;

/// Bytes in a gigabyte, which is what capacities are written in.
#[cfg(unix)]
const BYTES_PER_GB: u64 = 1_000_000_000;

/// The capacity and the free space of the file system that holds `path`, in gigabytes; nothing is known of a
/// path that cannot be looked at.
#[must_use]
#[cfg(unix)]
pub fn measure(path: &Path) -> (i64, i64) {
    match rustix::fs::statvfs(path) {
        Ok(stat) => {
            let unit = stat.f_frsize.max(1);
            let gb = |blocks: u64| {
                i64::try_from(blocks.saturating_mul(unit) / BYTES_PER_GB).unwrap_or(i64::MAX)
            };
            (gb(stat.f_blocks), gb(stat.f_bavail))
        }
        Err(_) => (0, 0),
    }
}

/// The host cannot measure capacity without a supported filesystem query.
#[must_use]
#[cfg(not(unix))]
pub fn measure(_path: &Path) -> (i64, i64) {
    (0, 0)
}

/// The identity of the file system that holds `path`, so that two storages on one disk are counted once.
#[cfg(unix)]
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|meta| meta.dev())
}

#[cfg(not(unix))]
fn device_of(_path: &Path) -> Option<u64> {
    None
}

/// The storages of the host.
pub struct Storages {
    settings: NodeSettings,
    drivers: Arc<DriverHost>,
}

impl std::fmt::Debug for Storages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storages").finish_non_exhaustive()
    }
}

impl Storages {
    /// The single driver's view of a pool; it is never replaced by the node's own machine.
    pub fn pool_resources(
        &self,
    ) -> Option<
        Result<
            orch_driver_contract::host::HostResources,
            orch_driver_contract::driver::DriverError,
        >,
    > {
        let drivers: Vec<_> = self
            .drivers
            .all()
            .into_iter()
            .filter(|(_, driver)| {
                driver.describe().host_scope == Some(orch_driver_contract::host::HostScope::Driver)
            })
            .collect();
        match drivers.as_slice() {
            [] => None,
            [(_, driver)] => Some(driver.contract().host_resources()),
            _ => Some(Err(orch_driver_contract::driver::DriverError::Unavailable(
                "several drivers claim the host's resource observations".into(),
            ))),
        }
    }

    /// A live path change must be activated by reopening drivers before any new data is made.
    #[must_use]
    pub fn policy_paths_match(
        &self,
        storages: &[orch_link_contract::policy::MachineStorage],
    ) -> bool {
        self.settings.storages.len() == storages.len()
            && storages.iter().all(|storage| {
                self.settings.storages.iter().any(|active| {
                    active.name == storage.name
                        && active.path == std::path::Path::new(&storage.path)
                        && active.default == storage.default
                })
            })
    }
    /// The storages of a host configured as `settings`, whose drivers are `drivers`.
    #[must_use]
    pub fn new(settings: &NodeSettings, drivers: Arc<DriverHost>) -> Self {
        Self {
            settings: settings.clone(),
            drivers,
        }
    }

    /// The storages as the host has them now.
    #[must_use]
    pub fn list(&self) -> Vec<StorageInfo> {
        let mut told: BTreeMap<String, StorageInfo> = BTreeMap::new();
        let drivers = self.drivers.all();
        let pool = drivers.iter().any(|(_, driver)| {
            driver.describe().host_scope == Some(orch_driver_contract::host::HostScope::Driver)
        });
        for (_, driver) in drivers.into_iter().filter(|(_, driver)| {
            !pool
                || driver.describe().host_scope
                    == Some(orch_driver_contract::host::HostScope::Driver)
        }) {
            for storage in driver.extras().storages().unwrap_or_default() {
                told.entry(storage.name.clone())
                    .and_modify(|known| known.private_views |= storage.private_views)
                    .or_insert(storage);
            }
        }
        if !told.is_empty() {
            return told.into_values().collect();
        }
        if pool {
            return Vec::new();
        }
        if self.settings.storages.is_empty() {
            let (capacity, free) = measure(&self.settings.data_dir);
            return implicit_storages(capacity, free);
        }
        self.settings
            .storages
            .iter()
            .map(|storage| {
                let (capacity_gb, free_gb) = measure(&storage.path);
                StorageInfo {
                    name: storage.name.clone(),
                    default: storage.default,
                    capacity_gb,
                    free_gb,
                    private_views: false,
                }
            })
            .collect()
    }

    /// The storages in the form the system is told them in.
    #[must_use]
    pub fn report(&self) -> Vec<NodeStorage> {
        self.list()
            .into_iter()
            .map(|storage| NodeStorage {
                name: storage.name,
                default: storage.default,
                capacity_gb: storage.capacity_gb,
                free_gb: storage.free_gb,
                private_views: storage.private_views,
            })
            .collect()
    }

    /// The storage a volume is made on: the one named, or the host's default when none is named.
    pub fn resolve(&self, requested: Option<&str>) -> Result<StorageInfo, StorageFault> {
        let storages = self.list();
        resolve_storage(requested, &storages).cloned()
    }

    /// What the host's disks hold together: the capacity and the free space of each file system that holds a
    /// storage, each counted once however many storages are on it.
    #[must_use]
    pub fn disk(&self) -> (i64, i64) {
        if let Some(resources) = self.pool_resources() {
            return resources
                .map(|resources| (resources.disk_gb, resources.disk_free_gb.unwrap_or(0)))
                .unwrap_or((0, 0));
        }
        let mut seen: Vec<u64> = Vec::new();
        let (mut capacity, mut free) = (0_i64, 0_i64);
        let paths: Vec<std::path::PathBuf> = if self.settings.storages.is_empty() {
            vec![self.settings.data_dir.clone()]
        } else {
            self.settings
                .storages
                .iter()
                .map(|storage| storage.path.clone())
                .collect()
        };
        for path in paths {
            match device_of(&path) {
                Some(device) if seen.contains(&device) => continue,
                Some(device) => seen.push(device),
                None => continue,
            }
            let (this_capacity, this_free) = measure(&path);
            capacity = capacity.saturating_add(this_capacity);
            free = free.saturating_add(this_free);
        }
        // Storages that are reported by a driver and have no directory the node can look at: their own sums.
        if seen.is_empty() {
            for storage in self.list() {
                capacity = capacity.saturating_add(storage.capacity_gb);
                free = free.saturating_add(storage.free_gb);
            }
        }
        (capacity, free)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::HostDriver;
    use crate::images::BlobDesk;
    use orch_testkit::doubles::driver::MemoryDriver;

    fn host(settings: &NodeSettings, drivers: Vec<HostDriver>) -> Arc<DriverHost> {
        DriverHost::of(settings, Arc::new(BlobDesk::new()), drivers)
    }

    #[test]
    fn a_host_that_declares_none_has_one_called_default() {
        let dir = tempfile::tempdir().unwrap();
        let settings = NodeSettings::in_dir(dir.path());
        let storages = Storages::new(&settings, host(&settings, Vec::new()));
        let list = storages.list();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].default), ("default", true));
        assert!(
            list[0].capacity_gb > 0,
            "the disk the node keeps its files on is measured"
        );
        assert_eq!(storages.resolve(None).unwrap().name, "default");
        assert!(storages.resolve(Some("fast")).is_err());
    }

    #[test]
    fn the_directories_the_operator_declared_are_measured_and_cannot_hold_private_views_without_a_driver()
     {
        let dir = tempfile::tempdir().unwrap();
        let (fast, bulk) = (dir.path().join("fast"), dir.path().join("bulk"));
        std::fs::create_dir_all(&fast).unwrap();
        std::fs::create_dir_all(&bulk).unwrap();
        let mut settings = NodeSettings::in_dir(dir.path());
        settings.storages = vec![
            crate::settings::StorageSetting {
                name: "fast".into(),
                path: fast,
                default: true,
            },
            crate::settings::StorageSetting {
                name: "bulk".into(),
                path: bulk,
                default: false,
            },
        ];
        let storages = Storages::new(&settings, host(&settings, Vec::new()));
        let report = storages.report();
        assert_eq!(report.len(), 2);
        assert!(report.iter().all(|storage| !storage.private_views));
        assert_eq!(storages.resolve(None).unwrap().name, "fast");
        assert_eq!(storages.resolve(Some("bulk")).unwrap().name, "bulk");
        let (capacity, _) = storages.disk();
        let (one, _) = measure(dir.path());
        assert_eq!(capacity, one, "two storages on one disk are one disk");
    }

    struct Reports(Vec<StorageInfo>);
    impl crate::drivers::DriverExtras for Reports {
        fn storages(&self) -> Option<Vec<StorageInfo>> {
            Some(self.0.clone())
        }
    }

    #[test]
    fn a_driver_that_says_what_its_storages_are_is_believed() {
        let dir = tempfile::tempdir().unwrap();
        let settings = NodeSettings::in_dir(dir.path());
        let told = vec![StorageInfo {
            name: "fast".into(),
            default: true,
            capacity_gb: 900,
            free_gb: 400,
            private_views: true,
        }];
        let driver =
            HostDriver::new(MemoryDriver::shared("memory")).with_extras(Arc::new(Reports(told)));
        let storages = Storages::new(&settings, host(&settings, vec![driver]));
        let report = storages.report();
        assert_eq!(report.len(), 1);
        assert_eq!((report[0].capacity_gb, report[0].free_gb), (900, 400));
        assert!(report[0].private_views);
    }
}
