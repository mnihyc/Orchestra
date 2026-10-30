//! Settings catalogue and resolution data, without evaluation.
//!
//! Whatever people decide per scope is a setting: a key of the [registry],
//! with a value at some of the scopes it can be set at. This crate holds the
//! registry as data, the rule by which the values of several scopes
//! [merge] into the one that holds, locks among them, and the shape in
//! which a resolved value is shown with its [provenance].
//!
//! It reads no storage: whoever resolves a setting reads the rows on the
//! path, hands them over as layers, and gets back the value with where it
//! comes from.

#![forbid(unsafe_code)]

mod examples;
pub mod merge;
pub mod provenance;
pub mod registry;
pub mod unit;

pub use merge::{Layer, resolve};
pub use provenance::{Resolved, ResolvedSetting, Source, SourceScope};
pub use registry::{Class, Merge, Scope, SettingDef, keys};
