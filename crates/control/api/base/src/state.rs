//! Application composition and shared service handles.
//!
//! One [`AppState`] is built at start and shared by every request. It
//! holds handles, not state of its own: what has to survive a restart is in
//! the database.

use crate::backplane::Link;

use crate::blobs::Blobs;

use crate::cache::Cache;

use crate::realtime::Realtime;

use orch_build_info::BuildInfo;

use orch_config::ServerConfig;

use orch_db::Db;

use orch_engine::Engine;

use orch_types::Clock;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
/// What every request can reach.
#[derive(Debug)]
pub struct AppState {
    /// The optional metrics listener's observed bind failure, shared only by this application.
    pub metrics_listener_failed: Arc<AtomicBool>,
    /// The database.
    pub db: Db,
    /// The deployment's configuration.
    pub config: Arc<ServerConfig>,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// The machines and the gateways: commands, presence, certificates,
    /// tickets and terminals.
    pub link: Link,
    /// The thread engine: inputs, turns, approvals and what a thread shows.
    pub engine: Engine,
    /// The browser's socket: what its connections follow and are sent.
    pub realtime: Realtime,
    /// What this program is.
    pub build: BuildInfo,
    /// The store of content, with the places where uploads wait.
    pub blobs: Blobs,
    /// What is remembered in memory about environments: the lists of their trees, where each was
    /// attached, and their measurements.
    pub cache: Cache,
    /// The nodes' transient host-port proposals and in-flight reads, isolated to this application.
    pub host_ports: crate::host_ports::Proposals,
    /// The built-in sources of a key's facts the program registered: each vendor's reader by name, its
    /// declaration and the addresses it is the default for. The API knows no vendor but through it.
    pub builtin_sources: Arc<orch_provider_contract::builtins::BuiltinSources>,
}

/// Observations stay with the same application that opens the listener.
impl AppState {
    /// Opens the optional listener and records its result without changing how a bind fails.
    pub async fn bind_metrics(&self, address: &str) -> std::io::Result<tokio::net::TcpListener> {
        let listener = tokio::net::TcpListener::bind(address).await;
        self.metrics_listener_failed
            .store(listener.is_err(), Ordering::Relaxed);
        listener
    }
}

/// The shared state, as handlers hold it.
pub type App = Arc<AppState>;
