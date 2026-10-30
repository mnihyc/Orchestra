//! The shipped node loader admits host and supervised external drivers while unknown names and forged declarations refuse honestly.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

use orch_node_core::drivers::DriverHost;
use orch_node_core::images::BlobDesk;
use orch_node_core::settings::NodeSettings;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_node_loads_the_builtin_host_and_a_configured_external_driver_through_the_common_contract()
 {
    let dir = tempfile::tempdir().unwrap();
    let mut config = orch_config::ServerConfig::default();
    config.node.drivers = vec!["host".into(), "unregistered".into()];
    config.node.data_dir = dir.path().to_string_lossy().into_owned();
    config.node.host.scopes = false;
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins/driver/driver.py");
    config.plugin.push(orch_config::server::Plugin {
        kind: "driver".into(),
        name: "processes".into(),
        command: vec![
            "python3".into(),
            example.to_string_lossy().into_owned(),
            "--base".into(),
            dir.path().join("external").to_string_lossy().into_owned(),
        ],
        ..Default::default()
    });
    let settings = NodeSettings::from_config(&config);
    let host = DriverHost::load(
        &settings,
        orch_node::composition::driver_registrations(&config, "0.1.0").unwrap(),
        Arc::new(BlobDesk::new()),
    )
    .await;
    assert!(host.get("host").is_some());
    assert!(host.get("processes").is_some(), "{:?}", host.failures());
    assert!(host.get("unregistered").is_none());
    assert!(host.failures()["unregistered"].contains("no such driver"));
    let external = host.get("processes").unwrap();
    assert_eq!(external.describe().name, "processes");
    assert!(
        external
            .contract()
            .list("node-driver-test")
            .await
            .unwrap()
            .is_empty()
    );
    let id = orch_types::new_id();
    let spec:orch_driver_contract::runtime::RuntimeSpec=serde_json::from_value(serde_json::json!({
        "runtime_id":id,"kind":"environment","name":orch_driver_contract::runtime::unit_name(orch_driver_contract::runtime::RuntimeKind::Environment,&id),"driver":"processes",
        "spec":{"command":["sleep","600"]},"resources":{},"roles":["environment"],"gateway_url":"wss://gateway.example.invalid","enroll_token":"test-token","ca_cert":"test-ca","tools":[],
        "volumes":{"workspace":{"name":format!("ws-{}",id.simple()),"storage":null},"home":{"name":format!("home-{}",id.simple()),"storage":null},"state":{"name":format!("state-{}",id.simple()),"storage":null}},
        "mounts":[],"env":{},"labels":{"orch.system":"node-driver-test","orch.runtime_id":id,"orch.kind":"environment"}
    })).unwrap();
    let unit = external.contract().create(&spec).await.unwrap();
    assert_eq!(external.contract().create(&spec).await.unwrap(), unit);
    external.contract().start(&unit).await.unwrap();
    assert_eq!(
        external.contract().inspect(&unit).await.unwrap().state,
        orch_driver_contract::units::UnitStateKind::Running
    );
    assert_eq!(
        external
            .contract()
            .list("node-driver-test")
            .await
            .unwrap()
            .as_slice(),
        std::slice::from_ref(&unit)
    );
    external
        .contract()
        .stop(&unit, std::time::Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        external.contract().inspect(&unit).await.unwrap().state,
        orch_driver_contract::units::UnitStateKind::Stopped
    );
    external.contract().remove(&unit).await.unwrap();
    assert_eq!(
        external.contract().inspect(&unit).await.unwrap().state,
        orch_driver_contract::units::UnitStateKind::Absent
    );
    let mut collision = config.clone();
    collision.plugin[0].name = "host".into();
    assert!(orch_node::composition::driver_registrations(&collision, "0.1.0").is_err());
}
