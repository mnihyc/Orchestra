//! Native Claude Code keeps its sandbox, parallel call identities and image results on the pinned binary.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
#[path = "../../../../scripts/harness-canary/model_only.rs"]
mod model_only;
#[path = "common/native_recording.rs"]
mod native_recording;

use orch_adapter_claude_code::{
    ClaudeCodeAdapter,
    process::{LaunchSpec, Launcher},
};
use orch_agent_contract::adapter::{AgentAdapter, AgentStart};
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::facts::AgentProfile;
use orch_conformance::native::{self, Fixture, ModelControl, ModelRequest};
use orch_testkit::doubles::model::QueueFilter;
use orch_types::{SystemClock, new_id};
use serde_json::json;
use std::sync::{Arc, Mutex};
/// The original native rig's complete-turn deadline, retained when its cases move to the shared runner.
const NATIVE_CASE_WITHIN: std::time::Duration = std::time::Duration::from_secs(90);

struct Isolated {
    network: Arc<model_only::ModelOnly>,
    mounts: Vec<std::path::PathBuf>,
    launches: Mutex<Vec<LaunchSpec>>,
}
impl std::fmt::Debug for Isolated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a networkless native harness launcher")
    }
}
impl Launcher for Isolated {
    fn command(&self, spec: &LaunchSpec) -> tokio::process::Command {
        self.launches.lock().unwrap().push(spec.clone());
        self.network
            .command(
                &common::image(),
                &spec.program,
                &spec.args,
                &spec.env,
                &self.mounts,
                Some(&spec.cwd),
            )
            .0
    }
}
async fn shared(case: &str) {
    let mut rig = common::Rig::new()
        .await
        .expect("native conformance requires its pinned image");
    let server = if case == "tools.result_form" {
        Some(
            native::elicitation::Server::start(rig.model.addr(), NATIVE_CASE_WITHIN)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let address = server
        .as_ref()
        .map_or(rig.model.addr(), |server| server.address);
    let network = model_only::ModelOnly::new(address).await;
    let binary = std::env::var("ORCH_CONFORMANCE_CLAUDE_BINARY").ok();
    let mut mounts: Vec<std::path::PathBuf> = vec![rig.home.path().into(), rig.work.path().into()];
    if let Some(path) = &binary {
        mounts.push(std::path::Path::new(path).parent().unwrap().into());
    }
    let launcher = Arc::new(Isolated {
        network,
        mounts,
        launches: Mutex::new(Vec::new()),
    });
    rig.adapter = ClaudeCodeAdapter::with_launcher(Arc::new(SystemClock), launcher.clone());
    let requests = || {
        rig.model
            .requests()
            .into_iter()
            .map(|request| ModelRequest {
                path: request.path,
                body: request.body,
                status: request.status,
                queued: request.script == Some(orch_testkit::doubles::model::ScriptSource::Queue),
            })
            .collect()
    };
    let launches = || {
        launcher
            .launches
            .lock()
            .unwrap()
            .iter()
            .map(|launch| launch.env.clone())
            .collect()
    };
    let elicitation = || {
        server
            .as_ref()
            .map_or_else(Vec::new, |server| server.seen())
    };
    let loopback = orch_agent_contract::loopback::Loopback::new(
        address.port(),
        orch_plugin_contract::Secret::new(common::TOKEN),
    );
    let control_model = |control: ModelControl| -> Result<(), String> {
        match control {
            ModelControl::Reset => {
                rig.model.reset();
                Ok(())
            }
            ModelControl::Queue {
                model: model_name,
                script,
            } => rig
                .model
                .queue_for(
                    &script,
                    &QueueFilter {
                        model: Some(model_name),
                        format: None,
                    },
                    1,
                )
                .map_err(|error| error.to_string()),
            ModelControl::QueueMatching {
                model: selected_model,
                script,
                pattern,
                times,
            } => rig
                .model
                .queue_matching(
                    &script,
                    &QueueFilter {
                        model: selected_model,
                        format: None,
                    },
                    times,
                    move |body| pattern.matches(body),
                )
                .map_err(|error| error.to_string()),
            ModelControl::Default { script } => rig
                .model
                .set_default(&script)
                .map_err(|error| error.to_string()),
        }
    };
    // Until the real subscription trial passes, only this ignored test sees the local probe.
    let local_checks = (case == "auto_review").then(|| {
        let mut checks = orch_adapter_claude_code::descriptor::manifest()
            .checks
            .clone();
        checks.probes.push(case.to_owned());
        checks
    });
    let version =
        std::env::var("ORCH_EXPECTED_CLAUDE_VERSION").unwrap_or_else(|_| "2.1.322".into());
    let recording = native_recording::NativeRecording::from_env(case, &version, &common::image());
    let observe_native = |event: &SessionEvent| {
        if let Some(recording) = &recording {
            recording.observe(event);
        }
    };
    let fixture = Fixture {
        start: AgentStart {
            provider_id: new_id(),
            provider_kind: None,
            agent_id: new_id(),
            harness_kind: "claude-code".into(),
            profile: AgentProfile {
                config: json!({}),
                env: {
                    let mut env = std::collections::BTreeMap::from([(
                        "CLAUDE_CODE_MAX_RETRIES".to_owned(),
                        "1".to_owned(),
                    )]);
                    if std::env::var("ORCH_CLAUDE_FEATURE_FLAGS_OFF").as_deref() == Ok("1") {
                        env.insert("CLAUDE_CODE_DISABLE_FEATURE_FLAGS".into(), "1".into());
                    }
                    env
                },
                ..AgentProfile::default()
            },
            home: Some(rig.home.path().display().to_string()),
            provider_config: json!({}),
            loopback,
            binary,
        },
        version,
        checks: local_checks
            .as_ref()
            .unwrap_or(&orch_adapter_claude_code::descriptor::manifest().checks),
        workspace: rig.work.path().into(),
        within: NATIVE_CASE_WITHIN,
        requests: &requests,
        control_model: Some(&control_model),
        launches: &launches,
        elicitation: &elicitation,
        observe: if recording.is_some() {
            Some(&observe_native)
        } else {
            None
        },
    };
    let informational = local_checks.is_some();
    let result = if informational {
        native::run_probe(&mut rig.adapter, &fixture, case).await
    } else {
        native::run_case(&mut rig.adapter, &fixture, case).await
    };
    model_only::record(
        "claude-code",
        case,
        if informational {
            "informational"
        } else if result.passed {
            "passed"
        } else {
            "failed"
        },
        &fixture.version,
        json!(result),
    );
    if let Some(recording) = &recording {
        recording.finish(result.passed, informational);
    }
    assert!(result.passed, "{result:?}");
}

#[tokio::test]
async fn native_startup_reaches_the_controlled_model_with_nonessential_traffic_disabled() {
    shared("startup").await;
}

#[tokio::test]
async fn native_parallel_calls_keep_separate_inputs_results_and_timeline_ids() {
    shared("parallel_calls").await;
}

#[tokio::test]
async fn native_image_read_returns_a_usable_image_block_to_the_model() {
    shared("image_result").await;
}

#[tokio::test]
async fn an_unproved_readonly_boundary_is_not_offered_and_a_session_requiring_it_is_refused() {
    shared("sandbox").await;
}

#[tokio::test]
async fn native_tool_answers_have_only_one_model_facing_form() {
    shared("tools.result_form").await;
}

#[tokio::test]
async fn a_managed_pinned_build_refuses_updates_before_any_download_can_run() {
    let mut rig = common::Rig::new()
        .await
        .expect("native conformance requires its pinned image");
    let binary = std::env::var("ORCH_CONFORMANCE_CLAUDE_BINARY").ok();
    let mut mounts: Vec<std::path::PathBuf> = vec![rig.home.path().into(), rig.work.path().into()];
    if let Some(path) = &binary {
        mounts.push(std::path::Path::new(path).parent().unwrap().into());
    }
    let launcher = Arc::new(Isolated {
        network: model_only::ModelOnly::new(rig.model.addr()).await,
        mounts,
        launches: Mutex::new(Vec::new()),
    });
    rig.adapter = ClaudeCodeAdapter::with_launcher(Arc::new(SystemClock), launcher.clone());
    rig.program = binary;
    let started = rig.start_quick().await;
    let mut request = launcher.launches.lock().unwrap()[0].clone();
    assert_eq!(
        request.env.get("DISABLE_UPDATES").map(String::as_str),
        Some("1")
    );
    request.args = vec!["update".to_owned()];
    request.label = "disabled-update-conformance".to_owned();
    request.cwd = rig.home.path().to_owned();
    // This disposable networkless probe can neither download nor change the pinned host installation.
    let result = orch_adapter_claude_code::process::run_to_end(
        launcher.as_ref(),
        &request,
        NATIVE_CASE_WITHIN,
    )
    .await
    .unwrap();
    let diagnostic =
        format!("{}\n{}", result.lines.join("\n"), result.exit.describe()).to_lowercase();
    assert!(
        diagnostic
            .lines()
            .any(|line| line.starts_with("updates are disabled by your administrator.")),
        "the pinned program must explicitly refuse the updater: {diagnostic}"
    );
    assert!(!rig.home.path().join("packages").exists());
    assert!(
        rig.model.requests().is_empty(),
        "no model call pays for this probe"
    );
    model_only::record(
        "claude-code",
        "managed-updates",
        "passed",
        &started.harness_version,
        json!({"update_refused":true,"downloaded_packages":false,"exit_code":result.exit.code}),
    );
}

#[tokio::test]
async fn a_candidate_builds_native_history_can_be_imported_into_a_fresh_program_home() {
    if std::env::var_os("ORCH_CONFORMANCE_CLAUDE_BINARY").is_some() {
        shared("native_import").await;
    }
}

#[tokio::test]
async fn changing_bypass_keeps_ask_mode_and_uses_the_matching_launch_environment() {
    let mut rig = common::Rig::new()
        .await
        .expect("native conformance requires its pinned image");
    let binary = std::env::var("ORCH_CONFORMANCE_CLAUDE_BINARY").ok();
    let mut mounts: Vec<std::path::PathBuf> = vec![rig.home.path().into(), rig.work.path().into()];
    if let Some(path) = &binary {
        mounts.push(std::path::Path::new(path).parent().unwrap().into());
    }
    let launcher = Arc::new(Isolated {
        network: model_only::ModelOnly::new(rig.model.addr()).await,
        mounts,
        launches: Mutex::new(Vec::new()),
    });
    rig.adapter = ClaudeCodeAdapter::with_launcher(Arc::new(SystemClock), launcher.clone());
    rig.program = binary;
    rig.start_quick().await;
    rig.open().await;
    for (index, mode) in [ApprovalMode::Ask, ApprovalMode::Bypass, ApprovalMode::Ask]
        .into_iter()
        .enumerate()
    {
        let marker = rig.work.path().join(format!("permission-marker-{index}"));
        let script = format!(
            "#!mock\ntool Bash {}\nthen\nsay done\n",
            json!({"command": format!("touch {}", marker.display()), "description": "Make a marker"})
        );
        let mut turn = rig.turn(&script);
        turn.approval_mode = mode;
        rig.adapter.start_turn(turn).await.unwrap();
        if mode == ApprovalMode::Bypass {
            let events = rig.until_the_turn_ends().await;
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
            );
            assert_eq!(common::outcome(&events), TurnOutcome::Completed);
            assert!(marker.exists());
        } else {
            let events = rig
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
            let Some(SessionEvent::ApprovalRequested(request)) = events.last() else {
                panic!("Ask must request approval")
            };
            assert!(!marker.exists());
            rig.adapter
                .resolve_approval(rig.thread, &request.request_ref, common::deny(request))
                .await
                .unwrap();
            let ended = rig.until_the_turn_ends().await;
            assert_eq!(common::outcome(&ended), TurnOutcome::Completed);
            assert!(!marker.exists());
        }
    }
    let launch_modes: Vec<_> = launcher
        .launches
        .lock()
        .unwrap()
        .iter()
        .filter(|launch| launch.label.starts_with("session-"))
        .map(|launch| {
            launch
                .env
                .get("CLAUDE_CODE_EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS")
                .cloned()
        })
        .collect();
    assert_eq!(launch_modes, [None, Some("1".to_owned()), None]);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
#[ignore = "explicit candidate-only staged classifier proof; real subscription and manifest activation remain pending"]
async fn native_auto_classifier_stages_hooks_and_model_policy_are_observed() {
    assert!(!orch_adapter_claude_code::descriptor::auto_proven());
    shared("auto_review").await;
}

/// A real candidate, a disposable home and the actual read-only control reply; no model credential or turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_pins_are_read_back_from_the_actual_candidate() {
    let mut rig = common::Rig::new()
        .await
        .expect("native pin proof requires the candidate image");
    let network = model_only::ModelOnly::new(rig.model.addr()).await;
    let binary = std::env::var("ORCH_CONFORMANCE_CLAUDE_BINARY").ok();
    let mut mounts: Vec<std::path::PathBuf> = vec![rig.home.path().into(), rig.work.path().into()];
    if let Some(path) = &binary {
        mounts.push(std::path::Path::new(path).parent().unwrap().into());
    }
    let launcher = Arc::new(Isolated {
        network,
        mounts,
        launches: Mutex::new(Vec::new()),
    });
    rig.adapter = ClaudeCodeAdapter::with_launcher(Arc::new(SystemClock), launcher.clone());
    rig.program = binary.clone();
    let started = rig.start_quick().await;
    let expected =
        std::env::var("ORCH_EXPECTED_CLAUDE_VERSION").unwrap_or_else(|_| "2.1.322".into());
    assert_eq!(started.harness_version, expected);
    let mut spec = rig.spec(orch_agent_contract::session::SessionMode::New, None);
    spec.settings.raw_events = true;
    spec.settings.background_wake = false;
    rig.adapter.open_session(spec).await.unwrap();
    // Opening a new logical session starts no native process. Inspect the actual quiet control launch.
    rig.adapter.warm_session(rig.thread).await.unwrap();
    let events=rig.until(|event|matches!(event,SessionEvent::ItemCompleted(item)
        if item.payload["marker"]=="harness_changed" && item.payload["data"]["change"]=="managed_settings_readback"),90).await;
    let actual = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::RawFrame(frame) if frame.direction == "from_harness" => {
                frame.payload.pointer("/response/response/effective")
            }
            _ => None,
        })
        .expect(
            "the actual native get_settings reply, never launch settings echoed as observation",
        );
    let mut pins = serde_json::Map::new();
    for (name, value) in [
        ("autoContinueAtUsageLimit", false),
        ("idleCompaction", false),
        ("precomputeCompactionEnabled", false),
        ("switchModelsOnFlag", false),
        ("disableAllHooks", true),
        ("disableWorkflows", true),
        ("enableWorkflows", false),
        ("workflowKeywordTriggerEnabled", false),
    ] {
        assert_eq!(
            actual[name],
            json!(value),
            "the native candidate must actually install {name}"
        );
        pins.insert(name.into(), actual[name].clone());
    }
    assert_eq!(actual["permissions"]["denyInsteadOfPrompting"], false);
    pins.insert(
        "permissions".into(),
        json!({"denyInsteadOfPrompting":actual["permissions"]["denyInsteadOfPrompting"]}),
    );
    let mut env = serde_json::Map::new();
    for (name, value) in [
        ("CLAUDE_CODE_DISABLE_MCP_TASK_BACKGROUND", "1"),
        ("CLAUDE_AUTO_BACKGROUND_TASKS", "0"),
        ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1"),
        ("CLAUDE_CODE_DISABLE_CRON", "1"),
        ("CLAUDE_CODE_DISABLE_WORKFLOWS", "1"),
        ("CLAUDE_CODE_AUTO_MODE_SERVER", "0"),
        ("CLAUDE_CODE_DISABLE_FEATURE_FLAGS", "1"),
    ] {
        assert_eq!(
            actual["env"][name], value,
            "the native candidate must actually install {name}"
        );
        env.insert(name.into(), actual["env"][name].clone());
    }
    pins.insert("env".into(), env.into());
    let summary = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item)
                if item.payload["data"]["change"] == "managed_settings_readback" =>
            {
                Some(&item.payload["data"]["value"])
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        summary["status"], "reported",
        "an absent/ignored pin is not passing native evidence: {summary}"
    );
    assert_eq!(summary["missing"], json!([]));
    assert!(
        rig.model.requests().is_empty(),
        "settings inspection must issue no model work"
    );
    let feature_flags_off = {
        let launches = launcher.launches.lock().unwrap();
        let launch = launches.last().unwrap();
        assert!(
            launch.label.starts_with("session-"),
            "native pin evidence needs a real managed session launch"
        );
        if let Some(binary) = &binary {
            assert_eq!(
                &launch.program, binary,
                "the readback case must use the selected candidate binary"
            );
        }
        assert!(!launch.env.contains_key("CLAUDE_CODE_DISABLE_MCP_TASKS"));
        assert_eq!(launch.env["CLAUDE_CODE_DISABLE_FEATURE_FLAGS"], "1");
        launch
            .env
            .get("CLAUDE_CODE_DISABLE_FEATURE_FLAGS")
            .map(String::as_str)
            == Some("1")
    };
    if let Ok(record) = std::env::var("ORCH_CONFORMANCE_RECORD") {
        let directory = std::path::Path::new(&record).join("claude-code");
        std::fs::create_dir_all(&directory).unwrap();
        // This is a redacted native control reply, not a manufactured fixture or a full settings dump.
        let evidence = json!({"version":started.harness_version,"source":"native get_settings reply; only named pins retained",
            "feature_flags_off":feature_flags_off,
            "frames":[{"type":"control_response","response":{"subtype":"success","response":{"effective":pins}}}]});
        std::fs::write(
            directory.join("managed-pins.json"),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
    rig.adapter.stop().await.unwrap();
    println!("CONFORMANCE passed claude-code managed_pins {expected}");
}
