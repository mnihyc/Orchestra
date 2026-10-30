//! A managed Pod request, with no credentials or node paths in its API object.
//!
//! Constructing a request is not enough to run a unit: staging, the authenticated
//! supervisor and exact native identity checks must succeed before it is exposed.

use k8s_openapi::api::core::v1::Pod;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::mounts::MountMode;
use orch_driver_contract::runtime::{RuntimeSpec, VolumeRole};
use serde_json::{Value, json};

use crate::admission::UnitPlan;
use crate::config::KubernetesConfig;
use crate::launch::LaunchPlan;
use crate::platform;

const STATE_DIR: &str = "/var/lib/orch";
const TOOLS_DIR: &str = "/opt/orch";
const RUN_DIR: &str = "/run/orch";
const RENDEZVOUS_DIR: &str = "/run/orch-rendezvous";
const CONTROL_DIR: &str = "/run/orch-control";
const HELPER: &str = "/usr/local/bin/orch-kube-helper";
pub(crate) const SUPERVISOR: &str = "/opt/orch/bin/orch-kube-supervisor";
/// A Pod selector that names one logical runtime, not a free-form user label.
pub(crate) const UNIT_SELECTOR: &str = "orch.io/runtime";
/// The node journals this creation intent before asking the API to create the Pod.
pub(crate) const INTENT_ANNOTATION: &str = "orch.unit-intent";

fn missing_claim(role: &str) -> DriverError {
    DriverError::InvalidSpec(format!("the {role} claim is required"))
}

/// Make a Pod request only after admission. Launch files are transferred privately later.
pub(crate) fn request(
    spec: &RuntimeSpec,
    plan: &UnitPlan,
    config: &KubernetesConfig,
    launch: &LaunchPlan,
    intent: orch_types::Id,
    os: &str,
    arch: &str,
) -> Result<Pod, DriverError> {
    let selector = platform::worker_selector(os, arch)?;
    if spec.labels.contains_key(INTENT_ANNOTATION) {
        return Err(DriverError::InvalidSpec(
            "the unit creation intent is reserved for the driver".into(),
        ));
    }
    let mut annotations = spec.labels.clone();
    annotations.insert(INTENT_ANNOTATION.into(), intent.to_string());
    let mut volumes = vec![
        json!({"name":"tools", "emptyDir":{}}),
        json!({"name":"run", "emptyDir":{"medium":"Memory"}}),
        json!({"name":"rendezvous", "emptyDir":{"medium":"Memory"}}),
        json!({"name":"control", "emptyDir":{"medium":"Memory"}}),
    ];
    let mut mounts = vec![
        json!({"name":"tools", "mountPath":TOOLS_DIR, "readOnly":true}),
        json!({"name":"run", "mountPath":RUN_DIR}),
        json!({"name":"rendezvous", "mountPath":RENDEZVOUS_DIR}),
    ];
    for (role, native_name, path) in [
        (VolumeRole::Workspace, "workspace", plan.workspace.as_str()),
        (VolumeRole::Home, "home", plan.home.as_str()),
        (VolumeRole::State, "state", STATE_DIR),
    ] {
        if let Some(claim) = plan.claims.get(&role) {
            volumes.push(json!({"name":native_name, "persistentVolumeClaim":{"claimName":crate::names::claim(claim)?}}));
            mounts.push(json!({"name":native_name, "mountPath":path}));
        } else if role == VolumeRole::State
            || spec.kind == orch_driver_contract::runtime::RuntimeKind::Environment
        {
            return Err(missing_claim(native_name));
        }
    }
    for (index, mount) in spec.mounts.iter().enumerate() {
        let name = format!("attached-{index}");
        volumes.push(json!({"name":name, "persistentVolumeClaim":{"claimName":crate::names::claim(&mount.source)?}}));
        mounts.push(json!({"name":name, "mountPath":mount.target, "readOnly":mount.mode == MountMode::ReadOnly}));
    }
    let env: Vec<Value> = launch
        .environment
        .iter()
        .map(|(name, value)| json!({"name":name,"value":value}))
        .collect();
    let pull_secrets: Vec<Value> = config
        .image_pull_secrets
        .iter()
        .map(|name| json!({"name":name}))
        .collect();
    let pull_secrets = (!pull_secrets.is_empty()).then_some(pull_secrets);
    let mut security = plan.security_context.clone();
    if let Some(capabilities) = security
        .pointer_mut("/capabilities")
        .and_then(Value::as_object_mut)
    {
        capabilities.retain(|_, value| value.as_array().is_none_or(|items| !items.is_empty()));
    }
    let mut resources = plan.resources.clone();
    if let Some(object) = resources.as_object_mut()
        && object
            .get("limits")
            .and_then(Value::as_object)
            .is_some_and(|limits| limits.is_empty())
    {
        object.remove("limits");
    }
    // Only the helper mounts its private control socket; the rendezvous socket is
    // authenticated and removed before any untrusted workload starts.
    let pod = json!({
        "apiVersion":"v1", "kind":"Pod",
        "metadata": {"name":spec.name, "namespace":config.namespace, "annotations":annotations,
            "labels":{(UNIT_SELECTOR):spec.runtime_id.simple().to_string()}},
        "spec": {
            "automountServiceAccountToken":false,
            "restartPolicy":"Never",
            "shareProcessNamespace":true,
            "nodeSelector":selector,
            "runtimeClassName":plan.class_name,
            "imagePullSecrets":pull_secrets,
            "initContainers":[{
                "name":"stage", "image":config.tool_stager_image, "imagePullPolicy":"IfNotPresent",
                "command":[HELPER,"stage"],
                "env":[{"name":"ORCH_POD_UID","valueFrom":{"fieldRef":{"apiVersion":"v1","fieldPath":"metadata.uid"}}}],
                "securityContext":{"runAsUser":0,"runAsGroup":0,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,
                    "capabilities":{"drop":["ALL"]},"seccompProfile":{"type":"RuntimeDefault"}},
                "volumeMounts":[{"name":"tools","mountPath":"/stage"}]
            }],
            "containers":[
                {"name":"workload", "image":plan.image, "imagePullPolicy": if plan.image.contains("@sha256:") { "IfNotPresent" } else { "Always" }, "command":[SUPERVISOR], "workingDir":"/",
                    "env":env, "resources":resources, "securityContext":security,
                    "volumeMounts":mounts},
                {"name":"guard", "image":config.tool_stager_image, "imagePullPolicy":"IfNotPresent", "command":[HELPER,"guard"],
                    "env":[{"name":"ORCH_POD_UID","valueFrom":{"fieldRef":{"apiVersion":"v1","fieldPath":"metadata.uid"}}}],
                    "securityContext":{"runAsUser":0,"allowPrivilegeEscalation":false,
                        "readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"],"add":["KILL","CHOWN"]},
                        "seccompProfile":{"type":"RuntimeDefault"}},
                    "volumeMounts":[{"name":"run","mountPath":RUN_DIR},
                        {"name":"rendezvous","mountPath":RENDEZVOUS_DIR},
                        {"name":"control","mountPath":CONTROL_DIR}]}
            ],
            "volumes":volumes
        }
    });
    serde_json::from_value(pod)
        .map_err(|_| DriverError::Internal("the managed Pod request could not be made".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RuntimeBoundary, RuntimeClass};
    use crate::launch;
    use orch_driver_contract::runtime::{RuntimeKind, unit_labels, unit_name};
    use std::collections::{BTreeMap, BTreeSet};

    fn config() -> KubernetesConfig {
        KubernetesConfig {
            kubeconfig: None,
            namespace: "fixture".into(),
            storage_class: "local-path".into(),
            volume_request: "1Gi".into(),
            runtime_classes: vec![RuntimeClass {
                runtime: "runc".into(),
                class_name: None,
                boundary: RuntimeBoundary::SharedKernel,
            }],
            image_pull_secrets: vec!["registry-credential".into()],
            tool_stager_image: format!("test.invalid/stager@sha256:{}", "a".repeat(64)),
        }
    }

    fn spec() -> RuntimeSpec {
        let id = orch_types::new_id();
        serde_json::from_value(json!({
            "runtime_id": id, "kind":"environment", "name":unit_name(RuntimeKind::Environment, &id),
            "driver":"kubernetes", "spec":{"image":"test.invalid/work:1"}, "resources":{},
            "roles":["environment"], "gateway_url":"wss://gateway.example:9443",
            "enroll_token":"synthetic-enrollment", "ca_cert":"synthetic-authority",
            "tools":[], "volumes":{"workspace":{"name":"workspace","storage":null},
                "home":{"name":"home","storage":null},"state":{"name":"state","storage":null}},
            "mounts":[], "env":{"TOKEN":"synthetic-env-secret"}, "labels":unit_labels("fixture",RuntimeKind::Environment,&id)
        }))
        .unwrap()
    }

    #[test]
    fn the_api_object_uses_claims_and_never_contains_private_launch_files() {
        let spec = spec();
        let config = config();
        let plan = crate::admission::plan(&spec, &config, &BTreeSet::new()).unwrap();
        let launch = launch::prepare(&spec, &plan, &[]).unwrap();
        let intent = orch_types::new_id();
        let pod = request(&spec, &plan, &config, &launch, intent, "linux", "x86_64").unwrap();
        let object = serde_json::to_value(&pod).unwrap();
        let encoded = object.to_string();
        assert!(!encoded.contains("synthetic-enrollment"));
        assert!(!encoded.contains("synthetic-authority"));
        assert!(!encoded.contains("synthetic-env-secret"));
        assert_eq!(object["spec"]["automountServiceAccountToken"], false);
        assert_eq!(object["spec"]["restartPolicy"], "Never");
        assert_eq!(object["spec"]["shareProcessNamespace"], true);
        assert_eq!(
            object["spec"]["nodeSelector"]["kubernetes.io/arch"],
            "amd64"
        );
        assert_eq!(
            object["spec"]["imagePullSecrets"][0]["name"],
            "registry-credential"
        );
        assert_eq!(
            object["spec"]["initContainers"][0]["env"][0]["valueFrom"]["fieldRef"]["fieldPath"],
            "metadata.uid"
        );
        assert_eq!(
            object["spec"]["initContainers"][0]["securityContext"]["runAsUser"],
            0
        );
        assert_eq!(object["spec"]["containers"][0]["command"][0], SUPERVISOR);
        let guard = &object["spec"]["containers"][1];
        let workload = &object["spec"]["containers"][0];
        assert_eq!(
            guard["env"][0]["valueFrom"]["fieldRef"]["fieldPath"],
            "metadata.uid"
        );
        assert!(
            !workload["volumeMounts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|mount| mount["name"] == "control")
        );
        assert!(
            guard["volumeMounts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|mount| mount["name"] == "control")
        );
        assert!(
            workload["volumeMounts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|mount| mount["name"] == "tools" && mount["readOnly"] == true)
        );
        let claims = object["spec"]["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|volume| volume["persistentVolumeClaim"]["claimName"].as_str())
            .collect::<BTreeSet<_>>();
        let expected_claims =
            ["home", "state", "workspace"].map(|name| crate::names::claim(name).unwrap());
        let expected: BTreeSet<_> = expected_claims.iter().map(String::as_str).collect();
        assert_eq!(claims, expected);
        let annotations: BTreeMap<String, String> =
            serde_json::from_value(object["metadata"]["annotations"].clone()).unwrap();
        for (name, value) in &spec.labels {
            assert_eq!(annotations.get(name), Some(value));
        }
        assert_eq!(annotations[INTENT_ANNOTATION], intent.to_string());
        assert_eq!(
            object["metadata"]["labels"][UNIT_SELECTOR],
            spec.runtime_id.simple().to_string()
        );
    }

    #[test]
    fn missing_environment_claim_is_refused_before_a_pod_is_submitted() {
        let spec = spec();
        let config = config();
        let mut plan = crate::admission::plan(&spec, &config, &BTreeSet::new()).unwrap();
        let launch = launch::prepare(&spec, &plan, &[]).unwrap();
        plan.claims.remove(&VolumeRole::Home);
        assert!(matches!(
            request(
                &spec,
                &plan,
                &config,
                &launch,
                orch_types::new_id(),
                "linux",
                "x86_64"
            ),
            Err(DriverError::InvalidSpec(_))
        ));
    }
}
