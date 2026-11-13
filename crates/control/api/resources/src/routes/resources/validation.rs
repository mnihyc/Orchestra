//! Whole-value schema refusals for resource forms, with field paths and no supplied values.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use orch_api_contract::common::FieldFault;

use orch_api_contract::common::Problem;

use orch_db::Conn;

use orch_types::Id;

use orch_types::Json;
/// Validates before any part of a resource form is written.
pub(crate) fn value(path: &str, schema: &Json, value: &Json) -> ApiResult<()> {
    orch_plugin::schema::validate(schema, value).map_err(|error| match error {
        orch_plugin::schema::Error::Value { faults } => Problem::invalid(
            faults
                .into_iter()
                .map(|fault| FieldFault {
                    path: format!("{path}{}", fault.path),
                    message: fault.message,
                })
                .collect(),
        )
        .into(),
        orch_plugin::schema::Error::Schema { .. } => {
            ApiError::internal("the definition's schema cannot be used")
        }
    })
}

/// The summarizer setting stores its kind beside the configuration its descriptor validates.
pub(crate) fn setting(engine: &orch_engine::Engine, key: &str, given: &Json) -> ApiResult<()> {
    if key != orch_settings_contract::registry::keys::AUTO_SUMMARIZER || given.is_null() {
        return Ok(());
    }
    let (kind, config) =
        orch_summarizer_contract::descriptor::split_setting(given).map_err(|error| {
            ApiError::from(Problem::invalid(vec![FieldFault {
                path: "value.kind".into(),
                message: error.to_string(),
            }]))
        })?;
    let schema = engine.summarizer_config_schema(&kind).ok_or_else(|| {
        ApiError::new(
            orch_api_contract::common::ErrorCode::SourceOffline,
            "the selected summarizer has no available configuration schema",
        )
    })?;
    value("value", &schema, &config)
}
/// The providers a setting's value names, each with the path of the member that holds it, wherever the value names one: a default provider, the
/// provider of a summarizer, of the Ask models, of the embeddings, of a search or image backend and of a failover candidate.
fn providers_named(key: &str, given: &Json) -> Vec<(String, Id)> {
    use orch_settings_contract::registry::keys;
    let mut found = Vec::new();
    let mut take = |path: String, value: Option<&Json>| {
        if let Some(id) = value.and_then(|value| serde_json::from_value::<Id>(value.clone()).ok()) {
            found.push((path, id));
        }
    };
    match key {
        keys::AGENT_DEFAULT_PROVIDER_ID => take("value".to_owned(), Some(given)),
        keys::AUTO_SUMMARIZER | keys::SEARCH_SEMANTIC_EMBEDDINGS => {
            take("value.provider_id".to_owned(), given.get("provider_id"));
        }
        keys::ASK_MODELS => {
            for (index, entry) in given.as_array().into_iter().flatten().enumerate() {
                take(
                    format!("value[{index}].provider_id"),
                    entry.get("provider_id"),
                );
            }
        }
        keys::AGENTS_WEB_SEARCH_BACKEND | keys::AGENTS_IMAGE_GENERATION_BACKEND => {
            take("value.provider_id".to_owned(), given.get("provider_id"));
            take(
                "value.offering.provider".to_owned(),
                given
                    .get("offering")
                    .and_then(|offering| offering.get("provider")),
            );
            for (capability, backend) in given
                .get("capabilities")
                .and_then(Json::as_object)
                .into_iter()
                .flatten()
            {
                take(
                    format!("value.capabilities.{capability}.offering.provider"),
                    backend
                        .get("offering")
                        .and_then(|offering| offering.get("provider")),
                );
            }
        }
        keys::CALLS_RULES => {
            for (index, rule) in given.as_array().into_iter().flatten().enumerate() {
                take(
                    format!("value[{index}].when.provider_id"),
                    rule.get("when").and_then(|when| when.get("provider_id")),
                );
                for (step, one) in rule
                    .get("steps")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    take(
                        format!("value[{index}].steps[{step}].candidate.provider_id"),
                        one.get("candidate")
                            .and_then(|candidate| candidate.get("provider_id")),
                    );
                }
            }
        }
        _ => {}
    }
    found
}

/// A thing a setting names has to exist: an identifier of a provider that is not one is refused at save, with its kind in the sentence, and not left to
/// fail at a later run. Whether the person may use it is the access engine's question, asked where the value is used.
pub(crate) async fn named_things_exist(conn: &mut Conn, key: &str, given: &Json) -> ApiResult<()> {
    let mut faults = Vec::new();
    for (path, id) in providers_named(key, given) {
        let present = orch_db::queries::resources::thread_engine::provider(conn, id)
            .await?
            .is_some_and(|row| row.deleted_at.is_none());
        if !present {
            faults.push(FieldFault {
                path,
                message: "no provider with this identifier exists".to_owned(),
            });
        }
    }
    if faults.is_empty() {
        Ok(())
    } else {
        Err(Problem::invalid(faults).into())
    }
}
orch_api_base::__api_bind_d2a05682ae!();

/// An absent or empty options schema declares no provider options; otherwise it is ordinary JSON Schema.
pub(crate) fn provider_options(schema: &Json, given: &Json) -> ApiResult<()> {
    if schema.is_null() || schema.as_object().is_some_and(serde_json::Map::is_empty) {
        return value(
            "config",
            &serde_json::json!({"type":"object","maxProperties":0}),
            given,
        );
    }
    value("config", schema, given)
}
