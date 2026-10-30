//! Serialize mutations of the same native name without blocking other names.

use std::collections::BTreeMap;
use std::ops::Bound::{self, Excluded, Unbounded};
use std::sync::{Arc, Mutex, Weak};

use orch_driver_contract::driver::DriverError;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio::time::Instant;

/// A lookup reclaims a small page of released names instead of scanning every name that is held or waited for.
/// One lookup adds at most one entry, so a sweep of this size also catches up with the names that were released.
const SWEEP_BATCH: usize = 32;

#[derive(Default)]
struct Names {
    entries: BTreeMap<String, Weak<AsyncMutex<()>>>,
    after: Option<String>,
}

impl Names {
    /// The lock of `name`, made if nobody holds or waits for it, after reclaiming the next page of released names.
    fn lock_of(&mut self, name: &str) -> Arc<AsyncMutex<()>> {
        // A `String` is both `Borrow<String>` and `Borrow<str>`, so the key type of the range has to be named.
        let start: Bound<&String> = self.after.as_ref().map_or(Unbounded, Excluded);
        let mut page: Vec<String> = self
            .entries
            .range::<String, _>((start, Unbounded))
            .take(SWEEP_BATCH)
            .map(|(key, _)| key.clone())
            .collect();
        if page.is_empty() {
            page = self.entries.keys().take(SWEEP_BATCH).cloned().collect();
        }
        self.after = page.last().cloned();
        for old in page {
            if self
                .entries
                .get(&old)
                .is_some_and(|lock| lock.strong_count() == 0)
            {
                self.entries.remove(&old);
            }
        }
        if let Some(lock) = self.entries.get(name).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        self.entries.insert(name.to_owned(), Arc::downgrade(&lock));
        lock
    }
}

#[derive(Default)]
pub(crate) struct KeyedLocks {
    names: Mutex<Names>,
}

impl KeyedLocks {
    /// The absolute deadline includes time waiting for another operation.
    pub(crate) async fn hold(
        &self,
        name: &str,
        deadline: Instant,
    ) -> Result<OwnedMutexGuard<()>, DriverError> {
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        let lock = self
            .names
            .lock()
            .map_err(|_| {
                DriverError::Unavailable("the native operation lock is unavailable".into())
            })?
            .lock_of(name);
        let held = tokio::time::timeout_at(deadline, lock.lock_owned())
            .await
            .map_err(|_| DriverError::Timeout)?;
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        Ok(held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn a_name_is_serialized_while_other_names_remain_independent() {
        let locks = Arc::new(KeyedLocks::default());
        let entered = Arc::new(AtomicUsize::new(0));
        let deadline = Instant::now() + Duration::from_secs(5);
        let first = locks.hold("workspace", deadline).await.unwrap();
        let other_locks = Arc::clone(&locks);
        let other_entered = Arc::clone(&entered);
        let waiter = tokio::spawn(async move {
            let _held = other_locks.hold("workspace", deadline).await.unwrap();
            other_entered.fetch_add(1, Ordering::SeqCst);
        });
        tokio::task::yield_now().await;
        assert_eq!(entered.load(Ordering::SeqCst), 0);
        let independent = locks.hold("home", deadline).await.unwrap();
        drop(independent);
        drop(first);
        waiter.await.unwrap();
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        let _next = locks.hold("new", deadline).await.unwrap();
        let names = locks.names.lock().unwrap();
        assert_eq!(names.entries.len(), 1);
        assert!(names.entries.contains_key("new"));
    }

    #[tokio::test]
    async fn an_expired_wait_does_not_take_the_name() {
        let locks = KeyedLocks::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        let _first = locks.hold("work", deadline).await.unwrap();
        assert!(matches!(
            locks.hold("work", Instant::now()).await,
            Err(DriverError::Timeout)
        ));
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn a_million_retired_names_keep_only_a_bounded_sweep_page_beyond_the_live_ones() {
        const LIVE: usize = 1000;
        const NAMES: usize = 1_000_000;
        let mut table = Names::default();
        let live: Vec<_> = (0..LIVE)
            .map(|index| table.lock_of(&format!("live-{index:04}")))
            .collect();
        let bound = LIVE + LIVE.div_ceil(SWEEP_BATCH - 1) + SWEEP_BATCH + 2;
        for index in 0..NAMES {
            drop(table.lock_of(&format!("retired-{index:07}")));
            assert!(
                table.entries.len() <= bound,
                "released names grew to {}",
                table.entries.len()
            );
        }
        for (index, held) in live.iter().enumerate() {
            let again = table.lock_of(&format!("live-{index:04}"));
            assert!(Arc::ptr_eq(held, &again), "a live lock was replaced");
        }
    }
}
