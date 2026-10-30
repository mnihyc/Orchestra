//! External extension JSONL contracts.
//!
//! Every extension point of Orch has built-in implementations and may have
//! external ones: programs that speak the same contract as lines of JSON.
//! This crate holds what is common to all of them: how an implementation is
//! registered ([`registration`]), the lines that pass between a host and it
//! ([`messages`]), how they agree on versions and on the methods they share
//! ([`negotiation`]), how long a plugin may take and whether it is alive
//! ([`lifecycle`]), who an implementation is and what is its own
//! ([`descriptor`]), and the two small shapes every extension contract
//! uses: a value that is never printed ([`secret`]) and a reference to a
//! person or a place ([`refs`]).

#![forbid(unsafe_code)]

pub mod descriptor;
pub mod lifecycle;
pub mod messages;
pub mod negotiation;
pub mod refs;
pub mod registration;
pub mod secret;

pub use descriptor::{Extensions, PluginIdentity, PluginState, PluginStatus};
pub use registration::{PluginKind, PluginRegistration};
pub use secret::Secret;
