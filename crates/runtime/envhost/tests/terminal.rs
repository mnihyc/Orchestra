//! Shells with real pseudo-terminals: they keep running with nobody attached, the next attachment gets the last output, one screen at a time, and an unattended end is reported.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use orch_envhost::blobs::MemoryBlobs;
#[cfg(unix)]
use orch_envhost::environment::{Report, Reports};
#[cfg(target_os = "linux")]
use orch_envhost::platform::{self, Sig};
#[cfg(target_os = "linux")]
use orch_envhost::processes::{Processes, ScopeKind};
use orch_envhost::terminal::{Attachment, Event, OpenSpec, Purpose, TerminalLinks, Terminals};
use orch_types::{Id, SystemClock};

const KEEP: usize = 256 * 1024;

fn spec(id: Id, cwd: &Path, record: bool) -> OpenSpec {
    OpenSpec {
        id,
        environment_id: orch_types::new_id(),
        purpose: Purpose::Shell,
        program: (if cfg!(windows) {
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
        } else {
            "/bin/sh".to_owned()
        })
        .into(),
        args: vec![],
        login: false,
        cwd: cwd.to_path_buf(),
        env: BTreeMap::new(),
        cols: 80,
        rows: 24,
        record,
    }
}

fn terminals(links: TerminalLinks) -> Terminals {
    Terminals::new(Arc::new(SystemClock), KEEP, links).unwrap()
}

/// Reads from an attachment until what has come holds `needle`; fails after ten seconds.
async fn read_until(attachment: &mut Attachment, seen: &mut String, needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !seen.contains(needle) {
        let event = tokio::time::timeout_at(deadline, attachment.next())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; saw {seen:?}"));
        match event {
            Some(Event::Data(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
            Some(other) => {
                panic!("unexpected {other:?} while waiting for {needle:?}; saw {seen:?}")
            }
            None => panic!("the attachment ended while waiting for {needle:?}; saw {seen:?}"),
        }
    }
}

fn type_line(attachment: &Attachment, line: &str) {
    let newline = if cfg!(windows) { "\r\n" } else { "\n" };
    attachment
        .send_input(format!("{line}{newline}").into_bytes())
        .unwrap();
}

async fn replay_until(terminals: &Terminals, id: Id, needle: &str) -> Attachment {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let attachment = terminals.attach(id, true).unwrap();
        if String::from_utf8_lossy(attachment.replay()).contains(needle) {
            return attachment;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the shell never printed {needle:?}"
        );
        drop(attachment);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_windows_shell_runs_in_its_directory_and_replays_output_after_detach() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    let id = orch_types::new_id();
    let mut open = spec(id, dir.path(), false);
    open.env.insert("ORCH_TEST_NUMBER".into(), "42".into());
    terminals.open(open).unwrap();
    let mut screen = terminals.attach(id, true).unwrap();
    screen.resize(100, 30).unwrap();
    type_line(&screen, "cd & echo native-%ORCH_TEST_NUMBER%");
    let mut seen = String::new();
    read_until(&mut screen, &mut seen, "native-42").await;
    assert!(
        seen.to_lowercase()
            .contains(&dir.path().to_string_lossy().to_lowercase()),
        "the shell starts in its requested directory: {seen}"
    );
    drop(screen);
    assert!(!terminals.list()[0].attached);
    let screen = replay_until(&terminals, id, "native-42").await;
    assert!(String::from_utf8_lossy(screen.replay()).contains("native-42"));
    drop(screen);
    terminals.close(id).await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_runs_commands_in_the_directory_it_was_opened_in_and_follows_a_resize() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), false)).unwrap();
    let mut screen = terminals.attach(id, true).unwrap();
    let mut seen = String::new();
    type_line(&screen, "pwd; echo sum-$((6*7))");
    read_until(&mut screen, &mut seen, "sum-42").await;
    assert!(
        seen.contains(&dir.path().canonicalize().unwrap().display().to_string()),
        "it starts where it was asked to: {seen}"
    );
    screen.resize(100, 30).unwrap();
    type_line(&screen, "stty size");
    read_until(&mut screen, &mut seen, "30 100").await;
    assert!(screen.resize(0, 10).is_err() && screen.resize(10, 5000).is_err());
    terminals.close(id).await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_keeps_running_with_nobody_attached_and_the_next_attachment_gets_what_it_printed() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), false)).unwrap();
    let screen = terminals.attach(id, true).unwrap();
    type_line(&screen, "sleep 0.6; echo finished-$((40+2))");
    drop(screen); // the tab is closed while the command runs
    let info = terminals.list().into_iter().next().unwrap();
    assert!(!info.attached && info.detached_since.is_some());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        terminals.activity().busy == 1,
        "a command is running in it, with nobody to see"
    );
    let screen = replay_until(&terminals, id, "finished-42").await;
    assert!(String::from_utf8_lossy(screen.replay()).contains("finished-42"));
    assert_eq!(
        terminals.list()[0].detached_since,
        None,
        "now somebody is attached again"
    );
    drop(screen);
    terminals.close(id).await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_attachment_takes_the_shell_from_the_first_which_is_told_and_can_no_longer_type() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), false)).unwrap();
    let mut first = terminals.attach(id, true).unwrap();
    let mut seen = String::new();
    type_line(&first, "echo one-$((1+1))");
    read_until(&mut first, &mut seen, "one-2").await;

    let mut second = terminals.attach(id, true).unwrap();
    assert!(
        String::from_utf8_lossy(second.replay()).contains("one-2"),
        "the screen is as it was"
    );
    // The first hears that it was taken, and its typing goes nowhere.
    // (What the shell printed before the change may still be on its way to the first; then it is told.)
    let told = loop {
        match tokio::time::timeout(Duration::from_secs(5), first.next())
            .await
            .unwrap()
        {
            Some(Event::Data(_)) => {}
            other => break other,
        }
    };
    assert_eq!(told, Some(Event::Taken));
    assert_eq!(first.next().await, None, "and that is the end of it");
    assert!(first.send_input(b"echo stolen\n".to_vec()).is_err());
    assert!(first.resize(10, 10).is_err());
    // The second works the shell, and the shell was not touched by the change.
    let mut seen2 = String::new();
    type_line(&second, "echo two-$((2+2))");
    read_until(&mut second, &mut seen2, "two-4").await;
    assert!(!seen2.contains("stolen"));
    drop(first);
    assert!(
        terminals.list()[0].attached,
        "the first letting go does not detach the second"
    );
    drop(second);
    terminals.close(id).await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attachment_that_only_watches_sees_the_screen_and_types_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), false)).unwrap();
    let mut owner = terminals.attach(id, true).unwrap();
    let mut seen = String::new();
    type_line(&owner, "echo watched-$((3*3))");
    read_until(&mut owner, &mut seen, "watched-9").await;
    let viewer = terminals.attach(id, false).unwrap();
    assert!(String::from_utf8_lossy(viewer.replay()).contains("watched-9"));
    assert!(viewer.send_input(b"rm -rf /\n".to_vec()).is_err());
    drop(viewer);
    terminals.close(id).await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_that_printed_more_keeps_its_last_256_kib_and_no_more() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), false)).unwrap();
    let screen = terminals.attach(id, true).unwrap();
    type_line(
        &screen,
        "yes abcdefghijklmnopqrstuvwxyz | head -c 900000; echo; echo end-of-output",
    );
    drop(screen);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let screen = loop {
        let attachment = terminals.attach(id, true).unwrap();
        let replay = attachment.replay();
        let tail =
            String::from_utf8_lossy(&replay[replay.len().saturating_sub(200)..]).into_owned();
        // The prompt that follows the last command is part of the screen too.
        if tail.contains("end-of-output") && replay.len() == KEEP {
            break attachment;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the output never arrived whole: {} bytes kept",
            attachment.replay().len()
        );
        drop(attachment);
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(screen.replay().len(), KEEP, "exactly the last 256 KiB");
    assert!(
        terminals.list()[0].bytes_out > 900_000,
        "but everything was counted: {}",
        terminals.list()[0].bytes_out
    );
    drop(screen);
    terminals.close(id).await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_that_exits_while_attached_tells_its_attachment_how() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), false)).unwrap();
    let mut screen = terminals.attach(id, true).unwrap();
    type_line(&screen, "echo goodbye; exit 3");
    let mut seen = String::new();
    read_until(&mut screen, &mut seen, "goodbye").await;
    let exit = loop {
        match tokio::time::timeout(Duration::from_secs(5), screen.next())
            .await
            .unwrap()
        {
            Some(Event::Exit { code }) => break code,
            Some(Event::Data(_)) => {}
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(exit, Some(3));
    let closed = terminals.close(id).await.unwrap();
    assert_eq!(closed.exit_code, Some(3));
    assert!(
        closed.bytes_out > 0 && closed.kept.is_none(),
        "nothing is kept where it was not asked for"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_that_exits_with_nobody_attached_is_reported_with_what_it_printed_last_and_closed_with_the_same()
 {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryBlobs::new();
    let (reports, mut inbox) = Reports::channel(16);
    let terminals = terminals(TerminalLinks {
        reports: Some(reports),
        blobs: Some(Arc::new(store.clone())),
        ..TerminalLinks::default()
    });
    let id = orch_types::new_id();
    let mut open = spec(id, dir.path(), true);
    let environment_id = open.environment_id;
    open.record = true;
    terminals.open(open).unwrap();
    let screen = terminals.attach(id, true).unwrap();
    type_line(&screen, "echo last-words; sleep 0.4; exit 5");
    drop(screen);

    let reported = tokio::time::timeout(Duration::from_secs(10), inbox.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reported.environment_id, environment_id);
    let Report::PtyExited(exited) = reported.report else {
        panic!("{:?}", reported.report)
    };
    assert_eq!((exited.terminal_id, exited.exit_code), (id, Some(5)));
    let kept = exited
        .kept
        .expect("the last of its output is kept where that was asked for");
    let text = String::from_utf8_lossy(&store.get_bytes(&kept.hash).unwrap()).into_owned();
    assert!(text.contains("last-words"), "{text}");

    let closed = terminals.close(id).await.unwrap();
    assert_eq!(closed.exit_code, Some(5));
    assert_eq!(
        closed.kept,
        Some(kept),
        "the answer to the close names the same blob"
    );
    assert_eq!(closed.bytes_out, exited.bytes_out);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_shell_ends_it_and_everything_it_started_and_keeps_its_output_only_if_asked() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryBlobs::new();
    let processes = Processes::new(ScopeKind::Started, Arc::new(SystemClock));
    let terminals = terminals(TerminalLinks {
        processes: Some(processes),
        blobs: Some(Arc::new(store.clone())),
        ..TerminalLinks::default()
    });
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), true)).unwrap();
    let mut screen = terminals.attach(id, true).unwrap();
    type_line(&screen, "sleep 300 & echo started-$!");
    let mut seen = String::new();
    // The typed line is echoed with "$!" in it; the number is what the shell prints after it ran.
    let number = regex::Regex::new(r"started-(\d+)\r?\n").unwrap();
    let pid: u32 = loop {
        if let Some(found) = number.captures(&seen) {
            break found[1].parse().unwrap();
        }
        let event = tokio::time::timeout(Duration::from_secs(10), screen.next())
            .await
            .unwrap();
        match event {
            Some(Event::Data(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
            other => panic!("{other:?}; saw {seen:?}"),
        }
    };
    let sleeper = format!("/proc/{pid}/cmdline");
    // (A process just forked still shows the shell's command line until it has executed `sleep`.)
    let mut is_sleeper = false;
    for _ in 0..100 {
        if std::fs::read_to_string(&sleeper)
            .unwrap_or_default()
            .starts_with("sleep")
        {
            is_sleeper = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(is_sleeper, "the number is a sleeper the shell started");
    drop(screen);

    let closed = terminals.close(id).await.unwrap();
    assert!(closed.kept.is_some(), "it was opened to be kept");
    let mut gone = false;
    for _ in 0..100 {
        let state = platform::os::state_of(pid);
        if state.is_none() || state == Some('Z') {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if !gone {
        // Never leave it running, whatever the assertion says.
        let _ = platform::send_signal(pid, None, Sig::Kill);
    }
    assert!(gone, "what the shell started ends with it");
    assert!(
        terminals.close(id).await.is_err(),
        "a closed shell is not there to close again"
    );
    assert!(terminals.attach(id, true).is_err());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_shell_with_a_command_running_counts_as_busy() {
    // Returning the foreground to the shell may be delayed by other builds;
    // this guard prevents hanging and does not establish whether it is busy.
    const PATIENCE: Duration = Duration::from_secs(300);
    // Poll the real foreground until the held command has relinquished it.
    const POLL: Duration = Duration::from_millis(25);
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), false)).unwrap();
    let mut screen = terminals.attach(id, true).unwrap();
    let mut seen = String::new();
    // The answer differs from the echoed input, so seeing it proves execution.
    type_line(&screen, "echo ready-$((6*7))");
    read_until(&mut screen, &mut seen, "ready-42").await;
    assert_eq!(
        terminals.activity().busy,
        0,
        "at its prompt it holds nothing up"
    );
    // The foreground child stays alive until this test releases it. Scheduling
    // cannot make a one-second command finish before we observe it as busy.
    type_line(
        &screen,
        "sh -c 'echo command-start-$((6*7)); read release; echo command-end-$((6*7))'",
    );
    read_until(&mut screen, &mut seen, "command-start-42").await;
    assert_eq!(
        terminals.activity().busy,
        1,
        "with a command running it does"
    );
    assert_eq!(terminals.activity().open, 1);
    type_line(&screen, "continue");
    read_until(&mut screen, &mut seen, "command-end-42").await;
    tokio::time::timeout(PATIENCE, async {
        while terminals.activity().busy != 0 {
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .expect("the completed command returns the foreground to the shell");
    assert_eq!(terminals.activity().busy, 0);
    drop(screen);
    terminals.close(id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shells_that_were_open_when_the_connector_stopped_are_found_again_to_be_reported_gone() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks {
        data_dir: Some(data.path().to_path_buf()),
        ..TerminalLinks::default()
    });
    let (first, second) = (orch_types::new_id(), orch_types::new_id());
    terminals.open(spec(first, dir.path(), false)).unwrap();
    terminals.open(spec(second, dir.path(), false)).unwrap();
    terminals.close(second).await.unwrap();
    // The connector goes away without closing the first.
    let lost = Terminals::recover(data.path());
    assert_eq!(lost, vec![first]);
    assert!(
        Terminals::recover(data.path()).is_empty(),
        "each is reported once"
    );
    terminals.close_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_cannot_be_opened_in_a_directory_that_is_not_there_or_with_a_screen_of_no_size() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = terminals(TerminalLinks::default());
    assert!(
        terminals
            .open(spec(
                orch_types::new_id(),
                &dir.path().join("missing"),
                false
            ))
            .is_err()
    );
    let mut flat = spec(orch_types::new_id(), dir.path(), false);
    flat.rows = 0;
    assert!(terminals.open(flat).is_err());
    let id = orch_types::new_id();
    terminals.open(spec(id, dir.path(), false)).unwrap();
    assert!(
        terminals.open(spec(id, dir.path(), false)).is_err(),
        "an identifier is opened once"
    );
    terminals.close(id).await.unwrap();
}
