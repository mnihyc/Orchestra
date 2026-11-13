//! Turns against the real harness and the scripted model server: text, tools, approvals, files, interrupts and errors.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, kinds, outcome, said};
use futures::StreamExt;
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::descriptor::HarnessSupport;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_testkit::doubles::model::QueueFilter;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_of_plain_text_streams_into_neutral_events_and_ends() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    let started = rig.start_quick().await;
    assert!(
        started.harness_version.starts_with("2.1."),
        "{}",
        started.harness_version
    );
    assert_eq!(started.descriptor.harness.status, HarnessSupport::Supported);
    assert_eq!(started.descriptor.check(), Ok(()));

    let opened = rig.open().await;
    assert!(!opened.resumed);
    let (turn, events) = rig.run("say hi please").await;

    assert_eq!(said(&events), "You said: say hi please");
    let order = kinds(&events);
    let started = events
        .iter()
        .position(|event| matches!(event, SessionEvent::TurnStarted(_)))
        .expect("the actual turn starts");
    assert!(
        events[..started].iter().all(|event| matches!(event,
            SessionEvent::ItemCompleted(item)
                if item.payload["marker"] == "harness_changed"
                    && item.payload["data"]["turn_id"].is_null()
        )),
        "only passive native observations may precede the turn: {order:?}"
    );
    assert!(
        order.contains(&"item.delta"),
        "the text streams in pieces: {order:?}"
    );
    assert_eq!(order.last(), Some(&"turn.completed"));
    let pieces: String = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemDelta(delta) => Some(delta.data.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        pieces, "You said: say hi please",
        "the pieces make the whole"
    );
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let SessionEvent::TurnCompleted(end) = events.last().unwrap() else {
        panic!("the turn ended");
    };
    assert_eq!(end.turn_id, turn);

    // The harness reached the model server through the connector's address, with the connector's token.
    let requests = rig.model.settled(1).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].path,
        "/llm/0123456789abcdef0123456789abcdef/v1/messages"
    );
    assert_eq!(requests[0].body["model"], "mock-large");

    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_absent_resolved_effort_omits_the_native_default_and_clears_a_previous_effort() {
    let mut rig = Rig::new()
        .await
        .expect("the pinned harness image is required");
    rig.start_quick().await;
    rig.open().await;
    for effort in [None, Some("high"), None] {
        let before = rig.model.requests().len();
        let mut turn = rig.turn("hello");
        turn.effort = effort.map(str::to_owned);
        rig.adapter.start_turn(turn).await.unwrap();
        let events = rig.until_the_turn_ends().await;
        assert_eq!(outcome(&events), TurnOutcome::Completed, "{events:?}");
        assert_eq!(said(&events), "You said: hello");
        let requests = rig.model.requests();
        let current: Vec<_> = requests[before..]
            .iter()
            .filter(|request| request.body.get("messages").is_some())
            .collect();
        assert!(!current.is_empty(), "the native turn reached the model");
        for request in current {
            assert_eq!(
                request.body.pointer("/output_config/effort").cloned(),
                effort.map(serde_json::Value::from),
                "the request carries exactly the resolved effort: {:?}",
                request.body
            );
        }
    }
    rig.adapter.stop().await.unwrap();
}

fn completed_calls(events: &[SessionEvent]) -> Vec<&orch_agent_contract::events::ItemCompleted> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(done) if done.payload.get("tool").is_some() => Some(done),
            _ => None,
        })
        .collect()
}

const WRITE_A_FILE: &str = "#!mock\nsay Let me run a command.\ntool Bash {\"command\":\"mkdir -p out && echo hello > out/hello.txt && cat out/hello.txt\",\"description\":\"Write a file\"}\nthen\nsay The command said: {{result}}\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_asks_for_permission_is_allowed_runs_and_its_output_arrives() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let (_, events) = rig.run_deciding(WRITE_A_FILE, common::allow).await;

    let asks: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ApprovalRequested(ask) => Some(ask),
            _ => None,
        })
        .collect();
    assert_eq!(asks.len(), 1, "{:?}", kinds(&events));
    assert_eq!(
        asks[0].request.request_kind,
        orch_agent_contract::events::HarnessRequestKind::Command
    );
    assert!(
        asks[0].title.starts_with("Run mkdir -p out"),
        "{}",
        asks[0].title
    );

    let calls = completed_calls(&events);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].payload["tool"]["kind"], "command");
    assert_eq!(calls[0].payload["tool"]["exit_code"], 0);
    assert_eq!(calls[0].payload["tool"]["output"]["inline"], "hello");
    assert_eq!(calls[0].payload["raw"]["name"], "Bash");
    assert!(
        calls[0].payload["raw"]["input"]["inline"]
            .as_str()
            .unwrap()
            .contains("mkdir -p out"),
        "the call as the harness made it is kept whole"
    );
    assert_eq!(
        std::fs::read_to_string(rig.work.path().join("out/hello.txt")).unwrap(),
        "hello\n",
        "the command ran"
    );
    assert!(
        said(&events).contains("The command said: hello"),
        "{}",
        said(&events)
    );
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_that_is_declined_does_not_run() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let (_, events) = rig.run_deciding(WRITE_A_FILE, common::deny).await;

    let calls = completed_calls(&events);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].status,
        orch_agent_contract::events::ItemStatus::Failed
    );
    assert!(
        calls[0].payload["raw"]["output"]["inline"]
            .as_str()
            .unwrap()
            .contains("The person said no."),
        "the agent is told why: {}",
        calls[0].payload["raw"]["output"]["inline"]
    );
    assert!(!rig.work.path().join("out").exists(), "nothing ran");
    assert_eq!(
        outcome(&events),
        TurnOutcome::Completed,
        "the turn goes on without it"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_that_is_written_and_one_that_is_edited_arrive_as_differences() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    std::fs::write(
        rig.work.path().join("existing.txt"),
        "line one\nline two\nline three\n",
    )
    .unwrap();
    rig.start_quick().await;
    rig.open().await;
    let dir = rig.directory();
    let script = format!(
        "#!mock\nsay I will write a file and edit another.\ntool Write {{\"file_path\":\"{dir}/new.txt\",\"content\":\"alpha\\nbeta\\n\"}}\ntool Edit {{\"file_path\":\"{dir}/existing.txt\",\"old_string\":\"line two\",\"new_string\":\"line 2 changed\"}}\nthen\nsay done\n"
    );
    let (_, events) = rig.run_deciding(&script, common::allow).await;

    let asked: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ApprovalRequested(ask) => Some(ask.request.request_kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        asked,
        vec![
            orch_agent_contract::events::HarnessRequestKind::FileChange,
            orch_agent_contract::events::HarnessRequestKind::FileChange
        ]
    );
    let calls = completed_calls(&events);
    let changes: Vec<_> = calls
        .iter()
        .map(|call| call.payload["tool"]["changes"][0].clone())
        .collect();
    assert_eq!(changes[0]["path"], "new.txt");
    assert_eq!(changes[0]["change"], "add");
    assert_eq!(
        (changes[0]["added"].as_i64(), changes[0]["removed"].as_i64()),
        (Some(2), Some(0))
    );
    assert_eq!(changes[1]["path"], "existing.txt");
    assert_eq!(changes[1]["change"], "modify");
    assert_eq!(
        changes[1]["diff"],
        "--- a/existing.txt\n+++ b/existing.txt\n@@ -1,3 +1,3 @@\n line one\n-line two\n+line 2 changed\n line three\n"
    );
    assert_eq!(
        std::fs::read_to_string(rig.work.path().join("new.txt")).unwrap(),
        "alpha\nbeta\n"
    );
    assert!(
        std::fs::read_to_string(rig.work.path().join("existing.txt"))
            .unwrap()
            .contains("line 2 changed")
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_is_interrupted_ends_interrupted_and_the_session_takes_the_next_turn() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let turn = rig.turn("#!mock\nchunk 4\ndelay 300\nsay This is a very long sentence that streams slowly so that an interrupt can land right in the middle of it, and it goes on and on.\n");
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until(|event| matches!(event, SessionEvent::ItemDelta(_)), 60)
        .await;
    rig.adapter.interrupt(rig.thread, turn_id).await.unwrap();
    let ending = rig.until_the_turn_ends().await;
    assert_eq!(outcome(&ending), TurnOutcome::Interrupted);
    let SessionEvent::TurnCompleted(end) = ending.last().unwrap() else {
        panic!("the turn ended");
    };
    assert!(end.error.is_none());

    // Interrupting again, when nothing runs, is not an error.
    rig.adapter.interrupt(rig.thread, turn_id).await.unwrap();
    let (_, next) = rig.run("hello again").await;
    assert_eq!(said(&next), "You said: hello again");
    assert_eq!(outcome(&next), TurnOutcome::Completed);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failure_of_the_model_ends_the_turn_with_a_typed_error() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let cases = [
        ("error invalid_request", "harness_error", false),
        ("error not_found", "model_unavailable", false),
        ("error authentication", "provider_unauthorized", false),
        ("error billing", "provider_exhausted", false),
        ("error server", "overloaded", true),
        ("error rate_limit retry-after=none", "rate_limited", true),
    ];
    for (script, code, retryable) in cases {
        // A failed turn leaves its message unanswered, and the harness joins it to the next one: each case
        // is a session of its own.
        rig.thread = orch_types::new_id();
        rig.open().await;
        let (_, events) = rig.run(&format!("#!mock\n{script}\n")).await;
        let SessionEvent::TurnCompleted(end) = events.last().unwrap() else {
            panic!("the turn ended");
        };
        assert_eq!(end.status, TurnOutcome::Failed, "{script}");
        let error = end
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("{script}: no error"));
        assert_eq!(
            (error.code.as_str(), error.retryable),
            (code, retryable),
            "{script}: {}",
            error.message
        );
        assert!(!error.message.is_empty());
        assert!(
            said(&events).is_empty(),
            "{script}: the failure is not something the agent said"
        );
    }
    // A session that failed is fine afterwards: the next turn runs in the same process.
    rig.thread = orch_types::new_id();
    rig.open().await;
    // The harness tries a refused request once more without its optional members, so the error is queued
    // for the few requests one turn can make.
    rig.model
        .queue_for("error invalid_request", &QueueFilter::default(), 3)
        .unwrap();
    let (_, failed) = rig.run("this one fails").await;
    assert_eq!(outcome(&failed), TurnOutcome::Failed);
    let (_, events) = rig.run("still here").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert_eq!(said(&events), "You said: still here");
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sub_agent_works_in_a_stream_of_its_own_inside_the_call_that_started_it() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let inner = "#!mock\\nsay Sub-agent speaking.\\ntool Bash {\\\"command\\\":\\\"mkdir -p sub && echo x > sub/x\\\"}\\nthen\\nsay sub done";
    let script = format!(
        "#!mock\nsay Starting a sub-agent.\ntool Agent {{\"description\":\"Look around\",\"prompt\":\"{inner}\",\"subagent_type\":\"general-purpose\",\"run_in_background\":false}}\nthen\nsay The sub-agent said: {{{{result}}}}\n"
    );
    let (_, mut events) = rig.run_deciding(&script, common::allow).await;

    let agent = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemStarted(started)
                if started.native_ref.starts_with("toolu_") && started.stream == "main" =>
            {
                match &started.content {
                    orch_agent_contract::events::ItemContent::ToolCall(call) => match &call.tool {
                        orch_agent_contract::events::ToolPayload::Subagent {
                            child_stream, ..
                        } => Some((started.native_ref.clone(), child_stream.clone())),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        })
        .expect("the call that starts a sub-agent");
    assert_eq!(agent.1, format!("sub:{}", agent.0));

    let in_sub: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(started) if started.stream == agent.1 => Some(started),
            _ => None,
        })
        .collect();
    assert!(
        in_sub.len() >= 2,
        "the sub-agent's words and its command: {}",
        in_sub.len()
    );
    assert!(
        in_sub
            .iter()
            .all(|item| item.parent_native_ref.as_deref() == Some(agent.0.as_str()))
    );
    let child_completions: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(done)
                if in_sub.iter().any(|item| item.native_ref == done.native_ref) =>
            {
                Some(done)
            }
            _ => None,
        })
        .collect();
    assert!(
        std::fs::metadata(rig.work.path().join("sub/x")).is_ok(),
        "its command ran; child completions: {child_completions:#?}"
    );
    if !events.iter().any(
        |event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == agent.0),
    ) {
        // A native task's terminal receipt can follow the main result. Observe its actual
        // close within the rig's existing event budget, without sending another input.
        let parent = agent.0.clone();
        events.extend(rig.until(|event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == parent), 90).await);
    }
    let finished = events.iter().find_map(|event| match event {
        SessionEvent::ItemCompleted(done) if done.native_ref == agent.0 => Some(done),
        _ => None,
    });
    let finished = finished.expect("the call ended");
    assert_eq!(finished.payload["tool"]["state"], "completed");
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert!(
        said(&events).contains("sub done"),
        "the report of the sub-agent reaches the agent: {}",
        said(&events)
    );
    rig.adapter.stop().await.unwrap();
}

/// The transcripts of sub-agents below a harness home.
fn sub_agent_transcripts(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            sub_agent_transcripts(&path, found);
        } else if path
            .parent()
            .and_then(std::path::Path::file_name)
            .is_some_and(|name| name == "subagents")
            && path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.starts_with("agent-") && name.ends_with(".jsonl"))
        {
            found.push(path);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sub_agent_message_of_text_and_a_call_is_settled_from_the_lines_that_carry_its_stop_reason()
 {
    // The case proves the shape of what the pinned program writes: it does not stand in for the image.
    let mut rig = Rig::new()
        .await
        .expect("the pinned harness image is required");
    rig.start_quick().await;
    rig.open().await;
    // The first message of the sub-agent has a text block and a call: the program writes a line for each block.
    let inner = "#!mock\\nsay Sub-agent speaking.\\ntool Bash {\\\"command\\\":\\\"echo x\\\"}\\nthen\\nsay sub done";
    let script = format!(
        "#!mock\nsay Starting a sub-agent.\ntool Agent {{\"description\":\"Look around\",\"prompt\":\"{inner}\",\"subagent_type\":\"general-purpose\",\"run_in_background\":false}}\nthen\nsay The sub-agent said: {{{{result}}}}\n"
    );
    let turn = rig.turn(&script);
    rig.adapter.start_turn(turn).await.unwrap();
    let mut reports = Vec::new();
    let mut ended = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    while !ended {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rig.events.next()).await {
            Ok(Some(AdapterEvent::Session { event, .. })) => {
                if let SessionEvent::ApprovalRequested(ask) = &event {
                    rig.adapter
                        .resolve_approval(rig.thread, &ask.request_ref, common::allow(ask))
                        .await
                        .unwrap();
                }
                ended = matches!(event, SessionEvent::TurnCompleted(_));
            }
            Ok(Some(AdapterEvent::ModelRequest(report))) => reports.push(*report),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => panic!("the turn did not end; reports so far: {reports:#?}"),
        }
    }
    let mut files = Vec::new();
    sub_agent_transcripts(rig.home.path(), &mut files);
    assert_eq!(files.len(), 1, "the sub-agent's transcript: {files:?}");
    // What the program wrote, message by message: the blocks of each, its stop reason, its output count.
    type Written = (Vec<String>, serde_json::Value, serde_json::Value);
    let mut messages: Vec<(String, Vec<Written>)> = Vec::new();
    for line in std::fs::read_to_string(&files[0]).unwrap().lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let message = &entry["message"];
        let (Some(id), "assistant") = (
            message["id"].as_str(),
            entry["type"].as_str().unwrap_or_default(),
        ) else {
            continue;
        };
        let blocks = message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|block| block["type"].as_str().unwrap_or_default().to_owned())
            .collect();
        let written = (
            blocks,
            message["stop_reason"].clone(),
            message["usage"]["output_tokens"].clone(),
        );
        match messages.iter_mut().find(|(known, _)| known == id) {
            Some((_, lines)) => lines.push(written),
            None => messages.push((id.to_owned(), vec![written])),
        }
    }
    assert!(
        messages.iter().any(|(_, lines)| lines.len() >= 2),
        "a message of several blocks is written as a line for each block: {messages:#?}"
    );
    // The settler reads the figure of a message from the lines that carry its stop reason, the one with the most
    // output; a line written before the message ended carries no stop reason, and none may carry more output than
    // the figure that is read. (The pinned program writes every line of a finished message with the stop reason and
    // the closing usage, and a line written before that with neither.)
    for (id, lines) in &messages {
        let figure = lines
            .iter()
            .filter(|line| line.1.is_string())
            .filter_map(|line| line.2.as_i64())
            .max();
        assert!(
            figure.is_some(),
            "a line of {id} carries the stop reason and the closing usage: {messages:#?}"
        );
        assert!(
            lines
                .iter()
                .filter_map(|line| line.2.as_i64())
                .all(|output| Some(output) <= figure),
            "no line of {id} carries more output than the lines that carry its stop reason: {messages:#?}"
        );
        let settled: Vec<_> = reports
            .iter()
            .filter(|report| {
                report.request.subagent
                    && report.request.provider_request_id.as_deref() == Some(id.as_str())
            })
            .collect();
        assert_eq!(
            settled.len(),
            1,
            "the message {id} is reported once, before the turn ends: {reports:#?}"
        );
        assert_eq!(
            settled[0].request.tokens.output_tokens, figure,
            "with the figure of its lines that carry the stop reason: {messages:#?}"
        );
    }
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_added_to_a_running_turn_is_taken_into_it() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let turn = rig.turn("#!mock\nsay Working.\ntool Bash {\"command\":\"sleep 3; echo slept > slept.txt; cat slept.txt\",\"description\":\"wait\"}\nthen\nsay First turn done: {{result}}\n");
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    let asked = rig
        .until(
            |event| matches!(event, SessionEvent::ApprovalRequested(_)),
            60,
        )
        .await;
    let SessionEvent::ApprovalRequested(ask) = asked.last().unwrap() else {
        panic!("asked");
    };
    rig.adapter
        .resolve_approval(rig.thread, &ask.request_ref, common::allow(ask))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    rig.adapter
        .steer(orch_agent_contract::session::TurnSteer {
            thread_id: rig.thread,
            turn_id,
            text: "STEER-MARKER please also mention pineapples".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let events = rig.until_the_turn_ends().await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let requests = rig.model.requests();
    assert_eq!(
        requests.len(),
        2,
        "one more request of the same turn took it in"
    );
    assert!(requests[1].body.to_string().contains("STEER-MARKER"));
    assert!(!requests[0].body.to_string().contains("STEER-MARKER"));

    // Adding to a turn that is over is refused.
    let late = rig
        .adapter
        .steer(orch_agent_contract::session::TurnSteer {
            thread_id: rig.thread,
            turn_id,
            text: "too late".into(),
            images: Vec::new(),
        })
        .await;
    assert!(matches!(
        late,
        Err(orch_agent_contract::adapter::AdapterError::NotFound(_))
    ));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compaction_is_announced_as_a_phase_when_it_begins_and_when_it_is_over() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let script = "#!mock\nusage 198000 50\nsay Reading a lot.\ntool Bash {\"command\":\"mkdir -p c && echo x > c/x\",\"description\":\"x\"}\nthen\nusage 3000 20\nsay Finished after compaction.\n";
    let (_, events) = rig.run_deciding(script, common::allow).await;
    let phases: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TurnPhase(phase) => Some(phase.kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        vec![
            orch_agent_contract::events::TurnPhaseKind::Compacting,
            orch_agent_contract::events::TurnPhaseKind::Working
        ]
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            SessionEvent::ItemStarted(item) if matches!(item.content,
                orch_agent_contract::events::ItemContent::Marker(
                    orch_agent_contract::events::MarkerPayload::Compacted { .. }
                ))
        )),
        "the engine writes the marker, with its duration, when the phase ends"
    );
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_things_the_agent_means_to_do_are_one_item_that_changes_as_they_are_done() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let script = "#!mock\ntool TaskCreate {\"subject\":\"Read the code\",\"description\":\"d\"}\ntool TaskCreate {\"subject\":\"Fix the bug\",\"description\":\"d\"}\nthen\ntool TaskUpdate {\"taskId\":\"1\",\"status\":\"completed\"}\nthen\nsay done\n";
    let (turn, events) = rig.run_deciding(script, common::allow).await;
    let plan = format!("plan:{turn}");
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(item) if item.native_ref == plan => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(
        started.len(),
        1,
        "one item for the turn: {:?}",
        kinds(&events)
    );
    let updates: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemUpdated(update) if update.native_ref == plan => {
                Some(update.payload["steps"].clone())
            }
            _ => None,
        })
        .collect();
    let last = updates.last().expect("the plan changed in place");
    assert_eq!(last[0]["text"], "Read the code");
    assert_eq!(last[0]["status"], "completed");
    assert_eq!(last[1]["status"], "pending");
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    rig.adapter.stop().await.unwrap();
}

/// A picture of one pixel, as a file would hold it.
const PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_of_the_turn_goes_to_the_model_beside_its_text() {
    use base64::Engine as _;
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(PIXEL_PNG)
        .unwrap();
    let path = rig.work.path().join("pixel.png");
    std::fs::write(&path, &bytes).unwrap();
    let mut turn = rig.turn("What is in this picture?");
    turn.images = vec![orch_agent_contract::session::ImageInput {
        blob: orch_types::Hash::of(&bytes),
        media_type: "image/png".into(),
        name: "pixel.png".into(),
        workspace_path: None,
        path: Some(path.display().to_string()),
    }];
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let request = rig.last_request();
    assert!(
        request.contains(r#""type":"image""#) && request.contains(r#""media_type":"image/png""#),
        "the picture arrived as an image block of the message"
    );
    assert!(request.contains("What is in this picture?"));
    rig.adapter.stop().await.unwrap();
}
