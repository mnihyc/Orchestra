//! The commands about volumes, on the in-memory hub with the in-memory driver: a volume is made on the storage named or on the host's default, a storage the host does not have is refused with the reason, and a volume a running unit stands on is not removed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;

use common::Rig;
use orch_driver_contract::runtime::RuntimeKind;
use orch_driver_contract::volumes::StorageInfo;
use orch_link_contract::commands::{NodeVolumeEnsure, NodeVolumeRemove, SerialKey};
use orch_testkit::doubles::driver::{MemoryDriver, Operation};

fn storage(name: &str, default: bool) -> StorageInfo {
    StorageInfo {
        name: name.to_owned(),
        default,
        capacity_gb: 1000,
        free_gb: 400,
        private_views: default,
    }
}

fn ensure(name: &str, storage: Option<&str>) -> NodeVolumeEnsure {
    NodeVolumeEnsure {
        driver: "memory".to_owned(),
        name: name.to_owned(),
        storage: storage.map(str::to_owned),
        labels: BTreeMap::from([("orch.system".to_owned(), "orch-test-system".to_owned())]),
    }
}

fn remove(name: &str) -> NodeVolumeRemove {
    NodeVolumeRemove {
        driver: "memory".to_owned(),
        name: name.to_owned(),
        storage: None,
        labels: BTreeMap::new(),
    }
}

fn key(name: &str) -> SerialKey {
    SerialKey::Volume(name.to_owned())
}

#[tokio::test]
async fn restored_storage_is_read_without_creation_and_refuses_other_systems_and_environments() {
    use orch_driver_contract::{driver::EnvironmentDriver, volumes::VolumeRequest};
    use orch_link_contract::commands::{NodeVolumeInspect, NodeVolumeInspectResult};
    let mut rig = Rig::with_driver(MemoryDriver::new("memory").replacing());
    rig.start().await;
    let environment = orch_types::new_id();
    let request = NodeVolumeInspect {
        driver: "memory".into(),
        name: format!("orch-ws-{environment}"),
        storage: None,
        environment_id: environment,
    };
    let absent: NodeVolumeInspectResult =
        serde_json::from_value(rig.succeed(&key(&request.name), &request).await).unwrap();
    assert!(!absent.exists);
    assert!(
        rig.driver.volumes().is_empty(),
        "inspection creates no replacement data"
    );
    let mut volume = ensure(&request.name, None);
    volume
        .labels
        .insert("orch.environment_id".into(), environment.to_string());
    rig.succeed(&key(&request.name), &volume).await;
    let owned: NodeVolumeInspectResult =
        serde_json::from_value(rig.succeed(&key(&request.name), &request).await).unwrap();
    assert!(owned.exists);
    let mut other = request.clone();
    other.environment_id = orch_types::new_id();
    assert_eq!(
        rig.refused(&key(&request.name), &other).await.code,
        "conflict"
    );
    let foreign = NodeVolumeInspect {
        name: format!("orch-home-{environment}"),
        ..request.clone()
    };
    rig.driver
        .ensure_volume(&VolumeRequest {
            name: foreign.name.clone(),
            storage: None,
            labels: BTreeMap::from([
                ("orch.system".into(), "another-system".into()),
                ("orch.environment_id".into(), environment.to_string()),
            ]),
        })
        .await
        .unwrap();
    assert_eq!(
        rig.refused(&key(&foreign.name), &foreign).await.code,
        "conflict"
    );
    assert_eq!(
        rig.driver.volumes().len(),
        2,
        "refusal neither adopts nor removes existing data"
    );
    assert_eq!(
        rig.driver.volume(&foreign.name).unwrap().labels["orch.system"],
        "another-system"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_volume_is_made_on_the_storage_named_or_on_the_hosts_default_with_its_labels() {
    let driver = MemoryDriver::new("memory")
        .replacing()
        .with_storages(vec![storage("fast", true), storage("bulk", false)]);
    let mut rig = Rig::with_driver(driver);
    rig.start().await;
    rig.succeed(&key("orch-ws-1"), &ensure("orch-ws-1", Some("bulk")))
        .await;
    rig.succeed(&key("orch-home-1"), &ensure("orch-home-1", None))
        .await;
    assert_eq!(
        rig.driver.volume("orch-ws-1").unwrap().storage.as_deref(),
        Some("bulk")
    );
    assert_eq!(
        rig.driver.volume("orch-home-1").unwrap().storage,
        None,
        "the driver's default is its own"
    );
    assert_eq!(
        rig.driver.volume("orch-ws-1").unwrap().labels["orch.system"],
        "orch-test-system"
    );
    // Asking again is done.
    rig.succeed(&key("orch-ws-1"), &ensure("orch-ws-1", Some("bulk")))
        .await;
    assert_eq!(rig.driver.volumes().len(), 2);
    rig.stop().await;
}

#[tokio::test]
async fn a_storage_the_host_does_not_have_is_refused_with_the_reason_before_the_driver_is_asked() {
    let driver = MemoryDriver::new("memory")
        .replacing()
        .with_storages(vec![storage("fast", true)]);
    let mut rig = Rig::with_driver(driver);
    rig.start().await;
    let error = rig.refused(&key("v"), &ensure("v", Some("slow"))).await;
    assert_eq!(error.code, "not_found");
    assert!(error.detail.contains("slow"), "{}", error.detail);
    assert!(rig.driver.targets_of(Operation::EnsureVolume).is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn a_driver_that_makes_no_volume_on_a_named_storage_is_not_asked_to() {
    let mut rig = Rig::new();
    rig.start().await;
    let error = rig.refused(&key("v"), &ensure("v", Some("fast"))).await;
    assert_eq!(error.code, "unsupported");
    assert!(rig.driver.targets_of(Operation::EnsureVolume).is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn a_name_that_could_leave_its_directory_and_a_driver_the_host_does_not_have_are_refused() {
    let mut rig = Rig::new();
    rig.start().await;
    for name in ["../etc", "a/b", ".hidden", ""] {
        let error = rig.refused(&key(name), &ensure(name, None)).await;
        assert_eq!(error.code, "invalid", "{name:?}");
    }
    let mut elsewhere = ensure("v", None);
    elsewhere.driver = "host".to_owned();
    assert_eq!(rig.refused(&key("v"), &elsewhere).await.code, "unsupported");
    assert!(rig.driver.targets_of(Operation::EnsureVolume).is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn a_volume_a_running_unit_stands_on_is_not_removed_and_one_nobody_runs_on_is() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "stands");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let state = spec.volumes[&orch_driver_contract::runtime::VolumeRole::State]
        .name
        .clone();

    let error = rig.refused(&key(&state), &remove(&state)).await;
    assert_eq!(error.code, "conflict");
    assert!(error.detail.contains(&spec.name), "{}", error.detail);
    assert!(rig.driver.volume(&state).is_some());

    rig.stop_runtime(spec.runtime_id).await;
    rig.succeed(&key(&state), &remove(&state)).await;
    assert!(rig.driver.volume(&state).is_none());
    // What is not there is removed already.
    rig.succeed(&key(&state), &remove(&state)).await;
    rig.stop().await;
}

#[tokio::test]
async fn a_backup_restored_while_recovery_waits_is_neither_overwritten_nor_deleted_as_scratch() {
    let mut rig = Rig::with_driver(MemoryDriver::new("memory").replacing());
    rig.start().await;
    let environment = orch_types::new_id();
    let name = format!("orch-ws-{environment}");
    let mut backup = ensure(&name, None);
    backup
        .labels
        .insert("orch.environment_id".into(), environment.to_string());
    rig.succeed(&key(&name), &backup).await;
    let mut scratch = backup.clone();
    scratch.labels.insert(
        "orch.recovery_job_id".into(),
        orch_types::new_id().to_string(),
    );
    assert_eq!(rig.refused(&key(&name), &scratch).await.code, "conflict");
    let removal = NodeVolumeRemove {
        driver: "memory".into(),
        name: name.clone(),
        storage: None,
        labels: scratch.labels.clone(),
    };
    assert_eq!(rig.refused(&key(&name), &removal).await.code, "conflict");
    assert_eq!(
        rig.driver.volume(&name).unwrap().labels,
        backup.labels,
        "the backup's original ownership metadata remains"
    );
    assert!(
        !rig.driver
            .calls()
            .iter()
            .any(|call| call.operation == Operation::RemoveVolume),
        "no destructive driver operation was admitted"
    );
    rig.stop().await;
}
