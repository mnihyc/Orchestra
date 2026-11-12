//! Separate provider identities use independent owned homes even when they share a native profile.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod common;
use async_trait::async_trait;
use common::Rig;
use orch_adapter_fake::FakeAdapter;
use orch_agent_contract::{
    adapter::{AdapterError, AgentAdapter, AgentStart as AdapterStart, AgentStarted, EventSink},
    descriptor::AgentDescriptor,
    session::{SessionOpened, SessionSpec, ThreadId, TurnStart},
};
use orch_connector_core::harnesses::AdapterRegistry;
use orch_link_contract::commands::{AgentStart, SerialKey};
use orch_types::new_id;
use serde_json::json;
use std::sync::{Arc, Mutex};
struct Capture {
    inner: FakeAdapter,
    starts: Arc<Mutex<Vec<AdapterStart>>>,
}
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
#[tokio::test]
async fn a_new_provider_identity_with_an_owned_home_cannot_read_the_profiles_existing_sign_in_directory()
 {
    let mut rig = Rig::new();
    rig.roles
        .retain(|role| *role != orch_link_contract::session::Role::Environment);
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("existing-account");
    std::fs::create_dir(&original).unwrap();
    std::fs::write(original.join("credential"), "synthetic original sign-in").unwrap();
    rig.config.environment.roots = vec![root.path().display().to_string()];
    let starts: Arc<Mutex<Vec<AdapterStart>>> = Arc::default();
    let captured = Arc::clone(&starts);
    let mut registration = common::fake_registration();
    registration.factory = Arc::new(move |clock| {
        Box::new(Capture {
            inner: FakeAdapter::new(clock),
            starts: Arc::clone(&captured),
        })
    });
    rig.adapters = AdapterRegistry::new();
    rig.adapters.register(registration).unwrap();
    rig.start().await;
    let old:AgentStart=serde_json::from_value(json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake","home":null,
        "profile":{"config":{},"env":{},"attach":{"home":{"kind":"existing_directory","path":original.display().to_string()}}}})).unwrap();
    let original_started =
        common::existing_directory_result(rig.send(&rig.harness_key(), &old).await).is_some();
    let mut copy = old.clone();
    copy.provider_id = new_id();
    copy.runtime = Some(
        serde_json::from_value(json!({"variables":{},"home":{"kind":"made_by_orch"}})).unwrap(),
    );
    let key = SerialKey::Harness {
        provider_id: copy.provider_id,
        agent_id: copy.agent_id,
    };
    rig.succeed(&key, &copy).await;
    let homes = starts
        .lock()
        .unwrap()
        .iter()
        .map(|start| start.home.clone().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(homes.len(), if original_started { 2 } else { 1 });
    if original_started {
        assert_eq!(std::path::Path::new(&homes[0]), original.as_path());
        assert_ne!(homes[0], homes[1]);
    }
    let fresh = std::path::Path::new(homes.last().unwrap());
    assert!(fresh.starts_with(rig.data.path()));
    assert!(!fresh.join("credential").exists());
    assert_eq!(
        std::fs::read_to_string(original.join("credential")).unwrap(),
        "synthetic original sign-in"
    );
    assert!(
        rig.connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .process(copy.provider_id, copy.agent_id)
            .unwrap()
            .home_owned()
    );
    if original_started {
        assert!(
            !rig.connector
                .as_ref()
                .unwrap()
                .hosting()
                .unwrap()
                .process(old.provider_id, old.agent_id)
                .unwrap()
                .home_owned()
        );
    }
    rig.kill().await;
}
