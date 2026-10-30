//! A local import crosses real authenticated links, files the copy once as the current owner, and prints refusals.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World};
use orch_api_contract::environment::McpScope;
use orch_db::queries::resources::access_fixtures;
use orch_link_contract::commands::{CommandKind, CommandResult, McpImportResult};
use orch_link_contract::enrollment::Trust;
use orch_link_contract::mcp_import::McpImportAsk;
use orch_link_contract::session::Role;
use orch_types::new_id;
use serde_json::json;

#[tokio::test]
async fn a_local_import_is_filed_as_the_machine_owner_and_replay_makes_no_second_copy_or_audit() {
    orch_telemetry::init_logging("gateway-test", orch_telemetry::LogFormat::Text);
    let mut world = World::start().await;
    let enrolled = world.enroll("tool-machine").await;
    let machine = world.start_machine_as(
        &enrolled,
        enrolled.endpoints(Trust::Pinned),
        vec![Role::Environment],
    );
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let (environment, ownership, org) = {
        let mut conn = world.db.acquire().await.unwrap();
        let org = new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            org,
            "org",
            None,
            "Tool team",
            world.user,
        )
        .await
        .unwrap();
        // An org made through Orch has its maker for a member, and an owner counts only while a
        // member of the root org: the imported copy's first owner is the machine's owner.
        orch_db::queries::access::testing::insert_member(&mut conn, org, world.user, org)
            .await
            .unwrap();
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("node", org, "live"),
            "any",
            "access.own",
            ("user", Some(world.user), None),
            None,
            world.clock.now(),
        )
        .await
        .unwrap();
        let environment = access_fixtures::insert_environment(
            &mut conn,
            world.user,
            Some(org),
            "Work",
            "connected",
        )
        .await
        .unwrap();
        let ownership = orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("environment", environment, "live"),
            "any",
            "access.own",
            ("user", Some(world.user), None),
            None,
            world.clock.now(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE environments SET machine_id=$2,workspace_root='/work',facts=$3 WHERE id=$1")
            .bind(environment).bind(machine.id).bind(json!({"mcp_found":[{"source":"fake","scope":"user","name":"files","directory":null,"transport":"stdio","digest":"copied"},{"source":"fake","scope":"user","name":"notes","directory":null,"transport":"stdio","digest":"copied"}]}))
            .execute(&mut *conn).await.unwrap();
        (environment, ownership, org)
    };
    // Named double for the harness's local file copy. Transport, gateway, API, owner checks,
    // command journal, resource page and audit are the actual implementations.
    machine.handler.on(CommandKind::McpImport, |_| {
        Ok(CommandResult::McpImport(McpImportResult {
            definition_fingerprint: None,
            transport: "stdio".to_owned(),
            digest: "copied".to_owned(),
            tools: Some(2),
        }))
    });
    let request = McpImportAsk {
        request_id: new_id(),
        environment_id: environment,
        source: "fake".to_owned(),
        scope: McpScope::User,
        directory: None,
        name: "files".to_owned(),
    };
    let answer = tokio::time::timeout(PATIENCE, machine.link.import_mcp(request.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.code, None, "{}", answer.detail);
    assert_eq!(answer.server_id, Some(request.request_id));
    let replay = machine.link.import_mcp(request.clone()).await.unwrap();
    assert_eq!(replay.server_id, answer.server_id);
    let rows = orch_db::queries::resources::mcp::servers(
        &mut world.db.acquire().await.unwrap(),
        &[request.request_id],
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].owner_id, Some(world.user));
    assert_eq!(rows[0].machine_id, Some(machine.id));
    assert_eq!(rows[0].spec["imported"]["by"], world.user.to_string());
    assert_eq!(
        machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == CommandKind::McpImport)
            .count(),
        1
    );
    let audit:Vec<(Option<orch_types::Id>,String,orch_types::Json)>=sqlx::query_as(
        "SELECT actor_id,summary,details FROM audit_log WHERE object_id=$1 AND action='mcp.imported'")
        .bind(request.request_id).fetch_all(world.db.pool()).await.unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].0, Some(world.user));
    assert!(audit[0].1.contains("command line"));
    assert_eq!(audit[0].2["via"], "command_line");

    let mut foreign = request.clone();
    foreign.request_id = new_id();
    foreign.environment_id = new_id();
    let refused = machine.link.import_mcp(foreign).await.unwrap();
    assert!(refused.server_id.is_none());
    assert_eq!(refused.code.as_deref(), Some("authz.denied"));
    assert!(refused.detail.contains("not registered on this machine"));
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(machine.id)
        .bind(
            serde_json::to_value(orch_link_contract::policy::LocalPolicy {
                mcp_import: false,
                ..Default::default()
            })
            .unwrap(),
        )
        .execute(world.db.pool())
        .await
        .unwrap();
    let mut later = request.clone();
    later.request_id = new_id();
    let refused = machine.link.import_mcp(later).await.unwrap();
    assert_eq!(refused.code.as_deref(), Some("policy.mcp_import_disabled"));
    assert!(refused.server_id.is_none());
    assert_eq!(
        machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == CommandKind::McpImport)
            .count(),
        1
    );
    // The link certificate stays the same through an ownership transfer. The next request must
    // use the new owner, never the person who owned the machine when that session began.
    let new_owner = {
        let mut conn = world.db.acquire().await.unwrap();
        let owner = orch_db::queries::resources::testing::insert_user(&mut conn, "later-owner")
            .await
            .unwrap();
        // A thing is passed on to an active member of its root org.
        orch_db::queries::access::testing::insert_member(&mut conn, org, owner, org)
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET owner_id=$2,facts='{}' WHERE id=$1")
            .bind(machine.id)
            .bind(owner)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE environments SET owner_id=$2 WHERE id=$1")
            .bind(environment)
            .bind(owner)
            .execute(&mut *conn)
            .await
            .unwrap();
        // A transfer changes Owners too; owner_id alone is attribution, never an authority shortcut.
        sqlx::query("UPDATE permission_entries SET holder_id=$2 WHERE id=$1")
            .bind(ownership)
            .bind(owner)
            .execute(&mut *conn)
            .await
            .unwrap();
        owner
    };
    let mut next = request.clone();
    next.request_id = new_id();
    // Another server found there: "files" is already imported on this machine under that name, and a
    // second copy of it would take a name that is taken (its page offers Import again instead).
    next.name = "notes".to_owned();
    let answer = machine.link.import_mcp(next.clone()).await.unwrap();
    assert_eq!(answer.code, None, "{}", answer.detail);
    let owner: orch_types::Id = sqlx::query_scalar("SELECT owner_id FROM mcp_servers WHERE id=$1")
        .bind(next.request_id)
        .fetch_one(world.db.pool())
        .await
        .unwrap();
    assert_eq!(owner, new_owner);
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(new_owner)
        .execute(world.db.pool())
        .await
        .unwrap();
    let mut later = request;
    later.request_id = new_id();
    let refused = machine.link.import_mcp(later).await.unwrap();
    assert_eq!(refused.code.as_deref(), Some("auth.suspended"));
    assert!(refused.server_id.is_none());
    machine.link.shutdown().await;
    world.stop().await;
}
