//! Idle fleet selection and recycling through the ordinary credential-returning stop path.

use crate::commands::{Outcome, Outgoing};
use crate::environments::{StopError, StopReason};
use crate::ids;
use crate::{Engine, EngineError};
use orch_db::queries::{environments::hosting as envs, jobs::changes};
use orch_link_contract::commands::{ProcList, SerialKey};
use orch_types::{Id, Json, Time, new_id};

pub mod operation;

/// The native idle/process candidate before any command or stop decision.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Actual current Environment state.
    pub state: envs::EnvironmentState,
    /// Actual current unit, if there is one.
    pub runtime: Option<envs::RuntimeFull>,
}

/// The one native process observation and its exact retained source, without a stop decision.
pub struct Observation {
    ready: bool,
    source: Option<operation::Input>,
}
impl Observation {
    /// Whether the native process filter admitted the same physical candidate.
    pub fn ready(&self) -> bool {
        self.ready
    }
    /// Carry its actual original/current minimum into the next native boundary.
    pub fn into_source(self) -> Option<operation::Input> {
        self.source
    }
}

/// The existing native idle and activity rules, without process IO or a permission grant.
pub async fn candidate_in(
    conn: &mut orch_db::Conn,
    environment: Id,
    idle_minutes: u32,
    now: Time,
) -> Result<Option<Candidate>, EngineError> {
    if crate::background_wake::running(conn, None, None, Some(environment)).await? {
        return Ok(None);
    }
    let Some(state) = envs::environment(conn, environment)
        .await?
        .filter(|state| state.deleted_at.is_none())
    else {
        return Ok(None);
    };
    if !orch_db::queries::environment_facts::has_unit(conn, environment).await?
        || state.status != "running"
        || state.remove_at.is_some()
        || now.get() - state.last_activity_at.unwrap_or(state.created_at)
            < chrono::Duration::minutes(i64::from(idle_minutes))
        || state
            .config
            .get("env.idle_stop_minutes")
            .and_then(Json::as_i64)
            == Some(0)
        || changes::environment_hold(conn, environment)
            .await?
            .is_some()
        || envs::active_turns(conn, environment).await? > 0
        || envs::open_shells(conn, environment).await? > 0
    {
        return Ok(None);
    }
    let runtime = envs::live_runtime(conn, environment).await?;
    Ok(Some(Candidate { state, runtime }))
}

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
        let Some(_state) = envs::lock_environment(&mut tx, environment)
            .await?
            .filter(|state| state.deleted_at.is_none())
        else {
            return Ok(false);
        };
        let Some(candidate) =
            candidate_in(&mut tx, environment, idle_minutes, principal.now()).await?
        else {
            return Ok(false);
        };
        if let Some(host) = host
            && candidate
                .runtime
                .as_ref()
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
        let Some(candidate) = candidate_in(
            &mut conn,
            environment,
            idle_minutes,
            self.inner.clock.time(),
        )
        .await?
        else {
            return Ok(false);
        };
        drop(conn);
        Ok(self
            .probe_recycle_candidate(&candidate, include_with_processes, None)
            .await?
            .ready())
    }

    /// The original bounded process probe, optionally carrying its actual request source.
    pub async fn probe_recycle_candidate(
        &self,
        candidate: &Candidate,
        include_with_processes: bool,
        source: Option<&operation::Input>,
    ) -> Result<Observation, EngineError> {
        let environment = candidate.state.id;
        let mut source = source.cloned();
        if include_with_processes {
            return Ok(Observation {
                ready: true,
                source,
            });
        }
        let Some(machine) = candidate.state.machine_id else {
            return Ok(Observation {
                ready: false,
                source,
            });
        };
        let mut command = Outgoing::new(
            ids::derived("proc.list", &[new_id().as_bytes()]),
            machine,
            SerialKey::Environment(environment),
            &ProcList {
                environment_id: environment,
            },
        )?;
        if let Some(current) = source.as_ref() {
            let mut tx = self.db().begin().await?;
            let Some((sealed, accepted)) = current
                .seal_probe(&mut tx, self.inner.clock.as_ref(), command)
                .await?
            else {
                return Ok(Observation {
                    ready: false,
                    source,
                });
            };
            command = sealed;
            source = Some(accepted);
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        let id = command.id;
        let result = tokio::time::timeout(
            self.inner.config.hosting.stop_wait,
            crate::commands::run(self.inner.commands.as_ref(), command),
        )
        .await;
        if result.is_err() {
            self.inner.commands.cancel(id).await?;
        }
        if let Some(current) = source.as_ref() {
            let mut tx = self.db().begin().await?;
            let check = current
                .checkpoint(&mut tx, self.inner.clock.as_ref())
                .await?;
            if !check.matches() {
                return Ok(Observation {
                    ready: false,
                    source,
                });
            }
            check.check(self.inner.clock.as_ref())?;
            source = Some(check.accepted_input()?);
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        let ready = match result {
            Ok(Ok(Outcome::Succeeded(value))) => value
                .get("processes")
                .and_then(Json::as_array)
                .is_some_and(|p| p.is_empty()),
            _ => false,
        };
        Ok(Observation { ready, source })
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
