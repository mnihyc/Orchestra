//! What the driver touches and what it leaves alone on a daemon that is shared: units of another system, containers
//! and volumes that are not the driver's, and the capabilities merged into the options of a unit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use bollard::models::{ContainerCreateBody, VolumeCreateRequest};
use bollard::query_parameters::CreateContainerOptionsBuilder;
use common::{BUSYBOX, Lab};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::units::{UnitRef, UnitStateKind};
use serde_json::json;

#[tokio::test]
async fn units_are_listed_by_their_system_and_a_unit_of_another_system_is_never_listed_or_touched()
{
    let Some(lab) = Lab::start().await else {
        return;
    };
    let mine = lab.run(&lab.request("environment", "mine", |_| {})).await;
    let other_system = format!("{}-other", lab.system);
    let theirs_spec = lab.request("provider", "theirs", {
        let other_system = other_system.clone();
        move |request| request["labels"]["orch.system"] = json!(other_system)
    });
    let theirs = lab.run(&theirs_spec).await;

    let listed = lab.driver.list(&lab.system).await.unwrap();
    assert_eq!(listed, vec![mine.clone()]);
    let listed_other = lab.driver.list(&other_system).await.unwrap();
    assert_eq!(listed_other, vec![theirs.clone()]);
    assert!(
        lab.driver
            .list(&format!("{}-nobody", lab.system))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(listed[0].kind.as_str(), "environment");
    assert_eq!(listed_other[0].kind.as_str(), "provider");
    assert_eq!(
        listed[0].runtime_id.to_string(),
        lab.inspect(&mine).await.config.unwrap().labels.unwrap()["orch.runtime_id"]
    );

    // Both are real units of the driver, and the listing of one system leaves the other's running.
    assert_eq!(
        lab.driver.inspect(&theirs).await.unwrap().state,
        UnitStateKind::Running
    );
    lab.driver
        .stop(&mine, Duration::from_secs(3))
        .await
        .unwrap();
    lab.driver.remove(&mine).await.unwrap();
    assert!(lab.driver.list(&lab.system).await.unwrap().is_empty());
    assert_eq!(
        lab.driver.inspect(&theirs).await.unwrap().state,
        UnitStateKind::Running
    );
}

/// A container made by hand with the labels of a system and none of the driver's: somebody else's, whatever it says.
async fn foreign_container(lab: &Lab, label: &str) -> UnitRef {
    let name = lab.name(label);
    lab.track_container(&name);
    let runtime_id = orch_types::new_id();
    let labels = [
        ("orch.system".to_owned(), lab.system.clone()),
        ("orch.runtime_id".to_owned(), runtime_id.to_string()),
        ("orch.kind".to_owned(), "environment".to_owned()),
        ("orch.test".to_owned(), lab.token.clone()),
    ];
    let created = lab
        .docker
        .create_container(
            Some(CreateContainerOptionsBuilder::default().name(&name).build()),
            ContainerCreateBody {
                image: Some(BUSYBOX.to_owned()),
                cmd: Some(vec!["sleep".into(), "3600".into()]),
                labels: Some(labels.into()),
                ..ContainerCreateBody::default()
            },
        )
        .await
        .unwrap();
    UnitRef {
        reference: created.id,
        name,
        runtime_id,
        kind: orch_driver_contract::runtime::RuntimeKind::Environment,
    }
}

#[tokio::test]
async fn a_container_that_is_not_the_drivers_is_never_listed_started_stopped_or_removed_whatever_it_is_called()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let theirs = foreign_container(&lab, "foreign").await;
    assert!(
        lab.driver.list(&lab.system).await.unwrap().is_empty(),
        "the system label alone is not the drivers"
    );
    assert_eq!(
        lab.driver.inspect(&theirs).await.unwrap().state,
        UnitStateKind::Absent
    );
    for result in [
        lab.driver.start(&theirs).await,
        lab.driver.stop(&theirs, Duration::from_secs(1)).await,
        lab.driver.remove(&theirs).await,
    ] {
        assert!(
            matches!(result, Err(DriverError::NotFound(_))),
            "{result:?}"
        );
    }
    assert!(lab.driver.stats(&theirs).await.is_err());
    // Asked for by name instead of by identifier, the answer is the same.
    let by_name = UnitRef {
        reference: String::new(),
        ..theirs.clone()
    };
    assert!(matches!(
        lab.driver.remove(&by_name).await,
        Err(DriverError::NotFound(_))
    ));
    // And a unit made under the name of one that is not the driver's is refused and the container is as it was.
    let mut request = lab.request("environment", "foreign", |_| {});
    request.runtime_id = theirs.runtime_id;
    request
        .labels
        .insert("orch.runtime_id".into(), theirs.runtime_id.to_string());
    let refused = lab.driver.create(&request).await.unwrap_err();
    assert!(
        matches!(refused, DriverError::Internal(ref why) if why.contains("is not the unit of this runtime")),
        "{refused:?}"
    );
    let state = lab
        .docker
        .inspect_container(&theirs.reference, None)
        .await
        .unwrap()
        .state
        .unwrap();
    assert_eq!(state.running, Some(false), "it was never started");
}

#[tokio::test]
async fn a_volume_that_is_not_the_drivers_is_not_removed_by_name() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let name = lab.name("foreign-volume");
    lab.track_volume(&name);
    lab.docker
        .create_volume(VolumeCreateRequest {
            name: Some(name.clone()),
            labels: Some([("orch.test".to_owned(), lab.token.clone())].into()),
            ..VolumeCreateRequest::default()
        })
        .await
        .unwrap();
    let refused = lab.driver.remove_volume(&name).await.unwrap_err();
    assert!(matches!(refused, DriverError::NotFound(_)), "{refused:?}");
    assert!(lab.has_volume(&name).await);
}

#[tokio::test]
async fn the_patches_of_capabilities_are_merged_into_the_options_of_the_unit() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "capabilities", |request| {
        request["spec"]["security"] = json!({ "read_only_rootfs": false, "no_new_privileges": true, "cap_add": ["CHOWN"], "cap_drop": ["NET_RAW"] });
        // What the node merged in from the capabilities of the environment.
        request["spec"]["cap_add"] = json!(["SYS_PTRACE", "CHOWN"]);
        request["spec"]["security_opt"] = json!(["seccomp=unconfined"]);
        request["spec"]["group_add"] = json!(["1234"]);
        request["spec"]["device_requests"] = json!([]);
        request["spec"]["network_mode"] = json!("none");
    });
    let unit = lab.create(&spec).await;
    let host = lab.inspect(&unit).await.host_config.unwrap();
    assert_eq!(
        host.cap_add.unwrap(),
        ["CHOWN", "SYS_PTRACE"],
        "lists are united"
    );
    assert_eq!(host.cap_drop.unwrap(), ["NET_RAW"]);
    let options = host.security_opt.unwrap();
    assert!(
        options.contains(&"no-new-privileges".to_owned())
            && options.contains(&"seccomp=unconfined".to_owned()),
        "{options:?}"
    );
    assert_eq!(host.group_add.unwrap(), ["1234"]);
    assert_eq!(
        host.network_mode.as_deref(),
        Some("none"),
        "the last network mode that is given"
    );
    assert_eq!(host.privileged, Some(false));
}

#[tokio::test]
async fn a_spec_the_driver_cannot_make_a_unit_of_is_refused_before_anything_is_made() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "invalid", |request| {
        request["spec"]["mounts"] = json!([ { "type": "tmpfs", "target": "/opt/orch/bin" } ]);
    });
    lab.track(&spec);
    assert!(matches!(
        lab.driver.create(&spec).await,
        Err(DriverError::InvalidSpec(_))
    ));
    assert!(matches!(
        lab.driver.validate(spec.kind, &spec.spec).await,
        Err(DriverError::InvalidSpec(ref why)) if why.contains("a place of Orch's own")
    ));
    assert!(!lab.has_container(&spec.name).await);
    for volume in spec.volumes.values() {
        assert!(
            !lab.has_volume(&volume.name).await,
            "{} was made for a unit that was refused",
            volume.name
        );
    }
    let fine = lab.request("environment", "valid", |_| {});
    assert_eq!(lab.driver.validate(fine.kind, &fine.spec).await, Ok(()));
}
