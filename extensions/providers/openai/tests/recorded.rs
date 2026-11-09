//! Sanitized Costs API shape: buckets and pagination as observed, with synthetic amounts and groups.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use async_trait::async_trait;
use orch_plugin_contract::Secret;
use orch_provider_contract::builtins::VendorClient;
use orch_provider_contract::source::{SourceAskCredential, SourceAskEndpoint, SourceAskProvider};
use orch_provider_contract::{FactMember, FactsSource, SourceAsk};
use orch_provider_openai::{OpenAi, builtin};
use orch_types::{Json, ManualClock, Time, new_id};
use serde_json::json;
use std::sync::{Arc, Mutex};
use url::Url;

struct Recorded {
    pages: Vec<Json>,
    asked: Mutex<Vec<Url>>,
}
#[async_trait]
impl VendorClient for Recorded {
    async fn get(&self, base: &Url, path: &str, key: &Secret, bound: usize) -> Option<Json> {
        assert_eq!(base.as_str(), "https://api.openai.com/v1");
        let request = base.join(path).unwrap();
        assert_eq!(request.path(), "/v1/organization/costs");
        assert_eq!(bound, orch_provider_contract::limits::REPORT_MAX_BYTES);
        assert_eq!(
            key.expose(),
            "synthetic-admin",
            "the model key cannot read the admin endpoint"
        );
        let mut asked = self.asked.lock().unwrap();
        let reply = self.pages.get(asked.len()).cloned();
        asked.push(request);
        reply
    }
}
fn ask(admin: bool) -> SourceAsk {
    SourceAsk {
        provider: SourceAskProvider {
            id: new_id(),
            name: "Test organization".into(),
        },
        endpoints: vec![SourceAskEndpoint {
            protocol: "openai-responses".into(),
            base_url: "https://api.openai.com/v1".into(),
        }],
        config: Default::default(),
        secrets: if admin {
            [("admin_key".into(), Secret::new("synthetic-admin"))].into()
        } else {
            Default::default()
        },
        credential: Some(SourceAskCredential {
            key: Secret::new("synthetic-model-key"),
        }),
    }
}
fn source(pages: Vec<Json>) -> (OpenAi, Arc<Recorded>) {
    let client = Arc::new(Recorded {
        pages,
        asked: Mutex::new(Vec::new()),
    });
    let at: Time = "2026-11-07T12:00:00Z".parse().unwrap();
    (
        OpenAi::new(client.clone(), Arc::new(ManualClock::new(at.get()))),
        client,
    )
}
fn page(more: bool, cursor: Json) -> Json {
    serde_json::from_str(&format!(r#"{{"object":"page","data":[{{"object":"bucket","start_time":1794009600,"end_time":1794096000,"results":[{{"object":"organization.costs.result","amount":{{"value":1.234567890123456789,"currency":"usd"}},"project_id":"project-test","line_item":"Generation, input"}}]}}],"has_more":{more},"next_page":{cursor}}}"#)).unwrap()
}
#[tokio::test]
async fn daily_costs_keep_decimal_precision_group_identity_and_each_page_cursor() {
    let (source, client) = source(vec![
        page(true, json!("opaque+/=")),
        json!({"object":"page","data":[],"has_more":false,"next_page":null}),
    ]);
    let report = source.read_facts(ask(true)).await.unwrap().0;
    assert_eq!(report["spend"][0]["used"], "1.234567890123456789");
    assert_eq!(report["spend"][0]["period"], "day");
    assert_eq!(report["spend"][0]["unit"], "USD");
    assert_eq!(report["spend"][0]["x_project_id"], "project-test");
    assert_eq!(report["spend"][0]["x_line_item"], "Generation, input");
    assert!(report.get("balances").is_none());
    assert!(
        report.get("credential_verified").is_none(),
        "the admin read never verifies the model key"
    );
    let asked = client.asked.lock().unwrap();
    assert_eq!(asked.len(), 2);
    assert!(
        asked[0]
            .query_pairs()
            .any(|(key, value)| key == "group_by" && value == "project_id")
    );
    assert!(
        asked[0]
            .query_pairs()
            .any(|(key, value)| key == "group_by" && value == "line_item")
    );
    assert!(!asked[0].query_pairs().any(|(name, _)| name == "page"));
    assert!(
        asked[1]
            .query_pairs()
            .any(|(key, value)| key == "page" && value == "opaque+/=")
    );
}
#[tokio::test]
async fn no_admin_key_makes_no_cost_or_model_key_request() {
    let (source, client) = source(vec![]);
    assert_eq!(source.read_facts(ask(false)).await.unwrap().0, json!({}));
    assert!(client.asked.lock().unwrap().is_empty());
    let descriptor = source.describe();
    assert_eq!(descriptor.check(), Ok(()));
    assert_eq!(descriptor.reports(), [FactMember::Spend]);
    assert_eq!(
        descriptor.facts.unwrap().admin_credential.unwrap().reports,
        [FactMember::Spend]
    );
}
#[tokio::test]
async fn an_incomplete_page_sequence_does_not_replace_spend_with_a_partial_total() {
    let (source, client) = source(vec![page(true, json!("again")), page(true, json!("again"))]);
    assert!(source.read_facts(ask(true)).await.is_err());
    assert_eq!(client.asked.lock().unwrap().len(), 2);
}
#[test]
fn only_the_documented_exact_origin_is_selected() {
    let entry = builtin();
    assert!((entry.matches)(
        &Url::parse("https://api.openai.com/v1").unwrap()
    ));
    for address in [
        "http://api.openai.com/v1",
        "https://api.openai.com.evil.test/v1",
        "https://user@api.openai.com/v1",
        "https://api.openai.com:444/v1",
        "https://api.openai.com/other",
        "https://api.openai.com/v1?key=x",
    ] {
        assert!(!(entry.matches)(&Url::parse(address).unwrap()));
    }
}
