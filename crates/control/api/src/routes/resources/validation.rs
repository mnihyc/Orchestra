//! Whole-value schema refusals for resource forms, with field paths and no supplied values.

use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::common::{FieldFault, Problem};
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
