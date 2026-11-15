//! Frozen physical placement of one source-owned native program work.
use orch_driver_contract::{network::NetworkTransport, runtime::ToolRef};
use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Frozen placement supplied by the actual controller producer, not by a program or a tunnel opener.
/// This record is never an acting credential or a source Read grant.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    /// The durable capability work.
    pub work: Id,
    /// Its ordinary private helper.
    pub helper: Id,
    /// The actual trusted provider connector driving the program from outside the unit.
    pub peer: Id,
    /// This node's owned physical unit serving the selected provider.
    pub parent: Id,
    /// Original template source, excluding a kept personal filesystem.
    pub template: Json,
    /// Exact installed builds from that provider placement.
    pub tools: Vec<ToolRef>,
    /// Exact provider transports, with no ordinary Orch exception.
    pub endpoints: Vec<NetworkTransport>,
    /// The accepted work's own lifetime.
    pub deadline: Time,
}
impl std::fmt::Debug for Admission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramAdmission")
            .field("work", &self.work)
            .field("helper", &self.helper)
            .field("peer", &self.peer)
            .field("parent", &self.parent)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}
