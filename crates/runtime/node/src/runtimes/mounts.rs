//! Approved live mount sets retained across node restarts and reapplied when a unit gets a new namespace.

use super::*;
use crate::ledger::MountConfiguration;
use orch_driver_contract::mounts::{MountChange, MountSpec};
use orch_link_contract::commands::{NodeMountsApply, NodeMountsApplyResult};

fn pending(reason: &str) -> NodeMountsApplyResult {
    NodeMountsApplyResult {
        applied: false,
        mounts: Json::Null,
        reason: Some(reason.to_owned()),
    }
}

impl Runtimes {
    pub(super) async fn recover_interrupted_mounts(
        &self,
        runtime: Id,
        host: &HostDriver,
        record: &Record,
    ) -> Result<bool, CommandError> {
        if !lock(&self.inner.cold_mounts).contains(&runtime) {
            return Ok(false);
        }
        if let Some(mut configuration) = self.inner.ledger.mounts(runtime)
            && configuration.wanted != configuration.applied
            && configuration.pending_reason.is_none()
        {
            let state = host
                .contract()
                .inspect(&record.unit)
                .await
                .map_err(|error| failure(&error))?;
            if state.state == UnitStateKind::Running {
                // The durable intent preceded the effects but no complete result was retained.
                // Stop this uncertain namespace; an accepted set on a normal restart is kept running.
                host.contract()
                    .stop(&record.unit, self.inner.settings.stop_grace)
                    .await
                    .map_err(|error| failure(&error))?;
                configuration.applied = configuration.base.clone();
                configuration.applied_started_at = None;
                configuration.pending_reason = Some("the node stopped an interrupted live mount change; recreate or an explicit start is needed".into());
                configuration.retry_pending = false;
                self.inner
                    .ledger
                    .set_mounts(runtime, configuration)
                    .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
                lock(&self.inner.cold_mounts).remove(&runtime);
                self.tell(runtime, host, &record.unit).await;
                return Ok(true);
            }
        }
        lock(&self.inner.cold_mounts).remove(&runtime);
        Ok(false)
    }

    fn defer_mounts(
        &self,
        runtime: Id,
        mut configuration: MountConfiguration,
        reason: String,
    ) -> Result<NodeMountsApplyResult, CommandError> {
        configuration.pending_reason = Some(reason.clone());
        self.inner
            .ledger
            .set_mounts(runtime, configuration)
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        Ok(pending(&reason))
    }

    /// Applies a complete approved set using the driver's declaration and the recorded owned unit.
    pub async fn apply_mounts(
        &self,
        request: &NodeMountsApply,
    ) -> Result<NodeMountsApplyResult, CommandError> {
        let _busy = self.inner.busy.enter(request.runtime_id);
        let serial = self.lock_of(request.runtime_id);
        let _serial = serial.lock().await;
        let (host, record) = self.require(request.runtime_id).await?;
        if record.kind != orch_driver_contract::runtime::RuntimeKind::Environment {
            return Err(failure(&DriverError::InvalidSpec(
                "a provider has no data mounts".into(),
            )));
        }
        self.recover_interrupted_mounts(request.runtime_id, &host, &record)
            .await?;
        let mut mounts: Vec<MountSpec> =
            serde_json::from_value(request.mounts.clone()).map_err(|_| {
                failure(&DriverError::InvalidSpec(
                    "the mount array is invalid".into(),
                ))
            })?;
        self.check_mount_paths(&mut mounts)?;
        let Some(mut configuration) = self.inner.ledger.mounts(request.runtime_id) else {
            return Ok(pending(
                "this old unit has no recorded mount baseline; recreate is needed",
            ));
        };
        configuration.wanted = mounts;
        configuration.pending_reason = None;
        configuration.retry_pending = false;
        self.inner
            .ledger
            .set_mounts(request.runtime_id, configuration.clone())
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        let result = self
            .apply_recorded_mounts(request.runtime_id, &host, &record, configuration)
            .await;
        self.tell(request.runtime_id, &host, &record.unit).await;
        result
    }

    async fn apply_recorded_mounts(
        &self,
        runtime: Id,
        host: &HostDriver,
        record: &Record,
        mut configuration: MountConfiguration,
    ) -> Result<NodeMountsApplyResult, CommandError> {
        let state = host
            .contract()
            .inspect(&record.unit)
            .await
            .map_err(|error| failure(&error))?;
        if state.state != UnitStateKind::Running {
            return self.defer_mounts(
                runtime,
                configuration,
                "the unit is not running; the change applies at its next start".into(),
            );
        }
        if state.started_at.is_some()
            && configuration.applied_started_at == state.started_at
            && configuration.applied == configuration.wanted
        {
            configuration.pending_reason = None;
            configuration.retry_pending = false;
            self.inner
                .ledger
                .set_mounts(runtime, configuration)
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
            return Ok(NodeMountsApplyResult {
                applied: true,
                mounts: Json::Null,
                reason: None,
            });
        }
        if configuration.wanted == configuration.base
            && configuration.applied_started_at != state.started_at
        {
            // A new namespace already has its ordinary mount set, including an abandoned live edit.
            configuration.applied = configuration.base.clone();
            configuration.applied_started_at = state.started_at;
            configuration.pending_reason = None;
            configuration.retry_pending = false;
            self.inner
                .ledger
                .set_mounts(runtime, configuration)
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
            return Ok(NodeMountsApplyResult {
                applied: true,
                mounts: Json::Null,
                reason: None,
            });
        }
        let declared = serde_json::to_value(host.describe())
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        if declared
            .pointer("/live_changes/mounts")
            .and_then(Json::as_bool)
            != Some(true)
        {
            configuration.retry_pending = false;
            if configuration.applied_started_at != state.started_at {
                configuration.applied = configuration.base.clone();
            }
            configuration.applied_started_at = state.started_at;
            return self.defer_mounts(
                runtime,
                configuration,
                "this driver does not apply mounts live; recreate is needed".into(),
            );
        }
        // A node restart keeps this namespace's accepted set. A unit restart gets its ordinary set.
        let before =
            if state.started_at.is_some() && configuration.applied_started_at == state.started_at {
                configuration.applied.clone()
            } else {
                configuration.base.clone()
            };
        let change = MountChange {
            before,
            after: configuration.wanted.clone(),
            reserved: configuration.reserved.clone(),
        };
        // A live-created private layer is owned data too, even after the mount is later detached.
        // Record it before driver work so ordinary volume removal cannot cut out a running unit's layer.
        let layers: Vec<_> = configuration
            .wanted
            .iter()
            .filter_map(|mount| mount.layer.as_ref().map(|layer| layer.name.clone()))
            .collect();
        if !layers.is_empty() {
            self.inner
                .ledger
                .update(runtime, |record| {
                    for layer in layers {
                        if !record.volumes.contains(&layer) {
                            record.volumes.push(layer);
                        }
                    }
                })
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        }
        match host.contract().apply_mounts(&record.unit, &change).await {
            Ok(mounts) => {
                configuration.applied = configuration.wanted.clone();
                configuration.applied_started_at = state.started_at;
                configuration.pending_reason = None;
                configuration.retry_pending = false;
                self.inner
                    .ledger
                    .set_mounts(runtime, configuration)
                    .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
                Ok(NodeMountsApplyResult {
                    applied: true,
                    mounts: serde_json::to_value(mounts).map_err(|error| {
                        CommandError::new(ErrorCode::Internal, error.to_string())
                    })?,
                    reason: None,
                })
            }
            Err(error) => {
                configuration.retry_pending = error.retryable();
                if !configuration.retry_pending {
                    configuration.applied = change.before.clone();
                    configuration.applied_started_at = state.started_at;
                }
                let mut reason = format!("{error}; the approved change waits for recreate");
                for mount in configuration
                    .wanted
                    .iter()
                    .chain(&configuration.applied)
                    .chain(&configuration.base)
                {
                    if !mount.source.is_empty() {
                        reason = reason.replace(&mount.source, "the selected mount data");
                    }
                }
                self.defer_mounts(runtime, configuration, reason)
            }
        }
    }

    pub(super) async fn restore_live_mounts(
        &self,
        runtime: Id,
        host: &HostDriver,
        record: &Record,
    ) {
        match self.recover_interrupted_mounts(runtime, host, record).await {
            Ok(true) => return,
            Err(error) => {
                tracing::warn!(%runtime,%error,"interrupted mount recovery could not finish");
                return;
            }
            Ok(false) => {}
        }
        let Some(configuration) = self.inner.ledger.mounts(runtime) else {
            return;
        };
        if configuration.wanted == configuration.base
            && configuration.applied == configuration.base
            && configuration.pending_reason.is_none()
        {
            return;
        }
        let Ok(current) = host.contract().inspect(&record.unit).await else {
            return;
        };
        if current.state != UnitStateKind::Running {
            return;
        }
        if configuration.pending_reason.is_some()
            && !configuration.retry_pending
            && current.started_at == configuration.applied_started_at
        {
            return;
        }
        match self
            .apply_recorded_mounts(runtime, host, record, configuration)
            .await
        {
            Ok(result) if result.applied && !result.mounts.is_null() => {
                // A connector may have opened its directory handles before a daemon-driven start
                // was reconciled. Its controller refreshes those handles from the completed record.
                self.inner
                    .reports
                    .runtime_if_changed(runtime, "running", Some("mount changes applied".into()))
                    .await;
                return;
            }
            Ok(result) if result.applied => {}
            Ok(result) => {
                tracing::warn!(runtime = %runtime, reason = ?result.reason, "live mounts remain pending for recreate")
            }
            Err(error) => {
                tracing::warn!(runtime = %runtime, %error, "live mounts could not be restored")
            }
        }
        self.tell(runtime, host, &record.unit).await;
    }

    /// Reconciles daemon-driven starts too; an unchanged start identity does no helper work.
    pub async fn reconcile_live_mounts(&self) {
        for record in self.inner.ledger.all() {
            let serial = self.lock_of(record.runtime_id);
            let Ok(_serial) = serial.try_lock() else {
                continue;
            };
            let Some(host) = self.inner.drivers.get(&record.driver) else {
                continue;
            };
            self.restore_live_mounts(record.runtime_id, &host, &record)
                .await;
        }
    }
}
