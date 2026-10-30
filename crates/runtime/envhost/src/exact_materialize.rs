//! Verified saved files are published once into an unused directory without merging later names.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use orch_link_contract::commands::{FsMaterialize, FsMaterializeResult};
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
    stage: String,
    stage_device: u64,
    stage_inode: u64,
    result: Option<FsMaterializeResult>,
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
    let Some(job) = request.job_id else {
        return Ok(None);
    };
    let path = states
        .store()
        .root()
        .join("materialize-jobs")
        .join(format!("{job}.json"));
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

/// Every object is verified before workspace effects, and a final directory race is refused by rename.
pub async fn apply(
    files: &Files,
    states: &FileStates,
    request: &FsMaterialize,
    parsed: &Parsed,
    source: &dyn BlobSource,
    generation: i64,
) -> Result<FsMaterializeResult> {
    files.gates().require_remote_write()?;
    let _held = states.action_gate().await;
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
    let located = files.root().locate(&parent_path, Locate::THROUGH)?;
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
    let journal_path = states
        .store()
        .root()
        .join("materialize-jobs")
        .join(format!("{job}.json"));
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
        if let Some(result) = journal.result {
            return Ok(result);
        }
        journal
    } else {
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
        let stage = format!(".orch-tmp-exact-{job}");
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
            stage,
            stage_device: meta.dev,
            stage_inode: meta.ino,
            result: None,
        };
        save(&journal_path, &journal)?;
        journal
    };
    let staged = parent.lstat(OsStr::new(&journal.stage));
    let already_published = match staged {
        Ok(meta)
            if (meta.dev, meta.ino) == (journal.stage_device, journal.stage_inode)
                && meta.is_dir() =>
        {
            false
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && parent.lstat(OsStr::new(name)).is_ok_and(|meta| {
                    (meta.dev, meta.ino) == (journal.stage_device, journal.stage_inode)
                        && meta.is_dir()
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
            &journal.stage
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
        parent
            .rename_to(OsStr::new(&journal.stage), &parent, OsStr::new(name), false)
            .map_err(|error| {
                HostError::io(
                    "publishing exact files only into the still-unused directory",
                    error,
                )
            })?;
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
    journal.result = Some(result);
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
