//! An unverified native runtime refuses attachment before it changes the workspace or publishes state.
#![cfg(not(target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use orch_envhost::blobs::MemoryBlobs;
use orch_envhost::environment::{EnvHost, HostSettings, Reports};
use orch_envhost::error::HostError;
use orch_link_contract::commands::{EnvAttach, IndexMode};
use orch_types::{SystemClock, new_id};

#[tokio::test]
async fn an_unverified_platform_refuses_attachment_without_workspace_or_environment_effects() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("kept.txt"), b"kept workspace bytes").unwrap();
    let (reports, mut receiver) = Reports::channel(16);
    let host = EnvHost::new(
        HostSettings::connected(dir.path().join("data"), vec![workspace.clone()]),
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    let environment_id = new_id();
    let result = host
        .env_attach(
            &EnvAttach {
                workspace: Default::default(),

                resources: None,
                data_generation: None,
                storage: None,
                data_sources: None,
                environment_id,
                workspace_root: Some(workspace.to_string_lossy().into_owned()),
                ignore: Vec::new(),
                index: IndexMode::Full,
                mounts: Some(Vec::new()),
                variables: BTreeMap::new(),
                secrets: BTreeMap::new(),
            },
            &BTreeMap::new(),
        )
        .await;
    assert!(
        matches!(&result, Err(HostError::Unsupported(_))),
        "{result:?}"
    );
    assert!(host.environments().is_empty());
    assert!(host.environment(environment_id).is_err());
    assert!(
        receiver.try_recv().is_err(),
        "no attachment report was published"
    );
    assert_eq!(std::fs::read_dir(&workspace).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(workspace.join("kept.txt")).unwrap(),
        b"kept workspace bytes"
    );
}
