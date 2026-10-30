//! Real device/socket grants keep running work; first-start replay precedes enrollment and withdrawal keeps installed files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
use common::{BUSYBOX, Lab};
use orch_driver_contract::capabilities::CapabilityState;
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::units::UnitRef;
use serde_json::json;
use std::time::Duration;

async fn live_lab() -> Option<Lab> {
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: ORCH_TEST_STATIC_NODE must name this release's static orch-node");
        return None;
    };
    let lab = Lab::with(|config, _| {
        config.live_helper_image = Some(BUSYBOX.into());
        config.live_helper_binary = Some(binary.into());
    })
    .await?;
    if !lab.driver.live_helpers_supported() {
        eprintln!("not run: this host cannot run the trusted live helper");
        return None;
    }
    lab.have(BUSYBOX).await;
    Some(lab)
}

async fn wait(lab: &Lab, unit: &UnitRef, path: &str, value: &str) {
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    while lab.read(unit, path).await.as_deref() != Some(value) {
        assert!(
            tokio::time::Instant::now() < until,
            "{path} did not become {value:?}; {}",
            lab.logs(unit).await
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn state(key: &str) -> CapabilityState {
    CapabilityState {
        allowed: vec![key.into()],
        patches: std::collections::BTreeMap::from([(
            key.into(),
            if key == "kvm" {
                json!({"devices":[{"path":"/dev/kvm","permissions":"rwm"}]})
            } else {
                json!({"binds":["/var/run/docker.sock:/var/run/docker.sock"]})
            },
        )]),
        ..CapabilityState::default()
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn optional_kvm_is_inert_then_openable_without_a_restart_and_withdrawal_keeps_installed_files()
 {
    use std::os::unix::fs::MetadataExt;
    let Ok(metadata) = std::fs::metadata("/dev/kvm") else {
        eprintln!("not run: this Docker lab needs the host's KVM device");
        return;
    };
    let Some(lab) = live_lab().await else {
        return;
    };
    let major = rustix::fs::major(metadata.rdev());
    let minor = rustix::fs::minor(metadata.rdev());
    let request=lab.request("environment","live-kvm",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});request["capabilities"]=json!(state("kvm"));
        request["env"]=json!({"LAB_RUN":format!("[ -f /opt/installed-kept ] || echo installed-before-grant > /opt/installed-kept; if mknod /dev/kvm c {major} {minor} 2>/dev/null; then echo made > /opt/mknod-result; else echo refused > /opt/mknod-result; fi; ( while :; do if [ -c /dev/kvm ]; then if /kvm-probe __runtime-helper kvm-version --target /dev/kvm > /opt/kvm-version 2>/dev/null; then echo usable > /opt/kvm-seen; else echo refused > /opt/kvm-seen; fi; else echo absent > /opt/kvm-seen; fi; sleep 0.1; done ) & echo $! > /opt/long-process")});
    });
    let unit = lab.create(&request).await;
    // The trusted grant helper runs outside the unit. Give this shell-only fixture its own
    // copy of the real static program so it can prove the KVM ioctl from inside the unit.
    let probe = std::fs::read(lab.config.live_helper_binary.as_ref().unwrap()).unwrap();
    let mut archive = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(probe.len() as u64);
    header.set_mode(0o755);
    archive
        .append_data(&mut header, "kvm-probe", probe.as_slice())
        .unwrap();
    let options = bollard::query_parameters::UploadToContainerOptionsBuilder::default()
        .path("/")
        .build();
    lab.docker
        .upload_to_container(
            &unit.reference,
            Some(options),
            bollard::body_full(archive.into_inner().unwrap().into()),
        )
        .await
        .unwrap();
    lab.driver.start(&unit).await.unwrap();
    lab.wait_done(&unit).await;
    wait(&lab, &unit, "/opt/mknod-result", "refused\n").await;
    wait(&lab, &unit, "/opt/kvm-seen", "absent\n").await;
    let inspected = lab.inspect(&unit).await;
    let pid = inspected.state.unwrap().pid;
    let host = inspected.host_config.unwrap();
    assert!(
        host.cap_drop
            .unwrap()
            .iter()
            .any(|cap| cap.eq_ignore_ascii_case("MKNOD"))
    );
    assert!(
        host.device_cgroup_rules
            .unwrap()
            .contains(&format!("c {major}:{minor} rwm"))
    );
    assert!(
        host.devices.unwrap_or_default().is_empty(),
        "an environment grant is given by the helper, never by Docker's devices"
    );
    let process = lab.read(&unit, "/opt/long-process").await.unwrap();
    let mut given = request.capabilities.clone();
    given.granted.push("kvm".into());
    assert!(
        !lab.driver
            .apply_capabilities(&unit, &request.capabilities, &given)
            .await
            .unwrap()
    );
    wait(&lab, &unit, "/opt/kvm-seen", "usable\n").await;
    wait(&lab, &unit, "/opt/kvm-version", "12\n").await;
    assert_eq!(lab.inspect(&unit).await.state.unwrap().pid, pid);
    assert_eq!(lab.read(&unit, "/opt/long-process").await.unwrap(), process);
    // An ordinary start replays the environment grant before this connector script can see its first input.
    lab.driver
        .stop(&unit, Duration::from_secs(2))
        .await
        .unwrap();
    lab.driver
        .start_with_capabilities(&unit, &given)
        .await
        .unwrap();
    wait(&lab, &unit, "/opt/kvm-seen", "usable\n").await;
    assert!(
        lab.driver
            .apply_capabilities(&unit, &given, &request.capabilities)
            .await
            .unwrap(),
        "withdrawal needs a same-container restart to evict open descriptors"
    );
    lab.driver
        .stop(&unit, Duration::from_secs(2))
        .await
        .unwrap();
    lab.driver
        .start_with_capabilities(&unit, &request.capabilities)
        .await
        .unwrap();
    wait(&lab, &unit, "/opt/kvm-seen", "absent\n").await;
    assert_eq!(
        lab.inspect(&unit).await.id.as_deref(),
        Some(unit.reference.as_str())
    );
    assert_eq!(
        lab.read(&unit, "/opt/installed-kept").await.as_deref(),
        Some("installed-before-grant\n")
    );
}

#[tokio::test]
async fn the_socket_is_given_live_and_an_environment_grant_is_present_before_the_first_connector_launch()
 {
    let Some(lab) = live_lab().await else {
        return;
    };
    let request=lab.request("environment","live-socket",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});request["capabilities"]=json!(state("docker_socket"));
        request["env"]=json!({"LAB_RUN":"[ -f /opt/socket-install ] || echo kept > /opt/socket-install; ( while :; do if [ -S /var/run/docker.sock ]; then echo present > /opt/socket-seen; else echo absent > /opt/socket-seen; fi; sleep 0.1; done ) & echo $! > /opt/long-process"});
    });
    let unit = lab.run(&request).await;
    wait(&lab, &unit, "/opt/socket-seen", "absent\n").await;
    let pid = lab.inspect(&unit).await.state.unwrap().pid;
    let mut given = request.capabilities.clone();
    given.granted.push("docker_socket".into());
    assert!(
        !lab.driver
            .apply_capabilities(&unit, &request.capabilities, &given)
            .await
            .unwrap()
    );
    wait(&lab, &unit, "/opt/socket-seen", "present\n").await;
    assert_eq!(lab.inspect(&unit).await.state.unwrap().pid, pid);
    assert!(
        lab.driver
            .apply_capabilities(&unit, &given, &request.capabilities)
            .await
            .unwrap()
    );
    lab.driver
        .stop(&unit, Duration::from_secs(2))
        .await
        .unwrap();
    lab.driver
        .start_with_capabilities(&unit, &request.capabilities)
        .await
        .unwrap();
    wait(&lab, &unit, "/opt/socket-seen", "absent\n").await;
    assert_eq!(
        lab.read(&unit, "/opt/socket-install").await.as_deref(),
        Some("kept\n")
    );
    let first=lab.request("environment","socket-first",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});request["capabilities"]=json!(given);
        request["env"]=json!({"LAB_RUN":"if [ -S /var/run/docker.sock ]; then echo ready-before-launch > /opt/first-check; else echo missing > /opt/first-check; fi"});
    });
    let first_unit = lab.run(&first).await;
    wait(
        &lab,
        &first_unit,
        "/opt/first-check",
        "ready-before-launch\n",
    )
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn kvm_defaults_are_docker_devices_and_a_template_without_kvm_cannot_open_a_manually_made_node()
 {
    use std::os::unix::fs::MetadataExt;
    let Ok(metadata) = std::fs::metadata("/dev/kvm") else {
        eprintln!("not run: the Docker lab needs KVM");
        return;
    };
    let Some(lab) = live_lab().await else {
        return;
    };
    let request=lab.request("environment","base-kvm",|request| {
        let mut base=state("kvm");base.base.push("kvm".into());request["capabilities"]=json!(base);
        request["spec"]["user"]=json!({"uid":0,"gid":0});request["env"]=json!({"LAB_RUN":"if ( exec 3<> /dev/kvm ) 2>/dev/null; then echo usable > /opt/base-check; else echo refused > /opt/base-check; fi"});
    });
    let base = lab.run(&request).await;
    wait(&lab, &base, "/opt/base-check", "usable\n").await;
    assert_eq!(
        lab.inspect(&base)
            .await
            .host_config
            .unwrap()
            .devices
            .unwrap()[0]
            .path_in_container
            .as_deref(),
        Some("/dev/kvm")
    );
    let major = rustix::fs::major(metadata.rdev());
    let minor = rustix::fs::minor(metadata.rdev());
    let request=lab.request("environment","not-allowed-kvm",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});request["env"]=json!({"LAB_RUN":format!("mknod /dev/kvm c {major} {minor}; if ( exec 3<> /dev/kvm ) 2>/dev/null; then echo opened > /opt/no-kvm-check; else echo refused > /opt/no-kvm-check; fi")});
    });
    let absent = lab.run(&request).await;
    wait(&lab, &absent, "/opt/no-kvm-check", "refused\n").await;
}
