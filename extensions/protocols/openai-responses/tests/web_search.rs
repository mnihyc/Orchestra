#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_model_contract::{
    parameters::{apply_request, resolve},
    protocol::OPENAI_RESPONSES,
};
use orch_protocol_openai_responses::OpenAiResponses;
use serde_json::{Value, json};

#[test]
fn search_usage_reads_successful_actions_and_authoritative_server_counts() {
    use orch_model_contract::protocol::WireProtocol;
    let reply = json!({"id":"reply-search","model":"chosen","usage":{"input_tokens":10,"output_tokens":2},"output":[
        {"type":"web_search_call","status":"completed","action":{"type":"search"}},
        {"type":"web_search_call","status":"completed","action":{"type":"open_page"}},
        {"type":"web_search_call","status":"failed","action":{"type":"search"}}
    ]});
    let mut observer = OpenAiResponses.usage_observer();
    observer.observe(reply.to_string().as_bytes());
    assert_eq!(observer.usage().unwrap().web_searches, Some(1));
    let mut deepseek = reply;
    deepseek["usage"]["server_tool_use"] = json!({"tool_search":3,"tool_fetch":1});
    let event = format!(
        "event: response.completed\ndata: {}\n\n",
        json!({"response":deepseek})
    );
    let mut observer = OpenAiResponses.usage_observer();
    for _ in 0..2 {
        for byte in event.as_bytes().chunks(1) {
            observer.observe(byte);
        }
    }
    assert_eq!(
        observer.usage().unwrap().web_searches,
        Some(3),
        "a cumulative final reply is not added twice"
    );
}

#[test]
fn search_modes_preserve_client_tools_and_use_protocol_fields() {
    let body = json!({"model":"chosen","input":"question","tools":[{"type":"function","name":"local_lookup"}]}).to_string();
    for (mode, expected) in [
        ("live", Some(true)),
        ("cached", Some(false)),
        ("disabled", None),
    ] {
        let sent = apply_request(&OpenAiResponses, "/responses", body.as_bytes(),
            &json!({"web_search":{"mode":mode,"search_context_size":"low","filters":{"allowed_domains":["example.com"]}}}), false).unwrap();
        let sent: Value = serde_json::from_slice(&sent).unwrap();
        assert_eq!(sent["tools"][0]["name"], "local_lookup");
        assert!(sent.get("web_search").is_none());
        if let Some(external) = expected {
            assert_eq!(sent["tools"][1]["type"], "web_search");
            assert_eq!(sent["tools"][1]["external_web_access"], external);
            assert_eq!(
                sent["tools"][1]["filters"]["allowed_domains"],
                json!(["example.com"])
            );
        } else {
            assert_eq!(sent["tools"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn offered_search_does_not_enable_it_and_explicit_off_overrides_the_default() {
    let mut model =
        serde_json::from_value(json!({"offered":{"web_search":["live","cached"]}})).unwrap();
    assert!(
        resolve(
            OPENAI_RESPONSES,
            &model,
            &json!({}),
            &json!({"web_search":"live"})
        )
        .is_err()
    );
    model =
        serde_json::from_value(json!({"web_search":["live"],"default_web_search":"live"})).unwrap();
    let options = resolve(
        OPENAI_RESPONSES,
        &model,
        &json!({"web_search":{"search_context_size":"low"}}),
        &json!({"web_search":"disabled"}),
    )
    .unwrap();
    assert_eq!(options["web_search"]["mode"], "disabled");
    assert_eq!(options["web_search"]["search_context_size"], "low");
    assert!(
        resolve(
            OPENAI_RESPONSES,
            &model,
            &json!({}),
            &json!({"web_search":"cached"})
        )
        .is_err()
    );
}

#[test]
fn defaults_do_not_enable_an_omitted_tool_or_modify_compaction() {
    let body = br#"{"model":"chosen","input":"text"}"#;
    for path in ["/responses", "/responses/compact"] {
        let sent = apply_request(
            &OpenAiResponses,
            path,
            body,
            &json!({"web_search":"live"}),
            true,
        )
        .unwrap();
        let sent: Value = serde_json::from_slice(&sent).unwrap();
        assert!(sent.get("tools").is_none());
    }
    assert!(
        apply_request(
            &OpenAiResponses,
            "/responses/compact",
            body,
            &json!({"web_search":"live"}),
            false
        )
        .is_err()
    );
}

#[test]
fn a_search_option_cannot_replace_tool_identity_or_other_tools() {
    for config in [
        json!({"type":"function"}),
        json!({"filters":{"allowed_domains":["example.com"],"blocked_domains":["example.org"]}}),
    ] {
        assert!(
            apply_request(
                &OpenAiResponses,
                "/responses",
                b"{}",
                &json!({"web_search":config}),
                false
            )
            .is_err()
        );
    }
}

#[test]
fn runtime_defaults_preserve_the_harness_tool_version_and_explicit_options() {
    let body = br#"{"tools":[{"type":"web_search_preview","search_context_size":"high","user_location":{"type":"approximate","city":"London"}}]}"#;
    let sent = apply_request(&OpenAiResponses, "/responses", body,
        &json!({"web_search":{"mode":"live","search_context_size":"low","user_location":{"type":"approximate","timezone":"Europe/London"}}}), true).unwrap();
    let sent: Value = serde_json::from_slice(&sent).unwrap();
    assert_eq!(sent["tools"][0]["type"], "web_search_preview");
    assert_eq!(sent["tools"][0]["search_context_size"], "high");
    assert_eq!(sent["tools"][0]["user_location"]["city"], "London");
    assert_eq!(
        sent["tools"][0]["user_location"]["timezone"],
        "Europe/London"
    );
}

#[test]
fn discovered_search_prices_keep_the_per_call_unit() {
    use orch_model_contract::protocol::WireProtocol;
    let body = json!({"data":[{"id":"chosen","pricing":{"prompt":"0.000002","completion":"0.000008","web_search":"0.00075"}}]});
    let models = OpenAiResponses.parse_models(body.to_string().as_bytes());
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
