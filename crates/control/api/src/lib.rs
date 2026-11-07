//! API application hosting HTTP, realtime and the machine backplane.
//!
//! Requests are bound to the contract's declarations in [`registry`]; what
//! surrounds every handler is in [`middleware`]; [`testing`] calls the
//! application in memory, for the tests of every route.

// The query depth of the deepest awaited futures passes the default limit since the fourteenth cut; ci-triage-c unnests them (future-unnesting), after which this returns to the default.
#![recursion_limit = "256"]
#![forbid(unsafe_code)]

#[cfg(feature = "test-observers")]
pub use orch_link::test_observers;

// The read-cut unit tests use the same native application and machine fixtures as route tests.
#[cfg(test)]
extern crate self as orch_api_core;
#[cfg(test)]
#[path = "../tests/places/mod.rs"]
mod places;
#[cfg(test)]
#[path = "../tests/rig/mod.rs"]
mod rig;
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod support;

pub mod archives;
pub mod attachments;
pub mod backplane;
pub mod blobs;
pub mod cache;
pub mod certificates;
pub mod component_updates;
pub mod content;
pub mod dispatch;
pub mod documents_upkeep;
pub mod harness_channels;
pub mod harness_checks;
pub mod host_ports;
pub mod hosting;
pub mod input_schedules;
pub(crate) mod key_operations;
pub mod local_ports;
pub mod machine_requests;
pub mod media;
pub mod middleware;
pub mod openapi;
pub mod port_tools;
pub mod ports_browser;
pub mod ports_runtime;
pub mod previews;
pub mod programs;
pub mod realtime;
pub mod recordings;
pub mod registry;
pub(crate) mod response_body;
pub mod routes;
mod sandbox_readiness;
pub mod server;
pub mod sign_in;
pub mod state;
pub mod syncs;
pub mod testing;
pub mod thread_access;
pub mod thread_engine;
pub mod thread_markers;
pub mod tickets;
pub mod uploads;
