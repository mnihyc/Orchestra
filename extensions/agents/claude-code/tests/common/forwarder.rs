//! A forwarder on the loopback that adds the key, the way the connector's does, so that a real model service can be reached while the harness holds none.
//!
//! The harness is given this server's address and the connector's token. Every request it makes is passed on
//! to the upstream with the key as a bearer token, and the answer is streamed back as it arrives. The key lives
//! in this process only: it is not in the harness's environment, its arguments or its container.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses a part of this")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_agent_contract::loopback::Loopback;
use orch_plugin_contract::Secret;
use tokio::task::JoinHandle;

use super::TOKEN;

/// The largest request body passed on: a long conversation with its tool results is a few megabytes.
const BODY_LIMIT: usize = 64 * 1024 * 1024;

/// The headers of the harness's request that are passed on; the credential is not among them.
const PASSED_ON: [&str; 6] = [
    "content-type",
    "accept",
    "anthropic-version",
    "anthropic-beta",
    "x-app",
    "user-agent",
];

struct Shared {
    upstream: String,
    key: String,
    client: reqwest::Client,
    requests: AtomicUsize,
    refused: AtomicUsize,
}

/// A running forwarder.
pub struct Forwarder {
    address: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Forwarder {
    /// Starts one that passes requests to `upstream` (no trailing slash) with `key`, on a free port of the range
    /// reserved for tests.
    pub async fn start(upstream: &str, key: &str) -> Self {
        let shared = Arc::new(Shared {
            upstream: upstream.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
            client: reqwest::Client::new(),
            requests: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
        });
        let app = Router::new()
            .fallback(forward)
            .with_state(Arc::clone(&shared));
        let seed = u64::from(std::process::id()) * 13 + 5;
        for attempt in 0..400u64 {
            let port = 41_000 + ((seed + attempt * 11) % 1000);
            let address = SocketAddr::from(([127, 0, 0, 1], u16::try_from(port).unwrap()));
            if let Ok(listener) = tokio::net::TcpListener::bind(address).await {
                let app = app.clone();
                let task = tokio::spawn(async move {
                    let _ = axum::serve(listener, app).await;
                });
                return Self {
                    address,
                    shared,
                    task,
                };
            }
        }
        panic!("no free port between 41000 and 41999");
    }

    /// The loopback the harness is given: this server's port and the connector's token.
    pub fn loopback(&self) -> Loopback {
        Loopback::new(self.address.port(), Secret::new(TOKEN))
    }

    /// How many requests were passed on.
    pub fn requests(&self) -> usize {
        self.shared.requests.load(Ordering::Relaxed)
    }

    /// How many were refused for presenting a token other than the connector's.
    pub fn refused(&self) -> usize {
        self.shared.refused.load(Ordering::Relaxed)
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn forward(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let presented = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if presented != Some(TOKEN) {
        shared.refused.fetch_add(1, Ordering::Relaxed);
        return StatusCode::UNAUTHORIZED.into_response();
    }
    shared.requests.fetch_add(1, Ordering::Relaxed);
    // /llm/<session key>/v1/messages?beta=true is the upstream's /v1/messages?beta=true.
    let path = parts.uri.path();
    let rest = path
        .strip_prefix("/llm/")
        .and_then(|after| after.split_once('/'))
        .map_or_else(|| path.to_owned(), |(_, rest)| format!("/{rest}"));
    let query = parts
        .uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let url = format!("{}{rest}{query}", shared.upstream);
    let Ok(bytes) = axum::body::to_bytes(body, BODY_LIMIT).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let mut outgoing = shared.client.request(parts.method, &url).body(bytes);
    for name in PASSED_ON {
        if let Some(value) = parts.headers.get(name) {
            outgoing = outgoing.header(name, value);
        }
    }
    outgoing = outgoing.header(header::AUTHORIZATION, format!("Bearer {}", shared.key));
    let upstream = match outgoing.send().await {
        Ok(upstream) => upstream,
        Err(error) => {
            return (StatusCode::BAD_GATEWAY, format!("{error}")).into_response();
        }
    };
    let status = upstream.status();
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let mut response = Response::builder().status(status);
    if let Some(value) = content_type {
        response = response.header(header::CONTENT_TYPE, value);
    }
    response = response.header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap()
}
