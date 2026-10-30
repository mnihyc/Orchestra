//! Cluster-backed checks for the connection, reported capacity and claim-incarnation primitives.
//! Run only against the explicitly supplied disposable fixture.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::core::v1::{Node, PersistentVolumeClaim, Pod};
use kube::Api;
use kube::api::{
    AttachParams, DeleteParams, ListParams, Patch, PatchParams, PostParams, Preconditions,
};
use orch_driver_contract::runtime::{
    LABEL_SYSTEM, RuntimeKind, RuntimeSpec, unit_labels, unit_name,
};
use orch_driver_contract::units::UnitRef;
use orch_driver_contract::volumes::VolumeRequest;

use crate::config::{KubernetesConfig, RuntimeBoundary, RuntimeClass};
use crate::{
    admission, capacity, claim_service, claims, client, launch, logs, platform, pod_identity,
    receipts, unit_create,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const FIXTURE_NAMESPACE: &str = "orch-k8s-proof";

fn private_parent() -> tempfile::TempDir {
    let parent = tempfile::tempdir().unwrap();
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    parent
}

#[tokio::test]
async fn configured_k3s_checks_capacity_and_fences_an_exact_claim() {
    let kubeconfig = std::env::var_os("ORCH_K3D_TEST_KUBECONFIG")
        .expect("set ORCH_K3D_TEST_KUBECONFIG to the disposable k3d fixture");
    let namespace_uid = std::env::var("ORCH_K3D_TEST_NAMESPACE_UID")
        .expect("set ORCH_K3D_TEST_NAMESPACE_UID to the disposable namespace UID");
    let image = std::env::var("ORCH_K3D_TEST_IMAGE")
        .expect("set ORCH_K3D_TEST_IMAGE to the fixture's disposable BusyBox image");
    let config = KubernetesConfig {
        kubeconfig: Some(kubeconfig.into()),
        namespace: FIXTURE_NAMESPACE.into(),
        storage_class: "local-path".into(),
        volume_request: "1Mi".into(),
        runtime_classes: vec![RuntimeClass {
            runtime: "runc".into(),
            class_name: None,
            boundary: RuntimeBoundary::SharedKernel,
        }],
        image_pull_secrets: Vec::new(),
        tool_stager_image: format!("test.invalid/stager@sha256:{}", "a".repeat(64)),
    };
    let cluster = client::Cluster::connect(config.clone(), REQUEST_TIMEOUT)
        .await
        .expect("connect to the explicitly configured cluster");
    assert_eq!(
        cluster.namespace_uid, namespace_uid,
        "refuse another namespace incarnation"
    );
    let nodes: Api<Node> = Api::all(cluster.client.clone());
    let observed =
        tokio::time::timeout(REQUEST_TIMEOUT, nodes.list(&ListParams::default().limit(1)))
            .await
            .expect("list nodes before the deadline")
            .expect("the fixture permits node discovery");
    assert_eq!(
        observed.items.len(),
        1,
        "the disposable fixture has one worker"
    );
    assert!(
        observed
            .metadata
            .continue_
            .as_deref()
            .is_none_or(str::is_empty),
        "refuse a larger cluster before mutation"
    );
    let selector = platform::worker_selector("linux", std::env::consts::ARCH).unwrap();
    let capacity = capacity::aggregate(&observed.items, &selector).unwrap();
    assert!(capacity.cpus > 0);
    assert!(capacity.cpu_millis.is_some_and(|millis| millis > 0));
    assert!(capacity.memory_mb > 0);
    assert_eq!(capacity.disk_free_gb, None);

    let claims_api: Api<PersistentVolumeClaim> =
        Api::namespaced(cluster.client.clone(), FIXTURE_NAMESPACE);
    let id = orch_types::new_id();
    let volume = VolumeRequest {
        name: format!("orch-k3d-proof-{}", id.simple()),
        storage: Some("local-path".into()),
        labels: BTreeMap::from([(LABEL_SYSTEM.into(), "k3d-proof".into())]),
    };
    let claim_parent = private_parent();
    let claim_journal = Arc::new(
        receipts::ReceiptStore::open(&claim_parent.path().join("journal"), &cluster.receipt_scope)
            .unwrap(),
    );
    let claims_service = claim_service::ClaimService::new(
        cluster.client.clone(),
        config.clone(),
        Arc::clone(&claim_journal),
        REQUEST_TIMEOUT,
    )
    .unwrap();
    let receipt = claims_service
        .ensure(&volume, tokio::time::Instant::now() + REQUEST_TIMEOUT)
        .await
        .expect("create and verify the fixture claim");
    let intent = claim_journal.begin(&volume).unwrap().intent;
    assert!(!receipt.uid.is_empty());
    let same = claims::ensure_claim(
        &claims_api,
        &config,
        &volume,
        intent,
        Some(&receipt.uid),
        REQUEST_TIMEOUT,
    )
    .await
    .expect("the matching receipt identifies the same claim");
    assert_eq!(same.uid, receipt.uid);
    assert_eq!(
        claims_service
            .ensure(&volume, tokio::time::Instant::now() + REQUEST_TIMEOUT)
            .await
            .unwrap()
            .uid,
        receipt.uid
    );
    assert!(
        matches!(
            claims::ensure_claim(&claims_api, &config, &volume, intent, None, REQUEST_TIMEOUT)
                .await,
            Err(orch_driver_contract::driver::DriverError::Unsupported(_))
        ),
        "a conflict without a recorded UID must not be adopted"
    );
    let mut larger = config.clone();
    larger.volume_request = "2Mi".into();
    assert!(
        claims::ensure_claim(
            &claims_api,
            &larger,
            &volume,
            intent,
            Some(&receipt.uid),
            REQUEST_TIMEOUT
        )
        .await
        .is_err(),
        "a smaller retained claim cannot satisfy a larger request"
    );
    assert!(
        claims::ensure_claim(
            &claims_api,
            &config,
            &volume,
            intent,
            Some("a-different-uid"),
            REQUEST_TIMEOUT,
        )
        .await
        .is_err()
    );
    let runtime = orch_types::new_id();
    let pod_name = format!("orch-log-proof-{}", runtime.simple());
    let pods: Api<Pod> = Api::namespaced(cluster.client.clone(), FIXTURE_NAMESPACE);
    let request: Pod = serde_json::from_value(serde_json::json!({
        "apiVersion":"v1", "kind":"Pod", "metadata":{"name":pod_name.clone(), "namespace":FIXTURE_NAMESPACE,
            "annotations":unit_labels("k3d-proof", RuntimeKind::Environment, &runtime)},
        "spec":{"restartPolicy":"Never", "automountServiceAccountToken":false,
            "volumes":[{"name":"claim","persistentVolumeClaim":{"claimName":receipt.native_name.clone()}}], "containers":[{
            "name":"workload", "image":image.clone(),
            "command":["/bin/sh", "-c", "echo fixture-log; sleep 300"],
            "volumeMounts":[{"name":"claim","mountPath":"/mnt/claim"}]}]}
    })).unwrap();
    let pod = tokio::time::timeout(
        REQUEST_TIMEOUT,
        pods.create(&PostParams::default(), &request),
    )
    .await
    .expect("create the fixture Pod before the deadline")
    .expect("create the fixture Pod");
    let unit = UnitRef {
        reference: pod.metadata.uid.expect("the fixture Pod has a UID"),
        name: pod_name,
        runtime_id: runtime,
        kind: RuntimeKind::Environment,
    };
    let running = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let pod = pods.get(&unit.name).await.expect("read the fixture Pod");
            if pod
                .status
                .as_ref()
                .and_then(|status| status.container_statuses.as_ref())
                .is_some_and(|statuses| {
                    statuses.iter().any(|status| {
                        status.name == "workload"
                            && status.container_id.is_some()
                            && status
                                .state
                                .as_ref()
                                .and_then(|state| state.running.as_ref())
                                .is_some()
                    })
                })
            {
                break pod;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("the fixture workload started before the deadline");
    let worker_name = running
        .spec
        .as_ref()
        .and_then(|spec| spec.node_name.as_deref())
        .expect("the fixture has an assigned worker");
    let worker = tokio::time::timeout(REQUEST_TIMEOUT, nodes.get(worker_name))
        .await
        .expect("read the assigned worker before the deadline")
        .expect("the assigned worker exists");
    assert!(
        !platform::worker_for_pod(&running, &worker, &selector, true)
            .unwrap()
            .uid()
            .is_empty()
    );
    let bound = claims_service
        .ensure(&volume, tokio::time::Instant::now() + REQUEST_TIMEOUT)
        .await
        .expect("verify the bound PV and its claim");
    assert_eq!(bound.uid, receipt.uid);
    let binding = claim_journal.begin(&volume).unwrap();
    assert!(binding.pv_uid.is_some() && binding.pv_name.is_some());
    let log_deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
    let output = tokio::time::timeout_at(log_deadline, async {
        loop {
            let output = logs::read_logs(
                &pods,
                logs::LogRead {
                    namespace: FIXTURE_NAMESPACE,
                    system: "k3d-proof",
                    unit: &unit,
                    max_bytes: 1024,
                    request_timeout: REQUEST_TIMEOUT,
                    stall_timeout: Duration::from_secs(5),
                    deadline: log_deadline,
                },
            )
            .await
            .expect("read the original container's bounded logs");
            if output.contains("fixture-log") {
                break output;
            }
            // Running precedes delivery of the first log bytes. Keep the
            // original absolute log deadline across every readiness read.
            tokio::time::sleep_until(
                (tokio::time::Instant::now() + Duration::from_millis(100)).min(log_deadline),
            )
            .await;
        }
    })
    .await
    .expect("the fixture log arrived before the original deadline");
    assert!(output.contains("fixture-log"));
    let deletion = DeleteParams {
        preconditions: Some(Preconditions {
            uid: Some(unit.reference.clone()),
            resource_version: None,
        }),
        grace_period_seconds: Some(0),
        ..DeleteParams::default()
    };
    tokio::time::timeout(REQUEST_TIMEOUT, pods.delete(&unit.name, &deletion))
        .await
        .expect("delete the fixture Pod before the deadline")
        .expect("delete only its exact UID");
    let deletion = DeleteParams {
        preconditions: Some(Preconditions {
            uid: Some(receipt.uid.clone()),
            resource_version: None,
        }),
        ..DeleteParams::default()
    };
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        claims_api.delete(&receipt.native_name, &deletion),
    )
    .await
    .expect("delete the fixture claim before the deadline")
    .expect("delete only its exact UID");

    let runtime = orch_types::new_id();
    let spec: RuntimeSpec = serde_json::from_value(serde_json::json!({
        "runtime_id":runtime, "kind":"environment", "name":unit_name(RuntimeKind::Environment, &runtime),
        "driver":"kubernetes", "spec":{"image":image.clone()},
        "resources":{}, "roles":["environment"], "gateway_url":"wss://gateway.example:9443",
        "enroll_token":"synthetic-enrollment", "ca_cert":"synthetic-authority", "tools":[],
        "volumes":{"workspace":{"name":"proof-work","storage":null},"home":{"name":"proof-home","storage":null},
            "state":{"name":"proof-state","storage":null}}, "mounts":[], "env":{},
        "labels":unit_labels("k3d-proof", RuntimeKind::Environment, &runtime)
    })).unwrap();
    let plan = admission::plan(&spec, &config, &BTreeSet::new()).unwrap();
    let launch = launch::prepare(&spec, &plan, &[]).unwrap();
    let parent = private_parent();
    let journal = Arc::new(
        receipts::ReceiptStore::open(&parent.path().join("journal"), &cluster.receipt_scope)
            .unwrap(),
    );
    let service = unit_create::UnitService::new(
        cluster.client.clone(),
        config.clone(),
        journal,
        REQUEST_TIMEOUT,
    )
    .unwrap();
    let made = service
        .create(
            &spec,
            &plan,
            &launch,
            "linux",
            std::env::consts::ARCH,
            tokio::time::Instant::now() + REQUEST_TIMEOUT,
        )
        .await
        .expect("create and verify an admitted fixture Pod");
    let retried = service
        .create(
            &spec,
            &plan,
            &launch,
            "linux",
            std::env::consts::ARCH,
            tokio::time::Instant::now() + REQUEST_TIMEOUT,
        )
        .await
        .expect("retry the same Pod incarnation");
    assert_eq!(made, retried);
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        pods.patch(
            &made.name,
            &PatchParams::default(),
            &Patch::Merge(
                serde_json::json!({"metadata":{"annotations":{"unexpected-fixture":"changed"}}}),
            ),
        ),
    )
    .await
    .expect("patch the fixture Pod before the deadline")
    .expect("change only the fixture annotation");
    assert!(
        service
            .create(
                &spec,
                &plan,
                &launch,
                "linux",
                std::env::consts::ARCH,
                tokio::time::Instant::now() + REQUEST_TIMEOUT
            )
            .await
            .is_err(),
        "unexpected annotations must be refused"
    );
    let deletion = DeleteParams {
        preconditions: Some(Preconditions {
            uid: Some(made.reference.clone()),
            resource_version: None,
        }),
        grace_period_seconds: Some(0),
        ..DeleteParams::default()
    };
    tokio::time::timeout(REQUEST_TIMEOUT, pods.delete(&made.name, &deletion))
        .await
        .expect("delete the admitted fixture Pod before the deadline")
        .expect("delete only its exact UID");

    let runtime = orch_types::new_id();
    let name = unit_name(RuntimeKind::Environment, &runtime);
    let labels = unit_labels("k3d-proof", RuntimeKind::Environment, &runtime);
    let request: Pod = serde_json::from_value(serde_json::json!({
        "apiVersion":"v1", "kind":"Pod", "metadata":{"name":name.clone(),"namespace":FIXTURE_NAMESPACE,"annotations":labels.clone()},
        "spec":{"restartPolicy":"Never", "automountServiceAccountToken":false,
            "volumes":[{"name":"ready","emptyDir":{}}],
            "initContainers":[{"name":"stage","image":image.clone(),"command":["/bin/sh","-c","while [ ! -f /ready/done ]; do sleep 1; done"],
                "volumeMounts":[{"name":"ready","mountPath":"/ready"}]}],
            "containers":[{"name":"workload","image":image,"command":["/bin/sh","-c","sleep 300"]}]}
    })).unwrap();
    let pod = tokio::time::timeout(
        REQUEST_TIMEOUT,
        pods.create(&PostParams::default(), &request),
    )
    .await
    .expect("create the staging fixture before the deadline")
    .expect("create the staging fixture");
    let unit = UnitRef {
        reference: pod.metadata.uid.expect("the staging fixture has a UID"),
        name,
        runtime_id: runtime,
        kind: RuntimeKind::Environment,
    };
    let (running, stage) = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let pod = pods
                .get(&unit.name)
                .await
                .expect("read the staging fixture");
            if let Ok(stage) =
                pod_identity::read_stage(&pod, FIXTURE_NAMESPACE, "k3d-proof", &unit, false)
            {
                break (pod, stage);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("the staging fixture started before the deadline");
    let worker_name = running
        .spec
        .as_ref()
        .and_then(|spec| spec.node_name.as_deref())
        .unwrap();
    let worker = tokio::time::timeout(REQUEST_TIMEOUT, nodes.get(worker_name))
        .await
        .unwrap()
        .unwrap();
    let worker = platform::worker_for_pod(&running, &worker, &selector, true).unwrap();
    let stage_parent = private_parent();
    let stage_journal =
        receipts::ReceiptStore::open(&stage_parent.path().join("journal"), &cluster.receipt_scope)
            .unwrap();
    let saved = stage_journal
        .begin_unit(runtime, RuntimeKind::Environment, &unit.name, &labels)
        .unwrap();
    stage_journal
        .bind_pod(runtime, saved.intent, &unit.reference)
        .unwrap();
    stage_journal
        .bind_worker(runtime, &unit.reference, &worker)
        .unwrap();
    stage_journal
        .bind_stage(runtime, &unit.reference, &stage)
        .unwrap();
    let params = AttachParams::default()
        .container("stage")
        .stdout(false)
        .stderr(true);
    let mut exec = tokio::time::timeout(
        REQUEST_TIMEOUT,
        pods.exec(&unit.name, ["/bin/touch", "/ready/done"], &params),
    )
    .await
    .expect("open the fixture exec before the deadline")
    .expect("open the fixture exec");
    let status = exec.take_status().expect("exec supplies a status channel");
    let (status, joined) =
        tokio::time::timeout(REQUEST_TIMEOUT, async { tokio::join!(status, exec.join()) })
            .await
            .expect("finish the fixture exec before the deadline");
    joined.expect("the fixture exec transport completed");
    assert_eq!(
        status
            .expect("the fixture exec reported its exit")
            .status
            .as_deref(),
        Some("Success")
    );
    let completed = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let pod = pods
                .get(&unit.name)
                .await
                .expect("read the completed staging fixture");
            if pod
                .status
                .as_ref()
                .and_then(|status| status.init_container_statuses.as_ref())
                .is_some_and(|statuses| {
                    statuses.iter().any(|status| {
                        status.name == "stage"
                            && status
                                .state
                                .as_ref()
                                .and_then(|state| state.terminated.as_ref())
                                .is_some_and(|exit| exit.exit_code == 0)
                    })
                })
            {
                break pod;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("the staging fixture completed before the deadline");
    pod_identity::verify_stage(
        &completed,
        FIXTURE_NAMESPACE,
        "k3d-proof",
        &unit,
        &stage,
        true,
    )
    .unwrap();
    let deletion = DeleteParams {
        preconditions: Some(Preconditions {
            uid: Some(unit.reference.clone()),
            resource_version: None,
        }),
        grace_period_seconds: Some(0),
        ..DeleteParams::default()
    };
    tokio::time::timeout(REQUEST_TIMEOUT, pods.delete(&unit.name, &deletion))
        .await
        .expect("delete the staging fixture before the deadline")
        .expect("delete only its exact UID");
}
