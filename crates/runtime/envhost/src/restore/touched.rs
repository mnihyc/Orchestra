//! The existing restore planner over explicit file-tool paths, without a directory walk.
use super::*;
use crate::state_store::StateRoot;
use orch_api_contract::source::{FileChangeImage, RestoreForm, TouchedRestore};
use std::collections::BTreeSet;

pub(super) fn scan_selected(
    files: &Files,
    directory: &RelPath,
    paths: &[String],
) -> Result<Observed> {
    let root = files.root().locate(directory, Locate::ENTRY)?;
    let meta = root
        .meta
        .ok_or_else(|| HostError::NotFound("the restore directory is absent".into()))?;
    if !meta.is_dir() || root.real != *directory {
        return Err(HostError::Changed(
            "the restore directory became another place".into(),
        ));
    }
    files.check_visible(directory, &root, true)?;
    let mut wanted = BTreeSet::new();
    for path in paths {
        let relative = files.parse(path)?;
        if relative.is_root() || relative.has_component(".git") {
            return Err(HostError::Ignored(path.clone()));
        }
        wanted.insert(relative.to_string());
        let mut parent = relative.parent();
        while let Some(path) = parent {
            if !path.is_root() {
                wanted.insert(path.to_string());
            }
            parent = path.parent();
        }
    }
    let mut entries = Vec::new();
    let mut indexed = Vec::new();
    let mut versions = BTreeMap::new();
    for path in wanted {
        if let Some(entry) = at_path(files, directory, &path)? {
            if entry.t != EntryKind::Dir && !paths.contains(&path) {
                return Err(HostError::Changed(
                    "a touched file's parent is no longer a directory".into(),
                ));
            }
            if entry.t == EntryKind::Dir && paths.contains(&path) {
                return Err(HostError::Unsupported(
                    "a file-tool restore never recursively replaces a directory".into(),
                ));
            }
            let full = crate::file_states::join(directory, &files.parse(&path)?, files)?;
            let located = files.root().locate(&full, Locate::ENTRY)?;
            let meta = located.meta.ok_or_else(|| {
                HostError::Changed("a touched path disappeared while planning".into())
            })?;
            versions.insert(path.clone(), PathVersion::of(&meta));
            indexed.push((
                full,
                IndexEntry {
                    kind: meta.kind,
                    size: meta.size,
                    mtime_ns: meta.mtime_ns,
                    mode: meta.mode,
                    link_target: entry.l.clone(),
                },
            ));
            entries.push(entry);
        }
    }
    let mut ancestor = Some(directory.clone());
    while let Some(path) = ancestor {
        indexed.push((
            path.clone(),
            IndexEntry {
                kind: platform::Kind::Dir,
                size: 0,
                mtime_ns: 0,
                mode: 0,
                link_target: None,
            },
        ));
        ancestor = path.parent();
    }
    let rules = GitRules::read(files, &indexed, directory)?;
    for entry in &entries {
        let full = crate::file_states::join(directory, &files.parse(&entry.p)?, files)?;
        if rules.ignored(&full, entry.t == EntryKind::Dir) {
            return Err(HostError::Ignored(entry.p.clone()));
        }
        if entry.t != EntryKind::Dir && !paths.contains(&entry.p) {
            return Err(HostError::Changed(
                "a touched file's parent is no longer a directory".into(),
            ));
        }
    }
    let ignore_fingerprint = rules.fingerprint;
    entries.sort_by(|a, b| a.p.cmp(&b.p));
    Ok(Observed {
        fingerprint: namespace_fingerprint(&entries)?,
        entries,
        indexed,
        versions,
        directory_version: PathVersion::of(&meta),
        ignore_fingerprint,
    })
}
fn image_hash(image: &FileChangeImage) -> Option<Hash> {
    match image {
        FileChangeImage::Absent => None,
        FileChangeImage::Text { text } => Some(Hash::of(text.as_bytes())),
        FileChangeImage::Blob { blob, .. } => Some(*blob),
    }
}
fn current_hash(entry: Option<&ManifestEntry>) -> Option<Hash> {
    entry.and_then(|entry| {
        entry
            .h
            .or_else(|| entry.l.as_ref().map(|link| Hash::of(link.as_bytes())))
    })
}
pub(super) fn touched_plan(
    files: &Files,
    states: &FileStates,
    request: &FsRestorePlan,
    touched: &TouchedRestore,
    generation: i64,
    now: Time,
) -> Result<(StoredPlan, FsRestorePlanResult)> {
    let directory = files.parse(&request.directory)?;
    let mut paths = touched
        .paths
        .iter()
        .map(|path| path.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    if paths.len() != touched.paths.len() {
        return Err(HostError::Changed(
            "the touched-file plan contains duplicate paths".into(),
        ));
    }
    if touched.overwrite.iter().any(|path| !paths.contains(path)) {
        return Err(HostError::Changed(
            "overwrite names a path outside the touched plan".into(),
        ));
    }
    let current = scan_selected(files, &directory, &paths)?;
    let mut target = current
        .entries
        .iter()
        .filter(|entry| !paths.contains(&entry.p))
        .cloned()
        .collect::<Vec<_>>();
    let mut objects = BTreeMap::new();
    let mut blockers = Vec::new();
    for selected in &touched.paths {
        let old = current
            .entries
            .iter()
            .find(|entry| entry.p == selected.path);
        match &selected.before {
            Some(FileChangeImage::Absent) => {}
            Some(FileChangeImage::Text { text }) => {
                let oid = states.store().write_object("blob", text.as_bytes())?;
                objects.insert(selected.path.clone(), oid);
                target.push(ManifestEntry {
                    p: selected.path.clone(),
                    t: EntryKind::File,
                    s: i64::try_from(text.len()).map_err(|_| {
                        HostError::Unsupported("the touched pre-image is too large".into())
                    })?,
                    m: 0,
                    x: old.map_or(NEW_FILE_BITS, |entry| entry.x),
                    h: Some(Hash::of(text.as_bytes())),
                    l: None,
                });
            }
            Some(FileChangeImage::Blob { .. }) => {
                blockers.push(format!("preimage_unavailable:{}", selected.path));
                if let Some(entry) = old {
                    target.push(entry.clone());
                }
            }
            None => {
                blockers.push(format!("preimage_unavailable:{}", selected.path));
                if let Some(entry) = old {
                    target.push(entry.clone());
                }
            }
        }
    }
    // Missing parents are explicit journaled mkdir effects, never a recursive replacement.
    for path in target
        .clone()
        .iter()
        .filter(|entry| entry.t == EntryKind::File)
    {
        let mut parent = files.parse(&path.p)?.parent();
        while let Some(relative) = parent {
            if !relative.is_root() && !target.iter().any(|entry| entry.p == relative.to_string()) {
                target.push(ManifestEntry {
                    p: relative.to_string(),
                    t: EntryKind::Dir,
                    s: 0,
                    m: 0,
                    x: NEW_DIRECTORY_BITS,
                    h: None,
                    l: None,
                });
            }
            parent = relative.parent();
        }
    }
    target.sort_by(|a, b| a.p.cmp(&b.p));
    let mut effects = effects(&current.entries, &target)?;
    for effect in &mut effects {
        let Some(source) = touched.paths.iter().find(|path| path.path == effect.path) else {
            // An absent parent needed by a recorded file is safe only under this plan's
            // ordinary namespace recheck; it has no agent write to overwrite.
            effect.later_change = Some(false);
            continue;
        };
        let old = current.entries.iter().find(|entry| entry.p == effect.path);
        effect.later_change = source
            .after
            .as_ref()
            .map(|image| image_hash(image) != current_hash(old));
    }
    // Recording-off disables whole states only. Explicit file-tool undo uses the shipped keeping
    // period and its selected bytes, rather than imposing the whole-directory recording bound.
    let policy = RecordPolicy {
        record_days: request
            .record_days
            .filter(|days| *days > 0)
            .unwrap_or(crate::file_states::DEFAULT_RECORD_DAYS),
        max_bytes: i64::MAX,
    };
    let built = crate::manifest::build(
        &crate::manifest::header_for(files.root(), &directory, generation, now.get()),
        target.clone(),
    )?;
    let expires = now
        .get()
        .checked_add_signed(
            chrono::Duration::try_days(policy.record_days).ok_or_else(|| {
                HostError::Unsupported("the touched keeping period is invalid".into())
            })?,
        )
        .ok_or_else(|| HostError::Unsupported("the touched keeping deadline overflowed".into()))?;
    let target_root = if blockers.is_empty() {
        let root = StateRoot {
            selected_paths: Some(paths.clone()),
            snapshot_id: orch_types::new_id(),
            request_hash: None,
            directory: request.directory.clone(),
            generation,
            kind: orch_link_contract::source::RecordKind::Restore,
            tree_ref: states.store().write_tree(&target, &objects)?,
            manifest_blob: built.hash,
            objects,
            files: built.file_count,
            bytes: built.total_bytes,
            created_at: now,
            expires_at: Time::new(expires),
            repository_head: None,
            ignore_fingerprint: current.ignore_fingerprint,
        };
        states.store().keep(&root, &built.bytes)?;
        Some(root.snapshot_id)
    } else {
        None
    };
    let form = RestoreForm::TouchedFiles {
        count: paths.len() as u64,
        whole_unavailable: touched.whole_unavailable.clone(),
    };
    let plan = StoredPlan {
        overwrite: touched.overwrite.clone(),
        target_root,
        form: form.clone(),
        touched_paths: Some(paths),
        touched_fingerprint: Some(Hash::of(
            &serde_json::to_vec(touched).map_err(|error| HostError::Internal(error.to_string()))?,
        )),
        blockers: blockers.clone(),
        snapshot_id: request.snapshot_id,
        target_manifest: built.hash,
        directory: request.directory.clone(),
        generation,
        namespace_fingerprint: current.fingerprint,
        ignore_fingerprint: current.ignore_fingerprint,
        directory_version: current.directory_version,
        versions: current.versions,
        current: current.entries,
        target,
        paths: effects.clone(),
        record_policy: policy,
    };
    let bytes =
        serde_json::to_vec(&plan).map_err(|error| HostError::Internal(error.to_string()))?;
    let hash = Hash::of(&bytes);
    atomic_write(
        &states.store().root().join("plans").join(hash.to_string()),
        &bytes,
    )?;
    Ok((
        plan,
        FsRestorePlanResult {
            form,
            blockers: blockers.clone(),
            plan_blob: hash,
            paths: effects,
            writers_active: false,
            undo_available: blockers.is_empty(),
        },
    ))
}
