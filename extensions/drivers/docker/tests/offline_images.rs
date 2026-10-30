//! Carried-in image content is selected locally and absent content never causes a registry fallback.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{BUSYBOX, Lab};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_docker::{DockerDriver, config::DockerConfig};
use serde_json::json;
use std::collections::BTreeMap;

#[tokio::test]
async fn a_carried_in_image_works_when_its_original_registry_cannot_be_reached() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let held = lab.docker.inspect_image(BUSYBOX).await.unwrap();
    let content = held.id.unwrap();
    let reference = "unreachable.invalid/offline/image:chosen";
    let driver = DockerDriver::connect(DockerConfig {
        offline_images: BTreeMap::from([(reference.to_owned(), content.clone())]),
        ..lab.config.clone()
    })
    .await
    .unwrap();
    let ensured = driver
        .ensure_image(&json!({"image":reference}))
        .await
        .unwrap();
    assert_eq!(ensured.digest.as_deref(), Some(content.as_str()));
    assert_eq!(
        driver.registry_digest(reference).await.unwrap(),
        Some(content)
    );
}

#[tokio::test]
async fn missing_carried_in_content_fails_locally_with_the_repair_instruction() {
    let reference = "unreachable.invalid/offline/image:missing";
    let Some(lab) = Lab::start().await else {
        return;
    };
    let driver = DockerDriver::connect(DockerConfig {
        offline_images: BTreeMap::from([(
            reference.to_owned(),
            format!("sha256:{}", "0".repeat(64)),
        )]),
        ..lab.config.clone()
    })
    .await
    .unwrap();
    let refused = driver
        .ensure_image(&json!({"image":reference}))
        .await
        .unwrap_err();
    assert!(
        matches!(refused, DriverError::Image(ref reason) if reason.contains("carried-in") && reason.contains("load its verified archive")),
        "{refused:?}"
    );
    assert_eq!(driver.registry_digest(reference).await.unwrap(), None);
}
