//! Chat Completions turns cross the connector's real relay unchanged, streamed or whole, under the same lease and credential policy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use axum::{
    Json as HttpJson, Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use common::model::{CHAT, MODEL, Options, ProviderRig};
use orch_link::commands::Progress;
use orch_testkit::doubles::model::ClientRequest;
use orch_types::Json;
use serde_json::json;
use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
};

type Seen = Arc<Mutex<Vec<(HeaderMap, Json)>>>;

fn reply_chunks(stream: bool) -> Vec<Bytes> {
    let usage = json!({"prompt_tokens":100,"completion_tokens":30,"total_tokens":130,
        "prompt_tokens_details":{"cached_tokens":20},"completion_tokens_details":{"reasoning_tokens":7}});
    if stream {
        [json!({"id":"chatcmpl-local","object":"chat.completion.chunk","model":MODEL,
            "choices":[{"index":0,"delta":{"role":"assistant","content":"Hello "},"finish_reason":null}]}),
         json!({"id":"chatcmpl-local","object":"chat.completion.chunk","model":MODEL,
            "choices":[{"index":0,"delta":{"content":"local model","tool_calls":[{"index":0,"id":"call-local","type":"function","function":{"name":"local_tool","arguments":"{}"}}]},"finish_reason":"stop"}]}),
         json!({"id":"chatcmpl-local","object":"chat.completion.chunk","model":MODEL,"choices":[],"usage":usage})]
            .into_iter().map(|chunk|Bytes::from(format!("data: {chunk}\n\n")))
            .chain([Bytes::from_static(b"data: [DONE]\n\n")]).collect()
    } else {
        vec![Bytes::from(json!({"id":"chatcmpl-local","object":"chat.completion","model":MODEL,
        "choices":[{"index":0,"message":{"role":"assistant","content":"Hello local model",
            "tool_calls":[{"id":"call-local","type":"function","function":{"name":"local_tool","arguments":"{}"}}]},"finish_reason":"stop"}],
        "usage":usage,"vendor_result":{"kept":true}}).to_string())]
    }
}

async fn answer(
    State(seen): State<Seen>,
    headers: HeaderMap,
    HttpJson(body): HttpJson<Json>,
) -> Response {
    let stream = body["stream"] == true;
    seen.lock().unwrap().push((headers, body));
    let chunks = reply_chunks(stream).into_iter().map(Ok::<_, Infallible>);
    (
        [
            (
                "content-type",
                if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            ),
            ("x-chat-upstream", "kept"),
        ],
        Body::from_stream(futures::stream::iter(chunks)),
    )
        .into_response()
}

struct Server {
    base: String,
    seen: Seen,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Seen::default();
        let router = Router::new()
            .route("/v1/chat/completions", post(answer))
            .with_state(Arc::clone(&seen));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { base, seen, task }
    }
}

async fn a_chat_turn(stream: bool) {
    let server = Server::start().await;
    let p = ProviderRig::build(
        Options {
            chat_stream: stream,
            ..Options::default()
        },
        |rig, _| rig.config.provider.upstreams.push(server.base.clone()),
    )
    .await;
    let started = p
        .start_provider(&[(CHAT, server.base.clone(), json!({}))])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    p.rig
        .agent_start(json!({"forwarded_protocols":[CHAT]}))
        .await;
    p.open_forwarded_session(CHAT, MODEL).await;
    let lease = p.open_lease(p.thread, &[MODEL], Some(CHAT), None).await;
    p.start_turn_for("Hello", MODEL).await;
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;
    assert_eq!(p.rig.messages(p.thread, p.segment), ["Hello local model"]);
    let calls = p.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].status, 200);
    let expected: Vec<u8> = reply_chunks(stream)
        .into_iter()
        .flat_map(|chunk| chunk.to_vec())
        .collect();
    assert_eq!(
        calls[0].body.as_bytes(),
        expected,
        "tools, custom fields and SSE framing pass unchanged"
    );
    let seen = server.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0["authorization"], format!("Bearer {}", p.key));
    assert_eq!(
        seen[0].1,
        json!({"model":MODEL,"messages":[{"role":"user","content":"Hello"}],"stream":stream,
        "stream_options":{"include_usage":true},"tools":[{"type":"function","function":{"name":"local_tool","parameters":{"type":"object","properties":{}}}}],
        "vendor_option":{"keep":"unchanged"}})
    );
    assert!(!p.given.lock().unwrap().join("\n").contains(&p.key));
    let usage = p.wait_provider_event("lease.usage", 1).await;
    assert_eq!(usage[0].body["lease_id"], json!(lease));
    assert_eq!(usage[0].body["usage"]["total_tokens"], 130);
    assert_eq!(usage[0].body["usage"]["cached_input_tokens"], 20);
    assert_eq!(usage[0].body["usage"]["reasoning_tokens"], 7);
    let harness = p.harness(p.thread);
    let models = harness.get("/v1/models").await;
    assert_eq!(models.json()["data"][0]["id"], MODEL);
    let denied = harness
        .post(
            "/v1/chat/completions",
            &json!({"model":"other","messages":[]}),
        )
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(denied.json()["error"]["code"], "model_not_allowed");
    let denied = harness
        .client()
        .send(harness.request("DELETE", "/v1/chat/completions/saved", None))
        .await
        .unwrap();
    assert_eq!(denied.status, StatusCode::NOT_FOUND);
    let closed = p.close_lease(lease).await;
    assert_eq!(closed["requests"], 1);
    let denied = harness
        .client()
        .send(harness.request("POST", "/v1/chat/completions", Some(&seen[0].1)))
        .await
        .unwrap();
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(denied.json()["error"]["code"], "lease_closed");
    assert_eq!(
        server.seen.lock().unwrap().len(),
        1,
        "denied and model-list requests never reach upstream"
    );
    // The caller's token is still required on this newly supported format.
    let anonymous = harness
        .client()
        .send(ClientRequest::post_json(
            &format!("/llm/{}/v1/chat/completions", harness.session_key),
            &seen[0].1,
        ))
        .await
        .unwrap();
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_non_streamed_chat_completions_turn_is_relayed_under_the_existing_lease_policy() {
    a_chat_turn(false).await;
}

#[tokio::test]
async fn a_streamed_chat_completions_turn_is_relayed_under_the_existing_lease_policy() {
    a_chat_turn(true).await;
}
