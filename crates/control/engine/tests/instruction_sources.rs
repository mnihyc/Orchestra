//! What a session is opened with is kept with its segment, source by source, in the order the agent reads it: Orch's
//! introduction first, each place's standing instructions with the hash of their words (by which a later change is
//! known), then the files its harness reported, a file of the harness's home without its path or environment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, eventually, queued};
use orch_agent_contract::session::{InstructionEvidence, InstructionFile};
use orch_authz::Caller;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, SessionMode, SessionOpenResult,
};
use orch_types::Json;
use serde_json::json;
use std::path::Path;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_a_session_was_opened_with_is_kept_source_by_source() {
    let world = World::new().await;
    world
        .set(
            world.topic,
            "agent.instructions",
            json!("Answer in one line."),
        )
        .await;
    // A harness that reports two files: its home's own, and the thread directory's.
    world
        .machine
        .handler
        .on(CommandKind::SessionOpen, |command| {
            let Command::SessionOpen(open) = command else {
                return Err(CommandError::new(ErrorCode::Internal, "not a session.open"));
            };
            Ok(CommandResult::SessionOpen(SessionOpenResult {
                native_session_ref: format!("native-{}", open.segment_id),
                resumed: open.mode == SessionMode::Resume,
                import_report: None,
                approval_reviewer: None,
                instruction_sources: vec![
                    InstructionFile::unmeasured(
                        "AGENTS.md of the harness's home",
                        None,
                        InstructionEvidence::Reported,
                        "a file of the harness's home",
                    )
                    .of_home(),
                    InstructionFile::unmeasured(
                        "AGENTS.md",
                        Some(Path::new("/workspace/web/AGENTS.md")),
                        InstructionEvidence::Reported,
                        "in the environment the tools run in",
                    ),
                ],
            }))
        });
    let thread = world.thread("Sources").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "Done").await;
    let kept: Json = world
        .scalar(
            "SELECT instruction_sources FROM segments WHERE thread_id = $1",
            thread,
        )
        .await;
    let sources = kept.as_array().unwrap();
    let kinds: Vec<&str> = sources
        .iter()
        .map(|source| source["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"preamble"), "{kept}");
    assert_eq!(&kinds[kinds.len() - 2..], ["file", "file"], "{kept}");
    let standing = sources
        .iter()
        .find(|source| source["kind"] == "standing")
        .unwrap();
    assert_eq!(standing["scope"], json!(world.topic));
    assert_eq!(standing["evidence"], "orch");
    assert!(standing["hash"].is_string());
    let home = &sources[sources.len() - 2];
    assert_eq!(home["home"], true);
    assert!(home.get("path").is_none_or(Json::is_null));
    assert!(home["environment"].is_null());
    let file = &sources[sources.len() - 1];
    assert_eq!(file["path"], "/workspace/web/AGENTS.md");
    assert_eq!(file["evidence"], "reported");
    assert_eq!(file["environment"], json!(world.environment));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn later_instruction_loads_refresh_the_list_without_creating_input_or_activity() {
    let world = World::new().await;
    let thread = world.thread("Later loads").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "Done").await;
    let before_items = world
        .count("SELECT count(*) FROM items WHERE thread_id=$1", thread)
        .await;
    let before_inputs = world
        .count(
            "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
            thread,
        )
        .await;
    let before_turns = world
        .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
        .await;
    let mut imported = InstructionFile::unmeasured(
        "nested/rule.md",
        Some(Path::new("/workspace/nested/rule.md")),
        InstructionEvidence::Reported,
        "loaded by an include, loaded size unknown",
    );
    imported.load_reason = Some("include".to_owned());
    imported.parent_path = Some("/workspace/CLAUDE.md".to_owned());
    let mut private = InstructionFile::unmeasured(
        "personal rules",
        Some(Path::new("/private-home/rules.md")),
        InstructionEvidence::Reported,
        "private source",
    );
    private.parent_path = Some("/private-home/CLAUDE.md".to_owned());
    for _ in 0..2 {
        world
            .say(
                thread,
                orch_link_contract::events::Event::InstructionSources(
                    orch_agent_contract::events::InstructionSources {
                        sources: vec![imported.clone(), private.clone()],
                    },
                ),
            )
            .await;
    }
    eventually("the reported source list is refreshed", || async {
        let sources: Json = world
            .scalar(
                "SELECT instruction_sources FROM segments WHERE thread_id=$1",
                thread,
            )
            .await;
        sources.as_array().is_some_and(|sources| {
            sources
                .iter()
                .any(|source| source["label"] == "nested/rule.md")
        })
    })
    .await;
    let sources: Json = world
        .scalar(
            "SELECT instruction_sources FROM segments WHERE thread_id=$1",
            thread,
        )
        .await;
    let files = sources.as_array().unwrap();
    assert_eq!(
        files
            .iter()
            .filter(|source| source["label"] == "nested/rule.md")
            .count(),
        1
    );
    let imported = files
        .iter()
        .find(|source| source["label"] == "nested/rule.md")
        .unwrap();
    assert_eq!(imported["parent_path"], "/workspace/CLAUDE.md");
    assert_eq!(imported["load_reason"], "include");
    assert!(imported["observed_at"].is_string());
    let private = files
        .iter()
        .find(|source| source["label"] == "personal rules")
        .unwrap();
    assert!(
        private["home"] == true
            && private["path"].is_null()
            && private["parent_path"].is_null()
            && private["environment"].is_null()
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM items WHERE thread_id=$1", thread)
            .await,
        before_items
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                thread
            )
            .await,
        before_inputs
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        before_turns
    );
}
