//! Resource summaries project native model facts into the existing capability resolver.
use super::*;
use orch_provider_contract::capabilities::{
    self as cap, Availability, Declarations, Endpoint, Offering, Price, Provider, Registry,
    Resolution, Support, ThreadContext,
};

/// The selected provider/model, actual installed execution ports and current Use go through
/// the same registry/resolver as every capability. No provider name decides its route.
pub async fn resolve_route(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    config: &Config,
    executors: Vec<String>,
) -> Result<Resolution, EngineError> {
    project_route(conn, principal, place, config, executors, false).await
}

async fn project_route(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    config: &Config,
    executors: Vec<String>,
    metadata: bool,
) -> Result<Resolution, EngineError> {
    let offering = config.offering().map_err(EngineError::unreadable)?;
    let provider = resources::provider(conn, offering.provider)
        .await?
        .ok_or_else(refused)?;
    let model = resources::model(conn, provider.id, &offering.offering)
        .await?
        .ok_or_else(refused)?;
    let program = config.backend.route == "summary.delegation";
    let protocol = if program {
        None
    } else {
        Some(super::completion_protocol(&provider.protocols)?.ok_or_else(refused)?)
    };
    let mut registry = Registry::default();
    registry
        .register("common", cap::catalogue().map_err(EngineError::unreadable)?)
        .map_err(EngineError::unreadable)?;
    registry
        .register(
            "summarizer-model",
            orch_summarizer_model::capabilities::declarations(),
        )
        .map_err(EngineError::unreadable)?;
    let mut declarations = Declarations::default();
    declarations.providers.insert(
        provider.id,
        Provider {
            title: provider.name,
            endpoints: protocol
                .iter()
                .map(|protocol| {
                    (
                        protocol.clone(),
                        Endpoint {
                            interface: protocol.clone(),
                            ..Endpoint::default()
                        },
                    )
                })
                .collect(),
            offerings: BTreeMap::from([(
                model.model_name.clone(),
                Offering {
                    title: model.model_name.clone(),
                    endpoint: protocol.clone().unwrap_or_default(),
                    kind: "model".into(),
                    enabled: model.enabled,
                    capabilities: BTreeMap::from([(
                        "generation".into(),
                        Support {
                            enabled: true,
                            ..Support::default()
                        },
                    )]),
                    groups: model.groups.clone(),
                    parameters: model.parameters.clone(),
                    prices: if program {
                        BTreeMap::new()
                    } else {
                        base_prices(&model.parameters)
                    },
                    ..Offering::default()
                },
            )]),
            ..Provider::default()
        },
    );
    registry
        .register("native-summary-model", declarations)
        .map_err(EngineError::unreadable)?;
    let allowed = require_model(conn, principal, place, config).await.is_ok();
    let context = ThreadContext {
        choices: BTreeMap::from([("summary".into(), Some("service".into()))]),
        backends: BTreeMap::from([("summary".into(), config.backend.clone())]),
        availability: BTreeMap::from([(
            offering.provider,
            BTreeMap::from([(
                offering.offering.clone(),
                Availability {
                    allowed,
                    show_provider: metadata,
                    show_offering: metadata,
                    show_price: metadata,
                    ..Availability::default()
                },
            )]),
        )]),
        executors,
        interfaces: protocol.into_iter().collect(),
        executor_support: BTreeMap::from([(
            "program-session".into(),
            crate::tools::capability_service::delegation::execution_support(),
        )]),
        ..ThreadContext::default()
    };
    registry
        .resolve(&context)
        .into_iter()
        .find(|result| result.capability == "summary")
        .ok_or_else(refused)
}

/// The actual installed model completion adapter's current preview, without borrowing a
/// person-issued operation or granting provider metadata Read. Unknown producers stay absent.
pub async fn preview_route(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    producer: Option<&options::Producer>,
    metadata: bool,
) -> Result<Option<Resolution>, EngineError> {
    let Some(producer) = producer.filter(|producer| producer.kind == "model") else {
        return Ok(None);
    };
    let Ok(config) = Config::read(&producer.config) else {
        return Ok(None);
    };
    project_route(
        conn,
        principal,
        place,
        &config,
        vec![
            orch_summarizer_model::capabilities::EXECUTOR.into(),
            "program-session".into(),
        ],
        metadata,
    )
    .await
    .map(Some)
}

// These are declared base quotes, never a request measurement or an inferred total.
fn base_prices(parameters: &Json) -> BTreeMap<String, Price> {
    let Some(rate) = parameters.get("rate") else {
        return BTreeMap::new();
    };
    [
        "input_per_mtok",
        "cached_input_per_mtok",
        "cache_write_per_mtok",
        "output_per_mtok",
    ]
    .into_iter()
    .filter_map(|key| {
        rate.get(key).and_then(Json::as_str).map(|amount| {
            (
                key.into(),
                Price {
                    amount: amount.into(),
                    currency: "credits".into(),
                    unit: "million_tokens".into(),
                    ..Price::default()
                },
            )
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn base_quotes_preserve_told_zero_and_leave_missing_rates_unknown() {
        assert!(base_prices(&json!({})).is_empty());
        let quotes = base_prices(
            &json!({"rate":{"input_per_mtok":"0","output_per_mtok":"2.5","cached_input_per_mtok":null}}),
        );
        assert_eq!(quotes.len(), 2);
        assert_eq!(
            quotes
                .get("input_per_mtok")
                .map(|quote| quote.amount.as_str()),
            Some("0")
        );
        assert_eq!(
            quotes.get("output_per_mtok").map(|quote| (
                quote.amount.as_str(),
                quote.currency.as_str(),
                quote.unit.as_str()
            )),
            Some(("2.5", "credits", "million_tokens"))
        );
        assert!(!quotes.contains_key("cached_input_per_mtok"));
    }
}
