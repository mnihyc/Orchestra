//! Bounded reuse of immutable schema grammar, never of supplied values or validation outcomes.

use super::{Error, compile, validate_compiled};
use crate::Json;
use jsonschema::Validator;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

/// Retention limits for compiled schemas. A cache miss or oversized schema still validates normally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLimits {
    /// Maximum retained whole-document or local-pointer validators, including failed compilations.
    /// Zero disables retention.
    pub entries: usize,
    /// Maximum total serialized schema and pointer bytes retained as exact cache keys.
    /// This counts source bytes, not the compiled library's allocation overhead.
    pub schema_bytes: usize,
}

impl Default for CacheLimits {
    fn default() -> Self {
        // Bound entry overhead and retained declaration text separately. Neither is an input limit:
        // eviction and bypass only give up reuse, keeping arbitrary valid declarations usable.
        Self {
            entries: 128,
            schema_bytes: 8 * 1024 * 1024,
        }
    }
}

// Exact content is compared, not only a digest, object address, selected fragment or declaration id.
// Every entry uses the module's one fixed LocalOnly/linear-regex/format-validation option set.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    schema: Arc<str>,
    pointer: Option<Arc<str>>,
}
impl Key {
    fn bytes(&self) -> usize {
        self.schema
            .len()
            .saturating_add(self.pointer.as_deref().map_or(0, str::len))
    }
}

type Compiled = Result<Arc<Validator>, Error>;
type Slot = Arc<OnceLock<Compiled>>;

#[derive(Default)]
struct State {
    entries: HashMap<Key, Slot>,
    order: VecDeque<Key>,
    schema_bytes: usize,
}

/// Shares compiled grammar between calls with the same exact schema and optional local pointer.
///
/// The standard helpers use one process cache with [`CacheLimits::default`]. Embedders can own a
/// cache with different limits. Entries are evicted least recently used; compilation errors share
/// the same bounds. Each retained entry compiles once even when callers arrive concurrently.
/// Compilation and instance evaluation run outside the cache mutex. No instance, permission,
/// selected policy result or configuration answer is retained.
pub struct ValidatorCache {
    limits: CacheLimits,
    state: Mutex<State>,
    #[cfg(test)]
    compilations: std::sync::atomic::AtomicUsize,
}

impl Default for ValidatorCache {
    fn default() -> Self {
        Self::new(CacheLimits::default())
    }
}

impl ValidatorCache {
    /// Create an independent cache. Limits affect reuse only, never acceptance or fault reporting.
    #[must_use]
    pub fn new(limits: CacheLimits) -> Self {
        Self {
            limits,
            state: Mutex::new(State::default()),
            #[cfg(test)]
            compilations: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn slot(&self, key: Key) -> Slot {
        let bytes = key.bytes();
        if self.limits.entries == 0 || bytes > self.limits.schema_bytes {
            return Arc::new(OnceLock::new());
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = state.entries.get(&key).cloned() {
            state.order.retain(|old| old != &key);
            state.order.push_back(key);
            return slot;
        }
        while state.entries.len() >= self.limits.entries
            || state.schema_bytes.saturating_add(bytes) > self.limits.schema_bytes
        {
            let Some(old) = state.order.pop_front() else {
                break;
            };
            if state.entries.remove(&old).is_some() {
                state.schema_bytes -= old.bytes();
            }
        }
        let slot = Arc::new(OnceLock::new());
        state.schema_bytes += bytes;
        state.order.push_back(key.clone());
        let _previous = state.entries.insert(key, Arc::clone(&slot));
        slot
    }

    fn compiled(&self, schema: &Json, pointer: Option<&str>) -> Compiled {
        let key = Key {
            schema: schema.to_string().into(),
            pointer: pointer.map(Arc::from),
        };
        let slot = self.slot(key);
        slot.get_or_init(|| {
            #[cfg(test)]
            self.compilations
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            compile(schema, pointer).map(Arc::new)
        })
        .clone()
    }

    /// Check declaration grammar using the same local retrieval, regex and format rules.
    pub fn check(&self, schema: &Json) -> Result<(), Error> {
        self.compiled(schema, None).map(|_| ())
    }

    /// Evaluate this value afresh; cached grammar never implies an accepted value.
    pub fn validate(&self, schema: &Json, value: &Json) -> Result<(), Error> {
        let validator = self.compiled(schema, None)?;
        validate_compiled(&validator, value)
    }

    /// Test a candidate at a local rule, retaining references to its complete source document.
    #[must_use]
    pub fn allows_at(&self, schema: &Json, pointer: &str, value: &Json) -> bool {
        self.compiled(schema, Some(pointer))
            .is_ok_and(|validator| validator.is_valid(value))
    }
}

#[cfg(test)]
mod tests;
