//! Provider management validates service choices against the installed declaration catalogue.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use orch_api_contract::resources::Upstream;

use orch_provider_contract::builtins::BuiltinSources;

use orch_provider_contract::capabilities::Endpoint;

use orch_provider_contract::services::ServiceConfig;

use orch_provider_contract::services::catalogue;

use orch_types::Json;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
pub(super) fn views(
    sources: &BuiltinSources,
    upstreams: &Json,
    choices: &BTreeMap<String, ServiceConfig>,
) -> ApiResult<Vec<orch_api_contract::resources::ProviderServiceOffering>> {
    let upstreams: Vec<Upstream> =
        serde_json::from_value(upstreams.clone()).map_err(ApiError::internal)?;
    let endpoints = upstreams
        .into_iter()
        .map(|upstream| {
            (
                upstream.protocol.clone(),
                Endpoint {
                    interface: upstream.protocol,
                    address: upstream.base_url,
                    ..Endpoint::default()
                },
            )
        })
        .collect();
    let declarations = sources.capabilities().declarations();
    let offered = catalogue(declarations, &endpoints, &BTreeMap::new());
    let safe_prices = |prices: BTreeMap<String, orch_provider_contract::capabilities::Price>| {
        prices
            .into_iter()
            .map(|(unit, price)| {
                (
                    unit,
                    orch_provider_contract::capabilities::Price {
                        amount: price.amount,
                        currency: price.currency,
                        unit: price.unit,
                        ..Default::default()
                    },
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    Ok(offered
        .offerings
        .iter()
        .map(|(id, offering)| {
            let endpoint = offered.endpoints.get(&offering.endpoint);
            let mut capabilities = Vec::new();
            let mut units = BTreeSet::new();
            let mut prices = BTreeMap::new();
            for (name, support) in &offering.capabilities {
                if !support.enabled {
                    continue;
                }
                if let Some(capability) = declarations.capabilities.get(name) {
                    units.insert(capability.ledger_unit.clone());
                }
                if let Some(binding) = endpoint.and_then(|endpoint| {
                    orch_provider_contract::services::service_binding(
                        declarations,
                        endpoint,
                        offering,
                        name,
                    )
                }) {
                    capabilities.push(name.clone());
                    units.extend(
                        binding
                            .counters
                            .values()
                            .filter(|counter| !counter.reported_cost && !counter.unit.is_empty())
                            .map(|counter| counter.unit.clone()),
                    );
                    prices.extend(binding.prices.clone());
                }
            }
            prices.extend(offering.prices.clone());
            units.extend(prices.keys().cloned());
            let catalogue_prices = safe_prices(prices.clone());
            let choice = choices.get(id);
            if let Some(choice) = choice {
                prices.extend(choice.prices.clone());
            }
            orch_api_contract::resources::ProviderServiceOffering {
                id: id.clone(),
                title: if offering.title.is_empty() {
                    id.clone()
                } else {
                    offering.title.clone()
                },
                available: !capabilities.is_empty()
                    && endpoint
                        .and_then(|endpoint| declarations.interfaces.get(&endpoint.interface))
                        .is_some_and(|interface| {
                            orch_provider_contract::services::json_implementation(
                                &interface.implementation,
                            )
                        }),
                capabilities,
                enabled: choice.is_some_and(|choice| choice.enabled),
                groups: choice
                    .and_then(|choice| choice.groups.clone())
                    .unwrap_or_else(|| offering.groups.clone()),
                declared_groups: offering.groups.clone(),
                units: units.into_iter().filter(|unit| !unit.is_empty()).collect(),
                catalogue_prices,
                prices: safe_prices(prices),
                explicit_prices: choice
                    .map(|choice| choice.prices.keys().cloned().collect())
                    .unwrap_or_default(),
            }
        })
        .collect())
}

pub(super) fn checked(
    sources: &BuiltinSources,
    upstreams: &Json,
    choices: &BTreeMap<String, ServiceConfig>,
    previous: &Json,
) -> ApiResult<Json> {
    let previous: BTreeMap<String, ServiceConfig> =
        serde_json::from_value(previous.clone()).map_err(ApiError::internal)?;
    let upstreams: Vec<Upstream> =
        serde_json::from_value(upstreams.clone()).map_err(ApiError::internal)?;
    let endpoints = upstreams
        .into_iter()
        .map(|upstream| {
            (
                upstream.protocol.clone(),
                Endpoint {
                    interface: upstream.protocol,
                    address: upstream.base_url,
                    ..Endpoint::default()
                },
            )
        })
        .collect();
    let declarations = sources.capabilities().declarations();
    let offered = catalogue(declarations, &endpoints, &BTreeMap::new());
    for (name, choice) in choices {
        let Some(service) = offered.offerings.get(name) else {
            if previous.get(name) == Some(choice) {
                continue;
            }
            return Err(ApiError::invalid(
                "service_offerings",
                "a service must be declared for this provider's configured endpoints",
            ));
        };
        if choice.groups.as_ref().is_some_and(|groups| {
            groups
                .iter()
                .any(|group| !orch_api_contract::resources::is_model_group_name(group))
        }) {
            return Err(ApiError::invalid(
                "service_offerings",
                "service groups need their usual short names",
            ));
        }
        let interface = offered
            .endpoints
            .get(&service.endpoint)
            .and_then(|endpoint| declarations.interfaces.get(&endpoint.interface));
        let mut units: BTreeSet<&str> = service.prices.keys().map(String::as_str).collect();
        if let Some(interface) = interface {
            units.extend(
                interface
                    .shape
                    .counters
                    .values()
                    .map(|counter| counter.unit.as_str())
                    .filter(|unit| !unit.is_empty()),
            );
        }
        units.extend(
            service
                .capabilities
                .keys()
                .filter_map(|name| declarations.capabilities.get(name))
                .map(|capability| capability.ledger_unit.as_str()),
        );
        for (unit, price) in &choice.prices {
            if unit.trim().is_empty()
                || price.unit != *unit
                || !units.contains(unit.as_str())
                    && previous
                        .get(name)
                        .and_then(|choice| choice.prices.get(unit))
                        != Some(price)
                || price.currency.trim().is_empty()
                || price.amount.starts_with('-')
                || !orch_provider_contract::limits::is_decimal(&price.amount)
            {
                return Err(ApiError::invalid(
                    "service_offerings",
                    "a service price needs a declared unit, currency and nonnegative decimal amount",
                ));
            }
        }
    }
    serde_json::to_value(choices).map_err(ApiError::internal)
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use orch_provider_contract::capabilities::Declarations;

    use orch_provider_contract::capabilities::EndpointRule;

    use orch_provider_contract::capabilities::Interface;

    use orch_provider_contract::capabilities::Offering;

    use orch_provider_contract::capabilities::Support;

    use serde_json::json;
    #[cfg(any(test, feature = "test-support"))]
    fn editing_a_current_service_preserves_only_unchanged_stale_offerings_and_units()
    -> Result<(), String> {
        let mut sources = BuiltinSources::default();
        sources.register_capabilities("fixture",Declarations {
            interfaces:BTreeMap::from([("fixture".into(),Interface {implementation:"http-json".into(),..Interface::default()})]),
            endpoint_rules:vec![EndpointRule {
                when:json!({"required":["host"],"properties":{"host":{"const":"service.example.invalid"}}}),
                endpoints:BTreeMap::from([("fixture".into(),Endpoint {interface:"fixture".into(),address:"https://service.example.invalid/".into(),..Endpoint::default()})]),
                offerings:BTreeMap::from([("current".into(),Offering {kind:"service".into(),endpoint:"fixture".into(),capabilities:BTreeMap::from([("web_search".into(),Support {enabled:true,..Support::default()})]),..Offering::default()})]),
                ..EndpointRule::default()
            }],..Declarations::default()
        }).map_err(|error|error.to_string())?;
        let upstreams = json!([{"protocol":"openai-responses","base_url":"https://service.example.invalid/","options":{}}]);
        let previous = json!({"removed":{"enabled":true,"future_member":{"kept":true}},"current":{"enabled":false,"prices":{"retired-unit":{"amount":"1","currency":"credits","unit":"retired-unit"}}}});
        let mut choices: BTreeMap<String, ServiceConfig> =
            serde_json::from_value(previous.clone()).map_err(|error| error.to_string())?;
        choices
            .get_mut("current")
            .ok_or("missing current service")?
            .enabled = true;
        let stored = checked(&sources, &upstreams, &choices, &previous)
            .map_err(|error| format!("{error:?}"))?;
        // An omitted prices map and its serialized empty default are the same choice.
        // Compare the whole typed row so unknown fields still have to survive.
        let stored_removed: ServiceConfig =
            serde_json::from_value(stored["removed"].clone()).map_err(|error| error.to_string())?;
        assert_eq!(Some(&stored_removed), choices.get("removed"));
        assert_eq!(
            stored["removed"]["future_member"],
            previous["removed"]["future_member"]
        );
        assert_eq!(stored["current"]["prices"], previous["current"]["prices"]);
        let mut repriced = choices.clone();
        repriced
            .get_mut("current")
            .and_then(|choice| choice.prices.get_mut("retired-unit"))
            .ok_or("missing retired unit price")?
            .amount = "2".into();
        assert!(
            checked(&sources, &upstreams, &repriced, &previous).is_err(),
            "an unchanged stale price is no permission to author another undeclared unit price"
        );
        choices
            .get_mut("removed")
            .ok_or("missing stale service")?
            .enabled = false;
        assert!(
            checked(&sources, &upstreams, &choices, &previous).is_err(),
            "an unchanged stale row is no permission to author another undeclared offering"
        );
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_editing_a_current_service_preserves_only_unchanged_stale_offerings_and_units()
    -> Result<(), String> {
        self::editing_a_current_service_preserves_only_unchanged_stale_offerings_and_units()
    }
}
