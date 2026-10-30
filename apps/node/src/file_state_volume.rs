//! A fixed owned file-state subtree is measured and carried independently of other home contents.

use std::path::Path;

use orch_driver_contract::volumes::{FileStatesMeasure, VolumeMeasure};

/// The immutable store's format parts; transient downloads and scoped credentials are deliberately excluded.
const PARTS: &[&str] = &[
    "git",
    "roots",
    "manifests",
    "journals",
    "plans",
    "results",
    "policies",
    "pins",
    "source-jobs",
    "git-jobs",
    "worktree-jobs",
    "materialize-jobs",
];
/// The closed selector maps to this one location on the Docker-bound owned home volume.
const STORE: &str = "/volume/.orch/file-states";

fn count(path: &Path) -> anyhow::Result<i64> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if meta.is_file() {
        return Ok(1);
    }
    if !meta.is_dir() {
        anyhow::bail!("a persistent store part contains an unsupported link or special file")
    }
    let mut files = 0i64;
    for entry in std::fs::read_dir(path)? {
        files = files.saturating_add(count(&entry?.path())?);
    }
    Ok(files)
}

fn measure(root: &Path) -> anyhow::Result<VolumeMeasure> {
    let mut measured = crate::volume_archive::measure_parts(root, PARTS)?;
    // Counts traverse metadata only; immutable object bytes are read once by the later archive.
    let roots = count(&root.join("roots"))?;
    let objects = count(&root.join("git/objects"))?;
    let mut journals = 0i64;
    for part in [
        "journals",
        "source-jobs",
        "git-jobs",
        "worktree-jobs",
        "materialize-jobs",
    ] {
        journals = journals.saturating_add(count(&root.join(part))?);
    }
    for part in PARTS {
        count(&root.join(part))?;
    }
    measured.file_states = Some(FileStatesMeasure {
        roots,
        objects,
        total_bytes: measured.total_bytes,
        journals,
        complete: true,
    });
    Ok(measured)
}

fn checked_root(root: &Path) -> anyhow::Result<Option<std::path::PathBuf>> {
    let parent = root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("the store has no owned parent"))?;
    let home = parent
        .parent()
        .ok_or_else(|| anyhow::anyhow!("the store has no owned home"))?;
    let mut closest = home;
    let mut absent = false;
    for path in [home, parent, root] {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() && !absent => closest = path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && path != home => {
                absent = true
            }
            _ => anyhow::bail!(
                "the file-state selector has a link, non-directory or changing parent"
            ),
        }
    }
    Ok(absent.then(|| closest.to_path_buf()))
}

fn export(root: &Path, output: &mut impl std::io::Write) -> anyhow::Result<()> {
    match checked_root(root)? {
        // A never-initialized store is empty. Carry only a root header with the owned home's
        // metadata; importing it does not change the choice to leave other home files behind.
        Some(parent) => crate::volume_archive::export_parts(&parent, &[], output),
        None => crate::volume_archive::export_parts(root, PARTS, output),
    }
}

/// Runs only the three fixed store operations before the node starts any asynchronous work.
pub fn run(operation: crate::runtime_helper::Operation) -> anyhow::Result<()> {
    use crate::runtime_helper::Operation;
    let root = Path::new(STORE);
    if operation == Operation::FileStatesImport {
        let parent = root
            .parent()
            .ok_or_else(|| anyhow::anyhow!("the file-state selector has no parent"))?;
        for path in [parent, root] {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_dir() => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(path)?
                }
                _ => anyhow::bail!("the file-state selector has an unexpected parent or root"),
            }
        }
    }
    let _ = checked_root(root)?;
    match operation {
        Operation::FileStatesMeasure => {
            println!("{}", serde_json::to_string(&measure(root)?)?);
            Ok(())
        }
        Operation::FileStatesExport => export(root, &mut std::io::stdout().lock()),
        Operation::FileStatesImport => {
            crate::volume_archive::import_subtree(root, &mut std::io::stdin().lock())
        }
        _ => anyhow::bail!("this is not a fixed file-state store operation"),
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod tests {
    use super::*;
    use orch_envhost::environment::{GateState, Gates, Limits};
    use orch_envhost::file_states::FileStates;
    use orch_envhost::files::Files;
    use orch_envhost::ignore::IgnoreRules;
    use orch_envhost::index::{HashCache, Index};
    use orch_envhost::paths::{PathStyle, RelPath, Workspace};
    use orch_envhost::state_store::ObjectStore;
    use orch_link_contract::commands::IndexMode;
    use orch_link_contract::source::{FsRecord, RecordKind, RecordState};
    use std::sync::Arc;

    #[test]
    fn a_real_recorded_store_carries_roots_objects_expiry_and_journals_without_other_home_files_or_command_credentials()
     {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let workspace = source.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("code"), b"kept recorded bytes").unwrap();
        let home = source.path().join("home");
        let root = home.join(".orch/file-states");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(home.join("other-home-file"), "left behind").unwrap();
        let files = Files::workspace(
            Workspace::open(&workspace, &[], PathStyle::Posix, true).unwrap(),
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let states = FileStates::new(ObjectStore::open(&root, &workspace).unwrap());
        states.request(
            &RelPath::root(),
            orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
        );
        let index = Index::new(IndexMode::Full, Limits::default().index_max_entries);
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        let at: orch_types::Time = "2026-10-14T10:00:00.000Z".parse().unwrap();
        let request = FsRecord {
            environment_id: orch_types::new_id(),
            snapshot_id: orch_types::new_id(),
            directory: String::new(),
            kind: RecordKind::Turn,
            record_days: 7,
            max_bytes: orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
        };
        let recorded = states.record(&files, &request, 17, at).unwrap();
        assert_eq!(recorded.state, RecordState::Complete);
        let original = states.store().load(request.snapshot_id, at).unwrap().0;
        let secret = root.join("source-credential-interrupted");
        std::fs::create_dir(&secret).unwrap();
        std::fs::write(secret.join("password"), "SCOPED_COMMAND_TOKEN").unwrap();
        let recovery = serde_json::to_vec(&serde_json::json!({"phase":"applying","snapshot_id":request.snapshot_id,"undo_snapshot_id":request.snapshot_id})).unwrap();
        std::fs::write(root.join("journals/recovery.json"), &recovery).unwrap();
        std::fs::create_dir_all(root.join("plans")).unwrap();
        std::fs::write(root.join("plans/frozen"), "kept plan metadata").unwrap();
        let measured = measure(&root).unwrap().file_states.unwrap();
        assert_eq!(measured.roots, 1);
        assert!(measured.objects > 0);
        assert_eq!(measured.journals, 1);
        let mut archive = Vec::new();
        crate::volume_archive::export_parts(&root, PARTS, &mut archive).unwrap();
        assert!(
            !archive
                .windows(b"SCOPED_COMMAND_TOKEN".len())
                .any(|part| part == b"SCOPED_COMMAND_TOKEN")
        );
        let copied = target.path().join("home/.orch/file-states");
        std::fs::create_dir_all(&copied).unwrap();
        crate::volume_archive::import_subtree(&copied, &mut &archive[..]).unwrap();
        let next_workspace = target.path().join("workspace");
        std::fs::create_dir(&next_workspace).unwrap();
        let loaded = ObjectStore::open(&copied, &next_workspace).unwrap();
        let (root, manifest) = loaded.load(request.snapshot_id, at).unwrap();
        assert_eq!(root.tree_ref, original.tree_ref);
        assert_eq!(root.manifest_blob, original.manifest_blob);
        assert_eq!(root.expires_at, original.expires_at);
        let entry = manifest
            .entries
            .iter()
            .find(|entry| entry.p == "code")
            .unwrap();
        let bytes = loaded.read_entry(&root, entry).unwrap();
        assert_eq!(std::fs::read(bytes.path()).unwrap(), b"kept recorded bytes");
        assert_eq!(
            std::fs::read(copied.join("journals/recovery.json")).unwrap(),
            recovery
        );
        let checked = loaded.verify_kept(at).unwrap();
        assert_eq!(checked.roots, 1);
        assert!(checked.objects > 0);
        assert_eq!(checked.journals, 1);
        let object = loaded
            .root()
            .join("git/objects")
            .join(&root.tree_ref[..2])
            .join(&root.tree_ref[2..]);
        std::fs::write(object, "corrupt carried tree").unwrap();
        assert!(
            loaded.verify_kept(at).is_err(),
            "corrupt required objects prevent source release"
        );
        assert_eq!(
            std::fs::read(copied.join("plans/frozen")).unwrap(),
            b"kept plan metadata"
        );
        assert!(!copied.join("source-credential-interrupted").exists());
        assert!(!target.path().join("home/other-home-file").exists());
    }

    #[test]
    fn a_never_initialized_store_moves_as_empty_without_creating_or_carrying_other_home_contents() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("home-file"), "left behind").unwrap();
        let missing = source.path().join(".orch/file-states");
        assert!(checked_root(&missing).unwrap().is_some());
        let measured = measure(&missing).unwrap().file_states.unwrap();
        assert!(measured.complete);
        assert_eq!(measured.roots, 0);
        assert_eq!(measured.total_bytes, 0);
        let mut bytes = Vec::new();
        export(&missing, &mut bytes).unwrap();
        assert!(
            !missing.exists(),
            "measure and export do not initialize the source store"
        );
        let copied = target.path().join(".orch/file-states");
        std::fs::create_dir_all(&copied).unwrap();
        crate::volume_archive::import_subtree(&copied, &mut bytes.as_slice()).unwrap();
        assert_eq!(std::fs::read_dir(&copied).unwrap().count(), 0);
        assert!(!target.path().join("home-file").exists());
        let workspace = target.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let store = ObjectStore::open(&copied, &workspace).unwrap();
        assert_eq!(
            store
                .verify_kept("2026-10-14T22:00:00.000Z".parse().unwrap())
                .unwrap()
                .roots,
            0
        );
    }
}
