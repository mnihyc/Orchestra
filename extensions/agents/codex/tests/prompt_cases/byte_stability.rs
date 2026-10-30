//! Actual pinned Responses requests keep the fixed core and own definitions across turn controls.

use super::*;
use orch_agent_contract::descriptor::SandboxMode;

const CORE: &str = "ORCH-FIXED-CORE-BEGIN\nUse the same fixed standing instructions for every real input.\nORCH-FIXED-CORE-END";

fn cores(value: &serde_json::Value) -> Vec<Vec<u8>> {
    match value {
        serde_json::Value::String(text) => {
            let Some(start) = text.find("ORCH-FIXED-CORE-BEGIN") else {
                return Vec::new();
            };
            assert_eq!(text.matches("ORCH-FIXED-CORE-BEGIN").count(), 1);
            let end = text[start..].find("ORCH-FIXED-CORE-END").unwrap()
                + start
                + "ORCH-FIXED-CORE-END".len();
            vec![text.as_bytes()[start..end].to_vec()]
        }
        serde_json::Value::Array(values) => values.iter().flat_map(cores).collect(),
        serde_json::Value::Object(values) => values.values().flat_map(cores).collect(),
        _ => Vec::new(),
    }
}

fn own_tool_bytes(request: &serde_json::Value) -> Vec<u8> {
    let tools: Vec<_> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| {
            tool["name"]
                .as_str()
                .is_some_and(|name| name == "mcp__orch" || name.starts_with("mcp__orch__"))
        })
        .cloned()
        .collect();
    assert!(
        !tools.is_empty(),
        "the native request must contain the real Orch namespace or functions"
    );
    serde_json::to_vec(&tools).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_modes_and_capability_context_preserve_core_tool_bytes_and_native_conversation_identity()
 {
    let mut rig = common::Rig::start("stable-control-bytes")
        .await
        .expect("the pinned image is required");
    let launcher = Arc::new(Docker {
        mounts: vec![rig.home.path().into(), rig.workspace.path().into()],
        names: Mutex::new(Vec::new()),
    });
    rig.adapter = CodexAdapter::with_launcher(launcher.clone());
    rig.mock = common::start_mock(
        orch_testkit::doubles::model::MockConfig::default().with_credential("prompt-test-token"),
    )
    .await;
    let server = server(rig.mock.url(), true).await;
    rig.loopback_port = Some(server.port);
    let started = rig.boot().await;
    assert!(!started.descriptor.capabilities.mcp.reload);
    let fixed = rig.workspace.path().join("fixed-context.txt");
    std::fs::write(
        &fixed,
        b"Fixed workspace data; control changes leave these bytes alone.\n",
    )
    .unwrap();
    let fixed_bytes = std::fs::read(&fixed).unwrap();
    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.settings.instructions = CORE.into();
    spec.settings.orch_tools = vec![OrchTool::ListShared, OrchTool::ReadShared];
    spec.settings.approval_mode = ApprovalMode::OnRequest;
    spec.settings.sandbox_mode = SandboxMode::Open;
    let opened = rig.adapter.open_session(spec.clone()).await.unwrap();
    let processes = launcher.names.lock().unwrap().len();
    let modes = [
        (ApprovalMode::OnRequest, SandboxMode::Open),
        (ApprovalMode::Ask, SandboxMode::Open),
        (ApprovalMode::Ask, SandboxMode::Workspace),
        (ApprovalMode::OnRequest, SandboxMode::Workspace),
        (ApprovalMode::OnRequest, SandboxMode::Workspace),
    ];
    let mut baseline_tools = None;
    for (index, (approval, sandbox)) in modes.into_iter().enumerate() {
        if index > 0 {
            // The engine's local Codex boundary resumes even with fixed instructions/files.
            spec.settings.mode = SessionMode::Resume;
            spec.settings.native_session_ref = Some(opened.native_session_ref.clone());
            spec.settings.approval_mode = approval;
            spec.settings.sandbox_mode = sandbox;
            spec.session_key = format!("stable-controls-{index}");
            let before = rig.mock.requests().len();
            let resumed = rig.adapter.open_session(spec.clone()).await.unwrap();
            assert!(resumed.resumed);
            assert_eq!(resumed.native_session_ref, opened.native_session_ref);
            assert_eq!(
                rig.mock.requests().len(),
                before,
                "resume is not an extra model input"
            );
        }
        let text = if index == 4 {
            "<orch>\nWhere you are\n- The environment has: kvm.\n</orch>\n\n#!mock\nsay capability-context-only\n".to_owned()
        } else {
            format!("#!mock\nsay stable-control-marker-{index}\n")
        };
        let mut turn = rig.turn(thread, &text);
        turn.approval_mode = approval;
        turn.sandbox_mode = sandbox;
        let before_requests = rig.mock.requests().len();
        rig.adapter.start_turn(turn).await.unwrap();
        let events = rig.until_turn_ends().await;
        assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(done) if done.status==TurnOutcome::Completed)),"{events:?}");
        assert_eq!(
            rig.mock.requests().len(),
            before_requests + 1,
            "one model request serves this real input"
        );
        let request = rig.mock.requests().last().unwrap().body.clone();
        assert_eq!(cores(&request), vec![CORE.as_bytes().to_vec()]);
        let tools = own_tool_bytes(&request);
        if let Some(before) = &baseline_tools {
            assert_eq!(&tools, before);
        } else {
            baseline_tools = Some(tools);
        }
        assert_eq!(
            launcher.names.lock().unwrap().len(),
            processes,
            "thread resume reuses the native app-server process"
        );
        assert_eq!(std::fs::read(&fixed).unwrap(), fixed_bytes);
        assert!(!rig.workspace.path().join("AGENTS.md").exists());
        if index == 4 {
            assert!(has_text(&request, "The environment has: kvm."));
            assert!(has_text(&request, "stable-control-marker-0"));
        }
    }
    server.schemas.lock().unwrap().insert(
        "read_shared".into(),
        json!({"type":"object","additionalProperties":false,
        "properties":{"refreshed_marker":{"type":"string","enum":["CURRENT-MCP-SCHEMA"]}}}),
    );
    spec.session_key = "accepted-next-turn-schema".into();
    let before = rig.mock.requests().len();
    let resumed = rig.adapter.open_session(spec.clone()).await.unwrap();
    assert!(resumed.resumed);
    assert_eq!(resumed.native_session_ref, opened.native_session_ref);
    assert_eq!(
        rig.mock.requests().len(),
        before,
        "schema refresh is not an extra model input"
    );
    let mut turn = rig.turn(thread, "#!mock\nsay after-mcp-refresh\n");
    turn.approval_mode = ApprovalMode::OnRequest;
    turn.sandbox_mode = SandboxMode::Workspace;
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_turn_ends().await;
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(done) if done.status==TurnOutcome::Completed)),"{events:?}");
    assert_eq!(rig.mock.requests().len(), before + 1);
    let request = rig.mock.requests().last().unwrap().body.clone();
    assert_eq!(cores(&request), vec![CORE.as_bytes().to_vec()]);
    let changed = own_tool_bytes(&request);
    assert_ne!(Some(&changed), baseline_tools.as_ref());
    assert!(
        String::from_utf8(changed)
            .unwrap()
            .contains("CURRENT-MCP-SCHEMA")
    );
    assert!(has_text(&request, "stable-control-marker-0"));
    assert_eq!(launcher.names.lock().unwrap().len(), processes);
    assert_eq!(std::fs::read(&fixed).unwrap(), fixed_bytes);
    rig.adapter.stop().await.unwrap();
}
