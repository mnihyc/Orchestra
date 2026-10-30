//! Machine link and backplane schemas; commands and events shared by both ends.
//!
//! Every machine of a system, a connector in a runtime or on a person's
//! computer and the node of a host, holds one session to a gateway. The
//! session carries events upward, commands downward and tunnels between
//! machines; content of any size moves over HTTPS beside it. This
//! package is what the two ends of that link, and the gateway and the
//! system between them, agree on: every message with the exact form it
//! has on the wire, and the rules that need no socket to be stated.
//!
//! Nothing here opens a connection, reads a clock or keeps a file. The
//! arithmetic of a stream, of a channel and of an upload is given as
//! plain values that are told what happened and say what to do, so that
//! each end is built on the same rules and can be tested without the
//! other.

#![forbid(unsafe_code)]

pub mod background;
pub mod backplane;
pub mod browser;
pub mod browser_window;
pub mod commands;
pub mod content;
pub mod continuations;
pub mod display;
pub mod enrollment;
pub mod enrollment_choices;
pub mod events;
pub mod foreign;
pub mod frames;
pub mod local_ports;
pub mod mcp_import;
pub mod policy;
pub mod ports;
pub mod session;
pub mod source;
pub mod streams;
pub mod tunnels;
pub mod version;
