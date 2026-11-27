//! Bounded reuse of immutable schema grammar, never of supplied values or validation outcomes.

use super::{Error, compile, validate_compiled};
use crate::{Hash, Json};
use jsonschema::Validator;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, Weak},
};

/// Idle retention limits. Active declarations own their grammar independently of these limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLimits {
    /// Maximum retained whole-document or local-pointer validators, including failed compilations.
    /// Zero disables idle retention.
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

// The digest indexes a declaration. Equality still checks its entire source and optional pointer.
#[derive(Clone, PartialEq, Eq)]
struct Key {
    digest: Hash,
    schema: Arc<str>,
    pointer: Option<Arc<str>>,
}
impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.digest.hash(state);
        self.pointer.hash(state);
    }
}
impl Key {
    fn new(schema: &Json, pointer: Option<&str>) -> Self {
        let schema: Arc<str> = schema.to_string().into();
        Self {
            digest: Hash::of(schema.as_bytes()),
            schema,
            pointer: pointer.map(Arc::from),
        }
    }

    fn bytes(&self) -> usize {
        self.schema
            .len()
            .saturating_add(self.pointer.as_deref().map_or(0, str::len))
    }
}

struct Grammar {
    source: Arc<str>,
    validator: Result<Validator, Error>,
}

/// An immutable declaration keeps its checked grammar, including a malformed declaration's refusal.
#[derive(Clone)]
pub struct PreparedSchema(Arc<Grammar>);

impl std::fmt::Debug for PreparedSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSchema").finish_non_exhaustive()
    }
}

impl PreparedSchema {
    /// Report a malformed declaration independently of any supplied value.
    pub fn check(&self) -> Result<(), Error> {
        self.0.validator.as_ref().map(|_| ()).map_err(Clone::clone)
    }

    /// Return the ordinary rule faults, or the saved refusal of a malformed declaration.
    pub fn validate(&self, value: &Json) -> Result<(), Error> {
        let validator = self.0.validator.as_ref().map_err(Clone::clone)?;
        validate_compiled(validator, value)
    }

    /// Evaluate the current instance under this declaration's compiled rules.
    #[must_use]
    pub fn is_valid(&self, value: &Json) -> bool {
        self.0
            .validator
            .as_ref()
            .is_ok_and(|validator| validator.is_valid(value))
    }
}

#[derive(Default)]
struct State {
    entries: HashMap<Key, PreparedSchema>,
    order: VecDeque<Key>,
    schema_bytes: usize,
    // Registered declarations keep their own strong handles even as unrelated forms age out.
    declarations: HashMap<Hash, Weak<Grammar>>,
}

impl State {
    fn pin(&mut self, digest: Hash, grammar: &PreparedSchema) {
        // Reclaim retired registrations before growing the index. Weak entries retain no schema text.
        if self.declarations.len() == self.declarations.capacity()
            && !self.declarations.contains_key(&digest)
        {
            self.declarations
                .retain(|_, entry| entry.strong_count() != 0);
        }
        self.declarations.insert(digest, Arc::downgrade(&grammar.0));
    }
}

// Cold schema validation can traverse hundreds of meta-schema frames. Both engine workloads
// passed with the suite's 32 MiB stack. Only one compiler reserves that stack at a time.
const COMPILER_STACK_BYTES: usize = 32 * 1024 * 1024;
static COMPILER: Mutex<()> = Mutex::new(());

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

    fn find(&self, key: &Key, pin: bool) -> Option<PreparedSchema> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(grammar) = state.entries.get(key).cloned() {
            state.order.retain(|old| old != key);
            state.order.push_back(key.clone());
            if pin {
                state.pin(key.digest, &grammar);
            }
            return Some(grammar);
        }
        if key.pointer.is_some() {
            return None;
        }
        state
            .declarations
            .get(&key.digest)
            .and_then(Weak::upgrade)
            .filter(|grammar| grammar.source == key.schema)
            .map(PreparedSchema)
    }

    fn compiled(
        &self,
        schema: &Json,
        pointer: Option<&str>,
        pin: bool,
    ) -> Result<PreparedSchema, Error> {
        let key = Key::new(schema, pointer);
        if let Some(ready) = self.find(&key, pin) {
            return Ok(ready);
        }
        // Cache hits and instance checks never wait on the compiler. Recheck the digest after
        // admission so two concurrent requests for a cold declaration share one compilation.
        let _compiler = COMPILER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(ready) = self.find(&key, pin) {
            return Ok(ready);
        }
        let unavailable = || Error::Schema {
            path: pointer.unwrap_or_default().to_owned(),
        };
        let validator = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("schema-compiler".into())
                .stack_size(COMPILER_STACK_BYTES)
                .spawn_scoped(scope, || compile(schema, pointer))
                .map_err(|_| unavailable())?
                .join()
                .map_err(|_| unavailable())
        })?;
        #[cfg(test)]
        self.compilations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let grammar = PreparedSchema(Arc::new(Grammar {
            source: Arc::clone(&key.schema),
            validator,
        }));
        let bytes = key.bytes();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pin {
            state.pin(key.digest, &grammar);
        }
        if self.limits.entries > 0 && bytes <= self.limits.schema_bytes {
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
            state.schema_bytes += bytes;
            state.order.push_back(key.clone());
            state.entries.insert(key, grammar.clone());
        }
        Ok(grammar)
    }

    /// Retain checked whole-document grammar for the life of a registered declaration.
    pub fn prepare(&self, schema: &Json) -> Result<PreparedSchema, Error> {
        self.compiled(schema, None, true)
    }

    /// Check declaration grammar using the same local retrieval, regex and format rules.
    pub fn check(&self, schema: &Json) -> Result<(), Error> {
        self.compiled(schema, None, false)?.check()
    }

    /// Evaluate this value afresh; cached grammar never implies an accepted value.
    pub fn validate(&self, schema: &Json, value: &Json) -> Result<(), Error> {
        self.compiled(schema, None, false)?.validate(value)
    }

    /// Test a candidate at a local rule, retaining references to its complete source document.
    #[must_use]
    pub fn allows_at(&self, schema: &Json, pointer: &str, value: &Json) -> bool {
        self.compiled(schema, Some(pointer), false)
            .is_ok_and(|validator| validator.is_valid(value))
    }
}

#[cfg(test)]
mod tests;
