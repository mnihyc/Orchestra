//! Payload types that belong to other contracts, carried as JSON until those contracts define them.
//!
//! The machine link carries the same concrete declarations and observations as the
//! adapter, model, driver and API contracts. Native observations live in the API
//! contract below those users, so these re-exports introduce no dependency cycle.
//! Item bodies are untagged here because the event carries their kind beside them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use orch_agent_contract::descriptor::AgentDescriptor;
pub use orch_agent_contract::discovery::{McpServerSpec, ModelSuggestion, NativeSession};
pub use orch_agent_contract::events::{HarnessRequest, ItemStatus};
pub use orch_agent_contract::facts::{AgentProfile, ProviderFacts};
pub use orch_agent_contract::history::HistoryReport;
pub use orch_agent_contract::login::LoginState;
pub use orch_agent_contract::session::ApprovalDecision;
pub use orch_api_contract::processes::{ProcessInfo, ResourceSample};
pub use orch_api_contract::resources::Rate;
pub use orch_api_contract::threads::TokenUsage;
pub use orch_api_contract::workspace::{
    ArchiveListing, DirEntry, FileMatch, FileStat, GitStatus, JobProgress,
};
pub use orch_model_contract::endpoint::EndpointOptions;
/// The driver recipe before the node opens its command-bound enrollment credential.
pub type RuntimeSpec = orch_driver_contract::runtime::RuntimeSpec<crate::commands::Secret>;
pub use orch_driver_contract::descriptor::DriverDescriptor;
pub use orch_driver_contract::units::UnitRef;

/// A complete adapter item body; its kind is the adjacent member of item.started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[expect(
    clippy::large_enum_variant,
    reason = "the wire body is constructed once, and tool output already has a bounded inline tail"
)]
pub enum ItemPayload {
    /// The agent's reasoning.
    Reasoning(orch_agent_contract::events::ReasoningPayload),
    /// Its plan.
    Plan(orch_agent_contract::events::PlanPayload),
    /// A native or Orch tool call.
    ToolCall(orch_agent_contract::events::ToolCallPayload),
    /// An informational line of native session history.
    Marker(orch_agent_contract::events::MarkerPayload),
    /// A failure in the harness.
    Error(orch_agent_contract::events::ErrorPayload),
    /// What the agent said.
    AgentMessage(orch_agent_contract::events::AgentMessagePayload),
}

impl ItemPayload {
    /// The adapter kind that owns these members, without guessing from a partial update.
    #[must_use]
    pub const fn kind(&self) -> orch_agent_contract::events::ItemKind {
        use orch_agent_contract::events::ItemKind;
        match self {
            Self::Reasoning(_) => ItemKind::Reasoning,
            Self::Plan(_) => ItemKind::Plan,
            Self::ToolCall(_) => ItemKind::ToolCall,
            Self::Marker(_) => ItemKind::Marker,
            Self::Error(_) => ItemKind::Error,
            Self::AgentMessage(_) => ItemKind::AgentMessage,
        }
    }
}
