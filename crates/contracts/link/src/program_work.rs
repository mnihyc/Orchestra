//! Frozen physical placement of one source-owned native program work.
use orch_driver_contract::network::NetworkTransport;
use orch_types::{Id, Time};
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

/// Trusted-connector control of one work's isolated session; never passed to the native program.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub work: Id,
    pub helper: Id,
    pub node: Id,
    pub ticket: crate::commands::TicketSlot,
}
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramSession")
            .field("work", &self.work)
            .field("helper", &self.helper)
            .field("node", &self.node)
            .finish_non_exhaustive()
    }
}

/// A frozen file's framing; its private bytes follow outside JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub path: String,
    pub bytes: u64,
}

/// One control request on an already authenticated, work-bound program tunnel.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Prepare {
        files: Vec<File>,
    },
    Start {
        invocation: Id,
        process: orch_driver_contract::work::WorkProcessSpec,
    },
    Write {
        invocation: Id,
        offset: u64,
        bytes: u64,
    },
    CloseInput {
        invocation: Id,
    },
    Output {
        invocation: Id,
        offset: u64,
    },
    Wait {
        invocation: Id,
    },
    ReadFile {
        path: String,
        max_bytes: u64,
    },
    Retire,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let op = match self {
            Self::Prepare { .. } => "prepare",
            Self::Start { .. } => "start",
            Self::Write { .. } => "write",
            Self::CloseInput { .. } => "close_input",
            Self::Output { .. } => "output",
            Self::Wait { .. } => "wait",
            Self::ReadFile { .. } => "read_file",
            Self::Retire => "retire",
        };
        f.debug_struct("ProgramRequest")
            .field("op", &op)
            .finish_non_exhaustive()
    }
}
impl Request {
    /// Body size comes from the admitted request; no private bytes occur in its JSON header.
    pub fn input_bytes(&self) -> Option<u64> {
        match self {
            Self::Prepare { files } => files
                .iter()
                .try_fold(0_u64, |total, file| total.checked_add(file.bytes)),
            Self::Write { bytes, .. } => Some(*bytes),
            _ => Some(0),
        }
    }
}

/// The factual answer; native bytes follow the header and are never diagnostic text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Ready,
    Started {
        reference: String,
    },
    Written {
        next: u64,
    },
    Output {
        offset: u64,
        stream: orch_driver_contract::work::WorkOutputStream,
        bytes: u64,
    },
    End {
        offset: u64,
    },
    Exited {
        code: Option<i64>,
    },
    File {
        bytes: u64,
    },
    Failed {
        code: String,
        detail: String,
        retryable: bool,
    },
}
impl Reply {
    pub fn output_bytes(&self) -> u64 {
        match self {
            Self::Output { bytes, .. } | Self::File { bytes } => *bytes,
            _ => 0,
        }
    }
}
