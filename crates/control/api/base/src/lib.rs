//! API application hosting HTTP, realtime and the machine backplane.
//!
//! Requests are bound to the contract's declarations in [`registry`]; what
//! surrounds every handler is in [`middleware`]; [`testing`] calls the
//! application in memory, for the tests of every route.
#![forbid(unsafe_code)]
#![cfg_attr(
    any(test, feature = "test-support"),
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "the native test cases fail by panicking"
    )
)]
#[cfg(feature = "test-observers")]
pub use orch_link::test_observers;
#[cfg(any(test, feature = "test-support"))]
#[path = "../tests/desk/mod.rs"]
pub mod desk;

#[cfg(any(test, feature = "test-support"))]
#[path = "../tests/native_close_support/mod.rs"]
pub mod native_close_support;

#[cfg(any(test, feature = "test-support"))]
#[path = "../tests/places/mod.rs"]
pub mod places;

#[cfg(any(test, feature = "test-support"))]
#[path = "../tests/realtime_support/mod.rs"]
pub mod realtime_support;

#[cfg(any(test, feature = "test-support"))]
#[path = "../tests/rig/mod.rs"]
pub mod rig;

#[cfg(any(test, feature = "test-support"))]
#[path = "../tests/session_ticket_support/mod.rs"]
pub mod session_ticket_support;

#[cfg(any(test, feature = "test-support"))]
#[path = "../tests/support/mod.rs"]
pub mod support;

#[cfg(any(test, feature = "test-support"))]
#[path = "../tests/thread_history_scope/mod.rs"]
pub mod thread_history_scope;

#[path = "archives.rs"]
pub mod archives;

#[path = "ask.rs"]
pub mod ask;

#[path = "attachments.rs"]
pub mod attachments;

#[path = "backplane.rs"]
pub mod backplane;

#[path = "blobs.rs"]
pub mod blobs;

#[path = "cache.rs"]
pub mod cache;

#[path = "certificates.rs"]
pub mod certificates;

#[path = "component_updates.rs"]
pub mod component_updates;

#[path = "content.rs"]
pub mod content;

#[path = "dispatch.rs"]
pub mod dispatch;

#[path = "documents_upkeep.rs"]
pub mod documents_upkeep;

#[path = "harness_channels.rs"]
pub mod harness_channels;

#[path = "harness_checks.rs"]
pub mod harness_checks;

#[path = "host_ports.rs"]
pub mod host_ports;

#[path = "hosting.rs"]
pub mod hosting;

#[path = "input_schedules/mod.rs"]
pub mod input_schedules;

#[path = "key_operations/mod.rs"]
pub mod key_operations;

#[path = "local_ports.rs"]
pub mod local_ports;

#[path = "machine_requests.rs"]
pub mod machine_requests;

#[path = "media.rs"]
pub mod media;

#[path = "middleware/mod.rs"]
pub mod middleware;

#[path = "openapi.rs"]
pub mod openapi;

#[path = "port_tools.rs"]
pub mod port_tools;

#[path = "ports_browser.rs"]
pub mod ports_browser;

#[path = "ports_runtime.rs"]
pub mod ports_runtime;

#[path = "previews.rs"]
pub mod previews;

#[path = "programs.rs"]
pub mod programs;

#[path = "realtime/mod.rs"]
pub mod realtime;

#[path = "recordings.rs"]
pub mod recordings;

#[path = "registry.rs"]
pub mod registry;

#[path = "response_body.rs"]
pub mod response_body;

#[path = "routes/mod.rs"]
pub mod routes;

#[path = "sandbox_readiness.rs"]
pub mod sandbox_readiness;

#[path = "server.rs"]
pub mod server;

#[path = "sign_in.rs"]
pub mod sign_in;

#[path = "state.rs"]
pub mod state;

#[path = "syncs.rs"]
pub mod syncs;

#[path = "testing.rs"]
pub mod testing;

#[path = "thread_access.rs"]
pub mod thread_access;

#[path = "thread_engine.rs"]
pub mod thread_engine;

#[path = "thread_markers.rs"]
pub mod thread_markers;

#[path = "thread_read_tools.rs"]
pub mod thread_read_tools;

#[path = "tickets.rs"]
pub mod tickets;

#[path = "upload_inventory.rs"]
pub mod upload_inventory;

#[path = "uploads.rs"]
pub mod uploads;

extern crate self as orch_api_base;
#[cfg(any(test, feature = "test-support"))]
extern crate self as orch_api_core;
pub mod route_assembly;
