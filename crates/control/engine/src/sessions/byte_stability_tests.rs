//! Real admission and current-snapshot bytes, with fixed standing text and an unchanged workspace.

use super::*;
use crate::test_common as common;
use common::{World, queued};
use orch_agent_contract::events::OrchTool;
use orch_authz::Caller;
use orch_link_contract::commands::{AgentStartResult, Command, CommandKind, CommandResult};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn world() -> TestResult<World> {
    let world = World::new().await;
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["orch_tools"] = json!("mcp");
    descriptor["capabilities"]["mcp"] =
        json!({"stdio":true,"http":true,"reload":false,"strict":true});
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(world.agent)
        .bind(&descriptor)
        .execute(world.pool.pool())
        .await?;
    world.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: false,
            harness_version: "1.0.0".into(),
            descriptor: serde_json::from_value(descriptor.clone()).map_err(|error| {
                orch_link_contract::commands::CommandError::new(
                    orch_link_contract::commands::ErrorCode::Internal,
                    error.to_string(),
                )
            })?,
        }))
    });
    world
        .set(
            world.topic,
            "orch_tools.enabled",
            json!(["list_shared", "read_shared"]),
        )
        .await;
    world
        .set(
            world.topic,
            "agent.instructions",
            json!("FIXED-CORE-RULE: preserve the fixed workspace and its instructions."),
        )
        .await;
    world.set(world.topic, "approval.mode", json!("ask")).await;
    world
        .set(world.topic, "sandbox.mode", json!("workspace"))
        .await;
    Ok(world)
}

async fn snapshot(world: &World, thread: Id) -> TestResult<StartSnapshot> {
    let mut conn = world.pool.acquire().await?;
    match world
        .engine()
        .load_turn_snapshot(&mut conn, thread, threads::turn_state::RUNNING)
        .await?
    {
        StartLoad::Ready(snapshot) => Ok(*snapshot),
        _ => Err("the accepted turn has no current snapshot".into()),
    }
}

fn assert_same_bytes(before: &StartSnapshot, after: &StartSnapshot) {
    assert_eq!(
        after.instructions().text.as_bytes(),
        before.instructions().text.as_bytes()
    );
    assert_eq!(
        after.instruction_blocks(),
        before.instruction_blocks(),
        "block boundaries and their exact text stay fixed"
    );
    assert_eq!(
        after.tool_blocks(),
        before.tool_blocks(),
        "all ordered definitions, descriptions and schemas stay fixed"
    );
    assert_eq!(after.orch_tools_revision(), before.orch_tools_revision());
    assert_eq!(after.segment.id, before.segment.id);
    assert_eq!(
        after.segment.native_session_ref,
        before.segment.native_session_ref
    );
    assert_eq!(after.cwd(), before.cwd());
    assert_eq!(after.directory(), before.directory());
}

#[tokio::test]
async fn approval_and_sandbox_only_changes_keep_core_and_ordered_tool_bytes_and_use_turn_controls()
-> TestResult {
    let world = world().await?;
    let thread = world.thread("Mode byte stability").await;
    world.running_turn(thread, "Baseline mode").await;
    let original = snapshot(&world, thread).await?;
    assert_eq!(
        original.tools,
        vec![OrchTool::ListShared, OrchTool::ReadShared]
    );
    assert!(original.instructions().text.contains("FIXED-CORE-RULE"));
    let choices = [
        (ApprovalMode::OnRequest, SandboxMode::Workspace),
        (ApprovalMode::OnRequest, SandboxMode::ReadOnly),
        (ApprovalMode::Ask, SandboxMode::ReadOnly),
        (ApprovalMode::Ask, SandboxMode::Workspace),
    ];
    for (index, (approval, sandbox)) in choices.into_iter().enumerate() {
        world.run_turn(thread, index, "Done").await;
        world
            .set(world.topic, "approval.mode", json!(approval))
            .await;
        world.set(world.topic, "sandbox.mode", json!(sandbox)).await;
        let (sent, _) = world.running_turn(thread, "Only the mode changes").await;
        let current = snapshot(&world, thread).await?;
        assert_eq!(current.approval, approval);
        assert_eq!(current.sandbox, sandbox);
        assert_eq!(serde_json::to_value(sent.approval_mode)?, json!(approval));
        assert_eq!(serde_json::to_value(sent.sandbox_mode)?, json!(sandbox));
        assert_same_bytes(&original, &current);
        assert_eq!(
            world.commands.issued_of("session.open"),
            1,
            "admitted mode controls alone do not reconfigure a stable session"
        );
        assert_eq!(world.commands.issued_of("session.mcp.reload"), 0);
        assert_eq!(
            world.commands.issued_of("fs.materialize"),
            0,
            "a mode change does not rewrite workspace guidance"
        );
        assert_eq!(world.commands.issued_of("turn.interrupt"), 0);
        assert_eq!(
            world.turn_starts(thread).len(),
            index + 2,
            "there is one real input for each accepted turn, without a hidden refresh turn"
        );
    }
    world.run_turn(thread, choices.len(), "Done").await;
    Ok(())
}

#[tokio::test]
async fn reordering_enabled_names_keeps_bytes_but_a_real_tool_membership_change_reconfigures_once()
-> TestResult {
    let world = world().await?;
    let thread = world.thread("Catalogue byte stability").await;
    world.running_turn(thread, "Original tools").await;
    let original = snapshot(&world, thread).await?;
    assert_eq!(
        original.tools,
        vec![OrchTool::ListShared, OrchTool::ReadShared]
    );
    world.run_turn(thread, 0, "Done").await;
    world
        .set(
            world.topic,
            "orch_tools.enabled",
            json!(["read_shared", "list_shared"]),
        )
        .await;
    world
        .running_turn(thread, "Same tools in another setting order")
        .await;
    assert_same_bytes(&original, &snapshot(&world, thread).await?);
    assert_eq!(world.commands.issued_of("session.open"), 1);
    world.run_turn(thread, 1, "Done").await;
    world
        .set(world.topic, "orch_tools.enabled", json!(["list_shared"]))
        .await;
    world
        .running_turn(thread, "One tool deliberately removed")
        .await;
    let changed = snapshot(&world, thread).await?;
    assert_eq!(changed.tools, vec![OrchTool::ListShared]);
    assert_ne!(changed.tool_blocks(), original.tool_blocks());
    assert_ne!(
        changed.orch_tools_revision(),
        original.orch_tools_revision()
    );
    assert_ne!(
        changed.instructions().text.as_bytes(),
        original.instructions().text.as_bytes(),
        "the offered guide legitimately loses its read clause"
    );
    assert_eq!(
        changed.segment.native_session_ref,
        original.segment.native_session_ref
    );
    let openings = world.seen.of_kind(CommandKind::SessionOpen);
    assert_eq!(openings.len(), 2);
    let Command::SessionOpen(reopened) = &openings[1] else {
        panic!("the second opening must be a session opening")
    };
    assert_eq!(reopened.mode, link::SessionMode::Resume);
    assert_eq!(
        reopened.native_session_ref,
        original.segment.native_session_ref
    );
    world.run_turn(thread, 2, "Done").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Same smaller catalogue"),
        )
        .await?;
    world.run_turn(thread, 3, "Done").await;
    assert_eq!(world.commands.issued_of("session.open"), 2);
    assert_eq!(world.commands.issued_of("session.mcp.reload"), 0);
    Ok(())
}
