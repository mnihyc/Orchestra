//! Native storage operations share serialization through their physical work and cleanup.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

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

/// Shared native storage-name fences for starts, data work and physical cleanup.
#[derive(Default)]
pub struct StorageLocks {
    locks: Mutex<VolumeHolds>,
}
impl StorageLocks {
    /// Holds the exact driver/name, or a shared name when driver is absent.
    pub async fn hold(&self, driver: Option<&str>, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let serial = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .serial((driver.map(str::to_owned), name.to_owned()));
        serial.lock_owned().await
    }
    /// Holds the original physical directory through data work and attachment publication.
    pub async fn hold_directory(
        &self,
        device: u64,
        inode: u64,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        self.hold(Some("directory"), &format!("directory:{device}:{inode}"))
            .await
    }
    /// Reserves one attachment's opened roots without retaining a partial group while it waits.
    pub async fn hold_directories(
        &self,
        directories: impl IntoIterator<Item = (u64, u64)>,
    ) -> crate::error::Result<Vec<tokio::sync::OwnedMutexGuard<()>>> {
        let directories: std::collections::BTreeSet<_> = directories.into_iter().collect();
        let serials: Vec<_> = directories
            .into_iter()
            .map(|(device, inode)| {
                self.locks
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .serial((
                        Some("directory".into()),
                        format!("directory:{device}:{inode}"),
                    ))
            })
            .collect();
        'retry: loop {
            crate::error::check_cancelled()?;
            let mut held = Vec::with_capacity(serials.len());
            for serial in &serials {
                match Arc::clone(serial).try_lock_owned() {
                    Ok(guard) => held.push(guard),
                    Err(_) => {
                        drop(held);
                        let waiting = Arc::clone(serial).lock_owned();
                        let guard = if let Some(cancel) = crate::error::cancellation() {
                            tokio::select! { guard = waiting => guard, () = cancel.cancelled() => return Err(crate::error::HostError::Cancelled) }
                        } else {
                            waiting.await
                        };
                        drop(guard);
                        continue 'retry;
                    }
                }
            }
            return Ok(held);
        }
    }

    #[cfg(test)]
    pub(crate) fn directory_contended(&self, device: u64, inode: u64) -> bool {
        self.locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(&(
                Some("directory".into()),
                format!("directory:{device}:{inode}"),
            ))
            .is_some_and(|serial| serial.strong_count() > 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn waiting_for_several_data_roots_does_not_hold_an_unblocked_home_or_workspace() {
        let locks = StorageLocks::default();
        let writing = locks.hold_directory(7, 2).await;
        let attach = locks.hold_directories([(7, 1), (7, 2)]);
        tokio::pin!(attach);
        assert!(futures::poll!(&mut attach).is_pending());
        let other = locks.hold_directory(7, 1);
        tokio::pin!(other);
        assert!(futures::poll!(&mut other).is_ready());
        drop(writing);
        assert!(futures::poll!(&mut attach).is_ready());
    }

    #[tokio::test]
    async fn changing_one_native_data_root_waits_for_its_reader_without_blocking_another_root() {
        let locks = StorageLocks::default();
        let original = locks.hold(Some("directory"), "first").await;
        let same = locks.hold(Some("directory"), "first");
        tokio::pin!(same);
        assert!(futures::poll!(&mut same).is_pending());
        let unrelated = locks.hold(Some("directory"), "second");
        tokio::pin!(unrelated);
        assert!(futures::poll!(&mut unrelated).is_ready());
        drop(original);
        assert!(futures::poll!(&mut same).is_ready());
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
