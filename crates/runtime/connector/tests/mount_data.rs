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
    rig.succeed(&NodeVolumeEnsure {
        driver: "directory".into(),
        name: name.clone(),
        storage: None,
        labels: labels.clone(),
    })
    .await;
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
