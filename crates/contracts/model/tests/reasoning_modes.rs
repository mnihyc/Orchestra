use orch_model_contract::parameters::{constraints, outgoing, resolve};
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};

fn model(value: Value) -> ModelParameters {
    serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("the model fixture must be valid: {error}"))
}

fn raw(entry: &ModelParameters, options: &Value) -> Result<Value, &'static str> {
    let mut body = options.clone();
    body["model"] = json!("chosen");
    body["input"] = json!("original input");
    outgoing(
        "openai-responses",
        entry,
        &serde_json::to_vec(&body)
            .unwrap_or_else(|error| panic!("the request fixture must encode: {error}")),
    )
    .map(|body| {
        serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("the outgoing body must be valid JSON: {error}"))
    })
}

#[test]
fn additional_reasoning_modes_are_closed_until_enabled_and_offers_never_enable_them() {
    for entry in [
        ModelParameters::default(),
        model(json!({"offered":{"reasoning_modes":["pro"]},"default_reasoning_mode":"pro"})),
    ] {
        for options in [
            json!({"reasoning":{"mode":"pro"}}),
            json!({"reasoning":{"mode":"future"}}),
        ] {
            assert!(resolve("openai-responses", &entry, &json!({}), &options).is_err());
            assert!(raw(&entry, &options).is_err());
        }
        let empty = raw(&entry, &json!({})).unwrap();
        assert!(empty.pointer("/reasoning/mode").is_none());
        for options in [
            json!({"reasoning":{"mode":"standard"}}),
            json!({"reasoning":{"mode":null}}),
        ] {
            assert_eq!(
                raw(&entry, &options).unwrap()["reasoning"]["mode"],
                "standard"
            );
        }
    }
}

#[test]
fn enabled_mode_starts_and_explicit_choices_share_the_final_form_and_ceiling() {
    let entry = model(json!({"reasoning_modes":["pro"],"default_reasoning_mode":"pro"}));
    let selected = resolve(
        "openai-responses",
        &entry,
        &json!({"reasoning":{"mode":"standard","summary":"concise"}}),
        &json!({}),
    )
    .unwrap();
    assert_eq!(
        selected["reasoning"],
        json!({"mode":"pro","summary":"concise"})
    );
    let sent = raw(&entry, &json!({})).unwrap();
    assert_eq!(sent["reasoning"]["mode"], "pro");
    assert_eq!(sent["model"], "chosen");
    assert_eq!(sent["input"], "original input");
    let ordinary = resolve(
        "openai-responses",
        &entry,
        &json!({}),
        &json!({"reasoning":{"mode":null}}),
    )
    .unwrap();
    assert_eq!(ordinary["reasoning"]["mode"], "standard");
    let form = constraints("openai-responses", &entry);
    assert_eq!(
        form["properties"]["reasoning"]["properties"]["mode"]["enum"],
        json!([null, "standard", "pro"])
    );
    assert_eq!(
        form["properties"]["reasoning"]["properties"]["mode"]["default"],
        "pro"
    );
    assert_ne!(form["properties"]["reasoning"]["x-orch-hidden"], true);
    for mode in [
        json!(""),
        json!(false),
        json!({}),
        json!([]),
        json!("future"),
    ] {
        assert!(raw(&entry, &json!({"reasoning":{"mode":mode}})).is_err());
    }
}

#[test]
fn schema_constraints_can_narrow_modes_but_never_enable_an_unopened_one() {
    for rule in [
        json!({"enum":["pro"]}),
        json!({"const":"pro"}),
        json!({"default":"pro"}),
    ] {
        let entry = model(json!({"request_options":{"openai-responses":{"properties":{
            "reasoning":{"type":"object","properties":{"mode":rule}}
        }}}}));
        assert!(raw(&entry, &json!({"reasoning":{"mode":"pro"}})).is_err());
    }
    let entry = model(
        json!({"reasoning_modes":["pro"],"request_options":{"openai-responses":{"properties":{
            "reasoning":{"properties":{"mode":{"enum":["standard"]}}}
        }}}}),
    );
    assert!(raw(&entry, &json!({"reasoning":{"mode":"pro"}})).is_err());
    assert_eq!(
        constraints("openai-responses", &entry)["properties"]["reasoning"]["properties"]["mode"]["enum"],
        json!(["standard"])
    );
}

#[test]
fn referenced_false_rules_omit_enabled_defaults_without_accepting_explicit_modes() {
    for rule in [
        json!(false),
        json!({"properties":{"mode":{"$ref":"#/$defs/absent"}}}),
    ] {
        let entry = model(
            json!({"reasoning_modes":["pro"],"default_reasoning_mode":"pro",
            "request_options":{"openai-responses":{"$defs":{"absent":false},"properties":{"reasoning":rule}}}}),
        );
        let sent = raw(&entry, &json!({})).unwrap();
        assert!(sent.pointer("/reasoning/mode").is_none());
        assert!(raw(&entry, &json!({"reasoning":{"mode":"pro"}})).is_err());
        assert!(resolve("openai-responses", &entry, &json!({}), &json!({})).is_ok());
    }
    let old: ModelParameters = serde_json::from_value(json!({"efforts":["low"]})).unwrap();
    assert!(old.reasoning_modes().is_empty());
    assert!(old.reasoning_mode_start().is_none());
    assert!(
        serde_json::to_value(old)
            .unwrap()
            .get("reasoning_modes")
            .is_none()
    );
}
