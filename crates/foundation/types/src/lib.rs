//! Shared primitive types without service or transport behavior.
//!
//! Everything that crosses a boundary of Orch is written with these: an
//! identifier, a moment, a content hash, a revision, and the shape in which
//! a person is asked for a value. They carry their wire form with them, so
//! that no contract restates it.

#![forbid(unsafe_code)]

pub mod fields;
pub mod hash;
pub mod id;
pub mod measurements;
pub mod media;
pub mod revision;
pub mod time;

pub use fields::{Choice, Field, FieldType};
pub use hash::Hash;
pub use id::{Id, new_id};
pub use revision::{Revision, Seq};
pub use time::{Clock, ManualClock, SystemClock, Time};

/// Any JSON value, where a contract leaves the shape to somebody else.
pub type Json = serde_json::Value;
