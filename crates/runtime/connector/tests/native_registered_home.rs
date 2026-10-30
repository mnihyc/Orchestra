//! A registered native account stays in its harness user's home; a native-capable API consumer is not its keeper.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::Rig;
use common::model::{MODEL, ProviderRig, RESPONSES, files_holding};
use orch_adapter_fake::FakeAdapter;
use orch_agent_contract::adapter::{
    AdapterError, AgentAdapter, AgentStart as AdapterStart, AgentStarted, EventSink,
};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::facts::ProviderFacts;
use orch_agent_contract::login::LoginRequest;
use orch_agent_contract::session::{SessionOpened, SessionSpec, ThreadId, TurnStart};
use orch_connector_core::harnesses::{AdapterRegistry, HomeRule};
use orch_link_contract::commands::{
    AgentStart, ProviderDiscoverModels, ProviderRefresh, SerialKey,
};
use orch_types::{Clock, new_id};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const EXISTING_LOGIN: &str = "a-native-login-already-in-the-users-home";

/// Fake harness equivalent of reading its own saved login during startup. No controller login request,
/// user HOME change, vendor process or real credential file is involved.
struct NativeHomeFake {
    inner: FakeAdapter,
    clock: Arc<dyn Clock>,
    user_home: PathBuf,
    starts: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl AgentAdapter for NativeHomeFake {
    fn kind(&self) -> &'static str {
        "fake"
    }
    async fn start(
        &mut self,
        mut start: AdapterStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        self.starts.lock().unwrap().push(start.home.clone());
        let home = start
            .home
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.user_home.clone());
        let login = match std::fs::read_to_string(home.join("native-login")) {
            Ok(login) => Some(login),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                return Err(AdapterError::Internal(
                    "the fake harness could not read its own saved login".into(),
                ));
            }
        };
        start.home = Some(home.display().to_string());
        let started = self.inner.start(start, sink).await?;
        if let Some(login) = login {
            // Restoring the fake's in-memory account stands for the native CLI opening its own file.
            let request: LoginRequest = serde_json::from_value(
                json!({"method":"fake_import","values":{"credential_text":login},"config":{}}),
            )
            .unwrap();
            self.inner.login(request).await?;
        }
        Ok(started)
    }
    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.inner.stop().await
    }
    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        self.inner.probe().await
    }
    async fn provider_facts(&mut self) -> Result<ProviderFacts, AdapterError> {
        let mut facts = self.inner.provider_facts().await?;
        facts.credential_verified = Some(true);
        facts
            .field_times
            .insert("credential_verified".into(), self.clock.time());
        Ok(facts)
    }
    async fn discover_models(&mut self) -> Result<Vec<ModelSuggestion>, AdapterError> {
        self.inner.discover_models().await
    }
    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        self.inner.open_session(spec).await
    }
    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        self.inner.close_session(thread).await
    }
    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        self.inner.start_turn(turn).await
    }
}

#[tokio::test]
async fn an_existing_native_login_is_read_from_its_users_home_and_a_wrong_private_home_is_reconciled_before_a_turn()
 {
    let mut rig = Rig::new();
    rig.config.provider.signin = false;
    let user_home = tempfile::tempdir().unwrap();
    std::fs::write(user_home.path().join("native-login"), EXISTING_LOGIN).unwrap();
    let starts: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let recorded = Arc::clone(&starts);
    let native_home = user_home.path().to_owned();
    let mut registration = common::fake_registration();
    registration.home_rule = HomeRule::Harness;
    registration.factory = Arc::new(move |clock| {
        Box::new(NativeHomeFake {
            inner: FakeAdapter::new(Arc::clone(&clock)),
            clock,
            user_home: native_home.clone(),
            starts: Arc::clone(&recorded),
        })
    });
    rig.adapters = AdapterRegistry::new();
    rig.adapters.register(registration).unwrap();
    rig.start().await;
    let mut start:AgentStart=serde_json::from_value(json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake",
        "profile":{"config":{},"env":{}},"home":null,"runtime":{"variables":{},"home":{"kind":"made_by_orch"}}})).unwrap();
    rig.succeed(&rig.harness_key(), &start).await;
    let refresh: ProviderRefresh =
        serde_json::from_value(json!({"provider_id":rig.provider})).unwrap();
    let wrong = rig
        .succeed(&SerialKey::Provider(rig.provider), &refresh)
        .await;
    assert_eq!(
        wrong["status"], "needs_setup",
        "the previous forced-private-home bug is reproduced"
    );
    assert!(wrong["facts"]["plan"].is_null());
    assert!(wrong["facts"]["credential_verified"].is_null());
    start.runtime.as_mut().unwrap().home = None;
    rig.succeed(&rig.harness_key(), &start).await;
    assert!(
        starts.lock().unwrap()[0]
            .as_ref()
            .is_some_and(|home| std::path::Path::new(home).starts_with(rig.data.path()))
    );
    assert_eq!(
        starts.lock().unwrap().last().unwrap(),
        &None,
        "an unset choice must reach the harness unchanged"
    );
    assert!(
        !rig.connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .process(rig.provider, rig.agent)
            .unwrap()
            .home_owned()
    );
    let ready = rig
        .succeed(&SerialKey::Provider(rig.provider), &refresh)
        .await;
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["facts"]["credential_verified"], true);
    assert_eq!(ready["facts"]["plan"], "pro");
    assert!(!ready["facts"]["windows"].as_array().unwrap().is_empty());
    let models: ProviderDiscoverModels =
        serde_json::from_value(json!({"provider_id":rig.provider})).unwrap();
    let models = rig
        .succeed(&SerialKey::Provider(rig.provider), &models)
        .await;
    assert!(!models["suggestions"].as_array().unwrap().is_empty());
    let thread = new_id();
    let segment = new_id();
    let work = tempfile::tempdir().unwrap();
    rig.open_session(rig.open_body(thread, segment, &work.path().display().to_string()))
        .await;
    rig.start_turn(thread, "#!fake\nsay native-home-works")
        .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    assert!(
        rig.applied(thread, segment)
            .iter()
            .any(|event| event.kind == "turn.completed" && event.body["status"] == "completed"),
        "the native turn must succeed, not merely end"
    );
    assert_eq!(
        std::fs::read_to_string(user_home.path().join("native-login")).unwrap(),
        EXISTING_LOGIN
    );
    assert!(
        files_holding(rig.data.path(), EXISTING_LOGIN).is_empty(),
        "the native login never entered the connector's own store"
    );
    let still_ready = rig
        .succeed(&SerialKey::Provider(rig.provider), &refresh)
        .await;
    assert_eq!(still_ready["status"], "ready");
    assert_eq!(
        still_ready["facts"]["windows"][0]["used_percent"], 5.0,
        "the harness counted its completed native turn"
    );
    rig.kill().await;
}

#[tokio::test]
async fn an_api_providers_native_capable_consumer_never_replaces_its_key_keeper_before_or_after_a_forwarded_turn()
 {
    let mut p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.start_harness().await;
    let refresh: ProviderRefresh =
        serde_json::from_value(json!({"provider_id":p.rig.provider})).unwrap();
    for after_turn in [false, true] {
        if after_turn {
            p.open_forwarded_session(RESPONSES, MODEL).await;
            // A forwarded call is carried only under the lease the system opens for its turn.
            p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
                .await;
            p.start_turn_for("say api-home-works", MODEL).await;
            p.rig
                .wait_for(p.thread, p.segment, "turn.completed", 1)
                .await;
            assert!(
                p.rig
                    .applied(p.thread, p.segment)
                    .iter()
                    .any(|event| event.kind == "turn.completed"
                        && event.body["status"] == "completed"),
                "{:?}",
                p.rig.applied(p.thread, p.segment)
            );
            assert_eq!(
                p.model_requests().len(),
                1,
                "the forwarded turn actually used the key's endpoint"
            );
        }
        let read = p
            .rig
            .succeed(&SerialKey::Provider(p.rig.provider), &refresh)
            .await;
        assert_eq!(read["status"], "ready");
        assert_eq!(read["facts"]["credential"]["yields"], "key");
    }
    p.rig.kill().await;
}
