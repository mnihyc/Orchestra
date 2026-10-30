//! Real Docker images survive an archived owner's restart and disappear when a restarted node sweeps an orphan.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
#[path = "../../../../extensions/drivers/docker/tests/common/mod.rs"]
mod docker_common;

use common::Rig;
use docker_common::{BUSYBOX, Lab};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::kept::KeptOwner;
use orch_link::commands::Progress;
use orch_link_contract::commands::{NodeKeptImage, NodeKeptSweep, SerialKey};
use orch_node_core::drivers::{DriverHost, HostDriver};
use orch_types::new_id;
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn a_node_that_stopped_before_cleanup_sweeps_its_orphan_and_a_purge_removes_the_archived_image()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    lab.have(BUSYBOX).await;
    let environment = new_id();
    let spec = lab.request("environment", "node-kept", |spec| {
        spec["spec"]["user"] = json!({"uid":0,"gid":0});
        spec["env"] = json!({"LAB_RUN":"echo installed > /opt/swept-program"});
    });
    let unit = lab.run(&spec).await;
    lab.driver
        .stop(&unit, Duration::from_secs(2))
        .await
        .unwrap();
    let image = lab
        .driver
        .keep_files(&unit, environment, 2, new_id())
        .await
        .unwrap();
    lab.track_image(&image.reference);
    lab.driver.remove(&unit).await.unwrap();
    let mut rig = Rig::new();
    rig.settings.system_name = lab.system.clone();
    rig.settings.drivers = vec!["docker".into()];
    let drivers = DriverHost::of(
        &rig.settings,
        rig.desk.clone(),
        vec![HostDriver::new(Arc::new(lab.driver.clone()))],
    );
    rig.start_with(drivers.clone()).await;
    let epoch = rig
        .role
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .ledger()
        .kept_epoch();
    assert!(matches!(
        rig.send(
            &SerialKey::None,
            &NodeKeptSweep {
                epoch,
                owners: vec![KeptOwner {
                    environment,
                    generation: image.generation,
                    capture: Some(image.capture)
                }]
            }
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert!(
        lab.docker.inspect_image(&image.reference).await.is_ok(),
        "archive retains the installed files without a unit"
    );
    rig.stop().await;
    rig.start_with(drivers.clone()).await;
    let epoch = rig
        .role
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .ledger()
        .kept_epoch();
    assert!(matches!(
        rig.send(
            &SerialKey::None,
            &NodeKeptSweep {
                epoch,
                owners: vec![]
            }
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert!(
        lab.docker.inspect_image(&image.reference).await.is_err(),
        "the next node sweep removes a name no generation owns"
    );
    assert!(
        lab.docker.inspect_image(BUSYBOX).await.is_ok(),
        "the shared base is not pruned"
    );
    let unit = lab.run(&spec).await;
    lab.driver
        .stop(&unit, Duration::from_secs(2))
        .await
        .unwrap();
    let archived = lab
        .driver
        .keep_files(&unit, environment, 3, new_id())
        .await
        .unwrap();
    lab.track_image(&archived.reference);
    lab.driver.remove(&unit).await.unwrap();
    assert!(matches!(
        rig.send(
            &SerialKey::Image(archived.reference.clone()),
            &NodeKeptImage {
                driver: "docker".into(),
                image: archived.clone()
            }
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert!(lab.docker.inspect_image(&archived.reference).await.is_err());
    rig.stop().await;
}
