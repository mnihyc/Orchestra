//! Instruction-root control metadata preserves user files, obeys local policy and stays out of copied workspace content.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

use orch_envhost::{
    blobs::MemoryBlobs,
    environment::{GateState, Gates, Limits},
    files::Files,
    ignore::IgnoreRules,
    index::HashCache,
    instruction_root, manifest,
    paths::{PathStyle, RelPath, Workspace},
    snapshot::NoProgress,
};
use orch_link_contract::commands::Selection;
use orch_types::SystemClock;
use std::path::Path;
use std::sync::Arc;

fn files(root: &Path, gates: Arc<GateState>) -> Files {
    Files::workspace(
        Workspace::open(root, &[], PathStyle::Posix, true).unwrap(),
        Arc::new(IgnoreRules::new(
            &["!.orch-root/".into(), "!**/.orch-root/**".into()],
            true,
        )),
        Arc::new(HashCache::new()),
        Limits::default(),
        gates,
    )
}

#[tokio::test]
async fn instructions_marker_guard_is_empty_repeatable_hidden_and_recreated_after_a_move() {
    let source = tempfile::tempdir().unwrap();
    let gates = Arc::new(GateState::new(Gates::OPEN));
    let source_files = files(source.path(), gates.clone());
    std::fs::write(
        source.path().join("AGENTS.md"),
        "the root's own instructions\n",
    )
    .unwrap();
    instruction_root::prepare(source_files.root(), &gates).unwrap();
    instruction_root::prepare(source_files.root(), &gates).unwrap();
    assert_eq!(
        std::fs::read_dir(source.path().join(".orch-root"))
            .unwrap()
            .count(),
        0
    );
    let listing = source_files.list(&RelPath::root()).unwrap();
    assert!(listing.entries.iter().any(|e| e.name == "AGENTS.md"));
    assert!(!listing.entries.iter().any(|e| e.name == ".orch-root"));
    let marker = RelPath::parse(".orch-root", PathStyle::Posix).unwrap();
    assert!(source_files.stat(&marker).is_err());
    assert!(source_files.make_dir(&marker).is_err());
    let selection = Selection {
        include: vec![],
        exclude: vec![],
    };
    let resolved = manifest::resolve(source_files.root(), &selection).unwrap();
    let entries = manifest::collect(&source_files, &resolved).unwrap();
    assert!(entries.iter().any(|e| e.p == "AGENTS.md"));
    assert!(!entries.iter().any(|e| e.p.contains(".orch-root")));
    let store = MemoryBlobs::new();
    let snapshot = source_files
        .snapshot(&selection, 1, &SystemClock, &store, &NoProgress)
        .await
        .unwrap();
    let bytes = store.get_bytes(&snapshot.result.manifest_blob).unwrap();
    let stored = manifest::parse(&bytes).unwrap();
    assert!(stored.entries.iter().any(|e| e.p == "AGENTS.md"));
    assert!(!stored.entries.iter().any(|e| e.p.contains(".orch-root")));
    let destination = tempfile::tempdir().unwrap();
    let moved_files = files(destination.path(), gates.clone());
    moved_files
        .materialize(
            orch_link_contract::commands::MaterializePurpose::Move,
            "",
            &stored,
            &store,
            &NoProgress,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(destination.path().join("AGENTS.md")).unwrap(),
        "the root's own instructions\n"
    );
    assert!(!destination.path().join(".orch-root").exists());
    instruction_root::prepare(moved_files.root(), &gates).unwrap();
    assert_eq!(
        std::fs::read_dir(destination.path().join(".orch-root"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn instructions_marker_guard_refuses_files_links_content_and_forbidden_personal_writes() {
    for kind in ["file", "link", "content"] {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join(".orch-root");
        match kind {
            "file" => std::fs::write(&marker, "mine").unwrap(),
            "link" => {
                std::fs::create_dir(root.path().join("owned")).unwrap();
                std::fs::write(root.path().join("owned/mine"), "mine").unwrap();
                std::os::unix::fs::symlink("owned", &marker).unwrap();
            }
            _ => {
                std::fs::create_dir(&marker).unwrap();
                std::fs::write(marker.join("mine"), "mine").unwrap();
            }
        }
        let gates = Arc::new(GateState::new(Gates::OPEN));
        let files = files(root.path(), gates.clone());
        assert!(instruction_root::prepare(files.root(), &gates).is_err());
        let preserved = if kind == "file" {
            marker
        } else {
            marker.join("mine")
        };
        assert_eq!(std::fs::read_to_string(preserved).unwrap(), "mine");
    }
    let root = tempfile::tempdir().unwrap();
    let gates = Arc::new(GateState::new(Gates::CLOSED));
    let files = files(root.path(), gates.clone());
    assert!(matches!(
        instruction_root::prepare(files.root(), &gates),
        Err(orch_envhost::error::HostError::RemoteWriteDisabled)
    ));
    assert!(!root.path().join(".orch-root").exists());
}
