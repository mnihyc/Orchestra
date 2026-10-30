//! Live agent/provider registrations over the existing plugin supervisor and the connector's one config file.

use async_trait::async_trait;
use orch_agent_contract::adapter::AgentAdapter;
use orch_api_contract::machine_programs::{
    MachineProgram, MachineProgramKey, MachineProgramKind as Kind, MachineProgramOrigin as Origin,
    MachineProgramRegistration, MachinePrograms, MachineProgramsCommand,
};
use orch_connector_core::{
    harnesses::{AdapterRegistration, AdapterRegistry, HomeRule},
    policy::PolicyStore,
    roles::Services,
};
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_plugin::{
    bridges::{agent::AdapterFactory, provider::ProviderBridge},
    registration::{BuiltIn, Plugins, hosted_by, registration_of},
    supervisor::{Options, Plugin},
};
use orch_plugin_contract::{PluginKind, PluginRegistration, PluginState, registration::PluginHost};
use orch_types::Json;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

#[derive(Clone)]
struct Entry {
    registration: PluginRegistration,
    origin: Origin,
    plugin: Plugin,
    accepted: Option<Json>,
    models: Option<Json>,
    tested: Option<Json>,
    detail: Option<String>,
    needs_reload: bool,
}
type Key = (Kind, String);
type Watcher = (
    Plugin,
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
);

/// The initial local registrations and later screen registrations share these same supervised processes.
pub struct Programs {
    entries: Mutex<BTreeMap<Key, Entry>>,
    changes: tokio::sync::Mutex<()>,
    registry: AdapterRegistry,
    config_file: PathBuf,
    template: Vec<PluginRegistration>,
    others: Plugins,
    builtin: BuiltIn,
    options: Options,
    watched: Mutex<BTreeMap<Key, Watcher>>,
    changed: tokio::sync::watch::Sender<u64>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}
fn error(value: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorCode::Unsupported, value.to_string())
}
fn plugin_kind(kind: Kind) -> PluginKind {
    match kind {
        Kind::Agent => PluginKind::Adapter,
        Kind::ProviderSource => PluginKind::Provider,
    }
}
fn public_kind(kind: PluginKind) -> Option<Kind> {
    match kind {
        PluginKind::Adapter => Some(Kind::Agent),
        PluginKind::Provider => Some(Kind::ProviderSource),
        _ => None,
    }
}
fn key(kind: Kind, name: &str) -> Key {
    (kind, name.to_owned())
}
fn current_declaration(entry: &Entry) -> bool {
    entry
        .plugin
        .negotiated()
        .is_some_and(|current| entry.accepted.as_ref() == Some(&current.descriptor))
}
fn source_of(entry: &Entry) -> Option<Arc<dyn orch_connector_core::sources::Program>> {
    let bridge = Arc::new(ProviderBridge::new(entry.plugin.clone()).ok()?);
    let mut checked = orch_connector_core::sources::Programs::default();
    checked.register(&entry.registration.name, bridge).ok()?;
    checked.get(&entry.registration.name)
}

/// The same best-effort, credential-free preview for registration, Test and configured startup.
/// A source that needs a real provider/fields may refuse this sparse request; it stays usable.
async fn preview_models(plugin: &Plugin) -> (Option<Json>, Option<String>) {
    if !plugin.supports(orch_provider_contract::wire::MODELS_LIST)
        || plugin.negotiated().is_none_or(|declared| {
            !declared
                .descriptor
                .get("models")
                .is_some_and(Json::is_object)
        })
    {
        return (None, None);
    }
    let read = async {
        let answer = plugin
            .start_call_within(
                orch_provider_contract::wire::MODELS_LIST,
                json!({"endpoints":[],"config":{},"secrets":{}}),
                orch_plugin_contract::lifecycle::PROVIDER_DESCRIBE_DEADLINE,
            )
            .map_err(error)?
            .await
            .map_err(error)?;
        if serde_json::to_vec(&answer).map_err(error)?.len()
            > orch_provider_contract::limits::REPORT_MAX_BYTES
        {
            return Err(error(
                "The models answer exceeds the provider contract's one MiB limit.",
            ));
        }
        if !answer
            .get("models")
            .is_some_and(|models| models.is_null() || models.is_array())
        {
            return Err(error("The source's models answer has no model list."));
        }
        let report: orch_provider_contract::source::ModelsReport =
            serde_json::from_value(answer).map_err(error)?;
        let raw = serde_json::to_value(report.models).map_err(error)?;
        let clean = orch_connector_core::sources::reports::models(&raw, &[]).map_err(error)?;
        let detail = (!clean.notes.is_empty()).then(|| {
            format!(
                "Some model suggestions were omitted: {}",
                clean.notes.join(", ")
            )
        });
        Ok::<_, CommandError>((json!({"models":clean.models}), detail))
    }
    .await;
    match read {
        Ok((models, detail)) => (Some(models), detail),
        Err(failure) => (
            None,
            Some(format!(
                "Describe succeeded. Models need provider configuration or a working source: {}",
                failure.detail
            )),
        ),
    }
}

fn configured(
    config: &orch_config::ConnectorConfig,
    template: &[PluginRegistration],
) -> Result<Vec<(PluginRegistration, Origin)>, CommandError> {
    let mut result = Vec::new();
    for entry in &config.plugin {
        let registration = registration_of(
            &entry.kind,
            &entry.name,
            &entry.command,
            Some(&entry.connect),
            &entry.env,
            &entry.restart,
        )
        .map_err(error)?;
        if public_kind(registration.kind).is_some() {
            result.push((
                registration,
                if entry.registered_by_orch {
                    Origin::Orch
                } else {
                    Origin::Local
                },
            ));
        }
    }
    result.extend(
        template
            .iter()
            .filter(|entry| public_kind(entry.kind).is_some())
            .cloned()
            .map(|entry| (entry, Origin::Local)),
    );
    Ok(result)
}

impl Programs {
    /// One scan for this process's configured sources, outside the startup/link and page-read paths.
    /// Screen registrations already have their preview; no idle timer or follow-up GET is needed.
    pub async fn preview_configured(&self) {
        let sources = lock(&self.entries)
            .iter()
            .filter(|((kind, _), entry)| {
                *kind == Kind::ProviderSource && entry.models.is_none() && entry.detail.is_none()
            })
            .map(|(key, entry)| (key.clone(), entry.plugin.clone()))
            .collect::<Vec<_>>();
        for (lookup, plugin) in sources {
            let _change = self.changes.lock().await;
            if !lock(&self.entries).get(&lookup).is_some_and(|entry| {
                entry.plugin.same_instance(&plugin)
                    && entry.models.is_none()
                    && entry.detail.is_none()
            }) {
                continue;
            }
            let (models, detail) = preview_models(&plugin).await;
            if models.is_none() && detail.is_none() {
                continue;
            }
            if let Some(entry) = lock(&self.entries).get_mut(&lookup) {
                entry.models = models;
                entry.detail = detail;
            }
            self.changed.send_modify(|value| *value += 1);
        }
    }

    /// Adopt the already-started configured plugins; no second process or alternate bridge is created.
    pub fn new(
        config: &orch_config::ConnectorConfig,
        template: Vec<PluginRegistration>,
        plugins: Plugins,
        registry: AdapterRegistry,
        config_file: PathBuf,
        options: Options,
        builtin: BuiltIn,
    ) -> Result<Self, CommandError> {
        let mut entries = BTreeMap::new();
        for (registration, origin) in configured(config, &template)? {
            let Some(kind) = public_kind(registration.kind) else {
                continue;
            };
            let plugin = plugins
                .get(registration.kind, &registration.name)
                .cloned()
                .ok_or_else(|| error("A configured program has no supervisor."))?;
            let accepted = plugin
                .negotiated()
                .map(|current| current.descriptor.clone());
            entries.insert(
                key(kind, &registration.name),
                Entry {
                    registration,
                    origin,
                    plugin,
                    accepted,
                    models: None,
                    tested: None,
                    detail: None,
                    needs_reload: false,
                },
            );
        }
        let this = Self {
            entries: Mutex::new(entries),
            changes: tokio::sync::Mutex::new(()),
            registry,
            config_file,
            template,
            others: plugins,
            builtin,
            options,
            watched: Mutex::new(BTreeMap::new()),
            changed: tokio::sync::watch::channel(0).0,
        };
        let current = lock(&this.entries).clone();
        this.registry
            .replace_external(Self::bindings(&current)?)
            .map_err(error)?;
        this.watch(&current);
        Ok(this)
    }

    fn bindings(entries: &BTreeMap<Key, Entry>) -> Result<Vec<AdapterRegistration>, CommandError> {
        let mut registrations = Vec::new();
        for ((kind, _), entry) in entries {
            if *kind != Kind::Agent || entry.needs_reload || !current_declaration(entry) {
                continue;
            }
            let factory = AdapterFactory::new(entry.plugin.clone()).map_err(error)?;
            let sample = factory.adapter();
            let descriptor = sample.declared();
            registrations.push(AdapterRegistration {
                kind: sample.kind(),
                adapter_version: descriptor.adapter_version,
                builtin: false,
                needs_binary: false,
                home_rule: HomeRule::Connector,
                factory: Arc::new(move |_| Box::new(factory.adapter())),
            });
        }
        Ok(registrations)
    }

    fn validate(&self, next: &BTreeMap<Key, Entry>) -> Result<(), CommandError> {
        let all: Vec<_> = next
            .values()
            .map(|entry| entry.registration.clone())
            .collect();
        hosted_by(PluginHost::Connector, &all, &self.builtin).map_err(error)?;
        self.registry
            .check_external(&Self::bindings(next)?)
            .map_err(error)
    }

    fn activate(&self, next: BTreeMap<Key, Entry>) -> Result<(), CommandError> {
        self.registry
            .replace_external(Self::bindings(&next)?)
            .map_err(error)?;
        self.watch(&next);
        *lock(&self.entries) = next;
        Ok(())
    }

    fn watch(&self, entries: &BTreeMap<Key, Entry>) {
        let mut watched = lock(&self.watched);
        watched.retain(|key, (plugin, stop, task)| {
            if entries
                .get(key)
                .is_some_and(|entry| entry.plugin.same_instance(plugin))
            {
                true
            } else {
                let _ = stop.send(true);
                task.abort();
                false
            }
        });
        for (key, entry) in entries {
            if watched.contains_key(key) {
                continue;
            }
            let mut reports = entry.plugin.watch();
            let (stop, mut cancelled) = tokio::sync::watch::channel(false);
            let changed = self.changed.clone();
            let task = tokio::spawn(async move {
                let mut last = {
                    let report = reports.borrow();
                    (report.state, report.version.clone(), report.detail.clone())
                };
                loop {
                    tokio::select! {
                        _=cancelled.changed()=>break,
                        result=reports.changed()=>{
                            if result.is_err(){break;}
                            let next={let report=reports.borrow();(report.state,report.version.clone(),report.detail.clone())};
                            if next!=last {last=next;changed.send_modify(|value|*value+=1);}
                        },
                    }
                }
            });
            watched.insert(key.clone(), (entry.plugin.clone(), stop, task));
        }
    }

    async fn start(
        &self,
        registration: PluginRegistration,
        origin: Origin,
    ) -> Result<Entry, CommandError> {
        hosted_by(
            PluginHost::Connector,
            std::slice::from_ref(&registration),
            &self.builtin,
        )
        .map_err(error)?;
        let plugin = Plugin::start(&registration, self.options.clone())
            .await
            .map_err(error)?;
        let outcome=tokio::time::timeout(orch_plugin_contract::lifecycle::PROVIDER_DESCRIBE_DEADLINE,async {
            // Interactive metadata must fit the browser request, including the contract's shutdown
            // grace on failure. Use the provider contract's existing ten-second describe budget.
            plugin.settled().await;
            let report=plugin.report();
            if report.state!=PluginState::Running {return Err(error(report.detail.unwrap_or_else(||"The program could not start; check its command and contract.".into())));}
            let described=plugin.start_call_within("describe",json!({}),orch_plugin_contract::lifecycle::PROVIDER_DESCRIBE_DEADLINE)
                .map_err(error)?.await.map_err(error)?;
            if plugin.negotiated().is_none_or(|initial|initial.descriptor!=described) {
                return Err(error("Describe disagrees with the program's initial declaration; correct the program and try again."));
            }
            match registration.kind {
                PluginKind::Adapter=>{let _=AdapterFactory::new(plugin.clone()).map_err(error)?;},
                PluginKind::Provider=>{
                    let bridge=Arc::new(ProviderBridge::new(plugin.clone()).map_err(error)?);
                    let mut checked=orch_connector_core::sources::Programs::default();
                    checked.register(&registration.name,bridge).map_err(error)?;
                },_=>return Err(error("Choose an agent adapter or a provider source.")),
            }
            Ok(described)
        }).await.unwrap_or_else(|_|Err(error("The program did not describe itself in time; check its command and contract.")));
        let accepted = match outcome {
            Ok(described) => Some(described),
            Err(failure) => {
                plugin.shutdown().await;
                return Err(failure);
            }
        };
        let (models, detail) = if registration.kind == PluginKind::Provider {
            preview_models(&plugin).await
        } else {
            (None, None)
        };
        Ok(Entry {
            registration,
            origin,
            plugin,
            accepted,
            models,
            tested: None,
            detail,
            needs_reload: false,
        })
    }

    /// Provider dispatch resolves the current registry on every call; removed programs cannot survive in a second map.
    pub fn source(&self, name: &str) -> Option<Arc<dyn orch_connector_core::sources::Program>> {
        let entry = lock(&self.entries)
            .get(&key(Kind::ProviderSource, name))?
            .clone();
        if entry.needs_reload || !current_declaration(&entry) {
            return None;
        }
        source_of(&entry)
    }

    /// The same supervisors feed the existing machine plugin facts and the Programs projection.
    pub fn reports(&self) -> Vec<orch_link_contract::session::MachineFactsPlugin> {
        let mut plugins = self
            .others
            .all()
            .iter()
            .filter(|plugin| public_kind(plugin.kind()).is_none())
            .cloned()
            .collect::<Vec<_>>();
        plugins.extend(
            lock(&self.entries)
                .values()
                .map(|entry| entry.plugin.clone()),
        );
        plugins
            .into_iter()
            .map(|plugin| {
                let report = plugin.report();
                orch_link_contract::session::MachineFactsPlugin {
                    kind: report.kind.as_str().into(),
                    name: report.name,
                    version: report.version,
                    state: match report.state {
                        PluginState::Running => "running",
                        PluginState::Restarting => "restarting",
                        PluginState::Failed => "failed",
                    }
                    .into(),
                    descriptor: if plugin.kind() == PluginKind::Provider {
                        plugin.negotiated().and_then(|description| {
                            serde_json::from_value(description.descriptor.clone()).ok()
                        })
                    } else {
                        None
                    },
                    executor: if plugin.kind() == PluginKind::Executor {
                        plugin.negotiated().and_then(|description| {
                            serde_json::from_value(description.descriptor.clone()).ok()
                        })
                    } else {
                        None
                    },
                }
            })
            .collect()
    }

    async fn retire(services: &Services, entry: &Entry) -> Result<(), CommandError> {
        if entry.registration.kind == PluginKind::Adapter
            && let Some(hosting) = services.agents.get()
            && let Some(kind) = entry.plugin.negotiated().and_then(|declared| {
                declared
                    .descriptor
                    .get("kind")
                    .and_then(Json::as_str)
                    .map(str::to_owned)
            })
        {
            hosting.stop_kind(&kind).await?;
        }
        entry.plugin.shutdown().await;
        Ok(())
    }

    /// App shutdown also stops programs registered after enrollment/startup.
    pub async fn shutdown(&self) {
        let watched = std::mem::take(&mut *lock(&self.watched));
        for (_, (_, stop, task)) in watched {
            let _ = stop.send(true);
            task.abort();
        }
        let entries = lock(&self.entries).values().cloned().collect::<Vec<_>>();
        for entry in entries {
            entry.plugin.shutdown().await;
        }
    }

    async fn test(&self, identity: MachineProgramKey) -> Result<(), CommandError> {
        let lookup = key(identity.kind, &identity.name);
        let entry = lock(&self.entries).get(&lookup).cloned().ok_or_else(|| {
            CommandError::new(ErrorCode::NotFound, "This program is not registered.")
        })?;
        let described = entry
            .plugin
            .start_call_within(
                "describe",
                json!({}),
                orch_plugin_contract::lifecycle::PROVIDER_DESCRIBE_DEADLINE,
            )
            .map_err(error)?
            .await
            .map_err(error)?;
        match identity.kind {
            Kind::Agent => {
                let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
                    serde_json::from_value(described.clone()).map_err(error)?;
                if entry.plugin.negotiated().is_none_or(|old| {
                    old.descriptor.get("kind").and_then(Json::as_str)
                        != Some(descriptor.kind.as_str())
                }) {
                    return Err(error(
                        "The adapter changed its harness kind; remove and register it again.",
                    ));
                }
                descriptor.check().map_err(|faults| {
                    error(format!("The adapter declaration is invalid: {faults:?}"))
                })?;
            }
            Kind::ProviderSource => {
                let descriptor: orch_provider_contract::source::SourceDescriptor =
                    serde_json::from_value(described.clone()).map_err(error)?;
                descriptor.check().map_err(error)?;
            }
        }
        let (models, mut detail) = if identity.kind == Kind::ProviderSource {
            preview_models(&entry.plugin).await
        } else {
            (None, None)
        };
        let needs_reload = entry
            .plugin
            .negotiated()
            .is_some_and(|negotiated| negotiated.descriptor != described);
        if needs_reload {
            detail = Some(
                "The program changed its declaration. Reload to apply it before using it.".into(),
            );
        }
        let mut next = lock(&self.entries).clone();
        if let Some(entry) = next.get_mut(&lookup) {
            entry.tested = Some(described);
            entry.models = models;
            entry.detail = detail;
            entry.needs_reload = needs_reload;
        }
        self.validate(&next)?;
        self.activate(next)
    }
}

#[async_trait]
impl orch_connector_core::programs::Handler for Programs {
    fn changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        Some(self.changed.subscribe())
    }
    fn snapshot(&self, policy: &PolicyStore) -> MachinePrograms {
        let entries = lock(&self.entries)
            .iter()
            .map(|((kind, _), entry)| {
                let report = entry.plugin.report();
                let accepted = entry
                    .plugin
                    .negotiated()
                    .map(|value| value.descriptor.clone());
                let descriptor = if entry.needs_reload {
                    entry.tested.clone().or(accepted)
                } else {
                    accepted
                }
                .unwrap_or(Json::Null);
                let activated = if *kind == Kind::ProviderSource {
                    source_of(entry).is_some()
                } else {
                    descriptor
                        .get("kind")
                        .and_then(Json::as_str)
                        .is_some_and(|kind| {
                            self.registry
                                .get(kind)
                                .is_some_and(|registration| !registration.builtin)
                        })
                };
                let current = current_declaration(entry);
                let ready = report.state == PluginState::Running
                    && !entry.needs_reload
                    && current
                    && activated;
                let detail = entry.detail.clone().or(report.detail).or_else(|| {
                    (report.state == PluginState::Running && (!activated || !current)).then(|| {
                        "Reload to validate and activate this program's reported declaration."
                            .into()
                    })
                });
                MachineProgram {
                    registration: MachineProgramRegistration {
                        kind: *kind,
                        name: entry.registration.name.clone(),
                        command: entry.registration.command.clone().unwrap_or_default(),
                    },
                    origin: entry.origin,
                    ready,
                    version: (!report.version.is_empty()).then_some(report.version),
                    descriptor,
                    models: entry.models.clone(),
                    detail,
                }
            })
            .collect();
        MachinePrograms {
            machine_view: None,
            entries,
            capabilities: [Kind::Agent, Kind::ProviderSource]
                .map(|kind| orch_connector_core::programs::capability(policy, kind))
                .to_vec(),
            config_file: Some(self.config_file.display().to_string()),
            can_manage: false,
        }
    }

    async fn execute(
        &self,
        request: MachineProgramsCommand,
        services: &Services,
    ) -> Result<MachinePrograms, CommandError> {
        let _change = self.changes.lock().await;
        let _ = services.policy.reload_from_file();
        match request {
            MachineProgramsCommand::Register(body) => {
                orch_connector_core::programs::require_registration(&services.policy, body.kind)?;
                let before = lock(&self.entries).clone();
                let lookup = key(body.kind, &body.name);
                if before.contains_key(&lookup) {
                    return Err(error(
                        "A program with this name is already registered; Test it or Remove it first.",
                    ));
                }
                let registration = registration_of(
                    plugin_kind(body.kind).as_str(),
                    &body.name,
                    &body.command,
                    None,
                    &BTreeMap::new(),
                    "",
                )
                .map_err(error)?;
                let entry = self.start(registration, Origin::Orch).await?;
                let mut next = before;
                next.insert(lookup, entry.clone());
                let applied = (|| {
                    self.validate(&next)?;
                    services.policy.change_program(
                        body.kind,
                        &body.name,
                        Some(orch_config::connector::ConnectorPlugin {
                            kind: plugin_kind(body.kind).as_str().into(),
                            name: body.name.clone(),
                            command: body.command,
                            registered_by_orch: true,
                            ..Default::default()
                        }),
                    )?;
                    self.activate(next)
                })();
                if let Err(failure) = applied {
                    entry.plugin.shutdown().await;
                    return Err(failure);
                }
            }
            MachineProgramsCommand::Test(identity) => self.test(identity).await?,
            MachineProgramsCommand::Remove(identity) => {
                let before = lock(&self.entries).clone();
                let lookup = key(identity.kind, &identity.name);
                let entry = before.get(&lookup).cloned().ok_or_else(|| {
                    CommandError::new(ErrorCode::NotFound, "This program is no longer registered.")
                })?;
                if entry.origin != Origin::Orch {
                    return Err(error(
                        "The operator keeps this program in the local configuration; remove it there and Reload.",
                    ));
                }
                let mut next = before;
                next.remove(&lookup);
                self.validate(&next)?;
                services
                    .policy
                    .change_program(identity.kind, &identity.name, None)?;
                self.activate(next)?;
                Self::retire(services, &entry).await?;
            }
            MachineProgramsCommand::Reload => {
                services.policy.reload_from_file().map_err(error)?;
                let desired = configured(&services.policy.config(), &self.template)?;
                let registrations = desired
                    .iter()
                    .map(|(entry, _)| entry.clone())
                    .collect::<Vec<_>>();
                hosted_by(PluginHost::Connector, &registrations, &self.builtin).map_err(error)?;
                let before = lock(&self.entries).clone();
                let mut next = BTreeMap::new();
                let mut started: Vec<Plugin> = Vec::new();
                for (registration, origin) in desired {
                    let Some(kind) = public_kind(registration.kind) else {
                        continue;
                    };
                    let lookup = key(kind, &registration.name);
                    let entry = if let Some(existing) = before.get(&lookup).filter(|existing| {
                        existing.registration == registration
                            && existing.origin == origin
                            && !existing.needs_reload
                            && current_declaration(existing)
                            && existing.plugin.report().state == PluginState::Running
                    }) {
                        existing.clone()
                    } else {
                        match self.start(registration, origin).await {
                            Ok(entry) => {
                                started.push(entry.plugin.clone());
                                entry
                            }
                            Err(failure) => {
                                for plugin in started {
                                    plugin.shutdown().await;
                                }
                                return Err(failure);
                            }
                        }
                    };
                    next.insert(lookup, entry);
                }
                if let Err(failure) = self.validate(&next) {
                    for plugin in started {
                        plugin.shutdown().await;
                    }
                    return Err(failure);
                }
                let retired = before
                    .iter()
                    .filter(|(lookup, entry)| {
                        entry.needs_reload
                            || !current_declaration(entry)
                            || entry.plugin.report().state != PluginState::Running
                            || next.get(*lookup).is_none_or(|next| {
                                next.registration != entry.registration
                                    || next.origin != entry.origin
                            })
                    })
                    .map(|(_, entry)| entry.clone())
                    .collect::<Vec<_>>();
                self.activate(next)?;
                for entry in retired {
                    Self::retire(services, &entry).await?;
                }
            }
        }
        Ok(self.snapshot(&services.policy))
    }
}
