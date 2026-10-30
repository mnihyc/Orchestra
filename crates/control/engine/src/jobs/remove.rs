//! Removal of an environment or a retired unit, with a handed-over credential returned first.
//!
//! Removal is a job of its own, started by the schedule (`reaper`) for an environment whose removal
//! was scheduled and for a retired runtime whose time has come, and never part of what scheduled it:
//! what a switch, a move or an update replaced is already out of use, and nothing waits for it. The
//! phases: `credential` (a credential that was handed over to the environment is returned first),
//! `stop`, `unit` (the host removes it), `volumes` (the workspace, the home and the state; a retired
//! runtime removes only what its successor does not use) and `forget` (one transaction: the rows say
//! it is gone, the connector's machine is revoked, the token that was never used is deleted).
//!
//! A removal that fails is tried again with a pause that doubles with each failure, up to an hour,
//! and the reason is on the environment and on its host. It blocks nothing.

use crate::commands::Outgoing;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::jobs::runner::{
    Done, Failure, INTERNAL, Job, MakeError, Making, Phase, Phased, Stop, Undone, make,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_db::DbError;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::environments::hosting::{self as rows, RuntimeFull, runtime_state};
use orch_db::queries::environments::retained;
use orch_db::queries::jobs::{JobRow, kind};
use orch_db::queries::resources::hosting as host_rows;
use orch_link_contract::commands::{
    NodeRuntimeRemove, NodeRuntimeStop, NodeVolumeRemove, SerialKey,
};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

const CREDENTIAL: Phase = Phase {
    name: "credential",
    weight: 5,
};
const STOP: Phase = Phase {
    name: "stop",
    weight: 15,
};
const UNIT: Phase = Phase {
    name: "unit",
    weight: 20,
};
const VOLUMES: Phase = Phase {
    name: "volumes",
    weight: 50,
};
const FORGET: Phase = Phase {
    name: "forget",
    weight: 10,
};

/// The phases of a removal.
#[must_use]
pub fn phases() -> Vec<Phase> {
    vec![CREDENTIAL, STOP, UNIT, VOLUMES, FORGET]
}

/// Queues the removal of an environment (all that is its own), or of one retired runtime, in the
/// caller's transaction. A job of the kinds that run one at a time that is not over is
/// [`MakeError::Running`].
pub async fn queue(
    conn: &mut orch_db::Conn,
    now: DateTime<Utc>,
    environment: Id,
    runtime: Option<Id>,
) -> Result<JobRow, MakeError> {
    let removal = if runtime.is_none() {
        Some(
            retained::removal(conn, environment)
                .await?
                .ok_or(MakeError::NoEnvironment)?,
        )
    } else {
        None
    };
    let scratch = runtime.is_none() && retained::is_scratch(conn, environment).await?;
    let state = rows::environment(conn, environment).await?;
    let temporary = state.as_ref().is_some_and(|state| {
        orch_db::queries::environments::temporary::mode(&state.config).is_some()
    });
    let facts = if runtime.is_none() {
        orch_db::queries::environment_facts::of(conn, environment).await?
    } else {
        None
    };
    let native = if let Some(facts) = facts.as_ref().filter(|facts| facts.attaches()) {
        let generation: i64 =
            sqlx::query_scalar("SELECT data_generation FROM environments WHERE id=$1")
                .bind(environment)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
        Some(json!({"machine":facts.machine_id,"generation":generation}))
    } else {
        None
    };
    let origin = match &removal {
        Some(removal) => {
            retained::removal_origin(conn, environment, removal.removal_revision).await?
        }
        None => Origin::default(),
    };
    // The acknowledged mount layout must outlive the shorter command journal. Restoration
    // uses it as it was before removal, rather than resolving a different current layout.
    let retained_runtime = if runtime.is_none() && !scratch {
        rows::live_runtime(conn, environment)
            .await?
            .map(|row| row.id)
    } else {
        None
    };
    let retained_spec = match retained_runtime {
        Some(runtime) => retained::definition(conn, runtime).await?,
        None => None,
    };
    let row = make(
        conn,
        &Making {
            kind: kind::ENVIRONMENT_REMOVE,
            environment,
            plan: json!({ "runtime": runtime, "scratch": scratch, "temporary":temporary,"native":native,"location_known":facts.as_ref().is_some_and(|facts|facts.attaches()||facts.makes_units()),"origin":origin, "removal_revision": removal.as_ref().map(|r| r.removal_revision), "volume_refs": removal.as_ref().map(|r| r.retained_volumes.clone()), "retained_runtime":retained_runtime, "retained_spec":retained_spec }),
            phases: &phases(),
            created_by: removal.as_ref().and_then(|r| r.deleted_by),
        },
        now,
    )
    .await?;
    if let Some(removal) = removal
        && !retained::begin_remove(conn, environment, removal.removal_revision).await?
    {
        return Err(MakeError::Db(DbError::Rule {
            constraint: "environment_removal_changed".into(),
        }));
    }
    Ok(row)
}

/// The pause before a removal that failed `failures` times is tried again: the first pause, doubled
/// with each failure, and no longer than `longest`.
#[must_use]
pub fn pause_after(failures: i64, first: Duration, longest: Duration) -> Duration {
    let doublings = u32::try_from(failures.saturating_sub(1).clamp(0, 30)).unwrap_or(30);
    first
        .checked_mul(1u32 << doublings)
        .unwrap_or(longest)
        .min(longest)
}

/// The phases of removing an environment or a retired unit.
#[derive(Debug, Clone)]
pub struct Removal {
    engine: Engine,
    environment: Id,
    runtime: Option<Id>,
    scratch: bool,
    temporary: bool,
    native: Option<Json>,
    location_known: bool,
}

fn failed(code: &str, message: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new(code, message))
}

impl Removal {
    /// The work of a job row of the kind `environment_remove`.
    #[must_use]
    pub fn of(engine: Engine, row: &JobRow) -> Self {
        Self {
            engine,
            environment: row.environment_id.unwrap_or_else(Id::nil),
            runtime: row
                .plan
                .get("runtime")
                .and_then(Json::as_str)
                .and_then(|id| id.parse().ok()),
            scratch: row.plan.get("scratch").and_then(Json::as_bool) == Some(true),
            temporary: row.plan.get("temporary").and_then(Json::as_bool) == Some(true),
            native: row
                .plan
                .get("native")
                .filter(|native| native.is_object())
                .cloned(),
            location_known: row.plan.get("location_known").and_then(Json::as_bool) == Some(true),
        }
    }

    /// The runtime this removal takes away: the retired one it was made for, else the environment's
    /// current one.
    async fn target(&self) -> Result<Option<RuntimeFull>, Stop> {
        let mut conn = self
            .engine
            .inner
            .db
            .acquire()
            .await
            .map_err(EngineError::from)?;
        Ok(match self.runtime {
            Some(id) => rows::runtime(&mut conn, id).await?,
            None => rows::live_runtime(&mut conn, self.environment).await?,
        }
        .filter(|runtime| runtime.state != runtime_state::REMOVED))
    }

    /// Forgets what the engine holds in memory of the removed environment's browsers: their profile list and
    /// their tab and viewer counts, which no row keeps.
    async fn forget_browsers(&self) -> Result<(), Stop> {
        let browsers = {
            let mut conn = self
                .engine
                .inner
                .db
                .acquire()
                .await
                .map_err(EngineError::from)?;
            orch_db::queries::browsers::ids_of_environment(&mut conn, self.environment).await?
        };
        self.engine
            .browsers()
            .memory()
            .forget_environment(self.environment, &browsers);
        Ok(())
    }

    async fn ask(
        &self,
        job: &Job,
        runtime: &RuntimeFull,
        machine: Id,
        word: &[u8],
        command: Outgoing,
    ) -> Result<(), Stop> {
        let _ = (runtime, machine, word);
        match job.command(command).await {
            Ok(_) => Ok(()),
            Err(Stop::Failed(failure)) if failure.code == "not_found" => Ok(()),
            Err(other) => Err(other),
        }
    }

    async fn host_machine(&self, runtime: &RuntimeFull) -> Result<Option<Id>, Stop> {
        let mut conn = self
            .engine
            .inner
            .db
            .acquire()
            .await
            .map_err(EngineError::from)?;
        Ok(host_rows::host(&mut conn, runtime.host_id)
            .await?
            .map(|host| host.machine_id))
    }
}

#[async_trait]
impl Phased for Removal {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "credential" => {
                if self.runtime.is_some() {
                    return Ok(Done::Skipped(
                        "a retired unit holds no credential".to_owned(),
                    ));
                }
                let mut conn = self
                    .engine
                    .inner
                    .db
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                if rows::providers_holding(&mut conn, self.environment)
                    .await?
                    .is_empty()
                {
                    return Ok(Done::Skipped(
                        "no credential was handed over to it".to_owned(),
                    ));
                }
                drop(conn);
                // Brought home first (the part that keeps providers); one that cannot be stops the
                // removal, which a person mends by bringing it home or giving it up.
                self.engine
                    .return_credentials_of(self.environment)
                    .await
                    .map_err(|error| {
                        failed(
                            "credential_return_failed",
                            format!(
                                "a credential that was handed over to the environment has to be brought home, or given up, before it is removed: {error}"
                            ),
                        )
                    })?;
                Ok(Done::Finished)
            }
            "stop" => {
                if let Some(native) = &self.native {
                    let machine = native
                        .get("machine")
                        .and_then(Json::as_str)
                        .and_then(|id| id.parse::<Id>().ok())
                        .ok_or_else(|| {
                            failed(
                                "no_host",
                                "Reconnect the machine keeping this environment before cleanup",
                            )
                        })?;
                    let command = Outgoing::new(
                        ids::derived("env.detach", &[job.id().as_bytes(), b"removal"]),
                        machine,
                        SerialKey::Environment(self.environment),
                        &orch_link_contract::commands::EnvDetach {
                            environment_id: self.environment,
                        },
                    )
                    .map_err(|error| failed(INTERNAL, error.to_string()))?;
                    job.command(command).await?;
                    return Ok(Done::Finished);
                }
                let Some(runtime) = self.target().await? else {
                    if self.temporary && !self.location_known {
                        return Err(failed(
                            "location_unknown",
                            "Reconnect the environment's machine so its owned files can be located",
                        ));
                    }
                    return Ok(Done::Skipped("it has no unit".to_owned()));
                };
                let Some(machine) = self.host_machine(&runtime).await? else {
                    return Err(failed(
                        "no_host",
                        "the host keeping the files is unavailable",
                    ));
                };
                if !matches!(
                    runtime.state.as_str(),
                    runtime_state::RUNNING | runtime_state::STOPPING | runtime_state::CREATING
                ) {
                    return Ok(Done::Skipped("its unit is not running".to_owned()));
                }
                let command = Outgoing::new(
                    ids::derived(
                        "node.runtime.stop",
                        &[job.id().as_bytes().as_slice(), b"removal"],
                    ),
                    machine,
                    SerialKey::Runtime(runtime.id),
                    &NodeRuntimeStop {
                        runtime_id: runtime.id,
                        reason: Some(orch_driver_contract::runtime::UnitReason::Delete),
                    },
                )
                .map_err(|error| failed(INTERNAL, error.to_string()))?;
                self.ask(job, &runtime, machine, b"stop", command).await?;
                Ok(Done::Finished)
            }
            "unit" => {
                let Some(runtime) = self.target().await? else {
                    return Ok(Done::Skipped("it has no unit".to_owned()));
                };
                if self.runtime.is_none() && !self.scratch && !self.temporary {
                    self.engine.archive_unit_files(job, &runtime).await?;
                }
                let Some(machine) = self.host_machine(&runtime).await? else {
                    return Err(failed(
                        "no_host",
                        "the host keeping the files is unavailable",
                    ));
                };
                let command = Outgoing::new(
                    ids::derived(
                        "node.runtime.remove",
                        &[job.id().as_bytes().as_slice(), b"removal"],
                    ),
                    machine,
                    SerialKey::Runtime(runtime.id),
                    &NodeRuntimeRemove {
                        runtime_id: runtime.id,
                        keep_volumes: true,
                        volume_refs: Vec::new(),
                    },
                )
                .map_err(|error| failed(INTERNAL, error.to_string()))?;
                self.ask(job, &runtime, machine, b"unit", command).await?;
                Ok(Done::Finished)
            }
            "volumes" => {
                if self.temporary
                    && let Some(native) = &self.native
                {
                    let machine = native
                        .get("machine")
                        .and_then(Json::as_str)
                        .and_then(|id| id.parse::<Id>().ok())
                        .ok_or_else(|| {
                            failed(
                                "no_host",
                                "Reconnect the machine keeping this environment before cleanup",
                            )
                        })?;
                    let generation =
                        native
                            .get("generation")
                            .and_then(Json::as_i64)
                            .ok_or_else(|| {
                                failed(INTERNAL, "The owned data generation was not recorded")
                            })?;
                    let command = Outgoing::new(
                        ids::derived("env.data.remove", &[job.id().as_bytes(), b"removal"]),
                        machine,
                        SerialKey::Environment(self.environment),
                        &orch_link_contract::commands::EnvDataRemove {
                            environment_id: self.environment,
                            data_generation: generation,
                        },
                    )
                    .map_err(|error| failed(INTERNAL, error.to_string()))?;
                    job.command(command).await?;
                    return Ok(Done::Finished);
                }
                if self.runtime.is_none() && !self.scratch {
                    return Ok(Done::Skipped(
                        "owned files stay for the frozen removal window".to_owned(),
                    ));
                }
                let Some(runtime) = self.target().await? else {
                    return Ok(Done::Skipped("it has no volumes".to_owned()));
                };
                let Some(machine) = self.host_machine(&runtime).await? else {
                    return Err(failed(
                        "no_host",
                        "the host keeping the files is unavailable",
                    ));
                };
                let roles: Vec<_> = runtime
                    .volumes
                    .as_object()
                    .into_iter()
                    .flat_map(|v| v.iter())
                    .collect();
                for (index, (role, volume)) in roles.iter().enumerate() {
                    if volume.get("directory").is_some_and(Json::is_object) {
                        continue;
                    }
                    let Some(name) = volume["name"].as_str() else {
                        continue;
                    };
                    let in_use = {
                        let mut conn = self
                            .engine
                            .inner
                            .db
                            .acquire()
                            .await
                            .map_err(EngineError::from)?;
                        self.runtime.is_some()
                            && rows::volume_in_use(&mut conn, runtime.host_id, name, runtime.id)
                                .await?
                    };
                    if in_use {
                        continue;
                    }
                    let command = Outgoing::new(
                        ids::derived(
                            "node.volume.remove",
                            &[job.id().as_bytes().as_slice(), role.as_bytes()],
                        ),
                        machine,
                        SerialKey::Volume(name.to_owned()),
                        &NodeVolumeRemove {
                            driver: runtime.driver.clone(),
                            name: name.to_owned(),
                            storage: volume["storage"].as_str().map(str::to_owned),
                            labels: BTreeMap::new(),
                        },
                    )
                    .map_err(|error| failed(INTERNAL, error.to_string()))?;
                    self.ask(job, &runtime, machine, role.as_bytes(), command)
                        .await?;
                    job.progress((index + 1) as f64 / roles.len() as f64, None)
                        .await?;
                }
                Ok(Done::Finished)
            }
            "forget" => {
                job.pass_commit().await?;
                let target = self.target().await?;
                let origin: Origin = job
                    .plan()
                    .await?
                    .get("origin")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                let now = job.now();
                let mut tx = self
                    .engine
                    .inner
                    .db
                    .begin()
                    .await
                    .map_err(EngineError::from)?;
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
                if let Some(runtime) = target {
                    rows::set_runtime_state(
                        &mut tx,
                        runtime.id,
                        &[
                            runtime_state::CREATING,
                            runtime_state::RUNNING,
                            runtime_state::STOPPING,
                            runtime_state::STOPPED,
                            runtime_state::FAILED,
                            runtime_state::RETIRED,
                        ],
                        runtime_state::REMOVED,
                        None,
                        now,
                    )
                    .await?;
                    host_rows::revoke_machine_of(&mut tx, runtime.id, now).await?;
                    host_rows::delete_runtime_secret(&mut tx, runtime.id, "enroll_token").await?;
                }
                if self.runtime.is_none() {
                    rows::close_shells(&mut tx, self.environment, now).await?;
                    if self.scratch {
                        retained::discard_scratch(&mut tx, self.environment, now).await?;
                        if self.temporary {
                            orch_db::queries::environments::temporary::forget(
                                &mut tx,
                                self.environment,
                            )
                            .await?;
                        }
                        orch_db::queries::jobs::stamp_environment(&mut tx, self.environment, now)
                            .await?;
                        tx.commit().await.map_err(DbError::from)?;
                        self.forget_browsers().await?;
                        return Ok(Done::Finished);
                    }
                    let promise = retained::removal(&mut tx, self.environment)
                        .await?
                        .ok_or_else(|| failed("environment_gone", "the environment is gone"))?;
                    if retained::keep_removed(
                        &mut tx,
                        self.environment,
                        promise.removal_revision,
                        now,
                    )
                    .await?
                    {
                        if self.temporary {
                            orch_db::queries::environments::temporary::forget(
                                &mut tx,
                                self.environment,
                            )
                            .await?;
                        }
                        let env = rows::environment(&mut tx, self.environment)
                            .await?
                            .ok_or_else(|| failed("environment_gone", "the environment is gone"))?;
                        let summary = if self.temporary {
                            format!(
                                "{} was removed without keeping a recovery copy of its owned files",
                                env.name
                            )
                        } else {
                            format!(
                                "The unit of {} was removed; its owned files are kept",
                                env.name
                            )
                        };
                        let mut entry = Entry::new(
                            promise.deleted_by.map_or(Actor::System, Actor::User),
                            audit::name!("environment.deleted"),
                            "environment",
                            Some(self.environment),
                            &summary,
                        );
                        entry.history = true;
                        entry.node_id = Some(env.home_node_id);
                        entry.org_id = Some(env.org_id);
                        entry.environment_id = Some(self.environment);
                        let actual = retained::removal(&mut tx, self.environment).await?;
                        entry.details = serde_json::json!({"bytes":promise.retained_bytes,"purge_at":actual.and_then(|r|r.volumes_purge_at)});
                        audit::append(&mut tx, &entry, &origin, now).await?;
                    }
                }
                orch_db::queries::jobs::stamp_environment(&mut tx, self.environment, now).await?;
                tx.commit().await.map_err(DbError::from)?;
                if self.runtime.is_none() {
                    self.forget_browsers().await?;
                }
                Ok(Done::Finished)
            }
            other => Err(failed(INTERNAL, format!("a removal has no phase {other}"))),
        }
    }

    /// A removal is not undone: what it took away stays away. What a failure does is date the next
    /// attempt, after a pause that grows, and say why on the environment and on the runtime.
    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        let Stop::Failed(failure) = why else {
            return Undone::Everything;
        };
        let now = job.now();
        let config = self.engine.inner.config.hosting;
        let Ok(mut conn) = self.engine.inner.db.acquire().await else {
            return Undone::Partly("the database could not be reached".to_owned());
        };
        let failures = rows::failed_removals(&mut conn, self.environment, self.runtime)
            .await
            .unwrap_or(0)
            + 1;
        let pause = pause_after(
            failures,
            config.removal_first_pause,
            config.reaper_retry_max,
        );
        let again = now + chrono::Duration::from_std(pause).unwrap_or_default();
        let detail = format!(
            "removing it failed ({} times): {}; it is tried again at {}",
            failures,
            failure.message,
            again.format("%Y-%m-%dT%H:%M:%SZ")
        );
        match self.runtime {
            Some(runtime) => {
                let _ = rows::postpone_removal(&mut conn, runtime, &detail, again, now).await;
            }
            None => {
                let _ = rows::postpone_environment_removal(
                    &mut conn,
                    self.environment,
                    &detail,
                    again,
                    now,
                )
                .await;
                if let Ok(Some(runtime)) = rows::live_runtime(&mut conn, self.environment).await {
                    let _ =
                        rows::postpone_removal(&mut conn, runtime.id, &detail, again, now).await;
                }
            }
        }
        Undone::Everything
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pause_between_two_attempts_to_remove_doubles_up_to_the_longest() {
        let first = Duration::from_secs(30);
        let longest = Duration::from_secs(3_600);
        let pauses: Vec<u64> = (1..=9)
            .map(|failures| pause_after(failures, first, longest).as_secs())
            .collect();
        assert_eq!(pauses, [30, 60, 120, 240, 480, 960, 1_920, 3_600, 3_600]);
        assert_eq!(pause_after(1_000, first, longest), longest, "no overflow");
        assert_eq!(pause_after(0, first, longest), first);
    }
}
