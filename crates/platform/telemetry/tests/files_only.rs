//! Daily-file logging does not fill launchd's stderr log, and a panic still reaches stderr.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

const CHILD_DIRECTORY: &str = "ORCH_TEST_FILES_ONLY_CHILD";
const LINE: &str = "ordinary tracing goes to the daily file";
const PANIC: &str = "the panic hook still has stderr";

#[test]
fn files_only_child() {
    let Some(directory) = std::env::var_os(CHILD_DIRECTORY) else {
        return;
    };
    orch_telemetry::logging::init_files_only(
        "connector",
        orch_telemetry::LogFormat::Text,
        std::path::Path::new(&directory),
        14,
    );
    tracing::warn!("{LINE}");
    panic!("{PANIC}");
}

#[tokio::test]
async fn tracing_is_in_the_pruned_daily_file_while_the_panic_is_only_on_stderr() {
    let directory = tempfile::tempdir().unwrap();
    // Isolate the global subscriber/panic hook in this test program's exact child case.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "files_only_child", "--nocapture"])
            .env(CHILD_DIRECTORY, directory.path())
            .env("RUST_BACKTRACE", "0")
            .env("ORCH_LOG", "info")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("the logging child did not end")
    .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(PANIC), "{stderr}");
    assert!(
        !stderr.contains(LINE),
        "ordinary tracing must not grow launchd.log"
    );
    let files: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    assert!(
        files[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("connector-")
    );
    let written = std::fs::read_to_string(&files[0]).unwrap();
    assert!(written.contains(LINE));
    assert!(!written.contains(PANIC));
}
