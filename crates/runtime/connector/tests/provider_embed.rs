//! Embeddings made on the system's own behalf in the provider's runtime: a batch of texts goes to the endpoint of the Embeddings format with the key and the secret headers its options say, the vectors come back in the texts' order with what they used, and a refusal says what was used without the secrets.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::model::ProviderRig;
use orch_link::commands::Progress;
use orch_link_contract::commands::{ProviderEmbed, decode_vector};
use orch_types::new_id;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The format's name, as providers write it.
const EMBEDDINGS: &str = "openai-embeddings";
/// The value of the endpoint's secret header.
const SECRET: &str = "tenant-secret-0451";

/// What a stub server was sent: the header lines, lowercased names, and the body.
type Seen = Arc<Mutex<Vec<(Vec<(String, String)>, Value)>>>;

/// A stub of an embeddings server on a port of its own. Each `POST /v1/embeddings` is answered with `status`;
/// a success gives every text a vector whose first number is the text's length in characters, the items in the
/// reverse order of the texts, so that the reader has to put them back by their index.
async fn stub(status: u16) -> (String, Seen) {
    stub_reply(status, None).await
}

async fn stub_reply(status: u16, answer: Option<Value>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let kept = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let kept = Arc::clone(&kept);
            let answer = answer.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 4096];
                let (head, mut body) = loop {
                    let read = socket.read(&mut buffer).await.unwrap();
                    assert!(read > 0, "the request ended before its headers did");
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        break (
                            String::from_utf8_lossy(&bytes[..end]).into_owned(),
                            bytes[end + 4..].to_vec(),
                        );
                    }
                };
                let headers: Vec<(String, String)> = head
                    .lines()
                    .skip(1)
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_owned()))
                    .collect();
                let length: usize = headers
                    .iter()
                    .find(|(name, _)| name == "content-length")
                    .map_or(0, |(_, value)| value.parse().unwrap());
                while body.len() < length {
                    let read = socket.read(&mut buffer).await.unwrap();
                    body.extend_from_slice(&buffer[..read]);
                }
                let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                assert!(head.starts_with("POST /v1/embeddings "), "{head}");
                let texts: Vec<String> = request["input"]
                    .as_array()
                    .map(|texts| {
                        texts
                            .iter()
                            .map(|text| text.as_str().unwrap().to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                kept.lock().unwrap().push((headers, request));
                let answer = if let Some(answer) = answer {
                    answer
                } else if status == 200 {
                    let data: Vec<Value> = texts
                        .iter()
                        .enumerate()
                        .rev()
                        .map(|(index, text)| {
                            let length = text.chars().count() as f64;
                            json!({"object":"embedding","index":index,"embedding":[length, 0.5]})
                        })
                        .collect();
                    json!({"object":"list","model":"local","data":data,
                           "usage":{"prompt_tokens":11,"total_tokens":11}})
                } else {
                    json!({"error":{"message":format!("refused, though {SECRET} was sent"),"type":"auth"}})
                };
                let text = answer.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
                socket.shutdown().await.ok();
            });
        }
    });
    (url, seen)
}

/// A rig whose connector may reach the stub, with the rig's provider started on it: no key, one secret header.
async fn started(url: &str) -> ProviderRig {
    let reachable = url.to_owned();
    let p = ProviderRig::with(move |rig, _| rig.config.provider.upstreams.push(reachable)).await;
    let id = new_id();
    let body: orch_link_contract::commands::ProviderStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider,
        "protocols": [EMBEDDINGS],
        "upstreams": [{"protocol": EMBEDDINGS, "base_url": url,
                       "options": {"auth": "none", "secret_headers": ["x-embed-secret"]}}],
        "credential": null,
        "secrets": {"x-embed-secret": p.sealed_key_for(id, SECRET)}
    }))
    .unwrap();
    let progress = p.command_with_id(id, &p.provider_key(), &body).await;
    assert!(
        matches!(progress, Progress::Succeeded(_)),
        "provider.start: {progress:?}"
    );
    p
}

fn asked() -> ProviderEmbed {
    ProviderEmbed {
        provider_id: new_id(),
        protocol: EMBEDDINGS.to_owned(),
        model_name: "local".to_owned(),
        inputs: vec![
            "title: none | text: 发布流程的说明".to_owned(),
            "title: none | text: the rollout plan".to_owned(),
        ],
        dimensions: Some(256),
    }
}

#[tokio::test]
async fn texts_become_vectors_in_their_order_through_the_endpoint_with_its_secret_header_and_no_key()
 {
    let (url, seen) = stub(200).await;
    let p = started(&url).await;
    let reply = p
        .rig
        .succeed(
            &p.provider_key(),
            &ProviderEmbed {
                provider_id: p.rig.provider,
                ..asked()
            },
        )
        .await;
    let vectors: Vec<Vec<f32>> = reply["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|vector| decode_vector(vector.as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        vectors,
        vec![vec![27.0, 0.5], vec![36.0, 0.5]],
        "each text's own vector, in the order the texts were given"
    );
    assert_eq!(reply["usage"]["input_tokens"], 11);
    assert_eq!(reply["usage"]["output_tokens"], 0);
    assert_eq!(reply["timing"]["request_ms"]["state"], "told");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one request for the batch");
    let (headers, body) = &seen[0];
    let header = |name: &str| {
        headers
            .iter()
            .find(|(each, _)| each == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(header("x-embed-secret"), Some(SECRET));
    assert_eq!(header("authorization"), None, "the endpoint takes no key");
    assert_eq!(body["model"], "local");
    assert_eq!(body["dimensions"], 256);
    assert_eq!(body["encoding_format"], "float");
    assert_eq!(body["input"][0], "title: none | text: 发布流程的说明");
}

#[tokio::test]
async fn a_refused_batch_fails_with_what_it_used_and_without_the_secret() {
    let (url, _) = stub(401).await;
    let p = started(&url).await;
    let result = p
        .rig
        .send(
            &p.provider_key(),
            &ProviderEmbed {
                provider_id: p.rig.provider,
                ..asked()
            },
        )
        .await;
    let Progress::Failed(error) = &result else {
        panic!("the endpoint refused: {result:?}");
    };
    assert_eq!(error.code, "login_required");
    let raw: orch_agent_contract::requests::UsageRequest =
        serde_json::from_value(error.details.as_ref().unwrap()["model_request"].clone()).unwrap();
    raw.check().unwrap();
    assert_eq!(
        raw.measured().outcome.value().map(String::as_str),
        Some("provider_unauthorized")
    );
    assert!(
        !format!("{result:?}").contains(SECRET),
        "the secret header's value is never repeated"
    );
}

#[tokio::test]
async fn a_successful_embedding_does_not_record_a_secret_echoed_as_the_response_id() {
    let (url, _) = stub_reply(
        200,
        Some(json!({
            "id": format!("request-{SECRET}"),
            "object": "list", "model": "local",
            "data": [
                {"object": "embedding", "index": 0, "embedding": [1.0, 0.5]},
                {"object": "embedding", "index": 1, "embedding": [2.0, 0.5]}
            ],
            "usage": {"prompt_tokens": 11, "total_tokens": 11}
        })),
    )
    .await;
    let p = started(&url).await;
    let reply = p
        .rig
        .succeed(
            &p.provider_key(),
            &ProviderEmbed {
                provider_id: p.rig.provider,
                ..asked()
            },
        )
        .await;
    assert!(!reply.to_string().contains(SECRET));
    assert_eq!(reply["request_ref"], "request-[secret]");
    assert_eq!(reply["usage"]["input_tokens"], 11);
}

#[tokio::test]
async fn embedding_errors_keep_their_classified_cause_and_a_filter_does_not_refuse_the_key() {
    for (status, kind, reason, error_type, message) in [
        (
            429,
            "provider_busy",
            None,
            "rate_limit_error",
            "Capacity reached",
        ),
        (
            403,
            "model_refused",
            Some("cyber"),
            "content_filter",
            "Flagged for cybersecurity risk",
        ),
        (
            200,
            "provider_busy",
            None,
            "overloaded_error",
            "The model is overloaded",
        ),
    ] {
        let (url, seen) = stub_reply(
            status,
            Some(json!({
                "error": {"type": error_type, "message": format!("{message}; {SECRET}")}
            })),
        )
        .await;
        let p = started(&url).await;
        let result = p
            .rig
            .send(
                &p.provider_key(),
                &ProviderEmbed {
                    provider_id: p.rig.provider,
                    ..asked()
                },
            )
            .await;
        let Progress::Failed(error) = &result else {
            panic!("the scripted endpoint refused this request: {result:?}");
        };
        assert_eq!(
            error.code, "internal",
            "a filter or capacity refusal is not a dead credential"
        );
        let details = error.details.as_ref().unwrap();
        assert_eq!(details["failure"]["code"], kind);
        assert_eq!(details["failure"]["reason"], json!(reason));
        assert_eq!(details["failure"]["status"], status);
        let request: orch_agent_contract::requests::UsageRequest =
            serde_json::from_value(details["model_request"].clone()).unwrap();
        request.check().unwrap();
        assert_eq!(request.id, "embeddings");
        assert_eq!(
            request.measured().outcome.value().map(String::as_str),
            Some(kind)
        );
        assert!(!format!("{result:?}").contains(SECRET));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn an_endpoint_of_another_format_cannot_make_vectors() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    let result = p
        .rig
        .send(
            &p.provider_key(),
            &ProviderEmbed {
                provider_id: p.rig.provider,
                protocol: common::model::RESPONSES.to_owned(),
                model_name: common::model::MODEL.to_owned(),
                ..asked()
            },
        )
        .await;
    assert!(
        matches!(&result, Progress::Failed(error) if error.code == "unsupported"),
        "{result:?}"
    );
    assert!(p.model_requests().is_empty(), "nothing reached the model");
}
