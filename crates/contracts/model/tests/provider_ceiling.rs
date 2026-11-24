use orch_model_contract::parameters::{apply_embedding, constraints, outgoing, resolve};
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};

#[test]
fn final_outgoing_values_obey_each_composed_endpoint_group_and_model_schema() {
    use orch_provider_contract::models::{RequestOptions, compose_request_options};
    let endpoint: RequestOptions = serde_json::from_value(json!({"openai-chat": {
        "type":"object","additionalProperties":false,"properties":{"temperature":{"type":"number","minimum":0,"maximum":1}}
    }}))
    .unwrap();
    let group: RequestOptions = serde_json::from_value(json!({"openai-chat": {
        "type":"object","additionalProperties":false,"properties":{"temperature":{"type":"number","maximum":0.5}}
    }}))
    .unwrap();
    let local = model(json!({"request_options":{"openai-chat":{
        "type":"object","properties":{"temperature":{"type":"number","default":0.25,"maximum":2}}
    }}}));
    let composed = compose_request_options(&local, Some(&endpoint), [&group]);
    let ordinary = resolve("openai-chat", &composed, &json!({}), &json!({})).unwrap();
    assert_eq!(ordinary["temperature"], 0.25);
    for temperature in [0.0, 0.25, 0.5] {
        let wire = outgoing(
            "openai-chat",
            &composed,
            &serde_json::to_vec(
                &json!({"model":"fixture-model","messages":[],"temperature":temperature}),
            )
            .unwrap(),
        )
        .unwrap();
        let body: Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(body["temperature"], temperature);
    }
    for temperature in [-1.0, 0.75, 1.5] {
        assert!(
            resolve(
                "openai-chat",
                &composed,
                &json!({}),
                &json!({"temperature":temperature})
            )
            .is_err()
        );
        assert!(
            outgoing(
                "openai-chat",
                &composed,
                &serde_json::to_vec(
                    &json!({"model":"fixture-model","messages":[],"temperature":temperature})
                )
                .unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn referenced_endpoint_rules_keep_their_own_local_roots_through_final_normalization() {
    use orch_provider_contract::models::{RequestOptions, compose_request_options};
    let endpoint: RequestOptions = serde_json::from_value(json!({"openai-chat": {
        "$defs":{"count":{"type":"number","maximum":0.5},"options":{
            "type":"object","additionalProperties":false,"properties":{"temperature":{"$ref":"#/$defs/count"},"service_tier":{"enum":["default"]}}}},
        "$ref":"#/$defs/options"
    }})).unwrap();
    let local = model(
        json!({"request_options":{"openai-chat":{"type":"object","properties":{"temperature":{"type":"number","default":0.25}}}}}),
    );
    let composed = compose_request_options(&local, Some(&endpoint), []);
    assert_eq!(
        resolve("openai-chat", &composed, &json!({}), &json!({})).unwrap()["temperature"],
        0.25
    );
    for temperature in [0.0, 0.5] {
        assert!(
            outgoing(
                "openai-chat",
                &composed,
                &serde_json::to_vec(
                    &json!({"model":"fixture-model","messages":[],"temperature":temperature})
                )
                .unwrap()
            )
            .is_ok()
        );
    }
    assert!(
        outgoing(
            "openai-chat",
            &composed,
            &serde_json::to_vec(&json!({"model":"fixture-model","messages":[],"temperature":0.75}))
                .unwrap()
        )
        .is_err()
    );
}

#[test]
fn a_translated_incoming_interface_cannot_discard_the_actual_endpoints_option_rule() {
    let layers = json!({"endpoints":{"openai-chat":{"openai-chat":{
        "type":"object","properties":{"temperature":{"type":"number","maximum":0.5}}
    }}},"groups":{}});
    let frozen = orch_provider_contract::models::composed_model_parameters(
        &json!({}),
        &[],
        &layers,
        Some("incoming-via-translator"),
    )
    .unwrap();
    for temperature in [0.25, 0.75] {
        let answer = outgoing(
            "openai-chat",
            &frozen,
            &serde_json::to_vec(
                &json!({"model":"translated-model","messages":[],"temperature":temperature}),
            )
            .unwrap(),
        );
        assert_eq!(answer.is_ok(), temperature == 0.25);
    }
}

fn model(value: Value) -> ModelParameters {
    serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("the model fixture must be valid: {error}"))
}

#[test]
fn final_options_enforce_enabled_tiers_and_programs_after_default_merging() {
    let entry = model(json!({"service_tiers":[{"id":"priority","label":"Fast"}],
        "access_programs":["daybreak_blue"]}));
    for tier in ["priority", "fast"] {
        let body = outgoing(
            "openai-responses",
            &entry,
            &serde_json::to_vec(&json!({"model":"chosen","input":"text","service_tier":tier,
                "access_programs":{"cyber":"daybreak_blue"}}))
            .unwrap(),
        )
        .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["service_tier"], tier);
        assert_eq!(body["model"], "chosen");
        assert_eq!(body["input"], "text");
    }
    for params in [
        json!({"service_tier":"ultrafast"}),
        json!({"access_programs":{"cyber":"daybreak_red"}}),
    ] {
        assert!(resolve("openai-responses", &entry, &json!({}), &params).is_err());
        let mut body = params;
        body["model"] = json!("chosen");
        assert!(
            outgoing(
                "openai-responses",
                &entry,
                &serde_json::to_vec(&body).unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn omission_null_and_auto_cannot_inherit_an_upstream_paid_default() {
    let entry = model(
        json!({"service_tiers":[{"id":"ultrafast","label":"Ultrafast"}],
        "default_service_tier":"ultrafast", "access_programs":["daybreak_blue"],
        "default_access_program":"daybreak_blue"}),
    );
    for params in [json!({}), json!({"service_tier":"auto"})] {
        let body = resolve("openai-responses", &entry, &json!({}), &params).unwrap();
        assert_eq!(body["service_tier"], "ultrafast");
        assert_eq!(body["access_programs"]["cyber"], "daybreak_blue");
    }
    let ordinary = resolve(
        "openai-responses",
        &entry,
        &json!({}),
        &json!({"service_tier":null,"access_programs":{"cyber":null}}),
    )
    .unwrap();
    assert_eq!(ordinary["service_tier"], "default");
    assert!(
        ordinary.get("access_programs").is_none(),
        "ordinary access uses omission"
    );
    let ordinary = resolve(
        "openai-responses",
        &ModelParameters::default(),
        &json!({}),
        &json!({}),
    )
    .unwrap();
    assert_eq!(ordinary["service_tier"], "default");
    assert!(
        ordinary.get("access_programs").is_none(),
        "ordinary access uses omission"
    );
}

#[test]
fn nested_allowed_fixed_and_default_options_use_the_shared_schema() {
    let entry = model(json!({"request_options":{"openai-chat":{
        "type":"object","additionalProperties":false,"properties":{
            "temperature":{"enum":[0,0.5],"default":0},
            "vendor":{"type":"object","additionalProperties":false,"properties":{
                "region":{"const":"local"},"mode":{"enum":["a","b"],"default":"a"}},
                "required":["region","mode"]}}}}}));
    entry.check_option_schemas().unwrap();
    let resolved = resolve(
        "openai-chat",
        &entry,
        &json!({}),
        &json!({"temperature":0.5,"vendor":{"mode":"b"}}),
    )
    .unwrap();
    assert_eq!(resolved["temperature"], 0.5);
    assert_eq!(resolved["vendor"], json!({"region":"local","mode":"b"}));
    for params in [
        json!({"vendor":{"region":"remote"}}),
        json!({"temperature":1}),
        json!({"vendor":{"mode":null}}),
        json!({"extra":true}),
    ] {
        assert!(resolve("openai-chat", &entry, &json!({}), &params).is_err());
    }
    let projected = constraints("openai-chat", &entry);
    assert_eq!(
        projected["properties"]["vendor"]["properties"]["region"]["const"],
        "local"
    );
    assert_eq!(
        projected["properties"]["service_tier"]["enum"],
        json!(["default"])
    );
    assert_eq!(projected["properties"]["temperature"]["default"], 0);
}

#[test]
fn anthropic_speed_is_independent_of_capacity_tier_and_native_parameter_names() {
    let entry = model(json!({"service_tiers":[{"id":"fast","label":"Fast"}]}));
    let body = resolve(
        "anthropic-messages",
        &entry,
        &json!({}),
        &json!({"speed":"fast","service_tier":"standard_only"}),
    )
    .unwrap();
    assert_eq!(body["speed"], "fast");
    assert_eq!(body["service_tier"], "standard_only");
    assert!(body.get("serviceTierForTurn").is_none());
    assert!(body.get("cyberAccessProgram").is_none());
}

#[test]
fn embeddings_cannot_replace_the_float_decoder_format() {
    let body =
        br#"{"model":"vectors","input":["text"],"encoding_format":"float","dimensions":256}"#;
    assert!(
        apply_embedding(
            "openai-embeddings",
            body,
            &json!({"encoding_format":"base64"}),
        )
        .is_err()
    );
    assert!(apply_embedding("openai-embeddings", body, &json!({"dimensions":512})).is_err());
    assert!(
        apply_embedding(
            "openai-embeddings",
            body,
            &json!({"encoding_format":"float"}),
        )
        .is_ok()
    );
}

#[test]
fn local_schema_references_and_effort_aliases_cannot_escape_final_constraints() {
    let entry = model(
        json!({"efforts":["low","high"],"service_tiers":[{"id":"priority","label":"Fast"}],
        "request_options":{"openai-responses":{"type":"object","$defs":{
            "tier":{"enum":["default","priority"]}, "region":{"const":"local"}},
            "properties":{"service_tier":{"$ref":"#/$defs/tier"},
                "region":{"$ref":"#/$defs/region"},
                "reasoning":{"type":"object","properties":{"effort":{"const":"low"}}}}}}}),
    );
    let body = outgoing(
        "openai-responses",
        &entry,
        br#"{"model":"chosen","input":"text","service_tier":"priority"}"#,
    )
    .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["region"], "local");
    assert_eq!(body["reasoning"]["effort"], "low");
    assert!(
        outgoing(
            "openai-responses",
            &entry,
            br#"{"model":"chosen","input":"text","effort":"high"}"#
        )
        .is_err()
    );
}

#[test]
fn option_declarations_cannot_require_hidden_conditional_combinations() {
    let entry = model(
        json!({"request_options":{"openai-chat":{"allOf":[{"properties":{"mode":{"const":"a"}}}]}}}),
    );
    assert!(entry.check_option_schemas().is_err());
}

#[test]
fn identifying_fields_are_options_and_are_never_filled_without_a_provider_choice() {
    for protocol in ["openai-responses", "openai-chat", "anthropic-messages"] {
        let selected =
            json!({"user":"chosen","safety_identifier":"chosen","metadata":{"user_id":"chosen"}});
        let resolved =
            resolve(protocol, &ModelParameters::default(), &json!({}), &selected).unwrap();
        for field in ["user", "safety_identifier", "metadata"] {
            assert_eq!(resolved[field], selected[field]);
        }
        let empty = resolve(
            protocol,
            &ModelParameters::default(),
            &json!({}),
            &json!({}),
        )
        .unwrap();
        for field in ["user", "safety_identifier", "metadata"] {
            assert!(empty.get(field).is_none());
        }
    }
}

#[test]
fn identifying_fields_obey_plain_const_and_false_rules_in_both_call_paths() {
    let entry = model(json!({"request_options":{"openai-responses":{"properties":{
        "user":{"const":"selected"},"safety_identifier":false,
        "metadata":{"type":"object","properties":{"user_id":{"enum":["selected"]}}}
    }}}}));
    for params in [
        json!({"user":"program-id"}),
        json!({"safety_identifier":"program-id"}),
        json!({"metadata":{"user_id":"program-id"}}),
    ] {
        assert!(resolve("openai-responses", &entry, &json!({}), &params).is_err());
        let mut body = params;
        body["model"] = json!("chosen");
        body["input"] = json!("unchanged input");
        assert!(
            outgoing(
                "openai-responses",
                &entry,
                &serde_json::to_vec(&body).unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn explicit_replacements_rewrite_or_strip_program_identifiers_before_validation() {
    let entry = model(json!({"request_options":{"anthropic-messages":{
        "$defs":{"strip":{"const":null,"x-orch-replace":true}},
        "properties":{
            "user":{"const":"provider-choice","x-orch-replace":true},
            "safety_identifier":{"$ref":"#/$defs/strip"},
            "metadata":{"type":"object","properties":{
                "user_id":{"$ref":"#/$defs/strip"},
                "tags":{"type":"array","items":{"type":"object","properties":{
                    "user":{"const":"tag-choice","x-orch-replace":true}
                }}}
            }}
        }
    }}}));
    entry.check_option_schemas().unwrap();
    let selected = json!({"user":"program-id","safety_identifier":"program-id",
        "metadata":{"user_id":"program-id","keep":"original","tags":[{"user":"program-id","keep":1}]}});
    let resolved = resolve("anthropic-messages", &entry, &json!({}), &selected).unwrap();
    let mut body = selected;
    body["model"] = json!("chosen");
    body["messages"] = json!([{"role":"user","content":"original input"}]);
    body["max_tokens"] = json!(32);
    let sent: Value = serde_json::from_slice(
        &outgoing(
            "anthropic-messages",
            &entry,
            &serde_json::to_vec(&body).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    for result in [&resolved, &sent] {
        assert_eq!(result["user"], "provider-choice");
        assert!(result.get("safety_identifier").is_none());
        assert!(result["metadata"].get("user_id").is_none());
        assert_eq!(result["metadata"]["keep"], "original");
        assert_eq!(
            result["metadata"]["tags"][0],
            json!({"user":"tag-choice","keep":1})
        );
        assert!(
            !serde_json::to_string(result)
                .unwrap()
                .contains("program-id")
        );
    }
    assert_eq!(sent["model"], body["model"]);
    assert_eq!(sent["messages"], body["messages"]);
    assert_eq!(sent["max_tokens"], body["max_tokens"]);
    let projected = constraints("anthropic-messages", &entry);
    assert_eq!(projected["properties"]["user"]["const"], "provider-choice");
    assert_eq!(projected["properties"]["user"]["x-orch-replace"], true);
}

#[test]
fn whole_metadata_replacement_discards_old_members_and_null_is_not_reinserted() {
    for replacement in [json!(null), json!({"purpose":"provider-choice"})] {
        let entry = model(json!({"request_options":{"openai-chat":{"properties":{
            "metadata":{"const":replacement,"x-orch-replace":true}
        }}}}));
        for selected in [
            json!({}),
            json!({"metadata":{"user_id":"program-id","other":"program-data"}}),
        ] {
            let resolved = resolve("openai-chat", &entry, &json!({}), &selected).unwrap();
            if replacement.is_null() {
                assert!(resolved.get("metadata").is_none());
            } else {
                assert_eq!(resolved["metadata"], replacement);
            }
        }
    }
    for invalid in [
        json!({"x-orch-replace":true}),
        json!({"const":"x","x-orch-replace":"true"}),
    ] {
        let entry =
            model(json!({"request_options":{"openai-chat":{"properties":{"user":invalid}}}}));
        assert!(entry.check_option_schemas().is_err());
    }
    let entry = model(json!({"request_options":{"openai-chat":{"properties":{
        "user":{"const":"chosen","x-orch-replace":false}
    }}}}));
    assert!(
        resolve(
            "openai-chat",
            &entry,
            &json!({}),
            &json!({"user":"program-id"})
        )
        .is_err()
    );
}

#[test]
fn declared_model_efforts_bound_aliases_and_their_dropdown_projection() {
    let entry = model(json!({"efforts":["low"]}));
    for params in [
        json!({"effort":"high"}),
        json!({"reasoning_effort":"high"}),
        json!({"reasoning":{"effort":"high"}}),
    ] {
        assert!(resolve("openai-responses", &entry, &json!({}), &params).is_err());
    }
    let allowed = resolve(
        "openai-responses",
        &entry,
        &json!({}),
        &json!({"effort":"low"}),
    )
    .unwrap();
    assert_eq!(allowed["reasoning"]["effort"], "low");
    assert!(allowed.get("effort").is_none());
    assert_eq!(
        constraints("openai-responses", &entry)["properties"]["reasoning"]["properties"]["effort"]
            ["enum"],
        json!([null, "low"])
    );
}

#[test]
fn gemini_compatible_requests_keep_tier_effort_and_nested_defaults_with_per_use_overrides() {
    let configured = model(json!({
        "efforts":["low","high"], "default_effort":"high",
        "service_tiers":[{"id":"priority","label":"Fast"},{"id":"flex","label":"Flex"}],
        "default_service_tier":"priority",
        "request_options":{"openai-chat":{"type":"object","properties":{
            "extra_body":{"type":"object","properties":{"google":{"type":"object","properties":{
                "cached_content":{"type":"string","default":"cachedContents/provider-default"}
            }}}}
        }}}
    }));
    let ordinary = resolve("openai-chat", &configured, &json!({}), &json!({})).unwrap();
    assert_eq!(ordinary["service_tier"], "priority");
    assert_eq!(ordinary["reasoning_effort"], "high");
    assert_eq!(
        ordinary["extra_body"]["google"]["cached_content"],
        "cachedContents/provider-default"
    );
    let selected = resolve("openai-chat", &configured, &json!({}), &json!({
        "service_tier":"flex", "effort":"low", "extra_body":{"google":{"cached_content":"cachedContents/chosen"}}
    })).unwrap();
    let wire = outgoing(
        "openai-chat",
        &configured,
        &serde_json::to_vec(&json!({
            "model":"gemini-model", "messages":[], "service_tier":selected["service_tier"],
            "reasoning_effort":selected["reasoning_effort"], "extra_body":selected["extra_body"]
        }))
        .unwrap(),
    )
    .unwrap();
    let body: Value = serde_json::from_slice(&wire).unwrap();
    assert_eq!(body["service_tier"], "flex");
    assert_eq!(body["reasoning_effort"], "low");
    assert_eq!(
        body["extra_body"]["google"]["cached_content"],
        "cachedContents/chosen"
    );
    assert!(
        resolve(
            "openai-chat",
            &configured,
            &json!({}),
            &json!({"service_tier":"unoffered"})
        )
        .is_err()
    );
}

#[test]
fn deepseek_compatible_requests_keep_declared_thinking_defaults_without_an_invented_tier() {
    let configured = model(json!({
        "efforts":["none","low","high","max"], "default_effort":"high",
        "request_options":{"openai-chat":{"type":"object","properties":{
            "service_tier":false,
            "thinking":{"type":"object","properties":{"type":{"enum":["enabled","disabled"],"default":"enabled"}}},
            "response_format":{"type":"object","properties":{"type":{"enum":["text","json_object"],"default":"text"}}}
        }}}
    }));
    let defaults = resolve("openai-chat", &configured, &json!({}), &json!({})).unwrap();
    assert!(defaults.get("service_tier").is_none());
    assert_eq!(defaults["reasoning_effort"], "high");
    assert_eq!(defaults["thinking"]["type"], "enabled");
    assert_eq!(defaults["response_format"]["type"], "text");
    let selected = resolve("openai-chat", &configured, &json!({}), &json!({
        "effort":"none", "thinking":{"type":"disabled"}, "response_format":{"type":"json_object"}
    })).unwrap();
    let wire = outgoing(
        "openai-chat",
        &configured,
        &serde_json::to_vec(&selected).unwrap(),
    )
    .unwrap();
    let body: Value = serde_json::from_slice(&wire).unwrap();
    assert!(body.get("service_tier").is_none());
    assert_eq!(body["thinking"]["type"], "disabled");
    assert_eq!(body["response_format"]["type"], "json_object");
    assert_eq!(body["reasoning_effort"], "none");
    let form = orch_model_contract::parameters::thread::forms(&configured);
    assert!(form["openai-chat"]["properties"].get("thinking").is_some());
    assert!(
        form["openai-chat"]["properties"]
            .get("service_tier")
            .is_none()
    );
}
