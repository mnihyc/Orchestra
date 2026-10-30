//! Create and identify a Pod without starting its connector.

use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ApiResource, DynamicObject, PostParams};
use kube::{Api, Client};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::{LABEL_SYSTEM, RuntimeKind, RuntimeSpec};
use orch_driver_contract::units::UnitRef;
use orch_types::Id;
use tokio::time::Instant;

use crate::admission::UnitPlan;
use crate::config::KubernetesConfig;
use crate::launch::LaunchPlan;
use crate::locks::KeyedLocks;
use crate::ownership::check_pod;
use crate::pod;
use crate::pod::{INTENT_ANNOTATION, UNIT_SELECTOR};
use crate::receipts::ReceiptStore;
use crate::verify_admission;

/// A matching intent alone is not authority to adopt an existing conflict after a lost response.
pub(crate) fn verify_created(
    meta: &ObjectMeta,
    namespace: &str,
    system: &str,
    unit: &UnitRef,
    intent: Id,
) -> Result<(), DriverError> {
    check_pod(meta, namespace, system, unit)?;
    if meta.deletion_timestamp.is_some()
        || meta
            .owner_references
            .as_ref()
            .is_some_and(|owners| !owners.is_empty())
        || meta
            .finalizers
            .as_ref()
            .is_some_and(|finalizers| !finalizers.is_empty())
        || meta.labels.as_ref().is_none_or(|labels| {
            labels.len() != 1
                || labels.get(UNIT_SELECTOR) != Some(&unit.runtime_id.simple().to_string())
        })
        || meta
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(INTENT_ANNOTATION))
            != Some(&intent.to_string())
    {
        return Err(DriverError::InvalidSpec(
            "the created Pod does not match its recorded intent".into(),
        ));
    }
    Ok(())
}

/// Kubernetes conflict means the name exists, not that it belongs to this recorded intent.
const HTTP_CONFLICT: u16 = 409;

/// Creates a Pod and durably records its UID; it does not start the connector.
pub(crate) struct UnitService {
    pods: Api<DynamicObject>,
    config: KubernetesConfig,
    journal: Arc<ReceiptStore>,
    locks: KeyedLocks,
    request_timeout: Duration,
}

impl UnitService {
    pub(crate) fn new(
        client: Client,
        config: KubernetesConfig,
        journal: Arc<ReceiptStore>,
        request_timeout: Duration,
    ) -> Result<Self, DriverError> {
        if request_timeout.is_zero() {
            return Err(DriverError::InvalidSpec(
                "the Kubernetes request timeout must be positive".into(),
            ));
        }
        let resource = ApiResource::erase::<Pod>(&());
        let pods = Api::namespaced_with(client, &config.namespace, &resource);
        Ok(Self {
            pods,
            config,
            journal,
            locks: KeyedLocks::default(),
            request_timeout,
        })
    }

    async fn quarantine(&self, runtime: Id, deadline: Instant) -> Result<(), DriverError> {
        let journal = Arc::clone(&self.journal);
        tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || journal.quarantine_unit(runtime)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Unavailable("the unit could not be quarantined".into()))?
    }

    /// A lost create response is ambiguous: a same-name conflict is refused, never adopted by its annotation alone.
    pub(crate) async fn create(
        &self,
        spec: &RuntimeSpec,
        plan: &UnitPlan,
        launch: &LaunchPlan,
        os: &str,
        arch: &str,
        deadline: Instant,
    ) -> Result<UnitRef, DriverError> {
        if spec.kind != RuntimeKind::Environment {
            return Err(DriverError::Unsupported(
                "Kubernetes currently creates environment units only".into(),
            ));
        }
        let _held = self.locks.hold(&spec.name, deadline).await?;
        let system = spec
            .labels
            .get(LABEL_SYSTEM)
            .filter(|system| !system.is_empty())
            .ok_or_else(|| DriverError::InvalidSpec("the unit owner is missing".into()))?;
        let journal = Arc::clone(&self.journal);
        let runtime = spec.runtime_id;
        let kind = spec.kind;
        let name = spec.name.clone();
        let labels = spec.labels.clone();
        let intent = tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || journal.begin_unit(runtime, kind, &name, &labels)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("the unit creation intent could not be recorded".into())
        })??;
        let creation_intent = intent.intent;
        let planned = pod::request(spec, plan, &self.config, launch, creation_intent, os, arch)?;
        let expected = serde_json::to_value(planned)
            .map_err(|_| DriverError::Internal("the managed Pod could not be written".into()))?;
        let request: DynamicObject = serde_json::from_value(expected.clone())
            .map_err(|_| DriverError::Internal("the managed Pod could not be written".into()))?;
        let actual = if let Some(uid) = &intent.pod_uid {
            let found = tokio::time::timeout_at(
                deadline,
                tokio::time::timeout(self.request_timeout, self.pods.get_opt(&spec.name)),
            )
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| DriverError::Unavailable("the recorded Pod could not be read".into()))?
            .ok_or_else(|| DriverError::NotFound("the recorded Pod is absent".into()))?;
            if found.metadata.uid.as_deref() != Some(uid.as_str()) {
                self.quarantine(runtime, deadline).await?;
                return Err(DriverError::NotFound(
                    "the recorded Pod was replaced".into(),
                ));
            }
            found
        } else {
            match tokio::time::timeout_at(
                deadline,
                tokio::time::timeout(
                    self.request_timeout,
                    self.pods.create(&PostParams::default(), &request),
                ),
            )
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| DriverError::Timeout)?
            {
                Ok(pod) => pod,
                Err(kube::Error::Api(status)) if status.code == HTTP_CONFLICT => {
                    return Err(DriverError::Unsupported(
                        "the existing Pod has no recorded incarnation; review it before retrying"
                            .into(),
                    ));
                }
                Err(_) => {
                    return Err(DriverError::Unavailable(
                        "the managed Pod could not be created".into(),
                    ));
                }
            }
        };
        let uid = actual
            .metadata
            .uid
            .clone()
            .filter(|uid| !uid.is_empty())
            .ok_or_else(|| {
                DriverError::Unavailable("the Kubernetes API did not identify the Pod".into())
            })?;
        let unit = UnitRef {
            reference: uid.clone(),
            name: spec.name.clone(),
            runtime_id: spec.runtime_id,
            kind: spec.kind,
        };
        if actual.metadata.name.as_deref() != Some(spec.name.as_str())
            || actual.metadata.namespace.as_deref() != Some(self.config.namespace.as_str())
        {
            self.quarantine(runtime, deadline).await?;
            return Err(DriverError::Unavailable(
                "the Kubernetes API returned an unexpected Pod".into(),
            ));
        }
        // Bind the returned UID before inspecting admission changes, so a rejected Pod remains identifiable.
        let journal = Arc::clone(&self.journal);
        let saved_uid = uid.clone();
        tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || {
                journal.bind_pod(runtime, creation_intent, &saved_uid)
            }),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("the unit identity could not be recorded".into())
        })??;
        let verified = verify_created(
            &actual.metadata,
            &self.config.namespace,
            system,
            &unit,
            creation_intent,
        )
        .and_then(|()| {
            let actual_json = serde_json::to_value(actual).map_err(|_| {
                DriverError::Internal("the admitted Pod could not be inspected".into())
            })?;
            verify_admission::same_isolation(&expected, &actual_json, false)
        });
        if let Err(error) = verified {
            self.quarantine(runtime, deadline).await?;
            return Err(error);
        }
        self.journal.bind_admission(runtime, &expected)?;
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        Ok(unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::runtime::{RuntimeKind, unit_labels};

    #[test]
    fn a_create_response_needs_the_exact_recorded_uid_and_intent() {
        let runtime = orch_types::new_id();
        let intent = orch_types::new_id();
        let unit = UnitRef {
            reference: "pod-uid".into(),
            name: "work".into(),
            runtime_id: runtime,
            kind: RuntimeKind::Environment,
        };
        let mut labels = unit_labels("system", RuntimeKind::Environment, &runtime);
        labels.insert(INTENT_ANNOTATION.into(), intent.to_string());
        let mut metadata = ObjectMeta {
            name: Some("work".into()),
            namespace: Some("ns".into()),
            uid: Some("pod-uid".into()),
            annotations: Some(labels),
            labels: Some(std::collections::BTreeMap::from([(
                UNIT_SELECTOR.into(),
                runtime.simple().to_string(),
            )])),
            ..ObjectMeta::default()
        };
        assert!(verify_created(&metadata, "ns", "system", &unit, intent).is_ok());
        metadata.uid = Some("replacement".into());
        assert!(verify_created(&metadata, "ns", "system", &unit, intent).is_err());
        metadata.uid = Some("pod-uid".into());
        assert!(verify_created(&metadata, "ns", "system", &unit, orch_types::new_id()).is_err());
        let mut adopted = metadata.clone();
        adopted.owner_references = Some(vec![
            serde_json::from_value(serde_json::json!({"apiVersion":"apps/v1",
            "kind":"ReplicaSet", "name":"foreign", "uid":"foreign-uid"}))
            .unwrap(),
        ]);
        assert!(verify_created(&adopted, "ns", "system", &unit, intent).is_err());
        let mut held = metadata.clone();
        held.finalizers = Some(vec!["example.test/foreign".into()]);
        assert!(verify_created(&held, "ns", "system", &unit, intent).is_err());
        metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("injected".into(), "label".into());
        assert!(verify_created(&metadata, "ns", "system", &unit, intent).is_err());
    }
}
