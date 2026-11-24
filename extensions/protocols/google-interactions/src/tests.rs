//! Native examples are fed in arbitrary byte cuts, with no credentials or external requests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
use super::*;
use orch_model_contract::generation::{GenerationDelta, GenerationStop};

#[test]
fn a_mixed_model_list_keeps_embedding_methods_out_of_conversational_choices() {
    let models = GoogleInteractions.parse_models(br#"{"models":[
        {"name":"models/local-chat","supportedGenerationMethods":["generateContent","countTokens"]},
        {"name":"models/local-vectors","supportedGenerationMethods":["embedContent","batchEmbedContents"]},
        {"name":"models/unknown","supportedGenerationMethods":["futureMethod"]}
    ]}"#);
    let purposes: Vec<_> = models
        .iter()
        .map(|model| {
            serde_json::to_value(&model.parameters).unwrap()["offered"]["purposes"].clone()
        })
        .collect();
    assert_eq!(
        purposes,
        vec![json!(["generation"]), json!(["embedding"]), Json::Null]
    );
}

fn stream(values: &[Json]) -> Vec<u8> {
    values
        .iter()
        .map(|value| format!("data: {value}\n\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn native_authentication_replaces_client_headers_and_keeps_the_real_header_sensitive() {
    use orch_model_contract::credentials::Credential;
    use orch_model_contract::endpoint::EndpointOptions;
    let protocol = GoogleInteractions;
    let options = EndpointOptions::default();
    assert!(protocol.check_endpoint_options(&options).is_ok());
    assert_eq!(
        protocol.default_auth(),
        orch_model_contract::endpoint::AuthMode::Header
    );
    assert_eq!(protocol.key_header(&options), Some("x-goog-api-key"));
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "authorization",
        http::HeaderValue::from_static("Bearer client-fixture"),
    );
    headers.insert(
        "x-goog-api-key",
        http::HeaderValue::from_static("client-fixture"),
    );
    let credential = Credential::new("provider-fixture".to_owned());
    protocol
        .authorize(
            &mut headers,
            Some(&credential),
            &options,
            &Default::default(),
        )
        .unwrap();
    assert_eq!(headers["x-goog-api-key"], "provider-fixture");
    assert!(headers["x-goog-api-key"].is_sensitive());
    assert!(!headers.contains_key("authorization"));
}

#[test]
fn native_search_results_text_and_reported_grounding_counts_survive_fragmented_streams() {
    let bytes = stream(&[
        json!({"event_type":"interaction.created","interaction":{"id":"native-request","model":"gemini-test","status":"in_progress"}}),
        json!({"event_type":"step.start","index":0,"step":{"type":"google_search_call","id":"search","arguments":{"queries":["example"]}}}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"step.start","index":1,"step":{"type":"google_search_result","call_id":"search","signature":"native-signature","result":[{"search_suggestions":"Suggestions"}]}}),
        json!({"event_type":"step.stop","index":1}),
        json!({"event_type":"step.start","index":2,"step":{"type":"model_output","content":[]}}),
        json!({"event_type":"step.delta","index":2,"delta":{"type":"text","text":"An answer 🙂"}}),
        json!({"event_type":"step.delta","index":2,"delta":{"type":"text_annotation_delta","annotations":[{"type":"url_citation","url":"https://example.invalid/source","title":"Source","start_index":0,"end_index":2}]}}),
        json!({"event_type":"step.stop","index":2,"usage":{"total_input_tokens":7,"total_cached_tokens":2,"total_output_tokens":23,"total_thought_tokens":49,"grounding_tool_count":[{"type":"google_search","count":2}]}}),
        json!({"event_type":"interaction.completed","interaction":{"id":"native-request","model":"gemini-test","status":"completed"}}),
    ]);
    let mut reader = GoogleInteractions.generation_stream().unwrap();
    let mut meter = GoogleInteractions.usage_observer();
    let mut text = String::new();
    for chunk in bytes.chunks(7) {
        meter.observe(chunk);
        for GenerationDelta::Text { text: delta } in reader.push(chunk).unwrap() {
            text.push_str(&delta);
        }
    }
    let answer = reader.finish().unwrap();
    assert_eq!(text, "An answer 🙂");
    assert_eq!(answer.stop, GenerationStop::Complete);
    assert_eq!(
        answer.message.continuation[1]["signature"],
        "native-signature"
    );
    assert_eq!(
        answer.native_result["response"]["steps"][2]["content"][0]["annotations"][0]["url"],
        "https://example.invalid/source"
    );
    let usage = meter.usage().unwrap();
    assert_eq!(usage.web_searches, Some(2));
    assert_eq!(usage.tokens.input_tokens, 7);
    assert_eq!(usage.tokens.cached_input_tokens, 2);
    assert_eq!(usage.tokens.output_tokens, 72);
    assert_eq!(usage.tokens.reasoning_tokens, 49);
    assert_eq!(usage.request_ref.as_deref(), Some("native-request"));
    assert!(meter.response_facts().completed);
}

#[test]
fn a_cut_or_unfinished_function_step_never_becomes_an_executable_call() {
    let events = [
        json!({"event_type":"step.start","index":0,"step":{"type":"function_call","id":"read-1","name":"read","arguments":{}}}),
        json!({"event_type":"interaction.completed","interaction":{"status":"requires_action"}}),
    ];
    for end in 1..=2 {
        let mut reader = GoogleInteractions.generation_stream().unwrap();
        reader.push(&stream(&events[..end])).unwrap();
        assert!(reader.finish().is_err());
    }
}

#[test]
fn native_function_history_remains_stateless_and_keeps_its_signed_steps() {
    let reply = json!({"status":"requires_action","steps":[{"type":"function_call","id":"call-1","name":"read","arguments":{"path":"topic"},"signature":"signed"}]});
    let answer = GoogleInteractions
        .generation_output(reply.to_string().as_bytes())
        .unwrap();
    assert_eq!(answer.stop, GenerationStop::Tools);
    let (_, path, bytes) = GoogleInteractions
        .generation_request(
            "gemini-test",
            &GenerationInput {
                system: "Follow the admitted read.".into(),
                messages: vec![
                    answer.message,
                    GenerationMessage {
                        role: GenerationRole::User,
                        content: vec![GenerationPart::ToolResult {
                            call_id: "call-1".into(),
                            content: "Source text".into(),
                            is_error: false,
                            witness: None,
                        }],
                        continuation: Vec::new(),
                    },
                ],
                tools: Vec::new(),
                max_output_tokens: 128,
            },
            true,
        )
        .unwrap();
    let body: Json = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(path, "/interactions");
    assert_eq!(body["store"], false);
    assert!(body.get("previous_interaction_id").is_none());
    assert_eq!(body["input"][0]["signature"], "signed");
    assert_eq!(body["input"][1]["type"], "function_result");
}

#[test]
fn an_unreported_search_count_stays_unknown_and_a_reported_zero_stays_zero() {
    for (extra, expected) in [
        (json!({}), None),
        (
            json!({"grounding_tool_count":[{"type":"google_search","count":0}]}),
            Some(0),
        ),
    ] {
        let mut usage = json!({"total_input_tokens":10,"total_output_tokens":5});
        usage
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let reply =
            json!({"id":"request","model":"gemini-test","status":"completed","usage":usage});
        let mut meter = GoogleInteractions.usage_observer();
        meter.observe(reply.to_string().as_bytes());
        meter.finish(true);
        assert_eq!(meter.usage().unwrap().web_searches, expected);
    }
}

#[test]
fn model_discovery_keeps_reported_limits_and_encodes_pagination_without_changing_the_route() {
    let protocol = GoogleInteractions;
    let models = protocol.parse_models(br#"{"models":[{"name":"models/gemini-test","displayName":"Gemini test","inputTokenLimit":1000000,"outputTokenLimit":64000}]}"#);
    assert_eq!(models[0].model_name, "gemini-test");
    assert_eq!(models[0].parameters.context_window, Some(1_000_000));
    assert_eq!(
        models[0]
            .parameters
            .offered
            .as_ref()
            .unwrap()
            .report
            .as_ref()
            .unwrap()
            .output_max,
        Some(64_000)
    );
    let next = protocol
        .next_models_request("/models", br#"{"nextPageToken":"/?x=1&host=other"}"#)
        .unwrap()
        .unwrap();
    assert!(next.starts_with("/models?pageToken="));
    assert!(!next.contains("&host="));
}

#[test]
fn streamed_thoughts_keep_their_native_type_summary_and_signature_on_the_next_request() {
    let events = stream(&[
        json!({"event_type":"step.start","index":0,"step":{"type":"thought","summary":[]}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_summary","content":{"type":"text","text":"Check the first source"}}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_summary","content":{"type":"text","text":"Then compare the other source"}}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"native-"}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"signature"}}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"step.start","index":1,"step":{"type":"model_output","content":[]}}),
        json!({"event_type":"step.delta","index":1,"delta":{"type":"text","text":"The answer"}}),
        json!({"event_type":"step.stop","index":1}),
        json!({"event_type":"interaction.completed","interaction":{"status":"completed"}}),
    ]);
    for width in [1, 7, events.len()] {
        let mut reader = GoogleInteractions.generation_stream().unwrap();
        for bytes in events.chunks(width) {
            reader.push(bytes).unwrap();
        }
        let answer = reader.finish().unwrap();
        let expected = json!({"type":"thought","signature":"native-signature","summary":[
            {"type":"text","text":"Check the first source"},
            {"type":"text","text":"Then compare the other source"}
        ]});
        assert_eq!(answer.message.continuation[0], expected);
        assert_eq!(
            answer.message.content,
            vec![GenerationPart::Text {
                text: "The answer".into()
            }]
        );
        let (_, _, request) = GoogleInteractions
            .generation_request(
                "gemini-test",
                &GenerationInput {
                    system: String::new(),
                    messages: vec![answer.message],
                    tools: Vec::new(),
                    max_output_tokens: 128,
                },
                true,
            )
            .unwrap();
        let request: Json = serde_json::from_slice(&request).unwrap();
        assert_eq!(request["input"][0], expected);
        assert_eq!(request["store"], false);
    }
}

// Examples from Google's Interactions streaming and grounding guides. These are
// transcribed public wire shapes, not recordings of a customer's account.
#[test]
fn a_streamed_function_call_replays_object_arguments_and_its_native_id() {
    let frames = stream(&[
        json!({"event_type":"step.start","index":0,"step":{"type":"function_call","id":"f1","name":"get_weather","arguments":{}}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":"{\"location\":"}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":"\"Paris\"}"}}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"interaction.completed","interaction":{"id":"i1","status":"requires_action"}}),
    ]);
    let mut stream = GoogleInteractions.generation_stream().unwrap();
    for chunk in frames.chunks(3) {
        stream.push(chunk).unwrap();
    }
    let answer = stream.finish().unwrap();
    assert_eq!(answer.stop, GenerationStop::Tools);
    let input = GenerationInput {
        system: String::new(),
        messages: vec![
            answer.message,
            GenerationMessage {
                role: GenerationRole::User,
                content: vec![GenerationPart::ToolResult {
                    call_id: "f1".into(),
                    content: "Sunny".into(),
                    is_error: false,
                    witness: None,
                }],
                continuation: vec![],
            },
        ],
        tools: vec![],
        max_output_tokens: 64,
    };
    let (_, _, body) = GoogleInteractions
        .generation_request("gemini-4-hydrogen", &input, false)
        .unwrap();
    let body: Json = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["input"][0]["arguments"], json!({"location":"Paris"}));
    assert_eq!(body["input"][1]["call_id"], "f1");
    assert_eq!(body["store"], false);
}

#[test]
fn a_streamed_citation_stays_with_its_text_and_a_leased_model_has_a_google_resource_name() {
    let annotation = json!({"type":"url_citation","url":"https://example.org/score","title":"Match report","start_index":0,"end_index":5});
    let bytes = stream(&[
        json!({"event_type":"step.start","index":0,"step":{"type":"model_output","content":[]}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":"Spain"}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"text_annotation_delta","annotations":[annotation]}}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"interaction.completed","interaction":{"status":"completed"}}),
    ]);
    let mut reader = GoogleInteractions.generation_stream().unwrap();
    reader.push(&bytes).unwrap();
    let result = reader.finish().unwrap();
    assert_eq!(
        result.message.continuation[0]["content"][0]["annotations"][0],
        annotation
    );
    let interfaces = orch_provider_contract::capabilities::protocol_interfaces().unwrap();
    let binding = &interfaces["google-interactions"].native_tools["web_search"];
    let sources = orch_provider_contract::capabilities::project_result(
        binding,
        &json!({"query":"score"}),
        &result.native_result["response"],
        2,
    )
    .unwrap();
    assert_eq!(sources[0]["url"], annotation["url"]);
    let response = GoogleInteractions.models_response(&["gemini-4-neon".into()]);
    assert_eq!(
        GoogleInteractions.parse_models(response.body())[0].model_name,
        "gemini-4-neon"
    );
}
