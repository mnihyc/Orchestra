use orch_model_contract::parameters::{apply, outgoing, thread};
use orch_provider_contract::models::ModelParameters;
use serde_json::{Value, json};

#[test]
fn a_thread_freezes_only_its_explicit_interface_choices_and_keeps_native_parameters_separate() {
    let mut params = json!({
        "effort":"low",
        "provider_options:wire-a":{"vendor":{"mode":"a"}},
        "provider_options:wire-b":{"temperature":0.25}
    });
    let frozen = thread::read(&params).unwrap();
    params["provider_options:wire-a"]["vendor"]["mode"] = json!("b");
    assert_eq!(frozen.len(), 2);
    assert_eq!(frozen["wire-a"], json!({"vendor":{"mode":"a"}}));
    assert_eq!(frozen["wire-b"], json!({"temperature":0.25}));
    assert!(thread::read(&json!({"effort":"low"})).unwrap().is_empty());
    for invalid in [
        json!({"provider_options:wire-a":{"model":"another-model"}}),
        json!({"provider_options:wire-a":{"messages":[]}}),
        json!({"provider_options:wire-a":false}),
        json!({"provider_options:":{}}),
        json!({"provider_options:native":{"effort":"low"}}),
    ] {
        assert!(thread::read(&invalid).is_err(), "{invalid}");
    }
}

#[test]
fn an_explicit_thread_value_replaces_a_harness_value_but_never_the_provider_ceiling() {
    let model: ModelParameters = serde_json::from_value(json!({"request_options":{
        "openai-chat":{"type":"object","properties":{
            "vendor":{"type":"object","properties":{
                "mode":{"enum":["a","b"]},"region":{"const":"local"}
            },"additionalProperties":false}
        }}
    }}))
    .unwrap();
    let original = json!({"model":"fixture","messages":[],"vendor":{"mode":"a"}});
    let selected = json!({"vendor":{"mode":"b"}});
    let selected = thread::selected("openai-chat", &model, &selected).unwrap();
    let wire = apply(
        "openai-chat",
        &serde_json::to_vec(&original).unwrap(),
        selected,
        false,
    )
    .unwrap();
    let body: Value =
        serde_json::from_slice(&outgoing("openai-chat", &model, &wire).unwrap()).unwrap();
    assert_eq!(body["vendor"], json!({"mode":"b","region":"local"}));
    assert_eq!(body["messages"], json!([]));
    assert!(thread::selected("another-wire", &model, selected).is_err());
    assert!(thread::selected("openai-chat", &model, &json!({"unknown":true})).is_err());
    assert!(thread::selected("openai-chat", &model, &json!({"model":"different"})).is_err());
    for invalid in [
        json!({"vendor":{"mode":"c"}}),
        json!({"vendor":{"region":"remote"}}),
    ] {
        let selected = thread::selected("openai-chat", &model, &invalid).unwrap();
        let wire = apply(
            "openai-chat",
            &serde_json::to_vec(&original).unwrap(),
            selected,
            false,
        )
        .unwrap();
        assert!(outgoing("openai-chat", &model, &wire).is_err());
    }
}

#[test]
fn thread_forms_keep_local_reference_roots_and_show_only_explicitly_opened_wire_fields() {
    let model: ModelParameters = serde_json::from_value(json!({"request_options":{
        "fixture-wire":{"$defs":{
            "mode":{"type":"string","enum":["a","b"]},
            "fields":{"type":"object","properties":{
                "choice":{"$ref":"#/$defs/mode"},"region":{"const":"local"},
                "model":{"type":"string"},"closed":false
            }}
        },"$ref":"#/$defs/fields"},
        "native":{"type":"object","properties":{"native_choice":{"type":"string"}}}
    }}))
    .unwrap();
    let forms = thread::forms(&model);
    assert_eq!(forms.len(), 1);
    let form = &forms["fixture-wire"];
    assert!(form.get("$ref").is_none());
    assert!(form["properties"].get("model").is_none());
    assert!(form["properties"].get("closed").is_none());
    assert_eq!(form["properties"]["region"]["const"], "local");
    assert_eq!(form["properties"]["region"]["x-orch-source"], "provider");
    assert!(orch_types::schema::validate(form, &json!({"choice":"a","region":"local"})).is_ok());
    assert!(orch_types::schema::validate(form, &json!({"choice":"c","region":"local"})).is_err());
    assert!(thread::forms(&ModelParameters::default()).is_empty());
}
