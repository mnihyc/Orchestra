#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_model_contract::web_search::WebSearchDeclaration;
use serde_json::json;

#[test]
fn an_indexed_program_mode_does_not_invent_support_on_a_wire_protocol() {
    let declared = orch_model_contract::web_search::declared("anthropic-messages").unwrap();
    let mut body =
        json!({"tools":[{"name":"keep_this_function","input_schema":{"type":"object"}}]});
    assert!(
        declared
            .apply("/messages", &mut body, &json!("indexed"))
            .is_err()
    );
    assert_eq!(body["tools"][0]["name"], "keep_this_function");
}

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

#[test]
fn endpoint_native_bindings_narrow_modes_and_strip_only_declared_wire_mode_members() {
    use orch_model_contract::parameters::outgoing;
    use orch_provider_contract::models::ModelParameters;
    let protocol = "openai-responses";
    let request = json!({"paths":["/responses"],"tool":{"type":"fixture_search"},"identities":[{"type":"fixture_search"}],"modes":{"live":{}},
        "strip_options":["external_web_access"],"options_schema":{"type":"object","properties":{},"additionalProperties":false,"x-orch-mode-overrides":[{"identity":{"external_web_access":false},"mode":"cached"}]}});
    let mut model:ModelParameters=serde_json::from_value(json!({"web_search":["live"],"native_tools":{(protocol):{"web_search":{"request":request}}}})).unwrap();
    let body = json!({"model":"fixture-model","tools":[{"type":"function","name":"local","parameters":{}},{"type":"fixture_search","external_web_access":true}]});
    let sent: serde_json::Value =
        serde_json::from_slice(&outgoing(protocol, &model, body.to_string().as_bytes()).unwrap())
            .unwrap();
    assert_eq!(sent["tools"][0], body["tools"][0]);
    assert_eq!(sent["tools"][1], json!({"type":"fixture_search"}));
    let mut cached = body.clone();
    cached["tools"][1]["external_web_access"] = json!(false);
    assert!(outgoing(protocol, &model, cached.to_string().as_bytes()).is_err());
    let mut unsupported = body.clone();
    unsupported["tools"][1]["search_context_size"] = json!("low");
    assert!(
        outgoing(protocol, &model, unsupported.to_string().as_bytes()).is_err(),
        "known incompatible options cannot disappear from the final ceiling"
    );
    model
        .native_tools
        .get_mut(protocol)
        .unwrap()
        .insert("web_search".into(), None);
    model.default_web_search = Some("live".into());
    assert!(
        outgoing(
            protocol,
            &model,
            json!({"model":"fixture-model"}).to_string().as_bytes()
        )
        .is_ok(),
        "a stored unsupported search start does not invalidate an ordinary request"
    );
    assert!(
        outgoing(
            protocol,
            &model,
            json!({"model":"fixture-model","tools":[{"type":"web_search"}]})
                .to_string()
                .as_bytes()
        )
        .is_err()
    );
}

#[test]
fn an_endpoint_declared_native_binding_normalizes_modes_even_without_a_stock_protocol_search_grammar()
 {
    use orch_model_contract::parameters::{constrain, outgoing};
    use orch_provider_contract::models::ModelParameters;
    let protocol = "openai-chat";
    let model: ModelParameters = serde_json::from_value(json!({"web_search":["live"],"native_tools":{(protocol):{"web_search":{"request":{"paths":["/chat/completions"],"tool":{"type":"fixture_search"},"identities":[{"type":"fixture_search"}],"modes":{"live":{}},"options_schema":{"type":"object","properties":{},"additionalProperties":false}}}}}})).unwrap();
    let options = constrain(protocol, &model, &json!({"web_search":"live"})).unwrap();
    assert_eq!(options["web_search"], json!({"mode":"live"}));
    let body = json!({"model":"fixture-model","messages":[],"tools":[{"type":"fixture_search"}]});
    let sent: serde_json::Value =
        serde_json::from_slice(&outgoing(protocol, &model, body.to_string().as_bytes()).unwrap())
            .unwrap();
    assert_eq!(sent["tools"], body["tools"]);
    assert!(sent.get("web_search").is_none());
}

fn multiple_native_forms() -> serde_json::Value {
    let common = json!({"type":"object","additionalProperties":false,"properties":{
        "engine":{"type":"string"},"engine_mode":{"type":"string"},
        "allowed_domains":{"type":"array","items":{"type":"string"}},
        "search_context_size":{"enum":["low","medium","high"]}}});
    let mut plugin = common.clone();
    plugin["properties"]["search_prompt"] = json!({"type":"string"});
    let mut server = common;
    server["properties"]["max_uses"] = json!({"type":"integer","minimum":0});
    json!({"paths":["/chat/completions"],"container":"/plugins","tool":{"id":"fixture_lookup"},"identities":[{"id":"fixture_lookup"}],
        "modes":{"live":{}},"options_schema":plugin,"option_names":{"engine_mode":"mode","allowed_domains":"include_domains"},
        "root_option_names":{"search_context_size":"/fixture_lookup_options/search_context_size"},
        "root_selection_identity":{"fixture_lookup_options":null},"model_variant_identity":"fixture_lookup",
        "forms":[{"paths":["/chat/completions"],"container":"/tools","options_pointer":"/parameters","optional_options_object":true,
            "tool":{"type":"fixture_native_lookup","parameters":{}},"identities":[{"type":"fixture_native_lookup"}],"modes":{"live":{}},
            "options_schema":server,"option_names":{"engine_mode":"mode"}}]})
}
fn model_with_native_forms() -> orch_provider_contract::models::ModelParameters {
    serde_json::from_value(json!({"web_search":["live"],"native_tools":{"openai-chat":{"web_search":{"request":multiple_native_forms()}}}})).unwrap()
}

#[test]
fn declared_containers_and_option_locations_preserve_client_tools_and_other_plugins() {
    let declaration: WebSearchDeclaration =
        serde_json::from_value(multiple_native_forms()).unwrap();
    let original = json!({"model":"opaque","tools":[{"type":"function","name":"fixture_native_lookup","parameters":{"opaque":true}}],"plugins":[{"id":"other","opaque":{"kept":true}}]});
    let mut body = original.clone();
    declaration.apply("/chat/completions",&mut body,&json!({"mode":"live","engine_mode":"deep","allowed_domains":["example.invalid"],"search_context_size":"high"})).unwrap();
    assert_eq!(body["tools"], original["tools"]);
    assert_eq!(body["plugins"][0], original["plugins"][0]);
    assert_eq!(
        body["plugins"][1],
        json!({"id":"fixture_lookup","mode":"deep","include_domains":["example.invalid"]})
    );
    assert_eq!(
        body["fixture_lookup_options"],
        json!({"search_context_size":"high"})
    );
    let selected = declaration.selection(&body).unwrap();
    assert_eq!(
        selected["mode"], "live",
        "the engine mode never becomes the capability mode"
    );
    assert_eq!(selected["engine_mode"], "deep");
    assert_eq!(selected["allowed_domains"], json!(["example.invalid"]));
    assert_eq!(selected["search_context_size"], "high");
    declaration
        .apply("/chat/completions", &mut body, &json!("disabled"))
        .unwrap();
    assert_eq!(body, original);
}

#[test]
fn native_forms_keep_their_own_ceiling_inside_the_shared_model_option_schema() {
    use orch_model_contract::parameters::outgoing;
    let mut model = model_with_native_forms();
    model.request_options = Some(std::collections::BTreeMap::from([(
        "openai-chat".into(),
        json!({
            "type":"object", "$defs":{"engine_depth":{"const":"fast"}},
            "properties":{"web_search":{
                "type":"object", "properties":{"engine_mode":{"$ref":"#/$defs/engine_depth"}}
            }}
        }),
    )]));
    let body = json!({"model":"opaque","tools":[{"type":"fixture_native_lookup","parameters":{"mode":"fast","max_uses":2}}],"plugins":[{"id":"other"}]});
    let sent: serde_json::Value = serde_json::from_slice(
        &outgoing("openai-chat", &model, body.to_string().as_bytes()).unwrap(),
    )
    .unwrap();
    assert_eq!(sent["tools"], body["tools"]);
    assert_eq!(sent["plugins"], body["plugins"]);
    let mut forbidden = body.clone();
    forbidden["tools"][0]["parameters"]["mode"] = json!("deep");
    assert!(outgoing("openai-chat", &model, forbidden.to_string().as_bytes()).is_err());
    let mut wrong_form = body;
    wrong_form["tools"][0]["parameters"]["search_prompt"] = json!("plugin-only option");
    assert!(
        outgoing("openai-chat", &model, wrong_form.to_string().as_bytes()).is_err(),
        "a union schema cannot smuggle another form's fields into the active form"
    );
}

#[test]
fn duplicate_native_forms_refuse_without_mistaking_a_same_named_client_function() {
    let declaration: WebSearchDeclaration =
        serde_json::from_value(multiple_native_forms()).unwrap();
    assert_eq!(
        declaration
            .selected_mode(&json!({"tools":[{"type":"function","name":"fixture_native_lookup"}]}))
            .unwrap(),
        "disabled"
    );
    let both = json!({"tools":[{"type":"fixture_native_lookup","parameters":{}}],"plugins":[{"id":"fixture_lookup"}]});
    assert!(declaration.selected_mode(&both).is_err());
    let repeated = json!({"plugins":[{"id":"fixture_lookup"},{"id":"fixture_lookup"}]});
    assert!(declaration.selection(&repeated).is_err());
}

#[test]
fn executable_model_variants_are_never_silently_renamed_or_allowed_through_a_disabled_ceiling() {
    use orch_model_contract::parameters::outgoing;
    let declaration: WebSearchDeclaration =
        serde_json::from_value(multiple_native_forms()).unwrap();
    let mut body = json!({"model":"opaque:fixture_lookup","plugins":[{"id":"other"}]});
    let before = body.clone();
    assert_eq!(declaration.selected_mode(&body).unwrap(), "live");
    assert!(
        declaration
            .apply("/chat/completions", &mut body, &json!("disabled"))
            .is_err()
    );
    assert_eq!(body, before);
    let model = model_with_native_forms();
    let sent: serde_json::Value = serde_json::from_slice(
        &outgoing("openai-chat", &model, body.to_string().as_bytes()).unwrap(),
    )
    .unwrap();
    let mut expected = before.clone();
    expected["service_tier"] = json!("default");
    assert_eq!(
        sent, expected,
        "ordinary protocol defaults preserve the implicit variant and existing plugins without inventing a tool"
    );
    let mut disabled = model.clone();
    disabled.web_search = Some(vec![]);
    assert!(outgoing("openai-chat", &disabled, body.to_string().as_bytes()).is_err());
    let mut fixed = model;
    fixed.request_options = Some(std::collections::BTreeMap::from([(
        "openai-chat".into(),
        json!({"type":"object","properties":{"web_search":{"properties":{"mode":{"const":"disabled"}}}}}),
    )]));
    assert!(outgoing("openai-chat", &fixed, body.to_string().as_bytes()).is_err());
}

#[test]
fn declared_root_intent_and_optional_nested_options_use_the_same_final_request_check() {
    use orch_model_contract::parameters::outgoing;
    let declaration: WebSearchDeclaration =
        serde_json::from_value(multiple_native_forms()).unwrap();
    let mut root = json!({"model":"opaque","fixture_lookup_options":{"search_context_size":"high"},"plugins":[{"id":"other"}]});
    assert_eq!(
        declaration.selection(&root).unwrap()["search_context_size"],
        "high"
    );
    declaration
        .apply("/chat/completions", &mut root, &json!("disabled"))
        .unwrap();
    assert!(root.get("fixture_lookup_options").is_none());
    assert_eq!(root["plugins"], json!([{"id":"other"}]));
    assert!(
        declaration
            .selected_mode(&json!({"fixture_lookup_options":[]}))
            .is_err()
    );
    let model = model_with_native_forms();
    let body = json!({"model":"opaque","tools":[{"type":"fixture_native_lookup"}]});
    let sent: serde_json::Value = serde_json::from_slice(
        &outgoing("openai-chat", &model, body.to_string().as_bytes()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        sent["tools"][0],
        json!({"type":"fixture_native_lookup","parameters":{}})
    );
}

#[test]
fn a_native_option_pointer_cannot_rewrite_the_owned_routing_model() {
    let mut source = multiple_native_forms();
    source["root_option_names"]["search_context_size"] = json!("/model");
    let declaration: WebSearchDeclaration = serde_json::from_value(source).unwrap();
    let mut body = json!({"model":"retained-model"});
    assert!(
        declaration
            .apply(
                "/chat/completions",
                &mut body,
                &json!({"mode":"live","search_context_size":"high"})
            )
            .is_err()
    );
    assert_eq!(body["model"], "retained-model");
}
