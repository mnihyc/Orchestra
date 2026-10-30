//! Live mounts and device nodes through the node's one-shot static helper in an owned unit's namespace.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use bollard::models::{ContainerCreateBody, HostConfig, Mount};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, LogsOptionsBuilder, StartContainerOptions, WaitContainerOptions,
};
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::mounts::check_target;
use orch_driver_contract::mounts::{AttachedMount, MountChange, MountKind, MountMode, MountSpec};
use orch_driver_contract::runtime::LABEL_SYSTEM;
use orch_driver_contract::units::UnitRef;

use crate::client::Daemon;
use crate::driver::DockerDriver;
use crate::units::{LABEL_PREPARE, driver_labels};
use crate::volumes::remove_helper;

/// One namespace operation gets thirty seconds, well above a local mount syscall but bounded if Docker stalls.
const HELPER_WAIT: Duration = Duration::from_secs(30);
/// Failed helpers leave a small diagnostic, without delaying rollback on an unresponsive daemon.
const HELPER_LOG_WAIT: Duration = Duration::from_secs(2);
/// A single trusted operation needs only its last error, never an unbounded Docker log.
const HELPER_LOG_BYTES: usize = 4096;
/// The single immutable helper executable inside its own image.
const HELPER_BINARY: &str = "/orch-runtime-helper";
/// Source mounts have exactly one name in that image.
const SOURCE: &str = "/src";

fn helper_failure(operation: &str, code: i64) -> DriverError {
    DriverError::Unsupported(format!(
        "the live {operation} helper exited with status {code}; ask the host's manager to check the node log before retrying, or recreate the environment"
    ))
}

async fn helper_diagnostic(daemon: &Daemon, helper: &str) -> String {
    tokio::time::timeout(HELPER_LOG_WAIT, async {
        let mut logs = daemon.docker().logs(
            helper,
            Some(
                LogsOptionsBuilder::default()
                    .stdout(true)
                    .stderr(true)
                    .tail("20")
                    .build(),
            ),
        );
        let mut text = String::new();
        while let Some(Ok(line)) = logs.next().await {
            for ch in line.to_string().chars() {
                if text.len() + ch.len_utf8() > HELPER_LOG_BYTES {
                    return text;
                }
                text.push(ch);
            }
        }
        text
    })
    .await
    .unwrap_or_else(|_| "the helper log could not be read before its deadline".into())
}

#[derive(Debug, Clone)]
pub(crate) struct Applied {
    namespace: String,
    specs: Vec<MountSpec>,
    mounted: Vec<AttachedMount>,
}

/// The helper's valid operations; attach and device permissions cannot be combined accidentally.
#[derive(Debug)]
pub(crate) enum NamespaceChange {
    Attach {
        source: Mount,
        read_only: bool,
    },
    Detach,
    GiveDevice {
        source: Mount,
        major: u64,
        minor: u64,
    },
    RemoveDevice,
}

pub(crate) async fn kernel_supported(daemon: &Daemon) -> bool {
    let Ok(info) = daemon.docker().info().await else {
        return false;
    };
    if info.os_type.as_deref() != Some("linux")
        || info
            .operating_system
            .as_deref()
            .is_some_and(|name| name.contains("Docker Desktop"))
        || daemon.facts().rootless
    {
        return false;
    }
    kernel_at_least_512(info.kernel_version.as_deref().unwrap_or_default())
}

fn kernel_at_least_512(version: &str) -> bool {
    let mut components = version.split('.');
    matches!((components.next().and_then(|part| part.parse::<u32>().ok()), components.next().and_then(|part| part.parse::<u32>().ok())), (Some(major), Some(minor)) if (major,minor)>=(5,12))
}

/// A static ELF64 binary has no interpreter whose code could come from an untrusted unit image.
pub fn static_binary(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 64];
    if file.read_exact(&mut header).is_err()
        || &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
    {
        return false;
    }
    let offset = u64::from_le_bytes(header[32..40].try_into().unwrap_or([0; 8]));
    let width = u16::from_le_bytes([header[54], header[55]]) as u64;
    let count = u16::from_le_bytes([header[56], header[57]]) as u64;
    // Node binaries have a handful of segments; bound malformed operator input without reading an arbitrary file.
    if width < 32 || count == 0 || count > 1024 {
        return false;
    }
    for index in 0..count {
        let Some(at) = index
            .checked_mul(width)
            .and_then(|part| offset.checked_add(part))
        else {
            return false;
        };
        let mut kind = [0u8; 4];
        if file.seek(SeekFrom::Start(at)).is_err() || file.read_exact(&mut kind).is_err() {
            return false;
        }
        if u32::from_le_bytes(kind) == 3 {
            return false;
        } // ELF PT_INTERP
    }
    true
}

impl DockerDriver {
    pub(crate) async fn change_live_mounts(
        &self,
        unit: &UnitRef,
        change: &MountChange,
    ) -> Result<Vec<AttachedMount>, DriverError> {
        let driver = self.clone();
        let unit = unit.clone();
        let change = change.clone();
        // Losing the waiting command does not abandon a namespace halfway through its transition.
        tokio::spawn(async move { driver.change_live_mounts_held(&unit, &change).await })
            .await
            .map_err(|_| {
                DriverError::Unavailable(
                    "the live mount transaction could not finish; recreate is needed".into(),
                )
            })?
    }

    async fn change_live_mounts_held(
        &self,
        unit: &UnitRef,
        change: &MountChange,
    ) -> Result<Vec<AttachedMount>, DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        if !self.live_helpers_supported() {
            return Err(DriverError::Unsupported(
                "this host cannot safely apply mounts live; recreate is needed".into(),
            ));
        }
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        // The helper joins the unit's namespaces from the host, which a unit with a kernel of its own does not share.
        if self.is_guest(&container) {
            return Err(DriverError::Unsupported(
                "this unit's runtime has a kernel of its own, which the host cannot reach to change its mounts; recreate is needed".into(),
            ));
        }
        let namespace = format!(
            "{}:{}",
            container.id.as_deref().unwrap_or_default(),
            container
                .state
                .as_ref()
                .and_then(|state| state.started_at.as_deref())
                .unwrap_or_default()
        );
        let cached = self
            .live_mounts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&unit.reference)
            .cloned()
            .filter(|cached| cached.namespace == namespace);
        if let Some(cached) = &cached
            && cached.specs == change.after
        {
            return Ok(cached.mounted.clone());
        }
        let before = cached
            .as_ref()
            .map_or(change.before.as_slice(), |cached| cached.specs.as_slice());
        let mut reserved = change.reserved.clone();
        reserved.extend(["/run/orch", "/opt/orch", "/proc", "/sys"].map(str::to_owned));
        let reserved: Vec<_> = reserved.iter().map(String::as_str).collect();
        let (after_mounts, attached) = self
            .live_sources(unit, &container, &change.after, &reserved)
            .await?;
        let (before_mounts, before_attached) = self
            .live_sources(unit, &container, before, &reserved)
            .await?;
        let archive = mounts_archive(&attached)?;
        let before_archive = mounts_archive(&before_attached)?;
        let mut actual = before.to_vec();
        let outcome = async {
            self.live_transition(unit, &mut actual, &change.after, &after_mounts)
                .await?;
            // Publish the accepted set only after every namespace operation has finished.
            self.daemon
                .put_archive(&unit.reference, "/run/orch", archive)
                .await
        }
        .await;
        if let Err(error) = outcome {
            let restored = async {
                self.live_transition(unit, &mut actual, before, &before_mounts)
                    .await?;
                self.daemon
                    .put_archive(&unit.reference, "/run/orch", before_archive)
                    .await
            }
            .await;
            if restored.is_err() {
                let _ = self
                    .daemon
                    .docker()
                    .stop_container(
                        &unit.reference,
                        None::<bollard::query_parameters::StopContainerOptions>,
                    )
                    .await;
                self.live_mounts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&unit.reference);
                return Err(DriverError::Unavailable("the live changes could not be restored safely; the unit was stopped for recreate".into()));
            }
            return Err(error);
        }
        self.live_mounts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                unit.reference.clone(),
                Applied {
                    namespace,
                    specs: change.after.clone(),
                    mounted: attached.clone(),
                },
            );
        Ok(attached)
    }

    async fn live_sources(
        &self,
        unit: &UnitRef,
        container: &bollard::models::ContainerInspectResponse,
        specs: &[MountSpec],
        reserved: &[&str],
    ) -> Result<(HashMap<String, Mount>, Vec<AttachedMount>), DriverError> {
        let planned = crate::mounts::plan(specs, reserved, self.facts().private_views(), &|path| {
            !self.config.check_host_paths || Path::new(path).is_dir()
        });
        if !planned.left_out.is_empty() {
            return Err(DriverError::InvalidSpec(
                "a live mount's target or location is unavailable; recreate is needed".into(),
            ));
        }
        for spec in specs.iter().filter(|spec| spec.kind == MountKind::Volume) {
            if self.daemon.volume(&spec.source).await?.is_none() {
                return Err(DriverError::NotFound(
                    "a selected mount volume is unavailable".into(),
                ));
            }
        }
        let mut mounts = planned.direct;
        if !planned.views.is_empty() {
            // A layer made for a live view records the unit's system, as one made with the unit does.
            let owner: std::collections::BTreeMap<String, String> = container
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref())
                .and_then(|labels| labels.get(LABEL_SYSTEM))
                .map(|system| (LABEL_SYSTEM.to_owned(), system.clone()))
                .into_iter()
                .collect();
            let prepared =
                crate::overlay::prepare(&self.daemon, &self.config, &planned.views, &owner).await?;
            let user = container
                .config
                .as_ref()
                .and_then(|config| config.user.as_deref())
                .unwrap_or("0:0");
            let ids = if user.is_empty() || user == "root" {
                (0, 0)
            } else {
                let (uid, gid) = user.split_once(':').ok_or_else(|| {
                    DriverError::Unsupported(
                        "the unit's numeric user/group is unknown; private changes need recreate"
                            .into(),
                    )
                })?;
                (
                    uid.parse().map_err(|_| {
                        DriverError::Unsupported("the unit's user is not numeric".into())
                    })?,
                    gid.parse().map_err(|_| {
                        DriverError::Unsupported("the unit's group is not numeric".into())
                    })?,
                )
            };
            let helper = format!("{}-layer-{}", unit.name, orch_types::new_id().simple());
            let image = self.config.live_helper_image.as_deref().ok_or_else(|| {
                DriverError::Unsupported("the trusted helper image is unavailable".into())
            })?;
            crate::volumes::set_roots(
                &self.daemon,
                &self.config,
                &helper,
                image,
                &crate::overlay::layer_roots(&prepared),
                ids,
                0,
            )
            .await?;
            mounts.extend(
                crate::overlay::create_views(&self.daemon, &self.config, &prepared)
                    .await?
                    .0,
            );
        }
        let mut by_name = HashMap::new();
        for spec in specs {
            let mount = mounts
                .iter()
                .find(|mount| mount.target.as_deref() == Some(spec.target.as_str()))
                .cloned()
                .ok_or_else(|| {
                    DriverError::Unavailable("the live source could not be prepared".into())
                })?;
            by_name.insert(spec.name.clone(), mount);
        }
        Ok((by_name, planned.attached))
    }

    async fn live_transition(
        &self,
        unit: &UnitRef,
        actual: &mut Vec<MountSpec>,
        wanted: &[MountSpec],
        sources: &HashMap<String, Mount>,
    ) -> Result<(), DriverError> {
        let removed: Vec<_> = actual
            .iter()
            .filter(|old| !wanted.contains(old))
            .cloned()
            .collect();
        for old in removed {
            self.namespace_helper(unit, &old.target, NamespaceChange::Detach)
                .await?;
            actual.retain(|spec| spec != &old);
        }
        for spec in wanted
            .iter()
            .filter(|spec| !actual.contains(spec))
            .cloned()
            .collect::<Vec<_>>()
        {
            let mut source = sources.get(&spec.name).cloned().ok_or_else(|| {
                DriverError::NotFound("the prepared live source is absent".into())
            })?;
            let read_only = source.read_only.unwrap_or(spec.mode == MountMode::ReadOnly);
            source.target = Some(SOURCE.into());
            self.namespace_helper(
                unit,
                &spec.target,
                NamespaceChange::Attach { source, read_only },
            )
            .await?;
            actual.push(spec);
        }
        Ok(())
    }

    /// Whether this node has the trusted static helper and the required Linux mount API.
    #[must_use]
    pub fn live_helpers_supported(&self) -> bool {
        self.live_helpers_kernel
            && self
                .config
                .live_helper_image
                .as_ref()
                .is_some_and(|image| !image.is_empty())
            && self
                .config
                .live_helper_binary
                .as_deref()
                .is_some_and(static_binary)
    }

    /// Attaches a host path without replacing the unit or stopping its processes.
    pub async fn attach_live_mount(
        &self,
        unit: &UnitRef,
        source: &str,
        target: &str,
        read_only: bool,
    ) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        self.namespace_helper(
            unit,
            target,
            NamespaceChange::Attach {
                source: crate::mounts::bind(source, SOURCE, read_only),
                read_only,
            },
        )
        .await
    }

    /// Detaches the target in this unit alone; capability withdrawal may then restart the same unit.
    pub async fn detach_live_mount(&self, unit: &UnitRef, target: &str) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        self.namespace_helper(unit, target, NamespaceChange::Detach)
            .await
    }

    /// Gives the host's character device, preserving its mode/group; its ordinary unit cgroup permission is separate.
    pub async fn give_live_device(
        &self,
        unit: &UnitRef,
        source: &str,
        target: &str,
        major: u64,
        minor: u64,
    ) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        self.namespace_helper(
            unit,
            target,
            NamespaceChange::GiveDevice {
                source: crate::mounts::bind(source, SOURCE, true),
                major,
                minor,
            },
        )
        .await
    }

    /// Removes a device path; the caller restarts the same container to revoke any descriptors a process kept open.
    pub async fn remove_live_device(
        &self,
        unit: &UnitRef,
        target: &str,
    ) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        self.namespace_helper(unit, target, NamespaceChange::RemoveDevice)
            .await
    }

    pub(crate) async fn namespace_helper(
        &self,
        unit: &UnitRef,
        target: &str,
        change: NamespaceChange,
    ) -> Result<(), DriverError> {
        let (operation, source, read_only, device, remove) = match change {
            NamespaceChange::Attach { source, read_only } => {
                ("attach", Some(source), read_only, None, false)
            }
            NamespaceChange::Detach => ("detach", None, false, None, false),
            NamespaceChange::GiveDevice {
                source,
                major,
                minor,
            } => ("device", Some(source), false, Some((major, minor)), false),
            NamespaceChange::RemoveDevice => ("device", None, false, None, true),
        };
        check_target(target).map_err(|_| {
            DriverError::InvalidSpec("the live target is not an absolute normalized path".into())
        })?;
        if !self.live_helpers_supported() {
            return Err(DriverError::Unsupported("live mount/device changes need Linux >=5.12 and a configured trusted image plus static node binary; recreate is needed here".into()));
        }
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        if !container
            .state
            .as_ref()
            .is_some_and(|state| state.running == Some(true) && state.paused != Some(true))
        {
            return Err(DriverError::Unavailable(
                "the unit is not running; the change waits for its next start".into(),
            ));
        }
        let id = container.id.ok_or_else(|| {
            DriverError::NotFound("the owned unit has no daemon reference".into())
        })?;
        let binary = self.config.live_helper_binary.as_deref().ok_or_else(|| {
            DriverError::Unsupported("the static node helper is not configured".into())
        })?;
        let image = self.config.live_helper_image.clone().ok_or_else(|| {
            DriverError::Unsupported("the trusted helper image is not configured".into())
        })?;
        let helper = format!("{}-live-{}", unit.name, orch_types::new_id().simple());
        let mut mounts = vec![crate::mounts::bind(
            &self.config.host_path(binary).to_string_lossy(),
            HELPER_BINARY,
            true,
        )];
        if let Some(source) = source {
            mounts.push(source);
        }
        let mut command = vec![
            "__runtime-helper".to_owned(),
            operation.to_owned(),
            "--target".to_owned(),
            target.to_owned(),
        ];
        for variable in container
            .config
            .as_ref()
            .and_then(|config| config.env.as_ref())
            .into_iter()
            .flatten()
        {
            if let Some((key, value)) = variable.split_once('=')
                && matches!(key, "ORCH_WORKSPACE_DIR" | "ORCH_HOME_DIR")
            {
                command.extend(["--reserved".into(), value.into()]);
            }
        }
        if read_only {
            command.push("--read-only".into());
        }
        if remove {
            command.push("--remove".into());
        }
        if let Some((major, minor)) = device {
            command.extend([
                "--major".into(),
                major.to_string(),
                "--minor".into(),
                minor.to_string(),
            ]);
        }
        let mut labels: HashMap<String, String> = driver_labels(&self.config).into_iter().collect();
        labels.insert(LABEL_PREPARE.to_owned(), helper.clone());
        labels.insert("orch.runtime_id".into(), unit.runtime_id.to_string());
        let body = ContainerCreateBody {
            image: Some(image),
            entrypoint: Some(vec![HELPER_BINARY.into()]),
            cmd: Some(command),
            user: Some("0:0".into()),
            labels: Some(labels),
            host_config: Some(HostConfig {
                pid_mode: Some(format!("container:{id}")),
                network_mode: Some("none".into()),
                cap_add: Some(vec![
                    "SYS_ADMIN".into(),
                    "SYS_CHROOT".into(),
                    "MKNOD".into(),
                ]),
                security_opt: Some(vec!["apparmor=unconfined".into()]),
                readonly_rootfs: Some(true),
                mounts: Some(mounts),
                device_cgroup_rules: device
                    .map(|(major, minor)| vec![format!("c {major}:{minor} m")]),
                ..HostConfig::default()
            }),
            ..ContainerCreateBody::default()
        };
        self.daemon
            .docker()
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&helper)
                        .build(),
                ),
                body,
            )
            .await
            .map_err(|_| {
                DriverError::Unavailable(
                    "Docker could not prepare the trusted live helper; recreate is needed".into(),
                )
            })?;
        let mut exit_code = None;
        let outcome = async {
            self.daemon
                .docker()
                .start_container(&helper, None::<StartContainerOptions>)
                .await
                .map_err(|_| {
                    DriverError::Unavailable(
                        "Docker could not start the live helper; recreate is needed".into(),
                    )
                })?;
            let mut waiting = self
                .daemon
                .docker()
                .wait_container(&helper, None::<WaitContainerOptions>);
            match tokio::time::timeout(HELPER_WAIT, waiting.next()).await {
                Ok(Some(Ok(ended))) if ended.status_code == 0 => Ok(()),
                Ok(Some(Ok(ended))) => {
                    exit_code = Some(ended.status_code);
                    Err(helper_failure(operation, ended.status_code))
                }
                Ok(Some(Err(bollard::errors::Error::DockerContainerWaitError {
                    code, ..
                }))) if code != 0 => {
                    exit_code = Some(code);
                    Err(helper_failure(operation, code))
                }
                Err(_) => Err(DriverError::Timeout),
                _ => Err(DriverError::Unavailable(
                    "the live helper's completion could not be observed".into(),
                )),
            }
        }
        .await;
        if outcome.is_err() {
            let diagnostic = helper_diagnostic(&self.daemon, &helper).await;
            tracing::warn!(runtime = %unit.runtime_id, %helper, %operation, ?exit_code, stderr = %diagnostic, "the live helper failed");
        }
        let cleanup = remove_helper(&self.daemon, &helper).await;
        if cleanup.is_err() {
            tracing::warn!(runtime = %unit.runtime_id, "the completed live helper could not be removed");
        }
        if matches!(
            &outcome,
            Err(DriverError::Timeout | DriverError::Unavailable(_))
        ) {
            // An unobserved operation may have committed. Its ephemeral namespace must not be
            // represented as either the accepted old set or the requested new set.
            self.daemon.docker().stop_container(&unit.reference, None::<bollard::query_parameters::StopContainerOptions>).await
                .map_err(|_| DriverError::Unavailable("the live helper's outcome is unknown and its unit could not be stopped safely".into()))?;
            return Err(DriverError::Unavailable(
                "the live helper's outcome is unknown; the unit was stopped for recreate".into(),
            ));
        }
        // Cleanup cannot turn an acknowledged namespace operation into a failed operation.
        outcome
    }
}

pub(crate) fn mounts_archive(attached: &[AttachedMount]) -> Result<Vec<u8>, DriverError> {
    // The owned run volume receives names/targets/modes alone, never host locations.
    let text = orch_driver_contract::runtime::mounts_file(attached);
    crate::archive::tar_of(
        &[crate::archive::Entry::File {
            path: "mounts.json".into(),
            mode: 0o644,
            uid: 0,
            gid: 0,
            content: text.into_bytes(),
        }],
        0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_mounts_require_the_recursive_readonly_mount_api() {
        assert!(!kernel_at_least_512("5.11.0"));
        assert!(kernel_at_least_512("5.12.0"));
        assert!(kernel_at_least_512("6.12.0-custom"));
        assert!(!kernel_at_least_512("unknown"));
    }

    #[test]
    fn a_failed_helper_names_the_operation_and_exit_status_and_a_next_action() {
        let failure = helper_failure("device", 1).to_string();
        assert!(failure.contains("device helper exited with status 1"));
        assert!(failure.contains("host's manager"));
        assert!(failure.contains("recreate"));
    }
}
