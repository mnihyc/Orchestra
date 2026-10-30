//! Enabled MCP definitions reach sessions once, reload between turns, and honor work grants and current machine policy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
#[path = "mcp_reconfiguration/proof.rs"]
mod proof;
#[path = "mcp_reconfiguration/revisions.rs"]
mod revision_cases;

use common::{World, queued};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_authz::{
    Caller,
    ownership::{NewAccess, start_resource},
};
use orch_db::queries::resources::mcp::{self as rows, McpRow};
use orch_link_contract::commands::{
    AgentStartResult, Command, CommandKind, CommandResult, McpStartResult,
};
use orch_types::{Id, Time};
use proof::{restart_ends_within, session_openings};
use serde_json::json;

async fn preset(world: &World, owner: Id) -> Id {
    preset_named(world, owner, "team-tool").await
}

async fn preset_named(world: &World, owner: Id, name: &str) -> Id {
    let id = orch_types::new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    rows::insert(
        &mut conn,
        &McpRow {
            imported: false,
            id,
            name: name.into(),
            description: String::new(),
            org_id: Some(world.root),
            home_id: Some(world.root),
            owner_id: Some(owner),
            machine_id: None,
            environment_id: None,
            spec: json!({"transport":"stdio","command":["team-tool"]}),
            enabled: true,
            revision: 1,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::McpServer.into(),
            id,
        },
        owner,
        NewAccess::OnlyMe,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let stored = rows::servers(&mut conn, &[id]).await.unwrap();
    assert_eq!(
        stored[0].owner_id,
        Some(owner),
        "the fixture's named owner must be an active member, rather than transferring the server to the setting's writer"
    );
    id
}

fn play_mcp(world: &World) {
    world.machine.handler.on(CommandKind::McpStart, |command| {
        assert!(matches!(command, Command::McpStart(_)));
        Ok(CommandResult::McpStart(McpStartResult {
            service: "mcp:team-tool".into(),
        }))
    });
}

#[tokio::test]
async fn the_harness_gets_the_selected_servers_and_only_a_changed_set_reopens_it() {
    let world = World::new().await;
    play_mcp(&world);
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["mcp"] =
        json!({"stdio":true,"http":true,"reload":false,"strict":true});
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
    let server = preset(&world, world.ada).await;
    world
        .set(world.root, "mcp.enabled", json!({server.to_string():true}))
        .await;
    let thread = world.thread("Tools").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("First"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "Done").await;
    let opened = session_openings(&world);
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0]["mcp"][0]["name"], "team-tool");
    assert_eq!(world.commands.issued_of("mcp.start"), 1);
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Same tools"))
        .await
        .unwrap();
    world.run_turn(thread, 1, "Done").await;
    assert_eq!(
        session_openings(&world).len(),
        1,
        "unchanged tools do not reopen a session"
    );
    world
        .set(world.root, "mcp.enabled", json!({server.to_string():false}))
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Without the tool"),
        )
        .await
        .unwrap();
    world.run_turn(thread, 2, "Done").await;
    let opened = session_openings(&world);
    assert_eq!(opened.len(), 2);
    assert!(
        opened[1]["mcp"].as_array().is_some_and(Vec::is_empty),
        "the resumed session receives the disabled set"
    );
    assert_eq!(opened[1]["mode"], "resume");
    assert_eq!(opened[1]["segment_id"], opened[0]["segment_id"]);
    assert_eq!(
        opened[1]["native_session_ref"],
        json!(format!(
            "native-{}",
            opened[0]["segment_id"].as_str().unwrap()
        ))
    );
    assert_eq!(
        opened[1]["instructions"], opened[0]["instructions"],
        "external membership does not rewrite the core text"
    );
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Still without it"),
        )
        .await
        .unwrap();
    world.run_turn(thread, 3, "Done").await;
    assert_eq!(
        world.commands.issued_of("session.open"),
        2,
        "the accepted next-turn reconfiguration is not repeated"
    );
    assert_eq!(
        world.commands.issued_of("session.mcp.reload"),
        0,
        "an adapter which declares no reload is never asked to reload"
    );
    assert_eq!(
        world.commands.issued_of("mcp.start"),
        1,
        "turn boundaries do not restart the shared tool"
    );
}

#[tokio::test]
async fn a_grant_to_work_in_the_setting_scope_is_usable_and_revocation_explains_the_missing_tool() {
    let world = World::new().await;
    let owner = world.person("tool-owner").await;
    let server = preset(&world, owner).await;
    let grant = {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("mcp_server", server, "live"),
            "any",
            "mcp.use",
            ("node", Some(world.root), None),
            None,
            world.clock.now(),
        )
        .await
        .unwrap()
    };
    world
        .set(world.root, "mcp.enabled", json!({server.to_string():true}))
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    assert!(
        matches!(
            orch_authz::usable::usable(
                &mut conn,
                &principal,
                server,
                None,
                &orch_authz::usable::Policies::default()
            )
            .await
            .unwrap(),
            orch_authz::usable::Usable::No
        ),
        "the setting's writer has no personal Use or ownership to mask the work grant"
    );
    let granted = orch_authz::usable::usable(
        &mut conn,
        &principal,
        server,
        Some(world.root),
        &orch_authz::usable::Policies::default(),
    )
    .await
    .unwrap();
    assert!(
        granted.granted().is_some_and(|allowed| allowed
            .work
            .iter()
            .any(|work| work.entry == grant && work.place == world.root)),
        "the exact grant to work at the setting scope supplies Use"
    );
    let resolved =
        orch_engine::mcp::resolve(&mut conn, world.environment, Time::new(world.clock.now()))
            .await
            .unwrap();
    assert!(
        resolved[0].reason.is_none(),
        "a work grant is not mistaken for missing personal Use"
    );
    assert!(
        orch_db::queries::access::remove_entry(&mut conn, grant, owner, world.clock.now())
            .await
            .unwrap()
    );
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    let refused =
        orch_engine::mcp::resolve(&mut conn, world.environment, Time::new(world.clock.now()))
            .await
            .unwrap();
    assert!(
        refused[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("may no longer use")),
        "revoking the work grant must refuse that same server: {refused:?}"
    );
}

#[tokio::test]
async fn a_manager_restart_refreshes_live_tools_without_another_server_session_input_or_turn_being_touched()
 {
    use orch_link_contract::commands::{Empty, McpRestartResult};
    let world = World::new().await;
    world.machine.handler.on(CommandKind::McpStart, |command| {
        let Command::McpStart(body) = command else {
            unreachable!()
        };
        let name = match body {
            orch_link_contract::commands::McpStart::Given(body) => {
                assert!(
                    body.secrets.is_empty(),
                    "an old authentication secret is no longer declared or sent"
                );
                &body.name
            }
            orch_link_contract::commands::McpStart::Local(body) => &body.name,
        };
        Ok(CommandResult::McpStart(McpStartResult {
            service: format!("mcp:{name}"),
        }))
    });
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["mcp"] =
        json!({"stdio":true,"http":true,"reload":true,"strict":true});
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
    let flaky = preset(&world, world.ada).await;
    let steady = preset_named(&world, world.ada, "steady").await;
    {
        let mut conn = world.pool.acquire().await.unwrap();
        // Retained encrypted data from authentication that was turned off must not even be requested from the link.
        orch_db::queries::settings::put_secret(
            &mut conn,
            "mcp_server",
            flaky,
            "OLD_TOKEN",
            b"unused-encrypted-data",
            Some(world.ada),
            world.clock.now(),
        )
        .await
        .unwrap();
    }
    world
        .set(
            world.root,
            "mcp.enabled",
            json!({flaky.to_string():true,steady.to_string():true}),
        )
        .await;
    let thread = world.thread("Uninterrupted work").await;
    let (turn, _) = world.running_turn(thread, "Keep working").await;
    let input_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let starts = world.commands.issued_of("mcp.start");
    let openings = world.commands.issued_of("session.open");
    sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{mcp_servers}',$2) WHERE id=$1")
        .bind(world.environment)
        .bind(json!([
            {"name":"team-tool","state":"failed","reason":"exit status 7: [masked]","revision":3},
            {"name":"steady","state":"running","reason":null,"revision":1}
        ]))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .machine
        .handler
        .on(CommandKind::McpRestart, |command| {
            let Command::McpRestart(body) = command else {
                unreachable!()
            };
            assert_eq!(serde_json::to_value(body).unwrap()["name"], "team-tool");
            assert_eq!(
                serde_json::to_value(body).unwrap()["secrets"],
                json!({}),
                "Restart does not forward the credential after authentication was removed"
            );
            Ok(CommandResult::McpRestart(McpRestartResult {
                service: "mcp:team-tool".into(),
                servers: serde_json::from_value(json!([
                    {"name":"team-tool","state":"running","reason":null,"revision":4},
                    {"name":"steady","state":"running","reason":null,"revision":1}
                ]))
                .unwrap(),
            }))
        });
    world
        .machine
        .handler
        .on(CommandKind::SessionMcpReload, move |command| {
            let Command::SessionMcpReload(body) = command else {
                unreachable!()
            };
            assert_eq!(body.thread_id, thread);
            assert_eq!(body.mcp.len(), 2);
            Ok(CommandResult::SessionMcpReload(Empty {}))
        });
    let mut tx = world.pool.begin().await.unwrap();
    let job = world
        .engine()
        .queue_mcp_restart(&mut tx, world.environment, "team-tool", world.ada)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    restart_ends_within(
        &world,
        job.id,
        "succeeded",
        std::time::Duration::from_secs(20),
    )
    .await;
    assert_eq!(world.commands.issued_of("mcp.restart"), 1);
    assert_eq!(
        world.commands.issued_of("mcp.start"),
        starts,
        "a restart refresh never starts another server"
    );
    assert_eq!(world.commands.issued_of("session.mcp.reload"), 1);
    assert_eq!(world.commands.issued_of("session.open"), openings);
    assert_eq!(world.commands.issued_of("turn.interrupt"), 0);
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(after, input_count);
    let actions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='mcp.restarted' AND environment_id=$1",
    )
    .bind(world.environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(actions, 1);
    let native: Option<String> = sqlx::query_scalar("SELECT native_session_ref FROM segments WHERE id=(SELECT current_segment_id FROM threads WHERE node_id=$1)")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    world
        .run_turn(thread, 0, "Finished with refreshed tools")
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Use the unchanged refreshed tools"),
        )
        .await
        .unwrap();
    world.run_turn(thread, 1, "Done").await;
    assert_eq!(
        world.commands.issued_of("session.open"),
        openings,
        "an accepted live reload cannot replace the comparison slot with a different hash domain"
    );
    assert_eq!(world.commands.issued_of("session.mcp.reload"), 1);
    assert_eq!(world.commands.issued_of("mcp.start"), starts);
    let still_native: Option<String> = sqlx::query_scalar("SELECT native_session_ref FROM segments WHERE id=(SELECT current_segment_id FROM threads WHERE node_id=$1)")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(
        still_native, native,
        "the same conversation continues after the live refresh"
    );
    world.engine().stop();
}

#[tokio::test]
async fn imports_on_another_machine_and_an_own_server_after_policy_revocation_are_unavailable() {
    let world = World::new().await;
    let server = preset(&world, world.ada).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let other =
        orch_db::queries::resources::testing::insert_machine(&mut conn, "other-tools", "connected")
            .await
            .unwrap();
    sqlx::query("UPDATE mcp_servers SET imported=true,machine_id=$2,spec=$3 WHERE id=$1")
        .bind(server)
        .bind(other)
        .bind(json!({"transport":"stdio","imported":{}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    rows::offer(
        &mut conn,
        server,
        other,
        None,
        &json!({}),
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    world
        .set(world.root, "mcp.enabled", json!({server.to_string():true}))
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let unavailable =
        orch_engine::mcp::resolve(&mut conn, world.environment, Time::new(world.clock.now()))
            .await
            .unwrap();
    assert!(
        unavailable[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("another machine")
    );
    sqlx::query("UPDATE mcp_servers SET imported=false,machine_id=NULL,environment_id=$2,spec=$3 WHERE id=$1")
        .bind(server)
        .bind(world.environment)
        .bind(json!({"transport":"stdio","command":["team-tool"]}))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET placement='connected' WHERE id=$1")
        .bind(world.environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    let revoked =
        orch_engine::mcp::resolve(&mut conn, world.environment, Time::new(world.clock.now()))
            .await
            .unwrap();
    assert!(
        revoked[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("machine permits")
    );
}
