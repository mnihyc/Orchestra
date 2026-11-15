//! Current session-tool admission over the authenticated control connection.
use crate::tunnels::TicketClaims;
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A few control round trips by default, stated by the installation's ordinary setting.
pub const DEFAULT_ADMISSION_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolSessionControl {
    pub available: bool,
    pub admission_timeout_ms: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolSessionQuery {
    /// One pending Open or Resume, never reused for a later request.
    pub n: Id,
    pub claims: Box<TicketClaims>,
    /// The API verifies its own signature again before answering.
    pub ticket: String,
}

impl std::fmt::Debug for ToolSessionQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolSessionQuery")
            .field("n", &self.n)
            .field("claims", &self.claims)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolSessionReply {
    pub n: Id,
    pub allowed: bool,
    /// The native facts checked by the API for this exact pending question.
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolSessionMember {
    pub member: String,
    pub nonce: Id,
}

/// A scope update closes old routes and pending questions; it never opens a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolSessionUpdate {
    pub opening_epoch: i64,
    pub thread_id: Id,
    pub segment_id: Id,
    pub opening_id: Option<Id>,
    pub revision: i64,
    pub live: bool,
    pub members: Vec<ToolSessionMember>,
}
