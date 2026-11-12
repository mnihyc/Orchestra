//! Static owner copies are issued and accepted once by the same retained first-Make job.
use super::*;
use orch_access_contract::audit::AuditAction;
use orch_link_contract::commands::{
    MachineKey, PROVIDER_COPY_FORM, ProviderCredentialAccept, ProviderCredentialCopy,
    ProviderCredentialHandover, ProviderCredentialIssue, SealedCredential,
};

fn headers(value: &Json) -> std::collections::BTreeSet<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|upstream| {
            upstream
                .pointer("/options/secret_headers")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .map(str::to_owned)
        })
        .collect()
}
pub(super) async fn context(
    conn: &mut Conn,
    owner: &JobRow,
    authority: &Authority,
) -> Result<Option<(Id, MachineKey, ProviderCredentialCopy)>, EngineError> {
    let Some(copy) = &authority.duplicate else {
        return Ok(None);
    };
    let Some(keeper) = copy.copy_keeper else {
        return Ok(None);
    };
    if !copy.carries_secrets
        || !matches!(authority.kind.as_str(), "api" | "subscription")
        || authority.kind == "subscription" && copy.moving.is_none()
    {
        return Err(EngineError::unreadable(
            "The owner copy job is not a static API provider creation.",
        ));
    }
    let source = orch_db::queries::resources::routes::providers(conn, &[copy.source])
        .await?
        .pop()
        .ok_or_else(|| EngineError::unreadable("The owner copy source disappeared."))?;
    let target = orch_db::queries::resources::routes::providers(conn, &[authority.provider])
        .await?
        .pop()
        .ok_or_else(|| EngineError::unreadable("The owner copy target disappeared."))?;
    if source.kind != authority.kind
        || source.key_machine_id.or(source.machine_id) != Some(keeper)
        || target.kind != authority.kind
    {
        return Err(EngineError::unreadable(
            "The owner copy physical source changed.",
        ));
    }
    let runtime = rows::live_runtime(conn, authority.provider)
        .await?
        .ok_or_else(|| {
            EngineError::unreadable("The owner copy target has no actual native candidate.")
        })?;
    if owner.plan["runtime"] != json!(runtime.id)
        || runtime.provider_id != Some(authority.provider)
        || runtime.generation != 1
    {
        return Err(EngineError::unreadable(
            "The owner copy target is another native candidate.",
        ));
    }
    let machine = runtime
        .machine_id
        .ok_or_else(|| EngineError::unreadable("The owner copy target has not enrolled."))?;
    let current = orch_db::queries::resources::machine(conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.is_active())
        .ok_or_else(|| EngineError::unreadable("The owner copy recipient is unavailable."))?;
    if orch_db::queries::settings::secret(conn, "provider", target.id, "api_key")
        .await?
        .is_some()
    {
        return Err(EngineError::unreadable(
            "The new provider received another credential before the owner copy.",
        ));
    }
    let source_headers = headers(&source.upstreams);
    let target_headers = headers(&target.upstreams);
    let handover = if authority.kind == "subscription" {
        if source.held_by_environment_id.is_some() || source.agent_id != target.agent_id {
            return Err(EngineError::unreadable(
                "the existing sign-in is held elsewhere or its native profile changed",
            ));
        }
        let profile = orch_db::queries::resources::thread_engine::agent(
            conn,
            source
                .agent_id
                .ok_or_else(|| EngineError::unreadable("the native sign-in profile is absent"))?,
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("the native sign-in profile disappeared"))?;
        let source_harness = orch_db::queries::resources::thread_engine::harness(
            conn, source.id, profile.id, keeper,
        )
        .await?;
        let target_harness = orch_db::queries::resources::thread_engine::harness(
            conn, target.id, profile.id, machine,
        )
        .await?;
        let descriptor: orch_agent_contract::descriptor::AgentDescriptor = serde_json::from_value(
            source_harness.map_or(profile.descriptor, |harness| harness.descriptor),
        )?;
        let accepted = target_harness
            .map(|harness| {
                serde_json::from_value::<orch_agent_contract::descriptor::AgentDescriptor>(
                    harness.descriptor,
                )
            })
            .transpose()?;

        let form = descriptor
            .credentials
            .issues
            .iter()
            .find(|form| {
                form.custody == orch_agent_contract::credentials::Custody::Transferred
                    && form.lifetime_secs.is_none()
                    && accepted
                        .as_ref()
                        .unwrap_or(&descriptor)
                        .credentials
                        .accepts
                        .contains(&form.form)
            })
            .ok_or_else(|| {
                EngineError::unreadable(
                    "the native harness has not declared a renewable sign-in handover form",
                )
            })?;
        let original = orch_db::queries::resources::machine(conn, keeper)
            .await?
            .filter(|machine| machine.is_active())
            .ok_or_else(|| EngineError::unreadable("the original sign-in keeper is unavailable"))?;
        Some(ProviderCredentialHandover {
            profile: profile.id,
            form: form.form.clone(),
            original: MachineKey {
                machine_id: keeper,
                public_key: original.link().public_key,
            },
            returning: false,
        })
    } else {
        None
    };
    let purpose = ProviderCredentialCopy {
        handover,
        job_id: owner.id,
        source_provider_id: source.id,
        target_provider_id: target.id,
        headers: source_headers
            .intersection(&target_headers)
            .cloned()
            .collect(),
    };
    Ok(Some((
        keeper,
        MachineKey {
            machine_id: machine,
            public_key: current.link().public_key,
        },
        purpose,
    )))
}
async fn receipt(
    conn: &mut Conn,
    owner: &JobRow,
    field: &str,
    kind: &str,
) -> Result<Option<orch_db::queries::link::CommandRow>, EngineError> {
    let Some(id) = owner.plan.get(field).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let id: Id = serde_json::from_value(id.clone())?;
    let Some(row) = orch_db::queries::link::command(conn, id).await? else {
        return Ok(None);
    };
    let native = json!({"id":row.id,"machine":row.machine_id,"kind":row.kind,"key":row.serial_key(),"body_hash":body_hash(&row.body)?.to_string()});
    if row.state != "succeeded"
        || row.kind != kind
        || owner
            .plan
            .get(COMMANDS)
            .and_then(Json::as_object)
            .and_then(|values| values.get(&id.to_string()))
            != Some(&native)
    {
        return Ok(None);
    }
    Ok(Some(row))
}
pub(super) async fn binding(
    conn: &mut Conn,
    owner: &JobRow,
    authority: &Authority,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    let Some((keeper, recipient, purpose)) = context(conn, owner, authority).await? else {
        return Ok(false);
    };
    match kind {
        "provider.credential.issue" => {
            let Ok(value) = serde_json::from_value::<ProviderCredentialIssue>(body.clone()) else {
                return Ok(false);
            };
            Ok(machine == keeper
                && key
                    == SerialKey::Provider(purpose.source_provider_id)
                        .wire()
                        .as_deref()
                && value.lease_id.is_none()
                && value.copy.as_ref() == Some(&purpose)
                && value.provider_id == purpose.source_provider_id
                && value.form
                    == purpose
                        .handover
                        .as_ref()
                        .map_or(PROVIDER_COPY_FORM, |handover| handover.form.as_str())
                && value.return_credential.is_none()
                && value.recipient == recipient)
        }
        "provider.credential.accept" => {
            let Ok(value) = serde_json::from_value::<ProviderCredentialAccept>(body.clone()) else {
                return Ok(false);
            };
            let Some(issue) = receipt(
                conn,
                owner,
                "provider_copy_issue",
                "provider.credential.issue",
            )
            .await?
            else {
                return Ok(false);
            };
            let Ok(issued_body) =
                serde_json::from_value::<ProviderCredentialIssue>(issue.body.clone())
            else {
                return Ok(false);
            };
            let Some(result) = issue.result else {
                return Ok(false);
            };
            let Ok(sealed) = serde_json::from_value::<SealedCredential>(result) else {
                return Ok(false);
            };
            Ok(issue.machine_id == keeper
                && issued_body.copy.as_ref() == Some(&purpose)
                && issued_body.recipient == recipient
                && sealed.form
                    == purpose
                        .handover
                        .as_ref()
                        .map_or(PROVIDER_COPY_FORM, |handover| handover.form.as_str())
                && sealed.expires_at.is_none()
                && machine == recipient.machine_id
                && key
                    == SerialKey::Provider(purpose.target_provider_id)
                        .wire()
                        .as_deref()
                && value.provider_id == purpose.target_provider_id
                && value.copy.as_ref() == Some(&purpose)
                && value.form == sealed.form
                && value.expires_at.is_none()
                && value.credential == sealed.credential)
        }
        _ => Ok(false),
    }
}
pub(in crate::jobs::provider_provision) async fn run(job: &Job) -> Result<(), Stop> {
    start_harnesses(job).await?;
    let engine = job.engine();
    let mut tx = engine.db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let Some(authority) = read(&owner)? else {
        return Ok(());
    };
    if authority
        .duplicate
        .as_ref()
        .and_then(|copy| copy.copy_keeper)
        .is_none()
    {
        return Ok(());
    }
    principal(&mut tx, &owner, engine.inner.clock.as_ref())
        .await
        .map_err(stop)?
        .ok_or_else(|| stop(EngineError::unreadable("The owner copy creator ended.")))?;
    let (keeper, recipient, purpose) = context(&mut tx, &owner, &authority)
        .await?
        .ok_or_else(|| EngineError::unreadable("The owner copy purpose disappeared."))?;
    let issued = receipt(
        &mut tx,
        &owner,
        "provider_copy_issue",
        "provider.credential.issue",
    )
    .await?;
    let accepted = receipt(
        &mut tx,
        &owner,
        "provider_copy_accept",
        "provider.credential.accept",
    )
    .await?;
    tx.rollback().await.map_err(DbError::from)?;
    if issued.is_none() {
        let command = Outgoing::new(
            ids::derived(
                "provider.duplicate.credential.issue",
                &[job.id().as_bytes()],
            ),
            keeper,
            SerialKey::Provider(purpose.source_provider_id),
            &ProviderCredentialIssue {
                return_credential: None,
                lease_id: None,
                copy: Some(purpose.clone()),
                provider_id: purpose.source_provider_id,
                form: purpose
                    .handover
                    .as_ref()
                    .map_or(PROVIDER_COPY_FORM, |handover| handover.form.as_str())
                    .into(),
                recipient: recipient.clone(),
            },
        )
        .map_err(EngineError::from)?;
        if let Err(error) = job.command(command).await {
            let owner = job.row().await?;
            let mut conn = engine.db().acquire().await?;
            if signed_out(&mut conn, &owner).await? {
                return Ok(());
            }
            return Err(error);
        }
    }
    audit(job).await?;
    if accepted.is_none() {
        let owner = job.row().await?;
        let mut conn = engine.db().acquire().await?;
        let issue = receipt(
            &mut conn,
            &owner,
            "provider_copy_issue",
            "provider.credential.issue",
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("The owner copy has no genuine issued receipt."))?;
        let sealed: SealedCredential = serde_json::from_value(issue.result.ok_or_else(|| {
            EngineError::unreadable("The owner copy issue has no sealed result.")
        })?)
        .map_err(EngineError::from)?;
        drop(conn);
        let command = Outgoing::new(
            ids::derived(
                "provider.duplicate.credential.accept",
                &[job.id().as_bytes()],
            ),
            recipient.machine_id,
            SerialKey::Provider(purpose.target_provider_id),
            &ProviderCredentialAccept {
                provider_id: purpose.target_provider_id,
                copy: Some(purpose.clone()),
                form: sealed.form,
                credential: sealed.credential,
                expires_at: sealed.expires_at,
            },
        )
        .map_err(EngineError::from)?;
        job.command(command).await?;
    }
    audit(job).await
}
pub(in crate::jobs::provider_provision) async fn audit(job: &Job) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    audit_on(&mut tx, &owner, job.engine().inner.clock.as_ref()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
pub(super) async fn audit_on(
    conn: &mut Conn,
    owner: &JobRow,
    clock: &dyn orch_types::Clock,
) -> Result<(), EngineError> {
    let Some(authority) = read(owner)? else {
        return Ok(());
    };
    let Some(copy) = &authority.duplicate else {
        return Ok(());
    };
    if copy.copy_keeper.is_none() {
        return Ok(());
    }
    let mut patch = json!({});
    for (field, kind, marker, target, action, message) in [
        (
            "provider_copy_issue",
            "provider.credential.issue",
            "provider_copy_issue_audited",
            copy.source,
            AuditAction::ProviderCredentialIssued,
            "issued a static credential for its owner's duplicate provider",
        ),
        (
            "provider_copy_accept",
            "provider.credential.accept",
            "provider_copy_accept_audited",
            authority.provider,
            AuditAction::ProviderUpdated,
            "accepted its owner's sealed static credential copy",
        ),
    ] {
        if owner.plan[marker] == json!(true) {
            continue;
        }
        let Some(row) = receipt(conn, owner, field, kind).await? else {
            continue;
        };
        let purpose: ProviderCredentialCopy = serde_json::from_value(row.body["copy"].clone())?;
        if purpose.job_id != owner.id
            || purpose.source_provider_id != copy.source
            || purpose.target_provider_id != authority.provider
        {
            return Err(EngineError::unreadable(
                "The accepted copy receipt belongs to another owner job.",
            ));
        }
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(authority.origin.user()),
            action.as_str(),
            "provider",
            Some(target),
            message,
        );
        entry.history = action.is_history_line();
        entry.details = json!({"purpose":if copy.moving.is_some(){"provider_move_out"}else{"provider_duplicate"},"job":owner.id,"source":purpose.source_provider_id,"target":purpose.target_provider_id,"command":row.id,"machine":row.machine_id});
        orch_db::audit::append(
            conn,
            &entry,
            &orch_db::audit::Origin {
                as_admin: copy.raw.as_admin().then_some(authority.origin.user()),
                ..orch_db::audit::Origin::default()
            },
            clock.now(),
        )
        .await?;
        patch[marker] = json!(true);
    }
    if patch.as_object().is_some_and(|patch| !patch.is_empty()) {
        jobs::merge_plan(conn, owner.id, &patch, clock.now()).await?;
    }
    Ok(())
}

/// Native success-ACK metadata. It grants no current caller, queue or send authority.
pub struct CopyAck {
    job: Id,
    command: Id,
}
impl CopyAck {
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        jobs::lock(conn, self.job)
            .await?
            .ok_or_else(|| EngineError::unreadable("The native owner copy journal disappeared."))?;
        Ok(())
    }
    pub async fn reconcile(
        &self,
        conn: &mut Conn,
        clock: &dyn orch_types::Clock,
    ) -> Result<(), EngineError> {
        let owner = jobs::job(conn, self.job)
            .await?
            .ok_or_else(|| EngineError::unreadable("The native owner copy journal disappeared."))?;
        let row = orch_db::queries::link::command(conn, self.command)
            .await?
            .ok_or_else(|| EngineError::unreadable("The native owner copy receipt disappeared."))?;
        if !matches_record(&owner, &row)? {
            return Err(EngineError::unreadable(
                "The native owner copy ACK has no exact canonical receipt.",
            ));
        }
        // The existing first ending remains authoritative if a late peer reply races expiry
        // or cancellation. A pending/failed row is never promoted or treated as a success.
        if row.state != "succeeded" {
            return Ok(());
        }
        audit_on(conn, &owner, clock).await
    }
}
fn matches_record(
    owner: &JobRow,
    row: &orch_db::queries::link::CommandRow,
) -> Result<bool, EngineError> {
    let Some(authority) = read(owner)? else {
        return Ok(false);
    };
    let Some(source) = &authority.duplicate else {
        return Ok(false);
    };
    if source.copy_keeper.is_none()
        || !source.carries_secrets
        || !matches!(
            authority.origin.operation(),
            "ProviderDuplicate" | "ProviderMoveOut"
        )
    {
        return Ok(false);
    }
    let Ok(purpose) = serde_json::from_value::<ProviderCredentialCopy>(row.body["copy"].clone())
    else {
        return Ok(false);
    };
    if purpose.job_id != owner.id
        || purpose.source_provider_id != source.source
        || purpose.target_provider_id != authority.provider
    {
        return Ok(false);
    }
    let native = json!({"id":row.id,"machine":row.machine_id,"kind":row.kind,"key":row.serial_key(),"body_hash":body_hash(&row.body)?.to_string()});
    Ok(owner
        .plan
        .get(COMMANDS)
        .and_then(Json::as_object)
        .and_then(|values| values.get(&row.id.to_string()))
        == Some(&native))
}
/// The authenticated native acknowledgment locates only this exact retained successful effect.
pub async fn recorded_copy(conn: &mut Conn, command: Id) -> Result<Option<CopyAck>, EngineError> {
    let Some(row) = orch_db::queries::link::command(conn, command).await? else {
        return Ok(None);
    };
    if !matches!(
        row.kind.as_str(),
        "provider.credential.issue" | "provider.credential.accept"
    ) || row.body.get("copy").is_none_or(Json::is_null)
    {
        return Ok(None);
    }
    let purpose: ProviderCredentialCopy = serde_json::from_value(row.body["copy"].clone())?;
    let owner = jobs::job(conn, purpose.job_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The native owner copy has no original job."))?;
    if !matches_record(&owner, &row)? {
        return Err(EngineError::unreadable(
            "The native owner copy ACK belongs to another canonical command.",
        ));
    }
    Ok(Some(CopyAck {
        job: owner.id,
        command,
    }))
}

pub(super) async fn move_ready(conn: &mut Conn, owner: &JobRow) -> Result<bool, EngineError> {
    let Some(authority) = read(owner)? else {
        return Ok(false);
    };
    let Some(copy) = authority.duplicate else {
        return Ok(false);
    };
    if copy.moving.is_none() {
        return Ok(false);
    }
    if copy.copy_keeper.is_none() {
        return Ok(true);
    }
    if signed_out(conn, owner).await? {
        return Ok(true);
    }
    Ok(receipt(
        conn,
        owner,
        "provider_copy_issue",
        "provider.credential.issue",
    )
    .await?
    .is_some()
        && receipt(
            conn,
            owner,
            "provider_copy_accept",
            "provider.credential.accept",
        )
        .await?
        .is_some())
}

/// Only an exact accepted native sign-in owns its compensation, never an expired actor's next act.
pub(super) async fn inverse_binding(
    conn: &mut Conn,
    owner: &JobRow,
    authority: &Authority,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    if crate::jobs::runner::abort_reason(owner)?.is_none()
        || owner.plan.get("provider_creator_committed") == Some(&json!(true))
    {
        return Ok(false);
    }
    let Some(copy) = authority
        .duplicate
        .as_ref()
        .filter(|copy| copy.moving.is_some())
    else {
        return Ok(false);
    };
    let Some(issue) = receipt(
        conn,
        owner,
        "provider_copy_issue",
        "provider.credential.issue",
    )
    .await?
    else {
        return Ok(false);
    };
    let issued: ProviderCredentialIssue = serde_json::from_value(issue.body.clone())?;
    let Some(mut purpose) = issued.copy else {
        return Ok(false);
    };
    let Some(handover) = purpose.handover.as_mut() else {
        return Ok(false);
    };
    if purpose.job_id != owner.id
        || purpose.source_provider_id != copy.source
        || purpose.target_provider_id != authority.provider
    {
        return Ok(false);
    }
    handover.returning = true;
    let original = handover.original.clone();
    let form = handover.form.clone();
    let runtime = rows::live_runtime(conn, authority.provider)
        .await?
        .filter(|runtime| {
            runtime.generation == 1 && runtime.machine_id == Some(issued.recipient.machine_id)
        });
    let source = rows::state(conn, copy.source).await?.filter(|source| {
        source.deleted_at.is_none()
            && source.key_machine_id.or(source.machine_id) == Some(original.machine_id)
    });
    if runtime.is_none() || source.is_none() {
        return Ok(false);
    }
    match kind {
        "provider.credential.issue" => {
            let request: ProviderCredentialIssue = serde_json::from_value(body.clone())?;
            Ok(machine == issued.recipient.machine_id
                && key == SerialKey::Provider(authority.provider).wire().as_deref()
                && request.lease_id.is_none()
                && request.provider_id == authority.provider
                && request.copy.as_ref() == Some(&purpose)
                && request.form == form
                && request.recipient == original
                && request.return_credential.as_ref()
                    == issue
                        .result
                        .as_ref()
                        .map(|result| serde_json::from_value::<SealedCredential>(result.clone()))
                        .transpose()?
                        .as_ref())
        }
        "provider.credential.accept" => {
            let request: ProviderCredentialAccept = serde_json::from_value(body.clone())?;
            let Some(returned) = receipt(
                conn,
                owner,
                "provider_move_return_issue",
                "provider.credential.issue",
            )
            .await?
            else {
                return Ok(false);
            };
            let sealed: SealedCredential =
                serde_json::from_value(returned.result.ok_or_else(|| {
                    EngineError::unreadable("the owned return has no native sealed receipt")
                })?)?;
            Ok(machine == original.machine_id
                && key == SerialKey::Provider(copy.source).wire().as_deref()
                && request.provider_id == copy.source
                && request.copy.as_ref() == Some(&purpose)
                && request.form == form
                && request.credential == sealed.credential
                && request.expires_at.is_none())
        }
        _ => Ok(false),
    }
}
pub(super) fn inverse(body: &Json) -> bool {
    body.pointer("/copy/handover/returning") == Some(&json!(true))
}
async fn remember_inverse(job: &Job, command: &Outgoing, field: &str) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let authority = read(&owner)?
        .ok_or_else(|| EngineError::unreadable("the owned sign-in source disappeared"))?;
    if !inverse_binding(
        &mut tx,
        &owner,
        &authority,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )
    .await?
    {
        return Err(EngineError::unreadable(
            "another native effect cannot borrow the sign-in inverse",
        )
        .into());
    }
    let mut journal = owner
        .plan
        .get(COMMANDS)
        .cloned()
        .unwrap_or_else(|| json!({}));
    journal
        .as_object_mut()
        .ok_or_else(|| EngineError::unreadable("the owned return journal is invalid"))?
        .insert(command.id.to_string(), canonical(command)?);
    let mut patch = json!({COMMANDS:journal});
    patch[field] = json!(command.id);
    jobs::merge_plan(&mut tx, job.id(), &patch, job.now()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
/// Return the actual renewed sign-in before the existing inverse can erase its fresh recipient unit.
pub(super) async fn undo(job: &Job) -> Result<(), Stop> {
    let owner = job.row().await?;
    let Some(authority) = read(&owner)? else {
        return Ok(());
    };
    let Some(copy) = &authority.duplicate else {
        return Ok(());
    };
    if copy.moving.is_none() || authority.kind != "subscription" {
        return Ok(());
    }
    let mut conn = job.engine().db().acquire().await?;
    if rows::state(&mut conn, copy.source)
        .await?
        .is_none_or(|source| source.deleted_at.is_some())
    {
        return Ok(());
    }

    let Some(issue) = receipt(
        &mut conn,
        &owner,
        "provider_copy_issue",
        "provider.credential.issue",
    )
    .await?
    else {
        if let Some(id) = owner
            .plan
            .get("provider_copy_issue")
            .filter(|id| !id.is_null())
        {
            let id: Id = serde_json::from_value(id.clone()).map_err(EngineError::from)?;
            if orch_db::queries::link::command(&mut conn, id)
                .await?
                .is_some_and(|row| {
                    matches!(row.state.as_str(), "sent" | "accepted")
                        || row
                            .error
                            .as_ref()
                            .and_then(|error| error.get("code"))
                            .and_then(Json::as_str)
                            .is_some_and(|code| code == "delivery_unknown" || code == "internal")
                })
            {
                return Err(EngineError::unreadable(
                    "the original keeper has not yet confirmed its sign-in custody",
                )
                .into());
            }
        }
        return Ok(());
    };
    let issued: ProviderCredentialIssue =
        serde_json::from_value(issue.body).map_err(EngineError::from)?;
    let mut purpose = issued
        .copy
        .ok_or_else(|| EngineError::unreadable("the native issue lost its owned purpose"))?;
    let handover = purpose
        .handover
        .as_mut()
        .ok_or_else(|| EngineError::unreadable("the native issue lost its renewable form"))?;
    handover.returning = true;
    let original = handover.original.clone();
    let form = handover.form.clone();
    let sealed: SealedCredential =
        serde_json::from_value(issue.result.ok_or_else(|| {
            EngineError::unreadable("the native issue lost its exact sealed value")
        })?)
        .map_err(EngineError::from)?;
    let attempt = owner
        .plan
        .get("provider_move_return_attempt")
        .and_then(Json::as_u64)
        .unwrap_or(0)
        .saturating_add(1);
    jobs::merge_plan(
        &mut conn,
        job.id(),
        &json!({"provider_move_return_attempt":attempt}),
        job.now(),
    )
    .await?;
    drop(conn);
    let command = Outgoing::new(
        ids::derived(
            "provider.move.return.issue",
            &[job.id().as_bytes(), &attempt.to_le_bytes()],
        ),
        issued.recipient.machine_id,
        SerialKey::Provider(authority.provider),
        &ProviderCredentialIssue {
            return_credential: Some(sealed),
            lease_id: None,
            copy: Some(purpose.clone()),
            provider_id: authority.provider,
            form: form.clone(),
            recipient: original.clone(),
        },
    )
    .map_err(EngineError::from)?;
    remember_inverse(job, &command, "provider_move_return_issue").await?;
    let returned: SealedCredential =
        serde_json::from_value(job.command_to_undo(command).await?).map_err(EngineError::from)?;
    let command = Outgoing::new(
        ids::derived(
            "provider.move.return.accept",
            &[job.id().as_bytes(), &attempt.to_le_bytes()],
        ),
        original.machine_id,
        SerialKey::Provider(copy.source),
        &ProviderCredentialAccept {
            copy: Some(purpose),
            provider_id: copy.source,
            form,
            credential: returned.credential,
            expires_at: returned.expires_at,
        },
    )
    .map_err(EngineError::from)?;
    remember_inverse(job, &command, "provider_move_return_accept").await?;
    job.command_to_undo(command).await?;
    Ok(())
}

async fn start_harnesses(job: &Job) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let Some(authority) = read(&owner)? else {
        return Ok(());
    };
    let Some(copy) = authority
        .duplicate
        .as_ref()
        .filter(|copy| copy.moving.is_some() && authority.kind == "subscription")
    else {
        return Ok(());
    };
    let Some(keeper) = copy.copy_keeper else {
        return Ok(());
    };
    principal(&mut tx, &owner, job.engine().inner.clock.as_ref())
        .await?
        .ok_or_else(|| EngineError::unreadable("the native sign-in creator ended"))?;
    let runtime = rows::live_runtime(&mut tx, authority.provider)
        .await?
        .ok_or_else(|| {
            EngineError::unreadable("the new sign-in runtime has no native candidate")
        })?;
    let target = runtime
        .machine_id
        .ok_or_else(|| EngineError::unreadable("the new sign-in keeper has not enrolled"))?;
    let source = copy.source;
    let provider = authority.provider;
    tx.commit().await.map_err(DbError::from)?;
    super::harness::start(job, source, keeper).await?;
    super::harness::start(job, provider, target).await
}

async fn signed_out(conn: &mut Conn, owner: &JobRow) -> Result<bool, EngineError> {
    let Some(authority) = read(owner)? else {
        return Ok(false);
    };
    let Some(copy) = authority
        .duplicate
        .filter(|copy| copy.moving.is_some() && authority.kind == "subscription")
    else {
        return Ok(false);
    };
    let Some(id) = owner
        .plan
        .get("provider_copy_issue")
        .filter(|id| !id.is_null())
    else {
        return Ok(false);
    };
    let id: Id = serde_json::from_value(id.clone())?;
    let Some(command) = orch_db::queries::link::command(conn, id).await? else {
        return Ok(false);
    };
    let canonical = json!({"id":command.id,"machine":command.machine_id,"kind":command.kind,"key":command.serial_key(),"body_hash":body_hash(&command.body)?.to_string()});
    if command.state != "failed"
        || command.kind != "provider.credential.issue"
        || command
            .error
            .as_ref()
            .and_then(|error| error.get("code"))
            .and_then(Json::as_str)
            != Some("login_required")
        || owner
            .plan
            .get(COMMANDS)
            .and_then(Json::as_object)
            .and_then(|values| values.get(&id.to_string()))
            != Some(&canonical)
    {
        return Ok(false);
    }
    Ok(rows::state(conn, copy.source)
        .await?
        .and_then(|source| source.login_state)
        .and_then(|login| login.get("state").and_then(Json::as_str).map(str::to_owned))
        .as_deref()
        == Some("idle"))
}
