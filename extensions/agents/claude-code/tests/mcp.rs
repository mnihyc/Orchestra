//! Orch's tools and the executor's tools reaching the real harness as MCP servers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::mcp::McpServer;
use common::{Rig, outcome, said, tool_calls};
use futures::StreamExt;
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
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
async fn a_native_mcp_image_keeps_its_bytes_scope_and_identity_after_resync() {
    use base64::Engine as _;
    let mut rig = Rig::new()
        .await
        .expect("the candidate native harness image is required");
    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(PIXEL)
        .unwrap();
    let hash = orch_types::Hash::of(&bytes);
    let server = McpServer::start_content(
        &[("camera", &[("snapshot", "Return a PNG image")][..])],
        |_, _| {
            (
                vec![
                    json!({"type":"text","text":"Camera result"}),
                    json!({"type":"image","mimeType":"image/png","data":PIXEL}),
                ],
                false,
            )
        },
    )
    .await;
    rig.start_beside(Loopback::new(
        server.address.port(),
        Secret::new(common::TOKEN),
    ))
    .await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.mcp = vec!["camera".into()];
    rig.open_with(spec).await;
    let (turn, events) = rig
        .run_deciding(
            "#!mock\ntool mcp__camera__snapshot {}\nthen\nsay Image received\n",
            common::allow,
        )
        .await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    assert_eq!(server.calls().len(), 1);
    let parent = events.iter().find_map(|event| match event {
        SessionEvent::ItemStarted(item) if matches!(&item.content,
            orch_agent_contract::events::ItemContent::ToolCall(call) if matches!(&call.tool, ToolPayload::Mcp { server, tool, .. } if server == "camera" && tool == "snapshot")) => Some(item.native_ref.clone()),
        _ => None,
    }).expect("the actual native MCP call is visible");
    let image = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item)
                if item
                    .payload
                    .pointer("/tool/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("image") =>
            {
                Some(item.clone())
            }
            _ => None,
        })
        .expect("the actual CLI returned an image part, not placeholder prose");
    assert_eq!(image.output_blob.as_ref().unwrap().hash, hash);
    assert_eq!(image.payload["tool"]["blob"], hash.to_string());
    assert_eq!(image.payload["tool"]["media_type"], "image/png");
    assert!(!image.payload.to_string().contains(PIXEL));
    let started = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemStarted(item) if item.native_ref == image.native_ref => Some(item),
            _ => None,
        })
        .unwrap();
    assert_eq!(started.turn_id, turn);
    assert_eq!(started.stream, "main");
    assert_eq!(started.parent_native_ref.as_deref(), Some(parent.as_str()));
    let cache = rig.adapter.output_directory().unwrap();
    assert_eq!(cache, rig.home.path().join("outputs"));
    assert_eq!(std::fs::read(cache.join(hash.to_string())).unwrap(), bytes);
    let requests = rig
        .model
        .requests()
        .iter()
        .filter(|request| request.body.get("messages").is_some())
        .count();
    assert!(rig.adapter.resync_session(rig.thread).await.unwrap() > 0);
    let replayed = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match rig
                .events
                .next()
                .await
                .expect("resync must publish its image")
            {
                AdapterEvent::Session {
                    event: SessionEvent::ItemCompleted(item),
                    ..
                } if item.native_ref == image.native_ref => break item,
                _ => {}
            }
        }
    })
    .await
    .expect("the exact original image is the resync barrier");
    assert_eq!(replayed.payload["tool"]["blob"], hash.to_string());
    assert_eq!(replayed.output_blob.as_ref().unwrap().hash, hash);
    assert_eq!(
        server.calls().len(),
        1,
        "resync never repeats the MCP operation"
    );
    assert_eq!(
        rig.model
            .requests()
            .iter()
            .filter(|request| request.body.get("messages").is_some())
            .count(),
        requests,
        "resync never starts a model call"
    );
    assert_eq!(std::fs::read(cache.join(hash.to_string())).unwrap(), bytes);
    rig.adapter.stop().await.unwrap();
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
    // The isolated launcher supplies the mock subscription transport; no provider variable is admitted from the profile.
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

    // The provider runtime exposes no native shell or files. Its native page reader remains available.
    let requests = rig.model.requests();
    let offered: Vec<String> = requests[0].body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    for own in ["Bash", "Read", "Write", "Edit"] {
        assert!(!offered.contains(&own.to_owned()), "{own} in {offered:?}");
    }
    assert!(
        offered.contains(&"WebFetch".to_owned()),
        "native pages are read from the provider runtime: {offered:?}"
    );
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
