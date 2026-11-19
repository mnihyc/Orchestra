//! What a home already has (sessions, servers) and the transcript of a session carried to another home or read again, against the real harness.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;

use common::{Rig, outcome};
use orch_agent_contract::adapter::{AdapterError, AgentAdapter};
use orch_agent_contract::discovery::{McpImportRequest, McpScope, McpSearch, McpTransport};
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::history::ExportKind;
use orch_agent_contract::session::SessionMode;
use orch_types::new_id;
use serde_json::json;

fn completed(events: &[SessionEvent]) -> BTreeMap<String, serde_json::Value> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(done) => {
                Some((done.native_ref.clone(), done.payload.clone()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sessions_a_home_has_are_listed_with_what_was_asked_in_them() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    let opened = rig.open().await;
    rig.run("Fix the login redirect\nand then the tests").await;
    let sessions = rig.adapter.discover_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].native_session_ref, opened.native_session_ref);
    assert_eq!(sessions[0].title.as_deref(), Some("Fix the login redirect"));
    assert_eq!(sessions[0].cwd.as_deref(), Some(rig.directory().as_str()));
    assert!(sessions[0].updated_at.is_some());
    assert!(
        sessions[0].adopted_thread_id.is_none(),
        "the API says who adopted it"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn re_reading_the_transcript_gives_back_the_items_and_the_turns_that_were_lived() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    let script = "#!mock\nsay Let me run a command.\ntool Bash {\"command\":\"mkdir -p out && echo hello > out/hello.txt && cat out/hello.txt\",\"description\":\"Write a file\"}\nthen\nsay The command said: {{result}}\n";
    let (turn, live) = rig.run_deciding(script, common::allow).await;
    let (_, second) = rig.run("a second turn").await;
    let last_live_usage = second
        .iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::UsageUpdated(usage) => Some(usage.clone()),
            _ => None,
        })
        .expect("the live second turn reports its use");
    let mut lived = completed(&live);
    lived.extend(completed(&second));

    let items = rig.adapter.resync_session(rig.thread).await.unwrap();
    let replayed = rig.drain(500).await;
    let again = completed(&replayed);
    assert_eq!(u32::try_from(again.len()).unwrap(), items);
    assert_eq!(
        again.keys().collect::<Vec<_>>(),
        lived.keys().collect::<Vec<_>>(),
        "the same items under the same names"
    );
    let command = again
        .values()
        .find(|payload| payload["tool"]["kind"] == "command")
        .unwrap();
    assert_eq!(
        command["tool"]["command"],
        "mkdir -p out && echo hello > out/hello.txt && cat out/hello.txt"
    );
    assert_eq!(command["tool"]["output"]["inline"], "hello");
    let ends: Vec<_> = replayed
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TurnCompleted(end) => Some((end.turn_id, end.status)),
            _ => None,
        })
        .collect();
    assert_eq!(ends.len(), 2, "both turns finished");
    assert_eq!(
        ends[0],
        (turn, TurnOutcome::Completed),
        "the turn is known by the id the input carried"
    );
    // A local context-control reply from the live last turn can share this queue with resync.
    // The breakdown keeps the aggregate totals unchanged; historical use is not replayed or charged anew.
    for usage in replayed.iter().filter_map(|event| match event {
        SessionEvent::UsageUpdated(usage) => Some(usage),
        _ => None,
    }) {
        assert_eq!(
            usage.turn_id, last_live_usage.turn_id,
            "only the live last turn's context may arrive late"
        );
        assert_eq!(
            usage.turn, last_live_usage.turn,
            "reading history adds no turn use"
        );
        assert_eq!(
            usage.thread, last_live_usage.thread,
            "reading history adds no session use"
        );
        assert!(
            usage
                .report
                .as_ref()
                .is_some_and(|report| report.context.is_some()),
            "a late live breakdown is allowed; historical usage is not replayed"
        );
    }
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_exported_from_one_home_is_imported_into_another_and_remembered() {
    let Some(mut source) = Rig::new().await else {
        return;
    };
    source.start_quick().await;
    let opened = source.open().await;
    source.run("first marker-aaa").await;
    let archive = source
        .adapter
        .export_session(source.thread, ExportKind::Native, None)
        .await
        .unwrap();
    assert_eq!(archive.format, "claude-code-jsonl-v1");
    assert!(archive.size > 0);
    assert!(matches!(
        source
            .adapter
            .export_session(source.thread, ExportKind::Neutral, None)
            .await,
        Err(AdapterError::Unsupported(_))
    ));

    let Some(mut target) = Rig::new().await else {
        return;
    };
    target.start_quick().await;
    let mut spec = target.spec(SessionMode::ImportNative, None);
    spec.archive = Some(archive);
    let imported = target.adapter.open_session(spec).await.unwrap();
    assert_eq!(
        imported.native_session_ref, opened.native_session_ref,
        "the same conversation"
    );
    target
        .until(|event| matches!(event, SessionEvent::SessionOpened(_)), 20)
        .await;
    let (_, events) = target.run("second marker-bbb").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let request = target.last_request();
    assert!(
        request.contains("marker-aaa") && request.contains("marker-bbb"),
        "it remembers what the other home heard"
    );
    source.adapter.stop().await.unwrap();
    target.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_servers_the_homes_own_configuration_names_are_listed_and_read_with_their_secrets_apart()
 {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    let directory = rig.directory();
    std::fs::write(
        rig.home.path().join(".claude.json"),
        json!({
            "mcpServers": { "docs": { "command": "npx", "args": ["-y", "docs-server"],
                                      "env": { "DOCS_TOKEN": "s3cr3t-value", "MODE": "fast" } } },
            "projects": { directory.clone(): { "mcpServers": { "remote": { "type": "http", "url": "https://mcp.example.test/mcp" } } } }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        rig.work.path().join(".mcp.json"),
        json!({ "mcpServers": { "local": { "command": "./serve" } } }).to_string(),
    )
    .unwrap();
    rig.start_quick().await;
    let found = rig
        .adapter
        .discover_mcp(McpSearch {
            directories: vec![directory.clone()],
        })
        .await
        .unwrap();
    let names: Vec<_> = found
        .iter()
        .map(|server| (server.name.as_str(), server.scope, server.transport))
        .collect();
    assert_eq!(
        names,
        [
            ("docs", McpScope::User, McpTransport::Stdio),
            ("remote", McpScope::Directory, McpTransport::Http),
            ("local", McpScope::Directory, McpTransport::Stdio),
        ]
    );
    let listed = serde_json::to_string(&found).unwrap();
    assert!(
        !listed.contains("s3cr3t-value") && !listed.contains("npx"),
        "the list reveals no definition"
    );

    let definition = rig
        .adapter
        .read_mcp(McpImportRequest {
            server_id: new_id(),
            source: "claude-code".into(),
            scope: McpScope::User,
            name: "docs".into(),
            directory: None,
        })
        .await
        .unwrap();
    assert_eq!(definition.spec.secret_env, ["DOCS_TOKEN"]);
    assert_eq!(definition.secrets["DOCS_TOKEN"].expose(), "s3cr3t-value");
    assert!(
        !serde_json::to_string(&definition.spec)
            .unwrap()
            .contains("s3cr3t-value")
    );
    rig.adapter.stop().await.unwrap();
}
