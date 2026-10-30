//! Settings, secrets metadata and parameter views.
//!
//! A setting has a key, a value, and a place where it is written: the system,
//! an org, a project, a topic or a thread, and for the keys that say so an
//! environment or a provider of its own. A [`ResolvedSetting`] is the value
//! that holds at one node with where it comes from and whether a lock above
//! decides it. Secrets are written once and never read back: only their names
//! are listed. A thread's parameters are its model, its effort and the other
//! parameters of its agent, each with whether the caller may change it.

use orch_types::{Json, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Page, PageQuery};
use crate::nodes::NodeRef;
use crate::routes::Access::{Permission, Rule, User};

/// The pattern the name of a secret matches: capitals, digits and underscores,
/// not beginning with a digit, at most [`SECRET_NAME_MAX_CHARS`] characters.
pub const SECRET_NAME_PATTERN: &str = "^[A-Z_][A-Z0-9_]{0,63}$";
/// The most characters of the name of a secret.
pub const SECRET_NAME_MAX_CHARS: u32 = 64;

/// Readable descendants where this place's effective setting still contributes, excluding this place itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingReachCounts {
    /// Departments below the place (root orgs as well, for the system).
    pub orgs: i64,
    /// Projects and nested folders.
    pub projects: i64,
    /// Topics.
    pub topics: i64,
    /// Threads.
    pub threads: i64,
}

/// A setting's actual inherited reach, using its merge/lock provenance and the caller's current read visibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SettingReach {
    /// Counts over all readable descendants, independent of the override page's cursor.
    pub applies: SettingReachCounts,
    /// Readable places whose own effective row replaces this place's sources. A wider ignored minimum
    /// or a value behind a lock is not an override. Additive rows retain the inherited contribution.
    pub overrides: Page<NodeRef>,
}

/// Whether a setting is a parameter of the work or a policy of those who govern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettingClass {
    /// A choice of whoever does the work.
    Parameter,
    /// A rule of those who govern the place.
    Policy,
}

/// How a setting's values from several places combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettingMerge {
    /// The nearest value wins.
    Replace,
    /// The values of all places together.
    Union,
    /// Only what every place allows.
    Intersect,
    /// Maps merged key by key.
    Map,
    /// Lists joined one after the other.
    Append,
    /// The smallest value.
    Min,
}

/// A setting as the registry defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingDef {
    /// Its key, such as `agent.model`.
    pub key: String,
    /// What it is called.
    pub title: String,
    /// What it does.
    pub description: String,
    /// The group of the settings screen that shows it.
    pub group: String,
    /// A JSON Schema of its value.
    pub schema: Json,
    /// Parameter or policy.
    pub class: SettingClass,
    /// The scopes it may be written at.
    pub scopes: Vec<String>,
    /// How values from several places combine.
    pub merge: SettingMerge,
    /// The value when nothing is written.
    pub default: Option<Json>,
    /// An authored value for this setting's help, distinct from its shipped/default value.
    #[serde(default)]
    pub example: Json,
    /// The key of its words in the application's own catalogue, for a setting the application
    /// ships: its title, its description and its keywords there, in the reader's language. The
    /// title and the description above are what is shown where the catalogue has none.
    #[serde(default)]
    pub text_key: Option<String>,
    /// Further words it is found by.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// How a number of it is shown, where it is a number of something.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<SettingDisplay>,
}

/// What a number of a setting counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettingDisplayUnit {
    /// Nothing more is said than the number.
    Raw,
    /// Bytes.
    Bytes,
    /// Milliseconds.
    Milliseconds,
    /// Seconds.
    Seconds,
    /// Minutes.
    Minutes,
    /// Hours.
    Hours,
    /// Days.
    Days,
    /// A percentage.
    Percent,
}

/// What a zero of a setting means, where it means something else than none of what it counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettingDisplayZero {
    /// Zero switches the thing off: it never happens.
    Never,
    /// Zero disables this behavior, shown as Off in its field.
    Off,
    /// Zero is no limit: nothing of this kind is held back.
    NoLimit,
}

/// How a number of a setting is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingDisplay {
    /// What it counts.
    pub unit: SettingDisplayUnit,
    /// What zero means, where it means something of its own.
    pub zero: Option<SettingDisplayZero>,
}

/// One value a setting was resolved from, with where it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingPart {
    /// Where it stands.
    pub source: SettingSource,
    /// The value there.
    pub value: Json,
    /// Whether the row is locked for everything below it.
    pub locked: bool,
    /// Whether a lock above leaves it out.
    pub ignored: bool,
}

/// Where the value of a setting at a node comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettingScope {
    /// Nothing was written; the registry's default.
    Default,
    /// A default that the thread's environment supplies.
    Environment,
    /// Written for the whole system.
    System,
    /// Written at an org.
    Org,
    /// Written at a project.
    Project,
    /// Written at a topic.
    Topic,
    /// Written at a thread.
    Thread,
    /// A value the environment keeps for one of its own keys; it is applied last.
    EnvironmentOwn,
    /// A value the provider keeps for one of its own keys; it is applied last.
    ProviderOwn,
    /// One host's own maintenance value.
    HostOwn,
}

/// Where a value comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingSource {
    /// The kind of place.
    pub scope: SettingScope,
    /// The node, where it is one.
    pub node: Option<NodeRef>,
}

/// A value written at one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingOwn {
    /// The value.
    pub value: Json,
    /// Whether it is locked for everything below.
    pub locked: bool,
    /// How many times the row was changed.
    pub revision: Revision,
}

/// The kinds of place a lock can be set at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LockScope {
    /// The whole system.
    System,
    /// An org.
    Org,
    /// A project.
    Project,
    /// A topic.
    Topic,
    /// A thread.
    Thread,
}

/// The lock above that decides a setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingLock {
    /// The kind of place.
    pub scope: LockScope,
    /// The node, where it is one.
    pub node: Option<NodeRef>,
}

/// The value that holds at a node, with where it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedSetting {
    /// The key.
    pub key: String,
    /// The value in effect at this node.
    pub value: Option<Json>,
    /// Where a replaced value comes from.
    pub source: SettingSource,
    /// The row written at this node itself.
    pub own: Option<SettingOwn>,
    /// The lock above that decides it.
    pub locked_by: Option<SettingLock>,
    /// Whether the caller holds the permission and no higher lock applies.
    pub writable: bool,
    /// Every value that was supplied for it, the default first and then from the top down, each
    /// with whether its row is locked and whether a lock above leaves it out: for a setting whose
    /// values combine, what each place adds.
    #[serde(default)]
    pub parts: Vec<SettingPart>,
    /// For the summarizer row: the safe reason its latest attempt failed or was skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_reason: Option<String>,
    /// Why this resolved setting does not apply to the selected harness, where one is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_applicable_reason: Option<String>,
    /// Values that can be chosen for this setting with its bound or usable reported agents. An empty
    /// list offers none; absence leaves the registry schema in force. Stored values stay visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_values: Option<Vec<String>>,
}

/// The name of a secret and when its value last changed. Values are never read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SecretInfo {
    /// The name.
    pub name: String,
    /// The node it is written at; `null` for an environment's own.
    pub node: Option<NodeRef>,
    /// When its value was last written.
    pub rotated_at: Option<Time>,
}

pub use crate::resources::{ModelOffered, ModelParameters, Rate, RateAbove, ServiceTier};

/// Why a model cannot be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelOptionReason {
    /// The provider does not list it.
    NotOnProvider,
    /// It is switched off on the provider.
    Disabled,
    /// An entry on the provider's page leaves it out for the caller.
    ExcludedByEntry,
    /// A policy leaves it out.
    ExcludedByPolicy,
}

/// A model a thread could be set to, with whether the caller may use it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelOption {
    /// Provider model groups used by the caps that cover this choice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// The name that is sent to the provider.
    pub model_name: String,
    /// The name that people see.
    pub display_name: String,
    /// What the model can do on this provider, its efforts among it.
    pub parameters: ModelParameters,
    /// What its tokenizer comes to, for the estimates where nobody counted.
    pub token_estimate: crate::programs::TokenEstimate,
    /// Whether the caller may use it.
    pub allowed: bool,
    /// Why not, when not.
    pub reason: Option<ModelOptionReason>,
}

/// When a changed parameter takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParameterApply {
    /// With the next turn.
    NextTurn,
    /// When the next session opens.
    NewSession,
    /// When the environment is made anew.
    RecreateEnvironment,
}

/// One parameter of a thread's agent, with its value and whether it can be changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParameterView {
    /// The parameter's key.
    pub key: String,
    /// Its definition: what it takes and how it is asked for.
    pub def: Json,
    /// Its value.
    pub value: Option<Json>,
    /// Where the value comes from.
    pub source: SettingSource,
    /// The lock above that decides it.
    pub locked_by: Option<SettingLock>,
    /// Whether the caller may change it.
    pub writable: bool,
    /// When a change takes effect.
    pub apply: ParameterApply,
}

/// The model of a thread and what it could be set to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadModelParameter {
    /// The model's routing name.
    pub value: Option<String>,
    /// Where it comes from.
    pub source: SettingSource,
    /// Whether the caller may change it.
    pub writable: bool,
    /// The models there are to choose from.
    pub options: Vec<ModelOption>,
}

/// The effort of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadEffortParameter {
    /// As it will be sent: the thread's own, else the model's default.
    pub value: Option<String>,
    /// Where it comes from.
    pub source: SettingSource,
    /// Whether the caller may change it.
    pub writable: bool,
    /// False when this harness takes no effort.
    pub supported: bool,
}

/// Limits over the harness's own sub-agents. The places of a thread resolve them independently of its model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SubagentLimits {
    /// How many spawned agents may be open concurrently.
    pub at_once: i64,
    /// The deepest spawned layer; 1 permits no nesting.
    pub depth: i64,
}

impl Default for SubagentLimits {
    fn default() -> Self {
        Self {
            at_once: 8,
            depth: 1,
        }
    }
}

/// The native controls this thread's harness declares, beside the place's resolved limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSubagentLimits {
    /// The smallest configured values along the hierarchy.
    pub limits: SubagentLimits,
    /// Names of the controls this harness enforces: at_once and depth; absent controls do not apply.
    pub supported: Vec<String>,
}

/// What can be set on a thread's agent, and what it is now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadParameters {
    /// The provider's ordinary resource view for a caller with provider View, shared by model options and status items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<crate::resources::Provider>,
    /// The model.
    pub model: ThreadModelParameter,
    /// The effort.
    pub effort: ThreadEffortParameter,
    /// The other parameters that are eligible and enabled.
    pub parameters: Vec<ParameterView>,
    /// The resolved native permission mode, independent of boundary requests.
    pub approval_mode: String,
    /// Modes offered by this harness, the place's policy and the current confinement; absent on older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_modes: Option<Vec<String>>,
    /// The session's native reviewer, where Auto was requested and the adapter declared one.
    #[serde(default)]
    pub approval_reviewer: Option<ApprovalReviewer>,
    /// Native sub-agent limits; absent on an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_limits: Option<ThreadSubagentLimits>,
}

/// The model used by the harness's own permission reviewer for this session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalReviewer {
    /// The routing name of the model; a native report takes precedence over the declared default.
    pub model: String,
    /// Whether the harness reported this model rather than the adapter declaring its default.
    pub reported: bool,
    /// Whether review usage is reported separately. Otherwise it is inside the harness's totals.
    pub usage_apart: bool,
}

/// Writes a setting at a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSettingSetBody {
    /// The value.
    pub value: Json,
    /// Whether to lock it for everything below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    /// The own-row revision the caller saw; zero requires that no own row exists. Omitted writes are unconditional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// Removes the own setting row only if it is still the revision the caller read; omitted is unconditional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSettingDeleteQuery {
    /// The revision of the own row that is to be removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// Writes a secret. Its name is added to the names of secrets at that node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSecretSetBody {
    /// The value, never shown again.
    pub value: String,
}

crate::endpoints! {
    /// Lists every setting there is, with its schema, class, scopes and merge rule.
    SettingRegistryGet: GET "/settings/registry", User, none, none, json Vec<SettingDef>, 200;
    /// Lists every setting that is valid at or below a node, as it resolves there.
    NodeSettingList: GET "/nodes/{id}/settings", Permission("node.view"), none, none, json Vec<ResolvedSetting>, 200;
    /// Counts where a setting still applies below this place and lists readable overriding places.
    NodeSettingReachGet: GET "/nodes/{id}/settings/{key}/reach", Permission("node.view"), query PageQuery, none, json SettingReach, 200;
    /// Writes a setting at a node, by the rule of its class and the scope written.
    NodeSettingSet: PUT "/nodes/{id}/settings/{key}", Rule("by the key's class and the scope written; for the two keys about where a thread runs, environment.manage on its environment"), none, json NodeSettingSetBody, json ResolvedSetting, 200;
    /// Removes a value written at a node, so that the setting is inherited again.
    NodeSettingDelete: DELETE "/nodes/{id}/settings/{key}", Rule("by the key's class and the scope written"), query NodeSettingDeleteQuery, none, json ResolvedSetting, 200;
    /// Lists the names of the secrets that apply at a node, resolved down the hierarchy.
    NodeSecretList: GET "/nodes/{id}/secrets", Permission("node.view"), none, none, json Vec<SecretInfo>, 200;
    /// Writes a secret at a node.
    NodeSecretSet: PUT "/nodes/{id}/secrets/{name}", Permission("settings.manage"), none, json NodeSecretSetBody, json SecretInfo, 200;
    /// Deletes a secret of a node.
    NodeSecretDelete: DELETE "/nodes/{id}/secrets/{name}", Permission("settings.manage"), none, none, none, 204;
    /// Reads a thread's model, effort and other parameters, with what the caller may change.
    ThreadParameterGet: GET "/threads/{id}/parameters", Permission("node.view"), none, none, json ThreadParameters, 200;
    /// Lists the settings of scope environment as they resolve for one environment.
    EnvironmentSettingList: GET "/environments/{id}/settings", Rule("any permission on the environment"), none, none, json Vec<ResolvedSetting>, 200;
    /// Lists the settings of scope provider as they resolve for one provider.
    ProviderSettingList: GET "/providers/{id}/settings", Rule("any permission on the provider"), none, none, json Vec<ResolvedSetting>, 200;
    /// Lists the host's maintenance settings and the system defaults they follow.
    HostSettingList: GET "/hosts/{id}/settings", Permission("host.view"), none, none, json Vec<ResolvedSetting>, 200;
    /// Changes one host maintenance setting without bypassing a system lock.
    HostSettingSet: PUT "/hosts/{id}/settings/{key}", Permission("host.manage"), none, json NodeSettingSetBody, json ResolvedSetting, 200;
    /// Makes one host maintenance setting follow the system again.
    HostSettingDelete: DELETE "/hosts/{id}/settings/{key}", Permission("host.manage"), query NodeSettingDeleteQuery, none, json ResolvedSetting, 200;
    /// Lists the names of the secrets an environment is given: those of its home and its own.
    EnvironmentSecretList: GET "/environments/{id}/secrets", Permission("environment.manage"), none, none, json Vec<SecretInfo>, 200;
    /// Writes a secret of one environment alone.
    EnvironmentSecretSet: PUT "/environments/{id}/secrets/{name}", Permission("environment.manage"), none, json NodeSecretSetBody, json SecretInfo, 200;
    /// Deletes a secret of one environment.
    EnvironmentSecretDelete: DELETE "/environments/{id}/secrets/{name}", Permission("environment.manage"), none, none, none, 204;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_value_that_a_lock_decides_says_where_it_comes_from_and_who_locked_it() {
        let written = json!({
            "key": "agent.model",
            "value": "gpt-6-sol",
            "source": {
                "scope": "project",
                "node": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "project", "name": "Web", "redacted": false }
            },
            "own": null,
            "locked_by": { "scope": "system", "node": null },
            "writable": false,
            "parts": [{
                "source": { "scope": "system", "node": null },
                "value": "gpt-6-sol",
                "locked": true,
                "ignored": false
            }]
        });
        let setting: ResolvedSetting = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(setting.source.scope, SettingScope::Project);
        assert_eq!(
            setting.locked_by.as_ref().map(|lock| lock.scope),
            Some(LockScope::System)
        );
        assert_eq!(serde_json::to_value(&setting).unwrap(), written);
    }

    #[test]
    fn a_thread_may_be_set_to_a_model_its_provider_lists_and_the_caller_may_use() {
        let option: ModelOption = serde_json::from_value(json!({
            "model_name": "gpt-6-sol",
            "display_name": "GPT-6 Sol",
            "token_estimate": { "ascii_prose_x100": 400, "ascii_structured_x100": 300, "other_x100": 100, "image_tokens": 1000, "measured": false },
            "parameters": { "efforts": ["low", "high"], "default_effort": "low", "rate": {
                "input_per_mtok": "1.25", "cached_input_per_mtok": "0.125", "output_per_mtok": "10",
                "tiers": { "fast": "2" }
            } },
            "allowed": false,
            "reason": "excluded_by_policy"
        }))
        .unwrap();
        assert_eq!(option.reason, Some(ModelOptionReason::ExcludedByPolicy));
        assert_eq!(
            option.parameters.efforts.as_deref().map(<[String]>::len),
            Some(2)
        );
        assert_eq!(
            option
                .parameters
                .rate
                .and_then(|rate| rate.tiers)
                .map(|tiers| tiers["fast"].clone()),
            Some("2".to_owned())
        );
    }

    #[test]
    fn a_setting_is_written_with_its_value_and_nothing_else_unless_locked() {
        assert!(serde_json::from_value::<NodeSettingSetBody>(json!({ "locked": true })).is_err());
        let body: NodeSettingSetBody = serde_json::from_value(json!({ "value": null })).unwrap();
        assert_eq!(body.value, Json::Null);
        assert_eq!(body.locked, None);
    }
}
