//! Existing model defaults and explicit endpoint-format options for model requests.

use crate::protocol::{ANTHROPIC_MESSAGES, OPENAI_CHAT, OPENAI_RESPONSES};
use orch_provider_contract::{calls, models::ModelParameters};
use orch_types::Json;
use serde_json::{Map, json};

/// Read the actual wire field, keeping Anthropic's capacity tier separate from inference speed.
#[must_use]
pub fn requested_service_tier(protocol: &str, params: &Json) -> Option<String> {
    let field = match protocol {
        ANTHROPIC_MESSAGES => "speed",
        OPENAI_RESPONSES | OPENAI_CHAT => "service_tier",
        _ => return None,
    };
    params.get(field).and_then(Json::as_str).map(str::to_owned)
}

/// These text formats declare a response tier/speed field the observer can read.
/// This is decoder support, not a claim that any model offers a paid speed.
#[must_use]
pub fn reports_service_tier(protocol: &str) -> bool {
    matches!(
        protocol,
        OPENAI_RESPONSES | OPENAI_CHAT | ANTHROPIC_MESSAGES
    )
}

/// Documented aliases in the OpenAI text formats, only for resolving an absent rate multiplier.
/// The actual reported value remains separate and is never replaced by this alias.
#[must_use]
pub fn service_tier_price_alias(protocol: &str, reported: &str) -> Option<&'static str> {
    match (protocol, reported) {
        (OPENAI_RESPONSES, "priority") => Some("fast"),
        _ => None,
    }
}

/// The requested processing class used by existing tier caps, not an assertion about the answer.
#[must_use]
pub fn requested_tier_for_caps(protocol: &str, params: &Json) -> Option<String> {
    requested_service_tier(protocol, params).and_then(|tier| match tier.as_str() {
        "default" | "standard" | "auto" => None,
        "fast" if protocol == OPENAI_RESPONSES => Some("priority".to_owned()),
        _ => Some(tier),
    })
}

/// Endpoint defaults are least specific, then the model's defaults, then this use's options.
/// Normalize effort aliases at each layer so an explicit value can replace a default alias.
pub fn resolve(
    protocol: &str,
    model: &ModelParameters,
    endpoint: &Json,
    selected: &Json,
) -> Result<Json, &'static str> {
    let model_defaults = with_model_defaults(protocol, model, &json!({}))?;
    let mut body = apply(protocol, b"{}", endpoint, false)?;
    body = apply(protocol, &body, &model_defaults, false)?;
    body = apply(protocol, &body, selected, false)?;
    let params: Json = serde_json::from_slice(&body).map_err(|_| "model options cannot be read")?;
    with_model_defaults(protocol, model, &params)
}

fn has_effort(params: &Json) -> bool {
    [
        "/effort",
        "/reasoning_effort",
        "/reasoning/effort",
        "/output_config/effort",
    ]
    .iter()
    .any(|path| params.pointer(path).is_some())
}

/// Resolve once before issuing a completion, against its actual current model and wire format.
/// Unknown vendor options remain JSON; being configurable does not declare vendor support.
pub fn with_model_defaults(
    protocol: &str,
    model: &ModelParameters,
    params: &Json,
) -> Result<Json, &'static str> {
    calls::completion_options(params)?;
    let mut options = params
        .as_object()
        .cloned()
        .ok_or("model options must be an object")?;
    if !has_effort(params)
        && let Some(effort) = &model.default_effort
    {
        options.insert("effort".to_owned(), json!(effort));
    }
    let tier_field = match protocol {
        ANTHROPIC_MESSAGES => Some(("speed", "standard")),
        OPENAI_RESPONSES | OPENAI_CHAT => Some(("service_tier", "default")),
        _ => None,
    };
    if let Some((field, ordinary)) = tier_field {
        if !options.contains_key(field)
            && let Some(start) = model.service_tier_start()
        {
            options.insert(field.to_owned(), json!(start));
        }
        if let Some(selected) = options.get_mut(field) {
            if selected.is_null() {
                *selected = json!(ordinary);
            }
            let value = selected
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("service tier must be text or null")?;
            if value != ordinary
                && value != "auto"
                && !model.service_tiers().iter().any(|tier| {
                    tier.id == value
                        || (protocol == OPENAI_RESPONSES
                            && value == "fast"
                            && tier.id == "priority")
                })
            {
                return Err("the selected model has not enabled the requested service tier");
            }
        }
    } else if model.service_tier_start().is_some() {
        return Err("this protocol has not declared model service tiers");
    }
    if protocol == OPENAI_RESPONSES {
        if !options.contains_key("access_programs")
            && let Some(start) = model.access_program_start()
        {
            options.insert("access_programs".to_owned(), json!({"cyber":start}));
        }
        if let Some(programs) = options.get_mut("access_programs") {
            let programs = programs
                .as_object_mut()
                .ok_or("access programs must be an object")?;
            if let Some(selected) = programs.get_mut("cyber") {
                if selected.is_null() {
                    *selected = json!("standard");
                }
                let value = selected
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or("access program must be text or null")?;
                if value != "standard"
                    && !model
                        .access_programs()
                        .iter()
                        .any(|program| program == value)
                {
                    return Err("the selected model has not enabled the requested access program");
                }
            }
        }
    } else if model.access_program_start().is_some() {
        return Err("this protocol has not declared model access programs");
    }
    Ok(Json::Object(options))
}

fn overlay(target: &mut Map<String, Json>, source: Map<String, Json>, defaults: bool) {
    for (key, value) in source {
        match (target.get_mut(&key), value) {
            (Some(Json::Object(held)), Json::Object(extra)) => overlay(held, extra, defaults),
            (Some(_), _) if defaults => {}
            (_, value) => {
                target.insert(key, value);
            }
        }
    }
}

/// Apply generation options after the protocol created its owned request fields.
/// Endpoint defaults fill absent members; explicit request values take precedence.
pub fn apply(
    protocol: &str,
    body: &[u8],
    params: &Json,
    defaults: bool,
) -> Result<Vec<u8>, &'static str> {
    calls::completion_options(params)?;
    let mut extra = params
        .as_object()
        .cloned()
        .ok_or("model options must be an object")?;
    if extra.is_empty() {
        return Ok(body.to_vec());
    }
    if has_effort(params)
        && matches!(
            protocol,
            OPENAI_RESPONSES | OPENAI_CHAT | ANTHROPIC_MESSAGES
        )
    {
        let effort = calls::requested_effort(params)?;
        extra.remove("effort");
        extra.remove("reasoning_effort");
        for key in ["reasoning", "output_config"] {
            if let Some(Json::Object(nested)) = extra.get_mut(key) {
                nested.remove("effort");
                if nested.is_empty() {
                    extra.remove(key);
                }
            }
        }
        let key = match protocol {
            OPENAI_RESPONSES => "reasoning",
            ANTHROPIC_MESSAGES => "output_config",
            OPENAI_CHAT => "reasoning_effort",
            _ => return Err("this protocol has not declared reasoning effort options"),
        };
        if key == "reasoning_effort" {
            extra.insert(key.to_owned(), json!(effort));
        } else {
            let nested = extra.entry(key.to_owned()).or_insert_with(|| json!({}));
            nested
                .as_object_mut()
                .ok_or("reasoning options must be an object")?
                .insert("effort".to_owned(), json!(effort));
        }
    }
    let mut payload: Json =
        serde_json::from_slice(body).map_err(|_| "model request must be an object")?;
    let members = payload
        .as_object_mut()
        .ok_or("model request must be an object")?;
    overlay(members, extra, defaults);
    serde_json::to_vec(&payload).map_err(|_| "model request cannot be encoded")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelParameters {
        serde_json::from_value(json!({
            "efforts":["low","high"],"default_effort":"low",
            "service_tiers":[{"id":"priority","label":"Fast"}],
            "default_service_tier":"priority"
        }))
        .expect("model parameters")
    }

    #[test]
    fn endpoint_model_and_explicit_options_have_one_preserving_precedence() {
        let resolved = resolve(
            OPENAI_RESPONSES,
            &model(),
            &json!({
                "service_tier":"default", "effort":"high",
                "extra_body":{"vendor":{"keep":[true,null,3],"switch":false}}
            }),
            &json!({
                "service_tier":null, "reasoning":{"effort":"high","summary":"concise"},
                "extra_body":{"vendor":{"switch":true}}
            }),
        )
        .expect("request options");
        assert_eq!(resolved["service_tier"], "default");
        assert_eq!(
            resolved["reasoning"],
            json!({"effort":"high","summary":"concise"})
        );
        assert_eq!(
            resolved["extra_body"]["vendor"],
            json!({"keep":[true,null,3],"switch":true})
        );
        assert!(resolved.get("effort").is_none());
    }

    #[test]
    fn documented_responses_alias_does_not_become_another_protocols_capability() {
        let options =
            with_model_defaults(OPENAI_RESPONSES, &model(), &json!({"service_tier":"fast"}))
                .expect("documented alias of enabled priority");
        assert_eq!(
            options["service_tier"], "fast",
            "requested spelling is retained"
        );
        assert_eq!(
            requested_tier_for_caps(OPENAI_RESPONSES, &options).as_deref(),
            Some("priority")
        );
        assert!(
            with_model_defaults(ANTHROPIC_MESSAGES, &model(), &json!({"speed":"fast"})).is_err()
        );
        assert_eq!(
            service_tier_price_alias(ANTHROPIC_MESSAGES, "priority"),
            None
        );
        assert!(!reports_service_tier("unlisted-format"));
        assert_eq!(
            requested_service_tier("unlisted-format", &json!({"service_tier":"fast"})),
            None
        );
    }

    #[test]
    fn endpoint_defaults_cannot_change_an_explicit_request_or_owned_fields() {
        let body =
            br#"{"model":"chosen","input":"material","temperature":0.7,"future":{"n":null}}"#;
        let merged = apply(
            OPENAI_RESPONSES,
            body,
            &json!({
                "temperature":0.2,"future":{"n":3,"enabled":true}
            }),
            true,
        )
        .expect("body defaults");
        let merged: Json = serde_json::from_slice(&merged).expect("JSON");
        assert_eq!(merged["model"], "chosen");
        assert_eq!(merged["input"], "material");
        assert_eq!(merged["temperature"], json!(0.7));
        assert_eq!(merged["future"], json!({"n":null,"enabled":true}));
        for protected in [
            "model",
            "input",
            "messages",
            "system",
            "tools",
            "max_output_tokens",
            "store",
            "headers",
        ] {
            let replacement = Json::Object(
                [(protected.to_owned(), json!("replacement"))]
                    .into_iter()
                    .collect(),
            );
            assert!(
                apply(OPENAI_RESPONSES, body, &replacement, false).is_err(),
                "{protected}"
            );
        }
    }
}
