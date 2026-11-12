//! Published, anonymized response shapes; no native admin credential or live account is used.
#![allow(clippy::unwrap_used, reason = "fixture assertions")]
use async_trait::async_trait;
use orch_plugin_contract::Secret;
use orch_provider_anthropic::{Anthropic, descriptor};
use orch_provider_contract::builtins::VendorClient;
use orch_provider_contract::{FactMember, FactsSource, SourceAsk};
use orch_types::{Clock, Json, ManualClock, Time};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use url::Url;
struct Recorded {
    calls: Mutex<Vec<(String, BTreeMap<String, String>)>>,
    limits: bool,
}
#[async_trait]
impl VendorClient for Recorded {
    async fn get(&self, _base: &Url, _path: &str, _key: &Secret, _bytes: usize) -> Option<Json> {
        panic!("the source must use its declared native authentication headers")
    }
    async fn get_headers(
        &self,
        base: &Url,
        path: &str,
        headers: &BTreeMap<String, Secret>,
        _bytes: usize,
    ) -> Option<Json> {
        assert_eq!(base.host_str(), Some("api.anthropic.com"));
        self.calls.lock().unwrap().push((
            path.into(),
            headers
                .iter()
                .map(|(name, value)| (name.clone(), value.expose().into()))
                .collect(),
        ));
        let path = path.split('?').next().unwrap();
        match path {
            "/v1/organizations/me" => Some(
                json!({"type":"organization","id":"org-fixture","name":"Fixture organization"}),
            ),
            "/v1/organizations/cost_report" => {
                Some(serde_json::from_str(include_str!("fixtures/costs.json")).unwrap())
            }
            "/v1/organizations/spend_limits" if self.limits => {
                Some(serde_json::from_str(include_str!("fixtures/limits.json")).unwrap())
            }
            "/v1/organizations/spend_limits/effective" if self.limits => {
                Some(serde_json::from_str(include_str!("fixtures/effective.json")).unwrap())
            }
            _ => None,
        }
    }
}
fn ask(admin: bool) -> SourceAsk {
    serde_json::from_value(json!({"provider":{"id":orch_types::new_id(),"name":"Fixture"},"endpoints":[{"protocol":"anthropic-messages","base_url":"https://api.anthropic.com"}],"credential":{"key":"model-key-must-not-be-used"},"secrets":if admin{json!({"admin_key":"admin-only-fixture"})}else{json!({})}})).unwrap()
}
fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(
        "2026-11-08T12:00:00Z".parse::<Time>().unwrap().get(),
    ))
}
#[tokio::test]
async fn costs_and_limits_preserve_fractional_minor_units_and_use_only_the_admin_headers() {
    let native = Arc::new(Recorded {
        calls: Mutex::new(vec![]),
        limits: true,
    });
    let report = Anthropic::new(native.clone(), clock())
        .read_facts(ask(true))
        .await
        .unwrap();
    assert_eq!(report.0["identity"], "Fixture organization");
    assert_eq!(report.0["spend"][0]["used"], "123.78912");
    assert_eq!(report.0["spend"][0]["unit"], "USD cents");
    assert_eq!(report.0["spend"][2]["used"], "12050.5");
    assert_eq!(report.0["spend"][2]["limit"], "50000");
    assert_eq!(report.0["windows"][0]["resets_at"], Json::Null);
    assert!(report.0.get("balances").is_none());
    let calls = native.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    for (path, headers) in &*calls {
        assert_eq!(headers.get("x-api-key").unwrap(), "admin-only-fixture");
        assert!(!headers.contains_key("Authorization"));
        if path.contains("spend_limits") {
            assert_eq!(
                headers.get("anthropic-beta").unwrap(),
                "spend-limit-reads-2026-09-26"
            );
        }
    }
}
#[tokio::test]
async fn optional_early_access_refusals_keep_costs_and_identity_without_an_error() {
    let native = Arc::new(Recorded {
        calls: Mutex::new(vec![]),
        limits: false,
    });
    let report = Anthropic::new(native, clock())
        .read_facts(ask(true))
        .await
        .unwrap();
    assert_eq!(report.0["identity"], "Fixture organization");
    assert_eq!(report.0["spend"].as_array().unwrap().len(), 1);
    assert!(report.0.get("windows").is_none());
}
#[tokio::test]
async fn without_an_admin_key_no_model_key_is_borrowed_and_members_are_not_offered() {
    let native = Arc::new(Recorded {
        calls: Mutex::new(vec![]),
        limits: true,
    });
    assert_eq!(
        Anthropic::new(native.clone(), clock())
            .read_facts(ask(false))
            .await
            .unwrap()
            .0,
        json!({})
    );
    assert!(native.calls.lock().unwrap().is_empty());
    assert_eq!(
        descriptor().unavailable_members(&[]),
        [FactMember::Identity, FactMember::Spend, FactMember::Windows]
    );
    assert!(!descriptor().reports().contains(&FactMember::Balances));
    assert!(!descriptor().wants_key());
    assert!(descriptor().check().is_ok());
}
