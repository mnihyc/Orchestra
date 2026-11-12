use orch_model_contract::parameters::{constraints, outgoing_at, resolve};
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};

fn model(value: Value) -> ModelParameters {
    serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("the model fixture must be valid: {error}"))
}
fn sent(entry: &ModelParameters, options: Value) -> Result<Value, &'static str> {
    let mut body = options;
    body["model"] = json!("chosen");
    body["messages"] = json!([]);
    body["max_tokens"] = json!(32);
    outgoing_at(
        "anthropic-messages",
        "/v1/messages",
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
fn undeclared_capacity_keeps_old_requests_and_additional_capacity_is_closed_until_enabled() {
    for entry in [
        ModelParameters::default(),
        model(
            json!({"offered":{"capacity_tiers":[{"id":"auto","label":"Priority where available"}]}}),
        ),
    ] {
        assert!(
            sent(&entry, json!({}))
                .unwrap()
                .get("service_tier")
                .is_none()
        );
        assert!(sent(&entry, json!({"service_tier":"auto"})).is_err());
        assert!(
            resolve(
                "anthropic-messages",
                &entry,
                &json!({}),
                &json!({"service_tier":"auto"})
            )
            .is_err()
        );
        assert_eq!(
            sent(&entry, json!({"service_tier":"standard_only"})).unwrap()["service_tier"],
            "standard_only"
        );
    }
    assert!(
        serde_json::to_value(ModelParameters::default())
            .unwrap()
            .get("capacity_tiers")
            .is_none()
    );
}

#[test]
fn enabled_capacity_and_speed_keep_independent_defaults_choices_and_forms() {
    let entry = model(
        json!({"service_tiers":[{"id":"fast","label":"Fast"}],"default_service_tier":"fast",
        "capacity_tiers":[{"id":"auto","label":"Priority where available"}],"default_capacity_tier":"auto"}),
    );
    let body = sent(&entry, json!({})).unwrap();
    assert_eq!(body["speed"], "fast");
    assert_eq!(body["service_tier"], "auto");
    let ordinary = sent(&entry, json!({"service_tier":null})).unwrap();
    assert_eq!(ordinary["service_tier"], "standard_only");
    assert_eq!(ordinary["speed"], "fast");
    let ordinary_speed = sent(&entry, json!({"speed":null})).unwrap();
    assert!(ordinary_speed.get("speed").is_none());
    assert_eq!(ordinary_speed["service_tier"], "auto");
    let explicit = resolve(
        "anthropic-messages",
        &entry,
        &json!({"service_tier":"standard_only"}),
        &json!({}),
    )
    .unwrap();
    assert_eq!(explicit["service_tier"], "auto");
    let form = constraints("anthropic-messages", &entry);
    assert_eq!(
        form["properties"]["service_tier"]["enum"],
        json!([null, "standard_only", "auto"])
    );
    assert_eq!(
        form["properties"]["speed"]["enum"],
        json!(["standard", "fast"])
    );
    assert_eq!(form["properties"]["service_tier"]["default"], "auto");
}

#[test]
fn false_and_schema_limits_narrow_capacity_without_breaking_omitted_fields_or_counting() {
    let mut entry = model(
        json!({"capacity_tiers":[{"id":"auto","label":"Automatic"}],"default_capacity_tier":"auto",
        "request_options":{"anthropic-messages":{"$defs":{"absent":false},"properties":{"service_tier":{"$ref":"#/$defs/absent"}}}}}),
    );
    assert!(
        sent(&entry, json!({}))
            .unwrap()
            .get("service_tier")
            .is_none()
    );
    assert!(sent(&entry, json!({"service_tier":"auto"})).is_err());
    entry.request_options = None;
    let counted: Value = serde_json::from_slice(
        &outgoing_at(
            "anthropic-messages",
            "/v1/messages/count_tokens",
            &entry,
            br#"{"model":"chosen","messages":[]}"#,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(counted.get("service_tier").is_none());
    assert!(counted.get("speed").is_none());
    assert!(
        outgoing_at(
            "anthropic-messages",
            "/v1/messages/count_tokens",
            &entry,
            br#"{"model":"chosen","messages":[],"service_tier":"auto"}"#
        )
        .is_err()
    );
    let declared = model(json!({"capacity_tiers":[]}));
    assert_eq!(
        sent(&declared, json!({})).unwrap()["service_tier"],
        "standard_only"
    );
    for value in [json!(false), json!(1), json!(""), json!("priority")] {
        assert!(sent(&entry, json!({"service_tier":value})).is_err());
    }
}
