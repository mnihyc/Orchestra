//! Document-shaped synthetic responses, not a live admin-account recording.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use async_trait::async_trait;
use orch_plugin_contract::Secret;
use orch_provider_anthropic::{Anthropic, builtin};
use orch_provider_contract::builtins::VendorClient;
use orch_provider_contract::source::{SourceAskCredential, SourceAskEndpoint, SourceAskProvider};
use orch_provider_contract::{FactMember, FactsSource, SourceAsk};
use orch_types::{Json, ManualClock, Time, new_id};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use url::Url;

#[derive(Clone)]
struct Asked {
    path: String,
    query: Vec<(String, String)>,
}
struct Recorded {
    answers: BTreeMap<String, Json>,
    requests: Mutex<Vec<Asked>>,
}
#[async_trait]
impl VendorClient for Recorded {
    async fn get(&self, _: &Url, _: &str, _: &Secret, _: usize) -> Option<Json> {
        panic!("facts require explicit header authentication")
    }
    async fn get_headers(
        &self,
        base: &Url,
        path: &str,
        headers: &BTreeMap<String, Secret>,
        bound: usize,
    ) -> Option<Json> {
        assert_eq!(base.as_str(), "https://api.anthropic.com/");
        assert_eq!(bound, orch_provider_contract::limits::REPORT_MAX_BYTES);
        assert_eq!(headers["anthropic-version"].expose(), "2026-10-21");
        assert_eq!(
            headers["x-api-key"].expose(),
            "test-admin",
            "the model key cannot read organization endpoints"
        );
        assert!(!headers.contains_key("Authorization"));
        let url = base.join(path).unwrap();
        let request = Asked {
            path: url.path().into(),
            query: url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
        };
        if request.path.contains("spend_limits") {
            assert_eq!(
                headers["anthropic-beta"].expose(),
                "spend-limit-reads-2026-09-26"
            );
        }
        self.requests.lock().unwrap().push(request.clone());
        let key = request
            .query
            .iter()
            .find(|(name, _)| name == "page")
            .map_or_else(
                || request.path.clone(),
                |(_, page)| format!("{}?page={page}", request.path),
            );
        self.answers.get(&key).cloned()
    }
}
fn ask(admin: bool) -> SourceAsk {
    SourceAsk {
        provider: SourceAskProvider {
            id: new_id(),
            name: "Test account".into(),
        },
        endpoints: vec![SourceAskEndpoint {
            protocol: "anthropic-messages".into(),
            base_url: "https://api.anthropic.com".into(),
        }],
        config: Default::default(),
        secrets: if admin {
            [("admin_key".into(), Secret::new("test-admin"))].into()
        } else {
            Default::default()
        },
        credential: Some(SourceAskCredential {
            key: Secret::new("test-model"),
        }),
    }
}
fn source(answers: BTreeMap<String, Json>) -> (Anthropic, Arc<Recorded>) {
    let client = Arc::new(Recorded {
        answers,
        requests: Mutex::new(Vec::new()),
    });
    let at: Time = "2026-11-07T12:00:00Z".parse().unwrap();
    (
        Anthropic::new(client.clone(), Arc::new(ManualClock::new(at.get()))),
        client,
    )
}
fn answers() -> BTreeMap<String, Json> {
    [
        ("/v1/organizations/me".into(), json!({"type":"organization","id":"test-org","name":"Example organization"})),
        ("/v1/organizations/cost_report".into(), json!({"data":[{"starting_at":"2026-11-06T00:00:00Z","ending_at":"2026-11-07T00:00:00Z","results":[{"amount":"123.78912","currency":"USD","description":"Token usage","workspace_id":"workspace-test"}]}],"has_more":false,"next_page":null})),
        ("/v1/organizations/spend_limits".into(), json!({"data":[{"type":"spend_limit","id":"test-limit","amount":"50000","currency":"USD","is_enabled":true,"period":"monthly","scope":{"type":"organization"}}],"next_page":null})),
        ("/v1/organizations/spend_limits/effective".into(), json!({"data":[{"scope":{"type":"user","user_id":"user-test"},"amount":"50000","currency":"USD","period":"monthly","source":{"type":"organization"},"spend_limit_id":"test-limit","period_to_date_spend":"31402.5"}],"next_page":null})),
    ].into()
}
#[tokio::test]
async fn costs_and_effective_limits_preserve_fractional_cents_without_inventing_a_balance() {
    let (source, client) = source(answers());
    let report = source.read_facts(ask(true)).await.unwrap().0;
    assert_eq!(report["identity"], "Example organization");
    assert_eq!(report["spend"][0]["used"], "123.78912");
    assert_eq!(report["spend"][0]["unit"], "USD cents");
    let descriptor = source.describe();
    assert!(
        descriptor
            .summary
            .contains("Priority Tier costs are not included")
    );
    assert_eq!(
        descriptor.summary_locales.get("en"),
        Some(&descriptor.summary)
    );
    assert!(descriptor.summary_locales["zh-CN"].contains("不包含 Priority Tier 费用"));
    assert!(descriptor.summary_locales["zh-CN"].contains("不提供预付余额"));
    let spends = report["spend"].as_array().unwrap();
    let effective = spends
        .iter()
        .find(|row| row["x_effective"] == true)
        .unwrap();
    assert_eq!(effective["used"], "31402.5");
    assert_eq!(effective["limit"], "50000");
    assert!(
        spends
            .iter()
            .find(|row| row["x_effective"] == false)
            .unwrap()["used"]
            .is_null()
    );
    let windows = report["windows"].as_array().unwrap();
    assert_eq!(
        windows.len(),
        1,
        "configured limits without an observed numerator are not usage windows"
    );
    assert!((windows[0]["used_percent"].as_f64().unwrap() - 62.805).abs() < 0.000001);
    assert!(
        windows[0]["resets_at"].is_null(),
        "no reset date is inferred from the period"
    );
    assert!(report.get("balances").is_none());
    assert!(report.get("credential_verified").is_none());
    assert_eq!(client.requests.lock().unwrap().len(), 4);
}
#[tokio::test]
async fn no_admin_key_reads_no_organization_or_model_endpoint() {
    let (source, client) = source(answers());
    assert_eq!(source.read_facts(ask(false)).await.unwrap().0, json!({}));
    assert!(client.requests.lock().unwrap().is_empty());
    let descriptor = source.describe();
    assert_eq!(descriptor.check(), Ok(()));
    assert_eq!(
        descriptor.unavailable_members(&[]),
        [FactMember::Identity, FactMember::Spend, FactMember::Windows]
    );
    assert!(!descriptor.reports().contains(&FactMember::Balances));
}
#[tokio::test]
async fn early_access_refusals_keep_the_independent_cost_and_identity_observation() {
    let mut answers = answers();
    answers.remove("/v1/organizations/spend_limits");
    answers.remove("/v1/organizations/spend_limits/effective");
    let (source, _) = source(answers);
    let report = source.read_facts(ask(true)).await.unwrap().0;
    assert_eq!(report["identity"], "Example organization");
    assert_eq!(report["spend"][0]["used"], "123.78912");
    assert!(
        report.get("windows").is_none(),
        "a refusal cannot clear an earlier window observation"
    );
}
#[tokio::test]
async fn a_missing_later_page_reports_its_failure_without_claiming_partial_costs() {
    let mut answers = answers();
    answers.get_mut("/v1/organizations/cost_report").unwrap()["has_more"] = json!(true);
    answers.get_mut("/v1/organizations/cost_report").unwrap()["next_page"] = json!("next-test");
    answers.remove("/v1/organizations/spend_limits");
    answers.remove("/v1/organizations/spend_limits/effective");
    let (source, client) = source(answers);
    let error = source.read_facts(ask(true)).await.unwrap_err();
    assert_eq!(error.code, orch_provider_contract::SourceErrorCode::NotTold);
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.query.contains(&("page".into(), "next-test".into())))
    );
}
#[tokio::test]
async fn future_periods_and_unlimited_effective_rows_never_become_zero_percent_windows() {
    let mut answers = answers();
    answers
        .get_mut("/v1/organizations/spend_limits/effective")
        .unwrap()["data"] = json!([
        {"scope":{"type":"user","user_id":"unlimited-test"},"amount":null,"currency":"USD","period":"monthly","period_to_date_spend":"0"},
        {"scope":{"type":"future"},"amount":"100","currency":"USD","period":"later","period_to_date_spend":"1"}
    ]);
    let (source, _) = source(answers);
    assert_eq!(
        source.read_facts(ask(true)).await.unwrap().0["windows"],
        json!([])
    );
}
#[test]
fn only_the_documented_exact_origin_is_selected() {
    let entry = builtin();
    assert!((entry.matches)(
        &Url::parse("https://api.anthropic.com").unwrap()
    ));
    for address in [
        "http://api.anthropic.com",
        "https://api.anthropic.com.evil.test",
        "https://user@api.anthropic.com",
        "https://api.anthropic.com:444",
        "https://api.anthropic.com/other",
    ] {
        assert!(!(entry.matches)(&Url::parse(address).unwrap()));
    }
}
