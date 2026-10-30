//! Discovery, profiles and lifecycle commands for browsers inside an environment.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use orch_api_contract::native::{
    EnvironmentFactsBrowser, EnvironmentFactsProgram, EnvironmentFactsVnc,
};

/// A kept profile, named rather than exposed as a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserProfile {
    /// Its directory's name.
    pub name: String,
    /// Its measured size, where available.
    pub size_bytes: Option<i64>,
    /// When it was last used.
    pub used_at: Option<Time>,
    /// The browser known to run on it.
    pub browser: Option<Id>,
}

/// How Orch starts a browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPreset {
    /// The full browser without a window.
    Headless,
    /// A window on a display.
    Window,
}

/// Looks for installed programs again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenLook {
    /// The environment.
    pub environment_id: Id,
}

/// A browser the connector still knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenLookBrowser {
    /// Its identifier.
    pub browser_id: Id,
    /// Its local driving address.
    pub endpoint: String,
    /// Its kept profile, where it has one.
    pub profile: Option<String>,
}

/// A display the connector still knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenLookDisplay {
    /// Its identifier.
    pub display_id: Id,
    /// Its endpoint.
    pub endpoint: String,
}

/// The newly found programs and processes, without page content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenLookResult {
    /// Browser support.
    pub browser: EnvironmentFactsBrowser,
    /// Display support.
    pub vnc: EnvironmentFactsVnc,
    /// Profiles kept there.
    pub profiles: Vec<BrowserProfile>,
    /// Browsers that still run.
    pub browsers: Vec<ScreenLookBrowser>,
    /// Displays that still run.
    pub displays: Vec<ScreenLookDisplay>,
}

/// The display of a browser with a window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserOpenDisplay {
    /// The known display.
    pub display_id: Id,
    /// Its X address.
    pub x_display: String,
}

/// Opens or joins a browser on a named profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserOpen {
    /// The environment.
    pub environment_id: Id,
    /// The identifier reserved by the server.
    pub browser_id: Id,
    /// A directory name, never a path.
    pub profile: String,
    /// Make a missing profile.
    pub create: bool,
    /// With or without a window.
    pub preset: BrowserPreset,
    /// The environment's extra arguments.
    pub args: Vec<String>,
    /// Its initial window size as width x height.
    pub viewport: String,
    /// Where its first tab goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The display for its window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<BrowserOpenDisplay>,
}

/// The browser opened, or the one joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserOpenResult {
    /// The service of its shared tunnel.
    pub service: String,
    /// Its address inside the environment.
    pub endpoint: String,
    /// Its own name and version.
    pub program: String,
    /// Whether a new process was started.
    pub started: bool,
    /// The browser already running on the profile, when joined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<Id>,
}

/// Takes in a browser an agent started itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserAdopt {
    /// The environment.
    pub environment_id: Id,
    /// The identifier reserved by the server.
    pub browser_id: Id,
    /// A loopback HTTP address inside that environment.
    pub endpoint: String,
}

/// The service and profile of an announced browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserAdoptResult {
    /// The service of its tunnel.
    pub service: String,
    /// Its own name and version.
    pub program: String,
    /// Its environment profile, where found.
    pub profile: Option<String>,
}

/// Asks a browser to quit and ends only a process Orch started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserClose {
    /// The browser.
    pub browser_id: Id,
}

/// An act on a profile while no browser runs on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProfileAction {
    /// Make an empty profile.
    Create,
    /// Give it another name.
    Rename,
    /// Remove its directory and sign-ins.
    Delete,
}

/// Creates, renames or deletes one profile by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserProfileCommand {
    /// The environment.
    pub environment_id: Id,
    /// What to do.
    #[serde(rename = "do")]
    pub action: BrowserProfileAction,
    /// Its name.
    pub name: String,
    /// The new name, only with rename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// The profiles after the act.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserProfileResult {
    /// All kept profiles.
    pub profiles: Vec<BrowserProfile>,
}

/// The state reported for a browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserReportedState {
    /// The browser answers.
    Open,
    /// The browser ended.
    Closed,
}

/// A browser's state or tab count changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserState {
    /// Its identifier.
    pub browser_id: Id,
    /// Its state.
    pub state: BrowserReportedState,
    /// How it ended, when it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Its number of tabs where known.
    pub tabs: Option<i64>,
    /// Its name and version, where newly known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
}
