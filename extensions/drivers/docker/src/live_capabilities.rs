//! Prepared device permissions, live capability grants and replay before the managed connector enrolls.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use bollard::models::{ContainerCreateBody, ContainerInspectResponse, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, LogsOptionsBuilder, StartContainerOptions, WaitContainerOptions,
};
use futures::StreamExt;
use orch_driver_contract::capabilities::CapabilityState;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::RuntimeSpec;
use orch_driver_contract::units::UnitRef;
use serde::{Deserialize, Serialize};

use crate::capabilities::CapabilityPatch;
use crate::client::map_error;
use crate::driver::DockerDriver;
use crate::units::{LABEL_PREPARE, driver_labels};

pub(crate) const LIVE: [&str; 2] = ["kvm", "docker_socket"];
const LABEL_STATE: &str = "orch.capabilities";
const LABEL_PREPARED: &str = "orch.capabilities_ready";
const PROGRAM: &str = "/opt/orch/runtime-helper";
const GATE: &str = "/run/orch-capability-gate";
// The metadata operation is bounded exactly like the namespace helper it belongs to.
const SOURCE_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceFacts {
    kind: String,
    major: u64,
    minor: u64,
    gid: u32,
    mode: u32,
}

/// The recorded grant boundary and the host sources prepared with it.
type RecordedCapabilities = (CapabilityState, BTreeMap<String, SourceFacts>);

fn live_patch(state: &CapabilityState, key: &str) -> Result<CapabilityPatch, DriverError> {
    let value = state
        .patches
        .get(key)
        .ok_or_else(|| DriverError::Unsupported(format!("{key} has no prepared patch")))?;
    let object = value.as_object().ok_or_else(|| {
        DriverError::InvalidSpec("a live capability patch must be an object".into())
    })?;
    if object
        .keys()
        .any(|member| !matches!(member.as_str(), "devices" | "group_add" | "binds"))
    {
        return Err(DriverError::Unsupported(format!(
            "{key} changes more than devices, groups and binds; recreate is needed"
        )));
    }
    let patch = CapabilityPatch::from_spec(value).map_err(DriverError::InvalidSpec)?;
    match key {
        "kvm"
            if patch.devices.len() == 1
                && patch.devices[0].path == "/dev/kvm"
                && patch.binds.is_empty() => {}
        "docker_socket" if patch.devices.is_empty() && patch.binds.len() == 1 => {
            let _ = socket_bind(&patch.binds[0])?;
        }
        _ => {
            return Err(DriverError::Unsupported(format!(
                "{key} has a patch that requires recreation"
            )));
        }
    }
    Ok(patch)
}

fn socket_bind(bind: &str) -> Result<(&str, &str, bool), DriverError> {
    let parts: Vec<_> = bind.split(':').collect();
    if !(2..=3).contains(&parts.len())
        || !parts[0].starts_with('/')
        || !matches!(parts[1], "/var/run/docker.sock" | "/run/docker.sock")
        || (parts.len() == 3 && !matches!(parts[2], "ro" | "rw"))
    {
        return Err(DriverError::Unsupported(
            "the Docker socket live patch names one socket and its ordinary target, with ro or rw"
                .into(),
        ));
    }
    Ok((parts[0], parts[1], parts.get(2) == Some(&"ro")))
}

impl DockerDriver {
    /// Reads the actual daemon-host node even when the node itself runs in a container.
    async fn source_facts(&self, source: &str) -> Result<SourceFacts, DriverError> {
        serde_json::from_value(self.helper_metadata(Some(source), None, None).await?).map_err(
            |_| DriverError::Unavailable("the host capability metadata was not valid".into()),
        )
    }

    async fn helper_metadata(
        &self,
        source: Option<&str>,
        unit: Option<&UnitRef>,
        check: Option<(
            &orch_driver_contract::runtime::DirectoryBinding,
            &ContainerCreateBody,
            (u32, u32),
        )>,
    ) -> Result<orch_types::Json, DriverError> {
        if !self.live_helpers_supported() {
            return Err(DriverError::Unsupported(
                "the trusted live helper is unavailable".into(),
            ));
        }
        let binary = self.config.live_helper_binary.as_ref().ok_or_else(|| {
            DriverError::Unsupported("the trusted helper binary is unavailable".into())
        })?;
        let helper = format!("orch-source-{}", orch_types::new_id().simple());
        let mut labels: HashMap<_, _> = driver_labels(&self.config).into_iter().collect();
        labels.insert(LABEL_PREPARE.into(), helper.clone());
        let mut mounts = vec![crate::mounts::bind(
            &self.config.host_path(binary).to_string_lossy(),
            PROGRAM,
            true,
        )];
        if let Some(source) = source {
            mounts.push(crate::mounts::bind(source, "/src", check.is_none()));
        }
        let mut body = ContainerCreateBody {
            image: self.config.live_helper_image.clone(),
            user: Some("0:0".into()),
            entrypoint: Some(vec![PROGRAM.into()]),
            cmd: Some(vec![
                "__runtime-helper".into(),
                if source.is_some() {
                    "source"
                } else {
                    "identity"
                }
                .into(),
                "--target".into(),
                "/src".into(),
            ]),
            labels: Some(labels),
            host_config: Some(HostConfig {
                network_mode: Some("none".into()),
                readonly_rootfs: Some(true),
                cap_drop: Some(vec!["ALL".into()]),
                pid_mode: unit.map(|unit| format!("container:{}", unit.reference)),
                mounts: Some(mounts),
                ..HostConfig::default()
            }),
            ..ContainerCreateBody::default()
        };
        if let Some((directory, unit_body, (uid, gid))) = check {
            let mut expected = directory.clone();
            expected.path = "/src".into();
            body.image = unit_body.image.clone();
            body.user = Some(format!("{uid}:{gid}"));
            body.cmd = Some(vec![
                "__runtime-helper".into(),
                "directory-check".into(),
                "--target".into(),
                "/src".into(),
                "--reserved".into(),
                serde_json::to_string(&expected)
                    .map_err(|error| DriverError::Internal(error.to_string()))?,
            ]);
            if let (Some(host), Some(unit_host)) = (&mut body.host_config, &unit_body.host_config) {
                host.group_add = unit_host.group_add.clone();
                host.cap_add = unit_host.cap_add.clone();
                host.cap_drop = unit_host.cap_drop.clone();
            }
        }
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
            .map_err(map_error)?;
        let outcome = tokio::time::timeout(SOURCE_WAIT, async {
            self.daemon
                .docker()
                .start_container(&helper, None::<StartContainerOptions>)
                .await
                .map_err(map_error)?;
            let mut waiting = self
                .daemon
                .docker()
                .wait_container(&helper, None::<WaitContainerOptions>);
            match waiting.next().await {
                Some(Ok(ended)) if ended.status_code == 0 => {}
                _ => {
                    return Err(DriverError::Unavailable(
                        "the host capability source could not be inspected".into(),
                    ));
                }
            }
            let mut logs = self.daemon.docker().logs(
                &helper,
                Some(LogsOptionsBuilder::default().stdout(true).build()),
            );
            let mut text = String::new();
            while let Some(line) = logs.next().await {
                text.push_str(&line.map_err(map_error)?.to_string());
                // The trusted binary writes six scalar fields; 4 KiB also bounds a mismatched operator binary.
                if text.len() > 4096 {
                    return Err(DriverError::Unavailable(
                        "the host capability metadata was too large".into(),
                    ));
                }
            }
            serde_json::from_str(&text).map_err(|_| {
                DriverError::Unavailable("the host capability metadata was not valid".into())
            })
        })
        .await
        .unwrap_or(Err(DriverError::Timeout));
        if let Err(error) = crate::volumes::remove_helper(&self.daemon, &helper).await {
            tracing::warn!(%helper,%error,"the source helper could not be removed");
        }
        outcome
    }

    /// Execute only the node's static binary as the unit's real identity. A final check in Launch
    /// also closes the window between this probe and Docker establishing the actual bind.
    pub(crate) async fn check_data_directories(
        &self,
        spec: &RuntimeSpec,
        body: &ContainerCreateBody,
        ids: (u32, u32),
    ) -> Result<(), DriverError> {
        for directory in spec
            .volumes
            .values()
            .filter_map(|volume| volume.directory.as_ref())
        {
            self.helper_metadata(Some(&directory.path),None,Some((directory,body,ids))).await.map_err(|error|DriverError::Unavailable(format!("this machine's owners must make {} a writable directory for user id {} (group {}), or restore it if it changed; Orch never changes its owner or mode: {error}",directory.path,ids.0,ids.1)))?;
        }
        Ok(())
    }

    fn gate_dir(&self, runtime: orch_types::Id) -> Result<PathBuf, DriverError> {
        let cache = self
            .config
            .tool_cache
            .dir
            .as_ref()
            .or(self.config.tool_cache.connector_dir.as_ref())
            .ok_or_else(|| {
                DriverError::Unavailable(
                    "the node has no private tool-cache directory for the capability gate".into(),
                )
            })?;
        Ok(cache.join(".runtime-gates").join(runtime.to_string()))
    }

    fn gate(&self, runtime: orch_types::Id, ready: Option<&str>) -> Result<(), DriverError> {
        let dir = self.gate_dir(runtime)?;
        std::fs::create_dir_all(&dir).map_err(|error| {
            DriverError::Unavailable(format!(
                "the capability launch gate could not be prepared: {error}"
            ))
        })?;
        let temporary = dir.join(format!(".ready-{}", orch_types::new_id().simple()));
        std::fs::write(&temporary, ready.unwrap_or_default().as_bytes()).map_err(|error| {
            DriverError::Unavailable(format!(
                "the capability launch gate could not be written: {error}"
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644))
                .map_err(|error| DriverError::Unavailable(error.to_string()))?;
        }
        std::fs::rename(&temporary, dir.join("ready")).map_err(|error| {
            DriverError::Unavailable(format!(
                "the capability launch gate could not be published: {error}"
            ))
        })
    }

    /// Prepares immutable device/group permissions without giving an optional node or bind.
    pub(crate) async fn prepare_live_capabilities(
        &self,
        spec: &RuntimeSpec,
        body: &mut ContainerCreateBody,
    ) -> Result<(), DriverError> {
        spec.capabilities.check()?;
        let state = &spec.capabilities;
        // A unit with a kernel of its own is given what it holds when it is made: nothing of the host's devices is
        // passed live, so there is nothing to prepare for it, and no right is taken from it for a grant that cannot come.
        let runtime = body
            .host_config
            .as_ref()
            .and_then(|host| host.runtime.as_deref());
        if self.boundary_of(runtime).has_kernel_of_its_own() {
            if spec
                .volumes
                .values()
                .any(|volume| volume.directory.is_some())
            {
                return Err(DriverError::Unsupported("existing directories are not offered under a runtime with a kernel of its own; choose storage made by Orch".into()));
            }
            return Ok(());
        }
        let directories: Vec<_> = spec
            .volumes
            .iter()
            .filter_map(|(role, volume)| {
                volume
                    .directory
                    .as_ref()
                    .map(|directory| (*role, directory))
            })
            .collect();
        if !directories.is_empty() && !self.live_helpers_supported() {
            return Err(DriverError::Unsupported("existing directories require this host's verified static launch helper; choose storage made by Orch".into()));
        }
        if !self.live_helpers_supported()
            || (directories.is_empty()
                && !state
                    .allowed
                    .iter()
                    .chain(&state.granted)
                    .any(|key| LIVE.contains(&key.as_str()) && !state.base.contains(key)))
        {
            return Ok(());
        }
        let host = body
            .host_config
            .as_mut()
            .ok_or_else(|| DriverError::Internal("the prepared unit has no host options".into()))?;
        // An explicitly approved broader capability keeps its own semantics. In particular
        // privileged already admits host devices independently of a later named KVM grant;
        // this preparation never makes an ordinary unit privileged to enable that grant.
        let mut prepared = BTreeMap::new();
        for key in LIVE {
            if !state
                .allowed
                .iter()
                .chain(&state.granted)
                .any(|held| held == key)
                || state.base.iter().any(|base| base == key)
            {
                continue;
            }
            let patch = live_patch(state, key)?;
            if key == "kvm" {
                if host.privileged != Some(true)
                    && let Some(added) = host.cap_add.as_mut()
                {
                    added.retain(|cap| !cap.eq_ignore_ascii_case("MKNOD"));
                }
                let drops = host.cap_drop.get_or_insert_with(Vec::new);
                if !drops.iter().any(|cap| cap.eq_ignore_ascii_case("MKNOD")) {
                    drops.push("MKNOD".into());
                }
            }
            if key == "docker_socket" {
                // Docker creates the empty file mountpoint before making a read-only rootfs.
                // The live socket later covers it; before/after a grant it is only an inert
                // read-only regular file, and no write to the unit's image is needed.
                let dir = self.gate_dir(spec.runtime_id)?;
                std::fs::create_dir_all(&dir)
                    .map_err(|error| DriverError::Unavailable(error.to_string()))?;
                let placeholder = dir.join("socket-placeholder");
                std::fs::write(&placeholder, [])
                    .map_err(|error| DriverError::Unavailable(error.to_string()))?;
                host.mounts
                    .get_or_insert_with(Vec::new)
                    .push(crate::mounts::bind(
                        &self.config.host_path(&placeholder).to_string_lossy(),
                        socket_bind(&patch.binds[0])?.1,
                        true,
                    ));
            }
            let source = if key == "kvm" {
                patch.devices[0].path.as_str()
            } else {
                socket_bind(&patch.binds[0])?.0
            };
            let facts = match self.source_facts(source).await {
                Ok(facts) => facts,
                Err(error) if !state.granted.iter().any(|grant| grant == key) => {
                    tracing::info!(runtime=%spec.runtime_id,capability=key,%error,"the optional capability has no ready host source");
                    continue;
                }
                Err(error) => return Err(error),
            };
            if facts.kind != if key == "kvm" { "device" } else { "socket" } {
                return Err(DriverError::Unsupported(format!(
                    "{key}'s host source has the wrong kind"
                )));
            }
            let groups = host.group_add.get_or_insert_with(Vec::new);
            for group in patch
                .group_add
                .iter()
                .cloned()
                .chain(std::iter::once(facts.gid.to_string()))
            {
                if !groups.contains(&group) {
                    groups.push(group);
                }
            }
            if key == "kvm" {
                if host.devices.as_ref().is_some_and(|devices| {
                    devices
                        .iter()
                        .any(|device| device.path_in_container.as_deref() == Some("/dev/kvm"))
                }) {
                    return Err(DriverError::InvalidSpec("optional kvm is already given by the template's devices; make it a default instead".into()));
                }
                let permissions = patch.devices[0].permissions.as_deref().unwrap_or("rwm");
                host.device_cgroup_rules
                    .get_or_insert_with(Vec::new)
                    .push(format!("c {}:{} {permissions}", facts.major, facts.minor));
                let drops = host.cap_drop.get_or_insert_with(Vec::new);
                if !drops.iter().any(|cap| cap.eq_ignore_ascii_case("MKNOD")) {
                    drops.push("MKNOD".into());
                }
            }
            prepared.insert(key.to_owned(), facts);
        }
        self.gate(spec.runtime_id, None)?;
        let binary =
            self.config.live_helper_binary.as_ref().ok_or_else(|| {
                DriverError::Unsupported("the static helper is unavailable".into())
            })?;
        host.mounts.get_or_insert_with(Vec::new).extend([
            crate::mounts::bind(
                &self.config.host_path(binary).to_string_lossy(),
                PROGRAM,
                true,
            ),
            crate::mounts::bind(
                &self
                    .config
                    .host_path(&self.gate_dir(spec.runtime_id)?)
                    .to_string_lossy(),
                GATE,
                true,
            ),
        ]);
        body.entrypoint = Some(vec![PROGRAM.into()]);
        body.cmd = Some(vec![
            "__runtime-helper".into(),
            "launch".into(),
            "--target".into(),
            format!("{GATE}/ready"),
        ]);
        for (role, directory) in directories {
            if directory.device.is_none() || directory.inode.is_none() {
                return Err(DriverError::InvalidSpec("an existing directory must first be checked by the machine against environment.roots".into()));
            }
            let target = match role {
                orch_driver_contract::runtime::VolumeRole::Workspace => body
                    .env
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .find_map(|value| value.strip_prefix("ORCH_WORKSPACE_DIR=")),
                orch_driver_contract::runtime::VolumeRole::Home => {
                    body.env.as_ref().into_iter().flatten().find_map(|value| {
                        value
                            .strip_prefix("ORCH_HOME_DIR=")
                            .or_else(|| value.strip_prefix("ORCH_HARNESS_HOME="))
                    })
                }
                orch_driver_contract::runtime::VolumeRole::State => None,
            }
            .ok_or_else(|| {
                DriverError::InvalidSpec("an existing data root has no launch target".into())
            })?;
            let mut expected = directory.clone();
            expected.path = target.into();
            body.cmd
                .as_mut()
                .ok_or_else(|| DriverError::Internal("the launch helper has no command".into()))?
                .extend([
                    "--reserved".into(),
                    serde_json::to_string(&expected)
                        .map_err(|error| DriverError::Internal(error.to_string()))?,
                ]);
        }
        let labels = body.labels.get_or_insert_with(HashMap::new);
        labels.insert(
            LABEL_STATE.into(),
            serde_json::to_string(state)
                .map_err(|error| DriverError::Internal(error.to_string()))?,
        );
        labels.insert(
            LABEL_PREPARED.into(),
            serde_json::to_string(&prepared)
                .map_err(|error| DriverError::Internal(error.to_string()))?,
        );
        Ok(())
    }

    fn recorded_capabilities(
        container: &ContainerInspectResponse,
    ) -> Result<Option<RecordedCapabilities>, DriverError> {
        let labels = container
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref());
        let Some(text) = labels.and_then(|labels| labels.get(LABEL_STATE)) else {
            return Ok(None);
        };
        let state = serde_json::from_str(text).map_err(|_| {
            DriverError::InvalidSpec("the unit's prepared capability boundary is invalid".into())
        })?;
        let prepared = labels
            .and_then(|labels| labels.get(LABEL_PREPARED))
            .ok_or_else(|| {
                DriverError::InvalidSpec("the unit's prepared device metadata is missing".into())
            })?;
        let prepared = serde_json::from_str(prepared).map_err(|_| {
            DriverError::InvalidSpec("the unit's prepared device metadata is invalid".into())
        })?;
        Ok(Some((state, prepared)))
    }

    async fn change_one_capability(
        &self,
        unit: &UnitRef,
        state: &CapabilityState,
        prepared: &BTreeMap<String, SourceFacts>,
        key: &str,
        give: bool,
    ) -> Result<(), DriverError> {
        let patch = live_patch(state, key)?;
        if !give {
            return if key == "kvm" {
                self.remove_live_device(unit, "/dev/kvm").await
            } else {
                self.detach_live_mount(unit, socket_bind(&patch.binds[0])?.1)
                    .await
            };
        }
        let ready = prepared.get(key).ok_or_else(|| {
            DriverError::Unsupported(format!(
                "{key} was not ready at creation; recreate is needed"
            ))
        })?;
        let source = if key == "kvm" {
            patch.devices[0].path.as_str()
        } else {
            socket_bind(&patch.binds[0])?.0
        };
        let now = self.source_facts(source).await?;
        if &now != ready {
            return Err(DriverError::Unsupported(format!(
                "{key}'s device or group changed on the host; recreate is needed"
            )));
        }
        if key == "kvm" {
            self.give_live_device(unit, source, "/dev/kvm", now.major, now.minor)
                .await
        } else {
            let (source, target, read_only) = socket_bind(&patch.binds[0])?;
            self.attach_live_mount(unit, source, target, read_only)
                .await
        }
    }

    pub(crate) async fn apply_live_capabilities(
        &self,
        unit: &UnitRef,
        before: &CapabilityState,
        after: &CapabilityState,
    ) -> Result<bool, DriverError> {
        before.check()?;
        after.check()?;
        if !before.same_boundary(after) {
            return Err(DriverError::Unsupported(
                "a changed template capability boundary needs recreation".into(),
            ));
        }
        let changed: Vec<_> = before
            .granted
            .iter()
            .chain(&after.granted)
            .filter(|key| {
                before.granted.contains(key) != after.granted.contains(key)
                    && !after.base.contains(key)
                    && !orch_driver_contract::capabilities::gateway_given(key)
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if changed.is_empty() {
            return Ok(false);
        }
        if changed.iter().any(|key| !LIVE.contains(&key.as_str())) {
            return Err(DriverError::Unsupported(
                "this capability needs recreation".into(),
            ));
        }
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        if self.is_guest(&container) {
            return Err(DriverError::Unsupported(
                "this unit's runtime has a kernel of its own, which the host cannot reach to give it devices; recreate is needed".into(),
            ));
        }
        let (boundary, prepared) = Self::recorded_capabilities(&container)?.ok_or_else(|| {
            DriverError::Unsupported(
                "the older unit has no prepared live boundary; recreate is needed".into(),
            )
        })?;
        if !boundary.same_boundary(after) {
            return Err(DriverError::InvalidSpec(
                "the requested capability boundary differs from the owned unit's preparation"
                    .into(),
            ));
        }
        let mut applied = Vec::new();
        for key in &changed {
            if let Err(error) = self
                .change_one_capability(unit, after, &prepared, key, after.granted.contains(key))
                .await
            {
                // The node stops and replays the old desired state on any failed transition, evicting
                // handles opened during a partially applied grant as well as restoring paths.
                tracing::warn!(runtime=%unit.runtime_id,?applied,%error,"the capability transition needs restoration");
                return Err(error);
            }
            applied.push(key.as_str());
        }
        Ok(changed
            .iter()
            .any(|key| before.granted.contains(key) && !after.granted.contains(key)))
    }

    pub(crate) async fn start_live_capabilities(
        &self,
        unit: &UnitRef,
        state: &CapabilityState,
    ) -> Result<(), DriverError> {
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        let Some((boundary, prepared)) = Self::recorded_capabilities(&container)? else {
            return self.start_unit(unit).await;
        };
        if !boundary.same_boundary(state) {
            return Err(DriverError::InvalidSpec(
                "the replayed capability boundary differs from this unit's preparation".into(),
            ));
        }
        let running = container
            .state
            .as_ref()
            .is_some_and(|actual| actual.running == Some(true));
        if running {
            let metadata = self.helper_metadata(None, Some(unit), None).await?;
            if let Some(started) = metadata.get("started").and_then(orch_types::Json::as_str)
                && std::fs::read_to_string(self.gate_dir(unit.runtime_id)?.join("ready"))
                    .is_ok_and(|ready| ready == started)
            {
                return Ok(());
            }
        }
        self.gate(unit.runtime_id, None)?;
        if !running {
            self.start_unit(unit).await?;
        }
        let result = async {
            for key in LIVE {
                if state.granted.iter().any(|grant| grant == key)
                    && !state.base.iter().any(|base| base == key)
                {
                    self.change_one_capability(unit, state, &prepared, key, true)
                        .await?;
                }
            }
            let metadata = self.helper_metadata(None, Some(unit), None).await?;
            let started = metadata
                .get("started")
                .and_then(orch_types::Json::as_str)
                .ok_or_else(|| {
                    DriverError::Unavailable(
                        "the unit's capability launch identity is unavailable".into(),
                    )
                })?;
            self.gate(unit.runtime_id, Some(started))
        }
        .await;
        if result.is_err() {
            let _ = self.stop_unit(unit, Duration::ZERO).await;
        }
        result
    }

    pub(crate) async fn start_recorded_capabilities(
        &self,
        unit: &UnitRef,
    ) -> Result<(), DriverError> {
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        if let Some((state, _)) = Self::recorded_capabilities(&container)? {
            self.start_live_capabilities(unit, &state).await
        } else {
            self.start_unit(unit).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_patches_cannot_smuggle_kernel_or_network_options_into_a_device_grant() {
        let mut state = CapabilityState {
            allowed: vec!["kvm".into()],
            patches: BTreeMap::from([("kvm".into(), json!({"devices":[{"path":"/dev/kvm"}]}))]),
            ..CapabilityState::default()
        };
        assert!(live_patch(&state, "kvm").is_ok());
        state.patches.get_mut("kvm").unwrap()["privileged"] = json!(true);
        assert!(matches!(
            live_patch(&state, "kvm"),
            Err(DriverError::Unsupported(_))
        ));
        assert!(
            socket_bind("/var/run/docker.sock:/var/run/docker.sock:ro")
                .unwrap()
                .2
        );
        assert!(socket_bind("/socket:/etc/shadow").is_err());
    }
}
