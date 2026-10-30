//! A harness process that is killed, ends by itself, prints half a line, goes silent, tries again without end, or refuses to start: the turn ends with what was last seen, the next turn starts a fresh process that takes the session up, and nothing waits.
//!
//! The real harness cannot be made to do these things on purpose, so a program stands in for it: it answers
//! the version and the handshake the way the harness does, and does what the first word of the person's
//! message says. It counts its own starts in a file beside it. Each test has a copy of its own, all of them
//! written before any test runs, because a program that is being written while another thread starts a
//! process cannot be started (the file is busy).

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use common::{Rig, kinds, outcome};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use serde_json::json;

/// The program that stands in for the harness. It reads one line at a time; the handshake is answered, and
/// a message of the person is answered as its first word asks. `mode` is the word.
const STAND_IN: &str = r#"#!/bin/sh
case "$1" in --version) echo '2.1.296 (Claude Code)'; exit 0;; esac
echo started >> "$0.starts"
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=$(printf '%s' "$line" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      case "$line" in
        *KILLED*) printf '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}},"api_message_id":"m1"}\n'; kill -9 $$ ;;
        *IDLEEXIT*) printf '{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","result":"fine","usage":{"input_tokens":1,"output_tokens":1}}\n'; sleep 0.2; echo 'idle server final diagnostic' >&2; exit 7 ;;
        *GONE*) printf '{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"I was saying"}]}}\n'; exit 0 ;;
        *HALFSILENT*) printf '{"type":"assistant","mess'; exec sleep 100000 ;;
        *HALF*) printf '{"type":"assistant","message":{"id":"m1","cont'; exit 3 ;;
        *SILENT*) exec sleep 100000 ;;
        *TRYING*) n=0; while :; do n=$((n+1)); printf '{"type":"system","subtype":"api_retry","attempt":%d,"max_retries":99,"retry_delay_ms":250,"error_status":null,"error":"unknown"}\n' "$n"; sleep 0.25; done ;;
        *) printf '{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","result":"fine","usage":{"input_tokens":1,"output_tokens":1}}\n' ;;
      esac
      ;;
  esac
done
echo stopped >> "$0.stops"
"#;

/// The same program, for a harness that refuses to start.
const REFUSING: &str = r#"#!/bin/sh
case "$1" in --version) echo '2.1.296 (Claude Code)'; exit 0;; esac
echo 'Error: the configuration directory is read-only' >&2
exit 1
"#;

/// The programs, one for each test, written once before any is started.
fn programs() -> &'static (tempfile::TempDir, Vec<(&'static str, PathBuf)>) {
    static PROGRAMS: OnceLock<(tempfile::TempDir, Vec<(&'static str, PathBuf)>)> = OnceLock::new();
    PROGRAMS.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let mut programs = Vec::new();
        for name in [
            "killed",
            "gone",
            "half",
            "silent",
            "halfsilent",
            "trying",
            "idle-unexpected",
            "idle-planned",
        ] {
            programs.push((name, write(&directory, name, STAND_IN)));
        }
        programs.push(("refusing", write(&directory, "refusing", REFUSING)));
        (directory, programs)
    })
}

fn write(directory: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
    let path = directory.path().join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn program(name: &str) -> PathBuf {
    programs()
        .1
        .iter()
        .find(|(given, _)| *given == name)
        .map(|(_, path)| path.clone())
        .unwrap()
}

/// How often the program of a test was started.
fn starts(name: &str) -> usize {
    std::fs::read_to_string(format!("{}.starts", program(name).display()))
        .map_or(0, |text| text.lines().count())
}

/// A rig on the stand-in of a test, started with the options a profile may state.
async fn rig(name: &str, config: serde_json::Value) -> Rig {
    let mut rig = Rig::standing_in(&program(name)).await;
    rig.start(config, std::collections::BTreeMap::new()).await;
    rig.open().await;
    rig
}

/// What the turn ended with: the events up to its end and its error.
fn error_of(events: &[SessionEvent]) -> orch_agent_contract::events::TurnError {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::TurnCompleted(ended) => ended.error.clone(),
            _ => None,
        })
        .expect("the turn ends with an error")
}

#[tokio::test]
async fn a_process_that_is_killed_in_the_middle_of_a_turn_ends_it_and_the_next_turn_starts_a_fresh_one()
 {
    let mut rig = rig("killed", json!({})).await;
    let turn = rig.turn("KILLED");
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig
        .until(|event| matches!(event, SessionEvent::SessionLost(_)), 30)
        .await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionLost(lost) if lost.reason.contains("signal"))),
        "the loss says how the process ended: {:?}",
        kinds(&events)
    );
    assert_eq!(starts("killed"), 1);
    // The next turn starts a process of its own, which takes the session up.
    let (_, events) = rig.run("fine").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert_eq!(starts("killed"), 2);
}

#[tokio::test]
async fn a_process_that_ends_by_itself_without_a_result_loses_the_session_with_what_it_last_said() {
    let mut rig = rig("gone", json!({})).await;
    let turn = rig.turn("GONE");
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig
        .until(|event| matches!(event, SessionEvent::SessionLost(_)), 30)
        .await;
    assert!(
        events.iter().any(|event| matches!(event,
            SessionEvent::ItemCompleted(item) if item.payload["text"] == "I was saying")),
        "what the harness had said is kept: {:?}",
        kinds(&events)
    );
    assert!(
        events.iter().any(|event| matches!(event, SessionEvent::SessionLost(lost) if lost.reason.contains("status 0"))),
        "{:?}",
        events.last()
    );
    let (_, events) = rig.run("fine").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert_eq!(starts("gone"), 2);
}

#[tokio::test]
async fn half_a_line_and_an_end_is_a_lost_session_and_the_half_line_is_not_a_frame() {
    let mut rig = rig("half", json!({})).await;
    let turn = rig.turn("HALF");
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig
        .until(|event| matches!(event, SessionEvent::SessionLost(_)), 30)
        .await;
    assert!(
        events.iter().all(|event| !matches!(
            event,
            SessionEvent::ItemStarted(_) | SessionEvent::TurnCompleted(_)
        )),
        "nothing is made of half a line: {:?}",
        kinds(&events)
    );
    let (_, events) = rig.run("fine").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
}

#[tokio::test]
async fn a_harness_that_says_nothing_past_its_deadline_has_its_turn_ended_and_is_stopped() {
    let mut rig = rig("silent", json!({ "silence_secs": 1 })).await;
    let began = Instant::now();
    let (_, events) = rig.run("SILENT").await;
    assert!(
        began.elapsed() < Duration::from_secs(20),
        "nothing waits for ever"
    );
    assert_eq!(outcome(&events), TurnOutcome::Failed);
    let error = error_of(&events);
    assert_eq!(error.code, "harness_error");
    assert!(error.retryable);
    assert!(
        error
            .said
            .as_deref()
            .unwrap()
            .contains("no word from the harness"),
        "{error:?}"
    );
    // The next turn is a fresh process.
    let (_, events) = rig.run("fine").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert_eq!(starts("silent"), 2);
}

#[tokio::test]
async fn half_a_line_and_silence_after_it_is_silence() {
    let mut rig = rig("halfsilent", json!({ "silence_secs": 1 })).await;
    let (_, events) = rig.run("HALFSILENT").await;
    assert_eq!(outcome(&events), TurnOutcome::Failed);
    assert_eq!(error_of(&events).code, "harness_error");
}

#[tokio::test]
async fn a_harness_that_goes_on_trying_again_is_stopped_when_its_patience_is_out() {
    // The harness is heard every quarter of a second, so only the patience for trying again can end this.
    let mut rig = rig(
        "trying",
        json!({ "silence_secs": 0, "retry_patience_secs": 1 }),
    )
    .await;
    let began = Instant::now();
    let (_, events) = rig.run("TRYING").await;
    assert!(began.elapsed() < Duration::from_secs(20));
    assert_eq!(outcome(&events), TurnOutcome::Failed);
    let error = error_of(&events);
    assert_eq!(error.code, "provider_unreachable");
    assert!(error.retryable);
    let said = error.said.unwrap();
    assert!(
        said.contains("announced") && said.contains("no answer (unknown)"),
        "{said}"
    );
    let (_, events) = rig.run("fine").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
}

#[tokio::test]
async fn a_harness_that_refuses_to_start_fails_the_turn_at_once_with_what_it_printed() {
    let mut rig = Rig::standing_in(&program("refusing")).await;
    rig.start(json!({}), std::collections::BTreeMap::new())
        .await;
    // The session opens (nothing runs until a turn), and the turn finds out.
    let spec = rig.spec(orch_agent_contract::session::SessionMode::New, None);
    rig.adapter.open_session(spec).await.unwrap();
    let turn = rig.turn("hello");
    let began = Instant::now();
    let error = rig.adapter.start_turn(turn).await.unwrap_err();
    assert!(
        began.elapsed() < Duration::from_secs(20),
        "nothing waits for ever"
    );
    assert!(error.to_string().contains("read-only"), "{error}");
}

#[tokio::test]
async fn an_unexpected_idle_exit_says_its_reason_and_the_next_turn_resumes() {
    let mut rig = rig("idle-unexpected", json!({})).await;
    let (_, completed) = rig.run("IDLEEXIT").await;
    assert_eq!(outcome(&completed), TurnOutcome::Completed);
    let lost = rig
        .until(|event| matches!(event, SessionEvent::SessionLost(_)), 30)
        .await;
    assert!(lost.iter().any(|event| matches!(event, SessionEvent::SessionLost(lost) if lost.reason.contains("status 7") && lost.reason.contains("idle server final diagnostic"))));
    let (_, next) = rig.run("fine").await;
    assert_eq!(outcome(&next), TurnOutcome::Completed);
    assert_eq!(starts("idle-unexpected"), 2);
}

#[tokio::test]
async fn an_idle_shutdown_requested_by_orch_is_not_reported_as_an_unexpected_exit() {
    let name = "idle-planned";
    let mut rig = Rig::standing_in(&program(name)).await;
    rig.start(
        json!({"idle_exit_secs":1}),
        std::collections::BTreeMap::new(),
    )
    .await;
    let mut spec = rig.spec(orch_agent_contract::session::SessionMode::New, None);
    spec.exec.executor = Some("test-executor".into());
    spec.exec.tool_set = Some("custom".into());
    rig.open_with(spec).await;
    let (_, completed) = rig.run("fine").await;
    assert_eq!(outcome(&completed), TurnOutcome::Completed);
    let stopped = PathBuf::from(format!("{}.stops", program(name).display()));
    tokio::time::timeout(Duration::from_secs(10), async {
        while !stopped.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Orch actually closed the idle process");
    while let Ok(event) = rig.events.try_recv() {
        assert!(!matches!(
            event,
            orch_agent_contract::adapter::AdapterEvent::Session {
                event: SessionEvent::SessionLost(_),
                ..
            }
        ));
    }
    rig.adapter.stop().await.unwrap();
}
