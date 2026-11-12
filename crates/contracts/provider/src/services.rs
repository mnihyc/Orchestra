//! Provider-neutral web backends and API service declarations; credentials remain at their keeper.
use crate::{SourceError, SourceErrorCode};
use async_trait::async_trait;
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Declaration implementations carried by the installed JSON service transport.
#[must_use]
pub fn json_implementation(implementation: &str) -> bool {
    matches!(implementation, "http-json" | "image-api")
}

/// A manager's choices for one extension-declared service. Discovery never enables it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ServiceConfig {
    /// Explicit opt-in; absent configuration and absent flags stay disabled.
    pub enabled: bool,
    /// The provider's own groups, matched by the ordinary offering-pattern matcher.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<String>>,
    /// Explicit prices replace catalogue estimates for these exact charged units.
    pub prices: BTreeMap<String, crate::capabilities::Price>,
    /// Future configuration is retained without becoming endpoint, capability or credential authority.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Json>,
}

/// The current account's declared services after applying its manager's explicit choices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceCatalogue {
    /// Provider-owned service endpoints, never agent-supplied URLs or credentials.
    pub endpoints: BTreeMap<String, crate::capabilities::Endpoint>,
    /// The same offering vocabulary consumed by the common capability resolver.
    pub offerings: BTreeMap<String, crate::capabilities::Offering>,
    /// Units priced explicitly by the provider's manager, independently of catalogue estimates.
    pub explicit_prices: BTreeMap<String, Vec<String>>,
}

/// The exact credential-free endpoint and codec a keeping runtime must share with the issuer.
pub fn definition_hash(
    endpoint: &crate::capabilities::Endpoint,
    binding: &crate::capabilities::Binding,
) -> Result<orch_types::Hash, serde_json::Error> {
    serde_json::to_vec(&serde_json::json!({"endpoint":endpoint,"binding":binding}))
        .map(|bytes| orch_types::Hash::of(&bytes))
}

/// A service offering may refine its fixed codec without changing the common HTTP adapter.
/// An explicit closed member remains closed; manager configuration never supplies these bindings.
#[must_use]
pub fn service_binding<'a>(
    declarations: &'a crate::capabilities::Declarations,
    endpoint: &'a crate::capabilities::Endpoint,
    offering: &'a crate::capabilities::Offering,
    capability: &str,
) -> Option<&'a crate::capabilities::Binding> {
    if let Some(binding) = offering.native_tools.get(capability) {
        return binding.as_ref();
    }
    if let Some(binding) = endpoint.native_tools.get(capability) {
        return binding.as_ref();
    }
    Some(&declarations.interfaces.get(&endpoint.interface)?.shape)
}

/// Select a native binding from installed interface/endpoint declarations without enabling a model.
/// An explicit closed member in the stored ceiling remains closed.
#[must_use]
pub fn native_binding(
    declarations: &crate::capabilities::Declarations,
    endpoint: &crate::capabilities::Endpoint,
    model: &str,
    ceiling: &crate::models::ModelParameters,
    capability: &str,
) -> Option<crate::capabilities::Binding> {
    if ceiling
        .native_tools
        .get(&endpoint.interface)
        .is_some_and(|tools| tools.get(capability) == Some(&None))
    {
        return None;
    }
    let address = url::Url::parse(&endpoint.address).ok()?;
    let projected =
        crate::capabilities::project_endpoint(declarations, &endpoint.interface, &address, model);
    match projected.native_tools.get(capability) {
        Some(binding) => binding.clone(),
        None => declarations
            .interfaces
            .get(&endpoint.interface)?
            .native_tools
            .get(capability)
            .cloned(),
    }
}

/// Read declarations against actual configured upstreams, then overlay only manager-owned service fields.
/// Unknown stored configurations are preserved by their owner but never create undeclared offerings.
#[must_use]
pub fn catalogue(
    declarations: &crate::capabilities::Declarations,
    upstreams: &BTreeMap<String, crate::capabilities::Endpoint>,
    configured: &BTreeMap<String, ServiceConfig>,
) -> ServiceCatalogue {
    let mut catalogue = ServiceCatalogue::default();
    for endpoint in upstreams.values() {
        let Ok(address) = url::Url::parse(&endpoint.address) else {
            continue;
        };
        let projected =
            crate::capabilities::project_endpoint(declarations, &endpoint.interface, &address, "");
        for (id, service) in projected.offerings {
            if service.kind != "service" || catalogue.offerings.contains_key(&id) {
                continue;
            }
            let mut service = service;
            for (program, switches) in &projected.native_switches {
                service
                    .native_switches
                    .entry(program.clone())
                    .or_default()
                    .extend(switches.clone());
            }
            service.enabled = configured.get(&id).is_some_and(|choice| choice.enabled);
            if let Some(choice) = configured.get(&id) {
                if let Some(groups) = &choice.groups {
                    service.groups = groups.clone();
                }
                service.prices.extend(choice.prices.clone());
            }
            if let Some(endpoint) = projected.endpoints.get(&service.endpoint) {
                if let Some(choice) = configured
                    .get(&id)
                    .filter(|choice| !choice.prices.is_empty())
                {
                    catalogue
                        .explicit_prices
                        .insert(id.clone(), choice.prices.keys().cloned().collect());
                }
                catalogue
                    .endpoints
                    .insert(service.endpoint.clone(), endpoint.clone());
                catalogue.offerings.insert(id, service);
            }
        }
    }
    catalogue
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod service_config_tests {
    use super::*;
    use crate::capabilities::{Declarations, Endpoint, EndpointRule, Offering, Price, Support};
    use serde_json::json;

    #[test]
    fn per_offering_service_codecs_override_the_endpoint_and_keep_explicit_closure() {
        use crate::capabilities::{Binding, Interface};
        let base = Binding {
            request: json!({"body":{"model":"base"}}),
            ..Binding::default()
        };
        let narrower = Binding {
            request: json!({"body":{"model":"selected-offering"}}),
            ..Binding::default()
        };
        let declarations = Declarations {
            interfaces: BTreeMap::from([(
                "fixture".into(),
                Interface {
                    shape: base.clone(),
                    ..Interface::default()
                },
            )]),
            ..Declarations::default()
        };
        let mut endpoint = Endpoint {
            interface: "fixture".into(),
            ..Endpoint::default()
        };
        let mut offering = Offering::default();
        assert_eq!(
            service_binding(&declarations, &endpoint, &offering, "image_generation"),
            Some(&base)
        );
        endpoint
            .native_tools
            .insert("image_generation".into(), Some(narrower.clone()));
        assert_eq!(
            service_binding(&declarations, &endpoint, &offering, "image_generation"),
            Some(&narrower)
        );
        offering
            .native_tools
            .insert("image_generation".into(), Some(base.clone()));
        assert_eq!(
            service_binding(&declarations, &endpoint, &offering, "image_generation"),
            Some(&base)
        );
        offering
            .native_tools
            .insert("image_generation".into(), None);
        assert!(service_binding(&declarations, &endpoint, &offering, "image_generation").is_none());
        offering.native_tools.clear();
        endpoint
            .native_tools
            .insert("image_generation".into(), None);
        assert!(service_binding(&declarations, &endpoint, &offering, "image_generation").is_none());
    }

    #[test]
    fn manager_choices_only_change_declared_service_enablement_groups_and_prices() {
        let declarations = Declarations {
            endpoint_rules: vec![EndpointRule {
                when: json!({"required":["host"],"properties":{"host":{"const":"provider.example.invalid"}}}),
                endpoints: BTreeMap::from([(
                    "service-endpoint".into(),
                    Endpoint {
                        address: "https://provider.example.invalid/tools".into(),
                        interface: "fixture-http".into(),
                        ..Endpoint::default()
                    },
                )]),
                offerings: BTreeMap::from([(
                    "fixture-search".into(),
                    Offering {
                        endpoint: "service-endpoint".into(),
                        kind: "service".into(),
                        enabled: true,
                        groups: vec!["web".into()],
                        capabilities: BTreeMap::from([(
                            "web_search".into(),
                            Support {
                                enabled: true,
                                ..Support::default()
                            },
                        )]),
                        prices: BTreeMap::from([(
                            "search".into(),
                            Price {
                                amount: "0.005".into(),
                                currency: "USD".into(),
                                unit: "search".into(),
                                ..Price::default()
                            },
                        )]),
                        ..Offering::default()
                    },
                )]),
                ..EndpointRule::default()
            }],
            ..Declarations::default()
        };
        let upstreams = BTreeMap::from([(
            "generation".into(),
            Endpoint {
                address: "https://provider.example.invalid".into(),
                interface: "fixture-generation".into(),
                ..Endpoint::default()
            },
        )]);
        let original = upstreams.clone();
        let discovered = catalogue(&declarations, &upstreams, &BTreeMap::new());
        assert!(
            !discovered.offerings["fixture-search"].enabled,
            "a declaration cannot opt an organization into spending"
        );
        assert_eq!(discovered.offerings["fixture-search"].groups, ["web"]);
        let choices: BTreeMap<String, ServiceConfig> = serde_json::from_value(json!({
            "fixture-search":{"enabled":true,"prices":{"search":{"amount":"3","currency":"credits","unit":"search"}},"endpoint":"https://untrusted.example.invalid","capabilities":{"image_generation":true}},
            "undeclared-service":{"enabled":true}
        })).unwrap();
        let configured = catalogue(&declarations, &upstreams, &choices);
        let service = &configured.offerings["fixture-search"];
        assert!(service.enabled);
        assert_eq!(service.groups, ["web"]);
        assert_eq!(service.prices["search"].amount, "3");
        assert_eq!(configured.explicit_prices["fixture-search"], ["search"]);
        assert_eq!(
            configured.endpoints[&service.endpoint].address,
            "https://provider.example.invalid/tools"
        );
        assert!(!service.capabilities.contains_key("image_generation"));
        assert!(!configured.offerings.contains_key("undeclared-service"));
        assert_eq!(
            serde_json::to_value(&choices).unwrap()["fixture-search"]["endpoint"],
            "https://untrusted.example.invalid",
            "unknown configuration is preserved without being executed"
        );
        assert_eq!(
            upstreams, original,
            "service projection must not rewrite generation endpoints"
        );
        let mut choices = choices;
        choices.get_mut("fixture-search").unwrap().groups = Some(Vec::new());
        assert!(
            catalogue(&declarations, &upstreams, &choices).offerings["fixture-search"]
                .groups
                .is_empty()
        );
        let unrelated = BTreeMap::from([(
            "generation".into(),
            Endpoint {
                address: "https://another.example.invalid".into(),
                ..original["generation"].clone()
            },
        )]);
        assert!(
            catalogue(&declarations, &unrelated, &choices)
                .offerings
                .is_empty()
        );
    }
}

/// The organization's explicit backend for the shared web-search service mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SearchBackendChoice {
    /// An auxiliary model request through that provider's declared native tool support.
    Model {
        /// Provider whose current credential and limits apply.
        provider_id: Id,
        /// Enabled native-search model, never a vendor inferred from its name.
        model_name: String,
    },
    /// One provider's explicitly enabled, extension-declared API services.
    Api {
        /// Provider whose current credential and limits apply.
        provider_id: Id,
        /// Declared search service id.
        search_service: String,
        /// Declared fetch service id; absence means fetch is unavailable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fetch_service: Option<String>,
    },
}

impl SearchBackendChoice {
    /// The provider whose manager, current Use and allowance govern this backend.
    #[must_use]
    pub const fn provider_id(&self) -> Id {
        match self {
            Self::Model { provider_id, .. } | Self::Api { provider_id, .. } => *provider_id,
        }
    }
}

/// A declared provider service's capability, separate from its model catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    /// Public web search.
    Search,
    /// Read one public page.
    Fetch,
}

/// A service is offered by discovery but only enabled by its provider manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ServiceDeclaration {
    /// Provider-local stable service id.
    pub id: String,
    /// Capability answered by this endpoint.
    pub kind: ServiceKind,
    /// Exact provider-owned endpoint; not a URL supplied by the agent.
    pub endpoint: String,
    /// Closed schema of the request accepted by this service.
    pub input_schema: Json,
    /// Vendor currency per successful call, as an exact decimal string.
    pub price_per_call: String,
    /// Vendor currency id, retained separately from Orch credits.
    pub currency: String,
    /// Whether only a successful nonempty answer incurs the per-call price.
    pub successful_only: bool,
}

/// One result, in the same form for a model tool and a provider API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchResult {
    /// The source page's title.
    pub title: String,
    /// Source URL.
    pub url: String,
    /// Short source excerpt, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Page material, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Source date, when reported rather than guessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// A fetched page, without treating source text as executable instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FetchedPage {
    /// Actual source URL.
    pub url: String,
    /// Reported title.
    pub title: Option<String>,
    /// Page content.
    pub content: String,
    /// Reported/declared content media type.
    pub media_type: String,
}

/// One interface for model-backed and extension API-backed search, never vendor dispatch in core.
#[async_trait]
pub trait SearchBackend: Send + Sync {
    /// Search through the selected, enabled backend.
    async fn search(&self, query: &str, count: u32) -> Result<Vec<SearchResult>, SourceError>;
    /// Fetch only when the backend actually declares support; never emulate it by searching.
    async fn fetch(&self, _url: &str) -> Result<FetchedPage, SourceError> {
        Err(SourceError::new(
            SourceErrorCode::Unsupported,
            "this backend does not offer page fetch",
        ))
    }
}
