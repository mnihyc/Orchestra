//! A managed program is a turn choice in the existing format, bounded by the same provider schema.
use orch_model_contract::parameters::{outgoing_at, outgoing_with_access_program};
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const PROTOCOL: &str = "openai-responses";
const REQUEST: &[u8] = br#"{"model":"fixture","input":"unchanged","stream":false}"#;

fn model() -> Result<ModelParameters, serde_json::Error> {
    serde_json::from_value(json!({"access_programs":["blue","red"],"default_access_program":"red"}))
}

#[test]
fn a_managed_program_overrides_a_native_selection_and_defaults_without_changing_the_model()
-> TestResult {
    let model = model()?;
    let original = serde_json::to_vec(&model)?;
    for request in [REQUEST.to_vec(), br#"{"model":"fixture","input":"unchanged","stream":false,"access_programs":{"cyber":"red"}}"#.to_vec()] {
        let sent: Value = serde_json::from_slice(&outgoing_with_access_program(
            PROTOCOL, "/responses", &model, &request, Some(Some("blue")),
        )?)?;
        assert_eq!(sent["access_programs"]["cyber"], "blue");
        assert_eq!(sent["model"], "fixture");
        assert_eq!(sent["input"], "unchanged");
        assert_eq!(sent["stream"], false);
    }
    assert_eq!(
        serde_json::to_vec(&model)?,
        original,
        "a choice is never a new provider default"
    );
    Ok(())
}

#[test]
fn missing_and_explicit_ordinary_choices_keep_ordinary_access_absent() -> TestResult {
    let plain = ModelParameters::default();
    let sent: Value = serde_json::from_slice(&outgoing_with_access_program(
        PROTOCOL,
        "/responses",
        &plain,
        REQUEST,
        None,
    )?)?;
    assert!(sent.get("access_programs").is_none());
    // Ordinary explicitly overrides a paid default, without adding an ordinary wire field.
    let sent: Value = serde_json::from_slice(&outgoing_with_access_program(
        PROTOCOL,
        "/responses",
        &model()?,
        REQUEST,
        Some(None),
    )?)?;
    assert!(sent.get("access_programs").is_none());
    let normal: Value =
        serde_json::from_slice(&outgoing_at(PROTOCOL, "/responses", &model()?, REQUEST)?)?;
    assert_eq!(
        normal["access_programs"]["cyber"], "red",
        "an unset choice retains the declared default"
    );
    Ok(())
}

#[test]
fn an_unopened_program_a_forbidden_wire_field_and_a_required_provider_const_stay_refused()
-> TestResult {
    assert!(
        outgoing_with_access_program(
            PROTOCOL,
            "/responses",
            &model()?,
            REQUEST,
            Some(Some("unopened"))
        )
        .is_err()
    );
    let forbidden: ModelParameters =
        serde_json::from_value(json!({"access_programs":["blue"],"request_options":{
            (PROTOCOL):{"type":"object","properties":{"access_programs":false}}
        }}))?;
    assert!(
        outgoing_with_access_program(
            PROTOCOL,
            "/responses",
            &forbidden,
            REQUEST,
            Some(Some("blue"))
        )
        .is_err()
    );
    let fixed: ModelParameters = serde_json::from_value(
        json!({"access_programs":["blue","red"],"request_options":{
            (PROTOCOL):{"type":"object","properties":{"access_programs":{"type":"object","properties":{"cyber":{"const":"red"}},"required":["cyber"]}},"required":["access_programs"]}
        }}),
    )?;
    for choice in ["blue", "standard"] {
        assert!(
            outgoing_with_access_program(
                PROTOCOL,
                "/responses",
                &fixed,
                REQUEST,
                Some(Some(choice))
            )
            .is_err(),
            "{choice}"
        );
    }
    Ok(())
}

#[test]
fn a_program_never_enters_an_undeclared_interface_or_the_compaction_request() -> TestResult {
    for protocol in ["openai-chat", "anthropic-messages", "future-interface"] {
        assert!(
            outgoing_with_access_program(
                protocol,
                "/responses",
                &model()?,
                REQUEST,
                Some(Some("blue"))
            )
            .is_err()
        );
    }
    let compact: Value = serde_json::from_slice(&outgoing_with_access_program(
        PROTOCOL,
        "/responses/compact",
        &ModelParameters::default(),
        REQUEST,
        Some(Some("blue")),
    )?)?;
    assert!(compact.get("access_programs").is_none());
    Ok(())
}
