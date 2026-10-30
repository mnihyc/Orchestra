//! Serialize claim creation and durably bind the native UID before answering, and remove exactly the
//! recorded claim once no Pod of the namespace uses it.
//!
//! A removal command names a volume, not an incarnation. It therefore acts on the incarnation this driver
//! recorded for that name, only while that claim is still the one recorded, and the delete itself carries the
//! recorded UID, so a claim made again under the name in the meantime is never taken.

use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::core::v1::{PersistentVolume, PersistentVolumeClaim, Pod};
use kube::api::{ApiResource, DeleteParams, DynamicObject, ListParams, Preconditions};
use kube::{Api, Client};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::volumes::VolumeRequest;
use tokio::time::Instant;

use crate::claims::{self, INTENT_ANNOTATION};
use crate::config::KubernetesConfig;
use crate::locks::KeyedLocks;
use crate::ownership::{ClaimReceipt, check_claim};
use crate::receipts::ReceiptStore;
use crate::verify_claim;

const HTTP_NOT_FOUND: u16 = 404;
/// Pods are read a hundred at a time while a claim's consumers are looked for.
const POD_PAGE: u32 = 100;
/// How often a deleted claim is looked at while its finalizers run.
const CLAIM_LOOK: Duration = Duration::from_millis(300);

/// A client and its already-open private journal, scoped to the same verified cluster identity.
pub(crate) struct ClaimService {
    claims: Api<PersistentVolumeClaim>,
    pods: Api<Pod>,
    raw_claims: Api<DynamicObject>,
    raw_volumes: Api<DynamicObject>,
    config: KubernetesConfig,
    journal: Arc<ReceiptStore>,
    locks: KeyedLocks,
    request_timeout: Duration,
}

impl ClaimService {
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
        let claims = Api::namespaced(client.clone(), &config.namespace);
        let pods = Api::namespaced(client.clone(), &config.namespace);
        let claim_resource = ApiResource::erase::<PersistentVolumeClaim>(&());
        let volume_resource = ApiResource::erase::<PersistentVolume>(&());
        let raw_claims = Api::namespaced_with(client.clone(), &config.namespace, &claim_resource);
        let raw_volumes = Api::all_with(client, &volume_resource);
        Ok(Self {
            claims,
            pods,
            raw_claims,
            raw_volumes,
            config,
            journal,
            locks: KeyedLocks::default(),
            request_timeout,
        })
    }

    async fn read_raw(
        &self,
        api: &Api<DynamicObject>,
        name: &str,
        deadline: Instant,
    ) -> Result<DynamicObject, DriverError> {
        tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(self.request_timeout, api.get_opt(name)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("the cluster storage object could not be read".into())
        })?
        .ok_or_else(|| DriverError::Unavailable("the cluster storage object is absent".into()))
    }

    /// Deletes the recorded claim once no Pod of any state, ours or foreign, uses it, and forgets its record.
    /// A claim that is already gone only has its record forgotten.
    pub(crate) async fn remove(
        &self,
        volume: &VolumeRequest,
        deadline: Instant,
    ) -> Result<(), DriverError> {
        let native = crate::names::claim(&volume.name)?;
        let journal = Arc::clone(&self.journal);
        let request = volume.clone();
        let intent = tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || journal.begin(&request)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Unavailable("the claim record could not be read".into()))??;
        let uid = intent.uid.clone().ok_or_else(|| {
            DriverError::Unavailable("the claim has no recorded incarnation".into())
        })?;
        let present = self.present(&native, deadline).await?;
        if let Some(found) = present {
            if found.metadata.uid.as_deref() != Some(uid.as_str()) {
                return Err(DriverError::Unavailable(
                    "the claim was replaced; review it before removing it".into(),
                ));
            }
            // The recorded claim must still be exactly what was made, with its protected labels.
            self.ensure(volume, deadline).await?;
            self.refuse_while_used(&native, deadline).await?;
            let params = DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(uid.clone()),
                    resource_version: None,
                }),
                ..DeleteParams::default()
            };
            match tokio::time::timeout_at(
                deadline,
                tokio::time::timeout(self.request_timeout, self.claims.delete(&native, &params)),
            )
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| DriverError::Timeout)?
            {
                Ok(_) => {}
                Err(kube::Error::Api(status)) if status.code == HTTP_NOT_FOUND => {}
                Err(_) => {
                    return Err(DriverError::Unavailable(
                        "the claim could not be deleted".into(),
                    ));
                }
            }
            // The claim's protection finalizer and its volume's reclaim take a moment.
            loop {
                let still = self.present(&native, deadline).await?;
                if still.is_none_or(|found| found.metadata.uid.as_deref() != Some(uid.as_str())) {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(DriverError::Timeout);
                }
                tokio::time::sleep_until((Instant::now() + CLAIM_LOOK).min(deadline)).await;
            }
        }
        let journal = Arc::clone(&self.journal);
        let name = volume.name.clone();
        tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || journal.release(&name, intent.intent, &uid)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Unavailable("the claim record could not be cleared".into()))?
    }

    async fn present(
        &self,
        native: &str,
        deadline: Instant,
    ) -> Result<Option<PersistentVolumeClaim>, DriverError> {
        tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(self.request_timeout, self.claims.get_opt(native)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Unavailable("the claim could not be read".into()))
    }

    /// Every extant Pod of the namespace is a consumer, read in pages.
    async fn refuse_while_used(&self, native: &str, deadline: Instant) -> Result<(), DriverError> {
        let mut cursor = String::new();
        loop {
            let page = tokio::time::timeout_at(
                deadline,
                tokio::time::timeout(
                    self.request_timeout,
                    self.pods.list(
                        &ListParams::default()
                            .limit(POD_PAGE)
                            .continue_token(&cursor),
                    ),
                ),
            )
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| DriverError::Unavailable("the Pods could not be listed".into()))?;
            if crate::ownership::claim_is_used(native, &page.items) {
                return Err(DriverError::Busy("a Pod still uses this volume".into()));
            }
            cursor = page.metadata.continue_.unwrap_or_default();
            if cursor.is_empty() {
                return Ok(());
            }
        }
    }

    /// Creates once or checks the exact recorded UID; an ambiguous lost response is not adopted.
    pub(crate) async fn ensure(
        &self,
        volume: &VolumeRequest,
        deadline: Instant,
    ) -> Result<ClaimReceipt, DriverError> {
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        let _held = self.locks.hold(&volume.name, deadline).await?;
        let journal = Arc::clone(&self.journal);
        let request = volume.clone();
        let intent = tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || journal.begin(&request)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("the claim creation intent could not be recorded".into())
        })??;
        let receipt = tokio::time::timeout_at(
            deadline,
            claims::ensure_claim(
                &self.claims,
                &self.config,
                volume,
                intent.intent,
                intent.uid.as_deref(),
                self.request_timeout,
            ),
        )
        .await
        .map_err(|_| DriverError::Timeout)??;
        let journal = Arc::clone(&self.journal);
        let name = volume.name.clone();
        let uid = receipt.uid.clone();
        tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || journal.bind(&name, intent.intent, &uid)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("the claim identity could not be recorded".into())
        })??;

        // Typed Kubernetes models discard fields they do not know; re-read raw data before acceptance.
        let raw = self
            .read_raw(&self.raw_claims, &receipt.native_name, deadline)
            .await?;
        check_claim(&raw.metadata, &self.config.namespace, &receipt)?;
        if raw.metadata.deletion_timestamp.is_some()
            || raw
                .metadata
                .owner_references
                .as_ref()
                .is_some_and(|owners| !owners.is_empty())
            || raw
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get(INTENT_ANNOTATION))
                != Some(&intent.intent.to_string())
        {
            return Err(DriverError::Unavailable(
                "the recorded claim changed while it was checked".into(),
            ));
        }
        let raw = serde_json::to_value(raw)
            .map_err(|_| DriverError::Internal("the claim could not be inspected".into()))?;
        let pv_name = raw
            .pointer("/spec/volumeName")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty());
        let pv_uid = if let Some(pv_name) = pv_name {
            let pv = self.read_raw(&self.raw_volumes, pv_name, deadline).await?;
            let pv = serde_json::to_value(pv).map_err(|_| {
                DriverError::Internal("the persistent volume could not be inspected".into())
            })?;
            let actual_request = raw
                .pointer("/spec/resources/requests/storage")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    DriverError::Unavailable("the claim has no storage request".into())
                })?;
            let uid = verify_claim::bound_pv(
                &pv,
                pv_name,
                &self.config.namespace,
                &receipt.native_name,
                &receipt.uid,
                &self.config.storage_class,
                actual_request,
            )?;
            if intent
                .pv_uid
                .as_deref()
                .is_some_and(|expected| expected != uid)
            {
                return Err(DriverError::Unavailable(
                    "the persistent volume was replaced".into(),
                ));
            }
            if intent
                .pv_name
                .as_deref()
                .is_some_and(|expected| expected != pv_name)
            {
                return Err(DriverError::Unavailable(
                    "the persistent volume name changed".into(),
                ));
            }
            Some(uid)
        } else {
            if intent.pv_uid.is_some() {
                return Err(DriverError::Unavailable(
                    "the recorded persistent volume is no longer bound".into(),
                ));
            }
            None
        };
        verify_claim::same_spec(
            &raw,
            &self.config.storage_class,
            &self.config.volume_request,
            pv_name,
        )?;
        if let Some(pv_uid) = pv_uid {
            let journal = Arc::clone(&self.journal);
            let name = volume.name.clone();
            let claim_uid = receipt.uid.clone();
            let pv_name = pv_name
                .ok_or_else(|| {
                    DriverError::Unavailable("the persistent volume name is missing".into())
                })?
                .to_owned();
            tokio::time::timeout_at(
                deadline,
                tokio::task::spawn_blocking(move || {
                    journal.bind_pv(&name, intent.intent, &claim_uid, &pv_name, &pv_uid)
                }),
            )
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| {
                DriverError::Unavailable(
                    "the persistent volume identity could not be recorded".into(),
                )
            })??;
        }
        if Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        Ok(receipt)
    }
}
