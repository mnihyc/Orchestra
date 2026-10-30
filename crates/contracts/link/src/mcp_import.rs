//! A local connector's request to file an imported tool server in the system as its machine's owner.

use orch_api_contract::environment::McpScope;
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One import may run a thirty-second machine command; leave time for admission, copying and its reply.
pub const ANSWER_WITHIN_SECS: u64 = 75;
/// A local console cannot accumulate an unbounded collection of unanswered imports.
pub const MAX_PENDING: usize = 16;

/// Only safe origin metadata crosses the link; the definition and its secrets stay on the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpImportAsk {
    /// Correlation and replay key, also the identifier of the copy the system files.
    pub request_id: Id,
    /// A registered environment on this authenticated machine; supplies the discovered configuration's context.
    pub environment_id: Id,
    /// The harness that found it.
    pub source: String,
    /// User configuration or a directory's configuration.
    pub scope: McpScope,
    /// For a directory configuration, its relative workspace directory.
    pub directory: Option<String>,
    /// The name in that configuration.
    pub name: String,
}

/// The system's definitive result, or a refusal; absence on a broken link is not proof of failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpImportAnswer {
    /// The request answered.
    pub request_id: Id,
    /// The server filed on success.
    pub server_id: Option<Id>,
    /// A stable refusal code, absent on success.
    pub code: Option<String>,
    /// A safe sentence for the local command or console.
    pub detail: String,
}
