//! The commands about runtimes, on the in-memory hub with the scripted controller and the in-memory driver: a unit is made, started, stopped, recreated as a successor on the same volumes, and removed, and the system is told what became of it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{PATIENCE, Rig, SYSTEM};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::{RuntimeKind, VolumeRole};
use orch_driver_contract::units::UnitStateKind;
use orch_link::commands::Progress;
use orch_link_contract::commands::{
    NodeCapabilitiesApply, NodeRuntimeStart, NodeRuntimeStop, NodeVolumeRemove, SerialKey,
};
use orch_node_core::ledger::Ledger;
use orch_testkit::doubles::driver::{MemoryDriver, Operation};
use orch_types::new_id;
use serde_json::json;

#[tokio::test]
async fn an_optional_live_grant_cannot_exceed_the_machines_capability_ceiling() {
    use orch_driver_contract::sources::SourceKind;
    use orch_link_contract::policy::{MachineDrivers, PolicyKey};
    use std::sync::Arc;

    let driver = MemoryDriver::new("memory").with_sources(vec![SourceKind::Image]);
    let mut descriptor = driver.describe();
    descriptor
        .live_changes
        .capabilities
        .push("docker_socket".into());
    let mut rig = Rig::with_driver(driver.with_descriptor(descriptor));
    rig.have_connector();
    rig.start().await;
    let policy = Arc::new(
        orch_connector_core::policy::PolicyStore::host(
            orch_config::ConnectorConfig::default(),
            None,
        )
        .unwrap(),
    );
    let mut drivers = MachineDrivers::managed();
    drivers.images.capabilities = vec!["kvm".into()];
    policy
        .set_here(PolicyKey::Drivers, &json!(&drivers))
        .unwrap();
    rig.role().runtimes().set_policy(Arc::clone(&policy));

    let mut spec = rig.spec(RuntimeKind::Environment, "live-ceiling");
    spec.capabilities = serde_json::from_value(json!({
        "allowed":["docker_socket"], "base":[], "granted":[],
        "patches":{"docker_socket":{"binds":["/var/run/docker.sock:/var/run/docker.sock"]}}
    }))
    .unwrap();
    rig.made(&spec).await;
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    rig.driver.clear_calls();
    let mut granted = spec.capabilities.clone();
    granted.granted.push("docker_socket".into());
    let error = rig
        .refused(
            &SerialKey::Runtime(spec.runtime_id),
            &NodeCapabilitiesApply {
                runtime_id: spec.runtime_id,
                capabilities: json!(granted),
            },
        )
        .await;
    assert_eq!(error.code, "unsupported");
    assert!(error.detail.contains("docker_socket"));
    assert!(rig.driver.targets_of(Operation::Stop).is_empty());
    assert!(rig.driver.targets_of(Operation::Start).is_empty());
    assert!(
        rig.role()
            .ledger()
            .capabilities(spec.runtime_id)
            .unwrap()
            .desired
            .granted
            .is_empty()
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_saved_capability_is_not_replayed_after_the_machine_owner_disallows_it() {
    use orch_driver_contract::sources::SourceKind;
    use orch_link_contract::policy::{MachineDrivers, PolicyKey};
    use std::sync::Arc;

    let driver = MemoryDriver::new("memory").with_sources(vec![SourceKind::Image]);
    let mut rig = Rig::with_driver(driver);
    rig.have_connector();
    rig.start().await;
    let policy = Arc::new(
        orch_connector_core::policy::PolicyStore::host(
            orch_config::ConnectorConfig::default(),
            None,
        )
        .unwrap(),
    );
    let mut drivers = MachineDrivers::managed();
    drivers.images.capabilities = vec!["kvm".into()];
    policy
        .set_here(PolicyKey::Drivers, &json!(&drivers))
        .unwrap();
    rig.role().runtimes().set_policy(Arc::clone(&policy));
    let mut spec = rig.spec(RuntimeKind::Environment, "replay-ceiling");
    spec.capabilities = serde_json::from_value(json!({
        "allowed":["kvm"], "base":["kvm"], "granted":[], "patches":{"kvm":{}}
    }))
    .unwrap();
    rig.made(&spec).await;
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(matches!(
        rig.stop_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    drivers.images.capabilities.clear();
    policy
        .set_here(PolicyKey::Drivers, &json!(&drivers))
        .unwrap();
    rig.driver.clear_calls();
    let error = match rig.start_runtime(spec.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("a capability outside the current policy was replayed: {other:?}"),
    };
    assert_eq!(error.code, "unsupported");
    assert!(error.detail.contains("kvm"));
    assert!(rig.driver.targets_of(Operation::Start).is_empty());
    rig.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn template_bind_mounts_obey_the_machines_directory_roots() {
    let mut rig = Rig::with_driver(MemoryDriver::new("docker").replacing());
    rig.settings.drivers = vec!["docker".into()];
    rig.have_connector();
    rig.start().await;

    // The fixture offers the system temporary directory, while the package is outside it.
    let outside = std::env::current_dir().unwrap();
    let offered = tempfile::tempdir().unwrap();
    let escaped = offered.path().join("escape");
    std::os::unix::fs::symlink(&outside, &escaped).unwrap();
    for (label, source) in [("outside", &outside), ("escape", &escaped)] {
        let mut spec = rig.spec(RuntimeKind::Environment, label);
        spec.driver = "docker".into();
        spec.spec["mounts"] = json!([{
            "type":"bind", "source":source, "target":"/extra"
        }]);
        let error = match rig.create(&spec).await {
            Progress::Failed(error) => error,
            other => panic!("a template mount outside the machine's roots was accepted: {other:?}"),
        };
        assert_eq!(error.code, "policy.root_not_allowed");
        assert!(rig.driver.unit(&spec.name).is_none());
    }

    let mut spec = rig.spec(RuntimeKind::Environment, "allowed");
    spec.driver = "docker".into();
    let directory = offered.path().join("data");
    std::fs::create_dir(&directory).unwrap();
    let link = offered.path().join("within");
    std::os::unix::fs::symlink(&directory, &link).unwrap();
    spec.spec["mounts"] = json!([{
        "type":"bind", "source":link, "target":"/extra"
    }]);
    rig.made(&spec).await;
    let actual = rig.driver.unit(&spec.name).unwrap();
    assert_eq!(
        actual.spec.spec["mounts"][0]["source"],
        json!(std::fs::canonicalize(&directory).unwrap())
    );
    std::fs::rename(&directory, offered.path().join("original")).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let error = match rig.start_runtime(spec.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("a replaced template directory was accepted: {other:?}"),
    };
    assert_eq!(error.code, "policy.root_not_allowed");
    assert!(rig.driver.targets_of(Operation::Start).is_empty());
    rig.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_capability_patch_cannot_add_a_template_bind_outside_the_machines_roots() {
    let mut rig = Rig::with_driver(MemoryDriver::new("docker").replacing());
    rig.settings.drivers = vec!["docker".into()];
    rig.have_connector();
    rig.start().await;
    let mut spec = rig.spec(RuntimeKind::Environment, "patch");
    spec.driver = "docker".into();
    spec.capabilities.base = vec!["probe".into()];
    spec.capabilities.patches.insert(
        "probe".into(),
        json!({"mounts":[{
            "type":"bind", "source":std::env::current_dir().unwrap(), "target":"/extra"
        }]}),
    );
    let error = match rig.create(&spec).await {
        Progress::Failed(error) => error,
        other => panic!("a capability patch escaped the machine's roots: {other:?}"),
    };
    assert_eq!(error.code, "unsupported");
    assert!(error.detail.contains("capability patch"));
    assert!(rig.driver.unit(&spec.name).is_none());
    rig.stop().await;
}

#[tokio::test]
async fn an_unavailable_driver_reports_its_schema_without_a_schedulable_descriptor() {
    use orch_node_core::drivers::{DriverHost, Factory, Registrations};
    use std::sync::Arc;

    let mut rig = Rig::new();
    rig.settings.drivers = vec!["cluster".into()];
    let schema = json!({"type":"object", "required":["namespace"]});
    let factory: Factory = Arc::new(|_| {
        Box::pin(async { Err(DriverError::Unavailable("the cluster is offline".into())) })
    });
    let host = DriverHost::load(
        &rig.settings,
        Registrations::new().register_with_schema("cluster", factory, schema.clone()),
        Arc::clone(&rig.desk),
    )
    .await;
    rig.start_with(host).await;
    rig.wait_told("the registered schema", |events| {
        events.iter().any(|event| {
            event.kind == "node.facts"
                && event.body["driver_configuration_schemas"]["cluster"] == schema
        })
    })
    .await;
    let report = rig
        .applied()
        .into_iter()
        .find(|event| event.kind == "node.facts")
        .unwrap()
        .body;
    assert_eq!(report["drivers"], json!({}));
    assert_eq!(report["driver_configuration_schemas"]["cluster"], schema);
    let mut old = report;
    old.as_object_mut()
        .unwrap()
        .remove("driver_configuration_schemas");
    let decoded: orch_link_contract::events::NodeFacts =
        serde_json::from_value(old.clone()).unwrap();
    assert!(decoded.driver_configuration_schemas.is_empty());
    assert_eq!(serde_json::to_value(decoded).unwrap(), old);
    rig.stop().await;
}

#[tokio::test]
async fn a_failed_restriction_keeps_an_open_unit_stopped_across_restart_until_an_explicit_open_plan()
 {
    use orch_api_contract::environment::EgressMode;
    use orch_link_contract::commands::NodeNetworkApply;
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "kept-stopped");
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(true));
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    let restriction = NodeNetworkApply {
        runtime_id: spec.runtime_id,
        required_mode: EgressMode::Restricted,
        allowed_hosts: vec!["pypi.org".to_owned()],
        always: Vec::new(),
        always_revision: String::new(),
        revision: "restrict-1".to_owned(),
    };
    let reply = rig
        .succeed(&SerialKey::Runtime(spec.runtime_id), &restriction)
        .await;
    assert_eq!(reply["required_mode"], "restricted");
    assert_eq!(reply["state"], "unavailable");
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    rig.stop().await;
    rig.start().await;
    let error = match rig.start_runtime(spec.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("the guarded old unit restarted: {other:?}"),
    };
    assert_eq!(error.code, "network.restricted_unavailable");
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    assert_eq!(
        rig.driver.volumes().len(),
        3,
        "the network guard removes no files"
    );
    let open = NodeNetworkApply {
        required_mode: EgressMode::Open,
        revision: "explicit-open".to_owned(),
        ..restriction
    };
    rig.succeed(&SerialKey::Runtime(spec.runtime_id), &open)
        .await;
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_is_made_started_stopped_and_removed_and_the_system_is_told_each_state() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "one");

    let unit = rig.made(&spec).await;
    assert_eq!(unit.name, spec.name);
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);

    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);

    assert!(matches!(
        rig.stop_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    assert_eq!(
        rig.driver.unit(&spec.name).unwrap().last_grace,
        Some(rig.settings.stop_grace),
        "a unit is given the node's grace to stop by itself"
    );

    assert!(matches!(
        rig.remove_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Absent);
    assert_eq!(
        rig.driver.volumes().len(),
        3,
        "a unit goes and its volumes stay"
    );

    rig.wait_state(spec.runtime_id, "absent").await;
    let told: Vec<String> = rig
        .states_of(spec.runtime_id)
        .into_iter()
        .map(|(state, _)| state)
        .collect();
    assert_eq!(told, ["stopped", "running", "stopped", "absent"]);
    rig.stop().await;
}

#[tokio::test]
async fn the_image_is_made_available_and_the_spec_checked_before_the_unit_is_made() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "ordered");
    rig.made(&spec).await;
    let order: Vec<Operation> = rig
        .driver
        .calls()
        .into_iter()
        .map(|call| call.operation)
        .filter(|operation| {
            matches!(
                operation,
                Operation::Validate | Operation::EnsureImage | Operation::Create
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            Operation::Validate,
            Operation::EnsureImage,
            Operation::Create
        ]
    );
    rig.stop().await;
}

#[tokio::test]
async fn asking_again_for_a_unit_that_is_made_answers_the_unit_that_is_there() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "twice");
    let first = rig.made(&spec).await;
    let second = rig.made(&spec).await;
    assert_eq!(first, second);
    assert_eq!(rig.driver.units().len(), 1, "created by name, once");
    rig.stop().await;
}

#[tokio::test]
async fn a_provider_runtime_is_made_the_same_way_with_its_state_volume_alone() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Provider, "prov");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    assert_eq!(rig.driver.volumes().len(), 1);
    rig.stop().await;
}

#[tokio::test]
async fn a_request_that_the_connector_inside_the_unit_could_not_use_is_refused_before_the_driver_is_asked()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;

    // A token of another authority than the certificate given.
    let strange = Rig::new();
    let mut spec = rig.spec(RuntimeKind::Environment, "strange");
    spec.enroll_token = strange.token().into();
    let error = match rig.create(&spec).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "invalid");
    assert!(
        error.detail.contains("another authority"),
        "{}",
        error.detail
    );

    // A plaintext token is refused by the link's typed body before dispatch.
    let spec = rig.spec(RuntimeKind::Environment, "plain");
    let mut body = serde_json::to_value(&spec).unwrap();
    body["enroll_token"] = json!(spec.enroll_token.expose());
    let error = serde_json::from_value::<orch_link_contract::commands::NodeRuntimeCreate>(body)
        .expect_err("the wire contract must refuse a plaintext enrollment token");
    assert!(
        error.to_string().contains("$sealed"),
        "the token must use the link's secret representation"
    );

    // A driver this host does not have.
    let mut elsewhere = rig.spec(RuntimeKind::Environment, "elsewhere");
    elsewhere.driver = "host".into();
    let error = match rig.create(&elsewhere).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "unsupported");

    assert!(
        rig.driver.targets_of(Operation::Create).is_empty(),
        "the driver was never asked to make any of them"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_host_without_the_connector_in_its_cache_makes_no_unit() {
    let mut rig = Rig::new();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "nothing-to-run");
    let error = match rig.create(&spec).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "not_found");
    assert!(
        error.detail.contains("node.tool.ensure"),
        "{}",
        error.detail
    );
    rig.stop().await;
}

#[tokio::test]
async fn an_enrollment_token_is_single_use_and_a_second_unit_is_not_given_the_first_ones() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let first = rig.spec(RuntimeKind::Environment, "first");
    rig.made(&first).await;
    let mut second = rig.spec(RuntimeKind::Environment, "second");
    second.enroll_token = first.enroll_token.clone();
    let error = match rig.create(&second).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert!(error.detail.contains("single-use"), "{}", error.detail);
    assert_eq!(rig.driver.units().len(), 1);
    // The same runtime asked for again, with the same token, is not another unit.
    rig.made(&first).await;
    rig.stop().await;
}

#[tokio::test]
async fn a_successor_is_made_on_the_same_volumes_while_the_old_unit_is_kept_and_two_units_never_run_on_one_volume()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let old = rig.spec(RuntimeKind::Environment, "old");
    let mut new = rig.spec(RuntimeKind::Environment, "new");
    new.volumes = old.volumes.clone();
    rig.made(&old).await;
    rig.start_runtime(old.runtime_id).await;

    // The successor is made while the old unit runs; it may not start until the old one has stopped.
    rig.made(&new).await;
    let error = match rig.start_runtime(new.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert!(error.detail.contains(&old.name), "{}", error.detail);
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Stopped);

    rig.stop_runtime(old.runtime_id).await;
    assert!(matches!(
        rig.start_runtime(new.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Running);
    assert_eq!(
        rig.driver.state_of(&old.name),
        UnitStateKind::Stopped,
        "the old unit is kept until it is told to go"
    );

    // And back: the old unit is started again as it was only when the successor is out of the way.
    let error = match rig.start_runtime(old.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "conflict");
    rig.remove_runtime(new.runtime_id).await;
    assert!(matches!(
        rig.start_runtime(old.runtime_id).await,
        Progress::Succeeded(_)
    ));

    // Removing the old unit once the successor has taken over leaves the volumes where they are.
    rig.stop_runtime(old.runtime_id).await;
    rig.remove_runtime(old.runtime_id).await;
    assert_eq!(rig.driver.volumes().len(), 3);
    rig.stop().await;
}

#[tokio::test]
async fn starts_on_independent_volumes_can_enter_the_driver_together() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let first = rig.spec(RuntimeKind::Environment, "independent-a");
    let second = rig.spec(RuntimeKind::Environment, "independent-b");
    rig.made(&first).await;
    rig.made(&second).await;
    rig.driver.clear_calls();
    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let one = rig.controller.issue(
        &SerialKey::Runtime(first.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: first.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &first.name).await;
    let two = rig.controller.issue(
        &SerialKey::Runtime(second.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: second.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &second.name).await;
    assert_eq!(
        rig.driver.targets_of(Operation::Start),
        [first.name.clone(), second.name.clone()]
    );
    rig.driver.release(Operation::Start);
    for command in [one, two] {
        assert!(matches!(
            tokio::time::timeout(PATIENCE, rig.controller.wait_command(command))
                .await
                .unwrap(),
            Progress::Succeeded(_)
        ));
    }
    rig.stop().await;
}

#[tokio::test]
async fn concurrent_starts_on_the_same_volumes_do_not_both_enter_the_driver() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let first = rig.spec(RuntimeKind::Environment, "first-shared");
    let mut second = rig.spec(RuntimeKind::Environment, "second-shared");
    second.volumes = first.volumes.clone();
    let independent = rig.spec(RuntimeKind::Environment, "independent");
    rig.made(&first).await;
    rig.made(&second).await;
    rig.made(&independent).await;
    rig.start_runtime(independent.runtime_id).await;
    rig.driver.clear_calls();
    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let one = rig.controller.issue(
        &SerialKey::Runtime(first.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: first.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &first.name).await;
    let two = rig.controller.issue(
        &SerialKey::Runtime(second.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: second.runtime_id,
        },
    );
    let other = rig.controller.issue(
        &SerialKey::Runtime(independent.runtime_id),
        deadline,
        &NodeRuntimeStop {
            runtime_id: independent.runtime_id,
            reason: None,
        },
    );
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(other))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rig.driver.targets_of(Operation::Start).as_slice(),
        std::slice::from_ref(&first.name)
    );
    rig.driver.release(Operation::Start);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(one))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    let error = match tokio::time::timeout(PATIENCE, rig.controller.wait_command(two))
        .await
        .unwrap()
    {
        Progress::Failed(error) => error,
        other => panic!("the second unit started on the same volumes: {other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert_eq!(rig.driver.state_of(&first.name), UnitStateKind::Running);
    assert_eq!(rig.driver.state_of(&second.name), UnitStateKind::Stopped);
    rig.stop().await;
}

#[tokio::test]
async fn a_successor_waits_when_the_predecessors_driver_is_unavailable() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let old = rig.spec(RuntimeKind::Environment, "old-driver");
    let mut new = rig.spec(RuntimeKind::Environment, "new-driver");
    new.volumes = old.volumes.clone();
    rig.made(&old).await;
    rig.made(&new).await;
    rig.role()
        .ledger()
        .update(old.runtime_id, |record| {
            record.driver = "unavailable-driver".into()
        })
        .unwrap();
    let error = match rig.start_runtime(new.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("the successor started without checking its predecessor: {other:?}"),
    };
    assert_eq!(error.code, "unavailable");
    assert!(error.retryable);
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Stopped);
    let name = old.volumes[&VolumeRole::Workspace].name.clone();
    let removal = NodeVolumeRemove {
        driver: "memory".into(),
        name: name.clone(),
        storage: None,
        labels: Default::default(),
    };
    let error = rig
        .refused(&SerialKey::Volume(name.clone()), &removal)
        .await;
    assert_eq!(error.code, "unavailable");
    assert!(error.retryable);
    assert!(rig.driver.volume(&name).is_some());
    let invalid = NodeVolumeRemove {
        driver: String::new(),
        name: name.clone(),
        storage: None,
        labels: Default::default(),
    };
    let error = rig.refused(&SerialKey::Volume(name), &invalid).await;
    assert_eq!(
        error.code, "unsupported",
        "an empty driver cannot collide with the name fence"
    );
    rig.stop().await;
}

#[tokio::test]
async fn removal_waits_for_an_in_flight_start_and_cannot_remove_the_running_volume() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "starting-volume");
    rig.made(&spec).await;
    let name = spec.volumes[&VolumeRole::Workspace].name.clone();
    rig.driver.clear_calls();
    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let start = rig.controller.issue(
        &SerialKey::Runtime(spec.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: spec.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &spec.name).await;
    let removal = rig.controller.issue(
        &SerialKey::Volume(name.clone()),
        deadline,
        &NodeVolumeRemove {
            driver: spec.driver.clone(),
            name: name.clone(),
            storage: None,
            labels: Default::default(),
        },
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rig.driver.targets_of(Operation::RemoveVolume).is_empty());
    rig.driver.release(Operation::Start);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(start))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    let error = match tokio::time::timeout(PATIENCE, rig.controller.wait_command(removal))
        .await
        .unwrap()
    {
        Progress::Failed(error) => error,
        other => panic!("the running volume was removed: {other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert!(rig.driver.volume(&name).is_some());
    rig.stop().await;
}

#[tokio::test]
async fn a_capability_restart_fences_a_successor_on_the_same_volumes() {
    let driver = MemoryDriver::new("memory").replacing();
    let mut descriptor = driver.describe();
    descriptor
        .capabilities
        .insert("debugger".into(), json!({"type":"object"}));
    descriptor.live_changes.capabilities.push("debugger".into());
    let mut rig = Rig::with_driver(driver.with_descriptor(descriptor));
    rig.have_connector();
    rig.start().await;
    let mut old = rig.spec(RuntimeKind::Environment, "restarting");
    old.capabilities = serde_json::from_value(
        json!({"allowed":["debugger"],"base":[],"granted":[],"patches":{"debugger":{}}}),
    )
    .unwrap();
    let mut new = rig.spec(RuntimeKind::Environment, "waiting-successor");
    new.volumes = old.volumes.clone();
    rig.made(&old).await;
    rig.made(&new).await;
    rig.start_runtime(old.runtime_id).await;
    rig.driver.clear_calls();
    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let change = rig.controller.issue(&SerialKey::Runtime(old.runtime_id), deadline,
        &NodeCapabilitiesApply {runtime_id:old.runtime_id, capabilities:json!({"allowed":["debugger"],"base":[],"granted":["debugger"],"patches":{"debugger":{}}})});
    rig.wait_call(Operation::Start, &old.name).await;
    let successor = rig.controller.issue(
        &SerialKey::Runtime(new.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: new.runtime_id,
        },
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.driver.targets_of(Operation::Start), [old.name.clone()]);
    rig.driver.release(Operation::Start);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(change))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    let error = match tokio::time::timeout(PATIENCE, rig.controller.wait_command(successor))
        .await
        .unwrap()
    {
        Progress::Failed(error) => error,
        other => panic!("the successor started during a capability restart: {other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert_eq!(rig.driver.state_of(&old.name), UnitStateKind::Running);
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Stopped);
    rig.stop().await;
}

#[tokio::test]
async fn capability_reconciliation_does_not_replay_over_a_running_successor() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let old = rig.spec(RuntimeKind::Environment, "replay-old");
    let mut new = rig.spec(RuntimeKind::Environment, "replay-successor");
    new.volumes = old.volumes.clone();
    rig.made(&old).await;
    let successor = rig.made(&new).await;
    assert!(matches!(
        rig.start_runtime(old.runtime_id).await,
        Progress::Succeeded(_)
    ));
    // Model a native start outside node commands. Reconciliation must not compound
    // that existing conflict by replaying a second start on the old unit.
    rig.driver.start(&successor).await.unwrap();
    let ledger = rig.role();
    let mut capabilities = ledger.ledger().capabilities(old.runtime_id).unwrap();
    capabilities.applied_started_at = Some(rig.clock.time());
    ledger
        .ledger()
        .set_capabilities(old.runtime_id, capabilities)
        .unwrap();
    rig.driver.clear_calls();
    ledger.runtimes().reconcile_live_capabilities().await;
    assert!(rig.driver.targets_of(Operation::Start).is_empty());
    assert_eq!(rig.driver.state_of(&new.name), UnitStateKind::Running);
    rig.stop().await;
}

#[tokio::test]
async fn commands_for_one_runtime_run_one_at_a_time_and_for_two_runtimes_side_by_side() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let (a, b) = (
        rig.spec(RuntimeKind::Environment, "a"),
        rig.spec(RuntimeKind::Environment, "b"),
    );
    rig.made(&a).await;
    rig.made(&b).await;
    rig.start_runtime(b.runtime_id).await;

    rig.driver.hold(Operation::Start);
    let deadline = rig.deadline();
    let first = rig.controller.issue(
        &SerialKey::Runtime(a.runtime_id),
        deadline,
        &NodeRuntimeStart {
            runtime_id: a.runtime_id,
        },
    );
    // The start of `a` is inside the driver, held there.
    rig.wait_call(Operation::Start, &a.name).await;
    let second = rig.controller.issue(
        &SerialKey::Runtime(a.runtime_id),
        deadline,
        &NodeRuntimeStop {
            runtime_id: a.runtime_id,
            reason: None,
        },
    );
    let other = rig.controller.issue(
        &SerialKey::Runtime(b.runtime_id),
        deadline,
        &NodeRuntimeStop {
            runtime_id: b.runtime_id,
            reason: None,
        },
    );
    // The other runtime's stop is done while `a` is held; the stop of `a` waits behind its start.
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(other))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rig.driver.targets_of(Operation::Stop),
        std::slice::from_ref(&b.name)
    );
    assert!(!rig.controller.progress(first).unwrap().is_final());

    rig.driver.release(Operation::Start);
    for id in [first, second] {
        assert!(matches!(
            tokio::time::timeout(PATIENCE, rig.controller.wait_command(id))
                .await
                .unwrap(),
            Progress::Succeeded(_)
        ));
    }
    assert_eq!(
        rig.driver.targets_of(Operation::Stop),
        [b.name.clone(), a.name.clone()]
    );
    assert_eq!(
        rig.driver.max_in_flight(&a.name),
        1,
        "never two calls for one unit at once"
    );
    assert_eq!(rig.driver.state_of(&a.name), UnitStateKind::Stopped);
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_the_host_has_no_room_for_fails_as_no_capacity_and_says_what_the_units_there_declare()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let (resident, tenant) = (
        rig.spec(RuntimeKind::Environment, "resident"),
        rig.spec(RuntimeKind::Environment, "tenant"),
    );
    rig.made(&resident).await;
    rig.start_runtime(resident.runtime_id).await;

    rig.driver.fail_next(
        Operation::Create,
        DriverError::NoCapacity("cannot allocate memory".into()),
    );
    let error = match rig.create(&tenant).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "no_capacity");
    assert!(
        error.detail.contains("cannot allocate memory"),
        "{}",
        error.detail
    );
    assert!(
        error.detail.contains("declare 512 MiB of memory"),
        "{}",
        error.detail
    );
    assert!(
        rig.driver.unit(&tenant.name).is_none(),
        "nothing was left half made"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_runtime_the_node_never_heard_of_cannot_be_started_or_stopped_and_is_already_removed() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let nobody = new_id();
    for progress in [
        rig.start_runtime(nobody).await,
        rig.stop_runtime(nobody).await,
    ] {
        let Progress::Failed(error) = progress else {
            panic!("{progress:?}");
        };
        assert_eq!(error.code, "not_found");
    }
    assert!(matches!(
        rig.remove_runtime(nobody).await,
        Progress::Succeeded(_)
    ));
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_the_node_has_no_record_of_is_found_by_the_label_of_its_system_and_removed_without_its_volumes()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    // A unit from before this node had a ledger, or from a ledger that was lost.
    let stray = rig.spec(RuntimeKind::Environment, "stray");
    for volume in stray.volumes.values() {
        orch_driver_contract::driver::EnvironmentDriver::ensure_volume(
            rig.driver.as_ref(),
            &orch_driver_contract::volumes::VolumeRequest {
                name: volume.name.clone(),
                storage: None,
                labels: std::collections::BTreeMap::new(),
            },
        )
        .await
        .unwrap();
    }
    rig.driver.plant(stray.clone());
    assert!(matches!(
        rig.remove_runtime(stray.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(rig.driver.unit(&stray.name).is_none());
    assert_eq!(
        rig.driver.volumes().len(),
        3,
        "its volumes were not touched"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_node_that_is_started_again_still_knows_the_units_it_made() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "durable");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    rig.stop().await;

    rig.start().await;
    assert!(matches!(
        rig.stop_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    let recorded = Ledger::open(&rig.settings.data_dir).unwrap();
    let record = recorded.get(spec.runtime_id).unwrap();
    assert_eq!(record.unit.name, spec.name);
    assert!(record.token.is_some() && record.started_at.is_some());
    assert_eq!(
        recorded.systems(),
        [SYSTEM],
        "the system the units belong to is remembered"
    );
    rig.stop().await;
}

// ---------------------------------------------------------------------------------------------- a connector that never enrolls

#[tokio::test]
async fn a_unit_that_ends_before_its_connector_enrolled_is_reported_failed_and_removed_and_its_volumes_stay()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "dies");
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(false));
    rig.start_runtime(spec.runtime_id).await;
    rig.driver.crash(
        &spec.name,
        1,
        "enrolling this runtime: the gateway cannot be reached",
    );

    rig.wait_state(spec.runtime_id, "failed").await;
    let (state, detail) = rig.states_of(spec.runtime_id).pop().unwrap();
    assert_eq!(state, "failed");
    let detail = detail.unwrap();
    assert!(detail.contains("before its connector enrolled"), "{detail}");
    assert!(detail.contains("the gateway cannot be reached"), "{detail}");
    assert!(detail.contains("exit status 1"), "{detail}");
    assert!(
        rig.driver.unit(&spec.name).is_none(),
        "the unit is cleaned up"
    );
    assert_eq!(rig.driver.volumes().len(), 3, "and its volumes are not");
    assert!(
        Ledger::open(&rig.settings.data_dir)
            .unwrap()
            .get(spec.runtime_id)
            .is_none()
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_whose_connector_does_not_enroll_in_time_is_stopped_removed_and_reported_failed() {
    let mut rig = Rig::new();
    rig.settings.enroll_timeout = Duration::from_secs(1);
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "silent");
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(false));
    rig.start_runtime(spec.runtime_id).await;

    rig.wait_state(spec.runtime_id, "failed").await;
    let detail = rig.states_of(spec.runtime_id).pop().unwrap().1.unwrap();
    assert!(
        detail.contains("did not enroll within 1 seconds"),
        "{detail}"
    );
    assert!(rig.driver.unit(&spec.name).is_none());
    let calls = rig.driver.targets_of(Operation::Stop);
    assert_eq!(
        calls,
        std::slice::from_ref(&spec.name),
        "it was asked to stop before it was removed"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_whose_connector_enrolled_is_not_watched_again_and_one_that_cannot_be_looked_into_is_not_judged()
 {
    let mut rig = Rig::new();
    rig.settings.enroll_timeout = Duration::from_secs(1);
    rig.have_connector();
    rig.start().await;
    let (good, unknown) = (
        rig.spec(RuntimeKind::Environment, "good"),
        rig.spec(RuntimeKind::Environment, "unknown"),
    );
    for spec in [&good, &unknown] {
        rig.made(spec).await;
    }
    rig.driver.set_enrolled(&good.name, Some(true));
    // A driver that cannot tell: nothing is known, and nothing is done.
    rig.driver.set_enrolled(&unknown.name, None);
    rig.start_runtime(good.runtime_id).await;
    rig.start_runtime(unknown.runtime_id).await;
    tokio::time::sleep(Duration::from_secs(4)).await;

    assert_eq!(rig.driver.state_of(&good.name), UnitStateKind::Running);
    assert_eq!(rig.driver.state_of(&unknown.name), UnitStateKind::Running);
    let recorded = Ledger::open(&rig.settings.data_dir).unwrap();
    assert!(recorded.get(good.runtime_id).unwrap().enrolled);
    assert!(!recorded.get(unknown.runtime_id).unwrap().enrolled);
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_whose_driver_is_not_loaded_just_now_is_not_forgotten_when_it_is_asked_to_go() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "orphaned");
    rig.made(&spec).await;
    rig.stop().await;

    // The node comes back with the daemon down: it has no driver, and a unit it made is on record.
    let nothing = orch_node_core::drivers::DriverHost::of(
        &rig.settings,
        std::sync::Arc::clone(&rig.desk),
        Vec::new(),
    );
    rig.start_with(nothing).await;
    let error = match rig.remove_runtime(spec.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "unavailable");
    assert!(
        error.retryable,
        "asking again when the driver is back may work"
    );
    assert!(error.detail.contains("memory"), "{}", error.detail);
    assert!(
        Ledger::open(&rig.settings.data_dir)
            .unwrap()
            .get(spec.runtime_id)
            .is_some()
    );
    assert!(
        rig.driver.unit(&spec.name).is_some(),
        "and nothing was done to the unit"
    );
    rig.stop().await;

    rig.start().await;
    assert!(matches!(
        rig.remove_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(rig.driver.unit(&spec.name).is_none());
    rig.stop().await;
}

#[tokio::test]
async fn two_requests_with_one_token_cannot_both_get_past_the_check_while_the_first_is_still_being_made()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let (first, mut second) = (
        rig.spec(RuntimeKind::Environment, "token-a"),
        rig.spec(RuntimeKind::Environment, "token-b"),
    );
    second.enroll_token = first.enroll_token.clone();

    // The first is held at its image, long before the unit is made and recorded.
    rig.driver.hold(Operation::EnsureImage);
    let id = new_id();
    rig.controller.issue_with_id(
        id,
        &SerialKey::Runtime(first.runtime_id),
        rig.deadline(),
        &rig.create_body(&first, id),
    );
    rig.until("the image of the first", || {
        !rig.driver.targets_of(Operation::EnsureImage).is_empty()
    })
    .await;
    let refused = match rig.create(&second).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(refused.code, "conflict");
    assert!(refused.detail.contains("single-use"), "{}", refused.detail);

    rig.driver.release(Operation::EnsureImage);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(id))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.units().len(), 1);
    rig.stop().await;
}

#[tokio::test]
async fn what_one_environment_wrote_in_a_private_view_is_a_volume_of_its_unit_that_no_second_unit_runs_on()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let layer = json!({ "name": "orch-over-shared", "storage": null });
    let source = tempfile::tempdir().unwrap();
    let mount = |name: &str| {
        json!({
            "name": name, "kind": "path", "source": source.path(), "target": "/data/src",
            "mode": "private", "layer": layer
        })
    };
    let (mut old, mut new) = (
        rig.spec(RuntimeKind::Environment, "view-old"),
        rig.spec(RuntimeKind::Environment, "view-new"),
    );
    old.mounts = vec![serde_json::from_value(mount("src")).unwrap()];
    new.mounts = vec![serde_json::from_value(mount("src")).unwrap()];
    // The two have volumes of their own and share the layer only.
    rig.made(&old).await;
    rig.made(&new).await;
    let record = Ledger::open(&rig.settings.data_dir)
        .unwrap()
        .get(old.runtime_id)
        .unwrap();
    assert!(
        record.volumes.contains(&"orch-over-shared".to_owned()),
        "{:?}",
        record.volumes
    );

    rig.start_runtime(old.runtime_id).await;
    let error = match rig.start_runtime(new.runtime_id).await {
        Progress::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    assert_eq!(error.code, "conflict");
    rig.stop().await;
}

#[tokio::test]
async fn a_new_unit_uses_its_nodes_verified_address_or_that_nodes_explicit_unit_override() {
    for override_address in ["", "wss://units.test/link"] {
        let mut rig = Rig::new();
        rig.settings.managed_endpoint = override_address.into();
        rig.have_connector();
        rig.start().await;
        for kind in [RuntimeKind::Environment, RuntimeKind::Provider] {
            let mut spec = rig.spec(kind, "node-address");
            spec.gateway_url = "wss://unreachable-server-wide-address.test:9443".into();
            rig.made(&spec).await;
            let actual = rig.driver.unit(&spec.name).unwrap().spec.gateway_url;
            assert_eq!(
                actual,
                if override_address.is_empty() {
                    orch_testkit::doubles::link::HUB_URL
                } else {
                    override_address
                }
            );
        }
        rig.stop().await;
    }
}
