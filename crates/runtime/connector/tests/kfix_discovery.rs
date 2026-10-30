//! A real harness's existing test sign-in is discovered on reconnect before any thread or model call.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::Rig;
use common::docker::{CodexDocker, Containers};
use orch_adapter_codex::CodexAdapter;
use orch_adapter_codex::process::{HarnessProcess, LaunchError, LaunchRequest, Launcher};
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_connector_core::roles::provider::{ProviderParts, plug_in};
use orch_link_contract::session::Role;
use orch_types::{Clock, SystemClock};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct TestHome {
    docker: CodexDocker,
    home: PathBuf,
}

#[async_trait]
impl Launcher for TestHome {
    async fn launch(&self, mut request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        // The harness runs only in the image and reads only the disposable test sign-in.
        request
            .env
            .insert("CODEX_HOME".to_owned(), self.home.display().to_string());
        self.docker.launch(request).await
    }
}

#[tokio::test]
async fn an_existing_sign_in_is_announced_again_after_reconnect_without_starting_a_thread() {
    let present = std::process::Command::new("docker")
        .args(["image", "inspect", "orch-dev-harness"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        present.success(),
        "the real harness image is required for this test"
    );
    let home = tempfile::tempdir().unwrap();
    // An existing test sign-in, accepted by the real harness's account/read path. It opens no live account.
    let id_token = "eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6InByb2JlQGV4YW1wbGUudGVzdCIsImh0dHBzOi8vYXBpLm9wZW5haS5jb20vYXV0aCI6eyJjaGF0Z3B0X3BsYW5fdHlwZSI6InRlYW0iLCJjaGF0Z3B0X2FjY291bnRfaWQiOiJwcm9iZS1hY2NvdW50In19.c2lnbmF0dXJl";
    std::fs::write(home.path().join("auth.json"), json!({
        "auth_mode": "chatgpt", "OPENAI_API_KEY": null,
        "tokens": {"id_token": id_token, "access_token": "test-access", "refresh_token": "test-refresh", "account_id": "probe-account"},
        "last_refresh": SystemClock.now().to_rfc3339(),
    }).to_string()).unwrap();
    let containers = Containers::new("kfix-binding-discovery");
    let home_path = home.path().to_owned();
    let launching = containers.clone();
    let mut registry = AdapterRegistry::new();
    registry
        .register(AdapterRegistration {
            kind: "codex",
            adapter_version: 1,
            builtin: true,
            needs_binary: true,
            home_rule: HomeRule::Harness,
            factory: Arc::new(move |clock| {
                Box::new(
                    CodexAdapter::with_launcher(Arc::new(TestHome {
                        docker: CodexDocker::new(launching.clone(), vec![home_path.clone()]),
                        home: home_path.clone(),
                    }))
                    .with_clock(clock),
                )
            }),
        })
        .unwrap();
    let mut rig = Rig::with_roles(vec![Role::Agent, Role::Provider]);
    rig.adapters = registry;
    rig.config.agent.harnesses =
        serde_json::from_value(json!([{"kind":"codex", "path":"codex"}])).unwrap();
    rig.plug = Some(Arc::new(|builder, roles, data, key, clock| {
        plug_in(
            builder,
            roles,
            ProviderParts::new(common::model::protocols(), data, key, clock).unwrap(),
        )
    }));
    // Account metadata may reach the harness's own server for limits; no model operation is used.
    let patience = Duration::from_secs(90);
    rig.patience = patience;
    rig.start().await;
    assert!(
        rig.controller
            .wait_until(patience, |controller| controller
                .applied("machine")
                .iter()
                .any(|event| event.kind == "provider.found"))
            .await
    );
    let first: Vec<_> = rig
        .controller
        .applied("machine")
        .into_iter()
        .filter(|event| event.kind == "provider.found")
        .collect();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].body["harness_kind"], "codex");
    assert!(
        first[0].body["identity"].is_null(),
        "the offline test sign-in reports no verified identity"
    );
    // The same connector stays running: only its transport session is replaced.
    rig.hub.faults().sever_now();
    assert!(
        rig.controller
            .wait_until(patience, |controller| controller
                .applied("machine")
                .iter()
                .filter(|event| event.kind == "provider.found")
                .count()
                == 2)
            .await
    );
    let found: Vec<_> = rig
        .controller
        .applied("machine")
        .into_iter()
        .filter(|event| event.kind == "provider.found")
        .collect();
    assert_eq!(found[0].body, found[1].body);
    assert!(
        rig.controller
            .applied(&format!("session:{}:{}", rig.provider, rig.agent))
            .is_empty()
    );
    rig.connector.take().unwrap().stop().await;
    containers.remove_all();
}
