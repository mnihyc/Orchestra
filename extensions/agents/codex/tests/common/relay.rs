//! A stand-in for the connector's forwarder, for the one check that reaches a real model: the harness is told
//! its model is at `/llm/<session key>/v1/responses` of this listener, which requires the token the launch
//! gave the harness, adds the real key of the interface and passes the answer back as it arrives. The harness
//! never holds the key; nor does this file: it is handed over in memory by a test that read it from its
//! environment, and it is written nowhere and never printed.
//!
//! The number of requests that go upstream is capped, so that a harness that retries cannot spend more than
//! the check intends.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// The headers of the request that are passed on.
const PASSED: [HeaderName; 3] = [
    header::ACCEPT,
    header::CONTENT_TYPE,
    header::CONTENT_ENCODING,
];

struct Inner {
    client: reqwest::Client,
    upstream: String,
    key: String,
    token: String,
    limit: usize,
    sent: AtomicUsize,
    log: Mutex<Vec<String>>,
}

/// The listener.
pub struct Relay {
    addr: SocketAddr,
    inner: Arc<Inner>,
    task: JoinHandle<()>,
}

impl Relay {
    /// Listens on a port of 41700..42000; requests are passed to `upstream` with `key` as their bearer, at most
    /// `limit` of them in all.
    pub async fn start(upstream: &str, key: String, token: String, limit: usize) -> Self {
        let mut listener = None;
        for port in 41700..42000u16 {
            if let Ok(bound) = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener.expect("a free port in 41700..42000");
        let addr = listener.local_addr().unwrap();
        let inner = Arc::new(Inner {
            client: reqwest::Client::new(),
            upstream: upstream.to_owned(),
            key,
            token,
            limit,
            sent: AtomicUsize::new(0),
            log: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/llm/{session}/v1/responses", post(forward))
            .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
            .with_state(Arc::clone(&inner));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, inner, task }
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// How many requests went upstream.
    pub fn upstream_requests(&self) -> usize {
        self.inner.sent.load(Ordering::SeqCst)
    }

    /// What happened to each request, without anything secret in it.
    pub fn log(&self) -> Vec<String> {
        self.inner.log.lock().unwrap().clone()
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn refuse(status: StatusCode, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"error":{{"message":"{message}","type":"relay"}}}}"#
        )))
        .unwrap()
}

async fn forward(State(inner): State<Arc<Inner>>, headers: HeaderMap, body: Bytes) -> Response {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if presented != Some(format!("Bearer {}", inner.token).as_str()) {
        inner
            .log
            .lock()
            .unwrap()
            .push("refused: the token of the launch was not presented".to_owned());
        return refuse(
            StatusCode::UNAUTHORIZED,
            "the token of the launch was not presented",
        );
    }
    if inner.sent.fetch_add(1, Ordering::SeqCst) >= inner.limit {
        inner.sent.fetch_sub(1, Ordering::SeqCst);
        inner
            .log
            .lock()
            .unwrap()
            .push("refused: the cap of requests is reached".to_owned());
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "the check has spent the requests it meant to",
        );
    }
    let mut request = inner
        .client
        .post(&inner.upstream)
        .bearer_auth(&inner.key)
        .body(body.clone());
    for name in PASSED {
        if let Some(value) = headers.get(&name) {
            request = request.header(name, value);
        }
    }
    let upstream = match request.send().await {
        Ok(answer) => answer,
        Err(error) => {
            inner
                .log
                .lock()
                .unwrap()
                .push(format!("upstream not reached: {}", error.without_url()));
            return refuse(StatusCode::BAD_GATEWAY, "the interface was not reached");
        }
    };
    let status = upstream.status();
    inner
        .log
        .lock()
        .unwrap()
        .push(format!("{} bytes up, answered {status}", body.len()));
    let mut response = Response::builder().status(status);
    if let Some(kind) = upstream.headers().get(header::CONTENT_TYPE) {
        response = response.header(header::CONTENT_TYPE, kind);
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap()
}
