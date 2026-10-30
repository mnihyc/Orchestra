//! Deep and wide trees are measured completely without following aliases or retaining a whole directory listing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

#[test]
fn a_deep_tree_counts_every_file_without_following_links() {
    let root = tempfile::tempdir().unwrap();
    let mut directory = root.path().to_path_buf();
    for level in 0..300 {
        directory.push("d");
        std::fs::create_dir(&directory).unwrap();
        for index in 0..12 {
            std::fs::write(directory.join(format!("{level}-{index}")), [1u8; 17]).unwrap();
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path(), directory.join("back-to-root")).unwrap();
    let measured = orch_node::volume_archive::measure(root.path()).unwrap();
    assert_eq!(measured.file_count, 3600);
    assert_eq!(measured.total_bytes, 3600 * 17);
}

#[test]
fn an_operator_supplied_large_fixture_is_counted_with_the_same_streaming_walker() {
    let Ok(path) = std::env::var("ORCH_MEASUREMENT_FIXTURE") else {
        return;
    };
    let measured = orch_node::volume_archive::measure(std::path::Path::new(&path)).unwrap();
    assert!(measured.file_count > 1_000_000);
    println!(
        "files={} bytes={}",
        measured.file_count, measured.total_bytes
    );
}
