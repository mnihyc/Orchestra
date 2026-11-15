//! Native examples are fed in arbitrary byte cuts, with no credentials or external requests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
use super::*;
use orch_model_contract::generation::{GenerationDelta, GenerationStop};

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
    assert!(options.check(protocol.default_auth()).is_ok());
    assert_eq!(
        options.key_header(protocol.default_auth()),
        Some("x-goog-api-key")
    );
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
        json!({"event_type":"step.start","index":1,"step":{"type":"google_search_result","call_id":"search","signature":"native-signature","result":[{"url":"https://example.invalid/source","title":"Source","snippet":"A useful result"}]}}),
        json!({"event_type":"step.stop","index":1}),
        json!({"event_type":"step.start","index":2,"step":{"type":"model_output","content":[]}}),
        json!({"event_type":"step.delta","index":2,"delta":{"type":"text","text":"An answer 🙂"}}),
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
        answer.native_result["response"]["steps"][1]["result"][0]["url"],
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
