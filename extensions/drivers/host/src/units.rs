//! Systemd scopes or process groups and lifecycle.
//!
//! The connector and its process tree are owned by an exact saved identity; its directories outlive every run.

use crate::identity::Identity;
use crate::stats::ProcessIdentity;
use async_trait::async_trait;
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::{LABEL_SYSTEM, RuntimeKind, RuntimeSpec, VolumeRole};
use orch_driver_contract::units::{ResourceSample, UnitRef, UnitState, UnitStateKind};
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest};
use orch_types::{Clock, Json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One operator-selected directory for persistent host volumes.
#[derive(Debug, Clone)]
pub struct HostStorage {
    /// Storage name visible in plans.
    pub name: String,
    /// The owned volumes' base directory.
    pub path: PathBuf,
    /// Whether unnamed storage lands here.
    pub default: bool,
}

/// Local host-process configuration, chosen only by the node operator.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The protected directory of host-driver records and default volumes.
    pub base: PathBuf,
    /// The connector from the node's verified current tool build.
    pub connector: PathBuf,
    /// The verified tools cache, read by name and version.
    pub tools: PathBuf,
    /// The host's configured named storages.
    pub storages: Vec<HostStorage>,
    /// Whether to probe and use a native systemd scope; false reports limits instead.
    pub prefer_scopes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Paths {
    state: PathBuf,
    workspace: Option<PathBuf>,
    home: Option<PathBuf>,
    run: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Unit {
    reference: UnitRef,
    spec: RuntimeSpec,
    identity: Identity,
    paths: Paths,
    process: Option<ProcessIdentity>,
    state: UnitState,
}

/// The built-in process driver, with one lock for each owned unit and no container or image assumption.
pub struct HostDriver {
    config: HostConfig,
    clock: Arc<dyn Clock>,
    controls: orch_driver_contract::process_limits::ProcessControls,
    locks: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

pub(crate) fn io_error(error: std::io::Error) -> DriverError {
    match error.kind() {
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::OutOfMemory => {
            DriverError::NoCapacity(error.to_string())
        }
        std::io::ErrorKind::NotFound => DriverError::NotFound(error.to_string()),
        _ => DriverError::Unavailable(error.to_string()),
    }
}

fn verify_directory(
    directory: &orch_driver_contract::runtime::DirectoryBinding,
    identity: &Identity,
) -> Result<(), DriverError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&directory.path).map_err(io_error)?;
        if !metadata.is_dir()
            || directory.device != Some(metadata.dev())
            || directory.inode != Some(metadata.ino())
        {
            return Err(DriverError::InvalidSpec("the existing directory changed after environment.roots was checked; recreate with an allowed directory".into()));
        }
        for access in ["-w", "-x"] {
            let mut command = if nix::unistd::Uid::effective().as_raw() != identity.uid {
                let mut command = std::process::Command::new("runuser");
                command.args(["--user", identity.name.as_str(), "--", "/usr/bin/test"]);
                command
            } else {
                std::process::Command::new("/usr/bin/test")
            };
            if !command
                .args([access, &directory.path])
                .status()
                .map_err(io_error)?
                .success()
            {
                return Err(DriverError::Unavailable(format!(
                    "this machine's owners must make {} writable and searchable for user id {} (group {}); Orch never changes its owner or mode",
                    directory.path, identity.uid, identity.gid
                )));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, identity);
        Err(DriverError::Unsupported(
            "this driver cannot verify existing directories on this platform".into(),
        ))
    }
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), DriverError> {
    use std::io::Write;
    let parent = path.parent().ok_or_else(|| {
        DriverError::InvalidSpec("the host record needs a parent directory".into())
    })?;
    std::fs::create_dir_all(parent).map_err(io_error)?;
    let bytes = serde_json::to_vec(value)
        .map_err(|_| DriverError::Internal("the host record could not be encoded".into()))?;
    let scratch = parent.join(format!(".write-{}", orch_types::new_id().simple()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&scratch).map_err(io_error)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(io_error)?;
    if let Err(error) = std::fs::rename(&scratch, path) {
        let _ = std::fs::remove_file(&scratch);
        return Err(io_error(error));
    }
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(io_error)
}

impl HostDriver {
    /// Opens owned records and probes actual native limits; unsupported platforms fail before advertising a driver.
    pub async fn open(config: HostConfig, clock: Arc<dyn Clock>) -> Result<Self, DriverError> {
        if !cfg!(target_os = "linux") {
            return Err(DriverError::Unsupported(
                "the built-in host driver currently verifies process identities on Linux".into(),
            ));
        }
        crate::volumes::directory(&config.base)?;
        for name in ["units", "volume-records", "runs", "volumes"] {
            crate::volumes::directory(&config.base.join(name))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config.base, std::fs::Permissions::from_mode(0o711))
                .map_err(io_error)?;
            for (name, mode) in [
                ("units", 0o700),
                ("volume-records", 0o700),
                ("runs", 0o711),
                ("volumes", 0o711),
            ] {
                std::fs::set_permissions(
                    config.base.join(name),
                    std::fs::Permissions::from_mode(mode),
                )
                .map_err(io_error)?;
            }
        }
        let controls = if config.prefer_scopes {
            crate::limits::probe().await
        } else {
            Default::default()
        };
        Ok(Self {
            config,
            clock,
            controls,
            locks: Mutex::new(BTreeMap::new()),
        })
    }

    async fn lock(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.unit_lock(name).lock_owned().await
    }

    fn unit_lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(name.into())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn record_path(&self, name: &str) -> Result<PathBuf, DriverError> {
        crate::volumes::check_name(name)?;
        Ok(self.config.base.join("units").join(format!("{name}.json")))
    }

    fn read(&self, reference: &UnitRef) -> Result<Option<Unit>, DriverError> {
        match std::fs::read(self.record_path(&reference.name)?) {
            Ok(bytes) => {
                let unit: Unit = serde_json::from_slice(&bytes).map_err(|_| {
                    DriverError::Internal(
                        "an owned host unit record is damaged and was left intact".into(),
                    )
                })?;
                if unit.reference != *reference
                    || unit
                        .spec
                        .labels
                        .get(LABEL_SYSTEM)
                        .is_none_or(String::is_empty)
                {
                    return Err(DriverError::NotFound(
                        "the named host process is not this owned unit".into(),
                    ));
                }
                Ok(Some(unit))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    fn require(&self, reference: &UnitRef) -> Result<Unit, DriverError> {
        self.read(reference)?
            .ok_or_else(|| DriverError::NotFound("the owned host unit is absent".into()))
    }
    fn save(&self, unit: &Unit) -> Result<(), DriverError> {
        write_json(&self.record_path(&unit.reference.name)?, unit)
    }

    fn file(&self, path: &Path, bytes: &[u8], identity: &Identity) -> Result<(), DriverError> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(DriverError::InvalidSpec(
                "a host launch file must not be a symbolic link".into(),
            ));
        }
        options
            .open(path)
            .map_err(io_error)?
            .write_all(bytes)
            .map_err(io_error)?;
        crate::identity::own(path, identity)
    }

    fn prepare_files(&self, unit: &Unit, spec: &RuntimeSpec) -> Result<(), DriverError> {
        self.file(
            &unit.paths.run.join("ca.pem"),
            spec.ca_cert.as_bytes(),
            &unit.identity,
        )?;
        self.file(
            &unit.paths.run.join("enroll"),
            spec.enroll_token.expose().as_bytes(),
            &unit.identity,
        )?;
        let attached = crate::mounts::attached(spec, &self.config)?;
        self.file(
            &unit.paths.run.join("mounts.json"),
            orch_driver_contract::runtime::mounts_file(&attached).as_bytes(),
            &unit.identity,
        )
    }

    fn command(&self, unit: &Unit) -> Result<tokio::process::Command, DriverError> {
        let connector = std::fs::canonicalize(&self.config.connector).map_err(io_error)?;
        let arguments = vec!["run".into(), "--managed".into()];
        let mut command = crate::identity::launch(&unit.identity, &connector, &arguments);
        if self.controls.any() {
            let program = command.as_std().get_program().to_owned();
            let args: Vec<_> = command
                .as_std()
                .get_args()
                .map(std::ffi::OsStr::to_owned)
                .collect();
            command = tokio::process::Command::new("systemd-run");
            command
                .args([
                    "--scope",
                    "--quiet",
                    "--expand-environment=no",
                    "--unit",
                    &format!("{}.scope", unit.reference.name),
                ])
                .args(
                    self.controls
                        .properties(&unit.spec.resources, false)
                        .into_iter()
                        .map(|property| format!("--property={property}")),
                )
                .arg(program)
                .args(args);
        }
        // prlimit affects only this child; there is no unsafe pre-exec callback or process-wide limit change.
        let program = command.as_std().get_program().to_owned();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(std::ffi::OsStr::to_owned)
            .collect();
        command = tokio::process::Command::new("prlimit");
        command.args(["--core=0:0", "--"]).arg(program).args(args);
        command
            .env_clear()
            .envs(unit.spec.launch().values)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("USER", &unit.identity.name)
            .env("LOGNAME", &unit.identity.name)
            .env("ORCH_CA_FILE", unit.paths.run.join("ca.pem"))
            .env("ORCH_ENROLL_FILE", unit.paths.run.join("enroll"))
            .env("ORCH_DATA_DIR", &unit.paths.state)
            .env("ORCH_TOOLS_DIR", &self.config.tools);
        if let Some(workspace) = &unit.paths.workspace {
            command
                .env("ORCH_WORKSPACE_DIR", workspace)
                .current_dir(workspace);
        }
        if let Some(home) = &unit.paths.home {
            if unit.spec.kind == orch_driver_contract::runtime::RuntimeKind::Provider {
                command.env("ORCH_HARNESS_HOME", home).env(
                    "ORCH_HARNESS_HOME_SOURCE",
                    serde_json::to_string(&unit.spec.harness_home.clone().unwrap_or_default())
                        .map_err(|error| DriverError::InvalidSpec(error.to_string()))?,
                );
            } else {
                command.env("ORCH_HOME_DIR", home).env("HOME", home);
            }
        }
        command.env("ORCH_MOUNTS_FILE", unit.paths.run.join("mounts.json"));
        let output = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(unit.paths.run.join("connector.log"))
            .map_err(io_error)?;
        crate::identity::own(&unit.paths.run.join("connector.log"), &unit.identity)?;
        command
            .stdin(std::process::Stdio::null())
            .stdout(output.try_clone().map_err(io_error)?)
            .stderr(output)
            .kill_on_drop(false);
        #[cfg(unix)]
        command.process_group(0);
        Ok(command)
    }
}

#[async_trait]
impl EnvironmentDriver for HostDriver {
    fn describe(&self) -> DriverDescriptor {
        crate::descriptor::probed_descriptor(self.controls)
    }
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        Some(crate::volumes::storages(&self.config))
    }
    async fn validate(&self, _kind: RuntimeKind, spec: &Json) -> Result<(), DriverError> {
        let account = spec
            .get("run_as")
            .and_then(Json::as_str)
            .ok_or_else(|| DriverError::InvalidSpec("a host template needs run_as".into()))?;
        crate::identity::resolve(account)?;
        if ["workspace_base", "home_base", "mounts_base"]
            .iter()
            .any(|key| spec.get(key).is_some())
        {
            return Err(DriverError::InvalidSpec("select named machine storages in env.storage or runtime.storage; host paths do not belong in a shared template".into()));
        }
        Ok(())
    }
    async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
        let _one = self.lock(&spec.name).await;
        spec.check().map_err(|faults| {
            DriverError::InvalidSpec(
                faults
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        self.validate(spec.kind, &spec.spec).await?;
        if spec.resources.disk_gb.is_some() {
            return Err(DriverError::Unsupported(
                "host directory disk limits require a configured filesystem quota".into(),
            ));
        }
        let reference = UnitRef {
            reference: spec.name.clone(),
            name: spec.name.clone(),
            runtime_id: spec.runtime_id,
            kind: spec.kind,
        };
        if let Some(mut existing) = self.read(&reference)? {
            if existing.process.is_some_and(crate::stats::alive) {
                return Ok(reference);
            }
            self.prepare_files(&existing, spec)?;
            existing.spec = spec.clone();
            existing.spec.enroll_token = "".into();
            existing.state = UnitState::of(UnitStateKind::Stopped);
            self.save(&existing)?;
            return Ok(reference);
        }
        let identity = crate::identity::resolve(spec.spec["run_as"].as_str().unwrap_or_default())?;
        let role = |role: VolumeRole| -> Result<Option<PathBuf>, DriverError> {
            let Some(volume) = spec.volumes.get(&role) else {
                return Ok(None);
            };
            if let Some(directory) = &volume.directory {
                verify_directory(directory, &identity)?;
                return Ok(Some(PathBuf::from(&directory.path)));
            }
            let request = VolumeRequest {
                name: volume.name.clone(),
                storage: volume.storage.clone(),
                labels: spec
                    .labels
                    .iter()
                    .filter(|(key, _)| key.as_str() == LABEL_SYSTEM)
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            };
            let path = crate::volumes::ensure(&self.config, &request)?.path;
            crate::identity::own(&path, &identity)?;
            Ok(Some(path))
        };
        let paths = Paths {
            state: role(VolumeRole::State)?.ok_or_else(|| {
                DriverError::InvalidSpec("a host unit needs its persistent state directory".into())
            })?,
            workspace: role(VolumeRole::Workspace)?,
            home: role(VolumeRole::Home)?,
            run: self.config.base.join("runs").join(&spec.name),
        };
        crate::volumes::directory(&paths.run)?;
        crate::identity::own(&paths.run, &identity)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&paths.run, std::fs::Permissions::from_mode(0o700))
                .map_err(io_error)?;
        }
        let mut unit = Unit {
            reference: reference.clone(),
            spec: spec.clone(),
            identity,
            paths,
            process: None,
            state: UnitState::of(UnitStateKind::Stopped),
        };
        self.prepare_files(&unit, spec)?;
        unit.spec.enroll_token = "".into();
        self.save(&unit)?;
        Ok(reference)
    }
    async fn start(&self, reference: &UnitRef) -> Result<(), DriverError> {
        let _one = self.lock(&reference.name).await;
        let mut unit = self.require(reference)?;
        if unit.process.is_some_and(crate::stats::alive) {
            return Ok(());
        }
        for directory in unit
            .spec
            .volumes
            .values()
            .filter_map(|volume| volume.directory.as_ref())
        {
            verify_directory(directory, &unit.identity)?;
        }
        let mut child = self.command(&unit)?.spawn().map_err(io_error)?;
        let pid = child.id().ok_or_else(|| {
            DriverError::Unavailable("the host connector did not supply a process identity".into())
        })?;
        let Some(process) = crate::stats::identity(pid) else {
            let _ = child.kill().await;
            return Err(DriverError::Unavailable(
                "the host connector exited before its identity could be verified".into(),
            ));
        };
        unit.process = Some(process);
        unit.state = UnitState::of(UnitStateKind::Running);
        unit.state.started_at = Some(self.clock.time());
        self.save(&unit)?;
        let path = self.record_path(&reference.name)?;
        let clock = Arc::clone(&self.clock);
        let serial = self.unit_lock(&reference.name);
        let _observer = tokio::spawn(async move {
            let ended = child.wait().await;
            let _one = serial.lock().await;
            let Ok(bytes) = std::fs::read(&path) else {
                return;
            };
            let Ok(mut saved) = serde_json::from_slice::<Unit>(&bytes) else {
                return;
            };
            if saved.process != Some(process) || saved.state.state != UnitStateKind::Running {
                return;
            }
            saved.process = None;
            saved.state.finished_at = Some(clock.time());
            saved.state.exit_code = ended
                .as_ref()
                .ok()
                .and_then(std::process::ExitStatus::code)
                .map(i64::from);
            saved.state.state = if ended.is_ok_and(|status| status.success()) {
                UnitStateKind::Stopped
            } else {
                UnitStateKind::Failed
            };
            saved.state.detail = (saved.state.state == UnitStateKind::Failed)
                .then(|| "The host connector exited.".into());
            let _ = write_json(&path, &saved);
        });
        Ok(())
    }
    async fn stop(&self, reference: &UnitRef, grace: Duration) -> Result<(), DriverError> {
        let _one = self.lock(&reference.name).await;
        let mut unit = self.require(reference)?;
        if let Some(process) = unit.process.filter(|process| crate::stats::alive(*process)) {
            let mut owned = crate::stats::tree(process);
            #[cfg(unix)]
            for member in owned
                .iter()
                .rev()
                .filter(|member| crate::stats::alive(**member))
            {
                if let Some(pid) = rustix::process::Pid::from_raw(member.pid as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
                }
            }
            let until = tokio::time::Instant::now() + grace;
            const STOP_POLL: Duration = Duration::from_millis(50);
            while owned.iter().copied().any(crate::stats::alive)
                && tokio::time::Instant::now() < until
            {
                tokio::time::sleep(STOP_POLL).await;
            }
            owned.extend(crate::stats::tree(process));
            owned.sort_by_key(|process| (process.pid, process.start_ticks));
            owned.dedup();
            #[cfg(unix)]
            for member in owned
                .iter()
                .rev()
                .filter(|member| crate::stats::alive(**member))
            {
                if let Some(pid) = rustix::process::Pid::from_raw(member.pid as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            }
        }
        unit.process = None;
        unit.state = UnitState::of(UnitStateKind::Stopped);
        unit.state.finished_at = Some(self.clock.time());
        self.file(&unit.paths.run.join("enroll"), &[], &unit.identity)?;
        self.save(&unit)
    }
    async fn remove(&self, reference: &UnitRef) -> Result<(), DriverError> {
        if self.read(reference)?.is_none() {
            return Ok(());
        }
        self.stop(reference, Duration::ZERO).await?;
        let _one = self.lock(&reference.name).await;
        let unit = self.require(reference)?;
        std::fs::remove_dir_all(&unit.paths.run).map_err(io_error)?;
        std::fs::remove_file(self.record_path(&reference.name)?).map_err(io_error)
    }
    async fn inspect(&self, reference: &UnitRef) -> Result<UnitState, DriverError> {
        let Some(unit) = self.read(reference)? else {
            return Ok(UnitState::of(UnitStateKind::Absent));
        };
        if unit.state.state == UnitStateKind::Running
            && !unit.process.is_some_and(crate::stats::alive)
        {
            let mut state = UnitState::of(UnitStateKind::Failed);
            state.detail = Some("The recorded host connector is no longer running.".into());
            return Ok(state);
        }
        Ok(unit.state)
    }
    async fn list(&self, system: &str) -> Result<Vec<UnitRef>, DriverError> {
        let mut units = Vec::new();
        for entry in std::fs::read_dir(self.config.base.join("units")).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if !entry.file_type().map_err(io_error)?.is_file()
                || entry
                    .path()
                    .extension()
                    .is_none_or(|extension| extension != "json")
            {
                continue;
            }
            let unit: Unit =
                serde_json::from_slice(&std::fs::read(entry.path()).map_err(io_error)?)
                    .map_err(|_| DriverError::Internal("a host unit record is damaged".into()))?;
            if unit
                .spec
                .labels
                .get(LABEL_SYSTEM)
                .is_some_and(|held| held == system)
            {
                units.push(unit.reference);
            }
        }
        Ok(units)
    }
    async fn stats(&self, reference: &UnitRef) -> Result<ResourceSample, DriverError> {
        let unit = self.require(reference)?;
        let process = unit
            .process
            .ok_or_else(|| DriverError::NotFound("the host connector is stopped".into()))?;
        crate::stats::sample(process, &unit.spec.resources, self.clock.as_ref())
    }
    async fn ensure_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        crate::volumes::ensure(&self.config, volume).map(|_| ())
    }
    async fn volume_exists(&self, volume: &VolumeRequest) -> Result<bool, DriverError> {
        crate::volumes::exists(&self.config, volume)
    }
    async fn volume_identity(
        &self,
        volume: &VolumeRequest,
    ) -> Result<Option<orch_driver_contract::volumes::VolumeIdentity>, DriverError> {
        let Some(found) = crate::volumes::read(&self.config, &volume.name)? else {
            return Ok(None);
        };
        if found.request.storage != volume.storage || !found.path.is_dir() {
            return Err(DriverError::InvalidSpec(
                "the existing volume has another storage or is missing".into(),
            ));
        }
        Ok(Some(orch_driver_contract::volumes::VolumeIdentity {
            labels: found.request.labels,
        }))
    }
    async fn remove_volume(&self, name: &str) -> Result<(), DriverError> {
        for reference in self.list_all()? {
            let unit = self.require(&reference)?;
            if unit.spec.volumes.values().any(|volume| volume.name == name) {
                return Err(DriverError::Busy(
                    "a host unit still owns this volume".into(),
                ));
            }
        }
        crate::volumes::remove(&self.config, name)
    }
}

impl HostDriver {
    fn list_all(&self) -> Result<Vec<UnitRef>, DriverError> {
        std::fs::read_dir(self.config.base.join("units"))
            .map_err(io_error)?
            .map(|entry| {
                let path = entry.map_err(io_error)?.path();
                let unit: Unit = serde_json::from_slice(&std::fs::read(path).map_err(io_error)?)
                    .map_err(|_| DriverError::Internal("a host unit record is damaged".into()))?;
                Ok(unit.reference)
            })
            .collect()
    }
}
