//! Ordinary Orch tools reach the pinned harness without a second approval, with truthful destructive annotations.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[path = "prompt_cases/byte_stability.rs"]
mod byte_stability;
mod common;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{OrchTool, SessionEvent, TurnOutcome};
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::SessionMode;
use orch_plugin_contract::Secret;
use orch_types::new_id;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use orch_adapter_claude_code::{
    ClaudeCodeAdapter,
    process::{LaunchSpec, Launcher},
};
#[derive(Debug)]
struct Docker {
    mounts: Vec<PathBuf>,
    names: Mutex<Vec<String>>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instructions_sources_claude_reports_imports_and_later_nested_loads_without_an_input() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned harness image is required for this proof");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    let root = rig.work.path().to_owned();
    std::fs::create_dir_all(root.join("lazy")).unwrap();
    std::fs::write(root.join("CLAUDE.md"), "ROOT-RULE\n").unwrap();
    std::fs::write(root.join("lazy/policy.md"), "IMPORTED-RULE\n").unwrap();
    // Initial reads can precede SDK hook registration. Demand-load both the nested instruction
    // and its import after registration; never invent a reported startup event from a file's text.
    std::fs::write(root.join("lazy/CLAUDE.md"), "NESTED-RULE\n@policy.md\n").unwrap();
    std::fs::write(root.join("lazy/example.txt"), "the file requested\n").unwrap();
    rig.start_quick().await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.workspace_root = root.display().to_string();
    spec.settings.cwd = root.display().to_string();
    spec.settings.directory = root.display().to_string();
    let opened = rig.open_with(spec).await;
    let script = format!(
        "#!mock\ntool Read {}\nthen\nsay complete\n",
        json!({"file_path":root.join("lazy/example.txt")})
    );
    let (_, mut events) = rig.run(&script).await;
    events.extend(rig.drain(200).await);
    assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    let mut files = opened.instruction_sources;
    files.extend(
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::InstructionSources(sources) => Some(&sources.sources),
                _ => None,
            })
            .flatten()
            .cloned(),
    );
    assert!(
        files.iter().any(|file| file.path.as_deref()
            == Some(root.join("lazy/policy.md").to_str().unwrap())
            && file.evidence == orch_agent_contract::session::InstructionEvidence::Reported
            && file.load_reason.as_deref() == Some("include")
            && file.parent_path.as_deref() == Some(root.join("lazy/CLAUDE.md").to_str().unwrap())),
        "the real pinned loader reports its import: {files:?}"
    );
    assert!(
        files.iter().any(|file| file.path.as_deref()
            == Some(root.join("lazy/CLAUDE.md").to_str().unwrap())
            && file.load_reason.as_deref() == Some("nested_traversal")),
        "the file read triggers the native lazy load: {files:?}"
    );
    assert!(
        files
            .iter()
            .all(|file| !file.home || (file.path.is_none() && file.bytes.is_none()))
    );
    let request = rig.last_request();
    assert!(request.contains("IMPORTED-RULE") && request.contains("NESTED-RULE"));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::TurnStarted(_)))
            .count(),
        1,
        "observing the loads starts no additional work"
    );
    rig.adapter.stop().await.unwrap();
}
impl Drop for Docker {
    fn drop(&mut self) {
        for name in self.names.lock().unwrap().iter() {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", name])
                .output();
        }
    }
}
impl Launcher for Docker {
    fn command(&self, spec: &LaunchSpec) -> tokio::process::Command {
        let name = format!("orchprobe-prompts-claude-{}", new_id().simple());
        self.names.lock().unwrap().push(name.clone());
        let mut command = tokio::process::Command::new("docker");
        command.args(["run", "-i", "--rm", "--name", &name, "--network", "host"]);
        for mount in &self.mounts {
            command.arg("-v").arg(format!("{0}:{0}", mount.display()));
        }
        command.arg("-w").arg(&spec.cwd);
        for (name, value) in &spec.env {
            if ![
                "PATH",
                "HOME",
                "USER",
                "LOGNAME",
                "SHELL",
                "TMPDIR",
                "XDG_RUNTIME_DIR",
                "TERM",
                "NO_COLOR",
            ]
            .contains(&name.as_str())
            {
                command.arg("-e").arg(format!("{name}={value}"));
            }
        }
        command
            .arg("orch-dev-harness")
            .arg(&spec.program)
            .args(&spec.args);
        command
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_no_approval_claude_runs_destructive_calls_in_each_ordinary_mode() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned harness image is required for this proof");
    let launcher = Arc::new(Docker {
        mounts: vec![rig.home.path().into(), rig.work.path().into()],
        names: Mutex::new(Vec::new()),
    });
    rig.adapter =
        ClaudeCodeAdapter::with_launcher(Arc::new(orch_types::SystemClock), launcher.clone());
    let server = server(rig.model.url(), true).await;
    rig.start_beside(Loopback::new(server.port, Secret::new(common::TOKEN)))
        .await;
    for mode in ORDINARY_MODES {
        rig.new_lane();
        let mut spec = rig.spec(SessionMode::New, None);
        spec.settings.approval_mode = mode;
        spec.settings.orch_tools = result_tools().into_iter().map(|(tool, _)| tool).collect();
        rig.open_with(spec).await;
        for (_, name) in result_tools() {
            let mut turn = rig.turn(&format!(
                "#!mock\ntool mcp__orch__{name} {{}}\nthen\nsay complete\n"
            ));
            turn.approval_mode = mode;
            rig.adapter.start_turn(turn).await.unwrap();
            let events = rig.until_the_turn_ends().await;
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, SessionEvent::ApprovalRequested(_))),
                "{mode:?} {name}"
            );
            assert_eq!(
                common::outcome(&events),
                TurnOutcome::Completed,
                "{mode:?} {name}: {events:?}"
            );
        }
    }
    assert_eq!(
        server.calls.lock().unwrap().len(),
        ORDINARY_MODES.len() * result_tools().len()
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_result_form_claude_delivers_read_text_structured_effects_and_one_error_copy() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned harness image is required for this proof");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    let server = server(rig.model.url(), true).await;
    let started = rig
        .start_beside(Loopback::new(server.port, Secret::new(common::TOKEN)))
        .await;
    eprintln!(
        "tools.result_form.claude pinned {}",
        started.harness_version
    );
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.orch_tools = result_tools().into_iter().map(|(tool, _)| tool).collect();
    rig.open_with(spec).await;
    for (_, name) in result_tools() {
        rig.adapter
            .start_turn(rig.turn(&format!(
                "#!mock\ntool mcp__orch__{name} {{}}\nthen\nsay complete\n"
            )))
            .await
            .unwrap();
        assert_eq!(
            common::outcome(&rig.until_the_turn_ends().await),
            TurnOutcome::Completed
        );
        let request = rig.model.requests().last().unwrap().body.to_string();
        assert_eq!(
            request.matches(&format!("RESULT-FORM-{name}")).count(),
            1,
            "{name}: {request}"
        );
    }
    rig.adapter
        .start_turn(
            rig.turn("#!mock\ntool mcp__orch__read_shared {\"refuse\":true}\nthen\nsay complete\n"),
        )
        .await
        .unwrap();
    assert_eq!(
        common::outcome(&rig.until_the_turn_ends().await),
        TurnOutcome::Completed
    );
    let request = rig.model.requests().last().unwrap().body.to_string();
    assert_eq!(request.matches("RESULT-FORM-ERROR").count(), 1, "{request}");
    // Each tool once, and the refused read.
    assert_eq!(server.calls.lock().unwrap().len(), result_tools().len() + 1);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_answer_budget_claude_receives_the_whole_default_and_maximum_in_both_scripts() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned image is required");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    let server = server(rig.model.url(), true).await;
    let started = rig
        .start_beside(Loopback::new(server.port, Secret::new(common::TOKEN)))
        .await;
    eprintln!(
        "tools.answer_budget.claude pinned {}",
        started.harness_version
    );
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.orch_tools = vec![OrchTool::ReadShared];
    spec.settings.instructions = "BUDGET-PREAMBLE\nBUDGET-T2-FALLBACK".into();
    rig.open_with(spec).await;
    for script in ["latin", "chinese"] {
        for limit in [None, Some(1000), Some(40000)] {
            let mut arguments = json!({"budget_probe":true,"script":script});
            if let Some(limit) = limit {
                arguments["max_bytes"] = json!(limit);
            }
            let expected = budget_text(&arguments);
            rig.adapter
                .start_turn(rig.turn(&format!(
                    "#!mock\ntool mcp__orch__read_shared {arguments}\nthen\nsay complete\n"
                )))
                .await
                .unwrap();
            let events = rig.until_the_turn_ends().await;
            assert_eq!(common::outcome(&events), TurnOutcome::Completed);
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
            );
            let request = rig.last_request();
            assert!(
                request.contains(&expected),
                "{script}/{limit:?}: the complete {}-byte result must be in the model request, never only a runtime file",
                expected.len()
            );
            assert_eq!(request.matches("BUDGET-T2-FALLBACK").count(), 1);
            assert!(
                has_text(
                    &rig.model.requests().last().unwrap().body,
                    &tool_description("read_shared")
                ),
                "the complete T10 description reaches the model after native tool discovery"
            );
        }
    }
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_timeout_claude_receives_the_connector_outcome_before_its_native_deadline() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned image is required");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    let server = server(rig.model.url(), true).await;
    rig.start_beside(Loopback::new(server.port, Secret::new(common::TOKEN)))
        .await;
    for tool in [OrchTool::ReadShared, OrchTool::OpenPort] {
        rig.new_lane();
        let mut spec = rig.spec(SessionMode::New, None);
        spec.settings.orch_tools = vec![tool];
        rig.open_with(spec).await;
        let before = std::time::Instant::now();
        let (_, events) = rig
            .run(&format!(
                "#!mock\ntool mcp__orch__{} {{\"timeout_probe\":true}}\nthen\nsay complete\n",
                tool.name()
            ))
            .await;
        assert_eq!(common::outcome(&events), TurnOutcome::Completed);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
        );
        let wait = if tool == OrchTool::OpenPort { 70 } else { 50 };
        assert!(before.elapsed() >= std::time::Duration::from_secs(wait));
        let request = rig.last_request();
        let marker = if tool == OrchTool::OpenPort {
            "PORT-WAITING-AFTER-70"
        } else {
            "Orch did not answer in time. Nothing was changed. Try again."
        };
        assert_eq!(
            request.matches(marker).count(),
            1,
            "the connector outcome arrives before native 60/80-second timeout; it is not an inaccessible result"
        );
    }
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_timeout_claude_keeps_a_persons_native_decision_waiting_past_the_ordinary_tool_deadline()
 {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned image is required");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    let server = server(rig.model.url(), true).await;
    rig.start_beside(Loopback::new(server.port, Secret::new(common::TOKEN)))
        .await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.approval_mode = ApprovalMode::Ask;
    spec.settings.orch_tools = vec![OrchTool::ReadShared]; // ordinary server deadline is 60 seconds
    rig.open_with(spec).await;
    let marker = rig.work.path().join("permission-after-deadline.txt");
    let arguments = json!({"command":format!("printf permission-proof > '{}'", marker.display())});
    rig.adapter
        .start_turn(rig.turn(&format!(
            "#!mock\ntool Bash {arguments}\nthen\nsay complete\n"
        )))
        .await
        .unwrap();
    let events = rig
        .until(
            |event| matches!(event, SessionEvent::ApprovalRequested(_)),
            30,
        )
        .await;
    let request = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ApprovalRequested(request) => Some(request),
            _ => None,
        })
        .unwrap()
        .clone();
    // This duration is deliberate: it crosses the ordinary 60-second MCP deadline on the real pin.
    tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    assert!(
        !marker.exists(),
        "an unanswered card neither runs nor times out as an allowing answer"
    );
    let waiting = rig.drain(100).await;
    assert!(
        !waiting.iter().any(|event| matches!(
            event,
            SessionEvent::TurnCompleted(_) | SessionEvent::ApprovalClosed(_)
        )),
        "the person's request remains pending: {waiting:?}"
    );
    rig.adapter
        .resolve_approval(rig.thread, &request.request_ref, common::allow(&request))
        .await
        .unwrap();
    assert_eq!(
        common::outcome(&rig.until_the_turn_ends().await),
        TurnOutcome::Completed
    );
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "permission-proof");
    rig.adapter.stop().await.unwrap();
}

fn budget_text(arguments: &serde_json::Value) -> String {
    let bytes = orch_agent_contract::tool_answers::max_bytes(arguments);
    let prefix = "BUDGET-BEGIN-";
    let suffix = "-BUDGET-END";
    let (unit, width) = if arguments["script"] == "chinese" {
        ("界", 3)
    } else {
        ("x", 1)
    };
    let room = bytes - prefix.len() - suffix.len();
    format!(
        "{prefix}{}{suffix}",
        unit.repeat(room / width) + &"x".repeat(room % width)
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prompts_fork_claude_keeps_the_core_once_and_reads_a_handoff_only_with_the_first_real_input()
 {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned image is required");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    rig.start_quick().await;
    let parent_lane = rig.lane();
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.instructions = "FORK-CORE-PREAMBLE\nFORK-CORE-GUIDE\nFORK-CORE-STANDING".into();
    let opened = rig.open_with(spec.clone()).await;
    rig.run("#!mock\nsay parent-history-only\n").await;
    let file = rig.work.path().join("handoff.md");
    std::fs::write(&file, "# Selected turns\nHANDOFF-FILE-MATERIAL\n").unwrap();
    let requests_before = rig.model.requests().len();
    rig.new_lane();
    let lane = rig.spec(SessionMode::Fork, Some(&opened.native_session_ref));
    spec.settings.thread_id = lane.settings.thread_id;
    spec.settings.segment_id = lane.settings.segment_id;
    spec.session_key = lane.session_key;
    spec.settings.mode = SessionMode::Fork;
    spec.settings.native_session_ref = Some(opened.native_session_ref);
    rig.open_with(spec).await;
    assert_eq!(
        rig.model.requests().len(),
        requests_before,
        "opening a fork and placing its file must not start model work"
    );
    let notice = format!(
        "<orch>A handoff of 1 selected turn is in {}. Read it before acting.</orch>",
        file.display()
    );
    let arguments = json!({"file_path":file});
    let (_, events) = rig
        .run(&format!(
            "{notice}\n#!mock\ntool Read {arguments}\nthen\nsay fork complete\n"
        ))
        .await;
    assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    let request = rig.last_request();
    for part in [
        "FORK-CORE-PREAMBLE",
        "FORK-CORE-GUIDE",
        "FORK-CORE-STANDING",
        "A handoff of 1 selected turn",
    ] {
        assert_eq!(
            request.matches(part).count(),
            1,
            "a fork retains one opening and one first-input notice: {part}"
        );
    }
    assert!(
        request.contains("HANDOFF-FILE-MATERIAL"),
        "the harness must reach the actual workspace file"
    );
    rig.set_lane(&parent_lane);
    rig.run("#!mock\nsay parent continues\n").await;
    assert!(!rig.last_request().contains("HANDOFF-FILE-MATERIAL"));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_guide_claude_uses_one_fallback_and_reads_new_announcement_choices_on_resume() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned image is required");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    let server = server(rig.model.url(), true).await;
    rig.start_beside(Loopback::new(server.port, Secret::new(common::TOKEN)))
        .await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.orch_tools = vec![OrchTool::Announce];
    spec.settings.instructions = "SCHEMA-PREAMBLE\nSCHEMA-T2-FALLBACK".into();
    let initial = announcement_schema(false);
    server
        .schemas
        .lock()
        .unwrap()
        .insert("announce".into(), initial.clone());
    spec.settings
        .orch_tool_schemas
        .insert("announce".into(), initial);
    let opened = rig.open_with(spec.clone()).await;
    rig.run("#!mock\ntool mcp__orch__announce {\"title\":\"first\"}\nthen\nsay complete\n")
        .await;
    assert!(has_value(
        &rig.model.requests().last().unwrap().body,
        &json!(["followers", "topic"])
    ));
    let changed = announcement_schema(true);
    server
        .schemas
        .lock()
        .unwrap()
        .insert("announce".into(), changed.clone());
    spec.settings
        .orch_tool_schemas
        .insert("announce".into(), changed);
    spec.settings.mode = SessionMode::Resume;
    spec.settings.native_session_ref = Some(opened.native_session_ref);
    spec.session_key = new_id().simple().to_string();
    let before = rig.model.requests().len();
    rig.adapter.close_session(rig.thread).await.unwrap();
    rig.open_with(spec).await;
    assert_eq!(rig.model.requests().len(), before);
    let (_, events) = rig.run("#!mock\ntool mcp__orch__announce {\"title\":\"second\",\"audience\":\"project\",\"delivery\":\"steer\"}\nthen\nsay complete\n").await;
    assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    let request = rig.model.requests().last().unwrap().body.clone();
    assert!(has_value(
        &request,
        &json!(["followers", "topic", "project"])
    ));
    assert!(has_value(&request, &json!(["steer"])));
    assert_eq!(request.to_string().matches("SCHEMA-T2-FALLBACK").count(), 1);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_answer_budget_and_handoff_claude_reach_separated_tools_without_a_runtime_file_substitute()
 {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned image is required");
    // The runtime has its fixture home, but no workspace mount. The named executor read below is a
    // controlled MCP endpoint reading the actual fixture workspace, not Claude's local Read tool.
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    let server = server(rig.model.url(), true).await;
    *server.workspace.lock().unwrap() = Some(rig.work.path().to_path_buf());
    rig.start_beside(Loopback::new(server.port, Secret::new(common::TOKEN)))
        .await;
    for native in [false, true] {
        rig.new_lane();
        let mut spec = rig.spec(SessionMode::New, None);
        spec.exec.executor = Some("orch-exec".into());
        spec.exec.tool_set = Some("claude-code".into());
        spec.settings.orch_tools = vec![OrchTool::ReadShared];
        spec.settings.arrangement = if native {
            orch_agent_contract::descriptor::ArrangementType::Separated
        } else {
            orch_agent_contract::descriptor::ArrangementType::KeySeparated
        };
        if native {
            spec.provider = orch_agent_contract::session::ProviderAccess::Native {
                provider_id: new_id(),
            };
        }
        rig.open_with(spec.clone()).await;
        for script in ["latin", "chinese"] {
            let arguments = json!({"budget_probe":true,"script":script,"max_bytes":40000});
            let expected = budget_text(&arguments);
            let (_, events) = rig
                .run(&format!(
                    "#!mock\ntool mcp__orch__read_shared {arguments}\nthen\nsay complete\n"
                ))
                .await;
            assert_eq!(common::outcome(&events), TurnOutcome::Completed);
            assert!(
                has_text(&rig.model.requests().last().unwrap().body, &expected),
                "both transports deliver the whole 40k-byte result, not a file at the runtime"
            );
        }
        let file = rig.work.path().join("handoff.md");
        std::fs::write(&file, "HANDOFF-ONLY-AT-EXECUTOR\n").unwrap();
        rig.new_lane();
        let lane = rig.spec(SessionMode::New, None);
        spec.settings.thread_id = lane.settings.thread_id;
        spec.settings.segment_id = lane.settings.segment_id;
        spec.session_key = lane.session_key;
        let before = rig.model.requests().len();
        rig.open_with(spec).await;
        assert_eq!(
            rig.model.requests().len(),
            before,
            "placing a handoff and opening starts no hidden input"
        );
        let arguments = json!({"file_path":file});
        let (_, events) = rig.run_deciding(&format!("<orch>The handoff file is {}. Read it before acting.</orch>\n#!mock\ntool mcp__exec__Read {arguments}\nthen\nsay complete\n", file.display()), common::allow).await;
        assert_eq!(common::outcome(&events), TurnOutcome::Completed);
        assert!(has_text(
            &rig.model.requests().last().unwrap().body,
            "HANDOFF-ONLY-AT-EXECUTOR"
        ));
    }
    rig.adapter.stop().await.unwrap();
}

fn announcement_schema(project: bool) -> serde_json::Value {
    let mut schema = json!({"type":"object","additionalProperties":false,"required":["title"],"properties":{"title":{"type":"string"},"audience":{"type":"string","enum":["followers","topic"]}}});
    if project {
        schema["properties"]["audience"]["enum"] = json!(["followers", "topic", "project"]);
        schema["properties"]["delivery"] = json!({"type":"string","enum":["steer"]});
    }
    schema
}

fn has_value(value: &serde_json::Value, expected: &serde_json::Value) -> bool {
    value == expected
        || match value {
            serde_json::Value::String(value) => serde_json::from_str::<serde_json::Value>(value)
                .is_ok_and(|value| has_value(&value, expected)),
            serde_json::Value::Array(values) => {
                values.iter().any(|value| has_value(value, expected))
            }
            serde_json::Value::Object(values) => {
                values.values().any(|value| has_value(value, expected))
            }
            _ => false,
        }
}

fn tool_description(name: &str) -> String {
    let definitions: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../crates/runtime/connector/src/orch_tools.json"
    ))
    .unwrap();
    definitions
        .iter()
        .find(|definition| definition["name"] == name)
        .unwrap()["description"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn has_text(value: &serde_json::Value, text: &str) -> bool {
    match value {
        serde_json::Value::String(value) => {
            value.contains(text)
                || serde_json::from_str::<serde_json::Value>(value)
                    .is_ok_and(|value| has_text(&value, text))
        }
        serde_json::Value::Array(values) => values.iter().any(|value| has_text(value, text)),
        serde_json::Value::Object(values) => values.values().any(|value| has_text(value, text)),
        _ => false,
    }
}

/// The ordinary approval modes: in each, every Orch tool is called once without a second approval.
const ORDINARY_MODES: [ApprovalMode; 3] = [
    ApprovalMode::Ask,
    ApprovalMode::OnRequest,
    ApprovalMode::Bypass,
];

fn result_tools() -> [(OrchTool, &'static str); 16] {
    [
        (OrchTool::ListShared, "list_shared"),
        (OrchTool::ReadShared, "read_shared"),
        (OrchTool::SearchShared, "search_shared"),
        (OrchTool::DownloadShared, "download_shared"),
        (OrchTool::EditDocument, "edit_document"),
        (OrchTool::ManageShared, "manage_shared"),
        (OrchTool::SendMessage, "send_message"),
        (OrchTool::Announce, "announce"),
        (OrchTool::RequestCapability, "request_capability"),
        (OrchTool::UseBrowser, "use_browser"),
        (OrchTool::UseDisplay, "use_display"),
        (OrchTool::ListPorts, "list_ports"),
        (OrchTool::OpenPort, "open_port"),
        (OrchTool::ClosePort, "close_port"),
        (OrchTool::ListPortConnections, "list_port_connections"),
        (OrchTool::Goal, "goal"),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instructions_file_chain_claude_and_file_boundary_keep_both_forms_and_changed_root_rules() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned image is required");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    let outer = rig.work.path().to_owned();
    let root = outer.join("workspace");
    let api = root.join("web/services/api");
    std::fs::create_dir_all(api.join("deep/leaf")).unwrap();
    std::fs::create_dir_all(outer.join(".claude/rules")).unwrap();
    for (path, text) in [
        ("AGENTS.md", "OUTSIDE-AGENTS"),
        ("CLAUDE.md", "OUTSIDE-CLAUDE"),
        ("CLAUDE.local.md", "OUTSIDE-LOCAL"),
        (".claude/rules/ancestor.md", "OUTSIDE-RULE"),
    ] {
        std::fs::write(outer.join(path), text).unwrap();
    }
    for (path, text) in [
        ("AGENTS.md", "ROOT-AGENTS-INITIAL\n"),
        ("CLAUDE.md", "ROOT-CLAUDE\n"),
        ("web/AGENTS.md", "WEB-AGENTS\n"),
        ("web/CLAUDE.md", "WEB-CLAUDE\n"),
        ("web/services/AGENTS.md", "SERVICES-AGENTS\n"),
        ("web/services/CLAUDE.md", "SERVICES-CLAUDE\n"),
        ("web/services/api/AGENTS.md", ""),
    ] {
        std::fs::write(root.join(path), text).unwrap();
    }
    let started = rig
        .start(
            json!({"setting_sources":"project,local"}),
            Default::default(),
        )
        .await;
    assert_eq!(started.harness_version, "2.1.296");
    for cwd in [api.clone(), api.join("deep/leaf")] {
        rig.new_lane();
        let mut spec = rig.spec(SessionMode::New, None);
        spec.settings.workspace_root = root.display().to_string();
        spec.settings.directory = api.display().to_string();
        spec.settings.cwd = cwd.display().to_string();
        let opened = rig.open_with(spec.clone()).await;
        rig.adapter.start_turn(rig.turn("#!mock\ntool Bash {\"command\":\"pwd\",\"description\":\"Show the working directory\"}\nthen\nsay Instruction sources: CLAUDE.md and AGENTS.md at workspace, web and web/services. Command said: {{result}}\n")).await.unwrap();
        let events = rig.until_the_turn_ends().await;
        assert_eq!(
            common::outcome(&events),
            TurnOutcome::Completed,
            "{events:?}"
        );
        let body = rig.model.requests().last().unwrap().body.to_string();
        assert!(events.iter().any(|event| matches!(event, SessionEvent::ItemCompleted(item)
            if item.payload["tool"]["kind"] == "command"
            && item.payload["tool"]["cwd"] == cwd.display().to_string()
            && item.payload["tool"]["output"]["inline"].as_str().is_some_and(|text| text.contains(&cwd.display().to_string())))), "pwd actually executes in the thread directory: {events:?}");
        let root_rule = if cwd == api {
            "ROOT-AGENTS-INITIAL"
        } else {
            "ROOT-AGENTS-CURRENT"
        };
        let positions = [
            "ROOT-CLAUDE",
            root_rule,
            "WEB-CLAUDE",
            "WEB-AGENTS",
            "SERVICES-CLAUDE",
            "SERVICES-AGENTS",
        ]
        .map(|marker| {
            body.find(marker)
                .expect("all applicable native forms reach the model")
        });
        assert!(positions.windows(2).all(|p| p[0] < p[1]));
        for excluded in [
            "OUTSIDE-AGENTS",
            "OUTSIDE-CLAUDE",
            "OUTSIDE-LOCAL",
            "OUTSIDE-RULE",
        ] {
            assert!(!body.contains(excluded), "{excluded}");
        }
        assert!(body.contains(&cwd.display().to_string()));
        if cwd == api {
            std::fs::write(root.join("AGENTS.md"), "ROOT-AGENTS-CURRENT\n").unwrap();
            for resumed in [false, true] {
                if resumed {
                    rig.adapter.close_session(rig.thread).await.unwrap();
                    spec.settings.mode = SessionMode::Resume;
                    spec.settings.native_session_ref = Some(opened.native_session_ref.clone());
                    rig.open_with(spec.clone()).await;
                }
                rig.adapter
                    .start_turn(rig.turn("#!mock\nsay changed root file on the next turn\n"))
                    .await
                    .unwrap();
                assert_eq!(
                    common::outcome(&rig.until_the_turn_ends().await),
                    TurnOutcome::Completed
                );
                let request = rig.model.requests().last().unwrap().body.clone();
                let reminder = request["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|m| m["content"].as_array().into_iter().flatten())
                    .filter_map(|c| c["text"].as_str())
                    .filter(|t| t.contains("ROOT-AGENTS-CURRENT"))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    reminder.contains("each replaces its earlier copy"),
                    "{reminder}"
                );
                assert!(!reminder.contains("ROOT-AGENTS-INITIAL"));
            }
        }
    }
    assert_eq!(std::fs::read_to_string(api.join("AGENTS.md")).unwrap(), "");
    assert!(!api.join("deep/AGENTS.md").exists());
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instructions_no_workspace_config_claude_refuses_project_settings_before_launch() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned image is required");
    rig.adapter = ClaudeCodeAdapter::with_launcher(
        Arc::new(orch_types::SystemClock),
        Arc::new(Docker {
            mounts: vec![rig.home.path().into(), rig.work.path().into()],
            names: Mutex::new(Vec::new()),
        }),
    );
    rig.start(json!({}), Default::default()).await;
    let config = rig.work.path().join(".claude");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(config.join("settings.json"), "malformed project settings").unwrap();
    let before = rig.model.requests().len();
    let error = rig
        .adapter
        .open_session(rig.spec(SessionMode::New, None))
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        orch_agent_contract::session::WORKSPACE_CONFIGURATION
    );
    assert!(error.to_string().contains(".claude/settings.json"));
    assert_eq!(rig.model.requests().len(), before);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instructions_next_turn_claude_uses_the_changed_file_after_a_real_resume() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned harness image is required for this proof");
    let launcher = Arc::new(Docker {
        mounts: vec![rig.home.path().into(), rig.work.path().into()],
        names: Mutex::new(Vec::new()),
    });
    rig.adapter = ClaudeCodeAdapter::with_launcher(Arc::new(orch_types::SystemClock), launcher);
    let started = rig.start_quick().await;
    eprintln!(
        "instructions.next_turn.claude pinned {}",
        started.harness_version
    );
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.instructions = "INITIAL-STANDING-RULE".into();
    let opened = rig.open_with(spec).await;
    rig.adapter
        .start_turn(rig.turn("#!mock\nsay first\n"))
        .await
        .unwrap();
    assert_eq!(
        common::outcome(&rig.until_the_turn_ends().await),
        TurnOutcome::Completed
    );
    let first = rig.model.requests().last().unwrap().body["system"].to_string();
    assert!(first.contains("INITIAL-STANDING-RULE"));
    rig.adapter.close_session(rig.thread).await.unwrap();
    let mut resumed = rig.spec(SessionMode::Resume, Some(&opened.native_session_ref));
    resumed.settings.instructions = format!("CURRENT-STANDING-RULE\n{}", "中文".repeat(20_000));
    rig.open_with(resumed).await;
    rig.adapter
        .start_turn(rig.turn("#!mock\nsay second\n"))
        .await
        .unwrap();
    assert_eq!(
        common::outcome(&rig.until_the_turn_ends().await),
        TurnOutcome::Completed
    );
    let second = rig.model.requests().last().unwrap().body["system"].to_string();
    assert!(second.contains("CURRENT-STANDING-RULE"));
    assert!(
        !second.contains("INITIAL-STANDING-RULE"),
        "a resumed snapshot must not retain the old rule"
    );
    assert!(
        second.contains(&"中文".repeat(20_000)),
        "the large UTF-8 instruction file reaches the model whole"
    );
    rig.adapter.stop().await.unwrap();
}

struct Server {
    port: u16,
    calls: Arc<Mutex<Vec<String>>>,
    schemas: SchemaMap,
    workspace: Workspace,
    task: tokio::task::JoinHandle<()>,
}
type SchemaMap = Arc<Mutex<std::collections::BTreeMap<String, serde_json::Value>>>;
type Workspace = Arc<Mutex<Option<PathBuf>>>;
type EndpointState = (String, Arc<Mutex<Vec<String>>>, bool, SchemaMap, Workspace);
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(model_url: String, result_form: bool) -> Server {
    use axum::{
        Router,
        body::Bytes,
        extract::{OriginalUri, State},
        response::IntoResponse,
        routing::any,
    };
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let schemas = SchemaMap::default();
    let workspace = Workspace::default();
    async fn answer(
        State((url, calls, result_form, schemas, workspace)): State<EndpointState>,
        OriginalUri(uri): OriginalUri,
        body: Bytes,
    ) -> axum::response::Response {
        if uri.path().starts_with("/llm/") {
            let response = reqwest::Client::new()
                .post(format!("{url}/v1/responses"))
                .header("authorization", "Bearer prompt-test-token")
                .header("content-type", "application/json")
                .body(body.to_vec())
                .send()
                .await
                .unwrap();
            let status = response.status();
            let content_type = response.headers().get("content-type").cloned();
            let bytes = response.bytes().await.unwrap();
            let mut response = (status, bytes).into_response();
            if let Some(kind) = content_type {
                response.headers_mut().insert("content-type", kind);
            }
            return response;
        }
        let Ok(request) = serde_json::from_slice::<serde_json::Value>(&body) else {
            return axum::http::StatusCode::METHOD_NOT_ALLOWED.into_response();
        };
        let Some(id) = request.get("id") else {
            return axum::http::StatusCode::ACCEPTED.into_response();
        };
        let result = match request["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion":request["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"orch","version":"1"}, "instructions":"PROMPT-GUIDE-MARKER"})
            }
            "tools/list" => {
                let names = if uri.path().ends_with("/exec") {
                    vec!["Read"]
                } else if result_form {
                    result_tools()
                        .into_iter()
                        .map(|(_, name)| name)
                        .collect::<Vec<_>>()
                } else {
                    vec![
                        "list_shared",
                        "edit_document",
                        "manage_shared",
                        "request_capability",
                    ]
                };
                let tools = names.iter().map(|name| {
                    let reading = ["Read", "list_shared", "read_shared", "search_shared", "list_ports", "list_port_connections"].contains(name);
                    let mut definition = json!({
                        "name":name,"description":if *name == "Read" { "Read the named file in the fixture executor's workspace.".to_owned() } else { tool_description(name) },
                        "inputSchema":{"type":"object","additionalProperties":false,"properties":{"refuse":{"type":"boolean"},"budget_probe":{"type":"boolean"},"timeout_probe":{"type":"boolean"},"script":{"type":"string"},"max_bytes":{"type":"integer","minimum":1000,"maximum":40000}}},
                        "annotations":{"readOnlyHint":reading,"destructiveHint":!reading,"openWorldHint":false}
                    });
                    if *name == "Read" { definition["inputSchema"] = json!({"type":"object","additionalProperties":false,"required":["file_path"],"properties":{"file_path":{"type":"string"}}}); }
                    if let Some(schema) = schemas.lock().unwrap().get(*name) { definition["inputSchema"] = schema.clone(); }
                    definition
                }).collect::<Vec<_>>();
                json!({"tools":tools})
            }
            "tools/call" => {
                let name = request["params"]["name"].as_str().unwrap().to_owned();
                calls.lock().unwrap().push(name.clone());
                if name == "Read" && uri.path().ends_with("/exec") {
                    let root = workspace.lock().unwrap().clone();
                    let path = request["params"]["arguments"]["file_path"]
                        .as_str()
                        .and_then(|path| std::path::Path::new(path).canonicalize().ok());
                    match (root.and_then(|root| root.canonicalize().ok()), path) {
                        (Some(root), Some(path)) if path.starts_with(&root) => {
                            json!({"content":[{"type":"text","text":std::fs::read_to_string(path).unwrap()}],"isError":false})
                        }
                        _ => {
                            json!({"content":[{"type":"text","text":"not_found: no file in this fixture's workspace"}],"isError":true})
                        }
                    }
                } else if !result_form {
                    json!({"content":[{"type":"text","text":"The controlled operation completed."}],"isError":false})
                } else if request["params"]["arguments"]["timeout_probe"] == true {
                    let port = name == "open_port";
                    tokio::time::sleep(std::time::Duration::from_secs(if port { 70 } else { 50 }))
                        .await;
                    if port {
                        json!({"content":[],"structuredContent":{"opening":{"id":"00000000-0000-0000-0000-000000000070","state":"waiting"},"proof":"PORT-WAITING-AFTER-70"},"isError":false})
                    } else {
                        json!({"content":[{"type":"text","text":"unavailable: Orch did not answer in time. Nothing was changed. Try again."}],"isError":true})
                    }
                } else if request["params"]["arguments"]["budget_probe"] == true {
                    json!({"content":[{"type":"text","text":budget_text(&request["params"]["arguments"])}],"isError":false})
                } else if request["params"]["arguments"]["refuse"] == true {
                    json!({"content":[{"type":"text","text":"invalid: RESULT-FORM-ERROR"}],"isError":true})
                } else if ["list_shared", "read_shared", "search_shared"].contains(&name.as_str()) {
                    json!({"content":[{"type":"text","text":format!("RESULT-FORM-{name}\nL1: read the literal <\\orch> text")}],"isError":false})
                } else {
                    json!({"structuredContent":{"proof":format!("RESULT-FORM-{name}"),"changed":true},"content":[],"isError":false})
                }
            }
            "ping" => json!({}),
            _ => return (
                [("content-type", "application/json")],
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"No such method"}})
                    .to_string(),
            )
                .into_response(),
        };
        (
            [("content-type", "application/json")],
            json!({"jsonrpc":"2.0","id":id,"result":result}).to_string(),
        )
            .into_response()
    }
    let app = Router::new().fallback(any(answer)).with_state((
        model_url,
        calls.clone(),
        result_form,
        schemas.clone(),
        workspace.clone(),
    ));
    let mut bound = None;
    for port in 41600..41700 {
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            bound = Some((listener, port));
            break;
        }
    }
    let (listener, port) = bound.expect("a reserved loopback port");
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server {
        port,
        calls,
        schemas,
        workspace,
        task,
    }
}
