//! Anonymized, authored Costs API shape, independently checked against its published documentation.
#![allow(clippy::unwrap_used, reason = "fixture assertions")]
use async_trait::async_trait;
use orch_plugin_contract::Secret;
use orch_provider_contract::builtins::VendorClient;
use orch_provider_contract::{FactMember, FactsSource, SourceAsk};
use orch_provider_openai::{OpenAi, descriptor};
use orch_types::{Clock, Json, ManualClock, Time};
use serde_json::json;
use std::sync::{Arc, Mutex};
use url::Url;
struct Recorded {
    calls: Mutex<Vec<(String, String)>>,
}
#[async_trait]
impl VendorClient for Recorded {
    async fn get(&self, base: &Url, path: &str, key: &Secret, _bytes: usize) -> Option<Json> {
        assert_eq!(base.host_str(), Some("api.openai.com"));
        self.calls
            .lock()
            .unwrap()
            .push((path.into(), key.expose().into()));
        Some(serde_json::from_str(include_str!("fixtures/costs.json")).unwrap())
    }
}
fn ask(admin: Option<&str>) -> SourceAsk {
    let mut value = json!({"provider":{"id":orch_types::new_id(),"name":"Fixture"},"endpoints":[{"protocol":"openai-responses","base_url":"https://api.openai.com/v1"}],"credential":{"key":"model-key-must-not-be-used"},"secrets":{}});
    if let Some(admin) = admin {
        value["secrets"]["admin_key"] = json!(admin);
    }
    serde_json::from_value(value).unwrap()
}
fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(
        "2026-11-08T12:00:00Z".parse::<Time>().unwrap().get(),
    ))
}
#[tokio::test]
async fn only_the_separate_admin_key_reads_exact_daily_costs_and_no_balance_is_invented() {
    let native = Arc::new(Recorded {
        calls: Mutex::new(vec![]),
    });
    let source = OpenAi::new(native.clone(), clock());
    let report = source
        .read_facts(ask(Some("admin-only-fixture")))
        .await
        .unwrap();
    assert_eq!(report.0["spend"][0]["used"], "1.23456789");
    assert_eq!(report.0["spend"][0]["unit"], "USD");
    assert_eq!(report.0["spend"][1]["used"], "0");
    assert!(report.0.get("balances").is_none());
    assert!(!descriptor().reports().contains(&FactMember::Balances));
    assert!(
        !descriptor().wants_key(),
        "the source must not receive the ordinary model key"
    );
    let calls = native.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, "admin-only-fixture");
    let url = Url::parse(&format!("https://api.openai.com{}", calls[0].0)).unwrap();
    assert_eq!(url.path(), "/v1/organization/costs");
    assert_eq!(
        url.query_pairs()
            .filter(|(key, _)| key == "group_by")
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>(),
        ["project_id", "line_item"]
    );
}
#[tokio::test]
async fn an_absent_admin_key_is_not_an_error_or_a_model_key_fallback() {
    let native = Arc::new(Recorded {
        calls: Mutex::new(vec![]),
    });
    let source = OpenAi::new(native.clone(), clock());
    assert_eq!(source.read_facts(ask(None)).await.unwrap().0, json!({}));
    assert!(native.calls.lock().unwrap().is_empty());
    assert_eq!(descriptor().unavailable_members(&[]), [FactMember::Spend]);
    assert!(
        descriptor()
            .unavailable_members(&["admin_key".into()])
            .is_empty()
    );
    assert!(descriptor().check().is_ok());
}
#[test]
fn missing_costs_are_not_zero_and_a_lookalike_origin_is_not_the_vendor() {
    assert!(orch_provider_openai::cost_rows(&json!({"data":[{"start_time":1,"end_time":2,"results":[{"amount":{"currency":"usd"}}]}]})).is_err());
    for base in [
        "https://api.openai.com.evil.test/v1",
        "http://api.openai.com/v1",
        "https://api.openai.com:9443/v1",
        "https://user@api.openai.com/v1",
    ] {
        assert!(!orch_provider_openai::known_vendor(
            &Url::parse(base).unwrap()
        ));
    }
}
