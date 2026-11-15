//! The native program sees its own interface while only the opposite controlled interface is reachable.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a native fixture fails by panicking"
)]

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures::{StreamExt, stream::BoxStream};
use orch_conformance::native::ModelRequest;
use orch_model_contract::translator::{ProtocolTranslator, ResponseTranslator, UpstreamSide};
use orch_translator_responses_messages::{MessagesToResponses, ResponsesToMessages};
use serde_json::{Value, json};
use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{net::TcpListener, task::JoinHandle};

struct Observed {
    id: u64,
    request: ModelRequest,
}
struct Inner {
    target: SocketAddr,
    translator: Arc<dyn ProtocolTranslator>,
    client: reqwest::Client,
    token: String,
    next: AtomicU64,
    observed: Mutex<Vec<Observed>>,
}
pub struct Translation {
    pub address: SocketAddr,
    inner: Arc<Inner>,
    task: JoinHandle<()>,
}
impl Drop for Translation {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Translation {
    pub async fn start(target: SocketAddr, responses_to_messages: bool, token: &str) -> Self {
        assert!(
            target.ip().is_loopback(),
            "the fixture reaches only its controlled service"
        );
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let translator: Arc<dyn ProtocolTranslator> = if responses_to_messages {
            Arc::new(ResponsesToMessages)
        } else {
            Arc::new(MessagesToResponses)
        };
        let inner = Arc::new(Inner {
            target,
            translator,
            client: reqwest::Client::new(),
            token: token.to_owned(),
            next: AtomicU64::new(1),
            observed: Mutex::new(Vec::new()),
        });
        let router = Router::new()
            .fallback(forward)
            .with_state(Arc::clone(&inner));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            address,
            inner,
            task,
        }
    }
    pub fn clear(&self) {
        self.inner.observed.lock().unwrap().clear();
    }
    /// Join actual script provenance with the original native request, without reconstructing that request.
    pub fn requests(&self, upstream: &[ModelRequest]) -> Vec<ModelRequest> {
        self.inner
            .observed
            .lock()
            .unwrap()
            .iter()
            .map(|observed| {
                let mut native = observed.request.clone();
                if let Some(actual) = upstream.iter().find(|request| {
                    request
                        .body
                        .pointer("/metadata/orch_native_fixture")
                        .and_then(Value::as_u64)
                        == Some(observed.id)
                }) {
                    native.queued = actual.queued;
                }
                native
            })
            .collect()
    }
}
fn model_path(path: &str) -> Option<&'static str> {
    [
        "/responses/input_tokens",
        "/responses/compact",
        "/responses",
        "/v1/messages/count_tokens",
        "/v1/messages",
    ]
    .into_iter()
    .find(|suffix| path.ends_with(suffix))
}
fn problem(error: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        axum::Json(json!({"error":{"type":"translation_error","message":error.to_string()}})),
    )
        .into_response()
}
async fn forward(State(inner): State<Arc<Inner>>, request: Request) -> Response {
    match exchange(&inner, request).await {
        Ok(response) => response,
        Err(error) => problem(error),
    }
}
struct Stream {
    incoming: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    converter: Option<Box<dyn ResponseTranslator>>,
    ended: bool,
}
impl Stream {
    async fn next(&mut self) -> io::Result<Option<Bytes>> {
        loop {
            if self.ended {
                return Ok(None);
            }
            let bytes = match self.incoming.next().await {
                Some(Ok(bytes)) => match &mut self.converter {
                    Some(converter) => {
                        converter.translate_chunk(bytes).map_err(io::Error::other)?
                    }
                    None => bytes,
                },
                Some(Err(error)) => return Err(io::Error::other(error)),
                None => {
                    self.ended = true;
                    match &mut self.converter {
                        Some(converter) => converter.finish().map_err(io::Error::other)?,
                        None => Bytes::new(),
                    }
                }
            };
            if !bytes.is_empty() {
                return Ok(Some(bytes));
            }
        }
    }
}
async fn exchange(
    inner: &Inner,
    request: Request,
) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, orch_testkit::doubles::model::MAX_REQUEST_BYTES).await?;
    let path = parts.uri.path();
    if parts.method == Method::GET && model_path(path).is_some() {
        return Ok(StatusCode::UPGRADE_REQUIRED.into_response());
    }
    let native_path = if parts.method == Method::POST {
        model_path(path)
    } else {
        None
    };
    let mut converter = None;
    let mut native_stream = false;
    let mut capture = None;
    let (target, body, headers) = if let Some(native_path) = native_path {
        let presented = parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        if presented != Some(format!("Bearer {}", inner.token).as_str()) {
            return Ok(StatusCode::UNAUTHORIZED.into_response());
        }
        let original: Value = serde_json::from_slice(&bytes)?;
        native_stream = original["stream"] == true;
        let side = UpstreamSide {
            base_url: format!("http://{}", inner.target),
            options: json!({}),
            output_tokens: 4096,
            session: None,
            credential: None,
            model: Some(serde_json::from_value(
                json!({"efforts":["low","medium","high"],"reasoning_modes":["adaptive"],"cache_lifetimes":["1h"]}),
            )?),
        };
        let translated = inner
            .translator
            .translate_request(native_path, bytes.clone(), &side)?;
        converter = Some(inner.translator.response_for(native_path, &bytes, &side)?);
        let id = inner.next.fetch_add(1, Ordering::SeqCst);
        inner.observed.lock().unwrap().push(Observed {
            id,
            request: ModelRequest {
                path: path.to_owned(),
                body: original,
                status: None,
                queued: false,
            },
        });
        capture = Some(id);
        // Synthetic metadata joins observations under concurrency; it is never a real credential or model input.
        let mut body: Value = serde_json::from_slice(&translated.body)?;
        if !body.get("metadata").is_some_and(Value::is_object) {
            body["metadata"] = json!({});
        }
        body["metadata"]["orch_native_fixture"] = json!(id);
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse()?);
        headers.insert("authorization", format!("Bearer {}", inner.token).parse()?);
        if inner.translator.to() == "anthropic-messages" {
            headers.insert("anthropic-version", "2023-06-01".parse()?);
        }
        (
            format!("http://{}{}", inner.target, translated.path),
            Bytes::from(body.to_string()),
            headers,
        )
    } else {
        (
            format!(
                "http://{}{}",
                inner.target,
                parts
                    .uri
                    .path_and_query()
                    .map_or(path, |path| path.as_str())
            ),
            bytes,
            parts.headers,
        )
    };
    let mut outgoing = inner.client.request(parts.method, target).body(body);
    for (name, value) in &headers {
        if !matches!(
            name.as_str(),
            "host" | "connection" | "content-length" | "transfer-encoding"
        ) {
            outgoing = outgoing.header(name, value);
        }
    }
    let answer = outgoing.send().await?;
    let status = answer.status();
    if let Some(id) = capture
        && let Some(captured) = inner
            .observed
            .lock()
            .unwrap()
            .iter_mut()
            .find(|observed| observed.id == id)
    {
        captured.request.status = Some(status.as_u16());
    }
    let mut response = Response::builder().status(status);
    for (name, value) in answer.headers() {
        if !matches!(
            name.as_str(),
            "content-length"
                | "transfer-encoding"
                | "connection"
                | "content-encoding"
                | "content-type"
        ) {
            response = response.header(name, value);
        }
    }
    let media = if converter.is_some() {
        if native_stream && status.is_success() {
            "text/event-stream"
        } else {
            "application/json"
        }
    } else {
        answer
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
    };
    response = response.header("content-type", media);
    let stream = Stream {
        incoming: answer.bytes_stream().boxed(),
        converter,
        ended: false,
    };
    let stream = futures::stream::try_unfold(stream, |mut stream| async move {
        stream
            .next()
            .await
            .map(|bytes| bytes.map(|bytes| (bytes, stream)))
    });
    Ok(response.body(Body::from_stream(stream))?)
}
