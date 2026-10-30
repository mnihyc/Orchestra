//! Processes of an environment, with real processes: a tree is stopped whole, a signal reaches only the process it names, nothing outside the scope is touched.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orch_envhost::platform::{self, Sig};
use orch_envhost::processes::{Processes, RootKind, ScopeKind};
use orch_executor_contract::descriptor::Attribution;
use orch_link_contract::commands::Signal;
use orch_types::{SystemClock, Time};

/// Every process a test starts, ended when the test ends whatever happened: each was started in a
/// group of its own by the test, and only those groups and numbers are ever signalled.
struct Started {
    groups: Vec<u32>,
    pids: Vec<u32>,
    children: Vec<Child>,
}

impl Started {
    fn new() -> Self {
        Self {
            groups: Vec::new(),
            pids: Vec::new(),
            children: Vec::new(),
        }
    }

    /// A process outside any tracking, in a group of its own.
    fn bystander(&mut self) -> u32 {
        let mut command = Command::new("sleep");
        command
            .arg("300")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        platform::prepare_group(&mut command);
        let child = command.spawn().unwrap();
        let pid = child.id();
        self.groups.push(pid);
        self.children.push(child);
        pid
    }

    fn adopt(&mut self, child: Child) -> u32 {
        let pid = child.id();
        self.groups.push(pid);
        self.children.push(child);
        pid
    }

    fn note_pids(&mut self, file: &Path) -> Vec<u32> {
        let found = read_pids(file);
        self.pids.extend(found.iter().copied());
        found
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        for group in &self.groups {
            let _ = platform::send_group(*group, Sig::Kill);
        }
        for pid in &self.pids {
            // Only numbers a script of the test printed; each is checked to be a sleep it started.
            if is_sleep_of_the_test(*pid) {
                let _ = platform::send_signal(*pid, None, Sig::Kill);
            }
        }
        for child in &mut self.children {
            let _ = child.wait();
        }
    }
}

fn is_sleep_of_the_test(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
        .is_ok_and(|text| text.starts_with("sleep\x00300"))
}

fn read_pids(file: &Path) -> Vec<u32> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn alive(pid: u32) -> bool {
    matches!(platform::os::state_of(pid), Some(state) if state != 'Z' && state != 'X')
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn processes(scope: ScopeKind) -> Processes {
    Processes::new(scope, Arc::new(SystemClock))
}

fn script(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("script.sh");
    std::fs::write(&path, body).unwrap();
    path
}

fn start_script(procs: &Processes, started: &mut Started, dir: &Path, body: &str) -> u32 {
    let path = script(dir, body);
    let mut command = Command::new("sh");
    command
        .arg(path)
        .env("D", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = procs
        .spawn_tracked(&mut command, RootKind::Executor)
        .unwrap();
    started.adopt(child)
}

/// A shell that starts sleepers in its group, in a subshell, and in a session of their own, and
/// writes their numbers down.
const TREE: &str = r#"
sleep 300 &
echo $! >> "$D/pids"
( sleep 300 & echo $! >> "$D/pids"; wait ) &
setsid sleep 300 &
echo $! >> "$D/pids"
wait
"#;

#[tokio::test]
async fn a_process_tree_is_stopped_whole_including_what_left_its_group_and_what_was_orphaned() {
    let dir = tempfile::tempdir().unwrap();
    let procs = processes(ScopeKind::Started);
    let mut started = Started::new();
    let bystander = started.bystander();
    let root = start_script(&procs, &mut started, dir.path(), TREE);
    wait_until("the tree to start", || {
        read_pids(&dir.path().join("pids")).len() >= 3
    });
    let members = started.note_pids(&dir.path().join("pids"));
    assert!(members.iter().all(|pid| alive(*pid)));

    // The sleeper in a session of its own is found by its parent; one more is orphaned first, by
    // ending the subshell that started it, which the list sees before it happens.
    let listed: Vec<u32> = procs.list().iter().map(|p| p.info.pid).collect();
    for pid in &members {
        assert!(
            listed.contains(pid),
            "{pid} is part of the environment: {listed:?}"
        );
    }
    assert!(
        !listed.contains(&bystander),
        "a process that was not started for the environment is not listed"
    );

    let report = procs
        .stop_tree(root, Duration::from_millis(500))
        .await
        .unwrap();
    assert!(
        report.members >= 5,
        "the shell, three sleepers and a subshell: {report:?}"
    );
    for pid in &members {
        wait_until("a member to end", || !alive(*pid));
    }
    assert!(alive(bystander), "a process outside the tree is left alone");
}

#[tokio::test]
async fn a_tree_that_ignores_the_request_to_end_is_killed_after_the_grace() {
    let dir = tempfile::tempdir().unwrap();
    let procs = processes(ScopeKind::Started);
    let mut started = Started::new();
    let body = r#"
trap '' TERM
sleep 300 &
echo $! >> "$D/pids"
while :; do sleep 1; done
"#;
    let root = start_script(&procs, &mut started, dir.path(), body);
    wait_until("the shell to start", || {
        !read_pids(&dir.path().join("pids")).is_empty()
    });
    started.note_pids(&dir.path().join("pids"));
    let begun = Instant::now();
    let report = procs
        .stop_tree(root, Duration::from_millis(300))
        .await
        .unwrap();
    assert!(report.killed, "TERM was ignored, so the tree was killed");
    assert!(
        begun.elapsed() >= Duration::from_millis(300),
        "but only after the grace"
    );
    wait_until("the shell to end", || !alive(root));
}

#[tokio::test]
async fn a_signal_names_the_process_by_number_and_start_and_only_the_right_pair_is_signalled() {
    let dir = tempfile::tempdir().unwrap();
    let procs = processes(ScopeKind::Started);
    let mut started = Started::new();
    let root = start_script(
        &procs,
        &mut started,
        dir.path(),
        "sleep 300 &\necho $! >> \"$D/pids\"\nwhile :; do sleep 0.2; done\n",
    );
    wait_until("the sleeper", || {
        !read_pids(&dir.path().join("pids")).is_empty()
    });
    let sleeper = started.note_pids(&dir.path().join("pids"))[0];
    let listed = procs.list();
    let entry = listed.iter().find(|p| p.info.pid == sleeper).unwrap();

    // The number is right and the start is another: the number names another process now.
    let wrong = Time::new(entry.info.started_at.get() + chrono::Duration::seconds(7));
    assert!(
        procs
            .signal(i64::from(sleeper), wrong, Signal::Kill)
            .is_err()
    );
    assert!(
        alive(sleeper),
        "a signal with the wrong start reaches nothing"
    );
    // A start that is the right one for another process does not do either.
    let other = listed.iter().find(|p| p.info.pid == root).unwrap();
    if other.info.started_at != entry.info.started_at {
        assert!(
            procs
                .signal(i64::from(sleeper), other.info.started_at, Signal::Term)
                .is_err()
        );
        assert!(alive(sleeper));
    }

    procs
        .signal(i64::from(sleeper), entry.info.started_at, Signal::Term)
        .unwrap();
    wait_until("the sleeper to end", || !alive(sleeper));
    assert!(alive(root), "its parent was not asked to end");
}

#[tokio::test]
async fn nothing_outside_the_environment_is_signalled_and_the_numbers_that_mean_everybody_are_refused()
 {
    let procs = processes(ScopeKind::Started);
    let mut started = Started::new();
    let bystander = started.bystander();
    let start = platform::os::exact_start_ms(bystander).unwrap();
    let started_at = Time::new(chrono::DateTime::from_timestamp_millis(start).unwrap());
    // The right number and the right start, but the process was not started for the environment.
    assert!(
        procs
            .signal(i64::from(bystander), started_at, Signal::Kill)
            .is_err()
    );
    assert!(alive(bystander));
    for refused in [0i64, 1, -1, -2, i64::MAX, i64::from(std::process::id())] {
        let result = procs.signal(refused, started_at, Signal::Kill);
        assert!(result.is_err(), "{refused} must be refused");
    }
    // Even with the machine's whole unit as the scope, the host itself is not signalled.
    let everything = processes(ScopeKind::All);
    everything.protect(std::process::id()).unwrap();
    assert!(
        everything
            .list()
            .iter()
            .all(|p| p.info.pid != std::process::id()),
        "the host is not listed"
    );
    assert!(
        everything
            .signal(i64::from(std::process::id()), started_at, Signal::Term)
            .is_err()
    );
    assert!(everything.signal(1, started_at, Signal::Term).is_err());
    assert!(alive(bystander) && alive(std::process::id()));
}

#[tokio::test]
async fn a_process_group_is_signalled_only_if_the_environment_started_it() {
    let own = platform::own_process_group().unwrap();
    assert!(
        platform::send_group(own, Sig::Kill).is_err(),
        "the host's own group is never signalled"
    );
    assert!(platform::send_group(1, Sig::Kill).is_err());
    assert!(platform::send_group(0, Sig::Kill).is_err());
    assert!(platform::send_signal(0, None, Sig::Kill).is_err());
    assert!(platform::send_signal(1, None, Sig::Kill).is_err());
    assert!(platform::send_signal(std::process::id(), None, Sig::Kill).is_err());
}

#[tokio::test]
async fn a_restarted_host_resumes_only_saved_owned_identities_and_a_second_resume_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join("private/held.json");
    let procs =
        Processes::with_store(ScopeKind::Started, Arc::new(SystemClock), journal.clone()).unwrap();
    let mut started = Started::new();
    let bystander = started.bystander();
    let root = start_script(
        &procs,
        &mut started,
        dir.path(),
        r#"
i=0
while :; do i=$((i+1)); echo $i > "$D/count"; sleep 0.02; done
"#,
    );
    let counter = || {
        std::fs::read_to_string(dir.path().join("count"))
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    wait_until("the owned counter", || counter() > 3);
    assert!(procs.suspend().unwrap() >= 1);
    assert!(procs.suspend().unwrap() >= 1);
    let frozen = counter();
    drop(procs);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(counter(), frozen);
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    // Even a saved number with its real start is not ownership of an unrelated process.
    saved["held"].as_array_mut().unwrap().push(serde_json::json!({"pid":bystander,"start_ms":platform::os::exact_start_ms(bystander).unwrap()}));
    std::fs::write(&journal, serde_json::to_vec(&saved).unwrap()).unwrap();
    let recovered =
        Processes::with_store(ScopeKind::Started, Arc::new(SystemClock), journal.clone()).unwrap();
    assert!(recovered.resume().unwrap() >= 1);
    wait_until("the recovered counter to run", || counter() > frozen + 3);
    assert_eq!(recovered.resume().unwrap(), 0);
    assert!(matches!(platform::os::state_of(bystander), Some(state) if state != 'T'));
    recovered.suspend().unwrap();
    drop(recovered);
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    let original_start = saved["held"]
        .as_array()
        .unwrap()
        .iter()
        .find(|held| held["pid"] == root)
        .unwrap()["start_ms"]
        .as_i64()
        .unwrap();
    for held in saved["held"].as_array_mut().unwrap() {
        if held["pid"] == root {
            held["start_ms"] = serde_json::json!(original_start + 1);
        }
    }
    std::fs::write(&journal, serde_json::to_vec(&saved).unwrap()).unwrap();
    let reused = Processes::with_store(ScopeKind::Started, Arc::new(SystemClock), journal).unwrap();
    reused.resume().unwrap();
    assert_eq!(
        platform::os::state_of(root),
        Some('T'),
        "a different start must never receive CONT"
    );
    platform::send_signal(root, Some(original_start), Sig::Continue).unwrap();
}

#[tokio::test]
async fn the_processes_an_environment_started_are_held_still_and_let_go_and_others_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let procs = processes(ScopeKind::Started);
    let mut started = Started::new();
    let bystander = started.bystander();
    let counter = r#"
i=0
while :; do i=$((i+1)); echo $i > "$D/count"; sleep 0.02; done
"#;
    let root = start_script(&procs, &mut started, dir.path(), counter);
    let read_count = || {
        std::fs::read_to_string(dir.path().join("count"))
            .ok()
            .and_then(|t| t.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    wait_until("the counter to run", || read_count() > 3);

    let held = procs.suspend().unwrap();
    assert!(held >= 1, "at least the shell: {held}");
    std::thread::sleep(Duration::from_millis(100));
    let frozen_at = read_count();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        read_count(),
        frozen_at,
        "the files stand still while the processes are held"
    );
    assert_eq!(platform::os::state_of(root), Some('T'));
    assert!(
        matches!(platform::os::state_of(bystander), Some(state) if state != 'T'),
        "a process the environment did not start is not held"
    );

    let released = procs.resume().unwrap();
    assert!(released >= 1);
    wait_until("the counter to run again", || read_count() > frozen_at + 3);
}

#[tokio::test]
async fn a_process_carries_the_thread_and_the_tool_call_its_executor_started_it_for_and_so_do_its_children()
 {
    let dir = tempfile::tempdir().unwrap();
    let procs = processes(ScopeKind::Started);
    procs.add_attribution(Attribution {
        thread_id: "ORCH_THREAD_ID".into(),
        tool_call_id: "ORCH_ITEM_ID".into(),
    });
    procs.add_attribution(Attribution {
        thread_id: "CODEX_THREAD_ID".into(),
        tool_call_id: "CODEX_TOOL_CALL_ID".into(),
    });
    let thread = orch_types::new_id();
    let item = orch_types::new_id();
    let other_thread = orch_types::new_id();
    let mut started = Started::new();

    let path = script(dir.path(), "sleep 300 &\necho $! >> \"$D/pids\"\nwait\n");
    let mut command = Command::new("sh");
    command
        .arg(&path)
        .env("D", dir.path())
        .env("ORCH_THREAD_ID", thread.to_string())
        .env("ORCH_ITEM_ID", item.to_string());
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let first = started.adopt(
        procs
            .spawn_tracked(&mut command, RootKind::Executor)
            .unwrap(),
    );

    let other_dir = tempfile::tempdir().unwrap();
    let path2 = other_dir.path().join("script.sh");
    std::fs::write(&path2, "sleep 300 &\necho $! >> \"$D/pids\"\nwait\n").unwrap();
    let mut second = Command::new("sh");
    second
        .arg(&path2)
        .env("D", other_dir.path())
        .env("CODEX_THREAD_ID", other_thread.to_string());
    second
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let second_pid = started.adopt(
        procs
            .spawn_tracked(&mut second, RootKind::Executor)
            .unwrap(),
    );

    wait_until("both sleepers", || {
        !read_pids(&dir.path().join("pids")).is_empty()
            && !read_pids(&other_dir.path().join("pids")).is_empty()
    });
    let child_of_first = started.note_pids(&dir.path().join("pids"))[0];
    started.note_pids(&other_dir.path().join("pids"));
    let listed = procs.list();
    let find = |pid: u32| listed.iter().find(|p| p.info.pid == pid).unwrap();
    assert_eq!(find(first).info.thread_id, Some(thread));
    assert_eq!(find(first).info.item_id, Some(item));
    assert_eq!(
        find(child_of_first).info.thread_id,
        Some(thread),
        "what the shell started has the shell's ids"
    );
    assert_eq!(find(child_of_first).info.item_id, Some(item));
    assert_eq!(
        find(second_pid).info.thread_id,
        Some(other_thread),
        "an executor with other variable names is read as it declares"
    );
    assert_eq!(
        find(second_pid).info.item_id,
        None,
        "and a variable that is not set says nothing"
    );

    // Ordered for a thread, its own processes come first.
    let ordered = Processes::ordered_for(listed.clone(), Some(other_thread));
    assert_eq!(ordered[0].info.thread_id, Some(other_thread));
}
