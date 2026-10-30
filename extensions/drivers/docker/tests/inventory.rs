//! The real daemon inventory removes exact owned objects while preserving other installations and dependencies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::Lab;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::resources::{ResourceKind, ResourceRef};
use orch_driver_contract::volumes::VolumeRequest;
use serde_json::json;
use std::time::Duration;

/// The ordinary grace period is ample for the fixture's sleeping connector to exit.
const STOP_GRACE: Duration = Duration::from_secs(5);

#[tokio::test]
async fn a_stopped_units_data_stays_in_use_and_only_objects_of_the_selected_installation_can_be_removed()
 {
    let lab = Lab::start()
        .await
        .expect("the inventory check requires a Docker daemon");
    let spec = lab.request("environment", "inventory", |_| {});
    let unit = lab.run(&spec).await;
    let foreign = VolumeRequest {
        name: lab.name("other-system"),
        storage: None,
        labels: [("orch.system".into(), "another-installation".into())].into(),
    };
    lab.track_volume(&foreign.name);
    lab.driver.ensure_volume(&foreign).await.unwrap();
    let inventory = lab.driver.resources(&lab.system).await.unwrap();
    let networks: Vec<_> = inventory
        .iter()
        .filter(|item| item.resource.kind == ResourceKind::Network)
        .map(|item| item.resource.clone())
        .collect();
    assert!(
        !networks.is_empty(),
        "the fixture exposes its owned networks as independent plan items"
    );
    let running = inventory
        .iter()
        .find(|item| item.resource.kind == ResourceKind::Unit && item.resource.id == unit.reference)
        .unwrap();
    assert!(running.busy);
    assert!(
        !inventory
            .iter()
            .any(|item| item.resource.id == foreign.name)
    );
    assert!(matches!(
        lab.driver
            .remove_resource(&lab.system, &running.resource)
            .await,
        Err(DriverError::Busy(_))
    ));
    let volume = inventory
        .iter()
        .find(|item| {
            item.resource.kind == ResourceKind::Volume
                && spec
                    .volumes
                    .values()
                    .any(|owned| owned.name == item.resource.id)
        })
        .unwrap();
    lab.driver
        .stop_resource(&lab.system, &running.resource, STOP_GRACE)
        .await
        .unwrap();
    assert!(matches!(
        lab.driver
            .remove_resource(&lab.system, &volume.resource)
            .await,
        Err(DriverError::Busy(_))
    ));
    assert!(matches!(
        lab.driver
            .remove_resource("another-installation", &running.resource)
            .await,
        Err(DriverError::NotFound(_))
    ));
    lab.driver
        .remove_resource(&lab.system, &running.resource)
        .await
        .unwrap();
    assert!(!lab.has_container(&unit.name).await);
    assert!(lab.has_volume(&volume.resource.id).await);
    let remaining = lab.driver.resources(&lab.system).await.unwrap();
    assert!(
        networks
            .iter()
            .all(|network| remaining.iter().any(|item| item.resource == *network)),
        "selecting only a unit never erases the plan's kept networks"
    );
    lab.driver
        .remove_resource(&lab.system, &volume.resource)
        .await
        .unwrap();
    assert!(!lab.has_volume(&volume.resource.id).await);
    assert!(matches!(
        lab.driver
            .remove_resource(
                &lab.system,
                &ResourceRef {
                    kind: ResourceKind::Volume,
                    id: foreign.name.clone()
                }
            )
            .await,
        Err(DriverError::NotFound(_))
    ));
    assert!(
        lab.has_volume(&foreign.name).await,
        "another installation's data remains intact"
    );
}

#[tokio::test]
async fn an_unused_build_with_multiple_owned_names_is_removed_without_forcing_or_pruning_other_images()
 {
    let lab = Lab::with(|config, _| {
        let token = config.labels["orch.test"].clone();
        config
            .labels
            .insert("orch.system".into(), format!("orch-test-{token}"));
    })
    .await
    .expect("the inventory check requires a Docker daemon");
    let built = lab.driver.ensure_image(&json!({"build":{"dockerfile":"FROM busybox:latest\nRUN echo inventory-fixture > /inventory-fixture\n"}})).await.unwrap();
    let id = built.digest.unwrap();
    lab.docker
        .tag_image(
            &id,
            Some(
                bollard::query_parameters::TagImageOptionsBuilder::default()
                    .repo(&lab.config.image_repository)
                    .tag("inventory-copy")
                    .build(),
            ),
        )
        .await
        .unwrap();
    assert!(
        lab.docker
            .inspect_image(&id)
            .await
            .unwrap()
            .repo_tags
            .unwrap()
            .len()
            >= 2
    );
    let inventory = lab.driver.resources(&lab.system).await.unwrap();
    let build = inventory
        .iter()
        .find(|item| item.resource.kind == ResourceKind::Build && item.resource.id == id)
        .unwrap();
    assert!(!build.busy);
    assert!(
        lab.docker
            .inspect_image(&id)
            .await
            .unwrap()
            .size
            .is_some_and(|size| size > 0)
    );
    assert_eq!(
        build.size_bytes, None,
        "a virtual image size must not become exclusive physical bytes or be added twice through shared layers"
    );
    let base = lab.docker.inspect_image("busybox:latest").await.unwrap().id;
    lab.driver
        .remove_resource(&lab.system, &build.resource)
        .await
        .unwrap();
    assert!(lab.docker.inspect_image(&id).await.is_err());
    assert_eq!(
        lab.docker.inspect_image("busybox:latest").await.unwrap().id,
        base
    );
}

#[tokio::test]
async fn a_helper_without_a_runtime_row_stops_and_removes_only_under_its_own_installation() {
    let lab = Lab::start()
        .await
        .expect("the inventory check requires a Docker daemon");
    let name = lab.name("orphan-helper");
    lab.track_container(&name);
    let made = lab
        .docker
        .create_container(
            Some(
                bollard::query_parameters::CreateContainerOptionsBuilder::default()
                    .name(&name)
                    .build(),
            ),
            bollard::models::ContainerCreateBody {
                image: Some(common::BUSYBOX.into()),
                cmd: Some(vec!["sleep".into(), "3600".into()]),
                labels: Some(std::collections::HashMap::from([
                    ("orch.system".into(), lab.system.clone()),
                    ("orch.driver".into(), "docker".into()),
                ])),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    lab.docker
        .start_container(
            &made.id,
            None::<bollard::query_parameters::StartContainerOptions>,
        )
        .await
        .unwrap();
    let resource = lab
        .driver
        .resources(&lab.system)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.resource.id == made.id)
        .unwrap();
    assert!(resource.busy);
    assert!(matches!(
        lab.driver
            .stop_resource("another-installation", &resource.resource, STOP_GRACE)
            .await,
        Err(DriverError::NotFound(_))
    ));
    assert!(lab.has_container(&name).await);
    lab.driver
        .stop_resource(&lab.system, &resource.resource, STOP_GRACE)
        .await
        .unwrap();
    lab.driver
        .remove_resource(&lab.system, &resource.resource)
        .await
        .unwrap();
    assert!(!lab.has_container(&name).await);
}
