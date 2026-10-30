//! Provider runtime variables win over profile values on every machine and remain sealed across restarts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::model::files_holding;
use common::{PATIENCE, Rig};
use orch_adapter_fake::FakeAdapter;
use orch_agent_contract::adapter::{
    AdapterError, AgentAdapter, AgentStart as AdapterStart, AgentStarted, EventSink,
};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_contract::session::{SessionOpened, SessionSpec, ThreadId, TurnStart};
use orch_connector_core::harnesses::AdapterRegistry;
use orch_link::commands::Progress;
use orch_link_contract::commands::{AgentStart, AgentStop};
use orch_pki::sealing::{RecipientKey, info_for_command, seal};
use orch_types::new_id;
use serde_json::json;
use std::sync::{Arc, Mutex};

const SECRET: &str = "runtime-secret-that-must-stay-on-its-machine";

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
async fn the_harness_receives_runtime_values_over_the_profile_and_never_writes_an_expanded_profile()
{
    for managed in [false, true] {
        let mut rig = Rig::new();
        rig.managed = managed;
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
        let command = new_id();
        let recipient = RecipientKey::from_certificate(&rig.identity.certificate);
        let sealed = seal(&recipient, &info_for_command(command), SECRET.as_bytes()).unwrap();
        let body:AgentStart=serde_json::from_value(json!({
            "provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake","home":null,
            "profile":{"config":{},"env":{"REGION":"profile","PROFILE_ONLY":"kept","SERVICE_TOKEN":"profile-token"}},
            "provider_config":{},"runtime":{"variables":{"REGION":"runtime","SERVICE_TOKEN":{"$sealed":sealed}}}
        })).unwrap();
        rig.controller
            .issue_with_id(command, &rig.harness_key(), rig.deadline(), &body);
        let result = tokio::time::timeout(PATIENCE, rig.controller.wait_command(command))
            .await
            .unwrap();
        assert!(matches!(result, Progress::Succeeded(_)), "{result:?}");
        {
            let starts = starts.lock().unwrap();
            assert_eq!(starts.last().unwrap().profile.env["REGION"], "runtime");
            assert_eq!(starts.last().unwrap().profile.env["SERVICE_TOKEN"], SECRET);
            assert_eq!(starts.last().unwrap().profile.env["PROFILE_ONLY"], "kept");
        }
        assert!(
            files_holding(rig.data.path(), SECRET).is_empty(),
            "neither the hosted record nor the restart store holds a clear secret"
        );
        let hosted = std::fs::read_to_string(rig.data.path().join("state/hosted.json")).unwrap();
        assert!(
            hosted.contains("profile-token"),
            "the requested profile is retained unchanged"
        );
        assert!(!hosted.contains(SECRET));
        let replay = new_id();
        let mut same = serde_json::to_value(&body).unwrap();
        same["runtime"]["variables"]["SERVICE_TOKEN"] = json!({"$sealed":seal(&recipient,&info_for_command(replay),SECRET.as_bytes()).unwrap()});
        let same: AgentStart = serde_json::from_value(same).unwrap();
        rig.controller
            .issue_with_id(replay, &rig.harness_key(), rig.deadline(), &same);
        assert!(matches!(
            tokio::time::timeout(PATIENCE, rig.controller.wait_command(replay))
                .await
                .unwrap(),
            Progress::Succeeded(_)
        ));
        assert_eq!(
            starts.lock().unwrap().len(),
            1,
            "fresh command-bound ciphertext for the same values never restarts the harness"
        );
        rig.kill().await;
        rig.start().await;
        assert!(
            rig.controller
                .wait_until(PATIENCE, |_| starts.lock().unwrap().len() >= 2)
                .await,
            "the retained harness starts again"
        );
        assert_eq!(
            starts.lock().unwrap().last().unwrap().profile.env["SERVICE_TOKEN"],
            SECRET
        );
        assert!(files_holding(rig.data.path(), SECRET).is_empty());
        rig.succeed(
            &rig.harness_key(),
            &AgentStop {
                provider_id: rig.provider,
                agent_id: rig.agent,
                logout: None,
            },
        )
        .await;
        assert!(
            !rig.data
                .path()
                .join("secrets")
                .join(format!("harness-runtime-{}-{}", rig.provider, rig.agent))
                .exists()
        );
        rig.kill().await;
    }
}

#[tokio::test]
async fn runtime_values_cannot_replace_connector_identity_or_use_an_unresolved_secret() {
    let mut rig = Rig::new();
    rig.start().await;
    for variables in [
        json!({"ORCH_MCP_TOKEN":"replacement"}),
        json!({"SERVICE_TOKEN":{"$secret":new_id()}}),
    ] {
        let body: AgentStart = serde_json::from_value(
            json!({"provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake",
            "profile":{"config":{},"env":{}},"home":null,"runtime":{"variables":variables}}),
        )
        .unwrap();
        let result = rig.send(&rig.harness_key(), &body).await;
        assert!(matches!(result, Progress::Failed(_)), "{result:?}");
        assert!(
            !rig.data.path().join("state/hosted.json").exists(),
            "a rejected start is never retained"
        );
    }
    rig.kill().await;
}
