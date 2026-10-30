//! Browsers of an environment, their named profiles, and tickets to watch or drive them.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::environment::{EnvironmentRef, EnvironmentSupportBrowser, ThroughThreadQuery};
use crate::nodes::NodeRef;
use crate::routes::Access::{Permission, Public, Rule};
use crate::users::UserRef;

/// The WebSocket subprotocol of a browser's picture and input.
pub const BROWSER_SUBPROTOCOL: &str = "orch.browser.v1";
/// A ticket lives thirty seconds, as a shell's single-use ticket does.
pub const BROWSER_TICKET_SECONDS: u32 = 30;

/// How a browser is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPreset {
    /// The full browser without a window.
    Headless,
    /// A window on a display.
    Window,
}

/// Who started the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserOrigin {
    /// The environment's host, on request.
    Started,
    /// An agent, which gave its endpoint to Orch.
    Announced,
}

/// Where a known browser stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserState {
    /// Its place is reserved while its process starts.
    Opening,
    /// Its driving address answers.
    Open,
    /// It has ended.
    Closed,
}

/// A directory that keeps a browser's sign-ins outside the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserProfile {
    /// Its lowercase directory name.
    pub name: String,
    /// Its size where the environment measured it.
    pub size_bytes: Option<i64>,
    /// When it was last used.
    pub used_at: Option<Time>,
    /// The known browser running on it now.
    pub browser: Option<Id>,
}

/// A browser known to Orch in an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Browser {
    /// Its identifier.
    pub id: Id,
    /// Where it runs.
    pub environment: EnvironmentRef,
    /// The named profile, where it has one.
    pub profile: Option<String>,
    /// How it came to be known.
    pub origin: BrowserOrigin,
    /// Its opener, or null for an agent.
    pub opened_by: Option<UserRef>,
    /// The thread it was opened beside, where the caller may view it.
    pub thread: Option<NodeRef>,
    /// How Orch started it, where Orch did.
    pub preset: Option<BrowserPreset>,
    /// The display its window uses.
    pub display: Option<Id>,
    /// Its address inside the environment, only for a holder of Manage.
    pub endpoint: Option<String>,
    /// The browser's own name and version.
    pub program: Option<String>,
    /// An agent's line for people about what the browser is for.
    pub note: Option<String>,
    /// The last reported number of tabs.
    pub tabs: Option<i64>,
    /// How many people have it on a screen.
    pub watching: i64,
    /// Who sent input recently.
    pub driving: Vec<UserRef>,
    /// Its state.
    pub state: BrowserState,
    /// How it ended.
    pub close_reason: Option<String>,
    /// When it was opened.
    pub opened_at: Time,
    /// Its last activity.
    pub last_used_at: Time,
    /// When it ended.
    pub closed_at: Option<Time>,
}

/// Why browsers are off although the caller may view them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowsersOff {
    /// The home place's policy forbids them.
    DisabledByPolicy,
    /// The environment's switch is off.
    EnvironmentSwitchOff,
    /// Its machine's owner forbids them locally.
    MachinePolicy,
    /// The environment is unreachable.
    EnvironmentOffline,
}

/// The availability of one way to start a browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentBrowsersPreset {
    /// The preset.
    pub name: BrowserPreset,
    /// Whether the environment can start it.
    pub available: bool,
    /// Why it cannot, where it cannot.
    pub reason: Option<String>,
}

/// How many browsers started by Orch run, and how many may.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentBrowsersLimit {
    /// Those started by Orch that still run.
    pub open: i64,
    /// The resolved limit.
    pub max: i64,
}

/// Browsers and profiles with the gates, available programs and limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentBrowsers {
    /// Why nothing is shown now, or null when on.
    pub off: Option<BrowsersOff>,
    /// What was found there.
    pub support: EnvironmentSupportBrowser,
    /// The ways one can be started.
    pub presets: Vec<EnvironmentBrowsersPreset>,
    /// The profiles it keeps.
    pub profiles: Vec<BrowserProfile>,
    /// Where they lie, only for a holder of Manage.
    pub profiles_dir: Option<String>,
    /// Live browsers by latest use, followed by those closed within a day.
    pub browsers: Vec<Browser>,
    /// The environment's limit.
    pub limit: EnvironmentBrowsersLimit,
}

/// The whole unretained list shared by a browser feed and HTTP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentBrowsersList {
    /// The browser rows visible to the caller.
    pub browsers: Vec<Browser>,
    /// The environment's named profiles.
    pub profiles: Vec<BrowserProfile>,
}

/// Asks the environment to discover installed programs again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentBrowserListQuery {
    /// Look before answering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<bool>,
}

/// Opens or joins the browser of a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentBrowserCreateBody {
    /// The profile's name, never a path.
    pub profile: String,
    /// Make it if missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<bool>,
    /// How it is started; the environment's setting when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<BrowserPreset>,
    /// Where its first tab goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Asks for a ticket that watches, or also drives.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserTicketCreateBody {
    /// Sending input requires Manage beside View.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<bool>,
}

/// The single-use credential of a browser socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTicketCreateReply {
    /// The credential.
    pub ticket: String,
    /// When it expires.
    pub ticket_expires_at: Time,
}

/// The socket's credential, supplied in its address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserAttachQuery {
    /// The ticket.
    pub ticket: String,
}

/// Makes an empty profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserProfileCreateBody {
    /// The profile's name.
    pub name: String,
}

/// Gives a profile another name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserProfileUpdateBody {
    /// Its new name.
    pub name: String,
}

/// Confirms removal of a profile and its sign-ins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserProfileDeleteQuery {
    /// Its name exactly.
    pub confirm: String,
}

crate::endpoints! {
    /// Lists an environment's browsers and profiles and optionally looks again for its programs.
    EnvironmentBrowserList: GET "/environments/{id}/browsers", Permission("browser.view"), query EnvironmentBrowserListQuery, none, json EnvironmentBrowsers, 200;
    /// Starts a browser, or joins the one already running on its profile.
    EnvironmentBrowserCreate: POST "/environments/{id}/browsers", Permission("browser.manage"), query ThroughThreadQuery, json EnvironmentBrowserCreateBody, json Browser, 201;
    /// Asks a browser to quit, keeping its profile.
    BrowserClose: DELETE "/browsers/{browser_id}", Rule("browser.manage on its environment"), none, none, json Browser, 200;
    /// Gives a ticket to watch a browser, or with Manage to drive it.
    BrowserTicketCreate: POST "/browsers/{browser_id}/ticket", Rule("browser.view on its environment; control also needs browser.manage"), none, json BrowserTicketCreateBody, json BrowserTicketCreateReply, 200;
    /// Watches and, on a control ticket, drives a browser through a socket.
    BrowserAttach: GET "/ws/browsers/{browser_id}", Public, query BrowserAttachQuery, none, raw, 101;
    /// Creates a named empty profile outside the workspace.
    BrowserProfileCreate: POST "/environments/{id}/browser-profiles", Permission("browser.manage"), none, json BrowserProfileCreateBody, json BrowserProfile, 201;
    /// Renames a profile that no browser runs on.
    BrowserProfileUpdate: PATCH "/environments/{id}/browser-profiles/{name}", Permission("browser.manage"), none, json BrowserProfileUpdateBody, json BrowserProfile, 200;
    /// Removes a profile and its sign-ins after its name is confirmed.
    BrowserProfileDelete: DELETE "/environments/{id}/browser-profiles/{name}", Permission("browser.manage"), query BrowserProfileDeleteQuery, none, none, 204;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_profile_keeps_unknown_size_and_use_as_null() {
        let value = json!({"name":"team", "size_bytes":null, "used_at":null, "browser":null});
        let profile: BrowserProfile = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(profile).unwrap(), value);
    }

    #[test]
    fn an_open_request_names_a_profile_and_refuses_paths_as_members() {
        let body: EnvironmentBrowserCreateBody =
            serde_json::from_value(json!({"profile":"team"})).unwrap();
        assert_eq!(body.create, None);
        assert!(
            serde_json::from_value::<EnvironmentBrowserCreateBody>(
                json!({"profile":"team", "path":"/tmp/profile"})
            )
            .is_err()
        );
    }
}
