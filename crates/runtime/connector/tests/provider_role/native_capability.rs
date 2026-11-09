//! A declared hosted capability uses the ordinary generation transport and final model ceiling.
use super::*;
use orch_link_contract::commands::{NativeCapabilityInput, ProviderGenerate};
use orch_model_contract::generation::{
    GenerationInput, GenerationMessage, GenerationPart, GenerationRole,
};

fn request(p: &ProviderRig) -> ProviderGenerate {
    let endpoint = orch_model_contract::services::generation_endpoint(
        RESPONSES,
        &p.mock.responses_base_url(),
        &json!({}),
    )
    .unwrap();
    let sources = orch_provider_contract::builtins::BuiltinSources::default();
    let mut model_parameters = common::model::enabled_parameters();
    model_parameters.web_search = Some(vec!["live".into()]);
    let binding = orch_provider_contract::services::native_binding(
        sources.capabilities().declarations(),
        &endpoint,
        MODEL,
        &model_parameters,
        "web_search",
    )
    .unwrap();
    let input = json!({"query":"Actual source query"});
    let auxiliary =
        orch_provider_contract::capabilities::render_auxiliary(&binding, &input).unwrap();
    ProviderGenerate {
        native_capability: Some(NativeCapabilityInput {
            capability: "web_search".into(),
            input,
            definition: orch_provider_contract::services::definition_hash(&endpoint, &binding)
                .unwrap(),
        }),
        capture_native_result: true,
        provider_id: p.rig.provider,
        protocol: RESPONSES.into(),
        model_name: MODEL.into(),
        params: auxiliary.get("options").cloned(),
        model_parameters,
        input: GenerationInput {
            system: String::new(),
            messages: vec![GenerationMessage {
                role: GenerationRole::User,
                content: vec![GenerationPart::Text {
                    text: auxiliary["prompt"].as_str().unwrap().into(),
                }],
                continuation: vec![],
            }],
            tools: vec![],
            max_output_tokens: 128,
        },
    }
}

#[tokio::test]
async fn declared_native_search_reaches_the_real_model_transport_and_retains_its_sources() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.mock.queue(r#"body 200 {"id":"actual-native-query","model":"mock-large","status":"completed","output":[{"id":"s1","type":"web_search_call","status":"completed","action":{"type":"search","query":"Actual source query","sources":[{"title":"An actual source","url":"https://source.example.invalid/"}]}},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Found the source","annotations":[{"type":"url_citation","title":"An actual source","url":"https://source.example.invalid/","start_index":0,"end_index":16}]}]}],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}}"#).unwrap();
    let mut request = request(&p);
    // A program control cannot replace the selected interface's wire ceiling.
    request.model_parameters.request_options = Some(std::collections::BTreeMap::from([
        (
            "native".into(),
            json!({"type":"object","properties":{"web_search":{"const":"disabled"}}}),
        ),
        (
            RESPONSES.into(),
            json!({"type":"object","properties":{"web_search":{"const":"live"}}}),
        ),
    ]));
    let result = p.rig.succeed(&SerialKey::None, &request).await;
    assert_eq!(result["output"]["stop"], "complete");
    assert_eq!(
        result["output"]["native_result"]["response"]["output"][0]["action"]["sources"][0]["url"],
        "https://source.example.invalid/"
    );
    assert_eq!(result["usage"]["input_tokens"], 5);
    assert!(!result.to_string().contains(&p.key));
    let observed = p.model_requests();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].credential.verdict, Verdict::Accepted);
    assert_eq!(observed[0].body["model"], MODEL);
    assert!(
        observed[0].body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["type"] == "web_search")
    );
    assert_eq!(
        observed[0].body["include"],
        json!(["web_search_call.action.sources"])
    );
}

#[tokio::test]
async fn native_auxiliary_intent_cannot_change_its_definition_prompt_or_model_ceiling() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    let good = request(&p);
    let mut wrong_definition = good.clone();
    wrong_definition
        .native_capability
        .as_mut()
        .unwrap()
        .definition = orch_types::Hash::of(b"another native binding");
    let mut no_capture = good.clone();
    no_capture.capture_native_result = false;
    let mut other_prompt = good.clone();
    other_prompt.input.system = "An unbound system instruction".into();
    let mut changed_options = good.clone();
    changed_options.params = Some(json!({"web_search":"disabled"}));
    let mut disabled_model = good.clone();
    disabled_model.model_parameters.web_search = Some(vec![]);
    let mut narrowed_model = good;
    // This is a wire request; the separate native schema controls program parameters.
    narrowed_model.model_parameters.request_options = Some(std::collections::BTreeMap::from([
        (
            RESPONSES.into(),
            json!({"type":"object","properties":{"web_search":{"const":"disabled"}}}),
        ),
        (
            "native".into(),
            json!({"type":"object","properties":{"web_search":{"const":"live"}}}),
        ),
    ]));
    for (intent, request) in [
        ("another declaration", wrong_definition),
        ("missing native capture", no_capture),
        ("another prompt", other_prompt),
        ("options removing declared search", changed_options),
        ("disabled model search", disabled_model),
        ("narrowed model option ceiling", narrowed_model),
    ] {
        let result = p.rig.send(&SerialKey::None, &request).await;
        assert!(
            matches!(result, Progress::Failed(_)),
            "{intent}: {result:?}"
        );
    }
    assert!(
        p.model_requests().is_empty(),
        "every refusal precedes the actual paid request"
    );
}

#[tokio::test]
async fn required_native_input_images_use_actual_command_blobs_without_a_text_fallback() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    let mut request = request(&p);
    request.model_parameters.input_images = Some(true);
    let bytes = bytes::Bytes::from_static(b"the materialized input image");
    let blob = p.rig.blobs.insert(bytes.clone());
    request.input.messages[0]
        .content
        .push(GenerationPart::ImageRef {
            blob,
            size: bytes.len() as i64,
            media_type: "image/png".into(),
            name: "source.png".into(),
            url: String::new(),
        });
    p.mock.queue(r#"body 200 {"id":"native-image-input","model":"mock-large","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Used the actual image"}]}],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}}"#).unwrap();
    let id = new_id();
    p.rig.blobs.open_command(id);
    p.rig
        .controller
        .issue_with_id(id, &SerialKey::None, p.rig.deadline(), &request);
    let result = tokio::time::timeout(p.rig.patience, p.rig.controller.wait_command(id))
        .await
        .unwrap();
    assert!(matches!(result, Progress::Succeeded(_)), "{result:?}");
    let observed = p.model_requests();
    assert_eq!(observed.len(), 1);
    assert_eq!(
        observed[0].body["input"][1]["content"][0]["type"],
        "input_image"
    );
    assert!(
        observed[0].body["input"][1]["content"][0]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    request.input.messages[0].content[1] = GenerationPart::ImageRef {
        blob: orch_types::Hash::of(b"missing"),
        size: 7,
        media_type: "image/png".into(),
        name: "missing.png".into(),
        url: "https://must-not-replace.example.invalid/".into(),
    };
    let id = new_id();
    p.rig.blobs.open_command(id);
    p.rig
        .controller
        .issue_with_id(id, &SerialKey::None, p.rig.deadline(), &request);
    let refused = tokio::time::timeout(p.rig.patience, p.rig.controller.wait_command(id))
        .await
        .unwrap();
    assert!(matches!(refused, Progress::Failed(_)), "{refused:?}");
    assert_eq!(
        p.model_requests().len(),
        1,
        "a required image cannot turn into a paid text-only request"
    );
}
