//! Host driver composition and managed runtime lifecycle.
//!
//! A host that runs Orch's units has one program on it, the node. It enrolls with a token a person made for the
//! host, holds one outbound session to the gateway, and does what the system asks of the host: make an image
//! available, make a volume on a named storage, put a tool in its cache, make a unit for a runtime and start,
//! stop and remove it. It tells the system what the host has, what it uses and what became of every unit. It
//! never reaches into a unit and never removes a volume with a unit.
//!
//! The node is a connector that plays the role `node`, and this crate is the part of it that is no program:
//!
//! * Who the host is and how it holds its session: [`enrollment`]; the role the connector is given: [`role`].
//! * What it is configured with, and what it records: [`settings`], [`ledger`].
//! * The drivers that make the units, and what the node needs of them beyond the contract: [`drivers`].
//! * What the commands act on: [`runtimes`] (the units), [`volumes`] and [`storages`] (where their data lives),
//!   [`images`] (what they are made from), [`tools`] (the programs they are given), [`tool_checks`] (what a build
//!   of an agent program offers) and [`managed`] (the connector in each unit and the token it enrolls with).
//! * What the node tells: [`facts`] (the host), [`reconciliation`] (the units).
//! * What the host's operator has run around every unit's life: [`hooks`].
//!
//! The drivers themselves are not here: a program composes them through [`drivers::Registrations`], so that
//! this crate knows no implementation.

#![forbid(unsafe_code)]

pub mod drivers;
pub mod enrollment;
pub mod facts;
pub mod hooks;
pub mod host_ports;
pub mod host_publish;
pub mod images;
pub mod ledger;
pub mod machine_policy;
pub mod managed;
pub mod reconciliation;
pub mod resources;
pub mod role;
pub mod runtimes;
pub mod settings;
pub mod skill_fetch;
pub mod storages;
pub mod tool_checks;
pub mod tools;
pub mod volumes;
mod work;
