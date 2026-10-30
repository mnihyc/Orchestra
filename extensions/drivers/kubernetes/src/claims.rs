//! Persistent claim requests with an owned creation intent.

use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::Api;
use kube::api::PostParams;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::LABEL_SYSTEM;
use orch_driver_contract::volumes::VolumeRequest;
use orch_types::Id;
use serde_json::json;
use std::time::Duration;

use crate::config::KubernetesConfig;
use crate::names::{self, LOGICAL_NAME_ANNOTATION};
use crate::ownership::ClaimReceipt;

const HTTP_CONFLICT: u16 = 409;
const HTTP_FORBIDDEN: u16 = 403;

/// A persisted creation intent distinguishes our retry from a foreign same-named claim.
pub(crate) const INTENT_ANNOTATION: &str = "orch.claim-intent";

/// Constructs an owned PVC without tying its lifetime to a Pod.
pub(crate) fn claim_request(
    config: &KubernetesConfig,
    volume: &VolumeRequest,
    intent: Id,
) -> Result<PersistentVolumeClaim, DriverError> {
    if volume.name.trim().is_empty() {
        return Err(DriverError::InvalidSpec("the volume name is empty".into()));
    }
    if volume
        .labels
        .get(LABEL_SYSTEM)
        .is_none_or(|system| system.trim().is_empty())
    {
        return Err(DriverError::InvalidSpec(
            "a volume must name the system that owns it".into(),
        ));
    }
    if volume.labels.contains_key(INTENT_ANNOTATION)
        || volume.labels.contains_key(LOGICAL_NAME_ANNOTATION)
    {
        return Err(DriverError::InvalidSpec(
            "the creation intent is reserved for the driver".into(),
        ));
    }
    if volume
        .storage
        .as_deref()
        .is_some_and(|name| name != "default" && name != config.storage_class)
    {
        return Err(DriverError::Unsupported(
            "the requested storage class is not configured on this cluster".into(),
        ));
    }
    let native_name = names::claim(&volume.name)?;
    let mut annotations = volume.labels.clone();
    annotations.insert(INTENT_ANNOTATION.into(), intent.to_string());
    annotations.insert(LOGICAL_NAME_ANNOTATION.into(), volume.name.clone());
    // ReadWriteOnce works with local-path storage; the scheduler chooses a node at first use.
    // A retained Pod remains a consumer, so the driver still checks all other consumers.
    serde_json::from_value(json!({
        "apiVersion": "v1", "kind": "PersistentVolumeClaim",
        "metadata": {"name": native_name, "namespace": config.namespace, "annotations": annotations},
        "spec": {"accessModes": ["ReadWriteOnce"], "storageClassName": config.storage_class,
            "resources": {"requests": {"storage": config.volume_request}}}
    }))
    .map_err(|_| DriverError::Internal("the persistent claim request could not be made".into()))
}

/// Creates a claim or verifies the exact UID already bound to its persisted intent.
pub(crate) async fn ensure_claim(
    claims: &Api<PersistentVolumeClaim>,
    config: &KubernetesConfig,
    volume: &VolumeRequest,
    intent: Id,
    expected_uid: Option<&str>,
    request_timeout: Duration,
) -> Result<ClaimReceipt, DriverError> {
    let requested = claim_request(config, volume, intent)?;
    let native_name = names::claim(&volume.name)?;
    let actual = if expected_uid.is_some() {
        tokio::time::timeout(request_timeout, claims.get(&native_name))
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| DriverError::Unavailable("the recorded claim could not be read".into()))?
    } else {
        match tokio::time::timeout(
            request_timeout,
            claims.create(&PostParams::default(), &requested),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        {
            Ok(claim) => claim,
            Err(kube::Error::Api(status)) if status.code == HTTP_CONFLICT => {
                return Err(DriverError::Unsupported(
                    "the existing claim has no recorded incarnation; review it before retrying"
                        .into(),
                ));
            }
            Err(kube::Error::Api(status)) if status.code == HTTP_FORBIDDEN => {
                return Err(DriverError::Unavailable(
                    "the Kubernetes identity cannot create persistent claims".into(),
                ));
            }
            Err(_) => {
                return Err(DriverError::Unavailable(
                    "the persistent claim could not be created".into(),
                ));
            }
        }
    };
    let annotations = actual.metadata.annotations.as_ref().ok_or_else(|| {
        DriverError::InvalidSpec("a same-named claim is not owned by this request".into())
    })?;
    if actual.metadata.name.as_deref() != Some(native_name.as_str())
        || actual.metadata.namespace.as_deref() != Some(config.namespace.as_str())
        || actual.metadata.deletion_timestamp.is_some()
        || actual
            .metadata
            .owner_references
            .as_ref()
            .is_some_and(|owners| !owners.is_empty())
        || annotations.get(INTENT_ANNOTATION) != Some(&intent.to_string())
        || annotations.get(LOGICAL_NAME_ANNOTATION) != Some(&volume.name)
        || volume
            .labels
            .iter()
            .any(|(key, value)| annotations.get(key) != Some(value))
        || actual
            .spec
            .as_ref()
            .and_then(|spec| spec.storage_class_name.as_deref())
            != Some(config.storage_class.as_str())
        || actual
            .spec
            .as_ref()
            .and_then(|spec| spec.access_modes.as_ref())
            .is_none_or(|modes| modes.as_slice() != ["ReadWriteOnce"])
    {
        return Err(DriverError::InvalidSpec(
            "a same-named claim does not match this creation intent".into(),
        ));
    }
    let requested =
        k8s_openapi::apimachinery::pkg::api::resource::Quantity(config.volume_request.clone());
    let actual_size = actual
        .spec
        .as_ref()
        .and_then(|spec| spec.resources.as_ref())
        .and_then(|resources| resources.requests.as_ref())
        .and_then(|requests| requests.get("storage"))
        .ok_or_else(|| {
            DriverError::InvalidSpec("the recorded claim has no storage request".into())
        })?;
    if !crate::capacity::at_least(actual_size, &requested)? {
        return Err(DriverError::InvalidSpec(
            "the recorded claim is smaller than the configured request".into(),
        ));
    }
    let uid = actual
        .metadata
        .uid
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| {
            DriverError::Unavailable("the Kubernetes API did not identify the claim".into())
        })?;
    if expected_uid.is_some_and(|expected| expected != uid) {
        return Err(DriverError::InvalidSpec(
            "the recorded claim was replaced by another incarnation".into(),
        ));
    }
    Ok(ClaimReceipt {
        name: volume.name.clone(),
        native_name,
        uid,
        labels: volume.labels.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RuntimeBoundary, RuntimeClass};
    use std::collections::BTreeMap;

    fn configuration() -> KubernetesConfig {
        KubernetesConfig {
            kubeconfig: None,
            namespace: "orch".into(),
            storage_class: "local-path".into(),
            volume_request: "10Gi".into(),
            runtime_classes: vec![RuntimeClass {
                runtime: "runc".into(),
                class_name: None,
                boundary: RuntimeBoundary::SharedKernel,
            }],
            image_pull_secrets: Vec::new(),
            tool_stager_image: format!("registry.example/stager@sha256:{}", "a".repeat(64)),
        }
    }

    #[test]
    fn workspace_claims_survive_removing_the_unit_and_keep_their_creation_identity() {
        let intent = orch_types::new_id();
        let volume = VolumeRequest {
            name: "workspace".into(),
            storage: None,
            labels: BTreeMap::from([(LABEL_SYSTEM.into(), "system".into())]),
        };
        let claim = claim_request(&configuration(), &volume, intent).unwrap();
        assert!(claim.metadata.owner_references.is_none());
        let expected_name = names::claim(&volume.name).unwrap();
        assert_eq!(claim.metadata.name.as_deref(), Some(expected_name.as_str()));
        let annotations = claim.metadata.annotations.unwrap();
        assert_eq!(annotations[INTENT_ANNOTATION], intent.to_string());
        assert_eq!(annotations[LOGICAL_NAME_ANNOTATION], volume.name);
        let spec = claim.spec.unwrap();
        assert_eq!(spec.storage_class_name.as_deref(), Some("local-path"));
        assert_eq!(spec.access_modes.unwrap(), ["ReadWriteOnce"]);
        assert_eq!(
            spec.resources.unwrap().requests.unwrap()["storage"].0,
            "10Gi"
        );
    }

    #[test]
    fn a_foreign_storage_or_a_missing_system_is_not_created() {
        let mut volume = VolumeRequest {
            name: "workspace".into(),
            storage: Some("another-class".into()),
            labels: BTreeMap::from([(LABEL_SYSTEM.into(), "system".into())]),
        };
        assert!(claim_request(&configuration(), &volume, orch_types::new_id()).is_err());
        volume.storage = None;
        volume.labels.clear();
        assert!(claim_request(&configuration(), &volume, orch_types::new_id()).is_err());
    }
}
