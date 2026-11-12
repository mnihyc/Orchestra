//! Native Environment replacement sources inside the shared replacement-job authority mechanism.

use super::*;
use crate::updates::operation::Input;
use orch_types::Clock;

/// Existing private job-plan member that brands a known accepted Environment update.
pub const SOURCE: &str = "environment_update";
pub(super) const COMMANDS: &str = "environment_update_commands";
pub(super) const NOTICES: &str = "environment_update_notice_commands";
const OP: &str = "NodeEnvironmentUpdateCreate";

mod command;
pub use command::Command;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    source: Input,
    batch: Id,
    recipe_hash: Hash,
    old_host: Id,
    old_machine: Option<Id>,
    old_generation: i32,
    old_template: Id,
    old_driver: String,
    host_selection: Option<Id>,
}

pub(super) fn present(row: &jobs::JobRow) -> bool {
    [SOURCE, COMMANDS, NOTICES, COMPOSED]
        .iter()
        .any(|field| row.plan.get(*field).is_some())
}
fn binding(row: &jobs::JobRow) -> Result<Option<Binding>, EngineError> {
    read(row, SOURCE)
}
fn owner(row: &jobs::JobRow, recipe: &Recipe, binding: &Binding) -> bool {
    matches!(recipe.target, Target::Environment(id) if row.environment_id==Some(id) && binding.source.environment()==id)
        && row.kind == "environment_recreate"
        && row.provider_id.is_none()
        && row.batch_id == Some(binding.batch)
        && row.created_by == Some(recipe.by)
        && binding.source.origin().operation() == OP
        && binding.source.origin().user() == recipe.by
        && binding.source.execution_template() == Some(recipe.template)
        && recipe.reason == UpdatePreflightReason::Update
        && !recipe.auto_start
        && recipe.capability.is_none()
}

/// Attach only the exact freshly queued member, on the original action transaction.
pub async fn retain(
    conn: &mut Conn,
    job: Id,
    batch: Id,
    source: &Input,
    now: Time,
) -> Result<(), EngineError> {
    let row = jobs::lock(conn, job)
        .await?
        .ok_or_else(|| EngineError::unreadable("the accepted update job disappeared"))?;
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    let old = envs::runtime(conn, recipe.old)
        .await?
        .ok_or_else(|| EngineError::unreadable("the accepted update runtime disappeared"))?;
    let binding = Binding {
        source: source.clone(),
        batch,
        recipe_hash: recipe_hash(&recipe)?,
        old_host: old.host_id,
        old_machine: old.machine_id,
        old_generation: old.generation,
        old_template: old.template_id,
        old_driver: old.driver,
        host_selection: recipe.host,
    };
    if !owner(&row, &recipe, &binding)
        || row.state != "queued"
        || row.cancel_requested()
        || old.kind != "environment"
        || old.environment_id != row.environment_id
        || old.provider_id.is_some()
        || old.successor_of.is_some()
        || !source.matches(conn).await?
        || row
            .plan
            .get("runtime")
            .is_some_and(|value| !value.is_null())
        || present(&row)
    {
        return Err(EngineError::unreadable(
            "the source does not match the newly accepted update job",
        ));
    }
    jobs::merge_plan(conn, job, &json!({SOURCE:binding}), now.get()).await?;
    Ok(())
}

pub(super) async fn structure(
    conn: &mut Conn,
    row: &jobs::JobRow,
) -> Result<Option<(Recipe, envs::RuntimeFull, Option<envs::RuntimeFull>)>, EngineError> {
    structure_for(conn, row, false).await
}
async fn structure_for(
    conn: &mut Conn,
    row: &jobs::JobRow,
    cleanup: bool,
) -> Result<Option<(Recipe, envs::RuntimeFull, Option<envs::RuntimeFull>)>, EngineError> {
    let Some(binding) = binding(row)? else {
        return Ok(None);
    };
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    let mut accepted = recipe.clone();
    accepted.host = binding.host_selection;
    if !owner(row, &recipe, &binding)
        || recipe_hash(&accepted)? != binding.recipe_hash
        || row.plan.get("committed").and_then(Json::as_bool) == Some(true)
        || !cleanup
            && (!row.is_open()
                || row.cancel_requested()
                || crate::jobs::runner::abort_reason(row)?.is_some())
    {
        return Ok(None);
    }
    let next: Option<Id> = row
        .plan
        .get("runtime")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()?;
    let ids: Vec<Id> = sqlx::query_scalar("SELECT id FROM runtimes WHERE environment_id=$1 AND kind='environment' AND (id=$2 OR id=$3) ORDER BY id FOR UPDATE")
        .bind(recipe.target.id()).bind(recipe.old).bind(next).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    if !ids.contains(&recipe.old)
        || next.is_some_and(|next| next == recipe.old || !ids.contains(&next))
    {
        return Ok(None);
    }
    let Some(old) = envs::runtime(conn, recipe.old).await?.filter(|old| {
        old.kind == "environment"
            && old.environment_id == row.environment_id
            && old.provider_id.is_none()
            && old.host_id == binding.old_host
            && old.machine_id == binding.old_machine
            && old.generation == binding.old_generation
            && old.template_id == binding.old_template
            && old.driver == binding.old_driver
    }) else {
        return Ok(None);
    };
    if !cleanup
        && envs::live_runtime(conn, recipe.target.id())
            .await?
            .is_none_or(|current| current.id != old.id)
    {
        return Ok(None);
    }
    let target = binding.host_selection.unwrap_or(old.host_id);
    if (next.is_none() && recipe.host != binding.host_selection)
        || (next.is_some() && recipe.host != Some(target))
    {
        return Ok(None);
    }
    let next = match next {
        None => None,
        Some(id) => {
            let Some(next) = envs::runtime(conn, id).await?.filter(|next| {
                next.kind == "environment"
                    && next.environment_id == row.environment_id
                    && next.provider_id.is_none()
                    && next.successor_of == Some(old.id)
                    && next.generation == old.generation.saturating_add(1)
                    && next.host_id == target
                    && next.template_id == recipe.template
                    && next.driver == old.driver
                    && next.spec_hash == recipe.hash
                    && !matches!(next.state.as_str(), "retired" | "removed")
            }) else {
                return Ok(None);
            };
            if row.plan.get("host") != Some(&json!(target)) {
                return Ok(None);
            }
            Some(next)
        }
    };
    Ok(Some((recipe, old, next)))
}

async fn frame(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<Option<crate::updates::operation::Checkpoint>, EngineError> {
    let Some(before) = jobs::job(conn, row.id).await? else {
        return Ok(None);
    };
    let Some(earlier) = binding(&before)? else {
        return Ok(None);
    };
    // A factual actor fence grants nothing. Reload the accepted minimum after this wait.
    let _ = orch_db::queries::transaction_guard::lock_native_actor(
        conn,
        earlier.source.origin().user(),
    )
    .await?;
    let Some(actual) = jobs::job(conn, row.id).await? else {
        return Ok(None);
    };
    let Some(binding) = binding(&actual)? else {
        return Ok(None);
    };
    if !earlier.source.same_request(&binding.source) {
        return Ok(None);
    }
    let row = &actual;
    // This checks the real raw identity and native actor/unit before job/runtime row checks.
    let check = match binding.source.checkpoint(conn, clock).await {
        Ok(check) => check,
        Err(EngineError::BeforeEffect(error))
            if error.is::<crate::updates::operation::Ended>()
                || error.is::<crate::updates::operation::Denied>() =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if !check.matches() {
        return Ok(None);
    }
    if structure(conn, row).await?.is_none() {
        return Ok(None);
    }
    check.check(clock)?;
    Ok(Some(check))
}

pub(super) async fn current(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<bool, EngineError> {
    Ok(frame(conn, row, clock).await?.is_some())
}
/// The source's accepted minimum is metadata of the real native phase/command, never a new grant.
pub(super) async fn accept(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<bool, EngineError> {
    let Some(check) = frame(conn, row, clock).await? else {
        return Ok(false);
    };
    let Some(actual) = jobs::job(conn, row.id).await? else {
        return Ok(false);
    };
    let Some(mut binding) = binding(&actual)? else {
        return Ok(false);
    };
    binding.source = check.into_accepted(clock)?;
    jobs::merge_plan(conn, row.id, &json!({SOURCE:binding}), clock.now()).await?;
    Ok(true)
}

pub(super) async fn guard_in(conn: &mut Conn, job: &Job) -> Result<(), Stop> {
    let initial = jobs::job(conn, job.id()).await?.ok_or(Stop::Lost)?;
    if frame(conn, &initial, job.engine().inner.clock.as_ref())
        .await?
        .is_none()
    {
        return Err(refused());
    }
    let row = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
    if row.cancel_requested() {
        return Err(Stop::Cancelled);
    }
    if let Some(reason) = crate::jobs::runner::abort_reason(&row)? {
        return Err(reason);
    }
    if !accept(conn, &row, job.engine().inner.clock.as_ref()).await? {
        return Err(refused());
    }
    Ok(())
}

pub(super) async fn cleanup_guard(job: &Job) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if structure_for(&mut tx, &row, true).await?.is_none() {
        return Err(refused());
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

pub(super) async fn callers(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
    let Some(check) = frame(conn, row, clock).await? else {
        return Ok(None);
    };
    Ok(Some(vec![(
        Some(check.principal().caller().clone()),
        check.until,
    )]))
}

/// Final raw/key lifetime check without reopening already committed native source associations.
pub(super) async fn deadline(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    let source = binding(row)?
        .ok_or_else(|| EngineError::unreadable("the known update source is missing"))?
        .source;
    source.check_deadline(conn, clock).await
}

/// The native canonical proposal is a composition only when it already carries real source evidence.
pub(super) fn composed(command: &Outgoing, job: Id) -> bool {
    command
        .authority
        .as_ref()
        .is_some_and(|previous| previous.command_kind() == command.kind.as_str())
        && command.body.pointer("/work/job_id") == Some(&json!(job))
}
const COMPOSED: &str = "environment_update_composed_commands";
pub(super) async fn remember_composed(
    conn: &mut Conn,
    row: &jobs::JobRow,
    command: &Outgoing,
    now: Time,
) -> Result<(), EngineError> {
    let Some(previous) = &command.authority else {
        return Err(EngineError::unreadable(
            "the native composition source disappeared",
        ));
    };
    let evidence = json!({"frame":canonical(command.id,command.machine,command.key.wire().as_deref(),command.kind.as_str(),&command.body)?,"previous":hash(&serde_json::to_value(previous)?)?});
    if let Some(old) = row
        .plan
        .get(COMPOSED)
        .and_then(|values| values.get(command.id.to_string()))
    {
        if old.get("frame") != evidence.get("frame") {
            return Err(EngineError::unreadable(
                "the accepted composition frame changed",
            ));
        }
        return Ok(());
    }
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,ARRAY[$2],COALESCE(plan->$2,'{}'::jsonb)||$3),updated_at=$4 WHERE id=$1")
        .bind(row.id).bind(COMPOSED).bind(json!({command.id.to_string():evidence})).bind(now.get())
        .execute(&mut *conn).await.map_err(DbError::from)?;
    Ok(())
}

/// Factual commands of the native Environment replacement, distinct from its authority check.
pub(super) async fn command_bound(
    conn: &mut Conn,
    row: &jobs::JobRow,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    let Some((recipe, old, next)) = structure(conn, row).await? else {
        return Ok(false);
    };
    command_bound_native(
        conn,
        row,
        (recipe, old, next),
        (id, machine, key, kind, body),
    )
    .await
}

fn native_body(body: &Json) -> Json {
    let mut body = body.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("work");
    }
    body
}
fn image_matches(driver: &str, spec: &Json, job: Id, body: &Json) -> Result<bool, EngineError> {
    let expected = orch_link_contract::commands::NodeImageEnsure {
        driver: driver.to_owned(),
        spec: spec.clone(),
        job_id: Some(job),
    };
    Ok(native_body(body) == serde_json::to_value(expected)?)
}
fn volume_matches(driver: &str, volumes: &Json, key: Option<&str>, body: &Json) -> bool {
    let Ok(command) =
        serde_json::from_value::<orch_link_contract::commands::NodeVolumeEnsure>(native_body(body))
    else {
        return false;
    };
    command.driver == driver
        && key == SerialKey::Volume(command.name.clone()).wire().as_deref()
        && volumes.as_object().is_some_and(|volumes| {
            volumes.values().any(|volume| {
                volume.get("name").and_then(Json::as_str) == Some(command.name.as_str())
                    && volume.get("storage").and_then(Json::as_str) == command.storage.as_deref()
            })
        })
}

/// Native shape checks over an already validated original/candidate pair.
pub(super) async fn command_bound_native(
    conn: &mut Conn,
    row: &jobs::JobRow,
    native: (Recipe, envs::RuntimeFull, Option<envs::RuntimeFull>),
    envelope: (Id, Id, Option<&str>, &str, &Json),
) -> Result<bool, EngineError> {
    let (recipe, old, next) = native;
    let (id, machine, key, kind, body) = envelope;
    if body
        .get("work")
        .filter(|work| !work.is_null())
        .is_some_and(|work| work.get("job_id") != Some(&json!(row.id)))
    {
        return Ok(false);
    }
    if let Some(kept) = row
        .plan
        .get(COMPOSED)
        .and_then(|values| values.get(id.to_string()))
    {
        return Ok(kept.get("frame") == Some(&canonical(id, machine, key, kind, body)?));
    }
    let Some(host) = hosts::host(conn, old.host_id).await? else {
        return Ok(false);
    };
    if matches!(
        kind,
        "node.network.apply" | "node.runtime.stop" | "node.runtime.keep-files"
    ) {
        let expected = match kind {
            "node.network.apply" => {
                serde_json::to_value(crate::network::apply(old.id, &recipe.network_policy))?
            }
            "node.runtime.stop" => serde_json::to_value(NodeRuntimeStop {
                runtime_id: old.id,
                reason: Some(orch_driver_contract::runtime::UnitReason::Recreate),
            })?,
            _ => serde_json::to_value(orch_link_contract::commands::NodeKeepFiles {
                runtime_id: old.id,
                environment_id: recipe.target.id(),
                generation: i64::from(old.generation) + 1,
                capture: row.id,
            })?,
        };
        // WorkReport is added by the native runner after the typed body is produced.
        let mut factual = body.clone();
        if let Some(object) = factual.as_object_mut() {
            object.remove("work");
        }
        return Ok(machine == host.machine_id
            && key == SerialKey::Runtime(old.id).wire().as_deref()
            && factual == expected);
    }
    let Some(next) = next else {
        return Ok(false);
    };
    if matches!(kind, "env.attach" | "exec.start" | "unit.after_up") {
        if next.machine_id != Some(machine)
            || body.get("environment_id") != Some(&json!(recipe.target.id()))
        {
            return Ok(false);
        }
        return Ok(match kind {
            "env.attach" | "exec.start" => {
                key == SerialKey::Environment(recipe.target.id()).wire().as_deref()
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
                    && command.is_some_and(|command| {
                        body.get("name") == command.get("name")
                            && body.pointer("/run/command") == command.get("command")
                            && body.pointer("/run/timeout_secs") == command.get("timeout_secs")
                    })
            }
            _ => false,
        });
    }
    let Some(host) = hosts::host(conn, next.host_id).await? else {
        return Ok(false);
    };
    if machine != host.machine_id {
        return Ok(false);
    }
    Ok(match kind {
        "node.runtime.create" => {
            key == SerialKey::Runtime(next.id).wire().as_deref()
                && body.get("runtime_id") == Some(&json!(next.id))
                && body.get("kind").and_then(Json::as_str) == Some("environment")
                && body.pointer("/labels/orch.environment_id") == Some(&json!(recipe.target.id()))
        }
        "node.runtime.start" => {
            key == SerialKey::Runtime(next.id).wire().as_deref()
                && body.get("runtime_id") == Some(&json!(next.id))
        }
        "node.image.ensure" => {
            image_matches(&next.driver, &recipe.definition, row.id, body)?
                && key
                    == SerialKey::Image(super::super::super::provision::image_key(
                        &recipe.definition,
                    ))
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
        "node.volume.ensure" => volume_matches(&next.driver, &next.volumes, key, body),
        _ => false,
    })
}

/// Seal the canonical proposal at the common runner handoff, retaining any original inner authority.
pub(super) async fn seal(job: &Job, command: &mut Outgoing) -> Result<(), Stop> {
    let initial = job.row().await?;
    if !present(&initial) {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await?;
    let notice = super::accepted_notice(
        &mut tx,
        &initial,
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )
    .await?;
    if !notice
        && frame(&mut tx, &initial, job.engine().inner.clock.as_ref())
            .await?
            .is_none()
    {
        return Err(refused());
    }
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if let Some(stored) =
        orch_db::queries::link::command_with_authority(&mut tx, command.id).await?
    {
        let actual = canonical(
            stored.command.id,
            stored.command.machine_id,
            stored.command.serial_key(),
            &stored.command.kind,
            &stored.command.body,
        )?;
        if actual
            != canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?
        {
            return Err(refused());
        }
        let authority: crate::commands::CommandAuthority = stored
            .authority
            .map(serde_json::from_value)
            .transpose()
            .map_err(EngineError::from)?
            .ok_or_else(refused)?;
        if !matches!(
            authority,
            crate::commands::CommandAuthority::EnvironmentUpdate(_)
        ) {
            return Err(refused());
        }
        command.authority = Some(authority);
    } else {
        command.authority = Some(crate::commands::CommandAuthority::environment_update(
            Command::capture(&row, command)?,
        ));
    }
    if !notice {
        deadline(&mut tx, &row, job.engine().inner.clock.as_ref()).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests;
