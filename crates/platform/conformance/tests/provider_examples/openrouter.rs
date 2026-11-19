//! The real Python OpenRouter source agrees with the built-in on identical recorded HTTP bodies.

use super::{HOST_VERSION, PYTHON, ask, entry, example};
use async_trait::async_trait;
use orch_model_contract::protocol::WireProtocol;
use orch_plugin::{Options, Plugin, bridges::provider::ProviderBridge};
use orch_plugin_contract::{PluginKind, Secret};
use orch_provider_contract::{FactMember, FactsSource, ModelsSource};
use orch_types::{Json, ManualClock, Time};
use serde_json::json;
use std::sync::Arc;
use url::Url;

struct Recorded(Json);
#[async_trait]
impl orch_provider_contract::builtins::VendorClient for Recorded {
    async fn get(&self, _base: &Url, path: &str, _key: &Secret, _maximum: usize) -> Option<Json> {
        self.0.get(path).cloned()
    }
}

fn recording(management: bool) -> Json {
    let budgets = json!([
        {"scope":"key_limit","limit_usd":50,"spend_usd":12.5,"remaining_usd":37.5,"reset_interval":"daily","resets_at":null,"guardrail_id":null,"budget_id":null,"include_byok":true},
        {"scope":"member","guardrail_id":"example-budget","budget_id":null,"limit_usd":100,"spend_usd":10,"remaining_usd":90,"reset_interval":"weekly","resets_at":null,"include_byok":false},
        {"scope":"workspace","budget_id":"monthly","guardrail_id":null,"limit_usd":1000,"spend_usd":30,"remaining_usd":970,"reset_interval":"monthly","resets_at":null,"include_byok":true}]);
    let mut replies = json!({"/api/v1/key":{"data":{"label":"Recorded key","limit":50,"limit_remaining":37.5,"limit_reset":"daily",
        "usage":12.5,"usage_daily":2,"usage_weekly":7,"usage_monthly":12.5,"expires_at":null,
        "byok_usage":1,"byok_usage_daily":0.1,"byok_usage_weekly":0.5,"byok_usage_monthly":1,
        "is_free_tier":false,"budgets":budgets,"effective_budget":budgets[0],
        "free_model_daily_requests":{"used":8,"limit":1000,"remaining":992}}},
        "/api/v1/models":{"data":[{"id":"openai/recorded","name":"Recorded","context_length":128000,
            "pricing":{"prompt":"0.0000002","completion":"0.0000008","input_cache_read":"0.0000001"}}]}});
    if management {
        replies["/api/v1/credits"] = json!({"data":{"total_credits":100,"total_usage":12.5}});
    }
    replies
}

#[tokio::test]
async fn a_key_program_and_the_builtin_have_identical_utc_windows_spend_and_optional_account_credits()
 {
    for (at, management, shape) in [
        ("2026-10-12T00:00:00.000Z", false, "budgets"),
        ("2026-12-31T23:59:59.000Z", true, "budgets"),
        ("2026-10-15T15:30:00.000Z", false, "never"),
        ("2026-10-15T15:30:00.000Z", true, "unknown"),
    ] {
        let at: Time = at.parse().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut replies = recording(management);
        if shape == "never" {
            replies["/api/v1/key"]["data"]["budgets"] = Json::Null;
            replies["/api/v1/key"]["data"]["effective_budget"] = Json::Null;
            replies["/api/v1/key"]["data"]["limit_reset"] = Json::Null;
        } else if shape == "unknown" {
            replies["/api/v1/key"]["data"]["budgets"] = json!([]);
            replies["/api/v1/key"]["data"]["effective_budget"] =
                json!({"scope":"member","limit_usd":"not a number"});
            replies["/api/v1/key"]["data"]["limit_reset"] = json!("vendor-specific");
            replies["/api/v1/key"]["data"]
                .as_object_mut()
                .unwrap()
                .remove("byok_usage_daily");
            replies["/api/v1/key"]["data"]
                .as_object_mut()
                .unwrap()
                .remove("is_free_tier");
        }
        let file = directory.path().join("recording.json");
        std::fs::write(&file, serde_json::to_vec(&replies).unwrap()).unwrap();
        let entry = entry(
            PluginKind::Provider,
            vec![
                PYTHON.into(),
                example("provider", "openrouter_key.py"),
                "--recording".into(),
                file.to_string_lossy().into_owned(),
                "--at".into(),
                at.to_string(),
            ],
        );
        let plugin = Plugin::start(&entry, Options::new(HOST_VERSION))
            .await
            .unwrap();
        plugin.settled().await;
        let bridge = ProviderBridge::new(plugin.clone()).unwrap();
        let mut query = ask("normal");
        query.endpoints = serde_json::from_value(
            json!([{"protocol":"openai-responses","base_url":"https://openrouter.ai/api/v1"}]),
        )
        .unwrap();
        query.credential =
            serde_json::from_value(json!({"key":"recorded-key-with-no-account"})).unwrap();
        let builtin = orch_provider_openrouter::OpenRouter::new(
            Arc::new(Recorded(replies.clone())),
            Arc::new(ManualClock::new(at.get())),
        );
        let facts = bridge.read_facts(query.clone()).await.unwrap();
        assert_eq!(facts, builtin.read_facts(query.clone()).await.unwrap());
        let rows = facts.0["spend"].as_array().unwrap();
        assert_eq!(
            rows.iter().find(|row| row["id"] == "byok:week").unwrap()["used"],
            "0.5"
        );
        let total = rows.iter().find(|row| row["id"] == "key:total").unwrap();
        if shape == "unknown" {
            assert!(total.get("x_openrouter_is_free_tier").is_none());
            assert!(!rows.iter().any(|row| row["id"] == "byok:day"));
            assert!(facts.0["windows"][0]["resets_at"].is_null());
        } else {
            assert_eq!(total["x_openrouter_is_free_tier"], false);
        }
        assert!(
            facts.0.get("plan").is_none(),
            "the vendor boolean must not manufacture a subscription plan"
        );
        if shape == "never" {
            assert_eq!(facts.0["balances"][0]["id"], "key");
            assert!(
                facts.0["balances"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|row| row["id"] != "account")
            );
        }
        if at.to_string() == "2026-10-12T00:00:00.000Z" {
            assert_eq!(
                facts.0["windows"][1]["resets_at"],
                "2026-10-19T00:00:00.000Z"
            );
        }
        assert!(
            !bridge.declared().reports().contains(&FactMember::Balances),
            "an ordinary key does not declare account-credit support"
        );
        let protocol = orch_protocol_openai_responses::OpenAiResponses;
        let expected =
            protocol.parse_models(&serde_json::to_vec(&replies["/api/v1/models"]).unwrap());
        assert_eq!(
            serde_json::to_value(bridge.list_models(query.clone()).await.unwrap()).unwrap(),
            json!({"models":expected})
        );
        let endpoints = plugin
            .call("x-endpoint.describe", serde_json::to_value(&query).unwrap())
            .await
            .unwrap();
        assert_eq!(
            endpoints["endpoints"][0]["base_url"],
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            endpoints["endpoints"][1]["base_url"],
            "https://openrouter.ai/api"
        );
        assert_eq!(
            endpoints["presents"],
            json!({"header":"authorization","scheme":"Bearer"})
        );
        assert_eq!(
            endpoints["endpoints"][0]["body"],
            json!({"store":false,"stream":true})
        );
        assert_eq!(
            endpoints["endpoints"][1]["options"]["headers"]["anthropic-version"],
            "2023-06-01"
        );
        let presented = plugin
            .call(
                "credential.present",
                json!({"credential":{"key":"recorded-key-with-no-account"},
            "endpoint":{"protocol":"anthropic-messages","base_url":"https://openrouter.ai/api"}}),
            )
            .await
            .unwrap();
        assert_eq!(
            presented["headers"]["authorization"],
            "Bearer recorded-key-with-no-account"
        );
        let refused = plugin
            .call(
                "credential.present",
                json!({"credential":{"key":"recorded-key-with-no-account"},
            "endpoint":{"protocol":"openai-responses","base_url":"https://other.invalid/api/v1"}}),
            )
            .await;
        assert!(
            refused.is_err(),
            "the key must not be presented to an unrelated origin"
        );
        assert!(
            !endpoints
                .to_string()
                .contains("recorded-key-with-no-account")
        );
        plugin.shutdown().await;
    }
}
