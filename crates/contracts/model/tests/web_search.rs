#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_model_contract::web_search::WebSearchDeclaration;
use serde_json::json;

#[test]
fn a_protocol_can_declare_nested_search_options_without_shared_vendor_code() {
    let declaration: WebSearchDeclaration = serde_json::from_value(json!({
        "paths":[], "path_suffixes":[":generate"], "tool":{"hostedSearch":{}}, "identities":[{"hostedSearch":null}],
        "options_pointer":"/hostedSearch", "modes":{"live":{}},
        "options_schema":{"type":"object","additionalProperties":false,"properties":{"sites":{"type":"array","items":{"type":"string"}}}}
    })).unwrap();
    let mut body = json!({"tools":[{"function":{"name":"lookup"}}]});
    declaration
        .apply(
            "/models/custom:generate",
            &mut body,
            &json!({"mode":"live","sites":["example.com"]}),
        )
        .unwrap();
    assert_eq!(body["tools"][0], json!({"function":{"name":"lookup"}}));
    assert_eq!(
        body["tools"][1],
        json!({"hostedSearch":{"sites":["example.com"]}})
    );
    assert_eq!(declaration.selected_mode(&body).unwrap(), "live");
    declaration
        .apply("/models/custom:generate", &mut body, &json!("disabled"))
        .unwrap();
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(declaration.selected_mode(&body).unwrap(), "disabled");
}

#[test]
fn search_uses_the_shared_schema_for_fixed_values_projection_and_final_bodies() {
    use orch_model_contract::parameters::{constraints, outgoing, resolve};
    use orch_provider_contract::models::ModelParameters;
    for (protocol, option, fixed) in [
        ("openai-responses", "search_context_size", json!("low")),
        ("anthropic-messages", "max_uses", json!(3)),
    ] {
        let model: ModelParameters = serde_json::from_value(json!({
            "web_search":["live"], "request_options":{(protocol):{
                "type":"object","properties":{"web_search":{
                    "type":"object","properties":{"mode":{"const":"live"},(option):{"const":fixed}},
                    "required":["mode"]}}
            }}
        }))
        .unwrap();
        model.check_option_schemas().unwrap();
        let resolved = resolve(protocol, &model, &json!({}), &json!({})).unwrap();
        assert_eq!(resolved["web_search"]["mode"], "live");
        assert_eq!(resolved["web_search"][option], fixed);
        let schema = constraints(protocol, &model);
        assert_eq!(
            schema["properties"]["web_search"]["properties"]["mode"]["const"],
            "live"
        );
        assert_eq!(
            schema["properties"]["web_search"]["properties"]["mode"]["x-orch-source"],
            "provider"
        );
        assert!(
            resolve(
                protocol,
                &model,
                &json!({}),
                &json!({"web_search":"disabled"})
            )
            .is_err()
        );
        let declaration = orch_model_contract::web_search::declared(protocol).unwrap();
        let path = &declaration.paths[0];
        let mut body = json!({"model":"chosen"});
        declaration
            .apply(path, &mut body, &resolved["web_search"])
            .unwrap();
        let sent = outgoing(protocol, &model, body.to_string().as_bytes()).unwrap();
        let sent: serde_json::Value = serde_json::from_slice(&sent).unwrap();
        assert!(sent.get("web_search").is_none());
        assert_eq!(declaration.selection(&sent).unwrap()[option], fixed);
        assert!(
            outgoing(
                protocol,
                &ModelParameters::default(),
                body.to_string().as_bytes()
            )
            .is_err()
        );
        body["tools"][0][option] = if option == "max_uses" {
            json!(4)
        } else {
            json!("high")
        };
        assert!(outgoing(protocol, &model, body.to_string().as_bytes()).is_err());
        assert!(outgoing(protocol, &model, br#"{"model":"chosen"}"#).is_err());
    }
}

#[test]
fn generation_search_policy_does_not_add_tools_to_side_requests() {
    use orch_model_contract::parameters::outgoing_at;
    use orch_provider_contract::models::ModelParameters;
    for (protocol, path) in [
        ("openai-responses", "/responses/compact"),
        ("anthropic-messages", "/v1/messages/count_tokens"),
    ] {
        let model: ModelParameters = serde_json::from_value(json!({"web_search":["live"],"default_web_search":"live",
            "request_options":{(protocol):{"type":"object","properties":{"web_search":{"properties":{"mode":{"const":"live"}}}}}}
        })).unwrap();
        let sent = outgoing_at(protocol, path, &model, br#"{"model":"chosen"}"#).unwrap();
        let sent: serde_json::Value = serde_json::from_slice(&sent).unwrap();
        assert!(sent.get("web_search").is_none());
        assert!(sent.get("tools").is_none());
    }
}

#[test]
fn legacy_search_tools_cannot_claim_cache_only_access_they_do_not_support() {
    use orch_model_contract::parameters::outgoing;
    use orch_provider_contract::models::ModelParameters;
    let model: ModelParameters = serde_json::from_value(json!({"web_search":["cached"]})).unwrap();
    let body = br#"{"model":"chosen","tools":[{"type":"web_search_preview","external_web_access":false}]}"#;
    assert!(outgoing("openai-responses", &model, body).is_err());
}

#[test]
fn native_search_uses_the_existing_parameter_constraint_projection() {
    use orch_agent_contract::parameters::web_search_parameter;
    use orch_provider_contract::models::ModelParameters;
    let model: ModelParameters = serde_json::from_value(json!({"web_search":["cached","live"],
        "request_options":{"native":{"type":"object","properties":{"web_search":{"const":"live"}}}}
    }))
    .unwrap();
    let parameter = web_search_parameter(true);
    assert_eq!(
        parameter
            .options_for(&model)
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        vec!["live"]
    );
    assert_eq!(
        parameter.projection(&model)["provider_constraint"]["const"],
        "live"
    );
    assert_eq!(parameter.start_for(&model), Some(json!("live")));
    assert!(!parameter.provider_allows(&model, &json!("disabled")));
}

#[test]
fn referenced_search_constraints_remain_bound_to_the_original_schema() {
    use orch_model_contract::parameters::{constraints, outgoing, resolve};
    use orch_provider_contract::models::ModelParameters;
    let model: ModelParameters = serde_json::from_value(json!({"web_search":["live","cached"],
        "request_options":{"openai-responses":{"type":"object","$defs":{"mode":{"const":"live"}},
            "properties":{"web_search":{"type":"object","properties":{"mode":{"$ref":"#/$defs/mode"}}}}}}
    })).unwrap();
    let params = resolve("openai-responses", &model, &json!({}), &json!({})).unwrap();
    assert_eq!(params["web_search"]["mode"], "live");
    assert_eq!(
        constraints("openai-responses", &model)["properties"]["web_search"]["properties"]["mode"]["enum"],
        json!(["live"])
    );
    assert!(
        outgoing(
            "openai-responses",
            &model,
            br#"{"model":"chosen","tools":[{"type":"web_search","external_web_access":false}]}"#
        )
        .is_err()
    );
}
