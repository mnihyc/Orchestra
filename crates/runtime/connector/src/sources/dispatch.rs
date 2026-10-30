//! Source choices resolved beside the credential, with keeper metadata, per-member reports and safe link results.

use super::{
    adapter::Adapter,
    address::{AddressClient, AddressOptions, AddressSource},
    endpoints::Endpoints,
    reports,
};
use crate::{
    forwarder::Forwarder,
    harnesses::Process,
    roles::{Services, provider::ProviderEntry},
    upstream::Upstream,
};
use orch_agent_contract::facts::{CredentialFacts, CredentialYield, ProviderFacts, normalize};
use orch_link_contract::commands::{
    ProviderDiscoverModelsResult, ProviderRefreshResult, ProviderResetConsumeResult,
    ProviderSourceRead,
};
use orch_model_contract::credentials::Credential;
use orch_plugin_contract::Secret;
use orch_provider_contract::choice::{Keeper, builtin, default_source};
use orch_provider_contract::source::{
    SourceAsk, SourceAskCredential, SourceAskEndpoint, SourceAskProvider,
};
use orch_provider_contract::{
    FactsSource, ModelsSource, SourceBy, SourceDescriptor, SourceElement, SourceError,
    SourceErrorCode, SourceSecret, SourceSpec,
};
use orch_types::{Hash, Id, Json, Time};
use std::collections::BTreeMap;
use std::sync::Arc;

struct CachedAddress {
    fingerprint: Hash,
    source: Arc<AddressSource>,
}

/// Address bindings cached only for the current choice of each provider and element on one connector.
#[derive(Default)]
pub struct Runtime {
    addresses: tokio::sync::Mutex<BTreeMap<(Id, SourceElement), CachedAddress>>,
}

impl Runtime {
    /// Releases opened address authorizations when a provider's keeping runtime is stopped.
    pub async fn forget(&self, provider: Id) {
        self.addresses
            .lock()
            .await
            .retain(|(id, _), _| *id != provider);
    }

    async fn address(
        &self,
        context: &Context,
        spec: &SourceSpec,
        opened: &Opened,
        element: SourceElement,
    ) -> Result<Arc<AddressSource>, SourceError> {
        let address = spec
            .address
            .clone()
            .ok_or_else(|| SourceError::new(SourceErrorCode::Malformed, "source.address"))?;
        let encoded = serde_json::to_vec(&(
            &address,
            spec.allow_http,
            &opened.authorization,
            &opened.ask.secrets,
            context.key.as_ref().map(Credential::expose),
            &context.protected,
        ))
        .map_err(|_| SourceError::new(SourceErrorCode::Malformed, "source.address"))?;
        let fingerprint = Hash::of(&encoded);
        let slot = (context.provider, element);
        if let Some(cached) = self
            .addresses
            .lock()
            .await
            .get(&slot)
            .filter(|cached| cached.fingerprint == fingerprint)
        {
            return Ok(Arc::clone(&cached.source));
        }
        let client = AddressClient::new(
            Upstream::new(Arc::clone(&context.services.clock)).map_err(|_| {
                SourceError::new(
                    SourceErrorCode::Unavailable,
                    "the source's HTTP client is unavailable",
                )
            })?,
        );
        let source = Arc::new(
            AddressSource::connect(
                client,
                AddressOptions {
                    address,
                    authorization: opened.authorization.clone(),
                    allow_http: spec.allow_http,
                    policy: Arc::clone(&context.services.policy),
                    clock: Arc::clone(&context.services.clock),
                    known_secrets: opened.protected.clone(),
                },
            )
            .await?,
        );
        self.addresses.lock().await.insert(
            slot,
            CachedAddress {
                fingerprint,
                source: Arc::clone(&source),
            },
        );
        Ok(source)
    }
}

/// One command's credential context; no sign-in can appear in SourceAsk.credential.
pub struct Context {
    services: Arc<Services>,
    command: Id,
    provider: Id,
    endpoints: Vec<SourceAskEndpoint>,
    key: Option<Credential>,
    key_required: bool,
    native: Option<Arc<Adapter>>,
    endpoints_source: Option<Arc<Endpoints>>,
    protected: Vec<String>,
    began: Time,
}

struct Opened {
    ask: SourceAsk,
    authorization: Option<Secret>,
    protected: Vec<String>,
    send_key: bool,
}
struct Facts {
    source: Arc<dyn FactsSource>,
    native: bool,
}

impl Context {
    /// A key stays in the provider's keeping role, even when another source supplies its account details.
    #[must_use]
    pub fn key(
        services: Arc<Services>,
        command: Id,
        provider: Arc<ProviderEntry>,
        forwarder: Arc<Forwarder>,
        key: Option<Credential>,
    ) -> Self {
        let endpoints = provider
            .endpoints()
            .iter()
            .map(|endpoint| SourceAskEndpoint {
                protocol: endpoint.protocol().to_owned(),
                base_url: endpoint.base_url().to_string(),
            })
            .collect();
        let mut protected: Vec<String> = provider
            .secrets()
            .values()
            .map(|secret| secret.expose().to_owned())
            .collect();
        if let Some(key) = &key {
            protected.push(key.expose().to_owned());
        }
        let began = services.clock.time();
        Self {
            provider: provider.id(),
            command,
            endpoints,
            key,
            key_required: provider.needs_key(),
            endpoints_source: Some(Arc::new(Endpoints::new(forwarder, provider))),
            native: None,
            services,
            protected,
            began,
        }
    }

    /// A sign-in remains exclusively inside its own harness; an external facts source still reads the keeper.
    #[must_use]
    pub fn native(
        services: Arc<Services>,
        command: Id,
        provider: Id,
        processes: Vec<Arc<Process>>,
        renew: bool,
    ) -> Self {
        let began = services.clock.time();
        let native = Arc::new(Adapter::new(
            provider,
            processes,
            renew,
            Arc::clone(&services.clock),
        ));
        Self {
            services,
            command,
            provider,
            endpoints: Vec::new(),
            key: None,
            key_required: false,
            native: Some(native),
            endpoints_source: None,
            protected: Vec::new(),
            began,
        }
    }

    fn open(&self, spec: &SourceSpec) -> Result<Opened, SourceError> {
        let provider = spec.provider.clone().unwrap_or_else(|| SourceAskProvider {
            id: self.provider,
            name: self.provider.to_string(),
        });
        if provider.id != self.provider {
            return Err(SourceError::new(
                SourceErrorCode::Refused,
                "the source belongs to another provider",
            ));
        }
        let open = |value: &SourceSecret| match value {
            SourceSecret::Sealed(value) => self
                .services
                .keys
                .open(self.command, value)
                .map(Secret::new)
                .map_err(super::adapter::command_error),
            SourceSecret::Ref(_) => Err(SourceError::new(
                SourceErrorCode::Refused,
                "a source secret has not been sealed to this machine",
            )),
        };
        let secrets = spec
            .secrets
            .iter()
            .map(|(name, value)| Ok((name.clone(), open(value)?)))
            .collect::<Result<BTreeMap<_, _>, SourceError>>()?;
        let authorization = spec.authorization.as_ref().map(open).transpose()?;
        let mut protected = self.protected.clone();
        protected.extend(secrets.values().map(|secret| secret.expose().to_owned()));
        if let Some(token) = &authorization {
            protected.push(token.expose().to_owned());
        }
        let send_key = spec.by != SourceBy::Address
            || spec
                .address
                .as_ref()
                .is_some_and(|address| address.send_credential);
        Ok(Opened {
            ask: SourceAsk {
                provider,
                endpoints: self.endpoints.clone(),
                config: spec.config.clone(),
                secrets,
                credential: None,
            },
            authorization,
            protected,
            send_key,
        })
    }

    fn ask(&self, opened: &Opened, descriptor: &SourceDescriptor) -> SourceAsk {
        let mut ask = opened.ask.clone();
        ask.secrets
            .retain(|name, _| descriptor.secret_fields().any(|field| field.name == *name));
        if opened.send_key && descriptor.wants_key() {
            ask.credential = self.key.as_ref().map(|key| SourceAskCredential {
                key: Secret::new(key.expose().to_owned()),
            });
        }
        ask
    }

    fn safe(&self, error: SourceError, opened: Option<&Opened>) -> SourceError {
        let secrets = opened.map_or(self.protected.as_slice(), |opened| {
            opened.protected.as_slice()
        });
        error.scrubbed(&secrets.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn descriptor(
        &self,
        descriptor: SourceDescriptor,
        opened: &Opened,
    ) -> Result<SourceDescriptor, SourceError> {
        let raw = serde_json::to_value(descriptor).map_err(|_| {
            SourceError::new(
                SourceErrorCode::Malformed,
                "the source's descriptor has another shape",
            )
        })?;
        let descriptor: SourceDescriptor =
            serde_json::from_value(reports::scrub_value(&raw, &opened.protected)).map_err(
                |_| {
                    SourceError::new(
                        SourceErrorCode::Malformed,
                        "the source's descriptor has another shape",
                    )
                },
            )?;
        descriptor.check().map_err(|_| {
            SourceError::new(
                SourceErrorCode::Malformed,
                "the source's descriptor has another shape",
            )
        })?;
        Ok(descriptor)
    }

    /// What keeps this provider's credential, as the rule of default sources asks it.
    fn keeper(&self) -> Keeper {
        if self.native.is_some() {
            Keeper::SignIn
        } else if self.key_required {
            Keeper::Key
        } else {
            Keeper::NoKey
        }
    }

    /// The default source of `element`, by the one rule the server names it by too.
    fn default_name(&self, element: SourceElement) -> Option<&'static str> {
        let endpoints: Vec<url::Url> = self
            .endpoints
            .iter()
            .filter_map(|endpoint| url::Url::parse(&endpoint.base_url).ok())
            .collect();
        default_source(
            element,
            self.keeper(),
            &endpoints,
            &self.services.builtin_sources,
        )
    }

    async fn facts(
        &self,
        spec: &SourceSpec,
        opened: &Opened,
    ) -> Result<Option<Facts>, SourceError> {
        let name = match spec.by {
            SourceBy::None => return Ok(None),
            SourceBy::Default => match self.default_name(SourceElement::Facts) {
                Some(name) => name,
                None => return Ok(None),
            },
            SourceBy::Builtin => spec
                .name
                .as_deref()
                .ok_or_else(|| SourceError::new(SourceErrorCode::Malformed, "source.name"))?,
            SourceBy::Program => {
                let program = self
                    .services
                    .provider_sources
                    .get(spec.name.as_deref().unwrap_or_default())
                    .ok_or_else(|| {
                        SourceError::new(
                            SourceErrorCode::Unavailable,
                            "the selected provider program is not registered on this machine",
                        )
                    })?;
                let source: Arc<dyn FactsSource> = program;
                return Ok(Some(Facts {
                    source,
                    native: false,
                }));
            }
            SourceBy::Address => {
                let source = self
                    .services
                    .source_runtime
                    .address(self, spec, opened, SourceElement::Facts)
                    .await?;
                return Ok(Some(Facts {
                    source,
                    native: false,
                }));
            }
        };
        if name == builtin::ADAPTER {
            return self
                .native
                .clone()
                .map(|source| {
                    Some(Facts {
                        source,
                        native: true,
                    })
                })
                .ok_or_else(|| {
                    SourceError::new(
                        SourceErrorCode::Unsupported,
                        "there is no native sign-in on this provider",
                    )
                });
        }
        // A vendor's reader is one the program registered; it reads through this machine's own transport.
        let entry = self
            .services
            .builtin_sources
            .get(name)
            .ok_or_else(|| SourceError::unsupported("facts.read"))?;
        let client = Upstream::new(Arc::clone(&self.services.clock)).map_err(|_| {
            SourceError::new(
                SourceErrorCode::Unavailable,
                "the vendor client is unavailable",
            )
        })?;
        let source = (entry.make)(Arc::new(client), Arc::clone(&self.services.clock));
        Ok(Some(Facts {
            source,
            native: false,
        }))
    }

    async fn models(
        &self,
        spec: &SourceSpec,
        opened: &Opened,
    ) -> Result<Option<Arc<dyn ModelsSource>>, SourceError> {
        let name = match spec.by {
            SourceBy::None => return Ok(None),
            SourceBy::Default => match self.default_name(SourceElement::Models) {
                Some(name) => name,
                None => return Ok(None),
            },
            SourceBy::Builtin => spec
                .name
                .as_deref()
                .ok_or_else(|| SourceError::new(SourceErrorCode::Malformed, "source.name"))?,
            SourceBy::Program => {
                let program = self
                    .services
                    .provider_sources
                    .get(spec.name.as_deref().unwrap_or_default())
                    .ok_or_else(|| {
                        SourceError::new(
                            SourceErrorCode::Unavailable,
                            "the selected provider program is not registered on this machine",
                        )
                    })?;
                let source: Arc<dyn ModelsSource> = program;
                return Ok(Some(source));
            }
            SourceBy::Address => {
                return Ok(Some(
                    self.services
                        .source_runtime
                        .address(self, spec, opened, SourceElement::Models)
                        .await?,
                ));
            }
        };
        match name {
            builtin::ADAPTER => self
                .native
                .clone()
                .map(|source| Some(source as Arc<dyn ModelsSource>))
                .ok_or_else(|| SourceError::unsupported("models.list")),
            builtin::ENDPOINTS => self
                .endpoints_source
                .clone()
                .map(|source| Some(source as Arc<dyn ModelsSource>))
                .ok_or_else(|| SourceError::unsupported("models.list")),
            _ => Err(SourceError::unsupported("models.list")),
        }
    }

    fn cleaned(
        &self,
        report: &orch_provider_contract::FactsReport,
        used: bool,
        native: bool,
        opened: &Opened,
    ) -> Result<(ProviderFacts, Option<SourceError>), SourceError> {
        let mut clean = reports::read(
            &report.0,
            self.services.clock.time(),
            used,
            &opened.protected,
        )
        .map_err(|code| {
            SourceError::new(
                code.parse().unwrap_or(SourceErrorCode::Malformed),
                "the source's facts report has another shape",
            )
        })?;
        if !native {
            // Identity and credential custody are the keeper's observation, even if a source supplies them.
            if let Some(facts) = clean.facts.as_object_mut() {
                facts.remove("identity");
            }
            let credential = clean.facts.get("credential").cloned();
            if let Some(times) = clean
                .facts
                .get_mut("field_times")
                .and_then(Json::as_object_mut)
            {
                times.remove("identity");
                let at = times.remove("credential");
                if used && let Some(at) = at {
                    for member in ["expires_at", "issued_at"] {
                        if credential.as_ref().is_some_and(|credential| {
                            credential.is_null() || credential.get(member).is_some()
                        }) {
                            times.insert(format!("credential.{member}"), at.clone());
                        }
                    }
                }
            }
            if !used && let Some(facts) = clean.facts.as_object_mut() {
                facts.remove("credential");
            }
        }
        if native
            && let Some(mut original) = self.native.as_ref().and_then(|native| native.latest())
        {
            normalize(&mut original);
            for (field, at) in original.field_times {
                if clean.facts["field_times"].get(&field).is_some() {
                    clean.facts["field_times"][&field] = serde_json::json!(at);
                }
            }
            if clean.facts["field_times"]["credential_verified"]
                .as_str()
                .and_then(|at| at.parse::<Time>().ok())
                .is_some_and(|at| at < self.began)
            {
                if let Some(facts) = clean.facts.as_object_mut() {
                    facts.remove("credential_verified");
                }
                if let Some(times) = clean.facts["field_times"].as_object_mut() {
                    times.remove("credential_verified");
                }
            }
        }
        let note = (!clean.notes.is_empty()).then(|| {
            SourceError::new(
                SourceErrorCode::Malformed,
                format!("Some facts were omitted: {}", clean.notes.join(", ")),
            )
        });
        let facts = reports::normalized(&clean).map_err(|_| {
            SourceError::new(
                SourceErrorCode::Malformed,
                "the source's facts report has another shape",
            )
        })?;
        Ok((facts, note))
    }

    fn keeper_facts(&self, original: Option<&ProviderFacts>, signed_out: bool) -> ProviderFacts {
        let now = self.services.clock.time();
        if self.native.is_none() {
            let mut facts = crate::usage::api_provider_facts(self.key_required);
            for member in ["method", "yields"] {
                facts
                    .field_times
                    .insert(format!("credential.{member}"), now);
            }
            return facts;
        }
        let mut facts = ProviderFacts::default();
        if let Some(original) = original {
            facts.identity = original.identity.clone();
            if original.identity.is_some() || original.field_times.contains_key("identity") {
                facts.field_times.insert(
                    "identity".to_owned(),
                    original.field_times.get("identity").copied().unwrap_or(now),
                );
            }
            if let Some(credential) = &original.credential {
                facts.credential = Some(CredentialFacts {
                    method: credential.method.clone(),
                    yields: credential.yields,
                    expires_at: None,
                    issued_at: None,
                });
                for member in ["method", "yields"] {
                    let field = format!("credential.{member}");
                    facts.field_times.insert(
                        field.clone(),
                        original
                            .field_times
                            .get(&field)
                            .or_else(|| original.field_times.get("credential"))
                            .copied()
                            .unwrap_or(now),
                    );
                }
            }
        } else if signed_out {
            facts.credential = Some(CredentialFacts {
                method: None,
                yields: CredentialYield::None,
                expires_at: None,
                issued_at: None,
            });
            for member in ["method", "yields"] {
                facts
                    .field_times
                    .insert(format!("credential.{member}"), now);
            }
            facts.field_times.insert("identity".to_owned(), now);
        }
        facts
    }

    fn add_keeper(&self, facts: &mut ProviderFacts, keeper: ProviderFacts) {
        if keeper.field_times.contains_key("identity") {
            facts.identity = keeper.identity;
            if let Some(at) = keeper.field_times.get("identity") {
                facts.field_times.insert("identity".to_owned(), *at);
            }
        }
        if let Some(credential) = keeper.credential {
            let held = facts.credential.get_or_insert_with(|| credential.clone());
            held.method = credential.method;
            held.yields = credential.yields;
            for (field, at) in keeper.field_times {
                if field.starts_with("credential.") {
                    facts.field_times.insert(field, at);
                }
            }
        }
    }

    /// Reads the selected facts and the native keeper independently; only the keeper supplies its identity and method.
    pub async fn refresh(&self, spec: Option<&SourceSpec>) -> ProviderRefreshResult {
        let default = SourceSpec::default();
        let spec = spec.unwrap_or(&default);
        let keeper = async {
            if let Some(native) = &self.native {
                tokio::time::timeout(
                    orch_provider_contract::wire::READ_DEADLINE,
                    native.observe(),
                )
                .await
                .map_err(|_| {
                    SourceError::new(
                        SourceErrorCode::Timeout,
                        "the credential keeper did not answer in time",
                    )
                })?
                .map_err(super::adapter::command_error)
            } else {
                Ok(None)
            }
        };
        let mut described = None;
        let read = async {
            let opened = self.open(spec)?;
            let selected = self.facts(spec, &opened).await?;
            let Some(selected) = selected else {
                return Ok((
                    ProviderFacts::default(),
                    ProviderSourceRead {
                        descriptor: None,
                        error: None,
                        supported: Some(false),
                    },
                ));
            };
            let descriptor = self.descriptor(selected.source.describe(), &opened)?;
            described = Some(descriptor.clone());
            if descriptor.facts.is_none() {
                return Ok((
                    ProviderFacts::default(),
                    ProviderSourceRead {
                        descriptor: Some(descriptor),
                        error: None,
                        supported: Some(false),
                    },
                ));
            }
            let ask = self.ask(&opened, &descriptor);
            let used = selected.native || ask.credential.is_some();
            let answered = tokio::time::timeout(
                orch_provider_contract::wire::READ_DEADLINE,
                selected.source.read_facts(ask),
            )
            .await
            .map_err(|_| {
                SourceError::new(
                    SourceErrorCode::Timeout,
                    "the facts source did not answer in time",
                )
            })?;
            described = Some(self.descriptor(selected.source.describe(), &opened)?);
            let report = answered.map_err(|error| self.safe(error, Some(&opened)))?;
            let (facts, note) = self.cleaned(&report, used, selected.native, &opened)?;
            Ok::<_, SourceError>((
                facts,
                ProviderSourceRead {
                    descriptor: described.clone(),
                    error: note.map(|note| self.safe(note, Some(&opened))),
                    supported: Some(true),
                },
            ))
        };
        let (keeper, read) = tokio::join!(keeper, read);
        let signed_out = self.native.is_some() && matches!(&keeper, Ok(None));
        let kept = self.keeper_facts(keeper.as_ref().ok().and_then(Option::as_ref), signed_out);
        match read {
            Ok((mut facts, source)) => {
                self.add_keeper(&mut facts, kept);
                let status = if signed_out {
                    "needs_setup"
                } else if exhausted(&facts, self.services.clock.time()) {
                    "exhausted"
                } else {
                    "ready"
                };
                ProviderRefreshResult {
                    source: Some(source),
                    status: status.to_owned(),
                    facts: Some(facts),
                }
            }
            Err(error)
                if signed_out
                    && (spec.is_default()
                        || (spec.by == SourceBy::Builtin
                            && spec.name.as_deref() == Some(builtin::ADAPTER))) =>
            {
                let _ = error;
                ProviderRefreshResult {
                    source: Some(ProviderSourceRead {
                        descriptor: self
                            .native
                            .as_ref()
                            .map(|native| FactsSource::describe(native.as_ref())),
                        error: None,
                        supported: Some(true),
                    }),
                    status: "needs_setup".to_owned(),
                    facts: Some(kept),
                }
            }
            Err(error) => ProviderRefreshResult {
                source: Some(ProviderSourceRead {
                    descriptor: described,
                    error: Some(self.safe(error, None)),
                    supported: None,
                }),
                status: "not_told".to_owned(),
                facts: self.native.as_ref().map(|_| kept),
            },
        }
    }

    /// Lists offered models, keeping unsupported, empty and a failed read distinct from one another.
    pub async fn discover(&self, spec: Option<&SourceSpec>) -> ProviderDiscoverModelsResult {
        let default = SourceSpec::default();
        let spec = spec.unwrap_or(&default);
        let mut described = None;
        let read = async {
            let opened = self.open(spec)?;
            let Some(source) = self.models(spec, &opened).await? else {
                return Ok((
                    Vec::new(),
                    ProviderSourceRead {
                        descriptor: None,
                        error: None,
                        supported: Some(false),
                    },
                ));
            };
            let descriptor = self.descriptor(source.describe(), &opened)?;
            described = Some(descriptor.clone());
            if !descriptor.lists_models() {
                return Ok((
                    Vec::new(),
                    ProviderSourceRead {
                        descriptor: Some(descriptor),
                        error: None,
                        supported: Some(false),
                    },
                ));
            }
            let report = tokio::time::timeout(
                orch_provider_contract::wire::LIST_DEADLINE,
                source.list_models(self.ask(&opened, &descriptor)),
            )
            .await
            .map_err(|_| {
                SourceError::new(
                    SourceErrorCode::Timeout,
                    "the models source did not answer in time",
                )
            })?
            .map_err(|error| self.safe(error, Some(&opened)))?;
            described = Some(self.descriptor(source.describe(), &opened)?);
            let raw = serde_json::to_value(&report.models)
                .map_err(|_| SourceError::new(SourceErrorCode::Malformed, "models"))?;
            let clean = reports::models(&raw, &opened.protected).map_err(|code| {
                SourceError::new(code.parse().unwrap_or(SourceErrorCode::Malformed), "models")
            })?;
            let supported = clean.models.is_some();
            let note = (!clean.notes.is_empty()).then(|| {
                self.safe(
                    SourceError::new(
                        SourceErrorCode::Malformed,
                        format!("Some models were omitted: {}", clean.notes.join(", ")),
                    ),
                    Some(&opened),
                )
            });
            Ok::<_, SourceError>((
                clean.models.unwrap_or_default(),
                ProviderSourceRead {
                    descriptor: described.clone(),
                    error: note,
                    supported: Some(supported),
                },
            ))
        }
        .await;
        match read {
            Ok((suggestions, source)) => ProviderDiscoverModelsResult {
                source: Some(source),
                suggestions,
            },
            Err(error) => ProviderDiscoverModelsResult {
                source: Some(ProviderSourceRead {
                    descriptor: described,
                    error: Some(self.safe(error, None)),
                    supported: None,
                }),
                suggestions: Vec::new(),
            },
        }
    }

    /// Spends through the same selected facts source, preserving the controller's logical request identity.
    pub async fn reset(
        &self,
        spec: Option<&SourceSpec>,
        request: orch_provider_contract::source::ResetRequest,
    ) -> Result<ProviderResetConsumeResult, SourceError> {
        let default = SourceSpec::default();
        let spec = spec.unwrap_or(&default);
        let opened = self.open(spec)?;
        let selected = self
            .facts(spec, &opened)
            .await?
            .ok_or_else(|| SourceError::unsupported("resets.spend"))?;
        let descriptor = self.descriptor(selected.source.describe(), &opened)?;
        if !descriptor.spends_resets() {
            return Err(SourceError::unsupported("resets.spend"));
        }
        let ask = self.ask(&opened, &descriptor);
        let used = selected.native || ask.credential.is_some();
        let keeper = async {
            if !selected.native
                && let Some(native) = &self.native
            {
                let _ = tokio::time::timeout(
                    orch_provider_contract::wire::READ_DEADLINE,
                    native.observe(),
                )
                .await;
            }
        };
        let spend = tokio::time::timeout(
            orch_provider_contract::wire::LIST_DEADLINE,
            selected.source.spend_reset(ask, request),
        );
        let (_, report) = tokio::join!(keeper, spend);
        let report = report
            .map_err(|_| {
                SourceError::new(
                    SourceErrorCode::Timeout,
                    "the reset source did not answer in time",
                )
            })?
            .map_err(|error| self.safe(error, Some(&opened)))?;
        if report.outcome.is_empty()
            || report.outcome.chars().count() > orch_provider_contract::limits::TEXT_MAX_CHARS
        {
            return Err(SourceError::new(
                SourceErrorCode::Malformed,
                "the reset source did not give a bounded outcome",
            ));
        }
        let (mut facts, error) = if let Some(facts) = &report.facts {
            self.cleaned(facts, used, selected.native, &opened)?
        } else {
            (ProviderFacts::default(), None)
        };
        if let Some(native) = &self.native {
            self.add_keeper(
                &mut facts,
                self.keeper_facts(native.latest().as_ref(), false),
            );
        } else {
            self.add_keeper(&mut facts, self.keeper_facts(None, false));
        }
        Ok(ProviderResetConsumeResult {
            source: Some(ProviderSourceRead {
                descriptor: Some(descriptor),
                error: error.map(|error| self.safe(error, Some(&opened))),
                supported: Some(true),
            }),
            outcome: reports::scrub_message(&report.outcome, &opened.protected)
                .chars()
                .take(orch_provider_contract::limits::TEXT_MAX_CHARS)
                .collect(),
            facts,
        })
    }
}

/// Only a present, unended limit or the vendor's explicit refusal of ordinary use exhausts a provider.
#[must_use]
pub fn exhausted(facts: &ProviderFacts, now: Time) -> bool {
    let ended = facts
        .limit_reached
        .as_ref()
        .and_then(|limit| limit.resets_at)
        .filter(|at| *at <= now);
    let old_denial_ended = ended.is_some_and(|ended| {
        facts
            .field_times
            .get("ordinary_usage_allowed")
            .is_none_or(|at| *at <= ended)
    });
    facts.ordinary_usage_allowed == Some(false) && !old_denial_ended
        || facts
            .limit_reached
            .as_ref()
            .is_some_and(|limit| limit.resets_at.is_none_or(|at| at > now))
}

/// Retries of one decision always reach the source with the same vendor idempotency key.
#[must_use]
pub fn reset_request(
    provider: Id,
    request: Id,
    reset_id: String,
) -> orch_provider_contract::source::ResetRequest {
    orch_provider_contract::source::ResetRequest {
        reset_id,
        idempotency_key: Hash::of(format!("provider-reset:{provider}:{request}").as_bytes())
            .to_string(),
    }
}

/// A reset failure carries safe metadata; the controller retries it with the same logical request id.
#[must_use]
pub fn failed_reset(error: SourceError) -> ProviderResetConsumeResult {
    ProviderResetConsumeResult {
        source: Some(ProviderSourceRead {
            descriptor: None,
            error: Some(error),
            supported: None,
        }),
        outcome: String::new(),
        facts: ProviderFacts::default(),
    }
}
