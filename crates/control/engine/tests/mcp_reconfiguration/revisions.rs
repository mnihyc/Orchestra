//! The native refresh is held before its acknowledgment, so concurrent changes cannot be hidden by that acknowledgment.

use super::proof::{reload_is_held, restart_ends};
use super::*;
use orch_link_contract::commands::{CommandError, Empty, ErrorCode, McpRestartResult};

async fn configured(reload: bool) -> World {
    let world = World::new().await;
    play_mcp(&world);
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["orch_tools"] = json!("mcp");
    descriptor["capabilities"]["mcp"] =
        json!({"stdio":true,"http":true,"reload":reload,"strict":true});
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(world.agent)
        .bind(&descriptor)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: false,
            harness_version: "1.0.0".into(),
            descriptor: serde_json::from_value(descriptor.clone()).unwrap(),
        }))
    });
    world.machine.handler.on(CommandKind::McpRestart, |_| {
        Ok(CommandResult::McpRestart(McpRestartResult {
            service: "mcp:team-tool".into(),
            servers: serde_json::from_value(json!([
                {"name":"team-tool","state":"running","reason":null,"revision":2}
            ]))
            .unwrap(),
        }))
    });
    world
        .set(
            world.topic,
            "orch_tools.enabled",
            json!(["read_shared", "announce"]),
        )
        .await;
    world
}

async fn queue_restart(world: &World) -> Id {
    let mut tx = world.pool.begin().await.unwrap();
    let job = world
        .engine()
        .queue_mcp_restart(&mut tx, world.environment, "team-tool", world.ada)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    job.id
}

#[tokio::test]
async fn an_external_reload_cannot_acknowledge_a_concurrently_changed_own_description_or_schema() {
    for schema_change in [false, true] {
        // This scripted protocol peer explicitly accepts live reload; no built-in adapter is made to claim it.
        let world = configured(true).await;
        let server = preset(&world, world.ada).await;
        world
            .set(world.root, "mcp.enabled", json!({server.to_string():true}))
            .await;
        let thread = world.thread("Concurrent catalogue change").await;
        let (turn, _) = world.running_turn(thread, "The original catalogue").await;
        let original = session_openings(&world)[0].clone();
        world
            .machine
            .handler
            .on(CommandKind::SessionMcpReload, |_| {
                Ok(CommandResult::SessionMcpReload(Empty {}))
            });
        world.machine.handler.hold(CommandKind::SessionMcpReload);
        let job = queue_restart(&world).await;
        reload_is_held(&world, job).await;
        if schema_change {
            world
                .set(world.topic, "broadcast.agent_scope", json!("project"))
                .await;
        } else {
            world
                .set(
                    world.topic,
                    "agent.messages",
                    json!({"tool_read_shared":"Read the current shared text without changing it."}),
                )
                .await;
        }
        world.machine.handler.release();
        restart_ends(&world, job, "succeeded").await;
        assert_eq!(world.commands.issued_of("mcp.restart"), 1);
        assert_eq!(world.turn_state(turn.turn_id).await, "running");
        assert_eq!(world.commands.issued_of("session.open"), 1);
        assert_eq!(world.commands.issued_of("turn.interrupt"), 0);
        world.run_turn(thread, 0, "The current turn finishes").await;
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                queued("Use the new catalogue"),
            )
            .await
            .unwrap();
        world.run_turn(thread, 1, "Done").await;
        let openings = session_openings(&world);
        assert_eq!(
            openings.len(),
            2,
            "the external acknowledgment does not claim to apply own-tool changes"
        );
        let changed = &openings[1];
        assert_eq!(changed["mode"], "resume");
        assert_eq!(changed["segment_id"], original["segment_id"]);
        assert_eq!(
            changed["native_session_ref"],
            json!(format!(
                "native-{}",
                original["segment_id"].as_str().unwrap()
            ))
        );
        assert_eq!(
            changed["instructions"], original["instructions"],
            "only catalogue bytes changed"
        );
        if schema_change {
            assert_eq!(
                changed["orch_tool_schemas"]["announce"]["properties"]["audience"]["enum"],
                json!(["followers", "topic", "project"])
            );
        } else {
            assert_eq!(
                changed["orch_tool_descriptions"]["read_shared"],
                "Read the current shared text without changing it."
            );
        }
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                queued("Same accepted catalogue"),
            )
            .await
            .unwrap();
        world.run_turn(thread, 2, "Done").await;
        assert_eq!(world.commands.issued_of("session.open"), 2);
        assert_eq!(world.commands.issued_of("session.mcp.reload"), 1);
    }
}

#[tokio::test]
async fn an_adapter_without_live_reload_takes_a_manager_restart_at_the_next_turn_in_the_same_conversation()
 {
    let world = configured(false).await;
    let server = preset(&world, world.ada).await;
    world
        .set(world.root, "mcp.enabled", json!({server.to_string():true}))
        .await;
    let thread = world.thread("Deferred native refresh").await;
    let (turn, _) = world.running_turn(thread, "Keep this conversation").await;
    let original = session_openings(&world)[0].clone();
    let job = queue_restart(&world).await;
    restart_ends(&world, job, "succeeded").await;
    assert_eq!(world.commands.issued_of("mcp.restart"), 1);
    assert_eq!(world.commands.issued_of("session.mcp.reload"), 0);
    assert_eq!(world.commands.issued_of("session.open"), 1);
    assert_eq!(world.commands.issued_of("turn.interrupt"), 0);
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    world.run_turn(thread, 0, "Finished").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Read refreshed definitions"),
        )
        .await
        .unwrap();
    world.run_turn(thread, 1, "Done").await;
    let openings = session_openings(&world);
    assert_eq!(openings.len(), 2);
    assert_eq!(openings[1]["mode"], "resume");
    assert_eq!(openings[1]["segment_id"], original["segment_id"]);
    assert_eq!(
        openings[1]["native_session_ref"],
        json!(format!(
            "native-{}",
            original["segment_id"].as_str().unwrap()
        ))
    );
    assert_eq!(openings[1]["instructions"], original["instructions"]);
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Same definitions"),
        )
        .await
        .unwrap();
    world.run_turn(thread, 2, "Done").await;
    assert_eq!(world.commands.issued_of("session.open"), 2);
    assert_eq!(world.commands.issued_of("session.mcp.reload"), 0);
}

#[tokio::test]
async fn revoking_external_authority_while_reload_is_held_still_removes_that_server_before_the_next_turn()
 {
    let world = configured(true).await;
    let owner = world.person("external-tool-owner").await;
    let server = preset(&world, owner).await;
    let grant = orch_db::queries::access::testing::add_entry(
        &mut world.pool.acquire().await.unwrap(),
        &orch_db::queries::access::PageKey::new("mcp_server", server, "live"),
        "any",
        "mcp.use",
        ("node", Some(world.root), None),
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    world
        .set(world.root, "mcp.enabled", json!({server.to_string():true}))
        .await;
    let thread = world.thread("Current external authority").await;
    let (turn, _) = world.running_turn(thread, "Initial access").await;
    assert_eq!(session_openings(&world)[0]["mcp"][0]["name"], "team-tool");
    world
        .machine
        .handler
        .on(CommandKind::SessionMcpReload, |_| {
            Ok(CommandResult::SessionMcpReload(Empty {}))
        });
    world.machine.handler.hold(CommandKind::SessionMcpReload);
    let job = queue_restart(&world).await;
    reload_is_held(&world, job).await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_db::queries::access::remove_entry(&mut conn, grant, owner, world.clock.now())
            .await
            .unwrap()
    );
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    world.machine.handler.release();
    restart_ends(&world, job, "succeeded").await;
    assert_eq!(world.commands.issued_of("mcp.restart"), 1);
    assert_eq!(world.commands.issued_of("session.mcp.reload"), 1);
    assert_eq!(world.commands.issued_of("session.open"), 1);
    assert_eq!(world.commands.issued_of("turn.interrupt"), 0);
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    world.run_turn(thread, 0, "Finished").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Current access only"),
        )
        .await
        .unwrap();
    world.run_turn(thread, 1, "Done").await;
    let openings = session_openings(&world);
    assert_eq!(openings.len(), 2);
    assert_eq!(openings[1]["mode"], "resume");
    assert_eq!(openings[1]["mcp"], json!([]));
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_engine::mcp::resolve(&mut conn, world.environment, Time::new(world.clock.now()))
            .await
            .unwrap()[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("may no longer use")
    );
}

#[tokio::test]
async fn a_refused_live_reload_does_not_acknowledge_the_external_revision() {
    let world = configured(true).await;
    let server = preset(&world, world.ada).await;
    world
        .set(world.root, "mcp.enabled", json!({server.to_string():true}))
        .await;
    let thread = world.thread("Refused refresh").await;
    let (turn, _) = world.running_turn(thread, "Keep the accepted tools").await;
    let original = session_openings(&world)[0].clone();
    world
        .machine
        .handler
        .on(CommandKind::SessionMcpReload, |_| {
            Err(CommandError::new(
                ErrorCode::Unsupported,
                "the controlled peer refused the refresh",
            ))
        });
    let job = queue_restart(&world).await;
    restart_ends(&world, job, "failed").await;
    assert_eq!(world.commands.issued_of("mcp.restart"), 1);
    assert_eq!(
        world.commands.issued_of("session.mcp.reload"),
        1,
        "the failed job actually reached the native refresh refusal"
    );
    assert_eq!(world.commands.issued_of("session.open"), 1);
    assert_eq!(world.commands.issued_of("turn.interrupt"), 0);
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    world.run_turn(thread, 0, "Finished").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Reconfigure at the boundary"),
        )
        .await
        .unwrap();
    world.run_turn(thread, 1, "Done").await;
    let openings = session_openings(&world);
    assert_eq!(
        openings.len(),
        2,
        "a refused native refresh is not a successful cache update"
    );
    assert_eq!(openings[1]["mode"], "resume");
    assert_eq!(openings[1]["segment_id"], original["segment_id"]);
    assert_eq!(
        openings[1]["native_session_ref"],
        json!(format!(
            "native-{}",
            original["segment_id"].as_str().unwrap()
        ))
    );
    assert_eq!(openings[1]["instructions"], original["instructions"]);
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Accepted now"))
        .await
        .unwrap();
    world.run_turn(thread, 2, "Done").await;
    assert_eq!(world.commands.issued_of("session.open"), 2);
}
