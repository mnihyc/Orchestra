//! The real pinned CLI's completion follow-up waits for controller turn ownership.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native regression assertions"
)]
mod common;
use common::Rig;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::SessionEvent;

async fn completion(script: &str) {
    let mut rig = Rig::new()
        .await
        .expect("the pinned harness image is required");
    rig.start_quick().await;
    rig.open().await;
    let mut first = rig.turn(script);
    first.approval_mode = ApprovalMode::Bypass;
    let origin = first.turn_id;
    rig.adapter.start_turn(first).await.unwrap();
    let events = rig
        .until(
            |event| {
                matches!(event, SessionEvent::BackgroundTasks(report)
        if report.native_wake.is_some())
            },
            90,
        )
        .await;
    let report = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::BackgroundTasks(report) if report.native_wake.is_some() => Some(report),
            _ => None,
        })
        .expect("the native CLI started a completion follow-up without a second prompt");
    assert!(report.tasks.iter().all(|task| task.origin_turn == origin));
    assert!(
        events.iter().any(
            |event| matches!(event, SessionEvent::TurnCompleted(done) if done.turn_id == origin)
        )
    );
    let finished = report.tasks.first().expect("a completed source task");
    let id = serde_json::json!([finished.id, finished.run_id]).to_string();
    let ended = rig
        .adapter
        .end_background_command(rig.thread, &id)
        .await
        .unwrap();
    assert_eq!(
        ended.state,
        orch_agent_contract::background::BackgroundEndState::AlreadyEnded,
        "a natural completion cannot be retroactively changed into a person's stop"
    );
    let mut admitted = rig.turn("This text must not be submitted as another user prompt");
    admitted.approval_mode = ApprovalMode::Bypass;
    admitted.native_wake = Some(report.native_wake.as_ref().unwrap().token.clone());
    let continuation = admitted.turn_id;
    rig.adapter.start_turn(admitted).await.unwrap();
    let released = rig.until_the_turn_ends().await;
    assert!(
        matches!(released.first(), Some(SessionEvent::TurnStarted(start)) if start.turn_id == continuation)
    );
    assert!(released.iter().any(
        |event| matches!(event, SessionEvent::TurnCompleted(done) if done.turn_id == continuation)
    ));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_background_command_followup_waits_for_an_admitted_continuation() {
    completion("#!mock\ntool Bash {\"command\":\"sleep 2\",\"description\":\"Build\",\"run_in_background\":true}\nthen\nsay Waiting for Build.\n").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_background_agent_followup_waits_for_an_admitted_continuation() {
    completion("#!mock\ntool Agent {\"subagent_type\":\"general-purpose\",\"description\":\"Review\",\"run_in_background\":true,\"prompt\":\"#!mock\\ntool Bash {\\\"command\\\":\\\"sleep 2\\\"}\\nthen\\nsay Review complete.\\n\"}\nthen\nsay Waiting for Review.\n").await;
}

async fn end_owned(child: bool) {
    // Setup/foreground, the running-work witness, End and cleanup each have the native phase budget.
    tokio::time::timeout(
        std::time::Duration::from_secs(90 * 4),
        Box::pin(end_owned_inner(child)),
    )
    .await
    .expect("the native exact-End proof must finish its four bounded phases");
}

async fn end_owned_inner(child: bool) {
    use orch_agent_contract::background::BackgroundEndState;
    let mut rig = Rig::new()
        .await
        .expect("the pinned harness image is required");
    rig.start_quick().await;
    rig.open().await;
    let started = rig.work.path().join("native-work-started");
    let release = rig.work.path().join("release-native-work");
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let command = format!(
        "printf ready > {}; until [ -e {} ]; do sleep 0.1; done",
        quote(&started),
        quote(&release)
    );
    let script = if child {
        let prompt = format!(
            "#!mock\ntool Bash {}\nthen\nsay Review complete.\n",
            serde_json::json!({"command":command})
        );
        format!(
            "#!mock\ntool Agent {}\nthen\nsay Waiting for Review.\n",
            serde_json::json!({"subagent_type":"general-purpose","description":"Review","run_in_background":true,"prompt":prompt})
        )
    } else {
        format!(
            "#!mock\ntool Bash {}\nthen\nsay Waiting for Build.\n",
            serde_json::json!({"command":command,"description":"Build","run_in_background":true})
        )
    };
    let mut first = rig.turn(&script);
    first.approval_mode = ApprovalMode::Bypass;
    let origin = first.turn_id;
    rig.adapter.start_turn(first).await.unwrap();
    let mut events = rig.until_the_turn_ends().await;
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        while !tokio::fs::try_exists(&started).await.unwrap() {
            events.extend(rig.drain(20).await);
        }
    })
    .await
    .expect("the owned native work starts its condition wait before End");
    let roster = rig.adapter.background_commands(rig.thread).await.unwrap();
    assert!(roster.supported);
    let reports: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::BackgroundTasks(_) | SessionEvent::ItemCompleted(_)
            )
        })
        .collect();
    assert!(
        !roster.items.is_empty(),
        "a named live native task; native reports: {reports:#?}"
    );
    let task = roster.items.first().expect("a named live native task");
    let kind = if child {
        orch_agent_contract::background::BackgroundTaskKind::Subagent
    } else {
        orch_agent_contract::background::BackgroundTaskKind::Command
    };
    assert!(events.iter().any(|event| {
        matches!(event, SessionEvent::BackgroundTasks(report) if report.tasks.iter().any(|actual| {
            actual.origin_turn == origin && actual.kind == kind
                && actual.state == orch_agent_contract::background::BackgroundTaskState::Running
                && serde_json::json!([actual.id, actual.run_id]) == serde_json::from_str::<serde_json::Value>(&task.id).unwrap_or_default()
        }))
    }), "End targets this input's actual running native task");
    let requests = rig.model.requests().len();
    let result = rig
        .adapter
        .end_background_command(rig.thread, &task.id)
        .await
        .unwrap();
    assert_eq!(result.state, BackgroundEndState::Ended);
    let repeated = rig
        .adapter
        .end_background_command(rig.thread, &task.id)
        .await
        .unwrap();
    assert_eq!(repeated.state, BackgroundEndState::AlreadyEnded);
    assert!(
        rig.adapter
            .background_commands(rig.thread)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let after = rig.drain(1_000).await;
    assert!(!after.iter().any(|event| matches!(event, SessionEvent::BackgroundTasks(report) if report.native_wake.is_some())));
    assert_eq!(
        rig.model.requests().len(),
        requests,
        "End is native control and causes no model prompt"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_end_stops_a_native_background_command_without_a_wake() {
    end_owned(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_end_stops_a_native_background_subagent_without_a_wake() {
    end_owned(true).await;
}
