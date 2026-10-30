//! What the driver declares about itself, checked against the daemon it is connected to, and the one thing the
//! declaration promises that only a running unit can show: that its processes can be held still and let go.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{Lab, my_ids};
use orch_driver_contract::descriptor::{Confinement, Recreate, ResourceLimits};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::mounts::MountPlacement;
use orch_driver_contract::runtime::RuntimeKind;
use orch_driver_contract::sources::SourceKind;
use orch_driver_contract::units::UnitStateKind;
use serde_json::json;

/// What is wrong with `value` by a schema, for the parts of JSON Schema the driver's schemas use: kinds, members,
/// required members, no other members, items, enumerations and alternatives. A bound or a pattern is not checked.
fn faults(schema: &serde_json::Value, value: &serde_json::Value, at: &str) -> Vec<String> {
    if let Some(alternatives) = schema.get("anyOf").and_then(serde_json::Value::as_array) {
        let all: Vec<Vec<String>> = alternatives
            .iter()
            .map(|alternative| faults(alternative, value, at))
            .collect();
        if all.iter().any(Vec::is_empty) {
            return Vec::new();
        }
        return all.into_iter().flatten().collect();
    }
    let mut found = Vec::new();
    if let Some(allowed) = schema.get("enum").and_then(serde_json::Value::as_array)
        && !allowed.contains(value)
    {
        found.push(format!("{at} is not one of the values"));
    }
    let Some(kind) = schema.get("type").and_then(serde_json::Value::as_str) else {
        return found;
    };
    let fits = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => true,
    };
    if !fits {
        let word = if kind == "integer" { "a number" } else { kind };
        found.push(format!(
            "{at} is not {}",
            if word == "a number" {
                word.to_owned()
            } else {
                format!("a {word}")
            }
        ));
        return found;
    }
    if let Some(members) = value.as_object() {
        let known = schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        for (name, member) in members {
            match known.and_then(|known| known.get(name)) {
                Some(inner) => found.extend(faults(inner, member, &format!("{at}.{name}"))),
                None if schema.get("additionalProperties") == Some(&json!(false)) => {
                    found.push(format!("{at}.{name} is not a member"));
                }
                None => {}
            }
        }
        for name in schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            if !members.contains_key(name) {
                found.push(format!("{at}.{name} is missing"));
            }
        }
    }
    if let (Some(items), Some(inner)) = (value.as_array(), schema.get("items")) {
        for (position, item) in items.iter().enumerate() {
            found.extend(faults(inner, item, &format!("{at}[{position}]")));
        }
    }
    found
}

#[tokio::test]
async fn the_declaration_is_complete_and_says_of_the_daemon_what_the_daemon_says_of_itself() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let descriptor = lab.driver.describe();
    descriptor.check().unwrap();
    let info = lab.docker.info().await.unwrap();
    assert_eq!(descriptor.name, "docker");
    assert!(
        descriptor.makes_units_for(RuntimeKind::Environment)
            && descriptor.makes_units_for(RuntimeKind::Provider)
    );
    assert_eq!(descriptor.confinement, Confinement::Isolated);
    assert_eq!(descriptor.recreate, Recreate::Replace);
    assert!(descriptor.volumes && descriptor.storages);
    assert_eq!(descriptor.mounts, MountPlacement::Placed);
    for source in [SourceKind::Image, SourceKind::Build, SourceKind::Archive] {
        assert!(descriptor.has_source(source), "{source:?}");
    }
    // The runtimes are those the daemon has, and a class it does not have is not accepted.
    let mut has: Vec<String> = info.runtimes.unwrap_or_default().into_keys().collect();
    has.sort();
    let mut says = descriptor.runtime_classes.clone().unwrap_or_default();
    says.sort();
    assert_eq!(says, has);
    assert!(descriptor.accepts_runtime_class(Some("runc")));
    assert!(descriptor.accepts_runtime_class(None));
    assert!(!descriptor.accepts_runtime_class(Some("orch-test-no-such-runtime")));
    // The limits are enforced where the kernel enforces them and only reported where it does not.
    let enforced = info.memory_limit == Some(true)
        && info.cpu_cfs_quota == Some(true)
        && info.pids_limit == Some(true);
    assert_eq!(
        descriptor.resource_limits.cpu,
        if enforced {
            ResourceLimits::Enforced
        } else {
            ResourceLimits::Reported
        }
    );
    // What the interface may show of the daemon is the daemon's own account.
    assert_eq!(
        descriptor.extensions.get("x_daemon").unwrap()["version"],
        json!(info.server_version.unwrap())
    );
    // The schema of each pool takes the spec of a small unit and refuses a member it does not know and a value of
    // the wrong kind; the driver checks the rest.
    for (kind, label) in [
        (RuntimeKind::Environment, "environment"),
        (RuntimeKind::Provider, "provider"),
    ] {
        let schema = &descriptor.spec_schemas[&kind];
        let request = lab.request(label, "declared", |_| {});
        assert_eq!(faults(schema, &request.spec, ""), Vec::<String>::new());
        let mut typo = request.spec.clone();
        typo["memry_mb"] = json!(512);
        assert_eq!(faults(schema, &typo, ""), [".memry_mb is not a member"]);
        let mut wrong = request.spec.clone();
        wrong["resources"] = json!({ "memory_mb": "lots" });
        assert_eq!(
            faults(schema, &wrong, ""),
            [".resources.memory_mb is not a number"]
        );
        lab.driver.validate(kind, &request.spec).await.unwrap();
    }
    // A runtime the daemon does not have is not one the host is offered the spec for, and a unit made from it is
    // refused with nothing left behind.
    let request = lab.request("environment", "no-runtime", |request| {
        request["spec"]["runtime"] = json!("orch-test-no-such-runtime");
    });
    assert!(!descriptor.accepts_runtime_class(request.spec["runtime"].as_str()));
    lab.track(&request);
    let refused = lab.driver.create(&request).await.unwrap_err();
    assert!(
        matches!(refused, DriverError::InvalidSpec(ref why) if why.contains("orch-test-no-such-runtime")),
        "{refused:?}"
    );
    assert!(!lab.has_container(&request.name).await);
    assert!(!lab.has_volume(&format!("{}-run", request.name)).await);
}

#[tokio::test]
async fn a_unit_is_held_still_and_let_go_and_stopping_it_while_held_works() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    assert!(lab.driver.describe().suspend, "this daemon can freeze");
    let (uid, gid) = my_ids(lab.dir.path());
    let spec = lab.request("environment", "held", |request| {
        request["spec"]["user"] = json!({ "uid": uid, "gid": gid });
        request["env"] = json!({
            "LAB_RUN": "( i=0; while true; do i=$((i+1)); echo $i > $ORCH_DATA_DIR/count.txt; sleep 0.1; done ) &"
        });
    });
    let unit = lab.run(&spec).await;
    let count = |unit: &orch_driver_contract::units::UnitRef| {
        let lab = &lab;
        let unit = unit.clone();
        async move {
            lab.read(&unit, "/var/lib/orch/count.txt")
                .await
                .and_then(|text| text.trim().parse::<u64>().ok())
                .unwrap_or(0)
        }
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while count(&unit).await < 3 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the unit does not count"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    lab.driver.suspend(&unit).await.unwrap();
    let state = lab.driver.inspect(&unit).await.unwrap();
    assert_eq!(
        (state.state, state.detail.as_deref()),
        (UnitStateKind::Running, Some("paused"))
    );
    let held = count(&unit).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        count(&unit).await,
        held,
        "a unit that is held does not go on"
    );
    lab.driver.resume(&unit).await.unwrap();
    let state = lab.driver.inspect(&unit).await.unwrap();
    assert_eq!(state.state, UnitStateKind::Running);
    assert_ne!(state.detail.as_deref(), Some("paused"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while count(&unit).await <= held {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the unit does not go on"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A unit that is held can be stopped.
    lab.driver.suspend(&unit).await.unwrap();
    lab.driver
        .stop(&unit, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        lab.driver.inspect(&unit).await.unwrap().state,
        UnitStateKind::Stopped
    );
    // And there is nothing to hold in a unit that is not there.
    lab.driver.remove(&unit).await.unwrap();
    assert!(matches!(
        lab.driver.suspend(&unit).await,
        Err(DriverError::NotFound(_))
    ));
}
