//! Exact accepted capability removal owns its bounded native replacement and cleanup.

use super::*;
use crate::{commands::CommandAuthority, jobs::command_identity};
use orch_authz::{Caller, CapturedOperation};
use orch_link_contract::commands::CommandKind;
use orch_types::{Clock, Hash, Time};
use serde::{Deserialize, Serialize};

const FIELD: &str = "capability_revocation";
const REQUIRED: &str = "capability_revocation_required";
const JOURNAL: &str = "capability_recreation_commands";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Custody {
    origin: CapturedOperation,
    job: Id,
    environment: Id,
    old: Id,
    host: Id,
    machine: Id,
    generation: i32,
    capability: String,
    desired: Vec<String>,
    recipe: Hash,
}
pub(crate) fn applies(row: &jobs::JobRow) -> bool {
    row.plan.get(FIELD).is_some() || row.plan.get(REQUIRED).is_some()
}
fn fingerprint(recipe: &Recipe) -> Result<Hash, EngineError> {
    let mut recipe = recipe.clone();
    // The runner records its selected physical Host in this same existing plan member.
    // This accepted Environment replacement always stays on the actual old Host.
    recipe.host = None;
    command_identity::hash(&serde_json::to_value(recipe)?)
}
fn custody(row: &jobs::JobRow) -> Result<Option<Custody>, EngineError> {
    let Some(value) = row.plan.get(FIELD) else {
        return Ok(None);
    };
    let Ok(custody) = serde_json::from_value::<Custody>(value.clone()) else {
        return Ok(None);
    };
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    if row.plan.get(REQUIRED) != Some(&json!(true))
        || custody.origin.operation() != "EnvironmentCapabilityRevoke"
        || custody.job != row.id
        || Some(custody.origin.user()) != row.created_by
        || row.kind != jobs::kind::ENVIRONMENT_RECREATE
        || row.environment_id != Some(custody.environment)
        || row.provider_id.is_some()
        || recipe.target != Target::Environment(custody.environment)
        || recipe.old != custody.old
        || recipe.by != custody.origin.user()
        || recipe.capability.as_deref() != Some(custody.capability.as_str())
        || recipe.host.is_some_and(|host| host != custody.host)
        || fingerprint(&recipe)? != custody.recipe
    {
        return Ok(None);
    }
    Ok(Some(custody))
}
/// Journal with the accepted revocation on its existing native transaction.
pub(crate) async fn record(
    conn: &mut Conn,
    row: &mut jobs::JobRow,
    caller: &Caller,
    capability: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let Some(operation) = caller.admitted_operation() else {
        if caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some())
        {
            return Err(EngineError::unreadable(
                "the capability replacement has no accepted origin",
            ));
        }
        return Ok(());
    };
    if operation != "EnvironmentCapabilityRevoke" {
        return Ok(());
    }
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    let Target::Environment(environment) = recipe.target else {
        return Err(EngineError::unreadable(
            "capability removal changed its target",
        ));
    };
    let env = envs::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("capability environment disappeared"))?;
    let runtime = envs::runtime(conn, recipe.old)
        .await?
        .ok_or_else(|| EngineError::unreadable("capability runtime disappeared"))?;
    let host = hosts::host(conn, runtime.host_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("capability host disappeared"))?;
    let mut desired = env.granted_capabilities.clone();
    desired.sort();
    desired.dedup();
    if desired.iter().any(|key| key == capability) {
        return Err(EngineError::unreadable(
            "the accepted capability removal was not recorded",
        ));
    }
    let custody = Custody {
        origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
        job: row.id,
        environment,
        old: runtime.id,
        host: runtime.host_id,
        machine: host.machine_id,
        generation: runtime.generation,
        capability: capability.to_owned(),
        desired,
        recipe: fingerprint(&recipe)?,
    };
    let patch = json!({"capability_revocation":custody,"capability_revocation_required":true});
    jobs::merge_plan(conn, row.id, &patch, now).await?;
    row.plan
        .as_object_mut()
        .ok_or_else(|| EngineError::unreadable("the capability replacement has no native plan"))?
        .extend(
            patch
                .as_object()
                .ok_or_else(|| {
                    EngineError::unreadable("the capability replacement has no custody")
                })?
                .clone(),
        );
    Ok(())
}
async fn current(conn: &mut Conn, row: &jobs::JobRow, cleanup: bool) -> Result<bool, EngineError> {
    let Some(custody) = custody(row)? else {
        return Ok(false);
    };
    let Some(env) = envs::environment(conn, custody.environment).await? else {
        return Ok(false);
    };
    let Some(old) = envs::runtime(conn, custody.old).await? else {
        return Ok(false);
    };
    let Some(host) = hosts::host(conn, custody.host).await? else {
        return Ok(false);
    };
    let mut desired = env.granted_capabilities.clone();
    desired.sort();
    desired.dedup();
    let committed = row.plan.get("committed").and_then(Json::as_bool) == Some(true);
    let generation = if committed {
        custody.generation.checked_add(1)
    } else {
        Some(custody.generation)
    };
    Ok(row.is_open()
        && (cleanup || !row.cancel_requested())
        && old.environment_id == Some(custody.environment)
        && old.host_id == custody.host
        && old.generation == custody.generation
        && host.machine_id == custody.machine
        && (cleanup || generation == Some(env.generation) && desired == custody.desired)
        && !desired.iter().any(|key| key == &custody.capability))
}
pub(crate) async fn guard_in(conn: &mut Conn, job: &Job, cleanup: bool) -> Result<(), Stop> {
    let row = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
    if !applies(&row) {
        return Ok(());
    }
    let Some(custody) = custody(&row)? else {
        return Err(fail(
            "access.denied",
            "the recorded capability removal changed",
        ));
    };
    envs::lock_environment(conn, custody.environment).await?;
    if !current(conn, &row, cleanup).await? {
        return Err(fail(
            "access.denied",
            "the recorded capability removal no longer owns this replacement",
        ));
    }
    Ok(())
}
pub(crate) async fn guard(job: &Job, cleanup: bool) -> Result<(), Stop> {
    if !applies(&job.row().await?) {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    guard_in(&mut tx, job, cleanup).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Exact private command identity signed by the native replacement runner, never an invoker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    job: Id,
    kind: CommandKind,
    canonical: Json,
    cleanup: bool,
}
struct CommandView<'a> {
    id: Id,
    machine: Id,
    key: Option<&'a str>,
    kind: &'a str,
    body: &'a Json,
}
impl Authority {
    /// Take the native source before the dispatcher locks its canonical command row.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        orch_db::queries::transaction_guard::lock_action(conn).await?;
        if let Some(row) = jobs::lock(conn, self.job).await?
            && let Some(custody) = custody(&row)?
        {
            envs::lock_environment(conn, custody.environment).await?;
        }
        Ok(())
    }
    /// The real native kind retained in the journal.
    pub fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    async fn callers(
        &self,
        conn: &mut Conn,
        command: CommandView<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let CommandView {
            id,
            machine,
            key,
            kind,
            body,
        } = command;
        orch_db::queries::transaction_guard::lock_action(conn).await?;
        let Some(row) = jobs::lock(conn, self.job).await? else {
            return Ok(None);
        };
        let Some(custody) = custody(&row)? else {
            return Ok(None);
        };
        envs::lock_environment(conn, custody.environment).await?;
        if machine != custody.machine {
            let Some(physical) = orch_db::queries::resources::machine(conn, machine).await? else {
                return Ok(None);
            };
            let candidate = row
                .plan
                .get("runtime")
                .cloned()
                .and_then(|value| serde_json::from_value::<Id>(value).ok());
            if physical.runtime_id != Some(custody.old) && physical.runtime_id != candidate {
                return Ok(None);
            }
            let Some(runtime) = physical.runtime_id else {
                return Ok(None);
            };
            if !envs::runtime(conn, runtime).await?.is_some_and(|runtime| {
                runtime.environment_id == Some(custody.environment)
                    && runtime.host_id == custody.host
            }) {
                return Ok(None);
            }
        }
        let lease: Option<Time> = row
            .plan
            .pointer("/runner/until")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok());
        let Some(lease) = lease.filter(|until| until.get() > clock.now()) else {
            return Ok(None);
        };
        let canonical = command_identity::canonical(id, machine, key, kind, body)?;
        if canonical != self.canonical
            || kind != self.kind.as_str()
            || self.cleanup
                && !matches!(
                    self.kind,
                    CommandKind::NodeRuntimeStop | CommandKind::NodeRuntimeRemove
                )
            || row
                .plan
                .get(JOURNAL)
                .and_then(|journal| journal.get(id.to_string()))
                != Some(&json!({"canonical":canonical,"cleanup":self.cleanup}))
            || !current(conn, &row, self.cleanup).await?
        {
            return Ok(None);
        }
        // The already accepted removal owns only this exact recipe and native journal.
        // It never restores the former actor or supplies authority for a new grant.
        Ok(Some(vec![(None, Some(lease))]))
    }
    /// Check the exact proposed insertion after its queue waits.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.callers(
            conn,
            CommandView {
                id: command.id,
                machine: command.machine_id,
                key: command.serial_key,
                kind: command.kind,
                body: command.body,
            },
            clock,
        )
        .await
    }
    /// Check the same recorded physical command before first wire delivery.
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.callers(
            conn,
            CommandView {
                id: command.id,
                machine: command.machine_id,
                key: command.serial_key(),
                kind: &command.kind,
                body: &command.body,
            },
            clock,
        )
        .await
    }
}
pub(crate) async fn decorate(job: &Job, command: &mut Outgoing, cleanup: bool) -> Result<(), Stop> {
    if !applies(&job.row().await?) {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    guard_in(&mut tx, job, cleanup).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let canonical = command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )?;
    let journal = json!({"canonical":canonical,"cleanup":cleanup});
    if row
        .plan
        .get(JOURNAL)
        .and_then(|entries| entries.get(command.id.to_string()))
        .is_some_and(|old| old != &journal)
    {
        return Err(fail(
            "capability_identity",
            "the capability cleanup command changed",
        ));
    }
    let mut entries = row.plan.get(JOURNAL).cloned().unwrap_or_else(|| json!({}));
    entries
        .as_object_mut()
        .ok_or_else(|| {
            fail(
                "capability_identity",
                "the capability command journal changed",
            )
        })?
        .insert(command.id.to_string(), journal);
    jobs::merge_plan(
        &mut tx,
        row.id,
        &json!({"capability_recreation_commands":entries}),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    let authority = CommandAuthority::CapabilityRecreation(Box::new(Authority {
        job: job.id(),
        kind: command.kind,
        canonical,
        cleanup,
    }));
    if command
        .authority
        .as_ref()
        .is_some_and(|existing| existing != &authority)
    {
        return Err(fail(
            "capability_identity",
            "the capability cleanup has another native owner",
        ));
    }
    command.authority = Some(authority);
    Ok(())
}
