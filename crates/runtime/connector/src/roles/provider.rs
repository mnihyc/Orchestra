//! Credential custody, sign-in on the provider by its adapter's methods, and model-serving role.
//!
//! The provider role is the machine that keeps the credential of a provider and lets it be used. For an
//! **API provider** (an upstream of one or more endpoints, each in a wire protocol) it holds the key, which
//! arrives sealed to this machine with `provider.start` and is kept in memory and in the sealed store
//! ([`crate::credentials`]), or is a key a person gave to this machine and the command carries none. It
//! serves the provider's model calls ([`crate::forwarder`]) under the leases the system opens for each turn
//! (`lease.open`, `lease.close`), lists the provider's models and says whether its key is taken
//! (`provider.discover_models`, `provider.refresh`), and issues the key as a copy, sealed to the machine
//! where a harness runs, when a lease asks for one. For a **subscription**, whose login a harness of the
//! agent role holds, it asks that harness to issue a copy of the login or to take the login back
//! (`provider.credential.issue`, `provider.credential.accept`); signing in and out and the state of the
//! sign-in are the harness's own commands and events, carried by the agent role.
//!
//! A machine that plays both roles answers a provider's models and state from whichever holds the provider:
//! a provider the role was started for here, or one whose sign-in a harness here holds (see
//! [`RoleHandler::claims`]).
//!
//! [`plug_in`] gives a connector the role, the listener's model endpoint and the server of the `llm`
//! tunnels; a machine that plays the agent role alone gets the endpoint and the tunnels' client only, which is
//! what a harness whose key is kept elsewhere needs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use orch_agent_contract::credentials::{GivenCredential, IssuedCredential};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode, LeaseCloseResult,
    LeaseMode, Secret as LinkSecret,
};
use orch_link_contract::events::Event;
use orch_link_contract::session::Role;
use orch_link_contract::streams::Stream;
use orch_model_contract::credentials::{Credential, WireError};
use orch_model_contract::endpoint::{EndpointOptions, check_base_url};
use orch_model_contract::protocol::SecretHeaders;
use orch_pki::certificates::LeafKey;
use orch_plugin_contract::Secret;
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::connector::{ConnectorBuilder, StartError};
use crate::credentials::{
    Direction, SecretStore, check_issue, exchange_info, local_key_name, provider_custodian_name,
    provider_ids, provider_record_name, seal_issued,
};
use crate::forwarder::{Forwarder, LlmEndpoint, LlmTunnels, Protocols};
use crate::harnesses::Process;
use crate::leases::{Lease, Leases};
use crate::loopback::EndpointKind;
use crate::roles::{Call, Keys, RoleHandler, Services};
use crate::upstream::{Upstream, UpstreamError};

/// The kinds of command the provider role takes, including text completions made on the system's behalf.
const TAKES: &[CommandKind] = &[
    CommandKind::UnitAfterUp,
    CommandKind::ProviderStart,
    CommandKind::ProviderStop,
    CommandKind::LeaseOpen,
    CommandKind::LeaseClose,
    CommandKind::ProviderCredentialIssue,
    CommandKind::ProviderCredentialAccept,
    CommandKind::ProviderRefresh,
    CommandKind::ProviderResetConsume,
    CommandKind::ProviderDiscoverModels,
    CommandKind::ProviderComplete,
    CommandKind::ProviderEmbed,
];

/// One endpoint of an API provider's upstream, as it was checked when the provider was started.
#[derive(Debug, Clone)]
pub struct EndpointEntry {
    protocol: String,
    base_url: Url,
    options: EndpointOptions,
}

impl EndpointEntry {
    /// An endpoint that speaks `protocol` at `base_url`.
    #[must_use]
    pub fn new(protocol: &str, base_url: Url, options: EndpointOptions) -> Self {
        Self {
            protocol: protocol.to_owned(),
            base_url,
            options,
        }
    }

    /// The wire protocol it speaks.
    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// Where it is; requests are made below this address.
    #[must_use]
    pub const fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// What it configures beyond its address.
    #[must_use]
    pub const fn options(&self) -> &EndpointOptions {
        &self.options
    }
}

/// An API provider started on this machine.
pub struct ProviderEntry {
    id: Id,
    endpoints: Vec<EndpointEntry>,
    credential: Option<Credential>,
    secrets: SecretHeaders,
    presenter: Option<Arc<crate::sources::presenter::Presenter>>,
    refused: AtomicBool,
}

impl std::fmt::Debug for ProviderEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderEntry")
            .field("id", &self.id)
            .field("endpoints", &self.endpoints)
            .finish_non_exhaustive()
    }
}

impl ProviderEntry {
    /// The provider.
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// Its endpoints, in the order the system listed them.
    #[must_use]
    pub fn endpoints(&self) -> &[EndpointEntry] {
        &self.endpoints
    }

    /// The endpoint that speaks `protocol`.
    #[must_use]
    pub fn endpoint_for(&self, protocol: &str) -> Option<&EndpointEntry> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.protocol == protocol)
    }

    /// The values of the secret headers of its endpoints, by name.
    #[must_use]
    pub const fn secrets(&self) -> &SecretHeaders {
        &self.secrets
    }

    /// A program chosen to present this key, in place of the endpoint's built-in presentation.
    #[must_use]
    pub fn presenter(&self) -> Option<&crate::sources::presenter::Presenter> {
        self.presenter.as_deref()
    }

    /// The key: the one `provider.start` brought, or else the one a person gave to this machine.
    #[must_use]
    pub fn credential(&self, store: &SecretStore) -> Option<Credential> {
        if let Some(held) = &self.credential {
            return Some(held.clone());
        }
        match store.get(&local_key_name(self.id)) {
            Ok(kept) => kept.map(Credential::new),
            Err(error) => {
                tracing::warn!(provider = %self.id, %error, "the key kept on this machine could not be read");
                None
            }
        }
    }

    /// Whether any endpoint takes a key.
    #[must_use]
    pub fn needs_key(&self) -> bool {
        self.endpoints.iter().any(|endpoint| {
            endpoint.options.auth != Some(orch_model_contract::endpoint::AuthMode::None)
        })
    }

    /// Notes whether the upstream refuses the key; true when that changed what was known.
    pub fn set_refused(&self, refused: bool) -> bool {
        self.refused.swap(refused, Ordering::SeqCst) != refused
    }
}

/// The API providers started on this machine.
#[derive(Debug, Default)]
pub struct Providers {
    map: Mutex<HashMap<Id, Arc<ProviderEntry>>>,
}

impl Providers {
    /// No provider.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn map(&self) -> MutexGuard<'_, HashMap<Id, Arc<ProviderEntry>>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The provider with this identifier.
    #[must_use]
    pub fn get(&self, id: Id) -> Option<Arc<ProviderEntry>> {
        self.map().get(&id).cloned()
    }

    /// Whether the provider is started here.
    #[must_use]
    pub fn contains(&self, id: Id) -> bool {
        self.map().contains_key(&id)
    }

    /// Starts a provider, in place of what it was.
    pub fn insert(&self, entry: ProviderEntry) {
        self.map().insert(entry.id, Arc::new(entry));
    }

    /// Forgets a provider.
    pub fn remove(&self, id: Id) -> Option<Arc<ProviderEntry>> {
        self.map().remove(&id)
    }
}

/// What `provider.start` brought, as it is kept in the sealed store: the endpoints as the system sent them,
/// and the key and the secret headers opened.
#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    upstreams: Vec<StoredUpstream>,
    credential: Option<String>,
    secrets: BTreeMap<String, String>,
    #[serde(default)]
    presenter: Option<crate::sources::presenter::Selection>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredUpstream {
    protocol: String,
    base_url: String,
    options: Json,
}

pub(crate) fn unsupported(detail: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::Unsupported, detail)
}

fn internal(detail: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::Internal, detail)
}

/// What is said when none of a provider's harness processes is signed in.
fn no_sign_in() -> CommandError {
    CommandError::new(
        ErrorCode::LoginRequired,
        "no harness of this machine is signed in to that provider",
    )
}

impl Stored {
    /// Checks what `provider.start` brought against the protocols this machine has, and makes the provider.
    fn into_entry(
        self,
        id: Id,
        protocols: &Protocols,
        services: &Services,
    ) -> Result<ProviderEntry, CommandError> {
        if self.upstreams.is_empty() {
            return Err(unsupported("a provider needs at least one endpoint"));
        }
        let mut endpoints: Vec<EndpointEntry> = Vec::new();
        for upstream in &self.upstreams {
            let entry = protocols.get(&upstream.protocol).ok_or_else(|| {
                unsupported(format!(
                    "this machine has no implementation of the protocol {:?}",
                    upstream.protocol
                ))
            })?;
            if endpoints
                .iter()
                .any(|known| known.protocol == upstream.protocol)
            {
                return Err(unsupported(format!(
                    "two endpoints speak {}; a provider has one for each protocol",
                    upstream.protocol
                )));
            }
            let options = EndpointOptions::parse(&upstream.options).map_err(|error| {
                unsupported(format!(
                    "the options of the {} endpoint: {error}",
                    upstream.protocol
                ))
            })?;
            options
                .check(entry.protocol().default_auth())
                .map_err(|faults| {
                    unsupported(format!(
                        "the options of the {} endpoint: {}",
                        upstream.protocol,
                        faults
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; ")
                    ))
                })?;
            // The system checked the scheme against its policy when the provider was saved (http is for
            // the owners who allowed it); what the connector asks is that it names a host.
            let base_url = check_base_url(&upstream.base_url, true)
                .map_err(|fault| unsupported(fault.to_string()))?;
            endpoints.push(EndpointEntry::new(&upstream.protocol, base_url, options));
        }
        let mut protected: Vec<String> = self.secrets.values().cloned().collect();
        if let Some(key) = &self.credential {
            protected.push(key.clone());
        }
        let presenter = self.presenter.map(|selected| {
            Arc::new(crate::sources::presenter::Presenter::new(
                selected, services, protected,
            ))
        });
        Ok(ProviderEntry {
            id,
            endpoints,
            credential: self.credential.map(Credential::new),
            secrets: self
                .secrets
                .into_iter()
                .map(|(name, value)| (name, Secret::new(value)))
                .collect(),
            refused: AtomicBool::new(false),
            presenter,
        })
    }
}

/// What the provider role needs that can fail to be made, made before the connector starts: the sealed
/// store, and the client to the upstreams.
pub struct ProviderParts {
    protocols: Protocols,
    upstream: Upstream,
    store: Arc<SecretStore>,
}

impl std::fmt::Debug for ProviderParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderParts").finish_non_exhaustive()
    }
}

impl ProviderParts {
    /// The parts for a connector whose data directory is `data_dir` and whose key is `key`, forwarding in
    /// the protocols `protocols`.
    pub fn new(
        protocols: Protocols,
        data_dir: &Path,
        key: &LeafKey,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StartError> {
        let keys = Arc::new(Keys::new(key.duplicate()?));
        Ok(Self {
            protocols,
            upstream: Upstream::new(clock)?,
            store: Arc::new(SecretStore::open(data_dir, keys)?),
        })
    }

    /// The same parts with another client to the upstreams (a test with an upstream of its own).
    #[must_use]
    pub fn with_upstream(mut self, upstream: Upstream) -> Self {
        self.upstream = upstream;
        self
    }
}

/// Gives a connector what serving model calls needs, for the roles it plays: the provider role where it
/// plays that one, and the listener's model endpoint and the tunnels' server where it plays that or the agent
/// role.
#[must_use]
pub fn plug_in(
    builder: ConnectorBuilder,
    roles: &[Role],
    parts: ProviderParts,
) -> ConnectorBuilder {
    if roles.contains(&Role::Provider) {
        builder.role(Box::new(move |services| {
            let role = ProviderRole::new(services, parts);
            Arc::new(role) as Arc<dyn RoleHandler>
        }))
    } else if roles.contains(&Role::Agent) {
        builder.endpoint(Box::new(move |services| {
            let forwarder = Forwarder::new(
                services,
                parts.protocols,
                Arc::new(Providers::new()),
                Arc::new(Leases::new()),
                parts.store,
                parts.upstream,
            );
            (
                EndpointKind::Llm,
                Arc::new(LlmEndpoint::new(forwarder)) as Arc<dyn crate::loopback::Endpoint>,
            )
        }))
    } else {
        builder
    }
}

/// The provider role.
pub struct ProviderRole {
    services: Arc<Services>,
    providers: Arc<Providers>,
    leases: Arc<Leases>,
    forwarder: Arc<Forwarder>,
    announced_sign_ins: Mutex<BTreeSet<(String, Option<String>)>>,
    discovery: tokio::sync::Mutex<()>,
    after_up: crate::after_up::Runner,
}

impl std::fmt::Debug for ProviderRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRole").finish_non_exhaustive()
    }
}

impl ProviderRole {
    /// The role over the services of a connector. It registers the listener's model endpoint and the server
    /// of the `llm` tunnels.
    #[must_use]
    pub fn new(services: &Arc<Services>, parts: ProviderParts) -> Self {
        let providers = Arc::new(Providers::new());
        let leases = Arc::new(Leases::stored(
            Arc::clone(&parts.store),
            services.clock.time(),
        ));
        let forwarder = Forwarder::new(
            services,
            parts.protocols,
            Arc::clone(&providers),
            Arc::clone(&leases),
            parts.store,
            parts.upstream,
        );
        services.loopback.register(
            EndpointKind::Llm,
            Arc::new(LlmEndpoint::new(Arc::clone(&forwarder))),
        );
        LlmTunnels::new(Arc::clone(&forwarder)).register(services);
        Self {
            services: Arc::clone(services),
            providers,
            leases,
            forwarder,
            announced_sign_ins: Mutex::new(BTreeSet::new()),
            discovery: tokio::sync::Mutex::new(()),
            after_up: crate::after_up::Runner::new(services),
        }
    }

    /// The forwarder.
    #[must_use]
    pub fn forwarder(&self) -> &Arc<Forwarder> {
        &self.forwarder
    }

    /// The providers started here.
    #[must_use]
    pub fn providers(&self) -> &Arc<Providers> {
        &self.providers
    }

    /// The leases open here.
    #[must_use]
    pub fn leases(&self) -> &Arc<Leases> {
        &self.leases
    }

    fn store(&self) -> &Arc<SecretStore> {
        self.forwarder.store()
    }

    fn open_secret(&self, call: &Call, secret: &LinkSecret) -> Result<String, CommandError> {
        match secret {
            LinkSecret::Sealed(sealed) => self.services.keys.open(call.context.id, sealed),
            LinkSecret::Ref(_) => Err(internal(
                "a reference to a secret reached this machine; the system seals a secret before it sends it",
            )),
        }
    }

    async fn start(
        &self,
        call: &Call,
        body: orch_link_contract::commands::ProviderStart,
    ) -> Result<(), CommandError> {
        let credential = body
            .credential
            .as_ref()
            .map(|secret| self.open_secret(call, secret))
            .transpose()?;
        let mut secrets = BTreeMap::new();
        for (name, secret) in &body.secrets {
            secrets.insert(name.clone(), self.open_secret(call, secret)?);
        }
        let stored = Stored {
            presenter: crate::sources::presenter::Selection::open(
                &self.services,
                call.context.id,
                body.provider_id,
                body.presenter.as_ref(),
            )?,
            upstreams: body
                .upstreams
                .iter()
                .map(|upstream| {
                    Ok(StoredUpstream {
                        protocol: upstream.protocol.clone(),
                        base_url: upstream.base_url.clone(),
                        options: upstream.options.clone(),
                    })
                })
                .collect::<Result<_, CommandError>>()?,
            credential,
            secrets,
        };
        let record = serde_json::to_string(&stored).map_err(|error| internal(error.to_string()))?;
        let entry =
            stored.into_entry(body.provider_id, self.forwarder.protocols(), &self.services)?;
        // A provider is kept before it is started: one the store could not keep is not started either, so
        // that what is running is what a restart brings back.
        let store = Arc::clone(self.store());
        let name = provider_record_name(body.provider_id);
        tokio::task::spawn_blocking(move || store.put(&name, &record))
            .await
            .map_err(|error| internal(error.to_string()))?
            .map_err(|error| internal(error.to_string()))?;
        self.providers.insert(entry);
        Ok(())
    }

    async fn stop(&self, provider_id: Id, purge: bool) -> Result<(), CommandError> {
        self.providers.remove(provider_id);
        self.services.source_runtime.forget(provider_id).await;
        let leases = self
            .leases
            .close_all_checked(provider_id, self.services.clock.time())
            .map_err(|error| internal(error.to_string()));
        let store = Arc::clone(self.store());
        let mut names = vec![provider_record_name(provider_id)];
        if purge {
            names.push(local_key_name(provider_id));
            names.push(provider_custodian_name(provider_id));
        }
        let removed =
            tokio::task::spawn_blocking(move || -> Result<(), crate::credentials::StoreError> {
                for name in names {
                    store.remove(&name)?;
                }
                Ok(())
            })
            .await
            .map_err(|error| internal(error.to_string()))?
            .map_err(|error| internal(error.to_string()));
        leases?;
        removed
    }

    /// The harness processes of the agent role that can hold the sign-in of a provider, in a fixed order. A
    /// provider may have several on a machine (one for each agent profile that runs on it) and only the one
    /// that was signed in has the login: the callers ask them in turn and pass over one that says it has none.
    fn processes_with_login(&self, provider_id: Id) -> Vec<Arc<Process>> {
        self.services
            .agents
            .get()
            .map(|hosting| hosting.processes_with_login(provider_id))
            .unwrap_or_default()
    }

    async fn issue(
        &self,
        body: orch_link_contract::commands::ProviderCredentialIssue,
    ) -> Result<orch_link_contract::commands::SealedCredential, CommandError> {
        let lease = self.leases.get(body.lease_id).ok_or_else(|| {
            CommandError::new(
                ErrorCode::LeaseClosed,
                "no lease of that identifier is open on this machine",
            )
        })?;
        if lease.provider_id() != body.provider_id {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the lease is for another provider",
            ));
        }
        let now = self.services.clock.time();
        let issued = if let Some(provider) = self.providers.get(body.provider_id) {
            let credential = provider.credential(self.store()).ok_or_else(|| {
                CommandError::new(
                    ErrorCode::LoginRequired,
                    "the provider has no key on this machine to issue a copy of",
                )
            })?;
            let (protocol, endpoint) = provider
                .endpoints()
                .iter()
                .filter_map(|endpoint| {
                    self.forwarder
                        .protocols()
                        .get(endpoint.protocol())
                        .map(|entry| (entry, endpoint))
                })
                .filter(|(entry, _)| lease.protocol().is_none_or(|wanted| wanted == entry.name()))
                .find(|(entry, _)| {
                    entry
                        .protocol()
                        .credential_forms()
                        .iter()
                        .any(|form| form.form == body.form)
                })
                .ok_or_else(|| {
                    unsupported(format!(
                        "no protocol of this provider issues the form {:?}",
                        body.form
                    ))
                })?;
            let declared = protocol
                .protocol()
                .credential_forms()
                .into_iter()
                .find(|form| form.form == body.form)
                .ok_or_else(|| unsupported("the form is not issued"))?;
            check_issue(&lease, &declared, &body.recipient, now)?;
            protocol
                .protocol()
                .issue_credential(&body.form, &credential, endpoint.base_url())
                .await
                .map_err(|error| match error {
                    WireError::UnknownForm(form) => {
                        unsupported(format!("the form {form:?} is not issued"))
                    }
                    other => internal(other.to_string()).retryable(),
                })?
        } else {
            self.issue_from_login(&lease, &body, now).await?
        };
        let sealed = seal_issued(
            &self.services.keys,
            body.provider_id,
            &issued,
            &body.recipient,
        )?;
        lease
            .issued_at_checked(now)
            .map_err(|error| internal(error.to_string()))?;
        Ok(sealed)
    }

    /// A copy of the login of a provider, issued by the one of its harness processes that is signed in.
    async fn issue_from_login(
        &self,
        lease: &Lease,
        body: &orch_link_contract::commands::ProviderCredentialIssue,
        now: Time,
    ) -> Result<IssuedCredential, CommandError> {
        let known = self
            .store()
            .get(&provider_custodian_name(body.provider_id))
            .map_err(|_| internal("the credential's custodian could not be read"))?
            .map(|profile| {
                profile
                    .parse::<Id>()
                    .map_err(|_| internal("the credential's custodian record is invalid"))
            })
            .transpose()?;
        let processes: Vec<_> = self
            .processes_with_login(body.provider_id)
            .into_iter()
            .filter(|process| known.is_none_or(|profile| process.agent_id == profile))
            .collect();
        if processes.is_empty() {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                "this machine holds neither the key nor the sign-in of that provider",
            ));
        }
        let mut signed_out = None;
        for process in processes {
            let declared = process
                .started()
                .1
                .and_then(|descriptor| {
                    descriptor
                        .credentials
                        .issues
                        .into_iter()
                        .find(|form| form.form == body.form)
                })
                .ok_or_else(|| {
                    unsupported(format!("the harness issues no form {:?}", body.form))
                })?;
            check_issue(lease, &declared, &body.recipient, now)?;
            // Write the destination before a login can move away. A failed store must never
            // leave a credential in transit with no durable route home.
            let name = provider_custodian_name(body.provider_id);
            let store = Arc::clone(self.store());
            let previous = store
                .get(&name)
                .map_err(|_| internal("the credential's custodian could not be read"))?;
            let profile = process.agent_id.to_string();
            let saved_name = name.clone();
            tokio::task::spawn_blocking(move || store.put(&saved_name, &profile))
                .await
                .map_err(|_| internal("the credential's custodian could not be kept"))?
                .map_err(|_| internal("the credential's custodian could not be kept"))?;
            let form = body.form.clone();
            match process
                .call(move |adapter| Box::pin(async move { adapter.issue_credential(&form).await }))
                .await
            {
                Err(error) if error.code == ErrorCode::LoginRequired.as_str() => {
                    let store = Arc::clone(self.store());
                    tokio::task::spawn_blocking(move || match previous {
                        Some(previous) => store.put(&name, &previous),
                        None => store.remove(&name),
                    })
                    .await
                    .map_err(|_| internal("the credential's custodian could not be restored"))?
                    .map_err(|_| internal("the credential's custodian could not be restored"))?;
                    signed_out = Some(error);
                }
                answered => return answered,
            }
        }
        Err(signed_out.unwrap_or_else(no_sign_in))
    }

    async fn accept(
        &self,
        body: orch_link_contract::commands::ProviderCredentialAccept,
    ) -> Result<(), CommandError> {
        let profile = self
            .store()
            .get(&provider_custodian_name(body.provider_id))
            .map_err(|_| internal("the credential's issuing profile could not be read"))?
            .and_then(|profile| profile.parse::<Id>().ok())
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::NotFound,
                    "the credential has no recorded issuing profile",
                )
            })?;
        let process = self
            .processes_with_login(body.provider_id)
            .into_iter()
            .find(|process| process.agent_id == profile)
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::NotFound,
                    "the credential's issuing profile is not running",
                )
            })?;
        let opened = self.services.keys.open_with(
            &exchange_info(Direction::Returned, body.provider_id, &body.form),
            &body.credential.value,
        )?;
        let given = GivenCredential {
            form: body.form,
            credential: Secret::new(opened),
            expires_at: body.expires_at,
            upstream: None,
        };
        process
            .call(move |adapter| Box::pin(async move { adapter.accept_credential(given).await }))
            .await
    }

    /// Facts, model lists and resets run beside the same key, through the selected source.
    fn source_context(
        &self,
        command: Id,
        provider: Arc<ProviderEntry>,
    ) -> crate::sources::dispatch::Context {
        let key = provider.credential(self.store());
        crate::sources::dispatch::Context::key(
            Arc::clone(&self.services),
            command,
            provider,
            Arc::clone(&self.forwarder),
            key,
        )
    }
}

pub(crate) fn refusal_of(status: u16) -> CommandError {
    match status {
        401 | 403 => CommandError::new(
            ErrorCode::LoginRequired,
            format!("the upstream refused the key ({status})"),
        ),
        _ => internal(format!("the upstream answered {status}")).retryable(),
    }
}

pub(crate) fn failure_of(error: &UpstreamError) -> CommandError {
    match error {
        UpstreamError::TimedOut(_) => {
            CommandError::new(ErrorCode::Timeout, error.to_string()).retryable()
        }
        _ => internal(error.to_string()).retryable(),
    }
}

#[async_trait]
impl RoleHandler for ProviderRole {
    fn role(&self) -> Role {
        Role::Provider
    }

    fn takes(&self) -> &'static [CommandKind] {
        TAKES
    }

    fn claims(&self, command: &Command) -> bool {
        match command {
            Command::UnitAfterUp(body) => {
                body.provider_id.is_some()
                    && body.environment_id.is_none()
                    && body.agent_id.is_none()
            }
            Command::ProviderRefresh(body) => self.providers.contains(body.provider_id),
            Command::ProviderResetConsume(body) => self.providers.contains(body.provider_id),
            Command::ProviderDiscoverModels(body) => self.providers.contains(body.provider_id),
            Command::ProviderComplete(body) => self.providers.contains(body.provider_id),
            Command::ProviderEmbed(body) => self.providers.contains(body.provider_id),
            _ => true,
        }
    }

    async fn handle(&self, call: Call, command: Command) -> Result<CommandResult, CommandError> {
        Ok(match command {
            Command::UnitAfterUp(body) => {
                if body.provider_id.is_none() || body.environment_id.is_some() {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "this command needs the provider runtime it runs in",
                    ));
                }
                let home = std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .ok_or_else(|| internal("the runtime's home directory is unavailable"))?;
                // Only a unit's own process environment contains its resolved runtime variables.
                // Directory providers select their Agent profile and its context instead; never
                // inherit unrelated variables from a physical credential keeper.
                let variables = if self.services.policy.policy().is_none() {
                    std::env::vars().collect()
                } else {
                    BTreeMap::new()
                };
                CommandResult::UnitAfterUp(
                    self.after_up
                        .run(&body, Path::new(&home), &variables)
                        .await?,
                )
            }
            Command::ProviderStart(body) => {
                self.start(&call, body).await?;
                CommandResult::ProviderStart(Empty {})
            }
            Command::ProviderStop(body) => {
                self.stop(body.provider_id, body.purge.unwrap_or(false))
                    .await?;
                CommandResult::ProviderStop(Empty {})
            }
            Command::LeaseOpen(body) => {
                self.open_lease(&body)?;
                CommandResult::LeaseOpen(Empty {})
            }
            Command::LeaseClose(body) => {
                let totals = self
                    .leases
                    .close_checked(body.lease_id, self.services.clock.time())
                    .map_err(|error| internal(error.to_string()))?;
                CommandResult::LeaseClose(LeaseCloseResult {
                    requests: totals.requests,
                    usage: totals.reported(),
                })
            }
            Command::ProviderCredentialIssue(body) => {
                CommandResult::ProviderCredentialIssue(self.issue(body).await?)
            }
            Command::ProviderCredentialAccept(body) => {
                self.accept(body).await?;
                CommandResult::ProviderCredentialAccept(Empty {})
            }
            Command::ProviderRefresh(body) => {
                let provider = self.providers.get(body.provider_id).ok_or_else(|| {
                    CommandError::new(ErrorCode::NotFound, "that provider is not started here")
                })?;
                let mut result = self
                    .source_context(call.context.id, Arc::clone(&provider))
                    .refresh(body.facts.as_ref())
                    .await;
                // An authenticated answer is proof; an unsupported or failed source is not.
                if result
                    .facts
                    .as_ref()
                    .is_some_and(|facts| facts.credential_verified == Some(true))
                {
                    provider.set_refused(false);
                }
                if result.status != "not_told" && provider.refused.load(Ordering::SeqCst) {
                    result.status = "error".to_owned();
                }
                CommandResult::ProviderRefresh(result)
            }
            Command::ProviderResetConsume(body) => {
                let provider = self.providers.get(body.provider_id).ok_or_else(|| {
                    CommandError::new(ErrorCode::NotFound, "that provider is not started here")
                })?;
                let result = self
                    .source_context(call.context.id, provider)
                    .reset(
                        body.facts.as_ref(),
                        crate::sources::dispatch::reset_request(
                            body.provider_id,
                            body.request_id,
                            body.reset_id,
                        ),
                    )
                    .await
                    .unwrap_or_else(crate::sources::dispatch::failed_reset);
                CommandResult::ProviderResetConsume(result)
            }
            Command::ProviderDiscoverModels(body) => {
                let provider = self.providers.get(body.provider_id).ok_or_else(|| {
                    CommandError::new(ErrorCode::NotFound, "that provider is not started here")
                })?;
                CommandResult::ProviderDiscoverModels(
                    self.source_context(call.context.id, provider)
                        .discover(body.models.as_ref())
                        .await,
                )
            }
            Command::ProviderComplete(body) => {
                let provider = self.providers.get(body.provider_id).ok_or_else(|| {
                    CommandError::new(ErrorCode::NotFound, "that API provider is not started here")
                })?;
                CommandResult::ProviderComplete(self.forwarder.complete(&provider, &body).await?)
            }
            Command::ProviderEmbed(body) => {
                let provider = self.providers.get(body.provider_id).ok_or_else(|| {
                    CommandError::new(ErrorCode::NotFound, "that API provider is not started here")
                })?;
                CommandResult::ProviderEmbed(self.forwarder.embed(&provider, &body).await?)
            }
            other => {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    format!("the provider role does not take {}", other.kind()),
                ));
            }
        })
    }

    async fn connected(&self, _session: Id) {
        let _discovery = self.discovery.lock().await;
        self.announced_sign_ins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        if let Some(hosting) = self.services.agents.get() {
            for found in crate::facts::local_sign_ins(hosting).await {
                let key = (found.harness_kind.clone(), found.identity.clone());
                if self
                    .announced_sign_ins
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains(&key)
                {
                    continue;
                }
                match self
                    .services
                    .publisher
                    .publish(Stream::Machine, Event::ProviderFound(found))
                    .await
                {
                    Ok(()) => {
                        self.announced_sign_ins
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(key);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a locally found sign-in could not be reported")
                    }
                }
            }
        }
    }

    async fn recover(&self) {
        let names = match self.store().names() {
            Ok(names) => names,
            Err(error) => {
                tracing::warn!(%error, "the providers kept on this machine could not be listed");
                return;
            }
        };
        for id in provider_ids(&names) {
            let loaded = self
                .store()
                .get(&provider_record_name(id))
                .map_err(|error| error.to_string())
                .and_then(|record| record.ok_or_else(|| "it is gone".to_owned()))
                .and_then(|record| {
                    serde_json::from_str::<Stored>(&record).map_err(|error| error.to_string())
                })
                .and_then(|stored| {
                    stored
                        .into_entry(id, self.forwarder.protocols(), &self.services)
                        .map_err(|error| error.detail)
                });
            match loaded {
                Ok(entry) => self.providers.insert(entry),
                Err(error) => {
                    tracing::warn!(provider = %id, %error, "a provider kept on this machine could not be started again");
                }
            }
        }
    }
}

impl ProviderRole {
    fn open_lease(
        &self,
        body: &orch_link_contract::commands::LeaseOpen,
    ) -> Result<(), CommandError> {
        let holds = match body.mode {
            LeaseMode::Forwarded => self.providers.contains(body.provider_id),
            LeaseMode::Shared | LeaseMode::Handover => {
                self.providers.contains(body.provider_id)
                    || !self.processes_with_login(body.provider_id).is_empty()
            }
        };
        if !holds {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                "this machine holds neither the key nor the sign-in of that provider; provider.start comes first",
            ));
        }
        if let Some(protocol) = &body.protocol
            && self.forwarder.protocols().get(protocol).is_none()
        {
            return Err(unsupported(format!(
                "this machine has no implementation of the protocol {protocol:?}"
            )));
        }
        if body.mode == LeaseMode::Forwarded
            && let Some(provider) = self.providers.get(body.provider_id)
            && let Some(protocol) = &body.protocol
            && matches!(
                self.forwarder.protocols().route(
                    protocol,
                    &provider
                        .endpoints()
                        .iter()
                        .map(EndpointEntry::protocol)
                        .collect::<Vec<_>>()
                ),
                orch_model_contract::translator::Route::Unserved
            )
        {
            return Err(unsupported(format!(
                "no endpoint of the provider speaks {protocol:?}"
            )));
        }
        self.leases
            .open(body, self.services.clock.time())
            .map(|_| ())
            .map_err(|error| match error {
                crate::leases::LeaseError::Closed => {
                    CommandError::new(ErrorCode::LeaseClosed, error.to_string())
                }
                other => internal(other.to_string()),
            })
    }
}
