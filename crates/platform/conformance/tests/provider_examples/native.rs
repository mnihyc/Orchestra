//! Actual Python subscription programs agree with the built-in converters on recorded native answers.

use super::{HOST_VERSION, PYTHON, ask, entry, example};
use orch_agent_contract::facts::ProviderFacts;
use orch_conformance::provider::{self, Example, Fixture, Sources};
use orch_conformance::report::Report;
use orch_model_contract::{
    credentials::Credential,
    endpoint::EndpointOptions,
    protocol::{SecretHeaders, WireProtocol},
};
use orch_plugin::{Options, Plugin, bridges::provider::ProviderBridge};
use orch_plugin_contract::PluginKind;
use orch_provider_contract::{FactsSource, ModelsSource};
use orch_types::Json;
use serde_json::json;
use std::path::{Path, PathBuf};

const AT: &str = "2026-10-13T09:00:00.000Z";

fn recorded(path: &str) -> Json {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    serde_json::from_slice(&std::fs::read(root.join(path)).unwrap()).unwrap()
}

async fn start(directory: &Path, script: &str, tape: &Json) -> (Plugin, ProviderBridge) {
    let tape_path = directory.join("tape.json");
    std::fs::write(&tape_path, serde_json::to_vec(tape).unwrap()).unwrap();
    let native = json!([
        PYTHON,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/provider_examples/recorded_native.py"),
        "--tape",
        tape_path,
        "--state",
        directory.join("reset.json")
    ])
    .to_string();
    let entry = entry(
        PluginKind::Provider,
        vec![
            PYTHON.into(),
            example("provider", script),
            "--harness-command".into(),
            native,
            "--at".into(),
            AT.into(),
        ],
    );
    let plugin = Plugin::start(&entry, Options::new(HOST_VERSION))
        .await
        .unwrap();
    plugin.settled().await;
    let bridge = ProviderBridge::new(plugin.clone()).unwrap();
    (plugin, bridge)
}

fn project(facts: &ProviderFacts, members: &[&str]) -> Json {
    let mut report = orch_connector_core::sources::adapter::report(facts)
        .unwrap()
        .0;
    report
        .as_object_mut()
        .unwrap()
        .retain(|name, _| members.contains(&name.as_str()));
    report
}

async fn endpoint_matches_the_plain_protocol(
    plugin: &Plugin,
    bridge: &ProviderBridge,
    codex: bool,
) {
    let mut request = ask("normal");
    request.endpoints.clear();
    let description = plugin
        .call(
            "x-endpoint.describe",
            serde_json::to_value(&request).unwrap(),
        )
        .await
        .unwrap();
    let protocol: Box<dyn WireProtocol> = if codex {
        Box::new(orch_protocol_openai_responses::OpenAiResponses)
    } else {
        Box::new(orch_protocol_anthropic_messages::AnthropicMessages)
    };
    assert_eq!(description["endpoint"]["protocol"], protocol.name());
    assert_eq!(
        description["endpoint"]["base_url"],
        if codex {
            "https://chatgpt.com/backend-api/codex"
        } else {
            "https://api.anthropic.com"
        }
    );
    let options = EndpointOptions::parse(&description["endpoint"]["options"]).unwrap();
    let mut headers = http::HeaderMap::new();
    protocol
        .authorize(
            &mut headers,
            Some(&Credential::new("disposable-native-access")),
            &options,
            &SecretHeaders::new(),
        )
        .unwrap();
    assert_eq!(headers["authorization"], "Bearer disposable-native-access");
    assert_eq!(
        description["presents"]["header"],
        options.key_header(protocol.default_auth()).unwrap()
    );
    assert_eq!(description["presents"]["scheme"], "Bearer");
    assert_eq!(
        description["models"],
        serde_json::to_value(bridge.list_models(request).await.unwrap()).unwrap()["models"]
    );
    assert!(
        description["request"]["preserve_harness_headers"]
            .as_bool()
            .unwrap()
    );
    if !codex {
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert_eq!(headers["anthropic-beta"], "oauth-2025-04-20");
    }
    assert!(!description.to_string().contains("disposable-native-access"));
    assert!(
        bridge.declared().presents.is_none(),
        "a sign-in never becomes a plugin-presented key"
    );
}

#[tokio::test]
async fn the_codex_program_reports_the_builtin_facts_models_and_idempotent_native_reset() {
    use orch_adapter_codex::provider::{self as native, LoginTracker};
    let directory = tempfile::tempdir().unwrap();
    // Shapes from codex provider::the_limits_of_a_plan... and tests/plan_facts.rs's reset fixture.
    let snapshot = json!({"planType":"pro","primary":{"usedPercent":37.5,"windowDurationMins":300,"resetsAt":1791730800},
        "secondary":{"usedPercent":12,"windowDurationMins":10080,"resetsAt":null},
        "credits":{"hasCredits":true,"unlimited":false,"balance":"1250"},"rateLimitReachedType":null});
    let resets = json!({"availableCount":1,"credits":[{"id":"credit-1","resetType":"codexRateLimits","status":"available", "title":"Full reset","grantedAt":1791849600,"expiresAt":1794441600}]});
    let model = json!({"id":"gpt-recorded","model":"gpt-recorded","displayName":"Recorded model","inputModalities":["text","image"],
        "supportedReasoningEfforts":[{"reasoningEffort":"high"}],"defaultReasoningEffort":"high"});
    let catalogue = json!({"models":[{"slug":"gpt-recorded","context_window":272000,"max_context_window":1000000}]});
    let tape = json!({"account":{"account":{"type":"chatgpt","planType":"pro"}},
        "rates":{"rateLimits":snapshot,"rateLimitResetCredits":resets,"ordinaryUsageAllowed":false},
        "models":{"data":[model],"nextCursor":null},"catalogue":catalogue});
    let (plugin, bridge) = start(directory.path(), "codex_plan.py", &tape).await;
    let mut expected =
        native::facts_from_snapshot(&snapshot, &LoginTracker::default(), AT.parse().unwrap());
    expected.available_resets = native::resets_from(&resets);
    expected.ordinary_usage_allowed = Some(false);
    expected
        .field_times
        .insert("limit_reached".into(), AT.parse().unwrap());
    let members = [
        "plan",
        "windows",
        "balances",
        "limit_reached",
        "ordinary_usage_allowed",
        "available_resets",
    ];
    let expected = project(&expected, &members);
    assert_eq!(bridge.read_facts(ask("normal")).await.unwrap().0, expected);
    let mut models = vec![native::suggestion_of(&model).unwrap()];
    native::add_windows(&mut models, &native::windows_of_catalogue(&catalogue));
    assert_eq!(
        serde_json::to_value(bridge.list_models(ask("normal")).await.unwrap()).unwrap(),
        json!({"models":models})
    );
    let mut report = Report::new("actual Python Codex source, recorded native answers");
    let fixture = Fixture {
        facts: vec![Example {
            ask: ask("normal"),
            expected,
        }],
        models: Vec::new(),
        reset: None,
        present: None,
        refusal: None,
        now: AT.parse().unwrap(),
        secrets: Vec::new(),
    };
    let descriptor = bridge.declared();
    provider::run(
        Sources {
            descriptor: &descriptor,
            facts: Some(&bridge),
            models: Some(&bridge),
            presenter: None,
        },
        Some(&fixture),
        &mut report,
    )
    .await;
    assert!(report.passed(), "{report}");
    endpoint_matches_the_plain_protocol(&plugin, &bridge, true).await;
    let reset = orch_provider_contract::source::ResetRequest {
        reset_id: "credit-1".into(),
        idempotency_key: "recorded-reset-once".into(),
    };
    let first = bridge
        .spend_reset(ask("normal"), reset.clone())
        .await
        .unwrap();
    let replay = bridge
        .spend_reset(ask("normal"), reset.clone())
        .await
        .unwrap();
    assert_eq!(first, replay);
    plugin.shutdown().await;
    let (restarted, bridge) = start(directory.path(), "codex_plan.py", &tape).await;
    assert_eq!(
        bridge.spend_reset(ask("normal"), reset).await.unwrap(),
        first
    );
    let spent: Json =
        serde_json::from_slice(&std::fs::read(directory.path().join("reset.json")).unwrap())
            .unwrap();
    assert_eq!(spent["spent"], 1);
    restarted.shutdown().await;

    let directory = tempfile::tempdir().unwrap();
    let update = json!({"method":"account/rateLimits/updated","params":{"rateLimits":snapshot}});
    let (plugin, bridge) = start(
        directory.path(),
        "codex_plan.py",
        &json!({"account":tape["account"],"rates":{},"notifications":[update]}),
    )
    .await;
    let updated = native::facts_from_update(
        "account/rateLimits/updated",
        &update["params"],
        &LoginTracker::default(),
        AT.parse().unwrap(),
    )
    .unwrap();
    assert_eq!(
        bridge.read_facts(ask("normal")).await.unwrap().0,
        project(&updated, &members)
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn the_claude_program_matches_the_native_usage_models_and_recorded_rate_limit_frames() {
    use orch_adapter_claude_code::provider as native;
    // Explicit protocol examples are independent of any account or captured vendor reply.
    let at: orch_types::Time = AT.parse().unwrap();
    let short_reset = orch_types::Time::new(at.get() + std::time::Duration::from_secs(5 * 60 * 60));
    let weekly_reset =
        orch_types::Time::new(at.get() + std::time::Duration::from_secs(7 * 24 * 60 * 60));
    let limit = 5000.0;
    let used = 123.0;
    let usage = json!({"subscription_type":"max", "rate_limits_available":true, "rate_limits":{
        "five_hour":{"utilization":25.0,"resets_at":short_reset},
        "seven_day":{"utilization":79.0,"resets_at":weekly_reset},
        "seven_day_opus":null,"seven_day_sonnet":null,"seven_day_oauth_apps":null,
        "model_scoped":[{"display_name":"Fable","utilization":12.0,"resets_at":weekly_reset}],
        "extra_usage":{"is_enabled":false,"monthly_limit":limit,"used_credits":used,
            "utilization":used / limit * 100.0,"currency":"USD"}
    }});
    let models = json!({"models":[{"value":"claude-recorded","displayName":"Recorded","supportedEffortLevels":["high"],"supportsFastMode":true},
        {"value":"default","resolvedModel":"claude-recorded","displayName":"Default (recommended)"}]});
    let directory = tempfile::tempdir().unwrap();
    let tape = json!({"initialize":models,"usage":usage});
    let (plugin, bridge) = start(directory.path(), "claude_plan.py", &tape).await;
    let expected = project(
        &native::usage_answer(&usage, AT.parse().unwrap()).unwrap(),
        &["plan", "windows", "extra_usage", "limit_reached"],
    );
    assert_eq!(bridge.read_facts(ask("normal")).await.unwrap().0, expected);
    assert_eq!(
        serde_json::to_value(bridge.list_models(ask("normal")).await.unwrap()).unwrap(),
        json!({"models":native::suggestions_from(&models)})
    );
    endpoint_matches_the_plain_protocol(&plugin, &bridge, false).await;
    plugin.shutdown().await;

    let recording = recorded(
        "extensions/agents/claude-code/tests/fixtures/failures/plan_limit_weekly_in_new_york.json",
    );
    let frame = recording["frames"]
        .as_array()
        .or_else(|| recording["events"].as_array())
        .expect("the adapter's recording has its native lines")
        .iter()
        .find(|frame| frame["type"] == "rate_limit_event")
        .unwrap()
        .clone();
    let directory = tempfile::tempdir().unwrap();
    let tape = json!({"initialize":models,"usage":{},"notifications":[frame]});
    let (plugin, bridge) = start(directory.path(), "claude_plan.py", &tape).await;
    assert_eq!(
        bridge.read_facts(ask("normal")).await.unwrap().0,
        project(
            &native::rate_limit_facts(&frame).unwrap(),
            &["windows", "extra_usage", "limit_reached"]
        )
    );
    plugin.shutdown().await;

    let directory = tempfile::tempdir().unwrap();
    let (plugin, bridge) = start(
        directory.path(),
        "claude_plan.py",
        &json!({"initialize":models,"usage_error":true}),
    )
    .await;
    assert_eq!(
        bridge.read_facts(ask("normal")).await.unwrap().0,
        json!({}),
        "unsupported get_usage is not invented empty/zero usage"
    );
    plugin.shutdown().await;
}
