use orch_provider_contract::models::ModelSuggestion;
use serde_json::json;

#[test]
fn old_model_suggestions_keep_their_body_and_empty_provenance_is_omitted() {
    let original = json!({"model_name":"fixture-model","display_name":"Fixture model",
        "parameters":{"context_window":200000},"listed":true});
    let model: ModelSuggestion = serde_json::from_value(original.clone()).unwrap();
    assert!(model.origins.is_empty());
    assert_eq!(serde_json::to_value(&model).unwrap(), original);
    let named = ModelSuggestion::named("old-model", None);
    assert!(
        serde_json::to_value(named)
            .unwrap()
            .get("origins")
            .is_none()
    );
}

#[test]
fn origin_metadata_is_open_and_does_not_rewrite_or_enable_model_parameters() {
    let original = json!({"model_name":"fixture-model","display_name":"Fixture model",
        "parameters":{"input_images":false},"listed":true,
        "origins":{"/parameters/input_images":"vendor","/future":"future-source"}});
    let model: ModelSuggestion = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(model.parameters.input_images, Some(false));
    assert!(model.parameters.web_search.is_none());
    assert_eq!(model.origins["/future"], "future-source");
    assert_eq!(serde_json::to_value(model).unwrap(), original);
}
