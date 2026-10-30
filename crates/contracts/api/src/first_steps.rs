//! Observed first steps in a readable org and the caller's portable setup choices.

use crate::documents::DocumentRef;
use crate::nodes::NodeRef;
use crate::personal::PersonalState;
use crate::routes::Access::{Rule, User};
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The way the person chose to work, never inferred from membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FirstStepsMode {
    /// Working alone.
    Solo,
    /// Working with a team.
    Team,
}

/// The private choice, with no manually checked setup facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirstStepsChoice {
    /// The chosen readable org, or no choice.
    pub org_id: Option<Id>,
    /// How the person chose to work there.
    pub work_mode: Option<FirstStepsMode>,
    /// Whether this person closed its checklist.
    pub dismissed: bool,
}

/// The org in view; absent uses the caller's still-readable saved choice or personal org.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirstStepsGetQuery {
    /// The org, at any level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// A setup step, independent of how it is worded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FirstStepsId {
    /// Choose an org and a mode.
    Team,
    /// Configure a credential or an endpoint.
    Provider,
    /// Enable a model explicitly.
    Models,
    /// Receive a completed answer.
    Work,
    /// Configure outward notifications.
    Away,
    /// Invite a teammate.
    Teammate,
}

/// What the latest readable observation proves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FirstStepsState {
    /// Observed complete.
    Done,
    /// Still open.
    Open,
    /// It could not be observed.
    Unknown,
}

/// A fact whose reader-facing sentence comes from the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FirstStepsFact {
    /// Choose an org.
    ChooseOrg,
    /// Solo was chosen.
    SoloChosen,
    /// Team was chosen.
    TeamChosen,
    /// A stored sign-in was reported.
    SignedIn,
    /// A key is set.
    KeySaved,
    /// An endpoint takes no key.
    EndpointConfigured,
    /// An explicitly enabled model exists.
    ModelsEnabled,
    /// A completed answer exists.
    AnsweredOnce,
    /// An outward route is configured, not proof of receipt.
    OutwardEnabled,
    /// An invitation awaits acceptance.
    InvitationWaiting,
    /// Another member joined.
    MemberJoined,
    /// The fact could not be read.
    Unknown,
    /// The step is open.
    NotDone,
}

/// An ordinary screen the caller may open to continue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FirstStepsAction {
    /// A same-origin application path.
    pub url: String,
    /// Whether the caller may act there.
    pub allowed: bool,
    /// A safe explanation when acting is not allowed.
    pub reason: Option<String>,
}

/// One observed setup step, with no hidden contents or independently stored tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FirstStepsStep {
    /// Which step.
    pub id: FirstStepsId,
    /// Its current observation state.
    pub state: FirstStepsState,
    /// Whether the completed answer came from a scripted adapter.
    pub scripted: bool,
    /// What was actually observed.
    pub fact: FirstStepsFact,
    /// A readable place where it was observed.
    pub source: Option<NodeRef>,
    /// How to continue on the ordinary screen.
    pub action: Option<FirstStepsAction>,
}

/// The ordinary defaults an invitation's role has; model access remains separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvitationRoleSummary {
    /// The role whose ordinary defaults were evaluated.
    pub role: crate::access::Role,
    /// May start a topic.
    pub start_topics: bool,
    /// May start a thread.
    pub start_threads: bool,
    /// May send inputs.
    pub send_inputs: bool,
    /// May answer requests in a thread.
    pub answer_requests: bool,
    /// A role never supplies provider access by itself.
    pub model_access_required: bool,
}

/// One permission-filtered read of setup in the chosen org, without probing a model or sending a notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FirstSteps {
    /// The chosen readable org.
    pub org: Option<NodeRef>,
    /// The person's choice for this org.
    pub work_mode: Option<FirstStepsMode>,
    /// Facts observed from current stored state.
    pub steps: Vec<FirstStepsStep>,
    /// Role summaries where available from the same defaults evaluation.
    pub invitation_roles: Vec<InvitationRoleSummary>,
    /// A currently readable ordinary Start here document.
    pub start_here: Option<DocumentRef>,
    /// When these facts were read.
    pub as_of: Time,
}

crate::endpoints! {
    /// Reads actual setup facts of a currently readable org.
    MeFirstStepsGet: GET "/me/first-steps", Rule("user; org View before setup facts"), query FirstStepsGetQuery, none, json FirstSteps, 200;
    /// Saves this person's choice and dismissal, preserving unrelated interface state.
    MeFirstStepsSet: PUT "/me/first-steps", User, none, json FirstStepsChoice, json PersonalState, 200;
}
