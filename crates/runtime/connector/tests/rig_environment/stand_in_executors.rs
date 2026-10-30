//! Executors of the tests that stand in for the real ones: one that answers every message of a WebSocket with the same message after the word `echo`, and one that offers MCP and tells a request what it looked like when it got there.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

use std::convert::Infallible;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::descriptor::SandboxMode;
use orch_envhost::environment::ExecutorFactory;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::descriptor::{ExecutorAvailability, ExecutorDescriptor};
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use orch_plugin_contract::Secret;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// The token the stand-ins present or ask for.
pub const TOKEN: &str = "stand-in-token";

/// An executor of a harness's own protocol (kind `echo-exec`).
pub fn echo_socket() -> ExecutorFactory {
    Arc::new(|| Box::new(EchoSocket { task: None }))
}

/// An executor that offers MCP (kind `orch-exec`, tool set `plain`), whose answer to a request is the
/// headers it arrived with.
pub fn header_echo() -> ExecutorFactory {
    Arc::new(|| Box::new(HeaderEcho { task: None }))
}

/// An executor of a harness's own protocol that can be started only where the environment holds the build of the
/// harness (kind `needs-a-build`), as Codex's exec server can: it says which build it found.
pub fn needs_a_build() -> ExecutorFactory {
    Arc::new(|| Box::new(NeedsABuild))
}

/// The handshake of the echoing executor: the token, as a bearer, at the root path, or nothing.
#[allow(
    clippy::result_large_err,
    reason = "the signature is the one the handshake callback of the WebSocket library has"
)]
fn check_the_token(
    request: &tokio_tungstenite::tungstenite::handshake::server::Request,
    response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    let presented = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok());
    if presented == Some(&format!("Bearer {TOKEN}")) && request.uri().path() == "/" {
        Ok(response)
    } else {
        Err(tokio_tungstenite::tungstenite::http::Response::builder()
            .status(401)
            .body(None)
            .unwrap())
    }
}

struct EchoSocket {
    task: Option<tokio::task::JoinHandle<()>>,
}

#[async_trait]
impl Executor for EchoSocket {
    fn kind(&self) -> &'static str {
        "echo-exec"
    }

    fn describe(&self) -> ExecutorDescriptor {
        serde_json::from_value(json!({
            "kind": "echo-exec", "title": "Echo", "protocol": "harness", "sandbox_modes": ["open"]
        }))
        .unwrap()
    }

    async fn available(&self, _: &EnvironmentContext) -> ExecutorAvailability {
        ExecutorAvailability::available("1.0.0", vec![SandboxMode::Open])
    }

    async fn start(&mut self, _: &EnvironmentContext) -> Result<ExecutorEndpoint, AdapterError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        self.task = Some(tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let Ok(mut socket) =
                        tokio_tungstenite::accept_hdr_async(stream, check_the_token).await
                    else {
                        return;
                    };
                    while let Some(Ok(message)) = socket.next().await {
                        let answer = match message {
                            Message::Text(text) => Message::text(format!("echo {text}")),
                            Message::Binary(bytes) => {
                                let mut echoed = b"echo ".to_vec();
                                echoed.extend_from_slice(&bytes);
                                Message::binary(echoed)
                            }
                            _ => continue,
                        };
                        if socket.send(answer).await.is_err() {
                            return;
                        }
                    }
                });
            }
        }));
        Ok(ExecutorEndpoint {
            address: format!("ws://127.0.0.1:{port}"),
            token: Some(Secret::new(TOKEN)),
            version: "1.0.0".to_owned(),
            tool_sets: Vec::new(),
            sandbox_modes: vec![SandboxMode::Open],
        })
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        Ok(())
    }
}

struct HeaderEcho {
    task: Option<tokio::task::JoinHandle<()>>,
}

#[async_trait]
impl Executor for HeaderEcho {
    fn kind(&self) -> &'static str {
        "orch-exec"
    }

    fn describe(&self) -> ExecutorDescriptor {
        serde_json::from_value(json!({
            "kind": "orch-exec", "title": "Header echo", "protocol": "mcp",
            "tool_sets": [{ "name": "plain", "summary": "Tools that say how the request came." }],
            "sandbox_modes": ["open"]
        }))
        .unwrap()
    }

    async fn available(&self, _: &EnvironmentContext) -> ExecutorAvailability {
        ExecutorAvailability {
            tool_sets: Some(vec!["plain".to_owned()]),
            ..ExecutorAvailability::available("1.0.0", vec![SandboxMode::Open])
        }
    }

    async fn start(&mut self, _: &EnvironmentContext) -> Result<ExecutorEndpoint, AdapterError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        self.task = Some(tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let service = service_fn(
                        |request: Request<hyper::body::Incoming>| async move {
                            let seen: Value = [
                                "authorization",
                                "x-orch-session",
                                "host",
                                "x-orch-environment",
                            ]
                            .iter()
                            .map(|name| {
                                (
                                    (*name).to_owned(),
                                    request
                                        .headers()
                                        .get(*name)
                                        .and_then(|value| value.to_str().ok())
                                        .map_or(Value::Null, |value| {
                                            Value::String(value.to_owned())
                                        }),
                                )
                            })
                            .collect::<serde_json::Map<_, _>>()
                            .into();
                            let path = request.uri().path().to_owned();
                            let body = request
                                .into_body()
                                .collect()
                                .await
                                .map(|b| b.to_bytes())
                                .unwrap_or_default();
                            let id = serde_json::from_slice::<Value>(&body)
                                .ok()
                                .and_then(|message| message.get("id").cloned())
                                .unwrap_or(Value::Null);
                            let answer = json!({ "jsonrpc": "2.0", "id": id, "result": { "seen": seen, "path": path } });
                            let mut response =
                                Response::new(Full::new(Bytes::from(answer.to_string())));
                            response.headers_mut().insert(
                                hyper::header::CONTENT_TYPE,
                                hyper::header::HeaderValue::from_static("application/json"),
                            );
                            Ok::<_, Infallible>(response)
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        }));
        Ok(ExecutorEndpoint {
            address: format!("http://127.0.0.1:{port}/mcp"),
            token: Some(Secret::new(TOKEN)),
            version: "1.0.0".to_owned(),
            tool_sets: vec!["plain".to_owned()],
            sandbox_modes: vec![SandboxMode::Open],
        })
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        Ok(())
    }
}

/// The tool whose build the executor above runs.
pub const BUILD_TOOL: &str = "codex";

struct NeedsABuild;

#[async_trait]
impl Executor for NeedsABuild {
    fn kind(&self) -> &'static str {
        "needs-a-build"
    }

    fn describe(&self) -> ExecutorDescriptor {
        serde_json::from_value(json!({
            "kind": "needs-a-build", "title": "Needs a build", "protocol": "harness", "sandbox_modes": ["open"]
        }))
        .unwrap()
    }

    async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
        match env.tool(BUILD_TOOL) {
            Some(build) => ExecutorAvailability::available(&build.version, vec![SandboxMode::Open]),
            None => ExecutorAvailability::unavailable("the environment holds no build of the tool"),
        }
    }

    async fn start(&mut self, _: &EnvironmentContext) -> Result<ExecutorEndpoint, AdapterError> {
        Err(AdapterError::HarnessUnavailable(
            "this stand-in is looked at and never started".to_owned(),
        ))
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        Ok(())
    }
}
