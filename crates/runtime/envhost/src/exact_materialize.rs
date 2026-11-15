//! Verified saved files are published once into an unused directory without merging later names.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use orch_link_contract::commands::{
    DirectoryCopyOutcome, FsDirectoryCopyResult, FsMaterialize, FsMaterializeIfAbsent,
    FsMaterializeResult,
};
use orch_types::Hash;
use serde::{Deserialize, Serialize};

use crate::blobs::{BlobSource, FileWriter, download_checked};
use crate::error::{HostError, Result};
use crate::file_states::FileStates;
use crate::files::Files;
use crate::manifest::{EntryKind, ManifestEntry, Parsed};
use crate::paths::{Locate, RelPath};
use crate::platform::{self, Dir};
use crate::state_store::{atomic_write, read_bounded};

/// Saved journals use the same expanded bound as the manifest whose paths they retain.
const JOURNAL_BOUND: u64 = crate::manifest::MANIFEST_MAX_EXPANDED_BYTES;
/// A stage is private until its one no-replace publication into the workspace.
const STAGE_MODE: u32 = 0o700;
/// Only executable bits travel with recorded file contents; ordinary file permissions use the caller's umask.
const FILE_MODE: u32 = 0o666;
const EXECUTABLE_BITS: u32 = 0o111;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    request: Hash,
    generation: i64,
    parent_device: u64,
    parent_inode: u64,
    ignore_fingerprint: Hash,
    stage: Option<Stage>,
    result: Option<FsDirectoryCopyResult>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
    name: String,
    device: u64,
    inode: u64,
}

fn conditional_request(request: &FsMaterializeIfAbsent) -> FsMaterialize {
    FsMaterialize {
        work: request.work.clone(),
        environment_id: request.environment_id,
        job_id: Some(request.job_id),
        manifest_blob: request.manifest_blob,
        target_root: request.target_root.clone(),
        purpose: orch_link_contract::commands::MaterializePurpose::Exact,
    }
}

fn journal_path(
    states: &FileStates,
    request: &FsMaterialize,
    conditional: bool,
) -> Result<std::path::PathBuf> {
    let job = request
        .job_id
        .ok_or_else(|| HostError::Unsupported("exact saved files require a durable job".into()))?;
    Ok(if conditional {
        states
            .store()
            .root()
            .join("conditional-materialize-jobs")
            .join(format!(
                "{job}-{}.json",
                Hash::of(request.target_root.as_bytes())
            ))
    } else {
        states
            .store()
            .root()
            .join("materialize-jobs")
            .join(format!("{job}.json"))
    })
}

fn save(path: &Path, journal: &Journal) -> Result<()> {
    atomic_write(
        path,
        &serde_json::to_vec(journal).map_err(|error| HostError::Internal(error.to_string()))?,
    )
}

/// A completed native effect returns its first counts without downloading expired content again.
pub(crate) fn completed(
    states: &FileStates,
    request: &FsMaterialize,
    generation: i64,
) -> Result<Option<FsMaterializeResult>> {
    Ok(completed_inner(states, request, generation, false)?.map(|result| result.counts))
}

pub(crate) fn completed_if_absent(
    states: &FileStates,
    request: &FsMaterializeIfAbsent,
    generation: i64,
) -> Result<Option<FsDirectoryCopyResult>> {
    completed_inner(states, &conditional_request(request), generation, true)
}

fn completed_inner(
    states: &FileStates,
    request: &FsMaterialize,
    generation: i64,
    conditional: bool,
) -> Result<Option<FsDirectoryCopyResult>> {
    let path = journal_path(states, request, conditional)?;
    if !path.exists() {
        return Ok(None);
    }
    let journal: Journal = serde_json::from_slice(&read_bounded(&path, JOURNAL_BOUND)?)
        .map_err(|_| HostError::Unsupported("the exact-file journal is unavailable".into()))?;
    let hash = Hash::of(
        &serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?,
    );
    if journal.request != hash || journal.generation != generation {
        return Err(HostError::Changed(
            "the exact-file job changed its choice or generation".into(),
        ));
    }
    Ok(journal.result)
}

pub(crate) fn same(entry: &ManifestEntry, parent: &Dir, name: &OsStr) -> Result<bool> {
    let meta = parent
        .lstat(name)
        .map_err(|error| HostError::io("checking an exact saved-file entry", error))?;
    Ok(match entry.t {
        EntryKind::Dir => meta.is_dir(),
        EntryKind::Link if meta.is_symlink() => {
            parent
                .read_link(name)
                .map_err(|error| HostError::io("checking saved link text", error))?
                .to_str()
                == entry.l.as_deref()
        }
        EntryKind::File if meta.is_file() => {
            let file = parent
                .open_read(name)
                .map_err(|error| HostError::io("checking saved file bytes", error))?;
            Some(
                crate::blobs::hash_file(&file)
                    .map_err(|error| HostError::io("checking saved file bytes", error))?,
            ) == entry.h
                && meta.mode & EXECUTABLE_BITS == entry.x & EXECUTABLE_BITS
        }
        _ => false,
    })
}

pub(crate) fn inventory(
    directory: &Dir,
    prefix: &str,
    wanted: &BTreeMap<String, ManifestEntry>,
    complete: bool,
) -> Result<usize> {
    let mut count = 0;
    for name in directory
        .names()
        .map_err(|error| HostError::io("checking the entire exact-file stage", error))?
    {
        let name_text = name.to_str().ok_or_else(|| {
            HostError::Changed("an unexpected non-UTF-8 name appeared in the stage".into())
        })?;
        let path = if prefix.is_empty() {
            name_text.to_owned()
        } else {
            format!("{prefix}/{name_text}")
        };
        let entry = wanted.get(&path).ok_or_else(|| {
            HostError::Changed("another writer added a name to the exact-file stage".into())
        })?;
        if !same(entry, directory, &name)? {
            return Err(HostError::Changed(
                "another writer changed an exact-file stage entry".into(),
            ));
        }
        count += 1;
        if entry.t == EntryKind::Dir {
            count += inventory(
                &directory
                    .open_child(&name)
                    .map_err(|error| HostError::io("holding a staged directory", error))?,
                &path,
                wanted,
                false,
            )?;
        }
    }
    if complete && count != wanted.len() {
        return Err(HostError::Changed(
            "the exact-file stage no longer has its complete namespace".into(),
        ));
    }
    Ok(count)
}

/// The ordinary exact publisher keeps its existing count-only result.
pub async fn apply(
    files: &Files,
    states: &FileStates,
    request: &FsMaterialize,
    parsed: &Parsed,
    source: &dyn BlobSource,
    generation: i64,
) -> Result<FsMaterializeResult> {
    Ok(apply_inner(
        files,
        states,
        request,
        parsed,
        source,
        generation,
        false,
        &orch_types::SystemClock,
    )
    .await?
    .counts)
}

/// The same verified staging and publication, with a durable positive keep decision.
pub async fn apply_if_absent(
    files: &Files,
    states: &FileStates,
    request: &FsMaterializeIfAbsent,
    parsed: &Parsed,
    source: &dyn BlobSource,
    generation: i64,
    clock: &dyn orch_types::Clock,
) -> Result<FsDirectoryCopyResult> {
    apply_inner(
        files,
        states,
        &conditional_request(request),
        parsed,
        source,
        generation,
        true,
        clock,
    )
    .await
}

fn keep_directory(
    parent: &Dir,
    journal_path: &Path,
    journal: &mut Journal,
    wanted: &BTreeMap<String, ManifestEntry>,
    clock: &dyn orch_types::Clock,
) -> Result<FsDirectoryCopyResult> {
    let at = clock.time();
    let cleanup_pending =
        journal
            .stage
            .as_ref()
            .is_some_and(|stage| match parent.lstat(OsStr::new(&stage.name)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Ok(meta)
                    if meta.is_dir() && (meta.dev, meta.ino) == (stage.device, stage.inode) =>
                {
                    let safe = parent
                        .open_child(OsStr::new(&stage.name))
                        .ok()
                        .is_some_and(|held| inventory(&held, "", wanted, false).is_ok());
                    !safe
                        || crate::files::remove_tree(parent, OsStr::new(&stage.name), 0)
                            .and_then(|()| {
                                parent.sync().map_err(|error| {
                                    HostError::io("syncing conditional copy cleanup", error)
                                })
                            })
                            .is_err()
                }
                _ => true,
            });
    let result = FsDirectoryCopyResult {
        outcome: DirectoryCopyOutcome::TargetKept,
        counts: FsMaterializeResult {
            files: 0,
            bytes: 0,
            skipped: 0,
        },
        at,
        cleanup_pending,
    };
    journal.result = Some(result.clone());
    save(journal_path, journal)?;
    Ok(result)
}

fn target_exists(parent: &Dir, name: &str) -> Result<bool> {
    match parent.lstat(OsStr::new(name)) {
        Ok(meta) if meta.is_dir() => Ok(true),
        Ok(_) => Err(HostError::Changed(
            "the destination is not a directory; its object is kept".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(HostError::io("checking the conditional directory", error)),
    }
}

/// Every object is verified before publication; the target is never merged or replaced.
#[allow(
    clippy::too_many_arguments,
    reason = "one exact publisher receives the conditional outcome rule and its native clock"
)]
async fn apply_inner(
    files: &Files,
    states: &FileStates,
    request: &FsMaterialize,
    parsed: &Parsed,
    source: &dyn BlobSource,
    generation: i64,
    conditional: bool,
    clock: &dyn orch_types::Clock,
) -> Result<FsDirectoryCopyResult> {
    if !conditional {
        files.gates().require_remote_write()?;
    }
    let _held = states.action_gate().await;
    crate::error::check_cancelled()?;
    let job = request
        .job_id
        .ok_or_else(|| HostError::Unsupported("exact saved files require a durable job".into()))?;
    let directory = files.target_of(&request.target_root)?;
    if directory.is_root() {
        return Err(HostError::Changed(
            "an exact saved point needs an unused child directory".into(),
        ));
    }
    let parent_path = directory.parent().unwrap_or_else(RelPath::root);
    let located = match files.root().locate(&parent_path, Locate::THROUGH) {
        Ok(located) => located,
        Err(HostError::NotFound(_)) if conditional => {
            files.gates().require_remote_write()?;
            files.make_dir(&parent_path)?;
            files.root().locate(&parent_path, Locate::THROUGH)?
        }
        Err(error) => return Err(error),
    };
    let located = if conditional && located.meta.is_none() && !parent_path.is_root() {
        files.gates().require_remote_write()?;
        files.make_dir(&parent_path)?;
        files.root().locate(&parent_path, Locate::THROUGH)?
    } else {
        located
    };
    files.check_visible(&parent_path, &located, true)?;
    if located.real != parent_path {
        return Err(HostError::Changed(
            "the exact-file parent changed through a link".into(),
        ));
    }
    let parent = files.root().open_dir(&located)?;
    let parent_meta = parent
        .stat_self()
        .map_err(|error| HostError::io("holding the exact-file parent", error))?;
    let name = directory
        .file_name()
        .ok_or_else(|| HostError::Changed("the exact-file destination has no name".into()))?;
    let request_hash = Hash::of(
        &serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?,
    );
    let rules = crate::file_states::GitRules::read(files, &[], &directory)?;
    let journal_path = journal_path(states, request, conditional)?;
    let mut wanted = BTreeMap::new();
    let mut named = std::collections::BTreeSet::new();
    for entry in &parsed.entries {
        let path = files.parse(&entry.p)?;
        if path.is_root()
            || path.has_component(".git")
            || entry.t == EntryKind::Other
            || entry.t == EntryKind::File && entry.h.is_none()
            || entry.t == EntryKind::Link && entry.l.is_none()
        {
            return Err(HostError::Unsupported(
                "the exact saved point has incomplete or repository-metadata coverage".into(),
            ));
        }
        let final_path = directory.append(&path)?;
        // Coverage is checked at its advertised destination, not at the private temporary name.
        let visible = crate::paths::Located {
            parent: parent
                .try_clone()
                .map_err(|error| HostError::io("checking exact-file coverage", error))?,
            name: Some(OsStr::new(name).to_owned()),
            real: final_path.clone(),
            meta: None,
        };
        files.check_visible(&final_path, &visible, entry.t == EntryKind::Dir)?;
        if rules.ignored(&final_path, entry.t == EntryKind::Dir) {
            return Err(HostError::Ignored(
                "current rules hide part of the exact saved coverage".into(),
            ));
        }
        if !named.insert(path.to_string()) {
            return Err(HostError::Changed(
                "the exact saved point repeats a name".into(),
            ));
        }
        wanted.insert(path.to_string(), entry.clone());
        let mut ancestor = path.parent();
        while let Some(parent) = ancestor.filter(|parent| !parent.is_root()) {
            wanted
                .entry(parent.to_string())
                .or_insert_with(|| ManifestEntry {
                    p: parent.to_string(),
                    t: EntryKind::Dir,
                    s: 0,
                    m: 0,
                    x: STAGE_MODE,
                    h: None,
                    l: None,
                });
            ancestor = parent.parent();
        }
    }
    let mut journal = if journal_path.exists() {
        let journal: Journal = serde_json::from_slice(&read_bounded(&journal_path, JOURNAL_BOUND)?)
            .map_err(|_| HostError::Unsupported("the exact-file journal is unavailable".into()))?;
        if journal.request != request_hash
            || journal.generation != generation
            || journal.ignore_fingerprint != rules.fingerprint
            || (journal.parent_device, journal.parent_inode) != (parent_meta.dev, parent_meta.ino)
        {
            return Err(HostError::Changed(
                "the exact-file plan, generation or parent changed".into(),
            ));
        }
        if let Some(result) = &journal.result {
            return Ok(result.clone());
        }
        journal
    } else {
        if conditional && target_exists(&parent, name)? {
            let mut kept = Journal {
                request: request_hash,
                generation,
                parent_device: parent_meta.dev,
                parent_inode: parent_meta.ino,
                ignore_fingerprint: rules.fingerprint,
                stage: None,
                result: None,
            };
            return keep_directory(&parent, &journal_path, &mut kept, &wanted, clock);
        }
        files.gates().require_remote_write()?;
        match parent.lstat(OsStr::new(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(HostError::Changed(
                    "the exact-file directory already exists; its files are kept".into(),
                ));
            }
            Err(error) => {
                return Err(HostError::io(
                    "checking the unused exact-file directory",
                    error,
                ));
            }
        }
        let stage = if conditional {
            format!(
                ".orch-tmp-exact-{job}-{}",
                Hash::of(request.target_root.as_bytes())
            )
        } else {
            format!(".orch-tmp-exact-{job}")
        };
        parent
            .mkdir(OsStr::new(&stage), STAGE_MODE)
            .map_err(|error| HostError::io("making the exact-file stage", error))?;
        let meta = parent
            .lstat(OsStr::new(&stage))
            .map_err(|error| HostError::io("holding the exact-file stage", error))?;
        let journal = Journal {
            request: request_hash,
            generation,
            parent_device: parent_meta.dev,
            parent_inode: parent_meta.ino,
            ignore_fingerprint: rules.fingerprint,
            stage: Some(Stage {
                name: stage,
                device: meta.dev,
                inode: meta.ino,
            }),
            result: None,
        };
        save(&journal_path, &journal)?;
        journal
    };
    let own_publication = journal.stage.as_ref().is_some_and(|stage| {
        parent
            .lstat(OsStr::new(name))
            .is_ok_and(|meta| meta.is_dir() && (meta.dev, meta.ino) == (stage.device, stage.inode))
    });
    if conditional && target_exists(&parent, name)? && !own_publication {
        return keep_directory(&parent, &journal_path, &mut journal, &wanted, clock);
    }
    if !conditional || !own_publication {
        files.gates().require_remote_write()?;
    }
    let stage = journal
        .stage
        .clone()
        .ok_or_else(|| HostError::Changed("the owned exact-file stage is unavailable".into()))?;
    let staged = parent.lstat(OsStr::new(&stage.name));
    let already_published = match staged {
        Ok(meta) if (meta.dev, meta.ino) == (stage.device, stage.inode) && meta.is_dir() => false,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && parent.lstat(OsStr::new(name)).is_ok_and(|meta| {
                    (meta.dev, meta.ino) == (stage.device, stage.inode) && meta.is_dir()
                }) =>
        {
            true
        }
        _ => {
            return Err(HostError::Changed(
                "the owned exact-file stage changed or disappeared".into(),
            ));
        }
    };
    let current = parent
        .open_child(OsStr::new(if already_published {
            name
        } else {
            &stage.name
        }))
        .map_err(|error| HostError::io("opening the exact-file stage", error))?;
    inventory(&current, "", &wanted, already_published)?;
    if !already_published {
        let mut verified = BTreeMap::new();
        for entry in wanted.values().filter(|entry| entry.t == EntryKind::File) {
            let temporary = tempfile::NamedTempFile::new_in(states.store().root())
                .map_err(|error| HostError::io("receiving exact saved bytes", error))?;
            let file = Arc::new(
                temporary
                    .reopen()
                    .map_err(|error| HostError::io("receiving exact saved bytes", error))?,
            );
            let mut writer = FileWriter::new(file);
            let hash = entry
                .h
                .ok_or_else(|| HostError::Unsupported("saved file content is missing".into()))?;
            let size = download_checked(source, hash, &mut writer)
                .await
                .map_err(|error| HostError::Content(error.to_string()))?;
            if size != entry.s {
                return Err(HostError::Changed(
                    "saved file size differs from its manifest".into(),
                ));
            }
            verified.insert(entry.p.clone(), temporary);
        }
        if conditional && target_exists(&parent, name)? {
            return keep_directory(&parent, &journal_path, &mut journal, &wanted, clock);
        }
        inventory(&current, "", &wanted, false)?;
        if crate::file_states::GitRules::read(files, &[], &directory)?.fingerprint
            != journal.ignore_fingerprint
        {
            return Err(HostError::Changed(
                "ignore coverage changed while exact bytes were received".into(),
            ));
        }
        for entry in wanted.values() {
            let mut held = current
                .try_clone()
                .map_err(|error| HostError::io("holding an exact-file directory", error))?;
            let mut components = entry.p.split('/').collect::<Vec<_>>();
            let entry_name = components
                .pop()
                .ok_or_else(|| HostError::Changed("a saved path has no name".into()))?;
            for component in components {
                held = held
                    .open_child(OsStr::new(component))
                    .map_err(|error| HostError::io("opening a saved parent directory", error))?;
            }
            match held.lstat(OsStr::new(entry_name)) {
                Ok(_) if same(entry, &held, OsStr::new(entry_name))? => continue,
                Ok(_) => {
                    return Err(HostError::Changed(
                        "another writer changed the owned saved-file stage".into(),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(HostError::io("checking a saved path", error)),
            }
            match entry.t {
                EntryKind::Dir => held
                    .mkdir(OsStr::new(entry_name), STAGE_MODE)
                    .map_err(|error| HostError::io("making a saved directory", error))?,
                EntryKind::Link => held
                    .symlink(
                        OsStr::new(entry.l.as_deref().ok_or_else(|| {
                            HostError::Unsupported("saved link text is missing".into())
                        })?),
                        OsStr::new(entry_name),
                    )
                    .map_err(|error| HostError::io("making a saved link", error))?,
                EntryKind::File => {
                    let raw = verified.get(&entry.p).ok_or_else(|| {
                        HostError::Unsupported("verified saved bytes are missing".into())
                    })?;
                    let mut input = raw
                        .reopen()
                        .map_err(|error| HostError::io("reading verified saved bytes", error))?;
                    let mut output = held
                        .create_new(
                            OsStr::new(entry_name),
                            FILE_MODE | (entry.x & EXECUTABLE_BITS),
                        )
                        .map_err(|error| {
                            HostError::io("writing saved bytes without replacing a name", error)
                        })?;
                    std::io::copy(&mut input, &mut output)
                        .and_then(|_| output.sync_all())
                        .map_err(|error| HostError::io("writing saved bytes", error))?;
                    let mode = platform::file_meta(&output)
                        .map_err(|error| HostError::io("checking saved file permissions", error))?
                        .mode;
                    platform::set_file_mode(
                        &output,
                        (mode & !EXECUTABLE_BITS) | (entry.x & EXECUTABLE_BITS),
                    )
                    .map_err(|error| HostError::io("keeping saved executable bits", error))?;
                }
                EntryKind::Other => {
                    return Err(HostError::Unsupported(
                        "special files are not exact saved files".into(),
                    ));
                }
            }
        }
        inventory(&current, "", &wanted, true)?;
        files.gates().require_remote_write()?;
        if crate::file_states::GitRules::read(files, &[], &directory)?.fingerprint
            != journal.ignore_fingerprint
        {
            return Err(HostError::Changed(
                "ignore coverage changed before exact-file publication".into(),
            ));
        }
        let current_parent = files.root().locate(&parent_path, Locate::THROUGH)?;
        let checked = files
            .root()
            .open_dir(&current_parent)?
            .stat_self()
            .map_err(|error| HostError::io("rechecking the exact-file parent", error))?;
        if current_parent.real != parent_path
            || (checked.dev, checked.ino) != (journal.parent_device, journal.parent_inode)
        {
            return Err(HostError::Changed(
                "the exact-file parent was recreated".into(),
            ));
        }
        crate::error::check_cancelled()?;
        if let Err(error) =
            parent.rename_to(OsStr::new(&stage.name), &parent, OsStr::new(name), false)
        {
            if conditional && target_exists(&parent, name)? {
                return keep_directory(&parent, &journal_path, &mut journal, &wanted, clock);
            }
            return Err(HostError::io(
                "publishing exact files only into the still-unused directory",
                error,
            ));
        }
        parent
            .sync()
            .map_err(|error| HostError::io("syncing the published exact directory", error))?;
    }
    let bytes = parsed
        .entries
        .iter()
        .try_fold(0i64, |total, entry| total.checked_add(entry.s))
        .ok_or_else(|| HostError::Unsupported("the saved byte total is too large".into()))?;
    let result = FsMaterializeResult {
        files: i64::try_from(
            parsed
                .entries
                .iter()
                .filter(|entry| entry.t == EntryKind::File)
                .count(),
        )
        .map_err(|_| HostError::Unsupported("too many saved files".into()))?,
        bytes,
        skipped: 0,
    };
    let result = FsDirectoryCopyResult {
        outcome: DirectoryCopyOutcome::Copied,
        counts: result,
        at: clock.time(),
        cleanup_pending: false,
    };
    journal.result = Some(result.clone());
    save(&journal_path, &journal)?;
    states.invalidate("not_ready");
    Ok(result)
}

#[cfg(all(test, unix))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod tests {
    use super::*;
    use crate::blobs::{BlobError, BlobWriter, MemoryBlobs};
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::manifest::ManifestHeader;
    use crate::paths::{PathStyle, Workspace};
    use crate::state_store::ObjectStore;
    use orch_link_contract::commands::MaterializePurpose;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(
        ignored: &[String],
    ) -> (
        tempfile::TempDir,
        Files,
        FileStates,
        MemoryBlobs,
        Parsed,
        FsMaterialize,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let files = Files::workspace(
            Workspace::open(&workspace, &[], PathStyle::Posix, true).unwrap(),
            Arc::new(IgnoreRules::new(ignored, true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let states =
            FileStates::new(ObjectStore::open(&temp.path().join("kept"), &workspace).unwrap());
        let blobs = MemoryBlobs::new();
        let hash = blobs.insert(vec![0, 255, 17]);
        let parsed = Parsed {
            header: ManifestHeader {
                v: crate::manifest::MANIFEST_VERSION,
                root: "/old/workspace/repo".into(),
                generated_at: "2026-10-14T10:00:00.000Z".parse().unwrap(),
                generation: 1,
                case_sensitive: true,
            },
            entries: vec![
                ManifestEntry {
                    p: "binary".into(),
                    t: EntryKind::File,
                    s: 3,
                    m: 0,
                    x: 0o755,
                    h: Some(hash),
                    l: None,
                },
                ManifestEntry {
                    p: "empty".into(),
                    t: EntryKind::Dir,
                    s: 0,
                    m: 0,
                    x: 0o755,
                    h: None,
                    l: None,
                },
                ManifestEntry {
                    p: "link".into(),
                    t: EntryKind::Link,
                    s: 0,
                    m: 0,
                    x: 0o777,
                    h: None,
                    l: Some("binary".into()),
                },
            ],
        };
        let manifest = crate::manifest::build(&parsed.header, parsed.entries.clone()).unwrap();
        let request = FsMaterialize {
            work: None,
            environment_id: orch_types::new_id(),
            job_id: Some(orch_types::new_id()),
            manifest_blob: manifest.hash,
            target_root: "exact".into(),
            purpose: MaterializePurpose::Exact,
        };
        (temp, files, states, blobs, parsed, request)
    }

    fn conditional(request: &FsMaterialize) -> orch_link_contract::commands::FsMaterializeIfAbsent {
        orch_link_contract::commands::FsMaterializeIfAbsent {
            work: None,
            environment_id: request.environment_id,
            job_id: request.job_id.unwrap(),
            manifest_blob: request.manifest_blob,
            target_root: request.target_root.clone(),
        }
    }
    struct NeverRead;
    #[async_trait::async_trait]
    impl BlobSource for NeverRead {
        async fn get(
            &self,
            _: Hash,
            _: &mut dyn BlobWriter,
        ) -> std::result::Result<i64, BlobError> {
            panic!("a retained conditional outcome must not read or replace source bytes")
        }
    }

    #[tokio::test]
    async fn an_existing_note_directory_is_kept_without_write_authority_and_the_first_outcome_survives_replay()
     {
        let (_temp, files, states, _blobs, parsed, request) = fixture(&[]);
        let target = files.root().root_path().join("exact");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("mine"), b"existing notes").unwrap();
        files.gates().set(Gates {
            remote_write: false,
            ..Gates::OPEN
        });
        let clock = orch_types::ManualClock::new("2026-11-15T10:00:00Z".parse().unwrap());
        let request = conditional(&request);
        let kept = apply_if_absent(&files, &states, &request, &parsed, &NeverRead, 2, &clock)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&kept).unwrap()["outcome"],
            "target_kept"
        );
        assert!(!kept.cleanup_pending);
        assert_eq!(
            std::fs::read(target.join("mine")).unwrap(),
            b"existing notes"
        );
        assert!(!target.join("binary").exists());
        std::fs::remove_dir_all(&target).unwrap();
        clock.advance(chrono::Duration::hours(1));
        assert_eq!(
            apply_if_absent(&files, &states, &request, &parsed, &NeverRead, 2, &clock)
                .await
                .unwrap(),
            kept
        );
        assert!(
            !target.exists(),
            "replay does not invent a later copy after the recorded keep"
        );
    }

    #[tokio::test]
    async fn a_note_directory_created_during_the_native_copy_wins_and_the_unchanged_private_stage_is_cleaned()
     {
        let (_temp, files, states, blobs, parsed, request) = fixture(&[]);
        let target = files.root().root_path().join("exact");
        let racing = Racing {
            blobs,
            target: target.clone(),
        };
        let clock = orch_types::ManualClock::new("2026-11-15T10:00:00Z".parse().unwrap());
        let result = apply_if_absent(
            &files,
            &states,
            &conditional(&request),
            &parsed,
            &racing,
            2,
            &clock,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(&result).unwrap()["outcome"],
            "target_kept"
        );
        assert_eq!(
            std::fs::read(target.join("later")).unwrap(),
            b"another writer"
        );
        assert!(!target.join("binary").exists());
        assert!(!result.cleanup_pending);
        assert!(
            std::fs::read_dir(files.root().root_path())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".orch-tmp-exact-"))
        );
    }

    #[tokio::test]
    async fn a_copied_note_directory_replays_its_first_receipt_without_overwriting_later_notes() {
        let (_temp, files, states, blobs, parsed, request) = fixture(&[]);
        let clock = orch_types::ManualClock::new("2026-11-15T10:00:00Z".parse().unwrap());
        let request = conditional(&request);
        let copied = apply_if_absent(&files, &states, &request, &parsed, &blobs, 2, &clock)
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(&copied).unwrap()["outcome"], "copied");
        assert_eq!(copied.counts.files, 1);
        let path = files.root().root_path().join("exact/binary");
        std::fs::write(&path, b"later notes").unwrap();
        clock.advance(chrono::Duration::hours(1));
        assert_eq!(
            apply_if_absent(&files, &states, &request, &parsed, &NeverRead, 2, &clock)
                .await
                .unwrap(),
            copied
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"later notes");
    }

    #[tokio::test]
    async fn a_conditional_publication_with_a_lost_answer_recovers_as_copied_instead_of_claiming_an_existing_target()
     {
        let (_temp, files, states, blobs, parsed, original) = fixture(&[]);
        let request = conditional(&original);
        let clock = orch_types::ManualClock::new("2026-11-15T10:00:00Z".parse().unwrap());
        apply_if_absent(&files, &states, &request, &parsed, &blobs, 2, &clock)
            .await
            .unwrap();
        let path = journal_path(&states, &conditional_request(&request), true).unwrap();
        let mut journal: Journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        journal.result = None;
        save(&path, &journal).unwrap();
        let recovered = apply_if_absent(&files, &states, &request, &parsed, &NeverRead, 2, &clock)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(recovered).unwrap()["outcome"],
            "copied"
        );
        assert_eq!(
            std::fs::read(files.root().root_path().join("exact/binary")).unwrap(),
            [0, 255, 17]
        );
    }

    struct ChangedPrivateStage {
        racing: Racing,
        parent: std::path::PathBuf,
    }
    #[async_trait::async_trait]
    impl BlobSource for ChangedPrivateStage {
        async fn get(
            &self,
            hash: Hash,
            writer: &mut dyn BlobWriter,
        ) -> std::result::Result<i64, BlobError> {
            let stage = std::fs::read_dir(&self.parent)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".orch-tmp-exact-")
                })
                .unwrap();
            std::fs::write(stage.join("authored"), b"later work").unwrap();
            self.racing.get(hash, writer).await
        }
    }
    #[tokio::test]
    async fn a_racing_target_and_changed_private_stage_preserve_both_and_report_unfinished_cleanup()
    {
        let (_temp, files, states, blobs, parsed, request) = fixture(&[]);
        let parent = files.root().root_path().to_path_buf();
        let source = ChangedPrivateStage {
            racing: Racing {
                blobs,
                target: parent.join("exact"),
            },
            parent: parent.clone(),
        };
        let clock = orch_types::ManualClock::new("2026-11-15T10:00:00Z".parse().unwrap());
        let kept = apply_if_absent(
            &files,
            &states,
            &conditional(&request),
            &parsed,
            &source,
            2,
            &clock,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(&kept).unwrap()["outcome"],
            "target_kept"
        );
        assert!(kept.cleanup_pending);
        let stage = std::fs::read_dir(&parent)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".orch-tmp-exact-")
            })
            .unwrap();
        assert_eq!(
            std::fs::read(stage.join("authored")).unwrap(),
            b"later work"
        );
        assert_eq!(
            std::fs::read(parent.join("exact/later")).unwrap(),
            b"another writer"
        );
    }

    #[tokio::test]
    async fn notes_only_copy_creates_missing_parents_only_when_native_writes_are_allowed() {
        let (_temp, files, states, blobs, parsed, original) = fixture(&[]);
        let mut request = conditional(&original);
        request.target_root = "fresh/.orch/memory".into();
        let clock = orch_types::ManualClock::new("2026-11-15T10:00:00Z".parse().unwrap());
        files.gates().set(Gates {
            remote_write: false,
            ..Gates::OPEN
        });
        assert!(matches!(
            apply_if_absent(&files, &states, &request, &parsed, &blobs, 2, &clock).await,
            Err(HostError::RemoteWriteDisabled)
        ));
        assert!(!files.root().root_path().join("fresh").exists());
        files.gates().set(Gates::OPEN);
        let made = apply_if_absent(&files, &states, &request, &parsed, &blobs, 2, &clock)
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(made).unwrap()["outcome"], "copied");
        assert_eq!(
            std::fs::read(files.root().root_path().join("fresh/.orch/memory/binary")).unwrap(),
            [0, 255, 17]
        );
    }

    #[tokio::test]
    async fn exact_files_keep_raw_bytes_links_empty_directories_and_the_first_result_without_touching_git()
     {
        let (_temp, files, states, blobs, parsed, request) = fixture(&[]);
        let root = files.root().root_path();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/index"), "staged original bytes").unwrap();
        let result = apply(&files, &states, &request, &parsed, &blobs, 2)
            .await
            .unwrap();
        assert_eq!(result.files, 1);
        assert_eq!(result.bytes, 3);
        assert_eq!(
            std::fs::read(root.join("exact/binary")).unwrap(),
            [0, 255, 17]
        );
        assert_eq!(
            std::fs::read_link(root.join("exact/link")).unwrap(),
            Path::new("binary")
        );
        assert!(root.join("exact/empty").is_dir());
        assert!(!root.join("exact/.git").exists());
        assert_ne!(
            std::fs::metadata(root.join("exact/binary"))
                .unwrap()
                .permissions()
                .mode()
                & EXECUTABLE_BITS,
            0
        );
        std::fs::write(root.join("exact/binary"), "later work").unwrap();
        assert_eq!(
            completed(&states, &request, 2).unwrap(),
            Some(result),
            "a completed copy needs no server blob to report its original outcome"
        );
        assert!(
            completed(&states, &request, 3).is_err(),
            "a different unit cannot inherit the old result"
        );
        assert_eq!(
            apply(&files, &states, &request, &parsed, &blobs, 2)
                .await
                .unwrap(),
            result
        );
        assert_eq!(
            std::fs::read(root.join("exact/binary")).unwrap(),
            b"later work"
        );
        assert_eq!(
            std::fs::read(root.join(".git/index")).unwrap(),
            b"staged original bytes"
        );
    }

    #[tokio::test]
    async fn a_lost_rename_answer_recovers_the_exact_owned_directory_once() {
        let (_temp, files, states, blobs, parsed, request) = fixture(&[]);
        let expected = apply(&files, &states, &request, &parsed, &blobs, 2)
            .await
            .unwrap();
        let path = states
            .store()
            .root()
            .join("materialize-jobs")
            .join(format!("{}.json", request.job_id.unwrap()));
        let mut journal: Journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        journal.result = None;
        save(&path, &journal).unwrap();
        assert_eq!(
            apply(&files, &states, &request, &parsed, &blobs, 2)
                .await
                .unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn exact_saved_files_include_nested_directories_and_their_raw_contents() {
        let (_temp, files, states, blobs, mut parsed, mut request) = fixture(&[]);
        let raw = blobs.insert(b"nested raw bytes\n".to_vec());
        parsed.entries.extend([
            ManifestEntry {
                p: "src".into(),
                t: EntryKind::Dir,
                s: 0,
                m: 0,
                x: 0o755,
                h: None,
                l: None,
            },
            ManifestEntry {
                p: "src/deep".into(),
                t: EntryKind::Dir,
                s: 0,
                m: 0,
                x: 0o755,
                h: None,
                l: None,
            },
            ManifestEntry {
                p: "src/deep/code.rs".into(),
                t: EntryKind::File,
                s: 17,
                m: 0,
                x: 0o644,
                h: Some(raw),
                l: None,
            },
        ]);
        let manifest = crate::manifest::build(&parsed.header, parsed.entries).unwrap();
        let parsed = crate::manifest::parse(&manifest.bytes).unwrap();
        request.manifest_blob = manifest.hash;
        let result = apply(&files, &states, &request, &parsed, &blobs, 2)
            .await
            .unwrap();
        assert_eq!(result.files, 2);
        assert_eq!(
            std::fs::read(files.root().root_path().join("exact/src/deep/code.rs")).unwrap(),
            b"nested raw bytes\n"
        );
        assert_eq!(
            std::fs::read(files.root().root_path().join("exact/binary")).unwrap(),
            [0, 255, 17]
        );
    }

    struct Racing {
        blobs: MemoryBlobs,
        target: std::path::PathBuf,
    }
    #[async_trait::async_trait]
    impl BlobSource for Racing {
        async fn get(
            &self,
            hash: Hash,
            writer: &mut dyn BlobWriter,
        ) -> std::result::Result<i64, BlobError> {
            std::fs::create_dir_all(&self.target).unwrap();
            std::fs::write(self.target.join("later"), "another writer").unwrap();
            self.blobs.get(hash, writer).await
        }
    }

    #[tokio::test]
    async fn an_existing_or_racing_directory_and_new_ignore_coverage_keep_every_later_file() {
        let (_temp, files, states, blobs, parsed, request) = fixture(&[]);
        let target = files.root().root_path().join("exact");
        let racing = Racing {
            blobs,
            target: target.clone(),
        };
        assert!(
            apply(&files, &states, &request, &parsed, &racing, 2)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(target.join("later")).unwrap(),
            b"another writer"
        );
        assert!(!target.join("binary").exists());
        let other = FsMaterialize {
            work: None,
            job_id: Some(orch_types::new_id()),
            ..request
        };
        assert!(
            apply(&files, &states, &other, &parsed, &racing, 2)
                .await
                .is_err()
        );
        let (_temp, files, states, blobs, parsed, request) = fixture(&["exact/binary".into()]);
        assert!(matches!(
            apply(&files, &states, &request, &parsed, &blobs, 2).await,
            Err(HostError::Ignored(_))
        ));
        assert!(!files.root().root_path().join("exact").exists());
    }
}
