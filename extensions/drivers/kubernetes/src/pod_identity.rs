//! The native identities of a managed Pod's two long-lived containers.

use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::units::UnitRef;

use crate::ownership::check_pod;
use crate::process_scope::container_id;

const WORKLOAD: &str = "workload";
const GUARD: &str = "guard";
const STAGE: &str = "stage";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StageIdentity(String);

impl StageIdentity {
    pub(crate) fn reference(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContainerIdentities {
    pub workload: String,
    pub guard: String,
}

fn unavailable() -> DriverError {
    DriverError::Unavailable(
        "the original workload and guard containers are not both running".into(),
    )
}

fn stage_unavailable() -> DriverError {
    DriverError::Unavailable("the original staging container cannot be verified".into())
}

/// Pin the running init container, or the same successfully completed container after transfer.
pub(crate) fn read_stage(
    pod: &Pod,
    namespace: &str,
    system: &str,
    unit: &UnitRef,
    allow_completed: bool,
) -> Result<StageIdentity, DriverError> {
    check_pod(&pod.metadata, namespace, system, unit)?;
    if pod.metadata.deletion_timestamp.is_some()
        || pod
            .spec
            .as_ref()
            .and_then(|spec| spec.restart_policy.as_deref())
            != Some("Never")
    {
        return Err(stage_unavailable());
    }
    if !pod
        .status
        .as_ref()
        .and_then(|status| status.phase.as_deref())
        .is_some_and(|phase| ["Pending", "Running"].contains(&phase))
    {
        return Err(stage_unavailable());
    }
    let statuses = pod
        .status
        .as_ref()
        .and_then(|status| status.init_container_statuses.as_ref())
        .ok_or_else(stage_unavailable)?;
    let mut matching = statuses.iter().filter(|status| status.name == STAGE);
    let status = matching.next().ok_or_else(stage_unavailable)?;
    if statuses.len() != 1 || matching.next().is_some() || status.restart_count != 0 {
        return Err(stage_unavailable());
    }
    let state = status.state.as_ref().ok_or_else(stage_unavailable)?;
    let running = state.running.is_some() && state.terminated.is_none() && state.waiting.is_none();
    let completed = allow_completed
        && state.running.is_none()
        && state.waiting.is_none()
        && state
            .terminated
            .as_ref()
            .is_some_and(|exit| exit.exit_code == 0);
    if !running && !completed {
        return Err(stage_unavailable());
    }
    let id = status
        .container_id
        .as_ref()
        .filter(|id| !id.is_empty())
        .ok_or_else(stage_unavailable)?;
    container_id(id)?;
    Ok(StageIdentity(id.clone()))
}

/// A reset restart count does not make a replacement init container the original one.
pub(crate) fn verify_stage(
    pod: &Pod,
    namespace: &str,
    system: &str,
    unit: &UnitRef,
    expected: &StageIdentity,
    allow_completed: bool,
) -> Result<(), DriverError> {
    if &read_stage(pod, namespace, system, unit, allow_completed)? != expected {
        return Err(DriverError::Unavailable(
            "the staging container was replaced".into(),
        ));
    }
    Ok(())
}

fn running_identity(statuses: &[ContainerStatus], name: &str) -> Result<String, DriverError> {
    let mut matching = statuses.iter().filter(|status| status.name == name);
    let status = matching.next().ok_or_else(unavailable)?;
    if matching.next().is_some()
        || status.restart_count != 0
        || status
            .state
            .as_ref()
            .and_then(|state| state.running.as_ref())
            .is_none()
    {
        return Err(unavailable());
    }
    let id = status
        .container_id
        .as_ref()
        .filter(|id| !id.is_empty())
        .ok_or_else(unavailable)?;
    // The scope and pidfd implementation currently supports only complete containerd identities.
    container_id(id)?;
    Ok(id.clone())
}

/// Reads the API's reported container identities; live status still requires the authenticated guard.
pub(crate) fn read(
    pod: &Pod,
    namespace: &str,
    system: &str,
    unit: &UnitRef,
) -> Result<ContainerIdentities, DriverError> {
    check_pod(&pod.metadata, namespace, system, unit)?;
    if pod.metadata.deletion_timestamp.is_some()
        || pod
            .spec
            .as_ref()
            .and_then(|spec| spec.restart_policy.as_deref())
            != Some("Never")
        || pod
            .spec
            .as_ref()
            .and_then(|spec| spec.share_process_namespace)
            != Some(true)
        || pod
            .status
            .as_ref()
            .and_then(|status| status.phase.as_deref())
            != Some("Running")
    {
        return Err(unavailable());
    }
    let statuses = pod
        .status
        .as_ref()
        .and_then(|status| status.container_statuses.as_ref())
        .ok_or_else(unavailable)?;
    if statuses.len() != 2 {
        return Err(unavailable());
    }
    let workload = running_identity(statuses, WORKLOAD)?;
    let guard = running_identity(statuses, GUARD)?;
    if workload == guard {
        return Err(unavailable());
    }
    Ok(ContainerIdentities { workload, guard })
}

/// Restart counts can reset after node recovery, so compare the full recorded IDs too.
pub(crate) fn verify(
    pod: &Pod,
    namespace: &str,
    system: &str,
    unit: &UnitRef,
    expected: &ContainerIdentities,
) -> Result<(), DriverError> {
    if &read(pod, namespace, system, unit)? != expected {
        return Err(DriverError::NotFound(
            "the workload or guard container was replaced".into(),
        ));
    }
    Ok(())
}

/// How the workload container of a unit ended, as the cluster recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkloadEnd {
    pub exit_code: i32,
    /// The cluster's one-word reason (`Error`, `OOMKilled`), reduced to letters and digits; empty when it gave none.
    pub reason: String,
}

impl WorkloadEnd {
    /// What a unit's state says of it.
    pub(crate) fn sentence(&self) -> String {
        if self.reason.is_empty() {
            "the workload container ended".to_owned()
        } else {
            format!("the workload container ended: {}", self.reason)
        }
    }
}

/// The end of the workload container that was pinned for this unit, when it has ended. The Pod must be the unit's own
/// (its name, namespace, annotations and UID), and the container the pinned one that has never been restarted: a
/// container of another identity that ended is no end of this unit's, and the answer for it, as for one that still
/// runs, is `None`. Everything the unit ran lived in that container, so an end is final: a Pod's container is not
/// started again here (its restart policy is `Never`).
pub(crate) fn workload_end(
    pod: &Pod,
    namespace: &str,
    system: &str,
    unit: &UnitRef,
    expected: &ContainerIdentities,
) -> Option<WorkloadEnd> {
    check_pod(&pod.metadata, namespace, system, unit).ok()?;
    if pod
        .spec
        .as_ref()
        .and_then(|spec| spec.restart_policy.as_deref())
        != Some("Never")
    {
        return None;
    }
    let statuses = pod.status.as_ref()?.container_statuses.as_ref()?;
    let mut matching = statuses.iter().filter(|status| status.name == WORKLOAD);
    let status = matching.next()?;
    if matching.next().is_some()
        || status.restart_count != 0
        || status.container_id.as_deref() != Some(expected.workload.as_str())
    {
        return None;
    }
    let ended = status.state.as_ref()?.terminated.as_ref()?;
    Some(WorkloadEnd {
        exit_code: ended.exit_code,
        reason: ended
            .reason
            .as_deref()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(40)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::runtime::{RuntimeKind, unit_labels};
    use serde_json::json;

    fn fixture() -> (Pod, UnitRef) {
        let runtime = orch_types::new_id();
        let unit = UnitRef {
            reference: "pod-uid".into(),
            name: "work".into(),
            runtime_id: runtime,
            kind: RuntimeKind::Environment,
        };
        let labels = unit_labels("system", RuntimeKind::Environment, &runtime);
        let pod = serde_json::from_value(json!({"metadata":{"name":"work","namespace":"ns","uid":"pod-uid","annotations":labels},
            "spec":{"restartPolicy":"Never","shareProcessNamespace":true,"containers":[{"name":"workload","image":"example"},{"name":"guard","image":"guard"}]},
            "status":{"phase":"Running","containerStatuses":[
                {"name":"workload","ready":true,"restartCount":0,"containerID":format!("containerd://{}", "a".repeat(64)),"state":{"running":{}}},
                {"name":"guard","ready":true,"restartCount":0,"containerID":format!("containerd://{}", "b".repeat(64)),"state":{"running":{}}}
            ]}})).unwrap();
        (pod, unit)
    }

    #[test]
    fn only_the_original_running_nonrestarted_pair_is_identified() {
        let (pod, unit) = fixture();
        let pair = read(&pod, "ns", "system", &unit).unwrap();
        assert_ne!(pair.workload, pair.guard);
        assert!(verify(&pod, "ns", "system", &unit, &pair).is_ok());
        let mut replaced = pod.clone();
        replaced.metadata.uid = Some("other-uid".into());
        assert!(read(&replaced, "ns", "system", &unit).is_err());
        let mut restarted = pod.clone();
        restarted
            .status
            .as_mut()
            .unwrap()
            .container_statuses
            .as_mut()
            .unwrap()[0]
            .restart_count = 1;
        assert!(read(&restarted, "ns", "system", &unit).is_err());
        let mut reset_count = pod.clone();
        reset_count
            .status
            .as_mut()
            .unwrap()
            .container_statuses
            .as_mut()
            .unwrap()[0]
            .container_id = Some(format!("containerd://{}", "c".repeat(64)));
        assert!(verify(&reset_count, "ns", "system", &unit, &pair).is_err());
        let mut not_running = pod;
        not_running
            .status
            .as_mut()
            .unwrap()
            .container_statuses
            .as_mut()
            .unwrap()[1]
            .state = None;
        assert!(read(&not_running, "ns", "system", &unit).is_err());
    }

    #[test]
    fn only_the_pinned_workload_container_that_ended_is_an_end_of_the_unit() {
        let (pod, unit) = fixture();
        let pair = read(&pod, "ns", "system", &unit).unwrap();
        assert_eq!(workload_end(&pod, "ns", "system", &unit, &pair), None);
        let mut raw = serde_json::to_value(pod).unwrap();
        raw["status"]["containerStatuses"][0]["state"] =
            json!({"terminated":{"exitCode":137,"reason":"OOMKilled"}});
        let ended: Pod = serde_json::from_value(raw.clone()).unwrap();
        // Its guard is not asked about it any more, and its end is known.
        assert!(read(&ended, "ns", "system", &unit).is_err());
        let end = workload_end(&ended, "ns", "system", &unit, &pair).unwrap();
        assert_eq!(
            end,
            WorkloadEnd {
                exit_code: 137,
                reason: "OOMKilled".into()
            }
        );
        assert_eq!(end.sentence(), "the workload container ended: OOMKilled");
        // The cluster's words are reduced to letters and digits, and a reason it did not give is not made up.
        raw["status"]["containerStatuses"][0]["state"] =
            json!({"terminated":{"exitCode":1,"reason":"bad reason: <b>"}});
        let odd: Pod = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(
            workload_end(&odd, "ns", "system", &unit, &pair)
                .unwrap()
                .reason,
            "badreasonb"
        );
        raw["status"]["containerStatuses"][0]["state"] = json!({"terminated":{"exitCode":1}});
        let silent: Pod = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(
            workload_end(&silent, "ns", "system", &unit, &pair)
                .unwrap()
                .sentence(),
            "the workload container ended"
        );
        // A container of another identity that ended, a restarted one, a Pod that may restart its containers, and
        // another Pod are no end of this unit's.
        let mut other = raw.clone();
        other["status"]["containerStatuses"][0]["containerID"] =
            json!(format!("containerd://{}", "c".repeat(64)));
        let other: Pod = serde_json::from_value(other).unwrap();
        assert_eq!(workload_end(&other, "ns", "system", &unit, &pair), None);
        let mut restarted = raw.clone();
        restarted["status"]["containerStatuses"][0]["restartCount"] = json!(1);
        let restarted: Pod = serde_json::from_value(restarted).unwrap();
        assert_eq!(workload_end(&restarted, "ns", "system", &unit, &pair), None);
        let mut always = raw.clone();
        always["spec"]["restartPolicy"] = json!("Always");
        let always: Pod = serde_json::from_value(always).unwrap();
        assert_eq!(workload_end(&always, "ns", "system", &unit, &pair), None);
        let mut replaced = raw;
        replaced["metadata"]["uid"] = json!("other-uid");
        let replaced: Pod = serde_json::from_value(replaced).unwrap();
        assert_eq!(workload_end(&replaced, "ns", "system", &unit, &pair), None);
    }

    #[test]
    fn staging_requires_the_same_init_container_before_and_after_completion() {
        let (pod, unit) = fixture();
        let mut raw = serde_json::to_value(pod).unwrap();
        let first = format!("containerd://{}", "c".repeat(64));
        raw["status"]["initContainerStatuses"] = json!([{"name":"stage","ready":false,"restartCount":0,
            "containerID":first,"state":{"running":{}}}]);
        let running: Pod = serde_json::from_value(raw.clone()).unwrap();
        let identity = read_stage(&running, "ns", "system", &unit, false).unwrap();
        let mut stale = raw.clone();
        stale["status"]["phase"] = json!("Failed");
        let stale: Pod = serde_json::from_value(stale).unwrap();
        assert!(read_stage(&stale, "ns", "system", &unit, false).is_err());
        raw["status"]["initContainerStatuses"][0]["state"] = json!({"terminated":{"exitCode":0}});
        let completed: Pod = serde_json::from_value(raw.clone()).unwrap();
        assert!(read_stage(&completed, "ns", "system", &unit, false).is_err());
        assert!(verify_stage(&completed, "ns", "system", &unit, &identity, true).is_ok());
        raw["status"]["initContainerStatuses"][0]["containerID"] =
            json!(format!("containerd://{}", "d".repeat(64)));
        let replaced: Pod = serde_json::from_value(raw.clone()).unwrap();
        assert!(verify_stage(&replaced, "ns", "system", &unit, &identity, true).is_err());
        raw["status"]["initContainerStatuses"][0]["state"]["terminated"]["exitCode"] = json!(1);
        let failed: Pod = serde_json::from_value(raw).unwrap();
        assert!(read_stage(&failed, "ns", "system", &unit, true).is_err());
    }
}
