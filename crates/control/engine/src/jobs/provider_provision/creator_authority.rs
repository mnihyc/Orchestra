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

mod duplicate;
mod harness;
pub use harness::Command as CreatorCommand;
mod transfer;
pub(super) use transfer::audit as copy_audit;
pub(super) use transfer::run as copy_credential;
pub use transfer::{CopyAck, recorded_copy};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duplicate: Option<duplicate::Origin>,
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
    retain_inner(conn, origin, provider, home, job, None, now).await
}
/// Retains the same first-Make family under the actual duplication operation and raw identity.
pub async fn retain_duplicate(
    conn: &mut Conn,
    raw: &crate::operation_scope::http_origin::HttpOrigin,
    source: Id,
    provider: Id,
    home: Id,
    job: Id,
    clock: &dyn orch_types::Clock,
) -> Result<(), EngineError> {
    retain_copy(conn, raw, (source, None), provider, home, job, clock).await
}
/// The same initial Make retains an owner's confirmed native source ending.
pub async fn retain_move(
    conn: &mut Conn,
    raw: &crate::operation_scope::http_origin::HttpOrigin,
    source: (Id, Hash),
    provider: Id,
    home: Id,
    job: Id,
    clock: &dyn orch_types::Clock,
) -> Result<(), EngineError> {
    retain_copy(
        conn,
        raw,
        (source.0, Some(source.1)),
        provider,
        home,
        job,
        clock,
    )
    .await
}
async fn retain_copy(
    conn: &mut Conn,
    raw: &crate::operation_scope::http_origin::HttpOrigin,
    source: (Id, Option<Hash>),
    provider: Id,
    home: Id,
    job: Id,
    clock: &dyn orch_types::Clock,
) -> Result<(), EngineError> {
    let (source, moving) = source;
    if raw.origin().operation()
        != if moving.is_some() {
            "ProviderMoveOut"
        } else {
            "ProviderDuplicate"
        }
        || source == provider
    {
        return Err(EngineError::unreadable(
            "The copied provider origin changed.",
        ));
    }
    let carries_secrets =
        !orch_db::queries::settings::secret_names_of(conn, &[("provider", provider)])
            .await?
            .is_empty();
    retain_inner(
        conn,
        raw.origin(),
        provider,
        home,
        job,
        Some(duplicate::Origin {
            source,
            raw: raw.clone(),
            carries_secrets,
            copy_keeper: None,
            moving,
        }),
        clock.time(),
    )
    .await?;
    let row = jobs::job(conn, job)
        .await?
        .ok_or_else(|| EngineError::unreadable("The copied provider job disappeared."))?;
    let principal = principal(conn, &row, clock)
        .await?
        .ok_or_else(|| EngineError::unreadable("The copied provider creator ended."))?;
    let actual_source = rows::state(conn, source)
        .await?
        .ok_or_else(|| EngineError::unreadable("The copied provider source disappeared."))?;
    let has_server_key = orch_db::queries::settings::secret(conn, "provider", provider, "api_key")
        .await?
        .is_some();
    if ((actual_source.kind == "api" && !has_server_key)
        || (actual_source.kind == "subscription" && moving.is_some()))
        && resources::effective(
            conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: source,
            },
            Policy::default(),
        )
        .await?
        .owner
        && let Some(keeper) = actual_source.key_machine_id.or(actual_source.machine_id)
    {
        let current = jobs::job(conn, job)
            .await?
            .ok_or_else(|| EngineError::unreadable("The copied provider job disappeared."))?;
        let mut authority = read(&current)?
            .ok_or_else(|| EngineError::unreadable("The copied provider authority disappeared."))?;
        let duplicate = authority
            .duplicate
            .as_mut()
            .ok_or_else(|| EngineError::unreadable("The copied provider source disappeared."))?;
        duplicate.copy_keeper = Some(keeper);
        duplicate.carries_secrets = true;
        jobs::merge_plan(conn, job, &json!({FIELD:authority}), clock.now()).await?;
    }
    if !principal.caller().key_deadline_allows(clock) {
        return Err(EngineError::unreadable(
            "The copied provider creator ended.",
        ));
    }
    Ok(())
}
/// The HTTP owner rechecks its one actual copied-provider Make after configuration waits,
/// immediately before committing the native creation transaction. No new authority is attached.
pub async fn copy_checkpoint(
    conn: &mut Conn,
    provider: Id,
    clock: &dyn orch_types::Clock,
) -> Result<(orch_authz::Caller, Option<Time>), EngineError> {
    let owners: Vec<Id> = sqlx::query_scalar("SELECT id FROM jobs WHERE provider_id=$1 AND kind='provider_provision' AND state='queued' AND plan->>'mode'='make' AND plan ? 'provider_creator' ORDER BY id FOR UPDATE")
        .bind(provider).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    if owners.len() != 1 {
        return Err(EngineError::unreadable(
            "The new provider has no unique retained Make.",
        ));
    }
    let row = jobs::job(conn, owners[0])
        .await?
        .ok_or_else(|| EngineError::unreadable("The copied provider Make disappeared."))?;
    if read(&row)?
        .is_none_or(|authority| authority.provider != provider || authority.duplicate.is_none())
    {
        return Err(EngineError::unreadable(
            "The copied provider source changed.",
        ));
    }
    let principal = principal(conn, &row, clock)
        .await?
        .ok_or_else(|| EngineError::unreadable("The copied provider creator ended."))?;
    let fresh = jobs::job(conn, row.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The copied provider Make disappeared."))?;
    let raw = read(&fresh)?
        .and_then(|authority| authority.duplicate)
        .ok_or_else(|| EngineError::unreadable("The copied provider raw origin disappeared."))?
        .raw;
    if !principal.caller().key_deadline_allows(clock)
        || raw.until().is_some_and(|end| clock.time() >= end)
    {
        return Err(EngineError::unreadable(
            "The copied provider lifetime ended.",
        ));
    }
    Ok((principal.caller().clone(), raw.until()))
}

async fn retain_inner(
    conn: &mut Conn,
    origin: &CapturedOperation,
    provider: Id,
    home: Id,
    job: Id,
    duplicate: Option<duplicate::Origin>,
    now: Time,
) -> Result<(), EngineError> {
    let row = jobs::lock(conn, job)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new provider job disappeared."))?;
    let current = rows::state(conn, provider)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new provider disappeared."))?;
    if !matches!(
        (origin.operation(), duplicate.is_some()),
        ("ProviderCreate", false) | ("ProviderDuplicate" | "ProviderMoveOut", true)
    ) || row.kind != provider_jobs::PROVISION
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
        duplicate,
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
        Ok(home)
            if matches!(
                home.object.kind,
                KeyTargetKind::Object(ObjectKind::Org | ObjectKind::Project)
            ) =>
        {
            home
        }
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
    clock: &dyn orch_types::Clock,
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
    let principal = if authority.duplicate.is_some() {
        duplicate::principal(conn, &authority, clock).await?
    } else {
        creator_principal(conn, &authority.origin, authority.provider, clock.time())
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("The provider creator credential is no longer authorized.")
            })?
    };
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
    if let Some(runtime) = planned
        && !jobs::lock_provider_creator_candidate(conn, authority.provider, runtime).await?
    {
        return Err(EngineError::unreadable(
            "The provider creator candidate disappeared.",
        ));
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
    if !principal.caller().key_deadline_allows(clock) {
        return Err(EngineError::unreadable(
            "The provider creator lifetime ended.",
        ));
    }
    let fresh = jobs::job(conn, row.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The provider creator job disappeared."))?;
    if read(&fresh)?
        .and_then(|authority| authority.duplicate)
        .is_some_and(|origin| origin.raw.until().is_some_and(|end| clock.time() >= end))
    {
        return Err(EngineError::unreadable(
            "The raw provider creator lifetime ended.",
        ));
    }
    if !principal.caller().key_deadline_allows(clock) {
        return Err(EngineError::unreadable(
            "The provider creator lifetime ended.",
        ));
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
    principal(&mut tx, &owner, job.engine().inner.clock.as_ref())
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
    if transfer::inverse(body) {
        return if matches!(
            kind,
            "provider.credential.issue" | "provider.credential.accept"
        ) {
            transfer::inverse_binding(conn, owner, &authority, machine, key, kind, body).await
        } else {
            Ok(false)
        };
    }
    let Some(runtime) = rows::live_runtime(conn, authority.provider).await? else {
        return Ok(false);
    };
    if owner.plan.get("runtime") != Some(&json!(runtime.id)) {
        return Ok(false);
    }
    let Some(host) = host_rows::host(conn, runtime.host_id).await? else {
        return Ok(false);
    };
    if matches!(
        kind,
        "provider.credential.issue" | "provider.credential.accept"
    ) {
        return transfer::binding(conn, owner, &authority, machine, key, kind, body).await;
    }
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
    principal(&mut tx, &owner, job.engine().inner.clock.as_ref())
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
    let mut patch = json!({COMMANDS:journal});
    if command.kind.as_str() == "provider.credential.issue" {
        patch["provider_copy_issue"] = json!(command.id);
    }
    if command.kind.as_str() == "provider.credential.accept" {
        patch["provider_copy_accept"] = json!(command.id);
    }
    jobs::merge_plan(&mut tx, job.id(), &patch, job.now()).await?;
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
    clock: &dyn orch_types::Clock,
) -> Result<bool, EngineError> {
    let owners = jobs::lock_provider_creator_command_owners(conn, command).await?;
    if owners.is_empty() {
        return Ok(!(matches!(
            kind,
            "provider.credential.issue" | "provider.credential.accept"
        ) && body.get("copy").is_some_and(|copy| !copy.is_null())));
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
    if transfer::inverse(body) {
        return Ok(true);
    }
    match principal(conn, &owner, clock).await {
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
    if !command_authorized(conn, command, machine, key, kind, body, clock).await? {
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
    if transfer::inverse(body) {
        return Ok(Some(Vec::new()));
    }
    if let Some(duplicate) = authority.duplicate {
        let current = duplicate
            .raw
            .current(conn, clock)
            .await
            .map_err(duplicate::error)?;
        current.check(clock).map_err(duplicate::error)?;
        return Ok(Some(vec![(
            Some(current.caller().clone()),
            current.until(),
        )]));
    }
    Ok(authority
        .origin
        .restore_current(conn, clock)
        .await?
        .map(|caller| vec![(Some(caller), None)]))
}

/// The source ending changes authority; take its native global lock before any row lock.
pub(super) async fn lock_commit(conn: &mut Conn, owner: &JobRow) -> Result<(), EngineError> {
    if read(owner)?
        .and_then(|authority| authority.duplicate)
        .is_some_and(|copy| copy.moving.is_some())
    {
        orch_db::queries::transaction_guard::lock_authority(conn).await?;
    } else {
        orch_db::queries::transaction_guard::lock_action(conn).await?;
    }
    Ok(())
}
/// The successful native initial Make and its original source Delete share this commit.
pub(super) async fn end_moved_source(
    engine: &Engine,
    conn: &mut Conn,
    owner: &JobRow,
    principal: &Principal,
    clock: &dyn orch_types::Clock,
) -> Result<crate::unit::Finished, EngineError> {
    let Some(authority) = read(owner)? else {
        return Ok(Default::default());
    };
    let Some(copy) = &authority.duplicate else {
        return Ok(Default::default());
    };
    let Some(expected) = copy.moving else {
        return Ok(Default::default());
    };
    if !transfer::move_ready(conn, owner).await? {
        return Err(EngineError::unreadable(
            "The provider move has no completed native credential copy.",
        ));
    }
    if authority.origin.operation() != "ProviderMoveOut"
        || principal.user() != authority.origin.user()
        || principal.caller().admitted_operation() != Some("ProviderMoveOut")
        || !principal.caller().key_deadline_allows(clock)
        || copy.raw.until().is_some_and(|end| clock.time() >= end)
    {
        return Err(EngineError::unreadable("The provider move lifetime ended."));
    }
    let source = orch_db::queries::resources::routes::providers(conn, &[copy.source])
        .await?
        .pop()
        .ok_or_else(|| EngineError::unreadable("The moving source disappeared."))?;
    if crate::providers::deleting::fingerprint(conn, &source).await? != expected {
        return Err(EngineError::unreadable(
            "The reviewed provider Delete plan changed.",
        ));
    }
    let boundary = crate::copy_out::authorize(
        conn,
        principal,
        crate::copy_out::Subject::Object(ObjectRef {
            kind: ObjectKind::Provider,
            id: source.id,
        }),
        Some(authority.home),
    )
    .await
    .map_err(|error| EngineError::unreadable(error.to_string()))?
    .ok_or_else(|| {
        EngineError::unreadable("The provider no longer moves between organizations.")
    })?;
    let origin = orch_db::audit::Origin {
        as_admin: copy.raw.as_admin().then_some(principal.user()),
        ..Default::default()
    };
    crate::copy_out::record(
        conn,
        boundary,
        orch_db::audit::Actor::User(principal.user()),
        &origin,
        authority.provider,
        clock.now(),
    )
    .await?;
    let (_, finished) =
        crate::providers::deleting::apply(engine, conn, &source, principal.user(), clock.now())
            .await?;
    let action = orch_access_contract::audit::AuditAction::ProviderDeleted;
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(principal.user()),
        action.as_str(),
        "provider",
        Some(source.id),
        "deleted the original provider after its native copy was prepared",
    );
    entry.history = action.is_history_line();
    entry.node_id = Some(source.home_id);
    entry.org_id = Some(source.org_id);
    entry.details = json!({"provider":source.id,"name":source.name,"created_id":authority.provider,"job":owner.id});
    orch_db::audit::append(conn, &entry, &origin, clock.now()).await?;
    if !principal.caller().key_deadline_allows(clock)
        || copy.raw.until().is_some_and(|end| clock.time() >= end)
    {
        return Err(EngineError::unreadable("The provider move lifetime ended."));
    }
    Ok(finished)
}

#[cfg(test)]
mod tests;

pub(super) async fn undo_transfer(job: &Job) -> Result<(), Stop> {
    transfer::undo(job).await
}
