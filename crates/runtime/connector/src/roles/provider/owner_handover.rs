//! Renewable sign-ins use the same sealed issue/accept commands under one exact owner job.
use super::*;
use orch_agent_contract::credentials::{Custody, GivenCredential};
use orch_link_contract::commands::{
    MachineKey, ProviderCredentialAccept, ProviderCredentialCopy, ProviderCredentialIssue, Sealed,
    SealedCredential,
};
use orch_types::Hash;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Held {
    purpose: ProviderCredentialCopy,
    credential: GivenCredential,
    installed: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Issued {
    purpose: ProviderCredentialCopy,
    recipient: MachineKey,
    result: SealedCredential,
}
fn name(provider: Id, job: Id) -> String {
    format!("provider-{provider}.move-signin-{job}")
}
fn issued(copy: &ProviderCredentialCopy) -> String {
    format!(
        "provider-{}.move-issued-{}-{}",
        copy.source_provider_id,
        copy.job_id,
        copy.handover
            .as_ref()
            .is_some_and(|handover| handover.returning)
    )
}
fn info(copy: &ProviderCredentialCopy) -> Result<Vec<u8>, CommandError> {
    serde_json::to_vec(&("orch/credential/v1/owner-signin-move", copy))
        .map_err(|_| internal("the owned handover context could not be encoded"))
}
fn form(copy: &ProviderCredentialCopy) -> Result<&str, CommandError> {
    copy.handover
        .as_ref()
        .map(|handover| handover.form.as_str())
        .filter(|form| !form.is_empty())
        .ok_or_else(|| unsupported("the owned handover has no transferred form"))
}
fn forward(copy: &ProviderCredentialCopy) -> ProviderCredentialCopy {
    let mut copy = copy.clone();
    if let Some(handover) = &mut copy.handover {
        handover.returning = false;
    }
    copy
}
fn read(store: &SecretStore, provider: Id, job: Id) -> Result<Option<Held>, CommandError> {
    store
        .get(&name(provider, job))
        .map_err(|_| internal("the owned sign-in could not be read"))?
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|_| internal("the owned sign-in receipt is invalid"))
        })
        .transpose()
}
fn save(store: &SecretStore, provider: Id, held: &Held) -> Result<(), CommandError> {
    store
        .put(
            &name(provider, held.purpose.job_id),
            &serde_json::to_string(held)
                .map_err(|_| internal("the owned sign-in could not be encoded"))?,
        )
        .map_err(|_| internal("the owned sign-in could not be kept"))
}
impl ProviderRole {
    fn handover_process(
        &self,
        copy: &ProviderCredentialCopy,
        provider: Id,
    ) -> Result<Arc<Process>, CommandError> {
        let handover = copy
            .handover
            .as_ref()
            .ok_or_else(|| unsupported("the closed sign-in purpose is absent"))?;
        self.services
            .agents
            .get()
            .ok_or_else(|| unsupported("this keeper has no native agent role"))?
            .process(provider, handover.profile)
    }
    pub(super) async fn issue_owner_handover(
        &self,
        body: &ProviderCredentialIssue,
    ) -> Result<SealedCredential, CommandError> {
        let copy = body
            .copy
            .as_ref()
            .ok_or_else(|| unsupported("the owned handover is absent"))?;
        let handover = copy
            .handover
            .as_ref()
            .ok_or_else(|| unsupported("the sign-in purpose is absent"))?;
        let from = if handover.returning {
            copy.target_provider_id
        } else {
            copy.source_provider_id
        };
        if body.lease_id.is_some()
            || body.provider_id != from
            || body.form != form(copy)?
            || copy.source_provider_id == copy.target_provider_id
            || !copy.headers.is_empty()
            || handover.returning && body.recipient != handover.original
            || !handover.returning && body.return_credential.is_some()
        {
            return Err(unsupported(
                "the handover does not match its real original and recipient",
            ));
        }
        let receipt = issued(copy);
        if let Some(saved) = self
            .store()
            .get(&receipt)
            .map_err(|_| internal("the native handover receipt could not be read"))?
        {
            let saved: Issued = serde_json::from_str(&saved)
                .map_err(|_| internal("the native handover receipt is invalid"))?;
            if saved.purpose != *copy || saved.recipient != body.recipient {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the owned handover already names another recipient",
                ));
            }
            return Ok(saved.result);
        }
        let credential = if handover.returning {
            let original = forward(copy);
            if let Some(mut held) = read(self.store(), from, copy.job_id)? {
                if held.purpose != original {
                    return Err(unsupported("another native job owns the pending sign-in"));
                }
                if held.installed {
                    let process = self.handover_process(copy, from)?;
                    held.credential = process
                        .call(|adapter| Box::pin(async move { adapter.return_credential().await }))
                        .await?;
                    held.installed = false;
                    save(self.store(), from, &held)?;
                }
                held.credential
            } else {
                let sealed = body
                    .return_credential
                    .as_ref()
                    .ok_or_else(|| unsupported("the original native issue receipt is absent"))?;
                if sealed.form != handover.form || sealed.expires_at.is_some() {
                    return Err(unsupported("the owned return has another native form"));
                }
                let text = self
                    .services
                    .keys
                    .open_with(&info(&original)?, &sealed.credential.value)?;
                serde_json::from_str::<GivenCredential>(&text)
                    .map_err(|_| internal("the original issued sign-in is invalid"))?
            }
        } else {
            let process = self.handover_process(copy, from)?;
            let support = process
                .started()
                .1
                .ok_or_else(|| {
                    unsupported("the source harness has no actual credential declaration")
                })?
                .credentials;
            if !support.issues.iter().any(|declared| {
                declared.form == handover.form
                    && declared.custody == Custody::Transferred
                    && declared.lifetime_secs.is_none()
            }) {
                return Err(unsupported(
                    "the source does not transfer that renewable sign-in form",
                ));
            }
            let form = handover.form.clone();
            process
                .call(move |adapter| Box::pin(async move { adapter.issue_credential(&form).await }))
                .await?
                .into_given(None)
        };
        if credential.form != handover.form || credential.expires_at.is_some() {
            return Err(unsupported(
                "a renewable sign-in cannot be replaced by an expiring access token",
            ));
        }
        let text = serde_json::to_string(&credential)
            .map_err(|_| internal("the native sign-in could not be encoded"))?;
        let value =
            self.services
                .keys
                .seal_with(&body.recipient.public_key, &info(copy)?, &text)?;
        let result = SealedCredential {
            form: handover.form.clone(),
            credential: Sealed { value },
            expires_at: None,
        };
        self.store()
            .put(
                &receipt,
                &serde_json::to_string(&Issued {
                    purpose: copy.clone(),
                    recipient: body.recipient.clone(),
                    result: result.clone(),
                })
                .map_err(|_| internal("the owned handover receipt could not be encoded"))?,
            )
            .map_err(|_| internal("the owned handover receipt could not be kept"))?;
        Ok(result)
    }
    pub(super) async fn accept_owner_handover(
        &self,
        body: &ProviderCredentialAccept,
    ) -> Result<(), CommandError> {
        let copy = body
            .copy
            .as_ref()
            .ok_or_else(|| unsupported("the owned handover is absent"))?;
        let handover = copy
            .handover
            .as_ref()
            .ok_or_else(|| unsupported("the sign-in purpose is absent"))?;
        let target = if handover.returning {
            copy.source_provider_id
        } else {
            copy.target_provider_id
        };
        if body.provider_id != target
            || body.form != handover.form
            || body.expires_at.is_some()
            || copy.source_provider_id == copy.target_provider_id
            || !copy.headers.is_empty()
        {
            return Err(unsupported(
                "the owned sign-in has another original or target",
            ));
        }
        let text = self
            .services
            .keys
            .open_with(&info(copy)?, &body.credential.value)?;
        let credential: GivenCredential =
            serde_json::from_str(&text).map_err(|_| internal("the sealed sign-in is invalid"))?;
        if credential.form != handover.form || credential.expires_at.is_some() {
            return Err(unsupported("the owned sign-in has another form"));
        }
        let mut held = match read(self.store(), target, copy.job_id)? {
            Some(held)
                if held.purpose == *copy
                    && Hash::of(held.credential.credential.expose().as_bytes())
                        == Hash::of(credential.credential.expose().as_bytes()) =>
            {
                held
            }
            Some(_) => {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the provider already accepted another owned sign-in",
                ));
            }
            None => {
                let held = Held {
                    purpose: copy.clone(),
                    credential,
                    installed: false,
                };
                save(self.store(), target, &held)?;
                held
            }
        };
        if held.installed {
            return Ok(());
        }
        let process = self.handover_process(copy, target)?;
        let declared = process
            .started()
            .1
            .ok_or_else(|| unsupported("the recipient has no actual credential declaration"))?
            .credentials;
        if !declared.accepts.contains(&handover.form) {
            return Err(unsupported(
                "the recipient does not accept the renewable sign-in form",
            ));
        }
        let given = held.credential.clone();
        if handover.returning {
            process
                .call(move |adapter| {
                    Box::pin(async move { adapter.accept_credential(given).await })
                })
                .await?;
        } else {
            process
                .call(move |adapter| Box::pin(async move { adapter.set_credential(given).await }))
                .await?;
        }
        held.installed = true;
        save(self.store(), target, &held)?;
        Ok(())
    }
}
