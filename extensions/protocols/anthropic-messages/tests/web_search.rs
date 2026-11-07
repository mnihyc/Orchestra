#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_model_contract::parameters::apply_request;
use orch_protocol_anthropic_messages::AnthropicMessages;
use serde_json::{Value, json};

#[test]
fn messages_search_usage_is_cumulative_and_missing_is_unknown() {
    use orch_model_contract::protocol::WireProtocol;
    let mut observer = AnthropicMessages.usage_observer();
    let start = "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n";
    observer.observe(start.as_bytes());
    assert_eq!(observer.usage().unwrap().web_searches, None);
    let delta = "event: message_delta\ndata: {\"usage\":{\"output_tokens\":5,\"server_tool_use\":{\"web_search_requests\":2}}}\n\n";
    for _ in 0..2 {
        for byte in delta.as_bytes().chunks(1) {
            observer.observe(byte);
        }
    }
    assert_eq!(observer.usage().unwrap().web_searches, Some(2));
}

#[test]
fn messages_search_is_a_server_tool_beside_existing_client_tools() {
    let body = br#"{"model":"chosen","messages":[],"tools":[{"name":"local_lookup","input_schema":{"type":"object"}}]}"#;
    let sent = apply_request(
        &AnthropicMessages,
        "/v1/messages",
        body,
        &json!({"web_search":{"mode":"live","max_uses":3,"allowed_domains":["example.com"]}}),
        false,
    )
    .unwrap();
    let sent: Value = serde_json::from_slice(&sent).unwrap();
    assert_eq!(sent["tools"][0]["name"], "local_lookup");
    assert_eq!(
        sent["tools"][1],
        json!({"type":"web_search_20250305","name":"web_search","max_uses":3,"allowed_domains":["example.com"]})
    );
    let disabled = apply_request(
        &AnthropicMessages,
        "/v1/messages",
        sent.to_string().as_bytes(),
        &json!({"web_search":"disabled"}),
        false,
    )
    .unwrap();
    let disabled: Value = serde_json::from_slice(&disabled).unwrap();
    assert_eq!(disabled["tools"].as_array().unwrap().len(), 1);
}

#[test]
fn messages_refuses_cache_only_and_conflicting_domain_lists() {
    for config in [
        json!("cached"),
        json!({"max_uses":"many"}),
        json!({"allowed_domains":["example.com"],"blocked_domains":["example.org"]}),
    ] {
        assert!(
            apply_request(
                &AnthropicMessages,
                "/v1/messages",
                b"{}",
                &json!({"web_search":config}),
                false
            )
            .is_err()
        );
    }
    assert!(
        apply_request(
            &AnthropicMessages,
            "/v1/messages/count_tokens",
            b"{}",
            &json!({"web_search":"live"}),
            false
        )
        .is_err()
    );
}

#[test]
fn discovered_search_prices_keep_the_per_call_unit() {
    use orch_model_contract::protocol::WireProtocol;
    let body = json!({"data":[{"id":"chosen","pricing":{"prompt":"0.000002","completion":"0.000008","web_search":"0.00075"}}]});
    let models = AnthropicMessages.parse_models(body.to_string().as_bytes());
    assert_eq!(
        models[0]
            .parameters
            .rate
            .as_ref()
            .unwrap()
            .web_search_per_call
            .as_deref(),
        Some("0.00075")
    );
}
