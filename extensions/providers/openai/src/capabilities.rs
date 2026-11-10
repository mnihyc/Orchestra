//! Native program features offered by the account's exact Responses endpoint.
use orch_provider_contract::capabilities::{Binding, Declarations, EndpointRule, NativeSwitch};
use serde_json::json;
use std::collections::BTreeMap;

fn responses_endpoint() -> orch_types::Json {
    json!({
        "required":["protocol","scheme","host","port","path","has_credentials","has_query","has_fragment"],
        "properties":{
            "protocol":{"const":"openai-responses"},
            "scheme":{"const":"https"},
            "host":{"const":"api.openai.com"},
            "port":{"enum":[null,443]},
            "path":{"enum":["/","/v1","/v1/"]},
            "has_credentials":{"const":false},
            "has_query":{"const":false},
            "has_fragment":{"const":false}
        }
    })
}

fn forwarded_native_model() -> orch_types::Json {
    json!({
        "required":["access","route_kind","endpoint_facts"],
        "properties":{
            "access":{"const":"forwarded"},
            "route_kind":{"const":"native-model"},
            "endpoint_facts":responses_endpoint()
        }
    })
}

fn native_search() -> Option<Binding> {
    // Retain the interface's hosted result/usage/options mapping for ordinary Responses requests.
    let mut binding = orch_provider_contract::capabilities::protocol_interfaces()
        .ok()?
        .remove("openai-responses")?
        .native_tools
        .remove("web_search")?;
    let mut options_schema = binding.request.get("options_schema")?.clone();
    options_schema
        .get_mut("properties")?
        .as_object_mut()?
        .remove("return_token_budget");
    let mut form: orch_types::Json =
        serde_json::from_str(include_str!("standalone-search.json")).ok()?;
    form["options_schema"] = options_schema;
    let request = binding.request.as_object_mut()?;
    request
        .entry("forms")
        .or_insert_with(|| json!([]))
        .as_array_mut()?
        .push(form);
    Some(binding)
}

/// Public scalar settings are selected by the capability registry, never by the program adapter.
#[must_use]
pub fn declarations() -> Declarations {
    let mut switches = BTreeMap::from([(
        "capabilities.remote_compaction".into(),
        NativeSwitch {
            capability: "generation".into(),
            value: Some(json!("v2")),
            when: forwarded_native_model(),
            ..NativeSwitch::default()
        },
    )]);
    let mut native_tools = BTreeMap::new();
    if let Some(search) = native_search() {
        native_tools.insert("web_search".into(), Some(search));
        switches.insert(
            "supports_standalone_web_search".into(),
            NativeSwitch {
                capability: "web_search".into(),
                value: Some(json!(true)),
                when: forwarded_native_model(),
                ..NativeSwitch::default()
            },
        );
    }
    Declarations {
        endpoint_rules: vec![
            EndpointRule {
                when: responses_endpoint(),
                native_switches: BTreeMap::from([("codex".into(), switches)]),
                native_tools,
                ..EndpointRule::default()
            },
            EndpointRule {
                when: json!({"allOf":[responses_endpoint(), {
                    "required":["model"], "properties":{"model":{"pattern":r"^gpt-(5\.6|6)([.-]|$)"}}
                }]}),
                model_metadata: BTreeMap::from([(
                    "codex".into(),
                    json!({
                        "requires_standalone_search":true, "use_responses_lite":true
                    }),
                )]),
                native_switches: BTreeMap::from([(
                    "codex".into(),
                    BTreeMap::from([(
                        "model.use_responses_lite".into(),
                        NativeSwitch {
                            capability: "generation".into(),
                            value: Some(json!(true)),
                            when: forwarded_native_model(),
                            ..NativeSwitch::default()
                        },
                    )]),
                )]),
                ..EndpointRule::default()
            },
        ],
        ..Declarations::default()
    }
}

#[cfg(test)]
mod model_metadata_tests {
    use super::*;
    #[test]
    fn native_model_dialect_is_declared_for_exact_endpoints_without_guessing_bundled_suffixes()
    -> Result<(), Box<dyn std::error::Error>> {
        let declarations = declarations();
        for (address, model, expected) in [
            ("https://api.openai.com/v1", "gpt-5.6", true),
            ("https://api.openai.com/v1/", "gpt-6-sol", true),
            ("https://api.openai.com/v1", "gpt-6.1-sol", true),
            ("https://api.openai.com/v1", "gpt-5.5", false),
            ("https://api.openai.com/v1", "openai/gpt-6-sol", false),
            ("https://openrouter.ai/api/v1", "openai/gpt-6-sol", false),
            (
                "https://api.openai.com.example.invalid/v1",
                "gpt-6-sol",
                false,
            ),
            ("https://api.openai.com:444/v1", "gpt-6-sol", false),
        ] {
            let projected = orch_provider_contract::capabilities::project_endpoint(
                &declarations,
                "openai-responses",
                &address.parse()?,
                model,
            );
            assert_eq!(
                projected
                    .model_metadata
                    .get("codex")
                    .and_then(|metadata| metadata.get("requires_standalone_search"))
                    .and_then(orch_types::Json::as_bool),
                expected.then_some(true),
                "{address} {model}"
            );
            assert_eq!(
                projected
                    .native_switches
                    .get("codex")
                    .and_then(|switches| switches.get("model.use_responses_lite"))
                    .and_then(|switch| switch.value.as_ref())
                    .and_then(orch_types::Json::as_bool),
                expected.then_some(true),
                "{address} {model}"
            );
        }
        Ok(())
    }
}
