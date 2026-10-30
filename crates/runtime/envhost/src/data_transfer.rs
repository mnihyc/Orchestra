//! Closed data-root transfers reuse checked snapshots and materialization on every hosting machine.

use crate::blobs::{BlobSink, BlobSource, MemoryWriter, download_checked};
use crate::error::{HostError, Result};
use crate::files::Files;
use crate::index::HashCache;
use crate::paths::Workspace;
use crate::snapshot::Progress;
use orch_link_contract::commands::{
    FsMaterializeResult, FsSnapshotResult, MaterializePurpose, Selection,
};
use orch_link_contract::source::EnvironmentDataPart;
use orch_types::{Clock, Hash};
use std::path::Path;
use std::sync::Arc;

/// Every persistent store component; identity, sealed credentials and transfer downloads are elsewhere.
const RECORDED_PARTS: &[&str] = &[
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

pub(crate) fn allowed(part: EnvironmentDataPart, path: &str) -> bool {
    match part {
        EnvironmentDataPart::FileStates => {
            RECORDED_PARTS.contains(&path.split('/').next().unwrap_or_default())
        }
        EnvironmentDataPart::Home => {
            path != ".orch/file-states" && !path.starts_with(".orch/file-states/")
        }
    }
}

pub(crate) fn selection(part: EnvironmentDataPart) -> Selection {
    match part {
        EnvironmentDataPart::FileStates => Selection {
            include: RECORDED_PARTS.iter().map(|name| (*name).into()).collect(),
            exclude: Vec::new(),
        },
        EnvironmentDataPart::Home => Selection {
            include: Vec::new(),
            exclude: vec![".orch/file-states".into()],
        },
    }
}

pub(crate) fn files(root: &Path, workspace: &Files) -> Result<Files> {
    let source = workspace.root();
    let root = Workspace::open(root, &[], source.style(), source.case_sensitive())?;
    Ok(at_root(root, workspace))
}

pub(crate) fn at_root(root: Workspace, workspace: &Files) -> Files {
    Files::owned_data(
        root,
        Arc::new(HashCache::new()),
        workspace.limits().clone(),
        Arc::clone(workspace.gates()),
    )
}

pub(crate) async fn export(
    files: &Files,
    part: EnvironmentDataPart,
    generation: i64,
    clock: &dyn Clock,
    sink: &dyn BlobSink,
    progress: &dyn Progress,
) -> Result<FsSnapshotResult> {
    let result = files
        .snapshot(&selection(part), generation, clock, sink, progress)
        .await?
        .result;
    if result.missing != 0 {
        return Err(HostError::Unsupported(
            "the complete promised data root could not be read; the source is kept".into(),
        ));
    }
    Ok(result)
}

pub(crate) async fn import(
    files: &Files,
    part: EnvironmentDataPart,
    hash: Hash,
    source: &dyn BlobSource,
    progress: &dyn Progress,
) -> Result<FsMaterializeResult> {
    files.gates().require_remote_write()?;
    let limit = usize::try_from(files.limits().max_blob_bytes.max(0))
        .unwrap_or(usize::MAX)
        .min(usize::try_from(crate::manifest::MANIFEST_MAX_EXPANDED_BYTES).unwrap_or(usize::MAX));
    let mut bytes = MemoryWriter::with_limit(limit);
    download_checked(source, hash, &mut bytes)
        .await
        .map_err(|error| HostError::Content(error.to_string()))?;
    let parsed = crate::manifest::parse(&bytes.into_bytes())?;
    if parsed.entries.iter().any(|entry| !allowed(part, &entry.p)) {
        return Err(HostError::Unsupported(
            "the data snapshot names a root outside the reviewed data part".into(),
        ));
    }
    let root = files.root().root_path().to_string_lossy().into_owned();
    Ok(files
        .materialize(MaterializePurpose::Move, &root, &parsed, source, progress)
        .await?
        .result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::MemoryBlobs;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::paths::PathStyle;
    use crate::snapshot::NoProgress;

    fn workspace(root: &Path) -> Files {
        Files::workspace(
            Workspace::open(root, &[], PathStyle::Posix, true).unwrap(),
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        )
    }

    #[tokio::test]
    async fn a_portable_recorded_store_keeps_raw_nested_bytes_and_never_copies_connector_identity()
    {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(from.path().join("git/objects/ab")).unwrap();
        std::fs::create_dir_all(from.path().join("journals")).unwrap();
        std::fs::write(
            from.path().join("git/objects/ab/object"),
            b"\0\xffrecorded bytes",
        )
        .unwrap();
        std::fs::write(
            from.path().join("journals/operation.json"),
            b"{\"phase\":\"complete\"}",
        )
        .unwrap();
        std::fs::write(from.path().join("identity.key"), b"never carry").unwrap();
        std::fs::write(from.path().join(".orch_ignore"), b"git/\njournals/\n").unwrap();
        let source = files(from.path(), &workspace(from.path())).unwrap();
        let target = files(to.path(), &workspace(to.path())).unwrap();
        let blobs = MemoryBlobs::new();
        let saved = export(
            &source,
            EnvironmentDataPart::FileStates,
            0,
            &orch_types::SystemClock,
            &blobs,
            &NoProgress,
        )
        .await
        .unwrap();
        assert_eq!(saved.missing, 0);
        target.gates().set(Gates::CLOSED);
        assert!(
            import(
                &target,
                EnvironmentDataPart::FileStates,
                saved.manifest_blob,
                &blobs,
                &NoProgress
            )
            .await
            .is_err()
        );
        assert!(std::fs::read_dir(to.path()).unwrap().next().is_none());
        target.gates().set(Gates::OPEN);
        import(
            &target,
            EnvironmentDataPart::FileStates,
            saved.manifest_blob,
            &blobs,
            &NoProgress,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(to.path().join("git/objects/ab/object")).unwrap(),
            b"\0\xffrecorded bytes"
        );
        assert_eq!(
            std::fs::read(to.path().join("journals/operation.json")).unwrap(),
            b"{\"phase\":\"complete\"}"
        );
        assert!(!to.path().join("identity.key").exists());
        assert_eq!(
            std::fs::read(from.path().join("identity.key")).unwrap(),
            b"never carry"
        );
        let home = export(
            &source,
            EnvironmentDataPart::Home,
            0,
            &orch_types::SystemClock,
            &blobs,
            &NoProgress,
        )
        .await
        .unwrap();
        assert!(
            import(
                &target,
                EnvironmentDataPart::FileStates,
                home.manifest_blob,
                &blobs,
                &NoProgress
            )
            .await
            .is_err(),
            "a home manifest cannot name arbitrary files inside the private recorded-state root"
        );
    }
}
