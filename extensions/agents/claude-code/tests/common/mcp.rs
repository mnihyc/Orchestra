//! A tool server that speaks MCP over HTTP, standing where the connector's listener stands.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses a part of this")]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use serde_json::{Value, json};

/// What the server was asked.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub server: String,
    pub authorization: Option<String>,
    pub method: String,
    pub params: Value,
}

type Answer = dyn Fn(&str, &Value) -> (String, bool) + Send + Sync;

struct Inner {
    tools: BTreeMap<String, Vec<(String, String)>>,
    answer: Box<Answer>,
    seen: Mutex<Vec<Recorded>>,
}

/// A running tool server.
pub struct McpServer {
    pub address: SocketAddr,
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

impl McpServer {
    /// Starts one on a free port of the range for tests. `tools` lists, for each server name, the tools it
    /// offers with a sentence on each; `answer` gives the text a call returns and whether it is an error.
    pub async fn start(
        tools: &[(&str, &[(&str, &str)])],
        answer: impl Fn(&str, &Value) -> (String, bool) + Send + Sync + 'static,
    ) -> Self {
        let inner = Arc::new(Inner {
            tools: tools
                .iter()
                .map(|(server, list)| {
                    (
                        (*server).to_owned(),
                        list.iter()
                            .map(|(n, d)| ((*n).to_owned(), (*d).to_owned()))
                            .collect(),
                    )
                })
                .collect(),
            answer: Box::new(answer),
            seen: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/mcp/{key}/{name}", any(handle))
            .with_state(Arc::clone(&inner));
        let (listener, address) = bind_in_range().await;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            address,
            inner,
            task,
        }
    }

    /// Everything it was asked, in order.
    pub fn seen(&self) -> Vec<Recorded> {
        self.inner.seen.lock().unwrap().clone()
    }

    /// The calls of tools it answered.
    pub fn calls(&self) -> Vec<Recorded> {
        self.seen()
            .into_iter()
            .filter(|r| r.method == "tools/call")
            .collect()
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn bind_in_range() -> (tokio::net::TcpListener, SocketAddr) {
    let seed = u64::from(std::process::id()) * 97 + 13;
    for attempt in 0..400u64 {
        let port = 41_000 + ((seed + attempt * 11) % 1000);
        let address = SocketAddr::from(([127, 0, 0, 1], u16::try_from(port).unwrap()));
        if let Ok(listener) = tokio::net::TcpListener::bind(address).await {
            return (listener, address);
        }
    }
    panic!("no free port between 41000 and 41999");
}

async fn handle(
    State(inner): State<Arc<Inner>>,
    Path((_key, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let method = message["method"].as_str().unwrap_or_default().to_owned();
    inner.seen.lock().unwrap().push(Recorded {
        server: name.clone(),
        authorization: headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        method: method.clone(),
        params: message["params"].clone(),
    });
    let id = message.get("id").cloned();
    let result = match method.as_str() {
        "initialize" => json!({
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": { "tools": {} },
            "serverInfo": { "name": name, "version": "1.0.0" }
        }),
        "tools/list" => {
            json!({ "tools": inner.tools.get(&name).into_iter().flatten().map(|(tool, description)| json!({
            "name": tool, "description": description,
            "inputSchema": { "type": "object", "additionalProperties": true, "properties": {
                "title": { "type": "string" }, "body": { "type": "string" }, "command": { "type": "string" },
                "file_path": { "type": "string" }, "content": { "type": "string" },
                "old_string": { "type": "string" }, "new_string": { "type": "string" } } }
        })).collect::<Vec<_>>() })
        }
        "tools/call" => {
            let tool = message["params"]["name"].as_str().unwrap_or_default();
            let (text, is_error) = (inner.answer)(tool, &message["params"]["arguments"]);
            json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
        }
        "ping" => json!({}),
        _ => {
            // Notifications and anything else that wants no answer.
            return StatusCode::ACCEPTED.into_response();
        }
    };
    let Some(id) = id else {
        return StatusCode::ACCEPTED.into_response();
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
    )
        .into_response()
}
