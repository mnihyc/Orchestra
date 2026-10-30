//! A conversation carried into a home is imported as items, on the real Codex in Docker; a thread's own history
//! is the harness's and is never exported or imported as a file.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, prompt_texts};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::history::{ExportKind, Fidelity, SessionArchive};
use orch_agent_contract::session::{LocalFile, SessionMode};
use orch_types::new_id;
use serde_json::json;

#[tokio::test]
async fn a_threads_own_history_is_not_exported_as_a_file() {
    let Some(mut rig) = Rig::start("export").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    rig.run_turn(thread, "first words").await;
    for kind in [ExportKind::Native, ExportKind::Neutral] {
        let error = rig
            .adapter
            .export_session(thread, kind, None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "unsupported", "{kind:?}: {error}");
    }
    assert!(
        !rig.home_path().join("orch/exports").exists(),
        "nothing of the history was written beside the harness's own store"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_native_archive_is_refused_with_the_way_the_history_travels_and_nothing_is_kept() {
    let Some(mut rig) = Rig::start("import-native").await else {
        return;
    };
    rig.boot().await;
    let file = rig.workspace_path().join("archive.jsonl");
    std::fs::write(&file, "{\"type\":\"session_meta\"}\n").unwrap();
    for through in [None, Some(1)] {
        let mut spec = rig.spec(new_id(), SessionMode::ImportNative, None);
        spec.archive = Some(SessionArchive {
            format: "codex-rollout-v1".to_owned(),
            path: file.to_string_lossy().into_owned(),
            size: 26,
        });
        spec.settings.upto_turn_ordinal = through;
        let error = rig.adapter.open_session(spec).await.unwrap_err();
        assert_eq!(error.code(), "unsupported", "{error}");
        assert!(
            error.to_string().contains("home") && error.to_string().contains("newest turns"),
            "{error}"
        );
    }
    let sessions = rig.home_path().join("sessions");
    assert!(
        !sessions.exists() || std::fs::read_dir(&sessions).unwrap().next().is_none(),
        "nothing was kept"
    );
    assert!(rig.adapter.probe().await.is_ok());
    rig.adapter.stop().await.unwrap();
}

/// A conversation as the neutral archive writes it: one item to a line, the main stream, oldest first.
fn neutral_history() -> String {
    let item = |kind: &str, turn: &str, payload: serde_json::Value| {
        json!({ "kind": kind, "turn_id": turn, "stream": "main", "status": "completed", "payload": payload })
            .to_string()
    };
    [
        item(
            "user_message",
            "t1",
            json!({ "parts": [ { "type": "text", "text": "Fix the login redirect" } ] }),
        ),
        item(
            "tool_call",
            "t1",
            json!({ "tool": { "kind": "command", "command": "pytest", "cwd": "/w", "exit_code": 1,
                "output": { "inline": "1 failed", "total_len": 8, "truncated": false, "blob": null } },
                "title": "pytest tests/login", "started_at": "2026-10-11T10:00:00.000Z", "duration_ms": 5, "raw": {} }),
        ),
        item(
            "agent_message",
            "t1",
            json!({ "text": "The handler drops the query string.", "text_len": 35, "phase": "final" }),
        ),
        item(
            "user_message",
            "t2",
            json!({ "parts": [ { "type": "text", "text": "Now add a test" } ] }),
        ),
        item(
            "agent_message",
            "t2",
            json!({ "text": "Added.", "text_len": 6, "phase": "final" }),
        ),
    ]
    .join("\n")
        + "\n"
}

#[tokio::test]
async fn a_conversation_of_another_harness_is_imported_as_items_the_model_is_told_in_order() {
    let Some(mut rig) = Rig::start("import-neutral").await else {
        return;
    };
    rig.boot().await;
    let file = rig.workspace_path().join("history.jsonl");
    let history = neutral_history();
    std::fs::write(&file, &history).unwrap();
    let landed = new_id();
    let mut spec = rig.spec(landed, SessionMode::ImportNeutral, None);
    spec.history = Some(LocalFile {
        path: file.to_string_lossy().into_owned(),
        size: i64::try_from(history.len()).unwrap(),
    });
    let imported = rig.adapter.open_session(spec).await.unwrap();
    let report = imported.import_report.expect("an import is reported");
    assert_eq!(report.fidelity, Fidelity::Structured);
    assert_eq!(report.by_kind["tool_call"].converted, 1);
    assert_eq!(report.by_kind["user_message"].kept, 2);

    rig.run_turn(landed, "carry on").await;
    let said = prompt_texts(&rig.model_requests()[0]);
    let at = |needle: &str| {
        said.iter()
            .position(|text| text.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} is not in {said:?}"))
    };
    assert!(at("Fix the login redirect") < at("pytest tests/login"));
    assert!(at("pytest tests/login") < at("The handler drops the query string."));
    assert!(at("The handler drops the query string.") < at("Now add a test"));
    assert!(at("Now add a test") < at("Added."));
    assert!(at("Added.") < at("carry on"));
    assert!(
        said[at("pytest tests/login")].contains("exit 1"),
        "{said:?}"
    );

    // The same conversation through its first turn only.
    let early = new_id();
    let mut spec = rig.spec(early, SessionMode::ImportNeutral, None);
    spec.history = Some(LocalFile {
        path: file.to_string_lossy().into_owned(),
        size: i64::try_from(history.len()).unwrap(),
    });
    spec.settings.upto_turn_ordinal = Some(1);
    rig.adapter.open_session(spec).await.unwrap();
    rig.run_turn(early, "carry on from the start").await;
    let said = prompt_texts(&rig.model_requests()[1]);
    assert!(
        said.iter()
            .any(|text| text.contains("Fix the login redirect"))
    );
    assert!(!said.iter().any(|text| text.contains("Now add a test")));

    let mut spec = rig.spec(new_id(), SessionMode::ImportNeutral, None);
    spec.history = None;
    assert_eq!(
        rig.adapter.open_session(spec).await.unwrap_err().code(),
        "invalid_input",
        "a neutral import names its history"
    );
    rig.adapter.stop().await.unwrap();
}

/// What a small context is told to the harness as: its window is the 272 000 the harness gives a model it does not know,
/// and the context in force is a few thousand, so that one response of that size makes the harness shorten the
/// conversation before the next turn.
const UNKNOWN_MODELS_OWN_WINDOW: i64 = 272_000;

/// The requests the scripted model received for the harness since `from`, as the words of the last one.
fn last_words(rig: &Rig, from: usize) -> String {
    let requests = rig.model_requests();
    assert!(requests.len() > from, "the harness made no request");
    requests.last().unwrap().body.to_string()
}

#[tokio::test]
async fn a_fork_through_the_second_turn_of_a_paginated_thread_the_harness_shortened_carries_the_summary_and_both_turns()
 {
    let Some(mut rig) = Rig::start("fork-shortened").await else {
        return;
    };
    rig.boot().await;
    let small = json!({ "context": 4_000, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.settings.params = small.clone();
    let opened = rig.adapter.open_session(spec).await.unwrap();
    // The first response uses nine tenths of the context: the harness shortens the conversation before the second turn
    // and writes a summary of the first (the scripted answer to that request), so a compaction stands before turn two.
    let mut first = rig.turn(
        thread,
        &common::script("PROBE_ASK_ONE", &["say PROBE_ANSWER_ONE", "usage 3900 10"]),
    );
    first.params = small.clone();
    rig.adapter.start_turn(first).await.unwrap();
    rig.until_turn_ends().await;
    rig.mock.queue("say PROBE_SUMMARY").unwrap();
    for ask in ["PROBE_ASK_TWO", "PROBE_ASK_THREE"] {
        let mut turn = rig.turn(thread, &common::script(ask, &["say an answer"]));
        turn.params = small.clone();
        rig.adapter.start_turn(turn).await.unwrap();
        rig.until_turn_ends().await;
    }
    assert!(
        rig.model_requests().iter().any(|request| {
            let body = request.body.to_string();
            body.contains("PROBE_ASK_TWO") && body.contains("PROBE_SUMMARY")
        }),
        "the harness shortened the conversation, so a later request carries the summary"
    );
    rig.adapter.close_session(thread).await.unwrap();

    // A fork through the second turn, in the home the thread lives in.
    let forked = new_id();
    let mut spec = rig.spec(forked, SessionMode::Fork, Some(&opened.native_session_ref));
    // Keep the follow-up from compacting again: the scripted model does not rewrite a summary's meaning, and
    // the assertion below is about what the native fork retained before another compaction replaces it.
    let followup = json!({ "context": UNKNOWN_MODELS_OWN_WINDOW, "context_window": UNKNOWN_MODELS_OWN_WINDOW });
    spec.settings.params = followup.clone();
    spec.settings.upto_turn_ordinal = Some(2);
    let fork = rig.adapter.open_session(spec).await.unwrap();
    assert_ne!(fork.native_session_ref, opened.native_session_ref);
    let asked = rig.model_requests().len();
    let mut turn = rig.turn(forked, "PROBE_FORK_FOLLOWUP");
    turn.params = followup;
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until_turn_ends().await;
    let body = last_words(&rig, asked);
    for kept in [
        "PROBE_ASK_ONE",
        "PROBE_SUMMARY",
        "PROBE_ASK_TWO",
        "PROBE_FORK_FOLLOWUP",
    ] {
        assert!(
            body.contains(kept),
            "the fork's request lacks {kept}: {body}"
        );
    }
    assert!(
        !body.contains("PROBE_ASK_THREE"),
        "the fork carries a turn after the one it was forked through"
    );
    rig.adapter.stop().await.unwrap();
}
