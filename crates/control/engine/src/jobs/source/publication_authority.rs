//! The original commit/push selection and its one exact native publication.
//!
//! This uses the existing file-job recipe, caller restoration, operation catalogue and
//! native issuer. A received publication result needs its owned frame, not fresh forward authority.

use super::*;
use crate::commands::{RetainedCommand, Stage};
use crate::operation_scope::{BindingError, Prepared, targets};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::operations::{FacetUsage, TargetResolver};

const ORIGIN: &str = "publication_origin";
const PLAN: &str = "publication_plan";
const COMMAND: &str = "publication_command";

fn refused() -> Stop {
    Failure::new(
        "access.denied",
        "The accepted Git publication no longer authorizes this work.",
    )
    .into()
}

/// Presence is checked independently of kind so invalid evidence never selects a legacy path.
pub(crate) fn is_restricted(row: &JobRow) -> bool {
    [ORIGIN, PLAN, COMMAND]
        .iter()
        .any(|field| row.plan.get(*field).is_some())
}

fn operation(plan: &Plan) -> Option<&'static str> {
    match plan.action {
        Action::Commit { .. } => Some("EnvironmentGitCommit"),
        Action::Push { .. } => Some("EnvironmentGitPush"),
        _ => None,
    }
}

fn recipe(row: &JobRow) -> Result<Plan, Stop> {
    let plan: Plan = serde_json::from_value(row.plan.clone()).map_err(EngineError::from)?;
    if !row.is_open()
        || row.cancel_requested()
        || row.kind != plan.action.kind()
        || row.environment_id != Some(plan.environment)
        || row.node_id != plan.thread
        || row.provider_id.is_some()
        || row.created_by != Some(plan.caller)
        || plan.creator_origin.is_some()
        || plan.scheduled_input.is_some()
        || operation(&plan).is_none()
        || row.plan.get(PLAN) != Some(&serde_json::to_value(&plan).map_err(EngineError::from)?)
    {
        return Err(refused());
    }
    Ok(plan)
}

fn original(row: &JobRow, plan: &Plan) -> Result<CapturedOperation, Stop> {
    let origin: CapturedOperation =
        serde_json::from_value(row.plan.get(ORIGIN).cloned().unwrap_or(Json::Null))
            .map_err(|_| refused())?;
    if origin.user() != plan.caller || Some(origin.operation()) != operation(plan) {
        return Err(refused());
    }
    Ok(origin)
}

pub(super) async fn current_callers(
    conn: &mut Conn,
    row: &JobRow,
    clock: &dyn orch_types::Clock,
) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
    let plan = recipe(row)
        .map_err(|_| EngineError::unreadable("Git publication authority could not be checked."))?;
    let origin = original(row, &plan)
        .map_err(|_| EngineError::unreadable("Git publication authority could not be checked."))?;
    Ok(origin
        .restore_current(conn, clock)
        .await?
        .map(|caller| vec![(Some(caller), None)]))
}

fn expected(row: &JobRow, plan: &Plan) -> Result<Outgoing, Stop> {
    let key = SerialKey::Environment(plan.environment);
    let command = match &plan.action {
        Action::Commit { body }
            if body.environment_id == plan.environment && body.job_id == row.id =>
        {
            Outgoing::new(
                crate::ids::derived("git.commit", &[row.id.as_bytes()]),
                plan.machine,
                key,
                body,
            )
        }
        Action::Push { body }
            if body.environment_id == plan.environment && body.job_id == row.id =>
        {
            Outgoing::new(
                crate::ids::derived("git.push", &[row.id.as_bytes()]),
                plan.machine,
                key,
                body,
            )
        }
        _ => return Err(refused()),
    };
    command.map_err(EngineError::from).map_err(Into::into)
}

/// Retain only the newly inserted file job while its actual API action guard still holds.
/// The old Plan shape stays unchanged; creator and schedule origins keep their existing meanings.
pub async fn retain(
    conn: &mut Conn,
    row: &JobRow,
    origin: &CapturedOperation,
    now: Time,
) -> Result<(), EngineError> {
    let plan: Plan = serde_json::from_value(row.plan.clone())?;
    if row.state != "queued"
        || row.kind != plan.action.kind()
        || row.created_by != Some(plan.caller)
        || row.environment_id != Some(plan.environment)
        || row.node_id != plan.thread
        || row.provider_id.is_some()
        || row.cancel_requested()
        || plan.creator_origin.is_some()
        || plan.scheduled_input.is_some()
        || origin.user() != plan.caller
        || Some(origin.operation()) != operation(&plan)
        || is_restricted(row)
    {
        return Err(EngineError::unreadable(
            "This is not the newly accepted Git publication.",
        ));
    }
    expected(row, &plan)
        .map_err(|_| EngineError::unreadable("The accepted Git command has another recipe."))?;
    rows::merge_plan(conn, row.id, &json!({ORIGIN:origin, PLAN:plan}), now.get()).await?;
    Ok(())
}

fn unavailable(error: &orch_authz::Error) -> bool {
    matches!(
        error,
        orch_authz::Error::NotFound(_)
            | orch_authz::Error::NoSuchUser(_)
            | orch_authz::Error::Suspended { .. }
            | orch_authz::Error::Archived { .. }
    )
}

/// Resolve the real Environment again for every original required facet. The account's
/// FilesManage and local write gates remain in source::check_at, after this scope restriction.
pub(super) async fn principal(
    conn: &mut Conn,
    row: &JobRow,
    plan: &Plan,
    now: Time,
) -> Result<Principal, Stop> {
    let accepted = recipe(row)?;
    if serde_json::to_value(&accepted).map_err(EngineError::from)?
        != serde_json::to_value(plan).map_err(EngineError::from)?
    {
        return Err(refused());
    }
    let origin = original(row, plan)?;
    let caller = origin
        .restore(conn, now)
        .await
        .map_err(EngineError::from)?
        .ok_or_else(refused)?;
    if caller.user() != plan.caller || caller.grant().and_then(|grant| grant.token()) != plan.token
    {
        return Err(refused());
    }
    let target = match targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: plan.environment,
        },
    )
    .await
    {
        Ok(target) => target,
        Err(targets::TargetError::Missing) => return Err(refused()),
        Err(targets::TargetError::Authz(error)) if unavailable(&error) => return Err(refused()),
        Err(targets::TargetError::Authz(error)) => return Err(EngineError::from(error).into()),
        Err(targets::TargetError::Db(error)) => return Err(EngineError::from(error).into()),
    };
    let mut prepared =
        Prepared::for_selection(origin.operation()).map_err(EngineError::unreadable)?;
    for index in 0..prepared.variant().facets.len() {
        let facet = &prepared.variant().facets[index];
        if facet.usage != FacetUsage::Required
            || facet.target != TargetResolver::Path("id")
            || facet.resources != [KeyTargetKind::Object(ObjectKind::Environment)]
        {
            return Err(
                EngineError::unreadable("The Git publication target declaration changed.").into(),
            );
        }
        prepared
            .bind(
                index,
                KeyTargetKind::Object(ObjectKind::Environment),
                std::slice::from_ref(&target),
            )
            .map_err(EngineError::unreadable)?;
    }
    let caller = match prepared.admit(&caller, false) {
        Ok(caller) => caller,
        Err(BindingError::Refused) => return Err(refused()),
        Err(error) => return Err(EngineError::unreadable(error).into()),
    };
    Principal::load(conn, caller, now)
        .await
        .map_err(EngineError::from)
        .map_err(Into::into)
}

fn recorded(row: &JobRow, plan: &Plan, command: &Outgoing, active: bool) -> Result<bool, Stop> {
    let expected = expected(row, plan)?;
    let recorded: Outgoing =
        serde_json::from_value(row.plan.get(COMMAND).cloned().unwrap_or(Json::Null))
            .map_err(|_| refused())?;
    if expected != recorded || expected != *command {
        return Ok(false);
    }
    if active
        && (row.plan.pointer("/active_command/id") != Some(&json!(command.id))
            || row.plan.pointer("/active_command/machine") != Some(&json!(command.machine))
            || row
                .plan
                .pointer("/active_command/kind")
                .and_then(Json::as_str)
                != Some(command.kind.as_str()))
    {
        return Ok(false);
    }
    Ok(true)
}

fn retained_matches(
    row: &JobRow,
    plan: &Plan,
    command: &Outgoing,
    retained: &RetainedCommand,
) -> Result<bool, Stop> {
    Ok(recorded(row, plan, command, false)?
        && retained.machine == command.machine
        && retained.kind == command.kind.as_str()
        && retained.key == command.key.wire()
        && retained.body == command.body)
}

/// Only this exact owned frame may supply the received-result shortcut. This deliberately
/// restores no forward credential: completed/accepted native work still needs reconciliation.
pub(crate) async fn require_receipt_identity(job: &Job, command: &Outgoing) -> Result<(), Stop> {
    if !is_restricted(&job.row().await?) {
        return Ok(());
    }
    let retained = job
        .engine()
        .inner
        .commands
        .retained_command(command.id)
        .await
        .map_err(EngineError::from)?
        .ok_or_else(refused)?;
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if let Some(reason) = super::super::runner::abort_reason(&row)? {
        return Err(reason);
    }
    let plan = recipe(&row)?;
    original(&row, &plan)?;
    if !retained_matches(&row, &plan, command, &retained)? {
        return Err(refused());
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Existing apply/verify/commit phases may reconcile an actual exact native receipt after
/// revocation. Before Accepted they continue through source::check_at's normal current gate.
pub(super) async fn received(job: &Job, plan: &Plan) -> Result<bool, Stop> {
    let row = job.row().await?;
    if !is_restricted(&row) {
        return Ok(false);
    }
    let current = recipe(&row)?;
    if serde_json::to_value(&current).map_err(EngineError::from)?
        != serde_json::to_value(plan).map_err(EngineError::from)?
    {
        return Err(refused());
    }
    let command = expected(&row, plan)?;
    if !job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(EngineError::from)?
        .is_some_and(|info| matches!(info.stage, Stage::Accepted | Stage::Over))
    {
        return Ok(false);
    }
    require_receipt_identity(job, &command).await?;
    Ok(true)
}

/// Factual post-publication observations use only the exact accepted physical source.
/// The existing serialized file-job hold excludes ordinary move/recreate/restart work
/// until this job finishes. If an out-of-band source fact changed, keep the result/audit
/// and let the ordinary watcher observe the replacement instead of borrowing it here.
pub(super) async fn reconciliation_current(job: &Job, plan: &Plan) -> Result<bool, Stop> {
    let row = job.row().await?;
    if !is_restricted(&row) {
        return Ok(true);
    }
    let current = recipe(&row)?;
    if serde_json::to_value(&current).map_err(EngineError::from)?
        != serde_json::to_value(plan).map_err(EngineError::from)?
    {
        return Err(refused());
    }
    require_receipt_identity(job, &expected(&row, plan)?).await?;
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if let Some(reason) = super::super::runner::abort_reason(&row)? {
        return Err(reason);
    }
    recipe(&row)?;
    let current = super::matching_target(&mut tx, plan, None).await?.is_some();
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(current)
}

/// Reuse the runner's existing canonical command point, before the immutable issuer.
pub(crate) async fn remember_command(job: &Job, command: &Outgoing) -> Result<(), Stop> {
    if !is_restricted(&job.row().await?) {
        return Ok(());
    }
    let retained = job
        .engine()
        .inner
        .commands
        .retained_command(command.id)
        .await
        .map_err(EngineError::from)?;
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if let Some(reason) = super::super::runner::abort_reason(&row)? {
        return Err(reason);
    }
    let plan = recipe(&row)?;
    if expected(&row, &plan)? != *command {
        return Err(refused());
    }
    if let Some(retained) = retained
        && !retained_matches(&row, &plan, command, &retained)?
    {
        return Err(refused());
    }
    super::check_at(&mut tx, row.id, Time::new(job.now()), &plan, true, None).await?;
    if let Some(previous) = row.plan.get(COMMAND)
        && previous != &json!(command)
    {
        return Err(refused());
    }
    rows::merge_plan(&mut tx, row.id, &json!({COMMAND:command}), job.now()).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Called under the existing final-wire action/command locks. Accepted rows use the
/// dispatcher's factual receipt path; no new command can borrow this origin or journal.
pub(super) async fn command_authorized(
    conn: &mut Conn,
    row: &JobRow,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    now: Time,
) -> Result<bool, EngineError> {
    let checked: Result<bool, Stop> = async {
        let plan = recipe(row)?;
        let command = expected(row, &plan)?;
        if command.id != id
            || command.machine != machine
            || command.key.wire().as_deref() != key
            || command.kind.as_str() != kind
            || command.body != *body
            || !recorded(row, &plan, &command, true)?
        {
            return Ok(false);
        }
        super::check_at(conn, row.id, now, &plan, true, None).await?;
        Ok(true)
    }
    .await;
    match checked {
        Ok(allowed) => Ok(allowed),
        Err(Stop::Failed(failure)) if failure.code == super::super::runner::INTERNAL => Err(
            EngineError::unreadable("Git publication authority could not be checked."),
        ),
        Err(Stop::Failed(_) | Stop::Cancelled | Stop::Lost) => Ok(false),
    }
}
