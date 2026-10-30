//! Real Docker proof of the operator's hooks through the node: a `post_up` is told the unit's own network from the pool, its subnet and the unit's address there, and a failing `pre_up` keeps a unit from starting, with its message.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
#[path = "../../../../extensions/drivers/docker/tests/common/mod.rs"]
mod docker_common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use common::Rig;
use docker_common::{Lab, address_on};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::sources::ImageEnsured;
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest};
use orch_driver_docker::DockerDriver;
use orch_link::commands::Progress;
use orch_node_core::drivers::{DriverExtras, DriverHost, HostDriver};
use orch_node_core::hooks::HookSettings;
use orch_node_core::images::ImageProgress;
use orch_types::Json;
use serde_json::json;

struct Extras(Arc<DockerDriver>);

#[async_trait]
impl DriverExtras for Extras {
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        Some(self.0.storages())
    }
    async fn ensure_image(
        &self,
        spec: &Json,
        _progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.0.ensure_image(spec).await
    }
    async fn volume_exists(&self, request: &VolumeRequest) -> Result<bool, DriverError> {
        self.0.volume_exists(request).await
    }
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn a_post_up_is_told_its_units_own_subnet_and_address_and_a_failing_pre_up_keeps_a_unit_from_starting()
 {
    let lab = Lab::start()
        .await
        .expect("the hooks' proof needs the real Docker daemon");
    let dir = tempfile::tempdir().unwrap();
    let told_path = dir.path().join("post_up.json");
    let mut rig = Rig::new();
    rig.settings.system_name = lab.system.clone();
    rig.settings.drivers = vec!["docker".to_owned()];
    rig.settings.hooks = HookSettings {
        pre_up: Some(script(
            dir.path(),
            "pre-up",
            "case \"$(cat)\" in *refused*) echo 'routing through the tunnel is not ready' >&2; exit 5;; esac",
        )),
        post_up: Some(script(
            dir.path(),
            "post-up",
            &format!("cat > '{}'", told_path.display()),
        )),
        pre_down: None,
        post_down: None,
        timeout: Duration::from_secs(30),
    };
    rig.have_connector();
    let driver = Arc::new(lab.driver.clone());
    let host = HostDriver::new(driver.clone()).with_extras(Arc::new(Extras(driver)));
    let drivers = DriverHost::of(&rig.settings, rig.desk.clone(), vec![host]);
    rig.start_with(drivers).await;
    let token = lab.token.clone();
    let make = |rig: &Rig, label: &str| {
        let spec = lab.request("environment", label, |request| {
            request["name"] = json!(format!("orchprobe-{token}-{label}"));
            request["tools"] = json!([]);
            request["enroll_token"] = json!(rig.token());
            request["ca_cert"] = json!(rig.authority.certificate().pem());
        });
        lab.track(&spec);
        spec
    };

    // A unit that comes up: its post_up is told the network of its own the unit stands on.
    let hooked = make(&rig, "hooked");
    rig.made(&hooked).await;
    let started = rig.start_runtime(hooked.runtime_id).await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    let told: Json = serde_json::from_slice(&std::fs::read(&told_path).unwrap()).unwrap();
    assert_eq!(told["event"], "post_up");
    assert_eq!(told["reason"], "start");
    assert_eq!(told["unit"]["name"], hooked.name.as_str());
    assert_eq!(told["network"]["egress"], "open");
    let own = format!("{}-net", hooked.name);
    let network = told["network"]["networks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|network| network["name"] == own.as_str())
        .unwrap_or_else(|| panic!("the unit's own network is told: {told}"))
        .clone();
    assert_eq!(network["role"], "unit");
    let subnet = network["subnet"].as_str().unwrap().to_owned();
    assert!(
        subnet.starts_with("10.199.") && subnet.ends_with("/28"),
        "a /28 of the shipped pool: {subnet}"
    );
    let inspected = lab.docker.inspect_network(&own, None).await.unwrap();
    let made_at = inspected
        .ipam
        .and_then(|ipam| ipam.config)
        .and_then(|configs| configs.into_iter().find_map(|config| config.subnet))
        .unwrap();
    assert_eq!(
        subnet, made_at,
        "the hook is told the subnet the daemon gave the network"
    );
    assert_eq!(
        network["address"].as_str().unwrap(),
        address_on(&lab.docker, &hooked.name, &own).await,
        "the hook is told the unit's own address there"
    );
    assert!(
        network["gateway"].is_string(),
        "an open unit's network has the host's address"
    );

    // A unit whose pre_up fails does not start, and its start says what the hook printed.
    let refused = make(&rig, "refused");
    rig.made(&refused).await;
    let Progress::Failed(error) = rig.start_runtime(refused.runtime_id).await else {
        panic!("a unit started past a failing pre_up");
    };
    assert_eq!(error.code, "hook.failed");
    assert!(
        error
            .detail
            .contains("routing through the tunnel is not ready"),
        "{}",
        error.detail
    );
    let container = lab
        .docker
        .inspect_container(&refused.name, None)
        .await
        .unwrap();
    assert_ne!(container.state.and_then(|state| state.running), Some(true));

    // Removed through the node, a unit's network goes with it.
    assert!(matches!(
        rig.stop_runtime(hooked.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(matches!(
        rig.remove_runtime(hooked.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(!lab.has_network(&own).await);
    assert!(matches!(
        rig.remove_runtime(refused.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(!lab.has_container(&refused.name).await);
}
