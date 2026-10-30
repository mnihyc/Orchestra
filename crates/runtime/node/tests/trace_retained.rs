//! Real Docker and node-command proof that kept bytes return stopped and frozen purge refuses foreign or successor files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
#[path = "../../../../extensions/drivers/docker/tests/common/mod.rs"]
mod docker_common;

use async_trait::async_trait;
use common::Rig;
use docker_common::Lab;
use orch_driver_contract::{
    driver::{DriverError, EnvironmentDriver},
    runtime::{RuntimeKind, VolumeRole, unit_labels, unit_name},
    sources::ImageEnsured,
    volumes::{StorageInfo, VolumeRequest},
};
use orch_driver_docker::DockerDriver;
use orch_link::commands::Progress;
use orch_link_contract::commands::{NodeRuntimeRemove, SerialKey};
use orch_node_core::{
    drivers::{DriverExtras, DriverHost, HostDriver},
    images::ImageProgress,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

struct Extras(Arc<DockerDriver>);
#[async_trait]
impl DriverExtras for Extras {
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        Some(self.0.storages())
    }
    async fn ensure_image(
        &self,
        spec: &Json,
        _progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.0.ensure_image(spec).await
    }
    async fn volume_exists(&self, request: &VolumeRequest) -> Result<bool, DriverError> {
        self.0.volume_exists(request).await
    }
}

fn frozen(spec: &orch_driver_contract::runtime::RuntimeSpec, environment: Id) -> Vec<Json> {
    let mut references:Vec<Json> = spec.volumes.iter().map(|(role,volume)|json!({"runtime_id":spec.runtime_id,"environment_id":environment,"host_id":new_id(),"driver":"docker","role":match role {VolumeRole::Workspace=>"workspace",VolumeRole::Home=>"home",VolumeRole::State=>"state"},"name":volume.name,"storage":volume.storage})).collect();
    for mount in &spec.mounts {
        if let Some(layer) = &mount.layer {
            references.push(json!({"runtime_id":spec.runtime_id,"environment_id":environment,"host_id":new_id(),
                "driver":"docker","role":format!("over:{}",mount.name),"name":layer.name,"storage":layer.storage}));
        }
    }
    references
}

#[tokio::test]
async fn real_owned_bytes_survive_removal_and_restart_return_stopped_and_only_confirmed_purge_erases_them()
 {
    let lab = Lab::with(|config, _| {
        config
            .labels
            .insert("orch.worker".to_owned(), "trace".to_owned());
    })
    .await
    .expect("Ktrace's retained-byte proof needs the real Docker daemon");
    let mut rig = Rig::new();
    rig.settings.system_name = lab.system.clone();
    rig.settings.drivers = vec!["docker".to_owned()];
    rig.have_connector();
    let driver = Arc::new(lab.driver.clone());
    let host = HostDriver::new(driver.clone()).with_extras(Arc::new(Extras(driver)));
    let drivers = DriverHost::of(&rig.settings, rig.desk.clone(), vec![host]);
    rig.start_with(drivers.clone()).await;
    let environment = new_id();
    let runtime = new_id();
    assert!(
        lab.driver.describe().private_views,
        "this retained-layout proof needs a driver that supports private views"
    );
    let source = lab.path("retained-private-source");
    std::fs::write(source.join("proof"), "original-shared-data").unwrap();
    let private = format!("orch-over-{environment}-records");
    let mut spec=lab.request("environment","trace-kept",|request| {
        request["runtime_id"]=json!(runtime);request["name"]=json!(unit_name(RuntimeKind::Environment,&runtime));
        request["labels"]=json!(unit_labels(&lab.system,RuntimeKind::Environment,&runtime));
        request["labels"]["orch.environment_id"]=json!(environment);
        request["tools"]=json!([]);request["enroll_token"]=json!(rig.token());request["ca_cert"]=json!(rig.authority.certificate().pem());
        request["volumes"]=json!({"workspace":{"name":format!("orch-ws-{environment}"),"storage":null},"home":{"name":format!("orch-home-{environment}"),"storage":null},"state":{"name":format!("orch-state-{environment}"),"storage":null}});
        request["spec"]["user"]=json!({"uid":0,"gid":0});
        request["mounts"]=json!([{ "name":"records","kind":"path","source":source,"target":"/data/records","mode":"private","layer":{"name":private,"storage":null}}]);
        request["env"]=json!({"LAB_RUN":format!("printf workspace-evidence > $ORCH_WORKSPACE_DIR/proof; printf home-evidence > $ORCH_HOME_DIR/proof; printf state-evidence > $ORCH_DATA_DIR/proof; printf private-evidence > /data/records/proof; mkdir -p $ORCH_HOME_DIR/.orch/file-states/{environment}/roots $ORCH_HOME_DIR/.orch/file-states/{environment}/journals; printf recorded-root-evidence > $ORCH_HOME_DIR/.orch/file-states/{environment}/roots/proof; printf restore-journal-evidence > $ORCH_HOME_DIR/.orch/file-states/{environment}/journals/proof")});
    });
    lab.track(&spec);
    let first = rig.made(&spec).await;
    assert!(matches!(
        rig.start_runtime(runtime).await,
        Progress::Succeeded(_)
    ));
    lab.wait_done(&first).await;
    assert!(matches!(
        rig.stop_runtime(runtime).await,
        Progress::Succeeded(_)
    ));
    assert!(matches!(
        rig.remove_runtime(runtime).await,
        Progress::Succeeded(_)
    ));
    for volume in spec.volumes.values() {
        assert!(lab.has_volume(&volume.name).await);
    }
    assert!(lab.has_volume(&private).await);
    assert_eq!(
        std::fs::read_to_string(source.join("proof")).unwrap(),
        "original-shared-data"
    );
    let refs = frozen(&spec, environment);
    let mut foreign = refs.clone();
    foreign[0]["environment_id"] = json!(new_id());
    assert!(matches!(
        rig.send(
            &SerialKey::Runtime(runtime),
            &NodeRuntimeRemove {
                runtime_id: runtime,
                keep_volumes: false,
                volume_refs: foreign
            }
        )
        .await,
        Progress::Failed(_)
    ));
    for volume in spec.volumes.values() {
        assert!(lab.has_volume(&volume.name).await);
    }
    rig.kill().await;
    rig.start_with(drivers).await;
    let restored = new_id();
    spec.runtime_id = restored;
    spec.name = unit_name(RuntimeKind::Environment, &restored);
    spec.labels = unit_labels(&lab.system, RuntimeKind::Environment, &restored);
    spec.labels
        .insert("orch.environment_id".to_owned(), environment.to_string());
    spec.labels
        .insert("orch.restore".to_owned(), "true".to_owned());
    spec.env = BTreeMap::new();
    lab.track(&spec);
    let second = rig.made(&spec).await;
    assert_eq!(
        lab.driver.inspect(&second).await.unwrap().state,
        orch_driver_contract::units::UnitStateKind::Stopped
    );
    for (path, expected) in [
        ("/workspace/proof", "workspace-evidence"),
        ("/home/agent/proof", "home-evidence"),
        ("/var/lib/orch/proof", "state-evidence"),
        ("/data/records/proof", "private-evidence"),
    ] {
        assert_eq!(
            lab.read(&second, path).await.as_deref(),
            Some(expected),
            "{path}"
        );
    }
    for (part, expected) in [
        ("roots", "recorded-root-evidence"),
        ("journals", "restore-journal-evidence"),
    ] {
        let path = format!("/home/agent/.orch/file-states/{environment}/{part}/proof");
        assert_eq!(lab.read(&second, &path).await.as_deref(), Some(expected));
    }
    assert_eq!(
        std::fs::read_to_string(source.join("proof")).unwrap(),
        "original-shared-data"
    );
    let blocked = rig
        .send(
            &SerialKey::Runtime(runtime),
            &NodeRuntimeRemove {
                runtime_id: runtime,
                keep_volumes: false,
                volume_refs: refs.clone(),
            },
        )
        .await;
    assert!(
        matches!(blocked, Progress::Failed(_)),
        "a stopped live successor still owns these files"
    );
    assert!(matches!(
        rig.remove_runtime(restored).await,
        Progress::Succeeded(_)
    ));
    for _ in 0..2 {
        let result = rig
            .send(
                &SerialKey::Runtime(runtime),
                &NodeRuntimeRemove {
                    runtime_id: runtime,
                    keep_volumes: false,
                    volume_refs: refs.clone(),
                },
            )
            .await;
        assert!(
            matches!(&result, Progress::Succeeded(_)),
            "the verified owned purge must succeed: {result:?}"
        );
    }
    for volume in spec.volumes.values() {
        assert!(!lab.has_volume(&volume.name).await);
    }
    assert!(!lab.has_volume(&private).await);
    let missing = new_id();
    spec.runtime_id = missing;
    spec.name = unit_name(RuntimeKind::Environment, &missing);
    spec.labels = unit_labels(&lab.system, RuntimeKind::Environment, &missing);
    spec.labels
        .insert("orch.environment_id".to_owned(), environment.to_string());
    spec.labels
        .insert("orch.restore".to_owned(), "true".to_owned());
    lab.track(&spec);
    assert!(
        matches!(rig.create(&spec).await, Progress::Failed(_)),
        "missing bytes cannot become an empty restored environment"
    );
    for volume in spec.volumes.values() {
        assert!(!lab.has_volume(&volume.name).await);
    }
    rig.stop().await;
    // The fixture owns every exact container and volume name, even if an assertion above fails.
}
