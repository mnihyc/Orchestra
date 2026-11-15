//! Bare capability units carry frozen work and a selected sign-in, without a connector identity.
use crate::{network::NetworkTransport, runtime::ToolRef, units::UnitRef};
use orch_types::{Id, Json, Time};

/// One frozen input file, including the selected provider's sign-in where needed.
/// File bytes never appear in diagnostics.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkFile {
    /// A relative path inside the work unit's empty scratch directory.
    pub path: String,
    /// Actual admitted bytes; never a path to a person's files.
    pub bytes: Vec<u8>,
}
impl std::fmt::Debug for WorkFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkFile")
            .field("path", &self.path)
            .field("bytes", &"[private]")
            .finish()
    }
}

/// A trusted producer prepares one work unit from its exact provider placement.
/// Only the original template's base source is used; its mounts and environment are excluded.
/// The supplied identities grant no source authority or permission to create a unit.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkUnitSpec {
    /// The durable capability work identity, also the unit's idempotency identity.
    pub work: Id,
    /// The actual owned unit serving this provider, including an environment used as its runtime.
    /// It supplies installed program builds and resource limits, never personal files.
    pub provider: UnitRef,
    /// The provider's original template, independent of a kept container filesystem.
    /// Drivers use only its base source; its mounts, environment and privileges are not inherited.
    pub template: Json,
    /// Verified installed program builds from this provider's runtime request.
    pub tools: Vec<ToolRef>,
    /// Exact provider transport facts, independent of ordinary runtime gateway allowances.
    pub endpoints: Vec<NetworkTransport>,
    /// The accepted work's lifetime, checked by the host and the unit's own watchdog.
    pub deadline: Time,
    /// Frozen material and the one needed sign-in; no paths are read from the host.
    pub files: Vec<WorkFile>,
}

impl std::fmt::Debug for WorkUnitSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkUnitSpec")
            .field("work", &self.work)
            .field("provider", &self.provider)
            .field("template", &"[private]")
            .field("tools", &self.tools)
            .field("endpoints", &self.endpoints)
            .field("deadline", &self.deadline)
            .field("files", &self.files)
            .finish()
    }
}
