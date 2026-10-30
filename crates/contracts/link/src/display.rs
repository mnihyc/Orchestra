//! Adoption, closure and state reports for displays served inside an environment.

pub use crate::browser_window::{DisplayOpen, DisplayOpenResult};
use crate::commands::Secret;
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Verifies and registers an agent's local VNC server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisplayAdopt {
    /// Its environment.
    pub environment_id: Id,
    /// Its reserved identifier.
    pub display_id: Id,
    /// A loopback port or a local socket path.
    pub endpoint: String,
    /// A stored reference, replaced by a machine-sealed value on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Secret>,
}

/// Safe facts learned from a VNC handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayAdoptResult {
    /// Its tunnel service.
    pub service: String,
    /// Its server's name, where known.
    pub name: Option<String>,
    /// Its dimensions, where known.
    pub geometry: Option<String>,
}

/// Stops an owned display or forgets an announced one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisplayClose {
    /// The display.
    pub display_id: Id,
}

/// A connector's lifecycle report, without images, input or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayState {
    /// The known display.
    pub display_id: Id,
    /// Whether it still answers.
    pub state: crate::browser::BrowserReportedState,
    /// Why it ended, where it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Its dimensions, where reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<String>,
}
