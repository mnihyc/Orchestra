//! Provider replacement owner eligibility, scoped requester bounds and native identity.
use super::environment_authority as global;
use super::*;
use crate::jobs::command_identity::{self, canonical, hash};
use crate::operation_scope::{BindingError, Prepared, targets};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::{CapturedOperation, Policy, Principal, resources};
use orch_types::{Hash, Time};
use serde::{Deserialize, Serialize};

pub mod environment;

fn lifecycle_provider(row: &jobs::JobRow) -> bool {
    row.provider_id.is_some()
        && row.environment_id.is_none()
        && crate::jobs::lifecycle_commands::environment::present(row)
}

const ORIGINS: &str = "provider_update_origins";
const BINDING: &str = "provider_update_binding";
const COMMANDS: &str = "provider_update_commands";
const NOTICES: &str = "provider_update_notice_commands";
const OWNER_COMMANDS: &str = "provider_recreate_commands";
const OWNER_NOTICES: &str = "provider_recreate_notice_commands";
const OPERATIONS: [&str; 2] = [
    "ProviderRuntimeUpdateBatchPreflight",
    "ProviderRuntimeUpdateBatchCreate",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    batch: Id,
    recipe_hash: Hash,
    old_host: Id,
    old_machine: Option<Id>,
    old_generation: i32,
    old_template: Id,
    old_driver: String,
    provider_machine: Option<Id>,
    host_selection: Option<Id>,
}

fn recipe_hash(recipe: &Recipe) -> Result<Hash, EngineError> {
    hash(&serde_json::to_value(recipe)?)
}
fn read<T: serde::de::DeserializeOwned>(
    row: &jobs::JobRow,
    field: &str,
) -> Result<Option<T>, EngineError> {
    row.plan
        .get(field)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}
fn origins_valid(origins: &[CapturedOperation], actor: Id) -> bool {
    origins.len() == 2
        && origins.iter().all(|origin| origin.user() == actor)
        && OPERATIONS.iter().all(|operation| {
            origins
                .iter()
                .filter(|origin| origin.operation() == *operation)
                .count()
                == 1
        })
}
pub(crate) fn is_restricted(row: &jobs::JobRow) -> bool {
    global::is_restricted(row)
        || environment::present(row)
        || [ORIGINS, BINDING, COMMANDS, NOTICES]
            .iter()
            .any(|field| row.plan.get(*field).is_some())
}
fn command_log(row: &jobs::JobRow) -> &'static str {
    if lifecycle_provider(row) {
        return crate::jobs::lifecycle_commands::JOURNAL;
    }
    if global::is_restricted(row) {
        return global::COMMANDS;
    }
    if environment::present(row) {
        return environment::COMMANDS;
    }
    if is_restricted(row) {
        COMMANDS
    } else {
        OWNER_COMMANDS
    }
}
fn notice_log(row: &jobs::JobRow) -> &'static str {
    if global::is_restricted(row) {
        return global::NOTICES;
    }
    if environment::present(row) {
        return environment::NOTICES;
    }
    if is_restricted(row) {
        NOTICES
    } else {
        OWNER_NOTICES
    }
}
fn refused() -> Stop {
    fail(
        "access.denied",
        "The accepted provider update no longer authorizes this work.",
    )
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

/// Re-resolve this one actual Provider for one original operation. Independent operation
/// names and all credentials inside each captured caller remain independent conjuncts.
pub async fn restore_origin(
    conn: &mut Conn,
    origin: &CapturedOperation,
    provider: Id,
    now: Time,
) -> Result<Option<Principal>, EngineError> {
    if !OPERATIONS.contains(&origin.operation()) {
        return Ok(None);
    }
    let Some(state) = providers::state(conn, provider)
        .await?
        .filter(|state| state.deleted_at.is_none())
    else {
        return Ok(None);
    };
    if crate::preflight::second_step_missing(conn, origin.user(), state.home_id).await? {
        return Ok(None);
    }
    let Some(caller) = origin.restore(conn, now).await? else {
        return Ok(None);
    };
    let mut plan = Prepared::for_selection(origin.operation()).map_err(EngineError::unreadable)?;
    let mut slots = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| {
            facet.target == TargetResolver::ResolvedCollection("affected_providers")
        });
    let Some((index, facet)) = slots.next() else {
        return Err(EngineError::unreadable(
            "The provider batch collection is not declared.",
        ));
    };
    if slots.next().is_some()
        || facet.usage != FacetUsage::Required
        || facet.action != KeyAction::Manage
        || facet.resources != [KeyTargetKind::Object(ObjectKind::Provider)]
    {
        return Err(EngineError::unreadable(
            "The provider batch collection metadata changed.",
        ));
    }
    let object = ObjectRef {
        kind: ObjectKind::Provider,
        id: provider,
    };
    let target = match targets::page_object(conn, object).await {
        Ok(target) => target,
        Err(targets::TargetError::Missing) => return Ok(None),
        Err(targets::TargetError::Authz(error)) if unavailable(&error) => return Ok(None),
        Err(targets::TargetError::Authz(error)) => return Err(error.into()),
        Err(targets::TargetError::Db(error)) => return Err(error.into()),
    };
    plan.bind(
        index,
        KeyTargetKind::Object(ObjectKind::Provider),
        &[target],
    )
    .map_err(EngineError::unreadable)?;
    let caller = match plan.admit(&caller, false) {
        Ok(caller) => caller,
        Err(BindingError::Refused) => return Ok(None),
        Err(error) => return Err(EngineError::unreadable(error)),
    };
    let principal = match Principal::load(conn, caller, now).await {
        Ok(principal) => principal,
        Err(error) if unavailable(&error) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match resources::needs(
        conn,
        &principal,
        &resources::Needs {
            required: Permission::ProviderManage,
            anchor: object,
            through: Some(state.home_id),
            items: vec![resources::Need {
                object,
                permissions: vec![Permission::ProviderManage],
                options: None,
            }],
        },
        Policy::default(),
    )
    .await
    {
        Ok(answer) => Ok(answer.is_allowed().then_some(principal)),
        Err(error) if unavailable(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Only the newly queued member of this actual batch can acquire its accepted origins.
pub async fn retain(
    conn: &mut Conn,
    job: Id,
    batch: Id,
    origins: &[CapturedOperation],
    now: Time,
) -> Result<(), EngineError> {
    let row = jobs::lock(conn, job)
        .await?
        .ok_or_else(|| EngineError::unreadable("The provider update job disappeared."))?;
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    let Target::Provider(provider) = recipe.target else {
        return Err(EngineError::unreadable(
            "This is not a provider update member.",
        ));
    };
    if row.kind != "provider_recreate"
        || row.provider_id != Some(provider)
        || row.environment_id.is_some()
        || row.batch_id != Some(batch)
        || row.state != "queued"
        || row.cancel_requested()
        || row.created_by != Some(recipe.by)
        || !origins_valid(origins, recipe.by)
        || recipe.reason != UpdatePreflightReason::Update
        || recipe.auto_start
        || recipe.capability.is_some()
        || row
            .plan
            .get("runtime")
            .is_some_and(|value| !value.is_null())
    {
        return Err(EngineError::unreadable(
            "This is not the exact newly accepted provider update member.",
        ));
    }
    if !jobs::lock_provider_update_runtimes(conn, provider, recipe.old, None).await? {
        return Err(EngineError::unreadable(
            "The accepted old runtime disappeared.",
        ));
    }
    let old = providers::live_runtime(conn, provider)
        .await?
        .filter(|old| old.id == recipe.old)
        .ok_or_else(|| EngineError::unreadable("The accepted provider runtime changed."))?;
    let state = providers::state(conn, provider)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or_else(|| EngineError::unreadable("The accepted provider disappeared."))?;
    let binding = Binding {
        batch,
        recipe_hash: recipe_hash(&recipe)?,
        old_host: old.host_id,
        old_machine: old.machine_id,
        old_generation: old.generation,
        old_template: old.template_id,
        old_driver: old.driver,
        provider_machine: state.machine_id,
        host_selection: recipe.host,
    };
    if is_restricted(&row) {
        if read::<Vec<CapturedOperation>>(&row, ORIGINS)?.as_deref() != Some(origins)
            || read::<Binding>(&row, BINDING)?.as_ref() != Some(&binding)
        {
            return Err(EngineError::unreadable(
                "This provider update member already has another origin.",
            ));
        }
        return Ok(());
    }
    jobs::merge_plan(
        conn,
        job,
        &json!({ORIGINS:origins,BINDING:binding}),
        now.get(),
    )
    .await?;
    Ok(())
}

/// The same current runtime identities for scoped and ordinary provider replacements.
/// Credential checks stay separate; this never chooses another actor or destination.
async fn runtime_structure(
    conn: &mut Conn,
    row: &jobs::JobRow,
    recipe: &Recipe,
    binding: Option<&Binding>,
) -> Result<Option<(envs::RuntimeFull, Option<envs::RuntimeFull>)>, EngineError> {
    let Target::Provider(provider) = recipe.target else {
        return Ok(None);
    };
    if row.kind != "provider_recreate"
        || row.provider_id != Some(provider)
        || row.environment_id.is_some()
        || row.created_by != Some(recipe.by)
        || !row.is_open()
        || row.cancel_requested()
        || crate::jobs::runner::abort_reason(row)?.is_some()
    {
        return Ok(None);
    }
    let next = row
        .plan
        .get("runtime")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value::<Id>)
        .transpose()?;
    if !jobs::lock_provider_update_runtimes(conn, provider, recipe.old, next).await? {
        return Ok(None);
    }
    let Some(state) = providers::lock(conn, provider).await?.filter(|state| {
        state.deleted_at.is_none()
            && binding.is_none_or(|binding| state.machine_id == binding.provider_machine)
    }) else {
        return Ok(None);
    };
    let Some(old) = providers::live_runtime(conn, provider)
        .await?
        .filter(|old| {
            old.id == recipe.old
                && old.kind == "provider"
                && old.provider_id == Some(provider)
                && old.environment_id.is_none()
                && old.successor_of.is_none()
                && binding.is_none_or(|binding| {
                    old.host_id == binding.old_host
                        && old.machine_id == binding.old_machine
                        && old.generation == binding.old_generation
                        && old.template_id == binding.old_template
                        && old.driver == binding.old_driver
                })
        })
    else {
        return Ok(None);
    };
    let _ = state;
    let target = match binding {
        Some(binding) => {
            let host = binding.host_selection.unwrap_or(binding.old_host);
            if (next.is_none() && recipe.host != binding.host_selection)
                || (next.is_some() && recipe.host != Some(host))
            {
                return Ok(None);
            }
            host
        }
        None => recipe.host.unwrap_or(old.host_id),
    };
    let candidate = match next {
        None => None,
        Some(next) => {
            let Some(candidate) = envs::runtime(conn, next).await?.filter(|candidate| {
                candidate.kind == "provider"
                    && candidate.provider_id == Some(provider)
                    && candidate.environment_id.is_none()
                    && candidate.successor_of == Some(old.id)
                    && candidate.generation == old.generation.saturating_add(1)
                    && candidate.host_id == target
                    && candidate.template_id == recipe.template
                    && candidate.driver == old.driver
                    && candidate.spec_hash == recipe.hash
                    && !matches!(candidate.state.as_str(), "retired" | "removed")
            }) else {
                return Ok(None);
            };
            if row.plan.get("host") != Some(&json!(target)) {
                return Ok(None);
            }
            Some(candidate)
        }
    };
    Ok(Some((old, candidate)))
}

async fn structure(
    conn: &mut Conn,
    row: &jobs::JobRow,
) -> Result<
    Option<(
        Recipe,
        Vec<CapturedOperation>,
        envs::RuntimeFull,
        Option<envs::RuntimeFull>,
    )>,
    EngineError,
> {
    let (Some(origins), Some(binding)) = (
        read::<Vec<CapturedOperation>>(row, ORIGINS)?,
        read::<Binding>(row, BINDING)?,
    ) else {
        return Ok(None);
    };
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    let mut accepted_recipe = recipe.clone();
    accepted_recipe.host = binding.host_selection;
    if row.batch_id != Some(binding.batch)
        || !origins_valid(&origins, recipe.by)
        || recipe_hash(&accepted_recipe)? != binding.recipe_hash
        || recipe.reason != UpdatePreflightReason::Update
    {
        return Ok(None);
    }
    let Some((old, candidate)) = runtime_structure(conn, row, &recipe, Some(&binding)).await?
    else {
        return Ok(None);
    };
    Ok(Some((recipe, origins, old, candidate)))
}

fn is_provider_recreate(row: &jobs::JobRow) -> bool {
    row.kind == "provider_recreate" && row.provider_id.is_some() && row.environment_id.is_none()
}

/// Only recorded ordinary frames gain the new identity proof; pre-existing
/// ordinary received frames without a journal keep their original replay path.
pub(crate) fn has_receipt_identity(row: &jobs::JobRow, command: Id) -> bool {
    lifecycle_provider(row)
        || is_restricted(row)
        || row
            .plan
            .get(OWNER_COMMANDS)
            .and_then(|values| values.get(command.to_string()))
            .is_some()
}

/// These are the existing provider replacement jobs, not environment/creation jobs.
pub(crate) fn applies(row: &jobs::JobRow) -> bool {
    lifecycle_provider(row)
        || is_restricted(row)
        || is_provider_recreate(row)
        || row.plan.get(OWNER_COMMANDS).is_some()
        || row.plan.get(OWNER_NOTICES).is_some()
}

async fn command_structure(
    conn: &mut Conn,
    row: &jobs::JobRow,
) -> Result<Option<(Recipe, envs::RuntimeFull, Option<envs::RuntimeFull>)>, EngineError> {
    if global::is_restricted(row) {
        return global::structure(conn, row, true).await;
    }
    if environment::present(row) {
        return environment::structure(conn, row).await;
    }
    if is_restricted(row) {
        return Ok(structure(conn, row)
            .await?
            .map(|(recipe, _, old, next)| (recipe, old, next)));
    }
    if !is_provider_recreate(row) {
        return Ok(None);
    }
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    Ok(runtime_structure(conn, row, &recipe, None)
        .await?
        .map(|(old, next)| (recipe, old, next)))
}

/// The actual provider's owner remains eligible at the selected destination.
/// Scoped members additionally restore their own original operations independently.
pub async fn current(conn: &mut Conn, row: &jobs::JobRow, now: Time) -> Result<bool, EngineError> {
    // A sampled timestamp cannot replace the real raw-HTTP/current-source checkpoint.
    if lifecycle_provider(row) {
        return Ok(false);
    }
    if global::is_restricted(row)
        || jobs::job(conn, row.id)
            .await?
            .as_ref()
            .is_some_and(global::is_restricted)
    {
        return Ok(false);
    }
    if jobs::job(conn, row.id)
        .await?
        .is_some_and(|actual| environment::present(&actual))
    {
        return Ok(false);
    }
    // The new native source requires its real clock and raw credential. A sampled time
    // remains the compatibility interface for the existing provider-only callers.
    if environment::present(row) {
        return Ok(false);
    }
    if !applies(row) {
        return Ok(true);
    }
    let (recipe, origins, old, next) = if is_restricted(row) {
        let Some(current) = structure(conn, row).await? else {
            return Ok(false);
        };
        current
    } else {
        let Some((recipe, old, next)) = command_structure(conn, row).await? else {
            return Ok(false);
        };
        (recipe, Vec::new(), old, next)
    };
    for origin in origins {
        let Some(principal) = restore_origin(conn, &origin, recipe.target.id(), now).await? else {
            return Ok(false);
        };
        // Preserve the ordinary reviewed-update prerequisite: only a newly selected template
        // requires TemplateUse. ProviderCreate's different placement contract is not borrowed.
        match crate::updates::template_usable(conn, &principal, recipe.template, old.template_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(EngineError::Authz(error)) if unavailable(&error) => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    let destination = next
        .as_ref()
        .map_or(recipe.host.unwrap_or(old.host_id), |next| next.host_id);
    crate::updates::provider_owner_host_usable(conn, recipe.target.id(), destination, now).await
}

/// Actual-clock replacement boundaries, including the Environment's real raw request source.
pub async fn current_with_clock(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn orch_types::Clock,
) -> Result<bool, EngineError> {
    if global::is_restricted(row) {
        let Some(actual) = jobs::job(conn, row.id).await?.filter(global::is_restricted) else {
            return Ok(false);
        };
        return Ok(global::current(conn, &actual, clock).await?.is_some());
    }
    let actual = jobs::job(conn, row.id).await?;
    if lifecycle_provider(row) || actual.as_ref().is_some_and(lifecycle_provider) {
        let Some(actual) = actual.filter(lifecycle_provider) else {
            return Ok(false);
        };
        return match crate::jobs::lifecycle_commands::environment::provider_current(
            conn, &actual, clock,
        )
        .await
        {
            Ok(checked) => {
                checked.check(clock)?;
                Ok(true)
            }
            Err(EngineError::BeforeEffect(_)) => Ok(false),
            Err(error) => Err(error),
        };
    }
    if environment::present(row) || actual.as_ref().is_some_and(environment::present) {
        match actual {
            Some(actual) if environment::present(&actual) => {
                environment::current(conn, &actual, clock).await
            }
            _ => Ok(false),
        }
    } else {
        current(conn, row, clock.time()).await
    }
}
async fn forward_current(
    conn: &mut Conn,
    row: &jobs::JobRow,
    now: Time,
    clock: Option<&dyn orch_types::Clock>,
) -> Result<bool, EngineError> {
    match clock {
        Some(clock) => current_with_clock(conn, row, clock).await,
        None => current(conn, row, now).await,
    }
}

pub(crate) async fn seal_command(job: &Job, command: &mut Outgoing) -> Result<(), Stop> {
    environment::seal(job, command).await
}

pub(crate) async fn deadline_in(conn: &mut Conn, job: &Job) -> Result<(), Stop> {
    let row = jobs::job(conn, job.id()).await?.ok_or(Stop::Lost)?;
    if lifecycle_provider(&row) {
        crate::jobs::lifecycle_commands::environment::check_deadline(
            conn,
            &row,
            job.engine().inner.clock.as_ref(),
        )
        .await?;
        return Ok(());
    }
    if global::is_restricted(&row) {
        global::deadline(conn, &row, job.engine().inner.clock.as_ref()).await?;
    }
    if environment::present(&row) {
        match environment::deadline(conn, &row, job.engine().inner.clock.as_ref()).await {
            Ok(()) => {}
            Err(EngineError::BeforeEffect(error))
                if error.is::<crate::updates::operation::Ended>()
                    || error.is::<crate::updates::operation::Denied>() =>
            {
                return Err(refused());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn thread_matches(owner: &jobs::JobRow, thread: &threads::ThreadRow) -> bool {
    if global::is_restricted(owner) || environment::present(owner) {
        thread.environment_id == owner.environment_id
    } else {
        thread.provider_id == owner.provider_id
    }
}

pub(crate) async fn guard_in(conn: &mut Conn, job: &Job) -> Result<(), Stop> {
    let initial = job.row().await?;
    if lifecycle_provider(&initial) {
        crate::jobs::lifecycle_commands::environment::lock_source(conn, &initial).await?;
        let row = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
        let checked = crate::jobs::lifecycle_commands::environment::current(
            conn,
            &row,
            job.engine().inner.clock.as_ref(),
            job.engine().inner.access.as_ref(),
            None,
        )
        .await?;
        checked.check(job.engine().inner.clock.as_ref())?;
        return Ok(());
    }
    if global::is_restricted(&job.row().await?) {
        let owner = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
        if owner.cancel_requested() {
            return Err(Stop::Cancelled);
        }
        if let Some(reason) = crate::jobs::runner::abort_reason(&owner)? {
            return Err(reason);
        }
        if global::current(conn, &owner, job.engine().inner.clock.as_ref())
            .await?
            .is_none()
        {
            return Err(refused());
        }
        return Ok(());
    }
    if jobs::job(conn, job.id())
        .await?
        .is_some_and(|row| environment::present(&row))
    {
        return environment::guard_in(conn, job).await;
    }
    if jobs::job(conn, job.id())
        .await?
        .is_none_or(|row| !applies(&row))
    {
        return Ok(());
    }
    let owner = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
    if owner.cancel_requested() {
        return Err(Stop::Cancelled);
    }
    if let Some(reason) = crate::jobs::runner::abort_reason(&owner)? {
        return Err(reason);
    }
    if applies(&owner) && !current(conn, &owner, Time::new(job.now())).await? {
        return Err(refused());
    }
    Ok(())
}

async fn held_thread(conn: &mut Conn, row: &jobs::JobRow, thread: Id) -> Result<bool, EngineError> {
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    Ok(recipe.threads.contains(&thread)
        && threads::lock(conn, thread)
            .await?
            .is_some_and(|thread| thread_matches(row, &thread))
        && rows::thread_hold(conn, thread)
            .await?
            .is_some_and(|held| held.id == row.id))
}

pub(crate) async fn cancel_approvals(job: &Job, thread: Id) -> Result<(), Stop> {
    if !applies(&job.row().await?) {
        return Ok(job.engine().cancel_pending_approvals(thread).await?);
    }
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    guard_in(&mut tx, job).await?;
    let row = jobs::job(&mut tx, job.id()).await?.ok_or(Stop::Lost)?;
    if !held_thread(&mut tx, &row, thread).await? {
        return Err(refused());
    }
    let mut unit = Unit::new(&mut tx, job.now());
    crate::locks::cancel_pending_approvals_in(&mut unit, thread).await?;
    let finished = unit.finish().await?;
    deadline_in(&mut tx, job).await?;
    job.engine().commit(tx, finished).await?;
    Ok(())
}

pub(crate) async fn interrupt(
    job: &Job,
    thread: Id,
    turn: &threads::TurnRow,
    by: Id,
) -> Result<(), Stop> {
    if !applies(&job.row().await?) {
        return Ok(job.engine().interrupt_for_change(thread, turn, by).await?);
    }
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    guard_in(&mut tx, job).await?;
    let row = jobs::job(&mut tx, job.id()).await?.ok_or(Stop::Lost)?;
    if row.created_by != Some(by) || !held_thread(&mut tx, &row, thread).await? {
        return Err(refused());
    }
    let Some(actual_turn) = threads::active_turn(&mut tx, thread)
        .await?
        .filter(|active| active.id == turn.id)
    else {
        return Ok(());
    };
    let mut unit = Unit::new(&mut tx, job.now());
    super::super::fork::interrupt_for_change_in(&mut unit, thread, &actual_turn, by).await?;
    let finished = unit.finish().await?;
    deadline_in(&mut tx, job).await?;
    job.engine().commit(tx, finished).await?;
    // This is the existing system completion of the exact decision accepted above.
    job.engine().drive_interrupt(thread).await?;
    Ok(())
}
pub(crate) async fn guard(job: &Job) -> Result<(), Stop> {
    if !applies(&job.row().await?) {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    guard_in(&mut tx, job).await?;
    deadline_in(&mut tx, job).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Compensation keeps ownership evidence, not withdrawn forward permissions. Never remove
/// a published/foreign unit merely because a stale phase retained its identifier.
pub(crate) async fn cleanup_guard(job: &Job) -> Result<(), Stop> {
    let row = job.row().await?;
    if lifecycle_provider(&row) {
        let mut tx = job.engine().db().begin().await?;
        let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if !crate::jobs::lifecycle_commands::environment::owns_recipe(&row)?
            || !crate::jobs::lifecycle_commands::environment::owns_provider_candidate(&mut tx, &row)
                .await?
        {
            return Err(refused());
        }
        tx.commit().await.map_err(DbError::from)?;
        return Ok(());
    }
    if global::is_restricted(&job.row().await?) {
        return global::cleanup(job).await;
    }
    if environment::present(&job.row().await?) {
        return environment::cleanup_guard(job).await;
    }
    if !is_restricted(&job.row().await?) {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let Some(binding) = read::<Binding>(&row, BINDING)? else {
        return Err(refused());
    };
    let mut recipe: Recipe = serde_json::from_value(row.plan.clone()).map_err(EngineError::from)?;
    recipe.host = binding.host_selection;
    let Target::Provider(provider) = recipe.target else {
        return Err(refused());
    };
    if row.kind != "provider_recreate"
        || row.provider_id != Some(provider)
        || row.environment_id.is_some()
        || row.batch_id != Some(binding.batch)
        || row.created_by != Some(recipe.by)
        || recipe_hash(&recipe)? != binding.recipe_hash
    {
        return Err(refused());
    }
    let next = row
        .plan
        .get("runtime")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value::<Id>)
        .transpose()
        .map_err(EngineError::from)?;
    if !jobs::lock_provider_update_runtimes(&mut tx, provider, recipe.old, next).await? {
        return Err(refused());
    }
    let old = envs::runtime(&mut tx, recipe.old)
        .await?
        .ok_or_else(refused)?;
    if old.kind != "provider"
        || old.provider_id != Some(provider)
        || old.environment_id.is_some()
        || old.host_id != binding.old_host
        || old.generation != binding.old_generation
        || old.template_id != binding.old_template
        || old.driver != binding.old_driver
    {
        return Err(refused());
    }
    if let Some(next) = next {
        let candidate = envs::runtime(&mut tx, next).await?.ok_or_else(refused)?;
        if candidate.kind != "provider"
            || candidate.provider_id != Some(provider)
            || candidate.environment_id.is_some()
            || candidate.successor_of != Some(old.id)
            || candidate.generation != old.generation.saturating_add(1)
            || candidate.host_id != binding.host_selection.unwrap_or(old.host_id)
            || candidate.template_id != recipe.template
            || candidate.spec_hash != recipe.hash
            || row.plan.get("host") != Some(&json!(binding.host_selection.unwrap_or(old.host_id)))
        {
            return Err(refused());
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

fn recorded_command_matches(
    row: &jobs::JobRow,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    if !is_provider_recreate(row)
        && !environment::present(row)
        && !global::is_restricted(row)
        && !lifecycle_provider(row)
    {
        return Ok(false);
    }
    command_identity::recorded_matches(row, command_log(row), id, machine, key, kind, body)
}
fn retained_command_matches(
    row: &jobs::JobRow,
    command: &Outgoing,
    retained: &crate::commands::RetainedCommand,
) -> Result<bool, EngineError> {
    if !is_provider_recreate(row)
        && !environment::present(row)
        && !global::is_restricted(row)
        && !lifecycle_provider(row)
    {
        return Ok(false);
    }
    command_identity::retained_matches(row, command_log(row), command, retained)
}

/// Consume only this owned job's exact retained receipt. Current forward authority is
/// deliberately not restored: an actual received result survives credential withdrawal.
/// Cancellation, abort and ownership still use the ordinary runner fences.
pub(crate) async fn require_receipt_identity(job: &Job, command: &Outgoing) -> Result<(), Stop> {
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
    if row.cancel_requested() {
        return Err(Stop::Cancelled);
    }
    if let Some(reason) = crate::jobs::runner::abort_reason(&row)? {
        return Err(reason);
    }
    let Some(retained) = retained else {
        return Err(refused());
    };
    if !retained_command_matches(&row, command, &retained)? {
        return Err(refused());
    }
    tx.rollback().await.map_err(DbError::from)?;
    Ok(())
}

async fn notice_bound(
    conn: &mut Conn,
    row: &jobs::JobRow,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    if kind != "turn.steer" {
        return Ok(false);
    }
    let Some((recipe, _, _)) = command_structure(conn, row).await? else {
        return Ok(false);
    };
    let Some(thread) = body
        .get("thread_id")
        .cloned()
        .map(serde_json::from_value::<Id>)
        .transpose()?
    else {
        return Ok(false);
    };
    let Some(turn) = body
        .get("turn_id")
        .cloned()
        .map(serde_json::from_value::<Id>)
        .transpose()?
    else {
        return Ok(false);
    };
    let Some(text) = body.get("text").and_then(Json::as_str) else {
        return Ok(false);
    };
    if !recipe.threads.contains(&thread)
        || key != SerialKey::Thread(thread).wire().as_deref()
        || command
            != ids::derived(
                "turn.steer",
                &[row.id.as_bytes(), thread.as_bytes(), text.as_bytes()],
            )
        || body
            .get("images")
            .is_some_and(|images| images != &json!([]))
        || body
            .get("native_action")
            .is_some_and(|action| !action.is_null())
    {
        return Ok(false);
    }
    if rows::thread_hold(conn, thread)
        .await?
        .is_none_or(|held| held.id != row.id)
    {
        return Ok(false);
    }
    let Some(actual) = threads::lock(conn, thread)
        .await?
        .filter(|thread| thread_matches(row, thread))
    else {
        return Ok(false);
    };
    if threads::active_turn(conn, thread)
        .await?
        .is_none_or(|active| active.id != turn || active.state != "running")
    {
        return Ok(false);
    }
    let provider_id = match recipe.target {
        Target::Provider(provider) => Some(provider),
        Target::Environment(_) => actual.provider_id,
    };
    let Some(provider_id) = provider_id else {
        return Ok(false);
    };
    let Some(provider) =
        orch_db::queries::resources::thread_engine::provider(conn, provider_id).await?
    else {
        return Ok(false);
    };
    let Some(environment) = actual.environment_id else {
        return Ok(false);
    };
    let Some(environment) = envs::environment(conn, environment).await? else {
        return Ok(false);
    };
    if crate::harnesses::machine_of(
        actual.arrangement.as_deref(),
        &provider,
        environment.machine_id,
    ) != Some(machine)
    {
        return Ok(false);
    }
    Ok(threads::notices::contains(conn, thread, &env_changes::notice_id(text)).await?)
}

/// The ordinary notice and this exact optional native steer are accepted in one guarded Tx.
pub(crate) async fn retain_notice(
    conn: &mut Conn,
    row: &jobs::JobRow,
    command: &Outgoing,
    clock: &dyn orch_types::Clock,
) -> Result<(), EngineError> {
    let now = clock.time();
    if !applies(row) {
        return Ok(());
    }
    if !current_with_clock(conn, row, clock).await?
        || !notice_bound(
            conn,
            row,
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &command.body,
        )
        .await?
    {
        return Err(EngineError::unreadable(
            "The provider update notice no longer has its accepted target.",
        ));
    }
    command_identity::remember(conn, row, notice_log(row), command, now).await
}
async fn accepted_notice(
    conn: &mut Conn,
    row: &jobs::JobRow,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    let native = canonical(id, machine, key, kind, body)?;
    if row
        .plan
        .get(notice_log(row))
        .and_then(|values| values.get(id.to_string()))
        != Some(&native)
    {
        return Ok(false);
    }
    notice_bound(conn, row, id, machine, key, kind, body).await
}

async fn command_bound(
    conn: &mut Conn,
    row: &jobs::JobRow,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    if global::is_restricted(row) {
        let Some(native) = global::structure(conn, row, true).await? else {
            return Ok(false);
        };
        return environment::command_bound_native(
            conn,
            row,
            native,
            (id, machine, key, kind, body),
        )
        .await;
    }
    if environment::present(row) {
        return environment::command_bound(conn, row, id, machine, key, kind, body).await;
    }
    if let Some(work) = body.get("work").filter(|work| !work.is_null()) {
        let work: orch_link_contract::commands::WorkReport = serde_json::from_value(work.clone())?;
        if work.job_id != row.id {
            return Ok(false);
        }
    }
    let Some((recipe, old, next)) = command_structure(conn, row).await? else {
        return Ok(false);
    };
    let Some(source) = hosts::host(conn, old.host_id).await? else {
        return Ok(false);
    };
    if matches!(
        kind,
        "node.network.apply"
            | "node.runtime.stop"
            | "node.runtime.export"
            | "node.runtime.volumes.export"
    ) {
        let effect = match kind {
            "node.network.apply" => {
                body == &serde_json::to_value(crate::network::apply(
                    old.id,
                    &recipe.network_policy,
                ))?
            }
            "node.runtime.stop" => {
                body == &serde_json::to_value(NodeRuntimeStop {
                    runtime_id: old.id,
                    reason: Some(orch_driver_contract::runtime::UnitReason::Recreate),
                })?
            }
            "node.runtime.export" => {
                body.get("path") == Some(&json!(format!("harness/{}", recipe.target.id())))
            }
            _ => true,
        };
        return Ok(effect
            && machine == source.machine_id
            && key == SerialKey::Runtime(old.id).wire().as_deref()
            && body.get("runtime_id") == Some(&json!(old.id)));
    }
    let Some(next) = next else {
        return Ok(false);
    };
    let Some(target) = hosts::host(conn, next.host_id).await? else {
        return Ok(false);
    };
    if matches!(kind, "unit.after_up" | "agent.start" | "provider.refresh") {
        if next.machine_id != Some(machine)
            || body.get("provider_id") != Some(&json!(recipe.target.id()))
        {
            return Ok(false);
        }
        return Ok(match kind {
            "provider.refresh" => key == SerialKey::Provider(recipe.target.id()).wire().as_deref(),
            "agent.start" => {
                let Some(state) = providers::state(conn, recipe.target.id()).await? else {
                    return Ok(false);
                };
                state.agent_id.is_some_and(|agent| {
                    body.get("agent_id") == Some(&json!(agent))
                        && key
                            == SerialKey::Harness {
                                provider_id: recipe.target.id(),
                                agent_id: agent,
                            }
                            .wire()
                            .as_deref()
                })
            }
            "unit.after_up" => {
                let run = ids::derived(
                    "unit.after_up.run",
                    &[row.id.as_bytes(), next.id.as_bytes()],
                );
                let command = body
                    .get("position")
                    .and_then(Json::as_u64)
                    .and_then(|position| usize::try_from(position).ok())
                    .and_then(|position| {
                        row.plan
                            .get("after_up_runs")
                            .and_then(|runs| runs.get(run.to_string()))
                            .and_then(|run| run.get("commands"))
                            .and_then(Json::as_array)
                            .and_then(|commands| commands.get(position))
                    });
                key == SerialKey::Runtime(next.id).wire().as_deref()
                    && body.get("run_id") == Some(&json!(run))
                    && body.get("environment_id").is_none_or(Json::is_null)
                    && body.get("agent_id").is_none_or(Json::is_null)
                    && command.is_some_and(|command| {
                        body.get("name") == command.get("name")
                            && body.pointer("/run/command") == command.get("command")
                            && body.pointer("/run/timeout_secs") == command.get("timeout_secs")
                    })
            }
            _ => false,
        });
    }
    if machine != target.machine_id {
        return Ok(false);
    }
    let driver = body.get("driver").and_then(Json::as_str);
    Ok(match kind {
        "node.runtime.create" => {
            key == SerialKey::Runtime(next.id).wire().as_deref()
                && body.get("runtime_id") == Some(&json!(next.id))
                && body.get("kind").and_then(Json::as_str) == Some("provider")
                && body.pointer("/labels/orch.provider_id") == Some(&json!(recipe.target.id()))
        }
        "node.runtime.import" => {
            key == SerialKey::Runtime(next.id).wire().as_deref()
                && body.get("runtime_id") == Some(&json!(next.id))
                && body.get("path") == Some(&json!(format!("harness/{}", recipe.target.id())))
                && body.get("archive") == row.plan.get("carried")
        }
        "node.runtime.start" | "node.runtime.volume.import" => {
            key == SerialKey::Runtime(next.id).wire().as_deref()
                && body.get("runtime_id") == Some(&json!(next.id))
        }
        "node.image.ensure" => {
            body.get("job_id") == Some(&json!(row.id))
                && driver == Some(next.driver.as_str())
                && body.get("spec") == Some(&recipe.definition)
                && key
                    == SerialKey::Image(super::super::provision::image_key(&recipe.definition))
                        .wire()
                        .as_deref()
        }
        "node.tool.ensure" => {
            body.get("job_id") == Some(&json!(row.id))
                && recipe.tools.iter().any(|tool| {
                    body.get("name") == Some(&json!(tool.name))
                        && body.get("version") == Some(&json!(tool.version))
                        && body.get("url") == Some(&json!(tool.url))
                        && body.get("sha256") == Some(&json!(tool.sha256))
                        && key == SerialKey::Tool(tool.name.clone()).wire().as_deref()
                })
        }
        "node.volume.ensure" => {
            driver == Some(next.driver.as_str())
                && next.volumes.as_object().is_some_and(|values| {
                    values.values().any(|volume| {
                        volume.get("name") == body.get("name")
                            && volume.get("storage").filter(|value| !value.is_null())
                                == body.get("storage").filter(|value| !value.is_null())
                            && volume
                                .get("name")
                                .and_then(Json::as_str)
                                .is_some_and(|name| {
                                    key == SerialKey::Volume(name.into()).wire().as_deref()
                                })
                    })
                })
        }
        _ => false,
    })
}

/// Keep the existing replacement's typed operands at the sole lifecycle proposal handoff.
/// This factual check grants no principal, scope, command or receipt authority.
pub(crate) async fn native_child_proposal(
    conn: &mut Conn,
    row: &jobs::JobRow,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    if lifecycle_provider(row) && is_provider_recreate(row) {
        return Ok(
            accepted_notice(conn, row, id, machine, key, kind, body).await?
                || command_bound(conn, row, id, machine, key, kind, body).await?,
        );
    }
    Ok(true)
}

/// Reuses the runner's canonical post-augmentation point and existing receipt shortcut.
pub async fn remember_command(job: &Job, command: &Outgoing) -> Result<(), Stop> {
    let initial = job.row().await?;
    if !applies(&initial) || lifecycle_provider(&initial) {
        // The final native lifecycle handoff records this exact proposal once.
        // Its Provider replacement operand guard remains an independent data check there.
        return Ok(());
    }
    let originally_scoped = is_restricted(&initial);
    // The existing issuer is first-write-wins. Read its immutable replay before taking
    // action locks, so a same-ID retry authorizes the actual stored frame and never
    // rewrites its body/deadline from subsequently changed transport settings.
    let retained = job
        .engine()
        .inner
        .commands
        .retained_command(command.id)
        .await
        .map_err(EngineError::from)?;
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    if environment::present(&initial)
        && !environment::current(&mut tx, &initial, job.engine().inner.clock.as_ref()).await?
        && !accepted_notice(
            &mut tx,
            &initial,
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &command.body,
        )
        .await?
    {
        return Err(refused());
    }
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if row.cancel_requested() {
        return Err(Stop::Cancelled);
    }
    if let Some(reason) = crate::jobs::runner::abort_reason(&row)? {
        return Err(reason);
    }
    if originally_scoped && !is_restricted(&row) {
        return Err(refused());
    }
    let mut actual = command.clone();
    if let Some(retained) = retained {
        let unlogged_ordinary = !is_restricted(&row)
            && row
                .plan
                .get(OWNER_COMMANDS)
                .and_then(|values| values.get(command.id.to_string()))
                .is_none();
        // A trusted runner may adopt an old unissued ordinary frame only when it
        // still equals the actual reconstructed command in every protected fact.
        let exact_ordinary = unlogged_ordinary
            && retained.machine == command.machine
            && retained.kind == command.kind.as_str()
            && retained.key == command.key.wire()
            && retained.body == command.body;
        if !exact_ordinary && !retained_command_matches(&row, command, &retained)? {
            return Err(refused());
        }
        actual.body = retained.body;
    }
    let notice = accepted_notice(
        &mut tx,
        &row,
        actual.id,
        actual.machine,
        actual.key.wire().as_deref(),
        actual.kind.as_str(),
        &actual.body,
    )
    .await?;
    if !notice
        && (!current_with_clock(&mut tx, &row, job.engine().inner.clock.as_ref()).await?
            || !command_bound(
                &mut tx,
                &row,
                actual.id,
                actual.machine,
                actual.key.wire().as_deref(),
                actual.kind.as_str(),
                &actual.body,
            )
            .await?
                && !(environment::present(&row) && environment::composed(&actual, row.id)))
    {
        return Err(refused());
    }
    if environment::present(&row) && !notice {
        if !environment::accept(&mut tx, &row, job.engine().inner.clock.as_ref()).await? {
            return Err(refused());
        }
        if environment::composed(&actual, row.id) {
            environment::remember_composed(&mut tx, &row, &actual, Time::new(job.now())).await?;
        }
    }
    command_identity::remember(
        &mut tx,
        &row,
        command_log(&row),
        &actual,
        Time::new(job.now()),
    )
    .await?;
    if environment::present(&row) && !notice {
        let fresh = jobs::job(&mut tx, row.id).await?.ok_or(Stop::Lost)?;
        environment::deadline(&mut tx, &fresh, job.engine().inner.clock.as_ref()).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// No unrelated command is changed; recognized pending IDs can never lose their original gate.
pub async fn command_authorized(
    conn: &mut Conn,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    now: Time,
) -> Result<bool, EngineError> {
    command_authorized_at(conn, id, machine, key, kind, body, (now, None)).await
}
async fn command_authorized_at(
    conn: &mut Conn,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    when: (Time, Option<&dyn orch_types::Clock>),
) -> Result<bool, EngineError> {
    let (now, clock) = when;
    let owners = jobs::lock_provider_update_command_owners(conn, id).await?;
    if owners.is_empty() {
        return Ok(true);
    }
    if owners.len() != 1 {
        return Ok(false);
    }
    let Some(row) = jobs::job(conn, owners[0]).await? else {
        return Ok(false);
    };
    if !recorded_command_matches(&row, id, machine, key, kind, body)? {
        return Ok(false);
    }
    if accepted_notice(conn, &row, id, machine, key, kind, body).await? {
        return Ok(true);
    }
    Ok(forward_current(conn, &row, now, clock).await?
        && command_bound(conn, &row, id, machine, key, kind, body).await?)
}

/// New unacknowledged forwarding retains all actual update origins to the final queue.
/// A recorded accepted notice remains factual completion, exactly as in command_authorized.
pub async fn command_current_callers(
    conn: &mut Conn,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    clock: &dyn orch_types::Clock,
) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
    if !command_authorized_at(
        conn,
        id,
        machine,
        key,
        kind,
        body,
        (clock.time(), Some(clock)),
    )
    .await?
    {
        return Ok(None);
    }
    let owners = jobs::lock_provider_update_command_owners(conn, id).await?;
    if owners.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if owners.len() != 1 {
        return Ok(None);
    }
    let Some(row) = jobs::job(conn, owners[0]).await? else {
        return Ok(None);
    };
    if accepted_notice(conn, &row, id, machine, key, kind, body).await? {
        return Ok(Some(Vec::new()));
    }
    if lifecycle_provider(&row) {
        let checked =
            crate::jobs::lifecycle_commands::environment::provider_current(conn, &row, clock)
                .await?;
        checked.check(clock)?;
        return Ok(Some(vec![(
            Some(checked.principal.caller().clone()),
            checked.until,
        )]));
    }
    if global::is_restricted(&row) {
        return global::current(conn, &row, clock).await;
    }
    if environment::present(&row) {
        return environment::callers(conn, &row, clock).await;
    }
    if !is_restricted(&row) {
        return Ok(Some(Vec::new()));
    }
    let Some(origins) = read::<Vec<CapturedOperation>>(&row, ORIGINS)? else {
        return Ok(None);
    };
    let mut callers = Vec::with_capacity(origins.len());
    for origin in origins {
        let Some(caller) = origin.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        callers.push((Some(caller), None));
    }
    Ok(Some(callers))
}

#[cfg(test)]
mod tests;
