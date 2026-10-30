//! Windows private files are published only after their access list is restricted.
#![cfg(windows)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_pki::certificates::write_private_file;

#[test]
fn an_owner_can_replace_a_private_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret");
    write_private_file(&path, "first").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
    write_private_file(&path, "second").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
}

#[test]
fn a_private_file_is_not_published_when_the_acl_tool_is_unavailable() {
    const PROBE: &str = "ORCH_TEST_PRIVATE_FILE_PROBE";
    if let Some(root) = std::env::var_os(PROBE) {
        let path = std::path::PathBuf::from(root).join("secret");
        assert!(write_private_file(&path, "private").is_err());
        assert!(!path.exists());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_private_file_is_not_published_when_the_acl_tool_is_unavailable",
        ])
        .env(PROBE, dir.path())
        .env("SystemRoot", dir.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!dir.path().join("secret").exists());
}
