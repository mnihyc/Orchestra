//! A multi-machine image-only driver passes the ordinary environment contract and retains storage through failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::runtime::RuntimeSpec;
use orch_driver_contract::units::UnitStateKind;
use orch_driver_contract::volumes::StorageInfo;
use orch_testkit::doubles::cluster::{ClusterDriver, ClusterMachine};
use serde_json::json;

fn cluster() -> ClusterDriver {
    ClusterDriver::new(
        vec![
            ClusterMachine {
                id: "machine-a".into(),
                cpus: 4,
                memory_mb: 8192,
            },
            ClusterMachine {
                id: "machine-b".into(),
                cpus: 8,
                memory_mb: 16384,
            },
        ],
        vec![StorageInfo {
            name: "durable".into(),
            default: true,
            capacity_gb: 900,
            free_gb: 600,
            private_views: false,
        }],
    )
}

fn base() -> RuntimeSpec {
    serde_json::from_value(json!({
        "runtime_id":orch_types::new_id(),"kind":"environment","name":"base","driver":"cluster-test",
        "spec":{"image":"registry.example.test/agent:1"},"resources":{},"roles":["environment"],
        "gateway_url":"wss://gateway.example.test","enroll_token":"test","ca_cert":"","tools":[],
        "volumes":{"workspace":{"name":"base-workspace","storage":"durable"},"home":{"name":"base-home","storage":"durable"},"state":{"name":"base-state","storage":"durable"}},
        "mounts":[],"env":{},"labels":{}
    })).unwrap()
}

#[tokio::test]
async fn an_image_only_pool_passes_the_same_environment_suite_as_a_local_driver() {
    let driver = cluster();
    let mut report = orch_conformance::report::Report::default();
    orch_conformance::driver::run(&driver, &base(), "cluster-suite", &mut report).await;
    assert!(
        report.passed(),
        "{:?}",
        report.failures().collect::<Vec<_>>()
    );
    assert!(
        driver.volumes().is_empty(),
        "the suite removes only its owned volumes"
    );
}

#[tokio::test]
async fn a_failed_member_is_recovered_elsewhere_with_its_volume_contents_and_without_optional_features()
 {
    let driver = cluster();
    let first = orch_conformance::driver::unit_request(&base(), "cluster-recover");
    let second = orch_conformance::driver::unit_request(&base(), "cluster-recover");
    let one = driver.create(&first).await.unwrap();
    let two = driver.create(&second).await.unwrap();
    assert_ne!(driver.machine_of(&one), driver.machine_of(&two));
    driver.start(&one).await.unwrap();
    driver.start(&two).await.unwrap();
    let volume = &first.volumes[&orch_driver_contract::runtime::VolumeRole::Workspace].name;
    driver
        .write_volume(volume, "kept.txt", b"survives a machine".to_vec())
        .unwrap();
    let failed = driver.machine_of(&one).unwrap();
    driver.fail_machine(&failed);
    assert_eq!(
        driver.inspect(&one).await.unwrap().state,
        UnitStateKind::Failed
    );
    assert_eq!(
        driver.inspect(&two).await.unwrap().state,
        UnitStateKind::Running
    );
    let healthy_capacity = driver.host_resources().unwrap();
    assert!(healthy_capacity.cpus == 4 || healthy_capacity.cpus == 8);
    let mut successor = first.clone();
    successor.runtime_id = orch_types::new_id();
    successor.name = format!("{}-recovered", first.name);
    successor.labels = orch_driver_contract::runtime::unit_labels(
        "cluster-recover",
        successor.kind,
        &successor.runtime_id,
    );
    let recovered = driver.create(&successor).await.unwrap();
    driver.start(&recovered).await.unwrap();
    assert_ne!(driver.machine_of(&recovered).unwrap(), failed);
    assert_eq!(
        driver.inspect(&one).await.unwrap().state,
        UnitStateKind::Failed
    );
    driver.stop(&one, std::time::Duration::ZERO).await.unwrap();
    driver.remove(&one).await.unwrap();
    assert_eq!(
        driver.volume_files(volume)["kept.txt"],
        b"survives a machine"
    );
    assert!(
        driver.remove_volume(volume).await.is_err(),
        "a successor's active volume is protected"
    );
    let availability = driver.describe().availability();
    for feature in [
        "host_paths",
        "build",
        "archive",
        "kept_files",
        "private_views",
        "suspend",
        "host_ports",
        "live_mounts",
    ] {
        assert!(!availability[feature].supported, "{feature}");
        assert!(
            availability[feature]
                .reason
                .as_ref()
                .is_some_and(|reason| !reason.is_empty()),
            "{feature}"
        );
    }
    assert!(driver.suspend(&recovered).await.is_err());
    assert!(
        driver
            .ensure_image(&json!({"build":{"dockerfile":"FROM x"}}))
            .await
            .is_err()
    );
    driver
        .stop(&recovered, std::time::Duration::ZERO)
        .await
        .unwrap();
    driver.remove(&recovered).await.unwrap();
    driver.remove_volume(volume).await.unwrap();
    assert!(driver.volume_files(volume).is_empty());
    assert_eq!(
        driver.inspect(&two).await.unwrap().state,
        UnitStateKind::Running
    );
}
