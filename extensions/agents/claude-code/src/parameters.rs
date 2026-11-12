//! Map effort, auxiliary models and the adapter's own parameters.
//!
//! The model and the reasoning effort are not parameters of the adapter: the engine states both with every
//! session and turn, and this adapter passes both to the harness every time, in the harness's terms. What
//! the adapter does publish are parameters of its own ([`definitions`]), and the rule by which the
//! models of the harness's helper steps are kept on the thread's model ([`model_environment`]).
//!
//! Several parameters read the model's entry at its provider: the **service tier**, which for this harness is its
//! fast mode on the models that have one, and the **context**. The harness is told the context and nothing
//! else: it keeps room for the answer by a margin of its own, so there is no point for the adapter to work out
//! ([`ONE_MILLION_ABOVE`], [`auto_compact_window`], [`model_argument`]).

use orch_agent_contract::parameters::{
    Condition, DefaultFromModel, MODEL_WINDOW_KEY, ModelFeature, ModelLimit, ModelList,
    ParameterApply, ParameterDef, ParameterDefault, ParameterGroup, ParameterOption,
    ParameterOptions, ParameterRange, ParameterScope, ParameterType, RangeMax,
};
use orch_types::Json;
use serde_json::json;

/// The parameter that is the turn's service tier: the harness's fast mode.
pub const SERVICE_TIER: &str = "service_tier";
/// The parameter that is the thread's context.
pub const CONTEXT: &str = "context";
/// The service tier that is the harness's fast mode.
pub const FAST_TIER: &str = "fast";
/// The least context a thread can be given: what the harness takes of the size at which it shortens a
/// conversation.
pub const CONTEXT_LEAST: i64 = 100_000;
/// A context above this is asked for by the suffix `[1m]` on the model's name: some models have 200 000
/// tokens unless the larger window is asked for, and the suffix changes nothing on a model that has it anyway.
pub const ONE_MILLION_ABOVE: i64 = 200_000;
/// The suffix that asks for the larger window.
const ONE_MILLION_SUFFIX: &str = "[1m]";
/// The parameter that names the model of the harness's background work.
pub const SMALL_MODEL: &str = "small_model";
/// The parameter that lets the harness load tool definitions on demand.
pub const TOOL_SEARCH: &str = "tool_search";
/// The parameter that decides whether a page's host is checked with the vendor before the page is fetched.
pub const WEBFETCH_PREFLIGHT: &str = "webfetch_preflight";

/// The reasoning efforts the harness knows. A value outside this list is not sent: the harness refuses to
/// start with one it does not know.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The effort to give the harness: the one asked for when the harness knows it.
#[must_use]
pub fn effort_for_harness(effort: Option<&str>) -> Option<&str> {
    effort.filter(|effort| EFFORTS.contains(effort))
}

/// Whether a page's host is checked with the vendor before the page is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preflight {
    /// Left on when the upstream is the vendor's own, off when it is anybody else's.
    Auto,
    /// Always checked.
    On,
    /// Never checked.
    Off,
}

/// The model as the harness is told its name: with the suffix that asks for the larger window when the
/// context is above [`ONE_MILLION_ABOVE`], and as it is otherwise.
#[must_use]
pub fn model_argument(model: &str, context: Option<i64>) -> String {
    if context.is_some_and(|context| context > ONE_MILLION_ABOVE) {
        format!("{model}{ONE_MILLION_SUFFIX}")
    } else {
        model.to_owned()
    }
}

/// What the harness is told as the size within which it shortens a conversation: the context, as it is. The
/// harness holds the size to the model's window and keeps room for the answer by a margin of its own.
#[must_use]
pub fn auto_compact_window(context: Option<i64>) -> Option<i64> {
    context.filter(|context| *context > 0)
}

/// The window the harness is to assume for a model whose name it does not know (a gateway's own name for a
/// model): the model's own window that its entry gives, or the context where that is the larger, so that the
/// size within which the conversation is shortened is not held to a smaller one.
#[must_use]
pub fn assumed_window(window: Option<i64>, context: Option<i64>) -> Option<i64> {
    [
        window.filter(|window| *window > 0),
        auto_compact_window(context),
    ]
    .into_iter()
    .flatten()
    .max()
}

/// The values of the adapter's parameters that a turn carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Values {
    /// The service tier of the turn; `None` is the ordinary speed.
    pub service_tier: Option<String>,
    /// The context the session is given; `None` leaves the harness to its own window.
    pub context: Option<i64>,
    /// The model's own window, as its entry gives it, for a model whose name the harness may not know.
    pub window: Option<i64>,
    /// The model of background work; `None` keeps it on the thread's model.
    pub small_model: Option<String>,
    /// Whether tool definitions are loaded on demand; `None` leaves it to the harness.
    pub tool_search: Option<bool>,
    /// The page check.
    pub webfetch_preflight: Preflight,
}

impl Values {
    /// The values in the `params` of a session or a turn; a value of the wrong type is ignored.
    #[must_use]
    pub fn from_json(params: &Json) -> Self {
        let number = |key: &str| params.get(key).and_then(Json::as_i64).filter(|n| *n > 0);
        Self {
            service_tier: params
                .get(SERVICE_TIER)
                .and_then(Json::as_str)
                .filter(|tier| !tier.is_empty())
                .map(str::to_owned),
            context: number(CONTEXT),
            window: number(MODEL_WINDOW_KEY),
            small_model: params
                .get(SMALL_MODEL)
                .and_then(Json::as_str)
                .filter(|model| !model.is_empty())
                .map(str::to_owned),
            tool_search: params.get(TOOL_SEARCH).and_then(Json::as_bool),
            webfetch_preflight: match params.get(WEBFETCH_PREFLIGHT).and_then(Json::as_str) {
                Some("on") => Preflight::On,
                Some("off") => Preflight::Off,
                _ => Preflight::Auto,
            },
        }
    }
}

/// The variables that keep every model the harness chooses on the thread's model: the one it uses, the
/// four aliases it resolves, and the sub-agents' (forced, so that a definition that names another model
/// cannot leave the allowlist). Background work goes to `small` when one is named. Where the harness is
/// behind a forwarder this is also what the forwarder checks on the wire.
#[must_use]
pub fn model_environment(model: &str, small: Option<&str>) -> Vec<(&'static str, String)> {
    let background = small.unwrap_or(model);
    vec![
        ("ANTHROPIC_MODEL", model.to_owned()),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", model.to_owned()),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", model.to_owned()),
        ("ANTHROPIC_DEFAULT_FABLE_MODEL", model.to_owned()),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", background.to_owned()),
        ("ANTHROPIC_SMALL_FAST_MODEL", background.to_owned()),
        ("CLAUDE_CODE_SUBAGENT_MODEL", model.to_owned()),
        ("CLAUDE_CODE_SUBAGENT_MODEL_FORCE", "1".to_owned()),
    ]
}

/// The parameters this adapter publishes.
#[must_use]
pub fn definitions() -> Vec<ParameterDef> {
    let scopes = vec![
        ParameterScope::Org,
        ParameterScope::Project,
        ParameterScope::Topic,
        ParameterScope::Thread,
    ];
    vec![
        ParameterDef {
            key: SERVICE_TIER.to_owned(),
            title: "Speed".to_owned(),
            description: "The harness's fast mode, for the models that have one: quicker answers at a higher price a token. On a subscription it draws on usage credits and not on the plan. Only where a manager of the provider enabled it for the model.".to_owned(),
            group: ParameterGroup::Model,
            kind: ParameterType::Enum,
            options: Some(ParameterOptions::FromModel {
                from_model: ModelList::ServiceTiers,
            }),
            range: None,
            default: ParameterDefault::FromModel {
                from_model: DefaultFromModel::DefaultServiceTier,
            },
            scopes: scopes.clone(),
            apply: ParameterApply::NextTurn,
            eligible_when: Some(Condition::ModelHas(ModelFeature::ServiceTiers)),
            feature: None,
        },
        ParameterDef {
            key: CONTEXT.to_owned(),
            title: "Context".to_owned(),
            description: "How much of a conversation the agent keeps, in tokens, up to the largest the model's managers opened. Claude Code shortens the conversation within that size, keeping room for the answer by a margin of its own: at about 967 000 for a context of 1 000 000.".to_owned(),
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
            scopes: scopes.clone(),
            apply: ParameterApply::NextTurn,
            eligible_when: Some(Condition::ModelHas(ModelFeature::ContextMax)),
            feature: None,
        },
        ParameterDef {
            key: SMALL_MODEL.to_owned(),
            title: "Model for background work".to_owned(),
            description: "The allowed model the harness uses for its helper steps; left empty it uses the thread's model.".to_owned(),
            group: ParameterGroup::Model,
            kind: ParameterType::String,
            options: None,
            range: None,
            default: ParameterDefault::Value(Json::Null),
            scopes: scopes.clone(),
            apply: ParameterApply::NewSession,
            eligible_when: None,
            feature: None,
        },
        ParameterDef {
            key: TOOL_SEARCH.to_owned(),
            title: "Load tools on demand".to_owned(),
            description: "Let the harness load the definitions of MCP tools when it needs them instead of all at once. Left unset it stays off behind a forwarder, which is the harness's own choice behind another endpoint, and otherwise on.".to_owned(),
            group: ParameterGroup::Advanced,
            kind: ParameterType::Boolean,
            options: None,
            range: None,
            default: ParameterDefault::Value(Json::Null),
            scopes: scopes.clone(),
            apply: ParameterApply::NewSession,
            eligible_when: None,
            feature: None,
        },
        ParameterDef {
            key: WEBFETCH_PREFLIGHT.to_owned(),
            title: "Check pages with the vendor".to_owned(),
            description: "Before it loads a page, the harness tells the vendor's API the page's host to check it against a blocklist. Automatic keeps that for the vendor's own upstream and turns it off for any other, where it would tell a third party which hosts are fetched. It applies to native WebFetch in either arrangement.".to_owned(),
            group: ParameterGroup::Safety,
            kind: ParameterType::Enum,
            options: Some(ParameterOptions::Listed(vec![
                ParameterOption { value: "auto".to_owned(), label: "Automatic".to_owned(), description: None },
                ParameterOption { value: "on".to_owned(), label: "Always check".to_owned(), description: None },
                ParameterOption { value: "off".to_owned(), label: "Never check".to_owned(), description: None },
            ])),
            range: None,
            default: ParameterDefault::Value(json!("auto")),
            scopes,
            apply: ParameterApply::NewSession,
            eligible_when: None,
            feature: None,
        },
        orch_agent_contract::parameters::managed_web_search_parameter(false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::parameters::check_parameters;

    #[test]
    fn what_the_adapter_publishes_is_consistent() {
        let defs = definitions();
        assert_eq!(check_parameters(&defs), Ok(()));
        let keys: Vec<_> = defs.iter().map(|def| def.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                SERVICE_TIER,
                CONTEXT,
                SMALL_MODEL,
                TOOL_SEARCH,
                WEBFETCH_PREFLIGHT,
                "web_search"
            ]
        );
    }

    #[test]
    fn the_ordinary_search_control_offers_service_without_native_search_and_never_selects_it_by_default()
     {
        use orch_agent_contract::{
            credentials::CredentialPath,
            descriptor::{Placement, ProviderKind},
            parameters::{Eligibility, Resolution},
        };
        let search =
            crate::descriptor::descriptor(crate::config::PORTABLE_ARCHIVE_CANDIDATE, false)
                .parameters
                .into_iter()
                .find(|definition| definition.key == "web_search")
                .unwrap();
        for model in [
            json!({}),
            json!({"offered":{"web_search":["live"]},"request_options":{
                "native":{"type":"object","properties":{"web_search":false}}
            }}),
        ] {
            for (credential_path, provider_kind) in [
                (CredentialPath::Native, ProviderKind::Subscription),
                (CredentialPath::Forwarded, ProviderKind::Api),
            ] {
                let context = Eligibility {
                    credential_path,
                    provider_kind,
                    environment_placement: Placement::Managed,
                    model: serde_json::from_value(model.clone()).unwrap(),
                    harness_version: Some(crate::config::PORTABLE_ARCHIVE_CANDIDATE.into()),
                };
                let features = std::collections::BTreeMap::new();
                assert!(search.is_shown(&context, &features));
                assert!(
                    search
                        .options_for(&context.model)
                        .iter()
                        .any(|option| option.value == "service")
                );
                assert_eq!(
                    search.resolve(&context, &features, None),
                    Resolution::SendNothing
                );
                assert_eq!(
                    search.resolve(&context, &features, Some(&json!("service"))),
                    Resolution::Send(json!("service"))
                );
                let projection = search.projection(&context.model);
                assert_eq!(projection["capability"], "web_search");
                assert_eq!(projection["resolved_default"], Json::Null);
                assert_eq!(search.apply, ParameterApply::NewSession);
            }
        }
    }

    #[test]
    fn managed_search_keeps_native_defaults_and_cannot_leave_a_fixed_native_search_running() {
        use orch_agent_contract::parameters::{ModelParameters, Resolution};

        let search = definitions()
            .into_iter()
            .find(|definition| definition.key == "web_search")
            .unwrap();
        let native: ModelParameters = serde_json::from_value(json!({
            "web_search":["cached","indexed","live"],"default_web_search":"live"
        }))
        .unwrap();
        assert_eq!(search.start_for(&native), Some(json!("live")));
        assert_eq!(
            search
                .options_for(&native)
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["disabled", "live", "service"]
        );
        let fixed: ModelParameters = serde_json::from_value(json!({
            "web_search":["live"],"request_options":{"native":{"type":"object",
                "properties":{"web_search":{"const":"live"}}
            }}
        }))
        .unwrap();
        assert_eq!(
            search.resolve_value(&fixed, Some(&json!("service"))),
            Resolution::Stale(json!("service"))
        );
    }

    #[test]
    fn a_literal_native_profile_search_key_still_accepts_only_native_values() {
        use orch_agent_contract::{
            parameters::{ModelParameters, web_search_parameter},
            profile_parameters::{Binding, ProfileParameters},
        };
        use std::collections::BTreeMap;

        let native = ProfileParameters {
            version: 1,
            bindings: vec![Binding {
                pointer: "/web_search".into(),
                parameter: web_search_parameter(false),
                values: BTreeMap::new(),
            }],
        };
        let model: ModelParameters = serde_json::from_value(json!({
            "web_search":["live"],"default_web_search":"live"
        }))
        .unwrap();
        for value in ["disabled", "live"] {
            assert_eq!(
                native
                    .validate(&json!({"web_search":value}), &model, &BTreeMap::new())
                    .unwrap()
                    .get("web_search"),
                Some(&json!(value))
            );
        }
        for value in ["service", "cached", "indexed"] {
            assert!(
                native
                    .validate(&json!({"web_search":value}), &model, &BTreeMap::new())
                    .is_err()
            );
        }
    }

    #[test]
    fn forwarded_speed_uses_only_the_model_tiers_a_manager_enabled() {
        use orch_agent_contract::{
            credentials::CredentialPath,
            descriptor::{Placement, ProviderKind},
            parameters::{Eligibility, Resolution},
        };
        let speed = definitions()
            .into_iter()
            .find(|def| def.key == SERVICE_TIER)
            .unwrap();
        let mut ctx = Eligibility {
            credential_path: CredentialPath::Forwarded,
            provider_kind: ProviderKind::Api,
            environment_placement: Placement::Managed,
            model: serde_json::from_value(json!({
                "service_tiers":[{"id":"fast","label":"Fast"}],
                "offered":{"service_tiers":[{"id":"fast","label":"Fast"},{"id":"flex","label":"Flex"}]}
            })).unwrap(),
            harness_version: Some(crate::config::PORTABLE_ARCHIVE_CANDIDATE.into()),
        };
        assert_eq!(
            speed.resolve(
                &ctx,
                &std::collections::BTreeMap::new(),
                Some(&json!("fast"))
            ),
            Resolution::Send(json!("fast"))
        );
        assert_eq!(
            speed.resolve(
                &ctx,
                &std::collections::BTreeMap::new(),
                Some(&json!("flex"))
            ),
            Resolution::Stale(json!("flex"))
        );
        ctx.model = serde_json::from_value(
            json!({"offered":{"service_tiers":[{"id":"fast","label":"Fast"}]}}),
        )
        .unwrap();
        assert_eq!(
            speed.resolve(
                &ctx,
                &std::collections::BTreeMap::new(),
                Some(&json!("fast"))
            ),
            Resolution::Hidden
        );
    }

    #[test]
    fn the_speed_and_the_context_are_read_from_the_params_as_they_are_sent() {
        let values = Values::from_json(&json!({
            "service_tier": "fast", "context": 1_000_000, "context_window": 200_000
        }));
        assert_eq!(values.service_tier.as_deref(), Some(FAST_TIER));
        assert_eq!(values.context, Some(1_000_000));
        assert_eq!(values.window, Some(200_000));
        let none = Values::from_json(
            &json!({ "service_tier": "", "context": -1, "context_window": "big" }),
        );
        assert_eq!(
            (none.service_tier, none.context, none.window),
            (None, None, None)
        );
        // The context is a value the process is started with: a change of it is a change that needs a restart.
        assert_ne!(
            Values::from_json(&json!({ "context": 200_000 })),
            Values::from_json(&json!({ "context": 300_000 }))
        );
        assert_ne!(
            Values::from_json(&json!({})),
            Values::from_json(&json!({ "service_tier": "fast" }))
        );
    }

    #[test]
    fn the_larger_window_is_asked_for_by_the_name_above_200000_and_the_assumed_window_is_the_larger_of_two()
     {
        assert_eq!(
            model_argument("claude-opus-5", Some(1_000_000)),
            "claude-opus-5[1m]"
        );
        assert_eq!(
            model_argument("claude-opus-5", Some(200_001)),
            "claude-opus-5[1m]"
        );
        assert_eq!(
            model_argument("claude-opus-5", Some(200_000)),
            "claude-opus-5"
        );
        assert_eq!(model_argument("claude-opus-5", None), "claude-opus-5");
        assert_eq!(auto_compact_window(Some(900_000)), Some(900_000));
        assert_eq!(auto_compact_window(Some(0)), None);
        assert_eq!(assumed_window(Some(128_000), Some(400_000)), Some(400_000));
        assert_eq!(assumed_window(Some(128_000), Some(100_000)), Some(128_000));
        assert_eq!(assumed_window(None, Some(100_000)), Some(100_000));
        assert_eq!(assumed_window(None, None), None);
    }

    #[test]
    fn values_are_read_from_the_params_and_a_wrong_type_is_left_out() {
        let values = Values::from_json(&json!({
            "small_model": "claude-haiku-5.5", "tool_search": true, "webfetch_preflight": "off"
        }));
        assert_eq!(values.small_model.as_deref(), Some("claude-haiku-5.5"));
        assert_eq!(values.tool_search, Some(true));
        assert_eq!(values.webfetch_preflight, Preflight::Off);
        let none = Values::from_json(
            &json!({ "small_model": 7, "tool_search": "yes", "webfetch_preflight": "maybe" }),
        );
        assert_eq!(none.small_model, None);
        assert_eq!(none.tool_search, None);
        assert_eq!(none.webfetch_preflight, Preflight::Auto);
    }

    #[test]
    fn every_model_the_harness_chooses_is_kept_on_the_threads_model_unless_background_work_has_one()
    {
        let kept = model_environment("gpt-x", None);
        assert!(
            kept.iter()
                .all(
                    |(name, value)| *name == "CLAUDE_CODE_SUBAGENT_MODEL_FORCE" && value == "1"
                        || value == "gpt-x"
                )
        );
        let with_small = model_environment("gpt-x", Some("gpt-x-mini"));
        let haiku = with_small
            .iter()
            .find(|(name, _)| *name == "ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .unwrap();
        assert_eq!(haiku.1, "gpt-x-mini");
        let sonnet = with_small
            .iter()
            .find(|(name, _)| *name == "ANTHROPIC_DEFAULT_SONNET_MODEL")
            .unwrap();
        assert_eq!(sonnet.1, "gpt-x");
    }

    #[test]
    fn an_effort_the_harness_does_not_know_is_not_sent() {
        assert_eq!(effort_for_harness(Some("xhigh")), Some("xhigh"));
        assert_eq!(effort_for_harness(Some("ludicrous")), None);
        assert_eq!(effort_for_harness(None), None);
    }
}
