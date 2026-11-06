//! Provider reports accept legacy numeric amounts; normalized HTTP replies remain exact decimal text.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

use orch_provider_contract::facts::ExtraUsage;
use orch_provider_contract::source::{FactsReport, ResetReport};
use schemars::generate::SchemaSettings;
use serde_json::{Value, json};

fn extra(used: Value, limit: Value) -> Value {
    json!({"enabled":true,"used":used,"limit":limit,"used_percent":2.46,
        "unit":"USD","decimal_places":null,"disabled_reason":null,"limit_reached":null})
}

#[test]
fn actual_source_report_schemas_accept_legacy_numbers_and_canonical_decimal_strings() {
    // This is the same Deserialize generator used by xtask's provider extension schemas.
    let input = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<FactsReport>()
        .to_value();
    let reset_input = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<ResetReport>()
        .to_value();
    for value in [
        extra(json!(123.0), json!(5000.0)), // the unchanged Python example's legacy report shape
        extra(json!("9007199254740993.1250"), json!("10000000000000000")),
        extra(json!(0), Value::Null),
    ] {
        let report = json!({"extra_usage":value.clone()});
        assert!(
            orch_plugin::schema::validate(&input, &report).is_ok(),
            "legacy source acceptance must not require a driver upgrade"
        );
        assert!(
            orch_plugin::schema::validate(
                &reset_input,
                &json!({"outcome":"fixture","facts":report})
            )
            .is_ok()
        );
        let normalized: ExtraUsage = serde_json::from_value(value).unwrap();
        let written = serde_json::to_value(normalized).unwrap();
        for field in ["used", "limit"] {
            assert!(written[field].is_string() || written[field].is_null());
        }
    }
    assert!(
        orch_plugin::schema::validate(&input, &json!({"extra_usage":{"enabled":true}})).is_ok(),
        "supported but missing amounts remain optional input"
    );
}

#[test]
fn the_actual_openapi_amount_schema_rejects_numbers_and_keeps_source_bounds_truthful() {
    let route = orch_api_contract::routes::all()
        .into_iter()
        .find(|route| route.name == "ProviderGet")
        .unwrap();
    let document = orch_api_core::openapi::document(&[route], "schema-control");
    let output = &document["components"]["schemas"]["ExtraUsage"];
    assert!(output.is_object());
    let legacy = extra(json!(123.25), json!(5000));
    assert!(
        orch_plugin::schema::validate(output, &legacy).is_err(),
        "legacy input acceptance must not widen normalized HTTP output"
    );
    let normalized: ExtraUsage = serde_json::from_value(legacy).unwrap();
    let canonical = serde_json::to_value(normalized).unwrap();
    assert_eq!(canonical["used"], "123.25");
    assert_eq!(canonical["limit"], "5000");
    assert!(orch_plugin::schema::validate(output, &canonical).is_ok());
    let input = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<FactsReport>()
        .to_value();
    for bad in [json!(true), json!({}), json!("1e3"), json!("9".repeat(41))] {
        assert!(
            orch_plugin::schema::validate(
                &input,
                &json!({"extra_usage":extra(bad.clone(),Value::Null)})
            )
            .is_err()
        );
        assert!(orch_plugin::schema::validate(output, &extra(bad, Value::Null)).is_err());
    }
    let mut wrong_percent = canonical;
    wrong_percent["used_percent"] = json!("2.46");
    assert!(orch_plugin::schema::validate(output, &wrong_percent).is_err());
    assert!(orch_plugin::schema::validate(&input, &json!({"extra_usage":wrong_percent})).is_err());
}
