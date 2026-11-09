//! Static owner copies are issued and accepted once by the same retained first-Make job.
use super::*;
use orch_access_contract::audit::AuditAction;
use orch_link_contract::commands::{
    MachineKey, PROVIDER_COPY_FORM, ProviderCredentialAccept, ProviderCredentialCopy,
    ProviderCredentialIssue, SealedCredential,
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
async fn context(
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
    if !copy.carries_secrets || authority.kind != "api" {
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
    if source.kind != "api"
        || source.key_machine_id.or(source.machine_id) != Some(keeper)
        || target.kind != "api"
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
    let purpose = ProviderCredentialCopy {
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
                && value.form == PROVIDER_COPY_FORM
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
                && sealed.form == PROVIDER_COPY_FORM
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
                lease_id: None,
                copy: Some(purpose.clone()),
                provider_id: purpose.source_provider_id,
                form: PROVIDER_COPY_FORM.into(),
                recipient: recipient.clone(),
            },
        )
        .map_err(EngineError::from)?;
        job.command(command).await?;
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
