//! Frozen template-first after-start commands and exactly one output-free history line per outcome.

use std::collections::{BTreeMap, BTreeSet};

use orch_access_contract::audit::AuditAction;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::{
    environments::hosting as envs,
    jobs,
    resources::{hosting as hosts, provider_runtime as providers},
};
use orch_link_contract::commands::{SerialKey, UnitAfterUp, UnitAfterUpResult};
use orch_settings::{Place, get};
use orch_settings_contract::{
    registry::keys,
    unit::{AfterUpCommand, NamedAfterUpCommand, setting_commands, template_commands},
};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::runner::{Done, Failure, Job, Stop, Undone};
use crate::{EngineError, commands::Outgoing, ids};

#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    commands: Vec<NamedAfterUpCommand>,
    #[serde(default)]
    recorded: BTreeSet<u32>,
    #[serde(default)]
    started: BTreeSet<u32>,
    #[serde(default)]
    began: BTreeMap<u32, Time>,
    #[serde(default)]
    finished: bool,
}

fn failure(message: impl Into<String>) -> Stop {
    Failure::new("after_up_failed", message).into()
}

/// The same visible reason on a unit start and on a directory attach.
pub(crate) const MACHINE_POLICY_OFF: &str =
    "environment.after_up: commands after start are kept off by this machine";

/// A directory's command outcome uses the same visible history shape as the unit job phase.
/// The run/position lock is held only while recording; no command runs inside this transaction.
pub(crate) async fn record_direct(
    engine: &crate::Engine,
    body: &UnitAfterUp,
    result: Option<&UnitAfterUpResult>,
) -> Result<(), EngineError> {
    let (kind, id) = match (body.environment_id, body.provider_id) {
        (Some(id), None) => ("environment", id),
        (None, Some(id)) => ("provider", id),
        _ => {
            return Err(EngineError::unreadable(
                "a command history line needs one environment or provider",
            ));
        }
    };
    let mut tx = engine.db().begin().await?;
    orch_db::queries::chats::lock_named(
        &mut tx,
        &format!("after_up:{}:{}", body.run_id, body.position),
    )
    .await?;
    let identity = json!({"run_id":body.run_id,"position":body.position});
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE object_kind=$1 AND object_id=$2 AND action='unit.command_ran' AND details @> $3)")
        .bind(kind).bind(id).bind(identity).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
    if exists {
        return Ok(());
    }
    let (node, org) = if kind == "environment" {
        let state = envs::environment(&mut tx, id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the command's environment is gone"))?;
        (Some(state.home_node_id), Some(state.org_id))
    } else {
        let state = providers::state(&mut tx, id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the command's provider is gone"))?;
        (Some(state.home_id), Some(state.org_id))
    };
    let summary = format!(
        "After-start command {} ended with {}",
        body.name,
        result
            .and_then(|result| result.exit)
            .map_or_else(|| "no confirmed exit".into(), |exit| format!("exit {exit}"))
    );
    let mut entry = Entry::new(
        Actor::System,
        audit::action!(AuditAction::UnitCommandRan).as_str(),
        kind,
        Some(id),
        &summary,
    );
    entry.history = true;
    entry.node_id = node;
    entry.org_id = org;
    entry.environment_id = body.environment_id;
    entry.details = json!({"run_id":body.run_id,"position":body.position,"name":body.name,"exit":result.and_then(|result|result.exit),
        "duration_ms":result.map(|result|result.duration_ms),"timed_out":result.is_some_and(|result|result.timed_out)});
    audit::append(
        &mut tx,
        &entry,
        &Origin::default(),
        engine.inner.clock.now(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Admission is shared by unit jobs and directory attachments before a command is sent.
pub(crate) fn admission(
    machine: &orch_db::queries::environment_facts::EnvironmentMachine,
) -> Result<bool, String> {
    if machine
        .policy
        .as_ref()
        .and_then(|p| p.get("environment.after_up"))
        .and_then(Json::as_bool)
        != Some(true)
    {
        return Ok(false);
    }
    if machine
        .descriptor
        .as_ref()
        .and_then(|d| d.pointer("/commands_after_start/supported"))
        .and_then(Json::as_bool)
        != Some(true)
    {
        return Err(machine
            .descriptor
            .as_ref()
            .and_then(|d| d.pointer("/commands_after_start/reason"))
            .and_then(Json::as_str)
            .unwrap_or("this runtime cannot enforce an after-start command's process group")
            .into());
    }
    Ok(true)
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn a_machine_policy_denial_skips_commands_and_platform_support_is_checked_identically() {
        for lifecycle in ["attach", "unit"] {
            let mut facts = orch_db::queries::environment_facts::EnvironmentMachine {
                machine_id: None,
                descriptor: Some(
                    json!({"lifecycle":lifecycle,"commands_after_start":{"supported":true,"reason":null}}),
                ),
                policy: Some(json!({"environment.after_up":false})),
            };
            assert_eq!(admission(&facts), Ok(false));
            facts.policy = Some(json!({"environment.after_up":true}));
            assert_eq!(admission(&facts), Ok(true));
            facts.descriptor = Some(
                json!({"lifecycle":lifecycle,"commands_after_start":{"supported":false,"reason":"process groups are unavailable"}}),
            );
            assert_eq!(
                admission(&facts),
                Err("process groups are unavailable".into())
            );
        }
    }
}

/// Each layer sorts by name independently; a scoped null removes only a scoped command.
pub(crate) fn commands(template: &Json, scoped: &Json) -> Result<Vec<NamedAfterUpCommand>, String> {
    let mut commands = template_commands(template)?;
    commands.extend(
        setting_commands(scoped)?
            .into_iter()
            .filter_map(|(name, run)| run.map(|run| NamedAfterUpCommand::new(name, run))),
    );
    Ok(commands)
}

/// Runs before ready/commit, including verification of a job queued by an older process.
pub(super) async fn run(job: &Job) -> Result<Done, Stop> {
    run_on(job, None, false).await
}

async fn run_on(job: &Job, restored: Option<Id>, undo: bool) -> Result<Done, Stop> {
    let row = job.row().await?;
    if !undo && row.plan.get("after_up_reused").and_then(Json::as_bool) == Some(true) {
        return Ok(Done::Skipped(
            "the running runtime was reused; it has no new unit run".into(),
        ));
    }
    let mut tx = job.engine().db().begin().await?;
    let row = jobs::lock(&mut tx, row.id)
        .await?
        .ok_or_else(|| failure("the start job is gone"))?;
    let (place, object, id, node, org, live, key, profile) =
        match (row.environment_id, row.provider_id) {
            (Some(id), None) => {
                let state = envs::environment(&mut tx, id)
                    .await?
                    .ok_or_else(|| failure("the environment is gone"))?;
                (
                    Place::Environment(id),
                    "environment",
                    id,
                    state.home_node_id,
                    state.org_id,
                    envs::live_runtime(&mut tx, id).await?,
                    keys::ENV_AFTER_UP,
                    None,
                )
            }
            (None, Some(id)) => {
                let state = providers::state(&mut tx, id)
                    .await?
                    .ok_or_else(|| failure("the provider is gone"))?;
                (
                    Place::Provider(id),
                    "provider",
                    id,
                    state.home_id,
                    state.org_id,
                    providers::live_runtime(&mut tx, id).await?,
                    keys::RUNTIME_AFTER_UP,
                    state.agent_id,
                )
            }
            _ => return Err(failure("a start command needs one environment or provider")),
        };
    // A directory is attached instead of a unit started: at the destination of a move that attaches a
    // machine's directory, and when a kept registration is restored on its own machine. Either runs the
    // directory's resolved commands on that machine, with no unit.
    let attaching = !undo
        && (row
            .plan
            .pointer("/portable/destination/lifecycle")
            .and_then(Json::as_str)
            == Some("attach")
            || (row.kind == jobs::retained::RESTORE
                && row.plan.get("connected").and_then(Json::as_bool) == Some(true)));
    let runtime = if attaching {
        None
    } else {
        Some(
            match restored.or_else(|| {
                row.plan
                    .get("runtime")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
            }) {
                Some(id) => envs::runtime(&mut tx, id).await?,
                None => live,
            }
            .ok_or_else(|| failure("the starting unit is gone"))?,
        )
    };
    if runtime.as_ref().is_some_and(|runtime| {
        runtime.environment_id != row.environment_id || runtime.provider_id != row.provider_id
    }) {
        return Err(failure("the starting unit belongs to a different resource"));
    }
    let identity = runtime.as_ref().map_or(id, |runtime| runtime.id);
    let run_id = ids::derived(
        if undo {
            "unit.after_up.rollback"
        } else {
            "unit.after_up.run"
        },
        &[job.id().as_bytes(), identity.as_bytes()],
    );
    let run_key = run_id.to_string();
    let mut runs: BTreeMap<String, Saved> = serde_json::from_value(
        row.plan
            .get("after_up_runs")
            .cloned()
            .unwrap_or_else(|| json!({})),
    )
    .map_err(EngineError::from)?;
    let saved = if let Some(saved) = runs.get(&run_key) {
        saved.clone()
    } else {
        let definition = if let Some(runtime) = &runtime {
            let template = hosts::template(&mut tx, runtime.template_id)
                .await?
                .ok_or_else(|| failure("the starting template is gone"))?;
            let recorded = jobs::changes::definition(&mut tx, runtime.id).await?;
            if undo {
                recorded
                    .as_ref()
                    .and_then(|definition| definition.get("spec"))
                    .unwrap_or(&template.spec)
                    .clone()
            } else {
                row.plan
                    .get("definition")
                    .or_else(|| {
                        row.plan
                            .get("created_spec")
                            .and_then(|spec| spec.get("spec"))
                    })
                    .or_else(|| {
                        recorded
                            .as_ref()
                            .and_then(|definition| definition.get("spec"))
                    })
                    .unwrap_or(&template.spec)
                    .clone()
            }
        } else {
            json!({})
        };
        // A guarded restore reads this exact retained environment before publication. Do not
        // undelete it early or let ordinary start jobs resolve arbitrary removed places.
        let scoped: Json = if row.kind == jobs::retained::RESTORE && row.environment_id == Some(id)
        {
            orch_db::queries::trace::read_scope(&[id], get(&mut tx, place, key)).await
        } else {
            get(&mut tx, place, key).await
        }
        .map_err(EngineError::from)?
        .unwrap_or_else(|| json!({}));
        let saved = Saved {
            commands: commands(&definition, &scoped).map_err(failure)?,
            recorded: BTreeSet::new(),
            started: BTreeSet::new(),
            began: BTreeMap::new(),
            finished: false,
        };
        runs.insert(run_key.clone(), saved.clone());
        let cleanup_runtime = if undo {
            row.plan
                .get("after_up_runtime")
                .cloned()
                .unwrap_or(Json::Null)
        } else {
            json!(runtime.as_ref().map(|runtime| runtime.id))
        };
        jobs::merge_plan(&mut tx,job.id(),&json!({"after_up_runs":runs,"after_up_runtime":cleanup_runtime,"after_up_started":row.plan.get("after_up_started").and_then(Json::as_bool)==Some(true) || !saved.commands.is_empty()}),job.now()).await?;
        saved
    };
    let machine = if saved.commands.is_empty() || saved.finished {
        None
    } else {
        let (physical, descriptor, recipient) = if let Some(runtime) = &runtime {
            let host = hosts::host(&mut tx, runtime.host_id)
                .await?
                .ok_or_else(|| failure("the hosting machine is gone"))?;
            let recipient = hosts::machine_of_runtime(&mut tx, runtime.id)
                .await?
                .filter(|(_, online)| *online)
                .ok_or_else(|| {
                    failure("the unit's connector is unavailable for commands after start")
                })?
                .0;
            (
                host.machine_id,
                host.drivers.get(&runtime.driver).cloned(),
                recipient,
            )
        } else {
            // A move names the machine it attaches; a restored registration, the machine it is kept on.
            let machine: Id = serde_json::from_value(
                row.plan
                    .get("candidate_machine")
                    .or_else(|| row.plan.get("connected_machine"))
                    .cloned()
                    .unwrap_or(Json::Null),
            )
            .map_err(EngineError::from)?;
            (
                machine,
                orch_db::queries::environment_facts::on_machine(&mut tx, machine, None).await?,
                machine,
            )
        };
        let mut facts = match row.environment_id {
            Some(id) => orch_db::queries::environment_facts::of(&mut tx, id).await?,
            None => orch_db::queries::environment_facts::of_provider(&mut tx, id).await?,
        }
        .filter(|facts| facts.machine_id == Some(physical));
        if facts.is_none() {
            facts = orch_db::queries::environment_facts::machine(&mut tx, physical, None).await?;
        }
        let mut facts =
            facts.ok_or_else(|| failure("the hosting machine has not reported its services"))?;
        facts.descriptor = descriptor;
        if !admission(&facts).map_err(failure)? {
            return Ok(Done::Skipped(MACHINE_POLICY_OFF.into()));
        }
        Some(recipient)
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if saved.finished {
        return Ok(Done::Finished);
    }
    if saved.commands.is_empty() {
        return Ok(Done::Skipped("no commands are set after start".into()));
    }
    let machine = machine.ok_or_else(|| failure("the unit's connector is unavailable"))?;
    let total = u32::try_from(saved.commands.len())
        .map_err(|_| failure("there are too many commands in this unit run"))?;
    for (position, entry) in saved.commands.into_iter().enumerate() {
        let position = u32::try_from(position)
            .map_err(|_| failure("there are too many commands in this unit run"))?;
        if !undo {
            job.progress(
                f64::from(position) / f64::from(total),
                Some(&format!("running after-start command {}", entry.name)),
            )
            .await?;
        }
        let body = UnitAfterUp {
            run_id,
            environment_id: row.environment_id,
            provider_id: row.provider_id,
            agent_id: if runtime.is_none() { profile } else { None },
            position,
            name: entry.name,
            run: AfterUpCommand {
                command: entry.command,
                timeout_secs: entry.timeout_secs,
            },
        };
        // A cancel or lost final reply still leaves an honest, output-free attempted-run history line.
        let mut tx = job.engine().db().begin().await?;
        let locked = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| failure("the start job is gone"))?;
        let mut runs: BTreeMap<String, Saved> =
            serde_json::from_value(locked.plan["after_up_runs"].clone())
                .map_err(EngineError::from)?;
        let saved = runs
            .get_mut(&run_key)
            .ok_or_else(|| failure("the unit run's command plan is gone"))?;
        saved.started.insert(position);
        saved.began.entry(position).or_insert(Time::new(job.now()));
        jobs::merge_plan(&mut tx, job.id(), &json!({"after_up_runs":runs}), job.now()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let outgoing = Outgoing::new(
            ids::derived(
                "unit.after_up",
                &[run_id.as_bytes(), &position.to_be_bytes()],
            ),
            machine,
            runtime
                .as_ref()
                .map_or(SerialKey::Environment(id), |runtime| {
                    SerialKey::Runtime(runtime.id)
                }),
            &body,
        )
        .map_err(EngineError::from)?;
        let outcome = if undo {
            job.command_to_undo(outgoing).await
        } else {
            job.command(outgoing).await
        };
        if matches!(&outcome,Err(Stop::Failed(why)) if why.code=="unsupported" && why.message.starts_with("environment.after_up (environment role):"))
        {
            let mut tx = job.engine().db().begin().await?;
            let locked = jobs::lock(&mut tx, job.id())
                .await?
                .ok_or_else(|| failure("the start job is gone"))?;
            let mut runs: BTreeMap<String, Saved> =
                serde_json::from_value(locked.plan["after_up_runs"].clone())
                    .map_err(EngineError::from)?;
            if let Some(saved) = runs.get_mut(&run_key) {
                saved.started.remove(&position);
                saved.finished = true;
            }
            jobs::merge_plan(&mut tx, job.id(), &json!({"after_up_runs":runs}), job.now()).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(Done::Skipped(MACHINE_POLICY_OFF.into()));
        }
        let value = outcome?;
        let result: UnitAfterUpResult = serde_json::from_value(value).map_err(EngineError::from)?;
        if result.name != body.name {
            return Err(failure(
                "the unit answered for a different after-start command",
            ));
        }
        // A replay can return the result again. History and its durable marker commit together.
        let mut tx = job.engine().db().begin().await?;
        let locked = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| failure("the start job is gone"))?;
        let mut runs: BTreeMap<String, Saved> =
            serde_json::from_value(locked.plan["after_up_runs"].clone())
                .map_err(EngineError::from)?;
        let saved = runs
            .get_mut(&run_key)
            .ok_or_else(|| failure("the unit run's command plan is gone"))?;
        if saved.recorded.insert(position) {
            let summary = format!(
                "After-start command {} ended with exit {}",
                result.name,
                result
                    .exit
                    .map_or_else(|| "unavailable".into(), |exit| exit.to_string())
            );
            let mut entry = Entry::new(
                row.created_by.map_or(Actor::System, Actor::User),
                audit::action!(AuditAction::UnitCommandRan).as_str(),
                object,
                Some(id),
                &summary,
            );
            entry.history = true;
            entry.node_id = Some(node);
            entry.org_id = Some(org);
            entry.environment_id = row.environment_id;
            entry.details = json!({"name":result.name,"exit":result.exit,"duration_ms":result.duration_ms,"timed_out":result.timed_out});
            let origin: Origin = serde_json::from_value(
                row.plan.get("origin").cloned().unwrap_or_else(|| json!({})),
            )
            .unwrap_or_default();
            audit::append(&mut tx, &entry, &origin, job.now()).await?;
            jobs::merge_plan(&mut tx, job.id(), &json!({"after_up_runs":runs}), job.now()).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if !result.passed() {
            let why = if result.timed_out {
                "timed out; its process group was killed".into()
            } else {
                result.exit.map_or_else(
                    || "could not run or was interrupted".into(),
                    |exit| format!("exited with {exit}"),
                )
            };
            let lines = result
                .first_lines
                .lines()
                .take(10)
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(1000)
                .collect::<String>();
            return Err(failure(format!(
                "after-start command {} {why}: {lines}",
                result.name
            )));
        }
    }
    let mut tx = job.engine().db().begin().await?;
    let locked = jobs::lock(&mut tx, job.id())
        .await?
        .ok_or_else(|| failure("the start job is gone"))?;
    let mut runs: BTreeMap<String, Saved> =
        serde_json::from_value(locked.plan["after_up_runs"].clone()).map_err(EngineError::from)?;
    runs.get_mut(&run_key)
        .ok_or_else(|| failure("the unit run's command plan is gone"))?
        .finished = true;
    jobs::merge_plan(&mut tx, job.id(), &json!({"after_up_runs":runs}), job.now()).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Done::Finished)
}

/// A cancelled command may have had effects without returning its final exit. Never invent success.
pub(super) async fn record_interrupted(job: &Job) -> Result<(), Stop> {
    if job.plan().await?.get("after_up_runs").is_none() {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await?;
    let row = jobs::lock(&mut tx, job.id())
        .await?
        .ok_or_else(|| failure("the start job is gone"))?;
    let mut runs: BTreeMap<String, Saved> =
        serde_json::from_value(row.plan["after_up_runs"].clone()).map_err(EngineError::from)?;
    let (object, id, node, org) = match (row.environment_id, row.provider_id) {
        (Some(id), None) => {
            let state = envs::environment(&mut tx, id)
                .await?
                .ok_or_else(|| failure("the environment is gone"))?;
            ("environment", id, state.home_node_id, state.org_id)
        }
        (None, Some(id)) => {
            let state = providers::state(&mut tx, id)
                .await?
                .ok_or_else(|| failure("the provider is gone"))?;
            ("provider", id, state.home_id, state.org_id)
        }
        _ => {
            return Err(failure(
                "a command attempt needs one environment or provider",
            ));
        }
    };
    for saved in runs.values_mut() {
        for position in &saved.started {
            if !saved.recorded.insert(*position) {
                continue;
            }
            let name = &saved
                .commands
                .get(
                    usize::try_from(*position)
                        .map_err(|_| failure("the saved command position is invalid"))?,
                )
                .ok_or_else(|| failure("the saved command position is missing"))?
                .name;
            let summary = format!("After-start command {name} ended without a confirmed exit");
            let mut entry = Entry::new(
                row.created_by.map_or(Actor::System, Actor::User),
                audit::action!(AuditAction::UnitCommandRan).as_str(),
                object,
                Some(id),
                &summary,
            );
            entry.history = true;
            entry.node_id = Some(node);
            entry.org_id = Some(org);
            entry.environment_id = row.environment_id;
            let duration_ms = saved
                .began
                .get(position)
                .map_or(0, |at| (job.now() - at.get()).num_milliseconds().max(0));
            entry.details =
                json!({"name":name,"exit":null,"duration_ms":duration_ms,"timed_out":false});
            let origin: Origin = serde_json::from_value(
                row.plan.get("origin").cloned().unwrap_or_else(|| json!({})),
            )
            .unwrap_or_default();
            audit::append(&mut tx, &entry, &origin, job.now()).await?;
        }
    }
    jobs::merge_plan(&mut tx, job.id(), &json!({"after_up_runs":runs}), job.now()).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Cleanup uses the frozen unit identity, even when its template, variables or host switch changed.
pub(super) async fn stop_failed(job: &Job, why: &Stop) -> Undone {
    let result = async {
        let row = job.row().await?;
        let runtime = row
            .plan
            .get("after_up_runtime")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
            .ok_or_else(|| failure("the after-start unit identity was not retained"))?;
        let mut conn = job.engine().db().acquire().await?;
        let unit = envs::runtime(&mut conn, runtime)
            .await?
            .ok_or_else(|| failure("the after-start unit is gone"))?;
        if unit.environment_id != row.environment_id || unit.provider_id != row.provider_id {
            return Err(failure(
                "the after-start unit belongs to a different resource",
            ));
        }
        let host = hosts::host(&mut conn, unit.host_id)
            .await?
            .ok_or_else(|| failure("the after-start host is gone"))?;
        drop(conn);
        let stopped = job
            .command_to_undo(
                Outgoing::new(
                    ids::derived(
                        "node.runtime.stop.after_up",
                        &[job.id().as_bytes(), runtime.as_bytes()],
                    ),
                    host.machine_id,
                    SerialKey::Runtime(runtime),
                    &orch_link_contract::commands::NodeRuntimeStop {
                        runtime_id: runtime,
                        reason: None,
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await;
        let unconfirmed = match stopped {
            Ok(_) => false,
            Err(Stop::Failed(failed)) if failed.code == "not_found" => false,
            _ => true,
        };
        let (to, detail) = match why {
            Stop::Cancelled => (
                "stopped",
                "the start was cancelled; its same unit and files were kept".to_owned(),
            ),
            Stop::Failed(failed) => ("failed", failed.message.clone()),
            Stop::Lost => return Err(Stop::Lost),
        };
        let detail = if unconfirmed {
            format!("{detail}; the unit's stop could not be confirmed")
        } else {
            detail
        };
        let mut tx = job.engine().db().begin().await?;
        envs::set_runtime_state(
            &mut tx,
            runtime,
            &envs::runtime_state::LIVE,
            if unconfirmed { "failed" } else { to },
            Some(&detail),
            job.now(),
        )
        .await?;
        if let Some(environment) = row.environment_id {
            envs::set_status(
                &mut tx,
                environment,
                &["provisioning", "running", "offline", "failed"],
                if unconfirmed { "failed" } else { to },
                Some(&detail),
                job.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok::<_, Stop>(unconfirmed)
    }
    .await;
    match result {
        Ok(false) => Undone::Everything,
        Ok(true) => {
            Undone::Partly("the unit's stop is unconfirmed; every file and volume was kept".into())
        }
        Err(_) => Undone::Partly(
            "the failed start could not confirm its unit stopped; no file or volume was removed"
                .into(),
        ),
    }
}

/// A rollback also starts a unit. Attach its old side before commands and never resume work on failure.
pub(super) async fn restored(job: &Job, runtime: Id) -> Result<(), Stop> {
    let result = async {
        let began = tokio::time::Instant::now();
        let machine = loop {
            let found =
                hosts::machine_of_runtime(&mut *job.engine().db().acquire().await?, runtime)
                    .await?;
            if let Some((machine, true)) = found {
                break machine;
            }
            if began.elapsed() >= job.engine().inner.config.hosting.enroll_timeout {
                return Err(failure(
                    "the restored unit's connector did not return before its enrollment deadline",
                ));
            }
            if job.engine().is_stopped() {
                return Err(Stop::Lost);
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        };
        if let Some(environment) = job.row().await?.environment_id {
            let with = crate::environments::attach_with(
                &mut *job.engine().db().acquire().await?,
                environment,
                job.now(),
            )
            .await?;
            let body = crate::environments::attach_body(environment, None, &with);
            job.command_to_undo(
                Outgoing::new(
                    ids::derived(
                        "env.attach.rollback",
                        &[job.id().as_bytes(), runtime.as_bytes()],
                    ),
                    machine,
                    SerialKey::Environment(environment),
                    &body,
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        }
        run_on(job, Some(runtime), true).await?;
        Ok::<(), Stop>(())
    }
    .await;
    if result.is_err() {
        if record_interrupted(job).await.is_err() {
            tracing::warn!("a restored command's interrupted outcome could not be recorded");
        }
        let mut conn = job.engine().db().acquire().await?;
        if let Some(unit) = envs::runtime(&mut conn, runtime).await?
            && let Some(host) = hosts::host(&mut conn, unit.host_id).await?
        {
            drop(conn);
            job.command_to_undo(
                Outgoing::new(
                    ids::derived(
                        "node.runtime.stop.after_up.rollback",
                        &[job.id().as_bytes(), runtime.as_bytes()],
                    ),
                    host.machine_id,
                    SerialKey::Runtime(runtime),
                    &orch_link_contract::commands::NodeRuntimeStop {
                        runtime_id: runtime,
                        reason: None,
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
            envs::set_runtime_state(
                &mut *job.engine().db().acquire().await?,
                runtime,
                &envs::runtime_state::LIVE,
                "failed",
                Some("the restored unit's after-start commands failed"),
                job.now(),
            )
            .await?;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn template_layer_runs_first_and_scoped_names_order_and_remove_only_their_layer() {
        let commands=commands(&json!({"after_up":[{"name":"90-template","command":["template"]}],"init":{"command":["old"]}}),
            &json!({"20-b":{"command":["b"]},"10-a":{"command":["a"]},"90-template":null})).unwrap();
        assert_eq!(
            commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            ["90-template", "init", "10-a", "20-b"]
        );
    }
}
