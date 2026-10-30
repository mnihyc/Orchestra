//! A Kubernetes driver composed by the node through the ordinary runtime contract.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{Namespace, Node, PersistentVolumeClaim, Pod};
use kube::Api;
use kube::api::{AttachParams, DeleteParams, ListParams, Preconditions};
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::mounts::AttachedMount;
use orch_driver_contract::runtime::{LABEL_SYSTEM, RuntimeSpec};
use orch_driver_contract::units::{UnitRef, UnitState, UnitStateKind};
use orch_driver_contract::volumes::{VolumeIdentity, VolumeRequest};
use orch_types::{Hash, Json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

use crate::authenticated_control;
use crate::client::Cluster;
use crate::config::{KubernetesConfig, RuntimeBoundary};
use crate::control_protocol::{self, GuardPhase, GuardReply, GuardRequest};
use crate::encrypted_transport;
use crate::locks::KeyedLocks;
use crate::pod_identity;
use crate::receipts::{ReceiptStore, UnitRecord};
use crate::stage_protocol::{self, StageHeader};
use crate::{
    admission, claim_service, class_limits, launch, platform, pod_read, stage_exchange, staging,
    template_schema, unit_create, verify_admission,
};

// One cluster call: a loaded API server and a nested test cluster both answer in seconds, not milliseconds.
const REQUEST: Duration = Duration::from_secs(30);
// A start, stop or inspection (an authenticated exchange with the unit's guard) is held to its node command's
// 120 seconds; its grace must stay under this.
const CONTROL: Duration = Duration::from_secs(90);
// A create waits for the cluster to pull images and place the Pod, and for the node's tools to be staged into it:
// the connector and every harness build the template asks for, hundreds of megabytes through the cluster's exec stream.
// The node command allows 600 seconds; this leaves it half a minute for what follows.
const CREATE: Duration = Duration::from_secs(570);
const REFRESH: Duration = Duration::from_secs(30);
// A trusted helper writes only fixed refusals to its error stream; this much of it is kept for the refusal.
const SAID_BYTES: u64 = 2048;
const PAGE: u32 = 100;
const HELPER: &str = "/usr/local/bin/orch-kube-helper";

fn unavailable() -> DriverError {
    DriverError::Unavailable("the original Kubernetes unit cannot be verified".into())
}

/// Names the step of a lifecycle operation that refused, so that a refusal says where it happened. The
/// step names are fixed words; nothing of the unit's data enters them.
fn at<T>(step: &str, result: Result<T, DriverError>) -> Result<T, DriverError> {
    result.map_err(|error| match error {
        DriverError::Unavailable(message) => DriverError::Unavailable(format!("{step}: {message}")),
        DriverError::InvalidSpec(message) => DriverError::InvalidSpec(format!("{step}: {message}")),
        DriverError::Internal(message) => DriverError::Internal(format!("{step}: {message}")),
        DriverError::Busy(message) => DriverError::Busy(format!("{step}: {message}")),
        DriverError::NotFound(message) => DriverError::NotFound(format!("{step}: {message}")),
        DriverError::Unsupported(message) => DriverError::Unsupported(format!("{step}: {message}")),
        other => other,
    })
}

/// What the Pod shows of a unit whose guard cannot be asked: that nothing of the unit is left to ask.
enum Ending {
    /// The cluster has no Pod of the unit's name.
    Absent,
    /// The workload container that was pinned for the unit has ended, and with it everything the unit ran.
    Workload(pod_identity::WorkloadEnd),
}

impl Ending {
    /// The state of the unit that this is.
    fn state(&self) -> UnitState {
        match self {
            Self::Absent => UnitState::of(UnitStateKind::Absent),
            Self::Workload(end) => {
                let mut state = UnitState::of(UnitStateKind::Failed);
                state.detail = Some(end.sentence());
                state.exit_code = Some(i64::from(end.exit_code));
                state
            }
        }
    }

    /// Why the unit cannot be started: the cluster does not run a Pod's container again.
    fn refusal(&self) -> DriverError {
        match self {
            Self::Absent => DriverError::NotFound("the unit's Pod is gone".into()),
            Self::Workload(end) => DriverError::NotFound(format!(
                "{} (exit status {}) and the cluster does not run a Pod's container again",
                end.sentence(),
                end.exit_code
            )),
        }
    }
}

fn private_directory(path: &Path) -> Result<(), DriverError> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(unavailable()),
    }
    let meta = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if !meta.is_dir()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(unavailable());
    }
    Ok(())
}

/// A verified cluster and node-owned immutable tool cache. Journals are sharded
/// by native subject: opening one unit never reads every unit on the cluster.
pub struct KubernetesDriver {
    cluster: Cluster,
    directory: PathBuf,
    tools: PathBuf,
    connector: PathBuf,
    locks: KeyedLocks,
    capacity: RwLock<Option<(Instant, orch_driver_contract::host::HostResources)>>,
    descriptor: DriverDescriptor,
    /// What the last trusted helper process said on its error stream.
    said: Arc<std::sync::Mutex<String>>,
}

impl KubernetesDriver {
    /// Opens the explicitly configured cluster; unavailable setup never produces an offered driver.
    pub async fn connect(
        config: KubernetesConfig,
        data: &Path,
        tools: &Path,
        connector: &Path,
    ) -> Result<Arc<Self>, DriverError> {
        if !cfg!(target_os = "linux") {
            return Err(DriverError::Unsupported(
                "the Kubernetes driver requires Linux".into(),
            ));
        }
        let cluster = Cluster::connect(config, REQUEST).await?;
        // The pinned peer/cgroup implementation has not been proved under kernels
        // hidden behind sandbox runtimes. Do not infer that support from a name.
        if cluster
            .configuration
            .runtime_classes
            .iter()
            .any(|class| class.runtime != "runc" || class.boundary != RuntimeBoundary::SharedKernel)
        {
            return Err(DriverError::Unsupported(
                "this Kubernetes helper currently supports the shared-kernel runc class only"
                    .into(),
            ));
        }
        for class in &cluster.configuration.runtime_classes {
            if class
                .class_name
                .as_ref()
                .is_some_and(|name| !cluster.classes.contains(name))
            {
                return Err(unavailable());
            }
        }
        let parent = data.join("kubernetes");
        private_directory(&parent)?;
        let directory = parent.join(&cluster.receipt_scope);
        private_directory(&directory)?;
        let descriptor = descriptor(&cluster.configuration)?;
        descriptor.check().map_err(|faults| {
            DriverError::Internal(format!("the Kubernetes declaration is invalid: {faults:?}"))
        })?;
        let driver = Arc::new(Self {
            cluster,
            directory,
            tools: tools.to_owned(),
            connector: connector.to_owned(),
            locks: KeyedLocks::default(),
            capacity: RwLock::new(None),
            descriptor,
            said: Arc::default(),
        });
        driver.refresh_capacity().await?;
        let weak = Arc::downgrade(&driver);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REFRESH).await;
                let Some(driver) = weak.upgrade() else {
                    break;
                };
                let _ = driver.refresh_capacity().await;
            }
        });
        Ok(driver)
    }

    /// Keeps, bounded, what a trusted helper process says on its error stream, for the refusal that follows.
    fn listen(&self, process: &mut kube::api::AttachedProcess) {
        let Some(mut stderr) = process.stderr() else {
            return;
        };
        let said = Arc::clone(&self.said);
        tokio::spawn(async move {
            let mut text = Vec::new();
            let _ = (&mut stderr).take(SAID_BYTES).read_to_end(&mut text).await;
            if let Ok(mut slot) = said.lock() {
                *slot = String::from_utf8_lossy(&text).into_owned();
            }
        });
    }

    /// The refusal, with what the helper said before it ended.
    fn explained(&self, error: DriverError) -> DriverError {
        let said = self
            .said
            .lock()
            .map(|mut slot| std::mem::take(&mut *slot))
            .unwrap_or_default();
        let said = said.trim();
        match error {
            DriverError::Unavailable(message) if !said.is_empty() => {
                DriverError::Unavailable(format!("{message} (the helper said: {said})"))
            }
            other => other,
        }
    }

    fn journal(&self, subject: &str) -> Result<Arc<ReceiptStore>, DriverError> {
        let hash = Hash::of(subject.as_bytes()).to_string();
        Ok(Arc::new(ReceiptStore::open(
            &self.directory.join(hash),
            &format!("{}:{subject}", self.cluster.receipt_scope),
        )?))
    }

    fn unit_journal(&self, unit: &UnitRef) -> Result<Arc<ReceiptStore>, DriverError> {
        self.journal(&format!("unit:{}", unit.runtime_id))
    }

    fn record(journal: &ReceiptStore, unit: &UnitRef) -> Result<UnitRecord, DriverError> {
        let record = journal.unit(unit.runtime_id)?.ok_or_else(unavailable)?;
        if &record.unit != unit {
            return Err(unavailable());
        }
        Ok(record)
    }

    async fn namespace(&self, deadline: Instant) -> Result<(), DriverError> {
        let api: Api<Namespace> = Api::all(self.cluster.client.clone());
        let found = tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(REQUEST, api.get(&self.cluster.configuration.namespace)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
        if found.metadata.uid.as_deref() != Some(self.cluster.namespace_uid.as_str())
            || found.metadata.deletion_timestamp.is_some()
        {
            return Err(unavailable());
        }
        Ok(())
    }

    async fn pod(
        &self,
        record: &UnitRecord,
        deadline: Instant,
        scheduling: bool,
    ) -> Result<Pod, DriverError> {
        self.namespace(deadline).await?;
        let raw = tokio::time::timeout_at(
            deadline,
            pod_read::read_pod(
                self.cluster.client.clone(),
                &self.cluster.configuration.namespace,
                &record.unit.name,
                REQUEST,
            ),
        )
        .await
        .map_err(|_| DriverError::Timeout)??
        .ok_or_else(|| DriverError::Unavailable("pod read: the Pod is absent".into()))?;
        let expected = record.admitted.as_ref().ok_or_else(|| {
            DriverError::Unavailable("pod read: the admitted Pod is not recorded".into())
        })?;
        at(
            "admission check",
            verify_admission::same_isolation(expected, &raw, true),
        )?;
        let pod: Pod = serde_json::from_value(raw)
            .map_err(|_| DriverError::Unavailable("pod read: the Pod cannot be decoded".into()))?;
        let system = record.labels.get(LABEL_SYSTEM).ok_or_else(unavailable)?;
        crate::ownership::check_pod(
            &pod.metadata,
            &self.cluster.configuration.namespace,
            system,
            &record.unit,
        )
        .map_err(|_| {
            DriverError::Unavailable("owner check: the Pod is not this unit's incarnation".into())
        })?;
        let name = pod
            .spec
            .as_ref()
            .and_then(|spec| spec.node_name.as_deref())
            .ok_or_else(|| DriverError::Unavailable("worker: the Pod has no worker yet".into()))?;
        let nodes: Api<Node> = Api::all(self.cluster.client.clone());
        let worker =
            tokio::time::timeout_at(deadline, tokio::time::timeout(REQUEST, nodes.get(name)))
                .await
                .map_err(|_| DriverError::Timeout)?
                .map_err(|_| DriverError::Timeout)?
                .map_err(|_| unavailable())?;
        let worker = at(
            "worker check",
            platform::worker_for_pod(
                &pod,
                &worker,
                &platform::worker_selector(std::env::consts::OS, std::env::consts::ARCH)?,
                scheduling,
            ),
        )?;
        if record
            .worker
            .as_ref()
            .is_some_and(|old| old.name != worker.name() || old.uid != worker.uid())
        {
            return Err(DriverError::Unavailable(
                "worker check: the Pod's worker was replaced".into(),
            ));
        }
        if let Some(identities) = &record.containers {
            pod_identity::verify(
                &pod,
                &self.cluster.configuration.namespace,
                system,
                &record.unit,
                identities,
            )
            .map_err(|_| {
                DriverError::Unavailable("container identity: a container was replaced".into())
            })?;
        }
        Ok(pod)
    }

    /// What the Pod shows after a command to the unit's guard failed: that the Pod is gone, or that the workload
    /// container pinned for the unit has ended. Either way nothing of the unit is left to answer, and the command
    /// will not succeed by being tried again. `None` when the Pod shows neither, or cannot be read now: the
    /// command's own refusal then stands. The namespace is checked first, so that a Pod is never taken for absent
    /// because another cluster was asked.
    async fn ending(
        &self,
        journal: &ReceiptStore,
        unit: &UnitRef,
        deadline: Instant,
    ) -> Option<Ending> {
        let record = Self::record(journal, unit).ok()?;
        self.namespace(deadline).await.ok()?;
        let raw = tokio::time::timeout_at(
            deadline,
            pod_read::read_pod(
                self.cluster.client.clone(),
                &self.cluster.configuration.namespace,
                &record.unit.name,
                REQUEST,
            ),
        )
        .await
        .ok()?
        .ok()?;
        let Some(raw) = raw else {
            return Some(Ending::Absent);
        };
        let identities = record.containers.as_ref()?;
        let system = record.labels.get(LABEL_SYSTEM)?;
        let pod: Pod = serde_json::from_value(raw).ok()?;
        pod_identity::workload_end(
            &pod,
            &self.cluster.configuration.namespace,
            system,
            &record.unit,
            identities,
        )
        .map(Ending::Workload)
    }

    /// Whether the unit's Pod is there, is the unit's own, and never had its containers pinned. Nothing of the system's
    /// ever ran in such a Pod: the connector is started only after the containers are pinned and the files are
    /// prepared, so a unit whose create ended before that (the staging timed out, an image could not be pulled) has no
    /// guard to ask and nothing to stop, and its Pod may be deleted by its UID. `false` when the Pod cannot be read now.
    async fn never_ran(&self, journal: &ReceiptStore, unit: &UnitRef, deadline: Instant) -> bool {
        let Ok(record) = Self::record(journal, unit) else {
            return false;
        };
        if record.containers.is_some() {
            return false;
        }
        let Some(system) = record.labels.get(LABEL_SYSTEM) else {
            return false;
        };
        if self.namespace(deadline).await.is_err() {
            return false;
        }
        let Ok(Ok(Some(raw))) = tokio::time::timeout_at(
            deadline,
            pod_read::read_pod(
                self.cluster.client.clone(),
                &self.cluster.configuration.namespace,
                &record.unit.name,
                REQUEST,
            ),
        )
        .await
        else {
            return false;
        };
        let Ok(pod) = serde_json::from_value::<Pod>(raw) else {
            return false;
        };
        crate::ownership::check_pod(
            &pod.metadata,
            &self.cluster.configuration.namespace,
            system,
            &record.unit,
        )
        .is_ok()
    }

    async fn ready_stage(
        &self,
        journal: &ReceiptStore,
        unit: &UnitRef,
        deadline: Instant,
    ) -> Result<pod_identity::StageIdentity, DriverError> {
        // What the last look found, so that a Pod that never gets ready says why when the time is up.
        let mut last: Option<DriverError>;
        loop {
            let record = Self::record(journal, unit)?;
            match self.pod(&record, deadline, true).await {
                Ok(pod) => {
                    let system = record.labels.get(LABEL_SYSTEM).ok_or_else(unavailable)?;
                    if let Ok(stage) = pod_identity::read_stage(
                        &pod,
                        &self.cluster.configuration.namespace,
                        system,
                        unit,
                        false,
                    ) {
                        let nodes: Api<Node> = Api::all(self.cluster.client.clone());
                        let name = pod
                            .spec
                            .as_ref()
                            .and_then(|spec| spec.node_name.as_deref())
                            .ok_or_else(unavailable)?;
                        let worker = tokio::time::timeout_at(
                            deadline,
                            tokio::time::timeout(REQUEST, nodes.get(name)),
                        )
                        .await
                        .map_err(|_| DriverError::Timeout)?
                        .map_err(|_| DriverError::Timeout)?
                        .map_err(|_| unavailable())?;
                        let worker = platform::worker_for_pod(
                            &pod,
                            &worker,
                            &platform::worker_selector(
                                std::env::consts::OS,
                                std::env::consts::ARCH,
                            )?,
                            true,
                        )?;
                        journal.bind_worker(unit.runtime_id, &unit.reference, &worker)?;
                        journal.bind_stage(unit.runtime_id, &unit.reference, &stage)?;
                        return Ok(stage);
                    }
                    last = Some(DriverError::Unavailable(
                        "init container: the staging container is not running yet".into(),
                    ));
                }
                Err(error @ DriverError::Unavailable(_)) => last = Some(error),
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                return Err(last.unwrap_or(DriverError::Timeout));
            }
            tokio::time::sleep_until((Instant::now() + Duration::from_millis(200)).min(deadline))
                .await;
        }
    }

    async fn stage(
        &self,
        journal: &ReceiptStore,
        unit: &UnitRef,
        archive: tempfile::NamedTempFile,
        header: StageHeader,
        deadline: Instant,
    ) -> Result<(), DriverError> {
        let stage = at(
            "init container",
            self.ready_stage(journal, unit, deadline).await,
        )?;
        let pods: Api<Pod> = Api::namespaced(
            self.cluster.client.clone(),
            &self.cluster.configuration.namespace,
        );
        let params = AttachParams::default()
            .container("stage")
            .stdin(true)
            .stdout(true)
            .stderr(true)
            .tty(false);
        let mut process = tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(REQUEST, pods.exec(&unit.name, [HELPER, "receive"], &params)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("receive exec: the Pod refused the exec session".into())
        })?;
        self.listen(&mut process);
        let mut input = process.stdin().ok_or_else(unavailable)?;
        let mut output = process.stdout().ok_or_else(unavailable)?;
        let mut file = tokio::fs::File::from_std(archive.reopen().map_err(|_| unavailable())?);
        at(
            "stage header",
            stage_protocol::write_header(&mut input, &header, &unit.reference, deadline).await,
        )?;
        at(
            "stage transfer",
            stage_protocol::transfer_archive(
                &mut file,
                &mut input,
                &header,
                &unit.reference,
                REQUEST,
                deadline,
            )
            .await,
        )?;
        let prepared = at(
            "stage receipt",
            stage_exchange::prepared(&mut output, &header, &unit.reference, REQUEST, deadline)
                .await,
        )?;
        let record = Self::record(journal, unit)?;
        let pod = at("stage pod check", self.pod(&record, deadline, true).await)?;
        at(
            "stage identity",
            pod_identity::verify_stage(
                &pod,
                &self.cluster.configuration.namespace,
                record.labels.get(LABEL_SYSTEM).ok_or_else(unavailable)?,
                unit,
                &stage,
                false,
            ),
        )?;
        journal.bind_stage_header(unit.runtime_id, &header)?;
        at(
            "stage release",
            stage_exchange::release(&mut input, &prepared, REQUEST, deadline).await,
        )?;
        drop(input);
        let status = process.take_status().ok_or_else(unavailable)?;
        let (status, joined) =
            tokio::time::timeout_at(deadline, async { tokio::join!(status, process.join()) })
                .await
                .map_err(|_| DriverError::Timeout)?;
        joined.map_err(|_| unavailable())?;
        if status.and_then(|status| status.status).as_deref() != Some("Success") {
            return Err(DriverError::Unavailable(
                "stage status: the receiving process did not end in success".into(),
            ));
        }
        loop {
            let record = Self::record(journal, unit)?;
            let pod = at("containers", self.pod(&record, deadline, true).await)?;
            if let Ok(identities) = pod_identity::read(
                &pod,
                &self.cluster.configuration.namespace,
                record.labels.get(LABEL_SYSTEM).ok_or_else(unavailable)?,
                unit,
            ) {
                pod_identity::verify_stage(
                    &pod,
                    &self.cluster.configuration.namespace,
                    record.labels.get(LABEL_SYSTEM).ok_or_else(unavailable)?,
                    unit,
                    &stage,
                    true,
                )?;
                journal.bind_containers(unit.runtime_id, &unit.reference, &identities)?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(DriverError::Unavailable(
                    "containers: the workload and the guard did not start in time".into(),
                ));
            }
            tokio::time::sleep_until((Instant::now() + Duration::from_millis(200)).min(deadline))
                .await;
        }
    }

    async fn bootstrap(&self, record: &UnitRecord, deadline: Instant) -> Result<(), DriverError> {
        self.pod(record, deadline, false).await?;
        let pods: Api<Pod> = Api::namespaced(
            self.cluster.client.clone(),
            &self.cluster.configuration.namespace,
        );
        let params = AttachParams::default()
            .container("guard")
            .stdin(true)
            .stdout(true)
            .stderr(true)
            .tty(false);
        let mut process = tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(
                REQUEST,
                pods.exec(&record.unit.name, [HELPER, "bootstrap"], &params),
            ),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("bootstrap exec: the Pod refused the exec session".into())
        })?;
        self.listen(&mut process);
        let mut input = process.stdin().ok_or_else(unavailable)?;
        let mut output = process.stdout().ok_or_else(unavailable)?;
        tokio::time::timeout_at(deadline, input.write_all(record.key.expose()))
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| unavailable())?;
        let mut ready = [0_u8; 5];
        tokio::time::timeout_at(deadline, output.read_exact(&mut ready))
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| {
                DriverError::Unavailable("bootstrap answer: the guard did not answer".into())
            })?;
        if &ready != b"ready" {
            return Err(DriverError::Unavailable(
                "bootstrap answer: the guard refused the key".into(),
            ));
        }
        drop(input);
        self.pod(record, deadline, false).await?;
        Ok(())
    }

    async fn exchange(
        &self,
        journal: &ReceiptStore,
        unit: &UnitRef,
        request: &[u8],
        deadline: Instant,
    ) -> Result<GuardReply, DriverError> {
        let record = Self::record(journal, unit)?;
        if record.removed {
            return Err(unavailable());
        }
        if record.stage_id.is_none() || record.stage_header.is_none() {
            return Err(unavailable());
        }
        at("pod check", self.pod(&record, deadline, false).await)?;
        let sequence = at(
            "sequence",
            journal.reserve_sequence(unit.runtime_id, &unit.reference, request),
        )?;
        let envelope =
            authenticated_control::seal_request(&record.key, &unit.reference, sequence, request)?;
        let pods: Api<Pod> = Api::namespaced(
            self.cluster.client.clone(),
            &self.cluster.configuration.namespace,
        );
        let params = AttachParams::default()
            .container("guard")
            .stdin(true)
            .stdout(true)
            .stderr(true)
            .tty(false);
        let mut process = tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(REQUEST, pods.exec(&unit.name, [HELPER, "relay"], &params)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("relay exec: the Pod refused the exec session".into())
        })?;
        self.listen(&mut process);
        let mut input = process.stdin().ok_or_else(unavailable)?;
        let mut output = process.stdout().ok_or_else(unavailable)?;
        at(
            "relay request",
            encrypted_transport::write_envelope(&mut input, sequence, &envelope, deadline).await,
        )?;
        let (reply_sequence, reply) = at(
            "relay reply",
            encrypted_transport::read_envelope(&mut output, deadline).await,
        )?;
        if reply_sequence != sequence {
            return Err(unavailable());
        }
        let reply = at(
            "relay authentication",
            authenticated_control::open_response(
                &record.key,
                &unit.reference,
                sequence,
                request,
                &reply,
            ),
        )?;
        at("pod check", self.pod(&record, deadline, false).await)?;
        at(
            "journal",
            journal.complete_sequence(unit.runtime_id, &unit.reference, sequence, request, &reply),
        )?;
        Ok(reply.response().clone())
    }

    async fn command(
        &self,
        journal: &ReceiptStore,
        unit: &UnitRef,
        command: GuardRequest,
        deadline: Instant,
    ) -> Result<GuardPhase, DriverError> {
        self.command_unexplained(journal, unit, command, deadline)
            .await
            .map_err(|error| self.explained(error))
    }

    async fn command_unexplained(
        &self,
        journal: &ReceiptStore,
        unit: &UnitRef,
        command: GuardRequest,
        deadline: Instant,
    ) -> Result<GuardPhase, DriverError> {
        // A lost reply is recovered before a new sequence. Its acknowledgement is
        // historical; the new Inspect below supplies the current observation.
        if let Some(pending) = Self::record(journal, unit)?.pending_request {
            self.exchange(journal, unit, &pending, deadline).await?;
        }
        let request = control_protocol::encode_request(&command, deadline)?;
        match self.exchange(journal, unit, &request, deadline).await? {
            GuardReply::State { phase } => Ok(phase),
            GuardReply::Refused { .. } => Err(DriverError::Unavailable(
                "the unit's guard refused the command".into(),
            )),
        }
    }

    async fn ensure_claim(
        &self,
        volume: &VolumeRequest,
        deadline: Instant,
    ) -> Result<(), DriverError> {
        let _held = self
            .locks
            .hold(&format!("claim:{}", volume.name), deadline)
            .await?;
        let journal = self.journal(&format!("claim:{}", volume.name))?;
        let service = claim_service::ClaimService::new(
            self.cluster.client.clone(),
            self.cluster.configuration.clone(),
            journal,
            REQUEST,
        )?;
        self.namespace(deadline).await?;
        service.ensure(volume, deadline).await?;
        Ok(())
    }

    async fn refresh_capacity(&self) -> Result<(), DriverError> {
        let deadline = Instant::now() + REQUEST;
        let selector = platform::worker_selector(std::env::consts::OS, std::env::consts::ARCH)?;
        let nodes: Api<Node> = Api::all(self.cluster.client.clone());
        let mut cursor = String::new();
        let mut available: Option<orch_driver_contract::host::HostResources> = None;
        loop {
            let params = ListParams::default().limit(PAGE).continue_token(&cursor);
            let page = tokio::time::timeout_at(deadline, nodes.list(&params))
                .await
                .map_err(|_| DriverError::Timeout)?
                .map_err(|_| unavailable())?;
            for node in page.items {
                if let Ok(observed) =
                    crate::capacity::aggregate(std::slice::from_ref(&node), &selector)
                {
                    if let Some(old) = &mut available {
                        // Conservatively reserve no more than fits on every eligible
                        // worker. Summed free memory does not prove one Pod fits.
                        old.cpus = old.cpus.min(observed.cpus);
                        old.cpu_millis = old
                            .cpu_millis
                            .zip(observed.cpu_millis)
                            .map(|(old, new)| old.min(new));
                        old.memory_mb = old.memory_mb.min(observed.memory_mb);
                    } else {
                        available = Some(observed);
                    }
                }
            }
            cursor = page.metadata.continue_.unwrap_or_default();
            if cursor.is_empty() {
                break;
            }
        }
        let available = available.ok_or_else(unavailable)?;
        *self.capacity.write().map_err(|_| unavailable())? = Some((Instant::now(), available));
        Ok(())
    }

    /// A bounded diagnostic tail from the exact retained workload container.
    pub async fn logs(&self, unit: &UnitRef, max_bytes: i64) -> Result<String, DriverError> {
        let deadline = Instant::now() + CONTROL;
        let _held = self
            .locks
            .hold(&unit.runtime_id.to_string(), deadline)
            .await?;
        let journal = self.unit_journal(unit)?;
        let record = Self::record(&journal, unit)?;
        self.pod(&record, deadline, false).await?;
        let pods = Api::namespaced(
            self.cluster.client.clone(),
            &self.cluster.configuration.namespace,
        );
        crate::logs::read_logs(
            &pods,
            crate::logs::LogRead {
                namespace: &self.cluster.configuration.namespace,
                system: record.labels.get(LABEL_SYSTEM).ok_or_else(unavailable)?,
                unit,
                max_bytes,
                request_timeout: REQUEST,
                stall_timeout: REQUEST,
                deadline,
            },
        )
        .await
    }

    /// Whether the connector in this unit has stored its identity, which the node's enrollment watch asks.
    /// A fresh yes also lets the guard forget the one-use token: that is safe only once enrollment is real.
    pub async fn connector_enrolled(&self, unit: &UnitRef) -> Option<bool> {
        let deadline = Instant::now() + CONTROL;
        let found = self.probe_enrolled(unit, deadline).await.ok()?;
        if found {
            // The token is spent either way; failing to forget it is not a reason to doubt the enrollment.
            let _ = self.enrollment_confirmed(unit).await;
        }
        Some(found)
    }

    async fn probe_enrolled(&self, unit: &UnitRef, deadline: Instant) -> Result<bool, DriverError> {
        let _held = self
            .locks
            .hold(&unit.runtime_id.to_string(), deadline)
            .await?;
        let journal = self.unit_journal(unit)?;
        let record = Self::record(&journal, unit)?;
        if record.removed {
            return Err(unavailable());
        }
        self.pod(&record, deadline, false).await?;
        let pods: Api<Pod> = Api::namespaced(
            self.cluster.client.clone(),
            &self.cluster.configuration.namespace,
        );
        let params = AttachParams::default()
            .container("workload")
            .stdin(false)
            .stdout(true)
            .stderr(false)
            .tty(false);
        let mut process = tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(
                REQUEST,
                pods.exec(&unit.name, [crate::pod::SUPERVISOR, "enrolled"], &params),
            ),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| {
            DriverError::Unavailable("enrolled probe exec: the Pod refused the exec session".into())
        })?;
        let mut output = process.stdout().ok_or_else(unavailable)?.take(2);
        let mut answer = Vec::new();
        tokio::time::timeout_at(deadline, output.read_to_end(&mut answer))
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| unavailable())?;
        drop(output);
        let _ = tokio::time::timeout_at(deadline, process.join()).await;
        match answer.as_slice() {
            b"1" => Ok(true),
            b"0" => Ok(false),
            _ => Err(unavailable()),
        }
    }

    /// Acknowledges the node's fresh observation that this connector enrolled.
    pub async fn enrollment_confirmed(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let deadline = Instant::now() + CONTROL;
        let _held = self
            .locks
            .hold(&unit.runtime_id.to_string(), deadline)
            .await?;
        let journal = self.unit_journal(unit)?;
        self.command(
            &journal,
            unit,
            GuardRequest::EnrollmentConfirmed {
                pod_uid: unit.reference.clone(),
            },
            deadline,
        )
        .await?;
        Ok(())
    }
}

fn descriptor(config: &KubernetesConfig) -> Result<DriverDescriptor, DriverError> {
    let mut descriptor: DriverDescriptor = serde_json::from_value(serde_json::json!({
        "name":"kubernetes", "title":"Kubernetes", "version":"1", "configuration_schema":crate::schema::configuration_schema()?,
        "host_scope":"driver", "tool_delivery":"driver", "gateway_access":"driver_network",
        "spec_schemas":{"environment":template_schema::environment(false)}, "confinement":"isolated",
        "network_egress":["open"], "recreate":"replace", "volumes":true,
        "data_storage":{"made_by_orch":"volume", "existing_directory":false, "reason":"Host paths are not offered on this cluster: use a volume."},
        "mounts":"placed", "private_views":false, "storages":false, "suspend":false, "sources":["image"],
        "resource_limits":{"cpu":"enforced","memory":"enforced","pids":"none","disk":"none"}, "process_scope":"all",
        "unavailable":{
            "host_paths":"Host paths are not offered on this cluster: use a volume.",
            "build":"Images are not built on this cluster: name an image it can pull.",
            "archive":"Image archives are not loaded on this cluster: name an image it can pull.",
            "kept_files":"Software installed outside the volumes is lost when the environment is replaced: keep it in the image or the volumes.",
            "private_views":"Private views are not offered on this cluster.",
            "suspend":"An environment is not held still on this cluster: stop it to copy its files.",
            "live_mounts":"To change a mount, the environment restarts."
        },
        "host_ports":{"supported":false, "reason":"Host ports are not offered on this cluster: an environment is reached through its place or its link."},
        "kept_file_transfer":{"supported":false, "reason":"Installed files do not travel between hosts from this cluster: keep them in the image or the volumes."}
    })).map_err(|_| DriverError::Internal("the Kubernetes declaration cannot be made".into()))?;
    descriptor.runtimes = config
        .runtime_classes
        .iter()
        .map(|class| {
            let mut runtime =
                orch_driver_contract::isolation::RuntimeClass::new(&class.runtime, class.boundary);
            runtime.keeps_software_on_stop = true;
            runtime.limits = class_limits::limits(false);
            runtime.unavailable = class_limits::unavailable();
            runtime
        })
        .collect();
    descriptor.runtime_classes = Some(
        descriptor
            .runtimes
            .iter()
            .map(|runtime| runtime.name.clone())
            .collect(),
    );
    descriptor.default_runtime = config
        .runtime_classes
        .iter()
        .find(|class| class.class_name.is_none())
        .or_else(|| config.runtime_classes.first())
        .map(|class| class.runtime.clone());
    Ok(descriptor)
}

#[async_trait]
impl EnvironmentDriver for KubernetesDriver {
    fn describe(&self) -> DriverDescriptor {
        self.descriptor.clone()
    }

    fn validate_configuration(&self, value: &Json) -> Result<(), DriverError> {
        let config: KubernetesConfig = serde_json::from_value(value.clone()).map_err(|_| {
            DriverError::InvalidSpec("the Kubernetes configuration is invalid".into())
        })?;
        config.check().map_err(|faults| {
            DriverError::InvalidSpec(
                faults
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })
    }

    async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
        let deadline = Instant::now() + CREATE;
        let _held = self
            .locks
            .hold(&spec.runtime_id.to_string(), deadline)
            .await?;
        let mut cluster = self.cluster.clone();
        cluster.refresh_classes().await?;
        let plan = at(
            "admission",
            admission::plan(spec, &cluster.configuration, &cluster.classes),
        )?;
        let attached = spec
            .mounts
            .iter()
            .map(|mount| AttachedMount {
                name: mount.name.clone(),
                path: mount.target.clone(),
                mode: mount.mode,
            })
            .collect::<Vec<_>>();
        let launch = launch::prepare(spec, &plan, &attached)?;
        for volume in spec.volumes.values() {
            let journal = self.journal(&format!("claim:{}", volume.name))?;
            let saved = journal.volume(&volume.name)?.ok_or_else(unavailable)?;
            if saved.storage != volume.storage
                || saved.labels.get(LABEL_SYSTEM) != spec.labels.get(LABEL_SYSTEM)
            {
                return Err(unavailable());
            }
            drop(journal);
            at("claim", self.ensure_claim(&saved, deadline).await)?;
        }
        let journal = self.journal(&format!("unit:{}", spec.runtime_id))?;
        let cache = self.tools.clone();
        let connector = self.connector.clone();
        let tools = spec.tools.clone();
        let directory = self.directory.clone();
        let (archive, receipt) = tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || {
                let mut archive =
                    tempfile::NamedTempFile::new_in(directory).map_err(|_| unavailable())?;
                let receipt = staging::archive(
                    &cache,
                    &connector,
                    &tools,
                    archive.as_file_mut(),
                    deadline.into_std(),
                )?;
                Ok::<_, DriverError>((archive, receipt))
            }),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())??;
        let service = unit_create::UnitService::new(
            self.cluster.client.clone(),
            cluster.configuration,
            Arc::clone(&journal),
            REQUEST,
        )?;
        let unit = at(
            "pod",
            service
                .create(
                    spec,
                    &plan,
                    &launch,
                    std::env::consts::OS,
                    std::env::consts::ARCH,
                    deadline,
                )
                .await,
        )?;
        let record = Self::record(&journal, &unit)?;
        if record.removed {
            return Err(unavailable());
        }
        if record.containers.is_none() {
            at(
                "stage",
                self.stage(
                    &journal,
                    &unit,
                    archive,
                    StageHeader {
                        pod_uid: unit.reference.clone(),
                        receipt,
                        tools: spec.tools.clone(),
                    },
                    deadline,
                )
                .await,
            )
            .map_err(|error| self.explained(error))?;
            let record = Self::record(&journal, &unit)?;
            at("bootstrap", self.bootstrap(&record, deadline).await)
                .map_err(|error| self.explained(error))?;
            let identities = record.containers.ok_or_else(unavailable)?;
            at(
                "attach",
                self.command(
                    &journal,
                    &unit,
                    GuardRequest::Attach {
                        pod_uid: unit.reference.clone(),
                        container_id: identities.workload,
                    },
                    deadline,
                )
                .await,
            )?;
        }
        if !Self::record(&journal, &unit)?.prepared {
            at(
                "prepare",
                self.command(
                    &journal,
                    &unit,
                    GuardRequest::Prepare {
                        pod_uid: unit.reference.clone(),
                        ca: launch.files.ca,
                        enrollment: launch.files.enrollment,
                        environment: launch.files.environment,
                        mounts: launch.files.mounts,
                    },
                    deadline,
                )
                .await,
            )?;
        }
        Ok(unit)
    }

    async fn start(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let deadline = Instant::now() + CONTROL;
        let _held = self
            .locks
            .hold(&unit.runtime_id.to_string(), deadline)
            .await?;
        let journal = self.unit_journal(unit)?;
        let phase = match self
            .command(
                &journal,
                unit,
                GuardRequest::Inspect {
                    pod_uid: unit.reference.clone(),
                },
                deadline,
            )
            .await
        {
            Ok(phase) => phase,
            // A unit that has ended is not started by asking its guard again: the cluster does not run a Pod's
            // container twice.
            Err(error) => {
                return Err(match self.ending(&journal, unit, deadline).await {
                    Some(ending) => ending.refusal(),
                    None => error,
                });
            }
        };
        if phase == GuardPhase::Running {
            return Ok(());
        }
        if phase != GuardPhase::Stopped || !Self::record(&journal, unit)?.prepared {
            return Err(unavailable());
        }
        if self
            .command(
                &journal,
                unit,
                GuardRequest::Start {
                    pod_uid: unit.reference.clone(),
                },
                deadline,
            )
            .await?
            != GuardPhase::Running
        {
            return Err(unavailable());
        }
        Ok(())
    }

    async fn stop(&self, unit: &UnitRef, grace: Duration) -> Result<(), DriverError> {
        if grace >= CONTROL {
            return Err(DriverError::Timeout);
        }
        let deadline = Instant::now() + CONTROL;
        let _held = self
            .locks
            .hold(&unit.runtime_id.to_string(), deadline)
            .await?;
        let journal = self.unit_journal(unit)?;
        let grace_millis = u64::try_from(grace.as_millis()).map_err(|_| DriverError::Timeout)?;
        match self
            .command(
                &journal,
                unit,
                GuardRequest::Stop {
                    pod_uid: unit.reference.clone(),
                    grace_millis,
                },
                deadline,
            )
            .await
        {
            Ok(GuardPhase::Stopped) => Ok(()),
            Ok(_) => Err(unavailable()),
            // Nothing is left to stop of a unit whose Pod is gone or whose workload container has ended.
            Err(error) => match self.ending(&journal, unit, deadline).await {
                Some(_) => Ok(()),
                None => Err(error),
            },
        }
    }

    async fn remove(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let deadline = Instant::now() + CONTROL;
        let _held = self
            .locks
            .hold(&unit.runtime_id.to_string(), deadline)
            .await?;
        let journal = self.unit_journal(unit)?;
        let record = Self::record(&journal, unit)?;
        if record.removed {
            return Ok(());
        }
        let ended = match self
            .command(
                &journal,
                unit,
                GuardRequest::Stop {
                    pod_uid: unit.reference.clone(),
                    grace_millis: 0,
                },
                deadline,
            )
            .await
        {
            Ok(GuardPhase::Stopped) => false,
            Ok(_) => return Err(unavailable()),
            // The guard cannot be asked when nothing of the unit is left: the Pod is gone, or its workload
            // container ended. Nothing needs stopping then, and a Pod that is still there is deleted below
            // by the UID the unit was made with.
            Err(error) => match self.ending(&journal, unit, deadline).await {
                Some(Ending::Absent) => return journal.removed_with_pod(unit.runtime_id),
                Some(Ending::Workload(_)) => true,
                // A create that ended before the containers were pinned left a Pod that never ran anything.
                None if self.never_ran(&journal, unit, deadline).await => true,
                None => return Err(error),
            },
        };
        if !ended {
            self.pod(&record, deadline, false).await?;
        }
        let pods: Api<Pod> = Api::namespaced(
            self.cluster.client.clone(),
            &self.cluster.configuration.namespace,
        );
        let params = DeleteParams {
            preconditions: Some(Preconditions {
                uid: Some(unit.reference.clone()),
                resource_version: None,
            }),
            grace_period_seconds: Some(0),
            ..DeleteParams::default()
        };
        tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(REQUEST, pods.delete(&unit.name, &params)),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
        if ended {
            // A command that never got its answer went to the Pod that is now gone.
            journal.removed_with_pod(unit.runtime_id)
        } else {
            journal.removed(unit.runtime_id)
        }
    }

    async fn inspect(&self, unit: &UnitRef) -> Result<UnitState, DriverError> {
        let deadline = Instant::now() + CONTROL;
        let _held = self
            .locks
            .hold(&unit.runtime_id.to_string(), deadline)
            .await?;
        let journal = self.unit_journal(unit)?;
        if Self::record(&journal, unit)?.removed {
            return Ok(UnitState::of(UnitStateKind::Absent));
        }
        let phase = match self
            .command(
                &journal,
                unit,
                GuardRequest::Inspect {
                    pod_uid: unit.reference.clone(),
                },
                deadline,
            )
            .await
        {
            Ok(phase) => phase,
            // A guard that cannot be asked because nothing of the unit is left is an answer too: the unit is
            // gone, or it ended, with the exit the cluster recorded.
            Err(error) => {
                return match self.ending(&journal, unit, deadline).await {
                    Some(ending) => Ok(ending.state()),
                    None => Err(error),
                };
            }
        };
        let state = match phase {
            GuardPhase::Running => UnitStateKind::Running,
            GuardPhase::Stopped => UnitStateKind::Stopped,
            GuardPhase::Failed => UnitStateKind::Failed,
            GuardPhase::Unprepared => return Err(unavailable()),
        };
        Ok(UnitState::of(state))
    }

    async fn list(&self, system: &str) -> Result<Vec<UnitRef>, DriverError> {
        let deadline = Instant::now() + CONTROL;
        let pods: Api<Pod> = Api::namespaced(
            self.cluster.client.clone(),
            &self.cluster.configuration.namespace,
        );
        let mut cursor = String::new();
        let mut units = Vec::new();
        loop {
            let page = tokio::time::timeout_at(
                deadline,
                pods.list(
                    &ListParams::default()
                        .limit(PAGE)
                        .continue_token(&cursor)
                        .labels(crate::pod::UNIT_SELECTOR),
                ),
            )
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(|_| unavailable())?;
            for pod in page.items {
                let Some(labels) = &pod.metadata.annotations else {
                    continue;
                };
                if labels.get(LABEL_SYSTEM).map(String::as_str) != Some(system) {
                    continue;
                }
                let Some(runtime) = labels
                    .get(orch_driver_contract::runtime::LABEL_RUNTIME_ID)
                    .and_then(|id| id.parse().ok())
                else {
                    return Err(unavailable());
                };
                let Some(name) = pod.metadata.name.clone() else {
                    return Err(unavailable());
                };
                let journal = self.journal(&format!("unit:{runtime}"))?;
                let record = journal.unit(runtime)?.ok_or_else(unavailable)?;
                if !record.removed {
                    crate::ownership::check_pod(
                        &pod.metadata,
                        &self.cluster.configuration.namespace,
                        system,
                        &record.unit,
                    )
                    .map_err(|_| unavailable())?;
                    if record.unit.name != name {
                        return Err(unavailable());
                    }
                    units.push(record.unit);
                }
            }
            cursor = page.metadata.continue_.unwrap_or_default();
            if cursor.is_empty() {
                break;
            }
        }
        Ok(units)
    }

    async fn ensure_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        self.ensure_claim(volume, Instant::now() + CREATE).await
    }

    async fn remove_volume(&self, name: &str) -> Result<(), DriverError> {
        let deadline = Instant::now() + CONTROL;
        let _held = self.locks.hold(&format!("claim:{name}"), deadline).await?;
        let journal = self.journal(&format!("claim:{name}"))?;
        let Some(volume) = journal.volume(name)? else {
            // This driver never made a claim under that name: there is nothing of its to remove.
            return Ok(());
        };
        self.namespace(deadline).await?;
        claim_service::ClaimService::new(
            self.cluster.client.clone(),
            self.cluster.configuration.clone(),
            journal,
            REQUEST,
        )?
        .remove(&volume, deadline)
        .await
    }

    async fn volume_identity(
        &self,
        volume: &VolumeRequest,
    ) -> Result<Option<VolumeIdentity>, DriverError> {
        let deadline = Instant::now() + REQUEST;
        let claims: Api<PersistentVolumeClaim> = Api::namespaced(
            self.cluster.client.clone(),
            &self.cluster.configuration.namespace,
        );
        let found = tokio::time::timeout_at(
            deadline,
            claims.get_opt(&crate::names::claim(&volume.name)?),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
        if found.is_none() {
            return Ok(None);
        }
        let journal = self.journal(&format!("claim:{}", volume.name))?;
        if journal.volume(&volume.name)?.as_ref() != Some(volume) {
            return Err(unavailable());
        }
        drop(journal);
        // ensure_claim rechecks the exact journalled UID and protected labels; it
        // does not adopt an unrecorded conflict or substitute a later incarnation.
        self.ensure_claim(volume, deadline).await?;
        Ok(Some(VolumeIdentity {
            labels: volume.labels.clone(),
        }))
    }

    async fn volume_exists(&self, volume: &VolumeRequest) -> Result<bool, DriverError> {
        Ok(self.volume_identity(volume).await?.is_some())
    }

    fn host_resources(&self) -> Result<orch_driver_contract::host::HostResources, DriverError> {
        self.capacity
            .read()
            .map_err(|_| unavailable())?
            .as_ref()
            .filter(|(at, _)| at.elapsed() <= REFRESH.saturating_mul(2))
            .map(|(_, value)| value.clone())
            .ok_or_else(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RuntimeBoundary, RuntimeClass};

    fn configuration() -> KubernetesConfig {
        KubernetesConfig {
            kubeconfig: None,
            namespace: "work".into(),
            storage_class: "local-path".into(),
            volume_request: "10Gi".into(),
            runtime_classes: vec![RuntimeClass {
                runtime: "runc".into(),
                class_name: None,
                boundary: RuntimeBoundary::SharedKernel,
            }],
            image_pull_secrets: Vec::new(),
            tool_stager_image: format!("registry.example/helper@sha256:{}", "a".repeat(64)),
        }
    }

    #[test]
    fn the_declaration_holds_together_and_offers_only_the_admitted_runtime() {
        let descriptor = descriptor(&configuration()).unwrap();
        assert_eq!(descriptor.check(), Ok(()));
        assert_eq!(descriptor.name, "kubernetes");
        assert_eq!(descriptor.default_runtime.as_deref(), Some("runc"));
        assert_eq!(
            descriptor.runtime_classes,
            Some(vec!["runc".to_owned()]),
            "only the runtimes the operator admitted"
        );
        assert_eq!(descriptor.network_egress.len(), 1, "open only");
    }

    #[test]
    fn a_unit_that_ended_is_failed_with_its_exit_and_one_without_a_pod_is_absent() {
        let ended = Ending::Workload(pod_identity::WorkloadEnd {
            exit_code: 137,
            reason: "OOMKilled".into(),
        });
        let state = ended.state();
        assert_eq!(state.state, UnitStateKind::Failed);
        assert_eq!(state.exit_code, Some(137));
        assert_eq!(
            state.detail.as_deref(),
            Some("the workload container ended: OOMKilled")
        );
        assert_eq!(Ending::Absent.state(), UnitState::of(UnitStateKind::Absent));
        assert!(matches!(
            ended.refusal(),
            DriverError::NotFound(text) if text.contains("OOMKilled") && text.contains("137")
        ));
        assert!(matches!(Ending::Absent.refusal(), DriverError::NotFound(_)));
    }

    #[test]
    fn every_feature_the_cluster_lacks_says_what_to_do_instead() {
        let descriptor = descriptor(&configuration()).unwrap();
        let availability = descriptor.availability();
        for (feature, support) in &availability {
            if !support.supported {
                let reason = support.reason.as_deref().unwrap_or_default();
                assert!(
                    !reason.contains("this driver does not") && reason.ends_with('.'),
                    "{feature} gives only a bare refusal: {reason}"
                );
            }
        }
        assert_eq!(
            availability["host_paths"].reason.as_deref(),
            Some("Host paths are not offered on this cluster: use a volume.")
        );
        assert!(availability["tools"].supported && availability["gateway"].supported);
    }
}
