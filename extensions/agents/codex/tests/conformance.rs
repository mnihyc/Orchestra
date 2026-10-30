//! Native Codex behavior is proved on the pinned binary, with every network cut except a controlled model.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
#[path = "../../../../scripts/harness-canary/model_only.rs"]
mod model_only;

use std::process::Stdio;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use orch_adapter_codex::{
    CodexAdapter,
    process::{HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl, ProcessExit},
};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_conformance::native::{self, Fixture, ModelRequest};
use serde_json::json;

struct Isolated {
    network: Arc<model_only::ModelOnly>,
    mounts: Vec<std::path::PathBuf>,
    launches: Mutex<Vec<LaunchRequest>>,
}
struct Control {
    child: tokio::process::Child,
    network: Arc<model_only::ModelOnly>,
    name: String,
}

#[async_trait]
impl ProcessControl for Control {
    async fn wait(&mut self) -> ProcessExit {
        match self.child.wait().await {
            Ok(status) => ProcessExit {
                code: status.code(),
                detail: String::new(),
            },
            Err(error) => ProcessExit {
                code: None,
                detail: error.to_string(),
            },
        }
    }
    fn kill(&mut self) {
        self.network.kill(&self.name);
        let _ = self.child.start_kill();
    }
}
#[async_trait]
impl Launcher for Isolated {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        self.launches.lock().unwrap().push(request.clone());
        let mut arguments = request.args.clone();
        if arguments.first().map(String::as_str) == Some("app-server") {
            arguments.insert(1, "--strict-config".into());
        }
        let (mut command, name) = self.network.command(
            &common::image(),
            &request.program,
            &arguments,
            &request.env,
            &self.mounts,
            None,
        );
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| LaunchError::Spawn {
                program: "docker".into(),
                source,
            })?;
        Ok(HarnessProcess {
            stdin: Box::new(child.stdin.take().unwrap()),
            stdout: Box::new(child.stdout.take().unwrap()),
            stderr: Some(Box::new(child.stderr.take().unwrap())),
            control: Box::new(Control {
                child,
                network: self.network.clone(),
                name,
            }),
        })
    }
}

async fn observed(case: &str) -> orch_api_contract::admin::HarnessConformanceCase {
    let mut rig = common::Rig::start(case)
        .await
        .expect("the native conformance image is required");
    let server = if matches!(case, "elicitation" | "tools.result_form") {
        Some(
            native::elicitation::Server::start(rig.mock.addr(), common::WAIT)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    rig.loopback_port = server.as_ref().map(|server| server.address.port());
    let network = model_only::ModelOnly::new(
        server
            .as_ref()
            .map_or(rig.mock.addr(), |server| server.address),
    )
    .await;
    let binary = std::env::var("ORCH_CONFORMANCE_CODEX_BINARY").ok();
    let mut mounts: Vec<std::path::PathBuf> =
        vec![rig.home_path().into(), rig.workspace_path().into()];
    if let Some(path) = &binary {
        mounts.push(
            std::path::Path::new(path)
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .into(),
        );
    }
    let launcher = Arc::new(Isolated {
        network,
        mounts,
        launches: Mutex::new(Vec::new()),
    });
    rig.adapter = CodexAdapter::with_launcher(launcher.clone());
    let mut start = rig.agent_start();
    start.binary = binary;
    let requests = || {
        rig.mock
            .requests()
            .into_iter()
            .map(|request| ModelRequest {
                path: request.path,
                body: request.body,
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
    let observe = |event: &orch_agent_contract::events::SessionEvent| {
        eprintln!("{}", json!({"case":case,"event":event}));
    };
    let fixture = Fixture {
        start,
        version: std::env::var("ORCH_EXPECTED_CODEX_VERSION").unwrap_or_else(|_| "0.162.1".into()),
        checks: &orch_adapter_codex::descriptor::manifest().checks,
        workspace: rig.workspace.path().into(),
        within: common::WAIT,
        requests: &requests,
        launches: &launches,
        elicitation: &elicitation,
        observe: if std::env::var_os("ORCH_CONFORMANCE_FRAMES").is_some() {
            Some(&observe)
        } else {
            None
        },
    };
    let informational = native::probes(fixture.checks)
        .iter()
        .any(|probe| probe == case);
    let result = if informational {
        native::run_probe(&mut rig.adapter, &fixture, case).await
    } else {
        native::run_case(&mut rig.adapter, &fixture, case).await
    };
    if !result.passed {
        eprintln!(
            "controlled model requests: {}",
            json!(
                rig.mock
                    .requests()
                    .into_iter()
                    .map(|request| json!({"path":request.path,"items":request.body.get("input").and_then(|input| input.as_array()).map(|items| items.iter().filter(|item| item.get("type").and_then(|kind| kind.as_str()) != Some("message")).collect::<Vec<_>>())}))
                    .collect::<Vec<_>>()
            )
        );
    }
    if case == "startup" {
        assert!(
            launcher
                .launches
                .lock()
                .unwrap()
                .iter()
                .any(|launch| launch.args.iter().any(|arg| arg == "app-server"))
        );
    }
    model_only::record(
        "codex",
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
    assert!(informational || result.passed, "{result:?}");
    result
}

async fn shared(case: &str) {
    let _ = observed(case).await;
}

#[tokio::test]
async fn a_thread_is_forked_in_its_home_and_reopened_in_a_copy_of_the_home() {
    let result = observed("native_import").await;
    assert_eq!(result.feature.as_deref(), Some("import_native"));
}

#[tokio::test]
async fn native_startup_and_model_calls_work_with_the_external_network_cut() {
    shared("startup").await;
}

#[tokio::test]
async fn native_command_approval_is_actionable_and_denial_prevents_the_write() {
    shared("command_approval").await;
}

#[tokio::test]
async fn native_user_input_is_answered_by_question_id() {
    shared("user_input").await;
}

#[tokio::test]
async fn the_unused_dynamic_surface_is_an_informational_probe() {
    shared("dynamic_tools").await;
}

#[tokio::test]
async fn a_loaded_mcp_server_elicits_a_real_form_and_receives_its_correlated_answer() {
    shared("elicitation").await;
}

#[tokio::test]
async fn native_subagent_activity_keeps_its_parent_stream() {
    shared("nested_subagent").await;
}

#[tokio::test]
async fn native_tool_answers_have_only_one_model_facing_form() {
    shared("tools.result_form").await;
}

#[tokio::test]
async fn a_fork_is_read_back_from_the_harness_as_the_turns_it_was_forked_through() {
    let result = observed("history.native_import").await;
    assert_eq!(result.feature.as_deref(), Some("import_native"));
    assert!(result.passed && result.error.is_none(), "{result:?}");
    assert!(result.unsupported.is_none(), "{result:?}");
}

#[tokio::test]
async fn a_native_bypass_turn_runs_a_command_without_an_approval_card() {
    let mut rig = common::Rig::start("bypass-command")
        .await
        .expect("the native conformance image is required");
    let network = model_only::ModelOnly::new(rig.mock.addr()).await;
    let binary = std::env::var("ORCH_CONFORMANCE_CODEX_BINARY").ok();
    let mut mounts: Vec<std::path::PathBuf> =
        vec![rig.home_path().into(), rig.workspace_path().into()];
    if let Some(path) = &binary {
        mounts.push(
            std::path::Path::new(path)
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .into(),
        );
    }
    let launcher = Arc::new(Isolated {
        network,
        mounts,
        launches: Mutex::new(Vec::new()),
    });
    rig.adapter = CodexAdapter::with_launcher(launcher);
    let mut start = rig.agent_start();
    start.binary = binary;
    rig.adapter.start(start, rig.sink.clone()).await.unwrap();
    let thread = orch_types::new_id();
    rig.open_new(thread).await;
    let marker = rig.workspace_path().join("bypass-marker");
    let script = format!(
        "#!mock\ntool exec_command {}\nthen\nsay command finished\n",
        json!({"cmd": format!("touch {}", marker.display())})
    );
    let turn = rig.turn(thread, &script);
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_turn_ends().await;
    assert!(marker.exists(), "the native command ran in its workspace");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
    );
    assert!(events.iter().any(|event| matches!(event, SessionEvent::TurnCompleted(end) if end.status == TurnOutcome::Completed)));
    rig.adapter.stop().await.unwrap();
}
