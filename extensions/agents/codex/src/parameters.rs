//! Map effort and eligible neutral parameters to Codex fields; the speeds and access programs Codex offers for a model, which a provider enables before a thread shows them.
//!
//! A turn states everything every time: the model, the effort, the approval mode and the sandbox, and the
//! values of the parameters the adapter publishes ([`definitions`]). They are passed to the harness on each
//! `turn/start`, so that a change takes effect with the next turn and nothing a turn set stays set for the
//! next one: the service tier in particular is passed as the tier of that turn alone, and the standard one
//! is named when no other is chosen.
//!
//! What costs more or reaches further is never switched on by being offered. The speeds a model has beside
//! the ordinary one, the access programs it lists and a context larger than the window it starts with are
//! reported as *offered* ([`offered_for`]); they become the model's own when a manager of the provider
//! enables them, and only then does a thread show the parameter. What a thread starts with of each is the
//! model's entry's to say, and the parameters take it from there.
//!
//! **The context** is the only parameter that belongs to the session and not to the turn. This harness holds
//! the threshold at which it shortens a conversation apart from the window of the model, so the adapter tells
//! it both ([`context_members`]); the threshold follows from the context by a rule (nine tenths, which is the
//! harness's own) and is set by nobody.

use std::path::Path;

use orch_agent_contract::credentials::CredentialPath;
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::parameters::{
    Condition, DefaultFromModel, MODEL_WINDOW_KEY, ModelFeature, ModelLimit, ModelList, Offered,
    ParameterApply, ParameterDef, ParameterDefault, ParameterGroup, ParameterOption,
    ParameterOptions, ParameterRange, ParameterScope, ParameterType, RangeMax, ServiceTier,
};
use orch_agent_contract::session::MountGrant;
use orch_types::Json;
use serde_json::{Map, json};

/// The key of the speed of a turn.
pub const SERVICE_TIER: &str = "service_tier";
/// The key of the context a session is given.
pub const CONTEXT: &str = "context";
/// The key of how much of its reasoning the harness summarises.
pub const REASONING_SUMMARY: &str = "reasoning_summary";
/// The key of the access program a turn asks for.
pub const CYBER_ACCESS_PROGRAM: &str = "cyber_access_program";

/// The feature of the resolved `agent.features` that switches on the access program parameter.
pub const CYBER_ACCESS_FEATURE: &str = "codex.cyber_access";

/// What a turn is told when it asks for no other speed: the harness's name for the ordinary one.
pub const STANDARD_SERVICE_TIER: &str = "default";

/// The least context a thread can be given. A context below this is not one the harness works well with.
pub const CONTEXT_LEAST: i64 = 32_000;

/// Where Codex is told to shorten a conversation, in tenths of the context: nine. That is the harness's own
/// rule (it holds any threshold to nine tenths of the window it works with); the windows of its catalogue
/// already leave the room for the model's answer.
const COMPACT_AT_TENTHS: i64 = 9;
/// How many tenths make a whole.
const TENTHS: i64 = 10;

const ALL_SCOPES: [ParameterScope; 4] = [
    ParameterScope::Org,
    ParameterScope::Project,
    ParameterScope::Topic,
    ParameterScope::Thread,
];

fn option(value: &str, label: &str, description: &str) -> ParameterOption {
    ParameterOption {
        value: value.to_owned(),
        label: label.to_owned(),
        description: Some(description.to_owned()),
    }
}

/// The parameters the adapter publishes.
#[must_use]
pub fn definitions() -> Vec<ParameterDef> {
    vec![
        ParameterDef {
            key: SERVICE_TIER.to_owned(),
            title: "Speed".to_owned(),
            description: "A faster mode of the model, which uses the plan up faster. Only the speeds a manager of the provider enabled for the model are offered.".to_owned(),
            group: ParameterGroup::Model,
            kind: ParameterType::Enum,
            options: Some(ParameterOptions::FromModel {
                from_model: ModelList::ServiceTiers,
            }),
            range: None,
            default: ParameterDefault::FromModel {
                from_model: DefaultFromModel::DefaultServiceTier,
            },
            scopes: ALL_SCOPES.to_vec(),
            apply: ParameterApply::NextTurn,
            eligible_when: Some(Condition::ModelHas(ModelFeature::ServiceTiers)),
            feature: None,
        },
        ParameterDef {
            key: CONTEXT.to_owned(),
            title: "Context".to_owned(),
            description: "How much of a conversation the agent keeps, in tokens, up to the largest the model's managers opened. Codex shortens the conversation at nine tenths of that size (at 270 000 for a context of 300 000), and works with no more than its catalogue allows the model.".to_owned(),
            group: ParameterGroup::Model,
            kind: ParameterType::Integer,
            options: None,
            range: Some(ParameterRange {
                min: CONTEXT_LEAST,
                max: RangeMax::FromModel {
                    from_model: ModelLimit::ContextMax,
                },
            }),
            default: ParameterDefault::FromModel {
                from_model: DefaultFromModel::DefaultContext,
            },
            scopes: ALL_SCOPES.to_vec(),
            apply: ParameterApply::NewSession,
            eligible_when: Some(Condition::ModelHas(ModelFeature::ContextMax)),
            feature: None,
        },
        ParameterDef {
            key: REASONING_SUMMARY.to_owned(),
            title: "Reasoning summary".to_owned(),
            description: "How much of its reasoning the agent summarises as it works.".to_owned(),
            group: ParameterGroup::Behaviour,
            kind: ParameterType::Enum,
            options: Some(ParameterOptions::Listed(vec![
                option("auto", "Auto", "The model decides how much to say."),
                option("concise", "Concise", "A short summary."),
                option("detailed", "Detailed", "A fuller account of the reasoning."),
                option("none", "None", "No summary."),
            ])),
            range: None,
            default: ParameterDefault::Value(json!("auto")),
            scopes: ALL_SCOPES.to_vec(),
            apply: ParameterApply::NextTurn,
            eligible_when: None,
            feature: None,
        },
        ParameterDef {
            key: CYBER_ACCESS_PROGRAM.to_owned(),
            title: "Access program".to_owned(),
            description: "An authorised program for security work, which the vendor grants to the account. Only the programs a manager of the provider enabled for the model are offered.".to_owned(),
            group: ParameterGroup::Safety,
            kind: ParameterType::Enum,
            options: Some(ParameterOptions::FromModel {
                from_model: ModelList::AccessPrograms,
            }),
            range: None,
            default: ParameterDefault::FromModel {
                from_model: DefaultFromModel::DefaultAccessProgram,
            },
            scopes: vec![ParameterScope::Thread],
            apply: ParameterApply::NextTurn,
            eligible_when: Some(Condition::All(vec![
                Condition::CredentialPath(CredentialPath::Native),
                Condition::ModelHas(ModelFeature::AccessPrograms),
            ])),
            feature: Some(CYBER_ACCESS_FEATURE.to_owned()),
        },
    ]
}

/// The word the harness uses for an approval mode.
#[must_use]
pub const fn approval_policy(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "untrusted",
        ApprovalMode::OnRequest | ApprovalMode::Auto => "on-request",
        ApprovalMode::Bypass => "never",
    }
}

/// The reviewer belongs to the mode; an obsolete independent parameter cannot select it.
#[must_use]
pub const fn approval_reviewer(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Auto => "auto_review",
        _ => "user",
    }
}

/// The word the harness uses for a sandbox mode when a thread is opened.
#[must_use]
pub const fn sandbox_word(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::Workspace => "workspace-write",
        SandboxMode::Open => "danger-full-access",
    }
}

/// The sandbox of a turn as the harness states it: for the mode `workspace`, the thread's directory and the
/// attached mounts that may be written are the places it may write, and nothing leaves the machine.
#[must_use]
pub fn sandbox_policy(mode: SandboxMode, directory: &str, mounts: &[MountGrant]) -> Json {
    match mode {
        SandboxMode::ReadOnly => json!({ "type": "readOnly", "networkAccess": false }),
        SandboxMode::Open => json!({ "type": "dangerFullAccess" }),
        SandboxMode::Workspace => {
            let mut roots = vec![directory.to_owned()];
            roots.extend(
                mounts
                    .iter()
                    .filter(|mount| !mount.read_only)
                    .map(|mount| mount.path.clone()),
            );
            json!({
                "type": "workspaceWrite",
                "writableRoots": roots,
                "networkAccess": false,
                "excludeTmpdirEnvVar": false,
                "excludeSlashTmp": false
            })
        }
    }
}

/// The places a thread works in, for the harness's own idea of its workspace: its directory, then every
/// mount that may be written.
#[must_use]
pub fn workspace_roots(directory: &str, mounts: &[MountGrant]) -> Vec<String> {
    let mut roots = vec![directory.to_owned()];
    roots.extend(
        mounts
            .iter()
            .filter(|mount| !mount.read_only)
            .map(|mount| mount.path.clone()),
    );
    roots
}

/// The collaboration mode a turn states: `plan` while it plans, `default` for the turn after a plan. Its settings
/// carry the turn's model and effort, since the mode takes precedence over them, and no developer instructions of
/// Orch's own: `null` keeps the harness's built-in instructions for the mode.
#[must_use]
pub fn collaboration_mode(plan: bool, model: &str, effort: Option<&str>) -> Json {
    json!({
        "mode": if plan { "plan" } else { "default" },
        "settings": {
            "model": model,
            "reasoning_effort": effort.filter(|effort| !effort.is_empty()),
            "developer_instructions": null,
        },
    })
}

/// Whether an access program is one of the Daybreak programs, for which a thread is told it is enabled.
#[must_use]
pub fn is_daybreak(program: &str) -> bool {
    program.starts_with("daybreak")
}

/// What the parameters a thread is shown and sent come to, for one turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnSettings {
    /// The speed of the turn; `None` is the ordinary one.
    pub service_tier: Option<String>,
    /// How much reasoning is summarised.
    pub summary: Option<String>,
    /// The access program asked for.
    pub cyber_access_program: Option<String>,
}

impl TurnSettings {
    /// Reads the values of `params` (an object by parameter key). A value of another type than text, or
    /// outside what the harness takes, is left out: a parameter that is not valid is not sent.
    #[must_use]
    pub fn from_params(params: &Json) -> Self {
        let text = |key: &str, allowed: Option<&[&str]>| {
            params
                .get(key)
                .and_then(Json::as_str)
                .filter(|value| !value.is_empty())
                .filter(|value| allowed.is_none_or(|allowed| allowed.contains(value)))
                .map(str::to_owned)
        };
        Self {
            service_tier: text(SERVICE_TIER, None),
            summary: text(
                REASONING_SUMMARY,
                Some(&["auto", "concise", "detailed", "none"]),
            ),
            cyber_access_program: text(
                CYBER_ACCESS_PROGRAM,
                Some(&["standard", "daybreakBlue", "daybreakRed"]),
            ),
        }
    }

    /// Puts the settings into the parameters of a `turn/start`. The speed is always stated, as the tier of
    /// this turn alone.
    pub fn put_into(&self, params: &mut Map<String, Json>, effort: Option<&str>) {
        params.insert(
            "serviceTierForTurn".to_owned(),
            json!(
                self.service_tier
                    .as_deref()
                    .unwrap_or(STANDARD_SERVICE_TIER)
            ),
        );
        if let Some(effort) = effort.filter(|effort| !effort.is_empty()) {
            params.insert("effort".to_owned(), json!(effort));
        }
        if let Some(summary) = &self.summary {
            params.insert("summary".to_owned(), json!(summary));
        }
        if let Some(program) = &self.cyber_access_program {
            params.insert("cyberAccessProgram".to_owned(), json!(program));
        }
    }
}

/// The point at which Codex is told to shorten a conversation for a context of `context` tokens: nine tenths
/// of it, in integers. That is the harness's own rule, written out only so that the smaller threshold of a
/// catalogue entry does not hold a raised window back; no room for the model's answer is taken, since the
/// windows of the harness's catalogue are already the model's window less its largest answer. With a million
/// tokens it is 900 000.
#[must_use]
pub fn compact_limit(context: i64) -> i64 {
    context.saturating_mul(COMPACT_AT_TENTHS) / TENTHS
}

/// The two members of a session's configuration that say how large the model's context is and where Codex
/// shortens the conversation, from the `params` the session was opened with: the context in force (the
/// parameter `context`) and the model's own window (under [`MODEL_WINDOW_KEY`]).
///
/// - A context that is the model's own window, or none (the model has no larger context open): the harness is
///   told that window where there is one, since a model it does not know has to be told it, and otherwise
///   nothing; and no threshold, so that its own rule holds.
/// - Any other context: the context is the window, and the threshold is [`compact_limit`] of it, the harness's
///   own rule written out because a catalogue entry may carry a smaller threshold of its own that would go on
///   shortening the conversation where the smaller window did.
#[must_use]
pub fn context_members(params: &Json) -> Map<String, Json> {
    let number = |key: &str| params.get(key).and_then(Json::as_i64).filter(|n| *n > 0);
    let (context, window) = (number(CONTEXT), number(MODEL_WINDOW_KEY));
    let mut members = Map::new();
    match context {
        Some(context) if Some(context) != window => {
            members.insert("model_context_window".to_owned(), json!(context));
            members.insert(
                "model_auto_compact_token_limit".to_owned(),
                json!(compact_limit(context)),
            );
        }
        _ => {
            if let Some(window) = window {
                members.insert("model_context_window".to_owned(), json!(window));
            }
        }
    }
    members
}

/// What a model entry of the harness's catalogue offers beyond the ordinary: its other speeds, the
/// access programs listed for the account, and a context larger than the window it starts with.
#[must_use]
pub fn offered_for(model: &Json) -> Option<Offered> {
    let tiers: Vec<ServiceTier> = model
        .get("serviceTiers")
        .and_then(Json::as_array)
        .map(|tiers| {
            tiers
                .iter()
                .filter_map(|tier| {
                    let id = tier.get("id")?.as_str()?;
                    let label = tier.get("name").and_then(Json::as_str).unwrap_or(id);
                    Some(ServiceTier {
                        id: id.to_owned(),
                        label: label.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let programs: Vec<String> = model
        .get("availableAccessPrograms")
        .and_then(|programs| programs.get("cyber"))
        .and_then(Json::as_array)
        .map(|programs| {
            programs
                .iter()
                .filter_map(Json::as_str)
                .filter(|program| *program != "standard")
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let window = window_of(model);
    // The most a configuration may set, where that is more than the window the model starts with.
    let larger = model
        .get("maxContextWindow")
        .and_then(Json::as_i64)
        .filter(|largest| window.is_none_or(|window| *largest > window));
    if tiers.is_empty() && programs.is_empty() && larger.is_none() {
        return None;
    }
    Some(Offered {
        service_tiers: (!tiers.is_empty()).then_some(tiers),
        access_programs: (!programs.is_empty()).then_some(programs),
        context_max: larger,
    })
}

/// The window the harness's catalogue gives a model, which it works with when it is told nothing.
#[must_use]
pub fn window_of(model: &Json) -> Option<i64> {
    model
        .get("contextWindow")
        .and_then(Json::as_i64)
        .filter(|window| *window > 0)
}

/// Whether `path` is inside `root`: the same directory or below it, compared by components.
#[must_use]
pub fn is_inside(path: &str, root: &str) -> bool {
    Path::new(path).starts_with(Path::new(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::parameters::check_parameters;

    #[test]
    fn the_parameters_published_are_consistent_and_five() {
        let defs = definitions();
        assert_eq!(check_parameters(&defs), Ok(()));
        let keys: Vec<_> = defs.iter().map(|def| def.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                SERVICE_TIER,
                CONTEXT,
                REASONING_SUMMARY,
                CYBER_ACCESS_PROGRAM
            ]
        );
    }

    #[test]
    fn the_context_is_an_integer_from_what_the_harness_takes_to_what_the_models_managers_opened() {
        let context = definitions()
            .into_iter()
            .find(|def| def.key == CONTEXT)
            .unwrap();
        assert_eq!(
            context.range,
            Some(ParameterRange {
                min: CONTEXT_LEAST,
                max: RangeMax::FromModel {
                    from_model: ModelLimit::ContextMax
                }
            })
        );
        assert_eq!(
            context.default,
            ParameterDefault::FromModel {
                from_model: DefaultFromModel::DefaultContext
            }
        );
        assert_eq!(context.apply, ParameterApply::NewSession);
        assert!(
            context.description.contains("270 000"),
            "it says in a sentence where the conversation is shortened"
        );
        // A speed and a program start where the model's entry says, and are left alone where it says nothing.
        for key in [SERVICE_TIER, CYBER_ACCESS_PROGRAM] {
            let def = definitions()
                .into_iter()
                .find(|def| def.key == key)
                .unwrap();
            assert!(
                matches!(def.default, ParameterDefault::FromModel { .. }),
                "{key}"
            );
        }
    }

    #[test]
    fn where_the_conversation_is_shortened_is_nine_tenths_of_the_context_in_integers() {
        assert_eq!(compact_limit(1_000_000), 900_000);
        assert_eq!(compact_limit(300_000), 270_000);
        assert_eq!(compact_limit(400_000), 360_000);
        assert_eq!(compact_limit(32_000), 28_800);
        // In integers: what does not divide is left off, and no room is taken for the answer at any size.
        assert_eq!(compact_limit(1_000_005), 900_004);
        assert_eq!(compact_limit(9), 8);
        assert_eq!(compact_limit(1), 0);
        assert_eq!(compact_limit(2_400_000), 2_160_000);
        // Never more than nine tenths and never a whole token less than that.
        for context in (32_000..=2_400_000).step_by(37_001) {
            let limit = compact_limit(context);
            assert!(
                limit * 10 <= context * 9 && context * 9 < (limit + 1) * 10,
                "{context}"
            );
        }
    }

    #[test]
    fn a_larger_context_is_told_to_the_harness_as_its_window_and_where_it_shortens_the_conversation()
     {
        let members = context_members(&json!({ "context": 1_000_000, "context_window": 272_000 }));
        assert_eq!(members["model_context_window"], 1_000_000);
        assert_eq!(members["model_auto_compact_token_limit"], 900_000);
        // A smaller context than the largest is a context like any other.
        let members = context_members(&json!({ "context": 300_000, "context_window": 272_000 }));
        assert_eq!(members["model_context_window"], 300_000);
        assert_eq!(members["model_auto_compact_token_limit"], 270_000);
        // A model whose entry gives no window is told the context all the same.
        let members = context_members(&json!({ "context": 1_000_000 }));
        assert_eq!(members["model_context_window"], 1_000_000);
        assert_eq!(members["model_auto_compact_token_limit"], 900_000);
    }

    #[test]
    fn the_models_own_window_is_told_as_it_is_and_the_harness_keeps_its_own_threshold() {
        // The context is the model's own window: no threshold, so that the harness's rule holds.
        let members = context_members(&json!({ "context": 272_000, "context_window": 272_000 }));
        assert_eq!(members["model_context_window"], 272_000);
        assert!(members.get("model_auto_compact_token_limit").is_none());
        // No larger context is open: nothing is chosen, and the window is still told.
        let members = context_members(&json!({ "context_window": 272_000 }));
        assert_eq!(members["model_context_window"], 272_000);
        assert!(members.get("model_auto_compact_token_limit").is_none());
        // Nothing is known of the model: the harness is told nothing.
        assert!(context_members(&json!({})).is_empty());
        assert!(context_members(&json!({ "context": "big", "context_window": 0 })).is_empty());
    }

    #[test]
    fn the_modes_of_approval_and_sandbox_are_the_words_of_the_harness() {
        assert_eq!(approval_policy(ApprovalMode::Ask), "untrusted");
        assert_eq!(approval_policy(ApprovalMode::OnRequest), "on-request");
        assert_eq!(approval_policy(ApprovalMode::Auto), "on-request");
        assert_eq!(approval_policy(ApprovalMode::Bypass), "never");
        assert_eq!(sandbox_word(SandboxMode::ReadOnly), "read-only");
        assert_eq!(sandbox_word(SandboxMode::Workspace), "workspace-write");
        assert_eq!(sandbox_word(SandboxMode::Open), "danger-full-access");
    }

    #[test]
    fn the_workspace_sandbox_may_write_the_directory_and_the_mounts_that_are_not_read_only() {
        let mounts = vec![
            MountGrant {
                name: "src".into(),
                path: "/data/src".into(),
                read_only: true,
            },
            MountGrant {
                name: "out".into(),
                path: "/data/out".into(),
                read_only: false,
            },
        ];
        let policy = sandbox_policy(SandboxMode::Workspace, "/workspace/web", &mounts);
        assert_eq!(policy["type"], "workspaceWrite");
        assert_eq!(
            policy["writableRoots"],
            json!(["/workspace/web", "/data/out"])
        );
        assert_eq!(policy["networkAccess"], false);
        assert_eq!(
            sandbox_policy(SandboxMode::ReadOnly, "/w", &[]),
            json!({ "type": "readOnly", "networkAccess": false })
        );
        assert_eq!(
            sandbox_policy(SandboxMode::Open, "/w", &[]),
            json!({ "type": "dangerFullAccess" })
        );
    }

    #[test]
    fn a_turn_states_its_speed_every_time_and_the_ordinary_one_by_name() {
        let mut params = Map::new();
        TurnSettings::from_params(&json!({})).put_into(&mut params, None);
        assert_eq!(params["serviceTierForTurn"], "default");
        assert!(params.get("effort").is_none() && params.get("summary").is_none());

        let mut params = Map::new();
        TurnSettings::from_params(&json!({
            "service_tier": "priority", "reasoning_summary": "concise", "approvals_reviewer": "auto_review",
            "cyber_access_program": "daybreakBlue"
        }))
        .put_into(&mut params, Some("high"));
        assert_eq!(params["serviceTierForTurn"], "priority");
        assert_eq!(params["effort"], "high");
        assert_eq!(params["summary"], "concise");
        assert!(
            params.get("approvalsReviewer").is_none(),
            "an obsolete parameter cannot change the mode"
        );
        assert_eq!(params["cyberAccessProgram"], "daybreakBlue");
    }

    #[test]
    fn a_value_the_harness_does_not_take_is_not_sent() {
        let settings = TurnSettings::from_params(&json!({
            "reasoning_summary": "verbose", "approvals_reviewer": 3, "cyber_access_program": "root", "service_tier": ""
        }));
        assert_eq!(settings, TurnSettings::default());
    }

    #[test]
    fn what_a_model_offers_beyond_the_ordinary_is_listed_as_offered_and_nothing_more() {
        let model = json!({
            "serviceTiers": [ { "id": "priority", "name": "Fast", "description": "2x speed" } ],
            "availableAccessPrograms": { "cyber": ["standard", "daybreakBlue"] }
        });
        let offered = offered_for(&model).unwrap();
        assert_eq!(
            offered.service_tiers,
            Some(vec![ServiceTier {
                id: "priority".into(),
                label: "Fast".into()
            }])
        );
        assert_eq!(
            offered.access_programs,
            Some(vec!["daybreakBlue".to_owned()])
        );
        assert_eq!(
            offered_for(&json!({ "serviceTiers": [], "availableAccessPrograms": null })),
            None
        );
    }

    #[test]
    fn a_larger_context_the_catalogue_lists_is_on_offer_and_the_window_it_starts_with_is_the_models_own()
     {
        let model = json!({
            "contextWindow": 272_000, "maxContextWindow": 1_000_000,
            "serviceTiers": [], "availableAccessPrograms": null
        });
        assert_eq!(window_of(&model), Some(272_000));
        assert_eq!(
            offered_for(&model),
            Some(Offered {
                context_max: Some(1_000_000),
                ..Offered::default()
            })
        );
        // A largest window that is no larger than the window offers nothing, and a list without either says nothing.
        assert_eq!(
            offered_for(&json!({ "contextWindow": 272_000, "maxContextWindow": 272_000 })),
            None
        );
        assert_eq!(window_of(&json!({ "model": "gpt-6-sol" })), None);
        assert_eq!(offered_for(&json!({ "model": "gpt-6-sol" })), None);
    }

    #[test]
    fn a_daybreak_program_is_told_apart_from_the_standard_one() {
        assert!(is_daybreak("daybreakRed") && is_daybreak("daybreakBlue"));
        assert!(!is_daybreak("standard"));
        assert!(is_inside("/workspace/web/api", "/workspace/web"));
        assert!(!is_inside("/workspace/webs", "/workspace/web"));
    }
}
