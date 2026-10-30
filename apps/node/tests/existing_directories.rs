//! A node and personal connector use the same roots check; the trusted helper verifies the real unit uid without chown.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_connector_core::policy::inspect_environment_directory;
use orch_link_contract::policy::LocalPolicy;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::process::CommandExt;

#[test]
fn existing_roots_refuse_an_outside_path_a_link_out_and_a_file() {
    let lab = tempfile::tempdir().unwrap();
    let allowed = lab.path().join("allowed");
    let outside = lab.path().join("outside");
    std::fs::create_dir_all(allowed.join("project")).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(allowed.join("file"), "data").unwrap();
    symlink(&outside, allowed.join("escape")).unwrap();
    let policy = LocalPolicy {
        environment_roots: vec![allowed.display().to_string()],
        ..LocalPolicy::default()
    };
    let selected = inspect_environment_directory(&policy, &allowed.join("project")).unwrap();
    let original = std::fs::metadata(allowed.join("project")).unwrap();
    assert_eq!(selected.device, Some(original.dev()));
    assert_eq!(selected.inode, Some(original.ino()));
    for path in [outside, allowed.join("escape"), allowed.join("file")] {
        let refused = inspect_environment_directory(&policy, &path).unwrap_err();
        assert!(refused.detail.contains("environment.roots"));
    }
}

#[test]
fn a_directory_not_writable_by_the_unit_names_its_owners_and_uid_without_changing_it() {
    let lab = tempfile::tempdir().unwrap();
    std::fs::set_permissions(lab.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = lab.path().join("existing");
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o555)).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let (uid, gid) = if rustix::process::geteuid().is_root() {
        (65534, 65534)
    } else {
        (
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        )
    };
    let expected =
        serde_json::json!({"path":path,"device":metadata.dev(),"inode":metadata.ino()}).to_string();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_orch-node"))
        .args([
            "__runtime-helper",
            "directory-check",
            "--target",
            path.to_str().unwrap(),
            "--reserved",
            &expected,
        ])
        .uid(uid)
        .gid(gid)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let reason = String::from_utf8_lossy(&output.stderr);
    assert!(
        reason.contains("owners") && reason.contains(&format!("user id {uid}")),
        "{reason}"
    );
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(
        (after.uid(), after.gid(), after.mode()),
        (metadata.uid(), metadata.gid(), metadata.mode())
    );
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
}
