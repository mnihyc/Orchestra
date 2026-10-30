//! The vendors' readers this program registers keep their recorded answers' numbers and dates through the connector's common reader of facts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use async_trait::async_trait;
use orch_agent_contract::facts::{ProviderFacts, normalize};
use orch_connector_core::sources::reports;
use orch_plugin_contract::Secret;
use orch_provider_contract::builtins::VendorClient;
use orch_provider_contract::source::{SourceAskCredential, SourceAskEndpoint, SourceAskProvider};
use orch_provider_contract::{FactMember, SourceAsk, SourceErrorCode};
use orch_types::{Clock, Json, ManualClock, Time, new_id};
use serde_json::json;
use std::sync::Arc;
use url::Url;

fn at() -> Time {
    "2026-10-13T00:00:00.000Z".parse().unwrap()
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(at().get()))
}

fn ask(base: &str) -> SourceAsk {
    SourceAsk {
        provider: SourceAskProvider {
            id: new_id(),
            name: "Recorded account".to_owned(),
        },
        endpoints: vec![SourceAskEndpoint {
            protocol: "openai-responses".to_owned(),
            base_url: base.to_owned(),
        }],
        config: Default::default(),
        secrets: Default::default(),
        credential: Some(SourceAskCredential {
            key: Secret::new("recorded-test-key"),
        }),
    }
}

fn same_members(before: &ProviderFacts, after: &ProviderFacts) {
    let mut before = before.clone();
    normalize(&mut before);
    let old = serde_json::to_value(&before).unwrap();
    let new = serde_json::to_value(after).unwrap();
    for member in FactMember::ALL {
        assert_eq!(
            new[member.as_str()],
            old[member.as_str()],
            "{}",
            member.as_str()
        );
    }
}

struct OpenRouterReply {
    credits: bool,
}

#[async_trait]
impl VendorClient for OpenRouterReply {
    async fn get(&self, _base: &Url, path: &str, key: &Secret, bound: usize) -> Option<Json> {
        assert_eq!(key.expose(), "recorded-test-key");
        assert_eq!(bound, orch_provider_contract::limits::REPORT_MAX_BYTES);
        match path {
            "/api/v1/key" => Some(json!({"data":{"label":"example","is_management_key":false,
                "limit":20.0,"limit_remaining":17.0,"usage":3.0,"usage_daily":0.1,"usage_weekly":1.0,"usage_monthly":2.0,
                "expires_at":"2026-12-01T00:00:00Z"}})),
            "/api/v1/credits" if self.credits => {
                Some(json!({"data":{"total_credits":50.0,"total_usage":3.0}}))
            }
            _ => None,
        }
    }
}

#[tokio::test]
async fn openrouters_recorded_key_and_optional_account_credits_have_the_same_canonical_facts() {
    for credits in [false, true] {
        let client = Arc::new(OpenRouterReply { credits });
        let clock = clock();
        let base = Url::parse("https://openrouter.ai/api/v1").unwrap();
        let before = orch_provider_openrouter::read(
            client.as_ref(),
            &base,
            &Secret::new("recorded-test-key"),
            orch_provider_openrouter::KnownVendor::OpenRouter,
            clock.as_ref(),
        )
        .await
        .unwrap();
        let source = (orch_provider_openrouter::builtin().make)(client, clock);
        let report = source.read_facts(ask(base.as_str())).await.unwrap();
        let read = reports::read(&report.0, at(), true, &[]).unwrap();
        assert!(read.notes.is_empty(), "{:?}", read.notes);
        let after = reports::normalized(&read).unwrap();
        same_members(&before, &after);
        let total = after
            .spend
            .iter()
            .find(|row| row.id == "key:total")
            .unwrap();
        assert_eq!(
            (
                total.used.as_deref(),
                total.limit.as_deref(),
                total.remaining.as_deref()
            ),
            (Some("3"), Some("20"), Some("17"))
        );
        let remaining = |id: &str| {
            after
                .balances
                .iter()
                .find(|balance| balance.id == id)
                .and_then(|balance| balance.remaining.clone())
        };
        assert_eq!(remaining("account").as_deref(), credits.then_some("47"));
        assert_eq!(after.credential_verified, Some(true));
        assert_eq!(after.field_times["spend.key:day"], at());
        assert_eq!(
            after
                .credential
                .clone()
                .unwrap()
                .expires_at
                .unwrap()
                .to_string(),
            "2026-12-01T00:00:00.000Z"
        );
        // This answer names no reset of the key's limit: it never starts over, and is a balance, not a window.
        assert!(after.windows.is_empty());
        assert_eq!(remaining("key").as_deref(), Some("17"));
    }
}

#[tokio::test]
async fn an_openrouter_keys_weekly_limit_and_free_requests_reach_the_stored_facts_as_windows() {
    struct Weekly;
    #[async_trait]
    impl VendorClient for Weekly {
        async fn get(&self, _base: &Url, path: &str, _key: &Secret, _bound: usize) -> Option<Json> {
            (path == "/api/v1/key").then(|| json!({"data":{"limit":20.0,"limit_remaining":15.0,"limit_reset":"weekly",
                "usage":5.0,"usage_daily":1.0,"usage_weekly":5.0,"usage_monthly":5.0,"budgets":[],"effective_budget":null,
                "free_model_daily_requests":{"used":25,"limit":50,"remaining":25},"expires_at":null}}))
        }
    }
    let source = (orch_provider_openrouter::builtin().make)(Arc::new(Weekly), clock());
    let report = source
        .read_facts(ask("https://openrouter.ai/api/v1"))
        .await
        .unwrap();
    let read = reports::read(&report.0, at(), true, &[]).unwrap();
    assert!(read.notes.is_empty(), "{:?}", read.notes);
    let facts = reports::normalized(&read).unwrap();
    // The clock reads Tuesday the 13th: the week starts over on Monday the 19th, the day at midnight.
    let windows: Vec<_> = facts
        .windows
        .iter()
        .map(|window| {
            (
                window.id.as_str(),
                window.used_percent,
                window.resets_at.map(|at| at.to_string()),
            )
        })
        .collect();
    assert_eq!(
        windows,
        [
            ("key", 25.0, Some("2026-10-19T00:00:00.000Z".to_owned())),
            (
                "free_requests",
                50.0,
                Some("2026-10-14T00:00:00.000Z".to_owned())
            )
        ]
    );
    assert_eq!(facts.field_times["windows.key"], at());
    assert!(
        facts.balances.is_empty(),
        "an ordinary key's account credits are not told"
    );
}

struct DeepSeekReply(Option<Json>);

#[async_trait]
impl VendorClient for DeepSeekReply {
    async fn get(&self, _base: &Url, path: &str, _key: &Secret, bound: usize) -> Option<Json> {
        assert_eq!(path, "/user/balance");
        assert_eq!(bound, orch_provider_contract::limits::REPORT_MAX_BYTES);
        self.0.clone()
    }
}

#[tokio::test]
async fn deepseeks_documented_balance_is_decimal_text_and_a_refused_or_changed_reply_is_not_told() {
    let documented = json!({"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]});
    let source = (orch_provider_deepseek::builtin().make)(
        Arc::new(DeepSeekReply(Some(documented))),
        clock(),
    );
    let report = source
        .read_facts(ask("https://api.deepseek.com/anthropic"))
        .await
        .unwrap();
    let read = reports::read(&report.0, at(), true, &[]).unwrap();
    assert!(read.notes.is_empty(), "{:?}", read.notes);
    let facts = reports::normalized(&read).unwrap();
    assert_eq!(facts.balances[0].remaining.as_deref(), Some("110.00"));
    assert_eq!(facts.balances[0].unit, "CNY");
    let parts = facts.balances[0].parts.as_ref().unwrap();
    assert_eq!(
        parts
            .iter()
            .map(|part| part.amount.as_str())
            .collect::<Vec<_>>(),
        ["10.00", "100.00"]
    );
    assert_eq!(facts.credential_verified, Some(true));
    assert_eq!(facts.field_times["balances.CNY"], at());
    for reply in [
        None,
        Some(json!({"is_available":true,"new_balance_infos":[]})),
    ] {
        let source =
            (orch_provider_deepseek::builtin().make)(Arc::new(DeepSeekReply(reply)), clock());
        assert_eq!(
            source
                .read_facts(ask("https://api.deepseek.com"))
                .await
                .unwrap_err()
                .code,
            SourceErrorCode::NotTold
        );
    }
}
