//! A controlled MCP server asks the real harness for a form while its only other endpoint is the scripted model.
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use orch_types::new_id;
use serde_json::{Value, json};

/// These controlled requests are tiny; eight MiB leaves room for the harness's tool catalogue without accepting an unbounded body.
const MAX_BODY: usize = 8 * 1024 * 1024;

struct Inner {
    model: SocketAddr,
    within: Duration,
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Value>>>,
    seen: Mutex<Vec<Value>>,
}

/// A controlled model relay and MCP endpoint, kept for one test.
pub struct Server {
    /// The sole service made reachable through the networkless harness's Unix relay.
    pub address: SocketAddr,
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    /// Starts the local counterpart on the test's reserved range, forwarding only the fixed model endpoint.
    ///
    /// # Errors
    /// No controlled-service port is free, or its address cannot be read.
    pub async fn start(model: SocketAddr, within: Duration) -> Result<Self, std::io::Error> {
        let mut listener = None;
        for port in 41000..=41999 {
            if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                listener = Some(bound);
                break;
            }
        }
        let listener =
            listener.ok_or_else(|| std::io::Error::other("no free controlled-service port"))?;
        let address = listener.local_addr()?;
        let inner = Arc::new(Inner {
            model,
            within,
            pending: Mutex::new(HashMap::new()),
            seen: Mutex::new(Vec::new()),
        });
        let app = Router::new().fallback(handle).with_state(inner.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            address,
            inner,
            task,
        })
    }

    /// Safe MCP requests and answers observed from the native client; no authorization headers are recorded.
    pub fn seen(&self) -> Vec<Value> {
        self.inner
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn answer(id: &Value, result: Value) -> Response {
    axum::Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

async fn handle(State(inner): State<Arc<Inner>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let headers = request.headers().clone();
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    if path.starts_with("/llm/") {
        // The destination is supplied by the test, never by an incoming URL or header.
        let client = match reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
        {
            Ok(client) => client,
            Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
        };
        let mut forwarded = client
            .request(method, format!("http://{}{path}", inner.model))
            .body(body);
        if let Some(value) = headers.get("authorization") {
            forwarded = forwarded.header("authorization", value);
        }
        forwarded = forwarded.header("content-type", "application/json");
        let upstream = match forwarded.send().await {
            Ok(reply) => reply,
            Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
        };
        let status = upstream.status();
        let content_type = upstream.headers().get("content-type").cloned();
        let mut reply = Response::new(Body::from_stream(upstream.bytes_stream()));
        *reply.status_mut() = status;
        if let Some(value) = content_type {
            reply.headers_mut().insert("content-type", value);
        }
        return reply;
    }
    if !path.starts_with("/mcp/") || !path.ends_with("/questions") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if method == axum::http::Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    inner
        .seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(request.clone());
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("method").is_none() {
        if let Some(id) = id.as_str()
            && let Some(waiting) = inner
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(id)
        {
            let _ = waiting.send(request.get("result").cloned().unwrap_or(Value::Null));
        }
        return StatusCode::ACCEPTED.into_response();
    }
    match request["method"].as_str().unwrap_or("") {
        "initialize" => answer(&id, json!({"protocolVersion":request["params"]["protocolVersion"],
            "capabilities":{"tools":{}},"serverInfo":{"name":"controlled-questions","version":"1"}})),
        "notifications/initialized" | "notifications/cancelled" => StatusCode::ACCEPTED.into_response(),
        "ping" => answer(&id,json!({})),
        "tools/list" => answer(&id, json!({"tools":[
            {"name":"ask","description":"Ask the person for a controlled choice.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
            {"name":"read_shared","description":"Read the controlled text.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
            {"name":"announce","description":"Return the controlled structured result.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}
        ]})),
        "tools/call" if request["params"]["name"] == "read_shared" => answer(&id, json!({"content":[{"type":"text","text":"Orch conformance text"}],"isError":false})),
        "tools/call" if request["params"]["name"] == "announce" => answer(&id, json!({"content":[],"structuredContent":{"conformance_accepted":true},"isError":false})),
        "tools/call" => {
            let request_id = format!("elicitation-{}", new_id().simple());
            let (send, receive) = tokio::sync::oneshot::channel();
            inner.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(request_id.clone(), send);
            let first = json!({"jsonrpc":"2.0","id":request_id,"method":"elicitation/create","params":{
                "mode":"form","message":"Choose a value for native conformance.",
                "requestedSchema":{"type":"object","properties":{"choice":{"type":"string","title":"Choice"}},"required":["choice"]}}});
            let initial = futures::stream::once(async move { Ok::<_,Infallible>(Event::default().event("message").data(first.to_string())) });
            let final_inner = inner.clone();
            let final_event = futures::stream::once(async move {
                let result = tokio::time::timeout(final_inner.within, receive).await;
                final_inner.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&request_id);
                let reply = match result {
                    Ok(Ok(value)) => json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":value.to_string()}],"isError":false}}),
                    _ => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"the controlled elicitation was not answered"}}),
                };
                Ok::<_,Infallible>(Event::default().event("message").data(reply.to_string()))
            });
            Sse::new(initial.chain(final_event)).into_response()
        }
        _ => axum::Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown controlled MCP method"}})).into_response(),
    }
}
