//! A chosen source supplies facts, suggestions and resets beside the key or native keeper, without fallback or credential leakage.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::model::ProviderRig;
use orch_connector_core::sources::Programs;
use orch_link_contract::commands::{
    ProviderDiscoverModels, ProviderLoginStart, ProviderRefresh, ProviderResetConsume, SerialKey,
};
use orch_provider_contract::source::{
    SourceCredential, SourceDescriptorFacts, SourceDescriptorModels,
};
use orch_provider_contract::{
    CredentialPresenter, FactMember, FactsReport, FactsSource, ModelsReport, ModelsSource,
    PresentAsk, Presented, ResetReport, SourceAsk, SourceBy, SourceDescriptor, SourceError,
    SourceErrorCode, SourceSpec,
};
use orch_types::{Json, new_id};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

struct State {
    facts: Json,
    models: Option<Vec<Json>>,
    error: Option<SourceError>,
    asks: Vec<SourceAsk>,
    resets: Vec<orch_provider_contract::source::ResetRequest>,
    spent: BTreeSet<String>,
}

struct Acme {
    state: Mutex<State>,
    wants_key: bool,
}
impl Acme {
    fn new(wants_key: bool) -> Self {
        Self {
            wants_key,
            state: Mutex::new(State {
                facts: json!({"plan":"Acme"}),
                models: None,
                error: None,
                asks: Vec::new(),
                resets: Vec::new(),
                spent: BTreeSet::new(),
            }),
        }
    }

    fn descriptor(&self) -> SourceDescriptor {
        let mut descriptor =
            SourceDescriptor::new("acme", "Acme", "The provider's registered account source.");
        descriptor.facts = Some(SourceDescriptorFacts {
            reports: vec![
                FactMember::Plan,
                FactMember::Windows,
                FactMember::OrdinaryUsageAllowed,
                FactMember::LimitReached,
                FactMember::AvailableResets,
                FactMember::Credential,
                FactMember::CredentialVerified,
            ],
            spends_resets: true,
            credential: if self.wants_key {
                SourceCredential::Key
            } else {
                SourceCredential::None
            },
            ..SourceDescriptorFacts::default()
        });
        descriptor.models = Some(SourceDescriptorModels::default());
        descriptor
    }
}

#[async_trait]
impl FactsSource for Acme {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }
    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        let mut state = self.state.lock().unwrap();
        state.asks.push(ask);
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        Ok(FactsReport(state.facts.clone()))
    }
    async fn spend_reset(
        &self,
        ask: SourceAsk,
        request: orch_provider_contract::source::ResetRequest,
    ) -> Result<ResetReport, SourceError> {
        let mut state = self.state.lock().unwrap();
        state.asks.push(ask);
        state.spent.insert(request.idempotency_key.clone());
        state.resets.push(request);
        // A confirmed outcome is valid without a second facts report.
        Ok(ResetReport {
            outcome: "Reset accepted by Acme".to_owned(),
            facts: None,
        })
    }
}

#[async_trait]
impl ModelsSource for Acme {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }
    async fn list_models(&self, ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        let mut state = self.state.lock().unwrap();
        state.asks.push(ask);
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        Ok(ModelsReport {
            models: state.models.clone(),
        })
    }
}

#[async_trait]
impl CredentialPresenter for Acme {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }
    async fn present(&self, _ask: PresentAsk) -> Result<Presented, SourceError> {
        Err(SourceError::unsupported("credential.present"))
    }
}

async fn rig(program: Arc<Acme>) -> ProviderRig {
    let mut programs = Programs::default();
    programs.register("acme", program).unwrap();
    let programs = Arc::new(programs);
    ProviderRig::with(move |rig, _| {
        rig.provider_sources = Some(programs);
    })
    .await
}

fn selected(p: &ProviderRig) -> SourceSpec {
    SourceSpec {
        by: SourceBy::Program,
        name: Some("acme".to_owned()),
        provider: Some(orch_provider_contract::source::SourceAskProvider {
            id: p.rig.provider,
            name: "Team account".to_owned(),
        }),
        ..SourceSpec::default()
    }
}

async fn refresh(p: &ProviderRig, facts: SourceSpec) -> Json {
    p.rig
        .succeed(
            &SerialKey::Provider(p.rig.provider),
            &ProviderRefresh {
                provider_id: p.rig.provider,
                retry: None,
                renew: None,
                facts: Some(facts),
            },
        )
        .await
}

#[tokio::test]
async fn a_keys_selected_plan_can_hold_work_and_only_an_authenticated_read_can_verify_its_key() {
    let program = Arc::new(Acme::new(false));
    program.state.lock().unwrap().facts = json!({"plan":"Acme","ordinary_usage_allowed":false,"credential_verified":true,
        "windows":[{"id":"week","label":"Weekly","used_percent":130}]});
    let p = rig(Arc::clone(&program)).await;
    p.start_responses_provider().await;
    let read = refresh(&p, selected(&p)).await;
    assert_eq!(read["status"], "exhausted");
    assert_eq!(read["facts"]["windows"][0]["used_percent"], 130.0);
    assert_eq!(read["facts"]["credential"]["yields"], "key");
    assert!(read["facts"]["credential_verified"].is_null());
    assert_eq!(
        read["facts"]["field_times"]["plan"],
        json!(p.rig.clock.time())
    );
    assert_eq!(read["source"]["descriptor"]["title"], "Acme");
    let state = program.state.lock().unwrap();
    assert_eq!(state.asks[0].provider.name, "Team account");
    assert!(state.asks[0].credential.is_none());
}

#[tokio::test]
async fn a_key_is_given_only_to_a_program_that_declares_it_and_its_error_never_echoes_the_key() {
    let program = Arc::new(Acme::new(true));
    let p = rig(Arc::clone(&program)).await;
    p.start_responses_provider().await;
    program.state.lock().unwrap().facts =
        json!({"credential_verified":true,"ordinary_usage_allowed":true});
    let read = refresh(&p, selected(&p)).await;
    assert_eq!(read["status"], "ready");
    assert_eq!(read["facts"]["credential_verified"], true);
    assert_eq!(
        program.state.lock().unwrap().asks[0]
            .credential
            .as_ref()
            .unwrap()
            .key
            .expose(),
        p.key
    );
    program.state.lock().unwrap().error = Some(SourceError::new(
        SourceErrorCode::Refused,
        format!("Acme refused {}", p.key),
    ));
    let failed = refresh(&p, selected(&p)).await;
    assert_eq!(failed["status"], "not_told");
    assert!(failed["facts"].is_null());
    assert_eq!(failed["source"]["descriptor"]["name"], "acme");
    assert_eq!(failed["source"]["error"]["code"], "refused");
    assert!(!failed.to_string().contains(&p.key));
}

#[tokio::test]
async fn a_key_provider_stays_ready_after_a_turn_even_when_its_harness_has_no_native_sign_in() {
    let program = Arc::new(Acme::new(true));
    program.state.lock().unwrap().facts =
        json!({"credential_verified":true,"ordinary_usage_allowed":true});
    let p = rig(Arc::clone(&program)).await;
    p.start_harness().await; // Same provider ID, native-capable harness, deliberately unsigned in.
    p.start_responses_provider().await;
    let before = refresh(&p, selected(&p)).await;
    assert_eq!(before["status"], "ready");
    assert_eq!(before["facts"]["credential"]["yields"], "key");
    p.open_lease(
        p.thread,
        &[common::model::MODEL],
        Some(common::model::RESPONSES),
        None,
    )
    .await;
    p.open_forwarded_session(common::model::RESPONSES, common::model::MODEL)
        .await;
    p.mock.queue("say API key still works").unwrap();
    p.start_turn_for("read through the API key", common::model::MODEL)
        .await;
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;
    let after = refresh(&p, selected(&p)).await;
    assert_eq!(after["status"], "ready");
    assert_eq!(after["facts"]["credential_verified"], true);
    assert_eq!(after["facts"]["credential"]["yields"], "key");
    assert!(!after.to_string().contains(&p.key));
}

#[tokio::test]
async fn a_custom_plan_still_reads_the_native_keeper_and_never_receives_or_replaces_its_sign_in() {
    let program = Arc::new(Acme::new(true));
    program.state.lock().unwrap().facts = json!({"identity":"forged","plan":"Acme","ordinary_usage_allowed":false,
        "credential_verified":true,"credential":{"method":"forged","yields":"key","expires_at":"2027-01-01T00:00:00Z"}});
    let p = rig(Arc::clone(&program)).await;
    p.start_harness().await;
    let login: ProviderLoginStart =
        serde_json::from_value(json!({"provider_id":p.rig.provider,"agent_id":p.rig.agent,
        "method":"fake_import","values":{"credential_text":"made-up-native-sign-in"},"config":{}}))
        .unwrap();
    p.rig.succeed(&p.rig.harness_key(), &login).await;
    let read = refresh(&p, selected(&p)).await;
    assert_eq!(read["status"], "exhausted");
    assert_eq!(read["facts"]["plan"], "Acme");
    assert_ne!(read["facts"]["identity"], "forged");
    assert!(read["facts"]["identity"].is_string());
    assert_eq!(read["facts"]["credential"]["method"], "fake_import");
    assert_ne!(read["facts"]["credential"]["yields"], "key");
    assert!(read["facts"]["credential"]["expires_at"].is_null());
    assert!(read["facts"]["credential_verified"].is_null());
    assert!(program.state.lock().unwrap().asks[0].credential.is_none());
    assert!(!read.to_string().contains("made-up-native-sign-in"));
}

#[tokio::test]
async fn a_partial_report_keeps_its_valid_admission_and_the_source_names_the_omitted_member() {
    let program = Arc::new(Acme::new(false));
    program.state.lock().unwrap().facts = json!({"plan":42,"ordinary_usage_allowed":false});
    let p = rig(program).await;
    p.start_responses_provider().await;
    let read = refresh(&p, selected(&p)).await;
    assert_eq!(read["status"], "exhausted");
    assert!(read["facts"]["plan"].is_null());
    assert_eq!(read["source"]["supported"], true);
    assert!(
        read["source"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("plan")
    );
    assert!(read["facts"]["field_times"].get("plan").is_none());
}

#[tokio::test]
async fn model_suggestions_distinguish_unlisted_empty_and_a_new_model_without_accepting_it() {
    let program = Arc::new(Acme::new(false));
    let p = rig(Arc::clone(&program)).await;
    p.start_responses_provider().await;
    let command = ProviderDiscoverModels {
        provider_id: p.rig.provider,
        models: Some(selected(&p)),
    };
    let unlisted = p.rig.succeed(&p.provider_key(), &command).await;
    assert_eq!(unlisted["source"]["supported"], false);
    program.state.lock().unwrap().models = Some(Vec::new());
    let empty = p.rig.succeed(&p.provider_key(), &command).await;
    assert_eq!(empty["source"]["supported"], true);
    assert_eq!(empty["suggestions"], json!([]));
    program.state.lock().unwrap().models = Some(vec![json!({"model_name":"acme-new"})]);
    let listed = p.rig.succeed(&p.provider_key(), &command).await;
    assert_eq!(listed["suggestions"][0]["model_name"], "acme-new");
    assert_eq!(listed["suggestions"][0]["display_name"], "acme-new");
    assert!(
        listed.get("accepted").is_none(),
        "a model read only returns suggestions"
    );
}

#[tokio::test]
async fn reset_deliveries_keep_one_source_and_one_vendor_key_and_a_receipt_needs_no_facts() {
    let program = Arc::new(Acme::new(false));
    let p = rig(Arc::clone(&program)).await;
    p.start_responses_provider().await;
    let command = ProviderResetConsume {
        provider_id: p.rig.provider,
        request_id: new_id(),
        reset_id: "one-credit".to_owned(),
        available_count: 1,
        facts: Some(selected(&p)),
    };
    for _ in 0..2 {
        let receipt = p.rig.succeed(&p.provider_key(), &command).await;
        assert_eq!(receipt["outcome"], "Reset accepted by Acme");
        assert!(receipt["source"]["error"].is_null());
        assert!(receipt["facts"]["field_times"].get("windows").is_none());
    }
    let state = program.state.lock().unwrap();
    assert_eq!(state.resets.len(), 2);
    assert_eq!(state.resets[0], state.resets[1]);
    assert_eq!(state.spent.len(), 1);
}

#[tokio::test]
async fn a_missing_selected_program_never_falls_back_and_none_makes_no_source_call() {
    let program = Arc::new(Acme::new(false));
    let p = rig(Arc::clone(&program)).await;
    p.start_responses_provider().await;
    let mut missing = selected(&p);
    missing.name = Some("absent".to_owned());
    let read = refresh(&p, missing).await;
    assert_eq!(read["status"], "not_told");
    assert_eq!(read["source"]["error"]["code"], "unavailable");
    let none = refresh(
        &p,
        SourceSpec {
            by: SourceBy::None,
            ..SourceSpec::default()
        },
    )
    .await;
    assert_eq!(none["status"], "ready");
    assert_eq!(none["source"]["supported"], false);
    assert!(program.state.lock().unwrap().asks.is_empty());
}

#[tokio::test]
async fn revoking_the_servers_http_permission_does_not_reuse_an_opened_address_binding() {
    use axum::{Json as HttpJson, Router, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Serving(tokio::task::JoinHandle<()>);
    impl Drop for Serving {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/",
        post(move |HttpJson(request): HttpJson<Json>| {
            let seen = Arc::clone(&seen);
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                let reply = if request["method"] == "describe" {
                    let mut descriptor = SourceDescriptor::new(
                        "internal",
                        "Internal details",
                        "An internal source.",
                    );
                    descriptor.facts = Some(SourceDescriptorFacts {
                        reports: vec![FactMember::Plan],
                        ..Default::default()
                    });
                    serde_json::to_value(descriptor).unwrap()
                } else {
                    json!({"plan":"Acme"})
                };
                HttpJson(json!({"result":reply}))
            }
        }),
    );
    let _serving = Serving(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let permitted = url.clone();
    let p = ProviderRig::with(move |rig, _| rig.config.provider.upstreams.push(permitted)).await;
    p.start_responses_provider().await;
    let mut source = SourceSpec {
        by: SourceBy::Address,
        allow_http: true,
        address: Some(orch_provider_contract::SourceAddress {
            url,
            auth: orch_provider_contract::AddressAuth::None,
            header: None,
            send_credential: false,
        }),
        ..SourceSpec::default()
    };
    assert_eq!(refresh(&p, source.clone()).await["facts"]["plan"], "Acme");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    source.allow_http = false;
    let refused = refresh(&p, source).await;
    assert_eq!(refused["status"], "not_told");
    assert_eq!(refused["source"]["error"]["code"], "refused");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "no old grant can send a later HTTP request"
    );
}
