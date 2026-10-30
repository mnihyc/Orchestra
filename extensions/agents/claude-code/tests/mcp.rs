//! Orch's tools and the executor's tools reaching the real harness as MCP servers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::mcp::McpServer;
use common::{Rig, outcome, said, tool_calls};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{OrchTool, SessionEvent, ToolPayload, TurnOutcome};
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::SessionMode;
use orch_plugin_contract::Secret;
use serde_json::json;

fn answer(tool: &str, arguments: &serde_json::Value) -> (String, bool) {
    match tool {
        "announce" => (
            json!({ "delivered_to": 2, "title": arguments["title"] }).to_string(),
            false,
        ),
        "list_shared" => ("/ (nothing shared)".to_owned(), false),
        "Bash" | "shell" => ("a.txt\nb.txt".to_owned(), false),
        "Read" => ("1\thello from the other machine".to_owned(), false),
        "Write" => ("File created successfully".to_owned(), false),
        "Edit" => ("The file has been updated successfully.".to_owned(), false),
        _ => ("no such tool".to_owned(), true),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orchs_tools_are_a_server_the_harness_presents_the_connectors_token_to() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    let server = McpServer::start(
        &[(
            "orch",
            &[
                ("announce", "Tell everybody."),
                ("list_shared", "List what was shared."),
            ][..],
        )],
        answer,
    )
    .await;
    rig.start_beside(Loopback::new(
        server.address.port(),
        Secret::new(common::TOKEN),
    ))
    .await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.orch_tools = vec![OrchTool::Announce, OrchTool::ListShared];
    rig.open_with(spec).await;
    let script = "#!mock\nsay I will tell the others.\ntool mcp__orch__announce {\"title\":\"found it\",\"body\":\"the root cause\"}\nthen\nsay said it\n";
    let (_, events) = rig.run_deciding(script, common::allow).await;

    let calls = server.calls();
    assert_eq!(
        calls.len(),
        1,
        "{:?}",
        server.seen().iter().map(|r| &r.method).collect::<Vec<_>>()
    );
    assert_eq!(calls[0].params["name"], "announce");
    assert_eq!(calls[0].params["arguments"]["title"], "found it");
    for seen in server.seen() {
        assert_eq!(
            seen.authorization.as_deref(),
            Some(format!("Bearer {}", common::TOKEN).as_str()),
            "the token reaches the server from the environment, not from an argument: {seen:?}"
        );
    }
    let done = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(done) if done.payload.get("tool").is_some() => Some(done),
            _ => None,
        })
        .unwrap();
    assert_eq!(done.payload["tool"]["kind"], "orch", "{}", done.payload);
    assert_eq!(done.payload["tool"]["tool"], "announce");
    assert_eq!(done.payload["tool"]["arguments"]["title"], "found it");
    assert_eq!(
        done.payload["tool"]["result"]["delivered_to"], 2,
        "{}",
        done.payload
    );
    assert_eq!(done.payload["raw"]["name"], "mcp__orch__announce");
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_harness_with_its_provider_acts_through_the_executors_tools_and_they_look_like_its_own() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    let server = McpServer::start(
        &[(
            "exec",
            &[
                ("Bash", "Run a command."),
                ("Read", "Read a file."),
                ("Write", "Write a file."),
                ("Edit", "Edit a file."),
            ][..],
        )],
        answer,
    )
    .await;
    rig.start_beside(Loopback::new(
        server.address.port(),
        Secret::new(common::TOKEN),
    ))
    .await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.provider = orch_agent_contract::session::ProviderAccess::Native {
        provider_id: orch_types::new_id(),
    };
    spec.exec.executor = Some("orch-exec".into());
    spec.exec.tool_set = Some("claude-code".into());
    spec.settings.arrangement = orch_agent_contract::descriptor::ArrangementType::Separated;
    let opened = rig.open_with(spec).await;
    assert!(!opened.native_session_ref.is_empty());
    let script = "#!mock\nsay I will look around.\ntool mcp__exec__Bash {\"command\":\"ls\"}\ntool mcp__exec__Write {\"file_path\":\"/srv/work/new.txt\",\"content\":\"alpha\\nbeta\\n\"}\ntool mcp__exec__Read {\"file_path\":\"/srv/work/new.txt\"}\nthen\nsay done\n";
    // The model server is reached through the profile's environment: a native provider calls nothing of the connector.
    let (_, events) = rig.run_deciding(script, common::allow).await;

    let started = tool_calls(&events);
    let kinds: Vec<&str> = started
        .iter()
        .map(|call| match &call.tool {
            ToolPayload::Command { .. } => "command",
            ToolPayload::FileChange { .. } => "file_change",
            ToolPayload::FileRead { .. } => "file_read",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        ["command", "file_change", "file_read"],
        "{started:#?}"
    );
    assert_eq!(
        started[0].raw.name, "mcp__exec__Bash",
        "the call as the harness made it is kept whole"
    );
    let calls = server.calls();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|call| call.server == "exec"));

    // Nothing of the machine the harness is on was reachable: it was only given tools that touch none.
    let requests = rig.model.requests();
    let offered: Vec<String> = requests[0].body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    for own in ["Bash", "Read", "Write", "Edit", "WebFetch"] {
        assert!(!offered.contains(&own.to_owned()), "{own} in {offered:?}");
    }
    assert!(
        offered.contains(&"mcp__exec__Bash".to_owned()),
        "{offered:?}"
    );
    assert!(
        offered.contains(&"Agent".to_owned()),
        "a sub-agent touches no machine"
    );
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert!(said(&events).contains("done"));
    rig.adapter.stop().await.unwrap();
}
