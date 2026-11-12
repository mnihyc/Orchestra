//! A permanent static owner copy is a closed purpose of the existing sealed issue/accept commands.
use super::*;
use orch_link_contract::commands::{
    MachineKey, PROVIDER_COPY_FORM, ProviderCredentialCopy, ProviderStartUpstream, Sealed,
    SealedCredential,
};
use orch_types::Hash;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Values {
    pub key: String,
    pub headers: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bundle {
    pub context: ProviderCredentialCopy,
    pub accepted: Hash,
    pub values: Values,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Issued {
    context: ProviderCredentialCopy,
    recipient: MachineKey,
    result: SealedCredential,
}
fn info(copy: &ProviderCredentialCopy) -> Result<Vec<u8>, CommandError> {
    serde_json::to_vec(&("orch/credential/v1/owner-copy", copy))
        .map_err(|_| internal("the owner copy context could not be encoded"))
}

pub(super) fn bundle_name(provider: Id) -> String {
    format!("provider-{provider}.copy")
}
pub(super) fn issue_prefix(provider: Id) -> String {
    format!("provider-{provider}.issued-copy-")
}
pub(super) fn load(store: &SecretStore, provider: Id) -> Result<Option<Bundle>, CommandError> {
    store
        .get(&bundle_name(provider))
        .map_err(|_| internal("the copied local credential could not be read"))?
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|_| internal("the copied local credential record is invalid"))
        })
        .transpose()
}
/// A changed definition cannot revive a removed native header on a later start.
pub(super) fn merge_headers(
    store: &SecretStore,
    provider: Id,
    upstreams: &[ProviderStartUpstream],
    headers: &mut BTreeMap<String, String>,
) -> Result<(), CommandError> {
    let Some(mut local) = load(store, provider)? else {
        return Ok(());
    };
    let declared = upstreams
        .iter()
        .flat_map(|upstream| {
            upstream
                .options
                .get("secret_headers")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
        })
        .collect::<std::collections::BTreeSet<_>>();
    let before = local.values.headers.len();
    local
        .values
        .headers
        .retain(|name, _| declared.contains(name.as_str()));
    for (name, value) in &local.values.headers {
        headers.entry(name.clone()).or_insert_with(|| value.clone());
    }
    if before != local.values.headers.len() {
        store
            .put(
                &bundle_name(provider),
                &serde_json::to_string(&local)
                    .map_err(|_| internal("the copied credential could not be encoded"))?,
            )
            .map_err(|_| internal("the removed copied headers could not be forgotten"))?;
    }
    Ok(())
}
impl ProviderRole {
    pub(super) async fn issue_owner_copy(
        &self,
        copy: &ProviderCredentialCopy,
        recipient: &MachineKey,
    ) -> Result<SealedCredential, CommandError> {
        let name = format!("{}{}", issue_prefix(copy.source_provider_id), copy.job_id);
        if let Some(saved) = self
            .store()
            .get(&name)
            .map_err(|_| internal("the owner copy receipt could not be read"))?
        {
            let saved: Issued = serde_json::from_str(&saved)
                .map_err(|_| internal("the owner copy receipt is invalid"))?;
            if saved.context != *copy || saved.recipient != *recipient {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the owner copy job already names another recipient",
                ));
            }
            return Ok(saved.result);
        }
        let mut values = if let Some(provider) = self.providers.get(copy.source_provider_id) {
            let key = provider.credential(self.store()).ok_or_else(|| {
                CommandError::new(
                    ErrorCode::LoginRequired,
                    "the source has no static key to copy",
                )
            })?;
            Values {
                key: key.expose().to_owned(),
                headers: provider
                    .secrets
                    .iter()
                    .map(|(name, value)| (name.clone(), value.expose().to_owned()))
                    .collect(),
            }
        } else {
            let record = self
                .store()
                .get(&provider_record_name(copy.source_provider_id))
                .map_err(|_| internal("the source provider record could not be read"))?;
            let record: Option<Stored> = record
                .map(|value| {
                    serde_json::from_str(&value)
                        .map_err(|_| internal("the source provider record is invalid"))
                })
                .transpose()?;
            let copied = load(self.store(), copy.source_provider_id)?;
            let held = record
                .as_ref()
                .and_then(|record| record.credential.clone())
                .or(self
                    .store()
                    .get(&local_key_name(copy.source_provider_id))
                    .map_err(|_| internal("the source static key could not be read"))?)
                .or_else(|| copied.as_ref().map(|bundle| bundle.values.key.clone()));
            Values {
                key: held.ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::LoginRequired,
                        "the source has no static key to copy",
                    )
                })?,
                headers: record
                    .map(|record| record.secrets)
                    .or_else(|| copied.map(|bundle| bundle.values.headers))
                    .unwrap_or_default(),
            }
        };
        values.headers.retain(|name, _| copy.headers.contains(name));
        let text = serde_json::to_string(&values)
            .map_err(|_| internal("the static owner copy could not be encoded"))?;
        let sealed = self
            .services
            .keys
            .seal_with(&recipient.public_key, &info(copy)?, &text)?;
        let result = SealedCredential {
            form: PROVIDER_COPY_FORM.into(),
            credential: Sealed { value: sealed },
            expires_at: None,
        };
        let saved = serde_json::to_string(&Issued {
            context: copy.clone(),
            recipient: recipient.clone(),
            result: result.clone(),
        })
        .map_err(|_| internal("the owner copy receipt could not be encoded"))?;
        let store = Arc::clone(self.store());
        tokio::task::spawn_blocking(move || store.put(&name, &saved))
            .await
            .map_err(|_| internal("the owner copy receipt could not be kept"))?
            .map_err(|_| internal("the owner copy receipt could not be kept"))?;
        Ok(result)
    }
    pub(super) async fn accept_owner_copy(
        &self,
        copy: &ProviderCredentialCopy,
        sealed: &Sealed,
    ) -> Result<(), CommandError> {
        let text = self.services.keys.open_with(&info(copy)?, &sealed.value)?;
        let values: Values = serde_json::from_str(&text)
            .map_err(|_| internal("the sealed static owner copy is invalid"))?;
        if values.key.is_empty() {
            return Err(CommandError::new(
                ErrorCode::LoginRequired,
                "the sealed static owner copy has no key",
            ));
        }
        let accepted = Hash::of(text.as_bytes());
        if let Some(before) = load(self.store(), copy.target_provider_id)? {
            return if before.context == *copy && before.accepted == accepted {
                Ok(())
            } else {
                Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the new provider already accepted another owner copy",
                ))
            };
        }
        if self
            .store()
            .get(&local_key_name(copy.target_provider_id))
            .map_err(|_| internal("the new provider's existing local key could not be checked"))?
            .is_some()
            || self
                .providers
                .get(copy.target_provider_id)
                .is_some_and(|provider| provider.credential.is_some())
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the new provider already has another credential",
            ));
        }
        let name = bundle_name(copy.target_provider_id);
        let value = serde_json::to_string(&Bundle {
            context: copy.clone(),
            accepted,
            values,
        })
        .map_err(|_| internal("the owner copy could not be encoded"))?;
        let store = Arc::clone(self.store());
        tokio::task::spawn_blocking(move || store.put(&name, &value))
            .await
            .map_err(|_| internal("the owner copy could not be stored"))?
            .map_err(|_| internal("the owner copy could not be stored"))?;
        Ok(())
    }
}
