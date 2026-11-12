//! The real pinned CLI's completion follow-up waits for controller turn ownership.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native regression assertions"
)]
mod common;
use common::Rig;
use futures::StreamExt as _;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::SessionEvent;

fn stopped_notice(frame: &serde_json::Value, task: &str, tool: &str) -> bool {
    let native_origin = frame["origin"]["kind"] == "task-notification"
        || (frame["origin"].is_null() && frame["isSynthetic"] == true);
    if frame["type"] != "user" || !native_origin {
        return false;
    }
    let texts: Vec<_> = match &frame["message"]["content"] {
        serde_json::Value::String(text) => vec![text.as_str()],
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect(),
        _ => Vec::new(),
    };
    let task_tag = format!("<task-id>{task}</task-id>");
    let tool_tag = format!("<tool-use-id>{tool}</tool-use-id>");
    texts
        .iter()
        .any(|text| text.contains(&task_tag) && text.contains(&tool_tag))
}

fn stop_reply_source(
    frame: &serde_json::Value,
    session: &str,
    task: &serde_json::Value,
    tool: &str,
) -> bool {
    frame["session_id"] == session
        && frame["parent_tool_use_id"].is_null()
        && (stopped_notice(frame, task[0].as_str().unwrap(), tool)
            || (frame["type"] == "result"
                && frame["origin"]["kind"] == "task-notification"
                && task[1].as_str().is_some()
                && frame["origin"]["runId"] == task[1]))
}

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
    let mut spec = rig.spec(orch_agent_contract::session::SessionMode::New, None);
    spec.settings.raw_events = true;
    let opened = rig.open_with(spec).await;
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
    // Native context inspection may count tokens without asking the model to generate.
    let prompts = |rows: Vec<orch_testkit::doubles::model::RecordedRequest>| {
        rows.into_iter()
            .filter(|row| row.format.is_some() && !row.path.ends_with("/messages/count_tokens"))
            .count()
    };
    let before = rig.model.requests();
    let requests = prompts(before.clone());
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
    let mut after = Vec::new();
    let mut measured: Vec<Box<orch_agent_contract::requests::ModelRequestReport>> = Vec::new();
    let key: serde_json::Value = serde_json::from_str(&task.id).unwrap();
    if child {
        // A synthetic input may be represented by its run-bound result alone.
        // Wait for the reply source and its completed accounting observation.
        tokio::time::timeout(std::time::Duration::from_secs(90), async {
            loop {
                let source = after.iter().find_map(|event| match event {
                    SessionEvent::RawFrame(raw)
                        if raw.direction == orch_adapter_claude_code::events::FROM_HARNESS
                            && stop_reply_source(
                                &raw.payload,
                                &opened.native_session_ref,
                                &key,
                                task.native_item_ref.as_deref().unwrap(),
                            ) =>
                    {
                        raw.payload["uuid"].as_str()
                    }
                    _ => None,
                });
                if source.is_some_and(|source| {
                    let prefix = format!("native:{source}:");
                    measured.iter().any(|report| {
                        report.request.id.starts_with(&prefix) && report.request.outcome.is_some()
                    })
                }) {
                    break;
                }
                match rig
                    .events
                    .next()
                    .await
                    .expect("the native event reader remains live")
                {
                    orch_agent_contract::adapter::AdapterEvent::Session { event, .. } => {
                        after.push(event)
                    }
                    orch_agent_contract::adapter::AdapterEvent::ModelRequest(report) => {
                        measured.push(report)
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or_else(|error| {
            let sources: Vec<_> = after.iter().filter_map(|event| match event {
                SessionEvent::RawFrame(raw)
                    if raw.direction == orch_adapter_claude_code::events::FROM_HARNESS =>
                {
                    Some(serde_json::json!({
                        "type": raw.payload["type"],
                        "subtype": raw.payload["subtype"],
                        "event": raw.payload["event"]["type"],
                        "session": raw.payload["session_id"],
                        "uuid": raw.payload["uuid"],
                        "origin": raw.payload["origin"]["kind"],
                        "run": raw.payload["origin"]["runId"],
                    }))
                }
                _ => None,
            }).collect();
            let requests: Vec<_> = measured.iter().map(|report| {
                (&report.request.id, &report.request.provider_request_id, report.request.outcome.is_some())
            }).collect();
            panic!("native stop reply and completed accounting were expected: {error}; task={key}; sources={sources:?}; requests={requests:?}");
        });
    }
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_millis(1_000), rig.events.next()).await
    {
        match event {
            orch_agent_contract::adapter::AdapterEvent::Session { event, .. } => after.push(event),
            orch_agent_contract::adapter::AdapterEvent::ModelRequest(report) => {
                measured.push(report)
            }
            _ => {}
        }
    }
    assert!(!after.iter().any(|event| matches!(event, SessionEvent::BackgroundTasks(report) if report.native_wake.is_some())));
    assert!(
        !after.iter().any(|event| matches!(
            event,
            SessionEvent::TurnStarted(_) | SessionEvent::TurnCompleted(_)
        )),
        "End admits no continuation turn"
    );
    assert!(
        !after.iter().any(|event| {
            matches!(event, SessionEvent::ItemStarted(item)
            if item.stream == orch_agent_contract::events::MAIN_STREAM
                && !matches!(&item.content,
                    orch_agent_contract::events::ItemContent::Marker(_)
                        | orch_agent_contract::events::ItemContent::Error(_)))
        }),
        "End publishes no unadmitted model text, reasoning or tool call"
    );
    assert!(
        !after.iter().any(|event| match event {
            SessionEvent::ItemUpdated(item) =>
                item.payload.get("text").is_some()
                    || item.payload.get("summary").is_some()
                    || item.payload.get("steps").is_some(),
            SessionEvent::ItemCompleted(item) =>
                item.payload.get("text").is_some()
                    || item.payload.get("summary").is_some()
                    || item.payload.get("steps").is_some(),
            SessionEvent::ItemDelta(delta) => matches!(
                delta.field,
                orch_agent_contract::events::DeltaField::Text
                    | orch_agent_contract::events::DeltaField::Summary
            ),
            _ => false,
        }),
        "a native cancellation response cannot leak through updates, completions or text deltas"
    );
    let outbound: Vec<_> = events
        .iter()
        .chain(&after)
        .filter_map(|event| match event {
            SessionEvent::RawFrame(raw)
                if raw.direction == orch_adapter_claude_code::events::TO_HARNESS =>
            {
                Some(&raw.payload)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        outbound
            .iter()
            .filter(|frame| frame["type"] == "user")
            .count(),
        1,
        "only the original authored input is written to the native session"
    );
    let stop: Vec<_> = outbound
        .iter()
        .filter(|frame| {
            frame
                .pointer("/request/subtype")
                .and_then(serde_json::Value::as_str)
                == Some("stop_task")
        })
        .collect();
    assert_eq!(
        stop.len(),
        1,
        "the repeat must not submit another native End"
    );
    assert_eq!(stop[0]["request"]["task_id"], key[0]);
    for event in &after {
        if let SessionEvent::BackgroundTasks(report) = event {
            for actual in &report.tasks {
                if serde_json::json!([actual.id, actual.run_id]) == key {
                    assert!(
                        !actual.state.wakes(),
                        "the native stop must never transiently become a waking failure"
                    );
                }
            }
        }
    }
    let recorded = rig.model.requests();
    let request_trace = |rows: &[orch_testkit::doubles::model::RecordedRequest]| {
        rows.iter()
            .map(|row| {
                // RecordedRequest already masks credentials. The last message distinguishes
                // a child tool result, a parent task notification and an unrelated request.
                serde_json::json!({
                    "seq": row.seq,
                    "path": row.path,
                    "model": row.model,
                    "script": row.script,
                    "round": row.round,
                    "status": row.status,
                    "outcome": row.outcome,
                    "last_message": row.body.get("messages")
                        .or_else(|| row.body.get("input"))
                        .and_then(serde_json::Value::as_array)
                        .and_then(|messages| messages.last()),
                })
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        prompts(recorded.clone()),
        requests + usize::from(child),
        "only the pinned subagent cancellation notice may add a native call; before: {:#?}; after: {:#?}; native reports after End: {after:#?}",
        request_trace(&before),
        request_trace(&recorded),
    );
    if child {
        let native: Vec<_> = after
            .iter()
            .filter_map(|event| match event {
                SessionEvent::RawFrame(raw)
                    if raw.direction == orch_adapter_claude_code::events::FROM_HARNESS =>
                {
                    Some(&raw.payload)
                }
                _ => None,
            })
            .collect();
        let terminal = native
            .iter()
            .find(|frame| {
                frame["type"] == "system"
                    && frame["subtype"] == "task_notification"
                    && frame["task_id"] == key[0]
                    && frame["run_id"] == key[1]
                    && frame["status"] == "stopped"
            })
            .expect("the actual matching native stop receipt is retained");
        let output = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::RawFrame(raw)
                    if raw.direction == orch_adapter_claude_code::events::FROM_HARNESS
                        && raw.payload["tool_use_result"]["agentId"] == key[0] =>
                {
                    raw.payload["tool_use_result"]["outputFile"].as_str()
                }
                _ => None,
            })
            .filter(|path| !path.is_empty())
            .expect("the actual native launch's output-file identity");
        assert_eq!(terminal["status"], "stopped");
        let source = native
            .iter()
            .find(|frame| stop_reply_source(frame, &opened.native_session_ref, &key, task.native_item_ref.as_deref().unwrap()))
            .unwrap_or_else(|| {
                let frames: Vec<_> = native.iter().filter(|frame|
                    frame["type"] == "user" || frame["type"] == "result").collect();
                panic!("the native cancellation reply must identify the stopped run; user/result frames: {frames:#?}")
            });
        if let Some(run) = source
            .pointer("/origin/runId")
            .filter(|value| !value.is_null())
        {
            assert_eq!(
                run, &key[1],
                "a supplied native run id must match the exact stopped run"
            );
        }
        let source_uuid = source["uuid"]
            .as_str()
            .expect("the native reply source UUID");
        let mut expected: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/cancellation-notice-2.1.319.json"))
                .unwrap();
        let text = expected["content"][1]["text"]
            .as_str()
            .unwrap()
            .replace("<NATIVE_OUTPUT_FILE>", output)
            .replace("<NATIVE_TASK_ID>", key[0].as_str().unwrap())
            .replace("<NATIVE_TOOL_ID>", task.native_item_ref.as_deref().unwrap());
        expected["content"][1]["text"] = serde_json::json!(text);
        let extra: Vec<_> = recorded
            .iter()
            .filter(|row| {
                row.seq > before.last().unwrap().seq
                    && row.format.is_some()
                    && !row.path.ends_with("/messages/count_tokens")
            })
            .collect();
        assert_eq!(extra.len(), 1);
        assert_eq!(
            extra[0].body["messages"].as_array().unwrap().last(),
            Some(&expected),
            "the entire added model message is the exact pinned native cancellation notice"
        );
        let message = native
            .iter()
            .find_map(|frame| {
                if frame["type"] == "stream_event"
                    && frame["parent_tool_use_id"].is_null()
                    && frame["event"]["type"] == "message_start"
                {
                    frame["event"]["message"]["id"].as_str()
                } else {
                    None
                }
            })
            .expect("the native cancellation call has its actual model-message identity");
        let usage = measured
            .iter()
            .find(|report| {
                report.request.provider_request_id.as_deref() == Some(message)
                    && report.request.outcome.is_some()
            })
            .expect("the native response remains a complete first-class request observation");
        assert_eq!(usage.thread_id, rig.thread);
        assert_eq!(usage.segment_id, rig.segment);
        assert_eq!(usage.turn_id, origin);
        let source_prefix = format!("native:{source_uuid}:");
        assert!(
            usage.request.id.starts_with(source_prefix.as_str()),
            "the ledger observation retains the exact native reply source identity"
        );
        assert!(usage.request.check().is_ok());
        assert!(
            usage.request.asked.is_none(),
            "no Orch model-input time is invented"
        );
        assert!(usage.request.tokens.input_tokens.is_some());
        assert!(
            usage
                .request
                .tokens
                .output_tokens
                .is_some_and(|tokens| tokens > 0)
        );
    }
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
