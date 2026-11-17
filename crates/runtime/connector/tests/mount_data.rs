//! A directory machine makes and attaches owned mount data through the shared volume command and storage policy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod rig_environment;
use orch_link_contract::commands::{
    EnvAttach, EnvDetach, FsFetch, NodeVolumeEnsure, NodeVolumeRemove,
};
use orch_types::new_id;
use rig_environment::EnvRig;
use serde_json::json;
use std::collections::BTreeMap;

#[tokio::test]
async fn a_machine_with_directory_data_facts_can_make_a_volume_attach_it_and_remove_it_after_detach()
 {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("workspace");
    let sibling = rig.directory("persons-directory");
    std::fs::write(sibling.join("kept"), "private files").unwrap();
    rig.start().await;
    let mount = new_id();
    let name = format!("orch-mount-{mount}");
    let labels = BTreeMap::from([("orch.mount_id".into(), mount.to_string())]);
    let ensure = NodeVolumeEnsure {
        driver: "directory".into(),
        name: name.clone(),
        storage: None,
        labels: labels.clone(),
    };
    if !cfg!(target_os = "linux") {
        assert_eq!(rig.fail(&ensure).await, "unsupported");
        assert_eq!(
            std::fs::read_to_string(sibling.join("kept")).unwrap(),
            "private files"
        );
        rig.stop().await;
        return;
    }
    rig.succeed(&ensure).await;
    let attach:EnvAttach=serde_json::from_value(json!({"environment_id":rig.environment,"workspace_root":workspace,"ignore":[],"index":"live",
        "mounts":[{"mount_id":mount,"name":"data","source":name,"volume":{"name":name,"storage":null},"mode":"read_only"}],"variables":{},"secrets":{}})).unwrap();
    let facts = rig.succeed(&attach).await;
    let path = std::path::PathBuf::from(facts["mounts"][0]["path"].as_str().unwrap());
    std::fs::write(path.join("hello"), "shared data").unwrap();
    let read = rig
        .succeed(&FsFetch {
            environment_id: rig.environment,
            mount: Some("data".into()),
            path: "hello".into(),
            expect: None,
            max_bytes: None,
        })
        .await;
    let hash = serde_json::from_value(read["hash"].clone()).unwrap();
    assert_eq!(rig.blobs.content(&hash).unwrap().as_ref(), b"shared data");
    let remove = NodeVolumeRemove {
        driver: "directory".into(),
        name,
        storage: None,
        labels,
    };
    assert_eq!(rig.fail(&remove).await, "conflict");
    rig.succeed(&EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    rig.succeed(&remove).await;
    rig.succeed(&remove).await;
    assert!(!path.exists());
    assert_eq!(
        std::fs::read_to_string(sibling.join("kept")).unwrap(),
        "private files"
    );
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_directory_machine_copies_the_whole_reviewed_mount_into_distinct_owned_locations() {
    use orch_link_contract::commands::{
        MountData, NodeMountExport, NodeMountImport, NodeMountMeasure, NodeVolumeCarryCheck,
        VolumeArchive,
    };
    let mut rig = EnvRig::new();
    let workspace = rig.directory("copy-workspace");
    let source = rig.directory("borrowed-copy-source");
    std::fs::write(source.join("visible.txt"), b"whole mount").unwrap();
    std::fs::write(source.join("hidden.txt"), b"kept despite ignore").unwrap();
    std::fs::write(source.join(".orch_ignore"), b"hidden.txt\n").unwrap();
    rig.start().await;
    let mount = new_id();
    let job = new_id();
    let names = [
        format!("orch-mount-{mount}"),
        format!("orch-mount-{mount}-kept-copy"),
    ];
    let labels = BTreeMap::from([
        ("orch.mount_id".into(), mount.to_string()),
        ("orch.copy_job".into(), job.to_string()),
    ]);
    let carry = rig
        .succeed(&NodeVolumeCarryCheck {
            driver: "directory".into(),
        })
        .await;
    assert_eq!(
        carry["available"], true,
        "the offer must be backed by an executable native transfer"
    );
    let data = MountData::Path {
        path: source.to_string_lossy().into_owned(),
    };
    let measured = rig
        .succeed(&NodeMountMeasure {
            work: None,
            driver: "directory".into(),
            mount_id: mount,
            data: data.clone(),
        })
        .await;
    assert_eq!(
        measured["file_count"], 3,
        "a mount copy never borrows workspace ignore rules"
    );
    let archive: VolumeArchive = serde_json::from_value(
        rig.succeed(&NodeMountExport {
            work: None,
            driver: "directory".into(),
            job_id: job,
            mount_id: mount,
            data,
            part_bytes: 1024,
        })
        .await,
    )
    .unwrap();
    assert!(archive.size > 0 && !archive.parts.is_empty());
    for name in &names {
        rig.succeed(&NodeVolumeEnsure {
            driver: "directory".into(),
            name: name.clone(),
            storage: None,
            labels: labels.clone(),
        })
        .await;
        rig.succeed(&NodeMountImport {
            work: None,
            driver: "directory".into(),
            job_id: job,
            mount_id: mount,
            name: name.clone(),
            storage: None,
            archive: archive.clone(),
        })
        .await;
    }
    let attach:EnvAttach=serde_json::from_value(json!({"environment_id":rig.environment,"workspace_root":workspace,"ignore":[],"index":"live",
        "mounts":[{"mount_id":mount,"name":"copied","source":names[0],"volume":{"name":names[0],"storage":null},"mode":"read_only"}],"variables":{},"secrets":{}})).unwrap();
    let facts = rig.succeed(&attach).await;
    let copied = std::path::PathBuf::from(facts["mounts"][0]["path"].as_str().unwrap());
    assert_eq!(
        std::fs::read(copied.join("visible.txt")).unwrap(),
        b"whole mount"
    );
    assert_eq!(
        std::fs::read(copied.join("hidden.txt")).unwrap(),
        b"kept despite ignore"
    );
    assert_eq!(
        rig.fail(&NodeMountImport {
            work: None,
            driver: "directory".into(),
            job_id: job,
            mount_id: mount,
            name: names[0].clone(),
            storage: None,
            archive
        })
        .await,
        "conflict",
        "an attached destination cannot be overwritten"
    );
    rig.succeed(&EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    for name in names {
        rig.succeed(&NodeVolumeRemove {
            driver: "directory".into(),
            name,
            storage: None,
            labels: labels.clone(),
        })
        .await;
    }
    assert!(!copied.exists());
    assert_eq!(
        std::fs::read(source.join("hidden.txt")).unwrap(),
        b"kept despite ignore"
    );
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_copy_cannot_claim_or_remove_an_older_owned_directory_with_the_same_mount_name() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("original-mount-workspace");
    rig.start().await;
    let mount = new_id();
    let name = format!("orch-mount-{mount}");
    let original = BTreeMap::from([("orch.mount_id".into(), mount.to_string())]);
    rig.succeed(&NodeVolumeEnsure {
        driver: "directory".into(),
        name: name.clone(),
        storage: None,
        labels: original.clone(),
    })
    .await;
    let attach:EnvAttach=serde_json::from_value(json!({"environment_id":rig.environment,"workspace_root":workspace,"ignore":[],"index":"live",
        "mounts":[{"mount_id":mount,"name":"original","source":name,"volume":{"name":name,"storage":null},"mode":"read_only"}],"variables":{},"secrets":{}})).unwrap();
    let facts = rig.succeed(&attach).await;
    let path = std::path::PathBuf::from(facts["mounts"][0]["path"].as_str().unwrap());
    std::fs::write(path.join("original.txt"), b"keep the original data").unwrap();
    rig.succeed(&EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    let mut proposed = original.clone();
    proposed.insert("orch.copy_job".into(), new_id().to_string());
    assert_eq!(
        rig.fail(&NodeVolumeEnsure {
            driver: "directory".into(),
            name: name.clone(),
            storage: None,
            labels: proposed.clone()
        })
        .await,
        "conflict"
    );
    assert_eq!(
        rig.fail(&NodeVolumeRemove {
            driver: "directory".into(),
            name: name.clone(),
            storage: None,
            labels: proposed
        })
        .await,
        "conflict",
        "compensation owns only a directory that exact copy actually made"
    );
    assert_eq!(
        std::fs::read(path.join("original.txt")).unwrap(),
        b"keep the original data"
    );
    rig.succeed(&NodeVolumeRemove {
        driver: "directory".into(),
        name,
        storage: None,
        labels: original,
    })
    .await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_mounts_new_owned_directory_is_distinct_and_can_be_cleaned_while_its_old_location_is_attached()
 {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("distinct-mount-workspace");
    rig.start().await;
    let mount = new_id();
    let original = format!("orch-mount-{mount}");
    let next = format!("orch-mount-{mount}-kept-copy");
    let labels = BTreeMap::from([("orch.mount_id".into(), mount.to_string())]);
    let copy_labels = BTreeMap::from([
        ("orch.mount_id".into(), mount.to_string()),
        ("orch.copy_job".into(), new_id().to_string()),
    ]);
    let attach = |name: &str| -> EnvAttach {
        serde_json::from_value(json!({"environment_id":rig.environment,"workspace_root":workspace,"ignore":[],"index":"live",
            "mounts":[{"mount_id":mount,"name":"data","source":name,"volume":{"name":name,"storage":null},"mode":"read_only"}],"variables":{},"secrets":{}})).unwrap()
    };
    rig.succeed(&NodeVolumeEnsure {
        driver: "directory".into(),
        name: original.clone(),
        storage: None,
        labels: labels.clone(),
    })
    .await;
    let old = rig.succeed(&attach(&original)).await;
    let old_path = std::path::PathBuf::from(old["mounts"][0]["path"].as_str().unwrap());
    std::fs::write(old_path.join("original.txt"), b"the old location").unwrap();
    rig.succeed(&NodeVolumeEnsure {
        driver: "directory".into(),
        name: next.clone(),
        storage: None,
        labels: copy_labels.clone(),
    })
    .await;
    rig.succeed(&NodeVolumeRemove {
        driver: "directory".into(),
        name: next.clone(),
        storage: None,
        labels: copy_labels.clone(),
    })
    .await;
    assert_eq!(
        std::fs::read(old_path.join("original.txt")).unwrap(),
        b"the old location"
    );
    rig.succeed(&NodeVolumeEnsure {
        driver: "directory".into(),
        name: next.clone(),
        storage: None,
        labels: copy_labels.clone(),
    })
    .await;
    rig.succeed(&EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    let new = rig.succeed(&attach(&next)).await;
    let new_path = std::path::PathBuf::from(new["mounts"][0]["path"].as_str().unwrap());
    assert_ne!(
        new_path, old_path,
        "the native volume names identify different actual directories"
    );
    assert!(!new_path.join("original.txt").exists());
    assert_eq!(
        rig.fail(&NodeVolumeRemove {
            driver: "directory".into(),
            name: next.clone(),
            storage: None,
            labels: copy_labels.clone()
        })
        .await,
        "conflict"
    );
    rig.succeed(&EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    rig.succeed(&NodeVolumeRemove {
        driver: "directory".into(),
        name: next,
        storage: None,
        labels: copy_labels,
    })
    .await;
    rig.succeed(&NodeVolumeRemove {
        driver: "directory".into(),
        name: original,
        storage: None,
        labels,
    })
    .await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_directory_mount_measurement_counts_the_whole_source_without_uploading_or_changing_it() {
    use orch_link_contract::commands::{MountData, NodeMountMeasure};
    let mut rig = EnvRig::new();
    let source = rig.directory("whole-mount-count");
    std::fs::write(source.join("visible"), b"abc").unwrap();
    std::fs::write(source.join("hidden"), b"defgh").unwrap();
    std::fs::write(source.join(".orch_ignore"), b"hidden\n").unwrap();
    rig.start().await;
    let measured = rig
        .succeed(&NodeMountMeasure {
            work: None,
            driver: "directory".into(),
            mount_id: new_id(),
            data: MountData::Path {
                path: source.to_string_lossy().into_owned(),
            },
        })
        .await;
    assert_eq!(measured["file_count"], 3);
    assert_eq!(measured["total_bytes"], 15);
    assert_eq!(std::fs::read(source.join("hidden")).unwrap(), b"defgh");
    let mount = new_id();
    let name = format!("orch-mount-{mount}");
    rig.succeed(&NodeVolumeEnsure {
        driver: "directory".into(),
        name: name.clone(),
        storage: None,
        labels: BTreeMap::from([("orch.mount_id".into(), mount.to_string())]),
    })
    .await;
    assert_eq!(
        rig.fail(&NodeMountMeasure {
            work: None,
            driver: "directory".into(),
            mount_id: mount,
            data: MountData::Volume {
                name,
                storage: Some("another-storage".into())
            },
        })
        .await,
        "conflict",
        "a native location cannot be reinterpreted as another storage"
    );
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_missing_owned_path_does_not_hide_its_still_attached_native_directory() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("workspace");
    rig.start().await;
    let mount = new_id();
    let name = format!("orch-mount-{mount}");
    let labels = BTreeMap::from([("orch.mount_id".into(), mount.to_string())]);
    rig.succeed(&NodeVolumeEnsure {
        driver: "directory".into(),
        name: name.clone(),
        storage: None,
        labels: labels.clone(),
    })
    .await;
    let attach: EnvAttach = serde_json::from_value(json!({"environment_id":rig.environment,"workspace_root":workspace,"ignore":[],"index":"live",
        "mounts":[{"mount_id":mount,"name":"data","source":name,"volume":{"name":name,"storage":null},"mode":"read_only"}],"variables":{},"secrets":{}})).unwrap();
    let facts = rig.succeed(&attach).await;
    let path = std::path::PathBuf::from(facts["mounts"][0]["path"].as_str().unwrap());
    std::fs::write(path.join("keep"), b"still used").unwrap();
    let moved = path.with_extension("moved");
    std::fs::rename(&path, &moved).unwrap();
    assert_eq!(
        rig.fail(&NodeVolumeRemove {
            driver: "directory".into(),
            name,
            storage: None,
            labels
        })
        .await,
        "conflict"
    );
    assert_eq!(std::fs::read(moved.join("keep")).unwrap(), b"still used");
    rig.succeed(&EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[path = "mount_data/serialization.rs"]
mod serialization;
