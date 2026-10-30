//! The suites run against real programs: the example plugins of the repository pass them, and a program that breaks one rule fails that rule by its name.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use bytes::Bytes;
use orch_conformance::report::{Outcome, Report};
use orch_conformance::{driver, summarizer, translator, transport};
use orch_driver_contract::runtime::RuntimeSpec;
use orch_model_contract::credentials::Credential;
use orch_model_contract::protocol::WireProtocol;
use orch_model_contract::translator::{
    ProtocolTranslator, TranslateError, TranslatedRequest, UpstreamSide,
};
use orch_plugin::bridges::driver::DriverBridge;
use orch_plugin::bridges::summarizer::SummarizerBridge;
use orch_plugin::bridges::translator::TranslatorBridge;
use orch_plugin::{Options, Plugin};
use orch_plugin_contract::registration::{PluginKind, PluginRegistration, PluginStart, Restart};
use serde_json::json;

mod provider_examples;

/// The python of this machine, which the examples are written for.
const PYTHON: &str = "python3";

/// The version a host says it is in `hello`.
const HOST_VERSION: &str = "0.1.0";

fn example(directory: &str, file: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/plugins")
        .join(directory)
        .join(file)
        .to_string_lossy()
        .into_owned()
}

fn entry(kind: PluginKind, command: Vec<String>) -> PluginRegistration {
    PluginRegistration {
        kind,
        name: "checked".to_owned(),
        command: Some(command),
        connect: None,
        env: BTreeMap::new(),
        restart: Some(Restart::Never),
    }
}

fn start_of(entry: &PluginRegistration) -> PluginStart {
    entry.start().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_example_summarizer_says_what_it_is_and_passes_the_transport_and_its_kinds_suite() {
    let entry = entry(
        PluginKind::Summarizer,
        vec![PYTHON.to_owned(), example("summarizer", "summarizer.py")],
    );
    let start = start_of(&entry);
    let hello = transport::discover(&start, HOST_VERSION).await.unwrap();
    assert_eq!(
        hello.kind,
        PluginKind::Summarizer,
        "its kind, whatever it was offered as"
    );
    assert_eq!(hello.name, "rules");

    let mut report = Report::new("summarizer \"rules\"");
    let negotiated =
        transport::run(&start, PluginKind::Summarizer, HOST_VERSION, &mut report).await;
    assert!(negotiated.is_some(), "{report}");

    let plugin = Plugin::start(&entry, Options::new(HOST_VERSION))
        .await
        .unwrap();
    plugin.settled().await;
    let bridge = SummarizerBridge::new(plugin.clone()).unwrap();
    summarizer::run(&bridge, &json!({ "prefix": "Now: " }), &mut report).await;
    plugin.shutdown().await;

    assert!(report.passed(), "{report}");
    assert_eq!(report.outcome("transport.shutdown"), Some(&Outcome::Passed));
    assert_eq!(
        report.outcome("summarizer.undeclared"),
        Some(&Outcome::Passed),
        "the example declares title, description and tags, and refuses an undeclared field"
    );
    for field in ["rollup", "handoff"] {
        assert!(
            matches!(
                report.outcome(&format!("summarizer.{field}")),
                Some(Outcome::Skipped(_))
            ),
            "a field outside the example's declaration is not advertised as supported"
        );
    }
}

// These are the two independently supplied wire formats, not the example's conversion helpers.
const RESPONSES_TEXT_STREAM: &str = concat!(
    ": heartbeat\r\n\r\nevent: response.created\r\n",
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_demo\",\"model\":\"text-model\",\"usage\":null}}\r\n\r\n",
    "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hé\"}\n\n",
    "event: response.reasoning_text.delta\ndata: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"private thinking\"}\n\n",
    "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\n\n",
    "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_demo\",\"model\":\"text-model\",\"status\":\"completed\",",
    "\"usage\":{\"input_tokens\":9,\"input_tokens_details\":{\"cached_tokens\":3},\"output_tokens\":4,\"output_tokens_details\":{\"reasoning_tokens\":2}}}}\n\n",
    "data: [DONE]\n\n",
);
const MESSAGES_TEXT_STREAM: &str = concat!(
    "event: message_start\ndata: {\"message\":{\"content\":[],\"id\":\"resp_demo\",\"model\":\"text-model\",\"role\":\"assistant\",\"stop_reason\":null,\"stop_sequence\":null,\"type\":\"message\",\"usage\":{\"cache_read_input_tokens\":0,\"input_tokens\":0,\"output_tokens\":0}},\"type\":\"message_start\"}\n\n",
    "event: content_block_start\ndata: {\"content_block\":{\"text\":\"\",\"type\":\"text\"},\"index\":0,\"type\":\"content_block_start\"}\n\n",
    "event: content_block_delta\ndata: {\"delta\":{\"text\":\"Hé\",\"type\":\"text_delta\"},\"index\":0,\"type\":\"content_block_delta\"}\n\n",
    "event: content_block_delta\ndata: {\"delta\":{\"text\":\"你好\",\"type\":\"text_delta\"},\"index\":0,\"type\":\"content_block_delta\"}\n\n",
    "event: content_block_stop\ndata: {\"index\":0,\"type\":\"content_block_stop\"}\n\n",
    "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"type\":\"message_delta\",\"usage\":{\"cache_read_input_tokens\":3,\"input_tokens\":6,\"output_tokens\":4}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

fn text_translation() -> translator::Fixture {
    translator::Fixture{
        path:"/messages".into(),
        body:json!({"model":"text-model","system":[{"type":"text","text":"Be brief. ","cache_control":{"type":"ephemeral"}},
                {"type":"text","text":"Plain text."}],
            "messages":[{"role":"user","content":"Hello"},
                {"role":"assistant","content":[{"type":"thinking","thinking":"private thinking"},{"type":"text","text":"Earlier."}]},
                {"role":"user","content":[{"type":"text","text":"你好"},{"type":"image","source":{"type":"base64","data":"not-an-image"}}]}],
            "tools":[{"name":"discarded","input_schema":{"type":"object"}}],"thinking":{"type":"enabled","budget_tokens":16},
            "max_tokens":32,"temperature":0.25,"stream":true}).to_string(),
        upstream:UpstreamSide{base_url:"https://upstream.example/v1".into(),options:json!({}),
            credential:Some(Credential::new("unused-fixture-key"))},
        request:TranslatedRequest{method:"POST".into(),path:"/responses".into(),query:BTreeMap::new(),
            headers:BTreeMap::from([("content-type".into(),"application/json".into())]),authorized:false,
            body:Bytes::from(r#"{"input":[{"content":"Hello","role":"user"},{"content":"Earlier.","role":"assistant"},{"content":"你好","role":"user"}],"instructions":"Be brief. Plain text.","max_output_tokens":32,"model":"text-model","store":false,"stream":true,"temperature":0.25}"#)},
        response:RESPONSES_TEXT_STREAM.into(),translated_response:MESSAGES_TEXT_STREAM.into(),
        implemented:vec!["openai-responses".into()],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_example_translator_passes_the_transport_and_exact_text_stream_suite_and_declares_its_losses()
 {
    let entry = entry(
        PluginKind::Translator,
        vec![PYTHON.to_owned(), example("translator", "translator.py")],
    );
    let start = start_of(&entry);
    let hello = transport::discover(&start, HOST_VERSION).await.unwrap();
    assert_eq!(hello.kind, PluginKind::Translator);
    assert_eq!(hello.name, "messages-text");
    let mut report = Report::new("translator messages-text");
    assert!(
        transport::run(&start, PluginKind::Translator, HOST_VERSION, &mut report)
            .await
            .is_some(),
        "{report}"
    );
    let plugin = Plugin::start(&entry, Options::new(HOST_VERSION))
        .await
        .unwrap();
    plugin.settled().await;
    let bridge = TranslatorBridge::new(plugin.clone()).unwrap();
    let descriptor = bridge.describe();
    let losses = descriptor.losses.dropped.join(" ").to_lowercase();
    for loss in ["tool", "image", "reasoning", "thinking", "cache control"] {
        assert!(losses.contains(loss), "a person must see {loss}");
    }
    assert!(
        descriptor
            .losses
            .approximated
            .iter()
            .any(|line| line.contains("Stop reasons"))
    );
    let fixture = text_translation();
    translator::run(&bridge, Some(&fixture), &mut report);
    // The request's stream flag is preserved; a false flag has a plain text JSON answer too.
    let mut plain = fixture.clone();
    let mut body: serde_json::Value = serde_json::from_str(&plain.body).unwrap();
    body["stream"] = json!(false);
    plain.body = body.to_string();
    plain.request.body = Bytes::from(
        std::str::from_utf8(&plain.request.body)
            .unwrap()
            .replace("\"stream\":true", "\"stream\":false"),
    );
    plain.response=r#"{"id":"resp_demo","model":"text-model","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"reasoning","summary":[]},{"type":"message","content":[{"type":"output_text","text":"Hé你好"}]}],"usage":{"input_tokens":9,"input_tokens_details":{"cached_tokens":3},"output_tokens":4}}"#.into();
    plain.translated_response=r#"{"content":[{"text":"Hé你好","type":"text"}],"id":"resp_demo","model":"text-model","role":"assistant","stop_reason":"max_tokens","stop_sequence":null,"type":"message","usage":{"cache_read_input_tokens":3,"input_tokens":6,"output_tokens":4}}"#.into();
    let mut plain_report = Report::new("translator messages-text, plain answer");
    translator::run(&bridge, Some(&plain), &mut plain_report);
    assert!(plain_report.passed(), "{plain_report}");
    // Observe the actual converted stream with the existing Messages reader. Cached input is
    // disjoint on that wire; an inclusive Responses count must not be counted a second time.
    let mut converted = bridge.response_translator();
    let mut observer = orch_protocol_anthropic_messages::AnthropicMessages.usage_observer();
    observer.observe(
        &converted
            .translate_chunk(Bytes::from_static(RESPONSES_TEXT_STREAM.as_bytes()))
            .unwrap(),
    );
    observer.observe(&converted.finish().unwrap());
    let usage = observer.usage().unwrap().tokens;
    assert_eq!(usage.input_tokens, 9);
    assert_eq!(usage.cached_input_tokens, 3);
    assert_eq!(usage.output_tokens, 4);
    assert_eq!(usage.total_tokens, 13);
    // No final event means refusal, not a forged successful ending with zero usage.
    let mut cut = bridge.response_translator();
    cut.translate_chunk(Bytes::from_static(b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"cut\",\"model\":\"text-model\"}}\n\n")).unwrap();
    assert!(matches!(cut.finish(), Err(TranslateError::Malformed(_))));
    // A converter finished with a refusal is freed, and that exact id cannot be reused.
    let converter = plugin.call("response_translator", json!({})).await.unwrap();
    let params = json!({"converter":converter["converter"]});
    assert!(
        plugin
            .call("finish_response", params.clone())
            .await
            .is_err()
    );
    let refused = plugin.call("finish_response", params).await.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("unknown or finished response converter")
    );
    plugin.shutdown().await;
    assert!(report.passed(), "{report}");
    for rule in [
        "transport.shutdown",
        "translator.descriptor",
        "translator.request",
        "translator.credential",
        "translator.response",
    ] {
        assert_eq!(report.outcome(rule), Some(&Outcome::Passed), "{report}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_example_notifier_keeps_every_rule_of_the_transport() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("notices.log");
    let entry = entry(
        PluginKind::Notifier,
        vec![
            PYTHON.to_owned(),
            example("notifier", "notifier.py"),
            "--file".to_owned(),
            file.to_string_lossy().into_owned(),
        ],
    );
    let mut report = Report::new("notifier \"file\"");
    transport::run(
        &start_of(&entry),
        PluginKind::Notifier,
        HOST_VERSION,
        &mut report,
    )
    .await;
    assert!(report.passed(), "{report}");
    assert_eq!(
        report.cases.len(),
        9,
        "every rule of the transport ran: {report}"
    );
}

/// A program that answers every request with a result, also a method that no contract has.
const CARELESS: &str = r#"
import json, sys
for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue
    request = json.loads(raw)
    if "id" not in request:
        if request.get("method") == "shutdown":
            break
        continue
    if request["method"] == "hello":
        result = {"kind": "summarizer", "transport": 1, "contract": 1, "name": "careless", "version": "0.0.1",
                  "methods": ["describe", "summarize"],
                  "descriptor": {"name": "careless", "title": "Careless", "config_schema": {},
                                 "fields": ["description"]}}
    else:
        result = {}
    sys.stdout.write(json.dumps({"id": request["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_answers_a_method_no_contract_has_fails_that_rule_and_only_that_one() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("careless.py");
    std::fs::write(&script, CARELESS).unwrap();
    let entry = entry(
        PluginKind::Summarizer,
        vec![PYTHON.to_owned(), script.to_string_lossy().into_owned()],
    );
    let mut report = Report::new("summarizer \"careless\"");
    transport::run(
        &start_of(&entry),
        PluginKind::Summarizer,
        HOST_VERSION,
        &mut report,
    )
    .await;
    assert!(!report.passed());
    assert_eq!(
        report
            .failures()
            .map(|case| case.rule.as_str())
            .collect::<Vec<_>>(),
        ["transport.unknown_method"],
        "{report}"
    );
    assert!(report.to_string().contains("as if it knew it"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_is_not_there_fails_the_handshake_and_nothing_else_is_asked() {
    let entry = entry(
        PluginKind::Driver,
        vec!["/nonexistent/orch-plugin-that-is-not-there".to_owned()],
    );
    let mut report = Report::new("driver \"missing\"");
    let negotiated = transport::run(
        &start_of(&entry),
        PluginKind::Driver,
        HOST_VERSION,
        &mut report,
    )
    .await;
    assert!(negotiated.is_none());
    assert!(!report.passed());
    assert!(matches!(
        report.outcome("transport.hello"),
        Some(Outcome::Failed(_))
    ));
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn the_process_driver_passes_the_node_bridge_and_a_wrong_runtime_names_the_broken_rule() {
    for wrong in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut command = vec![
            PYTHON.to_owned(),
            example("driver", "driver.py"),
            "--base".into(),
            directory.path().to_string_lossy().into_owned(),
        ];
        if wrong {
            command.push("--wrong-reply".into());
        }
        let entry = entry(PluginKind::Driver, command);
        let mut report = Report::new("driver processes");
        transport::run(
            &start_of(&entry),
            PluginKind::Driver,
            HOST_VERSION,
            &mut report,
        )
        .await;
        let plugin = Plugin::start(&entry, Options::new(HOST_VERSION))
            .await
            .unwrap();
        plugin.settled().await;
        let bridge = DriverBridge::new(plugin.clone()).unwrap();
        let base: RuntimeSpec = serde_json::from_value(json!({
            "runtime_id": orch_types::new_id(), "kind":"environment", "name":"base", "driver":"processes",
            "spec":{"command":["sleep","600"]}, "resources":{}, "roles":["environment"],
            "gateway_url":"wss://127.0.0.1:41000", "enroll_token":"test", "ca_cert":"", "tools":[],
            "volumes":{"workspace":{"name":"workspace","storage":null}, "home":{"name":"home","storage":null},
                "state":{"name":"state","storage":null}}, "mounts":[], "env":{}, "labels":{}
        })).unwrap();
        driver::run(&bridge, &base, "conformance-example", &mut report).await;
        plugin.shutdown().await;
        assert_eq!(report.passed(), !wrong, "{report}");
        if wrong {
            assert!(
                matches!(report.outcome("driver.create"), Some(Outcome::Failed(_))),
                "{report}"
            );
        }
        assert_eq!(
            std::fs::read_dir(directory.path().join("volumes"))
                .unwrap()
                .count(),
            0,
            "the suite cleans up only its own volumes"
        );
        assert_eq!(
            std::fs::read_dir(directory.path().join("units"))
                .unwrap()
                .count(),
            0,
            "the suite leaves no child process or unit behind"
        );
    }
}
