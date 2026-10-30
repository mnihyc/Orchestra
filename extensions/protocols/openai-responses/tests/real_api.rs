//! One real request in the Responses format, to see that the usage observer reads what a real API says.
//!
//! The check makes a request to a real API, so it runs only when the variable `ORCH_REAL_API_CHECK` is `1`; a
//! test run without it makes no request. The key is read from the variable `OPENROUTER_API_KEY` and goes
//! nowhere but into the credential of the protocol: it is not printed, not logged and not written. The request
//! is the smallest that gets an answer (a few tokens in, a few out) to the cheapest current model of the format.
//! When `ORCH_REAL_API_CAPTURE_DIR` names a directory, the answer's bytes (which hold no key) are also written
//! there, so that a change in what the vendor sends can be looked at.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::time::Duration;

use http::{HeaderMap, HeaderValue};
use orch_model_contract::credentials::Credential;
use orch_model_contract::endpoint::EndpointOptions;
use orch_model_contract::protocol::{SecretHeaders, WireProtocol};
use orch_protocol_openai_responses::OpenAiResponses;
use serde_json::json;

/// The variable that switches the check on.
const SWITCH: &str = "ORCH_REAL_API_CHECK";

/// The variable that holds the key of the API.
const KEY: &str = "OPENROUTER_API_KEY";

/// The variable that names a directory to write the answer into.
const CAPTURE: &str = "ORCH_REAL_API_CAPTURE_DIR";

/// The base address of an endpoint in the Responses format at the API of the checks.
const BASE: &str = "https://openrouter.ai/api/v1";

/// The cheapest current model of the format.
const MODEL: &str = "openai/gpt-5.6-luna";

/// Output tokens the request allows: room for a model that reasons first, and no more.
const MAX_OUTPUT_TOKENS: u32 = 64;

/// How long the request may take.
const PATIENCE: Duration = Duration::from_secs(90);

#[tokio::test]
async fn the_usage_observer_reads_the_usage_of_a_real_stream() {
    if std::env::var(SWITCH).as_deref() != Ok("1") {
        eprintln!("skipped: set {SWITCH}=1 to make one real request");
        return;
    }
    let key = std::env::var(KEY).unwrap_or_else(|_| panic!("{KEY} is not set"));
    let protocol = OpenAiResponses;
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    protocol
        .authorize(
            &mut headers,
            Some(&Credential::new(key)),
            &EndpointOptions::default(),
            &SecretHeaders::new(),
        )
        .unwrap();
    let body = json!({
        "model": MODEL, "input": "Reply with the single word: ok",
        "max_output_tokens": MAX_OUTPUT_TOKENS, "stream": true
    });
    assert!(protocol.allowed(&http::Method::POST, "/responses"));
    assert_eq!(
        protocol
            .model_of("/responses", body.to_string().as_bytes())
            .as_deref(),
        Some(MODEL)
    );
    let client = reqwest::Client::builder()
        .timeout(PATIENCE)
        .build()
        .unwrap();
    let mut response = client
        .post(format!("{BASE}/responses"))
        .headers(headers)
        .body(body.to_string())
        .send()
        .await
        .expect("the request could be made");
    let status = response.status();
    let mut observer = protocol.usage_observer();
    let mut whole = Vec::new();
    while let Some(chunk) = response.chunk().await.expect("the answer arrives") {
        observer.observe(&chunk);
        whole.extend_from_slice(&chunk);
    }
    if let Ok(dir) = std::env::var(CAPTURE) {
        std::fs::write(
            std::path::Path::new(&dir).join("responses-stream.txt"),
            &whole,
        )
        .unwrap();
    }
    assert!(
        status.is_success(),
        "the API answered {status}: {}",
        String::from_utf8_lossy(&whole)
            .chars()
            .take(300)
            .collect::<String>()
    );
    let usage = observer.usage().expect("the stream says what it used");
    eprintln!(
        "real usage: model={:?} input={} cached={} output={} reasoning={} total={} cost={:?} ref_present={}",
        usage.model,
        usage.tokens.input_tokens,
        usage.tokens.cached_input_tokens,
        usage.tokens.output_tokens,
        usage.tokens.reasoning_tokens,
        usage.tokens.total_tokens,
        usage.cost,
        usage.request_ref.is_some()
    );
    assert!(usage.tokens.input_tokens > 0 && usage.tokens.output_tokens > 0);
    assert_eq!(
        usage.tokens.total_tokens,
        usage.tokens.input_tokens + usage.tokens.output_tokens
    );
    assert!(usage.tokens.is_consistent());
    assert!(usage.model.is_some() && usage.request_ref.is_some());
}
