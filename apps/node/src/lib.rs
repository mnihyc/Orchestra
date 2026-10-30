//! The parts of the node program, shared by the executable and by the tests that run it against a real daemon.
//!
//! The executable is `src/main.rs`: it reads the command line ([`arguments`]) and does what it says. What is concrete
//! about the node is chosen in [`composition`] and [`docker`] and nowhere else: the library below the program (`orch-node-core`)
//! knows no driver, and the Docker driver knows nothing of the node. Keeping these two here, in a library, is
//! what lets a test make the very same driver the program makes.

#![deny(unsafe_code)]

pub mod arguments;
pub mod composition;
pub mod conformance;
pub mod conformance_unit;
pub mod docker;
pub mod file_state_volume;
#[cfg(all(target_os = "linux", feature = "kubernetes"))]
pub mod kubernetes;
pub mod runtime_helper;
pub mod uninstall;
pub mod update;
pub mod volume_archive;
