//! A real dsh session uses the authenticated Orch MCP address supplied by the generic adapter.
//! Only the native program named by ORCH_DSH_BIN runs; its provider is the repository's scripted model.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::post;
use axum::{Json as JsonBody, Router};
use common::{agent_start, spec_in, turn_for};
use futures::StreamExt as _;
use orch_adapter_acp::AcpAdapter;
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter, EventSink, EventStream};
use orch_agent_contract::events::{OptionScope, OptionStyle, OrchTool, SessionEvent};
use orch_agent_contract::session::{ApprovalDecision, SessionMode};
use orch_testkit::doubles::model::{MockConfig, MockLlm};
use orch_types::{Id, Json, new_id};
use serde_json::json;

#[derive(Clone)]
struct Gateway {
    model: String,
    token: String,
    calls: Arc<Mutex<Vec<Json>>>,
}

async fn mcp(
    State(gateway): State<Gateway>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    JsonBody(request): JsonBody<Json>,
) -> Response {
    if headers.get("authorization").and_then(|v| v.to_str().ok())
        != Some(format!("Bearer {}", gateway.token).as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    gateway
        .calls
        .lock()
        .unwrap()
        .push(json!({ "path": uri.path(), "request": request }));
    let Some(id) = request.get("id") else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match request["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": { "tools": {} }, "serverInfo": { "name": "orch", "version": "test" }
        }),
        Some("tools/list") => json!({ "tools": [{
            "name": "list_shared", "description": "Lists the shared test entries.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }, {
            "name": "goal", "description": "Reads the goal managed by Orch.",
            "inputSchema": { "type": "object", "properties": { "action": { "enum": ["get"] } },
                "required": ["action"], "additionalProperties": false }
        }] }),
        Some("tools/call") => {
            let text = match request["params"]["name"].as_str() {
                Some("list_shared") => {
                    assert_eq!(request["params"]["arguments"], json!({}));
                    "shared entry: quince"
                }
                Some("goal") => {
                    assert_eq!(request["params"]["arguments"], json!({ "action": "get" }));
                    "Orch goal: verify the release; status: working"
                }
                other => panic!("unexpected Orch tool: {other:?}"),
            };
            json!({ "content": [{ "type": "text", "text": text }] })
        }
        _ => return JsonBody(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "unknown method" } })).into_response(),
    };
    JsonBody(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

// Keep the actual model protocol and streaming response; this gateway only joins its address with MCP.
async fn proxy(
    State(gateway): State<Gateway>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut request = reqwest::Client::new().request(method, format!("{}{uri}", gateway.model));
    for (name, value) in &headers {
        if name != "host" && name != "content-length" {
            request = request.header(name, value);
        }
    }
    let response = request.body(body).send().await.unwrap();
    let mut reply = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if name != "content-length" && name != "transfer-encoding" {
            reply = reply.header(name, value);
        }
    }
    reply
        .body(Body::from_stream(response.bytes_stream()))
        .unwrap()
}

async fn completed(adapter: &mut AcpAdapter, events: &mut EventStream, thread: Id) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(180), events.next())
            .await
            .expect("the real session must produce an event within its existing native deadline")
            .expect("the session event stream remains open");
        if let AdapterEvent::Session {
            event: SessionEvent::ApprovalRequested(asked),
            ..
        } = &event
        {
            let option = asked
                .request
                .options
                .iter()
                .find(|option| {
                    option.style == OptionStyle::Allow && option.scope == Some(OptionScope::Once)
                })
                .expect("an MCP request must offer a one-call approval");
            adapter
                .resolve_approval(
                    thread,
                    &asked.request_ref,
                    ApprovalDecision::Option {
                        option_id: option.id.clone(),
                        note: None,
                    },
                )
                .await
                .unwrap();
        }
        let (name, body) = event.to_wire().unwrap();
        if name == "turn.completed" {
            assert_eq!(body["status"], "completed");
            return;
        }
    }
}

#[tokio::test]
async fn a_real_dsh_thread_calls_an_orch_tool_at_its_session_address_with_its_token() {
    let Ok(binary) = std::env::var("ORCH_DSH_BIN") else {
        eprintln!("skipped: set ORCH_DSH_BIN to the native dsh executable");
        return;
    };
    assert!(
        std::path::Path::new(&binary).is_file(),
        "ORCH_DSH_BIN must name an executable"
    );
    // Reserved probe ports can be changed together by the train; no external service or credential is used.
    let model_port = std::env::var("ORCH_DSH_MCP_MODEL_PORT").unwrap_or_else(|_| "41696".into());
    let mcp_port = std::env::var("ORCH_DSH_MCP_PORT").unwrap_or_else(|_| "41695".into());
    let mock = MockLlm::start_on(
        format!("127.0.0.1:{model_port}").parse().unwrap(),
        MockConfig::default(),
    )
    .await
    .unwrap();
    mock.set_default("say Ready for an Orch tool.\n").unwrap();
    let gateway = Gateway {
        model: mock.url(),
        token: "s".repeat(64),
        calls: Arc::default(),
    };
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{mcp_port}"))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/mcp/{session}/orch", post(mcp))
        .fallback(proxy)
        .with_state(gateway.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (provider, profile) = (new_id(), new_id());
    let mut start = agent_start(provider, profile, home.path());
    start.binary = Some(binary);
    start.loopback =
        serde_json::from_value(json!({ "base_url": base, "token": gateway.token })).unwrap();
    let mut adapter = AcpAdapter::new(orch_adapter_acp::programs::dsh().unwrap());
    let (sink, mut events) = EventSink::channel();
    adapter.start(start, sink).await.unwrap();
    let mut spec = spec_in(work.path(), profile, provider, SessionMode::New);
    spec.settings.model = "scripted".into();
    spec.settings.available_models = vec!["scripted".into()];
    spec.settings.orch_tools = vec![OrchTool::ListShared, OrchTool::Goal];
    adapter.open_session(spec.clone()).await.unwrap();
    adapter
        .start_turn(turn_for(&spec, "List the available tools."))
        .await
        .unwrap();
    completed(&mut adapter, &mut events, spec.settings.thread_id).await;
    let requests = mock.settled(1).await;
    let first = requests
        .iter()
        .find(|r| r.path.ends_with("/chat/completions"))
        .unwrap();
    let tools: Vec<_> = first.body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert!(
        tools
            .iter()
            .all(|name| !["create_goal", "get_goal", "update_goal"].contains(name)),
        "native goal tools must be disabled so every continuation passes Orch's checks: {tools:?}"
    );
    let goal = *tools
        .iter()
        .find(|name| name.ends_with("__goal"))
        .expect("Orch's goal tool must remain available");
    let tool = tools
        .into_iter()
        .find(|name| name.ends_with("list_shared"))
        .expect("Orch's MCP tool must be offered to the model");
    mock.queue(&format!(
        "tool {tool} {{}}\nthen\nsay Shared entries read.\n"
    ))
    .unwrap();
    adapter
        .start_turn(turn_for(&spec, "Read the shared entries."))
        .await
        .unwrap();
    completed(&mut adapter, &mut events, spec.settings.thread_id).await;
    let expected_path = format!("/mcp/{}/orch", spec.session_key);
    let calls = gateway.calls.lock().unwrap().clone();
    assert!(!calls.is_empty());
    assert!(calls.iter().all(|call| call["path"] == expected_path));
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["request"]["method"] == "tools/call")
            .count(),
        1
    );
    let requests = mock.settled(3).await;
    let last = requests
        .iter()
        .rfind(|r| r.path.ends_with("/chat/completions"))
        .unwrap();
    assert!(
        last.body["messages"]
            .to_string()
            .contains("shared entry: quince"),
        "the tool's answer must reach the model"
    );
    mock.queue(&format!(
        "tool {goal} {{\"action\":\"get\"}}\nthen\nsay Orch checks goal progress.\n"
    ))
    .unwrap();
    adapter
        .start_turn(turn_for(&spec, "Read the current goal from Orch."))
        .await
        .unwrap();
    completed(&mut adapter, &mut events, spec.settings.thread_id).await;
    let calls = gateway.calls.lock().unwrap().clone();
    assert!(calls.iter().all(|call| call["path"] == expected_path));
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["request"]["method"] == "tools/call"
                && call["request"]["params"]["name"] == "goal")
            .count(),
        1,
        "the goal call must reach Orch through the authenticated session MCP address"
    );
    let requests = mock.settled(5).await;
    let last = requests
        .iter()
        .rfind(|r| r.path.ends_with("/chat/completions"))
        .unwrap();
    assert!(
        last.body["messages"]
            .to_string()
            .contains("Orch goal: verify the release"),
        "the Orch goal result must reach the model"
    );
    adapter.stop().await.unwrap();
    server.abort();
}
