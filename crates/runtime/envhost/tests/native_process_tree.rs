//! Native tracked processes and terminal children end only within their owned tree.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orch_envhost::platform::{self, Sig};
use orch_envhost::processes::{Processes, RootKind, ScopeKind};
use orch_link_contract::commands::Signal;
use orch_types::{SystemClock, Time};
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};

/// The fixture has enough time to be observed; the test always ends it earlier.
const FIXTURE_LIFETIME: Duration = Duration::from_secs(60);
/// Native process tables may take time to show a just-started child.
const PATIENCE: Duration = Duration::from_secs(15);
/// Poll the process table without busy waiting.
const POLL: Duration = Duration::from_millis(25);
/// Windows reports process start times in whole seconds, so a child must start in a later second
/// for the fixture's parentage to be distinguishable from a recycled parent number.
#[cfg(windows)]
const START_TIME_RESOLUTION: Duration = Duration::from_secs(1);
const FIXTURE_ROLE: &str = "ORCH_TEST_NATIVE_TREE_ROLE";
const FIXTURE_MARKER: &str = "ORCH_TEST_NATIVE_TREE_MARKER";

#[test]
fn native_process_fixture() {
    match std::env::var(FIXTURE_ROLE).as_deref() {
        Ok("parent") => {
            #[cfg(windows)]
            std::thread::sleep(START_TIME_RESOLUTION);
            let mut child = fixture_command("leaf").spawn().unwrap();
            let marker = std::env::var_os(FIXTURE_MARKER).unwrap();
            std::fs::write(marker, format!("{} {}", std::process::id(), child.id())).unwrap();
            let _ = child.wait();
        }
        Ok("leaf") => std::thread::sleep(FIXTURE_LIFETIME),
        _ => {}
    }
}

fn fixture_command(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "native_process_fixture", "--nocapture"])
        .env(FIXTURE_ROLE, role)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn wait_for_marker(path: &Path) -> (u32, u32) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let values: Vec<u32> = text
                .split_whitespace()
                .filter_map(|value| value.parse().ok())
                .collect();
            if let [parent, child] = values.as_slice() {
                return (*parent, *child);
            }
        }
        assert!(
            Instant::now() < deadline,
            "the fixture did not start its child"
        );
        std::thread::sleep(POLL);
    }
}

fn start_of(processes: &Processes, pid: u32) -> (i64, Time) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(row) = processes.list().into_iter().find(|row| row.info.pid == pid) {
            return (row.start_ms, row.info.started_at);
        }
        assert!(Instant::now() < deadline, "process {pid} was not tracked");
        std::thread::sleep(POLL);
    }
}

fn is_alive(pid: u32, start_ms: i64) -> bool {
    let mut system = System::new();
    let id = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[id]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(id).is_some_and(|process| {
        let observed = i64::try_from(process.start_time()).unwrap_or(0) * 1000;
        let same = platform::os::exact_start_ms(pid)
            .map_or(observed == start_ms, |exact| exact == start_ms);
        same && !matches!(
            process.status(),
            ProcessStatus::Zombie | ProcessStatus::Dead
        )
    })
}

fn wait_ended(identities: &[(u32, i64)]) {
    let deadline = Instant::now() + PATIENCE;
    while identities.iter().any(|(pid, start)| is_alive(*pid, *start)) {
        assert!(
            Instant::now() < deadline,
            "a process of the fixture remained alive"
        );
        std::thread::sleep(POLL);
    }
}

struct Cleanup(Vec<(u32, i64)>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for (pid, start) in self.0.iter().rev() {
            let _ = platform::send_signal(*pid, Some(*start), Sig::Kill);
        }
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn ending_a_tracked_native_tree_preserves_an_unrelated_process() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("child-pids");
    let processes = Processes::new(ScopeKind::Started, Arc::new(SystemClock));
    let mut parent = fixture_command("parent");
    parent.env(FIXTURE_MARKER, &marker);
    let mut root = OwnedChild(
        processes
            .spawn_tracked(&mut parent, RootKind::Executor)
            .unwrap(),
    );
    let mut other = OwnedChild(fixture_command("leaf").spawn().unwrap());
    let (root_pid, child_pid) = wait_for_marker(&marker);
    assert_eq!(root.0.id(), root_pid);
    let (root_start, started_at) = start_of(&processes, root_pid);
    let (child_start, _) = start_of(&processes, child_pid);
    let _cleanup = Cleanup(vec![(root_pid, root_start), (child_pid, child_start)]);
    let wrong = Time::new(started_at.get() + chrono::Duration::seconds(7));
    assert!(
        platform::send_signal(root_pid, Some(wrong.get().timestamp_millis()), Sig::Kill).is_err()
    );
    assert!(
        processes
            .signal(i64::from(root_pid), wrong, Signal::Kill)
            .is_err()
    );
    assert!(root.0.try_wait().unwrap().is_none());
    processes
        .signal(i64::from(root_pid), started_at, Signal::Kill)
        .unwrap();
    wait_ended(&[(root_pid, root_start), (child_pid, child_start)]);
    assert!(
        other.0.try_wait().unwrap().is_none(),
        "an untracked process is not ended"
    );
    other.0.kill().unwrap();
    other.0.wait().unwrap();
    root.0.wait().unwrap();
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_windows_terminal_ends_its_shell_and_its_tracked_child_tree() {
    use orch_envhost::terminal::{OpenSpec, Purpose, TerminalLinks, Terminals};
    use std::collections::BTreeMap;

    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("terminal-child-pids");
    let processes = Processes::new(ScopeKind::Started, Arc::new(SystemClock));
    let terminals = Terminals::new(
        Arc::new(SystemClock),
        256 * 1024,
        TerminalLinks {
            processes: Some(processes.clone()),
            ..TerminalLinks::default()
        },
    )
    .unwrap();
    let id = orch_types::new_id();
    let mut env = BTreeMap::new();
    env.insert(FIXTURE_ROLE.to_owned(), "parent".to_owned());
    env.insert(
        FIXTURE_MARKER.to_owned(),
        marker.to_string_lossy().into_owned(),
    );
    let opened = terminals
        .open(OpenSpec {
            id,
            environment_id: orch_types::new_id(),
            purpose: Purpose::Shell,
            program: std::env::var("COMSPEC")
                .unwrap_or_else(|_| "cmd.exe".to_owned())
                .into(),
            args: Vec::new(),
            login: false,
            cwd: dir.path().to_owned(),
            env,
            cols: 80,
            rows: 24,
            record: false,
        })
        .unwrap();
    let shell_pid = opened
        .pid
        .expect("the terminal exposes its process identity");
    tokio::time::sleep(START_TIME_RESOLUTION).await;
    let screen = terminals.attach(id, true).unwrap();
    let executable = std::env::current_exe().unwrap();
    screen
        .send_input(
            format!(
                "\"{}\" --exact native_process_fixture --nocapture\r\n",
                executable.display()
            )
            .into_bytes(),
        )
        .unwrap();
    let (parent_pid, child_pid) = tokio::task::spawn_blocking(move || wait_for_marker(&marker))
        .await
        .unwrap();
    let (shell_start, _) = start_of(&processes, shell_pid);
    let (parent_start, _) = start_of(&processes, parent_pid);
    let (child_start, _) = start_of(&processes, child_pid);
    let identities = vec![
        (shell_pid, shell_start),
        (parent_pid, parent_start),
        (child_pid, child_start),
    ];
    let _cleanup = Cleanup(identities.clone());
    drop(screen);
    terminals.close(id).await.unwrap();
    tokio::task::spawn_blocking(move || wait_ended(&identities))
        .await
        .unwrap();
    assert!(terminals.attach(id, true).is_err());
}
