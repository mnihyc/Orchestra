//! Required fast headers follow the actual normalized body and preserve existing feature lists.
#![allow(clippy::unwrap_used, reason = "a protocol control fails by panicking")]
use http::{HeaderMap, HeaderValue};
use orch_model_contract::{
    endpoint::EndpointOptions,
    parameters::outgoing_at,
    protocol::{SecretHeaders, WireProtocol},
};
use orch_protocol_anthropic_messages::AnthropicMessages;
use serde_json::{Value, json};

#[test]
fn fast_request_headers_join_program_endpoint_and_required_betas_once() {
    let p = AnthropicMessages;
    let entry = serde_json::from_value(
        json!({"service_tiers":[{"id":"fast","label":"Fast"}],"default_service_tier":"fast"}),
    )
    .unwrap();
    let body = outgoing_at(
        p.name(),
        "/v1/messages",
        &entry,
        br#"{"model":"selected","messages":[],"max_tokens":32}"#,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["speed"],
        "fast"
    );
    let options=EndpointOptions::parse(&json!({"auth":"none","headers":{"anthropic-beta":"shared, endpoint","anthropic-version":"explicit-version"}})).unwrap();
    let mut headers = HeaderMap::new();
    headers.append(
        "anthropic-beta",
        HeaderValue::from_static("program, shared"),
    );
    headers.append("anthropic-beta", HeaderValue::from_static("second-program"));
    p.authorize(&mut headers, None, &options, &SecretHeaders::new())
        .unwrap();
    p.finalize_request_headers(&mut headers, &body).unwrap();
    p.finalize_request_headers(&mut headers, &body).unwrap();
    assert_eq!(
        headers["anthropic-beta"],
        "program, shared, second-program, endpoint, fast-mode-2026-02-01"
    );
    assert_eq!(headers.get_all("anthropic-beta").iter().count(), 1);
    assert_eq!(headers["anthropic-version"], "explicit-version");
}

#[test]
fn ordinary_or_omitted_speed_does_not_acquire_a_fast_beta_from_a_default() {
    let p = AnthropicMessages;
    for speed in [
        None,
        Some(json!(null)),
        Some(json!("standard")),
        Some(json!("auto")),
    ] {
        let mut body = json!({"model":"selected","messages":[],"max_tokens":32});
        if let Some(speed) = speed {
            body["speed"] = speed;
        }
        let body = outgoing_at(
            p.name(),
            "/v1/messages",
            &Default::default(),
            &serde_json::to_vec(&body).unwrap(),
        )
        .unwrap();
        assert!(
            serde_json::from_slice::<Value>(&body)
                .unwrap()
                .get("speed")
                .is_none()
        );
        let mut headers = HeaderMap::new();
        let options = EndpointOptions::parse(&json!({"auth":"none"})).unwrap();
        p.authorize(&mut headers, None, &options, &SecretHeaders::new())
            .unwrap();
        p.finalize_request_headers(&mut headers, &body).unwrap();
        assert!(headers.get("anthropic-beta").is_none());
    }
}
