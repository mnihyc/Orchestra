//! Projection of existing admitted thread/provider/model facts into the single capability resolver.
//! These previews never admit an effect; each executor retains its existing live permission and usage path.
use crate::access::ProviderUse;
use crate::{Engine, EngineError};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_api_contract::resources::Provider as ProviderView;
use orch_db::Conn;
use orch_db::queries::resources::ProviderModelRow;
use orch_provider_contract::capabilities::{
    self as cap, Availability, Endpoint, Offering, OfferingRef, Price, Provider, Registry,
    Resolution, Support, ThreadContext,
};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::BTreeMap;
mod backends;

/// Existing prepared projections supply metadata; absence stays hidden and is never replaced by another read.
pub struct ThreadProjection<'a> {
    /// Actual program descriptor, including extension-owned per-access facts.
    pub descriptor: &'a AgentDescriptor,
    /// Actual serialized credential path, independently of the provider's kind.
    pub access: &'a str,
    /// Actual selected arrangement, where the existing binding reader supplied it.
    pub arrangement: Option<&'a str>,
    /// Provider identity already bound to this thread.
    pub provider_id: Id,
    /// Same-operation admitted optional provider projection.
    pub provider: Option<&'a ProviderView>,
    /// Original model rows, preserving future members without a storage rewrite.
    pub models: &'a [ProviderModelRow],
    /// Current model resolved by the ordinary settings supplier.
    pub current: Option<&'a str>,
    /// Actual current Use/allowance answer from the existing resource-access supplier.
    pub allowed: bool,
    /// Already resolved setting values; settings whose value is not visible are absent.
    pub settings: BTreeMap<String, Json>,
}

/// Existing current readers and factual billing context for one backend preview.
/// Names/prices may come only from the same operation's admitted optional provider Read.
pub struct BackendRead<'a> {
    pub place: Id,
    pub principals: &'a [orch_authz::Principal],
    pub person: Option<Id>,
    pub harness_kind: Option<&'a str>,
    pub provider: Option<&'a ProviderView>,
}

/// A disposable snapshot, recomputed on each read or admission boundary, never a permission cache.
#[derive(Debug)]
pub struct Snapshot {
    /// Extensions may add their own interfaces, routes, offerings and installed execution ports here.
    pub registry: Registry,
    /// Actual current inputs for this one thread.
    pub context: ThreadContext,
}
impl Snapshot {
    /// Dispatch planning and inspector projection share the contract's exact resolver.
    #[must_use]
    pub fn resolve(&self) -> Vec<Resolution> {
        self.registry.resolve(&self.context)
    }
}

/// Existing setting roots needed by both the inspector and native session capture.
/// Unknown extension settings remain absent until their ordinary setting supplier is installed.
pub fn setting_definitions(
    sources: &orch_provider_contract::builtins::BuiltinSources,
) -> Vec<&'static orch_settings_contract::registry::SettingDef> {
    use orch_settings_contract::registry;
    let mut definitions = BTreeMap::new();
    for capability in sources.capabilities().declarations().capabilities.values() {
        for key in [
            capability.mode_setting.as_deref(),
            capability.backend_setting.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(def) = registry::all()
                .iter()
                .filter(|def| {
                    key == def.key
                        || key
                            .strip_prefix(def.key)
                            .is_some_and(|tail| tail.starts_with('.'))
                })
                .max_by_key(|def| def.key.len())
            {
                definitions.insert(def.key, def);
            }
        }
    }
    if let Some(def) = registry::get(registry::keys::CALLS_RULES) {
        definitions.insert(def.key, def);
    }
    definitions.into_values().collect()
}

/// Read a nested declared setting from the existing resolved setting map.
/// The longest present key wins, so dots inside a setting key are not assumed to be JSON members.
pub(crate) fn setting<'a>(values: &'a BTreeMap<String, Json>, name: &str) -> Option<&'a Json> {
    if let Some(value) = values.get(name) {
        return Some(value);
    }
    values
        .iter()
        .filter_map(|(key, value)| {
            let tail = name.strip_prefix(key)?.strip_prefix('.')?;
            let found = value
                .get(tail)
                .or_else(|| tail.split('.').try_fold(value, |at, member| at.get(member)))?;
            Some((key.len(), found))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, value)| value)
}

/// Project current native facts. Paid routes remain absent until an explicit admitted backend and its
/// installed executor are supplied by the owning capability package.
pub fn project(input: ThreadProjection<'_>) -> Result<Snapshot, EngineError> {
    project_with_sources(
        input,
        &orch_provider_contract::builtins::BuiltinSources::default(),
    )
}

/// Actual installed provider mappings join the same native snapshot; service execution is supplied separately.
pub fn project_with_sources(
    input: ThreadProjection<'_>,
    sources: &orch_provider_contract::builtins::BuiltinSources,
) -> Result<Snapshot, EngineError> {
    let account = input.provider.map(AccountProjection::from_view);
    project_with_account(input, sources, account)
}

/// Private source facts for dispatch; this type is never a public provider view or permission.
#[derive(Debug, Default)]
pub(crate) struct AccountProjection {
    pub account: Provider,
    pub credential_roles: Vec<String>,
    pub facts: Json,
}

impl AccountProjection {
    fn from_view(provider: &ProviderView) -> Self {
        let mut account = Provider {
            title: provider.base.name.clone(),
            ..Provider::default()
        };
        for upstream in &provider.upstreams {
            let id = upstream.protocol.clone();
            account
                .endpoints
                .entry(id.clone())
                .or_insert_with(|| Endpoint {
                    interface: id,
                    address: upstream.base_url.clone(),
                    credential_role: (upstream.options.get("auth").and_then(Json::as_str)
                        != Some("none"))
                    .then(|| "model".into()),
                    ..Endpoint::default()
                });
        }
        Self {
            account,
            credential_roles: if provider.key_set {
                vec!["model".into()]
            } else {
                Vec::new()
            },
            facts: json!({
                "plan": provider.facts.plan,
                "credential": provider.facts.credential,
                "credential_verified": provider.facts.credential_verified,
            }),
        }
    }
}

/// Inspectors and dispatch use one projection and one resolver. Private dispatch facts do not
/// grant optional public provider visibility or replace the ordinary effect guards.
pub(crate) fn project_with_account(
    input: ThreadProjection<'_>,
    sources: &orch_provider_contract::builtins::BuiltinSources,
    account: Option<AccountProjection>,
) -> Result<Snapshot, EngineError> {
    let mut registry = Registry::default();
    registry
        .register("common", cap::catalogue().map_err(EngineError::unreadable)?)
        .map_err(EngineError::unreadable)?;
    registry
        .register(
            "installed-providers",
            sources.capabilities().declarations().clone(),
        )
        .map_err(EngineError::unreadable)?;
    let mut program = input.descriptor.capability_program();
    if let Some(row) = input
        .models
        .iter()
        .find(|row| input.current == Some(row.model_name.as_str()))
    {
        let model: orch_provider_contract::models::ModelParameters =
            serde_json::from_value(row.parameters.clone()).unwrap_or_default();
        for (id, capability) in &registry.declarations().capabilities {
            let Some(key) = capability
                .mode_setting
                .as_deref()
                .and_then(|key| key.strip_prefix("agent.params."))
            else {
                continue;
            };
            let Some(def) = input
                .descriptor
                .parameters
                .iter()
                .find(|def| def.key == key)
            else {
                continue;
            };
            let choices = def.options_for(&model);
            if let Some(support) = program
                .access
                .get_mut(input.access)
                .and_then(|access| access.native.get_mut(id))
            {
                support
                    .modes
                    .retain(|mode| choices.iter().any(|choice| &choice.value == mode));
            }
        }
    }
    let mut declaration = cap::Declarations::default();
    declaration
        .programs
        .insert(input.descriptor.kind.clone(), program.clone());
    let account = account.unwrap_or_default();
    let mut provider_account = account.account;
    let mut context = ThreadContext {
        program: input.descriptor.kind.clone(),
        access: input.access.into(),
        current: input.current.map(|offering| OfferingRef {
            provider: input.provider_id,
            offering: offering.into(),
        }),
        ..ThreadContext::default()
    };
    context.facts = json!({"arrangement":input.arrangement});
    if !account.credential_roles.is_empty() {
        context
            .credential_roles
            .insert(input.provider_id, account.credential_roles);
    }
    if let Some(facts) = account.facts.as_object() {
        for (key, value) in facts {
            context.facts[key] = value.clone();
        }
    }
    let endpoint = program
        .access
        .get(input.access)
        .and_then(|access| {
            access
                .interfaces
                .iter()
                .find(|id| provider_account.endpoints.contains_key(*id))
                .cloned()
        })
        .unwrap_or_default();
    // These wire implementations are shipped in this application. A service declaration alone is not installation.
    context.interfaces = orch_model_contract::protocol::SUPPORTED
        .iter()
        .map(|name| (*name).into())
        .collect();
    for row in input.models {
        let mut offering = Offering {
            title: row.display_name.clone(),
            endpoint: endpoint.clone(),
            kind: "model".into(),
            enabled: row.enabled,
            groups: row.groups.clone(),
            parameters: row.parameters.clone(),
            ..Offering::default()
        };
        if let Some(actual) = provider_account.endpoints.get(&endpoint)
            && let Ok(base) = url::Url::parse(&actual.address)
        {
            let projected =
                sources.endpoint_capabilities(&actual.interface, &base, &row.model_name);
            offering.native_tools = projected.native_tools;
            offering.native_switches = projected.native_switches;
            if !projected.model_metadata.is_empty() {
                offering.extensions.insert(
                    "model_metadata".into(),
                    serde_json::to_value(projected.model_metadata)?,
                );
            }
            // Service entries remain discovery-only here. Their manager/enforcement producer supplies enabled facts.
            provider_account.endpoints.extend(projected.endpoints);
        }
        offering.capabilities.insert(
            "generation".into(),
            Support {
                enabled: row.enabled,
                modes: vec!["native".into()],
                ..Support::default()
            },
        );
        for (field, unit) in [
            ("web_search_per_call", "search"),
            ("web_fetch_per_call", "page"),
        ] {
            if let Some(rate) = row
                .parameters
                .get("rate")
                .and_then(|rate| rate.get(field))
                .and_then(Json::as_str)
            {
                offering.prices.insert(
                    unit.into(),
                    Price {
                        amount: rate.into(),
                        currency: "credits".into(),
                        unit: unit.into(),
                        ..Price::default()
                    },
                );
            }
        }
        if input.current == Some(row.model_name.as_str()) {
            context
                .availability
                .entry(input.provider_id)
                .or_default()
                .insert(
                    row.model_name.clone(),
                    Availability {
                        allowed: input.allowed,
                        show_provider: input.provider.is_some(),
                        show_offering: input.provider.is_some(),
                        show_price: input.provider.is_some(),
                        ..Availability::default()
                    },
                );
        }
        provider_account
            .offerings
            .insert(row.model_name.clone(), offering);
    }
    declaration
        .providers
        .insert(input.provider_id, provider_account);
    for (id, capability) in &registry.declarations().capabilities {
        if let Some(value) = capability
            .mode_setting
            .as_deref()
            .and_then(|key| setting(&input.settings, key))
        {
            // Invalid retained choices stay unavailable instead of silently using a paid or native default.
            if !value.is_null() {
                context
                    .choices
                    .insert(id.clone(), Some(value.as_str().unwrap_or("invalid").into()));
            }
        }
    }
    if let Some(rules) = input.settings.get("calls.rules") {
        context.recovery = Some(cap::RecoveryPolicy {
            rules: orch_provider_contract::calls::validate(rules)
                .map_err(EngineError::unreadable)?,
            scope: input
                .settings
                .get("calls.rules.source_scope")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .into(),
        });
    }
    registry
        .register("thread", declaration)
        .map_err(EngineError::unreadable)?;
    Ok(Snapshot { registry, context })
}

impl Engine {
    /// Ask the SAME supplier used by ordinary model admission, on the caller's current read snapshot.
    /// The API still retains its original caller restrictions; this returns a preview, never admission.
    pub async fn capability_use_preview(
        &self,
        conn: &mut Conn,
        ask: &ProviderUse<'_>,
    ) -> Result<bool, EngineError> {
        Ok(self
            .inner
            .access
            .provider_use(conn, ask)
            .await?
            .is_allowed())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    #[test]
    fn installed_capability_selectors_are_read_by_both_native_capture_and_inspection() {
        use orch_provider_contract::builtins::BuiltinSources;
        use orch_settings_contract::registry::keys;
        let mut sources = BuiltinSources::default();
        sources
            .register_capabilities(
                "fixture-control",
                serde_json::from_value(json!({
                    "capabilities": {
                        "fixture-control": {
                            "mode_setting":"threads.background_wake",
                            "backend_setting":"threads.agent_schedules"
                        },
                        "fixture-unknown": {
                            "mode_setting":"agent.params_sibling.value",
                            "backend_setting":"fixture.uninstalled.setting"
                        }
                    }
                }))
                .unwrap(),
            )
            .unwrap();
        let definitions = setting_definitions(&sources);
        let keys: Vec<_> = definitions
            .iter()
            .map(|definition| definition.key)
            .collect();
        assert!(keys.contains(&keys::AGENT_PARAMS));
        assert!(keys.contains(&keys::AGENTS_WEB_SEARCH_BACKEND));
        assert!(keys.contains(&keys::AGENTS_IMAGE_GENERATION_BACKEND));
        assert!(keys.contains(&keys::THREADS_BACKGROUND_WAKE));
        assert!(keys.contains(&keys::THREADS_AGENT_SCHEDULES));
        assert!(keys.contains(&keys::CALLS_RULES));
        assert!(!keys.contains(&"agent.params_sibling.value"));
        assert!(!keys.contains(&"fixture.uninstalled.setting"));
        assert_eq!(
            keys.iter()
                .filter(|key| **key == keys::AGENT_PARAMS)
                .count(),
            1
        );
    }

    #[test]
    fn dotted_settings_keep_exact_keys_and_a_nested_mode_without_reinterpreting_model_names() {
        let values = BTreeMap::from([
            (
                "agent.params".into(),
                json!({"web_search":"service","literal.with.dots":"on"}),
            ),
            ("agent.model".into(), json!("arbitrary-model")),
        ]);
        assert_eq!(
            setting(&values, "agent.params.web_search"),
            Some(&json!("service"))
        );
        assert_eq!(
            setting(&values, "agent.model"),
            Some(&json!("arbitrary-model"))
        );
        assert_eq!(
            setting(&values, "agent.params.literal.with.dots"),
            Some(&json!("on"))
        );
        assert!(setting(&values, "agents.page_fetch").is_none());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod projection_tests {
    use super::*;

    #[test]
    fn provider_declared_native_predicates_reach_the_carrier_for_each_actual_model_without_provider_read()
     {
        let descriptor: AgentDescriptor = serde_json::from_value(json!({
            "kind":"fixture-program","adapter_version":1,"harness":{"name":"Fixture","version":"1.0.0","supported_range":"=1.0.0","status":"supported"},"arrangements":[],
            "x_capability_program":{"access":{"forwarded":{"interfaces":["openai-responses"],"native":{"generation":{"enabled":true,"modes":["native"],"default_mode":"native"}},"orch_tools":true}},
                "native_switches":{"native_feature":{"capability":"generation","value":false,"values":{"yes":true,"no":false},"provider_overrides":true}}}
        })).unwrap();
        let provider = orch_types::new_id();
        let models = [1000, 999]
            .into_iter()
            .enumerate()
            .map(|(index, window)| ProviderModelRow {
                model_name: format!("private-model-{index}"),
                display_name: "Fixture model".into(),
                parameters: json!({"context_window":window,"future_private":"not-in-carrier"}),
                option_layers: Json::Null,
                groups: Vec::new(),
                enabled: true,
            })
            .collect::<Vec<_>>();
        let original = models[0].parameters.clone();
        let mut sources = orch_provider_contract::builtins::BuiltinSources::default();
        sources.register_capabilities("fixture-native", serde_json::from_value(json!({"endpoint_rules":[{
            "when":{"required":["host"],"properties":{"host":{"const":"private.provider.example.invalid"}}},
            "native_switches":{"fixture-program":{"native_feature":{"capability":"generation","value":true,"when":{
                "required":["offering","endpoint_facts"],"properties":{
                    "offering":{"required":["parameters"],"properties":{"parameters":{"required":["context_window"],"properties":{"context_window":{"const":1000}}}}},
                    "endpoint_facts":{"required":["host"],"properties":{"host":{"const":"private.provider.example.invalid"}}}
                }}}}}
        }]})).unwrap()).unwrap();
        for (index, expected) in [(0, true), (1, false)] {
            let snapshot = project_with_account(
                ThreadProjection {
                    descriptor: &descriptor,
                    access: "forwarded",
                    arrangement: Some("key_separated"),
                    provider_id: provider,
                    provider: None,
                    models: &models,
                    current: Some(&models[index].model_name),
                    allowed: true,
                    settings: BTreeMap::new(),
                },
                &sources,
                Some(AccountProjection {
                    account: Provider {
                        endpoints: BTreeMap::from([(
                            "openai-responses".into(),
                            Endpoint {
                                interface: "openai-responses".into(),
                                address: "https://private.provider.example.invalid/v1".into(),
                                credential_role: Some("model".into()),
                                ..Endpoint::default()
                            },
                        )]),
                        ..Provider::default()
                    },
                    credential_roles: vec!["model".into()],
                    ..AccountProjection::default()
                }),
            )
            .unwrap();
            let resolutions = snapshot.resolve();
            let generation = resolutions
                .iter()
                .find(|result| result.capability == "generation")
                .unwrap();
            assert_eq!(generation.native_config["native_feature"], expected);
            let encoded = serde_json::to_string(generation).unwrap();
            assert!(
                !encoded.contains("private.provider")
                    && !encoded.contains("private-model")
                    && !encoded.contains("not-in-carrier")
                    && !encoded.contains(&provider.to_string())
            );
        }
        assert_eq!(models[0].parameters, original);
    }
    #[test]
    fn the_actual_program_access_and_unchanged_model_rows_supply_the_same_inspector_and_matrix() {
        let descriptor: AgentDescriptor = serde_json::from_value(json!({
            "kind":"fixture-program", "adapter_version":1,
            "harness":{"name":"Fixture program","version":"1.0.0","supported_range":"=1.0.0","status":"supported"},
            "arrangements":[],
            "x_capability_program":{"title":"Fixture program","facts":[{"id":"actual-location","capabilities":["generation"],"when":{"required":["arrangement"],"properties":{"arrangement":{"const":"separated"}}},"text":{"en":"Runs beside the provider"}}],"access":{"native":{"native":{"generation":{"enabled":true,"modes":["native"],"default_mode":"native"},"web_search":{"enabled":true,"modes":["live"],"default_mode":"disabled"}},"orch_tools":true}}}
        })).unwrap();
        let provider = orch_types::new_id();
        let models = vec![ProviderModelRow {
            model_name: "opaque-name".into(),
            display_name: "Fixture model".into(),
            parameters: json!({"future_member":{"kept":true},"rate":{"web_search_per_call":"0.75"}}),
            option_layers: Json::Null,
            groups: vec![],
            enabled: true,
        }];
        let before = models[0].parameters.clone();
        let snapshot = project(ThreadProjection {
            descriptor: &descriptor,
            access: "native",
            arrangement: Some("separated"),
            provider_id: provider,
            provider: None,
            models: &models,
            current: Some("opaque-name"),
            allowed: true,
            settings: BTreeMap::from([("agent.params".into(), json!({"web_search":"service"}))]),
        })
        .unwrap();
        let routes = snapshot.resolve();
        assert_eq!(snapshot.context.facts["arrangement"], "separated");
        let generation = routes
            .iter()
            .find(|route| route.capability == "generation")
            .unwrap();
        assert_eq!(
            generation.route_view.as_ref().unwrap().kind,
            "native-program"
        );
        assert_eq!(
            generation.facts[0].id, "actual-location",
            "the selected binding's arrangement survives even without optional provider Read"
        );
        assert!(
            generation.provider.is_none()
                && generation.offering.is_none()
                && generation.prices.is_empty()
        );
        let search = routes
            .iter()
            .find(|route| route.capability == "web_search")
            .unwrap();
        assert!(search.route.is_none());
        assert!(
            !search.modes.iter().any(|mode| mode == "service"),
            "a declared route is not an installed paid executor"
        );
        assert_eq!(models[0].parameters, before);
        let cells = snapshot.registry.matrix([&snapshot.context]);
        assert_eq!(
            cells
                .iter()
                .find(|cell| cell.resolution.capability == "generation")
                .unwrap()
                .resolution,
            *generation
        );
        let mut another_access = snapshot.context.clone();
        another_access.access = "forwarded".into();
        let unavailable = snapshot.registry.resolve(&another_access);
        assert!(
            unavailable
                .iter()
                .find(|route| route.capability == "generation")
                .unwrap()
                .route
                .is_none()
        );
        let mut no_use = snapshot.context.clone();
        no_use
            .availability
            .get_mut(&provider)
            .unwrap()
            .get_mut("opaque-name")
            .unwrap()
            .allowed = false;
        assert_eq!(
            snapshot
                .registry
                .resolve(&no_use)
                .iter()
                .find(|route| route.capability == "generation")
                .unwrap()
                .reason
                .as_deref(),
            Some("current_authority_refused")
        );
    }
}
