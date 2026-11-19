//! Volumes on the storages a host declares: directories below which the data of a volume lives, measured for the
//! node, kept when the unit goes and removed with the volume.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{Lab, my_ids};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::VolumeRole;
use orch_driver_contract::volumes::VolumeRequest;
use orch_driver_docker::config::StorageConfig;
use serde_json::json;

/// A lab with a storage `fast` and a default storage `bulk`, both directories of the lab's own.
async fn lab_with_storages() -> Option<Lab> {
    Lab::with(|config, dir| {
        for (name, default) in [("fast", false), ("bulk", true)] {
            let path = dir.join(name);
            std::fs::create_dir_all(&path).unwrap();
            config.storages.push(StorageConfig {
                name: name.to_owned(),
                path,
                default,
            });
        }
    })
    .await
}

#[tokio::test]
async fn a_volume_lives_in_the_directory_of_its_storage_and_goes_with_the_volume() {
    let Some(lab) = lab_with_storages().await else {
        return;
    };
    // The host reports its storages, with the room each has.
    let storages = lab.driver.storages();
    assert_eq!(
        storages
            .iter()
            .map(|storage| (storage.name.as_str(), storage.default))
            .collect::<Vec<_>>(),
        [("fast", false), ("bulk", true)]
    );
    for storage in &storages {
        assert!(
            storage.capacity_gb >= storage.free_gb && storage.free_gb >= 0,
            "{storage:?}"
        );
        assert_eq!(
            storage.private_views,
            lab.driver.describe().private_views,
            "private views are as the daemon allows"
        );
    }
    // A unit with its state on `fast` and its workspace on the default writes into both, as a user who can
    // remove what it wrote.
    let (uid, gid) = my_ids(&lab.dir.path().join("fast"));
    let spec = lab.request("environment", "stored", |request| {
        request["volumes"]["state"]["storage"] = json!("fast");
        request["spec"]["user"] = json!({ "uid": uid, "gid": gid });
        request["env"] = json!({
            "LAB_RUN": "echo in-state > $ORCH_DATA_DIR/state.txt; echo in-workspace > $ORCH_WORKSPACE_DIR/workspace.txt"
        });
    });
    let state = spec.volumes[&VolumeRole::State].name.clone();
    let workspace = spec.volumes[&VolumeRole::Workspace].name.clone();
    let unit = lab.run(&spec).await;
    let in_state = lab.dir.path().join("fast").join(&state);
    let in_workspace = lab.dir.path().join("bulk").join(&workspace);
    assert_eq!(
        std::fs::read_to_string(in_state.join("state.txt")).unwrap(),
        "in-state\n"
    );
    assert_eq!(
        std::fs::read_to_string(in_workspace.join("workspace.txt")).unwrap(),
        "in-workspace\n"
    );
    // The volume is the bound directory, nothing else.
    let volume = lab.docker.inspect_volume(&state).await.unwrap();
    let options = volume.options;
    assert_eq!(options["o"], "bind");
    assert_eq!(options["device"], in_state.to_string_lossy());
    // The unit goes and its data stays; the volume goes and its data goes with it, and only its own.
    lab.driver
        .stop(&unit, Duration::from_secs(5))
        .await
        .unwrap();
    lab.driver.remove(&unit).await.unwrap();
    assert!(lab.has_volume(&state).await && in_state.join("state.txt").exists());
    lab.driver.remove_volume(&state).await.unwrap();
    assert!(!lab.has_volume(&state).await);
    assert!(!in_state.exists(), "the data went with the volume");
    assert!(
        in_workspace.join("workspace.txt").exists() && lab.has_volume(&workspace).await,
        "the volumes on other storages are left as they are"
    );
    lab.driver.remove_volume(&workspace).await.unwrap();
    assert!(!in_workspace.exists());
    // Asked again it is no error.
    lab.driver.remove_volume(&workspace).await.unwrap();
}

#[tokio::test]
async fn a_storage_the_host_does_not_declare_is_refused_and_a_volume_stays_where_it_was_made() {
    let Some(lab) = lab_with_storages().await else {
        return;
    };
    let request = VolumeRequest {
        name: lab.name("placed"),
        storage: Some("fast".into()),
        labels: [("orch.system".to_owned(), lab.system.clone())].into(),
    };
    lab.track_volume(&request.name);
    lab.driver.ensure_volume(&request).await.unwrap();
    lab.driver.ensure_volume(&request).await.unwrap();
    assert!(lab.dir.path().join("fast").join(&request.name).is_dir());
    // Asked for on another storage it is refused, and it is not moved.
    let elsewhere = VolumeRequest {
        storage: None,
        ..request.clone()
    };
    assert!(matches!(
        lab.driver.ensure_volume(&elsewhere).await,
        Err(DriverError::InvalidSpec(ref why)) if why.contains("another storage")
    ));
    let unknown = VolumeRequest {
        name: lab.name("nowhere"),
        storage: Some("tape".into()),
        labels: request.labels.clone(),
    };
    lab.track_volume(&unknown.name);
    assert!(matches!(
        lab.driver.ensure_volume(&unknown).await,
        Err(DriverError::InvalidSpec(ref why)) if why.contains("tape")
    ));
    assert!(!lab.has_volume(&unknown.name).await);
    assert!(!lab.dir.path().join("tape").exists());
    // A name the daemon would not take is refused before anything is made.
    for bad in ["", "../escape", "with space", "-leading"] {
        let refused = VolumeRequest {
            name: bad.to_owned(),
            storage: Some("fast".into()),
            labels: request.labels.clone(),
        };
        assert!(
            matches!(
                lab.driver.ensure_volume(&refused).await,
                Err(DriverError::InvalidSpec(_))
            ),
            "{bad:?}"
        );
    }
    assert!(!lab.dir.path().join("escape").exists());
    lab.driver.remove_volume(&request.name).await.unwrap();
    assert!(!lab.dir.path().join("fast").join(&request.name).exists());
}

#[tokio::test]
async fn the_data_of_a_volume_that_is_already_gone_is_removed_by_asking_again_and_a_name_cannot_reach_beyond_a_storage()
 {
    let Some(lab) = lab_with_storages().await else {
        return;
    };
    let request = VolumeRequest {
        name: lab.name("orphaned"),
        storage: Some("fast".into()),
        labels: [("orch.system".to_owned(), lab.system.clone())].into(),
    };
    lab.track_volume(&request.name);
    lab.driver.ensure_volume(&request).await.unwrap();
    let data = lab.dir.path().join("fast").join(&request.name);
    std::fs::write(data.join("left.txt"), "left behind").unwrap();
    // The daemon's volume goes by another way (a removal that was cut short, an operator), and the data stays.
    lab.docker
        .remove_volume(
            &request.name,
            None::<bollard::query_parameters::RemoveVolumeOptions>,
        )
        .await
        .unwrap();
    assert!(!lab.has_volume(&request.name).await && data.join("left.txt").exists());
    lab.driver.remove_volume(&request.name).await.unwrap();
    assert!(!data.exists(), "asked again, the removal is finished");
    // A name is a volume's name or it is refused, so that it can never be a path that leaves a storage.
    let sentinel = lab.dir.path().join("fast").join("sentinel");
    std::fs::create_dir_all(&sentinel).unwrap();
    for bad in ["../fast", "fast/sentinel", ".", "..", "", "sentinel/../.."] {
        let refused = lab.driver.remove_volume(bad).await;
        assert!(
            matches!(refused, Err(DriverError::InvalidSpec(_))),
            "{bad:?}: {refused:?}"
        );
    }
    assert!(sentinel.is_dir() && lab.dir.path().join("fast").is_dir());
}
