//! A program's key presentation is used by the real forwarder, renewed at its clock, kept sealed across restart, and refused safely without changing provider status.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::model::{
    MESSAGES, MODEL, ProviderRig, RESPONSES, files_holding, messages_request, responses_request,
};
use orch_connector_core::sources::Programs;
use orch_link::commands::Progress;
use orch_link_contract::commands::{
    CompletionPurpose, ProviderComplete, ProviderStart, ProviderStop,
};
use orch_plugin_contract::Secret;
use orch_provider_contract::source::SourceDescriptorPresents;
use orch_provider_contract::{
    CredentialPresenter, FactsReport, FactsSource, ModelsReport, ModelsSource, PresentAsk,
    Presented, SourceAsk, SourceDescriptor, SourceError, SourceErrorCode,
};
use orch_testkit::doubles::model::Verdict;
use orch_types::{Clock, Field, FieldType, Time, new_id};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const EXCHANGE_SECRET: &str = "made-up-exchange-secret";
const UNDECLARED_SECRET: &str = "made-up-undeclared-secret";

#[derive(Clone)]
struct Settings {
    expires_in: Option<i64>,
    fail: bool,
    forbidden: bool,
}
struct Exchange {
    clock: Mutex<Option<Arc<dyn Clock>>>,
    asks: Mutex<Vec<PresentAsk>>,
    settings: Mutex<Settings>,
}
impl Exchange {
    fn new(expires_in: Option<i64>) -> Self {
        Self {
            clock: Mutex::new(None),
            asks: Mutex::new(Vec::new()),
            settings: Mutex::new(Settings {
                expires_in,
                fail: false,
                forbidden: false,
            }),
        }
    }
    fn descriptor() -> SourceDescriptor {
        let mut descriptor = SourceDescriptor::new(
            "exchange",
            "Example exchange",
            "Presents only this example's key.",
        );
        descriptor.presents = Some(SourceDescriptorPresents::default());
        descriptor.fields = vec![
            Field::new("exchange", "Exchange secret", FieldType::Secret),
            Field::new("region", "Region", FieldType::Text),
        ];
        descriptor
    }
}
#[async_trait]
impl FactsSource for Exchange {
    fn describe(&self) -> SourceDescriptor {
        Self::descriptor()
    }
    async fn read_facts(&self, _ask: SourceAsk) -> Result<FactsReport, SourceError> {
        Err(SourceError::unsupported("facts.read"))
    }
}
#[async_trait]
impl ModelsSource for Exchange {
    fn describe(&self) -> SourceDescriptor {
        Self::descriptor()
    }
    async fn list_models(&self, _ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        Err(SourceError::unsupported("models.list"))
    }
}
#[async_trait]
impl CredentialPresenter for Exchange {
    fn describe(&self) -> SourceDescriptor {
        Self::descriptor()
    }
    async fn present(&self, ask: PresentAsk) -> Result<Presented, SourceError> {
        let settings = self.settings.lock().unwrap().clone();
        let call = {
            let mut asks = self.asks.lock().unwrap();
            asks.push(ask.clone());
            asks.len()
        };
        if settings.fail {
            return Err(SourceError::new(
                SourceErrorCode::Unavailable,
                format!(
                    "The exchange refused {} using {}",
                    ask.credential.key.expose(),
                    ask.secrets.get("exchange").unwrap().expose()
                ),
            ));
        }
        let now = self.clock.lock().unwrap().as_ref().unwrap().time();
        Ok(Presented {
            headers: BTreeMap::from([
                (
                    (if settings.forbidden {
                        "host"
                    } else {
                        "authorization"
                    })
                    .to_owned(),
                    Secret::new(format!("Bearer exchanged-{call}")),
                ),
                (
                    "x-presented-token".to_owned(),
                    Secret::new(format!("auxiliary-{call}")),
                ),
            ]),
            expires_at: settings
                .expires_in
                .map(|seconds| Time::new(now.get() + chrono::Duration::seconds(seconds))),
        })
    }
}

async fn rig(exchange: Arc<Exchange>) -> ProviderRig {
    let mut programs = Programs::default();
    programs
        .register("exchange", Arc::clone(&exchange))
        .unwrap();
    let programs = Arc::new(programs);
    ProviderRig::with(move |rig, _| {
        *exchange.clock.lock().unwrap() = Some(Arc::clone(&rig.clock));
        rig.provider_sources = Some(programs);
    })
    .await
}

async fn start(p: &ProviderRig, name: &str) {
    let id = new_id();
    let body:ProviderStart=serde_json::from_value(json!({"provider_id":p.rig.provider,"protocols":[RESPONSES,MESSAGES],
        "upstreams":[
            {"protocol":RESPONSES,"base_url":p.mock.responses_base_url(),"options":{"auth":"header","auth_header":"api-key","headers":{"x-fixed":"kept"},"query":{"region":"east"}}},
            {"protocol":MESSAGES,"base_url":p.mock.messages_base_url(),"options":{}}
        ],"credential":p.sealed_key_for(id,&p.key),"secrets":{},
        "presenter":{"by":"program","name":name,"provider":{"id":p.rig.provider,"name":"Team account"},"config":{"region":"east"},
            "secrets":{"exchange":p.sealed_key_for(id,EXCHANGE_SECRET),"undeclared":p.sealed_key_for(id,UNDECLARED_SECRET)}}})).unwrap();
    assert!(matches!(
        p.command_with_id(id, &p.provider_key(), &body).await,
        Progress::Succeeded(_)
    ));
}

async fn response(p: &ProviderRig, expected: &str) -> orch_testkit::doubles::model::ClientReply {
    p.mock.set_credential(Some(expected));
    p.harness(p.thread)
        .post("/v1/responses", &responses_request(MODEL, "hello"))
        .await
}

#[tokio::test]
async fn a_presented_token_replaces_the_key_and_is_reused_until_one_minute_before_its_expiry() {
    let exchange = Arc::new(Exchange::new(Some(300)));
    let p = rig(Arc::clone(&exchange)).await;
    start(&p, "exchange").await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let (first, second) = tokio::join!(response(&p, "exchanged-1"), response(&p, "exchanged-1"));
    assert_eq!(first.status, 200);
    assert_eq!(second.status, 200);
    p.rig.manual.advance(chrono::Duration::seconds(239));
    assert_eq!(response(&p, "exchanged-1").await.status, 200);
    assert_eq!(exchange.asks.lock().unwrap().len(), 1);
    p.rig.manual.advance(chrono::Duration::seconds(1));
    assert_eq!(response(&p, "exchanged-2").await.status, 200);
    let asks = exchange.asks.lock().unwrap();
    assert_eq!(asks.len(), 2);
    assert_eq!(asks[0].credential.key.expose(), p.key);
    assert_eq!(asks[0].provider.name, "Team account");
    assert_eq!(asks[0].secrets.len(), 1);
    assert_eq!(asks[0].secrets["exchange"].expose(), EXCHANGE_SECRET);
    assert_eq!(asks[0].config["region"], "east");
    let requests = p.model_requests();
    assert!(
        requests
            .iter()
            .all(|request| request.credential.verdict == Verdict::Accepted)
    );
    assert!(
        requests
            .iter()
            .all(|request| !request.headers.contains_key("api-key")),
        "the endpoint's original key header is removed"
    );
    assert!(requests.iter().all(|request| {
        request
            .headers
            .get("x-fixed")
            .is_some_and(|value| value == "kept")
    }));
    assert!(requests.iter().all(|request| {
        request
            .query
            .as_deref()
            .is_some_and(|query| query.contains("region=east"))
    }));
}

#[tokio::test]
async fn without_an_expiry_the_token_is_reused_for_five_minutes() {
    let exchange = Arc::new(Exchange::new(None));
    let p = rig(Arc::clone(&exchange)).await;
    start(&p, "exchange").await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    assert_eq!(response(&p, "exchanged-1").await.status, 200);
    p.rig.manual.advance(chrono::Duration::seconds(299));
    assert_eq!(response(&p, "exchanged-1").await.status, 200);
    assert_eq!(exchange.asks.lock().unwrap().len(), 1);
    p.rig.manual.advance(chrono::Duration::seconds(1));
    assert_eq!(response(&p, "exchanged-2").await.status, 200);
    assert_eq!(exchange.asks.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_system_completion_uses_the_same_cached_presentation_as_a_forwarded_call() {
    let exchange = Arc::new(Exchange::new(None));
    let p = rig(Arc::clone(&exchange)).await;
    start(&p, "exchange").await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    assert_eq!(response(&p, "exchanged-1").await.status, 200);
    let result = p
        .rig
        .succeed(
            &p.provider_key(),
            &ProviderComplete {
                params: None,
                provider_id: p.rig.provider,
                purpose: CompletionPurpose::Summary,
                protocol: RESPONSES.to_owned(),
                model_name: MODEL.to_owned(),
                system: "Summarize".to_owned(),
                input: "The work is ready.".to_owned(),
                max_output_tokens: 64,
            },
        )
        .await;
    assert!(result["text"].is_string());
    assert_eq!(exchange.asks.lock().unwrap().len(), 1);
    assert!(
        p.model_requests()
            .iter()
            .all(|request| request.credential.verdict == Verdict::Accepted)
    );
}

#[tokio::test]
async fn each_endpoint_gets_its_own_presentation_and_reuses_only_that_endpoints_headers() {
    let exchange = Arc::new(Exchange::new(None));
    let p = rig(Arc::clone(&exchange)).await;
    start(&p, "exchange").await;
    p.open_lease(p.thread, &[MODEL], None, None).await;
    assert_eq!(response(&p, "exchanged-1").await.status, 200);
    p.mock.set_credential(Some("exchanged-2"));
    assert_eq!(
        p.harness(p.thread)
            .post("/v1/messages", &messages_request(MODEL, "hello"))
            .await
            .status,
        200
    );
    assert_eq!(response(&p, "exchanged-1").await.status, 200);
    let asks = exchange.asks.lock().unwrap();
    assert_eq!(asks.len(), 2);
    assert_eq!(asks[0].endpoint.protocol, RESPONSES);
    assert_eq!(asks[1].endpoint.protocol, MESSAGES);
    assert_ne!(asks[0].endpoint.base_url, asks[1].endpoint.base_url);
}

#[tokio::test]
async fn a_failed_renewal_is_a_safe_502_in_either_protocol_without_an_upstream_call_or_status_change()
 {
    let exchange = Arc::new(Exchange::new(None));
    let p = rig(Arc::clone(&exchange)).await;
    start(&p, "exchange").await;
    p.open_lease(p.thread, &[MODEL], None, None).await;
    exchange.settings.lock().unwrap().fail = true;
    for (path, body) in [
        ("/v1/responses", responses_request(MODEL, "hello")),
        ("/v1/messages", messages_request(MODEL, "hello")),
    ] {
        let reply = p.harness(p.thread).post(path, &body).await;
        assert_eq!(reply.status, 502);
        assert!(reply.json().to_string().contains("credential_unavailable"));
        let body = String::from_utf8(reply.body().to_vec()).unwrap();
        assert!(body.contains("The exchange refused"));
        assert!(!body.contains(&p.key));
        assert!(!body.contains(EXCHANGE_SECRET));
    }
    assert!(p.model_requests().is_empty());
    assert!(p.provider_events_of("provider.status").is_empty());
}

#[tokio::test]
async fn an_expired_or_connection_changing_presentation_is_refused_without_using_the_raw_key() {
    for (expires, forbidden) in [(Some(-1), false), (None, true)] {
        let exchange = Arc::new(Exchange::new(expires));
        exchange.settings.lock().unwrap().forbidden = forbidden;
        let p = rig(exchange).await;
        start(&p, "exchange").await;
        p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
            .await;
        let reply = response(&p, &p.key).await;
        assert_eq!(reply.status, 502);
        assert!(reply.json().to_string().contains("credential_unavailable"));
        assert!(p.model_requests().is_empty());
        assert!(p.provider_events_of("provider.status").is_empty());
    }
}

#[tokio::test]
async fn a_selected_program_that_is_missing_never_falls_back_to_the_key() {
    let p = rig(Arc::new(Exchange::new(None))).await;
    start(&p, "missing").await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    assert_eq!(response(&p, &p.key).await.status, 502);
    assert!(p.model_requests().is_empty());
}

#[tokio::test]
async fn the_presenter_choice_recovers_from_the_encrypted_store_and_tokens_do_not_survive_a_restart()
 {
    let exchange = Arc::new(Exchange::new(None));
    let mut p = rig(Arc::clone(&exchange)).await;
    start(&p, "exchange").await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    assert_eq!(response(&p, "exchanged-1").await.status, 200);
    for secret in [
        p.key.as_str(),
        EXCHANGE_SECRET,
        UNDECLARED_SECRET,
        "exchanged-1",
    ] {
        assert!(files_holding(p.rig.data.path(), secret).is_empty());
    }
    p.rig.kill().await;
    p.rig.start().await;
    assert_eq!(
        response(&p, "exchanged-2").await.status,
        200,
        "the recovered lease uses a new presentation without another provider.start"
    );
    assert_eq!(exchange.asks.lock().unwrap().len(), 2);
    p.rig
        .succeed(
            &p.provider_key(),
            &ProviderStop {
                provider_id: p.rig.provider,
                purge: None,
            },
        )
        .await;
    for secret in [p.key.as_str(), EXCHANGE_SECRET, UNDECLARED_SECRET] {
        assert!(files_holding(p.rig.data.path(), secret).is_empty());
    }
}
