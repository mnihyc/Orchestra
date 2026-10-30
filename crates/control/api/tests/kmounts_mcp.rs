//! MCP resources and environment servers preserve revisions, local authority and secret-only storage.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::{ErrorCode, Listed};
use orch_api_contract::environment::*;
use orch_api_contract::resources::*;
use orch_api_core::routes::resources::sealing::{binding, install_master_key};
use orch_db::secrets::MasterKey;
use orch_link_contract::policy::LocalPolicy;
use orch_types::{Id, Json};
use resources::{Company, answer, company, name_person};
use serde_json::json;
use support::World;

fn spec() -> Json {
    json!({"transport":"stdio","command":["tool-server"],"secret_env":[]})
}

fn change(revision: i64) -> McpServerUpdateBody {
    McpServerUpdateBody {
        name: None,
        description: Some("Changed".into()),
        spec: None,
        secrets: None,
        enabled: None,
        expected_revision: revision,
    }
}

#[tokio::test]
async fn enabling_a_disabled_server_records_one_safe_factual_line_and_repeating_true_does_not() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = world
        .app
        .call(McpServerCreate, &[])
        .token(&c.viewer.token)
        .body(&McpServerCreateBody {
            name: "enable-proof".into(),
            description: None,
            home_id: Some(c.acme),
            spec: spec(),
            secrets: None,
            access: Some(NewResourceAccess::OnlyMe),
        })
        .send()
        .await
        .ok();
    let mut body = change(made.revision);
    assert_eq!(made.enabled_places, Some(0));
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "mcp.enabled",
            json!({(made.id.to_string()): true}),
        )
        .await;
    let counted = world
        .app
        .call(McpServerGet, &[&made.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(counted.enabled_places, Some(1));
    world
        .set_at(
            orch_settings::Target::Node(c.stranger.person.personal_org),
            "mcp.enabled",
            json!({(made.id.to_string()): true}),
        )
        .await;
    let counted = world
        .app
        .call(McpServerGet, &[&made.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(counted.enabled_places, Some(2));
    sqlx::query("UPDATE nodes SET deleted_at=created_at WHERE id=$1")
        .bind(c.stranger.person.personal_org)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let counted = world
        .app
        .call(McpServerGet, &[&made.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        counted.enabled_places,
        Some(1),
        "removed places no longer switch a server on"
    );
    body.enabled = Some(false);
    let disabled = world
        .app
        .call(McpServerUpdate, &[&made.id])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(disabled.enabled_places, Some(0));
    body.expected_revision = disabled.revision;
    body.enabled = Some(true);
    let enabled = world
        .app
        .call(McpServerUpdate, &[&made.id])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(enabled.enabled);
    body.expected_revision = enabled.revision;
    world
        .app
        .call(McpServerUpdate, &[&made.id])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    let records:Vec<(Option<Id>,String,Json)> = sqlx::query_as("SELECT actor_id,summary,details FROM audit_log WHERE action='mcp.enabled' AND object_id=$1")
        .bind(made.id).fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, Some(c.viewer.id()));
    assert!(records[0].1.contains("Enabled"));
    assert_eq!(records[0].2["name"], "enable-proof");
    assert!(
        !records[0].2.to_string().contains("tool-server"),
        "definitions remain on the protected page"
    );
}

async fn environment(world: &World, c: &Company, add: bool) -> (Id, Id) {
    let mut conn = world.conn().await;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn,
        c.admin.id(),
        "tools-machine",
    )
    .await
    .unwrap();
    // The enrollment fixture already starts its actual Owners and private Use rows.
    let owners: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries WHERE object_kind='machine'
         AND object_id=$1 AND permission='access.own' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(machine)
    .bind(c.admin.id())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(owners, 1);
    let policy = LocalPolicy {
        environment_roots: vec!["/srv/mcp".into()],
        exec_offers: vec!["plain".into()],
        mcp_add: add,
        ..LocalPolicy::default()
    };
    sqlx::query(
        "UPDATE machines SET online=true,facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1",
    )
    .bind(machine)
    .bind(serde_json::to_value(policy).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    let env = world
        .app
        .call(MachineEnvironmentCreate, &[&machine])
        .token(&c.admin.token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: c.acme,
            name: "Tools".into(),
            root: "/srv/mcp/work".into(),
            home_node_id: None,
            remote_write: None,
            terminal: None,
            access: None,
        })
        .send()
        .await
        .ok();
    (env.base.id, machine)
}

#[tokio::test]
async fn a_remote_presets_authentication_is_sealed_rotated_and_never_read_back() {
    install_master_key(MasterKey::new([7; 32]));
    let world = World::new().await;
    let c = company(&world).await;
    let definition = json!({"transport":"http","url":"https://tools.example.test/mcp",
        "authentication":{"kind":"bearer","secret":"MCP_BEARER_TOKEN"},"secret_env":["MCP_BEARER_TOKEN"],
        "tools_allow":["search"],"working_dir":"home"});
    let body = McpServerCreateBody {
        name: "remote-tools".into(),
        description: None,
        home_id: Some(c.acme),
        spec: definition,
        secrets: Some([("MCP_BEARER_TOKEN".into(), "initial-private-value".into())].into()),
        access: Some(NewResourceAccess::OnlyMe),
    };
    let made = world
        .app
        .call(McpServerCreate, &[])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(made.spec["authentication"]["secret"], "MCP_BEARER_TOKEN");
    assert_eq!(made.spec["tools_allow"], json!(["search"]));
    assert!(
        !serde_json::to_string(&made)
            .unwrap()
            .contains("initial-private-value")
    );
    let mut update = change(made.revision);
    update.secrets = Some([("MCP_BEARER_TOKEN".into(), "rotated-private-value".into())].into());
    let refused = world
        .app
        .call(McpServerUpdate, &[&made.id])
        .token(&c.stranger.token)
        .body(&update)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let changed = world
        .app
        .call(McpServerUpdate, &[&made.id])
        .token(&c.viewer.token)
        .body(&update)
        .send()
        .await
        .ok();
    let read = world
        .app
        .call(McpServerGet, &[&made.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        read.spec, made.spec,
        "rotating a secret preserves all public connection and tool settings"
    );
    assert!(
        !serde_json::to_string(&read)
            .unwrap()
            .contains("rotated-private-value")
    );
    // A stale retry cannot overwrite the accepted token.
    update.secrets = Some([("MCP_BEARER_TOKEN".into(), "stale-private-value".into())].into());
    assert_eq!(
        world
            .app
            .call(McpServerUpdate, &[&made.id])
            .token(&c.viewer.token)
            .body(&update)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    let sealed: Vec<u8> = sqlx::query_scalar("SELECT sealed FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 AND name='MCP_BEARER_TOKEN'")
        .bind(made.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        MasterKey::new([7; 32])
            .open(&sealed, &binding("mcp_server", made.id, "MCP_BEARER_TOKEN"))
            .unwrap(),
        b"rotated-private-value"
    );
    update.expected_revision = changed.revision;
    update.secrets = Some([("UNDECLARED".into(), "wrong-value".into())].into());
    assert_eq!(
        world
            .app
            .call(McpServerUpdate, &[&made.id])
            .token(&c.viewer.token)
            .body(&update)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let audit: String = sqlx::query_scalar(
        "SELECT COALESCE(string_agg(details::text,','),'') FROM audit_log WHERE object_id=$1",
    )
    .bind(made.id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    for value in [
        "initial-private-value",
        "rotated-private-value",
        "stale-private-value",
        "wrong-value",
    ] {
        assert!(!audit.contains(value));
    }
}

#[tokio::test]
async fn a_preset_has_current_revision_crud_and_no_foreign_access() {
    let world = World::new().await;
    let c = company(&world).await;
    let body = McpServerCreateBody {
        name: "search".into(),
        description: None,
        home_id: Some(c.acme),
        spec: spec(),
        secrets: None,
        access: Some(NewResourceAccess::OnlyMe),
    };
    let made = world
        .app
        .call(McpServerCreate, &[])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(made.owner.as_ref().unwrap().id, c.viewer.id());
    let listed = world
        .app
        .call(McpServerList, &[])
        .token(&c.viewer.token)
        .query(&McpServerListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        listed
            .into_iter()
            .filter_map(Listed::into_visible)
            .collect::<Vec<_>>(),
        vec![made.clone()]
    );
    assert_eq!(
        world
            .app
            .call(McpServerGet, &[&made.id])
            .token(&c.viewer.token)
            .send()
            .await
            .ok(),
        made
    );
    for request in ["read", "change", "delete", "reimport"] {
        let problem = match request {
            "read" => world
                .app
                .call(McpServerGet, &[&made.id])
                .token(&c.stranger.token)
                .send()
                .await
                .problem(),
            "change" => world
                .app
                .call(McpServerUpdate, &[&made.id])
                .token(&c.stranger.token)
                .body(&change(made.revision))
                .send()
                .await
                .problem(),
            "delete" => world
                .app
                .call(McpServerDelete, &[&made.id])
                .token(&c.stranger.token)
                .send()
                .await
                .problem(),
            _ => world
                .app
                .call(McpServerReimport, &[&made.id])
                .token(&c.stranger.token)
                .send()
                .await
                .problem(),
        };
        assert_eq!(problem.code, ErrorCode::AuthzDenied, "{request}");
    }
    let changed = world
        .app
        .call(McpServerUpdate, &[&made.id])
        .token(&c.viewer.token)
        .body(&change(made.revision))
        .send()
        .await
        .ok();
    assert_eq!(changed.description, "Changed");
    assert_eq!(changed.revision, made.revision + 1);
    let stale = world
        .app
        .call(McpServerUpdate, &[&made.id])
        .token(&c.viewer.token)
        .body(&change(made.revision))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["revision"], changed.revision);
    let mut invalid = body.clone();
    invalid.name = "broken".into();
    invalid.spec =
        json!({"transport":"http","url":"https://user:private@example.test","unknown":true});
    world
        .app
        .call(McpServerCreate, &[])
        .token(&c.viewer.token)
        .body(&invalid)
        .send()
        .await
        .problem();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mcp_servers WHERE deleted_at IS NULL")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "an invalid definition writes nothing");
    world
        .app
        .call(McpServerDelete, &[&made.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        world
            .app
            .call(McpServerGet, &[&made.id])
            .token(&c.viewer.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn only_an_environment_manager_can_queue_one_server_restart_and_the_list_shows_its_masked_exit()
 {
    use orch_api_contract::environment::EnvironmentMcpServerRestart;
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = environment(&world, &c, true).await;
    let made = world
        .app
        .call(EnvironmentMcpServerCreate, &[&env])
        .token(&c.admin.token)
        .body(&EnvironmentMcpServerCreateBody {
            name: "flaky".into(),
            description: None,
            spec: spec(),
            secrets: None,
            enable: Some(true),
        })
        .send()
        .await
        .ok();
    sqlx::query("UPDATE environments SET status='running',facts=jsonb_set(facts,'{mcp_servers}',$2) WHERE id=$1").bind(env).bind(json!([{"name":"flaky","state":"failed","reason":"The server ended (exit status: 7): [masked]","revision":3}])).execute(&world.db.pool).await.unwrap();
    sqlx::query("UPDATE machines SET online=true WHERE id=$1")
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let list = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    let server = list
        .servers
        .iter()
        .find(|server| server.server.id == made.id)
        .unwrap();
    assert_eq!(server.state, McpState::Failed);
    assert!(server.reason.as_deref().unwrap().contains("exit status: 7"));
    assert!(server.reason.as_deref().unwrap().contains("[masked]"));
    assert_eq!(
        world
            .app
            .call(EnvironmentMcpServerRestart, &[&env, &"flaky"])
            .token(&c.stranger.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='mcp_restart' AND environment_id=$1",
    )
    .bind(env)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    let begun = world
        .app
        .call(EnvironmentMcpServerRestart, &[&env, &"flaky"])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(begun.kind, "mcp_restart");
    assert_eq!(begun.environment_id, Some(env));
    let name: String = sqlx::query_scalar("SELECT plan->>'name' FROM jobs WHERE id=$1")
        .bind(begun.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(name, "flaky");
    let status: String = sqlx::query_scalar("SELECT status FROM environments WHERE id=$1")
        .bind(env)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "running");
}

#[tokio::test]
async fn an_environment_server_takes_local_policy_and_never_returns_a_secret() {
    install_master_key(MasterKey::new([7; 32]));
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = environment(&world, &c, false).await;
    let body = EnvironmentMcpServerCreateBody {
        name: "private-tool".into(),
        description: None,
        spec: json!({"transport":"stdio","command":["tool-server"],"secret_env":["TOKEN"]}),
        secrets: Some([("TOKEN".into(), "value-never-returned".into())].into()),
        enable: None,
    };
    assert_eq!(
        world
            .app
            .call(EnvironmentMcpServerCreate, &[&env])
            .token(&c.admin.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::PolicyMcpAddDisabled
    );
    assert_eq!(
        world
            .app
            .call(EnvironmentMcpServerCreate, &[&env])
            .token(&c.stranger.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        world
            .app
            .call(EnvironmentMcpGet, &[&env])
            .token(&c.stranger.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,mcp.add}','true') WHERE id=$1")
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let made = world
        .app
        .call(EnvironmentMcpServerCreate, &[&env])
        .token(&c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(made.environment.as_ref().unwrap().id, env);
    assert!(
        !serde_json::to_string(&made)
            .unwrap()
            .contains("value-never-returned")
    );
    let sealed: Vec<u8> = sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 AND name='TOKEN'",
    )
    .bind(made.id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        MasterKey::new([7; 32])
            .open(&sealed, &binding("mcp_server", made.id, "TOKEN"))
            .unwrap(),
        b"value-never-returned"
    );
    let audit: String = sqlx::query_scalar(
        "SELECT COALESCE(string_agg(details::text,','),'') FROM audit_log WHERE object_id=$1",
    )
    .bind(made.id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert!(!audit.contains("value-never-returned"));
    let enabled = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert!(
        enabled
            .servers
            .iter()
            .any(|server| server.server.id == made.id && server.server.own)
    );
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{policy,mcp.add}','false') WHERE id=$1",
    )
    .bind(machine)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let stopped = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(stopped.servers[0].state, McpState::Unavailable);
    assert!(
        stopped.servers[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("machine")
    );
    name_person(
        &world,
        "environment",
        env,
        "environment.manage",
        c.viewer.id(),
    )
    .await;
    let changed = world
        .app
        .call(McpServerUpdate, &[&made.id])
        .token(&c.viewer.token)
        .body(&change(made.revision))
        .send()
        .await
        .ok();
    assert_eq!(
        changed.description, "Changed",
        "the environment's manager needs no separate server page"
    );
    world
        .app
        .call(McpServerDelete, &[&made.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    let secrets: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1",
    )
    .bind(made.id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(secrets, 0);
}

#[tokio::test]
async fn imports_and_reimports_store_only_safe_origin_and_forgetting_removes_the_copy() {
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = environment(&world, &c, true).await;
    let found = json!([{"source":"codex","scope":"user","name":"search","directory":null,"transport":"stdio","digest":"one","tools":2}, {"source":"codex","scope":"directory","name":"folder-tool","directory":"tools","transport":"stdio","digest":"folder","tools":null}]);
    sqlx::query(
        "UPDATE environments SET facts=jsonb_build_object('mcp_found',$2::jsonb) WHERE id=$1",
    )
    .bind(env)
    .bind(found.clone())
    .execute(&world.db.pool)
    .await
    .unwrap();
    let body = EnvironmentMcpImportCreateBody {
        source: "codex".into(),
        scope: McpScope::User,
        name: "search".into(),
        directory: None,
        as_name: None,
        enable: Some(true),
    };
    assert_eq!(
        world
            .app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&c.stranger.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let replied = answer(
        &world,
        machine,
        json!({"transport":"stdio","digest":"one","tools":2}),
    );
    let imported = world
        .app
        .call(EnvironmentMcpImportCreate, &[&env])
        .token(&c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    replied.await.unwrap();
    assert_eq!(imported.spec, json!({"transport":"stdio"}));
    assert_eq!(imported.machine.as_ref().unwrap().id, machine);
    assert_eq!(imported.imported.as_ref().unwrap().by.id, c.admin.id());
    assert!(!imported.imported.as_ref().unwrap().stale);
    let mut changed_found = found;
    changed_found[0]["digest"] = json!("two");
    sqlx::query(
        "UPDATE environments SET facts=jsonb_build_object('mcp_found',$2::jsonb) WHERE id=$1",
    )
    .bind(env)
    .bind(changed_found)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let stale = world
        .app
        .call(McpServerGet, &[&imported.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert!(stale.imported.unwrap().stale);
    let replied = answer(
        &world,
        machine,
        json!({"transport":"stdio","digest":"two","tools":3}),
    );
    let reimported = world
        .app
        .call(McpServerReimport, &[&imported.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    replied.await.unwrap();
    assert!(!reimported.imported.as_ref().unwrap().stale);
    assert!(reimported.revision > imported.revision);
    let project = EnvironmentMcpImportCreateBody {
        source: "codex".into(),
        scope: McpScope::Project,
        name: "folder-tool".into(),
        directory: Some("tools".into()),
        as_name: None,
        enable: Some(false),
    };
    let replied = answer(
        &world,
        machine,
        json!({"transport":"stdio","digest":"folder","tools":null}),
    );
    let folder = world
        .app
        .call(EnvironmentMcpImportCreate, &[&env])
        .token(&c.admin.token)
        .body(&project)
        .send()
        .await
        .ok();
    replied.await.unwrap();
    let mut disable = change(folder.revision);
    disable.enabled = Some(false);
    let disabled = world
        .app
        .call(McpServerUpdate, &[&folder.id])
        .token(&c.admin.token)
        .body(&disable)
        .send()
        .await
        .ok();
    assert!(
        !disabled.enabled,
        "the retained resource starts explicitly disabled"
    );
    assert_eq!(folder.imported.as_ref().unwrap().scope, McpScope::Project);
    let wire_scope: String = sqlx::query_scalar(
        "SELECT body->>'scope' FROM commands WHERE kind='mcp.import' AND body->>'server_id'=$1",
    )
    .bind(folder.id.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(wire_scope, "directory");
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{policy,mcp.import}','false') WHERE id=$1",
    )
    .bind(machine)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let hidden = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert!(hidden.found.is_empty());
    assert!(!hidden.import.allowed);
    assert_eq!(
        world
            .app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&c.admin.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::PolicyMcpImportDisabled
    );
    let replied = answer(&world, machine, json!({}));
    world
        .app
        .call(McpServerDelete, &[&imported.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    replied.await.unwrap();
    let forgotten: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
    )
    .bind(imported.id.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(forgotten, 1);
    sqlx::query("UPDATE machines SET status='revoked',online=false,lost_at=$2 WHERE id=$1")
        .bind(machine)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let retained = world
        .app
        .call(McpServerGet, &[&folder.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(retained.id, folder.id);
    assert!(!retained.enabled);
    assert!(retained.machine.is_none());
    assert!(retained.offerings.is_empty());
    assert_eq!(retained.spec, json!({"transport":"stdio"}));
    assert_eq!(retained.imported.as_ref().unwrap().scope, McpScope::Project);
    assert_eq!(
        retained.imported.as_ref().unwrap().directory.as_deref(),
        Some("tools")
    );
}

#[tokio::test]
async fn selecting_a_different_found_definition_needs_both_server_management_and_native_import_authority()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let (first, old_machine) = environment(&world, &c, true).await;
    let original = orch_types::Hash::of(b"original definition");
    let different = orch_types::Hash::of(b"different definition");
    let found = json!({"source":"codex","scope":"user","name":"same-name","directory":null,"transport":"stdio","digest":"original","definition_fingerprint":original});
    sqlx::query("UPDATE environments SET facts=jsonb_build_object('mcp_found',jsonb_build_array($2::jsonb)) WHERE id=$1").bind(first).bind(&found).execute(&world.db.pool).await.unwrap();
    let replied = answer(
        &world,
        old_machine,
        json!({"transport":"stdio","digest":"original","definition_fingerprint":original,"tools":2}),
    );
    let imported = world
        .app
        .call(EnvironmentMcpImportCreate, &[&first])
        .token(&c.admin.token)
        .body(&EnvironmentMcpImportCreateBody {
            source: "codex".into(),
            scope: McpScope::User,
            name: "same-name".into(),
            directory: None,
            as_name: None,
            enable: Some(true),
        })
        .send()
        .await
        .ok();
    replied.await.unwrap();
    let mut conn = world.conn().await;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn,
        c.admin.id(),
        "host-3",
    )
    .await
    .unwrap();
    let policy = LocalPolicy {
        environment_roots: vec!["/srv/mcp".into()],
        mcp_add: true,
        ..LocalPolicy::default()
    };
    sqlx::query(
        "UPDATE machines SET online=true,facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1",
    )
    .bind(machine)
    .bind(serde_json::to_value(policy).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    let other = orch_types::new_id();
    let mut found = found;
    found["digest"] = json!("different");
    found["definition_fingerprint"] = json!(different);
    sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,placement,home_node_id,machine_id,status,facts,updated_at) VALUES($1,'Different source',$2,$3,'connected',$3,$4,'running',$5,$6)").bind(other).bind(c.admin.id()).bind(c.acme).bind(machine).bind(json!({"mcp_found":[found]})).bind(world.now()).execute(&mut *conn).await.unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: other,
        },
        c.admin.id(),
        orch_authz::ownership::NewAccess::OnlyMe,
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    let shown = world
        .app
        .call(McpServerGet, &[&imported.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    let offering = shown
        .offerings
        .iter()
        .find(|row| row.machine.id == machine)
        .unwrap();
    assert_eq!(offering.state, McpOfferingState::Found);
    assert!(offering.can_use_definition);
    assert_eq!(offering.reason.as_deref(), Some("different_definition"));
    let body = McpOfferingUseBody {
        expected_revision: shown.revision,
        expected_fingerprint: Some(different),
    };
    assert_eq!(
        world
            .app
            .call(McpOfferingUse, &[&shown.id, &machine])
            .token(&c.stranger.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    name_person(&world, "mcp_server", shown.id, "mcp.manage", c.viewer.id()).await;
    assert_eq!(
        world
            .app
            .call(McpOfferingUse, &[&shown.id, &machine])
            .token(&c.viewer.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied,
        "server management alone cannot read a machine's private configuration"
    );
    let replied = answer(
        &world,
        machine,
        json!({"transport":"stdio","digest":"different","definition_fingerprint":different,"tools":3}),
    );
    let selected = world
        .app
        .call(McpOfferingUse, &[&shown.id, &machine])
        .token(&c.admin.token)
        .body(&body)
        .send()
        .await
        .ok();
    replied.await.unwrap();
    assert_eq!(selected.id, imported.id);
    assert!(
        selected
            .offerings
            .iter()
            .any(|row| row.machine.id == machine && row.state == McpOfferingState::Ready)
    );
    assert!(
        selected
            .offerings
            .iter()
            .any(|row| row.machine.id == old_machine && row.state == McpOfferingState::Found)
    );
    assert!(
        selected
            .offerings
            .iter()
            .find(|row| row.machine.id == old_machine)
            .unwrap()
            .can_use_definition,
        "the original import retains the actual source environment for its manager to choose again"
    );
    let definition: Json = sqlx::query_scalar("SELECT spec FROM mcp_servers WHERE id=$1")
        .bind(selected.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        definition["imported"]["definition_fingerprint"],
        json!(different)
    );
    let audit:Json=sqlx::query_scalar("SELECT details FROM audit_log WHERE action='mcp.imported' AND object_id=$1 ORDER BY at DESC,id DESC LIMIT 1").bind(selected.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(audit["definition_after"], json!(different));
}
