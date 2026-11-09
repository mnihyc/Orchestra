//! API application hosting HTTP, realtime and the machine backplane.
//!
//! Requests are bound to the contract's declarations in [`registry`]; what
//! surrounds every handler is in [`middleware`]; [`testing`] calls the
//! application in memory, for the tests of every route.

#![forbid(unsafe_code)]

#[cfg(feature = "test-observers")]
pub use orch_link::test_observers;

// The read-cut unit tests use the same native application and machine fixtures as route tests.
#[cfg(test)]
extern crate self as orch_api_core;
#[cfg(test)]
#[path = "../tests/desk/mod.rs"]
mod desk;
#[cfg(test)]
#[path = "../tests/native_close_support/mod.rs"]
mod native_close_support;
#[cfg(test)]
#[path = "../tests/places/mod.rs"]
mod places;
#[cfg(test)]
#[path = "../tests/realtime_support/mod.rs"]
mod realtime_support;
#[cfg(test)]
#[path = "../tests/rig/mod.rs"]
mod rig;
#[cfg(test)]
#[path = "../tests/session_ticket_support/mod.rs"]
mod session_ticket_support;
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod support;
#[cfg(test)]
#[path = "../tests/thread_history_scope/mod.rs"]
mod thread_history_scope;

pub mod archives;
pub(crate) mod ask;
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
pub mod thread_read_tools;
pub mod tickets;
pub(crate) mod upload_inventory;
pub mod uploads;
