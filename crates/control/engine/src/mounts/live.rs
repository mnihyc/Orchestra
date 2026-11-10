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

enum MountPlan {
    Absent,
    Status {
        runtime: Id,
        reason: Option<String>,
    },
    Apply {
        runtime: Id,
        machine: Id,
        wanted: Json,
        layers: serde_json::Map<String, Json>,
    },
}

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

    /// One native read-only mount planner serves ordinary and scoped inherited application.
    async fn prepare_mount_application(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
    ) -> Result<MountPlan, EngineError> {
        let Some(env) = environments::environment(conn, environment).await? else {
            return Ok(MountPlan::Absent);
        };
        if !orch_db::queries::environment_facts::has_unit(conn, environment).await?
            || env.status != "running"
            || changes::environment_hold(conn, environment)
                .await?
                .is_some()
        {
            return Ok(MountPlan::Absent);
        }
        let Some(runtime) = environments::live_runtime(conn, environment).await? else {
            return Ok(MountPlan::Absent);
        };
        if runtime.state != "running" {
            return Ok(MountPlan::Absent);
        }
        let Some(host) = hosts::host(conn, runtime.host_id).await? else {
            return Ok(MountPlan::Absent);
        };
        let resolved = super::resolve(conn, environment, self.inner.clock.now()).await?;
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
        let old = match mounts::applied(conn, runtime.id).await? {
            Some(old) => Some(old),
            None => changes::definition(conn, runtime.id)
                .await?
                .and_then(|definition| definition.get("mounts").cloned()),
        };
        let requested = mounts::requested(conn, runtime.id).await?;
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
            return Ok(MountPlan::Status {
                runtime: runtime.id,
                reason: None,
            });
        }

        let created = changes::definition(conn, runtime.id).await?;
        if requested.is_none()
            && crate::isolation::unit_driver(&host.drivers, &runtime.driver, created.as_ref())
                .as_ref()
                .and_then(|descriptor| descriptor.pointer("/live_changes/mounts"))
                .and_then(Json::as_bool)
                != Some(true)
        {
            return Ok(MountPlan::Status {
                runtime: runtime.id,
                reason: Some("the driver does not apply mounts to running units".to_owned()),
            });
        }
        Ok(MountPlan::Apply {
            runtime: runtime.id,
            machine: host.machine_id,
            wanted,
            layers,
        })
    }

    /// A successful complete set is remembered by the existing command journal; a refusal remains an update.
    pub async fn apply_environment_mounts(&self, environment: Id) -> Result<(), EngineError> {
        self.apply_environment_mounts_origin(environment, None)
            .await
    }
    pub(crate) async fn apply_environment_mounts_for_setting(
        &self,
        environment: Id,
        origin: &orch_authz::CapturedOperation,
        change: &crate::setting_operations::Change,
    ) -> Result<(), EngineError> {
        self.apply_environment_mounts_origin(environment, Some((origin, change)))
            .await
    }
    async fn apply_environment_mounts_origin(
        &self,
        environment: Id,
        application: Option<(
            &orch_authz::CapturedOperation,
            &crate::setting_operations::Change,
        )>,
    ) -> Result<(), EngineError> {
        let serial = self.mount_serial(environment);
        let _serial = serial.lock().await;
        let mut tx = self.db().begin().await?;
        let mut ready = if let Some((origin, change)) = application {
            Some(
                crate::setting_operations::prepare_work(
                    &mut tx,
                    self.inner.clock.as_ref(),
                    origin,
                    crate::setting_operations::Work {
                        change: change.clone(),
                        target: crate::setting_operations::TargetWork::Environment(environment),
                    },
                )
                .await?,
            )
        } else {
            None
        };
        let plan = self.prepare_mount_application(&mut tx, environment).await?;
        let (runtime, machine, wanted, layers) = match plan {
            MountPlan::Absent => {
                tx.commit().await.map_err(orch_db::DbError::from)?;
                return Ok(());
            }
            MountPlan::Status { runtime, reason } => {
                if let Some(ready) = ready.take() {
                    ready
                        .first_effect
                        .check(self.inner.clock.as_ref())
                        .map_err(EngineError::unreadable)?;
                }
                mounts::live_result(&mut tx, runtime, reason.as_deref(), self.inner.clock.now())
                    .await?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                return Ok(());
            }
            MountPlan::Apply {
                runtime,
                machine,
                wanted,
                layers,
            } => (runtime, machine, wanted, layers),
        };
        if ready.as_ref().is_some_and(|ready| {
            !ready
                .value
                .runtime
                .is_some_and(|(id, _, recipient)| id == runtime && recipient == machine)
        }) {
            return Err(EngineError::unreadable(
                "the prepared setting runtime changed before mount application",
            ));
        }
        if !layers.is_empty() {
            if let Some(ready) = ready.take() {
                ready
                    .first_effect
                    .check(self.inner.clock.as_ref())
                    .map_err(EngineError::unreadable)?;
            }
            mounts::retain_layers(&mut tx, runtime, &Json::Object(layers)).await?;
        }
        // No connection is retained across native machine IO. The private journal evidence
        // checks the same original source again at actual queue/send/replay checkpoints.
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let mut command = Outgoing::new(
            orch_types::new_id(),
            machine,
            SerialKey::Runtime(runtime),
            &NodeMountsApply {
                runtime_id: runtime,
                mounts: wanted,
            },
        )?;
        if let Some((origin, change)) = application {
            command = crate::setting_operations::command(
                origin,
                change,
                crate::setting_operations::TargetWork::Environment(environment),
                runtime,
                command,
            )?;
        }
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
            runtime,
            if applied { None } else { reason.as_deref() },
            self.inner.clock.now(),
        )
        .await?;
        orch_db::queries::jobs::stamp_environment(&mut conn, environment, self.inner.clock.now())
            .await?;
        drop(conn);
        if applied {
            self.refresh_driver_mounts_origin(environment, runtime, application)
                .await?;
        }
        Ok(())
    }

    async fn refresh_driver_mounts(
        &self,
        environment: Id,
        runtime: Id,
    ) -> Result<bool, EngineError> {
        let origin =
            crate::setting_operations::last_mount_origin(&mut *self.db().acquire().await?, runtime)
                .await?;
        self.refresh_driver_mounts_origin(
            environment,
            runtime,
            origin.as_ref().map(|(origin, change)| (origin, change)),
        )
        .await
    }
    async fn refresh_driver_mounts_origin(
        &self,
        environment: Id,
        runtime: Id,
        application: Option<(
            &orch_authz::CapturedOperation,
            &crate::setting_operations::Change,
        )>,
    ) -> Result<bool, EngineError> {
        let (machine, generation, body) = {
            let mut conn = self.db().begin().await?;
            if let Some((origin, change)) = application {
                let ready = crate::setting_operations::prepare_work(
                    &mut conn,
                    self.inner.clock.as_ref(),
                    origin,
                    crate::setting_operations::Work {
                        change: change.clone(),
                        target: crate::setting_operations::TargetWork::Environment(environment),
                    },
                )
                .await?;
                if ready.value.runtime.is_none_or(|(id, _, _)| id != runtime) {
                    return Ok(false);
                }
            }
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
            conn.commit().await.map_err(orch_db::DbError::from)?;
            (machine, env.generation, body)
        };
        let mut command = Outgoing::new(
            orch_types::new_id(),
            machine,
            SerialKey::Environment(environment),
            &body,
        )?;
        if let Some((origin, change)) = application {
            command = crate::setting_operations::command(
                origin,
                change,
                crate::setting_operations::TargetWork::Environment(environment),
                runtime,
                command,
            )?;
        }
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
