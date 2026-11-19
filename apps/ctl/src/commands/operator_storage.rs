//! The updater owns release and dump files; its protected report grants the API no file-writing authority.

use clap::{Args, Subcommand};
use orch_api_contract::resource_facts::DependentRelation;
use orch_api_contract::storage::{
    KeptRecord, KeptWhy, OperatorStorageDependency, OperatorStorageInventory, OperatorStorageItem,
    StorageAvailability, StorageKind,
};
use orch_connector_core::uninstall::{Item, Plan, size};
use orch_pki::certificates::Certificate;
use orch_types::{Clock, Json, SystemClock, Time};
use orch_worker_core::backups::{self, Manifest};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The metadata and pairing evidence begin at their first published format.
const FORMAT: u32 = 1;
/// Matches the existing backup manifest reader's bounded metadata allocation.
const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
/// Payload names are the updater's existing installation vocabulary.
const PAYLOADS: [&str; 3] = ["server", "frontend", "cli"];
/// Backup pairs have always been published beneath this deployment-relative directory.
const BACKUPS: &str = "runtime/update-backups";
/// The updater publishes a lowercase BLAKE3 digest, with at most its final newline.
const COMPLETE_HASH_BYTES: usize = 64;

/// Internal arguments used by update.sh while its deployment lock is held.
#[derive(Debug, Args)]
pub struct Options {
    #[command(subcommand)]
    command: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Publish a protected read-only inventory; no application or updater files are removed.
    Inventory {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        keep: Option<u64>,
    },
    /// Remove a verified local dump pair after its data-name confirmation.
    DumpRemove {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Record a completed local blob copy on its existing immutable dump manifest.
    Pair {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        directory: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    device: u64,
    inode: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct FilePin {
    identity: Identity,
    bytes: u64,
    modified: Option<std::time::SystemTime>,
    changed: (i64, i64),
}

fn file_pin(path: &Path) -> Result<Option<FilePin>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("the selected file is unreadable".into()),
    };
    if !metadata.is_file() {
        return Err("the selected file must be a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(FilePin {
            identity: Identity {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            bytes: metadata.len(),
            modified: metadata.modified().ok(),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }))
    }
    #[cfg(not(unix))]
    {
        Err("the local updater requires stable Unix file identities".into())
    }
}

fn identity(path: &Path) -> Result<Identity, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "the recorded directory is unreadable")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("the recorded directory must be a real directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        Err("the local updater requires stable Unix directory identities".into())
    }
}

fn plain(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

fn release_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn read_json(path: &Path) -> Result<(Json, Vec<u8>), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "the stored manifest is unreadable")?;
    if !metadata.is_file() || metadata.len() > MAX_METADATA_BYTES {
        return Err("the stored manifest must be a bounded regular file".into());
    }
    let bytes = fs::read(path).map_err(|_| "the stored manifest is unreadable")?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err("the stored manifest exceeds its metadata bound".into());
    }
    Ok((
        serde_json::from_slice(&bytes).map_err(|_| "the stored manifest does not read")?,
        bytes,
    ))
}

fn replace_metadata(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "the existing metadata is unreadable")?;
    if !metadata.is_file() {
        return Err("the existing metadata must be a regular file".into());
    }
    backups::atomic(path, bytes).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Root-run updaters must leave service-written manifests readable by that same service.
        std::os::unix::fs::chown(path, Some(metadata.uid()), Some(metadata.gid()))
            .map_err(|_| "the metadata ownership could not be preserved")?;
    }
    Ok(())
}

struct Root {
    path: PathBuf,
    identity: Identity,
    system: String,
}

impl Root {
    fn open(path: &Path) -> Result<Self, String> {
        let identity = identity(path)?;
        let path = fs::canonicalize(path).map_err(|_| "the deployment directory is unreadable")?;
        if path.to_str().is_none() {
            return Err("the deployment path cannot be represented in a copyable command".into());
        }
        let pem = fs::read_to_string(path.join("config/secrets/orch_ca_cert.pem"))
            .map_err(|_| "the installation's public authority certificate is unreadable")?;
        let system = Certificate::from_pem(&pem)
            .map_err(|_| "the installation's public authority certificate does not read")?
            .fingerprint();
        Ok(Self {
            path,
            identity,
            system,
        })
    }

    fn check(&self) -> Result<(), String> {
        if identity(&self.path)? != self.identity {
            return Err("the deployment directory changed; review cleanup again".into());
        }
        Ok(())
    }

    fn directory(&self, relative: &Path) -> Result<PathBuf, String> {
        self.check()?;
        let mut path = self.path.clone();
        for component in relative.components() {
            let Component::Normal(part) = component else {
                return Err("the recorded directory must stay inside this deployment".into());
            };
            path.push(part);
            identity(&path)?;
        }
        Ok(path)
    }

    fn dump(&self, name: &str) -> Result<(PathBuf, PathBuf), String> {
        if !plain(name) || !name.ends_with(".dump") {
            return Err("name one plain dump filename".into());
        }
        let directory = self.directory(Path::new(BACKUPS))?;
        let dump = directory.join(name);
        Ok((dump.clone(), dump.with_extension("manifest.json")))
    }

    fn copy_path(&self, relative: &str) -> Result<PathBuf, String> {
        let path = Path::new(relative);
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(
                "the paired copy must be a relative directory beneath update-backups".into(),
            );
        }
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        let directory = self.directory(&Path::new(BACKUPS).join(parent))?;
        Ok(directory.join(
            path.file_name()
                .ok_or("the paired copy has no directory name")?,
        ))
    }

    fn copy(&self, relative: &str) -> Result<PathBuf, String> {
        let path = self.copy_path(relative)?;
        identity(&path)?;
        Ok(path)
    }

    fn command(&self, suffix: &str) -> String {
        format!(
            "{} {suffix}",
            quote(&self.path.join("update.sh").to_string_lossy())
        )
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CopyEvidence {
    format_version: u32,
    system_id: String,
    dump_hash: String,
    directory: String,
    identity: Identity,
    completed_at: Time,
    #[serde(default)]
    bytes: Option<u64>,
    #[serde(default)]
    purging: bool,
}

fn manifest(
    root: &Root,
    name: &str,
    verify_dump: bool,
) -> Result<(Manifest, Json, Vec<u8>), String> {
    let (dump, path) = root.dump(name)?;
    let (raw, bytes) = read_json(&path)?;
    let parsed: Manifest =
        serde_json::from_value(raw.clone()).map_err(|_| "the dump manifest does not read")?;
    if parsed.format_version != FORMAT
        || parsed.system_id != root.system
        || parsed.dump.name != name
        || parsed.completed_at < parsed.database_at
    {
        return Err("the dump manifest does not match this installation and filename".into());
    }
    if verify_dump {
        let (size, hash) = backups::hash_file(&dump).map_err(|error| error.to_string())?;
        if parsed.dump.bytes != size || parsed.dump.hash != hash {
            return Err("the dump bytes differ from their completed manifest".into());
        }
    }
    Ok((parsed, raw, bytes))
}

fn copy_evidence(root: &Root, parsed: &Manifest, raw: &Json) -> Result<CopyEvidence, String> {
    let evidence: CopyEvidence = serde_json::from_value(
        raw.get("paired_blobs").cloned().unwrap_or(Json::Null),
    )
    .map_err(
        |_| "the paired blob copy is not recorded; use backup --repair-record with --paired-blobs",
    )?;
    if evidence.format_version != FORMAT
        || evidence.system_id != root.system
        || evidence.dump_hash != parsed.dump.hash
        || evidence.completed_at < parsed.database_at
    {
        return Err("the paired blob copy does not match this dump".into());
    }
    Ok(evidence)
}

fn record_copy(root: &Root, name: &str, directory: &str, at: Time) -> Result<(), String> {
    let (parsed, mut raw, _) = manifest(root, name, true)?;
    if raw.pointer("/paired_blobs/purging").and_then(Json::as_bool) == Some(true) {
        return Err(
            "this pair's removal already started; it cannot become restorable again".into(),
        );
    }
    let copy = root.copy(directory)?;
    let (dump, manifest_path) = root.dump(name)?;
    if dump.starts_with(&copy) || manifest_path.starts_with(&copy) {
        return Err("the paired blob directory cannot contain its dump or manifest".into());
    }
    let text = fs::read_to_string(root.path.join("config/orch.toml"))
        .map_err(|_| "the deployment configuration is unreadable")?;
    let config: toml::Value =
        toml::from_str(&text).map_err(|_| "the deployment configuration does not read")?;
    let hours = match config
        .get("retention")
        .and_then(|value| value.get("blob_grace_hours"))
    {
        None => {
            orch_config::ServerConfig::default()
                .retention
                .blob_grace_hours
        }
        Some(value) => u64::try_from(value.as_integer().ok_or("blob_grace_hours is invalid")?)
            .map_err(|_| "blob_grace_hours is invalid")?,
    };
    let grace = chrono::Duration::try_hours(
        i64::try_from(hours).map_err(|_| "blob_grace_hours is too large")?,
    )
    .ok_or("blob_grace_hours is too large")?;
    let previous = copy_evidence(root, &parsed, &raw)
        .ok()
        .filter(|evidence| !evidence.purging)
        .filter(|evidence| evidence.directory == directory)
        .filter(|evidence| identity(&copy).is_ok_and(|current| current == evidence.identity));
    let completed_at = previous
        .as_ref()
        .map_or(at, |evidence| evidence.completed_at);
    if completed_at < parsed.database_at
        || completed_at
            .get()
            .signed_duration_since(parsed.database_at.get())
            > grace
    {
        return Err(
            "the copy was not recorded within this dump's blob grace; make a new complete pair"
                .into(),
        );
    }
    for entry in fs::read_dir(root.directory(Path::new(BACKUPS))?)
        .map_err(|_| "the backup directory is unreadable")?
    {
        let path = entry
            .map_err(|_| "the backup directory is unreadable")?
            .path();
        if path == manifest_path || path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        if let Ok((other, _)) = read_json(&path)
            && let Some(other) = other.get("paired_blobs")
            && let Ok(other) = serde_json::from_value::<CopyEvidence>(other.clone())
        {
            let other = Path::new(&other.directory);
            let chosen = Path::new(directory);
            if other.starts_with(chosen) || chosen.starts_with(other) {
                return Err("another dump already owns this copy or a directory inside it".into());
            }
        }
    }
    let copy_identity = identity(&copy)?;
    let bytes = size(&copy).ok_or("the completed copy could not be measured")?;
    if identity(&copy)? != copy_identity {
        return Err("the completed copy changed; record it again".into());
    }
    raw["paired_blobs"] = serde_json::to_value(CopyEvidence {
        format_version: FORMAT,
        system_id: root.system.clone(),
        dump_hash: parsed.dump.hash,
        directory: directory.into(),
        identity: copy_identity,
        completed_at,
        bytes: Some(bytes),
        purging: false,
    })
    .map_err(|_| "the copy record could not be encoded")?;
    replace_metadata(
        &manifest_path,
        &serde_json::to_vec(&raw).map_err(|_| "the copy record could not be encoded")?,
    )
}

fn row(id: String, kind: StorageKind, name: String, at: Time) -> KeptRecord {
    KeptRecord {
        accounting_key: id.clone(),
        id,
        kind,
        name,
        resource: None,
        place: None,
        host: None,
        machine: None,
        owner: None,
        bytes: None,
        included_in: None,
        why: KeptWhy::Unknown,
        since: None,
        until: None,
        facts: None,
        observed_at: at,
        availability: StorageAvailability::Ready,
        can_clean: false,
        blocker: None,
        operator_command: None,
    }
}

fn bytes(path: &Path) -> Option<i64> {
    size(path).and_then(|value| i64::try_from(value).ok())
}

fn state(root: &Root) -> Result<Vec<u8>, String> {
    let directory = root.directory(Path::new("runtime/update"))?;
    let path = directory.join("state");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err("the updater state is unreadable".into()),
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_METADATA_BYTES => {
            let bytes = fs::read(path).map_err(|_| "the updater state is unreadable")?;
            if bytes.len() as u64 > MAX_METADATA_BYTES || std::str::from_utf8(&bytes).is_err() {
                return Err("the updater state does not read".into());
            }
            Ok(bytes)
        }
        Ok(_) => Err("the updater state must be a bounded regular file".into()),
    }
}

fn state_field<'a>(state: &'a [u8], key: &str) -> Option<&'a str> {
    std::str::from_utf8(state).ok()?.lines().find_map(|line| {
        line.split_once('=')
            .filter(|(name, _)| *name == key)
            .map(|(_, value)| value)
    })
}

fn no_transaction(root: &Root) -> Result<(), String> {
    let directory = root.directory(Path::new("runtime/update"))?;
    match fs::symlink_metadata(directory.join("journal")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("the updater's current hold is unreadable".into()),
        Ok(_) => Err("an update is unfinished; run update.sh recover first".into()),
    }
}

fn inventory(root: &Root, keep: Option<u64>, at: Time) -> Result<OperatorStorageInventory, String> {
    let mut items = Vec::new();
    let mut issues = Vec::new();
    let state = state(root)?;
    for payload in PAYLOADS {
        let relative = Path::new("runtime").join(payload).join("releases");
        let directory = match root.directory(&relative) {
            Ok(directory) => directory,
            Err(reason) => {
                if state_field(&state, "current_tag").is_some()
                    || fs::symlink_metadata(root.path.join("runtime").join(payload)).is_ok()
                {
                    issues.push(format!("{payload}: {reason}"));
                }
                continue;
            }
        };
        let current = fs::read_link(root.path.join("runtime").join(payload).join("current")).ok();
        let previous = fs::read_link(root.path.join("runtime").join(payload).join("previous")).ok();
        let mut releases = Vec::new();
        for entry in fs::read_dir(directory).map_err(|_| "the release directory is unreadable")? {
            let entry = entry.map_err(|_| "the release directory is unreadable")?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !release_name(&name) {
                continue;
            }
            let path = entry.path();
            if identity(&path).is_err() {
                continue;
            }
            let marker = path.join(".complete");
            let Ok(metadata) = fs::symlink_metadata(&marker) else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > (COMPLETE_HASH_BYTES + 1) as u64 {
                continue;
            }
            let Ok(marker) = fs::read_to_string(&marker) else {
                continue;
            };
            let marker = marker.strip_suffix('\n').unwrap_or(&marker);
            if marker.len() != COMPLETE_HASH_BYTES
                || !marker
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                continue;
            }
            let modified = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok();
            releases.push((modified, name, path));
        }
        releases.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        let mut retained = 0_u64;
        for (modified, name, path) in releases {
            let mut record = row(
                format!("release:{payload}:{name}"),
                StorageKind::Release,
                format!("{payload}/{name}"),
                at,
            );
            record.bytes = bytes(&path);
            record.since = modified.map(|time| Time::new(time.into()));
            let mut dependencies = Vec::new();
            for (link, target) in [("current", &current), ("previous", &previous)] {
                if target.as_ref().and_then(|target| target.file_name())
                    == Some(std::ffi::OsStr::new(&name))
                {
                    dependencies.push(OperatorStorageDependency {
                        key: format!("{payload}/{link}"),
                        name: format!("{payload}/{link}"),
                        relation: if link == "current" {
                            DependentRelation::Attachment
                        } else {
                            DependentRelation::Restore
                        },
                    });
                }
            }
            if dependencies.is_empty() {
                retained = retained.saturating_add(1);
            }
            let eligible =
                dependencies.is_empty() && keep.is_some_and(|keep| keep > 0 && retained > keep);
            record.why = if dependencies.is_empty() {
                KeptWhy::Cache
            } else if dependencies
                .iter()
                .all(|edge| edge.relation == DependentRelation::Restore)
            {
                KeptWhy::Restore
            } else {
                KeptWhy::InUse
            };
            record.can_clean = eligible;
            if eligible {
                record.operator_command = Some(root.command("prune"));
            } else {
                record.blocker = Some(
                    if dependencies.is_empty() {
                        "release_retained"
                    } else {
                        "release_active"
                    }
                    .into(),
                );
            }
            items.push(OperatorStorageItem {
                record,
                dependencies,
            });
        }
    }
    if let Ok(directory) = root.directory(Path::new(BACKUPS)) {
        let mut names = std::collections::BTreeSet::new();
        for entry in fs::read_dir(directory).map_err(|_| "the backup directory is unreadable")? {
            let file = entry
                .map_err(|_| "the backup directory is unreadable")?
                .file_name();
            let Some(file) = file.to_str() else {
                issues.push("a backup filename is not recorded".into());
                continue;
            };
            if !plain(file) {
                continue;
            }
            if let Some(name) = file.strip_suffix(".manifest.json") {
                names.insert(format!("{name}.dump"));
            } else if file.ends_with(".dump") {
                names.insert(file.to_owned());
            }
        }
        for name in names {
            let path = root.dump(&name)?.1;
            let mut record = row(format!("dump:{name}"), StorageKind::Dump, name.clone(), at);
            record.why = KeptWhy::Restore;
            let mut dependencies = Vec::new();
            if state_field(&state, "previous_dump") == Some(format!("{BACKUPS}/{name}").as_str()) {
                dependencies.push(OperatorStorageDependency {
                    key: "updater/previous_dump".into(),
                    name: format!(
                        "rollback/{}",
                        state_field(&state, "previous_tag").unwrap_or("previous")
                    ),
                    relation: DependentRelation::Restore,
                });
            }
            match manifest(root, &name, false) {
                Ok((parsed, raw, _)) => {
                    record.since = Some(parsed.database_at);
                    record.bytes = bytes(&path)
                        .zip(bytes(&root.dump(&name)?.0))
                        .and_then(|(manifest, dump)| manifest.checked_add(dump));
                    match copy_evidence(root,&parsed,&raw) {
                        Ok(evidence) => match root.copy(&evidence.directory) {
                            Ok(copy) if identity(&copy).ok().as_ref()==Some(&evidence.identity) => {
                                record.bytes=record.bytes.zip(if evidence.purging {bytes(&copy)} else {evidence.bytes.and_then(|bytes|i64::try_from(bytes).ok())}).and_then(|(pair,copy)|pair.checked_add(copy));
                                record.can_clean=true;
                                record.operator_command=Some(root.command(&format!("dumps remove {}",quote(&name))));
                                if evidence.purging {record.why=KeptWhy::Unknown;}
                            }
                            _ if evidence.purging && root.copy_path(&evidence.directory).is_ok_and(|copy|matches!(fs::symlink_metadata(copy),Err(error) if error.kind()==std::io::ErrorKind::NotFound)) => { record.why=KeptWhy::Unknown;record.can_clean=true;record.operator_command=Some(root.command(&format!("dumps remove {}",quote(&name)))); }
                            _ => {record.blocker=Some("paired_copy_changed".into());}
                        },
                        Err(_) => {record.bytes=None;record.blocker=Some("paired_copy_unrecorded".into());}
                    }
                }
                Err(_) => {
                    record.blocker = Some("operator_manifest_unreadable".into());
                }
            }
            items.push(OperatorStorageItem {
                record,
                dependencies,
            });
        }
    } else if state_field(&state, "current_tag").is_some()
        || fs::symlink_metadata(root.path.join(BACKUPS)).is_ok()
    {
        issues.push("the backup inventory is unavailable".into());
    }
    Ok(OperatorStorageInventory {
        format_version: FORMAT,
        system_id: root.system.clone(),
        observed_at: at,
        items,
        issues,
    })
}

struct PreparedDump {
    dump: PathBuf,
    path: PathBuf,
    dump_pin: Option<FilePin>,
    manifest_pin: Option<FilePin>,
    raw: Json,
    before: Vec<u8>,
    evidence: CopyEvidence,
    copy: Option<PathBuf>,
    state_before: Vec<u8>,
    for_rollback: bool,
}

fn prepare_dump(root: &Root, name: &str) -> Result<PreparedDump, String> {
    no_transaction(root)?;
    let state_before = state(root)?;
    let for_rollback =
        state_field(&state_before, "previous_dump") == Some(format!("{BACKUPS}/{name}").as_str());
    let (dump, path) = root.dump(name)?;
    let dump_pin = file_pin(&dump)?;
    let manifest_pin = file_pin(&path)?;
    let (parsed, raw, before) = manifest(root, name, dump_pin.is_some())?;
    if file_pin(&dump)? != dump_pin {
        return Err("the dump changed while checking it; review cleanup again".into());
    }
    let evidence = copy_evidence(root, &parsed, &raw)?;
    if !evidence.purging && dump_pin.is_none() {
        return Err("the dump disappeared before cleanup".into());
    }
    let copy = root.copy_path(&evidence.directory)?;
    let copy = match fs::symlink_metadata(&copy) {
        Ok(_) if identity(&copy)? == evidence.identity => Some(copy),
        Ok(_) => return Err("the paired blob directory changed; review cleanup again".into()),
        Err(error) if evidence.purging && error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("the paired blob directory is unreadable".into()),
    };
    Ok(PreparedDump {
        dump,
        path,
        dump_pin,
        manifest_pin,
        raw,
        before,
        evidence,
        copy,
        state_before,
        for_rollback,
    })
}

fn remove_dump(root: &Root, name: &str, yes: bool) -> Result<(), String> {
    let prepared = match prepare_dump(root, name) {
        Ok(prepared) => prepared,
        Err(reason) => {
            let shown = name
                .chars()
                .map(|value| if value.is_control() { '�' } else { value })
                .collect::<String>();
            (Plan {
                name: shown.clone(),
                items: vec![Item {
                    name: format!("{}: {shown}", word("Dump pair", "转储备份集")),
                    remove: true,
                    bytes: None,
                    blocker: Some(reason.clone()),
                }],
            })
            .confirm_cleanup(true, true)
            .map_err(|error| error.to_string())?;
            return Err(reason);
        }
    };
    let PreparedDump {
        dump,
        path,
        dump_pin,
        manifest_pin,
        mut raw,
        before,
        mut evidence,
        copy,
        state_before,
        for_rollback,
    } = prepared;
    let mut items = vec![
        Item {
            name: name.into(),
            remove: true,
            bytes: size(&dump),
            blocker: None,
        },
        Item {
            name: path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            remove: true,
            bytes: size(&path),
            blocker: None,
        },
    ];
    if let Some(copy) = &copy {
        items.push(Item {
            name: evidence.directory.clone(),
            remove: true,
            bytes: size(copy),
            blocker: None,
        });
    }
    if for_rollback {
        println!(
            "{}: {}",
            word(
                "Removing this pair also ends its rollback recovery point",
                "移除此备份集也会移除对应的回滚恢复点"
            ),
            state_field(&state_before, "previous_tag").unwrap_or("previous")
        );
    }
    if !(Plan {
        name: name.into(),
        items,
    })
    .confirm_cleanup(yes, false)
    .map_err(|error| error.to_string())?
    {
        return Ok(());
    }
    root.check()?;
    no_transaction(root)?;
    if state(root)? != state_before {
        return Err("the updater state changed; review cleanup again".into());
    }
    root.dump(name)?;
    if file_pin(&dump)? != dump_pin || file_pin(&path)? != manifest_pin {
        return Err("a selected file changed; review cleanup again".into());
    }
    if read_json(&path)?.1 != before {
        return Err("the manifest changed; review cleanup again".into());
    }
    if let Some(copy) = &copy {
        root.copy_path(&evidence.directory)?;
        if identity(copy)? != evidence.identity {
            return Err("the paired blob directory changed; review cleanup again".into());
        }
    }
    evidence.purging = true;
    raw["paired_blobs"] = serde_json::to_value(&evidence)
        .map_err(|_| "the cleanup checkpoint could not be encoded")?;
    replace_metadata(
        &path,
        &serde_json::to_vec(&raw).map_err(|_| "the cleanup checkpoint could not be encoded")?,
    )?;
    let cleanup_manifest_pin = file_pin(&path)?;
    if for_rollback {
        let text =
            std::str::from_utf8(&state_before).map_err(|_| "the updater state does not read")?;
        let text = text
            .lines()
            .map(|line| {
                if line.starts_with("previous_dump=") {
                    "previous_dump="
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        replace_metadata(
            &root.directory(Path::new("runtime/update"))?.join("state"),
            text.as_bytes(),
        )?;
    }
    if let Some(copy) = copy {
        fs::remove_dir_all(copy)
            .map_err(|_| "the paired blob copy was only partly removed; review cleanup again")?;
    }
    if dump_pin.is_some() {
        root.dump(name)?;
        if file_pin(&dump)? != dump_pin {
            return Err("the dump changed; its cleanup checkpoint stays".into());
        }
        fs::remove_file(&dump)
            .map_err(|_| "the dump was not removed; its cleanup checkpoint stays")?;
    }
    root.dump(name)?;
    if file_pin(&path)? != cleanup_manifest_pin {
        return Err("the manifest changed; review cleanup again".into());
    }
    fs::remove_file(&path).map_err(|_| "the manifest was not removed; review cleanup again")?;
    println!(
        "{}: {name}",
        word(
            "Removed dump, manifest and paired blob copy",
            "已移除转储、清单及配对内容副本"
        )
    );
    Ok(())
}

fn word(english: &'static str, chinese: &'static str) -> &'static str {
    if ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
        .is_some_and(|value| value.starts_with("zh"))
    {
        chinese
    } else {
        english
    }
}

/// Runs only under the updater's local deployment lock; no HTTP endpoint invokes this helper.
pub fn run(options: &Options) -> Result<(), String> {
    let at = SystemClock.time();
    match &options.command {
        Action::Inventory { root, keep } => {
            let root = Root::open(root)?;
            let report = inventory(&root, *keep, at)?;
            let bytes = serde_json::to_vec(&report)
                .map_err(|_| "the storage metadata could not be encoded")?;
            if bytes.len() as u64 > MAX_METADATA_BYTES {
                return Err("the storage metadata exceeds its bounded publication size".into());
            }
            let directory = root.directory(Path::new("runtime/update"))?;
            backups::atomic(&directory.join("storage.json"), &bytes)
                .map_err(|error| error.to_string())
        }
        Action::DumpRemove { root, name, yes } => remove_dump(&Root::open(root)?, name, *yes),
        Action::Pair {
            root,
            name,
            directory,
        } => record_copy(&Root::open(root)?, name, directory, at),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;

    type Fixture = (tempfile::TempDir, Root, Time);

    fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join(BACKUPS))?;
        fs::create_dir_all(temp.path().join("runtime/update"))?;
        fs::create_dir_all(temp.path().join("runtime/data"))?;
        fs::create_dir_all(temp.path().join("config"))?;
        fs::write(
            temp.path().join("config/orch.toml"),
            "[retention]\nblob_grace_hours=24\n",
        )?;
        fs::write(
            temp.path().join("runtime/data/live"),
            b"keep the live store",
        )?;
        let root = Root {
            path: temp.path().to_owned(),
            identity: identity(temp.path())?,
            system: "isolated-system".into(),
        };
        let at = Time::new("2026-10-01T12:00:00Z".parse()?);
        let dump = root.path.join(BACKUPS).join("saved.dump");
        let content = b"isolated dump bytes";
        fs::write(&dump, content)?;
        fs::write(
            dump.with_extension("manifest.json"),
            serde_json::to_vec(
                &json!({"format_version":1,"system_id":root.system,"release":"v0.1.0","schema":"19","database_at":at,"completed_at":at,"cause":"manual","dump":{"name":"saved.dump","bytes":content.len(),"hash":blake3::hash(content).to_hex().to_string()},"included":{"database":true},"excluded":{"blobs":{"included":false}}}),
            )?,
        )?;
        Ok((temp, root, at))
    }

    fn copied(root: &Root) -> Result<PathBuf, std::io::Error> {
        let copy = root.path.join(BACKUPS).join("saved.data");
        fs::create_dir(&copy)?;
        fs::write(copy.join("immutable-blob"), b"matching copy")?;
        Ok(copy)
    }

    #[test]
    fn an_unrecorded_pair_keeps_every_file_and_never_offers_an_erase_command() {
        let (_temp, root, at) = fixture().unwrap();
        let copy = copied(&root).unwrap();
        assert!(remove_dump(&root, "saved.dump", true).is_err());
        assert!(root.dump("saved.dump").unwrap().0.is_file());
        assert!(copy.join("immutable-blob").is_file());
        let report = inventory(&root, Some(3), at).unwrap();
        let row = &report
            .items
            .iter()
            .find(|item| item.record.kind == StorageKind::Dump)
            .unwrap()
            .record;
        assert_eq!(row.blocker.as_deref(), Some("paired_copy_unrecorded"));
        assert_eq!(row.operator_command, None);
        assert_eq!(
            row.bytes, None,
            "an unrecorded copy cannot become a complete pair size"
        );
    }

    #[test]
    fn a_confirmed_pair_removes_only_its_recorded_copy_and_keeps_the_live_store_and_sibling() {
        let (_temp, root, at) = fixture().unwrap();
        let copy = copied(&root).unwrap();
        let sibling = root.path.join(BACKUPS).join("other.dump");
        fs::write(&sibling, b"keep another backup").unwrap();
        record_copy(&root, "saved.dump", "saved.data", at).unwrap();
        let report = inventory(&root, Some(3), at).unwrap();
        assert!(
            report
                .items
                .iter()
                .find(|item| item.record.name == "saved.dump")
                .unwrap()
                .record
                .operator_command
                .as_ref()
                .unwrap()
                .contains("dumps remove 'saved.dump'")
        );
        remove_dump(&root, "saved.dump", true).unwrap();
        assert!(!copy.exists());
        let (dump, manifest) = root.dump("saved.dump").unwrap();
        assert!(!dump.exists() && !manifest.exists());
        assert_eq!(fs::read(sibling).unwrap(), b"keep another backup");
        assert_eq!(
            fs::read(root.path.join("runtime/data/live")).unwrap(),
            b"keep the live store"
        );
    }

    #[test]
    fn replacing_the_paired_directory_never_adopts_its_new_data() {
        let (_temp, root, at) = fixture().unwrap();
        let copy = copied(&root).unwrap();
        record_copy(&root, "saved.dump", "saved.data", at).unwrap();
        fs::rename(&copy, root.path.join(BACKUPS).join("original.data")).unwrap();
        fs::create_dir(&copy).unwrap();
        fs::write(copy.join("unrelated"), b"do not erase").unwrap();
        assert!(remove_dump(&root, "saved.dump", true).is_err());
        assert_eq!(fs::read(copy.join("unrelated")).unwrap(), b"do not erase");
        assert!(root.dump("saved.dump").unwrap().0.is_file());
    }

    #[test]
    fn an_older_pair_receipt_cannot_backdate_a_different_or_replaced_copy() {
        let (_temp, root, at) = fixture().unwrap();
        let copy = copied(&root).unwrap();
        record_copy(&root, "saved.dump", "saved.data", at).unwrap();
        let later = Time::new(at.get() + chrono::Duration::days(2));
        record_copy(&root, "saved.dump", "saved.data", later).unwrap();
        let other = root.path.join(BACKUPS).join("new.data");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("blob"), b"new content").unwrap();
        assert!(record_copy(&root, "saved.dump", "new.data", later).is_err());
        fs::rename(&copy, root.path.join(BACKUPS).join("original.data")).unwrap();
        fs::create_dir(&copy).unwrap();
        fs::write(copy.join("blob"), b"replacement content").unwrap();
        assert!(record_copy(&root, "saved.dump", "saved.data", later).is_err());
        assert!(remove_dump(&root, "saved.dump", true).is_err());
        assert_eq!(fs::read(copy.join("blob")).unwrap(), b"replacement content");
    }

    #[test]
    fn a_live_store_path_and_a_link_cannot_be_registered_as_a_backup_copy() {
        let (_temp, root, at) = fixture().unwrap();
        assert!(record_copy(&root, "saved.dump", "../data", at).is_err());
        std::os::unix::fs::symlink(
            root.path.join("runtime/data"),
            root.path.join(BACKUPS).join("borrowed.data"),
        )
        .unwrap();
        assert!(record_copy(&root, "saved.dump", "borrowed.data", at).is_err());
        assert_eq!(
            fs::read(root.path.join("runtime/data/live")).unwrap(),
            b"keep the live store"
        );
    }

    #[test]
    fn an_unfinished_update_and_a_changed_dump_keep_the_whole_pair() {
        let (_temp, root, at) = fixture().unwrap();
        let copy = copied(&root).unwrap();
        record_copy(&root, "saved.dump", "saved.data", at).unwrap();
        fs::write(root.path.join("runtime/update/journal"), "step=restoring\n").unwrap();
        assert!(remove_dump(&root, "saved.dump", true).is_err());
        assert!(copy.is_dir());
        let (dump, manifest) = root.dump("saved.dump").unwrap();
        assert!(dump.is_file() && manifest.is_file());
        let (_other_temp, other, at) = fixture().unwrap();
        copied(&other).unwrap();
        record_copy(&other, "saved.dump", "saved.data", at).unwrap();
        fs::write(
            other.dump("saved.dump").unwrap().0,
            b"changed after the backup",
        )
        .unwrap();
        assert!(remove_dump(&other, "saved.dump", true).is_err());
        assert!(other.path.join(BACKUPS).join("saved.data").is_dir());
    }
}
