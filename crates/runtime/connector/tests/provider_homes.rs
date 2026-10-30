//! Harness home selection preserves borrowed data, enforces roots and reports actual ownership.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[cfg(target_os = "linux")]
mod common;

#[cfg(target_os = "linux")]
use async_trait::async_trait;
#[cfg(target_os = "linux")]
use common::Rig;
#[cfg(target_os = "linux")]
use orch_adapter_fake::FakeAdapter;
#[cfg(target_os = "linux")]
use orch_agent_contract::adapter::{
    AdapterError, AgentAdapter, AgentStart as AdapterStart, AgentStarted, EventSink,
};
#[cfg(target_os = "linux")]
use orch_agent_contract::descriptor::AgentDescriptor;
#[cfg(target_os = "linux")]
use orch_agent_contract::session::{SessionOpened, SessionSpec, ThreadId, TurnStart};
#[cfg(target_os = "linux")]
use orch_connector_core::harnesses::AdapterRegistry;
#[cfg(target_os = "linux")]
use orch_link_contract::commands::{AgentStart, AgentStop};
#[cfg(target_os = "linux")]
use serde_json::json;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};

#[cfg(target_os = "linux")]
struct Capture {
    inner: FakeAdapter,
    starts: Arc<Mutex<Vec<AdapterStart>>>,
}
#[cfg(target_os = "linux")]
#[async_trait]
impl AgentAdapter for Capture {
    fn kind(&self) -> &'static str {
        "fake"
    }
    async fn start(
        &mut self,
        start: AdapterStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        self.starts.lock().unwrap().push(start.clone());
        self.inner.start(start, sink).await
    }
    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.inner.stop().await
    }
    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        self.inner.probe().await
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_runtime_home_overrides_the_profile_and_only_made_data_is_owned() {
    use std::os::unix::fs::MetadataExt;
    let mut rig = Rig::new();
    rig.roles
        .retain(|role| *role != orch_link_contract::session::Role::Environment);
    let root = tempfile::tempdir().unwrap();
    let borrowed = root.path().join("existing");
    std::fs::create_dir(&borrowed).unwrap();
    std::fs::write(borrowed.join("credential"), "kept").unwrap();
    let before = std::fs::metadata(&borrowed).unwrap();
    rig.config.environment.roots = vec![root.path().display().to_string()];
    let starts: Arc<Mutex<Vec<AdapterStart>>> = Arc::default();
    let capture = Arc::clone(&starts);
    let mut registration = common::fake_registration();
    registration.factory = Arc::new(move |clock| {
        Box::new(Capture {
            inner: FakeAdapter::new(clock),
            starts: Arc::clone(&capture),
        })
    });
    rig.adapters = AdapterRegistry::new();
    rig.adapters.register(registration).unwrap();
    rig.start().await;
    let facts = rig.connector.as_ref().unwrap().facts();
    assert!(facts.environment_descriptor.is_none());
    assert!(
        facts
            .provider_runtime_descriptor
            .unwrap()
            .data_storage
            .existing_directory
    );
    let mut request:AgentStart=serde_json::from_value(json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake","home":"unchecked-relative-override",
        "profile":{"config":{},"env":{},"attach":{"home":{"kind":"made_by_orch"}}},
        "runtime":{"variables":{},"home":{"kind":"existing_directory","path":borrowed.display().to_string()}}})).unwrap();
    rig.succeed(&rig.harness_key(), &request).await;
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
    assert_eq!(
        starts.lock().unwrap().last().unwrap().home.as_deref(),
        borrowed.to_str()
    );
    rig.succeed(
        &rig.harness_key(),
        &AgentStop {
            provider_id: rig.provider,
            agent_id: rig.agent,
            logout: None,
        },
    )
    .await;
    request.home = None;
    request.runtime.as_mut().unwrap().home =
        Some(orch_agent_contract::facts::EnvironmentDataSource::MadeByOrch);
    rig.succeed(&rig.harness_key(), &request).await;
    assert!(
        rig.connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .process(rig.provider, rig.agent)
            .unwrap()
            .home_owned()
    );
    assert!(
        std::path::Path::new(
            starts
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .home
                .as_ref()
                .unwrap()
        )
        .starts_with(rig.data.path())
    );
    let after = std::fs::metadata(&borrowed).unwrap();
    assert_eq!(
        (after.uid(), after.gid(), after.mode(), after.ino()),
        (before.uid(), before.gid(), before.mode(), before.ino())
    );
    assert_eq!(
        std::fs::read_to_string(borrowed.join("credential")).unwrap(),
        "kept"
    );
    rig.kill().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_relative_override_never_bypasses_roots_or_a_link_out_of_them() {
    let mut rig = Rig::new();
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    std::fs::write(root.path().join("file"), "not a directory").unwrap();
    rig.config.environment.roots = vec![root.path().display().to_string()];
    rig.start().await;
    for path in [
        outside.path().to_owned(),
        root.path().join("escape"),
        root.path().join("file"),
    ] {
        let request:AgentStart=serde_json::from_value(json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake","home":"harness/looks-owned",
            "profile":{"config":{},"env":{},"attach":{"home":{"kind":"existing_directory","path":path.display().to_string()}}}})).unwrap();
        assert_eq!(
            rig.fail(&rig.harness_key(), &request).await,
            "policy.root_not_allowed"
        );
    }
    assert!(
        rig.connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .process(rig.provider, rig.agent)
            .is_err()
    );
    rig.kill().await;
}

/// Stand-in for a vendor harness that already has a login in its user's configuration directory.
/// It restores that login itself; Orch sends no sign-in command and never copies the credential.
#[cfg(target_os = "linux")]
struct ExistingLogin {
    inner: FakeAdapter,
    user_home: std::path::PathBuf,
    clock: Arc<dyn orch_types::Clock>,
    starts: Arc<Mutex<Vec<AdapterStart>>>,
}

#[cfg(target_os = "linux")]
#[async_trait]
impl AgentAdapter for ExistingLogin {
    fn kind(&self) -> &'static str {
        "fake"
    }
    async fn start(
        &mut self,
        start: AdapterStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        self.starts.lock().unwrap().push(start.clone());
        let home = start
            .home
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| self.user_home.clone());
        let credential = std::fs::read_to_string(home.join(".native-login")).ok();
        let mut selected = start;
        selected.home = Some(home.display().to_string());
        let mut started = self.inner.start(selected, sink).await?;
        started
            .descriptor
            .capabilities
            .provider_facts
            .push(orch_agent_contract::descriptor::ProviderFactKind::CredentialVerification);
        if let Some(credential) = credential {
            self.inner
                .login(orch_agent_contract::login::LoginRequest {
                    method: "fake_import".into(),
                    values: orch_agent_contract::login::LoginValues(
                        std::collections::BTreeMap::from([(
                            "credential_text".into(),
                            json!(credential),
                        )]),
                    ),
                    config: json!({}),
                })
                .await?;
        }
        Ok(started)
    }
    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.inner.stop().await
    }
    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        let mut descriptor = self.inner.probe().await?;
        descriptor
            .capabilities
            .provider_facts
            .push(orch_agent_contract::descriptor::ProviderFactKind::CredentialVerification);
        Ok(descriptor)
    }
    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        self.inner.open_session(spec).await
    }
    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        self.inner.close_session(thread).await
    }
    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        self.inner.provider_facts().await?; // A turn must use the existing login, rather than merely bypass admission.
        self.inner.start_turn(turn).await
    }
    async fn provider_facts(
        &mut self,
    ) -> Result<orch_agent_contract::facts::ProviderFacts, AdapterError> {
        let mut facts = self.inner.provider_facts().await?;
        facts.credential_verified = Some(true);
        facts
            .field_times
            .insert("credential_verified".into(), self.clock.time());
        Ok(facts)
    }
    async fn discover_models(
        &mut self,
    ) -> Result<Vec<orch_agent_contract::discovery::ModelSuggestion>, AdapterError> {
        self.inner.discover_models().await
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_existing_user_login_survives_home_reconciliation_and_connector_recovery_and_runs_a_turn()
 {
    use orch_connector_core::harnesses::HomeRule;
    use orch_link_contract::commands::{ProviderRefresh, SerialKey};
    let mut rig = Rig::new();
    let user = tempfile::tempdir().unwrap();
    let home = user.path().join(".claude");
    std::fs::create_dir(&home).unwrap();
    let secret = "existing-native-login-do-not-copy";
    // This test's machine owner offers this one existing home, including when the
    // test process keeps temporary files outside the ordinary system directory.
    rig.config
        .environment
        .roots
        .push(user.path().to_string_lossy().into_owned());
    std::fs::write(home.join(".native-login"), secret).unwrap();
    let starts: Arc<Mutex<Vec<AdapterStart>>> = Arc::default();
    let capture = Arc::clone(&starts);
    let mut registration = common::fake_registration();
    registration.home_rule = HomeRule::Harness;
    registration.factory = Arc::new(move |clock| {
        Box::new(ExistingLogin {
            inner: FakeAdapter::new(Arc::clone(&clock)),
            user_home: home.clone(),
            clock,
            starts: Arc::clone(&capture),
        })
    });
    rig.adapters = AdapterRegistry::new();
    rig.adapters.register(registration).unwrap();
    rig.start().await;
    assert!(!rig.connector.as_ref().unwrap().facts().adapters[0].default_home_owned);
    let mut request:AgentStart=serde_json::from_value(json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake",
        "profile":{},"home":null,"provider_config":{},"runtime":{"variables":{},"home":{"kind":"made_by_orch"}}})).unwrap();
    rig.succeed(&rig.harness_key(), &request).await;
    let read = ProviderRefresh {
        provider_id: rig.provider,
        retry: None,
        renew: None,
        facts: None,
    };
    assert_eq!(
        rig.succeed(&SerialKey::Provider(rig.provider), &read).await["status"],
        "needs_setup",
        "the old forced private home cannot see the user's login"
    );
    request.runtime.as_mut().unwrap().home = None;
    assert_eq!(
        rig.succeed(&rig.harness_key(), &request).await["reused"],
        false
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
    assert!(
        starts.lock().unwrap().last().unwrap().home.is_none(),
        "an unset home reaches the adapter unchanged"
    );
    let ready = rig.succeed(&SerialKey::Provider(rig.provider), &read).await;
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["facts"]["credential_verified"], true);
    assert!(ready["facts"]["plan"].is_string() && ready["facts"]["windows"].is_array());
    assert!(!ready.to_string().contains(secret));
    assert_eq!(
        rig.succeed(&rig.harness_key(), &request).await["reused"],
        true
    );
    assert_eq!(
        starts.lock().unwrap().len(),
        2,
        "reconciliation reuses the correct running process"
    );
    let (thread, segment) = (orch_types::new_id(), orch_types::new_id());
    rig.open_session(rig.open_body(thread, segment, user.path().to_str().unwrap()))
        .await;
    rig.start_turn(thread, "#!fake\nsay existing login works")
        .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    assert!(
        rig.kinds(thread, segment)
            .contains(&"turn.completed".to_owned())
    );
    rig.kill().await;
    rig.start().await;
    assert_eq!(
        rig.succeed(&SerialKey::Provider(rig.provider), &read).await["status"],
        "ready"
    );
    assert_eq!(
        std::fs::read_to_string(user.path().join(".claude/.native-login")).unwrap(),
        secret
    );
    assert!(
        common::model::files_holding(rig.data.path(), secret).is_empty(),
        "the user's credential stays in the user's home"
    );
    rig.kill().await;
}
