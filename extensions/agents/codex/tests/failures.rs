//! A model that fails, a stream that is cut, and a harness that dies, on the real Codex in Docker.
//!
//! The last test runs the faults of the recordings (`tests/fixtures/failures`, replayed without a container by
//! `recorded_failures.rs`) against the harness itself, so that a new version of it is held against the same
//! rows: a row whose code, advice to try again or mark of a cut answer differs from its fixture's expectation
//! fails the test with the row named (`CHANGED`); one whose words differ is only printed (`WORDS`), since no
//! mapping depends on a wording; and with `ORCH_RECORD_FAILURES=<directory>` each row is written there as
//! `codex-<row>.json` in the form of the fixtures, to be read and the fixture renewed. The image is
//! `ORCH_HARNESS_IMAGE` when that is set, else `orch-dev-harness`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, kinds, remove_containers, script, session_events};
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::events::{ItemContent, ItemStatus, SessionEvent, TurnOutcome, turn_error};
use orch_agent_contract::facts::HarnessState;
use orch_agent_contract::session::SessionMode;
use orch_types::new_id;

#[tokio::test]
async fn a_refusal_of_the_model_ends_the_turn_with_a_typed_failure() {
    let Some(mut rig) = Rig::start("refused").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    let text = script("go", &["error invalid_request the prompt is wrong"]);
    let events = rig.run_turn(thread, &text).await;
    let error_item = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemStarted(item) => match &item.content {
                ItemContent::Error(error) => Some(error),
                _ => None,
            },
            _ => None,
        })
        .expect("the error is on the timeline");
    assert_eq!(error_item.code, turn_error::HARNESS_ERROR);
    assert!(
        error_item.message.contains("the prompt is wrong"),
        "{}",
        error_item.message
    );
    let Some(SessionEvent::TurnCompleted(ended)) = events.last() else {
        panic!("the turn ends");
    };
    assert_eq!(ended.status, TurnOutcome::Failed);
    let failure = ended.error.as_ref().unwrap();
    assert_eq!(failure.code, turn_error::HARNESS_ERROR);
    assert!(!failure.retryable);
    assert!(failure.message.contains("the prompt is wrong"));
    // The session goes on after a failed turn.
    let events = rig.run_turn(thread, "are you there").await;
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn an_upstream_that_refuses_the_credential_is_reported_once_when_the_harness_first_says_so() {
    let Some(mut rig) = Rig::start("unauthorized").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    rig.mock.set_credential(Some("another-token"));
    let events = rig.run_turn(thread, "hello").await;
    let Some(SessionEvent::TurnCompleted(ended)) = events.last() else {
        panic!("the turn ends");
    };
    assert_eq!(ended.status, TurnOutcome::Failed);
    let failure = ended.error.as_ref().unwrap();
    assert_eq!(
        failure.code,
        turn_error::PROVIDER_UNAUTHORIZED,
        "{failure:?}"
    );
    let errors = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::ItemStarted(item) if matches!(item.content, ItemContent::Error(_))))
        .count();
    assert_eq!(
        errors,
        1,
        "the refusal has one typed error line: {:?}",
        kinds(&events)
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_stream_cut_in_the_middle_is_made_again_and_the_failed_attempt_is_left_cancelled() {
    let Some(mut rig) = Rig::start("cut").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    // The first request is answered with a text and then a broken connection; the harness asks again.
    rig.mock.queue("say a text that is cut short\ncut").unwrap();
    let events = rig.run_turn(thread, "tell me").await;
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(item)
                if matches!(item.content, ItemContent::AgentMessage(_)) =>
            {
                Some(item.native_ref.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        started.len(),
        2,
        "one item for each attempt: {:?}",
        kinds(&events)
    );
    let status_of = |reference: &str| {
        events
            .iter()
            .find_map(|event| match event {
                SessionEvent::ItemCompleted(item) if item.native_ref == reference => {
                    Some(item.status)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{reference} completes: {:?}", kinds(&events)))
    };
    assert_eq!(
        status_of(&started[0]),
        ItemStatus::Cancelled,
        "what the failed attempt streamed is not left open"
    );
    assert_eq!(status_of(&started[1]), ItemStatus::Completed);
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended))
        if ended.status == TurnOutcome::Completed && ended.error.is_none())
    );
    assert_eq!(rig.model_requests().len(), 2);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_harness_that_dies_is_reported_and_a_restart_recovers_the_conversation() {
    let Some(mut rig) = Rig::start("death").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    let first = rig.open_new(thread).await;
    rig.run_turn(thread, "remember the word banana").await;
    let long = script(
        "now wait",
        &[r#"tool exec_command {"cmd":"sleep 30","yield_time_ms":30000}"#],
    );
    let turn = rig.turn(thread, &long);
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until(|event| {
        matches!(event, AdapterEvent::Session { event: SessionEvent::ItemStarted(item), .. }
        if matches!(item.content, ItemContent::ToolCall(_)))
    })
    .await;

    // The process is killed from outside, in the middle of the turn.
    let name = rig.launcher.containers.lock().unwrap()[0].clone();
    remove_containers(&[name]);
    let events = rig
        .until(|event| matches!(event, AdapterEvent::Status(status) if status.status == HarnessState::Error))
        .await;
    let session = session_events(&events);
    assert!(
        session
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionLost(_))),
        "{:?}",
        kinds(&session)
    );
    assert!(
        session.iter().any(|event| matches!(event, SessionEvent::ItemCompleted(item) if item.status == ItemStatus::Cancelled)),
        "the command that was running ends: {:?}",
        kinds(&session)
    );
    let AdapterEvent::Status(status) = events.last().unwrap() else {
        panic!("the state of the harness is reported");
    };
    assert_eq!(status.sessions_open, 0);
    assert!(
        status
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("harness")),
        "{status:?}"
    );

    // Nothing can be asked of a harness that is gone.
    let error = rig
        .adapter
        .start_turn(rig.turn(thread, "hello"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "harness_unavailable");
    assert_eq!(
        rig.adapter.probe().await.unwrap_err().code(),
        "harness_unavailable"
    );

    // Started again, the harness serves the conversation it had.
    rig.boot().await;
    assert!(rig.adapter.probe().await.is_ok());
    let spec = rig.spec(thread, SessionMode::Resume, Some(&first.native_session_ref));
    let reopened = rig.adapter.open_session(spec).await.unwrap();
    assert!(reopened.resumed);
    rig.drain();
    let events = rig
        .run_turn(thread, "what word did I ask you to remember?")
        .await;
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );
    let requests = rig.model_requests();
    let last = requests.last().unwrap().body.to_string();
    assert!(
        last.contains("remember the word banana"),
        "the earlier conversation is carried over"
    );
    rig.adapter.stop().await.unwrap();
}

// ---------------------------------------------------------------------------------------------------------
// The recorded faults, against the harness itself

/// How far from the moment the server was told to give, in seconds, the moment the adapter reads may be: the
/// server counts the time from the request, and a row takes a while to reach it.
const MOMENT_SLACK_SECS: i64 = 600;

/// One fault: the fixture that holds what the harness once sent for it, what the model server is told to do,
/// and, for a limit with a time, how many seconds from now the server is told it ends.
struct Row {
    name: &'static str,
    fault: &'static str,
    ends_in: Option<i64>,
}

/// The faults that the harness gives up on after a try or a few; the ones it retries for minutes are left to
/// the recordings.
const ROWS: &[Row] = &[
    Row {
        name: "plan_limit_weekly_dated",
        fault: "error plan_limit window=weekly resets={ends} plan=pro",
        ends_in: Some(172_800),
    },
    Row {
        name: "plan_limit_session",
        fault: "error plan_limit window=session resets={ends} plan=plus",
        ends_in: Some(10_800),
    },
    Row {
        name: "plan_limit_without_a_time",
        fault: "error plan_limit window=session plan=plus",
        ends_in: None,
    },
    Row {
        name: "quota_ran_out",
        fault: "error billing",
        ends_in: None,
    },
    Row {
        name: "prompt_too_long_400",
        fault: "error context_length",
        ends_in: None,
    },
    Row {
        name: "invalid_request_400",
        fault: "error invalid_request something is wrong with the request",
        ends_in: None,
    },
    Row {
        name: "rate_limited_and_not_made_again",
        fault: "error rate_limit retry-after=none",
        ends_in: None,
    },
    Row {
        name: "key_refused_401",
        fault: "error authentication",
        ends_in: None,
    },
    Row {
        name: "unknown_model_404",
        fault: "error not_found",
        ends_in: None,
    },
    Row {
        name: "stream_cut_in_the_middle_of_a_sentence",
        fault: "chunk 4\nsay The answer is that the file\ncut_after 10",
        ends_in: None,
    },
    Row {
        name: "event_nobody_knows_is_ignored",
        fault: "say before\nevent mystery.event {\"n\": 1}\nsay after",
        ends_in: None,
    },
];

/// What a run of a row came to.
struct Seen {
    version: String,
    outcome: TurnOutcome,
    error: Option<orch_agent_contract::events::TurnError>,
    frames: Vec<serde_json::Value>,
}

/// The fault as the model server is told it: a limit's end is a moment in seconds since the epoch, counted from now.
fn fault_of(row: &Row) -> String {
    let ends = chrono::Utc::now().timestamp() + row.ends_in.unwrap_or_default();
    row.fault.replace("{ends}", &ends.to_string())
}

fn fixture(name: &str) -> serde_json::Value {
    let path = format!(
        "{}/tests/fixtures/failures/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Runs one fault through the real harness, the adapter between.
async fn run(index: usize, row: &Row) -> Option<Seen> {
    let mut rig = Rig::start(&format!("row{index}")).await?;
    let started = rig.boot().await;
    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.settings.raw_events = true;
    rig.adapter.open_session(spec).await.unwrap();
    let events = rig.run_turn(thread, &script("go", &[&fault_of(row)])).await;
    let frames = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::RawFrame(frame) if frame.direction == "in" => Some(frame.payload.clone()),
            _ => None,
        })
        .collect();
    let (outcome, error) = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::TurnCompleted(ended) => Some((ended.status, ended.error.clone())),
            _ => None,
        })
        .unwrap_or((TurnOutcome::Interrupted, None));
    rig.adapter.stop().await.unwrap();
    Some(Seen {
        version: started.harness_version,
        outcome,
        error,
        frames,
    })
}

/// What differs between the fixture's expectation and what was seen, in words; nothing when the row holds.
fn changes(row: &Row, seen: &Seen) -> Vec<String> {
    let expect = &fixture(row.name)["expect"];
    let mut changed = Vec::new();
    if expect["outcome"].as_str().unwrap() == "completed" {
        if seen.outcome != TurnOutcome::Completed {
            changed.push(format!(
                "the turn was expected to complete, it {:?}",
                seen.outcome
            ));
        }
        return changed;
    }
    if seen.outcome != TurnOutcome::Failed {
        changed.push(format!(
            "the turn was expected to fail, it {:?}",
            seen.outcome
        ));
        return changed;
    }
    let Some(error) = &seen.error else {
        changed.push("the failed turn says nothing of why".to_owned());
        return changed;
    };
    if error.code != expect["code"].as_str().unwrap() {
        changed.push(format!(
            "code {} where {} was recorded",
            error.code, expect["code"]
        ));
    }
    if error.retryable != expect["retryable"].as_bool().unwrap() {
        changed.push(format!("retryable is {}", error.retryable));
    }
    if error.incomplete != expect["incomplete"].as_bool().unwrap() {
        changed.push(format!("incomplete is {}", error.incomplete));
    }
    match (row.ends_in, error.resets_at) {
        (Some(secs), Some(at)) => {
            let wanted = chrono::Utc::now().timestamp() + secs;
            if (at.get().timestamp() - wanted).abs() > MOMENT_SLACK_SECS {
                changed.push(format!(
                    "the limit ends at {at}, not {secs} seconds from now"
                ));
            }
        }
        (Some(_), None) => changed.push("the limit has no time".to_owned()),
        (None, Some(at)) => changed.push(format!("a time {at} appeared where none was given")),
        (None, None) => {}
    }
    changed
}

/// Whether the vendor's words are the ones recorded (only to be told of; no mapping depends on them).
fn words_changed(row: &Row, seen: &Seen) -> Option<String> {
    let expect = &fixture(row.name)["expect"];
    let said = seen.error.as_ref()?.said.as_deref()?;
    for member in ["said_exactly", "said_contains"] {
        if let Some(part) = expect[member].as_str() {
            // A moment in a sentence depends on the day and the zone the harness ran in: what comes before
            // it ("You've hit your weekly limit") is what is compared.
            let lead = |text: &str| text.split(" · ").next().unwrap_or(text).to_owned();
            let holds = if member == "said_exactly" {
                lead(said) == lead(part)
            } else {
                said.contains(part)
            };
            if !holds {
                return Some(format!("was {part:?}, now {said:?}"));
            }
        }
    }
    None
}

fn record(row: &Row, seen: &Seen) {
    let Ok(directory) = std::env::var("ORCH_RECORD_FAILURES") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let body = serde_json::json!({
        "harness": "codex",
        "version": seen.version,
        "recorded": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "fault": fault_of(row).replace('\n', "; "),
        "frames": seen.frames,
        "observed": {
            "outcome": format!("{:?}", seen.outcome),
            "error": seen.error,
        },
    });
    std::fs::write(
        directory.join(format!("codex-{}.json", row.name)),
        serde_json::to_string_pretty(&body).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn every_fault_the_model_server_can_give_is_mapped_as_the_recordings_say() {
    let mut changed_rows = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let Some(seen) = run(index, row).await else {
            return;
        };
        record(row, &seen);
        let changes = changes(row, &seen);
        if changes.is_empty() {
            eprintln!("same    codex {} ({})", row.name, seen.version);
        } else {
            eprintln!(
                "CHANGED codex {} ({}): {}",
                row.name,
                seen.version,
                changes.join("; ")
            );
            changed_rows.push(row.name);
        }
        if let Some(words) = words_changed(row, &seen) {
            eprintln!("WORDS   codex {} ({}): {words}", row.name, seen.version);
        }
    }
    assert!(
        changed_rows.is_empty(),
        "rows that changed: {changed_rows:?}"
    );
}
