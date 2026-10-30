//! Native session command observations and installed browser facts shared by adapter and link payloads.

use crate::routes::Access::Permission;
use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::background::{BackgroundEndState, BackgroundPlace, BackgroundState, CommandCleanup};

/// A preview fits ordinary shell commands while bounding each roster reply.
pub const COMMAND_PREVIEW_BYTES: usize = 4_096;
/// Four ordinary runtimes fit in a bounded snapshot; larger lists stay explicitly partial.
pub const SNAPSHOT_COMMANDS: usize = 2_000;
/// Half a link frame leaves room for its envelope and identity metadata.
pub const PAGE_BYTES: usize = 128 * 1_024;

/// One native command, with references that only the controller can resolve to recorded items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NativeBackgroundCommand {
    /// An opaque session-owned command identity, never an arbitrary operating-system process id.
    pub id: String,
    /// The harness item that started it, when reported.
    pub native_item_ref: Option<String>,
    /// Its recorded turn, when the adapter can correlate it.
    pub turn_id: Option<Id>,
    /// Its bounded preview, when the harness reports one.
    pub command: Option<String>,
    /// Whether the native preview was shortened.
    pub command_truncated: bool,
    /// Its directory, when reported.
    pub cwd: Option<String>,
    /// Where it executes.
    pub place: BackgroundPlace,
    /// Its native start time, never an inferred observation time.
    pub since: Option<Time>,
    /// Its CPU use, when reported.
    pub cpu_percent: Option<f64>,
    /// Its memory use, when reported.
    pub memory_bytes: Option<i64>,
}

/// Current facts of one existing session; an unavailable session is never a known empty list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NativeBackground {
    /// Both native listing and exact termination are supported by this build.
    pub supported: bool,
    /// When the native list was read, when known.
    pub as_of: Option<Time>,
    /// Its current availability.
    pub state: BackgroundState,
    /// A safe explanation of a partial or unavailable list.
    pub reason: Option<String>,
    /// The bounded observed commands.
    pub items: Vec<NativeBackgroundCommand>,
    /// The next native page, when provided.
    pub next_cursor: Option<String>,
    /// The full count, only when the list is complete.
    pub total: Option<i64>,
}

impl NativeBackground {
    /// An honest unsupported observation without a guessed zero or timestamp.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            supported: false,
            as_of: None,
            state: BackgroundState::Unavailable,
            reason: Some(reason.into()),
            items: Vec::new(),
            next_cursor: None,
            total: None,
        }
    }
}

/// A native end observation contains no live roster or private process information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeBackgroundEnd {
    /// The exact command the caller named.
    pub id: String,
    /// The factual result.
    pub state: BackgroundEndState,
    /// A safe explanation, where needed.
    pub reason: Option<String>,
}

/// A program found in the environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFactsProgram {
    /// Its name.
    pub name: String,
    /// Its executable path.
    pub path: String,
    /// Its version where known.
    pub version: Option<String>,
}

/// What the environment has to run a browser.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFactsBrowser {
    /// Whether a private virtual screen can be opened, independently of the machine's own desktop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_window: Option<bool>,
    /// Programs in the order Orch starts them.
    pub programs: Vec<EnvironmentFactsProgram>,
    /// Whether a window can be opened.
    pub window: bool,
    /// The command line's version, or null.
    pub playwright_cli: Option<String>,
    /// The result of the sandbox probe, or null when nothing was found.
    pub sandbox: Option<bool>,
}

/// What the environment has to serve a display.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFactsVnc {
    /// Its display servers.
    pub servers: Vec<EnvironmentFactsProgram>,
    /// Its window manager, or null.
    pub window_manager: Option<String>,
}

/// The native artifact's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeActionKind {
    /// A native session command.
    Command,
    /// A loaded skill.
    Skill,
}

/// Where the harness reports the effective artifact comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeOriginKind {
    /// Shipped by the harness.
    Builtin,
    /// The harness user's configuration.
    User,
    /// This project's configuration.
    Project,
    /// A loaded plugin.
    Plugin,
    /// The harness did not report provenance.
    Unknown,
}

/// Safe provenance, with no private provider-home path or skill text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeOrigin {
    /// Reported provenance, never guessed from a name.
    pub kind: NativeOriginKind,
    /// A public source name, where reported.
    pub name: Option<String>,
    /// A safe workspace-relative path, where reported.
    pub path: Option<String>,
}

/// The authority the selected action requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum NativePermission {
    /// Ordinary input authority.
    #[serde(rename = "thread.interact")]
    Interact,
    /// Context-changing command authority.
    #[serde(rename = "thread.manage")]
    Manage,
}

/// A declared native delivery path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeDelivery {
    /// A new ordinary turn.
    Queue,
    /// Verified steering of an existing turn.
    Steer,
    /// Stops the current turn before invoking.
    Interrupt,
}

/// An effective winner that the actual native invocation can select.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeAction {
    /// Stable opaque selector within this loaded mapping.
    pub id: String,
    /// The harness's name.
    pub name: String,
    /// A person's display title.
    pub title: String,
    /// Command or skill.
    pub kind: NativeActionKind,
    /// Public help, without artifact contents.
    pub description: String,
    /// Arguments the adapter accepts.
    pub arguments_schema: Json,
    /// Native free-text hint, where present.
    pub argument_hint: Option<String>,
    /// Actual safe provenance.
    pub origin: NativeOrigin,
    /// Authority checked at acceptance and delivery.
    pub permission: NativePermission,
    /// Paths verified by this adapter.
    pub delivery_modes: Vec<NativeDelivery>,
    /// Whether this arrangement can invoke it now.
    pub available: bool,
    /// The reason it cannot, where known.
    pub reason: Option<String>,
}

/// The readiness of a session's catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeCatalogState {
    /// The session supplied its effective mapping.
    Ready,
    /// The session cannot list yet.
    NotReady,
    /// This adapter cannot guarantee native dispatch.
    Unsupported,
    /// The actual read failed.
    Failed,
}

/// Whether this arrangement loads the project's own skills.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeProjectSkills {
    /// Proven project discovery, rather than a copied artifact.
    pub available: bool,
    /// A truthful reason where discovery is unavailable.
    pub reason: Option<String>,
}

/// One session's effective catalogue; a cold session never claims an empty ready list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeCatalog {
    /// Changes when binding, process generation or loaded mapping changes.
    pub revision: String,
    /// Whether the list is ready to select from.
    pub state: NativeCatalogState,
    /// Invocable effective winners only.
    pub actions: Vec<NativeAction>,
    /// Safe current reason.
    pub reason: Option<String>,
    /// Project-skill discovery for this arrangement.
    pub project_skills: NativeProjectSkills,
    /// Last successful observation, where present.
    pub as_of: Option<Time>,
}

impl NativeCatalog {
    /// An adapter without a verified native selector refuses the feature truthfully.
    #[must_use]
    pub fn unavailable(state: NativeCatalogState, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            revision: String::new(),
            state,
            actions: Vec::new(),
            reason: Some(reason.clone()),
            project_skills: NativeProjectSkills {
                available: false,
                reason: Some(reason),
            },
            as_of: None,
        }
    }
}

/// A live catalogue read, optionally invalidating its adapter's loaded mapping cache.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadNativeActionListQuery {
    /// Refresh without starting a model turn.
    pub refresh: Option<bool>,
}

crate::endpoints! {
    /// Reads the effective commands and skills of an existing native session.
    ThreadNativeActionList: GET "/threads/{id}/native-actions", Permission("thread.view"), query ThreadNativeActionListQuery, none, json NativeCatalog, 200;
}
