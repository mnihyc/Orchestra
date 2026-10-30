//! Environment-side files, tools, processes and interactive services.
//!
//! This is the environment role of a connector: what carries out the commands that are about one
//! environment, on the machine or in the unit where the environment is. [`environment::EnvHost`]
//! holds the environments that are attached and has one function for each command; the other
//! modules are what those functions are made of.
//!
//! - **Files**: [`paths`] and [`ignore`] decide what a path may name, [`files`] lists, reads,
//!   writes, renames, copies and removes, [`search`] and [`archive`] look inside, and [`walk`],
//!   [`index`], [`manifest`], [`snapshot`] and [`materialize`] keep and move whole trees.
//! - **Running things**: [`processes`] finds and signals the processes of the environment,
//!   [`terminal`] keeps shells that outlive their screens, [`executors`] starts and stops the
//!   executors the environment offers, [`mcp`] keeps what is known of the tool servers and
//!   [`mcp_host`] runs them, [`git`] reads a repository, [`resources`] measures what is used, and
//!   [`watch`] and [`mounts`] follow the file system.
//! - **Under it all**: [`platform`] holds everything that differs between operating systems,
//!   [`blobs`] the two small traits through which content is moved, and [`error`] the failures
//!   and the code each one is answered with.
//!
//! The package never fetches or sends file content itself. Tool servers listen on loopback;
//! [`ports_sockets`] connects only to the currently owned local program selected by [`ports`].
//! It contains no `unsafe` code.

#![forbid(unsafe_code)]

/// Whether this build's environment runtime has the native containment and process proof required for release.
/// Linux is currently declared; the macOS and Windows implementations await their actual target runners.
pub const RUNTIME_VERIFIED: bool = cfg!(target_os = "linux");

pub mod archive;
pub mod blobs;
pub mod browser_cdp;
pub mod browser_discovery;
pub mod browser_screen;
pub mod browser_window;
pub mod browsers;
mod data_transfer;
pub mod directories;
mod directory_pages;
pub mod displays;
pub mod environment;
pub mod error;
pub mod exact_materialize;
pub mod executors;
pub mod file_states;
pub mod files;
pub mod git;
pub mod git_actions;
pub mod git_copy;
pub mod git_publication;
pub mod git_worktrees;
pub mod ignore;
pub mod index;
pub mod instruction_root;
pub mod manifest;
pub mod materialize;
pub mod mcp;
pub mod mcp_host;
pub mod measure;
pub mod media;
pub mod mounts;
pub mod paths;
pub mod platform;
pub mod ports;
pub mod ports_sockets;
pub mod process_limits;
pub mod processes;
pub mod resources;
pub mod restore;
pub mod search;
pub mod snapshot;
pub mod state_store;
pub mod terminal;
pub mod walk;
pub mod watch;
