//! Incarnation and consumer checks for cluster-owned objects.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::{LABEL_KIND, LABEL_RUNTIME_ID, LABEL_SYSTEM};
use orch_driver_contract::units::UnitRef;

/// The native incarnation and protected labels recorded when an owned claim was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimReceipt {
    pub name: String,
    pub native_name: String,
    pub uid: String,
    pub labels: BTreeMap<String, String>,
}

fn mismatch() -> DriverError {
    DriverError::NotFound("the owned object is absent or has been replaced".into())
}

/// Refuses a foreign or later Pod even if its logical name was reused.
pub(crate) fn check_pod(
    meta: &ObjectMeta,
    namespace: &str,
    system: &str,
    unit: &UnitRef,
) -> Result<(), DriverError> {
    if meta.namespace.as_deref() != Some(namespace)
        || meta.name.as_deref() != Some(unit.name.as_str())
        || unit.reference.is_empty()
        || meta.uid.as_deref() != Some(unit.reference.as_str())
    {
        return Err(mismatch());
    }
    let Some(annotations) = &meta.annotations else {
        return Err(mismatch());
    };
    if annotations.get(LABEL_SYSTEM).map(String::as_str) != Some(system)
        || annotations.get(LABEL_RUNTIME_ID).map(String::as_str)
            != Some(unit.runtime_id.to_string().as_str())
        || annotations.get(LABEL_KIND).map(String::as_str) != Some(unit.kind.as_str())
    {
        return Err(mismatch());
    }
    Ok(())
}

/// Rechecks a claim's exact UID and protected labels before constructing a native delete precondition.
pub(crate) fn check_claim(
    meta: &ObjectMeta,
    namespace: &str,
    receipt: &ClaimReceipt,
) -> Result<(), DriverError> {
    if meta.namespace.as_deref() != Some(namespace)
        || meta.name.as_deref() != Some(receipt.native_name.as_str())
        || receipt.uid.is_empty()
        || meta.uid.as_deref() != Some(receipt.uid.as_str())
    {
        return Err(mismatch());
    }
    let Some(annotations) = &meta.annotations else {
        return Err(mismatch());
    };
    if annotations
        .get(crate::names::LOGICAL_NAME_ANNOTATION)
        .map(String::as_str)
        != Some(receipt.name.as_str())
        || receipt
            .labels
            .get(LABEL_SYSTEM)
            .is_none_or(String::is_empty)
        || receipt
            .labels
            .iter()
            .any(|(key, value)| annotations.get(key) != Some(value))
    {
        return Err(mismatch());
    }
    Ok(())
}

/// Every extant Pod is a consumer, including a stopped or foreign Pod.
pub(crate) fn claim_is_used(name: &str, pods: &[Pod]) -> bool {
    pods.iter().any(|pod| {
        pod.spec
            .as_ref()
            .and_then(|spec| spec.volumes.as_ref())
            .is_some_and(|volumes| {
                volumes.iter().any(|volume| {
                    volume
                        .persistent_volume_claim
                        .as_ref()
                        .is_some_and(|claim| claim.claim_name == name)
                })
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::runtime::RuntimeKind;
    use serde_json::json;

    #[test]
    fn a_recreated_pod_with_the_same_name_cannot_be_selected_by_an_old_reference() {
        let runtime = orch_types::new_id();
        let unit = UnitRef {
            reference: "old-uid".into(),
            name: "work".into(),
            runtime_id: runtime,
            kind: RuntimeKind::Environment,
        };
        let mut meta = ObjectMeta {
            name: Some("work".into()),
            namespace: Some("orch".into()),
            uid: Some("old-uid".into()),
            annotations: Some(BTreeMap::from([
                (LABEL_SYSTEM.into(), "system".into()),
                (LABEL_RUNTIME_ID.into(), runtime.to_string()),
                (LABEL_KIND.into(), "environment".into()),
            ])),
            ..Default::default()
        };
        assert!(check_pod(&meta, "orch", "system", &unit).is_ok());
        meta.uid = Some("new-uid".into());
        assert!(check_pod(&meta, "orch", "system", &unit).is_err());
        meta.uid = Some("old-uid".into());
        assert!(check_pod(&meta, "orch", "another-system", &unit).is_err());
    }

    #[test]
    fn a_replaced_claim_and_a_claim_used_by_any_retained_pod_are_not_removable() {
        let receipt = ClaimReceipt {
            name: "Work_A".into(),
            native_name: crate::names::claim("Work_A").unwrap(),
            uid: "old-uid".into(),
            labels: BTreeMap::from([(LABEL_SYSTEM.into(), "system".into())]),
        };
        let mut meta = ObjectMeta {
            name: Some(receipt.native_name.clone()),
            namespace: Some("orch".into()),
            uid: Some("old-uid".into()),
            annotations: Some(
                receipt
                    .labels
                    .clone()
                    .into_iter()
                    .chain([(
                        crate::names::LOGICAL_NAME_ANNOTATION.into(),
                        receipt.name.clone(),
                    )])
                    .collect(),
            ),
            ..Default::default()
        };
        assert!(check_claim(&meta, "orch", &receipt).is_ok());
        meta.uid = Some("new-uid".into());
        assert!(check_claim(&meta, "orch", &receipt).is_err());
        let pod: Pod = serde_json::from_value(json!({"metadata":{"name":"foreign"}, "spec":{"containers":[{"name":"unit","image":"example"}],"volumes":[{"name":"work","persistentVolumeClaim":{"claimName":"work"}}]}, "status":{"phase":"Succeeded"}})).unwrap();
        assert!(claim_is_used("work", &[pod]));
        assert!(!claim_is_used("another", &[]));
    }
}
