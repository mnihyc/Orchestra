//! Host-process implementation of the runtime driver contract.
//!
//! The node starts connectors under a selected local account, keeps their directories and identities,
//! and reports actual systemd-scope support without claiming container confinement.

#![forbid(unsafe_code)]

pub mod descriptor;
pub mod identity;
pub mod limits;
pub mod mounts;
pub mod stats;
pub mod units;
pub mod volumes;

pub use units::{HostConfig, HostDriver, HostStorage};
