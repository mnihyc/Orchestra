//! A node's inventory protects dependencies and resolves opaque removal identities afresh.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, SYSTEM};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::kept::KeptImage;
use orch_driver_contract::resources::{ResourceInventory, ResourceKind, ResourceRef};
use orch_driver_contract::runtime::{RuntimeKind, RuntimeSpec};
use orch_link::commands::Progress;
use orch_link_contract::commands::{NodeResourcesList, NodeResourcesRemove, SerialKey};
use orch_types::new_id;

async fn inventory(rig: &Rig) -> ResourceInventory {
    match rig.send(&SerialKey::Machine, &NodeResourcesList {}).await {
        Progress::Succeeded(body) => serde_json::from_value(body).unwrap(),
        other => panic!("inventory failed: {other:?}"),
    }
}

async fn own_volumes(rig: &Rig, spec: &RuntimeSpec) {
    for volume in spec.volumes.values() {
        rig.succeed(
            &SerialKey::Volume(volume.name.clone()),
            &orch_link_contract::commands::NodeVolumeEnsure {
                driver: spec.driver.clone(),
                name: volume.name.clone(),
                storage: volume.storage.clone(),
                labels: spec.labels.clone(),
            },
        )
        .await;
    }
}

fn kept(system: &str) -> KeptImage {
    KeptImage {
        system: system.into(),
        environment: new_id(),
        generation: 1,
        capture: new_id(),
        reference: new_id().to_string(),
        bytes: 123,
        total_bytes: 456,
        layers: 1,
    }
}

#[tokio::test]
async fn only_this_installations_objects_are_named_and_running_work_and_stopped_dependencies_cannot_be_removed()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "inventory");
    own_volumes(&rig, &spec).await;
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(true));
    let started = rig.start_runtime(spec.runtime_id).await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    let first = inventory(&rig).await;
    assert!(first.unavailable.is_empty());
    let unit = first
        .resources
        .iter()
        .find(|entry| entry.resource.kind == ResourceKind::Unit)
        .unwrap();
    assert!(unit.busy);
    assert_ne!(
        unit.resource.id, unit.name,
        "a node's id is opaque even when a driver uses names"
    );
    let volume = first
        .resources
        .iter()
        .find(|entry| entry.resource.kind == ResourceKind::Volume)
        .unwrap();
    assert!(unit.depends_on.contains(&volume.resource));
    assert_eq!(volume.size_bytes, None, "unmeasured bytes stay unknown");
    for resource in [&unit.resource, &volume.resource] {
        assert!(matches!(
            rig.send(
                &SerialKey::Machine,
                &NodeResourcesRemove::new(resource.clone())
            )
            .await,
            Progress::Failed(_)
        ));
    }
    rig.driver
        .stop(&spec_to_unit(&rig, &unit.name), std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(
        matches!(
            rig.send(
                &SerialKey::Machine,
                &NodeResourcesRemove::new(volume.resource.clone())
            )
            .await,
            Progress::Failed(_)
        ),
        "a stopped unit still depends on its storage"
    );
    rig.stop().await;
}

fn spec_to_unit(rig: &Rig, name: &str) -> orch_driver_contract::units::UnitRef {
    rig.driver
        .units()
        .into_iter()
        .find(|unit| unit.unit.name == name)
        .unwrap()
        .unit
}

#[tokio::test]
async fn unknown_and_foreign_ids_are_refused_and_a_removed_objects_old_id_cannot_remove_anything_else()
 {
    let mut rig = Rig::new();
    let own = kept(SYSTEM);
    let foreign = kept("some-other-installation");
    rig.driver.remember_kept(own.clone());
    rig.driver.remember_kept(foreign.clone());
    rig.start().await;
    let listed = inventory(&rig).await;
    let item = listed
        .resources
        .iter()
        .find(|entry| entry.name == own.reference)
        .unwrap();
    assert!(
        !listed
            .resources
            .iter()
            .any(|entry| entry.name == foreign.reference)
    );
    assert!(matches!(
        rig.send(
            &SerialKey::Machine,
            &NodeResourcesRemove::new(ResourceRef {
                kind: ResourceKind::KeptLayer,
                id: foreign.reference.clone()
            })
        )
        .await,
        Progress::Failed(_)
    ));
    assert!(matches!(
        rig.send(
            &SerialKey::Machine,
            &NodeResourcesRemove::new(item.resource.clone())
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert!(matches!(
        rig.send(
            &SerialKey::Machine,
            &NodeResourcesRemove::new(item.resource.clone())
        )
        .await,
        Progress::Failed(_)
    ));
    assert_eq!(rig.driver.kept_names(), vec![foreign.reference]);
    rig.stop().await;
}

#[tokio::test]
async fn an_unused_tool_keeps_its_first_observation_date_until_explicitly_removed() {
    let mut rig = Rig::new();
    let version = rig.settings.tools_dir().join("sample/1.0");
    std::fs::create_dir_all(&version).unwrap();
    std::fs::write(version.join(".complete"), "a verified fixture").unwrap();
    std::fs::write(version.join("program"), b"tool").unwrap();
    std::fs::write(version.join(orch_node_core::tools::SIZE_MARKER), "4").unwrap();
    rig.start().await;
    let first = inventory(&rig).await;
    let tool = first
        .resources
        .iter()
        .find(|entry| entry.name == "sample 1.0")
        .unwrap();
    assert_eq!(tool.size_bytes, Some(4));
    assert_eq!(tool.last_used_at, Some(first.observed_at));
    rig.manual.advance(chrono::Duration::days(7));
    let next = inventory(&rig).await;
    assert_eq!(
        next.resources
            .iter()
            .find(|entry| entry.resource == tool.resource)
            .unwrap()
            .last_used_at,
        tool.last_used_at,
        "listing an unused tool is not use"
    );
    let mut conditional = NodeResourcesRemove::new(tool.resource.clone());
    conditional.expected_last_used_at = tool.last_used_at;
    rig.role
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .ledger()
        .resource_used(tool.resource.id.clone(), rig.clock.time())
        .unwrap();
    assert!(
        matches!(
            rig.send(&SerialKey::Machine, &conditional).await,
            Progress::Failed(_)
        ),
        "a use after the plan protects the cache"
    );
    assert!(version.exists());
    assert!(matches!(
        rig.send(
            &SerialKey::Machine,
            &NodeResourcesRemove::new(tool.resource.clone())
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert!(!version.exists());
    rig.stop().await;
}

#[tokio::test]
async fn a_local_purge_stops_owned_units_before_their_dependencies_and_keeps_another_installation()
{
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "local-purge");
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(true));
    let started = rig.start_runtime(spec.runtime_id).await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    let foreign = orch_driver_contract::volumes::VolumeRequest {
        name: "foreign-data".into(),
        storage: None,
        labels: [("orch.system".into(), "another-installation".into())].into(),
    };
    rig.driver.ensure_volume(&foreign).await.unwrap();
    rig.stop().await;
    let drivers = orch_node_core::drivers::DriverHost::of(
        &rig.settings,
        rig.desk.clone(),
        vec![orch_node_core::drivers::HostDriver::new(rig.driver.clone())],
    );
    let ledger = orch_node_core::ledger::Ledger::open(rig.data.path()).unwrap();
    let tools = orch_node_core::tools::ToolCache::inspect(&rig.settings).unwrap();
    let selected =
        orch_node_core::resources::snapshot(&drivers, &tools, &ledger, rig.clock.as_ref()).await;
    assert!(
        selected
            .resources
            .iter()
            .any(|item| item.resource.kind == ResourceKind::Unit && item.busy)
    );
    let result = orch_node_core::resources::purge(
        &drivers,
        &tools,
        &ledger,
        rig.clock.as_ref(),
        &selected.resources,
    )
    .await;
    assert!(
        result.iter().all(|(_, result)| result.is_ok()),
        "{result:?}"
    );
    assert_eq!(result.len(), selected.resources.len());
    assert_eq!(result[0].0.kind, ResourceKind::Unit);
    assert!(
        orch_node_core::resources::snapshot(&drivers, &tools, &ledger, rig.clock.as_ref())
            .await
            .resources
            .is_empty()
    );
    assert!(
        rig.driver
            .volumes()
            .iter()
            .any(|volume| volume.name == foreign.name)
    );
}

#[tokio::test]
async fn cancellation_between_items_reports_the_removed_object_and_keeps_every_unstarted_object() {
    let mut rig = Rig::new();
    rig.start().await;
    for number in 0..2 {
        rig.driver
            .ensure_volume(&orch_driver_contract::volumes::VolumeRequest {
                name: format!("unused-{number}"),
                storage: None,
                labels: [("orch.system".into(), SYSTEM.into())].into(),
            })
            .await
            .unwrap();
    }
    rig.stop().await;
    let drivers = orch_node_core::drivers::DriverHost::of(
        &rig.settings,
        rig.desk.clone(),
        vec![orch_node_core::drivers::HostDriver::new(rig.driver.clone())],
    );
    let ledger = orch_node_core::ledger::Ledger::open(rig.data.path()).unwrap();
    let tools = orch_node_core::tools::ToolCache::inspect(&rig.settings).unwrap();
    let selected =
        orch_node_core::resources::snapshot(&drivers, &tools, &ledger, rig.clock.as_ref()).await;
    assert_eq!(selected.resources.len(), 2);
    let cancel = tokio_util::sync::CancellationToken::new();
    let reporting = cancel.clone();
    let mut reported = Vec::new();
    let result = orch_node_core::resources::purge_controlled(
        &drivers,
        &tools,
        &ledger,
        rig.clock.as_ref(),
        &selected.resources,
        &cancel,
        |resource, result| {
            reported.push((resource.clone(), result.clone()));
            if result.is_ok() {
                reporting.cancel();
            }
        },
    )
    .await;
    assert_eq!(
        reported, result,
        "each result is reported once as soon as it is known"
    );
    assert_eq!(
        result.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    assert_eq!(
        result
            .iter()
            .filter(|(_, result)| result
                .as_ref()
                .is_err_and(|error| error.code == "cancelled"))
            .count(),
        1
    );
    assert_eq!(
        orch_node_core::resources::snapshot(&drivers, &tools, &ledger, rig.clock.as_ref())
            .await
            .resources
            .len(),
        1
    );
}

#[tokio::test]
async fn an_explicit_stop_choice_removes_the_unit_through_the_same_command_and_keeps_its_data() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "deliberate-stop");
    own_volumes(&rig, &spec).await;
    rig.made(&spec).await;
    rig.driver.set_enrolled(&spec.name, Some(true));
    let started = rig.start_runtime(spec.runtime_id).await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    let listed = inventory(&rig).await;
    let unit = listed
        .resources
        .iter()
        .find(|item| item.resource.kind == ResourceKind::Unit)
        .unwrap();
    let mut remove = NodeResourcesRemove::new(unit.resource.clone());
    remove.stop_first = true;
    assert!(matches!(
        rig.send(&SerialKey::Machine, &remove).await,
        Progress::Succeeded(_)
    ));
    let left = inventory(&rig).await;
    assert!(
        left.resources
            .iter()
            .all(|item| item.resource.kind != ResourceKind::Unit)
    );
    assert!(
        left.resources
            .iter()
            .any(|item| item.resource.kind == ResourceKind::Volume)
    );
    assert!(
        rig.role
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .ledger()
            .get(spec.runtime_id)
            .is_none(),
        "the exact removed native unit no longer leaves a local runtime record"
    );
    rig.stop().await;
}

#[tokio::test]
async fn helper_preparation_keeps_cache_removal_out_without_queueing_a_writer_ahead_of_work() {
    use orch_testkit::doubles::driver::Operation;
    let mut rig = Rig::new();
    let cached = rig.settings.tools_dir().join("sample/1.0");
    std::fs::create_dir_all(&cached).unwrap();
    std::fs::write(cached.join(".complete"), b"verified fixture").unwrap();
    std::fs::write(cached.join("keep"), b"tool").unwrap();
    rig.start().await;
    let listed = inventory(&rig).await;
    let tool = listed
        .resources
        .iter()
        .find(|item| item.resource.kind == ResourceKind::ToolVersion)
        .unwrap();
    rig.driver.hold(Operation::EnsureVolume);
    let work = rig.controller.issue(
        &SerialKey::Volume("preparing".into()),
        rig.deadline(),
        &orch_link_contract::commands::NodeVolumeEnsure {
            driver: "memory".into(),
            name: "preparing".into(),
            storage: None,
            labels: [("orch.system".into(), SYSTEM.into())].into(),
        },
    );
    rig.wait_call(Operation::EnsureVolume, "preparing").await;
    let removal = rig
        .send(
            &SerialKey::Machine,
            &NodeResourcesRemove::new(tool.resource.clone()),
        )
        .await;
    rig.driver.release(Operation::EnsureVolume);
    let prepared = tokio::time::timeout(common::PATIENCE, rig.controller.wait_command(work))
        .await
        .unwrap();
    assert!(
        matches!(removal, Progress::Failed(_)),
        "cleanup cannot run in the helper's preparation gap"
    );
    assert!(
        matches!(prepared, Progress::Succeeded(_)),
        "admitted work still completes"
    );
    assert!(cached.join("keep").is_file());
    assert!(matches!(
        rig.send(
            &SerialKey::Machine,
            &NodeResourcesRemove::new(tool.resource.clone())
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert!(!cached.exists());
    rig.stop().await;
}
