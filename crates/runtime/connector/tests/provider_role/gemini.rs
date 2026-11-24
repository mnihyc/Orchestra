//! A local HTTP fixture for Google's public models and Interactions REST examples.
use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use orch_link_contract::commands::ProviderGenerate;
use orch_model_contract::generation::{
    GenerationInput, GenerationMessage, GenerationPart, GenerationRole, GenerationTool,
};
use serde_json::Value;

const NAME: &str = "gemini-4-hydrogen";

fn authenticated(headers: &HeaderMap, expected: &str) {
    assert!(headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()) == Some(expected));
    assert!(!headers.contains_key("authorization"));
}

async fn list(State(key): State<String>, headers: HeaderMap) -> Json<Value> {
    authenticated(&headers, &key);
    Json(
        json!({"models":[{"name":format!("models/{NAME}"),"displayName":"Gemini 4 Hydrogen","inputTokenLimit":1048576,"supportedGenerationMethods":["generateContent"]}]}),
    )
}

async fn turn(
    State(key): State<String>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> impl axum::response::IntoResponse {
    authenticated(&headers, &key);
    assert_eq!(request["model"], NAME);
    assert_eq!(request["store"], false);
    assert_eq!(request["stream"], true);
    let tool_answer = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["type"] == "function_result");
    let (step, delta, status) = if let Some(result) = tool_answer {
        assert_eq!(result["call_id"], "weather-1");
        assert_eq!(result["result"], "sunny");
        assert!(
            request["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["type"] == "function_call" && s["arguments"] == json!({"city":"Paris"}))
        );
        (
            json!({"type":"model_output","content":[]}),
            json!({"type":"text","text":"Sunny in Paris."}),
            "completed",
        )
    } else {
        assert_eq!(request["tools"][0]["name"], "weather");
        (
            json!({"type":"function_call","name":"weather","id":"weather-1","arguments":{}}),
            json!({"type":"arguments_delta","arguments":"{\"city\":\"Paris\"}"}),
            "requires_action",
        )
    };
    let events = [
        json!({"event_type":"step.start","index":0,"step":step}),
        json!({"event_type":"step.delta","index":0,"delta":delta}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"interaction.completed","interaction":{"id":"fixture","status":status,"model":NAME,"usage":{"total_input_tokens":7,"total_output_tokens":3,"total_thought_tokens":2}}}),
    ];
    (
        [("content-type", "text/event-stream")],
        events
            .iter()
            .map(|e| format!("data: {e}\n\n"))
            .collect::<String>(),
    )
}

#[tokio::test]
async fn a_google_key_lists_its_models_and_completes_a_stateless_function_turn() {
    let p = ProviderRig::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/v1beta", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/v1beta/models", get(list))
        .route("/v1beta/interactions", post(turn))
        .with_state(p.key.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let started = p
        .start_provider(&[("google-interactions", address, json!({}))])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)));
    let listed = p
        .rig
        .succeed(
            &p.provider_key(),
            &ProviderDiscoverModels {
                provider_id: p.rig.provider,
                models: None,
                max_report_bytes: orch_link_contract::commands::UpstreamPolicy::default()
                    .whole_bytes,
            },
        )
        .await;
    assert_eq!(listed["suggestions"][0]["model_name"], NAME);
    let mut command = ProviderGenerate {
        native_capability: None,
        capture_native_result: false,
        provider_id: p.rig.provider,
        protocol: "google-interactions".into(),
        model_name: NAME.into(),
        params: None,
        model_parameters: common::model::enabled_parameters(),
        input: GenerationInput {
            system: "Use weather.".into(),
            messages: vec![GenerationMessage {
                role: GenerationRole::User,
                content: vec![GenerationPart::Text {
                    text: "Weather in Paris?".into(),
                }],
                continuation: vec![],
            }],
            tools: vec![GenerationTool {
                name: "weather".into(),
                description: "Look up weather".into(),
                parameters: json!({"type":"object","properties":{"city":{"type":"string"}}}),
            }],
            max_output_tokens: 64,
        },
    };
    let first = p.rig.succeed(&SerialKey::None, &command).await;
    assert_eq!(first["output"]["stop"], "tools");
    command
        .input
        .messages
        .push(serde_json::from_value(first["output"]["message"].clone()).unwrap());
    command.input.messages.push(GenerationMessage {
        role: GenerationRole::User,
        content: vec![GenerationPart::ToolResult {
            call_id: "weather-1".into(),
            content: "sunny".into(),
            is_error: false,
            witness: None,
        }],
        continuation: vec![],
    });
    let last = p.rig.succeed(&SerialKey::None, &command).await;
    assert_eq!(last["output"]["stop"], "complete");
    assert_eq!(
        last["output"]["message"]["content"][0]["text"],
        "Sunny in Paris."
    );
    assert_eq!(last["usage"]["output_tokens"], 5);
    assert!(!last.to_string().contains(&p.key));
    server.abort();
}
