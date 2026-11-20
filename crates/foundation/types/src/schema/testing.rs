//! Exact declaration compilation counts for tests that exercise a real consumer of the validator.
use crate::{Hash, Json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

static WATCHED: LazyLock<Mutex<HashMap<Hash, Weak<AtomicUsize>>>> = LazyLock::new(Mutex::default);

/// Observe one unique declaration through any cache or compiler thread for the lifetime of the handle.
pub fn observe_compilations(schema: &Json) -> Arc<AtomicUsize> {
    let mut watchers = WATCHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    watchers.retain(|_, count| count.strong_count() != 0);
    let digest = Hash::of(schema.to_string().as_bytes());
    if let Some(count) = watchers.get(&digest).and_then(Weak::upgrade) {
        return count;
    }
    let count = Arc::new(AtomicUsize::new(0));
    watchers.insert(digest, Arc::downgrade(&count));
    count
}

pub(super) fn record_compilation(schema: &Json) {
    let watchers = WATCHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !watchers.is_empty()
        && let Some(count) = watchers
            .get(&Hash::of(schema.to_string().as_bytes()))
            .and_then(Weak::upgrade)
    {
        count.fetch_add(1, Ordering::Relaxed);
    }
}
