//! A session whose tools are carried out by a Codex exec-server in a container of its own, while the harness
//! runs in another that cannot reach the workspace, on the real Codex in Docker.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::remote::{ExecServer, SplitProxy};
use common::{DockerLauncher, Rig, kinds, script};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ArrangementType;
use orch_agent_contract::events::{
    FileChangeKind, ItemStatus, SessionEvent, ToolCallPayload, ToolPayload, TurnOutcome,
};
use orch_agent_contract::session::SessionMode;
use orch_types::new_id;

#[tokio::test]
async fn the_tools_of_a_session_are_carried_out_by_an_exec_server_in_another_container() {
    let Some(mut rig) = Rig::start_with("remote", DockerLauncher::without_workspace).await else {
        return;
    };
    let workspace = rig.workspace_path().to_path_buf();
    let exec = ExecServer::start("remote", &workspace, &rig.token).await;
    let proxy = SplitProxy::start(rig.mock.addr(), exec.addr()).await;
    rig.loopback_port = Some(proxy.port());
    rig.boot().await;

    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.exec.executor = Some("codex-exec-server".to_owned());
    spec.settings.arrangement = ArrangementType::Separated;
    rig.adapter.open_session(spec).await.unwrap();

    // A command: it runs where the exec-server is, since the harness's own container has no such directory.
    let text = script(
        "write it",
        &[
            &format!(
                r#"tool exec_command {{"cmd":"echo from-the-executor > {0}/out.txt && echo printed-there && pwd"}}"#,
                workspace.display()
            ),
            "then",
            "say written",
        ],
    );
    let events = rig.run_turn(thread, &text).await;
    let call = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) => {
                serde_json::from_value::<ToolCallPayload>(item.payload.clone()).ok()
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("a tool call completes: {:?}", kinds(&events)));
    let ToolPayload::Command {
        exit_code,
        output,
        cwd,
        ..
    } = &call.tool
    else {
        panic!("a command");
    };
    assert_eq!(*exit_code, Some(0), "{:?}", call.tool);
    let shown = output.inline.clone();
    assert!(shown.contains("printed-there"), "{shown:?}");
    assert!(shown.contains(&*workspace.to_string_lossy()), "{shown:?}");
    assert_eq!(cwd, &*workspace.to_string_lossy());
    assert_eq!(
        std::fs::read_to_string(workspace.join("out.txt")).unwrap(),
        "from-the-executor\n"
    );
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );

    // A change of files, made by the harness's own tool for it, reaches the executor's disk too.
    let patch = "apply_patch <<'PATCH'\\n*** Begin Patch\\n*** Add File: made.txt\\n+made there\\n*** End Patch\\nPATCH";
    let text = script(
        "patch it",
        &[
            &format!(r#"tool exec_command {{"cmd":"{patch}"}}"#),
            "then",
            "say patched",
        ],
    );
    let events = rig.run_turn(thread, &text).await;
    let changed = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.status == ItemStatus::Completed => {
                match serde_json::from_value::<ToolCallPayload>(item.payload.clone()) {
                    Ok(call) => match call.tool {
                        ToolPayload::FileChange { changes } => Some(changes),
                        _ => None,
                    },
                    Err(_) => None,
                }
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("a change of files completes: {:?}", kinds(&events)));
    assert_eq!(changed[0].change, FileChangeKind::Add);
    assert_eq!(
        std::fs::read_to_string(workspace.join("made.txt")).unwrap(),
        "made there\n"
    );
    rig.adapter.stop().await.unwrap();
}
