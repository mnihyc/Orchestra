//! The creator that accepted this new environment remains the authority for its one pending provision.
use super::*;
use crate::operation_scope::{Prepared, compatible_selection, creator_files, targets};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::{NodePermission, ObjectKind, PermissionName};
use orch_api_contract::operations::TargetResolver;
use orch_authz::{Act, CapturedOperation, Policy};
use orch_db::Conn;
use orch_types::Hash;
use serde::{Deserialize, Serialize};

const FIELD: &str = "creator_provision";
const COMMANDS: &str = "creator_provision_commands";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authority {
    origin: CapturedOperation,
    thread: Id,
    environment: Id,
    home: Id,
    job: Id,
    template: Id,
}

fn read(row: &JobRow) -> Result<Option<Authority>, EngineError> {
    if crate::environment_creation::standalone(row) {
        return Ok(None);
    }
    row.plan
        .get(FIELD)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

/// Called only by the creator's private new-environment prefix after full scope admission.
/// The ordinary hosted writer supplies both identities; a pre-existing environment/job is excluded.
pub async fn retain(
    conn: &mut Conn,
    origin: &CapturedOperation,
    thread: Id,
    environment: Id,
    home: Id,
    job: Id,
    new_created: bool,
    now: Time,
) -> Result<(), EngineError> {
    if !new_created
        || !matches!(
            origin.operation(),
            "ThreadCreate" | "TopicCreate.with_thread"
        )
    {
        return Err(EngineError::unreadable(
            "Only this creator's new provision can retain its origin.",
        ));
    }
    let row = jobs::lock(conn, job)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new provision disappeared."))?;
    let env = env_rows::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new environment disappeared."))?;
    let actual = orch_db::queries::threads::thread(conn, thread)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new thread disappeared."))?;
    if row.kind != kind::ENVIRONMENT_PROVISION
        || row.environment_id != Some(environment)
        || row.state != "queued"
        || row.cancel_requested()
        || row.created_by != Some(origin.user())
        || row.plan.get("mode").and_then(Json::as_str) != Some("make")
        || row.plan.get("generation").and_then(Json::as_i64) != Some(0)
        || row
            .plan
            .get("runtime")
            .is_some_and(|value| !value.is_null())
        || env.generation != 0
        || env.placement != "managed"
        || env.deleted_at.is_some()
        || env.home_node_id != home
        || env.owner_id != origin.user()
        || actual.environment_id != Some(environment)
    {
        return Err(EngineError::unreadable(
            "The creator no longer owns this exact new-environment prefix.",
        ));
    }
    let authority = Authority {
        origin: origin.clone(),
        thread,
        environment,
        home,
        job,
        template: env
            .template_id
            .ok_or_else(|| EngineError::unreadable("The new template is missing."))?,
    };
    if crate::environment_creation::standalone(&row) {
        return Err(EngineError::unreadable(
            "the native provision already has a standalone creator",
        ));
    }
    if let Some(previous) = read(&row)? {
        if previous != authority {
            return Err(EngineError::unreadable(
                "The new provision already belongs to another creator.",
            ));
        }
        return Ok(());
    }
    jobs::merge_plan(conn, job, &json!({FIELD:authority}), now.get()).await?;
    Ok(())
}

pub(super) async fn principal(
    conn: &mut Conn,
    row: &JobRow,
    now: Time,
) -> Result<Option<Principal>, EngineError> {
    if crate::environment_creation::standalone(row) {
        return crate::environment_creation::principal_at(conn, row, now)
            .await
            .map(Some);
    }
    let Some(authority) = read(row)? else {
        return Ok(None);
    };
    if row.id != authority.job
        || row.kind != kind::ENVIRONMENT_PROVISION
        || !row.is_open()
        || row.cancel_requested()
        || crate::jobs::runner::abort_reason(row)?.is_some()
        || row.environment_id != Some(authority.environment)
        || row.plan.get("mode").and_then(Json::as_str) != Some("make")
        || row.plan.get("generation").and_then(Json::as_i64) != Some(0)
        || row.created_by != Some(authority.origin.user())
    {
        return Err(EngineError::unreadable(
            "The accepted creator provision no longer owns its work.",
        ));
    }
    let env = env_rows::environment(conn, authority.environment)
        .await?
        .filter(|env| {
            env.deleted_at.is_none()
                && env.generation == 0
                && env.status == status::PROVISIONING
                && env.home_node_id == authority.home
                && env.template_id == Some(authority.template)
        })
        .ok_or_else(|| EngineError::unreadable("The accepted new-environment binding changed."))?;
    let caller = authority
        .origin
        .restore(conn, now)
        .await?
        .ok_or_else(|| EngineError::unreadable("The creator credential is no longer valid."))?;
    let plan =
        Prepared::for_selection(authority.origin.operation()).map_err(EngineError::unreadable)?;
    if !compatible_selection(&caller, plan.variant()) {
        return Err(EngineError::unreadable(
            "The creator operation is no longer permitted.",
        ));
    }
    let mut facets = creator_files::parent_facets(
        conn,
        authority.origin.operation(),
        authority.thread,
        authority.environment,
    )
    .await?
    .ok_or_else(|| EngineError::unreadable("The creator parent changed."))?;
    let home = targets::node(conn, env.home_node_id)
        .await
        .map_err(EngineError::unreadable)?;
    facets.extend(creator_files::required_facet(
        &plan,
        TargetResolver::ResolvedCollection("creator_new_environment_homes"),
        KeyAction::Write,
        KeyTargetKind::Object(ObjectKind::Environment),
        &home,
    )?);
    let caller = caller
        .admit_facets(&facets)
        .map_err(EngineError::unreadable)?
        .into_caller();
    let principal = Principal::load(conn, caller, now).await?;
    if !orch_authz::nodes::check_named(
        conn,
        &principal,
        env.home_node_id,
        PermissionName::OfNode(NodePermission::Interact),
        Act::Use,
        Policy::default(),
    )
    .await?
    .is_allowed()
    {
        return Err(EngineError::unreadable(
            "The creator no longer has the environment home's Interact permission.",
        ));
    }
    let template = host_rows::template(conn, authority.template)
        .await?
        .filter(|template| {
            template.enabled && template.deleted_at.is_none() && template.kind == "environment"
        })
        .ok_or_else(|| EngineError::unreadable("The admitted template is unavailable."))?;
    if let Some(runtime) = row
        .plan
        .get("runtime")
        .and_then(Json::as_str)
        .and_then(|runtime| runtime.parse::<Id>().ok())
    {
        let current = env_rows::live_runtime(conn, authority.environment)
            .await?
            .filter(|candidate| {
                candidate.id == runtime
                    && candidate.environment_id == Some(authority.environment)
                    && candidate.template_id == authority.template
                    && candidate.successor_of.is_none()
                    // This is the same existing Make candidate predicate used by source files;
                    // publication still stamps the actual runtime's generation, never a guess.
                    && env.generation.checked_add(1) == Some(candidate.generation)
                    && matches!(candidate.state.as_str(), "creating" | "running")
            })
            .ok_or_else(|| EngineError::unreadable("The admitted provision candidate changed."))?;
        if row.plan.get("host") != Some(&json!(current.host_id)) {
            return Err(EngineError::unreadable(
                "The admitted provision host changed.",
            ));
        }
        let wanted = places::wanted(conn, env.home_node_id, template, &env.config).await?;
        let minutes_idle = setting::<i64>(
            conn,
            SettingsPlace::Environment(env.id),
            keys::ENV_RECYCLE_IDLE_MINUTES,
        )
        .await?
        .unwrap_or(IDLE_MINUTES_DEFAULT);
        let places = places::host_places_for_environment(
            conn,
            &principal,
            &Ask {
                home: env.home_node_id,
                wanted: &wanted,
                current: None,
                minutes_idle,
                now: now.get(),
            },
            env.id,
        )
        .await?;
        // created proves current HostUse/TemplateUse including their actual options. Later load
        // or physical capacity warnings do not invent a new phase-authorization condition.
        if !places.iter().any(|place| {
            place.host.id == current.host_id && !place.host.disabled && place.created.is_some()
        }) {
            return Err(EngineError::unreadable(
                "The creator no longer has the candidate host/template Use.",
            ));
        }
    }
    Ok(Some(principal))
}

pub(super) async fn phase(job: &Job, phase: &str) -> Result<(), Stop> {
    if job.plan().await?.get(FIELD).is_none() {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if read(&owner)?.is_none() && !crate::environment_creation::standalone(&owner) {
        return Ok(());
    }
    if phase == "commit"
        && owner
            .plan
            .get("provision_committed")
            .and_then(Json::as_bool)
            == Some(true)
    {
        return Ok(());
    }
    if crate::environment_creation::standalone(&owner) {
        crate::environment_creation::principal_current(
            &mut tx,
            &owner,
            job.engine().inner.clock.as_ref(),
            None,
        )
        .await?;
    } else {
        principal(&mut tx, &owner, Time::new(job.now())).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

fn body_hash(body: &Json) -> Result<Hash, serde_json::Error> {
    // Issued bodies pass through JSONB before first send. Object key order is
    // transport representation, so preserve the same effect across that roundtrip.
    let mut body = body.clone();
    body.sort_all_objects();
    Ok(Hash::of(&serde_json::to_vec(&body)?))
}

fn canonical(command: &Outgoing) -> Result<Json, EngineError> {
    Ok(
        json!({"id":command.id,"machine":command.machine,"kind":command.kind.as_str(),
        "key":command.key.wire(),"body_hash":body_hash(&command.body)?.to_string()}),
    )
}

/// Runs at the job runner's canonical post-augmentation point for only retained creator jobs.
pub async fn remember_command(job: &Job, command: &Outgoing) -> Result<(), Stop> {
    if job.plan().await?.get(FIELD).is_none() {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if read(&owner)?.is_none() && !crate::environment_creation::standalone(&owner) {
        return Ok(());
    }
    if crate::environment_creation::standalone(&owner) {
        crate::environment_creation::principal_current(
            &mut tx,
            &owner,
            job.engine().inner.clock.as_ref(),
            None,
        )
        .await?;
    } else {
        principal(&mut tx, &owner, Time::new(job.now())).await?;
    }
    let native = canonical(command)?;
    let mut records = owner
        .plan
        .get(COMMANDS)
        .cloned()
        .unwrap_or_else(|| json!({}));
    let map = records
        .as_object_mut()
        .ok_or_else(|| EngineError::unreadable("The creator command journal is invalid."))?;
    if map
        .get(&command.id.to_string())
        .is_some_and(|old| old != &native)
    {
        return Err(EngineError::unreadable("The creator command identity changed.").into());
    }
    map.insert(command.id.to_string(), native);
    // IDs derive from this job and its finite template/phase operands. Restart repeats IDs;
    // existing Busy retry policy keeps its original deadline. No new retry counter or TTL exists.
    jobs::merge_plan(&mut tx, job.id(), &json!({COMMANDS:records}), job.now()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// First-send validation. The dispatcher preserves actual Accepted/terminal receipts before this call.
pub async fn command_authorized(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    now: Time,
) -> Result<bool, EngineError> {
    let owners = jobs::lock_creator_provision_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(true);
    }
    if owners.len() != 1 {
        return Ok(false);
    }
    let Some(owner) = jobs::job(conn, owners[0]).await? else {
        return Ok(false);
    };
    if owner.plan.pointer("/active_command/id") != Some(&json!(command))
        || owner.plan.pointer("/active_command/machine") != Some(&json!(machine))
        || owner
            .plan
            .pointer("/active_command/kind")
            .and_then(Json::as_str)
            != Some(kind)
    {
        return Ok(false);
    }
    let native = json!({"id":command,"machine":machine,"kind":kind,"key":key,
        "body_hash":body_hash(body)?.to_string()});
    if owner
        .plan
        .get(COMMANDS)
        .and_then(Json::as_object)
        .and_then(|map| map.get(&command.to_string()))
        != Some(&native)
    {
        return Ok(false);
    }
    match principal(conn, &owner, now).await {
        Ok(principal) => Ok(principal.is_some()),
        // A withdrawn creator or changed retained binding refuses this new effect.
        // Database/access-engine failures retain their actual error category.
        Err(EngineError::Unreadable(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Current identity/time metadata for NEW first-send only; no completed-result reauthorization.
pub async fn command_current_callers(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    clock: &dyn orch_types::Clock,
) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
    if !command_authorized(conn, command, machine, key, kind, body, clock.time()).await? {
        return Ok(None);
    }
    let owners = jobs::lock_creator_provision_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if owners.len() != 1 {
        return Ok(None);
    }
    let Some(owner) = jobs::job(conn, owners[0]).await? else {
        return Ok(None);
    };
    if crate::environment_creation::standalone(&owner) {
        return match crate::environment_creation::principal_current(conn, &owner, clock, None).await
        {
            Ok(principal) => Ok(Some(vec![(Some(principal.caller().clone()), None)])),
            Err(EngineError::Unreadable(_)) => Ok(None),
            Err(error) => Err(error),
        };
    }
    let Some(authority) = read(&owner)? else {
        return Ok(None);
    };
    Ok(authority
        .origin
        .restore_current(conn, clock)
        .await?
        .map(|caller| vec![(Some(caller), None)]))
}

#[cfg(test)]
mod tests;
