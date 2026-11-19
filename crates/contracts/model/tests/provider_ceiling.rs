use orch_model_contract::parameters::{apply_embedding, constraints, outgoing, resolve};
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};

fn model(value: Value) -> ModelParameters {
    serde_json::from_value(value).unwrap()
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
    assert_eq!(ordinary["access_programs"]["cyber"], "standard");
    let ordinary = resolve(
        "openai-responses",
        &ModelParameters::default(),
        &json!({}),
        &json!({}),
    )
    .unwrap();
    assert_eq!(ordinary["service_tier"], "default");
    assert_eq!(ordinary["access_programs"]["cyber"], "standard");
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
