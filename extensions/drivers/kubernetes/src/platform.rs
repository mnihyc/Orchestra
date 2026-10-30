//! Select workers compatible with the node's verified Linux tool artifacts.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{Node, Pod};
use orch_driver_contract::driver::DriverError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerIdentity {
    name: String,
    uid: String,
}

impl WorkerIdentity {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn uid(&self) -> &str {
        &self.uid
    }
}

/// These Pods carry no toleration for an administrative or resource-pressure scheduling taint.
pub(crate) fn scheduling_blocked(node: &Node) -> bool {
    node.metadata.deletion_timestamp.is_some()
        || node.spec.as_ref().is_some_and(|spec| {
            spec.unschedulable == Some(true)
                || spec.taints.as_ref().is_some_and(|taints| {
                    taints
                        .iter()
                        .any(|taint| taint.effect != "PreferNoSchedule")
                })
        })
        || node
            .status
            .as_ref()
            .and_then(|status| status.conditions.as_ref())
            .is_some_and(|conditions| {
                conditions.iter().any(|condition| {
                    ["DiskPressure", "MemoryPressure", "PIDPressure"]
                        .contains(&condition.type_.as_str())
                        && condition.status != "False"
                })
            })
}

/// Converts the node artifact platform to Kubernetes' standard worker labels.
pub(crate) fn worker_selector(
    os: &str,
    arch: &str,
) -> Result<BTreeMap<String, String>, DriverError> {
    if os != "linux" {
        return Err(DriverError::Unsupported(
            "the Kubernetes driver needs Linux connector and tool builds".into(),
        ));
    }
    let arch = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => {
            return Err(DriverError::Unsupported(
                "the node's tool architecture is not supported by this driver".into(),
            ));
        }
    };
    Ok(BTreeMap::from([
        ("kubernetes.io/os".into(), "linux".into()),
        ("kubernetes.io/arch".into(), arch.into()),
    ]))
}

/// Verify the assigned worker's identity and platform; Ready is not proof of process termination.
pub(crate) fn worker_for_pod(
    pod: &Pod,
    node: &Node,
    selector: &BTreeMap<String, String>,
    require_schedulable: bool,
) -> Result<WorkerIdentity, DriverError> {
    let unavailable =
        || DriverError::Unavailable("the assigned cluster worker cannot be verified".into());
    if selector.get("kubernetes.io/os").map(String::as_str) != Some("linux")
        || !selector
            .get("kubernetes.io/arch")
            .is_some_and(|arch| ["amd64", "arm64"].contains(&arch.as_str()))
    {
        return Err(unavailable());
    }
    let name = pod
        .spec
        .as_ref()
        .and_then(|spec| spec.node_name.as_deref())
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(unavailable)?;
    let uid = node
        .metadata
        .uid
        .as_deref()
        .filter(|uid| !uid.trim().is_empty())
        .ok_or_else(unavailable)?;
    if node.metadata.name.as_deref() != Some(name)
        || node.metadata.deletion_timestamp.is_some()
        || require_schedulable && scheduling_blocked(node)
        || !selector.iter().all(|(key, value)| {
            node.metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get(key))
                == Some(value)
        })
    {
        return Err(unavailable());
    }
    let conditions = node
        .status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .ok_or_else(unavailable)?;
    let mut ready = conditions
        .iter()
        .filter(|condition| condition.type_ == "Ready");
    if ready
        .next()
        .is_none_or(|condition| condition.status != "True")
        || ready.next().is_some()
    {
        return Err(unavailable());
    }
    Ok(WorkerIdentity {
        name: name.to_owned(),
        uid: uid.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_are_scheduled_only_on_workers_that_can_run_the_nodes_tools() {
        let x86 = worker_selector("linux", "x86_64").unwrap();
        assert_eq!(x86["kubernetes.io/os"], "linux");
        assert_eq!(x86["kubernetes.io/arch"], "amd64");
        assert_eq!(
            worker_selector("linux", "aarch64").unwrap()["kubernetes.io/arch"],
            "arm64"
        );
        assert!(worker_selector("windows", "x86_64").is_err());
        assert!(worker_selector("linux", "unknown").is_err());
    }

    #[test]
    fn a_worker_must_match_the_pod_and_have_an_unambiguous_ready_identity() {
        let selector = worker_selector("linux", "x86_64").unwrap();
        let pod: Pod = serde_json::from_value(serde_json::json!({"metadata":{"name":"fixture"},
            "spec":{"nodeName":"worker-one", "containers":[{"name":"workload","image":"example"}]}})).unwrap();
        let mut node: Node = serde_json::from_value(
            serde_json::json!({"metadata":{"name":"worker-one","uid":"worker-uid",
            "labels":{"kubernetes.io/os":"linux","kubernetes.io/arch":"amd64"}},
            "status":{"conditions":[{"type":"Ready","status":"True"}]}}),
        )
        .unwrap();
        assert_eq!(
            worker_for_pod(&pod, &node, &selector, true).unwrap().uid,
            "worker-uid"
        );
        assert!(worker_for_pod(&pod, &node, &BTreeMap::new(), true).is_err());
        node.spec =
            Some(serde_json::from_value(serde_json::json!({"unschedulable":true})).unwrap());
        assert!(worker_for_pod(&pod, &node, &selector, true).is_err());
        assert!(worker_for_pod(&pod, &node, &selector, false).is_ok());
        node.metadata.uid = None;
        assert!(worker_for_pod(&pod, &node, &selector, false).is_err());
        node.metadata.uid = Some("worker-uid".into());
        node.metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("kubernetes.io/arch".into(), "arm64".into());
        assert!(worker_for_pod(&pod, &node, &selector, false).is_err());
        node.metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("kubernetes.io/arch".into(), "amd64".into());
        let conditions = node.status.as_mut().unwrap().conditions.as_mut().unwrap();
        conditions.push(conditions[0].clone());
        assert!(worker_for_pod(&pod, &node, &selector, false).is_err());
    }
}
