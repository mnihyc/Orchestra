//! The operator's lifecycle hooks: executables the node runs at four moments of every unit's life, told what the unit is.
//!
//! Unit hosts and directory connectors use the same runner and machine-policy hooks.

pub use orch_connector_core::lifecycle::*;
