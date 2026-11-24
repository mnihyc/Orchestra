//! Safe configuration observations captured when startup values are decided.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where a loaded value was supplied, independently of whether it equals its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The compiled value.
    Compiled,
    /// An explicit member of the configuration file.
    File,
    /// An environment override actually applied by the loader.
    Environment,
}

/// Provenance without any supplied values, environment names or file contents.
pub type Origins = BTreeMap<String, Origin>;

/// One allowlisted nonsecret value, frozen before the configuration leaves its reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupValue {
    /// Its public configuration name.
    pub key: &'static str,
    /// The exact native unsigned millisecond value; zero has the key's declared meaning.
    pub milliseconds: u64,
    /// Uncaptured provenance stays unreported.
    pub origin: Option<Origin>,
}

/// One process's loaded public values, never a request-time reread of the file or environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupConfiguration {
    /// Opaque observation identity, not a credential or a displayed process identifier.
    pub revision: String,
    /// When the reader captured the configuration.
    pub observed_at: SystemTime,
    /// The explicit safe allowlist, excluding URLs, credentials and paths.
    pub values: Vec<StartupValue>,
}

impl StartupConfiguration {
    pub(crate) fn new(values: Vec<StartupValue>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let observed_at = SystemTime::now();
        let stamp = observed_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            revision: format!(
                "{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            observed_at,
            values,
        }
    }
}
