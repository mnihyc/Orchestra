//! The original creator remains the authority for this provider's one pending initial Make.
use super::*;
use crate::operation_scope::{BindingError, Prepared, targets};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::CapturedOperation;
use orch_db::Conn;
use orch_db::queries::jobs;
use orch_types::Hash;
use serde::{Deserialize, Serialize};

const FIELD: &str = "provider_creator";
const COMMANDS: &str = "provider_creator_commands";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authority {
    origin: CapturedOperation,
    provider: Id,
    home: Id,
    job: Id,
    template: Id,
    named_host: Option<Id>,
    key_machine: Option<Id>,
    agent: Option<Id>,
    kind: String,
    harness: Option<String>,
    runtime_config_hash: Hash,
}

fn read(row: &JobRow) -> Result<Option<Authority>, EngineError> {
    row.plan
        .get(FIELD)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

pub(super) fn is_retained(row: &JobRow) -> bool {
    row.plan.get(FIELD).is_some()
}

pub(super) fn stop(error: EngineError) -> Stop {
    match error {
        EngineError::Unreadable(_) => Failure::new(
            "access.denied",
            "The accepted provider creation is no longer authorized for this work.",
        )
        .into(),
        error => error.into(),
    }
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

fn body_hash(body: &Json) -> Result<Hash, serde_json::Error> {
    let mut body = body.clone();
    body.sort_all_objects();
    Ok(Hash::of(&serde_json::to_vec(&body)?))
}

async fn template_and_host(conn: &mut Conn, provider: Id) -> Result<(Id, Option<Id>), EngineError> {
    let template = setting::<Id>(
        conn,
        SettingsPlace::Provider(provider),
        keys::RUNTIME_TEMPLATE_ID,
    )
    .await?
    .unwrap_or(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID);
    let host = setting::<Id>(
        conn,
        SettingsPlace::Provider(provider),
        keys::RUNTIME_HOST_ID,
    )
    .await?;
    Ok((template, host))
}

/// The fully admitted creator supplies this newly inserted provider and its exact Made job
/// before their transaction commits. Existing-provider and autonomous-start jobs cannot attach.
pub async fn retain(
    conn: &mut Conn,
    origin: &CapturedOperation,
    provider: Id,
    home: Id,
    job: Id,
    now: Time,
) -> Result<(), EngineError> {
    let row = jobs::lock(conn, job)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new provider job disappeared."))?;
    let current = rows::state(conn, provider)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new provider disappeared."))?;
    if origin.operation() != "ProviderCreate"
        || row.kind != provider_jobs::PROVISION
        || row.provider_id != Some(provider)
        || row.environment_id.is_some()
        || row.state != "queued"
        || row.cancel_requested()
        || row.created_by != Some(origin.user())
        || row.plan.get("mode").and_then(Json::as_str) != Some("make")
        || row
            .plan
            .get("runtime")
            .is_some_and(|value| !value.is_null())
        || row.plan.get("provider_recovery").is_some()
        || row.plan.get("target").is_some()
        || current.deleted_at.is_some()
        || current.owner_id != origin.user()
        || current.home_id != home
        || current.machine_id.is_some()
        || !matches!(current.kind.as_str(), "api" | "subscription")
        || rows::live_runtime(conn, provider).await?.is_some()
    {
        return Err(EngineError::unreadable(
            "This is not the creator's exact new-provider Make.",
        ));
    }
    let (template, named_host) = template_and_host(conn, provider).await?;
    let authority = Authority {
        origin: origin.clone(),
        provider,
        home,
        job,
        template,
        named_host,
        key_machine: current.key_machine_id,
        agent: current.agent_id,
        kind: current.kind,
        harness: current.harness_kind,
        runtime_config_hash: body_hash(&current.runtime_config)?,
    };
    if let Some(previous) = read(&row)? {
        if previous != authority {
            return Err(EngineError::unreadable(
                "This provider job already has another creator.",
            ));
        }
        return Ok(());
    }
    jobs::merge_plan(conn, job, &json!({FIELD:authority}), now.get()).await?;
    Ok(())
}

fn slot(
    plan: &Prepared,
    resolver: TargetResolver,
    action: KeyAction,
    kind: ObjectKind,
) -> Result<usize, EngineError> {
    let mut found = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.target == resolver);
    let Some((index, facet)) = found.next() else {
        return Err(EngineError::unreadable(
            "The provider creator effect is not declared.",
        ));
    };
    if found.next().is_some()
        || facet.usage != FacetUsage::Required
        || facet.action != action
        || facet.resources != [KeyTargetKind::Object(kind)]
    {
        return Err(EngineError::unreadable(
            "The provider creator effect metadata changed.",
        ));
    }
    Ok(index)
}

/// Current residual creation bounds shared with the creator's optional pending login.
/// Referencing a machine uses its existing ordinary Use permission; ProviderCreate does
/// not acquire an invented MachineWrite facet. MachineProviderCreate keeps its declared path.
pub async fn creator_principal(
    conn: &mut Conn,
    origin: &CapturedOperation,
    provider: Id,
    now: Time,
) -> Result<Option<Principal>, EngineError> {
    if !matches!(
        origin.operation(),
        "ProviderCreate" | "MachineProviderCreate"
    ) {
        return Ok(None);
    }
    let Some(current) = rows::state(conn, provider)
        .await?
        .filter(|state| state.deleted_at.is_none() && state.status != "disabled")
    else {
        return Ok(None);
    };
    if crate::preflight::second_step_missing(conn, origin.user(), current.home_id).await? {
        return Ok(None);
    }
    let Some(caller) = origin.restore(conn, now).await? else {
        return Ok(None);
    };
    let mut plan = Prepared::for_selection(origin.operation()).map_err(EngineError::unreadable)?;
    let home = match targets::node(conn, current.home_id).await {
        Ok(home) if home.object.kind == KeyTargetKind::Object(ObjectKind::Org) => home,
        Ok(_) | Err(targets::TargetError::Missing) => return Ok(None),
        Err(targets::TargetError::Db(error)) => return Err(error.into()),
        Err(targets::TargetError::Authz(error)) if unavailable(&error) => return Ok(None),
        Err(targets::TargetError::Authz(error)) => return Err(error.into()),
    };
    let home_slot = slot(
        &plan,
        TargetResolver::Resolved("home"),
        KeyAction::Manage,
        ObjectKind::Provider,
    )?;
    plan.bind(
        home_slot,
        KeyTargetKind::Object(ObjectKind::Provider),
        &[home],
    )
    .map_err(EngineError::unreadable)?;
    let mut referenced = Vec::new();
    if origin.operation() == "MachineProviderCreate" {
        let Some(machine) = current
            .machine_id
            .filter(|machine| current.key_machine_id == Some(*machine))
        else {
            return Ok(None);
        };
        let machine_slot = slot(
            &plan,
            TargetResolver::Path("id"),
            KeyAction::Write,
            ObjectKind::Machine,
        )?;
        let target = match targets::machine_subject(conn, machine).await {
            Ok(target) => target,
            Err(targets::TargetError::Missing) => return Ok(None),
            Err(targets::TargetError::Db(error)) => return Err(error.into()),
            Err(targets::TargetError::Authz(error)) if unavailable(&error) => return Ok(None),
            Err(targets::TargetError::Authz(error)) => return Err(error.into()),
        };
        plan.bind(
            machine_slot,
            KeyTargetKind::Object(ObjectKind::Machine),
            &[target],
        )
        .map_err(EngineError::unreadable)?;
        referenced.push(machine);
    } else if rows::live_runtime(conn, provider).await?.is_none() {
        referenced.extend(current.machine_id);
    }
    referenced.extend(current.key_machine_id);
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
    let object = ObjectRef {
        kind: ResourceKind::Provider.into(),
        id: provider,
    };
    let provider_allowed = match resources::needs(
        conn,
        &principal,
        &resources::Needs {
            required: Permission::ProviderManage,
            anchor: object,
            through: Some(current.home_id),
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
        Ok(answer) => answer.is_allowed(),
        Err(error) if unavailable(&error) => false,
        Err(error) => return Err(error.into()),
    };
    if !provider_allowed {
        return Ok(None);
    }
    referenced.sort_unstable();
    referenced.dedup();
    for machine in referenced {
        let machine_allowed = match resources::check_machine(
            conn,
            &principal,
            machine,
            Permission::MachineUse,
            Policy::default(),
        )
        .await
        {
            Ok(answer) => answer.is_allowed(),
            Err(error) if unavailable(&error) => false,
            Err(error) => return Err(error.into()),
        };
        if !orch_db::queries::resources::machine(conn, machine)
            .await?
            .is_some_and(|row| row.deleted_at.is_none() && row.is_active())
            || !machine_allowed
        {
            return Ok(None);
        }
    }
    Ok(Some(principal))
}

pub(super) async fn principal(
    conn: &mut Conn,
    row: &JobRow,
    now: Time,
) -> Result<Option<Principal>, EngineError> {
    let Some(authority) = read(row)? else {
        return Ok(None);
    };
    if row.id != authority.job
        || row.kind != provider_jobs::PROVISION
        || row.provider_id != Some(authority.provider)
        || row.environment_id.is_some()
        || !row.is_open()
        || row.cancel_requested()
        || crate::jobs::runner::abort_reason(row)?.is_some()
        || row.created_by != Some(authority.origin.user())
        || row.plan.get("mode").and_then(Json::as_str) != Some("make")
        || row.plan.get("provider_recovery").is_some()
        || row.plan.get("target").is_some()
    {
        return Err(EngineError::unreadable(
            "The creator no longer owns this provider Make.",
        ));
    }
    let current = rows::lock(conn, authority.provider)
        .await?
        .filter(|state| {
            state.deleted_at.is_none()
                && state.status != "disabled"
                && state.machine_id.is_none()
                && state.owner_id == authority.origin.user()
                && state.home_id == authority.home
                && state.key_machine_id == authority.key_machine
                && state.agent_id == authority.agent
                && state.kind == authority.kind
                && state.harness_kind == authority.harness
        })
        .ok_or_else(|| EngineError::unreadable("The accepted provider binding changed."))?;
    if body_hash(&current.runtime_config)? != authority.runtime_config_hash
        || template_and_host(conn, current.id).await? != (authority.template, authority.named_host)
    {
        return Err(EngineError::unreadable(
            "The accepted provider runtime choice changed.",
        ));
    }
    let principal = creator_principal(conn, &authority.origin, authority.provider, now)
        .await?
        .ok_or_else(|| {
            EngineError::unreadable("The provider creator credential is no longer authorized.")
        })?;
    let template = host_rows::template(conn, authority.template)
        .await?
        .filter(|template| {
            template.enabled && template.deleted_at.is_none() && template.kind == "provider"
        })
        .ok_or_else(|| EngineError::unreadable("The provider creator template is unavailable."))?;
    if !resources::provider_creation_template_use(conn, &principal, current.id, template.id).await?
    {
        return Err(EngineError::unreadable(
            "The creator no longer has the provider template's Use.",
        ));
    }
    let planned = row
        .plan
        .get("runtime")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value::<Id>)
        .transpose()?;
    if let Some(runtime) = planned {
        if !jobs::lock_provider_creator_candidate(conn, authority.provider, runtime).await? {
            return Err(EngineError::unreadable(
                "The provider creator candidate disappeared.",
            ));
        }
    }
    let candidate = rows::live_runtime(conn, authority.provider).await?;
    match row.plan.get("runtime").filter(|value| !value.is_null()) {
        Some(value) => {
            let runtime: Id = serde_json::from_value(value.clone())?;
            let candidate = candidate
                .filter(|candidate| {
                    candidate.id == runtime
                        && candidate.kind == "provider"
                        && candidate.provider_id == Some(authority.provider)
                        && candidate.environment_id.is_none()
                        && candidate.template_id == authority.template
                        && candidate.generation == 1
                        && candidate.successor_of.is_none()
                        && matches!(candidate.state.as_str(), "creating" | "running")
                })
                .ok_or_else(|| {
                    EngineError::unreadable("The creator's exact first provider candidate changed.")
                })?;
            if row.plan.get("host") != Some(&json!(candidate.host_id)) {
                return Err(EngineError::unreadable(
                    "The provider creator host binding changed.",
                ));
            }
            let host = host_rows::host(conn, candidate.host_id)
                .await?
                .filter(|host| {
                    !host.disabled
                        && host.machine_status == "active"
                        && (host.owner_id.is_none()
                            || host.owner_id == Some(current.owner_id)
                            || authority.named_host == Some(host.id))
                })
                .ok_or_else(|| {
                    EngineError::unreadable("The provider creator host is unavailable.")
                })?;
            if !resources::provider_creation_host_use(conn, &principal, current.id, host.id).await?
            {
                return Err(EngineError::unreadable(
                    "The creator no longer has the provider host's Use.",
                ));
            }
        }
        None if candidate.is_some() => {
            return Err(EngineError::unreadable(
                "Another provider candidate already owns this Make.",
            ));
        }
        None => {}
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
    if phase == "commit"
        && owner
            .plan
            .get("provider_creator_committed")
            .and_then(Json::as_bool)
            == Some(true)
    {
        return Ok(());
    }
    principal(&mut tx, &owner, Time::new(job.now()))
        .await
        .map_err(stop)?
        .ok_or_else(|| {
            stop(EngineError::unreadable(
                "The retained provider creator disappeared.",
            ))
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

fn canonical(command: &Outgoing) -> Result<Json, EngineError> {
    Ok(
        json!({"id":command.id,"machine":command.machine,"kind":command.kind.as_str(),"key":command.key.wire(),
        "body_hash":body_hash(&command.body)?.to_string()}),
    )
}

// These are the existing first-Make command destinations, not a generic job callback. The
// host executes lifecycle commands; only after-start commands run on the new connector.
async fn command_binding(
    conn: &mut Conn,
    owner: &JobRow,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    let Some(authority) = read(owner)? else {
        return Ok(false);
    };
    let Some(runtime) = rows::live_runtime(conn, authority.provider).await? else {
        return Ok(false);
    };
    if owner.plan.get("runtime") != Some(&json!(runtime.id)) {
        return Ok(false);
    }
    let Some(host) = host_rows::host(conn, runtime.host_id).await? else {
        return Ok(false);
    };
    if kind == "unit.after_up" {
        return Ok(runtime.machine_id == Some(machine)
            && key == SerialKey::Runtime(runtime.id).wire().as_deref()
            && body.get("provider_id") == Some(&json!(authority.provider))
            && body.get("environment_id").is_none_or(Json::is_null)
            && body.get("agent_id").is_none_or(Json::is_null)
            && body.get("run_id")
                == Some(&json!(ids::derived(
                    "unit.after_up.run",
                    &[owner.id.as_bytes(), runtime.id.as_bytes()]
                ))));
    }
    if machine != host.machine_id {
        return Ok(false);
    }
    let driver = body.get("driver").and_then(Json::as_str);
    Ok(match kind {
        "node.image.ensure" => {
            body.get("job_id") == Some(&json!(owner.id)) && driver == Some(runtime.driver.as_str())
        }
        "node.tool.ensure" => body.get("job_id") == Some(&json!(owner.id)),
        "node.volume.ensure" => {
            driver == Some(runtime.driver.as_str())
                && runtime.volumes.as_object().is_some_and(|volumes| {
                    volumes.values().any(|volume| {
                        volume.get("name") == body.get("name")
                            && volume.get("storage").filter(|value| !value.is_null())
                                == body.get("storage").filter(|value| !value.is_null())
                    })
                })
        }
        "node.runtime.create" => {
            key == SerialKey::Runtime(runtime.id).wire().as_deref()
                && body.get("runtime_id") == Some(&json!(runtime.id))
                && body.get("kind").and_then(Json::as_str) == Some("provider")
                && body.pointer("/labels/orch.provider_id") == Some(&json!(authority.provider))
        }
        "node.runtime.start" => {
            key == SerialKey::Runtime(runtime.id).wire().as_deref()
                && body.get("runtime_id") == Some(&json!(runtime.id))
        }
        _ => false,
    })
}

/// The ordinary runner invokes this after its canonical augmentation and active reservation.
pub async fn remember_command(job: &Job, command: &Outgoing) -> Result<(), Stop> {
    if job.plan().await?.get(FIELD).is_none() {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    principal(&mut tx, &owner, Time::new(job.now()))
        .await
        .map_err(stop)?
        .ok_or_else(|| {
            stop(EngineError::unreadable(
                "The retained provider creator disappeared.",
            ))
        })?;
    if !command_binding(
        &mut tx,
        &owner,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )
    .await?
    {
        return Err(EngineError::unreadable(
            "The provider command no longer names its actual candidate machine.",
        )
        .into());
    }
    let native = canonical(command)?;
    let mut journal = owner
        .plan
        .get(COMMANDS)
        .cloned()
        .unwrap_or_else(|| json!({}));
    let entries = journal.as_object_mut().ok_or_else(|| {
        EngineError::unreadable("The provider creator command journal is invalid.")
    })?;
    if entries
        .get(&command.id.to_string())
        .is_some_and(|old| old != &native)
    {
        return Err(
            EngineError::unreadable("The provider creator command identity changed.").into(),
        );
    }
    entries.insert(command.id.to_string(), native);
    // Existing finite phase/template IDs repeat on recovery. This journal follows job retention.
    jobs::merge_plan(&mut tx, job.id(), &json!({COMMANDS:journal}), job.now()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The dispatcher preserves actual Accepted/terminal receipts before this first-send gate.
pub async fn command_authorized(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    now: Time,
) -> Result<bool, EngineError> {
    let owners = jobs::lock_provider_creator_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(true);
    }
    if owners.len() != 1 {
        return Ok(false);
    }
    let Some(owner) = jobs::job(conn, owners[0]).await? else {
        return Ok(false);
    };
    if read(&owner)?.is_none() {
        return Ok(false);
    }
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
    let native = json!({"id":command,"machine":machine,"kind":kind,"key":key,"body_hash":body_hash(body)?.to_string()});
    if owner
        .plan
        .get(COMMANDS)
        .and_then(Json::as_object)
        .and_then(|entries| entries.get(&command.to_string()))
        != Some(&native)
    {
        return Ok(false);
    }
    if !command_binding(conn, &owner, machine, key, kind, body).await? {
        return Ok(false);
    }
    match principal(conn, &owner, now).await {
        Ok(principal) => Ok(principal.is_some()),
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
    let owners = jobs::lock_provider_creator_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if owners.len() != 1 {
        return Ok(None);
    }
    let Some(owner) = jobs::job(conn, owners[0]).await? else {
        return Ok(None);
    };
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
