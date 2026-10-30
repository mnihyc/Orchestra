//! Real Acme provider programs run the same trait suite as an address and deliberately broken sources.

use super::{HOST_VERSION, PYTHON, entry, example, start_of};
use orch_conformance::provider::{
    self, Example, Fixture, PresentExample, RefusalExample, ResetExample, Sources,
};
use orch_conformance::report::{Outcome, Report};
use orch_conformance::transport;
use orch_plugin::bridges::provider::ProviderBridge;
use orch_plugin::{Options, Plugin};
use orch_plugin_contract::PluginKind;
use orch_provider_contract::source::{FactsSource, ModelsSource, PresentAsk, SourceAsk};
use orch_types::Json;
use serde_json::json;

mod address;
mod native;
mod openrouter;

// The example's clock is an input, so both the 5-hour and calendar-month resets are reproducible.
const AT: &str = "2026-10-15T00:00:00Z";
const CANARY: &str = "provider-suite-secret-that-must-not-appear";

fn ask(mode: &str) -> SourceAsk {
    serde_json::from_value(
        json!({"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Disposable Acme"},
        "endpoints":[{"protocol":"openai-responses","base_url":"https://acme.invalid/v1"}],
        "config":{"mode":mode},"secrets":{"demo_secret":CANARY},"credential":null}),
    )
    .unwrap()
}

fn present(key: &str) -> PresentAsk {
    serde_json::from_value(
        json!({"credential":{"key":key},"secrets":{"demo_secret":CANARY},
        "provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Disposable Acme"},
        "endpoint":{"protocol":"openai-responses","base_url":"https://acme.invalid/v1"}}),
    )
    .unwrap()
}

fn facts(spent: bool, limited: bool) -> Json {
    json!({"plan":"acme","windows":[
        {"id":"five-hour","label":"5 hours","duration_minutes":300,"used_percent":if limited {100.0} else {35.0},
            "resets_at":"2026-10-15T05:00:00.000Z","is_active":true},
        {"id":"weekly","label":"Weekly","duration_minutes":10080,"used_percent":62.5,
            "resets_at":"2026-10-22T00:00:00.000Z","is_active":false}],
        "limit_reached":if limited {json!({"type":"five-hour","resets_at":"2026-10-15T05:00:00.000Z"})} else {Json::Null},
        "ordinary_usage_allowed":!limited,
        "balances":[{"id":"account","label":"Acme balance","unit":"USD","remaining":"12.50","total":"20.00","used":"7.50","unlimited":false,
            "parts":[{"label":"Granted","amount":"2.50"},{"label":"Topped up","amount":"10.00"}]}],
        "spend":[{"id":"month","label":"Monthly spend","unit":"USD","period":"month","used":"7.50","limit":"50.00","remaining":"42.50",
            "resets_at":"2026-11-01T00:00:00.000Z"}],
        "available_resets":{"available_count":if spent {0} else {1},"credits":[{"id":"acme-reset","title":"One example reset",
            "description":"Starts the five-hour window again.","reset_type":"five-hour","status":if spent {"redeemed"} else {"available"},
            "granted_at":"2026-10-15T00:00:00.000Z","expires_at":null}]}})
}

fn fixture(presenter: bool) -> Fixture {
    Fixture {
        facts: vec![
            Example {
                ask: ask("normal"),
                expected: facts(false, false),
            },
            Example {
                ask: ask("absent"),
                expected: json!({}),
            },
            Example {
                ask: ask("null"),
                expected: json!({"plan":null,"windows":null,"limit_reached":null,"ordinary_usage_allowed":null,
                "balances":null,"spend":null,"available_resets":null}),
            },
            Example {
                ask: ask("empty"),
                expected: json!({"plan":null,"windows":[],"limit_reached":null,"ordinary_usage_allowed":null,
                "balances":[],"spend":[],"available_resets":null}),
            },
        ],
        models: vec![
            Example {
                ask: ask("null"),
                expected: json!({"models":null}),
            },
            Example {
                ask: ask("empty"),
                expected: json!({"models":[]}),
            },
            Example {
                ask: ask("normal"),
                expected: json!({"models":[
                {"model_name":"acme-small","display_name":"Acme Small","parameters":{"context_window":32000},"listed":true},
                {"model_name":"acme-large","display_name":"Acme Large","parameters":{"context_window":128000},"listed":true}]}),
            },
        ],
        reset: Some(ResetExample {
            ask: ask("normal"),
            reset: serde_json::from_value(
                json!({"reset_id":"acme-reset","idempotency_key":"suite-one-reset"}),
            )
            .unwrap(),
        }),
        present: presenter.then(|| PresentExample {
            ask: present("acme-example-key"),
            expected: None,
        }),
        refusal: Some(RefusalExample {
            ask: ask("refused"),
            present: presenter.then(|| present("")),
        }),
        now: AT.parse().unwrap(),
        secrets: vec![CANARY.into(), "acme-example-key".into()],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_acme_program_passes_the_transport_and_provider_suite_with_one_explicit_reset() {
    let directory = tempfile::tempdir().unwrap();
    let entry = entry(
        PluginKind::Provider,
        vec![
            PYTHON.into(),
            example("provider", "plan.py"),
            "--state-dir".into(),
            directory.path().to_string_lossy().into_owned(),
            "--at".into(),
            AT.into(),
        ],
    );
    let start = start_of(&entry);
    let hello = transport::discover(&start, HOST_VERSION).await.unwrap();
    assert_eq!(hello.kind, PluginKind::Provider);
    assert_eq!(hello.name, "acme-plan");
    let mut report = Report::new("provider Acme, actual Python program");
    transport::run(&start, PluginKind::Provider, HOST_VERSION, &mut report).await;
    let plugin = Plugin::start(&entry, Options::new(HOST_VERSION))
        .await
        .unwrap();
    plugin.settled().await;
    let bridge = ProviderBridge::new(plugin.clone()).unwrap();
    let descriptor = bridge.declared();
    provider::run(
        Sources {
            descriptor: &descriptor,
            facts: Some(&bridge),
            models: Some(&bridge),
            presenter: Some(&bridge),
        },
        Some(&fixture(true)),
        &mut report,
    )
    .await;
    let after = serde_json::to_value(bridge.read_facts(ask("normal")).await.unwrap()).unwrap();
    assert_eq!(after, facts(true, false));
    // The flag changes an actual subsequent read; this is not a mock of an API status or a screen.
    std::fs::write(directory.path().join("limited"), b"limit").unwrap();
    let limited = serde_json::to_value(bridge.read_facts(ask("normal")).await.unwrap()).unwrap();
    assert_eq!(limited, facts(true, true));
    assert_eq!(
        serde_json::to_value(bridge.list_models(ask("empty")).await.unwrap()).unwrap(),
        json!({"models":[]})
    );
    plugin.shutdown().await;
    assert!(report.passed(), "{report}");
    for rule in [
        "transport.shutdown",
        "provider.descriptor",
        "provider.facts.shape",
        "provider.facts.states",
        "provider.models",
        "provider.resets.idempotent",
        "provider.credential.present",
        "provider.quiet",
        "provider.bounds",
    ] {
        assert_eq!(report.outcome(rule), Some(&Outcome::Passed), "{report}");
    }
    assert!(!report.to_string().contains(CANARY));
}

const BROKEN: &str = r#"
import sys
sys.path.insert(0, sys.argv[1])
import plan
original = plan.Acme.facts
fault = sys.argv[3]
def wrong(self, params):
    value = original(self, params)
    if isinstance(value.get("balances"), list) and value["balances"]:
        if fault == "money":
            value["balances"][0]["remaining"] = 12.5
        else:
            value["x_padding"] = "x" * (1024 * 1024 + 1)
    return value
plan.Acme.facts = wrong
sys.argv = ["plan.py", "--state-dir", sys.argv[2], "--at", "2026-10-15T00:00:00Z"]
plan.main()
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_real_program_returning_numeric_money_or_an_oversized_reply_fails_the_named_rule() {
    for (fault, rule) in [
        ("money", "provider.facts.shape"),
        ("bytes", "provider.bounds"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("broken.py");
        std::fs::write(&script, BROKEN).unwrap();
        let plan = std::path::PathBuf::from(example("provider", "plan.py"));
        let entry = entry(
            PluginKind::Provider,
            vec![
                PYTHON.into(),
                script.to_string_lossy().into_owned(),
                plan.parent().unwrap().to_string_lossy().into_owned(),
                directory
                    .path()
                    .join("account")
                    .to_string_lossy()
                    .into_owned(),
                fault.into(),
            ],
        );
        let plugin = Plugin::start(&entry, Options::new(HOST_VERSION))
            .await
            .unwrap();
        plugin.settled().await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        let descriptor = bridge.declared();
        let mut controlled = fixture(false);
        controlled.reset = None;
        controlled.refusal = None;
        let mut report = Report::new(format!("provider with controlled {fault} failure"));
        provider::run(
            Sources {
                descriptor: &descriptor,
                facts: Some(&bridge),
                models: Some(&bridge),
                presenter: Some(&bridge),
            },
            Some(&controlled),
            &mut report,
        )
        .await;
        plugin.shutdown().await;
        assert!(!report.passed());
        assert!(
            matches!(report.outcome(rule), Some(Outcome::Failed(_))),
            "{report}"
        );
        assert!(!report.to_string().contains(CANARY));
    }
}
