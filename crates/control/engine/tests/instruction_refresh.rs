//! Changed standing instructions are resolved and reconfigured before the next accepted turn without creating another input.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
use common::{World, queued};
use orch_authz::Caller;
use orch_link_contract::commands::{
    AgentStartResult, Command, CommandError, CommandKind, CommandResult, ErrorCode,
};
use serde_json::json;

#[tokio::test]
async fn instructions_next_turn_reconfigures_the_current_session_only_when_the_resolved_text_changes()
 {
    let world = World::new().await;
    world
        .set(
            world.topic,
            "agent.instructions",
            json!("INITIAL-SCOPED-RULE"),
        )
        .await;
    let thread = world.thread("Instruction walk").await;
    for (turn, body) in ["first", "changed", "unchanged"].into_iter().enumerate() {
        if turn == 1 {
            world
                .set(
                    world.topic,
                    "agent.instructions",
                    json!("CURRENT-SCOPED-RULE"),
                )
                .await;
        }
        world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued(body))
            .await
            .unwrap();
        world.run_turn(thread, turn, "completed").await;
    }
    let opened = world.seen.of_kind(CommandKind::SessionOpen);
    assert_eq!(
        opened.len(),
        2,
        "the third turn keeps the already reconfigured session"
    );
    let Command::SessionOpen(first) = &opened[0] else {
        panic!("a session open")
    };
    let Command::SessionOpen(changed) = &opened[1] else {
        panic!("a session open")
    };
    assert!(first.instructions.contains("INITIAL-SCOPED-RULE"));
    assert!(changed.instructions.contains("CURRENT-SCOPED-RULE"));
    assert!(!changed.instructions.contains("INITIAL-SCOPED-RULE"));
    assert_eq!(first.workspace_root, changed.workspace_root);
    assert_eq!(first.directory, changed.directory);
    let mut conn = world.pool.acquire().await.unwrap();
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(inputs, 3, "reconfiguration adds no hidden input");
}

#[tokio::test]
async fn instructions_file_chain_codex_resumes_each_local_turn_without_inventing_an_input_or_a_forbidden_marker()
 {
    let world = World::new().await;
    let mut descriptor = common::descriptor();
    descriptor["kind"] = json!("codex");
    descriptor["native_provider"]["harness_kind"] = json!("codex");
    descriptor["capabilities"]["session_reload"] = json!("turn");
    descriptor["capabilities"]["instruction_root_marker"] = json!(true);
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE agents SET harness_kind = 'codex', descriptor = $2 WHERE id = $1")
        .bind(world.agent)
        .bind(&descriptor)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET harness_kind = 'codex' WHERE id = $1")
        .bind(world.provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    // The machine reports the harness as a machine does, before the first thread asks for it: a thread is made
    // only on a profile whose harness the machine has.
    sqlx::query(
        "UPDATE machines SET facts = jsonb_set(jsonb_set(jsonb_set(facts,
            '{adapters}', (facts -> 'adapters') || $2),
            '{harnesses}', (facts -> 'harnesses') || $3),
            '{policy,agent.harnesses}', (facts #> '{policy,agent.harnesses}') || $4) WHERE id = $1",
    )
    .bind(world.machine_id)
    .bind(json!([{ "kind": "codex", "adapter_version": 1, "builtin": true }]))
    .bind(
        json!([{ "kind": "codex", "path": "/test/harness", "version": "1.0.0", "daemon": false }]),
    )
    .bind(json!([{ "kind": "codex", "path": "/test/harness" }]))
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    world
        .machine
        .handler
        .on(CommandKind::AgentStart, move |_: &Command| {
            Ok(CommandResult::AgentStart(AgentStartResult {
                reused: false,
                harness_version: "1.0.0".to_owned(),
                descriptor: serde_json::from_value(descriptor.clone()).unwrap(),
            }))
        });
    world
        .machine
        .handler
        .on(CommandKind::FsMkdir, |_: &Command| {
            Err(CommandError::new(
                ErrorCode::PolicyRemoteWriteDisabled,
                "the local directory's write policy forbids the marker",
            ))
        });
    let thread = world.thread("Cached directory instructions").await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE threads SET executor = NULL WHERE node_id = $1")
        .bind(thread)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    for (turn, body) in ["first", "next", "again"].into_iter().enumerate() {
        world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued(body))
            .await
            .unwrap();
        world.run_turn(thread, turn, "completed").await;
    }
    let opened = world.seen.of_kind(CommandKind::SessionOpen);
    assert_eq!(
        opened.len(),
        3,
        "each local Codex turn reloads directory instructions by resume"
    );
    for (index, command) in opened.iter().enumerate() {
        let Command::SessionOpen(open) = command else {
            panic!("a session open")
        };
        assert!(
            !open.instruction_root_marker,
            "a refused preparation sends no override"
        );
        assert_eq!(
            open.mode,
            if index == 0 {
                orch_link_contract::commands::SessionMode::New
            } else {
                orch_link_contract::commands::SessionMode::Resume
            }
        );
    }
    let preparations: Vec<_> = world
        .commands
        .issued_bodies("fs.mkdir")
        .into_iter()
        .filter(|(_, body)| body["path"] == ".orch-root")
        .collect();
    assert_eq!(
        preparations.len(),
        opened.len(),
        "each resume verifies the workspace marker; ordinary private-directory mkdir commands are separate"
    );
    assert!(
        preparations
            .iter()
            .all(|(_, body)| body["path"] == ".orch-root"
                && body["environment_id"] == json!(world.environment))
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        inputs, 3,
        "refresh is a control operation, not another model input"
    );
}
