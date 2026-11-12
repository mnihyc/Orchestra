//! Retained key-origin live capability work, using the ordinary native scope and shared command identity.
use super::{KIND, Plan};
use crate::{
    EngineError,
    commands::{Outgoing, Stage},
    jobs::{
        command_identity,
        runner::{Failure, Job, Stop, abort_reason},
    },
};
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn,
    queries::{
        approvals,
        environments::{capabilities as caps, hosting as envs},
        jobs,
        resources::hosting as hosts,
    },
};
use orch_link_contract::commands::{NodeCapabilitiesApply, SerialKey};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;

const JOURNAL: &str = "capability_commands";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    environment: Id,
    runtime: Id,
    host: Id,
    machine: Id,
    environment_generation: i32,
    runtime_generation: i32,
    recipe: Hash,
}
fn restricted(row: &jobs::JobRow) -> bool {
    row.kind == KIND
        && (row
            .plan
            .get("approval_origin_required")
            .and_then(Json::as_bool)
            == Some(true)
            || row.plan.get("approval_origin").is_some()
            || row.plan.get("approval_binding").is_some()
            || row.plan.get("revocation_origin").is_some()
            || row
                .plan
                .get("revocation_origin_required")
                .and_then(Json::as_bool)
                == Some(true)
            || row.plan.get(JOURNAL).is_some())
}

pub(super) fn has_origin(plan: &Plan) -> bool {
    plan.approval_origin.is_some()
        || plan.approval_origin_required
        || plan.revocation_origin.is_some()
        || plan.revocation_origin_required
}

fn retained_origin(plan: &Plan) -> Option<&orch_authz::CapturedOperation> {
    plan.revocation_origin
        .as_ref()
        .or(plan.approval_origin.as_ref())
}

/// A removed desired capability admits only its exact reducing native reconciliation.
/// This does not turn the retained actor/key into authority for a fresh grant.
fn valid_origin(plan: &Plan) -> bool {
    if plan.revocation_origin_required || plan.revocation_origin.is_some() {
        let Some(origin) = &plan.revocation_origin else {
            return false;
        };
        let mut removed = plan.before.clone();
        removed.granted.retain(|key| key != &plan.capability);
        return origin.operation() == "EnvironmentCapabilityRevoke"
            && Some(origin.user()) == plan.decided_by
            && plan.revocation_origin_required
            && !plan.grant
            && plan.requester.is_none()
            && plan.approval.is_none()
            && plan.approval_origin.is_none()
            && !plan.approval_origin_required
            && plan
                .before
                .granted
                .iter()
                .any(|key| key == &plan.capability)
            && plan.after == removed;
    }
    plan.approval_origin.as_ref().is_some_and(|origin| {
        origin.operation() == "ApprovalDecide.capability"
            && Some(origin.user()) == plan.decided_by
            && plan.approval_origin_required
            && plan.approval.is_some()
            && plan.grant
    })
}
fn refused() -> Stop {
    Failure::new(
        "access.denied",
        "the original live capability authority no longer permits new work",
    )
    .into()
}
fn identity() -> Stop {
    Failure::new(
        "capability_identity",
        "the live capability command does not match its retained native frame",
    )
    .into()
}
fn recipe(environment: Id, plan: &Plan) -> Result<Hash, EngineError> {
    let mut plan = plan.clone();
    plan.approval_binding = None;
    command_identity::hash(&json!({"environment":environment,"plan":plan}))
}
pub(crate) async fn bind(
    conn: &mut Conn,
    environment: Id,
    plan: &mut Plan,
) -> Result<(), EngineError> {
    if !has_origin(plan) {
        return Ok(());
    }
    if retained_origin(plan).is_none() {
        return Err(EngineError::unreadable(
            "the known live capability origin is missing",
        ));
    }
    if !valid_origin(plan) || plan.approval_binding.is_some() {
        return Err(EngineError::unreadable(
            "the live capability's accepted origin changed",
        ));
    }
    let env = envs::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the live capability environment is absent"))?;
    let runtime = envs::live_runtime(conn, environment)
        .await?
        .filter(|runtime| runtime.id == plan.runtime)
        .ok_or(orch_db::DbError::Retry)?;
    let host = hosts::host(conn, runtime.host_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the live capability Host is absent"))?;
    plan.approval_binding = Some(Binding {
        environment,
        runtime: runtime.id,
        host: runtime.host_id,
        machine: host.machine_id,
        environment_generation: env.generation,
        runtime_generation: runtime.generation,
        recipe: recipe(environment, plan)?,
    });
    Ok(())
}
fn structural(row: &jobs::JobRow) -> Result<Option<Plan>, EngineError> {
    let Ok(plan) = serde_json::from_value::<Plan>(row.plan.clone()) else {
        return Ok(None);
    };
    let Some(binding) = plan.approval_binding.as_ref() else {
        return Ok(None);
    };
    let Some(origin) = retained_origin(&plan) else {
        return Ok(None);
    };
    if row.kind != KIND
        || row.environment_id != Some(binding.environment)
        || plan.runtime != binding.runtime
        || row.created_by != Some(origin.user())
        || !valid_origin(&plan)
        || recipe(binding.environment, &plan)? != binding.recipe
    {
        return Ok(None);
    }
    Ok(Some(plan))
}
fn expected(row: &jobs::JobRow, plan: &Plan, undo: bool) -> Result<Outgoing, EngineError> {
    let binding = plan
        .approval_binding
        .as_ref()
        .ok_or_else(|| EngineError::unreadable("the live capability binding is missing"))?;
    let command = NodeCapabilitiesApply {
        runtime_id: plan.runtime,
        capabilities: json!(if undo && plan.grant {
            &plan.before
        } else {
            &plan.after
        }),
    };
    Ok(Outgoing::new(
        crate::ids::derived(
            if undo {
                "capability.live.undo"
            } else {
                "capability.live.apply"
            },
            &[row.id.as_bytes()],
        ),
        binding.machine,
        SerialKey::Runtime(plan.runtime),
        &command,
    )?
    .with_authority(crate::commands::CommandAuthority::capability_change(
        CapabilityChangeAuthority::capture(row.id, undo),
    )))
}
fn same_recipe(
    row: &jobs::JobRow,
    plan: &Plan,
    command: &Outgoing,
    undo: bool,
) -> Result<bool, EngineError> {
    let expected = expected(row, plan, undo)?;
    Ok(command_identity::canonical(
        expected.id,
        expected.machine,
        expected.key.wire().as_deref(),
        expected.kind.as_str(),
        &expected.body,
    )? == command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )?)
}
/// Actual receipt identity has no need to revive a stale active-pointer fence or current key.
/// All recipient/kind/key/body facts still come from the real retained frame and immutable journal.
pub(crate) async fn accepted_command(job: &Job, undo: bool) -> Result<Option<Outgoing>, Stop> {
    let row = job.row().await?;
    if !restricted(&row) {
        return Ok(None);
    }
    let plan = structural(&row)?.ok_or_else(identity)?;
    let command = expected(&row, &plan, undo)?;
    if !job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(EngineError::from)?
        .is_some_and(|info| matches!(info.stage, Stage::Accepted | Stage::Over))
    {
        return Ok(None);
    }
    require_receipt(job, &command, undo).await?;
    Ok(Some(command))
}
async fn require_receipt(job: &Job, command: &Outgoing, undo: bool) -> Result<(), Stop> {
    let row = job.row().await?;
    let plan = structural(&row)?.ok_or_else(identity)?;
    if !same_recipe(&row, &plan, command, undo)? {
        return Err(identity());
    }
    let retained = job
        .engine()
        .inner
        .commands
        .retained_command(command.id)
        .await
        .map_err(EngineError::from)?
        .ok_or_else(identity)?;
    let canonical = command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )?;
    let actual = command_identity::canonical(
        command.id,
        retained.machine,
        retained.key.as_deref(),
        &retained.kind,
        &retained.body,
    )?;
    if row
        .plan
        .get(JOURNAL)
        .and_then(|journal| journal.get(command.id.to_string()))
        != Some(&canonical)
        || actual != canonical
    {
        return Err(identity());
    }
    Ok(())
}
async fn target_current(
    conn: &mut Conn,
    row: &jobs::JobRow,
    plan: &Plan,
) -> Result<bool, EngineError> {
    let Some(binding) = &plan.approval_binding else {
        return Ok(false);
    };
    let Some(env) = envs::environment(conn, binding.environment).await? else {
        return Ok(false);
    };
    let Some(runtime) = envs::live_runtime(conn, binding.environment).await? else {
        return Ok(false);
    };
    let Some(host) = hosts::host(conn, binding.host).await? else {
        return Ok(false);
    };
    Ok(row.environment_id == Some(env.id)
        && env.generation == binding.environment_generation
        && runtime.id == binding.runtime
        && runtime.generation == binding.runtime_generation
        && runtime.host_id == binding.host
        && host.machine_id == binding.machine)
}
async fn current_origin(
    conn: &mut Conn,
    row: &jobs::JobRow,
    plan: &Plan,
    clock: &dyn Clock,
) -> Result<Option<Caller>, EngineError> {
    if !target_current(conn, row, plan).await? {
        return Ok(None);
    }
    let Some(approval) = approvals::approval(
        conn,
        plan.approval
            .ok_or_else(|| EngineError::unreadable("live approval missing"))?,
    )
    .await?
    else {
        return Ok(None);
    };
    let origin = plan
        .approval_origin
        .as_ref()
        .ok_or_else(|| EngineError::unreadable("live origin missing"))?;
    if approval.kind != "capability"
        || approval.state != "approved"
        || approval.environment_id != row.environment_id
        || approval.decided_by != Some(origin.user())
        || approval
            .decision
            .as_ref()
            .and_then(|value| value.get("approve"))
            .and_then(Json::as_bool)
            != Some(true)
        || approval.request.get("capability").and_then(Json::as_str)
            != Some(plan.capability.as_str())
        || approval.thread_id != plan.requester
    {
        return Ok(None);
    }
    let Some(caller) = origin.restore_current(conn, clock).await? else {
        return Ok(None);
    };
    let target = match crate::approvals::operation::governor(conn, &approval).await {
        Ok(target) => target,
        Err(crate::operation_scope::targets::TargetError::Missing) => return Ok(None),
        Err(crate::operation_scope::targets::TargetError::Db(error)) => return Err(error.into()),
        Err(crate::operation_scope::targets::TargetError::Authz(error)) => return Err(error.into()),
    };
    // Residual new physical work re-admits its exact accepted native operation. It
    // performs no new card mutation/disclosure; every declared required effect must bind.
    let mut prepared = crate::operation_scope::Prepared::for_selection(origin.operation())
        .map_err(EngineError::unreadable)?;
    let slot = prepared
        .variant()
        .facets
        .iter()
        .position(|facet| {
            facet.target
                == orch_api_contract::operations::TargetResolver::Resolved("approval_subject")
        })
        .ok_or_else(|| EngineError::unreadable("the capability scope subject is undeclared"))?;
    prepared
        .bind(slot, target.object.kind, &[target])
        .map_err(EngineError::unreadable)?;
    let caller = match prepared.admit(&caller, false) {
        Ok(caller) => caller,
        Err(crate::operation_scope::BindingError::Refused) => return Ok(None),
        Err(error) => return Err(EngineError::unreadable(error)),
    };
    let principal = Principal::load(conn, caller.clone(), clock.time()).await?;
    match crate::capabilities::native_decision_authority(conn, &principal, &approval).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => return Ok(None),
        Err(EngineError::Authz(
            orch_authz::Error::NotFound(_)
            | orch_authz::Error::NoSuchUser(_)
            | orch_authz::Error::Suspended { .. }
            | orch_authz::Error::Archived { .. },
        )) => return Ok(None),
        Err(error) => return Err(error),
    }
    let Some(facts) = caps::environment_facts(
        conn,
        row.environment_id
            .ok_or_else(|| EngineError::unreadable("live environment missing"))?,
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(definition) = caps::definition(conn, &plan.capability).await? else {
        return Ok(None);
    };
    if facts.deleted_at.is_some()
        || crate::capabilities::capability_policy(conn, facts.home_node_id, &definition)
            .await?
            .decision
            == "deny"
        || !caller.key_deadline_allows(clock)
    {
        return Ok(None);
    }
    Ok(Some(caller))
}

/// Physical removal completes the already committed revocation, even if its key later closes.
/// A new desired grant must never be overwritten by an older retained removal body.
async fn removal_current(
    conn: &mut Conn,
    row: &jobs::JobRow,
    plan: &Plan,
) -> Result<bool, EngineError> {
    if plan.grant || !valid_origin(plan) || !target_current(conn, row, plan).await? {
        return Ok(false);
    }
    let Some(environment) = row.environment_id else {
        return Ok(false);
    };
    let Some(facts) = caps::environment_facts(conn, environment).await? else {
        return Ok(false);
    };
    let Some(definition) = jobs::changes::definition(conn, plan.runtime).await? else {
        return Ok(false);
    };
    let Some(value) = definition.get("capabilities") else {
        return Ok(false);
    };
    let mut recorded: orch_driver_contract::capabilities::CapabilityState =
        serde_json::from_value(value.clone())?;
    // Desired grants have their own current check below; every other native input
    // remains the actual runtime definition from this accepted removal's recipe.
    recorded.granted.clone_from(&plan.before.granted);
    if recorded != plan.before {
        return Ok(false);
    }
    let current = facts
        .granted_capabilities
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    let desired = plan
        .after
        .granted
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    Ok(current == desired
        && !facts
            .granted_capabilities
            .iter()
            .any(|key| key == &plan.capability))
}
/// The final job issue hook. NodeCapabilitiesApply has no WorkReport/retry-Busy transform;
/// it is called after the existing runner's other canonicalization/active metadata writes.
pub(crate) async fn before_issue(
    job: &Job,
    command: &Outgoing,
    respect_cancel: bool,
) -> Result<(), Stop> {
    let row = job.row().await?;
    if !restricted(&row) {
        return Ok(());
    }
    let undo = !respect_cancel;
    if job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(EngineError::from)?
        .is_some_and(|info| matches!(info.stage, Stage::Accepted | Stage::Over))
    {
        return require_receipt(job, command, undo).await;
    }
    let result = remember(job, command, undo).await;
    if matches!(&result,Err(Stop::Failed(failure))if failure.code=="access.denied")
        && job
            .engine()
            .inner
            .commands
            .status(command.id)
            .await
            .map_err(EngineError::from)?
            .is_some_and(|info| matches!(info.stage, Stage::Accepted | Stage::Over))
    {
        // An ACK is never a status-only authority loan. Verify the same real frozen frame.
        require_receipt(job, command, undo).await?;
        return Ok(());
    }
    result
}
async fn remember(job: &Job, command: &Outgoing, undo: bool) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let plan = structural(&row)?.ok_or_else(refused)?;
    let lease = claim_deadline(&row).ok_or(Stop::Lost)?;
    if !same_recipe(&row, &plan, command, undo)? {
        return Err(identity());
    }
    if !undo && (row.cancel_requested() || abort_reason(&row)?.is_some()) {
        return Err(Stop::Cancelled);
    }
    let caller = if !plan.grant {
        if !removal_current(&mut tx, &row, &plan).await? {
            return Err(refused());
        }
        None
    } else if undo {
        if !target_current(&mut tx, &row, &plan).await? {
            return Err(refused());
        }
        None
    } else {
        Some(
            current_origin(&mut tx, &row, &plan, job.engine().inner.clock.as_ref())
                .await?
                .ok_or_else(refused)?,
        )
    };
    if let Some(retained) = job
        .engine()
        .inner
        .commands
        .retained_command(command.id)
        .await
        .map_err(EngineError::from)?
    {
        // Replays may use only this job's previously recorded identity, not a foreign first writer.
        let canonical = command_identity::canonical(
            command.id,
            retained.machine,
            retained.key.as_deref(),
            &retained.kind,
            &retained.body,
        )?;
        if row
            .plan
            .get(JOURNAL)
            .and_then(|journal| journal.get(command.id.to_string()))
            != Some(&canonical)
        {
            return Err(identity());
        }
    }
    // Undo has no forward active-pointer update in the generic runner. Its exact existing
    // inverse is still recorded, and supersedes older unaccepted frames without deleting proof.
    jobs::merge_plan(&mut tx,row.id,&json!({"active_command":{"id":command.id,"machine":command.machine,"kind":command.kind.as_str(),"reported":false}}),job.now()).await?;
    command_identity::remember(
        &mut tx,
        &row,
        JOURNAL,
        command,
        job.engine().inner.clock.time(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if lease.get() <= job.engine().inner.clock.now() {
        return Err(Stop::Lost);
    }
    if caller
        .as_ref()
        .is_some_and(|caller| !caller.key_deadline_allows(job.engine().inner.clock.as_ref()))
    {
        return Err(refused());
    }
    Ok(())
}
/// Actual unaccepted first-wire checkpoint. Retained journal IDs stay recognized even if
/// the active pointer changes or the job ends, so missing authority never becomes maintenance.
fn claim_deadline(row: &jobs::JobRow) -> Option<Time> {
    row.plan
        .pointer("/runner/until")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}
struct CommandRef<'a> {
    id: Id,
    machine: Id,
    key: Option<&'a str>,
    kind: &'a str,
    body: &'a Json,
}

async fn command_callers_for_row(
    conn: &mut Conn,
    row: &jobs::JobRow,
    command: CommandRef<'_>,
    expected_undo: Option<bool>,
    clock: &dyn Clock,
) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
    let CommandRef {
        id,
        machine,
        key,
        kind,
        body,
    } = command;
    let Some(plan) = structural(row)? else {
        return Ok(None);
    };
    let undo = id == crate::ids::derived("capability.live.undo", &[row.id.as_bytes()]);
    if expected_undo.is_some_and(|expected| expected != undo) {
        return Ok(None);
    }
    let command = expected(row, &plan, undo)?;
    let canonical = command_identity::canonical(id, machine, key, kind, body)?;
    if command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )? != canonical
        || row
            .plan
            .get(JOURNAL)
            .and_then(|journal| journal.get(id.to_string()))
            != Some(&canonical)
    {
        return Ok(None);
    }
    // Actual accepted/terminal reconciliation is a fact, with no original invoker lease.
    // Its current native row is already locked/canonical at the shared publication entry.
    if !command_identity::recorded_matches(row, JOURNAL, id, machine, key, kind, body)? {
        return Ok(None);
    }
    let Some(lease) = claim_deadline(row).filter(|lease| lease.get() > clock.now()) else {
        return Ok(None);
    };
    if row.state != "running"
        || row
            .plan
            .pointer("/runner/id")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
            .is_none()
    {
        return Ok(None);
    }
    if !plan.grant {
        return Ok(removal_current(conn, row, &plan)
            .await?
            .then(|| vec![(None, Some(lease))]));
    }
    if undo {
        return Ok(target_current(conn, row, &plan)
            .await?
            .then(|| vec![(None, Some(lease))]));
    }
    if row.cancel_requested() || abort_reason(row)?.is_some() {
        return Ok(None);
    }
    Ok(current_origin(conn, row, &plan, clock)
        .await?
        .filter(|caller| caller.key_deadline_allows(clock))
        .map(|caller| vec![(Some(caller), Some(lease))]))
}
pub async fn command_current_callers(
    conn: &mut Conn,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    clock: &dyn Clock,
) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
    let owners = jobs::lock_capability_command_owners(conn, id).await?;
    if owners.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if owners.len() != 1 {
        return Ok(None);
    }
    let Some(row) = jobs::job(conn, owners[0]).await? else {
        return Ok(None);
    };
    command_callers_for_row(
        conn,
        &row,
        CommandRef {
            id,
            machine,
            key,
            kind,
            body,
        },
        None,
        clock,
    )
    .await
}
/// Private command evidence contains association only. It grants nothing and cannot be
/// deserialized as a key claim or emitted in a node's wire body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityChangeAuthority {
    job_id: Id,
    undo: bool,
}
impl CapabilityChangeAuthority {
    pub fn capture(job_id: Id, undo: bool) -> Self {
        Self { job_id, undo }
    }
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let Some(row) = jobs::lock(conn, self.job_id).await? else {
            return Ok(None);
        };
        command_callers_for_row(
            conn,
            &row,
            CommandRef {
                id: command.id,
                machine: command.machine_id,
                key: command.serial_key(),
                kind: &command.kind,
                body: &command.body,
            },
            Some(self.undo),
            clock,
        )
        .await
    }
}
