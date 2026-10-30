//! Container lifecycle and the system's labels.
//!
//! A unit is a container of the name the request gives it, labelled with the system, the runtime and the pool
//! (`orch.system`, `orch.runtime_id`, `orch.kind`) and with the driver's own mark (`orch.driver`). Those labels are
//! how the system finds its units again and how the driver knows what is its to touch: it lists, starts, stops,
//! inspects and removes only containers that carry them, whatever name or identifier it is handed.
//!
//! Making a unit is idempotent by name. The unit's main process is the connector, from the tool cache, started with
//! the launch variables; the files it needs for one run (the authority, the enrollment token, the list of mounts)
//! are put into a small volume of the unit's own at `/run/orch`, owned by the unit's user, and the token is emptied
//! when the unit stops. Everything in the container's creation options is worked out by [`container_body`] from the
//! request without asking the daemon anything, so that every rule of it is tested without one.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, ContainerStateStatusEnum, ContainerSummary,
    HostConfig, HostConfigLogConfig, Mount, MountTmpfsOptions, MountType, ResourcesUlimits,
    RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, ListContainersOptionsBuilder, RemoveContainerOptionsBuilder,
    StopContainerOptionsBuilder,
};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::isolation::Boundary;
use orch_driver_contract::mounts::{AttachedMount, MountSpec};
use orch_driver_contract::runtime::{
    LABEL_KIND, LABEL_RUNTIME_ID, LABEL_SYSTEM, LAUNCH_VARIABLES, Resources, RuntimeKind,
    RuntimeSpec, VolumeRole, mounts_file,
};
use orch_driver_contract::units::{UnitRef, UnitState, UnitStateKind};
use orch_driver_contract::volumes::VolumeRequest;
use orch_types::{Id, Time};
use std::time::Duration;

use crate::archive::{Entry, tar_of};
use crate::capabilities::DOCKER_INSIDE;
use crate::client::{DaemonFacts, map_error, status_of};
use crate::config::DockerConfig;
use crate::driver::DockerDriver;
use crate::networks::{self, NetworkPlan};
use crate::overlay;
use crate::restricted::{self, Private, Reach};
use crate::spec::{DockerSpec, TemplateMountKind, layout};
use crate::volumes::{self, Root};

/// The label that marks everything the driver made.
pub const LABEL_DRIVER: &str = "orch.driver";

/// The value of [`LABEL_DRIVER`].
pub const DRIVER_NAME: &str = "docker";

/// The label of a helper container that is made to prepare volumes: it names the unit the helper is for.
pub const LABEL_PREPARE: &str = "orch.prepare";

/// Ordinary data mounts restored by Docker on a new start; contains no host locations.
pub(crate) const LABEL_MOUNTS_BASE: &str = "orch.mounts_base";

/// The label that says what holds the unit in under its runtime (`shared_kernel`, `own_kernel`, …), so that what is
/// done to a running unit is chosen from how it was made.
pub const LABEL_BOUNDARY: &str = "orch.boundary";

/// The label on a unit that holds Docker inside; its value is the size of the file Docker keeps its data in, in GB.
pub const LABEL_DOCKER_INSIDE: &str = "orch.docker_inside_gb";

/// The label that lists the volumes only this unit's runtime uses (its workspace, home and state), so that no
/// second unit is started on them while this one runs.
pub const LABEL_EXCLUSIVE: &str = "orch.exclusive_volumes";

/// The label that holds the numbers of the user the unit runs as, `uid:gid`, for the files put into it later.
pub const LABEL_IDS: &str = "orch.user_ids";

/// The label that holds the disk the unit may use, in GB, for the driver to report against when the storage cannot
/// enforce it.
pub const LABEL_DISK_LIMIT: &str = "orch.disk_limit_gb";

/// What follows a unit's name to make the name of its volume of run files.
pub const RUN_VOLUME_SUFFIX: &str = "-run";

/// What follows a unit's name to make the name of the helper that prepares its volumes.
pub const HELPER_SUFFIX: &str = "-prep";

/// A thousandth of a processor, in the daemon's unit of processor time (billionths of a processor).
const NANO_CPUS_PER_MILLI: i64 = 1_000_000;

/// A MiB, the unit memory is limited in.
const BYTES_PER_MB: i64 = 1 << 20;

/// How much memory a tmpfs for the harness directory may take when the template names none: a tmpfs counts
/// against the unit's memory limit, and the directory above the tools holds nothing but mount points.
const TOOLS_TMPFS_BYTES: i64 = 1 << 20;

/// The permissions of the directory above the tools: readable and enterable, never writable by the unit's user.
const TOOLS_TMPFS_MODE: i64 = 0o555;

/// How much longer than the grace the request to stop may take: the daemon needs a moment to kill and to settle.
pub(crate) const STOP_REQUEST_SLACK: Duration = Duration::from_secs(30);

/// The permissions of the directories of the volumes that are the unit's own.
const WORK_MODE: u32 = 0o755;
const PRIVATE_MODE: u32 = 0o700;

/// The permissions of the files of one run.
const CA_MODE: u32 = 0o644;
const SECRET_MODE: u32 = 0o600;

/// A thing marked as the driver's own; a thing without the mark is somebody else's.
#[must_use]
pub fn is_driver_label_set(labels: &HashMap<String, String>) -> bool {
    labels.get(LABEL_DRIVER).map(String::as_str) == Some(DRIVER_NAME)
}

/// The labels the driver puts on everything it makes by itself: the configured ones, and its mark.
#[must_use]
pub fn driver_labels(config: &DockerConfig) -> BTreeMap<String, String> {
    let mut labels = config.labels.clone();
    labels.insert(LABEL_DRIVER.to_owned(), DRIVER_NAME.to_owned());
    labels
}

/// The name of the volume of run files of a unit.
#[must_use]
pub fn run_volume_name(unit: &str) -> String {
    format!("{unit}{RUN_VOLUME_SUFFIX}")
}

/// The name of the helper that prepares the volumes of a unit.
#[must_use]
pub fn helper_name(unit: &str) -> String {
    format!("{unit}{HELPER_SUFFIX}")
}

/// Whether a container name is one the daemon takes.
#[must_use]
pub fn is_container_name(name: &str) -> bool {
    name.len() <= 128
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// A lock for each key, so that two commands for one unit do not interleave while commands for others go on.
#[derive(Debug, Default)]
pub struct KeyedLocks {
    locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl KeyedLocks {
    /// Waits for the lock of `key` and holds it until the guard is dropped.
    pub async fn lock(&self, key: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self
                .locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A lock nobody holds or waits for is forgotten, so that the map does not grow with every unit.
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            Arc::clone(locks.entry(key.to_owned()).or_default())
        };
        lock.lock_owned().await
    }
}

/// The user and group a container runs as, from its labels.
fn ids_of(container: &ContainerInspectResponse) -> Option<(u32, u32)> {
    let labels = container.config.as_ref()?.labels.as_ref()?;
    let (uid, gid) = labels.get(LABEL_IDS)?.split_once(':')?;
    Some((uid.parse().ok()?, gid.parse().ok()?))
}

/// The labels of an inspected container.
fn labels_of(container: &ContainerInspectResponse) -> HashMap<String, String> {
    container
        .config
        .as_ref()
        .and_then(|config| config.labels.clone())
        .unwrap_or_default()
}

/// The unit a set of labels names, when it names one: it has the driver's mark, a system, a runtime and a pool.
#[must_use]
pub fn unit_from_labels(
    labels: &HashMap<String, String>,
    reference: &str,
    name: &str,
) -> Option<UnitRef> {
    if !is_driver_label_set(labels) || labels.get(LABEL_SYSTEM).is_none_or(String::is_empty) {
        return None;
    }
    let runtime_id: Id = labels.get(LABEL_RUNTIME_ID)?.parse().ok()?;
    let kind = match labels.get(LABEL_KIND)?.as_str() {
        "environment" => RuntimeKind::Environment,
        "provider" => RuntimeKind::Provider,
        _ => return None,
    };
    Some(UnitRef {
        reference: reference.to_owned(),
        name: name.to_owned(),
        runtime_id,
        kind,
    })
}

/// Whether the container is the unit `unit` names: the labels it carries are the driver's and say this runtime.
fn is_unit(container: &ContainerInspectResponse, unit: &UnitRef) -> bool {
    let labels = labels_of(container);
    is_driver_label_set(&labels)
        && labels
            .get(LABEL_SYSTEM)
            .is_some_and(|system| !system.is_empty())
        && labels.get(LABEL_RUNTIME_ID).map(String::as_str)
            == Some(unit.runtime_id.to_string().as_str())
        && labels.get(LABEL_KIND).map(String::as_str) == Some(unit.kind.as_str())
}

/// A time of the daemon, which writes a moment that never was as the first of January of year 1.
fn docker_time(text: Option<&str>) -> Option<Time> {
    let text = text?;
    if text.starts_with("0001-") {
        return None;
    }
    text.parse().ok()
}

/// The state of a container in the terms of the driver contract.
///
/// A container that ended by `SIGTERM` or `SIGKILL` without having run out of memory was stopped, by the driver
/// or by somebody who meant it; one that ended on its own with status 0 stopped as well. Any other status, and a
/// container the daemon could not start, is a failure.
#[must_use]
pub fn state_of(container: &ContainerInspectResponse) -> UnitState {
    let Some(state) = container.state.as_ref() else {
        return UnitState::of(UnitStateKind::Absent);
    };
    let exit_code = state.exit_code.filter(|_| state.running != Some(true));
    let mut out = UnitState {
        state: UnitStateKind::Stopped,
        detail: None,
        exit_code,
        started_at: docker_time(state.started_at.as_deref()),
        finished_at: docker_time(state.finished_at.as_deref()),
    };
    let error = state.error.as_deref().filter(|error| !error.is_empty());
    match state.status {
        Some(ContainerStateStatusEnum::RUNNING) => {
            out.state = UnitStateKind::Running;
            out.exit_code = None;
            out.finished_at = None;
            if state.paused == Some(true) {
                out.detail = Some("paused".to_owned());
            }
        }
        Some(ContainerStateStatusEnum::PAUSED) => {
            out.state = UnitStateKind::Running;
            out.exit_code = None;
            out.finished_at = None;
            out.detail = Some("paused".to_owned());
        }
        Some(ContainerStateStatusEnum::RESTARTING) => {
            out.state = UnitStateKind::Running;
            out.detail = Some("restarting".to_owned());
        }
        Some(ContainerStateStatusEnum::REMOVING) => out.state = UnitStateKind::Absent,
        Some(ContainerStateStatusEnum::CREATED) => {
            out.exit_code = None;
            if let Some(error) = error {
                out.state = UnitStateKind::Failed;
                out.detail = Some(error.to_owned());
            } else {
                out.detail = Some("created".to_owned());
            }
        }
        _ => {
            if let Some(error) = error {
                out.state = UnitStateKind::Failed;
                out.detail = Some(error.to_owned());
            } else if state.oom_killed == Some(true) {
                out.state = UnitStateKind::Failed;
                out.detail = Some("out of memory".to_owned());
            } else {
                match exit_code {
                    Some(0) | None => {}
                    // Ended by SIGKILL or SIGTERM (128 + the signal).
                    Some(137 | 143) => out.detail = Some("stopped by a signal".to_owned()),
                    Some(code) => {
                        out.state = UnitStateKind::Failed;
                        out.detail = Some(format!("exited with status {code}"));
                    }
                }
            }
        }
    }
    out
}

/// The limits the daemon is asked to enforce, and what it cannot be asked: a note for each limit the host does
/// not enforce.
pub fn apply_limits(
    resources: &Resources,
    facts: &DaemonFacts,
    host: &mut HostConfig,
) -> Vec<String> {
    apply_limits_under(resources, facts, Boundary::SharedKernel, host)
}

/// [`apply_limits`] for a unit under a runtime with this boundary: a guest with a kernel of its own has no block
/// device queue of the host's to weigh, so the disk weight is left out and said.
pub fn apply_limits_under(
    resources: &Resources,
    facts: &DaemonFacts,
    boundary: Boundary,
    host: &mut HostConfig,
) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(millis) = resources.cpu_millis.filter(|millis| *millis > 0) {
        if facts.cpu_quota {
            // A unit cannot be given more processors than the host has: the daemon refuses such a request.
            let most = if facts.cpus > 0 {
                facts.cpus.saturating_mul(1000)
            } else {
                i64::MAX
            };
            host.nano_cpus = Some(millis.min(most).saturating_mul(NANO_CPUS_PER_MILLI));
        } else {
            notes.push("this host does not limit processor time".to_owned());
        }
    }
    if let Some(mb) = resources.memory_mb.filter(|mb| *mb > 0) {
        if facts.memory_limit {
            let bytes = mb.saturating_mul(BYTES_PER_MB);
            host.memory = Some(bytes);
            // Memory plus swap equal to memory: the unit cannot spill over into swap.
            if facts.swap_limit {
                host.memory_swap = Some(
                    resources
                        .memory_swap_mb
                        .unwrap_or(mb)
                        .saturating_mul(BYTES_PER_MB),
                );
            }
        } else {
            notes.push("this host does not limit memory".to_owned());
        }
    }
    if let Some(weight) = resources
        .blkio_weight
        .and_then(|weight| u16::try_from(weight).ok())
    {
        if boundary.has_kernel_of_its_own() {
            notes.push(
                "this runtime has a kernel of its own: the disk weight is not applied".to_owned(),
            );
        } else {
            host.blkio_weight = Some(weight);
        }
    }
    if let Some(pids) = resources.pids.filter(|pids| *pids > 0) {
        if facts.pids_limit {
            host.pids_limit = Some(pids);
        } else {
            notes.push("this host does not limit processes".to_owned());
        }
    }
    if let Some(gb) = resources.disk_gb.filter(|gb| *gb > 0) {
        if facts.disk_quota {
            host.storage_opt = Some(HashMap::from([("size".to_owned(), format!("{gb}G"))]));
        } else {
            notes.push(
                "this host's storage has no quotas; the disk is measured and not limited"
                    .to_owned(),
            );
        }
    }
    notes
}

/// The name each launch variable of the contract takes in a unit: the places the driver fixes, and the volumes
/// of the spec.
fn launch_path(name: &str, docker: &DockerSpec) -> Option<String> {
    Some(match name {
        "ORCH_CA_FILE" => layout::CA_FILE.to_owned(),
        "ORCH_ENROLL_FILE" => layout::ENROLL_FILE.to_owned(),
        "ORCH_DATA_DIR" => layout::STATE_DIR.to_owned(),
        "ORCH_TOOLS_DIR" => layout::TOOLS_DIR.to_owned(),
        "ORCH_WORKSPACE_DIR" => docker.workspace_mount.clone(),
        "ORCH_HOME_DIR" => docker.home_mount.clone(),
        "ORCH_MOUNTS_FILE" => layout::MOUNTS_FILE.to_owned(),
        _ => return None,
    })
}

/// The environment of the connector: every launch variable of the contract that this pool takes, and the
/// variables the request gives.
pub fn launch_env(spec: &RuntimeSpec, docker: &DockerSpec) -> Result<Vec<String>, DriverError> {
    let launch = spec.launch();
    let mut values = launch.values;
    for variable in &launch.paths {
        let value = launch_path(variable.name, docker).ok_or_else(|| {
            DriverError::Internal(format!(
                "the launch variable {} is not known to the Docker driver",
                variable.name
            ))
        })?;
        values.insert(variable.name.to_owned(), value);
    }
    if spec.kind == RuntimeKind::Provider && spec.volumes.contains_key(&VolumeRole::Home) {
        values.insert("ORCH_HARNESS_HOME".into(), "/opt/orch/harness-home".into());
        values.insert(
            "ORCH_HARNESS_HOME_SOURCE".into(),
            serde_json::to_string(&spec.harness_home.clone().unwrap_or_default())
                .map_err(|error| DriverError::InvalidSpec(error.to_string()))?,
        );
    }
    // A home for whatever runs in the unit that wants one, unless the request says where.
    values
        .entry("HOME".to_owned())
        .or_insert_with(|| match spec.kind {
            RuntimeKind::Environment => docker.home_mount.clone(),
            RuntimeKind::Provider => layout::STATE_DIR.to_owned(),
        });
    debug_assert!(
        LAUNCH_VARIABLES
            .iter()
            .all(|v| values.contains_key(v.name) || v.only_for.is_some_and(|k| k != spec.kind))
    );
    Ok(values
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect())
}

/// The places on the host that are bound into every unit, as the daemon names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolBinds {
    /// The directory with the connector.
    pub connector: String,
    /// One `(name, directory)` for each harness build.
    pub tools: Vec<(String, String)>,
}

/// What the request puts into the container as mounts, besides the template's own and the request's mounts:
/// the volumes, the run files, the connector and the tools.
pub fn unit_mounts(
    spec: &RuntimeSpec,
    docker: &DockerSpec,
    run_volume: &str,
    tools: &ToolBinds,
) -> Result<Vec<Mount>, DriverError> {
    let mut mounts = Vec::new();
    let volume = |role: VolumeRole| {
        spec.volumes
            .get(&role)
            .map(|volume| volume.name.clone())
            .ok_or_else(|| {
                DriverError::InvalidSpec(
                    format!("the request has no {role:?} volume").to_lowercase(),
                )
            })
    };
    mounts.push(crate::mounts::volume(
        &volume(VolumeRole::State)?,
        layout::STATE_DIR,
        false,
    ));
    if spec.kind == RuntimeKind::Environment {
        for (role, target) in [
            (VolumeRole::Workspace, &docker.workspace_mount),
            (VolumeRole::Home, &docker.home_mount),
        ] {
            let source = spec.volumes.get(&role).ok_or_else(|| {
                DriverError::InvalidSpec("the data root binding is missing".into())
            })?;
            mounts.push(match &source.directory {
                Some(directory) => crate::mounts::bind(&directory.path, target, false),
                None => crate::mounts::volume(&source.name, target, false),
            });
        }
    } else if let Some(source) = spec.volumes.get(&VolumeRole::Home) {
        mounts.push(match &source.directory {
            Some(directory) => {
                crate::mounts::bind(&directory.path, "/opt/orch/harness-home", false)
            }
            None => crate::mounts::volume(&source.name, "/opt/orch/harness-home", false),
        });
    }
    mounts.push(crate::mounts::volume(run_volume, layout::RUN_DIR, false));
    mounts.push(crate::mounts::bind(
        &tools.connector,
        layout::CONNECTOR_DIR,
        true,
    ));
    // The directory above the tools exists whether or not any tool is asked for, and is no place to write.
    mounts.push(Mount {
        typ: Some(MountType::TMPFS),
        target: Some(layout::TOOLS_DIR.to_owned()),
        tmpfs_options: Some(MountTmpfsOptions {
            size_bytes: Some(TOOLS_TMPFS_BYTES),
            mode: Some(TOOLS_TMPFS_MODE),
            options: None,
        }),
        ..Mount::default()
    });
    for (name, directory) in &tools.tools {
        mounts.push(crate::mounts::bind(
            directory,
            &format!("{}/{name}", layout::TOOLS_DIR),
            true,
        ));
    }
    for mount in &docker.mounts {
        mounts.push(match mount.kind {
            TemplateMountKind::Tmpfs => Mount {
                typ: Some(MountType::TMPFS),
                target: Some(mount.target.clone()),
                ..Mount::default()
            },
            TemplateMountKind::Volume => Mount {
                typ: Some(MountType::VOLUME),
                source: mount.source.clone(),
                target: Some(mount.target.clone()),
                read_only: Some(mount.read_only),
                ..Mount::default()
            },
            TemplateMountKind::Bind => crate::mounts::bind(
                mount.source.as_deref().unwrap_or_default(),
                &mount.target,
                mount.read_only,
            ),
        });
    }
    Ok(mounts)
}

/// Everything the container's creation options are worked out from.
#[derive(Debug)]
pub struct Plan<'a> {
    /// The request.
    pub spec: &'a RuntimeSpec,
    /// Its spec, read.
    pub docker: &'a DockerSpec,
    /// The driver's settings.
    pub config: &'a DockerConfig,
    /// What the daemon is.
    pub facts: &'a DaemonFacts,
    /// The image the container is made from.
    pub image: String,
    /// The network it is put on.
    pub network: NetworkPlan,
    /// The host entry for the gateway.
    pub gateway_entry: String,
    /// Every mount of the container, in the daemon's terms.
    pub mounts: Vec<Mount>,
    /// The numbers of the user it runs as, when they are known.
    pub ids: Option<(u32, u32)>,
    /// The names of the volumes that only this runtime's units use.
    pub exclusive: Vec<String>,
    /// For a restricted unit: how far it reaches, and its own network with its proxy. None for an open unit.
    pub egress: Option<(Reach, Private)>,
    /// What holds the unit in under its runtime.
    pub boundary: Boundary,
}

/// The creation options of the container.
pub fn container_body(plan: &Plan<'_>) -> Result<ContainerCreateBody, DriverError> {
    // A unit suppresses memory files by default, while a debugging shell may raise its own limit.
    const CORE_SOFT_LIMIT: i64 = 0;
    const CORE_HARD_LIMIT: i64 = -1;
    let spec = plan.spec;
    let docker = plan.docker;
    let mut labels: HashMap<String, String> = driver_labels(plan.config).into_iter().collect();
    labels.extend(spec.labels.clone());
    labels.insert(LABEL_EXCLUSIVE.to_owned(), plan.exclusive.join(","));
    if let Some(gb) = spec.resources.disk_gb {
        labels.insert(LABEL_DISK_LIMIT.to_owned(), gb.to_string());
    }
    labels.insert(LABEL_BOUNDARY.to_owned(), plan.boundary.as_str().to_owned());
    if spec
        .capabilities
        .base
        .iter()
        .chain(&spec.capabilities.granted)
        .any(|key| key == DOCKER_INSIDE)
    {
        labels.insert(
            LABEL_DOCKER_INSIDE.to_owned(),
            plan.config.docker_inside_gb.to_string(),
        );
    }
    if let Some((uid, gid)) = plan.ids {
        labels.insert(LABEL_IDS.to_owned(), format!("{uid}:{gid}"));
    }
    let mut host = HostConfig {
        init: Some(true),
        // A crashed process must not leave its memory in files people read. A debugging shell may raise its own limit.
        ulimits: Some(vec![ResourcesUlimits {
            name: Some("core".to_owned()),
            soft: Some(CORE_SOFT_LIMIT),
            hard: Some(CORE_HARD_LIMIT),
        }]),
        mounts: Some(plan.mounts.clone()),
        network_mode: Some(plan.network.name.clone()),
        extra_hosts: Some(vec![plan.gateway_entry.clone()]),
        dns: (!docker.network.dns.is_empty()).then(|| docker.network.dns.clone()),
        readonly_rootfs: Some(docker.security.read_only_rootfs),
        cap_add: (!docker.security.cap_add.is_empty()).then(|| docker.security.cap_add.clone()),
        cap_drop: (!docker.security.cap_drop.is_empty()).then(|| docker.security.cap_drop.clone()),
        security_opt: docker
            .security
            .no_new_privileges
            .then(|| vec!["no-new-privileges".to_owned()]),
        runtime: docker.runtime.clone(),
        // Nothing restarts a unit behind the system's back: a unit that fails is found failed and replaced.
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::NO),
            maximum_retry_count: None,
        }),
        log_config: Some(HostConfigLogConfig {
            typ: Some("json-file".to_owned()),
            config: Some(HashMap::from([
                ("max-size".to_owned(), plan.config.log_max_size.clone()),
                ("max-file".to_owned(), plan.config.log_max_files.to_string()),
            ])),
        }),
        ..HostConfig::default()
    };
    for note in apply_limits_under(&spec.resources, plan.facts, plan.boundary, &mut host) {
        tracing::warn!(unit = %spec.name, "{note}");
    }
    docker
        .patch
        .apply(&mut host, plan.config.seccomp_dir.as_deref());
    let mut env = launch_env(spec, docker)?;
    if let Some((reach, private)) = &plan.egress {
        // Nothing a template or a capability asks may give a restricted unit a way around its proxy: what would is
        // refused before anything is made, and the rest is put the way the proxy needs it.
        let own_kernel = plan.boundary.has_kernel_of_its_own();
        if let Some(why) = restricted::refusal(&host, own_kernel) {
            return Err(DriverError::Unsupported(format!(
                "network.restricted_unavailable: {why}"
            )));
        }
        restricted::restrict_host(&mut host, private, own_kernel);
        let proxy_variables = private.variables();
        env.retain(|entry| {
            let name = entry.split('=').next().unwrap_or_default();
            !proxy_variables.iter().any(|(variable, _)| variable == name)
        });
        env.extend(
            proxy_variables
                .into_iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        if let Some(label) = reach.label() {
            labels.insert(restricted::LABEL_EGRESS.to_owned(), label.to_owned());
        }
    }
    Ok(ContainerCreateBody {
        user: docker.user.container_user(),
        env: Some(env),
        image: Some(plan.image.clone()),
        // The root: a working directory that is the place of a volume would be made in the image's file system and
        // owned by root, and the volume would take that ownership when it is first filled.
        working_dir: Some("/".to_owned()),
        entrypoint: Some(vec![layout::CONNECTOR_PROGRAM.to_owned()]),
        cmd: Some(vec!["run".to_owned(), "--managed".to_owned()]),
        labels: Some(labels),
        host_config: Some(host),
        ..ContainerCreateBody::default()
    })
}

/// The files of one run of a unit, owned by the unit's user. The list of mounts is written when the mounts that were
/// attached are known, which is when the unit is made; a unit that is asked for again gets fresh credentials only.
#[must_use]
pub fn run_files(
    spec: &RuntimeSpec,
    attached: Option<&[AttachedMount]>,
    ids: (u32, u32),
) -> Vec<Entry> {
    let launch = spec.launch();
    let file = |name: &str, content: Vec<u8>, mode: u32| Entry::File {
        path: name.to_owned(),
        content,
        mode,
        uid: ids.0,
        gid: ids.1,
    };
    let mut files = vec![
        file("ca.pem", launch.ca_file.into_bytes(), CA_MODE),
        file(
            "enroll",
            launch.enroll_file.expose().as_bytes().to_vec(),
            SECRET_MODE,
        ),
    ];
    if let (RuntimeKind::Environment, Some(attached)) = (spec.kind, attached) {
        files.push(file(
            "mounts.json",
            mounts_file(attached).into_bytes(),
            CA_MODE,
        ));
    }
    files
}

impl DockerDriver {
    /// The unit a container is, when it is the driver's and says which runtime it is for.
    fn unit_of(&self, container: &ContainerInspectResponse) -> Option<UnitRef> {
        let name = container.name.as_deref()?.trim_start_matches('/');
        unit_from_labels(&labels_of(container), container.id.as_deref()?, name)
    }

    /// The container of a unit, when there is one and it is the unit's: the driver never answers for a container
    /// it did not make, whatever the reference says.
    pub(crate) async fn container_of(
        &self,
        unit: &UnitRef,
    ) -> Result<Option<ContainerInspectResponse>, DriverError> {
        let key = if unit.reference.is_empty() {
            &unit.name
        } else {
            &unit.reference
        };
        let found = match self.daemon.container(key).await? {
            Some(container) => Some(container),
            // A unit made again (on a network of its own) has another identifier and the same name and labels.
            None if !unit.reference.is_empty() => self.daemon.container(&unit.name).await?,
            None => None,
        };
        let Some(container) = found else {
            return Ok(None);
        };
        Ok(is_unit(&container, unit).then_some(container))
    }

    /// Makes a unit, or answers the one of that name; see `EnvironmentDriver::create`.
    pub(crate) async fn create_unit(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
        let _held = self.locks.lock(&spec.name).await;
        if !is_container_name(&spec.name) {
            return Err(DriverError::InvalidSpec(format!(
                "{:?} is not a name for a unit",
                spec.name
            )));
        }
        // The shape every driver is promised is checked, but not the name: the driver finds and touches its units by
        // their labels and never by their names, so a name other than the one the identifier gives (as the units of
        // a test on a shared daemon have) does no harm.
        if let Err(faults) = spec.check() {
            let relevant: Vec<String> = faults
                .iter()
                .filter(|fault| {
                    !matches!(
                        fault,
                        orch_driver_contract::runtime::SpecFault::WrongName { .. }
                    )
                })
                .map(ToString::to_string)
                .collect();
            if !relevant.is_empty() {
                return Err(DriverError::InvalidSpec(relevant.join("; ")));
            }
        }
        let live = if self.live_helpers_supported() {
            crate::live_capabilities::LIVE.as_slice()
        } else {
            &[]
        };
        let creation = spec.capabilities.creation_spec(&spec.spec, live);
        let docker = DockerSpec::parse(spec.kind, &creation)?;
        let expected = UnitRef {
            reference: String::new(),
            name: spec.name.clone(),
            runtime_id: spec.runtime_id,
            kind: spec.kind,
        };
        if let Some(found) = self.daemon.container(&spec.name).await? {
            if !is_unit(&found, &expected) {
                return Err(DriverError::Internal(format!(
                    "a container named {} exists and is not the unit of this runtime",
                    spec.name
                )));
            }
            // The same unit asked for again: it is answered as it is, with the files of its next run renewed
            // when it is not running.
            let running = found.state.as_ref().and_then(|state| state.running) == Some(true);
            if !running && let Some(ids) = ids_of(&found) {
                let id = found.id.clone().unwrap_or_default();
                self.daemon
                    .put_archive(
                        &id,
                        layout::RUN_DIR,
                        tar_of(&run_files(spec, None, ids), self.mtime())?,
                    )
                    .await?;
            }
            return self.unit_of(&found).ok_or_else(|| {
                DriverError::Internal(format!("the container {} lost its labels", spec.name))
            });
        }
        self.make_unit(spec, &docker).await
    }

    /// The moment of the clock, in seconds, for the files put into a container.
    pub(crate) fn mtime(&self) -> u64 {
        u64::try_from(self.clock.now().timestamp()).unwrap_or(0)
    }

    async fn make_unit(
        &self,
        spec: &RuntimeSpec,
        docker: &DockerSpec,
    ) -> Result<UnitRef, DriverError> {
        let config = &*self.config;
        let daemon = &self.daemon;
        let gateway_entry = networks::gateway_entry(config);
        let system = restricted::system_of(&spec.labels);
        // A restricted unit's network of its own, with its proxy's orders written before anything starts; an open
        // unit's network of its own, from which it reaches the gateway and the internet. Both are cut from the pool.
        let reach = docker.network.reach(spec.network_policy.restricted());
        let egress = match reach {
            Reach::Open => None,
            restricted => Some((restricted, self.prepare_egress(spec, restricted).await?)),
        };
        let network_plan = match &egress {
            Some((_, private)) => NetworkPlan {
                name: private.network.clone(),
                role: restricted::NETWORK_ROLE.to_owned(),
            },
            None => {
                let own = networks::ensure_open(
                    daemon,
                    config,
                    &networks::unit_network_name(&spec.name),
                    networks::ROLE_UNIT,
                    spec.runtime_id,
                    system,
                )
                .await?;
                NetworkPlan {
                    name: own.name,
                    role: networks::ROLE_UNIT.to_owned(),
                }
            }
        };
        // Retained volumes and private layers are checked independently of the unit, so keep their resource owner
        // as well as the system. The runtime identity changes when the same data is restored.
        let owner: BTreeMap<String, String> = spec
            .labels
            .iter()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    LABEL_SYSTEM | "orch.environment_id" | "orch.provider_id"
                )
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        for volume in spec
            .volumes
            .values()
            .filter(|volume| volume.directory.is_none())
        {
            volumes::ensure(
                daemon,
                config,
                &VolumeRequest {
                    name: volume.name.clone(),
                    storage: volume.storage.clone(),
                    labels: owner.clone(),
                },
            )
            .await?;
        }
        // The image.
        let image = self.image_for(spec, docker).await?;
        // The connector and the tools.
        let tools = self.tool_binds(spec)?;
        // A restricted unit's proxy, made from the operator's trusted image before the unit, going out on a network
        // of its own from the pool.
        if let Some((_, private)) = &egress {
            let outward =
                restricted::ensure_outward(daemon, config, &spec.name, spec.runtime_id, system)
                    .await?;
            restricted::ensure_proxy(
                daemon,
                config,
                private,
                &restricted::ProxyMaking {
                    unit: &spec.name,
                    image: &image,
                    connector_dir: &tools.connector,
                    outward: &outward.name,
                    gateway_entry: &gateway_entry,
                    system,
                },
            )
            .await?;
        }
        // The mounts the request attaches.
        let mut reserved: Vec<&str> = vec!["/opt/orch", layout::RUN_DIR, layout::STATE_DIR];
        if spec.kind == RuntimeKind::Environment {
            reserved.push(&docker.workspace_mount);
            reserved.push(&docker.home_mount);
        }
        reserved.extend(docker.mounts.iter().map(|mount| mount.target.as_str()));
        let check = self.config.check_host_paths;
        let mut planned = crate::mounts::plan(
            &spec.mounts,
            &reserved,
            self.daemon.facts().private_views(),
            &|path: &str| !check || std::path::Path::new(path).is_dir(),
        );
        for (name, why) in &planned.left_out {
            tracing::warn!(unit = %spec.name, mount = %name, "not attached: {why}");
        }
        self.require_volumes(&mut planned, &spec.mounts).await?;
        let prepared = overlay::prepare(daemon, config, &planned.views, &owner).await?;
        // Who the unit runs as.
        let helper = helper_name(&spec.name);
        let ids = self.ids_for(docker, &image, &helper).await?;
        // What the unit's own volumes are called.
        let run_volume = run_volume_name(&spec.name);
        let run_existed = daemon.volume(&run_volume).await?.is_some();
        volumes::ensure(
            daemon,
            config,
            &VolumeRequest {
                name: run_volume.clone(),
                storage: None,
                labels: BTreeMap::new(),
            },
        )
        .await?;
        let mut made_volumes: Vec<String> = if run_existed {
            Vec::new()
        } else {
            vec![run_volume.clone()]
        };
        let outcome = self
            .make_container(
                spec,
                docker,
                Made {
                    image,
                    network_plan,
                    gateway_entry,
                    tools,
                    planned,
                    prepared,
                    ids,
                    run_volume,
                    helper,
                    egress,
                },
                &mut made_volumes,
            )
            .await;
        if outcome.is_err() {
            for volume in made_volumes {
                if let Err(error) = volumes::remove(daemon, config, &volume).await {
                    tracing::warn!(%volume, %error, "a volume made for a unit that failed could not be removed");
                }
            }
        }
        outcome
    }

    async fn make_container(
        &self,
        spec: &RuntimeSpec,
        docker: &DockerSpec,
        made: Made,
        made_volumes: &mut Vec<String>,
    ) -> Result<UnitRef, DriverError> {
        let config = &*self.config;
        let daemon = &self.daemon;
        // The directories the volumes need: the top of each owned by the user, and the layers' own.
        let mut roots = vec![Root {
            volume: spec.volumes[&VolumeRole::State].name.clone(),
            role: "state".to_owned(),
            mode: PRIVATE_MODE,
            directories: Vec::new(),
        }];
        {
            for (role, label) in [
                (VolumeRole::Workspace, "workspace"),
                (VolumeRole::Home, "home"),
            ] {
                let Some(volume) = spec.volumes.get(&role) else {
                    continue;
                };
                if volume.directory.is_some() {
                    continue;
                }
                roots.push(Root {
                    volume: volume.name.clone(),
                    role: label.to_owned(),
                    mode: if spec.kind == RuntimeKind::Provider {
                        PRIVATE_MODE
                    } else {
                        WORK_MODE
                    },
                    directories: Vec::new(),
                });
            }
        }
        roots.push(Root {
            volume: made.run_volume.clone(),
            role: "run".to_owned(),
            mode: PRIVATE_MODE,
            directories: Vec::new(),
        });
        roots.extend(overlay::layer_roots(&made.prepared));
        let ids = made.ids.unwrap_or((0, 0));
        volumes::set_roots(
            daemon,
            config,
            &made.helper,
            &made.image,
            &roots,
            ids,
            self.mtime(),
        )
        .await?;
        let (view_mounts, view_volumes) =
            overlay::create_views(daemon, config, &made.prepared).await?;
        made_volumes.extend(view_volumes);
        let mut mounts = unit_mounts(spec, docker, &made.run_volume, &made.tools)?;
        mounts.extend(made.planned.direct.iter().cloned());
        mounts.extend(view_mounts);
        let boundary = self.boundary_of(docker.runtime.as_deref());
        // What the template holds from the start, and what was granted, must be something the runtime gives.
        let offered = crate::runtimes::capabilities_for(boundary);
        if let Some(key) = spec
            .capabilities
            .base
            .iter()
            .chain(&spec.capabilities.granted)
            .find(|key| {
                !orch_driver_contract::capabilities::gateway_given(key)
                    && !offered.contains_key(key.as_str())
            })
        {
            return Err(DriverError::Unsupported(format!(
                "the capability {key} is not offered under this runtime ({})",
                boundary.as_str()
            )));
        }
        // A guest with a kernel of its own cannot reach the daemon's resolver at 127.0.0.11: its resolver file is its
        // own, from the run volume, naming its proxy's filtered resolver when it is restricted.
        let mut extra_files = Vec::new();
        if boundary.has_kernel_of_its_own() {
            let nameservers = match &made.egress {
                Some((_, private)) => vec![private.proxy.to_string()],
                None => crate::guest::open_resolvers(config, &docker.network.dns),
            };
            extra_files.push(Entry::File {
                path: crate::guest::RESOLV_FILE.to_owned(),
                content: crate::guest::resolv_conf(&nameservers).into_bytes(),
                mode: CA_MODE,
                uid: ids.0,
                gid: ids.1,
            });
            mounts.push(crate::mounts::volume_file(
                &made.run_volume,
                crate::guest::RESOLV_FILE,
                crate::guest::RESOLV_TARGET,
            ));
        }
        let exclusive: Vec<String> = spec
            .volumes
            .values()
            .filter(|volume| volume.directory.is_none())
            .map(|volume| volume.name.clone())
            .collect();
        let plan = Plan {
            spec,
            docker,
            config,
            facts: daemon.facts(),
            image: made.image.clone(),
            network: made.network_plan.clone(),
            gateway_entry: made.gateway_entry.clone(),
            mounts,
            ids: Some(ids),
            exclusive,
            egress: made.egress.clone(),
            boundary,
        };
        let mut body = container_body(&plan)?;
        self.prepare_live_capabilities(spec, &mut body).await?;
        self.check_data_directories(spec, &body, ids).await?;
        if spec.kind == RuntimeKind::Environment {
            body.labels.get_or_insert_with(HashMap::new).insert(
                LABEL_MOUNTS_BASE.into(),
                mounts_file(&made.planned.attached),
            );
        }
        let created = daemon
            .docker()
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&spec.name)
                        .build(),
                ),
                body,
            )
            .await
            .map_err(|error| match &error {
                bollard::errors::Error::DockerResponseServerError {
                    status_code: 404,
                    message,
                } => DriverError::Image(message.clone()),
                bollard::errors::Error::DockerResponseServerError { message, .. }
                    if message.contains("does not exist") && message.contains("bind source") =>
                {
                    DriverError::NotFound(message.clone())
                }
                _ => map_error(error),
            })?;
        let mut run = run_files(spec, Some(&made.planned.attached), ids);
        run.extend(extra_files);
        let files = tar_of(&run, self.mtime());
        let put = match files {
            Ok(archive) => {
                daemon
                    .put_archive(&created.id, layout::RUN_DIR, archive)
                    .await
            }
            Err(error) => Err(error),
        };
        if let Err(error) = put {
            if let Err(removal) = daemon
                .docker()
                .remove_container(
                    &created.id,
                    Some(
                        RemoveContainerOptionsBuilder::default()
                            .force(true)
                            .v(true)
                            .build(),
                    ),
                )
                .await
            {
                tracing::warn!(unit = %spec.name, %removal, "a unit that could not be prepared could not be removed");
            }
            return Err(error);
        }
        tracing::info!(unit = %spec.name, container = %created.id, "made a unit");
        Ok(UnitRef {
            reference: created.id,
            name: spec.name.clone(),
            runtime_id: spec.runtime_id,
            kind: spec.kind,
        })
    }

    /// Whether a container is a unit with a kernel of its own, from the boundary it was made with.
    pub(crate) fn is_guest(&self, container: &ContainerInspectResponse) -> bool {
        self.boundary_of_unit(container).has_kernel_of_its_own()
    }

    /// What holds a container in: the boundary it was made with, else the one its runtime has now.
    pub(crate) fn boundary_of_unit(&self, container: &ContainerInspectResponse) -> Boundary {
        labels_of(container)
            .get(LABEL_BOUNDARY)
            .and_then(|word| Boundary::parse(word))
            .unwrap_or_else(|| {
                self.boundary_of(
                    container
                        .host_config
                        .as_ref()
                        .and_then(|host| host.runtime.as_deref()),
                )
            })
    }

    /// What holds a unit in under the runtime a spec names, or the daemon's default one where it names none.
    pub(crate) fn boundary_of(&self, runtime: Option<&str>) -> Boundary {
        let name = runtime.unwrap_or(self.daemon.facts().default_runtime.as_str());
        crate::runtimes::boundary_of(&self.config, name)
    }

    /// The numbers of the user the unit runs as: the spec's, the image's, or the root's.
    async fn ids_for(
        &self,
        docker: &DockerSpec,
        image: &str,
        helper: &str,
    ) -> Result<Option<(u32, u32)>, DriverError> {
        if let Some(numbers) = docker.user.numbers() {
            return Ok(Some(numbers));
        }
        let name = match &docker.user.name {
            Some(name) => Some(name.clone()),
            None => self
                .daemon
                .image(image)
                .await?
                .and_then(|inspect| inspect.config)
                .and_then(|config| config.user)
                .filter(|user| !user.is_empty()),
        };
        let Some(name) = name else {
            return Ok(Some((0, 0)));
        };
        // The image's own user may be written as numbers.
        let (user, group) = name
            .split_once(':')
            .map_or((name.as_str(), None), |(u, g)| (u, Some(g)));
        if let Ok(uid) = user.parse::<u32>() {
            let gid = group.and_then(|group| group.parse().ok()).unwrap_or(uid);
            return Ok(Some((uid, gid)));
        }
        volumes::user_numbers(&self.daemon, &self.config, helper, image, user)
            .await
            .map(Some)
    }

    /// The directories of the connector and the tools that the request asks for, in the daemon's terms.
    fn tool_binds(&self, spec: &RuntimeSpec) -> Result<ToolBinds, DriverError> {
        let config = &*self.config;
        let connector = config.tool_cache.connector_dir.as_ref().ok_or_else(|| {
            DriverError::Unavailable("this host has no connector to put into a unit".to_owned())
        })?;
        let check = |path: &std::path::Path, what: &str| -> Result<(), DriverError> {
            if config.check_host_paths && !path.is_dir() {
                return Err(DriverError::NotFound(format!(
                    "{what} is not in the tool cache of this host"
                )));
            }
            Ok(())
        };
        check(connector, "the connector")?;
        // Where a link is the directory it points to is bound, so that a unit stays on the build it was made with.
        let resolve = |path: &std::path::Path| {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
        };
        let mut tools = Vec::new();
        for tool in &spec.tools {
            let cache = config.tool_cache.dir.as_ref().ok_or_else(|| {
                DriverError::Unavailable("this host has no tool cache".to_owned())
            })?;
            let directory = cache.join(&tool.name).join(&tool.version);
            check(&directory, &format!("{} {}", tool.name, tool.version))?;
            tools.push((
                tool.name.clone(),
                config
                    .host_path(&resolve(&directory))
                    .to_string_lossy()
                    .into_owned(),
            ));
        }
        Ok(ToolBinds {
            connector: config
                .host_path(&resolve(connector))
                .to_string_lossy()
                .into_owned(),
            tools,
        })
    }

    /// Moves the mounts whose volume is not there from the attached to the left out.
    async fn require_volumes(
        &self,
        planned: &mut crate::mounts::Planned,
        asked: &[MountSpec],
    ) -> Result<(), DriverError> {
        let mut missing: Vec<String> = Vec::new();
        for mount in asked {
            if mount.kind == orch_driver_contract::mounts::MountKind::Volume
                && planned
                    .attached
                    .iter()
                    .any(|attached| attached.name == mount.name)
                && self.daemon.volume(&mount.source).await?.is_none()
            {
                missing.push(mount.name.clone());
            }
        }
        for name in missing {
            planned.attached.retain(|attached| attached.name != name);
            planned.views.retain(|view| view.mount.name != name);
            if let Some(source) = asked
                .iter()
                .find(|mount| mount.name == name)
                .map(|mount| mount.source.clone())
            {
                planned
                    .direct
                    .retain(|mount| mount.source.as_deref() != Some(source.as_str()));
                planned
                    .left_out
                    .push((name, format!("the volume {source} does not exist")));
            }
        }
        Ok(())
    }

    /// Starts a unit; see `EnvironmentDriver::start`.
    pub(crate) async fn start_unit(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        let mut container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| not_a_unit(unit))?;
        // A unit made on the shared network once given to units without a way out is never started there again: it
        // is made again on a network of its own, with its proxy, first.
        if restricted::on_old_network(&container, &self.config) {
            container = self.remake_restricted(unit, &container).await?;
        }
        // An open unit made on a common bridge starts on a network of its own, its container kept.
        if restricted::reach_of(&container) == Reach::Open {
            container = self.settle_own_network(unit, container).await?;
        }
        // A restricted unit's proxy runs before the unit, which has no other way out.
        if restricted::reach_of(&container) != Reach::Open {
            restricted::start_proxy(&self.daemon, &self.config, &unit.name, self.clock.as_ref())
                .await?;
        }
        let id = container.id.clone().unwrap_or_default();
        let exclusive = labels_of(&container)
            .get(LABEL_EXCLUSIVE)
            .cloned()
            .unwrap_or_default();
        for volume in exclusive.split(',').filter(|volume| !volume.is_empty()) {
            if let Some(other) = self.running_on(volume, &id).await? {
                return Err(DriverError::Internal(format!(
                    "the unit {other} is running on the volume {volume}; two units never run on one volume"
                )));
            }
        }
        if container
            .state
            .as_ref()
            .is_none_or(|state| state.running != Some(true))
            && let Some(base) = labels_of(&container).get(LABEL_MOUNTS_BASE)
        {
            let mounts: Vec<AttachedMount> = serde_json::from_str(base).map_err(|_| {
                DriverError::InvalidSpec("the unit's ordinary mount record is invalid".into())
            })?;
            self.daemon
                .put_archive(
                    &id,
                    "/run/orch",
                    crate::live_mounts::mounts_archive(&mounts)?,
                )
                .await?;
        }
        self.daemon
            .docker()
            .start_container(
                &id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(map_error)?;
        if let Some(gigabytes) = labels_of(&container)
            .get(LABEL_DOCKER_INSIDE)
            .and_then(|gigabytes| gigabytes.parse::<u64>().ok())
        {
            // The data place of Docker inside is a mount of the guest, gone when the unit stops: it is made again at
            // every start, before anything runs there that would use it.
            let place = crate::guest::DataPlace::under(self.boundary_of_unit(&container));
            crate::guest::prepare_docker_inside(&self.daemon, &id, gigabytes, place).await?;
        }
        tracing::info!(unit = %unit.name, "started a unit");
        Ok(())
    }

    /// A running container other than `except` that has the volume, if there is one.
    async fn running_on(&self, volume: &str, except: &str) -> Result<Option<String>, DriverError> {
        let filters: HashMap<String, Vec<String>> = HashMap::from([
            ("volume".to_owned(), vec![volume.to_owned()]),
            ("status".to_owned(), vec!["running".to_owned()]),
        ]);
        let found = self
            .daemon
            .docker()
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await
            .map_err(map_error)?;
        Ok(found
            .into_iter()
            .find(|container| container.id.as_deref() != Some(except))
            .map(|container| {
                container
                    .names
                    .and_then(|names| names.into_iter().next())
                    .map_or_else(
                        || "another".to_owned(),
                        |name| name.trim_start_matches('/').to_owned(),
                    )
            }))
    }

    /// Stops a unit; see `EnvironmentDriver::stop`.
    pub(crate) async fn stop_unit(
        &self,
        unit: &UnitRef,
        grace: Duration,
    ) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| not_a_unit(unit))?;
        let id = container.id.clone().unwrap_or_default();
        let seconds = i32::try_from(grace.as_secs() + u64::from(grace.subsec_nanos() > 0))
            .unwrap_or(i32::MAX);
        let patient = self.daemon.patient(grace + STOP_REQUEST_SLACK);
        patient
            .stop_container(
                &id,
                Some(StopContainerOptionsBuilder::default().t(seconds).build()),
            )
            .await
            .map_err(map_error)?;
        // The token of one run does not outlive it.
        if let Some(ids) = ids_of(&container) {
            let empty = tar_of(
                &[Entry::File {
                    path: "enroll".to_owned(),
                    content: Vec::new(),
                    mode: SECRET_MODE,
                    uid: ids.0,
                    gid: ids.1,
                }],
                self.mtime(),
            )?;
            if let Err(error) = self.daemon.put_archive(&id, layout::RUN_DIR, empty).await {
                tracing::warn!(unit = %unit.name, %error, "the enrollment file of a stopped unit could not be emptied");
            }
        }
        // A restricted unit's proxy stops after the unit.
        if restricted::reach_of(&container) != Reach::Open
            && let Err(error) = restricted::stop_proxy(&self.daemon, &unit.name).await
        {
            tracing::warn!(unit = %unit.name, %error, "the proxy of a stopped unit could not be stopped");
        }
        tracing::info!(unit = %unit.name, "stopped a unit");
        Ok(())
    }

    /// Removes a unit and the files of its runs, never its volumes; see `EnvironmentDriver::remove`.
    pub(crate) async fn remove_unit(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        if let Some(container) = self.container_of(unit).await? {
            let id = container.id.clone().unwrap_or_default();
            // Anonymous volumes the image declared go with the unit, which is why a unit is removed with `v`;
            // named volumes are never removed by it.
            match self
                .daemon
                .docker()
                .remove_container(
                    &id,
                    Some(
                        RemoveContainerOptionsBuilder::default()
                            .force(true)
                            .v(true)
                            .build(),
                    ),
                )
                .await
            {
                Ok(()) => {}
                Err(error) if status_of(&error) == Some(404) => {}
                Err(error) => return Err(map_error(error)),
            }
        } else if self.daemon.container(&unit.name).await?.is_some() {
            return Err(not_a_unit(unit));
        }
        // The unit's own network goes with it, and a restricted unit's proxy, its way out and its directory; what an
        // open unit has none of costs answers of not found.
        if let Err(error) = restricted::remove_all(&self.daemon, &self.config, &unit.name).await {
            tracing::warn!(unit = %unit.name, %error, "the proxy or the network of a removed unit could not be removed");
        }
        volumes::remove_helper(&self.daemon, &helper_name(&unit.name)).await?;
        volumes::remove(&self.daemon, &self.config, &run_volume_name(&unit.name)).await?;
        tracing::info!(unit = %unit.name, "removed a unit");
        Ok(())
    }

    /// The state of a unit; see `EnvironmentDriver::inspect`.
    pub(crate) async fn inspect_unit(&self, unit: &UnitRef) -> Result<UnitState, DriverError> {
        Ok(match self.container_of(unit).await? {
            Some(container) => state_of(&container),
            None => UnitState::of(UnitStateKind::Absent),
        })
    }

    /// Every unit of the system; see `EnvironmentDriver::list`.
    pub(crate) async fn list_units(&self, system: &str) -> Result<Vec<UnitRef>, DriverError> {
        let filters: HashMap<String, Vec<String>> = HashMap::from([(
            "label".to_owned(),
            vec![
                format!("{LABEL_SYSTEM}={system}"),
                format!("{LABEL_DRIVER}={DRIVER_NAME}"),
            ],
        )]);
        let found: Vec<ContainerSummary> = self
            .daemon
            .docker()
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&filters)
                    .build(),
            ))
            .await
            .map_err(map_error)?;
        let mut units = Vec::new();
        for container in found {
            let labels = container.labels.clone().unwrap_or_default();
            let name = container
                .names
                .as_ref()
                .and_then(|names| names.first())
                .map(|name| name.trim_start_matches('/').to_owned())
                .unwrap_or_default();
            // Whatever was found by the system label is checked once more: the system must be exactly the one asked.
            if labels.get(LABEL_SYSTEM).map(String::as_str) != Some(system) {
                continue;
            }
            // A restricted unit's proxy is the node's and no unit.
            if labels.contains_key(restricted::LABEL_PROXY_OF) {
                continue;
            }
            match unit_from_labels(&labels, container.id.as_deref().unwrap_or_default(), &name) {
                Some(unit) => units.push(unit),
                None => {
                    tracing::warn!(container = %name, "a container of the system has labels that do not name a unit")
                }
            }
        }
        Ok(units)
    }
}

/// What making the container is given after the steps before it.
struct Made {
    image: String,
    network_plan: NetworkPlan,
    gateway_entry: String,
    tools: ToolBinds,
    planned: crate::mounts::Planned,
    prepared: Vec<overlay::Prepared>,
    ids: Option<(u32, u32)>,
    run_volume: String,
    helper: String,
    egress: Option<(Reach, Private)>,
}

fn not_a_unit(unit: &UnitRef) -> DriverError {
    DriverError::NotFound(format!("there is no unit {} of this driver", unit.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const RUNTIME: &str = "01990000-0000-7000-8000-0199abcd1234";

    fn request(kind: &str) -> RuntimeSpec {
        let unit = if kind == "environment" {
            "orch-environment-0199abcd1234"
        } else {
            "orch-provider-0199abcd1234"
        };
        let volumes = if kind == "environment" {
            json!({ "workspace": { "name": "orch-ws-e1", "storage": null },
                    "home": { "name": "orch-home-e1", "storage": null },
                    "state": { "name": "orch-state-e1", "storage": null } })
        } else {
            json!({ "state": { "name": "orch-state-p1", "storage": null } })
        };
        serde_json::from_value(json!({
            "runtime_id": RUNTIME, "kind": kind, "name": unit, "driver": "docker",
            "spec": { "image": "busybox:latest", "user": { "name": "agent", "uid": 1000, "gid": 1000 },
                      "security": { "read_only_rootfs": true, "no_new_privileges": true, "cap_add": ["CHOWN"], "cap_drop": ["ALL"] },
                      "network": { "mode": "bridge", "egress": "open", "dns": ["1.1.1.1"] }, "runtime": "runc",
                      "mounts": [ { "type": "tmpfs", "target": "/tmp" } ] },
            "resources": { "cpu_millis": 4000, "memory_mb": 8192, "pids": 4096, "disk_gb": 50 },
            "roles": [ if kind == "environment" { "environment" } else { "provider" } ],
            "gateway_url": "wss://gateway.orch.internal:9443", "enroll_token": "enroll-123",
            "ca_cert": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
            "tools": [ { "name": "codex", "version": "0.160.1" } ],
            "volumes": volumes, "mounts": [], "env": { "NAME": "value" },
            "labels": { "orch.system": "prod", "orch.runtime_id": RUNTIME, "orch.kind": kind }
        }))
        .unwrap()
    }

    fn facts() -> DaemonFacts {
        DaemonFacts {
            memory_limit: true,
            swap_limit: true,
            cpu_quota: true,
            pids_limit: true,
            cpus: 10,
            os_type: "linux".into(),
            cgroup_version: 2,
            ..DaemonFacts::default()
        }
    }

    fn body(kind: &str, facts: &DaemonFacts) -> ContainerCreateBody {
        let spec = request(kind);
        let docker = DockerSpec::parse(spec.kind, &spec.spec).unwrap();
        let config = DockerConfig::default();
        let tools = ToolBinds {
            connector: "/cache/connector".into(),
            tools: vec![("codex".into(), "/cache/codex/0.160.1".into())],
        };
        let mounts =
            unit_mounts(&spec, &docker, "orch-environment-0199abcd1234-run", &tools).unwrap();
        let network = NetworkPlan {
            name: networks::unit_network_name(&spec.name),
            role: networks::ROLE_UNIT.to_owned(),
        };
        container_body(&Plan {
            spec: &spec,
            docker: &docker,
            config: &config,
            facts,
            image: "busybox:latest".into(),
            network,
            gateway_entry: "gateway.orch.internal:host-gateway".into(),
            mounts,
            ids: Some((1000, 1000)),
            exclusive: vec!["orch-ws-e1".into()],
            egress: None,
            boundary: Boundary::SharedKernel,
        })
        .unwrap()
    }

    #[test]
    fn limits_are_asked_of_the_daemon_in_its_own_units() {
        let created = body("environment", &facts());
        let host = created.host_config.unwrap();
        assert_eq!(host.nano_cpus, Some(4_000_000_000));
        assert_eq!(host.memory, Some(8192 * 1024 * 1024));
        assert_eq!(host.memory_swap, host.memory);
        assert_eq!(host.pids_limit, Some(4096));
        assert_eq!(
            host.storage_opt, None,
            "no quotas on this storage; the disk is measured instead"
        );
    }

    #[test]
    fn a_limit_the_host_cannot_enforce_is_left_off_and_said() {
        let mut host = HostConfig::default();
        let notes = apply_limits(
            &Resources {
                cpu_millis: Some(4000),
                memory_mb: Some(512),
                pids: Some(10),
                disk_gb: Some(5),
                ..Resources::default()
            },
            &DaemonFacts::default(),
            &mut host,
        );
        assert_eq!(notes.len(), 4);
        assert_eq!(host, HostConfig::default());
        let mut host = HostConfig::default();
        apply_limits(
            &Resources {
                cpu_millis: Some(64_000),
                ..Resources::default()
            },
            &facts(),
            &mut host,
        );
        assert_eq!(
            host.nano_cpus,
            Some(10_000_000_000),
            "no more processors than the host has"
        );
        let mut host = HostConfig::default();
        apply_limits(
            &Resources {
                disk_gb: Some(5),
                ..Resources::default()
            },
            &DaemonFacts {
                disk_quota: true,
                ..facts()
            },
            &mut host,
        );
        assert_eq!(host.storage_opt.unwrap()["size"], "5G");
    }

    #[test]
    fn the_main_process_is_the_connector_with_the_launch_variables_and_no_restart_behind_anybodys_back()
     {
        let created = body("environment", &facts());
        assert_eq!(
            created.entrypoint.unwrap(),
            ["/opt/orch/bin/orch-connector"]
        );
        assert_eq!(created.cmd.unwrap(), ["run", "--managed"]);
        assert_eq!(created.user.as_deref(), Some("1000:1000"));
        let env = created.env.unwrap();
        for expected in [
            "ORCH_ROLES=environment",
            "ORCH_GATEWAY_URL=wss://gateway.orch.internal:9443",
            "ORCH_CA_FILE=/run/orch/ca.pem",
            "ORCH_ENROLL_FILE=/run/orch/enroll",
            "ORCH_DATA_DIR=/var/lib/orch",
            "ORCH_TOOLS_DIR=/opt/orch/tools",
            "ORCH_WORKSPACE_DIR=/workspace",
            "ORCH_HOME_DIR=/home/agent",
            "ORCH_MOUNTS_FILE=/run/orch/mounts.json",
            "NAME=value",
            "HOME=/home/agent",
        ] {
            assert!(env.contains(&expected.to_owned()), "{expected} in {env:?}");
        }
        assert!(
            !env.iter().any(|line| line.contains("enroll-123")),
            "the token is a file and never a variable"
        );
        let host = created.host_config.unwrap();
        assert_eq!(host.init, Some(true));
        assert_eq!(
            host.restart_policy.unwrap().name,
            Some(RestartPolicyNameEnum::NO)
        );
    }

    #[test]
    fn a_providers_runtime_gets_the_variables_of_its_pool_alone() {
        let created = body("provider", &facts());
        let env = created.env.unwrap();
        assert!(env.contains(&"ORCH_ROLES=provider".to_owned()));
        assert!(env.contains(&"ORCH_DATA_DIR=/var/lib/orch".to_owned()));
        assert!(!env.iter().any(
            |line| line.starts_with("ORCH_WORKSPACE_DIR") || line.starts_with("ORCH_HOME_DIR")
        ));
        assert!(!env.iter().any(|line| line.starts_with("ORCH_MOUNTS_FILE")));
        assert!(env.contains(&"HOME=/var/lib/orch".to_owned()));
        assert_eq!(created.working_dir.as_deref(), Some("/"));
    }

    #[test]
    fn the_unit_is_confined_as_the_spec_says_and_found_by_its_labels() {
        let created = body("environment", &facts());
        let host = created.host_config.unwrap();
        assert_eq!(host.readonly_rootfs, Some(true));
        assert_eq!(host.cap_add.unwrap(), ["CHOWN"]);
        assert_eq!(host.cap_drop.unwrap(), ["ALL"]);
        assert_eq!(host.security_opt.unwrap(), ["no-new-privileges"]);
        assert_eq!(host.runtime.as_deref(), Some("runc"));
        assert_eq!(host.dns.unwrap(), ["1.1.1.1"]);
        assert_eq!(
            host.network_mode.as_deref(),
            Some("orch-environment-0199abcd1234-net"),
            "a unit stands on a network of its own, never on a bridge it shares"
        );
        assert_eq!(
            host.extra_hosts.unwrap(),
            ["gateway.orch.internal:host-gateway"]
        );
        let labels = created.labels.unwrap();
        assert_eq!(labels["orch.system"], "prod");
        assert_eq!(labels["orch.runtime_id"], RUNTIME);
        assert_eq!(labels["orch.kind"], "environment");
        assert_eq!(labels["orch.driver"], "docker");
        assert_eq!(labels["orch.user_ids"], "1000:1000");
        assert_eq!(labels[LABEL_EXCLUSIVE], "orch-ws-e1");
    }

    #[test]
    fn the_mounts_are_the_volumes_the_run_files_the_connector_and_the_tools_read_only() {
        let created = body("environment", &facts());
        let mounts = created.host_config.unwrap().mounts.unwrap();
        let at = |target: &str| {
            mounts
                .iter()
                .find(|m| m.target.as_deref() == Some(target))
                .unwrap_or_else(|| panic!("{target}"))
        };
        assert_eq!(at("/workspace").source.as_deref(), Some("orch-ws-e1"));
        assert_eq!(at("/home/agent").source.as_deref(), Some("orch-home-e1"));
        assert_eq!(at("/var/lib/orch").source.as_deref(), Some("orch-state-e1"));
        assert_eq!(
            at("/run/orch").source.as_deref(),
            Some("orch-environment-0199abcd1234-run")
        );
        let connector = at("/opt/orch/bin");
        assert_eq!(
            (connector.typ, connector.read_only),
            (Some(MountType::BIND), Some(true))
        );
        assert_eq!(connector.source.as_deref(), Some("/cache/connector"));
        let tool = at("/opt/orch/tools/codex");
        assert_eq!(
            (tool.typ, tool.read_only),
            (Some(MountType::BIND), Some(true))
        );
        assert_eq!(at("/opt/orch/tools").typ, Some(MountType::TMPFS));
        assert_eq!(at("/tmp").typ, Some(MountType::TMPFS));
    }

    #[test]
    fn the_files_of_a_run_are_the_authority_the_token_and_the_mounts_owned_by_the_user() {
        let spec = request("environment");
        let files = run_files(
            &spec,
            Some(&[AttachedMount {
                name: "src".into(),
                path: "/data/src".into(),
                mode: orch_driver_contract::mounts::MountMode::ReadOnly,
            }]),
            (1000, 1001),
        );
        let find = |wanted: &str| {
            files.iter().find_map(|entry| match entry {
                Entry::File {
                    path,
                    content,
                    mode,
                    uid,
                    gid,
                } if path == wanted => Some((content.clone(), *mode, *uid, *gid)),
                _ => None,
            })
        };
        assert_eq!(
            find("enroll"),
            Some((b"enroll-123".to_vec(), 0o600, 1000, 1001))
        );
        assert!(
            String::from_utf8(find("ca.pem").unwrap().0)
                .unwrap()
                .starts_with("-----BEGIN")
        );
        let mounts: serde_json::Value =
            serde_json::from_slice(&find("mounts.json").unwrap().0).unwrap();
        assert_eq!(
            mounts,
            json!([ { "name": "src", "path": "/data/src", "mode": "read_only" } ])
        );
        assert!(
            run_files(&request("provider"), Some(&[]), (0, 0))
                .iter()
                .all(|e| !matches!(e, Entry::File { path, .. } if path == "mounts.json"))
        );
    }

    #[test]
    fn a_container_is_the_unit_of_a_runtime_only_with_the_drivers_mark_and_the_same_labels() {
        let labels = HashMap::from([
            (LABEL_DRIVER.to_owned(), DRIVER_NAME.to_owned()),
            (LABEL_SYSTEM.to_owned(), "prod".to_owned()),
            (LABEL_RUNTIME_ID.to_owned(), RUNTIME.to_owned()),
            (LABEL_KIND.to_owned(), "provider".to_owned()),
        ]);
        let unit = unit_from_labels(&labels, "abc", "orch-provider-0199abcd1234").unwrap();
        assert_eq!(unit.kind, RuntimeKind::Provider);
        assert_eq!(unit.runtime_id.to_string(), RUNTIME);
        for missing in [LABEL_DRIVER, LABEL_SYSTEM, LABEL_RUNTIME_ID, LABEL_KIND] {
            let mut without = labels.clone();
            without.remove(missing);
            assert_eq!(unit_from_labels(&without, "abc", "n"), None, "{missing}");
        }
        let mut other = labels.clone();
        other.insert(LABEL_SYSTEM.to_owned(), String::new());
        assert_eq!(unit_from_labels(&other, "abc", "n"), None);
    }

    fn inspected(state: serde_json::Value) -> ContainerInspectResponse {
        serde_json::from_value(json!({ "Id": "abc", "Name": "/u", "State": state })).unwrap()
    }

    #[test]
    fn the_state_of_a_container_is_one_of_four_with_the_detail_the_daemon_gives() {
        let running = state_of(&inspected(
            json!({ "Status": "running", "Running": true, "ExitCode": 0,
            "StartedAt": "2026-10-11T10:00:00.123456789Z", "FinishedAt": "0001-01-01T00:00:00Z" }),
        ));
        assert_eq!(running.state, UnitStateKind::Running);
        assert_eq!(
            running.started_at.unwrap().to_string(),
            "2026-10-11T10:00:00.123Z"
        );
        assert_eq!(running.finished_at, None);
        let paused = state_of(&inspected(
            json!({ "Status": "paused", "Running": true, "Paused": true }),
        ));
        assert_eq!(
            (paused.state, paused.detail.as_deref()),
            (UnitStateKind::Running, Some("paused"))
        );
        let created = state_of(&inspected(json!({ "Status": "created" })));
        assert_eq!(created.state, UnitStateKind::Stopped);
        let exited = state_of(&inspected(json!({ "Status": "exited", "ExitCode": 0,
            "FinishedAt": "2026-10-11T10:05:00Z" })));
        assert_eq!(
            (exited.state, exited.exit_code),
            (UnitStateKind::Stopped, Some(0))
        );
        assert_eq!(
            exited.finished_at.unwrap().to_string(),
            "2026-10-11T10:05:00.000Z"
        );
        let crashed = state_of(&inspected(json!({ "Status": "exited", "ExitCode": 3 })));
        assert_eq!(
            (crashed.state, crashed.exit_code),
            (UnitStateKind::Failed, Some(3))
        );
        assert_eq!(crashed.detail.as_deref(), Some("exited with status 3"));
    }

    #[test]
    fn a_unit_ended_by_a_signal_was_stopped_unless_it_ran_out_of_memory() {
        for code in [137, 143] {
            let stopped = state_of(&inspected(
                json!({ "Status": "exited", "ExitCode": code, "OOMKilled": false }),
            ));
            assert_eq!(stopped.state, UnitStateKind::Stopped, "{code}");
        }
        let oom = state_of(&inspected(
            json!({ "Status": "exited", "ExitCode": 137, "OOMKilled": true }),
        ));
        assert_eq!(
            (oom.state, oom.detail.as_deref()),
            (UnitStateKind::Failed, Some("out of memory"))
        );
        let refused = state_of(&inspected(
            json!({ "Status": "created", "Error": "exec: no such file" }),
        ));
        assert_eq!(
            (refused.state, refused.detail.as_deref()),
            (UnitStateKind::Failed, Some("exec: no such file"))
        );
        let removing = state_of(&inspected(json!({ "Status": "removing" })));
        assert_eq!(removing.state, UnitStateKind::Absent);
        assert_eq!(
            state_of(&ContainerInspectResponse::default()).state,
            UnitStateKind::Absent
        );
    }

    #[test]
    fn names_of_units_are_the_names_the_daemon_takes() {
        assert!(is_container_name("orch-environment-0199abcd1234"));
        assert!(
            !is_container_name("")
                && !is_container_name("-x")
                && !is_container_name("a/b")
                && !is_container_name("a b")
        );
        assert_eq!(run_volume_name("u"), "u-run");
        assert_eq!(helper_name("u"), "u-prep");
    }

    #[tokio::test]
    async fn commands_for_one_unit_wait_for_each_other_and_commands_for_two_do_not() {
        let locks = Arc::new(KeyedLocks::default());
        let first = locks.lock("a").await;
        let other = tokio::time::timeout(Duration::from_millis(200), locks.lock("b")).await;
        assert!(other.is_ok(), "another unit is not held up");
        let same = tokio::time::timeout(Duration::from_millis(100), locks.lock("a")).await;
        assert!(same.is_err(), "the same unit waits");
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), locks.lock("a"))
                .await
                .is_ok()
        );
    }
}
