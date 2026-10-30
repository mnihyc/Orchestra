//! Real Docker live mounts preserve a running process, enforce read-only and disappear on detach.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{BUSYBOX, Lab};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::mounts::{MountChange, MountKind, MountMode, MountSpec};
use orch_driver_contract::units::UnitRef;
use serde_json::json;
use std::time::Duration;

async fn wait_file(lab: &Lab, unit: &UnitRef, path: &str, expected: &str) {
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if lab.read(unit, path).await.as_deref() == Some(expected) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "{path} did not become {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_live_readonly_mount_appears_and_detaches_without_restarting_the_process_and_is_reapplied_after_start()
 {
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: set ORCH_TEST_STATIC_NODE to the released static orch-node binary");
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
        eprintln!("not run: this daemon has no safe live namespace helper");
        return;
    }
    lab.have(BUSYBOX).await;
    let data = lab.path("live-source");
    std::fs::write(data.join("a.txt"), "original\n").unwrap();
    let request = lab.request("environment","live-mounted",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});
        request["env"]=json!({"LAB_RUN":"( while :; do if [ -f /data/late/a.txt ]; then cat /data/late/a.txt > /opt/mount-seen; if echo changed > /data/late/new.txt 2>/dev/null; then echo writable > /opt/mount-mode; else echo readonly > /opt/mount-mode; fi; else echo absent > /opt/mount-seen; fi; sleep 0.1; done ) & echo $! > /opt/process-kept"});
    });
    let unit = lab.run(&request).await;
    wait_file(&lab, &unit, "/opt/mount-seen", "absent\n").await;
    let pid = lab.inspect(&unit).await.state.unwrap().pid;
    let process = lab.read(&unit, "/opt/process-kept").await.unwrap();
    let mount = MountSpec {
        name: "live-data".into(),
        kind: MountKind::Path,
        source: data.to_string_lossy().into_owned(),
        target: "/data/late".into(),
        mode: MountMode::ReadOnly,
        layer: None,
    };
    let reserved = vec![
        "/workspace".into(),
        "/home/agent".into(),
        "/var/lib/orch".into(),
    ];
    let attach = MountChange {
        before: vec![],
        after: vec![mount.clone()],
        reserved: reserved.clone(),
    };
    let applied = lab.driver.apply_mounts(&unit, &attach).await.unwrap();
    assert_eq!(applied[0].mode, MountMode::ReadOnly);
    wait_file(&lab, &unit, "/opt/mount-seen", "original\n").await;
    wait_file(&lab, &unit, "/opt/mount-mode", "readonly\n").await;
    assert!(!data.join("new.txt").exists());
    assert_eq!(lab.inspect(&unit).await.state.unwrap().pid, pid);
    assert_eq!(lab.read(&unit, "/opt/process-kept").await.unwrap(), process);
    let detach = MountChange {
        before: vec![mount.clone()],
        after: vec![],
        reserved: reserved.clone(),
    };
    lab.driver.apply_mounts(&unit, &detach).await.unwrap();
    wait_file(&lab, &unit, "/opt/mount-seen", "absent\n").await;
    assert_eq!(lab.inspect(&unit).await.state.unwrap().pid, pid);
    assert_eq!(lab.read(&unit, "/opt/process-kept").await.unwrap(), process);
    lab.driver.apply_mounts(&unit, &attach).await.unwrap();
    lab.driver
        .stop(&unit, Duration::from_secs(2))
        .await
        .unwrap();
    lab.driver.start(&unit).await.unwrap();
    wait_file(&lab, &unit, "/opt/mount-seen", "absent\n").await;
    // The node passes the retained desired set after every new start identity.
    lab.driver.apply_mounts(&unit, &attach).await.unwrap();
    wait_file(&lab, &unit, "/opt/mount-seen", "original\n").await;
}

#[tokio::test]
async fn a_live_private_mount_keeps_changes_in_its_layer_and_never_writes_the_data() {
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: ORCH_TEST_STATIC_NODE is required");
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
    if !lab.driver.live_helpers_supported() || !lab.driver.describe().private_views {
        eprintln!("not run: this daemon cannot make a live private view");
        return;
    }
    lab.have(BUSYBOX).await;
    let data = lab.path("private-live-source");
    std::fs::write(data.join("a.txt"), "underneath\n").unwrap();
    let request = lab.request("environment","private-live",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});
        request["env"]=json!({"LAB_RUN":"( while :; do if [ -f /data/private/a.txt ]; then echo changed > /data/private/a.txt; echo here > /data/private/only-here; cat /data/private/a.txt > /opt/private-seen; fi; sleep 0.1; done ) &"});
    });
    let unit = lab.run(&request).await;
    let layer = lab.name("live-over");
    lab.track_volume(&layer);
    lab.track_volume(&format!("{layer}-view"));
    let mount = MountSpec {
        name: "private-data".into(),
        kind: MountKind::Path,
        source: data.to_string_lossy().into_owned(),
        target: "/data/private".into(),
        mode: MountMode::Private,
        layer: Some(orch_driver_contract::mounts::MountLayer {
            name: layer,
            storage: None,
        }),
    };
    let applied = lab
        .driver
        .apply_mounts(
            &unit,
            &MountChange {
                before: vec![],
                after: vec![mount],
                reserved: vec![
                    "/workspace".into(),
                    "/home/agent".into(),
                    "/var/lib/orch".into(),
                ],
            },
        )
        .await
        .unwrap();
    assert_eq!(applied[0].mode, MountMode::Private);
    wait_file(&lab, &unit, "/opt/private-seen", "changed\n").await;
    assert_eq!(
        std::fs::read_to_string(data.join("a.txt")).unwrap(),
        "underneath\n"
    );
    assert!(!data.join("only-here").exists());
}

#[tokio::test]
async fn a_failed_second_attach_restores_the_previous_set_and_keeps_the_process_running() {
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: ORCH_TEST_STATIC_NODE is required");
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
        eprintln!("not run: this daemon has no safe live namespace helper");
        return;
    }
    lab.have(BUSYBOX).await;
    let old_data = lab.path("rollback-old");
    let new_data = lab.path("rollback-new");
    std::fs::write(old_data.join("a.txt"), "kept\n").unwrap();
    std::fs::write(new_data.join("a.txt"), "new\n").unwrap();
    let request = lab.request("environment","mount-rollback",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});
        request["env"]=json!({"LAB_RUN":"echo occupied > /opt/not-a-directory; ( while :; do if [ -f /data/kept/a.txt ]; then cat /data/kept/a.txt > /opt/rollback-seen; else echo absent > /opt/rollback-seen; fi; sleep 0.1; done ) & echo $! > /opt/process-kept"});
    });
    let unit = lab.run(&request).await;
    let old = MountSpec {
        name: "kept-data".into(),
        kind: MountKind::Path,
        source: old_data.to_string_lossy().into_owned(),
        target: "/data/kept".into(),
        mode: MountMode::ReadOnly,
        layer: None,
    };
    let reserved = vec![
        "/workspace".into(),
        "/home/agent".into(),
        "/var/lib/orch".into(),
    ];
    lab.driver
        .apply_mounts(
            &unit,
            &MountChange {
                before: vec![],
                after: vec![old.clone()],
                reserved: reserved.clone(),
            },
        )
        .await
        .unwrap();
    wait_file(&lab, &unit, "/opt/rollback-seen", "kept\n").await;
    let pid = lab.inspect(&unit).await.state.unwrap().pid;
    let process = lab.read(&unit, "/opt/process-kept").await.unwrap();
    let changed = MountSpec {
        source: new_data.to_string_lossy().into_owned(),
        ..old.clone()
    };
    let impossible = MountSpec {
        name: "cannot-attach".into(),
        target: "/opt/not-a-directory/child".into(),
        ..changed.clone()
    };
    assert!(
        lab.driver
            .apply_mounts(
                &unit,
                &MountChange {
                    before: vec![old],
                    after: vec![changed, impossible],
                    reserved
                }
            )
            .await
            .is_err()
    );
    wait_file(&lab, &unit, "/opt/rollback-seen", "kept\n").await;
    assert_eq!(lab.inspect(&unit).await.state.unwrap().pid, pid);
    assert_eq!(lab.read(&unit, "/opt/process-kept").await.unwrap(), process);
    let mounts: serde_json::Value =
        serde_json::from_str(&lab.read(&unit, "/run/orch/mounts.json").await.unwrap()).unwrap();
    assert_eq!(mounts.as_array().unwrap().len(), 1);
    assert_eq!(mounts[0]["name"], "kept-data");
    assert_eq!(mounts[0]["path"], "/data/kept");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_same_helper_gives_and_removes_a_device_with_the_hosts_numbers_group_and_mode() {
    use std::os::unix::fs::MetadataExt;
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: ORCH_TEST_STATIC_NODE is required");
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
        eprintln!("not run: this daemon has no safe live namespace helper");
        return;
    }
    lab.have(BUSYBOX).await;
    // /dev/null is usable under Docker's ordinary device rules; KVM's separate grant rules have their own tests.
    let host = std::fs::metadata("/dev/null").unwrap();
    let major = u64::from(rustix::fs::major(host.rdev()));
    let minor = u64::from(rustix::fs::minor(host.rdev()));
    let request = lab.request("environment","live-device",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});
        request["env"]=json!({"LAB_RUN":"( while :; do if [ -c /dev/orch-live-null ]; then stat -c '%g:%a:%t:%T' /dev/orch-live-null > /opt/device-details; if echo usable > /dev/orch-live-null; then echo usable > /opt/device-seen; else echo refused > /opt/device-seen; fi; else echo absent > /opt/device-seen; fi; sleep 0.1; done ) & echo $! > /opt/process-kept"});
    });
    let unit = lab.run(&request).await;
    wait_file(&lab, &unit, "/opt/device-seen", "absent\n").await;
    let pid = lab.inspect(&unit).await.state.unwrap().pid;
    let process = lab.read(&unit, "/opt/process-kept").await.unwrap();
    lab.driver
        .give_live_device(&unit, "/dev/null", "/dev/orch-live-null", major, minor)
        .await
        .unwrap();
    wait_file(&lab, &unit, "/opt/device-seen", "usable\n").await;
    wait_file(
        &lab,
        &unit,
        "/opt/device-details",
        &format!(
            "{}:{:o}:{major:x}:{minor:x}\n",
            host.gid(),
            host.mode() & 0o777
        ),
    )
    .await;
    assert_eq!(lab.inspect(&unit).await.state.unwrap().pid, pid);
    assert_eq!(lab.read(&unit, "/opt/process-kept").await.unwrap(), process);
    lab.driver
        .remove_live_device(&unit, "/dev/orch-live-null")
        .await
        .unwrap();
    wait_file(&lab, &unit, "/opt/device-seen", "absent\n").await;
    assert_eq!(lab.inspect(&unit).await.state.unwrap().pid, pid);
    assert_eq!(lab.read(&unit, "/opt/process-kept").await.unwrap(), process);
}
