//! Actual pinned model-request bytes across control changes and a next-turn MCP schema refresh.

use super::*;
use orch_agent_contract::descriptor::SandboxMode;

const CORE: &str = "ORCH-FIXED-CORE-BEGIN\nUse the same fixed standing instructions for every real input.\nORCH-FIXED-CORE-END";

fn cores(value: &serde_json::Value) -> Vec<Vec<u8>> {
    match value {
        serde_json::Value::String(text) => {
            let Some(start) = text.find("ORCH-FIXED-CORE-BEGIN") else {
                return Vec::new();
            };
            assert_eq!(
                text.matches("ORCH-FIXED-CORE-BEGIN").count(),
                1,
                "opening text is not duplicated in a string"
            );
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
                .is_some_and(|name| name.starts_with("mcp__orch__"))
        })
        .cloned()
        .collect();
    assert!(
        !tools.is_empty(),
        "the native request must contain actual Orch definitions"
    );
    serde_json::to_vec(&tools).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_modes_and_capability_context_preserve_core_and_tool_bytes_and_mcp_refresh_resumes_history()
 {
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
    let started = rig
        .start_beside(Loopback::new(server.port, Secret::new(common::TOKEN)))
        .await;
    assert!(
        !started.descriptor.capabilities.mcp.reload,
        "the real adapter never promises its unsupported reload method"
    );
    let fixed = rig.work.path().join("fixed-context.txt");
    std::fs::write(
        &fixed,
        b"Fixed workspace data; no mode or capability changes this file.\n",
    )
    .unwrap();
    let fixed_bytes = std::fs::read(&fixed).unwrap();
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.instructions = CORE.into();
    spec.settings.orch_tools = vec![OrchTool::ListShared, OrchTool::ReadShared];
    spec.settings.approval_mode = ApprovalMode::OnRequest;
    spec.settings.sandbox_mode = SandboxMode::Open;
    let opened = rig.open_with(spec.clone()).await;
    // The harness's workspace sandbox needs bubblewrap and socat and cannot start in a stock
    // container, where the container is the boundary and only open is enforced: the controls
    // changed here are those such a place runs.
    let modes = [
        (ApprovalMode::OnRequest, SandboxMode::Open),
        (ApprovalMode::Ask, SandboxMode::Open),
        (ApprovalMode::Bypass, SandboxMode::Open),
        (ApprovalMode::OnRequest, SandboxMode::Open),
        (ApprovalMode::OnRequest, SandboxMode::Open),
    ];
    let mut baseline_tools = None;
    let mut previous_processes = None;
    for (index, (approval, sandbox)) in modes.into_iter().enumerate() {
        // These are ordinary turn controls on one opening, as the engine sends them. The local
        // process independently resumes between turns to read workspace guidance again.
        let text = if index == 4 {
            "<orch>\nWhere you are\n- The environment has: kvm.\n</orch>\n\n#!mock\nsay capability-context-only\n".to_owned()
        } else {
            format!("#!mock\nsay stable-control-marker-{index}\n")
        };
        let mut turn = rig.turn(&text);
        turn.approval_mode = approval;
        turn.sandbox_mode = sandbox;
        let before_requests = rig.model.requests().len();
        rig.adapter.start_turn(turn).await.unwrap();
        assert_eq!(
            common::outcome(&rig.until_the_turn_ends().await),
            TurnOutcome::Completed
        );
        assert_eq!(
            rig.model.requests().len(),
            before_requests + 1,
            "one model request serves this real input"
        );
        let request = rig.model.requests().last().unwrap().body.clone();
        assert_eq!(
            cores(&request),
            vec![CORE.as_bytes().to_vec()],
            "the exact opening core appears once"
        );
        let tools = own_tool_bytes(&request);
        if let Some(before) = &baseline_tools {
            assert_eq!(&tools, before);
        } else {
            baseline_tools = Some(tools);
        }
        let processes = launcher.names.lock().unwrap().len();
        if let Some(before) = previous_processes {
            assert_eq!(
                processes,
                before + 1,
                "one local native resume refreshes files, without a new conversation"
            );
        }
        previous_processes = Some(processes);
        assert_eq!(std::fs::read(&fixed).unwrap(), fixed_bytes);
        assert!(
            !rig.work.path().join("AGENTS.md").exists(),
            "control changes do not invent workspace instructions"
        );
        if index == 4 {
            assert!(has_text(&request, "The environment has: kvm."));
            assert!(
                has_text(&request, "stable-control-marker-0"),
                "history survives control changes"
            );
        }
    }
    server.schemas.lock().unwrap().insert(
        "read_shared".into(),
        json!({"type":"object","additionalProperties":false,
        "properties":{"refreshed_marker":{"type":"string","enum":["CURRENT-MCP-SCHEMA"]}}}),
    );
    spec.settings.mode = SessionMode::Resume;
    spec.settings.native_session_ref = Some(opened.native_session_ref.clone());
    spec.settings.approval_mode = ApprovalMode::OnRequest;
    spec.settings.sandbox_mode = SandboxMode::Open;
    spec.session_key = "accepted-next-turn-schema".into();
    let before = rig.model.requests().len();
    // The conversation is open here: opening it again changes what its next turn starts with and
    // is no new opening of a session, so nothing announces one.
    let resumed = rig.adapter.open_session(spec.clone()).await.unwrap();
    assert!(resumed.resumed);
    assert_eq!(resumed.native_session_ref, opened.native_session_ref);
    assert_eq!(rig.model.requests().len(), before);
    let mut turn = rig.turn("#!mock\nsay after-mcp-refresh\n");
    turn.approval_mode = ApprovalMode::OnRequest;
    turn.sandbox_mode = SandboxMode::Open;
    rig.adapter.start_turn(turn).await.unwrap();
    assert_eq!(
        common::outcome(&rig.until_the_turn_ends().await),
        TurnOutcome::Completed
    );
    assert_eq!(rig.model.requests().len(), before + 1);
    let request = rig.model.requests().last().unwrap().body.clone();
    assert_eq!(cores(&request), vec![CORE.as_bytes().to_vec()]);
    let changed = own_tool_bytes(&request);
    assert_ne!(
        Some(&changed),
        baseline_tools.as_ref(),
        "a successful resume must really load the changed schema"
    );
    assert!(
        String::from_utf8(changed)
            .unwrap()
            .contains("CURRENT-MCP-SCHEMA")
    );
    assert!(has_text(&request, "stable-control-marker-0"));
    assert_eq!(
        launcher.names.lock().unwrap().len(),
        previous_processes.unwrap() + 1
    );
    assert_eq!(std::fs::read(&fixed).unwrap(), fixed_bytes);
    rig.adapter.stop().await.unwrap();
}
