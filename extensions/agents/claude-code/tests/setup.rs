//! What the harness loads and keeps, against the real harness: instruction files, its memory, the servers of its own configuration.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, outcome};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{OrchTool, SessionEvent, TurnOutcome};
use orch_agent_contract::session::{NoteKeeper, SessionMemory, SessionMode};

/// The `system/init` message of the harness, from the diagnostics of a turn.
fn init_of(events: &[SessionEvent]) -> serde_json::Value {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::RawFrame(raw)
                if raw.direction == "from_harness"
                    && raw.payload["type"] == "system"
                    && raw.payload["subtype"] == "init" =>
            {
                Some(raw.payload.clone())
            }
            _ => None,
        })
        .expect("the harness said how it started")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_instruction_file_of_the_directory_is_read_beside_the_harnesss_own() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    std::fs::write(
        rig.work.path().join("CLAUDE.md"),
        "CLAUDE-MD-MARKER keep this\n",
    )
    .unwrap();
    std::fs::write(
        rig.work.path().join("AGENTS.md"),
        "AGENTS-MD-MARKER keep this\n",
    )
    .unwrap();
    rig.start_quick().await;
    rig.open().await;
    let (_, events) = rig.run("hello").await;
    assert_eq!(outcome(&events), TurnOutcome::Completed);
    let request = rig.last_request();
    assert!(request.contains("CLAUDE-MD-MARKER"), "its own file");
    assert!(
        request.contains("AGENTS-MD-MARKER"),
        "the file of the directory, beside it"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_memory_of_the_harness_is_in_the_thread_directory_and_never_in_its_home() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    let memory = rig.work.path().join(".orch/memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(
        memory.join("MEMORY.md"),
        "- [note](note.md) MEMORY-INDEX-MARKER\n",
    )
    .unwrap();
    std::fs::write(memory.join("note.md"), "a note\n").unwrap();
    rig.start_quick().await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.memory = Some(SessionMemory {
        directory: memory.display().to_string(),
        kept_by: NoteKeeper::Harness,
    });
    spec.settings.raw_events = true;
    rig.open_with(spec).await;
    let (_, events) = rig.run("hello").await;
    let request = rig.last_request();
    assert!(
        request.contains("MEMORY-INDEX-MARKER"),
        "the index of the thread's directory is loaded"
    );
    assert!(request.contains(&memory.display().to_string()));
    assert_eq!(
        init_of(&events)["memory_paths"]["auto"],
        format!("{}/", memory.display())
    );
    let in_home: Vec<_> = glob_memory(rig.home.path());
    assert!(
        in_home.is_empty(),
        "nothing of a memory in the home: {in_home:?}"
    );
    rig.adapter.stop().await.unwrap();
}

fn glob_memory(home: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    if let Ok(projects) = std::fs::read_dir(home.join("projects")) {
        for project in projects.filter_map(Result::ok) {
            let memory = project.path().join("memory");
            if memory.exists() {
                found.push(memory);
            }
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_keeps_no_memory_has_none_of_the_harnesss_own() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    let memory = rig.work.path().join(".orch/memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join("MEMORY.md"), "- MEMORY-INDEX-MARKER\n").unwrap();
    rig.start_quick().await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.raw_events = true;
    rig.open_with(spec).await;
    let (_, events) = rig.run("hello").await;
    assert!(
        init_of(&events).get("memory_paths").is_none(),
        "{}",
        init_of(&events)
    );
    assert!(!rig.last_request().contains("MEMORY-INDEX-MARKER"));
    assert!(glob_memory(rig.home.path()).is_empty());
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_harness_loads_no_server_that_orch_did_not_give_it() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    // A server in the directory's own file and one in the user's file of the home.
    let server = r#"{ "mcpServers": { "stray": { "command": "sleep", "args": ["600"] } } }"#;
    std::fs::write(rig.work.path().join(".mcp.json"), server).unwrap();
    std::fs::create_dir_all(rig.home.path()).unwrap();
    std::fs::write(rig.home.path().join(".claude.json"), server).unwrap();
    rig.start_quick().await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.raw_events = true;
    rig.open_with(spec).await;
    let (_, events) = rig.run("hello").await;
    let init = init_of(&events);
    assert_eq!(init["mcp_servers"], serde_json::json!([]), "{init}");
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn everything_that_starts_a_turn_by_itself_is_offered_to_nobody() {
    let Some(mut rig) = Rig::new().await else {
        return;
    };
    rig.start_quick().await;
    rig.open().await;
    rig.run("hello").await;
    let requests = rig.model.requests();
    let tools: Vec<String> = requests[0].body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    for never in [
        "CronCreate",
        "ScheduleWakeup",
        "Workflow",
        "SendMessage",
        "EnterPlanMode",
        "ExitWorktree",
    ] {
        assert!(!tools.contains(&never.to_owned()), "{never} in {tools:?}");
    }
    for needed in [
        "Bash",
        "Read",
        "Write",
        "Edit",
        "Agent",
        "AskUserQuestion",
        "TaskCreate",
    ] {
        assert!(
            tools.contains(&needed.to_owned()),
            "{needed} missing from {tools:?}"
        );
    }
    let _ = OrchTool::Announce;
    rig.adapter.stop().await.unwrap();
}
