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

#[tokio::test]
async fn reviewing_a_native_system_allows_its_shared_directory_and_revocation_closes_that_attachment()
 {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("work");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("kept"), b"original").unwrap();
    let (reports, _events) = Reports::channel(32);
    let host = EnvHost::new(
        HostSettings::connected(dir.path().join("data"), vec![workspace.clone()]),
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    let id = new_id();
    let body = EnvAttach {
        resources: None,
        data_generation: None,
        storage: None,
        data_sources: None,
        environment_id: id,
        workspace_root: Some(workspace.to_string_lossy().into_owned()),
        ignore: vec![],
        index: IndexMode::Live,
        mounts: Some(vec![]),
        variables: BTreeMap::new(),
        secrets: BTreeMap::new(),
    };
    host.review_system(|| Some(1)).await.unwrap();
    host.env_attach(&body, &BTreeMap::new()).await.unwrap();
    let original = host.environment(id).unwrap();
    assert!(original.context().reviewed_native_system);
    assert_eq!(
        original.files().root().root_path(),
        std::fs::canonicalize(&workspace).unwrap()
    );
    host.review_system(|| None).await.unwrap();
    assert!(host.environment(id).is_err());
    assert!(host.env_attach(&body, &BTreeMap::new()).await.is_err());
    host.review_system(|| Some(2)).await.unwrap();
    host.env_attach(&body, &BTreeMap::new()).await.unwrap();
    assert!(
        host.environment(id)
            .unwrap()
            .context()
            .reviewed_native_system
    );
    assert!(
        !original.context().reviewed_native_system,
        "a later review cannot authorize a retired registration"
    );
    assert_eq!(std::fs::read(workspace.join("kept")).unwrap(), b"original");
}

#[cfg(windows)]
#[test]
fn checking_the_directorys_native_write_rights_leaves_its_contents_untouched() {
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(scratch.path().join("kept"), b"kept").unwrap();
    orch_envhost::paths::Workspace::require_directory_write(scratch.path()).unwrap();
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 1);
}
