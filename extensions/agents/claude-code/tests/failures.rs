//! The real Claude Code in its image against the model server that fails on purpose: what the adapter makes of each fault, row by row, and the recording that the canary of a new release compares.
//!
//! Every row has a fixture in `tests/fixtures/failures` (what the version recorded there wrote for it), which
//! `recorded_failures.rs` replays without a container. This file runs the harness itself, so that a new version
//! is held against the same rows:
//!
//! * a row whose code, advice to try again or mark of a cut answer differs from what its fixture expects fails
//!   the test, with the row named: those are the rows that changed;
//! * a row whose words differ (the vendor changed its sentence, which no mapping depends on) is printed as
//!   `WORDS` and does not fail;
//! * with `ORCH_RECORD_FAILURES=<directory>` each row is also written there as `claude-code-<row>.json`, in the
//!   form of the fixtures (the frames the harness wrote, the fault, what the adapter made of them), so that a
//!   changed row can be read and its fixture renewed.
//!
//! The image is `ORCH_HARNESS_IMAGE` when that is set, else `orch-dev-harness`. A machine without Docker or the
//! image skips the test and says so. The harness is told to try again once, not ten times, so that a row takes
//! seconds and not minutes; how often it tries is recorded in the fixtures, which were made with its defaults.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::Rig;
use orch_adapter_claude_code::events::FROM_HARNESS;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{SessionEvent, TurnError, TurnOutcome};
use orch_agent_contract::login::{LoginRequest, LoginStateKind, LoginValues};
use orch_agent_contract::session::{ProviderAccess, SessionMode};
use orch_types::new_id;
use serde_json::{Value, json};

/// A token as the vendor's command prints it, for a test: the plan's limits are answered to one that signed in.
const SUBSCRIBER_TOKEN: &str = "sk-ant-oat01-failure-rows-0123456789";

/// How far from the moment the server was told to give, in seconds, the moment the adapter reads may be: the
/// server counts the time from the request, and a row takes a while to reach it.
const MOMENT_SLACK_SECS: i64 = 600;

/// One fault: the fixture that holds what the harness once wrote for it, what the model server is told to do,
/// whether the harness is signed in as a subscriber (a plan's limits are answered only to one), and, for a limit
/// with a time, how many seconds from now the server is told it ends.
struct Row {
    name: &'static str,
    fault: &'static str,
    subscriber: bool,
    ends_in: Option<i64>,
}

const ROWS: &[Row] = &[
    Row {
        name: "plan_limit_weekly_dated",
        fault: "error plan_limit window=weekly resets={ends}",
        subscriber: true,
        ends_in: Some(172_800),
    },
    Row {
        name: "plan_limit_of_one_family_of_models",
        fault: "error plan_limit window=weekly_opus resets={ends}",
        subscriber: true,
        ends_in: Some(172_800),
    },
    Row {
        name: "plan_limit_without_a_time",
        fault: "error plan_limit window=session",
        subscriber: true,
        ends_in: None,
    },
    Row {
        name: "throttled_though_not_the_plans_limit",
        fault: "error rate_limit retry-after=2",
        subscriber: true,
        ends_in: None,
    },
    Row {
        name: "rate_limited_with_a_key",
        fault: "error rate_limit retry-after=none",
        subscriber: false,
        ends_in: None,
    },
    Row {
        name: "overloaded_529",
        fault: "error overloaded retry-after=none",
        subscriber: false,
        ends_in: None,
    },
    Row {
        name: "server_error_500",
        fault: "error server",
        subscriber: false,
        ends_in: None,
    },
    Row {
        name: "sign_in_refused_401",
        fault: "error authentication",
        subscriber: true,
        ends_in: None,
    },
    Row {
        name: "forbidden_403",
        fault: "error permission",
        subscriber: false,
        ends_in: None,
    },
    Row {
        name: "balance_ran_out_402",
        fault: "error billing",
        subscriber: false,
        ends_in: None,
    },
    Row {
        name: "unknown_model_404",
        fault: "error not_found",
        subscriber: false,
        ends_in: None,
    },
    Row {
        name: "prompt_too_long_400",
        fault: "error context_length",
        subscriber: false,
        ends_in: None,
    },
    Row {
        name: "stream_cut_in_the_middle_of_a_sentence",
        fault: "chunk 4\nsay The answer is that the file\ncut_after 10",
        subscriber: true,
        ends_in: None,
    },
    Row {
        name: "stream_cut_in_the_middle_of_a_tool_call",
        fault: "chunk 4\ntool Bash {\"command\":\"ls -la /tmp\"}\ncut_after 12",
        subscriber: true,
        ends_in: None,
    },
    Row {
        name: "answer_that_is_no_json",
        fault: "body 200 this is not json at all",
        subscriber: true,
        ends_in: None,
    },
    Row {
        name: "event_nobody_knows_is_ignored",
        fault: "say before\nevent mystery_event {\"n\": 1}\nsay after",
        subscriber: true,
        ends_in: None,
    },
];

/// What a run of a row came to.
struct Seen {
    version: String,
    outcome: TurnOutcome,
    error: Option<TurnError>,
    frames: Vec<Value>,
}

/// The fault as the model server is told it: a limit's end is a moment in seconds since the epoch, counted from now.
fn fault_of(row: &Row) -> String {
    let ends = chrono::Utc::now().timestamp() + row.ends_in.unwrap_or_default();
    row.fault.replace("{ends}", &ends.to_string())
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/failures/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Runs one fault through the real harness, the adapter between.
async fn run(row: &Row) -> Option<Seen> {
    let mut rig = Rig::new().await?;
    let quick = ("CLAUDE_CODE_MAX_RETRIES".to_owned(), "1".to_owned());
    let started = if row.subscriber {
        // The harness reaches the model server on its own with the token of its sign-in, as a subscriber does.
        rig.model.set_credential(Some(SUBSCRIBER_TOKEN));
        let env = BTreeMap::from([quick, ("ANTHROPIC_BASE_URL".to_owned(), rig.model.url())]);
        let started = rig.start(json!({}), env).await;
        rig.adapter
            .login(LoginRequest {
                method: "setup_token".into(),
                values: LoginValues(BTreeMap::from([(
                    "token".to_owned(),
                    json!(SUBSCRIBER_TOKEN),
                )])),
                config: json!({}),
            })
            .await
            .unwrap();
        rig.logins_until(|state| state.state == LoginStateKind::Completed, 20)
            .await;
        started
    } else {
        rig.start(json!({}), BTreeMap::from([quick])).await
    };
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.raw_events = true;
    if row.subscriber {
        spec.provider = ProviderAccess::Native {
            provider_id: new_id(),
        };
    }
    rig.open_with(spec).await;
    let (_, events) = rig.run(&format!("#!mock\n{}\n", fault_of(row))).await;
    let frames = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::RawFrame(frame) if frame.direction == FROM_HARNESS => {
                Some(frame.payload.clone())
            }
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
    match expect["outcome"].as_str().unwrap() {
        "completed" => {
            if seen.outcome != TurnOutcome::Completed {
                changed.push(format!(
                    "the turn was expected to complete, it {:?}",
                    seen.outcome
                ));
            }
            return changed;
        }
        _ => {
            if seen.outcome != TurnOutcome::Failed {
                changed.push(format!(
                    "the turn was expected to fail, it {:?}",
                    seen.outcome
                ));
                return changed;
            }
        }
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
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let body = json!({
        "harness": "claude-code",
        "version": seen.version,
        "recorded": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "fault": fault_of(row).replace('\n', "; "),
        "subscriber": row.subscriber,
        "frames": seen.frames,
        "observed": {
            "outcome": format!("{:?}", seen.outcome),
            "error": seen.error,
        },
    });
    std::fs::write(
        directory.join(format!("claude-code-{}.json", row.name)),
        serde_json::to_string_pretty(&body).unwrap(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_fault_the_model_server_can_give_is_mapped_as_the_recordings_say() {
    let mut changed_rows = Vec::new();
    for row in ROWS {
        let Some(seen) = run(row).await else {
            return;
        };
        record(row, &seen);
        let changes = changes(row, &seen);
        if changes.is_empty() {
            eprintln!("same    claude-code {} ({})", row.name, seen.version);
        } else {
            eprintln!(
                "CHANGED claude-code {} ({}): {}",
                row.name,
                seen.version,
                changes.join("; ")
            );
            changed_rows.push(row.name);
        }
        if let Some(words) = words_changed(row, &seen) {
            eprintln!(
                "WORDS   claude-code {} ({}): {words}",
                row.name, seen.version
            );
        }
    }
    assert!(
        changed_rows.is_empty(),
        "rows that changed: {changed_rows:?}"
    );
}
