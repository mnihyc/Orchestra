//! Model-backed title, description and tag generation selected by configuration.
//!
//! It frames supplied activity as material, calls the selected API provider through a host port,
//! and returns constrained text or tags with the actual call's usage. Scheduling and permission
//! checks belong to the host; malformed text never turns the transcript into a summary.

#![forbid(unsafe_code)]

pub mod completion;
pub mod descriptor;
pub mod prompts;

pub use completion::{Complete, Model};
