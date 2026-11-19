//! OpenRouter's recorded answers read as a subscription's facts: the key's limit and each budget as windows with their next reset, the free models' requests of the day, the account's credits for a management key alone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use async_trait::async_trait;
use orch_agent_contract::facts::ProviderFacts;
use orch_plugin_contract::Secret;
use orch_provider_contract::builtins::{BuiltinSources, VendorClient};
use orch_provider_contract::members::declared;
use orch_provider_contract::source::{SourceAskCredential, SourceAskEndpoint, SourceAskProvider};
use orch_provider_contract::{FactMember, MemberState, SourceAsk, member_state};
use orch_provider_openrouter::builtin;
use orch_types::{Clock, Json, ManualClock, Time, new_id};
use serde_json::json;
use std::sync::{Arc, Mutex};
use url::Url;

const BASE: &str = "https://openrouter.ai/api/v1";

/// A Thursday afternoon: the day starts over at midnight, the week on Monday the 19th, the month on the 1st.
fn now() -> Time {
    "2026-10-15T15:30:00.000Z".parse().unwrap()
}

/// OpenRouter as recorded answers: the key's, and the account's credits where it answers them. An ordinary key
/// is answered 403 there, which the keeping machine's transport tells as nothing.
struct Vendor {
    key: Json,
    credits: Option<Json>,
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl VendorClient for Vendor {
    async fn get(&self, base: &Url, path: &str, key: &Secret, reply_bytes: usize) -> Option<Json> {
        assert_eq!(base.as_str(), BASE);
        assert_eq!(key.expose(), "recorded-key");
        assert_eq!(
            reply_bytes,
            orch_provider_contract::limits::REPORT_MAX_BYTES
        );
        self.asked.lock().unwrap().push(path.to_owned());
        match path {
            "/api/v1/key" => Some(self.key.clone()),
            "/api/v1/credits" => self.credits.clone(),
            _ => None,
        }
    }
}

/// What the built-in a program registers reports for these answers.
async fn report(key: Json, credits: Option<Json>) -> Json {
    let vendor = Arc::new(Vendor {
        key,
        credits,
        asked: Mutex::new(Vec::new()),
    });
    let table = BuiltinSources::new([builtin()]).unwrap();
    let entry = table
        .reading(&[Url::parse(BASE).unwrap()])
        .copied()
        .expect("OpenRouter's address is its");
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(now().get()));
    let source = (entry.make)(vendor.clone(), clock);
    let ask = SourceAsk {
        provider: SourceAskProvider {
            id: new_id(),
            name: "OpenRouter".to_owned(),
        },
        endpoints: vec![SourceAskEndpoint {
            protocol: "openai-responses".to_owned(),
            base_url: BASE.to_owned(),
        }],
        config: Default::default(),
        secrets: Default::default(),
        credential: Some(SourceAskCredential {
            key: Secret::new("recorded-key"),
        }),
    };
    let told = source.read_facts(ask).await.unwrap().0;
    assert_eq!(
        vendor.asked.lock().unwrap().as_slice(),
        ["/api/v1/key", "/api/v1/credits"]
    );
    told
}

/// The key's answer with these limits, budgets and free requests, and spending of 5 USD this week.
fn key(limit: Json, remaining: Json, reset: Json, budgets: Json, effective: Json) -> Json {
    json!({"data": {
        "label": "test-only-openrouter-key", "limit": limit, "limit_remaining": remaining, "limit_reset": reset,
        "include_byok_in_limit": false, "usage": 5.0, "usage_daily": 1.0, "usage_weekly": 5.0, "usage_monthly": 5.0,
        "byok_usage": 0.0, "byok_usage_daily": 0.0, "byok_usage_weekly": 0.0, "byok_usage_monthly": 0.0,
        "is_free_tier": false, "free_model_daily_requests": {"used": 25, "limit": 50, "remaining": 25},
        "budgets": budgets, "effective_budget": effective, "expires_at": null
    }})
}

fn window(
    id: &str,
    label: &str,
    minutes: Option<i64>,
    used: f64,
    resets: &str,
    active: Option<bool>,
) -> Json {
    json!({"id": id, "label": label, "duration_minutes": minutes, "used_percent": used, "resets_at": resets,
        "model_family": null, "is_active": active, "reset_not_started": null, "warning_percent": null, "severity": null})
}

fn free_requests() -> Json {
    window(
        "free_requests",
        "Free model requests, daily",
        Some(1440),
        50.0,
        "2026-10-16T00:00:00.000Z",
        None,
    )
}

#[tokio::test]
async fn an_ordinary_key_with_a_weekly_limit_shows_it_as_a_window_and_its_credits_as_unsupported() {
    let told = report(
        key(
            json!(20.0),
            json!(15.0),
            json!("weekly"),
            json!([]),
            Json::Null,
        ),
        None,
    )
    .await;
    assert_eq!(
        told["windows"],
        json!([
            window(
                "key",
                "Key limit, weekly",
                Some(10_080),
                25.0,
                "2026-10-19T00:00:00.000Z",
                None
            ),
            free_requests()
        ])
    );
    let total = told["spend"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "key:total")
        .unwrap();
    assert_eq!(
        (&total["used"], &total["limit"], &total["remaining"]),
        (&json!("5"), &json!("20"), &json!("15"))
    );
    // OpenRouter answers the account's credits with 403 to an ordinary key: nothing is told, and no error.
    assert!(told.get("balances").is_none());
    let facts: ProviderFacts = serde_json::from_value(told.clone()).unwrap();
    let members = declared(
        Some(&(builtin().descriptor)()),
        &orch_provider_contract::members::KEY_KEEPER,
    );
    assert_eq!(
        member_state(FactMember::Balances, &facts, &members),
        MemberState::Unsupported
    );
    assert_eq!(told["credential_verified"], true);
}

#[tokio::test]
async fn a_management_keys_answer_adds_the_accounts_credits() {
    let told = report(
        key(
            json!(20.0),
            json!(15.0),
            json!("weekly"),
            json!([]),
            Json::Null,
        ),
        Some(json!({"data": {"total_credits": 50.0, "total_usage": 12.5}})),
    )
    .await;
    assert_eq!(
        told["balances"],
        json!([{"id": "account", "label": "Credits", "unit": "USD", "remaining": "37.5", "total": "50", "used": "12.5",
            "unlimited": null, "parts": null, "expires_at": null}])
    );
}

#[tokio::test]
async fn budgets_are_windows_the_binding_one_marked_and_one_that_never_starts_over_a_balance() {
    let weekly = |scope: &str, guardrail: Json, limit: f64, spend: f64| {
        json!({
        "scope": scope, "guardrail_id": guardrail, "budget_id": null, "limit_usd": limit, "spend_usd": spend,
        "remaining_usd": limit - spend, "reset_interval": "weekly", "resets_at": "2026-10-19T00:00:00Z", "include_byok": false})
    };
    let member = weekly("member", json!("g-1"), 50.0, 40.0);
    let lifetime = json!({"scope": "workspace", "guardrail_id": null, "budget_id": "b-7", "limit_usd": 500.0,
        "spend_usd": 125.0, "remaining_usd": 375.0, "reset_interval": null, "resets_at": null, "include_byok": false});
    let budgets = json!([
        weekly("workspace_default_member", Json::Null, 100.0, 40.0),
        member.clone(),
        lifetime
    ]);
    let told = report(
        key(Json::Null, Json::Null, Json::Null, budgets, member),
        None,
    )
    .await;
    assert_eq!(
        told["windows"],
        json!([
            window(
                "budget:workspace_default_member",
                "Workspace member default budget, weekly",
                Some(10_080),
                40.0,
                "2026-10-19T00:00:00.000Z",
                Some(false)
            ),
            window(
                "budget:member:g-1",
                "Member budget, weekly",
                Some(10_080),
                80.0,
                "2026-10-19T00:00:00.000Z",
                Some(true)
            ),
            free_requests()
        ])
    );
    assert_eq!(
        told["balances"],
        json!([{"id": "budget:workspace:b-7", "label": "Workspace budget", "unit": "USD", "remaining": "375",
            "total": "500", "used": "125", "unlimited": null, "parts": null, "expires_at": null}])
    );
}

#[tokio::test]
async fn the_keys_limit_kept_as_a_budget_is_one_window_with_the_vendors_reset() {
    let own = json!({"scope": "key_limit", "guardrail_id": null, "budget_id": null, "limit_usd": 10.0, "spend_usd": 7.5,
        "remaining_usd": 2.5, "reset_interval": "daily", "resets_at": "2026-10-16T00:00:00Z", "include_byok": false});
    let told = report(
        key(
            json!(10.0),
            json!(2.5),
            json!("daily"),
            json!([own.clone()]),
            own,
        ),
        None,
    )
    .await;
    assert_eq!(
        told["windows"],
        json!([
            window(
                "key",
                "Key limit, daily",
                Some(1440),
                75.0,
                "2026-10-16T00:00:00.000Z",
                Some(true)
            ),
            free_requests()
        ])
    );
}

#[tokio::test]
async fn a_key_without_a_limit_has_no_window_of_its_own_and_one_whose_limit_never_starts_over_has_a_balance()
 {
    let unlimited = report(
        key(Json::Null, Json::Null, Json::Null, json!([]), Json::Null),
        None,
    )
    .await;
    assert_eq!(unlimited["windows"], json!([free_requests()]));
    assert!(unlimited.get("balances").is_none());
    let once = report(
        key(json!(10.0), json!(4.0), Json::Null, json!([]), Json::Null),
        None,
    )
    .await;
    assert_eq!(
        once["windows"],
        json!([free_requests()]),
        "a limit that never starts over is no window"
    );
    assert_eq!(
        once["balances"],
        json!([{"id": "key", "label": "Key limit", "unit": "USD", "remaining": "4", "total": "10", "used": null,
            "unlimited": null, "parts": null, "expires_at": null}])
    );
    let total = once["spend"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "key:total")
        .unwrap();
    assert_eq!(
        (&total["limit"], &total["remaining"]),
        (&json!("10"), &json!("4")),
        "the key's spending says it too"
    );
}
