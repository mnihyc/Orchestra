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

/// One program invocation inside an already prepared work unit. No host environment is inherited.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkProcessSpec {
    /// The executable inside the unit's base image or verified program mounts.
    pub program: String,
    /// Actual arguments, kept as separate words.
    pub args: Vec<String>,
    /// Program options and the selected provider's sign-in variables only.
    pub env: std::collections::BTreeMap<String, String>,
    /// A relative scratch directory, absent for the scratch root.
    pub cwd: Option<String>,
}
impl std::fmt::Debug for WorkProcessSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkProcessSpec")
            .field("program", &self.program)
            .field("args", &"[private]")
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// One stream of the program, without merging diagnostics into its protocol output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkOutputStream {
    Stdout,
    Stderr,
}

/// Actual bytes from the isolated program. Callers apply their ordinary native redaction and parser.
pub struct WorkOutput {
    /// Which stream produced the bytes.
    pub stream: WorkOutputStream,
    /// The exact bytes; never included in diagnostics of this transport.
    pub bytes: Vec<u8>,
}

/// The keeping node owns this handle; the program receives only its standard streams.
#[async_trait::async_trait]
pub trait WorkProcess: Send + Sync {
    /// The driver's actual process identity, for the existing node journal, never source authority.
    fn reference(&self) -> &str;
    /// Start once, after the keeping node retained this process identity under the actual work.
    /// An uncertain first acknowledgement never starts another invocation.
    async fn start(&self) -> Result<(), crate::driver::DriverError>;
    /// Send bytes to this exact program.
    async fn write(&self, bytes: &[u8]) -> Result<(), crate::driver::DriverError>;
    /// Close its input without ending its output.
    async fn close_input(&self) -> Result<(), crate::driver::DriverError>;
    /// The next output chunk. Only one receiver reads it; cancellation consumes no bytes.
    async fn next_output(&self) -> Result<Option<WorkOutput>, crate::driver::DriverError>;
    /// Its actual exit code, absent when the keeping driver cannot report it.
    async fn wait(&self) -> Result<Option<i64>, crate::driver::DriverError>;
    /// End this owned work unit, including its program, without touching its provider.
    async fn terminate(&self) -> Result<(), crate::driver::DriverError>;
}
