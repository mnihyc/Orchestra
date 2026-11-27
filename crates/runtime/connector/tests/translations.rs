//! Registered external translators carry a leased model request across formats, convert streamed tools and usage, and leave endpoint credential custody intact.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::model::{MESSAGES, MODEL, ProviderRig, RESPONSES, protocols};
use orch_connector_core::roles::provider::{ProviderParts, plug_in};
use orch_link::commands::Progress;
use orch_plugin::registration::{BuiltIn, Plugins, registration_of};
use orch_plugin::supervisor::Options;
use orch_plugin_contract::registration::PluginHost;
use orch_testkit::doubles::model::Verdict;
use serde_json::json;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_external_translator_carries_json_streamed_tools_errors_and_usage_through_the_real_connector()
 {
    let data = tempfile::tempdir().unwrap();
    let program = data.path().join("translator.py");
    orch_testkit::doubles::plugin::write_translator(&program).unwrap();
    let registration = registration_of(
        "translator",
        "controlled-translator",
        &[
            if cfg!(windows) { "python" } else { "python3" }.to_owned(),
            program.to_string_lossy().into_owned(),
        ],
        None,
        &std::collections::BTreeMap::new(),
        "never",
    )
    .unwrap();
    let plugins = Plugins::start(
        PluginHost::Connector,
        &[registration],
        &BuiltIn::new(),
        &Options::new("test"),
    )
    .await
    .unwrap();
    tokio::time::timeout(common::PATIENCE, plugins.settled())
        .await
        .unwrap();
    let translators: Vec<_> = plugins
        .translators()
        .into_iter()
        .map(|(_, translator)| translator)
        .collect();
    assert_eq!(translators.len(), 1);
    let mut p = ProviderRig::with(move |rig, _| {
        rig.plug = Some(Arc::new(move |builder, roles, data, key, clock| {
            let parts = ProviderParts::new(
                protocols().with_translators(translators.clone()),
                data,
                key,
                clock,
            )
            .unwrap();
            plug_in(builder, roles, parts)
        }));
    })
    .await;
    let started = p
        .start_provider(&[(MESSAGES, p.mock.messages_base_url(), json!({}))])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    p.mock
        .queue("say Translated answer.\nusage 100 20 cached=10 cache_write=5")
        .unwrap();
    let answer = harness
        .post("/v1/responses", &json!({"model": MODEL, "input":"hello"}))
        .await;
    assert_eq!(answer.status, 200);
    assert_eq!(
        answer.json()["output"][0]["content"][0]["text"],
        "Translated answer."
    );
    p.mock
        .queue("say Streamed answer.\ntool lookup {\"name\":\"world\"}\nusage 200 30")
        .unwrap();
    let streamed = harness.post("/v1/responses", &json!({"model":MODEL, "input":"hello", "stream":true,
        "tools":[{"type":"function", "name":"lookup", "parameters":{"type":"object", "properties":{"name":{"type":"string"}}}}]
    })).await;
    assert_eq!(streamed.status, 200);
    let text = streamed.text();
    assert!(
        text.contains("response.output_text.delta")
            && text.contains("response.function_call_arguments.delta"),
        "{text}"
    );
    let beyond_ceiling = harness
        .post(
            "/v1/responses",
            &json!({"model":MODEL,"input":"hello",
        "translated_options":{"speed":"ultrafast"}}),
        )
        .await;
    assert_eq!(
        beyond_ceiling.status, 403,
        "translation cannot introduce a disabled upstream speed"
    );
    assert_eq!(
        p.model_requests().len(),
        2,
        "the translated refusal never reached upstream HTTP"
    );
    let usage = p.wait_provider_event("lease.usage", 2).await;
    assert_eq!(usage[0].body["usage"]["input_tokens"], 100);
    assert_eq!(usage[0].body["usage"]["cache_write_tokens"], 5);
    let close = p.close_lease(lease).await;
    assert_eq!(close["requests"], 2);
    assert_eq!(close["usage"]["total_tokens"], 350);
    let requests = p.model_requests();
    assert!(
        requests
            .iter()
            .all(|request| request.path.ends_with("/messages")
                && request.credential.verdict == Verdict::Accepted
                && request.credential.presented_by.len() == 1)
    );
    p.mock.set_credential(Some("refused-key"));
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let refused = harness
        .post("/v1/responses", &json!({"model":MODEL, "input":"hello"}))
        .await;
    assert_eq!(refused.status, 401);
    assert!(refused.json().get("error").is_some());
    assert!(!refused.text().contains(&p.key));
    p.rig.kill().await;
    plugins.shutdown().await;
}

#[test]
fn the_builtin_translator_keeps_tools_images_signed_reasoning_and_declares_its_losses() {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::{MessagesToResponses, ResponsesToMessages};
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        output_tokens: 4096,
        session: None,
        model: Some(
            serde_json::from_value(json!({
                "offered":{"report":{"output_max":8192}}
            }))
            .unwrap(),
        ),
    };
    let request = json!({
        "model":MODEL, "instructions":"Follow the user's instructions.", "stream":true,
        "input":[
            {"role":"user","content":[{"type":"input_text","text":"Look"},{"type":"input_image","image_url":"data:image/png;base64,YQ=="}]},
            {"type":"function_call","call_id":"call_a","name":"lookup","arguments":"{\"name\":\"world\"}"},
            {"type":"function_call_output","call_id":"call_a","output":"found"}
        ],
        "tools":[{"type":"custom","name":"apply_patch","description":"Edit a file","format":{"type":"grammar","syntax":"lark","definition":"start: \"patch\""}}],
        "tool_choice":"auto","parallel_tool_calls":false
    });
    let translated = ResponsesToMessages
        .translate_request("/responses", Bytes::from(request.to_string()), &upstream)
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&translated.body).unwrap();
    assert_eq!(translated.path, "/v1/messages");
    assert!(!translated.authorized);
    assert_eq!(body["max_tokens"], 8192);
    assert_eq!(body["messages"][0]["content"][1]["source"]["data"], "YQ==");
    assert_eq!(body["messages"][1]["content"][0]["id"], "call_a");
    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "call_a");
    assert_eq!(
        body["tools"][0]["input_schema"]["properties"]["input"]["type"],
        "string"
    );
    assert_eq!(body["tool_choice"]["disable_parallel_tool_use"], true);
    let original = json!({"id":"msg_answer","type":"message","model":MODEL,"role":"assistant",
        "content":[{"type":"thinking","thinking":"A summary","signature":"exact+signature/="},
          {"type":"tool_use","id":"call_patch","name":"apply_patch","input":{"input":"patch"}}],
        "stop_reason":"tool_use","usage":{"input_tokens":30,"cache_read_input_tokens":7,"cache_creation_input_tokens":5,"output_tokens":8}});
    let mut converter = ResponsesToMessages
        .response_for("/responses", request.to_string().as_bytes(), &upstream)
        .unwrap();
    assert!(
        converter
            .translate_chunk(Bytes::from(original.to_string()))
            .unwrap()
            .is_empty()
    );
    let result: serde_json::Value = serde_json::from_slice(&converter.finish().unwrap()).unwrap();
    assert_eq!(result["output"][1]["type"], "custom_tool_call");
    assert_eq!(result["output"][1]["input"], "patch");
    assert_eq!(result["usage"]["input_tokens"], 42);
    let mut history = request["input"].as_array().unwrap().clone();
    history.extend(result["output"].as_array().unwrap().iter().cloned());
    let round_trip = ResponsesToMessages
        .translate_request(
            "/responses",
            Bytes::from(
                json!({
                    "model":MODEL,"input":history,"max_output_tokens":8192
                })
                .to_string(),
            ),
            &upstream,
        )
        .unwrap();
    let restored: serde_json::Value = serde_json::from_slice(&round_trip.body).unwrap();
    assert_eq!(
        restored["messages"].as_array().unwrap().last().unwrap()["content"][0],
        original["content"][0]
    );

    let reverse = MessagesToResponses.translate_request("/v1/messages",Bytes::from(json!({
        "model":MODEL,"max_tokens":8192,"stream":true,
        "system":[{"type":"text","text":"System"}],
        "messages":[{"role":"user","content":"Hello"},
          {"role":"assistant","content":[{"type":"tool_use","id":"call_a","name":"lookup","input":{"name":"world"}}]},
          {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_a","content":"found"}]}],
        "tools":[{"name":"lookup","input_schema":{"type":"object"}}]
    }).to_string()),&upstream).unwrap();
    let reverse: serde_json::Value = serde_json::from_slice(&reverse.body).unwrap();
    assert_eq!(reverse["input"][1]["type"], "function_call");
    assert_eq!(reverse["input"][2]["call_id"], "call_a");
    assert_eq!(reverse["max_output_tokens"], 8192);
    for translator in [
        &ResponsesToMessages as &dyn ProtocolTranslator,
        &MessagesToResponses,
    ] {
        let description = translator.describe();
        assert!(!description.losses.preserved.is_empty());
        assert!(!description.losses.approximated.is_empty());
        assert!(!description.losses.dropped.is_empty());
    }
}

#[test]
fn translated_streams_deliver_tool_pieces_before_eof_and_never_turn_a_cut_into_success() {
    use bytes::Bytes;
    use orch_model_contract::translator::ProtocolTranslator;
    use orch_translator_responses_messages::{MessagesToResponses, ResponsesToMessages};
    let mut converter = ResponsesToMessages.response_translator();
    let mut converted = Vec::new();
    for event in [
        json!({"type":"message_start","message":{"id":"msg_a","model":MODEL,"usage":{"input_tokens":12,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_a","name":"lookup","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"name\":\"world\"}"}}),
    ] {
        // Any UTF-8 character, CRLF or event boundary may straddle transport pieces.
        let bytes = format!(
            "event: {}\r\ndata: {}\r\n\r\n",
            event["type"].as_str().unwrap(),
            event
        );
        for byte in bytes.as_bytes() {
            converted.extend(
                converter
                    .translate_chunk(Bytes::copy_from_slice(&[*byte]))
                    .unwrap(),
            );
        }
    }
    let text = String::from_utf8(converted).unwrap();
    assert!(
        text.contains("response.function_call_arguments.delta"),
        "{text}"
    );
    assert!(!text.contains("response.completed"), "{text}");
    let mut parser = orch_model_contract::usage::SseParser::new();
    let events = parser
        .push(text.as_bytes())
        .into_iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(&event.data).unwrap())
        .collect::<Vec<_>>();
    let initial = events
        .iter()
        .find(|event| event["type"] == "response.output_item.added")
        .unwrap();
    assert_eq!(
        initial["item"]["arguments"], "",
        "argument deltas must not append after an initial empty object"
    );

    assert!(
        converter.finish().is_err(),
        "an absent message_stop is a cut"
    );

    let mut reverse = MessagesToResponses.response_translator();
    let start = reverse
        .translate_chunk(Bytes::from(format!(
            "event: response.created\ndata: {}\n\n",
            json!({"type":"response.created","response":{"id":"resp_a","model":MODEL,"usage":null}})
        )))
        .unwrap();
    assert!(String::from_utf8_lossy(&start).contains("message_start"));
    let error = reverse.translate_chunk(Bytes::from("event: error\ndata: {\"type\":\"error\",\"code\":\"rate_limit_exceeded\",\"message\":\"Retry later\"}\n\n")).unwrap();
    assert!(String::from_utf8_lossy(&error).contains("rate_limit_error"));
    assert!(!String::from_utf8_lossy(&error).contains("message_stop"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builtin_translation_streams_both_directions_and_counts_the_unmodified_upstream_usage() {
    use orch_translator_responses_messages::{MessagesToResponses, ResponsesToMessages};
    for (incoming, outgoing) in [(RESPONSES, MESSAGES), (MESSAGES, RESPONSES)] {
        let mut p = ProviderRig::with(move |rig, _| {
            rig.plug = Some(Arc::new(move |builder, roles, data, key, clock| {
                let parts = ProviderParts::new(
                    protocols().with_translators(vec![
                        Arc::new(ResponsesToMessages),
                        Arc::new(MessagesToResponses),
                    ]),
                    data,
                    key,
                    clock,
                )
                .unwrap();
                plug_in(builder, roles, parts)
            }));
        })
        .await;
        let base = if outgoing == MESSAGES {
            p.mock.messages_base_url()
        } else {
            p.mock.responses_base_url()
        };
        let started = p.start_provider(&[(outgoing, base, json!({}))]).await;
        assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
        let lease = p.open_lease(p.thread, &[MODEL], Some(incoming), None).await;
        let harness = p.harness(p.thread);
        p.mock.queue("say Answer before the tool.\ntool lookup {\"name\":\"world\"}\nusage 100 20 cached=10").unwrap();
        let (path, body) = if incoming == RESPONSES {
            (
                "/v1/responses",
                json!({"model":MODEL,"input":"Hello","stream":true,"max_output_tokens":8192,
                "tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"name":{"type":"string"}}}}]}),
            )
        } else {
            (
                "/v1/messages",
                json!({"model":MODEL,"messages":[{"role":"user","content":"Hello"}],
                "stream":true,"max_tokens":8192,"tools":[{"name":"lookup","input_schema":{"type":"object","properties":{"name":{"type":"string"}}}}]}),
            )
        };
        let answer = harness.post(path, &body).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        let text = answer.text();
        let mut parser = orch_model_contract::usage::SseParser::new();
        let events = parser
            .push(text.as_bytes())
            .into_iter()
            .map(|event| serde_json::from_str::<serde_json::Value>(&event.data).unwrap())
            .collect::<Vec<_>>();
        let spoken = events
            .iter()
            .filter_map(|event| match event["type"].as_str() {
                Some("response.output_text.delta") => event["delta"].as_str(),
                Some("content_block_delta") if event["delta"]["type"] == "text_delta" => {
                    event["delta"]["text"].as_str()
                }
                _ => None,
            })
            .collect::<String>();
        assert_eq!(spoken, "Answer before the tool.", "{text}");
        assert!(
            text.contains(if incoming == RESPONSES {
                "response.function_call_arguments.delta"
            } else {
                "input_json_delta"
            }),
            "{text}"
        );
        assert!(
            text.contains(if incoming == RESPONSES {
                "response.completed"
            } else {
                "message_stop"
            }),
            "{text}"
        );
        let usage = p.wait_provider_event("lease.usage", 1).await;
        assert_eq!(usage[0].body["usage"]["input_tokens"], 100);
        assert_eq!(usage[0].body["usage"]["cached_input_tokens"], 10);
        let close = p.close_lease(lease).await;
        assert_eq!(close["requests"], 1);
        let requests = p.model_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].credential.verdict, Verdict::Accepted);
        assert_eq!(requests[0].credential.presented_by.len(), 1);
        p.rig.kill().await;
    }
}

#[test]
fn translation_keeps_output_limits_counting_compaction_and_signed_reverse_continuation_truthful() {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::{MessagesToResponses, ResponsesToMessages};
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        output_tokens: 4096,
        session: None,
        model: Some(
            serde_json::from_value(json!({"offered":{"report":{"output_max":8192}}})).unwrap(),
        ),
    };
    for (stop, status) in [
        ("end_turn", "completed"),
        ("tool_use", "completed"),
        ("max_tokens", "incomplete"),
        ("model_context_window_exceeded", "incomplete"),
        ("stop_sequence", "completed"),
        ("pause_turn", "failed"),
    ] {
        let mut converter = ResponsesToMessages.response_translator();
        let mut answer = Vec::new();
        for event in [
            json!({"type":"message_start","message":{"id":"msg_a","model":MODEL,"usage":{}}}),
            json!({"type":"message_delta","delta":{"stop_reason":stop},"usage":{"output_tokens":8}}),
            json!({"type":"message_stop"}),
        ] {
            answer.extend(
                converter
                    .translate_chunk(Bytes::from(format!("data: {event}\n\n")))
                    .unwrap(),
            );
        }
        answer.extend(converter.finish().unwrap());
        let text = String::from_utf8(answer).unwrap();
        assert!(
            text.contains(&format!("\"status\":\"{status}\"")),
            "{stop}: {text}"
        );
        assert!(text.contains(if status == "completed" {
            "event: response.completed"
        } else if status == "failed" {
            "event: response.failed"
        } else {
            "event: response.incomplete"
        }));
    }
    let count = MessagesToResponses
        .translate_request(
            "/v1/messages/count_tokens",
            Bytes::from(
                json!({
                    "model":MODEL,"messages":[{"role":"user","content":"Count without generation"}]
                })
                .to_string(),
            ),
            &upstream,
        )
        .unwrap();
    assert_eq!(count.path, "/responses/input_tokens");
    let request: serde_json::Value = serde_json::from_slice(&count.body).unwrap();
    assert!(request.get("max_output_tokens").is_none());
    let mut converter = MessagesToResponses
        .response_for("/v1/messages/count_tokens", b"{}", &upstream)
        .unwrap();
    converter
        .translate_chunk(Bytes::from_static(b"{\"input_tokens\":123}"))
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&converter.finish().unwrap()).unwrap(),
        json!({"input_tokens":123})
    );

    let reasoning = json!({"type":"reasoning","id":"rs_a","encrypted_content":"exact-opaque-vendor-state","summary":[{"type":"summary_text","text":"Summary"}]});
    let mut converter = MessagesToResponses.response_translator();
    converter.translate_chunk(Bytes::from(json!({"id":"resp_a","model":MODEL,"status":"completed",
        "output":[reasoning],"usage":{"input_tokens":50,"input_tokens_details":{"cached_tokens":10},"output_tokens":8}
    }).to_string())).unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&converter.finish().unwrap()).unwrap();
    let converted = MessagesToResponses.translate_request("/v1/messages",Bytes::from(json!({
        "model":MODEL,"max_tokens":8192,"messages":[{"role":"user","content":"Hello"},{"role":"assistant","content":answer["content"]}]
    }).to_string()),&upstream).unwrap();
    let request: serde_json::Value = serde_json::from_slice(&converted.body).unwrap();
    assert_eq!(request["input"][1], reasoning);
    assert_eq!(answer["usage"]["input_tokens"], 40);
    assert_eq!(answer["usage"]["cache_read_input_tokens"], 10);

    let compact = ResponsesToMessages
        .translate_request(
            "/responses/compact",
            Bytes::from(
                json!({
                    "model":MODEL,"input":"A conversation to retain"
                })
                .to_string(),
            ),
            &upstream,
        )
        .unwrap();
    let request: serde_json::Value = serde_json::from_slice(&compact.body).unwrap();
    assert_eq!(request["stream"], false);
    let mut converter = ResponsesToMessages
        .response_for("/responses/compact", b"{}", &upstream)
        .unwrap();
    converter.translate_chunk(Bytes::from(json!({"id":"msg_a","content":[{"type":"text","text":"Continue the saved work."}],"stop_reason":"end_turn","usage":{}}).to_string())).unwrap();
    let summary: serde_json::Value = serde_json::from_slice(&converter.finish().unwrap()).unwrap();
    assert_eq!(summary["object"], "response.compaction");
    let restored = ResponsesToMessages
        .translate_request(
            "/responses",
            Bytes::from(
                json!({
                    "model":MODEL,"input":summary["output"]
                })
                .to_string(),
            ),
            &upstream,
        )
        .unwrap();
    let request: serde_json::Value = serde_json::from_slice(&restored.body).unwrap();
    assert_eq!(
        request["messages"][0]["content"][0]["text"],
        "Continue the saved work."
    );
}

#[test]
fn a_native_compaction_trigger_returns_one_compaction_item_and_restores_its_summary_on_the_next_turn()
 {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::ResponsesToMessages;
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        model: None,
        output_tokens: 8192,
        session: None,
    };
    let request = json!({"model":MODEL,"stream":true,"input":[
        {"role":"user","content":"Remember the pending file change."},{"type":"compaction_trigger"}
    ],"tools":[{"type":"function","name":"edit","parameters":{"type":"object"}}]});
    let converted = ResponsesToMessages
        .translate_request("/responses", Bytes::from(request.to_string()), &upstream)
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&converted.body).unwrap();
    assert_eq!(body["stream"], true);
    assert!(
        body.get("tools").is_none(),
        "a compaction cannot invoke client tools"
    );
    assert_eq!(
        body["max_tokens"], 8192,
        "an unknown offering needs no caller workaround"
    );
    let mut converter = ResponsesToMessages
        .response_for("/responses", request.to_string().as_bytes(), &upstream)
        .unwrap();
    let mut output = Vec::new();
    for event in [
        json!({"type":"message_start","message":{"id":"msg_compact","model":MODEL,"usage":{"input_tokens":20}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Finish the pending file change."}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":8}}),
        json!({"type":"message_stop"}),
    ] {
        output.extend(
            converter
                .translate_chunk(Bytes::from(format!("data: {event}\n\n")))
                .unwrap(),
        );
    }
    output.extend(converter.finish().unwrap());
    let mut events = orch_model_contract::usage::SseParser::new();
    let events = events
        .push(&output)
        .into_iter()
        .map(|event| serde_json::from_str::<serde_json::Value>(&event.data).unwrap())
        .collect::<Vec<_>>();
    let items = events
        .iter()
        .filter(|event| event["type"] == "response.output_item.done")
        .collect::<Vec<_>>();
    assert_eq!(
        items.len(),
        1,
        "the pinned harness requires one compaction item"
    );
    assert_eq!(items[0]["item"]["type"], "compaction");
    assert_eq!(events.last().unwrap()["type"], "response.completed");
    let next = ResponsesToMessages.translate_request("/responses",Bytes::from(json!({
        "model":MODEL,"input":[items[0]["item"].clone(),{"role":"user","content":"Continue"}]
    }).to_string()),&upstream).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&next.body).unwrap();
    assert!(
        body["messages"][0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|block| block["text"] == "Finish the pending file change.")
    );
}

#[test]
fn changing_upstream_keeps_visible_reasoning_summaries_without_sending_foreign_opaque_state() {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::{MessagesToResponses, ResponsesToMessages};
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        model: None,
        output_tokens: 8192,
        session: None,
    };
    let converted=ResponsesToMessages.translate_request("/responses",Bytes::from(json!({
        "model":MODEL,"input":[{"role":"user","content":"Continue"},
          {"type":"reasoning","encrypted_content":"foreign-vendor-state","summary":[{"type":"summary_text","text":"Visible summary"}]}]
    }).to_string()),&upstream).unwrap();
    let text = String::from_utf8(converted.body.to_vec()).unwrap();
    assert!(text.contains("Visible summary"));
    assert!(!text.contains("foreign-vendor-state"));
    let converted=MessagesToResponses.translate_request("/v1/messages",Bytes::from(json!({
        "model":MODEL,"max_tokens":8192,"messages":[{"role":"user","content":"Continue"},
          {"role":"assistant","content":[{"type":"thinking","thinking":"Visible summary","signature":"foreign-vendor-signature"},
            {"type":"redacted_thinking","data":"foreign-redacted-state"}]}]
    }).to_string()),&upstream).unwrap();
    let text = String::from_utf8(converted.body.to_vec()).unwrap();
    assert!(text.contains("Visible summary"));
    assert!(!text.contains("foreign-vendor-signature"));
    assert!(!text.contains("foreign-redacted-state"));
}

#[test]
fn native_namespaced_client_tools_keep_their_identity_when_the_other_format_returns_them() {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::ResponsesToMessages;
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        output_tokens: 4096,
        session: None,
        model: None,
    };
    let request = json!({"model":MODEL,"input":[{"role":"user","content":"Choose the right execution tool"}],"tools":[
        {"type":"namespace","name":"workspace","tools":[{"type":"function","name":"run","parameters":{"type":"object"}}]},
        {"type":"namespace","name":"isolated","tools":[{"type":"function","name":"run","parameters":{"type":"object"}}]}
    ]});
    let sent = ResponsesToMessages
        .translate_request("/responses", Bytes::from(request.to_string()), &upstream)
        .unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
    let tools = sent["tools"]
        .as_array()
        .expect("both namespaced tools are declared");
    assert_eq!(tools.len(), 2);
    assert_ne!(tools[0]["name"], tools[1]["name"]);
    for namespace in ["isolated", "workspace"] {
        let original = json!({"model":MODEL,"tools":request["tools"],"input":[{"type":"function_call","namespace":namespace,"name":"run","call_id":"original","arguments":"{}"}]});
        let original = ResponsesToMessages
            .translate_request("/responses", Bytes::from(original.to_string()), &upstream)
            .unwrap();
        let original: serde_json::Value = serde_json::from_slice(&original.body).unwrap();
        let wire = &original["messages"][1]["content"][0]["name"];
        assert!(tools.iter().any(|tool| &tool["name"] == wire));
        let answer = json!({"id":"msg_names","model":MODEL,"type":"message","role":"assistant","stop_reason":"tool_use",
            "content":[{"type":"tool_use","id":"call_kept","name":wire,"input":{"command":"pwd"}}],"usage":{"input_tokens":1,"output_tokens":1}});
        let mut reader = ResponsesToMessages
            .response_for("/responses", request.to_string().as_bytes(), &upstream)
            .unwrap();
        let mut bytes = reader
            .translate_chunk(Bytes::from(answer.to_string()))
            .unwrap()
            .to_vec();
        bytes.extend_from_slice(&reader.finish().unwrap());
        let answer: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(answer["output"][0]["name"], "run");
        assert_eq!(answer["output"][0]["namespace"], namespace);
        assert_eq!(answer["output"][0]["call_id"], "call_kept");
        let follow = json!({"model":MODEL,"tools":request["tools"],"input":[answer["output"][0],{"type":"function_call_output","call_id":"call_kept","output":"done"}]});
        let follow = ResponsesToMessages
            .translate_request("/responses", Bytes::from(follow.to_string()), &upstream)
            .unwrap();
        let follow: serde_json::Value = serde_json::from_slice(&follow.body).unwrap();
        assert_eq!(follow["messages"][1]["content"][0]["name"], *wire);
        assert_eq!(
            follow["messages"][2]["content"][0]["tool_use_id"],
            "call_kept"
        );
    }
}

#[test]
fn native_additional_tools_and_reasoning_updates_survive_without_inventing_a_tool_result_for_a_notification()
 {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::ResponsesToMessages;
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        output_tokens: 4096,
        session: None,
        model: Some(
            serde_json::from_value(json!({"efforts":["high"],"reasoning_modes":["adaptive"]}))
                .unwrap(),
        ),
    };
    let request = json!({"model":MODEL,"input":[
        {"type":"additional_tools","role":"developer","tools":[{"type":"namespace","name":"functions","tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]}]},
        {"type":"configuration_update","reasoning":{"effort":"high"}},
        {"type":"function_call_output","name":"notifications","namespace":"chat","output":"A person mentioned the current work."},
        {"role":"user","content":"Continue with these facts"}
    ]});
    let sent = ResponsesToMessages
        .translate_request("/responses", Bytes::from(request.to_string()), &upstream)
        .unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
    assert_eq!(sent["tools"][0]["name"], "lookup");
    assert_eq!(sent["output_config"]["effort"], "high");
    assert!(
        sent["messages"]
            .to_string()
            .contains("A person mentioned the current work.")
    );
    assert!(sent["messages"].to_string().contains("chat.notifications"));
    assert!(
        !sent["messages"]
            .to_string()
            .contains("\"tool_use_id\":null")
    );
}

#[test]
fn client_tool_search_and_another_agents_message_remain_part_of_a_translated_conversation() {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::ResponsesToMessages;
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        output_tokens: 4096,
        session: None,
        model: None,
    };
    let request = json!({"model":MODEL,"tools":[{"type":"tool_search","execution":"client","parameters":{"type":"object","properties":{"query":{"type":"string"}}}}],"input":[
        {"type":"tool_search_call","call_id":"search_one","execution":"client","status":"completed","arguments":{"query":"read a file"}},
        {"type":"tool_search_output","call_id":"search_one","execution":"client","status":"completed","tools":[{"type":"function","name":"read_file","parameters":{"type":"object"}}]},
        {"type":"agent_message","author":"worker","recipient":"supervisor","content":[{"type":"input_text","text":"The requested file is ready."}]}
    ]});
    let sent = ResponsesToMessages
        .translate_request("/responses", Bytes::from(request.to_string()), &upstream)
        .unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
    let tools = sent["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "read_file"));
    assert!(tools.iter().any(|tool| tool["name"] == "tool_search"));
    let messages = sent["messages"].to_string();
    assert!(
        messages.contains("search_one")
            && messages.contains("The requested file is ready.")
            && messages.contains("worker")
            && messages.contains("supervisor")
    );
    let mut reader = ResponsesToMessages
        .response_for("/responses", request.to_string().as_bytes(), &upstream)
        .unwrap();
    let answer = json!({"id":"msg_search","model":MODEL,"type":"message","role":"assistant","stop_reason":"tool_use",
        "content":[{"type":"tool_use","id":"search_two","name":"tool_search","input":{"query":"find a definition"}}],"usage":{"input_tokens":1,"output_tokens":1}});
    let mut bytes = reader
        .translate_chunk(Bytes::from(answer.to_string()))
        .unwrap()
        .to_vec();
    bytes.extend_from_slice(&reader.finish().unwrap());
    let answer: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(answer["output"][0]["type"], "tool_search_call");
    assert_eq!(answer["output"][0]["execution"], "client");
    assert_eq!(answer["output"][0]["call_id"], "search_two");
    assert_eq!(
        answer["output"][0]["arguments"]["query"],
        "find a definition"
    );
}

#[test]
fn an_invalid_output_allowance_is_refused_instead_of_spending_the_translated_default() {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::ResponsesToMessages;
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        output_tokens: 4096,
        session: None,
        model: None,
    };
    for limit in [json!(0), json!(-1), json!(1.5), json!("32")] {
        let request =
            json!({"model":MODEL,"input":"Keep the explicit allowance","max_output_tokens":limit});
        assert!(
            ResponsesToMessages
                .translate_request("/responses", Bytes::from(request.to_string()), &upstream)
                .is_err()
        );
    }
}

#[test]
fn an_optional_translated_cache_breakpoint_does_not_block_a_model_that_disallows_cache_options() {
    use bytes::Bytes;
    use orch_model_contract::translator::{ProtocolTranslator, UpstreamSide};
    use orch_translator_responses_messages::ResponsesToMessages;
    let upstream = UpstreamSide {
        base_url: "https://controlled.invalid".into(),
        options: json!({}),
        credential: None,
        output_tokens: 4096,
        session: None,
        model: Some(
            serde_json::from_value(
                json!({"request_options":{"common":{"properties":{"cache":false}}}}),
            )
            .unwrap(),
        ),
    };
    let request =
        json!({"model":MODEL,"instructions":"Keep the task","input":"The current question"});
    let translated = ResponsesToMessages
        .translate_request("/responses", Bytes::from(request.to_string()), &upstream)
        .unwrap();
    let translated: serde_json::Value = serde_json::from_slice(&translated.body).unwrap();
    assert!(!translated.to_string().contains("cache_control"));
    assert!(translated.to_string().contains("Keep the task"));
    assert!(translated.to_string().contains("The current question"));
}
