//! Existing workspace/home binds retain metadata and content through restart, replacement and confirmed owned-volume cleanup.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
use common::{BUSYBOX, Lab};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::runtime::{DirectoryBinding, VolumeRole};
use serde_json::json;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[tokio::test]
async fn a_provider_uses_its_borrowed_harness_home_and_cleanup_leaves_the_directory_whole() {
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: ORCH_TEST_STATIC_NODE must name the released static node");
        return;
    };
    let Some(lab) = Lab::with(|config, _| {
        config.live_helper_image = Some(BUSYBOX.into());
        config.live_helper_binary = Some(binary.into());
    })
    .await
    else {
        return;
    };
    if !lab.driver.live_helpers_supported() {
        eprintln!("not run: this daemon cannot verify existing directory binds");
        return;
    }
    lab.have(BUSYBOX).await;
    let path = lab.path("provider-existing-home");
    std::fs::write(path.join("credential"), "existing sign-in").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o2777)).unwrap();
    let before = std::fs::metadata(&path).unwrap();
    let uid = if before.uid() == 65534 { 65533 } else { 65534 };
    let mut request = lab.request("provider", "provider-borrowed-home", |body| {
        body["spec"]["user"] = json!({"uid":uid,"gid":uid});
        body["env"]["LAB_RUN"] = json!("cat \"$ORCH_HARNESS_HOME/credential\"");
    });
    request.harness_home = Some(
        orch_driver_contract::runtime::EnvironmentDataSource::ExistingDirectory {
            path: path.display().to_string(),
        },
    );
    request.volumes.insert(
        VolumeRole::Home,
        orch_driver_contract::runtime::VolumeRef {
            name: lab.name("provider-home-reference"),
            storage: None,
            directory: Some(DirectoryBinding {
                path: path.display().to_string(),
                device: Some(before.dev()),
                inode: Some(before.ino()),
            }),
        },
    );
    let unit = lab.run(&request).await;
    let inspected = lab.inspect(&unit).await;
    let mount = inspected
        .mounts
        .as_ref()
        .unwrap()
        .iter()
        .find(|mount| mount.destination.as_deref() == Some("/opt/orch/harness-home"))
        .unwrap();
    assert_eq!(mount.source.as_deref(), path.to_str());
    assert_eq!(
        lab.read(&unit, "/opt/orch/harness-home/credential")
            .await
            .as_deref(),
        Some("existing sign-in")
    );
    lab.driver
        .stop(&unit, std::time::Duration::from_secs(2))
        .await
        .unwrap();
    lab.driver.start(&unit).await.unwrap();
    lab.driver.remove(&unit).await.unwrap();
    for volume in request
        .volumes
        .values()
        .filter(|volume| volume.directory.is_none())
    {
        lab.driver.remove_volume(&volume.name).await.unwrap();
    }
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(
        (after.uid(), after.gid(), after.mode(), after.ino()),
        (before.uid(), before.gid(), before.mode(), before.ino())
    );
    assert_eq!(
        std::fs::read_to_string(path.join("credential")).unwrap(),
        "existing sign-in"
    );
}

#[tokio::test]
async fn borrowed_data_is_bound_without_chown_and_survives_restart_recreate_and_purge() {
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: ORCH_TEST_STATIC_NODE must name the released static node");
        return;
    };
    let Some(lab) = Lab::with(|config, _| {
        config.live_helper_image = Some(BUSYBOX.into());
        config.live_helper_binary = Some(binary.into());
    })
    .await
    else {
        return;
    };
    if !lab.driver.live_helpers_supported() {
        eprintln!("not run: this daemon cannot verify existing directory binds");
        return;
    }
    lab.have(BUSYBOX).await;
    let path = lab.path("borrowed");
    std::fs::write(path.join("kept.txt"), "person's files").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o2777)).unwrap();
    let before = std::fs::metadata(&path).unwrap();
    let uid = if before.uid() == 65534 { 65533 } else { 65534 };
    let binding = DirectoryBinding {
        path: path.display().to_string(),
        device: Some(before.dev()),
        inode: Some(before.ino()),
    };
    let mut request = lab.request("environment", "borrowed-roots", |body| {
        body["spec"]["user"] = json!({"uid":uid,"gid":uid});
        body["env"]["LAB_RUN"] = json!("cat /workspace/kept.txt");
    });
    for role in [VolumeRole::Workspace, VolumeRole::Home] {
        request.volumes.get_mut(&role).unwrap().directory = Some(binding.clone());
    }
    let first = lab.run(&request).await;
    let mounts = lab.inspect(&first).await.mounts.unwrap();
    for target in ["/workspace", "/home/agent"] {
        let actual = mounts
            .iter()
            .find(|mount| mount.destination.as_deref() == Some(target))
            .unwrap();
        assert_eq!(actual.source.as_deref(), Some(path.to_str().unwrap()));
    }
    lab.driver
        .stop(&first, std::time::Duration::from_secs(2))
        .await
        .unwrap();
    lab.driver.start(&first).await.unwrap();
    assert_eq!(
        lab.read(&first, "/workspace/kept.txt").await.as_deref(),
        Some("person's files")
    );
    lab.driver.remove(&first).await.unwrap();
    let mut replacement = lab.request("environment", "borrowed-recreated", |body| {
        body["spec"]["user"] = json!({"uid":uid,"gid":uid});
    });
    for role in [VolumeRole::Workspace, VolumeRole::Home] {
        replacement.volumes.get_mut(&role).unwrap().directory = Some(binding.clone());
    }
    let second = lab.run(&replacement).await;
    lab.driver.remove(&second).await.unwrap();
    // Exactly the references a confirmed purge is permitted to remove; borrowed paths never enter it.
    for volume in replacement
        .volumes
        .values()
        .filter(|volume| volume.directory.is_none())
    {
        lab.driver.remove_volume(&volume.name).await.unwrap();
    }
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(
        (after.uid(), after.gid(), after.mode()),
        (before.uid(), before.gid(), before.mode())
    );
    assert_eq!(
        std::fs::read_to_string(path.join("kept.txt")).unwrap(),
        "person's files"
    );
}
