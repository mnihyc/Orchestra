//! The node treats a multi-machine pool as a host and recovers units without exposing local paths.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, SYSTEM};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::runtime::{RuntimeKind, VolumeRole};
use orch_driver_contract::units::UnitStateKind;
use orch_driver_contract::volumes::StorageInfo;
use orch_link::commands::Progress;
use orch_link_contract::commands::{
    NodeImageEnsure, NodeProbe, NodeProbeResult, NodeVolumeRemove, SerialKey,
};
use orch_node_core::drivers::{DriverHost, HostDriver};
use orch_testkit::doubles::cluster::{ClusterDriver, ClusterMachine};
use serde_json::json;
use std::sync::Arc;

fn cluster() -> Arc<ClusterDriver> {
    Arc::new(
        ClusterDriver::new(
            vec![
                ClusterMachine {
                    id: "member-a".into(),
                    cpus: 3,
                    memory_mb: 7000,
                },
                ClusterMachine {
                    id: "member-b".into(),
                    cpus: 5,
                    memory_mb: 11000,
                },
            ],
            vec![
                StorageInfo {
                    name: "fast".into(),
                    default: true,
                    capacity_gb: 700,
                    free_gb: 400,
                    private_views: false,
                },
                StorageInfo {
                    name: "bulk".into(),
                    default: false,
                    capacity_gb: 1300,
                    free_gb: 900,
                    private_views: false,
                },
            ],
        )
        .with_archive_codec(Arc::new(orch_testkit::doubles::cluster::VolumeCodec)),
    )
}

fn hosted(rig: &Rig, driver: &Arc<ClusterDriver>) -> Arc<DriverHost> {
    let contract: Arc<dyn EnvironmentDriver> = driver.clone();
    DriverHost::of(
        &rig.settings,
        rig.desk.clone(),
        vec![HostDriver::new(contract)],
    )
}

async fn content_command<B: orch_link_contract::commands::CommandBody>(
    rig: &Rig,
    key: &SerialKey,
    body: &B,
) -> orch_types::Json {
    let command = orch_types::new_id();
    rig.blobs.open_command(command);
    rig.controller
        .issue_with_id(command, key, rig.deadline(), body);
    match tokio::time::timeout(common::PATIENCE, rig.controller.wait_command(command))
        .await
        .expect("the content command must finish")
    {
        Progress::Succeeded(value) => value,
        other => panic!("the content command did not succeed: {other:?}"),
    }
}

#[tokio::test]
async fn a_new_drivers_configuration_is_accepted_only_when_it_matches_its_declared_schema() {
    let driver = cluster();
    let mut rig = Rig::new();
    rig.settings
        .driver_configurations
        .insert("cluster-test".into(), json!({"namespace":"agents"}));
    let host = hosted(&rig, &driver);
    assert_eq!(host.names(), ["cluster-test"]);
    assert!(
        host.check_configuration(
            "cluster-test",
            rig.settings.driver_configurations.get("cluster-test")
        )
        .is_ok()
    );
    assert!(
        host.check_configuration("cluster-test", None)
            .unwrap_err()
            .detail
            .contains("restart")
    );
    assert!(
        host.check_configuration("cluster-test", Some(&json!({"namespace":"different"})))
            .unwrap_err()
            .detail
            .contains("restart")
    );
    for invalid in [
        json!({"namespace":null}),
        json!({"namespace":17}),
        json!({"namespace":""}),
        json!({"namespace":"agents","extra":true}),
    ] {
        rig.settings
            .driver_configurations
            .insert("cluster-test".into(), invalid);
        let host = hosted(&rig, &driver);
        assert!(host.names().is_empty());
        assert!(host.failures()["cluster-test"].contains("configuration is invalid"));
    }
}

#[tokio::test]
async fn an_unavailable_pool_observation_never_substitutes_the_nodes_machine_or_directory() {
    let memory = orch_testkit::doubles::driver::MemoryDriver::new("unobserved-pool");
    let mut descriptor = memory.describe();
    descriptor.host_scope = Some(orch_driver_contract::host::HostScope::Driver);
    let mut rig = Rig::with_driver(memory.with_descriptor(descriptor));
    rig.start().await;
    let probe: NodeProbeResult =
        serde_json::from_value(rig.succeed(&SerialKey::Machine, &NodeProbe {}).await).unwrap();
    assert_eq!(
        (
            probe.capacity.cpus,
            probe.capacity.memory_mb,
            probe.capacity.disk_gb
        ),
        (0, 0, 0)
    );
    assert!(
        probe.storages.is_empty(),
        "the node's filesystem is not a storage class"
    );
    let storages = orch_node_core::storages::Storages::new(&rig.settings, rig.drivers().await);
    assert!(
        orch_node_core::facts::LoadSampler::new()
            .sample(&storages)
            .is_none()
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_pool_reports_its_own_capacity_storage_classes_and_reasons_without_measuring_the_node() {
    let driver = cluster();
    let mut rig = Rig::new();
    rig.start_with(hosted(&rig, &driver)).await;
    let probe: NodeProbeResult =
        serde_json::from_value(rig.succeed(&SerialKey::Machine, &NodeProbe {}).await).unwrap();
    assert_eq!(
        (
            probe.capacity.cpus,
            probe.capacity.memory_mb,
            probe.capacity.disk_gb
        ),
        (8, 18000, 2000)
    );
    assert_eq!(
        probe
            .storages
            .iter()
            .map(|storage| storage.name.as_str())
            .collect::<Vec<_>>(),
        ["bulk", "fast"]
    );
    assert_eq!(
        probe
            .storages
            .iter()
            .map(|storage| storage.free_gb)
            .sum::<i64>(),
        1300
    );
    let descriptor = &probe.drivers["cluster-test"];
    let support = descriptor.availability();
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
        assert!(!support[feature].supported);
        assert!(
            support[feature]
                .reason
                .as_ref()
                .is_some_and(|reason| !reason.is_empty())
        );
    }
    assert!(
        !descriptor
            .live_changes
            .gateway_given
            .iter()
            .any(|key| key == "port_host")
    );
    assert!(
        descriptor
            .live_changes
            .gateway_given
            .iter()
            .any(|key| key == "port_place")
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_failed_member_recovers_on_another_machine_and_removal_protects_the_live_successors_volumes()
 {
    let driver = cluster();
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start_with(hosted(&rig, &driver)).await;
    let mut first = rig.spec(RuntimeKind::Environment, "cluster-one");
    first.driver = "cluster-test".into();
    for volume in first.volumes.values_mut() {
        volume.storage = Some("fast".into());
    }
    let mut other = rig.spec(RuntimeKind::Environment, "cluster-two");
    other.driver.clone_from(&first.driver);
    for volume in other.volumes.values_mut() {
        volume.storage = Some("bulk".into());
    }
    let one = rig.made(&first).await;
    let two = rig.made(&other).await;
    assert_ne!(driver.machine_of(&one), driver.machine_of(&two));
    assert!(matches!(
        rig.start_runtime(first.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(matches!(
        rig.start_runtime(other.runtime_id).await,
        Progress::Succeeded(_)
    ));
    let workspace = &first.volumes[&VolumeRole::Workspace].name;
    driver
        .write_volume(workspace, "proof.txt", b"persistent".to_vec())
        .unwrap();
    let failed = driver.machine_of(&one).unwrap();
    driver.fail_machine(&failed);
    assert_eq!(
        driver.inspect(&one).await.unwrap().state,
        UnitStateKind::Failed
    );
    assert!(matches!(
        rig.remove_runtime(first.runtime_id).await,
        Progress::Succeeded(_)
    ));
    let mut recovered = first.clone();
    recovered.runtime_id = orch_types::new_id();
    recovered.name = format!("{}-recovered", first.name);
    recovered.labels =
        orch_driver_contract::runtime::unit_labels(SYSTEM, recovered.kind, &recovered.runtime_id);
    let next = rig.made(&recovered).await;
    assert_ne!(driver.machine_of(&next).unwrap(), failed);
    assert!(matches!(
        rig.start_runtime(recovered.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(driver.volume_files(workspace)["proof.txt"], b"persistent");
    let remove = NodeVolumeRemove {
        driver: recovered.driver.clone(),
        name: workspace.clone(),
        storage: Some("fast".into()),
        labels: Default::default(),
    };
    assert_eq!(
        rig.refused(&SerialKey::Volume(workspace.clone()), &remove)
            .await
            .code,
        "conflict"
    );
    assert!(matches!(
        rig.stop_runtime(recovered.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(
        driver.remove_volume(workspace).await.is_err(),
        "a stopped successor still owns its persistent volumes"
    );
    assert_eq!(driver.volume_files(workspace)["proof.txt"], b"persistent");
    assert!(matches!(
        rig.remove_runtime(recovered.runtime_id).await,
        Progress::Succeeded(_)
    ));
    rig.succeed(&SerialKey::Volume(workspace.clone()), &remove)
        .await;
    assert!(driver.volume_files(workspace).is_empty());
    assert_eq!(
        driver.inspect(&two).await.unwrap().state,
        UnitStateKind::Running
    );
    rig.stop().await;
}

#[tokio::test]
async fn an_image_only_driver_refuses_a_build_before_any_source_work() {
    let driver = cluster();
    let mut rig = Rig::new();
    rig.start_with(hosted(&rig, &driver)).await;
    let request = NodeImageEnsure {
        driver: "cluster-test".into(),
        spec: json!({"build":{"dockerfile":"FROM x"}}),
        job_id: None,
    };
    let failure = rig
        .refused(&SerialKey::Image("unavailable-build".into()), &request)
        .await;
    assert_eq!(failure.code, "unsupported");
    assert!(failure.detail.contains("no builder"), "{}", failure.detail);
    rig.stop().await;
}

#[tokio::test]
async fn a_pool_that_cannot_transfer_installed_files_never_fetches_the_requested_archive() {
    use orch_driver_contract::kept::KeptImage;
    use orch_link_contract::commands::{BlobRef, NodeKeptImport};
    let driver = cluster();
    let mut rig = Rig::new();
    rig.start_with(hosted(&rig, &driver)).await;
    let image = KeptImage {
        system: SYSTEM.into(),
        environment: orch_types::new_id(),
        generation: 1,
        capture: orch_types::new_id(),
        reference: "opaque-kept-artifact".into(),
        bytes: 0,
        total_bytes: 0,
        layers: 0,
    };
    let request = NodeKeptImport {
        driver: "cluster-test".into(),
        image: image.clone(),
        archive: BlobRef {
            hash: orch_types::Hash::of(b"an archive unavailable to this command"),
            size: 0,
        },
    };
    let failure = rig
        .refused(&SerialKey::Image(image.reference), &request)
        .await;
    assert_eq!(failure.code, "unsupported");
    assert_eq!(
        failure.detail,
        driver.describe().kept_file_transfer.reason.unwrap()
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_running_pool_snapshots_and_streams_the_selected_store_before_any_successor_exists() {
    use orch_driver_contract::volumes::OwnedVolumeSelector;
    use orch_link_contract::commands::{
        NodeRuntimeVolumeImport, NodeRuntimeVolumesExport, NodeRuntimeVolumesExportResult,
        NodeRuntimeVolumesMeasure, NodeRuntimeVolumesMeasureResult, NodeVolumeEnsure,
        SelectedVolume,
    };
    use orch_testkit::doubles::cluster::ClusterFile;
    use std::collections::BTreeMap;
    let driver = cluster();
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start_with(hosted(&rig, &driver)).await;
    let mut spec = rig.spec(RuntimeKind::Environment, "archive-source");
    spec.driver = "cluster-test".into();
    spec.volumes.get_mut(&VolumeRole::Home).unwrap().name =
        format!("orch-home-{}", spec.runtime_id);
    let unit = rig.made(&spec).await;
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    let source = spec.volumes[&VolumeRole::Home].clone();
    // Distinct values make owners, modes, timestamps and binary xattrs part of the witness.
    let witness = ClusterFile {
        data: b"recorded root".to_vec(),
        uid: 1234,
        gid: 2345,
        mode: 0o640,
        atime: 1710000000,
        atime_ns: 23,
        mtime: 1720000000,
        mtime_ns: 47,
        xattrs: BTreeMap::from([("user.proof".into(), vec![0, 255, 42])]),
    };
    let path = ".orch/file-states/roots/original.json";
    driver
        .write_volume_file(&source.name, path, witness.clone())
        .unwrap();
    driver
        .write_volume(
            &source.name,
            ".orch/file-states/source-credential-interrupted/password",
            b"transient".to_vec(),
        )
        .unwrap();
    driver
        .write_volume(&source.name, "ordinary-home-file", b"not selected".to_vec())
        .unwrap();
    let selected = SelectedVolume {
        selector: OwnedVolumeSelector::FileStates,
        volume: source,
    };
    let measure: NodeRuntimeVolumesMeasureResult = serde_json::from_value(
        rig.succeed(
            &SerialKey::Runtime(spec.runtime_id),
            &NodeRuntimeVolumesMeasure {
                work: None,
                runtime_id: spec.runtime_id,
                volumes: vec![selected.clone()],
            },
        )
        .await,
    )
    .unwrap();
    assert_eq!(measure.volumes[0].measure.file_count, 1);
    assert_eq!(measure.volumes[0].measure.file_states.unwrap().roots, 1);
    let exported: NodeRuntimeVolumesExportResult = serde_json::from_value(
        content_command(
            &rig,
            &SerialKey::Runtime(spec.runtime_id),
            &NodeRuntimeVolumesExport {
                work: None,
                runtime_id: spec.runtime_id,
                volumes: vec![selected],
                part_bytes: 512,
            },
        )
        .await,
    )
    .unwrap();
    assert!(
        exported.volumes[0].archive.parts.len() > 1,
        "the archive really crosses content part boundaries"
    );
    assert_eq!(
        driver.inspect(&unit).await.unwrap().state,
        UnitStateKind::Running
    );
    let successor = orch_types::new_id();
    let mut target = spec.volumes[&VolumeRole::Home].clone();
    target.name = format!("orch-home-{successor}");
    rig.succeed(
        &SerialKey::Volume(target.name.clone()),
        &NodeVolumeEnsure {
            driver: "cluster-test".into(),
            name: target.name.clone(),
            storage: target.storage.clone(),
            labels: BTreeMap::from([
                ("orch.system".into(), SYSTEM.into()),
                ("orch.environment_id".into(), successor.to_string()),
                ("orch.runtime_id".into(), successor.to_string()),
                ("orch.carried".into(), "true".into()),
            ]),
        },
    )
    .await;
    let import = NodeRuntimeVolumeImport {
        work: None,
        provider_id: None,
        runtime_id: successor,
        driver: "cluster-test".into(),
        environment_id: successor,
        selected: SelectedVolume {
            selector: OwnedVolumeSelector::FileStates,
            volume: target.clone(),
        },
        archive: exported.volumes[0].archive.clone(),
    };
    let mut foreign = import.clone();
    foreign.environment_id = orch_types::new_id();
    let requests = rig.blobs.requests();
    assert!(
        rig.refused(&SerialKey::Runtime(successor), &foreign)
            .await
            .detail
            .contains("ownership")
    );
    assert_eq!(
        rig.blobs.requests(),
        requests,
        "a foreign owner is refused before fetching archive bytes"
    );
    assert!(driver.stored_files(&target.name).is_empty());
    content_command(&rig, &SerialKey::Runtime(successor), &import).await;
    assert_eq!(
        driver.stored_files(&target.name),
        BTreeMap::from([(path.into(), witness)])
    );
    assert!(
        driver.unit_spec(successor).is_none(),
        "import precedes successor creation"
    );
    let mut next = spec.clone();
    next.runtime_id = successor;
    next.enroll_token = rig.token().into();
    next.name = format!("{}-successor", spec.name);
    for volume in next.volumes.values_mut() {
        volume.name = format!("{}-{successor}", volume.name);
    }
    next.volumes.insert(VolumeRole::Home, target);
    next.labels = orch_driver_contract::runtime::unit_labels(SYSTEM, next.kind, &successor);
    let next_unit = rig.made(&next).await;
    assert!(matches!(
        rig.start_runtime(successor).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(
        driver.inspect(&next_unit).await.unwrap().state,
        UnitStateKind::Running
    );
    assert_eq!(
        rig.refused(&SerialKey::Runtime(successor), &import)
            .await
            .code,
        "conflict"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_buffered_archive_waits_for_the_drivers_open_input_before_feeding_any_bytes() {
    use orch_driver_contract::volumes::{
        OwnedVolumeSelector, VolumeProgress, VolumeRequest, VolumeWork,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Work(Arc<AtomicBool>);
    #[async_trait::async_trait]
    impl VolumeWork for Work {
        fn input_ready(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
        fn report(&self, _: VolumeProgress) {}
        fn is_cancelled(&self) -> bool {
            false
        }
        async fn cancelled(&self) {
            std::future::pending::<()>().await;
        }
        fn timeout_secs(&self) -> u64 {
            u64::from(
                orch_link_contract::commands::CommandKind::NodeRuntimeVolumeImport.deadline_secs(),
            )
        }
    }
    struct Input<'a> {
        bytes: &'a [u8],
        ready: Arc<AtomicBool>,
    }
    impl tokio::io::AsyncRead for Input<'_> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            assert!(
                self.ready.load(Ordering::SeqCst),
                "the node must wait for its reader before consuming an already buffered archive"
            );
            let bytes = self.bytes;
            let count = bytes.len().min(buffer.remaining());
            buffer.put_slice(&bytes[..count]);
            self.bytes = &bytes[count..];
            std::task::Poll::Ready(Ok(()))
        }
    }
    let driver = cluster();
    let rig = Rig::new();
    let host = hosted(&rig, &driver);
    let volume = VolumeRequest {
        name: format!("orch-home-{}", orch_types::new_id()),
        storage: Some("fast".into()),
        labels: Default::default(),
    };
    driver.ensure_volume(&volume).await.unwrap();
    driver
        .write_volume(&volume.name, "proof", b"original".to_vec())
        .unwrap();
    let archive = driver
        .volume_archive(&volume.name, OwnedVolumeSelector::Home)
        .unwrap();
    driver
        .write_volume(&volume.name, "proof", b"changed".to_vec())
        .unwrap();
    let ready = Arc::new(AtomicBool::new(false));
    let work: Arc<dyn VolumeWork> = Arc::new(Work(ready.clone()));
    let mut input = Input {
        bytes: &archive,
        ready,
    };
    let installed = host.get("cluster-test").unwrap();
    tokio::time::timeout(
        common::PATIENCE,
        installed.extras().import_selected_volume_stream_reporting(
            &volume,
            OwnedVolumeSelector::Home,
            &mut input,
            &work,
        ),
    )
    .await
    .expect("the input-open handshake must finish")
    .unwrap();
    assert_eq!(driver.volume_files(&volume.name)["proof"], b"original");
}

#[tokio::test]
async fn cluster_inventory_uses_owned_opaque_incarnations_and_keeps_stopped_and_foreign_dependencies()
 {
    use orch_driver_contract::resources::{ResourceKind, ResourceRef};
    use orch_driver_contract::runtime::ToolRef;
    use orch_driver_contract::volumes::VolumeRequest;
    let driver = cluster();
    let rig = Rig::new();
    let mut own = rig.spec(RuntimeKind::Environment, "inventory-own");
    own.driver = "cluster-test".into();
    own.tools = vec![ToolRef {
        name: "compiler".into(),
        version: "test-build".into(),
    }];
    let unit = driver.create(&own).await.unwrap();
    driver.start(&unit).await.unwrap();
    let workspace = &own.volumes[&VolumeRole::Workspace];
    driver
        .write_volume(&workspace.name, "proof", b"retained".to_vec())
        .unwrap();
    let first = driver.resources(SYSTEM).await.unwrap();
    assert!(
        first
            .iter()
            .all(|entry| entry.labels["orch.system"] == SYSTEM)
    );
    assert!(
        first
            .iter()
            .all(|entry| entry.size_bytes.is_none() && entry.last_used_at.is_none())
    );
    let unit_id = first
        .iter()
        .find(|entry| entry.resource.kind == ResourceKind::Unit)
        .unwrap()
        .resource
        .clone();
    let volume_id = first
        .iter()
        .find(|entry| entry.resource.kind == ResourceKind::Volume && entry.name == workspace.name)
        .unwrap()
        .resource
        .clone();
    assert_ne!(unit_id.id, unit.reference);
    assert_ne!(volume_id.id, workspace.name);
    assert!(
        driver
            .remove_resource(
                SYSTEM,
                &ResourceRef {
                    kind: ResourceKind::Image,
                    id: volume_id.id.clone()
                }
            )
            .await
            .is_err(),
        "a kind is part of the exact resource identity"
    );
    assert!(driver.remove_resource(SYSTEM, &unit_id).await.is_err());
    assert!(
        driver
            .stop_resource(SYSTEM, &volume_id, std::time::Duration::ZERO)
            .await
            .is_err()
    );
    assert!(
        driver
            .stop_resource("another-installation", &unit_id, std::time::Duration::ZERO)
            .await
            .is_err()
    );
    assert_eq!(
        driver.inspect(&unit).await.unwrap().state,
        UnitStateKind::Running
    );
    assert!(
        driver
            .remove_resource("another-installation", &volume_id)
            .await
            .is_err()
    );
    assert!(
        driver
            .remove_resource(
                SYSTEM,
                &ResourceRef {
                    kind: ResourceKind::Volume,
                    id: "unknown-object".into()
                }
            )
            .await
            .is_err()
    );
    driver
        .stop_resource(SYSTEM, &unit_id, std::time::Duration::ZERO)
        .await
        .unwrap();
    let stopped = driver.resources(SYSTEM).await.unwrap();
    let held = stopped
        .iter()
        .find(|entry| entry.resource == unit_id)
        .unwrap();
    assert!(!held.busy);
    assert!(held.depends_on.contains(&volume_id));
    assert!(held.depends_on.contains(&ResourceRef {
        kind: ResourceKind::ToolVersion,
        id: "compiler/test-build".into()
    }));
    assert!(driver.remove_resource(SYSTEM, &volume_id).await.is_err());
    let mut foreign = rig.spec(RuntimeKind::Environment, "inventory-foreign");
    foreign.driver = own.driver.clone();
    foreign.labels = orch_driver_contract::runtime::unit_labels(
        "another-installation",
        foreign.kind,
        &foreign.runtime_id,
    );
    foreign
        .volumes
        .insert(VolumeRole::Workspace, workspace.clone());
    let foreign_unit = driver.create(&foreign).await.unwrap();
    let foreign_before = driver.resources("another-installation").await.unwrap();
    let foreign_id = foreign_before
        .iter()
        .find(|entry| entry.resource.kind == ResourceKind::Unit)
        .unwrap()
        .resource
        .clone();
    driver.remove_resource(SYSTEM, &unit_id).await.unwrap();
    assert_eq!(driver.volume_files(&workspace.name)["proof"], b"retained");
    let replacement = driver.create(&own).await.unwrap();
    driver.start(&replacement).await.unwrap();
    let replacement_id = driver
        .resources(SYSTEM)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.resource.kind == ResourceKind::Unit)
        .unwrap()
        .resource;
    assert_ne!(replacement_id, unit_id);
    assert!(
        driver
            .stop_resource(SYSTEM, &unit_id, std::time::Duration::ZERO)
            .await
            .is_err()
    );
    assert_eq!(
        driver.inspect(&replacement).await.unwrap().state,
        UnitStateKind::Running
    );
    driver
        .stop_resource(SYSTEM, &replacement_id, std::time::Duration::ZERO)
        .await
        .unwrap();
    driver
        .remove_resource(SYSTEM, &replacement_id)
        .await
        .unwrap();
    assert!(
        driver.remove_resource(SYSTEM, &volume_id).await.is_err(),
        "a foreign stopped consumer still protects the exact storage object"
    );
    assert!(driver.remove_resource(SYSTEM, &foreign_id).await.is_err());
    assert_eq!(
        driver.resources("another-installation").await.unwrap(),
        foreign_before
    );
    driver
        .remove_resource("another-installation", &foreign_id)
        .await
        .unwrap();
    assert_eq!(
        driver.inspect(&foreign_unit).await.unwrap().state,
        UnitStateKind::Absent
    );
    driver.remove_resource(SYSTEM, &volume_id).await.unwrap();
    let request = VolumeRequest {
        name: workspace.name.clone(),
        storage: workspace.storage.clone(),
        labels: own.labels.clone(),
    };
    driver.ensure_volume(&request).await.unwrap();
    driver
        .write_volume(&workspace.name, "proof", b"replacement".to_vec())
        .unwrap();
    let replaced = driver.resources(SYSTEM).await.unwrap();
    let current = &replaced
        .iter()
        .find(|entry| entry.name == workspace.name)
        .unwrap()
        .resource;
    assert_ne!(
        current, &volume_id,
        "a reused logical name is a different physical resource"
    );
    assert!(
        driver.remove_resource(SYSTEM, &volume_id).await.is_err(),
        "an old plan cannot remove its replacement"
    );
    assert_eq!(
        driver.volume_files(&workspace.name)["proof"],
        b"replacement"
    );
    assert_eq!(
        driver
            .resources("another-installation")
            .await
            .unwrap()
            .len(),
        2,
        "the foreign installation's own home and state were never purged"
    );
}
