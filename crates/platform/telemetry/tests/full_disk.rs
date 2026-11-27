//! A full disk under the day's file and under standard error loses the lines written meanwhile and nothing more:
//! the code that logged goes on.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

const CHILD_DIRECTORY: &str = "ORCH_TEST_FULL_DISK_CHILD";
const WENT_ON: &str = "the code that logged went on";

#[test]
fn full_disk_child() {
    let Some(directory) = std::env::var_os(CHILD_DIRECTORY) else {
        return;
    };
    orch_telemetry::logging::init_with_files(
        "connector",
        orch_telemetry::LogFormat::Text,
        std::path::Path::new(&directory),
        14,
    );
    // Every task of a connector logs: its link's reconnecting does when a cached address cannot be saved.
    let logged = std::thread::spawn(|| {
        for attempt in 1..=3 {
            tracing::warn!(attempt, "a cached file could not be saved");
        }
    })
    .join();
    assert!(logged.is_ok(), "the thread that logged ended with its line");
    println!("{WENT_ON}");
}

#[tokio::test]
async fn a_log_written_onto_a_full_disk_loses_its_lines_and_never_the_code_that_logs() {
    let directory = tempfile::tempdir().unwrap();
    // The day's file and standard error both answer as a full disk does.
    std::os::unix::fs::symlink(
        "/dev/full",
        directory.path().join(format!(
            "connector-{}.log",
            chrono::Utc::now().format("%Y-%m-%d")
        )),
    )
    .unwrap();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "full_disk_child", "--nocapture"])
            .env(CHILD_DIRECTORY, directory.path())
            .env("RUST_BACKTRACE", "0")
            .env("ORCH_LOG", "info")
            .stderr(full)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("the logging child did not end")
    .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(WENT_ON), "{stdout}");
}
