//! Real host-process lifecycle: account, launch paths, restart, retained directories and refusal to touch another unit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::{RuntimeKind, RuntimeSpec, VolumeRole, unit_name};
use orch_driver_contract::units::UnitStateKind;
use orch_driver_contract::volumes::VolumeRequest;
use orch_driver_host::{HostConfig, HostDriver, HostStorage};
use orch_types::{SystemClock, new_id};
use serde_json::json;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

fn current_user() -> String {
    nix::unistd::User::from_uid(nix::unistd::Uid::effective())
        .unwrap()
        .unwrap()
        .name
}

fn connector(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("orch-connector");
    std::fs::write(&path,b"#!/bin/sh\nset -eu\nid -u > \"$ORCH_WORKSPACE_DIR/run-uid\"\nprintf '%s\\n' \"$ORCH_ROLES\" \"$ORCH_GATEWAY_URL\" \"$ORCH_CA_FILE\" \"$ORCH_ENROLL_FILE\" \"$ORCH_DATA_DIR\" \"$ORCH_TOOLS_DIR\" \"$ORCH_WORKSPACE_DIR\" \"$ORCH_HOME_DIR\" \"$ORCH_MOUNTS_FILE\" > \"$ORCH_WORKSPACE_DIR/launch\"\nsleep 600\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn config(dir: &std::path::Path) -> HostConfig {
    HostConfig {
        base: dir.join("host"),
        connector: connector(dir),
        tools: dir.join("tools"),
        storages: vec![
            HostStorage {
                name: "workspace".into(),
                path: dir.join("workspaces"),
                default: false,
            },
            HostStorage {
                name: "home".into(),
                path: dir.join("homes"),
                default: false,
            },
            HostStorage {
                name: "state".into(),
                path: dir.join("state"),
                default: true,
            },
        ],
        prefer_scopes: false,
    }
}

fn spec(_dir: &std::path::Path, account: &str) -> RuntimeSpec {
    let id = new_id();
    serde_json::from_value(json!({
        "runtime_id":id,"kind":"environment","name":unit_name(RuntimeKind::Environment,&id),"driver":"host",
        "spec":{"run_as":account},
        "resources":{},"roles":["environment"],"gateway_url":"wss://gateway.example.invalid","enroll_token":"test-private-enrollment","ca_cert":"test-ca",
        "tools":[],"volumes":{"workspace":{"name":format!("ws-{}",id.simple()),"storage":"workspace"},"home":{"name":format!("home-{}",id.simple()),"storage":"home"},"state":{"name":format!("state-{}",id.simple()),"storage":"state"}},
        "mounts":[],"env":{},"labels":{"orch.system":"host-driver-test","orch.runtime_id":id,"orch.kind":"environment"}
    })).unwrap()
}

async fn ready(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the connector did not write its launch facts");
}

#[tokio::test]
async fn a_host_connector_gets_every_launch_path_and_keeps_its_files_across_restart_and_unit_removal()
 {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let spec = spec(dir.path(), &current_user());
    let driver = HostDriver::open(config.clone(), Arc::new(SystemClock))
        .await
        .unwrap();
    let described = driver.describe();
    assert_eq!(described.check(), Ok(()));
    assert_eq!(
        described.resource_limits.cpu,
        orch_driver_contract::descriptor::ResourceLimits::Reported
    );
    assert_eq!(
        described.resource_limits.disk,
        orch_driver_contract::descriptor::ResourceLimits::Reported
    );
    let unit = driver.create(&spec).await.unwrap();
    assert_eq!(driver.create(&spec).await.unwrap(), unit);
    driver.start(&unit).await.unwrap();
    let workspace = dir
        .path()
        .join("workspaces")
        .join(&spec.volumes[&VolumeRole::Workspace].name);
    ready(&workspace.join("launch")).await;
    let launch = std::fs::read_to_string(workspace.join("launch")).unwrap();
    assert_eq!(
        launch.lines().count(),
        orch_driver_contract::runtime::LAUNCH_VARIABLES.len()
    );
    assert!(launch.lines().all(|line| !line.is_empty()));
    assert_eq!(
        std::fs::read_to_string(workspace.join("run-uid"))
            .unwrap()
            .trim(),
        nix::unistd::Uid::effective().as_raw().to_string()
    );
    assert!(
        !std::fs::read_to_string(
            config
                .base
                .join("units")
                .join(format!("{}.json", unit.name))
        )
        .unwrap()
        .contains("test-private-enrollment")
    );
    let marker = workspace.join("keep.txt");
    std::fs::write(&marker, b"installed work").unwrap();
    let mut foreign = unit.clone();
    foreign.runtime_id = new_id();
    assert!(matches!(
        driver.stop(&foreign, Duration::ZERO).await,
        Err(DriverError::NotFound(_))
    ));
    assert_eq!(
        driver.inspect(&unit).await.unwrap().state,
        UnitStateKind::Running
    );
    assert!(driver.list("another-system").await.unwrap().is_empty());
    driver.stop(&unit, Duration::from_secs(1)).await.unwrap();
    let reopened = HostDriver::open(config.clone(), Arc::new(SystemClock))
        .await
        .unwrap();
    assert_eq!(reopened.create(&spec).await.unwrap(), unit);
    reopened.start(&unit).await.unwrap();
    assert_eq!(std::fs::read(&marker).unwrap(), b"installed work");
    reopened.remove(&unit).await.unwrap();
    assert_eq!(
        reopened.inspect(&unit).await.unwrap().state,
        UnitStateKind::Absent
    );
    assert_eq!(std::fs::read(&marker).unwrap(), b"installed work");
    let volume = VolumeRequest {
        name: spec.volumes[&VolumeRole::Workspace].name.clone(),
        storage: spec.volumes[&VolumeRole::Workspace].storage.clone(),
        labels: BTreeMap::from([("orch.system".into(), "host-driver-test".into())]),
    };
    assert!(reopened.volume_exists(&volume).await.unwrap());
    reopened.remove_volume(&volume.name).await.unwrap();
    assert!(!workspace.exists());
    assert!(!reopened.volume_exists(&volume).await.unwrap());
}

#[tokio::test]
async fn a_host_node_that_is_already_root_can_launch_an_unprivileged_account_with_owned_directories()
 {
    if !nix::unistd::Uid::effective().is_root() {
        eprintln!("root account-switch case unavailable; no elevation attempted");
        return;
    }
    let Some(user) = nix::unistd::User::from_name("nobody").unwrap() else {
        eprintln!("the disposable unprivileged account is absent");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o711)).unwrap();
    let config = config(dir.path());
    let spec = spec(dir.path(), &user.name);
    let driver = HostDriver::open(config, Arc::new(SystemClock))
        .await
        .unwrap();
    let unit = driver.create(&spec).await.unwrap();
    driver.start(&unit).await.unwrap();
    let workspace = dir
        .path()
        .join("workspaces")
        .join(&spec.volumes[&VolumeRole::Workspace].name);
    ready(&workspace.join("run-uid")).await;
    assert_eq!(
        std::fs::read_to_string(workspace.join("run-uid"))
            .unwrap()
            .trim(),
        user.uid.as_raw().to_string()
    );
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        std::fs::metadata(&workspace).unwrap().uid(),
        user.uid.as_raw()
    );
    driver.remove(&unit).await.unwrap();
}

#[tokio::test]
async fn a_reused_pid_in_a_saved_host_record_never_signals_the_unrelated_process() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let spec = spec(dir.path(), &current_user());
    let driver = HostDriver::open(config.clone(), Arc::new(SystemClock))
        .await
        .unwrap();
    let unit = driver.create(&spec).await.unwrap();
    let mut unrelated = tokio::process::Command::new("sleep")
        .arg("600")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = unrelated.id().unwrap();
    let identity = orch_driver_host::stats::identity(pid).unwrap();
    let record = config
        .base
        .join("units")
        .join(format!("{}.json", unit.name));
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    stored["process"] = json!({"pid":pid,"start_ticks":identity.start_ticks+1});
    stored["state"]["state"] = json!("running");
    std::fs::write(&record, serde_json::to_vec(&stored).unwrap()).unwrap();
    assert_eq!(
        driver.inspect(&unit).await.unwrap().state,
        UnitStateKind::Failed
    );
    driver.stop(&unit, Duration::ZERO).await.unwrap();
    assert!(unrelated.try_wait().unwrap().is_none());
    unrelated.kill().await.unwrap();
}
