//! Omission preserves ordinary behavior when a format/model does not accept a wire selection.
#![allow(clippy::unwrap_used, reason = "a contract control fails by panicking")]
use orch_model_contract::parameters::{outgoing_at, resolve};
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};
fn model(value: Value) -> ModelParameters {
    serde_json::from_value(value).unwrap()
}

#[test]
fn forbidden_tier_fields_do_not_acquire_defaults_but_explicit_values_still_refuse() {
    for (protocol, path, field) in [
        ("openai-responses", "/responses", "service_tier"),
        ("openai-chat", "/chat/completions", "service_tier"),
        ("anthropic-messages", "/v1/messages", "speed"),
    ] {
        let entry = model(
            json!({"service_tiers":[{"id":"fast","label":"Fast"}],"default_service_tier":"fast",
            "request_options":{(protocol):{"type":"object","properties":{(field):false}}}}),
        );
        let resolved = resolve(protocol, &entry, &json!({}), &json!({})).unwrap();
        assert!(resolved.get(field).is_none(), "{protocol}: {resolved}");
        let raw = json!({"model":"selected","input":"native input","messages":[],"max_tokens":128});
        let outgoing: Value = serde_json::from_slice(
            &outgoing_at(protocol, path, &entry, &serde_json::to_vec(&raw).unwrap()).unwrap(),
        )
        .unwrap();
        assert!(outgoing.get(field).is_none(), "{protocol}: {outgoing}");
        assert_eq!(outgoing["model"], raw["model"]);
        assert_eq!(outgoing["input"], raw["input"]);
        for value in [
            json!(null),
            json!("auto"),
            json!("default"),
            json!("standard"),
            json!("fast"),
        ] {
            assert!(resolve(protocol, &entry, &json!({}), &json!({(field):value})).is_err());
        }
    }
}

#[test]
fn a_forbidden_program_object_does_not_receive_standard_or_a_cross_endpoint_paid_start() {
    let entry = model(
        json!({"access_programs":["daybreak_blue"],"default_access_program":"daybreak_blue",
        "request_options":{"openai-responses":{"type":"object","properties":{"access_programs":false}}}}),
    );
    let resolved = resolve("openai-responses", &entry, &json!({}), &json!({})).unwrap();
    assert!(resolved.get("access_programs").is_none());
    let raw = json!({"model":"selected","input":"x"});
    let outgoing: Value = serde_json::from_slice(
        &outgoing_at(
            "openai-responses",
            "/responses",
            &entry,
            &serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(outgoing.get("access_programs").is_none());
    for value in [
        json!({}),
        json!({"cyber":"standard"}),
        json!({"cyber":"daybreak_blue"}),
    ] {
        assert!(
            resolve(
                "openai-responses",
                &entry,
                &json!({}),
                &json!({"access_programs":value})
            )
            .is_err()
        );
    }
}

#[test]
fn ordinary_message_speed_is_omitted_and_an_enabled_fast_choice_is_retained() {
    for entry in [
        ModelParameters::default(),
        model(
            json!({"service_tiers":[{"id":"fast","label":"Fast"}],"default_service_tier":"fast"}),
        ),
    ] {
        for selected in [json!({"speed":"standard"}), json!({"speed":null})] {
            let resolved = resolve("anthropic-messages", &entry, &json!({}), &selected).unwrap();
            assert!(resolved.get("speed").is_none(), "{resolved}");
            let body = json!({"model":"selected","messages":[],"max_tokens":128,"speed":selected["speed"]});
            let outgoing: Value = serde_json::from_slice(
                &outgoing_at(
                    "anthropic-messages",
                    "/v1/messages",
                    &entry,
                    &serde_json::to_vec(&body).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(outgoing.get("speed").is_none(), "{outgoing}");
        }
    }
    for selected in [json!({}), json!({"speed":"auto"})] {
        assert!(
            resolve(
                "anthropic-messages",
                &ModelParameters::default(),
                &json!({}),
                &selected
            )
            .unwrap()
            .get("speed")
            .is_none()
        );
    }
    let fast = model(
        json!({"service_tiers":[{"id":"fast","label":"Fast"}],"default_service_tier":"fast"}),
    );
    for selected in [json!({}), json!({"speed":"fast"}), json!({"speed":"auto"})] {
        assert_eq!(
            resolve("anthropic-messages", &fast, &json!({}), &selected).unwrap()["speed"],
            "fast"
        );
    }
    assert!(
        resolve(
            "anthropic-messages",
            &ModelParameters::default(),
            &json!({}),
            &json!({"speed":"fast"})
        )
        .is_err()
    );
}

#[test]
fn false_referenced_rules_do_not_make_side_endpoints_require_a_removed_option() {
    let compact = model(
        json!({"access_programs":["daybreak_blue"],"default_access_program":"daybreak_blue",
        "request_options":{"openai-responses":{"type":"object","$defs":{"unsupported":false},"properties":{"access_programs":{"$ref":"#/$defs/unsupported"}}}}}),
    );
    let body = outgoing_at(
        "openai-responses",
        "/responses/compact",
        &compact,
        br#"{"model":"selected","input":[]}"#,
    )
    .unwrap();
    assert!(
        serde_json::from_slice::<Value>(&body)
            .unwrap()
            .get("access_programs")
            .is_none()
    );
    let count = model(
        json!({"service_tiers":[{"id":"fast","label":"Fast"}],"default_service_tier":"fast",
        "request_options":{"anthropic-messages":{"type":"object","properties":{"speed":false}}}}),
    );
    let body = outgoing_at(
        "anthropic-messages",
        "/v1/messages/count_tokens",
        &count,
        br#"{"model":"selected","messages":[]}"#,
    )
    .unwrap();
    assert!(
        serde_json::from_slice::<Value>(&body)
            .unwrap()
            .get("speed")
            .is_none()
    );
}
