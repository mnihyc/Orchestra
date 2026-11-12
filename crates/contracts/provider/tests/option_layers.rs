//! Form specificity never removes the original endpoint or group value constraints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use orch_provider_contract::models::{ModelParameters, RequestOptions, compose_request_options};
use orch_types::schema;
use serde_json::{Value, json};

fn options(schema: Value) -> RequestOptions {
    [("fixture".into(), schema)].into_iter().collect()
}
fn entry(schema: Value) -> ModelParameters {
    ModelParameters {
        request_options: Some(options(schema)),
        ..Default::default()
    }
}

#[test]
fn an_unchanged_model_preserves_its_original_serialized_declaration() {
    let old = entry(
        json!({"type":"object","properties":{"choice":{"enum":["one","two"],"default":"one"}}}),
    );
    let composed = compose_request_options(&old, None, []);
    assert_eq!(
        serde_json::to_vec(&composed).unwrap(),
        serde_json::to_vec(&old).unwrap()
    );
    assert_eq!(
        compose_request_options(&old, Some(&RequestOptions::new()), [&RequestOptions::new()]),
        old
    );
    assert_eq!(
        compose_request_options(&ModelParameters::default(), None, []),
        ModelParameters::default()
    );
}

#[test]
fn property_forms_follow_endpoint_then_saved_group_order_then_model() {
    let endpoint = options(json!({"type":"object","properties":{
        "choice":{"enum":["a","b","c"],"default":"a"},
        "untouched":{"const":"endpoint"}}}));
    let first =
        options(json!({"type":"object","properties":{"choice":{"enum":["a","b"],"default":"a"}}}));
    let last =
        options(json!({"type":"object","properties":{"choice":{"enum":["b"],"default":"b"}}}));
    let model = entry(
        json!({"type":"object","properties":{"choice":{"const":"b","title":"Model choice"}}}),
    );
    let grouped = compose_request_options(
        &ModelParameters::default(),
        Some(&endpoint),
        [&first, &last],
    );
    assert_eq!(
        grouped.option_rule("fixture", "choice").unwrap()["default"],
        "b"
    );
    let composed = compose_request_options(&model, Some(&endpoint), [&first, &last]);
    assert_eq!(
        composed.option_rule("fixture", "choice").unwrap(),
        &json!({"const":"b","title":"Model choice"})
    );
    assert_eq!(
        composed.option_rule("fixture", "untouched").unwrap()["const"],
        "endpoint"
    );
    let schema = composed.option_schema("fixture").unwrap();
    schema::validate(schema, &json!({"choice":"b","untouched":"endpoint"})).unwrap();
    for choice in ["a", "c", "elsewhere"] {
        assert!(
            schema::validate(schema, &json!({"choice":choice,"untouched":"endpoint"})).is_err()
        );
    }
}

#[test]
fn a_more_specific_form_cannot_reopen_a_value_or_drop_a_required_property() {
    let endpoint = options(
        json!({"type":"object","required":["choice"],"properties":{"choice":{"enum":["allowed"]}}}),
    );
    let group =
        options(json!({"type":"object","properties":{"choice":{"type":"string","maxLength":8}}}));
    let model =
        entry(json!({"type":"object","properties":{"choice":{"enum":["allowed","reopened"]}}}));
    let composed = compose_request_options(&model, Some(&endpoint), [&group]);
    let schema = composed.option_schema("fixture").unwrap();
    assert_eq!(schema["required"], json!(["choice"]));
    schema::validate(schema, &json!({"choice":"allowed"})).unwrap();
    assert!(schema::validate(schema, &json!({"choice":"reopened"})).is_err());
    assert!(schema::validate(schema, &json!({})).is_err());
    let closed = options(
        json!({"type":"object","additionalProperties":false,"properties":{"choice":{"type":"string"}}}),
    );
    let extra = entry(json!({"type":"object","properties":{"new_member":{"type":"boolean"}}}));
    let composed = compose_request_options(&extra, Some(&closed), []);
    assert!(
        schema::validate(
            composed.option_schema("fixture").unwrap(),
            &json!({"new_member":true})
        )
        .is_err()
    );
}

#[test]
fn colliding_local_references_keep_each_original_schema_root_and_literal_data() {
    let endpoint = options(json!({"$defs":{"value":{"type":"integer","minimum":2}},
        "type":"object","properties":{"count":{"$ref":"#/$defs/value"}}}));
    let model = entry(
        json!({"$defs":{"value":{"type":"integer","maximum":5},"body":{"type":"object","properties":{
        "count":{"$ref":"#/$defs/value"},"literal":{"const":{"$ref":"#/literal-value"}}}}},
        "$ref":"#/$defs/body"}),
    );
    let composed = compose_request_options(&model, Some(&endpoint), []);
    let schema = composed.option_schema("fixture").unwrap();
    assert_eq!(
        composed.option_rule("fixture", "count").unwrap()["maximum"],
        5
    );
    assert_eq!(
        composed.option_rule("fixture", "literal").unwrap()["const"],
        json!({"$ref":"#/literal-value"})
    );
    for count in [2, 4, 5] {
        schema::validate(
            schema,
            &json!({"count":count,"literal":{"$ref":"#/literal-value"}}),
        )
        .unwrap();
    }
    for count in [1, 6] {
        assert!(
            schema::validate(
                schema,
                &json!({"count":count,"literal":{"$ref":"#/literal-value"}})
            )
            .is_err()
        );
    }
}

#[test]
fn an_unrelated_interface_is_kept_and_never_gets_another_interfaces_options() {
    let endpoint: RequestOptions = [(
        "another".into(),
        json!({"type":"object","properties":{"a":{"const":true}}}),
    )]
    .into_iter()
    .collect();
    let model = entry(json!({"type":"object","properties":{"b":{"const":false}}}));
    let composed = compose_request_options(&model, Some(&endpoint), []);
    assert_eq!(
        composed.option_schema("fixture"),
        model.option_schema("fixture")
    );
    assert_eq!(composed.option_schema("another"), endpoint.get("another"));
    assert_eq!(composed.web_search, model.web_search);
    assert_eq!(composed.offered, model.offered);
}

#[test]
fn endpoint_native_options_follow_the_interface_the_existing_arrangement_selected() {
    use orch_provider_contract::models::composed_model_parameters;
    let layers = json!({"endpoints":{
        "wire-one":{"wire-one":{"type":"object","properties":{"choice":{"const":"one"}}},"native":{"type":"object","properties":{"speed":{"const":"slow"}}}},
        "wire-two":{"wire-two":{"type":"object","properties":{"choice":{"const":"two"}}},"native":{"type":"object","properties":{"speed":{"const":"fast"}}}}
    },"groups":{}});
    let one = composed_model_parameters(&json!({}), &[], &layers, Some("wire-one")).unwrap();
    let two = composed_model_parameters(&json!({}), &[], &layers, Some("wire-two")).unwrap();
    assert_eq!(one.option_rule("native", "speed").unwrap()["const"], "slow");
    assert_eq!(two.option_rule("native", "speed").unwrap()["const"], "fast");
    let catalogue = composed_model_parameters(&json!({}), &[], &layers, None).unwrap();
    assert_eq!(
        catalogue.option_rule("wire-one", "choice").unwrap()["const"],
        "one"
    );
    assert_eq!(
        catalogue.option_rule("wire-two", "choice").unwrap()["const"],
        "two"
    );
    assert!(catalogue.option_schema("native").is_none());
    let translated =
        composed_model_parameters(&json!({}), &[], &layers, Some("incoming-via-translator"))
            .unwrap();
    assert_eq!(
        translated.option_rule("wire-one", "choice").unwrap()["const"],
        "one"
    );
    assert_eq!(
        translated.option_rule("wire-two", "choice").unwrap()["const"],
        "two"
    );
    assert!(translated.option_schema("native").is_none());
}
