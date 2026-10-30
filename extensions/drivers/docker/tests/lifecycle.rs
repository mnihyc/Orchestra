//! The life of a unit on a real Docker daemon: made, started with the launch inputs, stopped, made again on the same
//! volumes, updated to another image, removed with its volumes kept and then with them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{ALPINE, BUSYBOX, Lab};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::units::UnitStateKind;
use orch_driver_contract::volumes::VolumeRequest;
use serde_json::json;

#[tokio::test]
async fn a_unit_is_made_from_a_reference_and_its_connector_finds_every_launch_input() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "launch", |_| {});
    let unit = lab.run(&spec).await;
    let found = lab.launch(&unit).await;
    assert_eq!(found["args"], "run --managed");
    assert_eq!(found["ORCH_ROLES"], "environment");
    assert_eq!(
        found["ORCH_GATEWAY_URL"],
        "wss://gateway.orch.internal:9443"
    );
    assert_eq!(found["ORCH_CA_FILE"], "/run/orch/ca.pem");
    assert_eq!(found["ORCH_ENROLL_FILE"], "/run/orch/enroll");
    assert_eq!(found["ORCH_DATA_DIR"], "/var/lib/orch");
    assert_eq!(found["ORCH_TOOLS_DIR"], "/opt/orch/tools");
    assert_eq!(found["ORCH_WORKSPACE_DIR"], "/workspace");
    assert_eq!(found["ORCH_HOME_DIR"], "/home/agent");
    assert_eq!(found["ORCH_MOUNTS_FILE"], "/run/orch/mounts.json");
    assert!(found["ca_file"].starts_with("-----BEGIN CERTIFICATE-----"));
    assert_eq!(found["enroll_file"], "enroll-token-of-a-test");
    assert_eq!(
        found["enroll_mode"], "600 1000:1000",
        "readable only by the connector's user"
    );
    assert_eq!(found["run_dir"], "700 1000:1000");
    assert_eq!(found["mounts_file"], "[]");
    assert_eq!(found["tools"].trim(), "codex");
    assert_eq!(found["tool_marker"], "codex 1.0.0");
    assert_eq!(found["tools_dir_writable"], "no", "the tools are read-only");
    assert_eq!(
        found["workspace_writable"], "yes",
        "a volume made empty is the user's, whatever the image"
    );
    assert_eq!(found["data_writable"], "yes");
    assert_eq!(found["uid"], "1000 gid=1000");
    assert!(found["hosts"].contains("gateway.orch.internal"));
    // The container is what the request said.
    let inspected = lab.inspect(&unit).await;
    let host = inspected.host_config.unwrap();
    assert_eq!(host.init, Some(true));
    assert_eq!(
        host.network_mode.as_deref(),
        Some(format!("{}-net", unit.name).as_str()),
        "an environment stands on a network of its own"
    );
    let labels = inspected.config.unwrap().labels.unwrap();
    assert_eq!(labels["orch.system"], lab.system);
    assert_eq!(labels["orch.driver"], "docker");
    assert_eq!(labels["orch.kind"], "environment");
    let state = lab.driver.inspect(&unit).await.unwrap();
    assert_eq!(state.state, UnitStateKind::Running);
    assert!(state.started_at.is_some() && state.exit_code.is_none());
}

#[tokio::test]
async fn the_image_s_own_entrypoint_and_command_are_not_run_and_a_providers_runtime_is_confined() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("provider", "provider", |request| {
        request["spec"]["user"] = json!({ "uid": 65532, "gid": 65532 });
        request["spec"]["security"] = json!({ "read_only_rootfs": true, "no_new_privileges": true, "cap_add": [], "cap_drop": ["ALL"] });
        request["spec"]["mounts"] = json!([ { "type": "tmpfs", "target": "/tmp" } ]);
        request["tools"] = json!([]);
        request["env"] = json!({ "LAB_RUN": "echo hello > /tmp/probe && cat /tmp/probe" });
    });
    let unit = lab.run(&spec).await;
    let found = lab.launch(&unit).await;
    assert_eq!(found["ORCH_ROLES"], "provider");
    assert_eq!(
        found["ORCH_WORKSPACE_DIR"], "",
        "a provider's runtime has no workspace"
    );
    assert_eq!(
        found["root_writable"], "no",
        "the file system outside the volumes is read-only"
    );
    assert_eq!(
        found["data_writable"], "yes",
        "the state volume is the user's"
    );
    assert_eq!(found["uid"], "65532 gid=65532");
    assert_eq!(
        found["tools"].trim(),
        "",
        "the directory of tools is there and empty"
    );
    assert_eq!(
        lab.read(&unit, "/var/lib/orch/run.txt").await.as_deref(),
        Some("hello\n")
    );
    let inspected = lab.inspect(&unit).await;
    assert_eq!(
        inspected
            .host_config
            .as_ref()
            .unwrap()
            .network_mode
            .as_deref(),
        Some(format!("{}-net", unit.name).as_str()),
        "a provider's runtime stands on a network of its own too"
    );
    assert_eq!(inspected.host_config.unwrap().readonly_rootfs, Some(true));
}

#[tokio::test]
async fn files_written_to_the_workspace_survive_a_stop_and_a_successor_and_an_update_to_another_image()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    lab.have(ALPINE).await;
    let first = lab.request("environment", "first", |request| {
        request["env"] = json!({ "LAB_RUN": "echo from-the-first > $ORCH_WORKSPACE_DIR/marker.txt; echo home > $ORCH_HOME_DIR/home.txt; echo state > $ORCH_DATA_DIR/state.txt; cat /etc/os-release | head -1 > $ORCH_WORKSPACE_DIR/os-first.txt" });
    });
    let unit = lab.run(&first).await;
    assert_eq!(
        lab.driver.inspect(&unit).await.unwrap().state,
        UnitStateKind::Running
    );
    // A stop with a grace: the unit ends by the signal, which is a stop and no failure.
    lab.driver
        .stop(&unit, Duration::from_secs(5))
        .await
        .unwrap();
    let stopped = lab.driver.inspect(&unit).await.unwrap();
    assert_eq!(stopped.state, UnitStateKind::Stopped, "{stopped:?}");
    assert!(stopped.finished_at.is_some());
    // The token of that run is gone with it.
    assert_eq!(
        lab.read(&unit, "/run/orch/enroll").await.as_deref(),
        Some("")
    );
    // Started again, the same unit finds its files.
    lab.driver.start(&unit).await.unwrap();
    lab.driver
        .stop(&unit, Duration::from_secs(5))
        .await
        .unwrap();

    // A successor on the same volumes, from the same image: the files are there, the old unit is still there.
    let same_volumes = |label: &str, image: &str| {
        let volumes = first.volumes.clone();
        let image = image.to_owned();
        lab.request("environment", label, move |request| {
            request["volumes"] = serde_json::to_value(&volumes).unwrap();
            request["spec"]["image"] = json!(image);
            request["env"] = json!({ "LAB_RUN": "cat $ORCH_WORKSPACE_DIR/marker.txt > $ORCH_DATA_DIR/seen.txt; cat $ORCH_HOME_DIR/home.txt >> $ORCH_DATA_DIR/seen.txt; cat $ORCH_DATA_DIR/state.txt >> $ORCH_DATA_DIR/seen.txt; echo from-the-successor >> $ORCH_WORKSPACE_DIR/marker.txt; cat /etc/os-release | head -1 > $ORCH_WORKSPACE_DIR/os-successor.txt" });
        })
    };
    let second = same_volumes("second", BUSYBOX);
    let successor = lab.run(&second).await;
    assert_eq!(
        lab.read(&successor, "/var/lib/orch/seen.txt")
            .await
            .as_deref(),
        Some("from-the-first\nhome\nstate\n")
    );
    assert_eq!(
        lab.driver.inspect(&unit).await.unwrap().state,
        UnitStateKind::Stopped,
        "the old unit is kept until it is told to go"
    );
    lab.driver
        .stop(&successor, Duration::from_secs(5))
        .await
        .unwrap();

    // Updated to another image: the files are kept, and the unit runs the other image.
    let third = same_volumes("third", ALPINE);
    let updated = lab.run(&third).await;
    assert_eq!(
        lab.read(&updated, "/var/lib/orch/seen.txt")
            .await
            .as_deref(),
        Some("from-the-first\nfrom-the-successor\nhome\nstate\n"),
        "{}",
        lab.logs(&updated).await
    );
    let image_of = |unit: &orch_driver_contract::units::UnitRef| {
        let docker = lab.docker.clone();
        let id = unit.reference.clone();
        async move {
            docker
                .inspect_container(&id, None)
                .await
                .unwrap()
                .image
                .unwrap()
        }
    };
    assert_ne!(
        image_of(&successor).await,
        image_of(&updated).await,
        "another image"
    );
    let os = lab.read(&updated, "/workspace/os-successor.txt").await;
    assert!(
        os.as_deref().is_some_and(|text| text.contains("Alpine")),
        "the unit ran on alpine: {os:?}; log: {}",
        lab.logs(&updated).await
    );
    let marker = lab.read(&updated, "/workspace/marker.txt").await.unwrap();
    assert_eq!(
        marker,
        "from-the-first\nfrom-the-successor\nfrom-the-successor\n"
    );
}

#[tokio::test]
async fn a_unit_is_removed_with_its_volumes_kept_and_then_the_volumes_with_their_data() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "removal", |request| {
        request["env"] = json!({ "LAB_RUN": "echo precious > $ORCH_WORKSPACE_DIR/data.txt" });
    });
    let unit = lab.run(&spec).await;
    lab.driver
        .stop(&unit, Duration::from_secs(5))
        .await
        .unwrap();
    lab.driver.remove(&unit).await.unwrap();
    assert!(!lab.has_container(&spec.name).await);
    assert_eq!(
        lab.driver.inspect(&unit).await.unwrap().state,
        UnitStateKind::Absent
    );
    assert!(
        !lab.has_volume(&format!("{}-run", spec.name)).await,
        "the files of a run go with the unit"
    );
    for volume in spec.volumes.values() {
        assert!(
            lab.has_volume(&volume.name).await,
            "{} is kept",
            volume.name
        );
    }
    // A removal that is asked again is no error.
    lab.driver.remove(&unit).await.unwrap();
    // The data are still there for a unit made again on the volumes.
    let again = lab.request("environment", "removal-again", {
        let volumes = spec.volumes.clone();
        move |request| {
            request["volumes"] = serde_json::to_value(&volumes).unwrap();
            request["env"] =
                json!({ "LAB_RUN": "cat $ORCH_WORKSPACE_DIR/data.txt > $ORCH_DATA_DIR/seen.txt" });
        }
    });
    let successor = lab.run(&again).await;
    assert_eq!(
        lab.read(&successor, "/var/lib/orch/seen.txt")
            .await
            .as_deref(),
        Some("precious\n")
    );
    lab.driver
        .stop(&successor, Duration::from_secs(5))
        .await
        .unwrap();
    lab.driver.remove(&successor).await.unwrap();
    for volume in spec.volumes.values() {
        lab.driver.remove_volume(&volume.name).await.unwrap();
        assert!(
            !lab.has_volume(&volume.name).await,
            "{} is gone",
            volume.name
        );
    }
    lab.driver
        .remove_volume(&spec.volumes.values().next().unwrap().name)
        .await
        .unwrap();
}

#[tokio::test]
async fn limits_are_visible_in_the_inspection_and_are_the_limits_the_unit_runs_under() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "limits", |request| {
        request["resources"] =
            json!({ "cpu_millis": 1500, "memory_mb": 256, "pids": 321, "disk_gb": 5 });
    });
    let unit = lab.run(&spec).await;
    let inspected = lab.inspect(&unit).await;
    let host = inspected.host_config.unwrap();
    assert_eq!(host.nano_cpus, Some(1_500_000_000));
    assert_eq!(host.memory, Some(256 * 1024 * 1024));
    assert_eq!(host.memory_swap, host.memory, "no swap beyond the limit");
    assert_eq!(host.pids_limit, Some(321));
    let found = lab.launch(&unit).await;
    assert_eq!(found["cgroup_memory"], (256 * 1024 * 1024).to_string());
    assert_eq!(found["cgroup_pids"], "321");
    assert!(
        found["cgroup_cpu"].starts_with("150000 100000"),
        "{}",
        found["cgroup_cpu"]
    );
    let sample = lab.driver.stats(&unit).await.unwrap();
    assert_eq!(sample.cpu_limit_millis, Some(1500));
    assert_eq!(sample.memory_limit_bytes, Some(256 * 1024 * 1024));
    assert_eq!(
        sample.disk_limit_bytes,
        Some(5 * (1 << 30)),
        "the disk is measured against its limit where the storage cannot enforce it"
    );
    assert!(sample.memory_bytes > 0 && sample.pids >= 1);
    assert!(sample.cpu_percent >= 0.0);
}

#[tokio::test]
async fn a_unit_that_ends_by_itself_is_stopped_when_it_ended_well_and_failed_when_it_did_not() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let good = lab
        .run(&lab.request("environment", "ends-well", |request| {
            request["env"] = json!({ "LAB_EXIT": "0" })
        }))
        .await;
    let state = lab.wait_stopped(&good).await;
    assert_eq!(
        (state.state, state.exit_code),
        (UnitStateKind::Stopped, Some(0))
    );
    let bad = lab
        .run(&lab.request("environment", "ends-badly", |request| {
            request["env"] = json!({ "LAB_EXIT": "3" })
        }))
        .await;
    let state = lab.wait_stopped(&bad).await;
    assert_eq!(
        (state.state, state.exit_code),
        (UnitStateKind::Failed, Some(3))
    );
    assert_eq!(state.detail.as_deref(), Some("exited with status 3"));
    assert!(state.finished_at.is_some());
}

#[tokio::test]
async fn a_unit_asked_for_again_is_the_unit_that_exists_also_when_asked_at_the_same_moment() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "twice", |_| {});
    lab.track(&spec);
    let driver = lab.driver.clone();
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let driver = driver.clone();
        let spec = spec.clone();
        tasks.push(tokio::spawn(async move { driver.create(&spec).await }));
    }
    let mut units = Vec::new();
    for task in tasks {
        units.push(task.await.unwrap().unwrap());
    }
    assert!(units.iter().all(|unit| unit == &units[0]), "{units:?}");
    let listed = lab.driver.list(&lab.system).await.unwrap();
    assert_eq!(listed.len(), 1);
    // Asked for again with a new token while stopped, it gets the new token for its next run.
    let mut renewed = spec.clone();
    renewed.enroll_token = "a-newer-token".into();
    let again = lab.driver.create(&renewed).await.unwrap();
    assert_eq!(again, units[0]);
    assert_eq!(
        lab.read(&again, "/run/orch/enroll").await.as_deref(),
        Some("a-newer-token")
    );
}

#[tokio::test]
async fn two_units_never_run_on_one_set_of_volumes_at_once() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let first = lab.request("environment", "exclusive-a", |_| {});
    let unit = lab.run(&first).await;
    let second = lab.request("environment", "exclusive-b", {
        let volumes = first.volumes.clone();
        move |request| request["volumes"] = serde_json::to_value(&volumes).unwrap()
    });
    let successor = lab.create(&second).await;
    let refused = lab.driver.start(&successor).await.unwrap_err();
    assert!(
        matches!(refused, DriverError::Internal(ref why) if why.contains("two units never run on one volume")),
        "{refused:?}"
    );
    lab.driver
        .stop(&unit, Duration::from_secs(5))
        .await
        .unwrap();
    lab.driver.start(&successor).await.unwrap();
}

#[tokio::test]
async fn a_unit_whose_image_is_not_there_to_pull_is_refused_with_an_image_error() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "no-image", |request| {
        request["spec"]["image"] = json!("localhost:1/orch-test/missing:never");
    });
    lab.track(&spec);
    let error = lab.driver.create(&spec).await.unwrap_err();
    assert!(matches!(error, DriverError::Image(_)), "{error:?}");
    assert!(!lab.has_container(&spec.name).await);
    assert!(
        !lab.has_volume(&format!("{}-run", spec.name)).await,
        "nothing is left of a unit that was not made"
    );
}

#[tokio::test]
async fn a_volume_is_made_once_and_the_request_for_it_again_is_no_error() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let request = VolumeRequest {
        name: lab.name("volume"),
        storage: None,
        labels: [("orch.system".to_owned(), lab.system.clone())].into(),
    };
    lab.track_volume(&request.name);
    lab.driver.ensure_volume(&request).await.unwrap();
    lab.driver.ensure_volume(&request).await.unwrap();
    let volume = lab.docker.inspect_volume(&request.name).await.unwrap();
    assert_eq!(volume.labels["orch.system"], lab.system);
    assert_eq!(volume.labels["orch.driver"], "docker");
    assert_eq!(volume.labels["orch.test"], lab.token);
    let elsewhere = VolumeRequest {
        storage: Some("fast".into()),
        ..request.clone()
    };
    assert!(
        matches!(
            lab.driver.ensure_volume(&elsewhere).await,
            Err(DriverError::InvalidSpec(_))
        ),
        "the host has no such storage"
    );
    lab.driver.remove_volume(&request.name).await.unwrap();
    assert!(!lab.has_volume(&request.name).await);
}
