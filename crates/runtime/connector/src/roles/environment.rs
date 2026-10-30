//! Compose environment-host services.
//!
//! The environment role is the exec node of a workspace. It carries out every command that is about an
//! environment (attaching it, its files, its snapshots and moves, its processes, shells, executors and
//! tool servers) by handing it to the environment host ([`orch_envhost::environment::EnvHost`]), which
//! holds the environments that are attached here, and turns the host's answer into the command's result.
//! The machine's local policy has been applied before a command gets here ([`crate::policy::gate`]) and
//! is applied again by the host, which is given the limits as they change; a path outside the workspace, a
//! write the policy forbids and a shell it forbids are refused whoever asks.
//!
//! What the host says on its own account (the facts of an environment changed, a manifest of its tree was
//! uploaded, a shell ended while nobody was attached, a watched directory changed, the processes it has,
//! what it uses) is put on the environment's stream in the way the catalogue gives each kind: durable ones
//! in the spool, the measurements beside it. The progress of a snapshot or a move goes out as the host
//! reports it.
//!
//! Content that a command moves goes through the link's transfers under the command's own identifier
//! ([`crate::content::CommandBlobs`]). Tunnels that end here are served by [`crate::shells`] (a person's
//! shell) and [`crate::tool_tunnels`] (an executor, a tool server); what a harness on this machine reaches
//! its tools through is [`crate::tool_endpoints`]. The tool servers the harnesses here have configured are
//! found and imported by [`crate::mcp_import`].

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use async_trait::async_trait;
use orch_api_contract::workspace::JobProgress as HostJobProgress;
use orch_driver_contract::runtime::ToolRef;
use orch_envhost::environment::{
    EnvHost, ExecutorFactory, Gates, HostSettings, ManagedPlaces, Report as HostReport, Reported,
    Reports,
};
use orch_envhost::mcp::McpGates;
use orch_envhost::processes::ScopeKind;
use orch_envhost::snapshot::Progress;
use orch_envhost::watch::WatchMode;
use orch_link::commands::CommandExecution;
use orch_link::machine::MachineLink;
use orch_link::tunnels::channel::IncomingTunnel;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, EnvAttach, ErrorCode, McpStart,
    Secret as LinkSecret,
};
use orch_link_contract::events::{
    EnvStatus, Event, JobProgress, ProcsSnapshot, Report as LinkReport,
};
use orch_link_contract::policy::LocalPolicy;
use orch_link_contract::session::Role;
use orch_link_contract::streams::Stream;
use orch_link_contract::tunnels::{OpenErrorCode, TunnelKind};
use orch_types::Id;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::facts::FactsDraft;
use crate::harnesses::Hosting;
use crate::loopback::{Endpoint, EndpointKind};
use crate::mcp_import::McpImports;
use crate::roles::{Call, RoleHandler, Services};
use crate::tool_endpoints::{ExecEndpoint, McpEndpoint};
use crate::tool_tunnels::ToolRelay;
use crate::tunnels::TunnelServer;

/// The kinds of command the environment role takes: every kind the catalogue addresses to it.
fn directory_mount_id(name: &str, labels: &BTreeMap<String, String>) -> Result<Id, CommandError> {
    let id: Id = name
        .strip_prefix("orch-mount-")
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| {
            CommandError::new(
                ErrorCode::Conflict,
                "owned mount data must name its mount identifier",
            )
        })?;
    if labels.get("orch.mount_id") != Some(&id.to_string()) {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "the owned-data labels do not identify this mount",
        ));
    }
    Ok(id)
}

const TAKES: &[CommandKind] = &[
    CommandKind::NodeVolumeEnsure,
    CommandKind::NodeVolumeRemove,
    CommandKind::UnitAfterUp,
    CommandKind::DisplayOpen,
    CommandKind::DisplayAdopt,
    CommandKind::DisplayClose,
    CommandKind::ScreenLook,
    CommandKind::BrowserOpen,
    CommandKind::BrowserAdopt,
    CommandKind::BrowserClose,
    CommandKind::BrowserProfile,
    CommandKind::PtyOpen,
    CommandKind::PtyClose,
    CommandKind::PtyStatus,
    CommandKind::EnvAttach,
    CommandKind::EnvDetach,
    CommandKind::EnvDataRemove,
    CommandKind::ExecStart,
    CommandKind::McpStart,
    CommandKind::McpRestart,
    CommandKind::McpImport,
    CommandKind::McpForget,
    CommandKind::McpStop,
    CommandKind::FsList,
    CommandKind::FsUnwatch,
    CommandKind::FsStat,
    CommandKind::FsFetch,
    CommandKind::FsWrite,
    CommandKind::FsMkdir,
    CommandKind::FsRename,
    CommandKind::FsRemove,
    CommandKind::FsCopy,
    CommandKind::FsArchiveList,
    CommandKind::FsArchiveRead,
    CommandKind::FsSearch,
    CommandKind::FsManifest,
    CommandKind::FsSnapshot,
    CommandKind::FsMeasure,
    CommandKind::FsRecord,
    CommandKind::FsRecordRead,
    CommandKind::FsRecordPrune,
    CommandKind::FsRecordVerify,
    CommandKind::FsDataMeasure,
    CommandKind::FsDataExport,
    CommandKind::FsDataImport,
    CommandKind::FsRestorePlan,
    CommandKind::FsRestore,
    CommandKind::WorkspaceObserve,
    CommandKind::FsMaterialize,
    CommandKind::GitStatus,
    CommandKind::GitPrepare,
    CommandKind::GitPrepareCleanup,
    CommandKind::GitWorktree,
    CommandKind::GitWorktreeCleanup,
    CommandKind::GitWorktreeRepair,
    CommandKind::GitWorktreeCopyPlan,
    CommandKind::GitWorktreeSnapshot,
    CommandKind::GitWorktreeStandalone,
    CommandKind::GitCommit,
    CommandKind::GitPush,
    CommandKind::GitDiff,
    CommandKind::ProcList,
    CommandKind::PortsLook,
    CommandKind::ProcSignal,
    CommandKind::ProcSuspend,
    CommandKind::ProcResume,
    CommandKind::WatchSet,
];

/// How many reports of the host wait to be put on their stream before the host starts to drop the
/// measurements among them: some seconds of a busy tree's changes, a procs list and a sample each second.
/// The reports that cannot be dropped wait for room instead.
const REPORT_QUEUE: usize = 256;

/// How often the tool servers the harnesses here have configured are looked at again: a person edits a
/// harness's file at no moment the connector can know of, and a look costs the reading of a few small files.
const DISCOVERY_EVERY: Duration = Duration::from_secs(30);

/// How long the host is given to publish the tree's list before it goes offline: the link is still up
/// when the roles stop, and a stop is not to wait on a slow line.
const BEFORE_OFFLINE: Duration = Duration::from_secs(5);

/// What a program chooses for the environment role: the executors an environment can run, and where a
/// runtime the system made has its places.
#[derive(Clone, Default)]
pub struct EnvironmentParts {
    /// Makes the executors that each environment can run.
    pub executors: Vec<ExecutorFactory>,
    /// Where a runtime the system made has its workspace, home, mounts file and tools; `None` for a
    /// machine a person connected, whose environments are directories it offers.
    pub places: Option<ManagedPlaces>,
    /// Which processes of the machine are the environment's; by default every one in a runtime the system
    /// made, and only those started for it on a person's machine.
    pub process_scope: Option<ScopeKind>,
    /// The shell people get, when the machine's own is not wanted.
    pub shell: Option<String>,
    /// How watching is done; by default with the operating system's notices.
    pub watch: Option<WatchMode>,
    /// The ports the tool servers may listen on, on the machine itself; by default the system chooses.
    pub ports: Option<RangeInclusive<u16>>,
}

impl std::fmt::Debug for EnvironmentParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentParts")
            .field("executors", &self.executors.len())
            .finish_non_exhaustive()
    }
}

/// The environment role.
pub struct EnvironmentRole {
    local_ports: Arc<crate::local_ports::LocalPorts>,
    ports: Arc<crate::ports_carrier::PortCarrier>,
    host: EnvHost,
    services: Arc<Services>,
    link: MachineLink,
    imports: McpImports,
    relay: Arc<ToolRelay>,
    stop: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    desired: crate::reconciliation::environments::Desired,
    generation: i64,
    lifecycle: tokio::sync::Mutex<()>,
    after_up: crate::after_up::Runner,
    hooks: crate::lifecycle::Hooks,
}

impl std::fmt::Debug for EnvironmentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentRole").finish_non_exhaustive()
    }
}

// An attach can be cancelled before its async host call returns. A new listener must
// not retain a bearer for an environment that never completed attachment.
struct AttachingPorts {
    readers: Arc<crate::local_ports::LocalPorts>,
    environment: Id,
    armed: bool,
}
impl Drop for AttachingPorts {
    fn drop(&mut self) {
        if self.armed {
            self.readers.detach(self.environment);
        }
    }
}

fn internal(what: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorCode::Internal, what.to_string())
}

/// The limits the host is given from the policy of the machine.
fn gates_of(policy: &LocalPolicy) -> Gates {
    Gates {
        remote_write: policy.environment_remote_write,
        deliver_files: policy.environment_deliver_files,
        terminal: policy.environment_terminal,
        browser: policy.environment_browser,
        vnc: policy.environment_vnc,
    }
}

fn mcp_gates_of(policy: &LocalPolicy) -> McpGates {
    McpGates {
        import: policy.mcp_import,
        add: policy.mcp_add,
    }
}

fn roots_of(policy: &LocalPolicy, data_dir: &std::path::Path) -> Vec<PathBuf> {
    policy
        .environment_roots
        .iter()
        .map(PathBuf::from)
        .chain(std::iter::once(data_dir.join("environment-data")))
        .collect()
}

/// What the host starts with: the policy of the machine if a person connected it, the places a driver
/// gave if the system made it, and the tunables of the connector's configuration.
fn settings_of(services: &Services, parts: &EnvironmentParts) -> HostSettings {
    let data_dir = services.data_dir.clone();
    let mut settings = match (services.policy.policy(), &parts.places) {
        (Some(policy), _) => {
            let roots = roots_of(&policy, &data_dir);
            let mut settings = HostSettings::connected(data_dir, roots);
            settings.gates = gates_of(&policy);
            settings.exec_offers_allowed = Some(policy.exec_offers.clone());
            settings.mcp = mcp_gates_of(&policy);
            settings
        }
        (None, Some(places)) => HostSettings::managed(
            data_dir,
            places.clone(),
            parts.process_scope.unwrap_or(ScopeKind::All),
        ),
        (None, None) => {
            // A runtime the system made, told of no places: nothing limits it, and a directory is attached
            // by the root the command names.
            let mut settings = HostSettings::connected(data_dir, Vec::new());
            settings.gates = Gates::OPEN;
            settings.allowed_roots = None;
            settings.mcp = McpGates {
                import: true,
                add: true,
            };
            settings
        }
    };
    settings.process_controls = services.process_controls;
    // The builds the system put in a runtime it made are the tools the environment was made with.
    settings.tools = services
        .installed
        .iter()
        .map(|found| ToolRef {
            name: found.kind.clone(),
            version: found.version.clone(),
        })
        .collect();
    let tunables = &services.config.envhost;
    settings.limits.rescan = Duration::from_secs(tunables.rescan_secs);
    settings.limits.manifest_interval = Duration::from_secs(tunables.manifest_interval_secs);
    settings.limits.index_max_entries = tunables.index_max_entries;
    settings.limits.index_scan_max = std::time::Duration::from_secs(tunables.index_scan_max_secs);
    settings.mcp_restarts = tunables.mcp_restarts;
    settings.mcp_restart_window = Duration::from_secs(tunables.mcp_restart_window_secs);
    if let Some(scope) = parts.process_scope {
        settings.process_scope = scope;
    }
    if let Some(mode) = parts.watch {
        settings.watch = mode;
    }
    settings.shell.clone_from(&parts.shell);
    settings.mcp_ports.clone_from(&parts.ports);
    settings
}

/// The newest progress of a snapshot or a move. A slow spool keeps one pending sample rather than
/// every intermediate counter reported by the host's synchronous workers.
#[derive(Clone)]
struct JobProgressSink {
    clock: Arc<dyn orch_types::Clock>,
    queue: Option<watch::Sender<Option<HostJobProgress>>>,
    latest: Arc<Mutex<(Option<HostJobProgress>, std::time::Instant)>>,
    started: std::time::Instant,
}

impl JobProgressSink {
    fn new(
        queue: Option<watch::Sender<Option<HostJobProgress>>>,
        clock: Arc<dyn orch_types::Clock>,
    ) -> Self {
        Self {
            queue,
            clock,
            latest: Arc::new(Mutex::new((None, std::time::Instant::now()))),
            started: std::time::Instant::now(),
        }
    }
}

impl Progress for JobProgressSink {
    fn counting(&self, files: i64, bytes: i64, directories: i64) {
        self.report(&HostJobProgress {
            phase: "measure".into(),
            phases: vec![orch_api_contract::workspace::JobPhase {
                name: "measure".into(),
                weight: 100,
                state: orch_api_contract::workspace::PhaseState::Running,
            }],
            percent: 0,
            files_done: files,
            files_total: None,
            bytes_done: bytes,
            bytes_total: None,
            rate_bps: None,
            eta_seconds: None,
            message: None,
            activity: Some(orch_api_contract::workspace::JobActivity {
                waiting_for: None,
                updated_at: self.clock.time(),
                stall_after_seconds: 0,
                directories_done: Some(directories),
                stopping: false,
            }),
        });
    }
    fn report(&self, progress: &HostJobProgress) {
        let mut progress = progress.clone();
        let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(before) = &latest.0
            && before.phase == progress.phase
        {
            progress.bytes_done = progress.bytes_done.max(before.bytes_done);
            progress.percent = progress.percent.max(before.percent);
        }
        latest.0 = Some(progress.clone());
        latest.1 = std::time::Instant::now();
        if let Some(queue) = &self.queue {
            let _ = queue.send_replace(Some(progress));
        }
    }

    fn transfer(&self, bytes: i64) {
        let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        let should_send = latest.1.elapsed() >= orch_envhost::snapshot::PROGRESS_INTERVAL;
        let Some(progress) = &mut latest.0 else {
            return;
        };
        progress.bytes_done = progress.bytes_done.saturating_add(bytes);
        let elapsed = self.started.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            let rate = (progress.bytes_done as f64 / elapsed) as i64;
            progress.rate_bps = Some(rate);
            progress.eta_seconds = progress
                .bytes_total
                .filter(|_| rate > 0)
                .map(|total| total.saturating_sub(progress.bytes_done).max(0) / rate);
        }
        if let Some(total) = progress.bytes_total.filter(|total| *total > 0) {
            let all: u32 = progress.phases.iter().map(|phase| phase.weight).sum();
            let done: u32 = progress
                .phases
                .iter()
                .filter(|phase| phase.state == orch_api_contract::workspace::PhaseState::Done)
                .map(|phase| phase.weight)
                .sum();
            let current: u32 = progress
                .phases
                .iter()
                .filter(|phase| phase.state == orch_api_contract::workspace::PhaseState::Running)
                .map(|phase| phase.weight)
                .sum();
            if all > 0 {
                let fraction = (progress.bytes_done as f64 / total as f64).clamp(0.0, 1.0);
                progress.percent = progress.percent.max(
                    ((f64::from(done) + f64::from(current) * fraction) * 100.0 / f64::from(all))
                        as u32,
                );
            }
        }
        if should_send && let Some(queue) = &self.queue {
            let _ = queue.send_replace(Some(progress.clone()));
            latest.1 = std::time::Instant::now();
        }
    }
}

impl EnvironmentRole {
    /// Starts the role over a host of its own, made from the connector's services and what the program
    /// chose. `hosting` is where the harness processes of this machine are, for the tool servers their
    /// configuration names.
    #[must_use]
    pub fn start(
        services: &Arc<Services>,
        link: &MachineLink,
        hosting: Option<Arc<Hosting>>,
        parts: &EnvironmentParts,
    ) -> Arc<Self> {
        let (reports, queue) = Reports::channel(REPORT_QUEUE);
        let settings = settings_of(services, parts);
        let generation = settings.generation;
        let host = EnvHost::new(
            settings,
            Arc::clone(&services.clock),
            reports,
            Arc::new(services.content.standing_blobs()),
            parts.executors.clone(),
        );
        let stop = CancellationToken::new();
        let local_ports = crate::local_ports::LocalPorts::new(
            host.clone(),
            link.clone(),
            Arc::clone(&services.clock),
            services.policy.policy().is_none(),
        );
        let ports = crate::ports_carrier::PortCarrier::new(
            host.clone(),
            Arc::clone(&services.policy),
            Arc::clone(&services.clock),
        );
        ports.local_reader(&local_ports);
        let role = Arc::new(Self {
            local_ports,
            ports,
            host: host.clone(),
            services: Arc::clone(services),
            link: link.clone(),
            imports: McpImports::new(host.clone(), hosting),
            relay: ToolRelay::new(services, link, Some(host)),
            stop: stop.clone(),
            tasks: Mutex::new(Vec::new()),
            desired: crate::reconciliation::environments::Desired::new(services),
            generation,
            lifecycle: tokio::sync::Mutex::new(()),
            after_up: crate::after_up::Runner::new(services),
            hooks: crate::lifecycle::Hooks::for_machine(Arc::clone(&services.policy)),
        });
        let tasks = vec![
            tokio::spawn(publish_reports(
                Arc::clone(services),
                link.clone(),
                role.host.clone(),
                queue,
                stop.clone(),
            )),
            tokio::spawn(follow_policy(Arc::downgrade(&role), stop.clone())),
            tokio::spawn(look_for_tool_servers(Arc::downgrade(&role), stop.clone())),
        ];
        let server: Arc<dyn TunnelServer> = Arc::new(EnvironmentTunnels(Arc::downgrade(&role)));
        for kind in [
            TunnelKind::Pty,
            TunnelKind::Exec,
            TunnelKind::Mcp,
            TunnelKind::Browser,
            TunnelKind::Rfb,
            TunnelKind::Port,
        ] {
            services.tunnels.register(kind, Arc::clone(&server));
        }
        role.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(tasks);
        role
    }

    /// The environment host.
    #[must_use]
    pub fn host(&self) -> &EnvHost {
        &self.host
    }

    /// The tool servers of the harnesses' own configuration, found and imported here.
    #[must_use]
    pub fn imports(&self) -> &McpImports {
        &self.imports
    }

    /// The connector's services.
    #[must_use]
    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    /// The machine's link.
    #[must_use]
    pub fn link(&self) -> &MachineLink {
        &self.link
    }

    /// What the executors and tool servers here are reached through.
    #[must_use]
    pub fn relay(&self) -> &Arc<ToolRelay> {
        &self.relay
    }

    /// Serves a tunnel that another machine, or the system, opens to this one: a person's shell, an
    /// executor, a tool server. A tunnel of any other kind is refused at once, so that whoever opened it is
    /// not left waiting for an answer.
    pub async fn serve_tunnel(&self, incoming: IncomingTunnel) {
        match incoming.kind {
            TunnelKind::Browser => crate::browser_tunnel::serve(self, incoming).await,
            TunnelKind::Rfb => crate::display_tunnel::serve(self, incoming).await,
            TunnelKind::Port => self.ports.serve(incoming).await,
            TunnelKind::Pty => crate::shells::serve(self, incoming).await,
            TunnelKind::Exec | TunnelKind::Mcp => {
                crate::tool_tunnels::serve(&self.relay, incoming).await;
            }
            TunnelKind::Llm => incoming.refuse(
                OpenErrorCode::NoSuchService,
                "this machine does not serve tunnels of that kind",
            ),
        }
    }

    async fn publish(&self, environment_id: Id, event: Event) {
        let stream = Stream::Env { environment_id };
        if let Err(error) = self.services.publisher.publish(stream, event).await {
            tracing::warn!(%error, "an event of an environment could not be put in the spool");
        }
    }

    /// Says how the environment stands: `running` while it is attached here, `offline` when it is not. The
    /// catalogue has no event for the state of a tool server, so a server that was given up on is said in
    /// the detail of the environment's status.
    pub(crate) async fn status(&self, environment_id: Id, status: &str, detail: Option<String>) {
        let generation = self
            .host
            .environment(environment_id)
            .map_or(0, |environment| environment.facts().generation);
        self.publish(
            environment_id,
            Event::EnvStatus(EnvStatus {
                status: status.to_owned(),
                detail,
                generation,
            }),
        )
        .await;
    }

    fn unseal(
        &self,
        command: Id,
        secrets: &BTreeMap<String, LinkSecret>,
    ) -> Result<BTreeMap<String, String>, CommandError> {
        let mut opened = BTreeMap::new();
        for (name, secret) in secrets {
            let value = match secret {
                LinkSecret::Sealed(sealed) => self.services.keys.open(command, sealed)?,
                LinkSecret::Ref(_) => {
                    return Err(internal(format!(
                        "{name} is a reference to a secret that was not replaced"
                    )));
                }
            };
            opened.insert(name.clone(), value);
        }
        Ok(opened)
    }

    async fn attach(&self, command: Id, body: &EnvAttach) -> Result<CommandResult, CommandError> {
        let secrets = self.unseal(command, &body.secrets)?;
        let previous = self.host.environment(body.environment_id).is_ok();
        if !previous {
            self.directory_hook(body.environment_id, crate::lifecycle::HookEvent::PreUp)
                .await?;
        }
        let local = self
            .local_ports
            .variables(body.environment_id)
            .await
            .map_err(|_| internal("the local port listener is unavailable"))?;
        let mut attaching = AttachingPorts {
            readers: Arc::clone(&self.local_ports),
            environment: body.environment_id,
            armed: !previous,
        };
        // The original command/journal remains unchanged. These fresh local values are given only
        // to the environment's actual started processes and cannot be overridden by user secrets.
        let mut private = body.clone();
        private.variables.extend(local.clone());
        if let (Some(policy), Some(sources)) =
            (self.services.policy.policy(), body.data_sources.as_ref())
        {
            if body
                .storage
                .as_ref()
                .is_some_and(|storage| storage.state.is_some())
            {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "this directory connector keeps private state in its machine data directory; its declared storage roles are workspace, home and mounts",
                ));
            }
            let mut resolved = sources.clone();
            for (role, source) in [
                ("workspace", &mut resolved.workspace),
                ("home", &mut resolved.home),
            ] {
                let path = match source.directory() {
                    Some(path) => {
                        crate::policy::inspect_environment_directory(
                            &policy,
                            std::path::Path::new(path),
                        )?
                        .path
                    }
                    None => {
                        let requested = body.storage.as_ref().and_then(|storage| match role {
                            "workspace" => storage.workspace.as_deref(),
                            _ => storage.home.as_deref(),
                        });
                        let path = crate::storages::directory_generation(
                            &self.services.data_dir,
                            &self.services.config,
                            &policy,
                            body.environment_id,
                            role,
                            requested,
                            body.data_generation.unwrap_or(0),
                        )?;
                        path.to_string_lossy().into_owned()
                    }
                };
                crate::policy::writable_environment_directory(std::path::Path::new(&path))?;
                *source = orch_driver_contract::runtime::EnvironmentDataSource::ExistingDirectory {
                    path,
                };
            }
            private.workspace_root = resolved.workspace.directory().map(str::to_owned);
            private.data_sources = Some(resolved);
        }
        if let Some(policy) = self.services.policy.policy() {
            for mount in private.mounts.iter_mut().flatten() {
                mount.source = if let Some(volume) = &mount.volume {
                    if volume.directory.is_some()
                        || volume.name != format!("orch-mount-{}", mount.mount_id)
                    {
                        return Err(CommandError::new(
                            ErrorCode::Conflict,
                            "a mount's owned data reference does not match its identifier",
                        ));
                    }
                    crate::storages::recorded_directory(&self.services.data_dir,&policy,mount.mount_id,"mount")?
                        .ok_or_else(||CommandError::new(ErrorCode::NotFound,"the mount's owned data is absent; its location must be made before attaching it"))?.to_string_lossy().into_owned()
                } else {
                    crate::policy::inspect_environment_path(
                        &policy,
                        std::path::Path::new(&mount.source),
                        false,
                    )?
                    .path
                };
            }
        }
        // Keep these process variables on browser/display spawns too: those deliberately
        // omit the separately supplied user secrets. No user secret may override a local value.
        let mut secrets = secrets;
        for name in local.keys() {
            secrets.remove(name);
        }
        let attached = match self.host.env_attach(&private, &secrets).await {
            Ok(facts) => facts,
            Err(error) => return Err(error.into()),
        };
        attaching.armed = false;
        if !previous
            && let Err(error) = self
                .directory_hook(body.environment_id, crate::lifecycle::HookEvent::PostUp)
                .await
        {
            let detach = orch_link_contract::commands::EnvDetach {
                environment_id: body.environment_id,
            };
            let _ = self.host.env_detach(&detach).await;
            return Err(error);
        }
        if self.services.policy.policy().is_none() {
            self.services
                .loopback
                .environment_ports(body.environment_id, local.get("ORCH_PORTS_URL").cloned());
        }
        // Restore adopted credentials before the ready report can trigger screen reconciliation.
        if let Some(saved) = self.desired.current(body.environment_id)? {
            let live = self
                .host
                .environment(body.environment_id)?
                .displays()
                .list()
                .await;
            for kept in saved.displays.values() {
                if let Command::DisplayAdopt(display) = &kept.command
                    && !live
                        .iter()
                        .any(|known| known.display_id == display.display_id)
                    && self.adopt_display(kept.id, display).await.is_err()
                {
                    self.publish(
                        body.environment_id,
                        Event::DisplayState(orch_link_contract::display::DisplayState {
                            display_id: display.display_id,
                            state: orch_link_contract::browser::BrowserReportedState::Closed,
                            reason: Some("link_lost".to_owned()),
                            geometry: None,
                        }),
                    )
                    .await;
                }
            }
        }
        // What the harnesses here have configured is part of the facts the answer carries.
        let facts = self
            .imports
            .rediscover(body.environment_id)
            .await
            .unwrap_or(attached);
        self.status(body.environment_id, "running", None).await;
        Ok(CommandResult::EnvAttach(facts))
    }

    async fn directory_hook(
        &self,
        environment: Id,
        event: crate::lifecycle::HookEvent,
    ) -> Result<(), CommandError> {
        use crate::lifecycle::{
            DOCUMENT_VERSION, HookDocument, HookHost, HookNetwork, HookRuntime, HookUnit,
            UnitReason,
        };
        if !self.hooks.wants(event) {
            return Ok(());
        }
        let document = HookDocument {
            version: DOCUMENT_VERSION,
            event,
            reason: if event.fails_closed() {
                UnitReason::Start
            } else {
                UnitReason::Stop
            },
            replay: false,
            at: self.services.clock.time(),
            host: HookHost {
                system: String::new(),
                name: self.services.config.connector.name.clone(),
            },
            runtime: HookRuntime {
                id: environment,
                kind: orch_driver_contract::runtime::RuntimeKind::Environment,
                driver: "directory".into(),
            },
            environment: None,
            provider: None,
            unit: HookUnit {
                id: environment.to_string(),
                name: environment.to_string(),
            },
            network: HookNetwork {
                mode: "connected".into(),
                egress: orch_driver_contract::network::EgressMode::Open,
                networks: Vec::new(),
            },
            ports: Vec::new(),
        };
        if let Some(run) = self
            .hooks
            .run(&document, self.services.clock.as_ref())
            .await
            && run.failed
            && event.fails_closed()
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                format!("a machine lifecycle hook failed: {}", run.first_lines),
            ));
        }
        Ok(())
    }

    /// Reconnects an announced server under the current machine policy.
    async fn adopt_display(
        &self,
        command: Id,
        body: &orch_link_contract::display::DisplayAdopt,
    ) -> Result<orch_link_contract::display::DisplayAdoptResult, CommandError> {
        if self
            .services
            .policy
            .policy()
            .is_some_and(|p| !p.environment_vnc)
        {
            return Err(display_error(
                orch_envhost::displays::DisplayError::Disabled,
            ));
        }
        let password = match &body.password {
            Some(LinkSecret::Sealed(value)) => {
                Some(self.services.keys.open(command, value)?.into_bytes())
            }
            Some(LinkSecret::Ref(_)) => {
                return Err(internal("the display password was not sealed"));
            }
            None => None,
        };
        let environment = self
            .host
            .environment(body.environment_id)
            .map_err(|_| display_error(orch_envhost::displays::DisplayError::NotFound))?;
        environment
            .displays()
            .adopt(body.display_id, &body.endpoint, password)
            .await
            .map_err(display_error)
    }

    /// Progress that goes out on the environment's stream for a job, and the task that puts it there.
    fn job_progress(
        &self,
        command: Id,
        environment_id: Id,
        job: Option<Id>,
    ) -> (JobProgressSink, JoinHandle<()>) {
        let Some(job_id) = job else {
            return (
                JobProgressSink::new(None, Arc::clone(&self.services.clock)),
                tokio::spawn(async {}),
            );
        };
        let (queue, mut reports) = watch::channel::<Option<HostJobProgress>>(None);
        let services = Arc::clone(&self.services);
        let task = tokio::spawn(async move {
            while reports.changed().await.is_ok() {
                let Some(progress) = reports.borrow_and_update().clone() else {
                    continue;
                };
                let event = Event::JobProgress(JobProgress {
                    command_id: Some(command),
                    job_id,
                    progress,
                    log: None,
                });
                if let Err(error) = services
                    .publisher
                    .publish(Stream::Env { environment_id }, event)
                    .await
                {
                    tracing::warn!(%error, "the progress of a job could not be put in the spool");
                }
            }
        });
        (
            JobProgressSink::new(Some(queue), Arc::clone(&self.services.clock)),
            task,
        )
    }
}

impl EnvironmentRole {
    #[allow(
        clippy::too_many_lines,
        reason = "one arm for each command of the role is the clearest form of the table"
    )]
    async fn carry_out(&self, id: Id, command: Command) -> Result<CommandResult, CommandError> {
        if let Command::UnitAfterUp(body) = &command {
            if body.provider_id.is_some() || body.agent_id.is_some() {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "this command needs the environment it runs in",
                ));
            }
            let environment = body.environment_id.ok_or_else(|| {
                CommandError::new(ErrorCode::Conflict, "an environment is required")
            })?;
            let environment = self.host.environment(environment)?;
            return self
                .after_up
                .run_in(
                    body,
                    environment.files().root().root_path(),
                    environment.variables(),
                    &environment.processes().launcher(),
                )
                .await
                .map(CommandResult::UnitAfterUp);
        }
        self.desired.remember(id, &command, self.generation)?;
        let blobs = self.services.content.blobs_for(id);
        Ok(match command {
            Command::NodeVolumeEnsure(body) => {
                let mount = directory_mount_id(&body.name, &body.labels)?;
                let policy = self.services.policy.policy().ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::PolicyRootNotAllowed,
                        "this machine has no environment.roots policy",
                    )
                })?;
                crate::storages::directory(
                    &self.services.data_dir,
                    &self.services.config,
                    &policy,
                    mount,
                    "mount",
                    body.storage.as_deref(),
                )?;
                CommandResult::NodeVolumeEnsure(Empty {})
            }
            Command::NodeVolumeRemove(body) => {
                let mount = directory_mount_id(&body.name, &body.labels)?;
                if self.host.environments().iter().any(|environment| {
                    environment
                        .facts()
                        .mounts
                        .iter()
                        .any(|attached| attached.mount_id == mount)
                }) {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "this owned directory is still attached to an environment",
                    ));
                }
                let policy = self.services.policy.policy().ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::PolicyRootNotAllowed,
                        "this machine has no environment.roots policy",
                    )
                })?;
                crate::storages::remove_directory(
                    &self.services.data_dir,
                    &policy,
                    mount,
                    "mount",
                )?;
                CommandResult::NodeVolumeRemove(Empty {})
            }
            Command::EnvAttach(body) => return self.attach(id, &body).await,
            Command::EnvDetach(body) => {
                let reservation = self.host.reserve_detach(body.environment_id)?;
                self.directory_hook(body.environment_id, crate::lifecycle::HookEvent::PreDown)
                    .await?;
                self.local_ports.detach(body.environment_id);
                self.services
                    .loopback
                    .environment_ports(body.environment_id, None);
                self.host.detach_reserved(&reservation).await?;
                self.directory_hook(body.environment_id, crate::lifecycle::HookEvent::PostDown)
                    .await?;
                self.status(body.environment_id, "offline", None).await;
                CommandResult::EnvDetach(Empty {})
            }
            Command::EnvDataRemove(body) => {
                self.host.require_recording_idle(body.environment_id)?;
                if self.host.environment(body.environment_id).is_ok() {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "Detach this environment before removing its owned files",
                    ));
                }
                let policy = self.services.policy.policy().ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::PolicyRootNotAllowed,
                        "This machine has not allowed owned directory cleanup",
                    )
                })?;
                let data_dir = self.services.data_dir.clone();
                tokio::task::spawn_blocking(move || {
                    for role in ["workspace", "home"] {
                        crate::storages::remove_generation(
                            &data_dir,
                            &policy,
                            body.environment_id,
                            role,
                            body.data_generation,
                        )?;
                    }
                    Ok::<_, CommandError>(())
                })
                .await
                .map_err(internal)??;
                CommandResult::EnvDataRemove(Empty {})
            }
            Command::DisplayOpen(body) => {
                if self
                    .services
                    .policy
                    .policy()
                    .is_some_and(|p| !p.environment_vnc)
                {
                    return Err(display_error(
                        orch_envhost::displays::DisplayError::Disabled,
                    ));
                }
                CommandResult::DisplayOpen(
                    self.host.display_open(&body).await.map_err(display_error)?,
                )
            }
            Command::DisplayAdopt(body) => {
                CommandResult::DisplayAdopt(self.adopt_display(id, &body).await?)
            }
            Command::DisplayClose(body) => {
                for environment in self.host.environments() {
                    if environment
                        .displays()
                        .list()
                        .await
                        .iter()
                        .any(|d| d.display_id == body.display_id)
                    {
                        environment.displays().close(body.display_id).await;
                    }
                }
                CommandResult::DisplayClose(Empty {})
            }
            Command::ScreenLook(body) => CommandResult::ScreenLook(
                self.host.screen_look(&body).await.map_err(browser_error)?,
            ),
            Command::BrowserOpen(body) => CommandResult::BrowserOpen(
                self.host.browser_open(&body).await.map_err(browser_error)?,
            ),
            Command::BrowserAdopt(body) => CommandResult::BrowserAdopt(
                self.host
                    .browser_adopt(&body)
                    .await
                    .map_err(browser_error)?,
            ),
            Command::BrowserClose(body) => {
                self.host
                    .browser_close(&body)
                    .await
                    .map_err(browser_error)?;
                CommandResult::BrowserClose(Empty {})
            }
            Command::BrowserProfile(body) => CommandResult::BrowserProfile(
                self.host
                    .browser_profile(&body)
                    .await
                    .map_err(browser_error)?,
            ),
            Command::FsList(body) => CommandResult::FsList(self.host.fs_list(&body).await?),
            Command::FsUnwatch(body) => {
                self.host.fs_unwatch(&body)?;
                CommandResult::FsUnwatch(Empty {})
            }
            Command::FsStat(body) => CommandResult::FsStat(self.host.fs_stat(&body).await?),
            Command::FsFetch(body) => {
                CommandResult::FsFetch(self.host.fs_fetch(&body, &blobs).await?)
            }
            Command::FsWrite(body) => {
                if let Some(work) = &body.work {
                    let (progress, task) =
                        self.job_progress(id, body.environment_id, Some(work.job_id));
                    let mut reported = HostJobProgress {
                        phase: work.phase.clone(),
                        phases: vec![orch_api_contract::workspace::JobPhase {
                            name: work.phase.clone(),
                            weight: 100,
                            state: orch_api_contract::workspace::PhaseState::Running,
                        }],
                        percent: 0,
                        files_done: 0,
                        files_total: work.files_total,
                        bytes_done: 0,
                        bytes_total: work.bytes_total,
                        rate_bps: None,
                        eta_seconds: None,
                        message: None,
                        activity: None,
                    };
                    progress.report(&reported);
                    let outcome = orch_envhost::snapshot::with_transfer_progress(
                        Arc::new(progress.clone()),
                        self.host.fs_write(&body, &blobs),
                    )
                    .await;
                    if let Ok(stat) = &outcome {
                        reported.files_done = 1;
                        reported.bytes_done = stat.entry.size;
                        reported.percent = 100;
                        progress.report(&reported);
                    }
                    drop(progress);
                    let _ = task.await;
                    CommandResult::FsWrite(outcome?)
                } else {
                    CommandResult::FsWrite(self.host.fs_write(&body, &blobs).await?)
                }
            }
            Command::FsMkdir(body) => CommandResult::FsMkdir(self.host.fs_mkdir(&body).await?),
            Command::FsRename(body) => CommandResult::FsRename(self.host.fs_rename(&body).await?),
            Command::FsRemove(body) => {
                self.host.fs_remove(&body).await?;
                CommandResult::FsRemove(Empty {})
            }
            Command::FsCopy(body) => CommandResult::FsCopy(self.host.fs_copy(&body).await?),
            Command::FsArchiveList(body) => {
                CommandResult::FsArchiveList(self.host.fs_archive_list(&body).await?)
            }
            Command::FsArchiveRead(body) => {
                CommandResult::FsArchiveRead(self.host.fs_archive_read(&body, &blobs).await?)
            }
            Command::FsSearch(body) => CommandResult::FsSearch(self.host.fs_search(&body).await?),
            Command::FsManifest(body) => {
                CommandResult::FsManifest(self.host.fs_manifest(&body, &blobs).await?)
            }
            Command::FsMeasure(body) => {
                CommandResult::FsMeasure(self.host.fs_measure(&body).await?)
            }
            Command::FsRecord(body) => {
                CommandResult::FsRecord(self.host.fs_record(&body, &blobs).await?)
            }
            Command::FsRecordRead(body) => {
                CommandResult::FsRecordRead(self.host.fs_record_read(&body, id, &blobs).await?)
            }
            Command::FsRecordPrune(body) => {
                CommandResult::FsRecordPrune(self.host.fs_record_prune(&body).await?)
            }
            Command::FsRestorePlan(body) => {
                CommandResult::FsRestorePlan(self.host.fs_restore_plan(&body, &blobs).await?)
            }
            Command::FsRestore(body) => {
                let (progress, task) =
                    self.job_progress(id, body.environment_id, Some(body.job_id));
                let outcome = orch_envhost::snapshot::with_transfer_progress(
                    Arc::new(progress.clone()),
                    self.host.fs_restore(&body, &blobs),
                )
                .await;
                drop(progress);
                let _ = task.await;
                CommandResult::FsRestore(outcome?)
            }
            Command::WorkspaceObserve(body) => {
                CommandResult::WorkspaceObserve(self.host.workspace_observe(&body, &blobs).await?)
            }
            Command::FsRecordVerify(body) => {
                CommandResult::FsRecordVerify(self.host.fs_record_verify(&body).await?)
            }
            Command::FsDataMeasure(body) => {
                let (progress, task) = self.job_progress(
                    id,
                    body.environment_id,
                    body.work.as_ref().map(|work| work.job_id),
                );
                let outcome = orch_envhost::snapshot::with_transfer_progress(
                    Arc::new(progress.clone()),
                    self.host.fs_data_measure(&body),
                )
                .await;
                drop(progress);
                let _ = task.await;
                CommandResult::FsDataMeasure(outcome?)
            }
            Command::FsDataExport(body) => {
                let (progress, task) =
                    self.job_progress(id, body.environment_id, Some(body.job_id));
                let outcome = orch_envhost::snapshot::with_transfer_progress(
                    Arc::new(progress.clone()),
                    self.host.fs_data_export(&body, &blobs, &progress),
                )
                .await;
                drop(progress);
                let _ = task.await;
                CommandResult::FsDataExport(outcome?)
            }
            Command::FsDataImport(body) => {
                let (progress, task) =
                    self.job_progress(id, body.environment_id, Some(body.job_id));
                let outcome = orch_envhost::snapshot::with_transfer_progress(
                    Arc::new(progress.clone()),
                    self.host.fs_data_import(&body, &blobs, &progress),
                )
                .await;
                drop(progress);
                let _ = task.await;
                CommandResult::FsDataImport(outcome?)
            }
            Command::FsSnapshot(body) => {
                let (progress, task) =
                    self.job_progress(id, body.environment_id, Some(body.job_id));
                let outcome = orch_envhost::snapshot::with_transfer_progress(
                    Arc::new(progress.clone()),
                    self.host.fs_snapshot(&body, &blobs, &progress),
                )
                .await;
                drop(progress);
                let _ = task.await;
                CommandResult::FsSnapshot(outcome?)
            }
            Command::FsMaterialize(body) => {
                let (progress, task) = self.job_progress(id, body.environment_id, body.job_id);
                let outcome = orch_envhost::snapshot::with_transfer_progress(
                    Arc::new(progress.clone()),
                    self.host.fs_materialize(&body, &blobs, &progress),
                )
                .await;
                drop(progress);
                let _ = task.await;
                CommandResult::FsMaterialize(outcome?)
            }
            Command::GitStatus(body) => {
                CommandResult::GitStatus(self.host.git_status(&body).await?)
            }
            Command::GitPrepare(body) => {
                let result = if let Some(result) = self.host.git_prepare_completed(&body)? {
                    result
                } else {
                    let sealed = body
                        .credential
                        .clone()
                        .map(|credential| BTreeMap::from([("repository".to_owned(), credential)]))
                        .unwrap_or_default();
                    let opened = self.unseal(id, &sealed)?;
                    self.host
                        .git_prepare(&body, opened.get("repository").map(String::as_str))
                        .await?
                };
                CommandResult::GitPrepare(result)
            }
            Command::GitCommit(body) => {
                CommandResult::GitCommit(self.host.git_commit(&body).await?)
            }
            Command::GitWorktree(body) => {
                CommandResult::GitWorktree(self.host.git_worktree(&body).await?)
            }
            Command::GitWorktreeCleanup(body) => {
                CommandResult::GitWorktreeCleanup(self.host.git_worktree_cleanup(&body).await?)
            }
            Command::GitPrepareCleanup(body) => {
                CommandResult::GitPrepareCleanup(self.host.git_prepare_cleanup(&body).await?)
            }
            Command::GitWorktreeRepair(body) => CommandResult::GitWorktreeRepair(
                self.host.git_worktree_repair(&body, &blobs).await?,
            ),
            Command::GitWorktreeCopyPlan(body) => {
                CommandResult::GitWorktreeCopyPlan(self.host.git_worktree_copy_plan(&body).await?)
            }
            Command::GitWorktreeSnapshot(body) => {
                let (progress, task) =
                    self.job_progress(id, body.environment_id, Some(body.job_id));
                let result = self
                    .host
                    .git_worktree_snapshot(&body, &blobs, &progress)
                    .await;
                drop(progress);
                let _ = task.await;
                CommandResult::GitWorktreeSnapshot(result?)
            }
            Command::GitWorktreeStandalone(body) => {
                self.host.git_worktree_standalone(&body).await?;
                CommandResult::GitWorktreeStandalone(Empty {})
            }
            Command::GitPush(body) => {
                let sealed = body
                    .credential
                    .clone()
                    .map(|credential| BTreeMap::from([("repository".to_owned(), credential)]))
                    .unwrap_or_default();
                let opened = self.unseal(id, &sealed)?;
                CommandResult::GitPush(
                    self.host
                        .git_push(&body, opened.get("repository").map(String::as_str))
                        .await?,
                )
            }
            Command::GitDiff(body) => CommandResult::GitDiff(self.host.git_diff(&body).await?),
            Command::ProcList(body) => CommandResult::ProcList(self.host.proc_list(&body).await?),
            Command::ProcSignal(body) => {
                self.host.proc_signal(&body).await?;
                CommandResult::ProcSignal(Empty {})
            }
            Command::ProcSuspend(body) => {
                CommandResult::ProcSuspend(self.host.proc_suspend(&body).await?)
            }
            Command::ProcResume(body) => {
                CommandResult::ProcResume(self.host.proc_resume(&body).await?)
            }
            Command::WatchSet(body) => {
                self.host.watch_set(&body).await?;
                CommandResult::WatchSet(Empty {})
            }
            Command::PtyOpen(body) => CommandResult::PtyOpen(self.host.pty_open(&body).await?),
            Command::PtyClose(body) => CommandResult::PtyClose(self.host.pty_close(&body).await?),
            Command::PtyStatus(body) => CommandResult::PtyStatus(self.host.pty_status(&body)?),
            Command::ExecStart(body) => {
                CommandResult::ExecStart(self.host.exec_start(&body).await?)
            }
            Command::McpStart(body) => {
                let secrets = match &body {
                    McpStart::Given(given) => self.unseal(id, &given.secrets)?,
                    McpStart::Local(_) => BTreeMap::new(),
                };
                CommandResult::McpStart(self.host.mcp_start(&body, &secrets).await?)
            }
            Command::McpStop(body) => {
                self.host.mcp_stop(&body).await?;
                CommandResult::McpStop(Empty {})
            }
            Command::McpRestart(body) => {
                let secrets = match &body.0 {
                    McpStart::Given(given) => self.unseal(id, &given.secrets)?,
                    McpStart::Local(_) => BTreeMap::new(),
                };
                CommandResult::McpRestart(self.host.mcp_restart(&body, &secrets).await?)
            }
            Command::McpForget(body) => {
                self.host.mcp_forget(body.server_id)?;
                CommandResult::McpForget(Empty {})
            }
            Command::McpImport(body) => CommandResult::McpImport(self.imports.import(&body).await?),
            Command::PortsLook(body) => {
                CommandResult::PortsLook(self.host.ports_look(&body).await?)
            }
            other => {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    format!(
                        "{} is not carried out by the environment role",
                        other.kind()
                    ),
                ));
            }
        })
    }
}

#[async_trait]
impl RoleHandler for EnvironmentRole {
    async fn begin(&self, call: Call, command: Command) -> CommandExecution {
        let Command::FsRecord(body) = command else {
            return CommandExecution::ready(self.handle(call, command).await);
        };
        let until = self
            .services
            .clock
            .now()
            .checked_add_signed(chrono::Duration::seconds(i64::from(
                CommandKind::FsRecord.deadline_secs(),
            )));
        let Some(until) = until else {
            return CommandExecution::ready(Err(CommandError::new(
                ErrorCode::Internal,
                "the recording deadline is invalid",
            )));
        };
        let captured = self
            .host
            .fs_record_capture(
                &body,
                call.context.id,
                orch_types::Time::new(until),
                call.context.cancel,
            )
            .await;
        let captured = match captured {
            Ok(captured) => captured,
            Err(error) => return CommandExecution::ready(Err(error.into())),
        };
        let orch_envhost::file_states::CapturedState {
            result,
            input,
            hold,
        } = captured;
        let Some(input) = input else {
            return CommandExecution::ready(Ok(CommandResult::FsRecord(result)));
        };
        let blobs = self.services.content.blobs_for(call.context.id);
        CommandExecution::Captured {
            finish: Box::pin(async move {
                input.publish(&blobs).await.map_err(CommandError::from)?;
                Ok(CommandResult::FsRecord(result))
            }),
            hold: Box::new(hold),
        }
    }
    fn role(&self) -> Role {
        Role::Environment
    }

    fn takes(&self) -> &'static [CommandKind] {
        TAKES
    }

    fn claims(&self, command: &Command) -> bool {
        let storage_driver =
            orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
                env!("CARGO_PKG_VERSION"),
                Default::default(),
            );
        if let Command::NodeVolumeEnsure(body) = command {
            return body.driver == storage_driver.name;
        }
        if let Command::NodeVolumeRemove(body) = command {
            return body.driver == storage_driver.name;
        }
        !matches!(command, Command::UnitAfterUp(body) if body.environment_id.is_none() || body.provider_id.is_some())
    }

    async fn handle(&self, call: Call, command: Command) -> Result<CommandResult, CommandError> {
        orch_envhost::error::with_cancellation(call.context.cancel.clone(), async {
            if crate::reconciliation::environments::Desired::retains(&command) {
                let _action = self.lifecycle.lock().await;
                let command = self.services.policy.gate(command)?.command;
                self.carry_out(call.context.id, command).await
            } else {
                self.carry_out(call.context.id, command).await
            }
        })
        .await
    }

    fn contribute(&self, facts: &mut FactsDraft) {
        if let Some(policy) = self.services.policy.policy() {
            if facts.build_fetch.is_none() {
                facts.build_fetch=Some(orch_driver_contract::descriptor::OperationSupport {supported:false,reason:Some("this machine uses installed harnesses; downloading requested builds is not available".into())});
            }
            facts.environment_data_storage = Some(orch_driver_contract::descriptor::EnvironmentDataStorage {
                made_by_orch: (orch_envhost::RUNTIME_VERIFIED && !crate::storages::facts(&self.services.config,&policy).is_empty()).then_some(orch_driver_contract::descriptor::MadeDataKind::Directory),
                existing_directory: orch_envhost::RUNTIME_VERIFIED,
                reason: (!orch_envhost::RUNTIME_VERIFIED).then(|| "the local environment runtime has not verified directory access on this platform".into()),
            });
        }
    }

    async fn recover(&self) {
        let records = match self.desired.records() {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(%error, "saved environment services could not be reconstructed");
                return;
            }
        };
        for saved in records {
            let _action = self.lifecycle.lock().await;
            // A detach may have arrived since the startup snapshot. Its tombstone wins.
            let saved = match self.desired.current(saved.environment) {
                Ok(Some(saved)) => saved,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error, "a saved environment changed during recovery");
                    continue;
                }
            };
            if saved.generation != self.generation {
                continue;
            }
            if self.host.environment(saved.environment).is_err() {
                let attached = match self.services.policy.gate(saved.attach.command.clone()) {
                    Ok(gated) => self.carry_out(saved.attach.id, gated.command).await,
                    Err(error) => Err(error),
                };
                if attached.is_err() {
                    self.status(saved.environment, "offline", Some("the saved environment could not be restored under this machine's current policy".to_owned())).await;
                    continue;
                }
            }
            let mut failed = false;
            for (name, kept) in saved.executors {
                if self
                    .host
                    .executor_endpoint(saved.environment, &name)
                    .await
                    .is_some()
                {
                    continue;
                }
                let result = match self.services.policy.gate(kept.command) {
                    Ok(gated) => self.carry_out(kept.id, gated.command).await,
                    Err(error) => Err(error),
                };
                failed |= result.is_err();
            }
            for (name, kept) in saved.servers {
                if self
                    .host
                    .mcp_endpoint(saved.environment, &name)
                    .await
                    .is_some()
                {
                    continue;
                }
                let result = match self.services.policy.gate(kept.command) {
                    Ok(gated) => self.carry_out(kept.id, gated.command).await,
                    Err(error) => Err(error),
                };
                failed |= result.is_err();
            }
            if failed {
                self.status(saved.environment, "running", Some("an executor or MCP server could not be reconstructed under current local policy".to_owned())).await;
            }
        }
    }

    async fn stop(&self) {
        self.local_ports.stop();
        for environment in self.host.environments() {
            self.services
                .loopback
                .environment_ports(environment.id(), None);
        }
        self.stop.cancel();
        let _ = tokio::time::timeout(BEFORE_OFFLINE, self.host.before_offline()).await;
        for environment in self.host.environments() {
            let body = orch_link_contract::commands::EnvDetach {
                environment_id: environment.id(),
            };
            let _ = self.host.env_detach(&body).await;
        }
        let tasks: Vec<JoinHandle<()>> = self
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        for task in tasks {
            task.abort();
        }
    }
}

/// Puts what the host says on its own account on the stream of the environment it is about.
async fn publish_reports(
    services: Arc<Services>,
    link: MachineLink,
    host: EnvHost,
    mut queue: mpsc::Receiver<Reported>,
    stop: CancellationToken,
) {
    loop {
        let reported = tokio::select! {
            () = stop.cancelled() => return,
            reported = queue.recv() => reported,
        };
        let Some(Reported {
            environment_id,
            report,
        }) = reported
        else {
            return;
        };
        let stream = Stream::Env { environment_id };
        let at = services.clock.time();
        let durable = match report {
            HostReport::BrowserState(state) => Some(Event::BrowserState(state)),
            HostReport::DisplayState(state) => Some(Event::DisplayState(state)),
            HostReport::Facts(facts) => Some(Event::EnvFacts(*facts)),
            HostReport::Manifest(manifest) => Some(Event::EnvManifest(manifest)),
            HostReport::PtyExited(ended) => Some(Event::PtyExited(ended)),
            HostReport::FsChanged(changed) => {
                link.report(&stream, at, &LinkReport::FsChanged(changed));
                None
            }
            HostReport::GitChanged(changed) => {
                link.report(&stream, at, &LinkReport::GitChanged(changed));
                None
            }
            HostReport::Terminals(snapshot) => {
                link.report(&stream, at, &LinkReport::TerminalsSnapshot(snapshot));
                None
            }
            HostReport::Procs { as_of, processes } => {
                link.report(
                    &stream,
                    at,
                    &LinkReport::ProcsSnapshot(ProcsSnapshot { as_of, processes }),
                );
                None
            }
            HostReport::Stats(sample) => {
                link.report(&stream, at, &LinkReport::StatsSample(sample));
                None
            }
            HostReport::Ports(snapshot) => {
                link.report(&stream, at, &LinkReport::PortsSnapshot(snapshot));
                None
            }
            HostReport::McpFailed { name, reason } => {
                let detail = format!("the tool server {name} failed: {reason}");
                server_changed(&services, &host, environment_id, Some(detail));
                None
            }
            HostReport::McpRestarted { name } => {
                tracing::info!(server = %name, "a tool server runs again");
                server_changed(&services, &host, environment_id, None);
                None
            }
        };
        if let Some(event) = durable
            && let Err(error) = services.publisher.publish(stream, event).await
        {
            tracing::warn!(%error, "a report of an environment could not be put in the spool");
        }
    }
}

/// What a tool server that was given up on, or runs again, comes to for the system: the facts of the
/// environment are read again (the offers that need a tool server follow the servers), and the status keeps
/// its word, `running`, with a detail that names what failed. Done on a task of its own, since the queue the
/// host's reports wait in is the one being read.
fn server_changed(
    _services: &Arc<Services>,
    host: &EnvHost,
    environment_id: Id,
    _detail: Option<String>,
) {
    let host = host.clone();
    tokio::spawn(async move {
        let _ = host.facts_changed(environment_id).await;
        // The ordinary facts report carries the per-server state; no environment lifecycle changes here.
    });
}

/// Gives the host the limits of the machine again whenever its owner, or the system for the keys handed
/// over, changes them. A shell that the policy no longer allows is closed.
async fn follow_policy(role: Weak<EnvironmentRole>, stop: CancellationToken) {
    let Some(mut changes) = role.upgrade().map(|role| role.services.policy.changes()) else {
        return;
    };
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            changed = changes.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
        let Some(role) = role.upgrade() else {
            return;
        };
        let Some(policy) = role.services.policy.policy() else {
            continue;
        };
        role.host.set_policy(
            gates_of(&policy),
            Some(roots_of(&policy, &role.services.data_dir)),
            Some(policy.exec_offers.clone()),
            mcp_gates_of(&policy),
        );
        if !policy.environment_terminal {
            for environment in role.host.environments() {
                let _ = environment.terminals().close_all().await;
            }
        }
    }
}

/// Looks again, every so often, at the tool servers the harnesses here have configured, for each
/// environment that is attached.
async fn look_for_tool_servers(role: Weak<EnvironmentRole>, stop: CancellationToken) {
    let mut ticker = tokio::time::interval(DISCOVERY_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let Some(role) = role.upgrade() else {
            return;
        };
        for environment in role.host.environments() {
            let _ = role.imports.rediscover(environment.id()).await;
        }
    }
}

/// The tunnels of the kinds the environment role serves, as the connector's registry hands them over.
struct EnvironmentTunnels(Weak<EnvironmentRole>);

#[async_trait]
impl TunnelServer for EnvironmentTunnels {
    async fn serve(&self, tunnel: IncomingTunnel) {
        match self.0.upgrade() {
            Some(role) => role.serve_tunnel(tunnel).await,
            None => tunnel.refuse(OpenErrorCode::PeerOffline, "this connector is stopping"),
        }
    }
}

/// What the environment role and the tools of the harnesses here add to a connector.
pub struct Joined {
    /// The role, when the machine plays it.
    pub role: Option<Arc<EnvironmentRole>>,
    /// The addresses a harness here reaches its tools at.
    pub endpoints: Vec<(EndpointKind, Arc<dyn Endpoint>)>,
}

/// Makes what a connector needs for the environment role and for the harnesses it hosts to reach their
/// tools: the role when the machine plays it (`parts` is then given), and the endpoints of the listener
/// when it hosts harnesses (`hosts_harnesses`).
#[must_use]
pub fn join(
    services: &Arc<Services>,
    link: &MachineLink,
    hosting: Option<Arc<Hosting>>,
    parts: Option<&EnvironmentParts>,
    hosts_harnesses: bool,
) -> Joined {
    let role = parts.map(|parts| EnvironmentRole::start(services, link, hosting, parts));
    let mut endpoints: Vec<(EndpointKind, Arc<dyn Endpoint>)> = Vec::new();
    if hosts_harnesses {
        let relay = role.as_ref().map_or_else(
            || ToolRelay::new(services, link, None),
            |role| Arc::clone(role.relay()),
        );
        endpoints.push((
            EndpointKind::Exec,
            Arc::new(ExecEndpoint::new(Arc::clone(&relay))),
        ));
        endpoints.push((EndpointKind::AnyMcp, Arc::new(McpEndpoint::new(relay))));
    }
    Joined { role, endpoints }
}

// Browser command refusals carry the host's typed cause, with no page content.
fn browser_error(error: orch_envhost::browsers::BrowserError) -> CommandError {
    CommandError {
        code: error.code().to_owned(),
        detail: error.to_string(),
        retryable: false,
        details: None,
    }
}

fn display_error(error: orch_envhost::displays::DisplayError) -> CommandError {
    CommandError {
        code: error.code().to_owned(),
        detail: error.to_string(),
        retryable: false,
        details: None,
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[tokio::test]
    async fn a_million_updates_keep_one_pending_sample_and_flush_the_last_on_close() {
        let (queue, mut reports) = watch::channel(None);
        let progress = JobProgressSink::new(Some(queue), Arc::new(orch_types::SystemClock));
        for done in 1..=1_000_000 {
            progress.counting(done, done * 4096, done / 10);
        }
        assert!(reports.changed().await.is_ok());
        {
            let latest = reports.borrow_and_update();
            let Some(latest) = latest.as_ref() else {
                panic!("a sample must remain while the consumer is paused");
            };
            assert_eq!(latest.files_done, 1_000_000);
            assert_eq!(latest.bytes_done, 4_096_000_000);
        }
        assert!(
            reports.has_changed().is_ok_and(|changed| !changed),
            "no stale sample waits behind the latest one"
        );
        progress.counting(1_000_001, 4_096_004_096, 100_000);
        drop(progress);
        assert!(
            reports.changed().await.is_ok(),
            "closing must retain the unseen final sample"
        );
        assert_eq!(
            reports.borrow_and_update().as_ref().map(|p| p.files_done),
            Some(1_000_001)
        );
        assert!(reports.changed().await.is_err());
    }
}
