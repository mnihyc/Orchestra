//! A node builds a context-free, multi-stage Dockerfile with the real Docker driver.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
#[path = "../../../../extensions/drivers/docker/tests/common/mod.rs"]
mod docker_common;

use std::sync::Arc;

use common::Rig;
use docker_common::{BUSYBOX, Lab};
use orch_driver_contract::sources::Source;
use orch_driver_docker::build::{definition_alias, spec_alias};
use orch_link_contract::commands::{NodeImageEnsure, NodeImageEnsureResult, SerialKey};
use orch_node_core::drivers::{DriverHost, HostDriver};
use serde_json::json;

#[tokio::test]
async fn a_node_builds_a_context_free_developer_style_recipe_on_a_fresh_host() {
    let Some(lab) = Lab::start().await else {
        // The in-memory node test covers the same source when no daemon is reachable.
        return;
    };
    let recipe = format!(
        "FROM {BUSYBOX} AS orch-base\n\
         RUN printf '%s\\n' 'notice' > /notice\n\
         FROM orch-base\n\
         ARG VERSION\n\
         RUN printf '%s\\n' '#!/bin/sh' 'exit 0' > /helper && chmod 0755 /helper && test -s /notice && test -x /helper\n\
         LABEL org.opencontainers.image.version=\"$VERSION\"\n"
    );
    let spec = json!({ "build": {
        "dockerfile": recipe, "context": null, "args": { "VERSION": "test" }, "target": null
    } });
    let Source::Build(build) = Source::from_spec(&spec).unwrap() else {
        panic!("build")
    };
    let definition = definition_alias(&lab.config, &build.definition_hash(None));
    let alias = spec_alias(&lab.config, &build);
    lab.track_image(&definition);
    lab.track_image(&alias);
    let mut rig = Rig::new();
    rig.settings.system_name = lab.system.clone();
    rig.settings.drivers = vec!["docker".into()];
    let host = HostDriver::new(Arc::new(lab.driver.clone()));
    let drivers = DriverHost::of(&rig.settings, rig.desk.clone(), vec![host]);
    rig.start_with(drivers).await;

    let ensured = rig
        .succeed(
            &SerialKey::Image("built-in recipe".into()),
            &NodeImageEnsure {
                driver: "docker".into(),
                spec: spec.clone(),
                job_id: None,
            },
        )
        .await;
    let result: NodeImageEnsureResult = serde_json::from_value(ensured).unwrap();
    assert!(result.digest.is_some());
    let record = result.built.unwrap();
    assert_eq!(record.bases.len(), 1);
    assert_eq!(record.bases[0].reference, BUSYBOX);
    let labels = lab
        .docker
        .inspect_image(&alias)
        .await
        .unwrap()
        .config
        .unwrap()
        .labels
        .unwrap();
    assert_eq!(labels["org.opencontainers.image.version"], "test");
    rig.stop().await;
}
