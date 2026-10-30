//! The connector as a whole: the link, the listener, the policy, the roles it plays and the commands they answer, started together and stopped together.
//!
//! [`ConnectorBuilder`] takes what a program has chosen (the identity enrollment left, the addresses
//! of the gateway, the roles, the adapters it can host, how to dial) and starts everything in the order
//! it depends on itself: the data directory and what it holds, the local policy, the link, the listener
//! for harnesses, the roles, the dispatcher that carries the system's commands out, and last what a
//! previous run left unfinished. [`Connector`] is what runs.
//!
//! A role other than the agent role is added with [`ConnectorBuilder::role`]; an endpoint of the
//! listener, with [`ConnectorBuilder::endpoint`]. Both are made from the [`Services`] the connector
//! has, which is why they are given as functions of them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_config::ConnectorConfig;
use orch_link::content::BlobEndpoint;
use orch_link::framing::LinkMeter;
use orch_link::machine::{MachineLink, StartError as LinkStartError};
use orch_link::reconnect::Dialer;
use orch_link_contract::enrollment::Endpoint;
use orch_link_contract::events::Event;
use orch_link_contract::session::{Platform, Role};
use orch_link_contract::streams::Stream;
use orch_pki::PkiError;
use orch_pki::certificates::ServiceName;
use orch_pki::enrollment::MachineIdentity;
use orch_pki::verification::{ServerTrust, machine_client_config};
use orch_types::Clock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::content::{Content, EndpointNow, HttpsBlobs};
use crate::dispatch::Dispatcher;
use crate::enrollment::{
    DataDir, EnrollError, EnrollTransport, RENEWAL_LOOK_EVERY, renew_if_due, renew_when_due_using,
};
use crate::facts::{FactsDraft, Host, machine_facts};
use crate::harnesses::{AdapterRegistry, Hosting};
use crate::https::{HttpsClient, https_base};
use crate::ledger::{Ledger, LedgerError};
use crate::loopback::{Endpoint as LoopbackEndpoint, EndpointKind, Loopback};
use crate::policy::PolicyStore;
use crate::roles::agent::AgentRole;
use crate::roles::environment::{self, EnvironmentParts};
use crate::roles::{AgentSlot, Keys, RoleHandler, Services};
use crate::session::{LinkSettings, machine_config, watch};
use crate::tunnels::Tunnels;

/// How often the machine's facts are looked at again to see whether they changed: ten minutes. The rates
/// and the round trip in them drift all the time, and the system averages them, so a report every ten
/// minutes keeps its figures fresh without filling the spool; a change of policy is reported at once.
pub const FACTS_EVERY: Duration = Duration::from_secs(600);

/// Why a connector could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// An uninstall owns this instance's data until its work is complete.
    #[error("{0}")]
    Instance(#[from] crate::uninstall::Error),
    /// The machine policy contains an unknown key or invalid explicit value.
    #[error("{0}")]
    Policy(#[from] crate::policy::PolicyFault),
    /// The idle connector installed a verified build; its program must restart on it.
    #[error("the connector installed its verified update and is ready to restart")]
    Updated,
    /// The data directory could not be made or read, or the certificate could not be renewed.
    #[error("{0}")]
    DataDir(#[from] EnrollError),
    /// The record of what is hosted is damaged.
    #[error("{0}")]
    Ledger(#[from] LedgerError),
    /// The link could not be started.
    #[error("{0}")]
    Link(#[from] LinkStartError),
    /// A key or a certificate could not be used.
    #[error("{0}")]
    Pki(#[from] PkiError),
    /// The listener for harnesses could not be bound.
    #[error("the listener for harnesses could not be bound: {0}")]
    Listener(std::io::Error),
    /// The sealed store of the provider role could not be used.
    #[error("{0}")]
    Store(#[from] crate::credentials::StoreError),
    /// Two roles take one kind of command.
    #[error("{0}")]
    Roles(#[from] crate::dispatch::DuplicateKind),
}

/// What a program has chosen for the connector.
pub struct ConnectorParams {
    /// Whether authenticated server defaults may refresh this machine's list.
    pub follow_system_endpoints: bool,
    /// A node's explicit preference list for hosted units.
    pub hosted_endpoints: Option<Vec<Endpoint>>,
    /// The connector's version, as the hello says it.
    pub version: String,
    /// Its data directory.
    pub data_dir: PathBuf,
    /// Its configuration.
    pub config: ConnectorConfig,
    /// Who it is.
    pub identity: MachineIdentity,
    /// Where the gateway is, in the order to try.
    pub endpoints: Vec<Endpoint>,
    /// The roles it plays.
    pub roles: Vec<Role>,
    /// Whether the system made the machine: it then has no local policy of its own.
    pub managed: bool,
    /// How to reach the gateway.
    pub dialer: Arc<dyn Dialer>,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// What it is built for.
    pub platform: Platform,
    /// The adapters it can host.
    pub adapters: AdapterRegistry,
    /// The read-only directory of tools a managed runtime is given.
    pub tools_dir: Option<PathBuf>,
    /// How content is moved, where it is not by HTTPS to the gateway's endpoint.
    pub blobs: Option<Arc<dyn BlobEndpoint>>,
}

/// Makes a role from the services of the connector.
pub type RoleFactory = Box<dyn FnOnce(&Arc<Services>) -> Arc<dyn RoleHandler> + Send>;
/// Makes an endpoint of the listener from the services of the connector.
pub type EndpointFactory =
    Box<dyn FnOnce(&Arc<Services>) -> (EndpointKind, Arc<dyn LoopbackEndpoint>) + Send>;

/// Reads the current reports of programs hosted by the connector's composition.
pub type PluginFacts =
    dyn Fn() -> Vec<orch_link_contract::session::MachineFactsPlugin> + Send + Sync;

/// Starts a connector.
pub struct ConnectorBuilder {
    supervised_roles: bool,
    verified_updates: bool,
    params: ConnectorParams,
    roles: Vec<RoleFactory>,
    endpoints: Vec<EndpointFactory>,
    environment: Option<EnvironmentParts>,
    plugin_facts: Option<Arc<PluginFacts>>,
    programs: Option<Arc<dyn crate::programs::Handler>>,
    installation: Option<orch_api_contract::machine_programs::MachineInstallation>,
    provider_sources: Arc<crate::sources::Programs>,
    builtin_sources: Arc<orch_provider_contract::builtins::BuiltinSources>,
}

impl ConnectorBuilder {
    /// A builder for a connector with these parameters. The agent role is part of it whenever the roles
    /// include `agent`.
    #[must_use]
    pub fn new(params: ConnectorParams) -> Self {
        Self {
            verified_updates: false,
            supervised_roles: false,
            params,
            roles: Vec::new(),
            endpoints: Vec::new(),
            environment: None,
            plugin_facts: None,
            programs: None,
            installation: None,
            provider_sources: Arc::new(crate::sources::Programs::default()),
            builtin_sources: Arc::default(),
        }
    }

    /// Gives the environment role what the program chose for it: the executors an environment can run and
    /// where a runtime the system made has its places. The role is part of the connector whenever the roles
    /// include `environment`, with nothing chosen if this is not called.
    #[must_use]
    pub fn environment(mut self, parts: EnvironmentParts) -> Self {
        self.environment = Some(parts);
        self
    }

    /// Reports the current states and declarations of locally hosted programs on every facts read.
    #[must_use]
    pub fn plugin_facts(mut self, facts: Arc<PluginFacts>) -> Self {
        self.plugin_facts = Some(facts);
        self
    }

    /// Supplies locally registered provider programs to both credential-keeping roles and the forwarder.
    #[must_use]
    pub fn provider_sources(mut self, programs: Arc<crate::sources::Programs>) -> Self {
        self.provider_sources = programs;
        self
    }

    /// Supplies the built-in sources of a key's facts the program carries: the vendors' readers, which the
    /// connector knows by this table alone. Without it, no vendor is read by default.
    #[must_use]
    pub fn builtin_sources(
        mut self,
        table: Arc<orch_provider_contract::builtins::BuiltinSources>,
    ) -> Self {
        self.builtin_sources = table;
        self
    }

    /// Adds a role.
    #[must_use]
    pub fn role(mut self, role: RoleFactory) -> Self {
        self.roles.push(role);
        self
    }

    /// Declares the composed physical program's updater before commands can arrive during recovery.
    #[must_use]
    pub fn verified_updates(mut self, enabled: bool) -> Self {
        self.verified_updates = enabled;
        self
    }

    /// Runtime program registration belongs to this composition's existing plugin supervisor.
    #[must_use]
    pub fn programs(mut self, handler: Arc<dyn crate::programs::Handler>) -> Self {
        self.programs = Some(handler);
        self
    }

    /// Observed local installation; no placement inference is used by the server or page.
    #[must_use]
    pub fn installation(
        mut self,
        installation: orch_api_contract::machine_programs::MachineInstallation,
    ) -> Self {
        self.installation = Some(installation);
        self
    }

    /// Adds an endpoint to the listener for harnesses.
    #[must_use]
    pub fn endpoint(mut self, endpoint: EndpointFactory) -> Self {
        self.endpoints.push(endpoint);
        self
    }

    /// Starts everything.
    pub async fn start(self) -> Result<Connector, StartError> {
        Connector::start(self).await
    }
}

/// A connector that runs.
pub struct Connector {
    identity_reload: Arc<tokio::sync::Notify>,
    role_changes: Arc<crate::role_changes::Requests>,
    pub(crate) update_requests: Arc<crate::automatic::Requests>,
    environment_role: Option<Arc<crate::roles::environment::EnvironmentRole>>,
    link: MachineLink,
    services: Arc<Services>,
    roles: Vec<Arc<dyn RoleHandler>>,
    hosting: Option<Arc<Hosting>>,
    loopback: Arc<Loopback>,
    stop: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    turn_admission: Arc<tokio::sync::RwLock<()>>,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector").finish_non_exhaustive()
    }
}

fn blob_clients(
    identity: &MachineIdentity,
    clock: &Arc<dyn Clock>,
) -> Result<(HttpsClient, HttpsClient), PkiError> {
    let tls = identity.tls()?;
    let pinned = machine_client_config(
        Some(ServerTrust::Authority(Box::new(identity.authority.clone()))),
        ServiceName::Gateway,
        Some(&tls),
        Arc::clone(clock),
    )?;
    let public = machine_client_config(
        Some(ServerTrust::Authority(Box::new(identity.authority.clone()))),
        ServiceName::Frontend,
        None,
        Arc::clone(clock),
    )?;
    Ok((HttpsClient::new(pinned), HttpsClient::new(public)))
}

impl Connector {
    #[allow(
        clippy::too_many_lines,
        reason = "the order things are started in is the point; one function shows it whole"
    )]
    async fn start(builder: ConnectorBuilder) -> Result<Self, StartError> {
        let ConnectorBuilder {
            supervised_roles,
            verified_updates,
            params,
            roles: extra_roles,
            endpoints: extra_endpoints,
            environment: environment_parts,
            plugin_facts,
            programs,
            installation,
            provider_sources,
            builtin_sources,
        } = builder;
        let data = DataDir::open(&params.data_dir)?;
        // Nothing a command left in the cache outlives a run: the commands themselves do not.
        let scratch = data.root().join("cache").join("commands");
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        let ledger = Arc::new(Ledger::open(data.root())?);
        // Every physical machine owns one policy; a child unit inherits its host's ceiling.
        let policy = Arc::new(
            if params.managed
                && !params
                    .roles
                    .contains(&orch_link_contract::session::Role::Node)
            {
                PolicyStore::managed(params.config.clone())
            } else if params.roles.contains(&Role::Node) {
                PolicyStore::host(params.config.clone(), Some(data.config_path()))?
            } else {
                PolicyStore::connected(params.config.clone(), Some(data.config_path()))?
            },
        );
        policy.offered_roles(&params.roles);
        let link = MachineLink::start(
            machine_config(LinkSettings {
                version: &params.version,
                platform: params.platform,
                roles: params.roles.clone(),
                identity: &params.identity,
                endpoints: params.endpoints.clone(),
                data_dir: data.root(),
                config: &params.config,
                follow_system_endpoints: params.follow_system_endpoints,
                hosted_endpoints: params.hosted_endpoints.clone(),
                connected: !params.managed,
            })?,
            Arc::clone(&params.dialer),
            Arc::clone(&params.clock),
        )?;
        let loopback = Arc::new(Loopback::start().await.map_err(StartError::Listener)?);
        let blobs: Arc<dyn BlobEndpoint> = match params.blobs.clone() {
            Some(blobs) => blobs,
            None => {
                let (pinned, public) = blob_clients(&params.identity, &params.clock)?;
                let endpoint_link = link.clone();
                Arc::new(HttpsBlobs::new(
                    pinned,
                    public,
                    Arc::new(move || {
                        let endpoint = endpoint_link.connected_endpoint()?;
                        let trust = endpoint.trust;
                        https_base(&endpoint.url).ok().map(|base| EndpointNow {
                            base,
                            trust,
                            session_token: endpoint_link.session_token(),
                        })
                    }),
                ))
            }
        };
        let tunnels = Tunnels::new(link.clone());
        let process_controls = if policy.policy().is_some()
            && params
                .roles
                .iter()
                .any(|role| matches!(role, Role::Environment | Role::Agent))
        {
            orch_envhost::process_limits::UserGroup::probe().await
        } else {
            Default::default()
        };
        let port_publisher = Arc::new(crate::host_publish::Publisher::with_clock(
            Arc::clone(&tunnels),
            Arc::clone(&params.clock),
        ));
        tunnels.serve_port_records(port_publisher.records());
        let agents = Arc::new(AgentSlot::default());
        // What the system put in the tools directory of a runtime it made; a machine a person connected
        // has none, and keeps its harnesses where its owner listed them.
        let installed = match params.tools_dir.as_deref() {
            Some(directory) if params.managed => {
                crate::builds::installed(directory, &params.adapters)
            }
            _ => Vec::new(),
        };
        let services = Arc::new(Services {
            link: link.clone(),
            process_controls,
            clock: Arc::clone(&params.clock),
            data_dir: data.root().to_owned(),
            publisher: Arc::new(link.clone()),
            policy: Arc::clone(&policy),
            provider_sources,
            builtin_sources,
            source_runtime: Arc::new(crate::sources::dispatch::Runtime::default()),
            loopback: Arc::clone(&loopback),
            content: Arc::new(Content::new(blobs, LinkMeter::new(), scratch)),
            keys: Arc::new(Keys::new(params.identity.key.duplicate()?)),
            config: params.config.clone(),
            tools_dir: params.tools_dir.clone(),
            installed,
            tunnels: Arc::clone(&tunnels),
            port_publisher,
            agents: Arc::clone(&agents),
        });

        let mut roles: Vec<Arc<dyn RoleHandler>> = Vec::new();
        if let Some(role) = params
            .roles
            .iter()
            .find(|role| matches!(role, Role::Node | Role::Environment))
            && services.policy.policy().is_some()
        {
            roles.push(Arc::new(crate::roles::ports::PortsRole::new(
                Arc::clone(&services),
                *role,
            )));
        }
        let mut hosting = None;
        if params.roles.contains(&Role::Agent) {
            let hosted = Hosting::new(
                Arc::clone(&services),
                Arc::new(params.adapters.clone()),
                ledger,
            );
            agents.fill(Arc::clone(&hosted));
            roles.push(Arc::new(AgentRole::new(Arc::clone(&hosted))));
            // Orch's own tools are presented by the connector itself, to the harnesses it hosts.
            loopback.register(
                EndpointKind::Mcp(orch_agent_contract::loopback::ORCH_SERVER.to_owned()),
                Arc::new(crate::orch_tools::OrchEndpoint::new(Arc::clone(&hosted))),
            );
            hosting = Some(hosted);
        }
        // The environment role, when the machine plays it, and the addresses a harness here reaches its
        // tools at, when it hosts harnesses.
        let tools = environment::join(
            &services,
            &link,
            hosting.clone(),
            params
                .roles
                .contains(&Role::Environment)
                .then(|| environment_parts.unwrap_or_default())
                .as_ref(),
            params.roles.contains(&Role::Agent),
        );
        let environment_role = tools.role;
        roles.extend(environment_role.iter().map(|role| {
            let role: Arc<dyn RoleHandler> = role.clone();
            role
        }));
        for (kind, endpoint) in tools.endpoints {
            loopback.register(kind, endpoint);
        }
        for factory in extra_roles {
            roles.push(factory(&services));
        }
        for factory in extra_endpoints {
            let (kind, endpoint) = factory(&services);
            loopback.register(kind, endpoint);
        }

        let node = params.roles.contains(&Role::Node);
        let role_changes = Arc::new(crate::role_changes::Requests::new(
            supervised_roles && !params.managed && !node,
        ));
        let facts_role_changes = Arc::clone(&role_changes);
        let host = Host::detect();
        let facts_roles = roles.clone();
        let facts_policy = Arc::clone(&policy);
        let facts_link = link.clone();
        let facts_services = Arc::clone(&services);
        let plugins = params
            .config
            .plugin
            .iter()
            .map(|plugin| orch_link_contract::session::MachineFactsPlugin {
                kind: plugin.kind.clone(),
                name: plugin.name.clone(),
                version: String::new(),
                state: "registered".to_owned(),
                descriptor: None,
                executor: None,
            })
            .collect::<Vec<_>>();
        let facts_programs = programs.clone();
        let facts_now: Arc<crate::dispatch::FactsNow> = Arc::new(move || {
            let mut draft = FactsDraft {
                process_controls: facts_services.process_controls,
                dedicated: facts_policy.config().connector.dedicated,
                plugins: plugin_facts
                    .as_ref()
                    .map_or_else(|| plugins.clone(), |read| read()),
                ..FactsDraft::default()
            };
            for role in &facts_roles {
                role.contribute(&mut draft);
            }
            if draft.storages.is_empty()
                && let Some(policy) = facts_policy.policy()
            {
                draft.storages = crate::storages::facts(&facts_services.config, &policy);
            }
            draft.harnesses.extend(facts_services.installed_harnesses());
            let mut facts = machine_facts(
                &host,
                draft,
                facts_policy.policy(),
                facts_link.rates(),
                facts_link.clock_offset_ms(),
            );
            facts.programs = facts_programs
                .as_ref()
                .map(|handler| handler.snapshot(&facts_policy));
            facts.role_management = Some(facts_role_changes.capability(&facts_policy));
            facts.installation = installation.clone();
            facts
        });
        let mut dispatcher = Dispatcher::new(Arc::clone(&policy), Arc::clone(&facts_now));
        let update_requests = Arc::new(crate::automatic::Requests::new(
            !params.managed || node,
            if node {
                crate::update::Program::Node
            } else {
                crate::update::Program::Connector
            },
            params.version.clone(),
        ));
        if verified_updates {
            update_requests.enable();
        }
        dispatcher.updates(Arc::clone(&update_requests));
        dispatcher.role_changes(Arc::clone(&role_changes), Arc::clone(&services));
        let programs_changed = programs.as_ref().and_then(|handler| handler.changes());
        if let Some(handler) = programs {
            dispatcher.programs(handler, Arc::clone(&services));
        }
        let turn_admission = dispatcher.turn_admission();
        for role in &roles {
            dispatcher.add(role)?;
        }
        // What the system asks is kept for the console's Activity, in memory only.
        let activity = Arc::new(crate::local_socket::Activity::default());
        link.serve(Arc::new(crate::local_socket::Recording::new(
            Arc::new(dispatcher),
            Arc::clone(&activity),
            Arc::clone(&params.clock),
        )));

        let stop = CancellationToken::new();
        let mut tasks = vec![tokio::spawn(watch(link.clone())), tunnels.run()];
        let mut advertised = link.advertised_endpoints();
        let endpoint_data = data.clone();
        let endpoint_stop = stop.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                if let Some(endpoints) = advertised.borrow_and_update().clone()
                    && let Err(error) = endpoint_data.store_server_endpoints(endpoints)
                {
                    tracing::warn!(%error, "the system's address list could not be saved");
                }
                tokio::select! {
                    () = endpoint_stop.cancelled() => return,
                    changed = advertised.changed() => if changed.is_err() { return; },
                }
            }
        }));
        if let Some(mut changes) = programs_changed {
            let stopped = stop.clone();
            let publisher = Arc::clone(&services.publisher);
            let facts = Arc::clone(&facts_now);
            tasks.push(tokio::spawn(async move {
                loop {tokio::select! {
                    ()=stopped.cancelled()=>break,
                    changed=changes.changed()=>{
                        if changed.is_err(){break;}
                        if let Err(error)=publisher.publish(Stream::Machine,Event::MachineFacts(facts())).await {
                            tracing::warn!(%error,"a program state change could not be reported");
                        }
                    },
                }}
            }));
        }
        tasks.push(tokio::spawn(crate::roles::ports::watch(
            Arc::clone(&services),
            stop.clone(),
        )));
        tasks.push(tokio::spawn(crate::facts::on_connections(
            link.clone(),
            roles.clone(),
            stop.clone(),
        )));
        tasks.push(tokio::spawn(report_facts(
            link.clone(),
            facts_now,
            policy.changes(),
            stop.clone(),
        )));
        // What the owner changes in the file while the connector runs holds without a restart.
        tasks.push(tokio::spawn(crate::policy::watch_file(
            Arc::clone(&policy),
            stop.clone(),
        )));
        // The console and `orch-connector mcp` ask a person's connector over its local socket; a runtime the
        // system made has nobody at it.
        let identity_reload = Arc::new(tokio::sync::Notify::new());
        if !params.managed || params.roles.contains(&Role::Node) {
            let state = crate::local_socket::LocalState {
                identity_reload: supervised_roles.then(|| Arc::clone(&identity_reload)),
                identity_fingerprint: params.identity.certificate.fingerprint(),
                link: link.clone(),
                version: params.version.clone(),
                data_dir: services.data_dir.clone(),
                environments: environment_role.clone(),
                hosting: hosting.clone(),
                activity: Arc::clone(&activity),
            };
            tasks.push(tokio::spawn(crate::local_socket::serve(
                Arc::new(state),
                stop.clone(),
            )));
        }

        let connector = Self {
            identity_reload,
            role_changes,
            update_requests,
            environment_role,
            link,
            services,
            roles,
            hosting,
            loopback,
            stop,
            tasks: Mutex::new(tasks),
            turn_admission,
        };
        for role in &connector.roles {
            role.recover().await;
        }
        Ok(connector)
    }

    /// The link to the gateway.
    #[must_use]
    pub fn link(&self) -> &MachineLink {
        &self.link
    }

    /// What the roles of this connector are given.
    #[must_use]
    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    /// The actual attached environment role, for composing services over its one native scope.
    #[must_use]
    pub fn environment_role(&self) -> Option<&Arc<crate::roles::environment::EnvironmentRole>> {
        self.environment_role.as_ref()
    }

    /// The listener for harnesses.
    #[must_use]
    pub fn loopback(&self) -> &Arc<Loopback> {
        &self.loopback
    }

    /// The harness processes it hosts, if it plays the agent role.
    #[must_use]
    pub fn hosting(&self) -> Option<&Arc<Hosting>> {
        self.hosting.as_ref()
    }

    /// How many turns are running in the sessions it hosts: nothing that is running is lost by stopping it
    /// when this is nought.
    #[must_use]
    pub fn turns_running(&self) -> usize {
        self.hosting.as_ref().map_or(0, |hosting| {
            hosting
                .recorded()
                .iter()
                .flat_map(|harness| harness.sessions.iter())
                .filter(|session| session.running_turn.is_some())
                .count()
        })
    }

    /// Holds new turn starts out only if every previously admitted start and running turn is idle.
    /// Keep this owned guard through installation and stopping; dropping it resumes ordinary admission.
    pub fn idle_for_update(&self) -> Option<tokio::sync::OwnedRwLockWriteGuard<()>> {
        let guard = Arc::clone(&self.turn_admission).try_write_owned().ok()?;
        (self.turns_running() == 0).then_some(guard)
    }

    /// The machine's facts as they are now.
    #[must_use]
    pub fn facts(&self) -> orch_link_contract::session::MachineFacts {
        let mut draft = FactsDraft {
            process_controls: self.services.process_controls,
            dedicated: self.services.policy.config().connector.dedicated,
            ..FactsDraft::default()
        };
        for role in &self.roles {
            role.contribute(&mut draft);
        }
        if draft.storages.is_empty()
            && let Some(policy) = self.services.policy.policy()
        {
            draft.storages = crate::storages::facts(&self.services.config, &policy);
        }
        draft.harnesses.extend(self.services.installed_harnesses());
        let mut facts = machine_facts(
            &Host::detect(),
            draft,
            self.services.policy.policy(),
            self.link.rates(),
            self.link.clock_offset_ms(),
        );
        facts.role_management = Some(self.role_changes.capability(&self.services.policy));
        facts
    }

    fn end_tasks(&self) {
        self.stop.cancel();
        let tasks: Vec<JoinHandle<()>> = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect();
        for task in tasks {
            task.abort();
        }
    }

    /// Stops: the roles stop what they run, the listener closes, and the link ends the session and puts
    /// what it holds on disk. What was hosted stays on record for the next start.
    pub async fn stop(self) {
        self.link.cancel_recordings().await;
        for role in &self.roles {
            role.stop().await;
        }
        self.services.port_publisher.close_all().await;
        self.loopback.stop();
        self.end_tasks();
        self.link.shutdown().await;
    }

    /// Lets go of everything as a process that is killed does: nothing is stopped and the record of what
    /// was hosted is not touched; the link puts what it holds on disk, as the operating system would
    /// leave it. For tests of what the next start finds.
    pub async fn abandon(self) {
        for role in &self.roles {
            role.abandon();
        }
        if let Some(hosting) = &self.hosting {
            hosting.abandon();
        }
        self.services.port_publisher.close_all().await;
        self.loopback.stop();
        self.end_tasks();
        self.link.shutdown().await;
    }
}

async fn report_facts(
    link: MachineLink,
    facts: Arc<crate::dispatch::FactsNow>,
    mut policy_changes: tokio::sync::watch::Receiver<u64>,
    stop: CancellationToken,
) {
    let mut ticker = tokio::time::interval(FACTS_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick is at once: the facts are reported when the connector starts, whether or not a
    // session stands yet; the spool keeps them for it.
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = ticker.tick() => {}
            changed = policy_changes.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
        let now = facts();
        if let Err(error) = link
            .publish(&Stream::Machine, &Event::MachineFacts(now))
            .await
        {
            tracing::warn!(%error, "the facts of the machine could not be put in the spool");
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Keeping it running

/// How a program makes the connector it runs, once it knows who the connector is: the builder with
/// everything else chosen (the dialer that shows this identity, the adapters, the roles).
pub type MakeConnector<'a> =
    dyn Fn(MachineIdentity) -> Result<ConnectorBuilder, StartError> + Send + Sync + 'a;

/// How [`run_until`] keeps a connector going.
#[derive(Debug, Clone, Copy)]
pub struct Supervision {
    /// How often the certificate is looked at.
    pub look_every: Duration,
    /// How often a restart that waits for the harnesses to have nothing running looks again.
    pub idle_every: Duration,
}

impl Default for Supervision {
    fn default() -> Self {
        Self {
            look_every: RENEWAL_LOOK_EVERY,
            idle_every: IDLE_LOOK_EVERY,
        }
    }
}

/// Renewal timing and the optional updater owned by the physical program.
pub struct RunOptions<'a> {
    /// A physical program's local address choice when connector configuration has no list.
    pub local_endpoints: Option<Vec<Endpoint>>,
    /// How renewal and idle restart are supervised.
    pub supervision: Supervision,
    /// A verified updater; absent for managed unit connectors.
    pub updates: Option<&'a dyn crate::automatic::Updates>,
}

/// How often a connector that wants to start again looks whether its harnesses are idle: half a minute,
/// the order of a turn's shortest runs, so that the wait ends soon after the last turn does and the look
/// costs nothing.
pub const IDLE_LOOK_EVERY: Duration = Duration::from_secs(30);

/// Runs the connector `make` makes until `shutdown` resolves. The certificate is renewed when it is due
/// (two thirds of its life), with a key made anew; the link was built with the old pair, so the
/// connector is then stopped and started again with the new, as soon as none of its harnesses has a turn
/// running (a turn would not survive it). `var` reads the environment the configuration is read through.
pub async fn run_until(
    data_dir: &DataDir,
    make: &MakeConnector<'_>,
    transport: &dyn EnrollTransport,
    clock: Arc<dyn Clock>,
    supervision: Supervision,
    var: &(dyn Fn(&str) -> Option<String> + Sync),
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), StartError> {
    run_until_with_updates(
        data_dir,
        make,
        transport,
        clock,
        RunOptions {
            local_endpoints: None,
            supervision,
            updates: None,
        },
        var,
        shutdown,
    )
    .await
}

/// The ordinary renewal supervisor, with an optional program-owned verified automatic updater.
pub async fn run_until_with_updates(
    data_dir: &DataDir,
    make: &MakeConnector<'_>,
    transport: &dyn EnrollTransport,
    clock: Arc<dyn Clock>,
    options: RunOptions<'_>,
    var: &(dyn Fn(&str) -> Option<String> + Sync),
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), StartError> {
    let managed = data_dir.record()?.managed;
    let node = data_dir
        .config(var)?
        .connector
        .roles
        .iter()
        .any(|role| role == "node");
    let _instance = (!managed || node)
        .then(|| crate::uninstall::InstanceLock::running(data_dir.root()))
        .transpose()?;
    let RunOptions {
        local_endpoints,
        supervision,
        updates,
    } = options;
    tokio::pin!(shutdown);
    loop {
        let mut identity = data_dir.identity()?;
        let config = data_dir.config(var)?;
        let endpoints = if config.endpoints.is_empty() {
            match &local_endpoints {
                Some(endpoints) => endpoints.clone(),
                None => data_dir.endpoints(&config)?,
            }
        } else {
            data_dir.endpoints(&config)?
        };
        // A certificate that came due while the connector was off is renewed before the link is made
        // with it; if that fails the old one is tried, which is good until it ends.
        match renew_if_due(
            &mut identity,
            data_dir,
            &endpoints,
            transport,
            clock.as_ref(),
        )
        .await
        {
            Ok(true) => {
                tracing::info!("the certificate was due and has been renewed");
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, "the certificate is due and could not be renewed; starting with the old one")
            }
        }
        let mut builder = make(identity)?.verified_updates(updates.is_some());
        builder.supervised_roles = true;
        let connector = builder.start().await?;
        let renewal_link = connector.link().clone();
        let renewed = renew_when_due_using(
            data_dir,
            move || renewal_link.endpoints(),
            transport,
            clock.as_ref(),
            supervision.look_every,
        );
        let mut roles_admission = tokio::select! {
            () = &mut shutdown => {
                connector.stop().await;
                return Ok(());
            }
            _ = renewed => None,
            () = connector.identity_reload.notified() => None,
            guard = connector.role_changes.until_answered()=>Some(guard),
            _guard = crate::automatic::until_installed(&connector, data_dir, updates) => {
                connector.stop().await;
                return Err(StartError::Updated);
            }
        };
        // Renewal shares the same idle fence. A role command arriving during renewal must still
        // finish its journaled answer before either path tears down the role handlers.
        while roles_admission.is_none() {
            if let Some(guard) = connector.idle_for_update() {
                roles_admission = Some(guard);
                break;
            }
            tokio::select! {
                () = &mut shutdown => {connector.stop().await;return Ok(());},
                guard = connector.role_changes.until_answered()=>roles_admission=Some(guard),
                () = tokio::time::sleep(supervision.idle_every) => {},
            }
        }
        tracing::info!("starting again with the current identity and saved roles");
        connector.stop().await;
        drop(roles_admission);
    }
}
