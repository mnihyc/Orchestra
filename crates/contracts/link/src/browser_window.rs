//! Opening the virtual display needed by a browser with a window.

use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Opens a virtual screen in an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisplayOpen {
    /// The environment.
    pub environment_id: Id,
    /// The reserved display.
    pub display_id: Id,
    /// Its pixel dimensions.
    pub geometry: String,
}

/// The new local display service and its X address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayOpenResult {
    /// Its tunnel service.
    pub service: String,
    /// Its loopback endpoint.
    pub endpoint: String,
    /// The X address for a program that opens a window.
    pub x_display: String,
}
