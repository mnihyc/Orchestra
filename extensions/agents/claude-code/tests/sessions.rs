//! Sessions against the real harness: resume, fork, a process that dies, and what changes between turns.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::{Rig, kinds, outcome, said};
use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_agent_contract::adapter::{AdapterError, AgentAdapter, AgentStart, EventSink};
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::session::SessionMode;
use orch_types::{SystemClock, new_id};
use serde_json::json;

fn record_home_footprint(home: &std::path::Path) {
    let Ok(directory) = std::env::var("ORCH_HISTORY_FOOTPRINT_DIR") else {
        return;
    };
    let mut totals = std::collections::BTreeMap::<String, u64>::new();
    for top in std::fs::read_dir(home).unwrap() {
        let top = top.unwrap();
        let name = top.file_name().to_string_lossy().into_owned();
        let mut pending = vec![top.path()];
        let mut bytes = 0;
        while let Some(path) = pending.pop() {
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.extend(
                    std::fs::read_dir(path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            } else if metadata.is_file() {
                bytes += metadata.len();
            }
        }
        totals.insert(name, bytes);
    }
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(std::path::Path::new(&directory).join("claude-code-home.json"),serde_json::to_vec_pretty(&json!({"program":"Claude Code","sample":"short scripted run","at":chrono::Utc::now().to_rfc3339(),"by_directory":totals})).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_session_and_resume_load_only_delivered_servers_and_leave_no_synced_downloads() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.raw_events = true;
    let opened = rig.open_with(spec).await;
    for resume in [false, true] {
        if resume {
            rig.adapter.close_session(rig.thread).await.unwrap();
            let mut spec = rig.spec(SessionMode::Resume, Some(&opened.native_session_ref));
            spec.settings.raw_events = true;
            assert!(rig.open_with(spec).await.resumed);
        }
        let (_, events) = rig
            .run(if resume {
                "resume supervised"
            } else {
                "start supervised"
            })
            .await;
        assert_eq!(outcome(&events), TurnOutcome::Completed);
        let init = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::RawFrame(frame)
                    if frame.payload["type"] == "system" && frame.payload["subtype"] == "init" =>
                {
                    Some(&frame.payload)
                }
                _ => None,
            })
            .expect("the pinned build reports its actual connector list");
        assert!(
            init["mcp_servers"].as_array().unwrap().is_empty(),
            "no account or home connector is loaded"
        );
        for path in [
            "skills/synced",
            "plugins/synced",
            "plugins/marketplaces",
            "packages",
        ] {
            assert!(
                !rig.home.path().join(path).exists(),
                "no unsolicited download in {path}"
            );
        }
    }
    {
        let launches = rig.launcher.launched.lock().unwrap();
        let sessions: Vec<_> = launches
            .iter()
            .filter(|launch| launch.args.iter().any(|arg| arg == "--strict-mcp-config"))
            .collect();
        assert_eq!(sessions.len(), 2);
        assert!(
            sessions
                .iter()
                .all(|launch| launch.env.get("DISABLE_UPDATES").map(String::as_str) == Some("1"))
        );
    }
    record_home_footprint(rig.home.path());
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cleanup_removes_the_native_session_files_and_preserves_unrelated_home_data() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let spec = rig.spec(SessionMode::New, None);
    let segment = spec.settings.segment_id;
    let opened = rig.open_with(spec).await;
    rig.run("remember this before cleanup").await;
    let source = orch_agent_contract::history::NativePrefixSource {
        thread_id: rig.thread,
        segment_id: segment,
        native_session_ref: opened.native_session_ref.clone(),
    };
    let refused = rig
        .adapter
        .manage_history(vec![source.clone().into()], true)
        .await
        .unwrap();
    assert_eq!(refused[0].error.as_deref(), Some("history.not_owned"));
    assert!(!refused[0].removed);
    let neighbour = rig.home.path().join("projects/orch/other.jsonl");
    std::fs::write(&neighbour, "an unrelated transcript").unwrap();
    let measured = rig
        .adapter
        .manage_history(
            vec![orch_agent_contract::history::NativeHistorySource {
                source: source.clone(),
                started_by_orch: true,
                inventory_pass: None,
            }],
            false,
        )
        .await
        .unwrap();
    assert!(measured[0].bytes.unwrap() > 0, "{measured:?}");
    assert!(measured[0].error.is_none(), "{measured:?}");
    record_home_footprint(rig.home.path());
    let removed = rig
        .adapter
        .manage_history(
            vec![orch_agent_contract::history::NativeHistorySource {
                source,
                started_by_orch: true,
                inventory_pass: None,
            }],
            true,
        )
        .await
        .unwrap();
    assert!(removed[0].removed, "{removed:?}");
    assert!(removed[0].error.is_none(), "{removed:?}");
    assert!(
        !orch_adapter_claude_code::sessions::managed_transcript(
            rig.home.path(),
            &opened.native_session_ref
        )
        .exists()
    );
    assert_eq!(
        std::fs::read_to_string(neighbour).unwrap(),
        "an unrelated transcript"
    );
    let resume = rig.spec(SessionMode::Resume, Some(&opened.native_session_ref));
    assert_eq!(
        rig.adapter.open_session(resume).await.unwrap_err().code(),
        "session_not_found"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_is_resumed_in_a_new_process_and_remembers_the_conversation() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let opened = rig.open().await;
    rig.run("first marker-aaa").await;
    rig.adapter.close_session(rig.thread).await.unwrap();

    let again = rig
        .adapter
        .open_session(rig.spec(SessionMode::Resume, Some(&opened.native_session_ref)))
        .await
        .unwrap();
    assert!(again.resumed);
    assert_eq!(
        again.native_session_ref, opened.native_session_ref,
        "the same conversation"
    );
    rig.until(|event| matches!(event, SessionEvent::SessionOpened(_)), 20)
        .await;
    let (_, events) = rig.run("second marker-bbb").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let request = rig.last_request();
    assert!(
        request.contains("marker-aaa") && request.contains("marker-bbb"),
        "the harness remembers"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_is_resumed_by_a_whole_new_adapter_on_the_same_home() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let opened = rig.open().await;
    rig.run("first marker-aaa").await;
    rig.adapter.stop().await.unwrap();

    // The connector restarted: a new adapter, the same home.
    let (sink, events) = EventSink::channel();
    let mut fresh = ClaudeCodeAdapter::with_launcher(Arc::new(SystemClock), rig.launcher.clone());
    fresh
        .start(
            AgentStart {
                provider_id: new_id(),
                agent_id: new_id(),
                harness_kind: "claude-code".into(),
                profile: AgentProfile {
                    harness_version: None,
                    attach: None,
                    config: json!({}),
                    env: BTreeMap::from([("CLAUDE_CODE_MAX_RETRIES".to_owned(), "1".to_owned())]),
                },
                home: Some(rig.home.path().display().to_string()),
                provider_config: json!({}),
                loopback: rig.loopback(),
                binary: None,
            },
            sink,
        )
        .await
        .unwrap();
    rig.events = events;
    fresh
        .open_session(rig.spec(SessionMode::Resume, Some(&opened.native_session_ref)))
        .await
        .unwrap();
    rig.until(|event| matches!(event, SessionEvent::SessionOpened(_)), 20)
        .await;
    let turn = rig.turn("second marker-bbb");
    fresh.start_turn(turn).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let request = rig.last_request();
    assert!(request.contains("marker-aaa") && request.contains("marker-bbb"));
    fresh.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_the_harness_does_not_have_is_not_found() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let spec = rig.spec(
        SessionMode::Resume,
        Some("99999999-9999-4999-8999-999999999999"),
    );
    let error = rig.adapter.open_session(spec).await.unwrap_err();
    assert!(
        matches!(error, AdapterError::SessionNotFound(_)),
        "{error:?}"
    );
    let spec = rig.spec(SessionMode::Resume, None);
    assert!(matches!(
        rig.adapter.open_session(spec).await.unwrap_err(),
        AdapterError::NotFound(_)
    ));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_session_forks_from_the_first_as_it_was_when_the_fork_was_made() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let parent_lane = rig.lane();
    let parent = rig.open().await;
    rig.run("first marker-aaa").await;

    let fork_lane = rig.new_lane();
    let forked = rig
        .adapter
        .open_session(rig.spec(SessionMode::Fork, Some(&parent.native_session_ref)))
        .await
        .unwrap();
    assert_ne!(forked.native_session_ref, parent.native_session_ref);
    assert!(!forked.resumed);
    rig.until(|event| matches!(event, SessionEvent::SessionOpened(_)), 20)
        .await;

    // The parent goes on after the fork was made.
    rig.set_lane(&parent_lane);
    let (_, events) = rig.run("second marker-bbb").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);

    // The fork has the parent as it was, and goes its own way.
    rig.set_lane(&fork_lane);
    let (_, events) = rig.run("third marker-ccc").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let request = rig.last_request();
    assert!(request.contains("marker-aaa") && request.contains("marker-ccc"));
    assert!(
        !request.contains("marker-bbb"),
        "what the parent did after the fork is not in it"
    );

    rig.set_lane(&parent_lane);
    let (_, events) = rig.run("fourth marker-ddd").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let request = rig.last_request();
    assert!(
        request.contains("marker-aaa")
            && request.contains("marker-bbb")
            && request.contains("marker-ddd")
    );
    assert!(
        !request.contains("marker-ccc"),
        "the parent does not see the fork"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_at_an_earlier_turn_is_not_supported_and_the_engine_takes_the_next_method() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let parent = rig.open().await;
    rig.run("first marker-aaa").await;
    rig.new_lane();
    let mut spec = rig.spec(SessionMode::Fork, Some(&parent.native_session_ref));
    spec.settings.upto_turn_ordinal = Some(0);
    assert!(matches!(
        rig.adapter.open_session(spec).await.unwrap_err(),
        AdapterError::Unsupported(_)
    ));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_process_that_dies_in_a_turn_is_reported_and_the_next_turn_recovers_the_session() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    rig.run("first marker-aaa").await;

    let turn = rig.turn("#!mock\nchunk 4\ndelay 400\nsay This streams slowly enough for the process to be killed in the middle of it.\n");
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until(|event| matches!(event, SessionEvent::ItemDelta(_)), 60)
        .await;
    rig.launcher.kill_all();
    let events = rig
        .until(|event| matches!(event, SessionEvent::SessionLost(_)), 30)
        .await;
    let order = kinds(&events);
    assert!(
        !order.contains(&"turn.completed"),
        "the engine fails the turn on a lost session: {order:?}"
    );
    let SessionEvent::SessionLost(lost) = events.last().unwrap() else {
        panic!("the session was lost");
    };
    assert!(lost.reason.contains("harness process"), "{}", lost.reason);

    // The next turn starts a new process that resumes the conversation.
    let (_, events) = rig.run("after the crash marker-ccc").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert_eq!(said(&events), "You said: after the crash marker-ccc");
    let request = rig.last_request();
    assert!(
        request.contains("marker-aaa") && request.contains("marker-ccc"),
        "it remembers"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_effort_the_mode_and_the_model_are_changed_between_turns() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let effort_of = |rig: &Rig| {
        let requests = rig.model.requests();
        let body = &requests.last().unwrap().body;
        (
            body["model"].as_str().unwrap().to_owned(),
            body["output_config"]["effort"].as_str().map(str::to_owned),
        )
    };

    let mut turn = rig.turn("one");
    turn.effort = Some("low".into());
    let id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until_the_turn_ends().await;
    let _ = id;
    assert_eq!(effort_of(&rig), ("mock-large".into(), Some("low".into())));

    // The effort changes in the running process.
    let mut turn = rig.turn("two");
    turn.effort = Some("max".into());
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until_the_turn_ends().await;
    assert_eq!(effort_of(&rig), ("mock-large".into(), Some("max".into())));

    // The model changes with a process that resumes the conversation.
    let mut turn = rig.turn("three marker-model");
    turn.model = "mock-small".into();
    turn.effort = Some("max".into());
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert_eq!(effort_of(&rig), ("mock-small".into(), Some("max".into())));
    assert!(
        rig.last_request().contains("two"),
        "the conversation went along"
    );

    // A mode that accepts edits asks nothing about a file in the directory.
    let dir = rig.directory();
    let mut turn = rig.turn(&format!(
        "#!mock\ntool Write {{\"file_path\":\"{dir}/quiet.txt\",\"content\":\"no question\\n\"}}\nthen\nsay done\n"
    ));
    turn.model = "mock-small".into();
    turn.effort = Some("max".into());
    turn.approval_mode = ApprovalMode::OnRequest;
    turn.sandbox_mode = SandboxMode::Open;
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    assert!(
        !kinds(&events).contains(&"approval.requested"),
        "{:?}",
        kinds(&events)
    );
    assert_eq!(
        std::fs::read_to_string(rig.work.path().join("quiet.txt")).unwrap(),
        "no question\n"
    );
    rig.adapter.stop().await.unwrap();
}
