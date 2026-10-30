//! Commands and changes of files that ask to be allowed, are allowed or refused, and a turn that is interrupted,
//! on the real Codex in Docker.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, kinds, script};
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{
    FileChangeKind, HarnessRequestDetail, HarnessRequestKind, ItemContent, ItemStatus,
    SessionEvent, ToolCallPayload, ToolPayload, TurnOutcome,
};
use orch_agent_contract::session::SessionMode;
use orch_types::new_id;

/// A rig with a session that asks before it does what it does not already trust.
async fn asking(label: &str) -> Option<(Rig, orch_types::Id)> {
    let mut rig = Rig::start(label).await?;
    rig.boot().await;
    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.settings.approval_mode = ApprovalMode::Ask;
    rig.adapter.open_session(spec).await.unwrap();
    Some((rig, thread))
}

fn asking_turn(
    rig: &Rig,
    thread: orch_types::Id,
    text: &str,
) -> orch_agent_contract::session::TurnStart {
    let mut turn = rig.turn(thread, text);
    turn.approval_mode = ApprovalMode::Ask;
    turn
}

fn tool_call(payload: &serde_json::Value) -> ToolCallPayload {
    serde_json::from_value(payload.clone()).unwrap()
}

#[tokio::test]
async fn a_command_asks_is_allowed_runs_and_its_output_arrives_as_events() {
    let Some((mut rig, thread)) = asking("allow").await else {
        return;
    };
    let text = script(
        "run it",
        &[
            r#"tool exec_command {"cmd":"echo hello-from-shell && echo problem 1>&2 && pwd"}"#,
            "then",
            "say all done",
        ],
    );
    let turn = asking_turn(&rig, thread, &text);
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();

    let (before, request) = rig.until_approval().await;
    assert!(
        kinds(&before).contains(&"turn.started"),
        "{:?}",
        kinds(&before)
    );
    let started = before
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemStarted(item) => Some(item),
            _ => None,
        })
        .expect("the command is on the timeline while it waits to be allowed");
    let ItemContent::ToolCall(call) = &started.content else {
        panic!("a tool call");
    };
    assert!(matches!(
        &call.tool,
        ToolPayload::Command {
            exit_code: None,
            ..
        }
    ));
    assert_eq!(call.raw.name, "commandExecution");
    assert!(call.raw.output.is_none(), "nothing came back yet");

    assert_eq!(request.turn_id, turn_id);
    assert_eq!(request.request.request_kind, HarnessRequestKind::Command);
    assert_eq!(
        request.title,
        "Run echo hello-from-shell && echo problem 1>&2 && pwd"
    );
    let HarnessRequestDetail::Command { command, cwd, .. } = &request.request.detail else {
        panic!("a command");
    };
    assert_eq!(
        command, "echo hello-from-shell && echo problem 1>&2 && pwd",
        "as the agent wrote it, without the shell around it"
    );
    assert_eq!(cwd, &rig.workspace_path().to_string_lossy());
    let options: Vec<_> = request
        .request
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect();
    assert_eq!(options, ["once", "always", "deny", "abort"]);

    rig.choose(thread, &request.request_ref, "once").await;
    let events = rig.until_turn_ends().await;
    assert!(
        events.iter().all(|event| event.kind() != "approval.closed"),
        "the harness's note that the request is settled is no withdrawal: {:?}",
        kinds(&events)
    );
    let completed = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.native_ref == started.native_ref => {
                Some(item)
            }
            _ => None,
        })
        .next_back()
        .expect("the command completes");
    assert_eq!(completed.status, ItemStatus::Completed);
    let call = tool_call(&completed.payload);
    let ToolPayload::Command {
        command,
        exit_code,
        output,
        actions,
        ..
    } = &call.tool
    else {
        panic!("a command");
    };
    assert_eq!(command, "echo hello-from-shell && echo problem 1>&2 && pwd");
    assert_eq!(*exit_code, Some(0));
    assert!(
        output.inline.contains("hello-from-shell") && output.inline.contains("problem"),
        "{output:?}"
    );
    assert!(
        output
            .inline
            .contains(&*rig.workspace_path().to_string_lossy())
    );
    assert_eq!(
        output.total_len,
        i64::try_from(output.inline.len()).unwrap()
    );
    assert!(!actions.is_empty());
    let raw_input: serde_json::Value = serde_json::from_str(&call.raw.input.inline).unwrap();
    assert!(
        raw_input["command"]
            .as_str()
            .unwrap()
            .starts_with("/bin/bash -lc"),
        "the call as the harness made it, whole"
    );
    assert_eq!(call.raw.output.as_ref().unwrap().inline, output.inline);
    assert!(!call.raw.is_error);
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );
    let said = events.iter().any(|event| {
        matches!(event, SessionEvent::ItemCompleted(item)
        if item.payload.get("text").and_then(|text| text.as_str()) == Some("all done"))
    });
    assert!(said, "{:?}", kinds(&events));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_declined_command_is_not_run_and_the_turn_goes_on() {
    let Some((mut rig, thread)) = asking("decline").await else {
        return;
    };
    let marker = rig.workspace_path().join("declined.txt");
    let text = script(
        "run it",
        &[
            &format!(
                r#"tool exec_command {{"cmd":"touch {}"}}"#,
                marker.display()
            ),
            "then",
            "say fine without it",
        ],
    );
    let turn = asking_turn(&rig, thread, &text);
    rig.adapter.start_turn(turn).await.unwrap();
    let (_, request) = rig.until_approval().await;
    rig.choose(thread, &request.request_ref, "deny").await;
    let events = rig.until_turn_ends().await;
    assert!(!marker.exists(), "the command was not run");
    let refused = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.payload.get("tool").is_some() => Some(item),
            _ => None,
        })
        .expect("the command's item completes");
    assert_eq!(refused.status, ItemStatus::Cancelled);
    let call = tool_call(&refused.payload);
    assert!(matches!(
        &call.tool,
        ToolPayload::Command {
            exit_code: None,
            ..
        }
    ));
    assert!(
        call.raw.output.is_none() && call.raw.is_error,
        "{:?}",
        call.raw
    );
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed),
        "refusing a command does not stop the turn"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn refusing_and_stopping_ends_the_turn_as_interrupted() {
    let Some((mut rig, thread)) = asking("abort").await else {
        return;
    };
    let marker = rig.workspace_path().join("aborted.txt");
    let text = script(
        "run it",
        &[&format!(
            r#"tool exec_command {{"cmd":"touch {}"}}"#,
            marker.display()
        )],
    );
    let turn = asking_turn(&rig, thread, &text);
    rig.adapter.start_turn(turn).await.unwrap();
    let (_, request) = rig.until_approval().await;
    rig.choose(thread, &request.request_ref, "abort").await;
    let events = rig.until_turn_ends().await;
    assert!(!marker.exists());
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Interrupted)
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_change_of_files_asks_arrives_as_a_difference_and_is_made() {
    let Some((mut rig, thread)) = asking("patch").await else {
        return;
    };
    std::fs::write(rig.workspace_path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(rig.workspace_path().join("old.txt"), "gone\n").unwrap();
    let patch = "apply_patch <<'PATCH'\\n*** Begin Patch\\n*** Update File: a.txt\\n@@\\n one\\n-two\\n+TWO\\n three\\n*** Add File: b.txt\\n+hello\\n+world\\n*** Delete File: old.txt\\n*** End Patch\\nPATCH";
    let text = script(
        "patch it",
        &[
            &format!(r#"tool exec_command {{"cmd":"{patch}"}}"#),
            "then",
            "say patched",
        ],
    );
    let turn = asking_turn(&rig, thread, &text);
    rig.adapter.start_turn(turn).await.unwrap();
    let (_, request) = rig.until_approval().await;
    assert_eq!(request.request.request_kind, HarnessRequestKind::FileChange);
    let HarnessRequestDetail::FileChange { changes, .. } = &request.request.detail else {
        panic!("a change of files");
    };
    assert_eq!(request.title, "Change 3 files");
    let by_path = |path: &str| changes.iter().find(|change| change.path == path).unwrap();
    let modified = by_path("a.txt");
    assert_eq!(modified.change, FileChangeKind::Modify);
    assert_eq!((modified.added, modified.removed), (1, 1));
    assert!(modified.diff.as_deref().unwrap().contains("-two\n+TWO"));
    let added = by_path("b.txt");
    assert_eq!(added.change, FileChangeKind::Add);
    assert_eq!(
        added.diff.as_deref(),
        Some("@@ -0,0 +1,2 @@\n+hello\n+world\n")
    );
    assert_eq!((added.added, added.removed), (2, 0));
    let removed = by_path("old.txt");
    assert_eq!(removed.change, FileChangeKind::Delete);
    assert_eq!((removed.added, removed.removed), (0, 1));
    assert!(
        !rig.workspace_path().join("b.txt").exists(),
        "nothing is made before it is allowed"
    );

    rig.choose(thread, &request.request_ref, "once").await;
    let events = rig.until_turn_ends().await;
    assert_eq!(
        std::fs::read_to_string(rig.workspace_path().join("a.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    assert_eq!(
        std::fs::read_to_string(rig.workspace_path().join("b.txt")).unwrap(),
        "hello\nworld\n"
    );
    assert!(!rig.workspace_path().join("old.txt").exists());
    let done = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.payload["tool"]["kind"] == "file_change" => {
                Some(item)
            }
            _ => None,
        })
        .expect("the change completes");
    assert_eq!(done.status, ItemStatus::Completed);
    let call = tool_call(&done.payload);
    let ToolPayload::FileChange { changes } = &call.tool else {
        panic!("a change of files");
    };
    assert_eq!(changes.len(), 3);
    assert_eq!(call.title, "Edit 3 files");
    assert_eq!(call.raw.name, "fileChange");
    assert!(
        call.raw.input.inline.contains("*** ") || call.raw.input.inline.contains("\"changes\"")
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_turn_is_interrupted_and_a_command_that_goes_on_stays_open_and_is_marked() {
    let Some(mut rig) = Rig::start("interrupt").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    let text = script(
        "wait for it",
        &[
            r#"tool exec_command {"cmd":"for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do echo tick$i; sleep 1; done","yield_time_ms":30000}"#,
            "then",
            "say finished",
        ],
    );
    let turn = rig.turn(thread, &text);
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    let before = rig
        .until(|event| {
            matches!(event, AdapterEvent::Session { event: SessionEvent::ItemDelta(delta), .. }
            if delta.field == orch_agent_contract::events::DeltaField::Output)
        })
        .await;
    assert!(
        common::session_events(&before)
            .iter()
            .any(|event| event.kind() == "item.started")
    );
    rig.adapter.interrupt(thread, turn_id).await.unwrap();
    let events = rig.until_turn_ends().await;
    let Some(SessionEvent::TurnCompleted(ended)) = events.last() else {
        panic!("the turn ends");
    };
    assert_eq!(
        (ended.turn_id, ended.status),
        (turn_id, TurnOutcome::Interrupted)
    );
    let marked = events.iter().find_map(|event| match event {
        SessionEvent::ItemUpdated(update) => Some(update),
        _ => None,
    });
    let marked = marked.expect("the command that goes on is marked as running in the background");
    assert_eq!(marked.payload["tool"]["kind"], "command");
    assert_eq!(marked.payload["tool"]["background"], true);
    assert!(
        events.iter().all(|event| !matches!(event, SessionEvent::ItemCompleted(item) if item.payload.get("tool").is_some())),
        "the harness completes it when its process ends, not before"
    );
    // A turn that has ended is interrupted by nothing, and the session takes the next turn.
    rig.adapter.interrupt(thread, turn_id).await.unwrap();
    let events = rig.run_turn(thread, "are you there").await;
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_request_that_is_withdrawn_when_the_turn_is_interrupted_is_reported_as_closed() {
    let Some((mut rig, thread)) = asking("withdrawn").await else {
        return;
    };
    let text = script("run it", &[r#"tool exec_command {"cmd":"echo never"}"#]);
    let turn = asking_turn(&rig, thread, &text);
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    let (_, request) = rig.until_approval().await;
    rig.adapter.interrupt(thread, turn_id).await.unwrap();
    let events = rig.until_turn_ends().await;
    let closed = events
        .iter()
        .position(|event| matches!(event, SessionEvent::ApprovalClosed(closed) if closed.request_ref == request.request_ref))
        .unwrap_or_else(|| panic!("{:?}", kinds(&events)));
    assert!(
        closed < events.len() - 1,
        "the request is reported withdrawn before the turn is reported ended"
    );
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Interrupted)
    );
    // The harness's own note that the request is settled follows, and is not reported again; and an answer
    // that comes too late is refused.
    let late = rig
        .adapter
        .resolve_approval(
            thread,
            &request.request_ref,
            orch_agent_contract::session::ApprovalDecision::Option {
                option_id: "once".into(),
                note: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(late.code(), "not_found");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        common::session_events(&rig.drain())
            .iter()
            .all(|event| event.kind() != "approval.closed"),
        "not reported twice"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_running_turn_can_be_steered_and_only_one_turn_runs_at_a_time() {
    let Some(mut rig) = Rig::start("steer").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    let text = script(
        "wait",
        &[
            r#"tool exec_command {"cmd":"sleep 25","yield_time_ms":30000}"#,
            "then",
            "say done",
        ],
    );
    let turn = rig.turn(thread, &text);
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until(|event| {
        matches!(
            event,
            AdapterEvent::Session {
                event: SessionEvent::ItemStarted(_),
                ..
            }
        )
    })
    .await;
    let second = rig.turn(thread, "another");
    let refused = rig.adapter.start_turn(second).await.unwrap_err();
    assert_eq!(refused.code(), "conflict");
    rig.adapter
        .steer(orch_agent_contract::session::TurnSteer {
            thread_id: thread,
            turn_id,
            text: "also say banana".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let elsewhere = rig
        .adapter
        .steer(orch_agent_contract::session::TurnSteer {
            thread_id: thread,
            turn_id: new_id(),
            text: "to a turn that does not run".into(),
            images: Vec::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(elsewhere.code(), "conflict");
    rig.adapter.interrupt(thread, turn_id).await.unwrap();
    let events = rig.until_turn_ends().await;
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Interrupted)
    );
    rig.adapter.stop().await.unwrap();
}
