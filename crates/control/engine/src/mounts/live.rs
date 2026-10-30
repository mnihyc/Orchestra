//! Applying resolved mount changes to a running unit, with explicit pending-recreate reasons on refusal.

use crate::commands::{Outcome, Outgoing};
use crate::{Engine, EngineError};
use orch_db::queries::{
    environments::hosting as environments,
    jobs::changes,
    resources::{hosting as hosts, mounts},
};
use orch_link_contract::commands::{NodeMountsApply, NodeMountsApplyResult, SerialKey};
use orch_types::{Id, Json};

impl Engine {
    pub(crate) fn mount_serial(&self, environment: Id) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .inner
            .mount_changes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(&environment).and_then(std::sync::Weak::upgrade) {
            Some(serial) => serial,
            None => {
                let serial = std::sync::Arc::new(tokio::sync::Mutex::new(()));
                locks.insert(environment, std::sync::Arc::downgrade(&serial));
                serial
            }
        }
    }

    /// Reopens connector directory handles after a completed replay, without holding up turn supervision.
    pub(crate) async fn keep_live_mount_facts(&self) -> Result<(), EngineError> {
        // Four independent attaches bound the work started by one supervisor pass.
        const REFRESH_BATCH: usize = 4;
        let pending: Vec<(Id,Id)> = sqlx::query_as("SELECT r.environment_id,r.id FROM runtimes r JOIN environments e ON e.id=r.environment_id JOIN machines m ON m.id=e.machine_id AND m.online AND m.deleted_at IS NULL WHERE r.state='running' AND r.successor_of IS NULL AND r.state_detail='mount changes applied' AND e.status='running' AND e.deleted_at IS NULL ORDER BY r.updated_at,r.id LIMIT $1")
            .bind(self.inner.config.sweep_batch).fetch_all(&mut *self.db().acquire().await?).await.map_err(orch_db::DbError::from)?;
        let mut started = 0;
        for (environment, runtime) in pending {
            if started >= REFRESH_BATCH {
                break;
            }
            let Ok(held) = self.mount_serial(environment).try_lock_owned() else {
                continue;
            };
            started += 1;
            let engine = self.clone();
            tokio::spawn(async move {
                let _held = held;
                let refreshed = matches!(
                    engine.refresh_driver_mounts(environment, runtime).await,
                    Ok(true)
                );
                if let Ok(mut conn) = engine.db().acquire().await {
                    // Rotate an unavailable connector behind other pending ones; an interrupted
                    // controller keeps the completion marker for the next process.
                    let _ = sqlx::query("UPDATE runtimes SET state_detail=CASE WHEN $2 THEN NULL ELSE state_detail END,updated_at=$3 WHERE id=$1 AND state_detail='mount changes applied'").bind(runtime).bind(refreshed).bind(engine.inner.clock.now()).execute(&mut *conn).await;
                }
            });
        }
        Ok(())
    }

    /// Tries the live path for every environment affected by a resource edit.
    pub async fn mount_resource_changed(&self, mount: Id) -> Result<(), EngineError> {
        let environments = mounts::candidates(&mut *self.db().acquire().await?, mount).await?;
        for environment in environments {
            self.apply_environment_mounts(environment).await?;
        }
        Ok(())
    }

    /// Tries the live path after an inherited mount setting changed.
    pub async fn mount_scope_changed(&self, node: Id) -> Result<(), EngineError> {
        let environments =
            environments::environments_under(&mut *self.db().acquire().await?, node).await?;
        for environment in environments {
            self.apply_environment_mounts(environment).await?;
        }
        Ok(())
    }

    /// A successful complete set is remembered by the existing command journal; a refusal remains an update.
    pub async fn apply_environment_mounts(&self, environment: Id) -> Result<(), EngineError> {
        let serial = self.mount_serial(environment);
        let _serial = serial.lock().await;
        let (runtime, machine, wanted) = {
            let mut conn = self.db().acquire().await?;
            let Some(env) = environments::environment(&mut conn, environment).await? else {
                return Ok(());
            };
            if !orch_db::queries::environment_facts::has_unit(&mut conn, environment).await?
                || env.status != "running"
                || changes::environment_hold(&mut conn, environment)
                    .await?
                    .is_some()
            {
                return Ok(());
            }
            let Some(runtime) = environments::live_runtime(&mut conn, environment).await? else {
                return Ok(());
            };
            if runtime.state != "running" {
                return Ok(());
            }
            let Some(host) = hosts::host(&mut conn, runtime.host_id).await? else {
                return Ok(());
            };
            let resolved = super::resolve(&mut conn, environment, self.inner.clock.now()).await?;
            let layers: serde_json::Map<String, Json> = resolved
                .iter()
                .filter_map(|mount| {
                    mount
                        .layer
                        .clone()
                        .map(|layer| (format!("over:{}", mount.view.mount.id), layer))
                })
                .collect();
            let desired = resolved
                .into_iter()
                .filter_map(|mount| mount.spec)
                .collect::<Vec<_>>();
            let wanted = serde_json::to_value(desired)?;
            let old = match mounts::applied(&mut conn, runtime.id).await? {
                Some(old) => Some(old),
                None => changes::definition(&mut conn, runtime.id)
                    .await?
                    .and_then(|definition| definition.get("mounts").cloned()),
            };
            let requested = mounts::requested(&mut conn, runtime.id).await?;
            let replay_pending = runtime.state_detail.as_deref().is_some_and(|detail| {
                detail.starts_with("mount changes pending:")
                    || detail.starts_with("mount changes need recreate:")
            });
            if old.as_ref() == Some(&wanted)
                && requested
                    .as_ref()
                    .is_none_or(|requested| requested == &wanted)
                && !replay_pending
            {
                mounts::live_result(&mut conn, runtime.id, None, self.inner.clock.now()).await?;
                return Ok(());
            }
            let created = changes::definition(&mut conn, runtime.id).await?;
            if requested.is_none()
                && crate::isolation::unit_driver(&host.drivers, &runtime.driver, created.as_ref())
                    .as_ref()
                    .and_then(|descriptor| descriptor.pointer("/live_changes/mounts"))
                    .and_then(Json::as_bool)
                    != Some(true)
            {
                mounts::live_result(
                    &mut conn,
                    runtime.id,
                    Some("the driver does not apply mounts to running units"),
                    self.inner.clock.now(),
                )
                .await?;
                return Ok(());
            }
            if !layers.is_empty() {
                mounts::retain_layers(&mut conn, runtime.id, &Json::Object(layers)).await?;
            }
            (runtime, host.machine_id, wanted)
        };
        let command = Outgoing::new(
            orch_types::new_id(),
            machine,
            SerialKey::Runtime(runtime.id),
            &NodeMountsApply {
                runtime_id: runtime.id,
                mounts: wanted,
            },
        )?;
        let (applied, reason) = match self.ask_bounded(command).await? {
            Some(Outcome::Succeeded(value)) => {
                let result: NodeMountsApplyResult = serde_json::from_value(value)?;
                (result.applied, result.reason)
            }
            _ => (
                false,
                Some("the node did not finish the live mount change".to_owned()),
            ),
        };
        let mut conn = self.db().acquire().await?;
        mounts::live_result(
            &mut conn,
            runtime.id,
            if applied { None } else { reason.as_deref() },
            self.inner.clock.now(),
        )
        .await?;
        orch_db::queries::jobs::stamp_environment(&mut conn, environment, self.inner.clock.now())
            .await?;
        drop(conn);
        if applied {
            self.refresh_driver_mounts(environment, runtime.id).await?;
        }
        Ok(())
    }

    async fn refresh_driver_mounts(
        &self,
        environment: Id,
        runtime: Id,
    ) -> Result<bool, EngineError> {
        let (machine, generation, body) = {
            let mut conn = self.db().acquire().await?;
            let Some(env) = environments::environment(&mut conn, environment).await? else {
                return Ok(false);
            };
            let Some(machine) = env.machine_id else {
                return Ok(false);
            };
            if orch_db::queries::resources::machine(&mut conn, machine)
                .await?
                .is_none_or(|machine| !machine.online || !machine.is_active())
            {
                return Ok(false);
            }
            if environments::live_runtime(&mut conn, environment)
                .await?
                .is_none_or(|current| current.id != runtime)
            {
                return Ok(false);
            }
            let with =
                crate::environments::attach_with(&mut conn, environment, self.inner.clock.now())
                    .await?;
            let mut body: orch_link_contract::commands::EnvAttach =
                match mounts::last_attach(&mut conn, environment, machine).await? {
                    Some(body) => serde_json::from_value(body)?,
                    None => crate::environments::attach_body(environment, None, &with),
                };
            body.workspace_root = None;
            body.mounts = None;
            body.variables = with.variables;
            body.secrets = with.secrets;
            (machine, env.generation, body)
        };
        let command = Outgoing::new(
            orch_types::new_id(),
            machine,
            SerialKey::Environment(environment),
            &body,
        )?;
        if let Some(Outcome::Succeeded(facts)) = self.ask_bounded(command).await? {
            let mut conn = self.db().acquire().await?;
            if environments::environment(&mut conn, environment)
                .await?
                .is_some_and(|env| env.machine_id == Some(machine) && env.generation == generation)
                && environments::live_runtime(&mut conn, environment)
                    .await?
                    .is_some_and(|current| current.id == runtime)
            {
                environments::record_facts(
                    &mut conn,
                    environment,
                    &facts,
                    None,
                    self.inner.clock.now(),
                )
                .await?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}
