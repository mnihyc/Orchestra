//! Idle fleet selection and recycling through the ordinary credential-returning stop path.

use crate::commands::{Outcome, Outgoing};
use crate::environments::{StopError, StopReason};
use crate::ids;
use crate::{Engine, EngineError};
use orch_db::queries::{environments::hosting as envs, jobs::changes};
use orch_link_contract::commands::{ProcList, SerialKey};
use orch_types::{Id, Json, new_id};

impl Engine {
    /// Rechecks the selected unit while the request's fresh authority protects its stop decision.
    pub async fn recycle_environment_in(
        &self,
        mut tx: orch_db::Tx,
        principal: &orch_authz::Principal,
        origin: &orch_db::audit::Origin,
        environment: Id,
        idle_minutes: u32,
        host: Option<Id>,
    ) -> Result<bool, StopError> {
        let Some(state) = envs::lock_environment(&mut tx, environment)
            .await?
            .filter(|state| state.deleted_at.is_none())
        else {
            return Ok(false);
        };
        if !orch_db::queries::environment_facts::has_unit(&mut tx, environment).await?
            || state.status != "running"
            || state.remove_at.is_some()
            || principal.now().get() - state.last_activity_at.unwrap_or(state.created_at)
                < chrono::Duration::minutes(i64::from(idle_minutes))
            || state
                .config
                .get("env.idle_stop_minutes")
                .and_then(Json::as_i64)
                == Some(0)
            || changes::environment_hold(&mut tx, environment)
                .await?
                .is_some()
            || envs::active_turns(&mut tx, environment).await? > 0
            || envs::open_shells(&mut tx, environment).await? > 0
        {
            return Ok(false);
        }
        if let Some(host) = host
            && envs::live_runtime(&mut tx, environment)
                .await?
                .is_none_or(|runtime| runtime.host_id != host)
        {
            return Ok(false);
        }
        self.stop_environment_in(
            tx,
            principal,
            origin,
            environment,
            false,
            StopReason::Recycle(principal.user()),
        )
        .await?;
        Ok(true)
    }

    /// Whether an environment is idle enough to recycle, with no held job, active turn or open shell.
    pub async fn recyclable(
        &self,
        environment: Id,
        idle_minutes: u32,
        include_with_processes: bool,
    ) -> Result<bool, EngineError> {
        let mut conn = self.db().acquire().await?;
        let Some(state) = envs::environment(&mut conn, environment)
            .await?
            .filter(|s| s.deleted_at.is_none())
        else {
            return Ok(false);
        };
        if !orch_db::queries::environment_facts::has_unit(&mut conn, environment).await?
            || state.status != "running"
            || state.remove_at.is_some()
        {
            return Ok(false);
        }
        let last = state.last_activity_at.unwrap_or(state.created_at);
        if self.inner.clock.now() - last < chrono::Duration::minutes(i64::from(idle_minutes)) {
            return Ok(false);
        }
        if state
            .config
            .get("env.idle_stop_minutes")
            .and_then(Json::as_i64)
            == Some(0)
        {
            return Ok(false);
        }
        if changes::environment_hold(&mut conn, environment)
            .await?
            .is_some()
            || envs::active_turns(&mut conn, environment).await? > 0
            || envs::open_shells(&mut conn, environment).await? > 0
        {
            return Ok(false);
        }
        if include_with_processes {
            return Ok(true);
        }
        let Some(machine) = state.machine_id else {
            return Ok(false);
        };
        drop(conn);
        let command = Outgoing::new(
            ids::derived("proc.list", &[new_id().as_bytes()]),
            machine,
            SerialKey::Environment(environment),
            &ProcList {
                environment_id: environment,
            },
        )?;
        let id = command.id;
        let result = tokio::time::timeout(
            self.inner.config.hosting.stop_wait,
            crate::commands::run(self.inner.commands.as_ref(), command),
        )
        .await;
        match result {
            Ok(Ok(Outcome::Succeeded(value))) => Ok(value
                .get("processes")
                .and_then(Json::as_array)
                .is_some_and(|p| p.is_empty())),
            Err(_) => {
                self.inner.commands.cancel(id).await?;
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Rechecks the same eligibility as the dry run immediately before stopping a candidate.
    pub async fn recycle_environment(
        &self,
        environment: Id,
        idle_minutes: u32,
        include_with_processes: bool,
        by: Id,
    ) -> Result<bool, StopError> {
        if !self
            .recyclable(environment, idle_minutes, include_with_processes)
            .await?
        {
            return Ok(false);
        }
        self.stop_environment(environment, false, StopReason::Recycle(by))
            .await?;
        Ok(true)
    }
}
