//! The node's role: every `node.*` command carried out through the drivers, and the events the node publishes.
//!
//! The node is a machine like a connector: it enrolls with a host token, holds a session, takes commands one
//! at a time by their serial key, journals them and publishes events. All of that is the connector's, and a
//! node is a connector that plays the role `node`. This module is that role ([`NodeRole`]): it is given to
//! the connector's builder, and it carries out the ten commands of the role.
//!
//! | Command | Carried out by |
//! |---------|----------------|
//! | `node.runtime.create`, `.start`, `.stop`, `.remove`, `.export`, `.import` | [`crate::runtimes`] |
//! | `node.volume.ensure`, `.remove` | [`crate::volumes`] |
//! | `node.image.ensure` | [`crate::images`] |
//! | `node.template.import` | the driver, here |
//! | `node.tool.ensure` | [`crate::tools`] |
//! | `node.tool.check` | [`crate::tool_checks`] |
//! | `node.resources.list`, `.remove` | [`crate::resources`] |
//! | `node.probe` | [`crate::facts`], [`crate::reconciliation`] |
//!
//! **What runs beside the commands**, from the moment the connector has started: every `[node] reconcile_secs`
//! the node tries again the drivers that could not be loaded, tells the host's facts if they changed, and
//! reconciles its units with the drivers' ([`crate::reconciliation`]); every few seconds it looks at what the
//! host uses and tells it as `node.load` asks; every `[node] image_check_hours` it asks the registries what is
//! behind the tags in use. All of it stops with the role.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use orch_connector_core::roles::{Call, Keys, RoleHandler, Services};
use orch_driver_contract::imports::{ImportRequest, NoteLevel as DriverNoteLevel};
use orch_driver_contract::runtime::RuntimeKind as DriverKind;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode, NodeTemplateImport,
    NodeTemplateImportNote, NodeTemplateImportResult, NodeToolEnsureResult, NoteLevel, RuntimeKind,
};
use orch_link_contract::session::Role;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::drivers::{DriverHost, failure};
use crate::facts::{Facts, LOAD_LOOK_EVERY, LoadSampler, LoadWatch, Reports, capacity};
use crate::images::{BlobDesk, Images};
use crate::ledger::{Ledger, LedgerError};
use crate::reconciliation::Reconciler;
use crate::runtimes::{Busy, RuntimeParts, Runtimes};
use crate::settings::NodeSettings;
use crate::storages::Storages;
use crate::tools::{ToolCache, ToolError};
use crate::volumes::Volumes;

/// The kinds of command the role takes.
pub const NODE_KINDS: &[CommandKind] = &[
    CommandKind::NodeResourcesList,
    CommandKind::NodeResourcesRemove,
    CommandKind::NodeRuntimeCreate,
    CommandKind::NodeRuntimeStart,
    CommandKind::NodeRuntimeStop,
    CommandKind::NodeRuntimeRemove,
    CommandKind::NodeNetworkApply,
    CommandKind::NodeMountsApply,
    CommandKind::NodeCapabilitiesApply,
    CommandKind::NodeResourcesApply,
    CommandKind::NodeResourcesInspect,
    CommandKind::NodeKeepEstimate,
    CommandKind::NodeKeepFiles,
    CommandKind::NodeKeptRemove,
    CommandKind::NodeKeptExport,
    CommandKind::NodeKeptImport,
    CommandKind::NodeKeptSweep,
    CommandKind::NodeVolumeEnsure,
    CommandKind::NodeVolumeInspect,
    CommandKind::NodeVolumeRemove,
    CommandKind::NodeRuntimeExport,
    CommandKind::NodeRuntimeImport,
    CommandKind::NodeRuntimeVolumesMeasure,
    CommandKind::NodeRuntimeVolumesExport,
    CommandKind::NodeRuntimeVolumeImport,
    CommandKind::NodeVolumeCarryCheck,
    CommandKind::NodeMountMeasure,
    CommandKind::NodeMountFingerprint,
    CommandKind::NodeMountExport,
    CommandKind::NodeMountImport,
    CommandKind::NodeImageEnsure,
    CommandKind::NodeTemplateImport,
    CommandKind::NodeToolEnsure,
    CommandKind::NodeToolCheck,
    CommandKind::NodeToolConformance,
    CommandKind::NodeSkillFetch,
    CommandKind::NodeProbe,
];

/// Why a node's files could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    /// The canonical machine policy could not be read or imported.
    #[error("{0}")]
    Policy(#[from] crate::machine_policy::PolicyOpenError),
    /// The ledger of the runtimes is damaged or cannot be read.
    #[error("{0}")]
    Ledger(#[from] LedgerError),
    /// The tool cache cannot be made.
    #[error("{0}")]
    Tools(#[from] ToolError),
}

/// What of a node can be made before the connector is: the files it keeps.
#[derive(Debug, Clone)]
pub struct Prepared {
    settings: NodeSettings,
    ledger: Arc<Ledger>,
    tools: Arc<ToolCache>,
}

/// Makes a role from the connector's services, as the connector's builder takes it.
pub type RoleFactory = Box<dyn FnOnce(&Arc<Services>) -> Arc<dyn RoleHandler> + Send>;

impl Prepared {
    /// The operational node file, distinct from its shared policy/identity file.
    #[must_use]
    pub fn config_file(&self) -> Option<&std::path::Path> {
        self.settings.config_file.as_deref()
    }
    /// Opens the ledger and the tool cache in the node's data directory, making them if they are not there.
    pub fn open(settings: &NodeSettings) -> Result<Self, PrepareError> {
        let mut settings = settings.clone();
        crate::machine_policy::configure(&mut settings)?;
        std::fs::create_dir_all(settings.data_dir.join("state")).map_err(|source| {
            PrepareError::Ledger(LedgerError::Io {
                path: settings.data_dir.clone(),
                source,
            })
        })?;
        Ok(Self {
            settings: settings.clone(),
            ledger: Arc::new(Ledger::open(&settings.data_dir)?),
            tools: Arc::new(ToolCache::open(&settings)?),
        })
    }

    /// The node’s operational address settings.
    #[must_use]
    pub fn settings(&self) -> &NodeSettings {
        &self.settings
    }

    /// The tool cache.
    #[must_use]
    pub fn tools(&self) -> &Arc<ToolCache> {
        &self.tools
    }

    /// The ledger.
    #[must_use]
    pub fn ledger(&self) -> &Arc<Ledger> {
        &self.ledger
    }

    /// The factory the connector's builder takes: it makes the role from the connector's services.
    #[must_use]
    pub fn role(self, drivers: Arc<DriverHost>, desk: Arc<BlobDesk>) -> RoleFactory {
        Box::new(move |services| {
            let role: Arc<dyn RoleHandler> = Arc::new(NodeRole::new(services, self, drivers, desk));
            role
        })
    }
}

/// Everything the role does its work with, shared with what runs beside the commands.
struct Inner {
    clock: Arc<dyn orch_types::Clock>,
    settings: NodeSettings,
    drivers: Arc<DriverHost>,
    ledger: Arc<Ledger>,
    tools: Arc<ToolCache>,
    volumes: Arc<Volumes>,
    images: Arc<Images>,
    runtimes: Runtimes,
    facts: Arc<Facts>,
    reconciler: Arc<Reconciler>,
    storages: Arc<Storages>,
    reports: Arc<Reports>,
    /// The host openings the node holds.
    publisher: Arc<crate::host_publish::Publisher>,
    /// What moves content between the host and the server.
    content: Arc<orch_connector_core::content::Content>,
    /// What fetches skills' files for the server.
    skills: Option<crate::skill_fetch::SkillFetch>,
    /// Opens the source address sealed to this machine for a fetch command.
    keys: Arc<Keys>,
    link_connected: std::sync::atomic::AtomicBool,
    stop: CancellationToken,
}

/// The node's role.
pub struct NodeRole {
    inner: Arc<Inner>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeRole").finish_non_exhaustive()
    }
}

impl NodeRole {
    /// The role of a node whose files are open, whose drivers are loaded, and which the connector's
    /// `services` serve.
    #[must_use]
    pub fn new(
        services: &Arc<Services>,
        prepared: Prepared,
        drivers: Arc<DriverHost>,
        desk: Arc<BlobDesk>,
    ) -> Self {
        let Prepared {
            settings,
            ledger,
            tools,
        } = prepared;
        let tools = Arc::new(
            tools
                .with_policy(Arc::clone(&services.policy))
                .with_link(services.link.clone(), Arc::clone(&services.clock)),
        );
        for (_, driver) in drivers.all() {
            driver.contract().control_link(false);
        }
        desk.attach(Arc::clone(&services.content));
        let reports = Arc::new(Reports::new(Arc::clone(&services.publisher)));
        let storages = Arc::new(Storages::new(&settings, Arc::clone(&drivers)));
        let volumes = Arc::new(Volumes::new(
            Arc::clone(&drivers),
            Arc::clone(&storages),
            Arc::clone(&ledger),
        ));
        let images = Arc::new(Images::new(
            Arc::clone(&drivers),
            Arc::clone(&ledger),
            Arc::clone(&reports),
            desk,
        ));
        let busy = Busy::new();
        let publisher = Arc::clone(&services.port_publisher);
        let published: Arc<dyn crate::hooks::PublishedPorts> = publisher.clone();
        let runtimes = Runtimes::new(RuntimeParts {
            drivers: Arc::clone(&drivers),
            ledger: Arc::clone(&ledger),
            volumes: Arc::clone(&volumes),
            storages: Arc::clone(&storages),
            images: Arc::clone(&images),
            tools: Arc::clone(&tools),
            reports: Arc::clone(&reports),
            keys: Arc::clone(&services.keys),
            clock: Arc::clone(&services.clock),
            settings: settings.clone(),
            busy: Arc::clone(&busy),
            ports: Some(published),
            content: Some(Arc::clone(&services.content)),
        });
        runtimes.set_policy(Arc::clone(&services.policy));
        runtimes.set_link(services.link.clone());
        images.set_policy(Arc::clone(&services.policy));
        volumes.set_policy(Arc::clone(&services.policy));
        let facts = Arc::new(Facts::new(
            Arc::clone(&drivers),
            Arc::clone(&storages),
            Arc::clone(&tools),
            Arc::clone(&reports),
        ));
        let reconciler = Arc::new(Reconciler::new(
            Arc::clone(&drivers),
            Arc::clone(&ledger),
            Arc::clone(&reports),
            Arc::clone(&images),
            busy,
            &settings,
        ));
        Self {
            inner: Arc::new(Inner {
                clock: Arc::clone(&services.clock),
                settings,
                drivers,
                ledger,
                tools,
                volumes,
                images,
                runtimes,
                facts,
                reconciler,
                storages,
                reports,
                publisher,
                content: Arc::clone(&services.content),
                skills: crate::skill_fetch::SkillFetch::new(Some(Arc::clone(&services.policy)))
                    .ok(),
                keys: Arc::clone(&services.keys),
                link_connected: std::sync::atomic::AtomicBool::new(false),
                stop: CancellationToken::new(),
            }),
            tasks: Mutex::new(Vec::new()),
        }
    }

    /// What the node recorded of its runtimes.
    #[must_use]
    pub fn ledger(&self) -> &Arc<Ledger> {
        &self.inner.ledger
    }

    /// The runtimes of the node.
    #[must_use]
    pub fn runtimes(&self) -> &Runtimes {
        &self.inner.runtimes
    }

    /// The reconciliation, which a test can run a pass of.
    #[must_use]
    pub fn reconciler(&self) -> &Arc<Reconciler> {
        &self.inner.reconciler
    }

    /// The facts of the host.
    #[must_use]
    pub fn facts(&self) -> &Arc<Facts> {
        &self.inner.facts
    }

    /// The images of the host.
    #[must_use]
    pub fn images(&self) -> &Arc<Images> {
        &self.inner.images
    }

    /// One turn of what the node does on its interval: the drivers that could not be loaded are tried again,
    /// the facts are told if they changed, and the units are reconciled.
    pub async fn tick(&self) {
        self.inner.tick().await;
    }

    fn spawn(&self, work: impl std::future::Future<Output = ()> + Send + 'static) {
        self.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tokio::spawn(work));
    }
}

impl Inner {
    async fn tick(&self) {
        let loaded = self.drivers.retry().await;
        if !loaded.is_empty() {
            tracing::info!(drivers = ?loaded, "drivers that could not be loaded are loaded now");
        }
        for name in loaded {
            if let Some(driver) = self.drivers.get(&name) {
                driver.contract().control_link(false);
                driver.contract().control_link(
                    self.link_connected
                        .load(std::sync::atomic::Ordering::SeqCst),
                );
            }
        }
        self.facts.report_if_changed().await;
        self.reconciler.pass().await;
        self.runtimes.report_networks().await;
        self.runtimes.recover_volume_copy_holds().await;
        // A unit that came up and ended by itself is given the operator's post_down.
        self.runtimes.settle_hooks().await;
        self.runtimes.reconcile_live_mounts().await;
        self.runtimes.reconcile_live_capabilities().await;
    }

    /// Asks the registries what is behind the tags in use, and tells the system which units run an image that
    /// has fallen behind.
    async fn look_at_images(&self) {
        for behind in self.images.look().await {
            tracing::info!(image = %behind.key, "{}", behind.why);
        }
        self.reconciler.pass().await;
    }

    async fn template_import(
        &self,
        body: NodeTemplateImport,
    ) -> Result<NodeTemplateImportResult, CommandError> {
        let host = self.drivers.get(&body.driver).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                format!("this host has no driver {:?}", body.driver),
            )
        })?;
        if host.describe().import(&body.format).is_none() {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                format!(
                    "the driver {} reads no import of the format {:?}",
                    body.driver, body.format
                ),
            ));
        }
        let proposal = host
            .contract()
            .import(&ImportRequest {
                kind: match body.kind {
                    RuntimeKind::Environment => DriverKind::Environment,
                    RuntimeKind::Provider => DriverKind::Provider,
                },
                format: body.format,
                values: body.values,
            })
            .await
            .map_err(|error| failure(&error))?;
        Ok(NodeTemplateImportResult {
            spec: proposal.spec,
            notes: proposal
                .notes
                .into_iter()
                .map(|note| NodeTemplateImportNote {
                    level: match note.level {
                        DriverNoteLevel::Info => NoteLevel::Info,
                        DriverNoteLevel::Warning => NoteLevel::Warning,
                        DriverNoteLevel::Blocker => NoteLevel::Blocker,
                    },
                    text: note.text,
                })
                .collect(),
        })
    }
}

#[async_trait]
impl RoleHandler for NodeRole {
    async fn interrupted(&self, id: orch_types::Id, command: &Command) -> Result<(), CommandError> {
        let driver = match command {
            Command::NodeMountMeasure(body) => Some(body.driver.clone()),
            Command::NodeMountFingerprint(body) => Some(body.driver.clone()),
            Command::NodeMountExport(body) => Some(body.driver.clone()),
            Command::NodeMountImport(body) => Some(body.driver.clone()),
            Command::NodeRuntimeVolumeImport(body) => Some(body.driver.clone()),
            Command::NodeRuntimeVolumesMeasure(body) => self
                .inner
                .ledger
                .get(body.runtime_id)
                .map(|record| record.driver),
            Command::NodeRuntimeVolumesExport(body) => self
                .inner
                .ledger
                .get(body.runtime_id)
                .map(|record| record.driver),
            _ => return Ok(()),
        }
        .ok_or_else(|| {
            CommandError::new(
                ErrorCode::Conflict,
                "the interrupted volume command's driver is unavailable",
            )
        })?;
        let driver = self.inner.drivers.get(&driver).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Internal,
                "the interrupted volume command's driver is unavailable",
            )
        })?;
        driver
            .extras()
            .stop_volume_work(id)
            .await
            .map_err(|error| failure(&error))
    }
    fn claims(&self, command: &Command) -> bool {
        match command {
            Command::NodeVolumeEnsure(body) => self.inner.drivers.get(&body.driver).is_some(),
            Command::NodeVolumeRemove(body) => self.inner.drivers.get(&body.driver).is_some(),
            _ => true,
        }
    }
    fn contribute(&self, facts: &mut orch_connector_core::facts::FactsDraft) {
        facts.build_fetch = Some(orch_driver_contract::descriptor::OperationSupport {
            supported: true,
            reason: None,
        });
        // A node reports its actual driver storages through the same machine facts as a directory role.
        facts.storages = self.inner.facts.current().storages;
    }

    fn role(&self) -> Role {
        Role::Node
    }

    fn takes(&self) -> &'static [CommandKind] {
        NODE_KINDS
    }

    async fn handle(&self, call: Call, command: Command) -> Result<CommandResult, CommandError> {
        let node = &self.inner;
        // Volume and filesystem work can create helpers too; none may race cache removal.
        let _resources = if matches!(&command, Command::NodeResourcesRemove(_)) {
            None
        } else {
            Some(node.runtimes.hold_resources().await)
        };
        let id = call.context.id;
        let cancel = &call.context.cancel;
        let report = match &command {
            Command::NodeMountMeasure(body) => body.work.clone(),
            Command::NodeMountFingerprint(body) => body.work.clone(),
            Command::NodeMountExport(body) => body.work.clone(),
            Command::NodeMountImport(body) => body.work.clone(),
            Command::NodeRuntimeVolumesMeasure(body) => body.work.clone(),
            Command::NodeRuntimeVolumesExport(body) => body.work.clone(),
            Command::NodeRuntimeVolumeImport(body) => body.work.clone(),
            _ => None,
        };
        let (work, reporting) = crate::work::start(
            Arc::clone(&node.reports),
            id,
            Arc::clone(&node.clock),
            report,
            cancel.clone(),
            u64::from(command.kind().deadline_secs()),
        );
        let outcome = match command {
            Command::NodeResourcesList(_) => node
                .runtimes
                .resources()
                .await
                .map(CommandResult::NodeResourcesList),
            Command::NodeResourcesRemove(body) => node
                .runtimes
                .remove_resource(&body)
                .await
                .map(|()| CommandResult::NodeResourcesRemove(Empty {})),
            Command::NodeRuntimeCreate(body) => {
                let body = serde_json::to_value(body.0)
                    .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
                let made = node.runtimes.create(id, body, cancel).await;
                node.images.finish(id).await;
                made.map(CommandResult::NodeRuntimeCreate)
            }
            Command::NodeRuntimeStart(body) => node
                .runtimes
                .start(body.runtime_id)
                .await
                .map(|()| CommandResult::NodeRuntimeStart(Empty {})),
            Command::NodeRuntimeStop(body) => node
                .runtimes
                .stop(body.runtime_id, body.reason)
                .await
                .map(|()| CommandResult::NodeRuntimeStop(Empty {})),
            Command::NodeNetworkApply(body) => node
                .runtimes
                .apply_network(&body)
                .await
                .map(CommandResult::NodeNetworkApply),
            Command::NodeMountsApply(body) => node
                .runtimes
                .apply_mounts(&body)
                .await
                .map(CommandResult::NodeMountsApply),
            Command::NodeCapabilitiesApply(body) => node
                .runtimes
                .apply_capabilities(&body)
                .await
                .map(CommandResult::NodeCapabilitiesApply),
            Command::NodeResourcesApply(body) => node
                .runtimes
                .apply_resources(&body)
                .await
                .map(|()| CommandResult::NodeResourcesApply(Empty {})),
            Command::NodeResourcesInspect(body) => node
                .runtimes
                .inspect_resources(&body)
                .await
                .map(CommandResult::NodeResourcesInspect),
            Command::NodeKeepEstimate(body) => node
                .runtimes
                .estimate_kept_files(body.runtime_id)
                .await
                .map(CommandResult::NodeKeepEstimate),
            Command::NodeKeepFiles(body) => node
                .runtimes
                .keep_unit_files(body)
                .await
                .map(CommandResult::NodeKeepFiles),
            Command::NodeKeptRemove(body) => node
                .runtimes
                .remove_kept_image(&body)
                .await
                .map(|()| CommandResult::NodeKeptRemove(Empty {})),
            Command::NodeKeptExport(body) => node
                .runtimes
                .export_kept_image(id, &body)
                .await
                .map(CommandResult::NodeKeptExport),
            Command::NodeKeptImport(body) => node
                .runtimes
                .import_kept_image(id, &body)
                .await
                .map(|()| CommandResult::NodeKeptImport(Empty {})),
            Command::NodeKeptSweep(body) => node
                .runtimes
                .sweep_kept_images(&body)
                .await
                .map(|()| CommandResult::NodeKeptSweep(Empty {})),
            Command::NodeRuntimeRemove(body) => node
                .runtimes
                .remove_retained(&body)
                .await
                .map(|()| CommandResult::NodeRuntimeRemove(Empty {})),
            Command::NodeVolumeCarryCheck(body) => Ok(CommandResult::NodeVolumeCarryCheck(
                node.runtimes.check_volume_carry(&body).await,
            )),
            Command::NodeRuntimeVolumesMeasure(body) => node
                .runtimes
                .measure_volumes_reporting(&body, &work)
                .await
                .map(CommandResult::NodeRuntimeVolumesMeasure),
            Command::NodeMountMeasure(body) => node
                .runtimes
                .measure_mount_reporting(&body, &work)
                .await
                .map(CommandResult::NodeMountMeasure),
            Command::NodeMountFingerprint(body) => node
                .runtimes
                .fingerprint_mount_reporting(&body, &work)
                .await
                .map(CommandResult::NodeMountFingerprint),
            Command::NodeMountExport(body) => node
                .runtimes
                .export_mount_reporting(id, body, Arc::clone(&work))
                .await
                .map(CommandResult::NodeMountExport),
            Command::NodeMountImport(body) => node
                .runtimes
                .import_mount_reporting(id, body, Arc::clone(&work))
                .await
                .map(|()| CommandResult::NodeMountImport(Empty {})),
            Command::NodeRuntimeVolumesExport(body) => node
                .runtimes
                .export_volumes_reporting(id, body, Arc::clone(&work))
                .await
                .map(CommandResult::NodeRuntimeVolumesExport),
            Command::NodeRuntimeVolumeImport(body) => node
                .runtimes
                .import_volume_reporting(id, body, Arc::clone(&work))
                .await
                .map(|()| CommandResult::NodeRuntimeVolumeImport(Empty {})),
            Command::NodeRuntimeExport(body) => node
                .runtimes
                .export(id, &body)
                .await
                .map(CommandResult::NodeRuntimeExport),
            Command::NodeRuntimeImport(body) => node
                .runtimes
                .import(id, &body)
                .await
                .map(|()| CommandResult::NodeRuntimeImport(Empty {})),
            Command::NodeVolumeEnsure(mut body) => {
                body.labels.insert(
                    orch_driver_contract::runtime::LABEL_SYSTEM.into(),
                    node.settings.system_name.clone(),
                );
                node.volumes
                    .ensure(&body)
                    .await
                    .map(|()| CommandResult::NodeVolumeEnsure(Empty {}))
            }
            Command::NodeVolumeInspect(body) => node
                .volumes
                .inspect(&body, &node.settings.system_name)
                .await
                .map(CommandResult::NodeVolumeInspect),
            Command::NodeVolumeRemove(body) => node
                .volumes
                .remove(&body, &node.settings.system_name)
                .await
                .map(|()| CommandResult::NodeVolumeRemove(Empty {})),
            Command::NodeImageEnsure(body) => {
                let _resources = node.runtimes.hold_resources().await;
                let ensured = node
                    .images
                    .ensure(id, &body.driver, &body.spec, body.job_id, cancel)
                    .await;
                node.images.finish(id).await;
                if let Ok(ensured) = &ensured
                    && let Some(digest) = &ensured.digest
                {
                    crate::resources::image_used(
                        &node.drivers,
                        &node.tools,
                        &node.ledger,
                        &body.driver,
                        digest,
                        node.clock.time(),
                    )
                    .await?;
                }
                ensured.map(CommandResult::NodeImageEnsure)
            }
            Command::NodeTemplateImport(body) => node
                .template_import(body)
                .await
                .map(CommandResult::NodeTemplateImport),
            Command::NodeToolEnsure(body) => {
                let _resources = node.runtimes.hold_resources().await;
                let path = node
                    .tools
                    .ensure_reporting(&body, cancel, &node.reports)
                    .await?;
                crate::resources::tool_used(
                    &node.ledger,
                    &body.name,
                    &body.version,
                    node.clock.time(),
                )?;
                Ok(CommandResult::NodeToolEnsure(NodeToolEnsureResult {
                    path: path.display().to_string(),
                }))
            }
            Command::NodeToolCheck(body) => {
                let _resources = node.runtimes.hold_resources().await;
                crate::resources::tool_used(
                    &node.ledger,
                    &body.name,
                    &body.version,
                    node.clock.time(),
                )?;
                crate::tool_checks::check(&node.tools, &body, cancel)
                    .await
                    .map(CommandResult::NodeToolCheck)
                    .map_err(CommandError::from)
            }
            Command::NodeToolConformance(body) => {
                let _resources = node.runtimes.hold_resources().await;
                crate::resources::tool_used(
                    &node.ledger,
                    &body.name,
                    &body.version,
                    node.clock.time(),
                )?;
                crate::tool_checks::conformance(&node.tools, &node.drivers, &body, cancel)
                    .await
                    .map(CommandResult::NodeToolConformance)
            }
            Command::NodeSkillFetch(body) => {
                let fetch = node.skills.as_ref().ok_or_else(|| {
                    CommandError::new(ErrorCode::Unsupported, "this node fetches no skill")
                })?;
                fetch
                    .fetch(&node.content, &node.keys, id, &body, cancel)
                    .await
                    .map(CommandResult::NodeSkillFetch)
            }
            Command::NodeProbe(_) => Ok(CommandResult::NodeProbe(
                node.facts.probe(node.reconciler.units().await),
            )),
            other => Err(CommandError::new(
                ErrorCode::Unsupported,
                format!("{} is not a command of a node", other.kind()),
            )),
        };
        drop(work);
        let _ = reporting.await;
        outcome
    }

    async fn recover(&self) {
        let node = &self.inner;
        node.runtimes.recover_volume_copy_holds().await;
        node.runtimes.rewatch();
        let me = Arc::clone(node);
        self.spawn(async move {
            let mut ticker=tokio::time::interval(std::time::Duration::from_secs(3600));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {tokio::select! {()=me.stop.cancelled()=>return,_=ticker.tick()=>me.runtimes.report_kept_images().await}}
        });
        // The operator's hooks again for the units found running (they must be idempotent), beside the rest.
        let runtimes = node.runtimes.clone();
        self.spawn(async move { runtimes.replay_hooks().await });
        // The facts and the reconciliation, at once and then on the interval.
        let (me, every) = (Arc::clone(node), node.settings.reconcile_every);
        self.spawn(async move {
            let mut ticker = tokio::time::interval(every);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    () = me.stop.cancelled() => return,
                    _ = ticker.tick() => me.tick().await,
                }
            }
        });
        // What the host uses.
        let me = Arc::clone(node);
        self.spawn(async move {
            let (mut sampler, mut watch) = (LoadSampler::new(), LoadWatch::new());
            let mut ticker = tokio::time::interval(LOAD_LOOK_EVERY);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    () = me.stop.cancelled() => return,
                    _ = ticker.tick() => {}
                }
                let Some(load) = sampler.sample(&me.storages) else {
                    continue;
                };
                if watch.due(load, &capacity(&me.storages), tokio::time::Instant::now()) {
                    me.reports.load(load).await;
                }
            }
        });
        // The tags in use.
        let every = node.settings.image_check_every;
        if !every.is_zero() {
            let me = Arc::clone(node);
            self.spawn(async move {
                loop {
                    tokio::select! {
                        () = me.stop.cancelled() => return,
                        () = tokio::time::sleep(every) => me.look_at_images().await,
                    }
                }
            });
        }
    }

    async fn connected(&self, _session: orch_types::Id) {
        self.inner
            .link_connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for (_, driver) in self.inner.drivers.all() {
            driver
                .contract()
                .control_endpoints(self.inner.runtimes.gateway_addresses());
            driver.contract().control_link(true);
        }
        self.inner.runtimes.refresh_control_addresses().await;
        // A full host report asks the system to reconcile every runtime, including those without a connector.
        self.inner.reports.facts(self.inner.facts.current()).await;
    }

    async fn disconnected(&self) {
        self.inner
            .link_connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        for (_, driver) in self.inner.drivers.all() {
            driver.contract().control_link(false);
        }
    }

    async fn stop(&self) {
        self.disconnected().await;
        self.inner.publisher.close_all().await;
        self.inner.stop.cancel();
        self.inner.runtimes.stop_watching();
        self.inner.runtimes.stop_copy_work().await;
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

    fn abandon(&self) {
        self.inner.stop.cancel();
        self.inner.runtimes.abandon_watches();
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
