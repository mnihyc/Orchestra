//! The pinned native conversation really applies resumed cwd and disabled notes while preserving its history and files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, outcome};
use orch_agent_contract::{
    adapter::AgentAdapter,
    descriptor::ApprovalMode,
    events::{SessionEvent, TurnOutcome},
    session::{NoteKeeper, SessionMemory, SessionMode},
};
use serde_json::{Value, json};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const CORE: &str =
    "CONFIG-FIXED-CORE-BEGIN\nKeep these exact session instructions.\nCONFIG-FIXED-CORE-END";

fn fixed_core(value: &Value) -> Vec<Vec<u8>> {
    match value {
        Value::String(text) => {
            text.find("CONFIG-FIXED-CORE-BEGIN")
                .map_or_else(Vec::new, |start| {
                    assert_eq!(text.matches("CONFIG-FIXED-CORE-BEGIN").count(), 1);
                    let end = start
                        + text[start..]
                            .find("CONFIG-FIXED-CORE-END")
                            .expect("complete fixed core")
                        + "CONFIG-FIXED-CORE-END".len();
                    vec![text.as_bytes()[start..end].to_vec()]
                })
        }
        Value::Array(values) => values.iter().flat_map(fixed_core).collect(),
        Value::Object(values) => values.values().flat_map(fixed_core).collect(),
        _ => Vec::new(),
    }
}

fn tools(body: &Value) -> Vec<u8> {
    serde_json::to_vec(body.get("tools").expect("actual native tools")).unwrap()
}

fn request(rig: &Rig) -> Value {
    rig.model
        .requests()
        .into_iter()
        .rfind(|request| request.format.is_some())
        .expect("actual controlled model input")
        .body
}

fn command(rig: &Rig, marker: &str) -> String {
    let counter = rig.work.path().join("tool-count");
    format!(
        "#!mock\nsay {marker}\ntool Bash {}\nthen\nsay {marker} complete: {{{{result}}}}\n",
        json!({"command":format!("echo call >> '{}' && pwd",counter.display()),"description":"Count this real call and report the actual cwd"})
    )
}

// Rig::run intentionally defaults each turn to Ask. This controlled configuration
// proof must keep the same Bypass policy it explicitly installed in SessionSettings.
async fn run_config(rig: &mut Rig, text: &str) -> Proof<Vec<SessionEvent>> {
    let mut turn = rig.turn(text);
    turn.approval_mode = ApprovalMode::Bypass;
    rig.adapter.start_turn(turn).await?;
    let events = rig.until_the_turn_ends().await; // Retain the ordinary 90-second native watchdog.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_))),
        "the accepted controlled policy must remain the same for this turn"
    );
    Ok(events)
}

fn actual_cwd(events: &[SessionEvent], cwd: &str) {
    assert!(events.iter().any(|event|matches!(event,SessionEvent::ItemCompleted(item)
        if item.payload["tool"]["kind"]=="command" && item.payload["tool"]["cwd"]==cwd
            && item.payload["tool"]["output"]["inline"].as_str().is_some_and(|text|text.lines().any(|line|line==cwd)))),
        "the actual native command must run in the accepted directory");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cwd_only_resume_changes_the_actual_process_once_and_preserves_the_native_conversation()
-> Proof {
    let mut rig = Rig::new()
        .await
        .expect("the pinned native image/container is required; this proof never silently skips");
    assert_eq!(rig.start_quick().await.harness_version, "2.1.296");
    let child = rig.work.path().join("child");
    std::fs::create_dir_all(&child)?;
    let fixed = rig.work.path().join("human-data.bin");
    std::fs::write(&fixed, b"unchanged workspace\0\xff")?;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.instructions = CORE.into();
    spec.settings.raw_events = true;
    spec.settings.approval_mode = ApprovalMode::Bypass;
    let opened = rig.open_with(spec.clone()).await;
    let script = command(&rig, "CONFIG-FIRST");
    let events = run_config(&mut rig, &script).await?;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    actual_cwd(&events, &rig.directory());
    let path = orch_adapter_claude_code::history::find_transcript(
        rig.home.path(),
        &opened.native_session_ref,
    )
    .ok_or("the actual source transcript is absent")?;
    let original = std::fs::read(&path)?;
    let first_tools = tools(&request(&rig));
    let count = rig.model.requests().len();
    let launches = rig.launcher.launched.lock().unwrap().len();
    spec.settings.mode = SessionMode::Resume;
    spec.settings.native_session_ref = Some(opened.native_session_ref.clone());
    spec.settings.cwd = child.display().to_string();
    // An in-place reconfiguration returns its acknowledgement; it does not emit a second
    // SessionOpened. Bound that acknowledgement by the existing open helper's 20 seconds.
    let resumed = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        rig.adapter.open_session(spec.clone()),
    )
    .await??;
    assert!(resumed.resumed);
    assert_eq!(resumed.native_session_ref, opened.native_session_ref);
    assert_eq!(
        rig.model.requests().len(),
        count,
        "reconfiguration submits no model input"
    );
    orch_testkit::prefix_history::claude_source_preserved(
        &original,
        &std::fs::read(&path)?,
        &opened.native_session_ref,
    )?;
    let script = command(&rig, "CONFIG-CHANGED-CWD");
    let events = run_config(&mut rig, &script).await?;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    actual_cwd(&events, &spec.settings.cwd);
    assert_eq!(
        rig.launcher.launched.lock().unwrap().len(),
        launches + 1,
        "one normal resumed process applies the new setting"
    );
    let body = request(&rig);
    assert!(body.to_string().contains("CONFIG-FIRST"));
    assert_eq!(fixed_core(&body), vec![CORE.as_bytes().to_vec()]);
    assert_eq!(tools(&body), first_tools);
    assert!(std::fs::read(&path)?.starts_with(&original));
    let script = command(&rig, "CONFIG-UNCHANGED-CWD");
    let events = run_config(&mut rig, &script).await?;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    actual_cwd(&events, &spec.settings.cwd);
    assert_eq!(
        rig.launcher.launched.lock().unwrap().len(),
        launches + 2,
        "the unchanged turn keeps only the existing local-file reread"
    );
    assert_eq!(
        std::fs::read_to_string(rig.work.path().join("tool-count"))?,
        "call\ncall\ncall\n",
        "old native tools never replay"
    );
    assert_eq!(std::fs::read(fixed)?, b"unchanged workspace\0\xff");
    assert!(
        !rig.work.path().join("AGENTS.md").exists() && !rig.work.path().join("CLAUDE.md").exists()
    );
    rig.adapter.stop().await?;
    rig.prefix_proof_succeeded();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabling_notes_on_resume_disables_real_native_memory_without_deleting_notes_or_original_history()
-> Proof {
    let mut rig = Rig::new()
        .await
        .expect("the pinned native image/container is required; this proof never silently skips");
    assert_eq!(rig.start_quick().await.harness_version, "2.1.296");
    let notes = rig.work.path().join(".orch/memory");
    std::fs::create_dir_all(&notes)?;
    let file = notes.join("MEMORY.md");
    std::fs::write(&file, b"CONFIG-ORIGINAL-NOTE\n")?;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.instructions = CORE.into();
    spec.settings.raw_events = true;
    spec.settings.approval_mode = ApprovalMode::Bypass;
    spec.settings.memory = Some(SessionMemory {
        directory: notes.display().to_string(),
        kept_by: NoteKeeper::Harness,
    });
    let opened = rig.open_with(spec.clone()).await;
    let events = run_config(&mut rig, "#!mock\nsay CONFIG-MEMORY-FIRST\n").await?;
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
        .ok_or("native memory init absent")?;
    assert_eq!(
        init["memory_paths"]["auto"],
        format!("{}/", notes.display())
    );
    let path = orch_adapter_claude_code::history::find_transcript(
        rig.home.path(),
        &opened.native_session_ref,
    )
    .ok_or("native source absent")?;
    let original = std::fs::read(&path)?;
    let first_tools = tools(&request(&rig));
    // A person changes the notes after the first model input. Disabled memory must not load these unseen bytes.
    std::fs::write(&file, b"CONFIG-UNSEEN-NOTES-WHILE-OFF\n")?;
    let count = rig.model.requests().len();
    let launches = rig.launcher.launched.lock().unwrap().len();
    spec.settings.mode = SessionMode::Resume;
    spec.settings.native_session_ref = Some(opened.native_session_ref.clone());
    spec.settings.memory = None;
    // Same native conversation: use the actual reconfigure acknowledgement, not a new-session event.
    let resumed = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        rig.adapter.open_session(spec),
    )
    .await??;
    assert!(resumed.resumed);
    assert_eq!(resumed.native_session_ref, opened.native_session_ref);
    assert_eq!(rig.model.requests().len(), count);
    orch_testkit::prefix_history::claude_source_preserved(
        &original,
        &std::fs::read(&path)?,
        &opened.native_session_ref,
    )?;
    let events = run_config(&mut rig, "#!mock\nsay CONFIG-MEMORY-OFF\n").await?;
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
        .ok_or("resumed native init absent")?;
    assert!(
        init.get("memory_paths").is_none(),
        "the native process confirms disabled automatic memory"
    );
    {
        let launches_now = rig.launcher.launched.lock().unwrap();
        assert_eq!(launches_now.len(), launches + 1);
        assert_eq!(
            launches_now
                .last()
                .ok_or("resumed launch absent")?
                .env
                .get("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
                .map(String::as_str),
            Some("1")
        );
    }
    let body = request(&rig);
    assert!(body.to_string().contains("CONFIG-MEMORY-FIRST"));
    assert!(!body.to_string().contains("CONFIG-UNSEEN-NOTES-WHILE-OFF"));
    assert_eq!(fixed_core(&body), vec![CORE.as_bytes().to_vec()]);
    assert_eq!(tools(&body), first_tools);
    assert!(std::fs::read(&path)?.starts_with(&original));
    let events = run_config(&mut rig, "#!mock\nsay CONFIG-MEMORY-UNCHANGED\n").await?;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert_eq!(rig.launcher.launched.lock().unwrap().len(), launches + 2);
    assert_eq!(std::fs::read(file)?, b"CONFIG-UNSEEN-NOTES-WHILE-OFF\n");
    assert!(
        !rig.work.path().join("AGENTS.md").exists() && !rig.work.path().join("CLAUDE.md").exists()
    );
    rig.adapter.stop().await?;
    rig.prefix_proof_succeeded();
    Ok(())
}

/// The native question remains a person-content request beside local tools and through the executor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_questions_reach_the_person_and_return_content_in_each_admitted_mode_and_placement()
-> Proof {
    use common::mcp::McpServer;
    use orch_agent_contract::descriptor::ArrangementType;
    use orch_agent_contract::events::{HarnessRequestDetail, HarnessRequestKind};
    use orch_agent_contract::loopback::Loopback;
    use orch_agent_contract::session::{ApprovalDecision, ProviderAccess};
    use orch_plugin_contract::Secret;
    use std::collections::BTreeMap;

    for separated in [false, true] {
        let mut rig = Rig::new()
            .await
            .expect("the pinned native image/container is required");
        let server = if separated {
            Some(
                McpServer::start(
                    &[("exec", &[("Read", "Read a workspace file.")][..])],
                    |_, _| ("This question should not run a workspace tool".into(), true),
                )
                .await,
            )
        } else {
            None
        };
        let started = if let Some(server) = &server {
            rig.start_beside(Loopback::new(
                server.address.port(),
                Secret::new(common::TOKEN),
            ))
            .await
        } else {
            rig.start_quick().await
        };
        assert_eq!(started.harness_version, "2.1.296");
        for mode in [
            ApprovalMode::Ask,
            ApprovalMode::OnRequest,
            ApprovalMode::Bypass,
        ] {
            rig.new_lane();
            let mut spec = rig.spec(SessionMode::New, None);
            spec.settings.approval_mode = mode;
            if separated {
                spec.exec.executor = Some("orch-exec".into());
                spec.exec.tool_set = Some("claude-code".into());
                spec.settings.arrangement = ArrangementType::Separated;
                spec.provider = ProviderAccess::Native {
                    provider_id: orch_types::new_id(),
                };
            }
            rig.open_with(spec).await;
            let text = "#!mock\ntool AskUserQuestion {\"questions\":[{\"question\":\"Which value?\",\"header\":\"Choice\",\"multiSelect\":false,\"options\":[{\"label\":\"First\",\"description\":\"First value\"},{\"label\":\"Second\",\"description\":\"Second value\"}]}]}\nthen\nsay The person's answer was received\n";
            let mut turn = rig.turn(text);
            turn.approval_mode = mode;
            let before = rig.model.requests().len();
            rig.adapter.start_turn(turn).await?;
            // Keep the rig's established native turn watchdog.
            let mut events = rig
                .until(
                    |event| {
                        matches!(
                            event,
                            SessionEvent::ApprovalRequested(_) | SessionEvent::TurnCompleted(_)
                        )
                    },
                    90,
                )
                .await;
            let asked = events
                .iter()
                .find_map(|event| {
                    if let SessionEvent::ApprovalRequested(request) = event {
                        Some(request.clone())
                    } else {
                        None
                    }
                })
                .expect("the native program must ask the person");
            assert_eq!(asked.request.request_kind, HarnessRequestKind::UserInput);
            let HarnessRequestDetail::UserInput { questions, .. } = &asked.request.detail else {
                panic!("a native content question")
            };
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].id, "q1");
            assert!(questions[0].allow_other);
            let source = asked
                .request
                .detail
                .native_item_ref()
                .expect("native question source correlation")
                .to_owned();
            let chosen = format!("The person's own choice for {mode:?} separated={separated}");
            rig.adapter
                .resolve_approval(
                    rig.thread,
                    &asked.request_ref,
                    ApprovalDecision::Answers {
                        answers: BTreeMap::from([("q1".into(), vec![chosen.clone()])]),
                        note: None,
                    },
                )
                .await?;
            events.extend(rig.until_the_turn_ends().await);
            assert_eq!(outcome(&events), TurnOutcome::Completed);
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
                    .count(),
                1
            );
            assert!(events.iter().any(|event| matches!(event, SessionEvent::ItemStarted(item) if item.native_ref == source)
                || matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == source)), "native source item must remain available for the question's work detail");
            let received = rig
                .model
                .requests()
                .into_iter()
                .skip(before)
                .any(|request| {
                    request
                        .body
                        .get("messages")
                        .and_then(Value::as_array)
                        .is_some_and(|messages| {
                            messages.iter().any(|message| {
                                message
                                    .get("content")
                                    .and_then(Value::as_array)
                                    .is_some_and(|parts| {
                                        parts.iter().any(|part| {
                                            part["type"] == "tool_result"
                                                && part["tool_use_id"].as_str()
                                                    == Some(source.as_str())
                                                && part["content"].to_string().contains(&chosen)
                                        })
                                    })
                            })
                        })
                });
            assert!(
                received,
                "the actual native model continuation must contain this person's free-text answer"
            );
            println!(
                "{}",
                json!({"native_question":true,"adapter":"claude-code","version":started.harness_version,"separated":separated,"mode":format!("{mode:?}"),"answer_returned":true,"source_correlated":true})
            );
        }
        if let Some(server) = &server {
            assert!(
                server
                    .seen()
                    .iter()
                    .any(|request| request.server == "exec" && request.method == "tools/list"),
                "the separated harness must load its actual executor tool surface"
            );
            assert!(
                server.calls().is_empty(),
                "asking a person does not run a workspace tool"
            );
        }
        rig.adapter.stop().await?;
    }
    Ok(())
}
