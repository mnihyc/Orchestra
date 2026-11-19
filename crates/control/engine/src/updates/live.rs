//! Accepted resource limits applied to the existing unit, with completed acknowledgments defining pending updates.

use super::Target;
use crate::{
    Engine, EngineError,
    commands::{Outcome, Outgoing},
};
use orch_db::Conn;
use orch_db::queries::{
    environments::hosting as environments,
    jobs::changes,
    resources::{hosting as hosts, provider_runtime as providers},
};
use orch_driver_contract::runtime::RuntimeChanges;
use orch_link_contract::commands::{NodeResourcesApply, SerialKey};
use orch_settings::get;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};

struct RuntimeReading {
    runtime: Id,
    machine: Id,
    online: bool,
    changes: RuntimeChanges,
}

async fn read_changes(
    conn: &mut Conn,
    target: Target,
) -> Result<Option<RuntimeReading>, EngineError> {
    let runtime = match target {
        Target::Environment(id) => environments::live_runtime(conn, id).await?,
        Target::Provider(id) => providers::live_runtime(conn, id).await?,
    };
    let Some(runtime) = runtime else {
        return Ok(None);
    };
    let Some(host) = hosts::host(conn, runtime.host_id).await? else {
        return Ok(None);
    };
    let created = changes::definition(conn, runtime.id).await?;
    let live_changes = serde_json::from_value(
        crate::isolation::unit_driver(&host.drivers, &runtime.driver, created.as_ref())
            .and_then(|driver| driver.get("live_changes").cloned())
            .unwrap_or_else(|| serde_json::json!({})),
    )?;
    let resources = created
        .and_then(|spec| spec.get("resources").cloned())
        .and_then(|limits| serde_json::from_value(limits).ok())
        .unwrap_or_default();
    let pending_reason = runtime
        .state_detail
        .as_deref()
        .and_then(|detail| detail.strip_prefix("resource changes pending: "))
        .map(str::to_owned);
    Ok(Some(RuntimeReading {
        runtime: runtime.id,
        machine: host.machine_id,
        online: host.online,
        changes: RuntimeChanges {
            pending_reason,
            live_changes,
            resources,
            memory_used_bytes: None,
        },
    }))
}

impl Engine {
    /// Stored limits, supported changes and pending reason on the caller's connection, without a host request.
    pub async fn runtime_changes_from(
        &self,
        conn: &mut Conn,
        target: Target,
    ) -> Result<Option<RuntimeChanges>, EngineError> {
        Ok(read_changes(conn, target)
            .await?
            .map(|reading| reading.changes))
    }

    /// Current use is measured on demand for a change form, never assumed from a configured limit.
    pub async fn runtime_changes(
        &self,
        target: Target,
    ) -> Result<Option<orch_driver_contract::runtime::RuntimeChanges>, EngineError> {
        let reading = {
            let mut conn = self.db().acquire().await?;
            read_changes(&mut conn, target).await?
        };
        let Some(reading) = reading else {
            return Ok(None);
        };
        let pending_reason = reading.changes.pending_reason.clone();
        let fallback = reading.changes;
        if !reading.online {
            return Ok(Some(fallback));
        }
        let command = Outgoing::new(
            orch_types::new_id(),
            reading.machine,
            SerialKey::Runtime(reading.runtime),
            &orch_link_contract::commands::NodeResourcesInspect {
                runtime_id: reading.runtime,
            },
        )?;
        let mut found = match self.ask_bounded(command).await? {
            Some(Outcome::Succeeded(value)) => serde_json::from_value(value).unwrap_or(fallback),
            _ => fallback,
        };
        found.pending_reason = pending_reason;
        Ok(Some(found))
    }
    pub async fn apply_environment_resources(&self, environment: Id) -> Result<bool, EngineError> {
        self.apply_runtime_resources(Target::Environment(environment))
            .await
    }

    pub async fn apply_provider_resources(&self, provider: Id) -> Result<bool, EngineError> {
        self.apply_runtime_resources(Target::Provider(provider))
            .await
    }

    pub async fn resource_scope_changed(&self, node: Id) -> Result<(), EngineError> {
        let environments =
            environments::environments_under(&mut *self.db().acquire().await?, node).await?;
        for environment in environments {
            self.apply_environment_resources(environment).await?;
        }
        let providers:Vec<Id>=sqlx::query_scalar("SELECT p.id FROM providers p JOIN nodes n ON n.id=p.home_id WHERE $1=ANY(n.path) AND p.deleted_at IS NULL AND EXISTS(SELECT 1 FROM runtimes r WHERE r.provider_id=p.id AND r.state NOT IN ('retired','removed'))").bind(node).fetch_all(&mut *self.db().acquire().await?).await.map_err(orch_db::DbError::from)?;
        for provider in providers {
            self.apply_provider_resources(provider).await?;
        }
        Ok(())
    }

    async fn apply_runtime_resources(&self, target: Target) -> Result<bool, EngineError> {
        let serial = self.mount_serial(target.id());
        let _held = serial.lock().await;
        let (runtime, machine, limits) = {
            let mut conn = self.db().acquire().await?;
            let runtime = match target {
                Target::Environment(id) => environments::live_runtime(&mut conn, id).await?,
                Target::Provider(id) => providers::live_runtime(&mut conn, id).await?,
            };
            let Some(runtime) = runtime else {
                return Ok(false);
            };
            if !matches!(runtime.state.as_str(), "running" | "stopped") {
                return Ok(false);
            }
            let held = match target {
                Target::Environment(id) => changes::environment_hold(&mut conn, id).await?,
                Target::Provider(id) => changes::provider_hold(&mut conn, id).await?,
            };
            if held.is_some() {
                return Ok(false);
            }
            let Some(host) = hosts::host(&mut conn, runtime.host_id).await? else {
                return Ok(false);
            };
            let Some(recorded) = changes::definition(&mut conn, runtime.id).await? else {
                return Ok(false);
            };
            if crate::isolation::unit_driver(&host.drivers, &runtime.driver, Some(&recorded))
                .as_ref()
                .and_then(|driver| driver.pointer("/live_changes/resources"))
                .and_then(Json::as_bool)
                != Some(true)
            {
                return Ok(false);
            }
            let key = match target {
                Target::Environment(_) => keys::ENV_RESOURCES,
                Target::Provider(_) => keys::RUNTIME_RESOURCES,
            };
            let own: Option<Json> = get(&mut conn, target.place(), key).await?;
            let definition = recorded.get("spec").unwrap_or(&Json::Null).clone();
            let limits = crate::places::limits_of(&definition, own.as_ref());
            let before = recorded
                .get("resources")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            if before == limits {
                return Ok(true);
            }
            if before.get("disk_gb") != limits.get("disk_gb") {
                return Ok(false);
            }
            (runtime, host.machine_id, limits)
        };
        let command = Outgoing::new(
            orch_types::new_id(),
            machine,
            SerialKey::Runtime(runtime.id),
            &NodeResourcesApply {
                runtime_id: runtime.id,
                resources: limits,
            },
        )?;
        let outcome = self.ask_bounded(command).await?;
        let applied = matches!(&outcome, Some(Outcome::Succeeded(_)));
        let pending = if applied {
            None
        } else {
            Some(match outcome {
                Some(Outcome::Failed(error)) => {
                    format!("resource changes pending: {}", error.detail)
                }
                _ => {
                    "resource changes pending: the host has not acknowledged the live change".into()
                }
            })
        };
        {
            let mut conn = self.db().acquire().await?;
            sqlx::query("UPDATE runtimes SET state_detail=$2 WHERE id=$1 AND (state_detail IS NULL OR state_detail LIKE 'resource changes pending:%')").bind(runtime.id).bind(pending).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
        }
        if applied {
            let mut conn = self.db().acquire().await?;
            if let Target::Environment(id) = target {
                orch_db::queries::jobs::stamp_environment(&mut conn, id, self.inner.clock.now())
                    .await?;
            }
        }
        Ok(applied)
    }
}
