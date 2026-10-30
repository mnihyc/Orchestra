//! The identity of a compiled Orch component.
//!
//! Each executable constructs this from the values emitted by its own build
//! script. No value depends on the environment of the running program.

#![forbid(unsafe_code)]

pub mod release;

use serde::Serialize;
use std::fmt;

/// A component's release and build counter, for diagnostics and handshakes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BuildInfo {
    /// The separately shipped component, such as `cli` or `connector`.
    pub component: &'static str,
    /// Canonical semantic release version, without a leading `v`.
    pub version: &'static str,
    /// The release tag, when this build was made for a tag.
    pub tag: Option<&'static str>,
    /// The positive component build counter, represented as decimal text.
    /// Text preserves the entire counter in JavaScript clients.
    pub build_number: &'static str,
}

impl BuildInfo {
    /// Constructs the identity from validated build-script output.
    #[must_use]
    pub const fn new(
        component: &'static str,
        version: &'static str,
        tag: &'static str,
        build_number: &'static str,
    ) -> Self {
        Self {
            component,
            version,
            tag: if tag.is_empty() { None } else { Some(tag) },
            build_number,
        }
    }
}

impl fmt::Display for BuildInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} (build {})",
            self.component, self.version, self.build_number
        )?;
        if let Some(tag) = self.tag {
            write!(f, " [{tag}]")?;
        }
        Ok(())
    }
}
