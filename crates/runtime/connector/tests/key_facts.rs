//! The transport a vendor's reader uses on the machine that keeps the key: the key as a bearer token to the vendor's own address, a JSON answer read, a refusal or another answer telling nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::routing::get;
use orch_connector_core::upstream::Upstream;
use orch_plugin_contract::Secret;
use orch_provider_contract::builtins::VendorClient;
use orch_types::SystemClock;
use serde_json::json;
use std::sync::{Arc, Mutex};
use url::Url;

/// What the vendor was asked: the path with its query, and whether the key came as a bearer token.
type Asked = Arc<Mutex<Vec<(String, bool)>>>;

struct Vendor {
    base: Url,
    asked: Asked,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Vendor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn answer(State(asked): State<Asked>, uri: Uri, headers: HeaderMap) -> (StatusCode, String) {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some("Bearer made-up-key");
    asked.lock().unwrap().push((uri.to_string(), bearer));
    match uri.path() {
        "/api/v1/key" => (StatusCode::OK, json!({"data": {"usage": 3.0}}).to_string()),
        "/api/v1/text" => (StatusCode::OK, "not json".to_owned()),
        _ => (
            StatusCode::FORBIDDEN,
            json!({"error": "Only management keys can perform this operation"}).to_string(),
        ),
    }
}

async fn vendor() -> Vendor {
    let mut bound = None;
    for port in 41450..41470 {
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            bound = Some(listener);
            break;
        }
    }
    let listener = bound.unwrap();
    let base = Url::parse(&format!(
        "http://{}/api/v1?ignored=1",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let asked: Asked = Arc::default();
    let router = Router::new()
        .route("/api/v1/key", get(answer))
        .route("/api/v1/text", get(answer))
        .route("/api/v1/credits", get(answer))
        .with_state(asked.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Vendor { base, asked, task }
}

#[tokio::test]
async fn the_key_goes_as_a_bearer_token_to_the_vendors_own_path_and_its_json_answer_is_read() {
    let vendor = vendor().await;
    let client = Upstream::new(Arc::new(SystemClock)).unwrap();
    let key = Secret::new("made-up-key");
    let answer = client.get(&vendor.base, "/api/v1/key", &key, 1024).await;
    assert_eq!(answer, Some(json!({"data": {"usage": 3.0}})));
    assert_eq!(
        vendor.asked.lock().unwrap().as_slice(),
        [("/api/v1/key".to_owned(), true)],
        "the base's own query is not carried to the vendor's path"
    );
}

#[tokio::test]
async fn a_refusal_or_an_answer_that_is_no_json_tells_nothing() {
    let vendor = vendor().await;
    let client = Upstream::new(Arc::new(SystemClock)).unwrap();
    let key = Secret::new("made-up-key");
    assert_eq!(
        client
            .get(&vendor.base, "/api/v1/credits", &key, 1024)
            .await,
        None,
        "OpenRouter's 403 to an ordinary key"
    );
    assert_eq!(
        client.get(&vendor.base, "/api/v1/text", &key, 1024).await,
        None
    );
    assert_eq!(vendor.asked.lock().unwrap().len(), 2);
}
