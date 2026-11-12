//! Optional management credentials are limited to account-credit reads; ordinary keys still work where admitted.
#![allow(clippy::unwrap_used, reason = "fixture assertions")]
use async_trait::async_trait;
use orch_plugin_contract::Secret;
use orch_provider_contract::builtins::VendorClient;
use orch_provider_contract::{FactsSource, SourceAsk};
use orch_types::{Json, SystemClock};
use serde_json::json;
use std::sync::{Arc, Mutex};
use url::Url;
struct Recorded(Mutex<Vec<(String, String)>>);
#[async_trait]
impl VendorClient for Recorded {
    async fn get(&self, _base: &Url, path: &str, key: &Secret, _bytes: usize) -> Option<Json> {
        self.0
            .lock()
            .unwrap()
            .push((path.into(), key.expose().into()));
        match path {
            "/api/v1/key" => Some(
                json!({"data":{"usage":2.5,"usage_daily":2.5,"usage_weekly":2.5,"usage_monthly":2.5,"limit":10,"limit_remaining":7.5,"limit_reset":null}}),
            ),
            "/api/v1/credits" => Some(json!({"data":{"total_credits":10.0,"total_usage":2.5}})),
            _ => None,
        }
    }
}
#[tokio::test]
async fn a_management_key_never_replaces_the_model_key_and_an_ordinary_key_can_still_return_credits()
 {
    for admin in [false, true] {
        let client = Arc::new(Recorded(Mutex::new(vec![])));
        let source =
            orch_provider_openrouter::OpenRouter::new(client.clone(), Arc::new(SystemClock));
        let ask:SourceAsk=serde_json::from_value(json!({"provider":{"id":orch_types::new_id(),"name":"Fixture"},"endpoints":[{"protocol":"openai-chat","base_url":"https://openrouter.ai/api/v1"}],"credential":{"key":"fixture-model"},"secrets":if admin{json!({"admin_key":"fixture-management"})}else{json!({})}})).unwrap();
        let report = source.read_facts(ask).await.unwrap();
        let calls = client.0.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            [
                ("/api/v1/key".into(), "fixture-model".into()),
                (
                    "/api/v1/credits".into(),
                    if admin {
                        "fixture-management".into()
                    } else {
                        "fixture-model".into()
                    }
                )
            ]
        );
        assert!(
            report.0["balances"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == "account" && row["remaining"] == "7.5")
        );
    }
}
