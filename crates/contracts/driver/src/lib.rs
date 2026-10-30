//! EnvironmentDriver contract for provider and environment runtimes.
//!
//! A driver turns a template into a running unit: a container, a microVM, a process group. `orch-node`
//! treats every driver, built in or external, through the [`driver::EnvironmentDriver`] trait and the
//! [`descriptor::DriverDescriptor`] each declares, and nothing outside a driver knows a path, an image, a
//! user or a network.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`driver`] | The trait and its errors |
//! | [`descriptor`] | The declaration: pools, confinement, capabilities, mounts, sources, imports |
//! | [`runtime`] | The request to make a unit, the same for every driver, and what every driver must deliver to the connector |
//! | [`sources`] | Where a unit's file system comes from |
//! | [`imports`] | What a spec can be proposed from |
//! | [`units`] | Unit identity, state, and reconciliation |
//! | [`volumes`] | Persistent storage on a host's named storages |
//! | [`mounts`] | How mounts are attached |
//! | [`capabilities`] | Per-driver capability patches |
//! | [`isolation`] | Runtimes as declared classes: boundary, what holds differently under each, and its limits |
//! | [`network`] | Egress modes, the network policy of a runtime, and what is in force for a unit |

#![forbid(unsafe_code)]

pub mod capabilities;
pub mod descriptor;
pub mod driver;
pub mod host;
pub mod imports;
pub mod isolation;
pub mod kept;
pub mod mounts;
pub mod network;
pub mod process_limits;
pub mod resources;
pub mod runtime;
pub mod sources;
pub mod units;
pub mod volumes;
