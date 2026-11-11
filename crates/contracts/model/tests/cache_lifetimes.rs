use orch_model_contract::parameters::{constraints, outgoing, requested_cache_lifetimes};
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};

fn entry(value: Value) -> ModelParameters {
    serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("the model fixture must be valid: {error}"))
}
fn body() -> Value {
    json!({"model":"chosen","max_tokens":32,"messages":[{"role":"user","content":"original input"}]})
}
fn send(model: &ModelParameters, body: &Value) -> Result<Value, &'static str> {
    outgoing(
        "anthropic-messages",
        model,
        &serde_json::to_vec(body)
            .unwrap_or_else(|error| panic!("the request fixture must encode: {error}")),
    )
    .map(|bytes| {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("the outgoing body must be valid JSON: {error}"))
    })
}
fn cases() -> Vec<Value> {
    let directive = json!({"type":"ephemeral","ttl":"1h"});
    vec![
        json!({"cache_control":directive}),
        json!({"system":[{"type":"text","text":"original system","cache_control":directive}]}),
        json!({"tools":[{"name":"client","input_schema":{},"cache_control":directive}]}),
        json!({"messages":[{"role":"user","content":[{"type":"text","text":"original message","cache_control":directive}]}]}),
        json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"call","content":[{"type":"text","text":"original result","cache_control":directive}]}]}]}),
    ]
}

#[test]
fn one_hour_is_closed_at_every_real_wire_position_until_the_manager_enables_it() {
    let enabled = entry(json!({"cache_lifetimes":["1h"]}));
    let offered = entry(json!({"offered":{"cache_lifetimes":["1h"]}}));
    for extra in cases() {
        let mut request = body();
        request
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(send(&ModelParameters::default(), &request).is_err());
        assert!(send(&offered, &request).is_err());
        let sent = send(&enabled, &request).unwrap();
        assert_eq!(
            requested_cache_lifetimes("anthropic-messages", &sent).unwrap(),
            vec!["1h"]
        );
        for field in ["messages", "system", "tools", "cache_control"] {
            if let Some(value) = request.get(field) {
                assert_eq!(sent.get(field), Some(value));
            }
        }
    }
    assert_eq!(
        constraints("anthropic-messages", &enabled)["properties"]["cache_control"]["properties"]["ttl"]
            ["enum"],
        json!(["5m", "1h"])
    );
}

#[test]
fn user_text_tool_arguments_and_tool_schemas_are_not_cache_directives() {
    let mut request = body();
    request["messages"] = json!([{"role":"assistant","content":[{"type":"tool_use","id":"call","name":"client","input":{"cache_control":{"ttl":"1h"},"content":[{"cache_control":{"ttl":"1h"}}]}}]},
        {"role":"user","content":"literal cache_control ttl 1h is user data"}]);
    request["tools"] = json!([{"name":"client","input_schema":{"properties":{"cache_control":{"const":{"ttl":"1h"}}}}}]);
    let sent = send(&ModelParameters::default(), &request).unwrap();
    assert_eq!(sent["messages"], request["messages"]);
    assert_eq!(sent["tools"], request["tools"]);
    assert!(
        requested_cache_lifetimes("anthropic-messages", &sent)
            .unwrap()
            .is_empty()
    );
    let mut ordinary = body();
    ordinary["cache_control"] = json!({"type":"ephemeral"});
    assert_eq!(
        requested_cache_lifetimes(
            "anthropic-messages",
            &send(&ModelParameters::default(), &ordinary).unwrap()
        )
        .unwrap(),
        vec!["5m"]
    );
}

#[test]
fn nested_native_cache_controls_consume_the_same_schema_refs_and_explicit_replacements() {
    let restricted = entry(
        json!({"cache_lifetimes":["1h"],"request_options":{"anthropic-messages":{
            "$defs":{"short":{"type":"object","properties":{"ttl":{"enum":["5m"]}}}},
            "properties":{"cache_control":{"$ref":"#/$defs/short"}}
        }}}),
    );
    let mut request = body();
    request["system"] = cases()[1]["system"].clone();
    assert!(send(&restricted, &request).is_err());
    assert_eq!(
        constraints("anthropic-messages", &restricted)["properties"]["cache_control"]["properties"]
            ["ttl"]["enum"],
        json!(["5m"])
    );
    let strip = entry(
        json!({"request_options":{"anthropic-messages":{"properties":{
            "cache_control":{"const":null,"x-orch-replace":true}
        }}}}),
    );
    let sent = send(&strip, &request).unwrap();
    assert!(sent["system"][0].get("cache_control").is_none());
    assert_eq!(sent["system"][0]["text"], "original system");
    assert_eq!(sent["messages"], request["messages"]);
    let narrow = entry(
        json!({"request_options":{"anthropic-messages":{"properties":{
            "cache_control":{"type":"object","properties":{"type":{"const":"ephemeral"},"ttl":{"const":"5m","x-orch-replace":true}}}
        }}}}),
    );
    let sent = send(&narrow, &request).unwrap();
    assert_eq!(sent["system"][0]["cache_control"]["ttl"], "5m");
    assert_eq!(sent["system"][0]["text"], "original system");
}
