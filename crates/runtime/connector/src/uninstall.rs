//! Installer receipts, shared-program ownership and quiescence for local uninstall plans.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The receipt belongs to this application's data directory, rather than to a guessed user path.
pub const RECEIPT: &str = ".orch-installation.json";
/// Format changes are refused rather than treated as an empty list of other instances.
const VERSION: u32 = 1;

/// What an installer, rather than a discovered existing file, placed for one instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installation {
    /// The supported receipt format.
    pub version: u32,
    /// The actual executable that this instance shares with its peers.
    pub program: PathBuf,
    /// Its exact application data root.
    pub data_dir: PathBuf,
    /// Configuration files created by the installer; existing supplied files are absent.
    pub configuration: Vec<PathBuf>,
    /// Whether the installed connector service has its named-directory suffix.
    pub named_service: bool,
    /// The named service's suffix, derived from the installer's path spelling before canonicalization.
    pub service_suffix: Option<String>,
    /// Whether the installer started a service for this instance.
    pub service_installed: bool,
    /// Whether this program was installed by the installer, rather than run from another location.
    pub program_owned: bool,
    /// Whether uninstall has ended this instance while retaining its data.
    pub uninstalled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Instances {
    version: u32,
    program: PathBuf,
    program_owned: bool,
    instances: BTreeMap<PathBuf, bool>,
}

/// A receipt error preserves the files whose ownership cannot be proved.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A required file cannot be read or changed.
    #[error("{path}: {source}")]
    Io {
        /// The exact local file involved.
        path: PathBuf,
        /// The operating system's failure.
        source: std::io::Error,
    },
    /// A different operation owns the program's installation lock.
    #[error("{0}")]
    Busy(String),
    /// Missing or malformed metadata is unknown, never proof that no other instance exists.
    #[error("the installation ownership record is invalid: {0}")]
    Invalid(String),
    /// The displayed plan cannot proceed or has not been accepted.
    #[error("{0}")]
    Refused(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

fn registry_path(program: &Path) -> Result<PathBuf, Error> {
    let name = program
        .file_name()
        .ok_or_else(|| Error::Invalid("the program has no file name".into()))?;
    let mut file = std::ffi::OsString::from(".");
    file.push(name);
    file.push(".instances.json");
    Ok(program.with_file_name(file))
}

fn program_lock(program: &Path) -> Result<File, Error> {
    // The verified updater already uses this same lock, so its swap cannot race program removal.
    let mut name = program.as_os_str().to_owned();
    name.push(".update-lock");
    let path = PathBuf::from(name);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(io(&path))?;
    file.try_lock().map_err(|error| {
        Error::Busy(format!(
            "the program is being installed, updated or uninstalled: {error}"
        ))
    })?;
    Ok(file)
}

fn write(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|error| Error::Invalid(error.to_string()))?;
    crate::policy::write_atomically(path, &bytes).map_err(io(path))
}

/// Reads a receipt without creating a data directory or following a substituted receipt link.
pub fn inspect(data_dir: &Path) -> Result<Option<Installation>, Error> {
    let path = data_dir.join(RECEIPT);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(Error::Invalid("the receipt is not a real file".into()));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(&path)(error)),
        _ => {}
    }
    let receipt: Installation = serde_json::from_slice(&std::fs::read(&path).map_err(io(&path))?)
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let actual = std::fs::canonicalize(data_dir).map_err(io(data_dir))?;
    if receipt.version != VERSION || receipt.data_dir != actual {
        return Err(Error::Invalid(
            "the receipt names another directory or format".into(),
        ));
    }
    Ok(Some(receipt))
}

/// Reads ownership eligibility for a reported command without creating a lock or changing any file.
pub fn available(data_dir: &Path, program: &Path) -> Result<bool, Error> {
    let Some(receipt) = inspect(data_dir)? else {
        return Ok(false);
    };
    let program = std::fs::canonicalize(program).map_err(io(program))?;
    if receipt.program != program {
        return Ok(false);
    }
    let path = registry_path(&program)?;
    if !std::fs::symlink_metadata(&path)
        .map_err(io(&path))?
        .is_file()
    {
        return Ok(false);
    }
    let registry: Instances = serde_json::from_slice(&std::fs::read(&path).map_err(io(&path))?)
        .map_err(|error| Error::Invalid(error.to_string()))?;
    Ok(registry.version == VERSION
        && registry.program == program
        && registry.instances.contains_key(&receipt.data_dir))
}

/// Counts a new or revived manual instance before an installer-registered program admits work.
pub fn register_running(data_dir: &Path, program: &Path, named: bool) -> Result<(), Error> {
    match inspect(data_dir) {
        Ok(Some(receipt)) if !receipt.uninstalled => return Ok(()),
        Err(_) => return Ok(()),
        _ => {}
    }
    let program = std::fs::canonicalize(program).map_err(io(program))?;
    let path = registry_path(&program)?;
    let registry = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Instances>(&bytes).ok());
    if registry.is_some_and(|registry| registry.version == VERSION && registry.program == program) {
        register(&program, data_dir, Vec::new(), named, false, false)?;
    }
    Ok(())
}

/// Records an installer-owned program/configuration and this instance before its service is started.
pub fn register(
    program: &Path,
    data_dir: &Path,
    configuration: Vec<PathBuf>,
    named_service: bool,
    service_installed: bool,
    program_owned: bool,
) -> Result<Installation, Error> {
    let service_suffix = named_service.then(|| crate::service::suffix_of(data_dir));
    let program = std::fs::canonicalize(program).map_err(io(program))?;
    let data_dir = std::fs::canonicalize(data_dir).map_err(io(data_dir))?;
    let _lock = program_lock(&program)?;
    let previous = inspect(&data_dir)?.filter(|receipt| !receipt.uninstalled);
    if previous.as_ref().is_some_and(|receipt| {
        receipt.program != program || receipt.service_suffix != service_suffix
    }) {
        return Err(Error::Invalid(
            "this directory belongs to another program or service instance".into(),
        ));
    }
    let mut configuration_paths = std::collections::BTreeSet::new();
    for path in configuration {
        configuration_paths.insert(std::fs::canonicalize(&path).map_err(io(&path))?);
    }
    let mut configuration = configuration_paths;
    let service_installed = service_installed
        || previous
            .as_ref()
            .is_some_and(|receipt| receipt.service_installed);
    if let Some(previous) = previous {
        configuration.extend(previous.configuration);
    }
    let path = registry_path(&program)?;
    let mut registry = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Instances>(&bytes)
            .map_err(|error| Error::Invalid(error.to_string()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Instances {
            version: VERSION,
            program: program.clone(),
            program_owned,
            instances: BTreeMap::new(),
        },
        Err(error) => return Err(io(&path)(error)),
    };
    if registry.version != VERSION || registry.program != program {
        return Err(Error::Invalid(
            "the program registry names another installation".into(),
        ));
    }
    registry.program_owned |= program_owned;
    registry.instances.insert(data_dir.clone(), true);
    let receipt = Installation {
        version: VERSION,
        program,
        data_dir,
        configuration: configuration.into_iter().collect(),
        named_service,
        service_suffix,
        service_installed,
        program_owned: registry.program_owned,
        uninstalled: false,
    };
    write(&path, &registry)?;
    write(&receipt.data_dir.join(RECEIPT), &receipt)?;
    Ok(receipt)
}

/// Holds installation ownership steady from the final plan check through last-instance removal.
pub struct Removing {
    _lock: File,
    path: PathBuf,
    registry: Instances,
    receipt: Installation,
}

impl Removing {
    /// The verified exact paths for this removal, held steady by the installation lock.
    #[must_use]
    pub fn installation(&self) -> &Installation {
        &self.receipt
    }
    /// Validates the exact program and instance; an unknown registry never permits program deletion.
    pub fn begin(receipt: Installation, executing_program: &Path) -> Result<Self, Error> {
        let executing_program =
            std::fs::canonicalize(executing_program).map_err(io(executing_program))?;
        if executing_program != receipt.program {
            return Err(Error::Invalid(
                "this is not the installed executable".into(),
            ));
        }
        let lock = program_lock(&receipt.program)?;
        let path = registry_path(&receipt.program)?;
        if !std::fs::symlink_metadata(&path)
            .map_err(io(&path))?
            .is_file()
        {
            return Err(Error::Invalid(
                "the program registry is not a real file".into(),
            ));
        }
        let registry: Instances = serde_json::from_slice(&std::fs::read(&path).map_err(io(&path))?)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        if registry.version != VERSION
            || registry.program != receipt.program
            || (!receipt.uninstalled && !registry.instances.contains_key(&receipt.data_dir))
        {
            return Err(Error::Invalid(
                "this instance is not in the program registry".into(),
            ));
        }
        Ok(Self {
            _lock: lock,
            path,
            registry,
            receipt,
        })
    }

    /// Other registered instances keep the shared program.
    pub fn other_instances(&self) -> usize {
        self.registry
            .instances
            .iter()
            .filter(|(directory, active)| **active && **directory != self.receipt.data_dir)
            .count()
    }

    /// Whether the exact program is owned and no other instance needs it.
    pub fn may_remove_program(&self) -> bool {
        self.registry.program_owned && self.other_instances() == 0
    }

    /// Nested active or retained instances are never destroyed by their parent's purge.
    #[must_use]
    pub fn purge_blocker(&self) -> Option<String> {
        if self.receipt.data_dir.parent().is_none()
            || self.receipt.program.starts_with(&self.receipt.data_dir)
        {
            return Some("the data directory contains the program or is a filesystem root".into());
        }
        self.registry.instances.keys().any(|directory| directory != &self.receipt.data_dir && directory.starts_with(&self.receipt.data_dir))
            .then(|| "another instance keeps data inside this directory; remove or move that instance first".into())
    }

    /// Ends the instance only after its service/data work succeeded; retained data keeps its receipt.
    pub fn finish(mut self, data_purged: bool) -> Result<(), Error> {
        if data_purged {
            self.registry.instances.remove(&self.receipt.data_dir);
        } else {
            self.registry
                .instances
                .insert(self.receipt.data_dir.clone(), false);
            self.receipt.uninstalled = true;
            write(&self.receipt.data_dir.join(RECEIPT), &self.receipt)?;
        }
        write(&self.path, &self.registry)
    }
}

/// Adds the common service, program, installer configuration and data fates to a terminal plan.
#[must_use]
pub fn installation_items(removing: &Removing, purge: bool) -> Vec<Item> {
    let receipt = removing.installation();
    let mut items = vec![
        Item {
            name: receipt.program.display().to_string(),
            remove: removing.may_remove_program(),
            bytes: size(&receipt.program),
            blocker: (!removing.may_remove_program()).then(|| {
                if removing.other_instances() > 0 {
                    format!(
                        "kept for {} other active instances",
                        removing.other_instances()
                    )
                } else {
                    "the program was not placed by the installer".into()
                }
            }),
        },
        Item {
            name: receipt.data_dir.display().to_string(),
            remove: purge,
            bytes: size(&receipt.data_dir),
            blocker: if purge {
                removing.purge_blocker()
            } else {
                Some("kept for restore until you purge it".into())
            },
        },
    ];
    if receipt.service_installed && !receipt.uninstalled {
        items.insert(
            0,
            Item {
                name: "service".into(),
                remove: true,
                bytes: None,
                blocker: None,
            },
        );
    }
    items.extend(receipt.configuration.iter().map(|path| Item {
        name: path.display().to_string(),
        remove: true,
        bytes: size(path),
        blocker: configuration_blocker(path),
    }));
    items
}

fn configuration_blocker(path: &Path) -> Option<String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(metadata)
            if metadata.is_file()
                && std::fs::canonicalize(path).is_ok_and(|actual| actual == path) =>
        {
            None
        }
        _ => Some("the installed configuration path was replaced or cannot be verified".into()),
    }
}

fn remove_file(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(path)(error)),
    }
}

/// Removes verified installer files only after service and runtime work succeeded.
/// On Windows the running executable's removal is scheduled after this process exits.
pub fn finish_files(removing: Removing, purge: bool) -> Result<bool, Error> {
    let receipt = removing.installation();
    if purge && let Some(blocker) = removing.purge_blocker() {
        return Err(Error::Invalid(blocker));
    }
    if inspect(&receipt.data_dir)?.as_ref() != Some(receipt) {
        return Err(Error::Invalid(
            "the installation record changed after planning".into(),
        ));
    }
    for path in &receipt.configuration {
        if let Some(blocker) = configuration_blocker(path) {
            return Err(Error::Refused(blocker));
        }
    }
    for path in &receipt.configuration {
        remove_file(path)?;
    }
    if purge {
        std::fs::remove_dir_all(&receipt.data_dir).map_err(io(&receipt.data_dir))?;
    }
    #[cfg(not(windows))]
    let scheduled = false;
    #[cfg(windows)]
    let scheduled = removing.may_remove_program();
    if removing.may_remove_program() {
        #[cfg(windows)]
        {
            let script = format!(
                "Wait-Process -Id {} -ErrorAction SilentlyContinue; Remove-Item -LiteralPath '{}' -Force -ErrorAction Stop",
                std::process::id(),
                receipt.program.to_string_lossy().replace('\'', "''")
            );
            std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .spawn()
                .map_err(io(&receipt.program))?;
        }
        #[cfg(not(windows))]
        remove_file(&receipt.program)?;
    }
    removing.finish(purge)?;
    Ok(scheduled)
}

/// A stable sibling lock survives data removal, preventing a new run from entering during purge.
/// It holds no identity or credentials and is shared by both physical programs.
pub struct InstanceLock(File);

impl InstanceLock {
    /// A running process holds a shared lock until all its role work has ended.
    pub fn running(data_dir: &Path) -> Result<Self, Error> {
        if let Some(parent) = data_dir
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        let file = Self::open(data_dir)?;
        file.try_lock_shared().map_err(|_| {
            Error::Busy("this instance is being uninstalled; wait for its cleanup to settle".into())
        })?;
        Ok(Self(file))
    }

    /// Uninstall takes the exclusive lock after stopping the recorded service.
    pub fn removing(data_dir: &Path) -> Result<Self, Error> {
        let file = Self::open(data_dir)?;
        file.try_lock().map_err(|_| {
            Error::Busy("this instance is still running; stop its exact process and retry".into())
        })?;
        Ok(Self(file))
    }

    fn open(data_dir: &Path) -> Result<File, Error> {
        let absolute = std::path::absolute(data_dir).map_err(io(data_dir))?;
        let path = if absolute.exists() {
            std::fs::canonicalize(&absolute).map_err(io(&absolute))?
        } else {
            let parent = absolute
                .parent()
                .ok_or_else(|| Error::Invalid("a data directory needs a parent".into()))?;
            std::fs::canonicalize(parent).map_err(io(parent))?.join(
                absolute
                    .file_name()
                    .ok_or_else(|| Error::Invalid("a data directory needs a name".into()))?,
            )
        };
        let mut name = path.as_os_str().to_owned();
        name.push(".orch-instance-lock");
        let path = PathBuf::from(name);
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(io(&path))
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Measures files without following symbolic links; unreadable sizes stay unknown in a plan.
pub fn size(path: &Path) -> Option<u64> {
    let mut pending = vec![path.to_owned()];
    let mut bytes = 0_u64;
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).ok()? {
                pending.push(entry.ok()?.path());
            }
        } else {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Some(bytes)
}

/// A read-only preview row; a blocker is stated before anything is stopped or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// What will be removed or kept.
    pub name: String,
    /// Whether the confirmed operation removes it.
    pub remove: bool,
    /// Its measured bytes where known.
    pub bytes: Option<u64>,
    /// Why removal is blocked, or why an unselected item stays.
    pub blocker: Option<String>,
}

/// Local commands use the same plan-before-confirmation shape, in the terminal's language.
pub struct Plan {
    /// The exact enrolled name typed once to confirm.
    pub name: String,
    /// Every known removal, retention and blocker.
    pub items: Vec<Item>,
}

#[derive(Clone, Copy)]
enum PlanIntent {
    Uninstall,
    Cleanup,
}

impl Plan {
    /// Prints every fate and size, then asks once unless the caller explicitly selected yes or preview.
    pub fn confirm(&self, yes: bool, preview: bool) -> Result<bool, Error> {
        self.confirm_intent(yes, preview, PlanIntent::Uninstall)
    }

    /// Reuses the same local data-fate confirmation for operator cleanup.
    pub fn confirm_cleanup(&self, yes: bool, preview: bool) -> Result<bool, Error> {
        self.confirm_intent(yes, preview, PlanIntent::Cleanup)
    }

    fn confirm_intent(&self, yes: bool, preview: bool, intent: PlanIntent) -> Result<bool, Error> {
        use std::io::Write;
        let chinese = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
            .is_some_and(|value| value.starts_with("zh"));
        println!(
            "{}: {}",
            match (chinese, intent) {
                (true, PlanIntent::Uninstall) => "卸载计划",
                (false, PlanIntent::Uninstall) => "Uninstall plan",
                (true, PlanIntent::Cleanup) => "清理计划",
                (false, PlanIntent::Cleanup) => "Cleanup plan",
            },
            self.name
        );
        let remove = self.items.iter().filter(|item| item.remove).count();
        let kept = self.items.len().saturating_sub(remove);
        let blocked = self
            .items
            .iter()
            .filter(|item| item.remove && item.blocker.is_some())
            .count();
        if chinese {
            println!("  {remove} 项移除，{kept} 项保留，{blocked} 项受阻");
        } else {
            println!("  {remove} to remove, {kept} kept, {blocked} blocked");
        }
        for item in &self.items {
            let action = if item.remove && item.blocker.is_some() {
                if chinese { "受阻" } else { "Blocked" }
            } else if item.remove {
                if chinese { "移除" } else { "Remove" }
            } else if chinese {
                "保留"
            } else {
                "Keep"
            };
            let bytes = item.bytes.map_or_else(
                || {
                    if chinese {
                        "大小未知".into()
                    } else {
                        "size unknown".into()
                    }
                },
                |bytes| format!("{bytes} B"),
            );
            println!(
                "  {action}: {} ({bytes}){}",
                item.name,
                item.blocker
                    .as_ref()
                    .map_or_else(String::new, |reason| format!(" — {reason}"))
            );
        }
        if preview {
            return Ok(false);
        }
        if self
            .items
            .iter()
            .any(|item| item.remove && item.blocker.is_some())
        {
            return Err(Error::Refused("the plan has blocked removals".into()));
        }
        if yes {
            return Ok(true);
        }
        print!(
            "{} '{}': ",
            if chinese {
                "输入名称以确认"
            } else {
                "Type the name to confirm"
            },
            self.name
        );
        std::io::stdout().flush().map_err(io(Path::new("stdout")))?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(io(Path::new("stdin")))?;
        if answer.trim_end() != self.name {
            return Err(Error::Refused(
                "the confirmation did not match; nothing was removed".into(),
            ));
        }
        Ok(true)
    }
}

/// Ctrl-C requests a stop between irreversible items rather than abandoning the current native effect.
pub struct Cancellation {
    token: tokio_util::sync::CancellationToken,
    listener: tokio::task::JoinHandle<()>,
}

impl Cancellation {
    /// Installs one listener only after the plan was accepted.
    #[must_use]
    pub fn listen() -> Self {
        let token = tokio_util::sync::CancellationToken::new();
        let signal = token.clone();
        let listener = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                signal.cancel();
            }
        });
        Self { token, listener }
    }

    /// The current safe cancellation request.
    #[must_use]
    pub fn token(&self) -> &tokio_util::sync::CancellationToken {
        &self.token
    }

    /// Checks before the next irreversible file operation.
    pub fn check(&self) -> Result<(), Error> {
        if self.token.is_cancelled() {
            Err(Error::Refused(
                "uninstall was cancelled; completed removals stay removed".into(),
            ))
        } else {
            Ok(())
        }
    }
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        self.listener.abort();
    }
}
