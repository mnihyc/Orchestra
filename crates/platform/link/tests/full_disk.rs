//! A machine whose disk is full keeps its link: the verified address it cannot save and the log lines it cannot
//! write are lost, and the link connects again by itself after a break, for as long as the disk stays full.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::TimeZone;
use orch_link::machine::LinkStatus;
use orch_link_contract::session::Role;
use orch_link_contract::version::Limits;
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, ManualClock, new_id};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const CHILD_DIRECTORY: &str = "ORCH_TEST_FULL_DISK_LINK_CHILD";
const READY: &str = "logging started onto the full disk";
const CONNECTED_AGAIN: &str = "the link connected again after the break";

async fn within<T>(seconds: u64, what: &str, work: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(seconds), work)
        .await
        .unwrap_or_else(|_| panic!("{what} took longer than {seconds} s"))
}

/// The link of a machine whose every save of its verified address fails, through its first connection, a break and
/// the next connection.
async fn connects_and_connects_again(directory: &Path) {
    let clock = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 11, 26, 18, 0, 0)
            .single()
            .unwrap(),
    );
    let shared: Arc<dyn Clock> = Arc::new(clock);
    let pki = TestPki::new(Arc::clone(&shared));
    let hub = Hub::with_limits(Arc::clone(&shared), pki.certificate(), Limits::default(), 1);
    let id = new_id();
    let identity = pki.machine(id);
    let _controller = ScriptedController::attach(
        &hub,
        Arc::clone(&shared),
        id,
        vec![Role::Agent, Role::Environment],
    );
    // A directory where the verified address is kept fails every save of it, as a full disk does.
    let data = directory.join("data");
    std::fs::create_dir_all(data.join("connection.json").join("taken")).unwrap();
    let machine = ScriptedMachine::start_in(&hub, &shared, &identity, &MachineSpec::default(), &data);
    within(20, "the first connection", machine.link.connected()).await;
    let first = hub.session_of(id);
    assert!(first.is_some());
    hub.faults().sever_now();
    within(30, "connecting again after the break", async {
        loop {
            let again = hub.session_of(id).is_some_and(|session| Some(session) != first);
            if again && matches!(*machine.link.status().borrow(), LinkStatus::Connected { .. }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
}

#[test]
fn full_disk_link_child() {
    let Some(directory) = std::env::var_os(CHILD_DIRECTORY) else {
        return;
    };
    let directory = PathBuf::from(directory);
    orch_telemetry::logging::init_with_files(
        "connector",
        orch_telemetry::LogFormat::Text,
        &directory.join("logs"),
        14,
    );
    // Standard error still took the first line; from here on it fails as the day's file does.
    println!("{READY}");
    let mut go = String::new();
    std::io::stdin().read_line(&mut go).unwrap();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(connects_and_connects_again(&directory));
    println!("{CONNECTED_AGAIN}");
}

#[tokio::test]
async fn a_machine_that_can_save_nothing_and_log_nothing_connects_again_after_a_break() {
    let directory = tempfile::tempdir().unwrap();
    let logs = directory.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    // The day's file answers as a full disk does.
    std::os::unix::fs::symlink(
        "/dev/full",
        logs.join(format!(
            "connector-{}.log",
            chrono::Utc::now().format("%Y-%m-%d")
        )),
    )
    .unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "full_disk_link_child", "--nocapture"])
        .env(CHILD_DIRECTORY, directory.path())
        .env("RUST_BACKTRACE", "0")
        .env("ORCH_LOG", "info")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    within(30, "logging to start", async {
        while let Some(line) = lines.next_line().await.unwrap() {
            let ready = line == READY;
            seen.push(line);
            if ready {
                return;
            }
        }
        panic!("the child ended before logging started: {seen:?}");
    })
    .await;
    // Nobody reads standard error any more: each write to it fails, as onto the full disk under the day's file.
    drop(child.stderr.take());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"go\n")
        .await
        .unwrap();
    let status = within(120, "the child", async {
        while let Some(line) = lines.next_line().await.unwrap() {
            seen.push(line);
        }
        child.wait().await.unwrap()
    })
    .await;
    assert!(status.success(), "{seen:#?}");
    assert!(seen.iter().any(|line| line == CONNECTED_AGAIN), "{seen:#?}");
}
