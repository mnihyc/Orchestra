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

/// The actual executable and native platform being reviewed for directory execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlatformBuild {
    pub os: String,
    pub arch: String,
    pub executable: orch_types::Hash,
}

/// The local or delegated decision for an unverified native directory runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlatformConsent {
    #[default]
    Off,
    Approved {
        build: PlatformBuild,
        /// Server attribution exists only for a decision made by a machine manager through Orch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirmation: Option<Confirmation>,
    },
}

impl PlatformConsent {
    /// The admission subject. Reaffirming it does not replace an existing attachment.
    #[must_use]
    pub fn build(&self) -> Option<&PlatformBuild> {
        match self {
            Self::Off => None,
            Self::Approved { build, .. } => Some(build),
        }
    }

    #[must_use]
    pub fn accepts(&self, current: &PlatformBuild) -> bool {
        self.build() == Some(current)
    }
}
