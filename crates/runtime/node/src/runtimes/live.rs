//! Live limits and capability changes stay within the prepared unit and persist their replay state.

use super::*;
use crate::ledger::CapabilityConfiguration;
use orch_driver_contract::capabilities::CapabilityState;
use orch_driver_contract::runtime::Resources;
use orch_link_contract::commands::{
    NodeCapabilitiesApply, NodeCapabilitiesApplyResult, NodeResourcesApply,
};

fn invalid(detail: &str) -> CommandError {
    CommandError {
        code: crate::drivers::INVALID.into(),
        detail: detail.into(),
        retryable: false,
        details: None,
    }
}

impl Runtimes {
    pub async fn inspect_resources(
        &self,
        request: &orch_link_contract::commands::NodeResourcesInspect,
    ) -> Result<orch_driver_contract::runtime::RuntimeChanges, CommandError> {
        let serial = self.lock_of(request.runtime_id);
        let _held = serial.lock().await;
        let (host, record) = self.require(request.runtime_id).await?;
        Ok(orch_driver_contract::runtime::RuntimeChanges {
            pending_reason: None,
            live_changes: host.describe().live_changes,
            resources: record.resources,
            memory_used_bytes: host
                .contract()
                .stats(&record.unit)
                .await
                .ok()
                .map(|sample| sample.memory_bytes),
        })
    }
    /// Changes declared limits on the same unit, rejecting a creation-fixed disk change before effects.
    pub async fn apply_resources(&self, request: &NodeResourcesApply) -> Result<(), CommandError> {
        let _busy = self.inner.busy.enter(request.runtime_id);
        let serial = self.lock_of(request.runtime_id);
        let _one = serial.lock().await;
        let (host, record) = self.require(request.runtime_id).await?;
        let resources: Resources = serde_json::from_value(request.resources.clone())
            .map_err(|_| invalid("the resource limits are not valid"))?;
        if resources.disk_gb != record.resources.disk_gb {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "a creation-fixed disk limit requires recreation",
            ));
        }
        if !host.describe().live_changes.resources {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "this driver requires recreation for resource changes",
            ));
        }
        host.contract()
            .apply_resources(&record.unit, &resources)
            .await
            .map_err(|error| failure(&error))?;
        self.inner
            .ledger
            .update(record.runtime_id, |record| record.resources = resources)
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        Ok(())
    }

    /// Gives or withdraws only grants inside this unit's frozen template boundary, never replacing its container.
    pub async fn apply_capabilities(
        &self,
        request: &NodeCapabilitiesApply,
    ) -> Result<NodeCapabilitiesApplyResult, CommandError> {
        let _busy = self.inner.busy.enter(request.runtime_id);
        let serial = self.lock_of(request.runtime_id);
        let _one = serial.lock().await;
        let (host, record) = self.require(request.runtime_id).await?;
        let after: CapabilityState = serde_json::from_value(request.capabilities.clone())
            .map_err(|_| invalid("the capability state is not valid"))?;
        after.check().map_err(|error| failure(&error))?;
        let before = self
            .inner
            .ledger
            .capabilities(request.runtime_id)
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Unsupported,
                    "this older unit has no prepared live-capability boundary; recreate it first",
                )
            })?;
        if !before.desired.same_boundary(&after) {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "changing the prepared allowlist, defaults or patches requires recreation",
            ));
        }
        let native = |state: &CapabilityState| {
            state
                .granted
                .iter()
                .filter(|key| !orch_driver_contract::capabilities::gateway_given(key))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        };
        let old = native(&before.applied);
        let new = native(&after);
        let declared = host.describe().live_changes.capabilities;
        if old
            .symmetric_difference(&new)
            .any(|key| !declared.contains(key))
        {
            return Ok(NodeCapabilitiesApplyResult {
                applied: false,
                restarted: false,
                reason: Some("the changed capability needs recreation on this driver".into()),
            });
        }
        self.inner
            .ledger
            .set_capabilities(
                record.runtime_id,
                CapabilityConfiguration {
                    desired: after.clone(),
                    applied: before.applied.clone(),
                    changing: true,
                    applied_started_at: before.applied_started_at,
                },
            )
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        match host
            .contract()
            .apply_capabilities(&record.unit, &before.applied, &after)
            .await
        {
            Ok(restarted) => {
                if restarted {
                    self.restart_capabilities(&host, &record, &after).await?;
                }
                let actual = host
                    .contract()
                    .inspect(&record.unit)
                    .await
                    .map_err(|error| failure(&error))?;
                self.inner
                    .ledger
                    .set_capabilities(
                        record.runtime_id,
                        CapabilityConfiguration {
                            desired: after.clone(),
                            applied: after,
                            changing: false,
                            applied_started_at: actual.started_at,
                        },
                    )
                    .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
                if restarted {
                    self.restore_live_mounts(request.runtime_id, &host, &record)
                        .await;
                    self.tell(request.runtime_id, &host, &record.unit).await;
                }
                Ok(NodeCapabilitiesApplyResult {
                    applied: true,
                    restarted,
                    reason: None,
                })
            }
            Err(error) => {
                // A partially given device may already be open. Restarting the same container
                // with the old set evicts those handles; a failed restoration leaves it stopped.
                self.restart_capabilities(&host, &record, &before.applied)
                    .await?;
                let actual = host
                    .contract()
                    .inspect(&record.unit)
                    .await
                    .map_err(|error| failure(&error))?;
                self.inner
                    .ledger
                    .set_capabilities(
                        record.runtime_id,
                        CapabilityConfiguration {
                            desired: before.applied.clone(),
                            applied: before.applied,
                            changing: false,
                            applied_started_at: actual.started_at,
                        },
                    )
                    .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
                Ok(NodeCapabilitiesApplyResult {
                    applied: false,
                    restarted: true,
                    reason: Some(error.to_string()),
                })
            }
        }
    }

    pub(super) async fn start_capabilities(
        &self,
        host: &HostDriver,
        record: &Record,
    ) -> Result<(), CommandError> {
        let state = self
            .inner
            .ledger
            .capabilities(record.runtime_id)
            .map(|state| state.desired)
            .unwrap_or_default();
        if let Err(error) = host
            .contract()
            .start_with_capabilities(&record.unit, &state)
            .await
        {
            self.close_after_failed_up(host, record).await;
            return Err(self.failed_for_room(&error));
        }
        let actual = host
            .contract()
            .inspect(&record.unit)
            .await
            .map_err(|error| failure(&error))?;
        self.inner
            .ledger
            .set_capabilities(
                record.runtime_id,
                CapabilityConfiguration {
                    desired: state.clone(),
                    applied: state,
                    changing: false,
                    applied_started_at: actual.started_at,
                },
            )
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        Ok(())
    }

    async fn restart_capabilities(
        &self,
        host: &HostDriver,
        record: &Record,
        state: &CapabilityState,
    ) -> Result<(), CommandError> {
        // A capability withdrawal restarts this unit; keep its volumes fenced against a successor.
        let _volume_holds = self.hold_start_volumes(record).await;
        let reason = UnitReason::Stop;
        let _ = self
            .hook(HookEvent::PreDown, reason, false, host, record)
            .await;
        host.contract()
            .stop(&record.unit, self.inner.settings.stop_grace)
            .await
            .map_err(|error| failure(&error))?;
        let _ = self
            .hook(HookEvent::PostDown, reason, false, host, record)
            .await;
        self.note_phase(record.runtime_id, HookPhase::Down);
        self.keep_off_shared_volumes(record).await?;
        if let Some(run) = self
            .hook(HookEvent::PreUp, UnitReason::Start, false, host, record)
            .await
            && run.failed
        {
            return Err(hook_failure(&run));
        }
        if let Err(error) = host
            .contract()
            .start_with_capabilities(&record.unit, state)
            .await
        {
            self.close_after_failed_up(host, record).await;
            return Err(failure(&error));
        }
        self.restore_live_mounts(record.runtime_id, host, record)
            .await;
        if let Some(run) = self
            .hook(HookEvent::PostUp, UnitReason::Start, false, host, record)
            .await
            && run.failed
        {
            self.close_after_failed_up(host, record).await;
            return Err(hook_failure(&run));
        }
        self.note_phase(record.runtime_id, HookPhase::Up);
        self.tell(record.runtime_id, host, &record.unit).await;
        Ok(())
    }

    /// Completes interrupted authorized changes and releases the gate after an operator's daemon start.
    pub async fn reconcile_live_capabilities(&self) {
        for record in self.inner.ledger.all() {
            let serial = self.lock_of(record.runtime_id);
            let Ok(_one) = serial.try_lock() else {
                continue;
            };
            let Some(host) = self.inner.drivers.get(&record.driver) else {
                continue;
            };
            let Some(state) = self.inner.ledger.capabilities(record.runtime_id) else {
                continue;
            };
            let Ok(actual) = host.contract().inspect(&record.unit).await else {
                continue;
            };
            if !state.changing
                && (actual.state != UnitStateKind::Running
                    || actual.started_at == state.applied_started_at)
            {
                continue;
            }
            let result = if state.changing {
                self.restart_capabilities(&host, &record, &state.desired)
                    .await
            } else {
                // The native unit can stop after the earlier inspection. A replay
                // must not start it again over a successor that has taken its volumes.
                let _volume_holds = self.hold_start_volumes(&record).await;
                match self.keep_off_shared_volumes(&record).await {
                    Ok(()) => self.start_capabilities(&host, &record).await,
                    Err(error) => Err(error),
                }
            };
            if result.is_ok() {
                if let Ok(actual) = host.contract().inspect(&record.unit).await {
                    let _ = self.inner.ledger.set_capabilities(
                        record.runtime_id,
                        CapabilityConfiguration {
                            desired: state.desired.clone(),
                            applied: state.desired,
                            changing: false,
                            applied_started_at: actual.started_at,
                        },
                    );
                }
            } else if let Err(error) = result {
                tracing::warn!(runtime=%record.runtime_id,%error,"the capability replay remains unfinished");
            }
        }
    }
}
