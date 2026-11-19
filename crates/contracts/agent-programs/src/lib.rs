//! Agent program manifests and the registry of the built-in programs.
//!
//! An agent program is an extension: a protocol adapter in Orch, a manifest, and optionally a companion plugin
//! that runs inside the program. The manifest ([`manifest::ProgramManifest`]) is static data, one for each
//! program, and everything that used to be a list of Codex and Claude Code in shared code reads it from the
//! registry ([`registry::Programs`]) instead.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`localized`] | Words by language, with the English fallback |
//! | [`catalogue`] | Titles of what a check of a build reports |
//! | [`manifest`] | The manifest and its parts: launch, placement, checks, files, privacy |
//! | [`channels`] | Where builds are published and how each is verified |
//! | [`executors`] | The manifest of an executor, with its tool sets |
//! | [`registry`] | The built-in manifests, and lookups |

#![forbid(unsafe_code)]

pub mod catalogue;
pub mod channels;
pub mod executors;
pub mod localized;
pub mod manifest;
pub mod registry;
pub mod versions;

pub use catalogue::Catalogue;
pub use channels::{Arch, Channel};
pub use executors::{ExecutorManifest, ToolSetManifest};
pub use localized::Localized;
pub use manifest::ProgramManifest;
pub use registry::{Programs, intern, shipped, shipped_executor};
