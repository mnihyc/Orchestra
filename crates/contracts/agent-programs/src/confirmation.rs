//! The recorded manager and moment of an execution decision, separate from its factual subject.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Attribution supplied by the server after the actual manager authorizes a decision.
/// It grants no permission and never identifies a running native process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Confirmation {
    /// The actual manager, never inferred from the machine's local configuration.
    pub by: Id,
    /// When that manager made this decision.
    pub at: Time,
}
