//! A locally registered program presents a key, with endpoint-specific renewal and no fallback after a failed exchange.

use super::Programs;
use crate::roles::{Services, provider::EndpointEntry};
use http::{HeaderMap, HeaderName, HeaderValue};
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_model_contract::credentials::Credential;
use orch_plugin_contract::Secret;
use orch_provider_contract::source::{SourceAskCredential, SourceAskEndpoint, SourceAskProvider};
use orch_provider_contract::{
    CredentialPresenter, PresentAsk, Presented, SourceBy, SourceError, SourceErrorCode,
    SourceSecret, SourceSpec, limits, wire,
};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

/// Opened only on the keeping machine, then persisted inside its encrypted provider record.
#[derive(Clone, Serialize, Deserialize)]
pub struct Selection {
    provider: SourceAskProvider,
    name: String,
    config: BTreeMap<String, Json>,
    secrets: BTreeMap<String, Secret>,
}

impl std::fmt::Debug for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Selection")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Selection {
    /// A default/none choice keeps the endpoint's own key presentation; only a program may replace it.
    pub fn open(
        services: &Services,
        command: Id,
        provider: Id,
        spec: Option<&SourceSpec>,
    ) -> Result<Option<Self>, CommandError> {
        let Some(spec) = spec else {
            return Ok(None);
        };
        if matches!(spec.by, SourceBy::Default | SourceBy::None) {
            return Ok(None);
        }
        if spec.by != SourceBy::Program {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "only a registered local program may present a key",
            ));
        }
        let selected = spec.provider.clone().unwrap_or_else(|| SourceAskProvider {
            id: provider,
            name: provider.to_string(),
        });
        if selected.id != provider {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the presenter belongs to another provider",
            ));
        }
        let name = spec
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Unsupported,
                    "the presenter needs its registered program name",
                )
            })?;
        let secrets = spec
            .secrets
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    SourceSecret::Sealed(value) => services.keys.open(command, value)?,
                    SourceSecret::Ref(_) => {
                        return Err(CommandError::new(
                            ErrorCode::Conflict,
                            "a presenter secret was not sealed to this machine",
                        ));
                    }
                };
                Ok((name.clone(), Secret::new(value)))
            })
            .collect::<Result<_, CommandError>>()?;
        Ok(Some(Self {
            provider: selected,
            name,
            config: spec.config.clone(),
            secrets,
        }))
    }
}

struct Cached {
    key: Hash,
    headers: HeaderMap,
    until: Time,
}
type Slot = Arc<tokio::sync::Mutex<Option<Cached>>>;

/// A provider's chosen program, with a separate cache for every endpoint and current key.
pub struct Presenter {
    selection: Selection,
    programs: Arc<Programs>,
    clock: Arc<dyn Clock>,
    protected: Vec<String>,
    slots: Mutex<BTreeMap<(String, String), Slot>>,
}

impl Presenter {
    /// Tokens and opened source fields live beside the key and are forgotten with this provider entry.
    #[must_use]
    pub fn new(selection: Selection, services: &Services, protected: Vec<String>) -> Self {
        Self {
            selection,
            programs: Arc::clone(&services.provider_sources),
            clock: Arc::clone(&services.clock),
            protected,
            slots: Mutex::new(BTreeMap::new()),
        }
    }

    fn failed(error: SourceError, secrets: &[String]) -> SourceError {
        let error = error.scrubbed(&secrets.iter().map(String::as_str).collect::<Vec<_>>());
        SourceError {
            code: SourceErrorCode::CredentialUnavailable,
            message: error.message,
            retryable: error.retryable,
        }
    }

    /// A whole exchange, including waiting for another caller's refresh, stays inside the presentation deadline.
    pub async fn headers(
        &self,
        endpoint: &EndpointEntry,
        credential: Option<&Credential>,
    ) -> Result<HeaderMap, SourceError> {
        let mut secrets = self.protected.clone();
        secrets.extend(
            self.selection
                .secrets
                .values()
                .map(|secret| secret.expose().to_owned()),
        );
        let credential = credential
            .filter(|key| !key.expose().is_empty())
            .ok_or_else(|| {
                SourceError::new(
                    SourceErrorCode::CredentialUnavailable,
                    "the presenter has no provider key",
                )
            })?;
        secrets.push(credential.expose().to_owned());
        tokio::time::timeout(
            wire::deadline(wire::CREDENTIAL_PRESENT),
            self.obtain(endpoint, credential, &secrets),
        )
        .await
        .map_err(|_| {
            SourceError::new(
                SourceErrorCode::CredentialUnavailable,
                "the credential presenter did not answer in time",
            )
        })?
    }

    async fn obtain(
        &self,
        endpoint: &EndpointEntry,
        credential: &Credential,
        secrets: &[String],
    ) -> Result<HeaderMap, SourceError> {
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(
                slots
                    .entry((
                        endpoint.protocol().to_owned(),
                        endpoint.base_url().to_string(),
                    ))
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None))),
            )
        };
        let mut cache = slot.lock().await;
        let now = self.clock.time();
        let key = Hash::of(credential.expose().as_bytes());
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.key == key && cached.until > now)
        {
            return Ok(cached.headers.clone());
        }
        let mut secrets = secrets.to_vec();
        if let Some(cached) = cache.as_ref() {
            for value in cached
                .headers
                .values()
                .filter_map(|value| value.to_str().ok())
            {
                secrets.push(value.to_owned());
                if let Some(token) = value.strip_prefix("Bearer ") {
                    secrets.push(token.to_owned());
                }
            }
        }
        let unavailable = |message| {
            Self::failed(
                SourceError::new(SourceErrorCode::CredentialUnavailable, message),
                &secrets,
            )
        };
        let source = self.programs.get(&self.selection.name).ok_or_else(|| {
            unavailable("the credential presenter is not available on this machine")
        })?;
        let descriptor = CredentialPresenter::describe(source.as_ref());
        if descriptor.check().is_err() || !descriptor.presents_credential() {
            return Err(unavailable(
                "the selected program does not declare credential presentation",
            ));
        }
        let mut fields = self.selection.secrets.clone();
        fields.retain(|name, _| descriptor.secret_fields().any(|field| field.name == *name));
        let reply = source
            .present(PresentAsk {
                provider: self.selection.provider.clone(),
                endpoint: SourceAskEndpoint {
                    protocol: endpoint.protocol().to_owned(),
                    base_url: endpoint.base_url().to_string(),
                },
                config: self.selection.config.clone(),
                secrets: fields,
                credential: SourceAskCredential {
                    key: Secret::new(credential.expose().to_owned()),
                },
            })
            .await
            .map_err(|error| Self::failed(error, &secrets))?;
        let headers = checked(&reply).map_err(&unavailable)?;
        if reply.expires_at.is_some_and(|at| at <= self.clock.time()) {
            return Err(unavailable(
                "the credential presenter returned an expired credential",
            ));
        }
        let until = reply.reuse_until(self.clock.time());
        *cache = Some(Cached {
            key,
            headers: headers.clone(),
            until,
        });
        Ok(headers)
    }
}

fn checked(reply: &Presented) -> Result<HeaderMap, &'static str> {
    if serde_json::to_vec(reply)
        .map_err(|_| "the credential presenter returned malformed headers")?
        .len()
        > limits::REPORT_MAX_BYTES
    {
        return Err("the credential presenter returned too much");
    }
    let mut headers = HeaderMap::new();
    for (name, value) in &reply.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| "the credential presenter returned an invalid header")?;
        if orch_model_contract::endpoint::HOP_BY_HOP.contains(&name.as_str())
            || orch_model_contract::endpoint::FORWARDER_HEADERS.contains(&name.as_str())
        {
            return Err("the credential presenter returned a connection header");
        }
        if headers.contains_key(&name) {
            return Err("the credential presenter repeated a header");
        }
        let mut value = HeaderValue::from_str(value.expose())
            .map_err(|_| "the credential presenter returned an invalid header value")?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    Ok(headers)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use orch_provider_contract::source::SourceDescriptorPresents;
    use orch_provider_contract::{
        FactsReport, FactsSource, ModelsReport, ModelsSource, SourceAsk, SourceDescriptor,
    };

    struct Stalled;
    impl Stalled {
        fn descriptor() -> SourceDescriptor {
            let mut descriptor =
                SourceDescriptor::new("stalled", "Stalled", "A stalled test exchange.");
            descriptor.presents = Some(SourceDescriptorPresents::default());
            descriptor
        }
    }
    #[async_trait]
    impl FactsSource for Stalled {
        fn describe(&self) -> SourceDescriptor {
            Self::descriptor()
        }
        async fn read_facts(&self, _ask: SourceAsk) -> Result<FactsReport, SourceError> {
            Err(SourceError::unsupported(wire::FACTS_READ))
        }
    }
    #[async_trait]
    impl ModelsSource for Stalled {
        fn describe(&self) -> SourceDescriptor {
            Self::descriptor()
        }
        async fn list_models(&self, _ask: SourceAsk) -> Result<ModelsReport, SourceError> {
            Err(SourceError::unsupported(wire::MODELS_LIST))
        }
    }
    #[async_trait]
    impl CredentialPresenter for Stalled {
        fn describe(&self) -> SourceDescriptor {
            Self::descriptor()
        }
        async fn present(&self, _ask: PresentAsk) -> Result<Presented, SourceError> {
            std::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_presenter_releases_the_request_at_the_shared_eight_second_deadline() {
        let mut programs = Programs::default();
        programs.register("stalled", Arc::new(Stalled)).unwrap();
        let at: Time = "2026-10-13T00:00:00Z".parse().unwrap();
        let presenter = Presenter {
            selection: Selection {
                provider: SourceAskProvider {
                    id: orch_types::new_id(),
                    name: "Team".to_owned(),
                },
                name: "stalled".to_owned(),
                config: BTreeMap::new(),
                secrets: BTreeMap::new(),
            },
            programs: Arc::new(programs),
            clock: Arc::new(orch_types::ManualClock::new(at.get())),
            protected: Vec::new(),
            slots: Mutex::new(BTreeMap::new()),
        };
        let endpoint = EndpointEntry::new(
            "openai-responses",
            url::Url::parse("https://example.invalid/v1").unwrap(),
            Default::default(),
        );
        let key = Credential::new("made-up-key");
        let began = tokio::time::Instant::now();
        let error = presenter.headers(&endpoint, Some(&key)).await.unwrap_err();
        assert_eq!(error.code, SourceErrorCode::CredentialUnavailable);
        assert_eq!(began.elapsed(), wire::deadline(wire::CREDENTIAL_PRESENT));
        assert!(!error.message.contains(key.expose()));
    }
}
