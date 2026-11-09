//! The provider's declared side form uses the existing option builder and final ceiling.
use orch_model_contract::parameters::outgoing_with_access_program;
use orch_model_contract::web_search::{WebSearchDeclaration, for_model};
use orch_provider_contract::capabilities::Binding;
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn parameters() -> Result<ModelParameters, Box<dyn std::error::Error>> {
    let mut binding: Binding = orch_provider_contract::capabilities::protocol_interfaces()?
        .remove("openai-responses")
        .ok_or("Responses interface")?
        .native_tools
        .remove("web_search")
        .ok_or("search binding")?;
    let mut side: Value = serde_json::from_str(include_str!(
        "../../../../extensions/providers/openai/src/standalone-search.json"
    ))?;
    let mut schema = binding.request["options_schema"].clone();
    schema["properties"]
        .as_object_mut()
        .ok_or("search option schema")?
        .remove("return_token_budget");
    side["options_schema"] = schema;
    binding.request["forms"] = json!([side]);
    Ok(ModelParameters {
        web_search: Some(vec!["cached".into(), "indexed".into(), "live".into()]),
        native_tools: BTreeMap::from([(
            "openai-responses".into(),
            BTreeMap::from([("web_search".into(), Some(binding))]),
        )]),
        ..ModelParameters::default()
    })
}

fn request(mode: Value) -> Value {
    json!({"id":"session","model":"opaque-model","input":[],"max_output_tokens":100,
        "commands":{"search_query":[{"q":"source"}]},
        "settings":{"external_web_access":mode,"filters":{"allowed_domains":["example.invalid"]}}})
}

#[test]
fn the_side_request_keeps_its_inputs_and_search_ceiling_without_generation_only_options()
-> Result<(), Box<dyn std::error::Error>> {
    for (wire, mode, normalized) in [
        (json!(false), "cached", json!(false)),
        (json!("cached"), "cached", json!(false)),
        (json!("indexed"), "indexed", json!("indexed")),
        (json!(true), "live", json!(true)),
        (json!("live"), "live", json!(true)),
    ] {
        let model = parameters()?;
        let body = request(wire);
        let encoded = outgoing_with_access_program(
            "openai-responses",
            "/alpha/search",
            &model,
            body.to_string().as_bytes(),
            Some(Some("generation-program")),
        )?;
        let sent: Value = serde_json::from_slice(&encoded)?;
        assert_eq!(sent["id"], body["id"]);
        assert_eq!(sent["model"], body["model"]);
        assert_eq!(sent["commands"], body["commands"]);
        assert_eq!(sent["settings"]["filters"], body["settings"]["filters"]);
        assert_eq!(sent["settings"]["external_web_access"], normalized);
        assert!(sent.get("tools").is_none());
        assert!(sent.get("access_programs").is_none());
        let form = for_model("openai-responses", &model).ok_or("search declaration")?;
        form.admit_native_request(
            "/alpha/search",
            &sent,
            &BTreeMap::from([("supports_standalone_web_search".into(), json!(true))]),
            mode,
        )?;
    }
    Ok(())
}

#[test]
fn native_side_requests_cannot_change_mode_bypass_search_options_or_move_to_another_path()
-> Result<(), Box<dyn std::error::Error>> {
    let mut model = parameters()?;
    let body = request(json!("indexed"));
    let form: WebSearchDeclaration =
        for_model("openai-responses", &model).ok_or("search declaration")?;
    for flag in [json!(false), json!("true"), Value::Null] {
        assert!(
            form.admit_native_request(
                "/alpha/search",
                &body,
                &BTreeMap::from([("supports_standalone_web_search".into(), flag)]),
                "indexed"
            )
            .is_err()
        );
    }
    assert!(
        form.admit_native_request(
            "/alpha/search",
            &body,
            &BTreeMap::from([("supports_standalone_web_search".into(), json!(true))]),
            "cached"
        )
        .is_err()
    );
    assert!(
        outgoing_with_access_program(
            "openai-responses",
            "/responses",
            &model,
            body.to_string().as_bytes(),
            None
        )
        .is_err()
    );
    let mut extra = body.clone();
    extra["settings"]["undeclared_option"] = json!(true);
    assert!(
        outgoing_with_access_program(
            "openai-responses",
            "/alpha/search",
            &model,
            extra.to_string().as_bytes(),
            None
        )
        .is_err()
    );
    model.web_search = Some(vec!["cached".into()]);
    assert!(
        outgoing_with_access_program(
            "openai-responses",
            "/alpha/search",
            &model,
            body.to_string().as_bytes(),
            None
        )
        .is_err()
    );
    model.web_search = Some(vec!["indexed".into()]);
    model.request_options = Some(BTreeMap::from([(
        "native".into(),
        json!({
            "type":"object","properties":{"web_search":{"type":"object","properties":{"filters":{"const":{"allowed_domains":["only.example.invalid"]}}}}}
        }),
    )]));
    assert!(
        outgoing_with_access_program(
            "openai-responses",
            "/alpha/search",
            &model,
            body.to_string().as_bytes(),
            None
        )
        .is_err()
    );
    Ok(())
}
