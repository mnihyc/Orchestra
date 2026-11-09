//! Real scoped HTTP import, native queue/ACK, filing waits and committed receipt publication.

use super::*;
use http::StatusCode;
use orch_api_core::{testing::RawReply, thread_access::AuthzAccess};
use orch_db::queries::link;
use orch_engine::commands::CommandAuthority;
use tokio::task::JoinHandle;

const OPERATION: &str = "EnvironmentMcpImportCreate";

async fn registered_environment(world: &World, c: &Company) -> (Id, Id) {
    let (environment, machine) = super::environment(world, c, true).await;
    // These scope controls read the imported resource in the company namespace.
    // Register that namespace explicitly; a person's machine otherwise has its personal home.
    let updated = sqlx::query("UPDATE machines SET home_id=$2,org_id=$2 WHERE id=$1")
        .bind(machine)
        .bind(c.acme)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(updated.rows_affected(), 1);
    (environment, machine)
}

fn rule(
    operation: &str,
    kind: &str,
    id: Id,
    reach: &str,
    resources: &[&str],
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,"allow":[{"operation":operation,"actions":actions}]})
}
fn manage(env: Id) -> Vec<Json> {
    vec![rule(
        OPERATION,
        "environment",
        env,
        "self",
        &["environment", "mcp_server"],
        &["manage"],
    )]
}
fn full(env: Id, org: Id, machine: Id) -> Vec<Json> {
    let mut rules = manage(env);
    rules.push(rule(
        OPERATION,
        "environment",
        env,
        "self",
        &["environment"],
        &["read"],
    ));
    rules.push(rule(
        OPERATION,
        "org",
        org,
        "subtree",
        &["mcp_server", "org", "project", "topic", "thread", "group"],
        &["read"],
    ));
    rules.push(rule(
        OPERATION,
        "machine",
        machine,
        "self",
        &["machine"],
        &["read"],
    ));
    rules
}
fn body(name: &str) -> EnvironmentMcpImportCreateBody {
    EnvironmentMcpImportCreateBody {
        source: "codex".to_owned(),
        scope: McpScope::User,
        name: "search".to_owned(),
        directory: None,
        as_name: Some(name.to_owned()),
        enable: Some(true),
    }
}
async fn found(world: &World, env: Id, digest: &str) {
    sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2::jsonb) WHERE id=$1")
        .bind(env).bind(json!([{"source":"codex","scope":"user","name":"search","directory":null,"transport":"stdio","digest":digest,"tools":2}]))
        .execute(&world.db.pool).await.unwrap();
}
fn start(world: &World, env: Id, token: String, name: &str) -> JoinHandle<RawReply> {
    let app = world.app.clone();
    let body = body(name);
    tokio::spawn(async move {
        let request = app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&token)
            .body(&body);
        request.send_bytes().await
    })
}
async fn done(request: JoinHandle<RawReply>) -> RawReply {
    tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .expect("native import completes")
        .unwrap()
}

/// The native peer accepts the actual queued frame only after its production source verifier.
async fn accept(world: &World, machine: Id, env: Id, request: &JoinHandle<RawReply>) -> (Id, Id) {
    let id = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let id: Option<Id> = sqlx::query_scalar("SELECT id FROM commands WHERE machine_id=$1 AND kind='mcp.import' AND state IN ('queued','sent') ORDER BY created_at,id LIMIT 1")
                .bind(machine).fetch_optional(&world.db.pool).await.unwrap();
            if let Some(id) = id { break id; }
            assert!(!request.is_finished(), "the scoped HTTP act must reach its actual native copy");
            tokio::task::yield_now().await;
        }
    }).await.expect("native import queue arrival");
    let mut tx = world.db.pool.begin().await.unwrap();
    let stored = link::command_with_authority(&mut tx, id)
        .await
        .unwrap()
        .unwrap();
    let authority: CommandAuthority = serde_json::from_value(stored.authority.unwrap()).unwrap();
    let access = AuthzAccess::new(world.app.app.clock.clone());
    assert!(
        authority
            .current_callers(
                &mut tx,
                &stored.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_some()
    );
    assert!(link::mark_accepted(&mut tx, id, world.now()).await.unwrap());
    let server = serde_json::from_value(stored.command.body["server_id"].clone()).unwrap();
    assert_eq!(stored.command.body["environment_id"], json!(env));
    assert!(stored.command.body.get("authority").is_none());
    tx.commit().await.unwrap();
    (id, server)
}
async fn finish(world: &World, command: Id) {
    let result = json!({"transport":"stdio","digest":"one","tools":2});
    assert!(
        link::finish_command(
            &mut *world.conn().await,
            command,
            &link::Ending::Succeeded(&result),
            world.now()
        )
        .await
        .unwrap()
    );
}
async fn marks(world: &World, machine: Id, server: Id) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='mcp.import'),(SELECT count(*) FROM mcp_servers WHERE id=$2),(SELECT count(*) FROM audit_log WHERE action='mcp.imported' AND object_id=$2),(SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='mcp.forget')")
        .bind(machine).bind(server).fetch_one(&world.db.pool).await.unwrap()
}
async fn wait_insert(world: &World, pid: i32, request: &JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "native import must reach its actual filing INSERT");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO mcp_servers%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("actual native import header INSERT wait");
}

#[tokio::test]
async fn discovered_origins_name_their_stored_import_before_enablement_and_respect_the_read_selection()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = registered_environment(&world, &c).await;
    found(&world, env, "one").await;
    let mut input = body("saved-search");
    input.enable = Some(false);
    let app = world.app.clone();
    let token = c.admin.token.clone();
    let request = tokio::spawn(async move {
        let request = app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&token)
            .body(&input);
        request.send_bytes().await
    });
    let (command, server) = accept(&world, machine, env, &request).await;
    finish(&world, command).await;
    assert_eq!(done(request).await.status, StatusCode::CREATED);
    let read = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert!(
        read.servers.is_empty(),
        "an imported copy need not be switched on yet"
    );
    assert_eq!(read.found[0].imported, Some(server));
    found(&world, env, "changed-native-copy").await;
    let read = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(
        read.found[0].imported,
        Some(server),
        "a native edit does not undo the recorded import"
    );
    let environment_read = rule(
        "EnvironmentMcpGet",
        "environment",
        env,
        "self",
        &["environment"],
        &["read"],
    );
    let narrow = create_scopes::key(&world, c.admin.id(), vec![environment_read.clone()]).await;
    let read = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&narrow)
        .send()
        .await
        .ok();
    assert_eq!(
        read.found[0].imported, None,
        "an uncovered resource identity is not added to discovery output"
    );
    let allowed = create_scopes::key(
        &world,
        c.admin.id(),
        vec![
            environment_read,
            rule(
                "EnvironmentMcpGet",
                "mcp_server",
                server,
                "self",
                &["mcp_server"],
                &["read"],
            ),
        ],
    )
    .await;
    let read = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&allowed)
        .send()
        .await
        .ok();
    assert_eq!(read.found[0].imported, Some(server));
    sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2) WHERE id=$1")
        .bind(env).bind(json!([{"source":"fake","scope":"user","name":"search","directory":null,"transport":"stdio","digest":"one","imported":server}]))
        .execute(&world.db.pool).await.unwrap();
    let read = world
        .app
        .call(EnvironmentMcpGet, &[&env])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(
        read.found[0].imported, None,
        "neither the same name in a different program nor a native-claimed ID establishes an import"
    );
}

#[tokio::test]
async fn a_delegated_import_belongs_to_its_importer_in_the_registered_machines_home() {
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = registered_environment(&world, &c).await;
    let department = places::make_department(
        &world,
        &c.boss,
        c.acme,
        "Tools department",
        "tools-department",
    )
    .await;
    sqlx::query("UPDATE machines SET home_id=$2 WHERE id=$1")
        .bind(machine)
        .bind(department.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let separate_home: bool =
        sqlx::query_scalar("SELECT home_node_id IS DISTINCT FROM $2 FROM environments WHERE id=$1")
            .bind(env)
            .bind(department.base.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(
        separate_home,
        "the machine's home is distinct from the environment's"
    );
    let policy = LocalPolicy {
        mcp_import: true,
        policy_delegated: vec!["mcp.import".to_owned()],
        ..LocalPolicy::default()
    };
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',$2) WHERE id=$1")
        .bind(machine)
        .bind(serde_json::to_value(policy).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    resources::name_person(
        &world,
        "environment",
        env,
        "environment.manage",
        c.viewer.id(),
    )
    .await;
    resources::name_person(&world, "machine", machine, "machine.manage", c.viewer.id()).await;
    found(&world, env, "one").await;
    let request = start(&world, env, c.viewer.token.clone(), "delegated-import");
    let (command, server) = accept(&world, machine, env, &request).await;
    finish(&world, command).await;
    let reply = done(request).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let row = orch_db::queries::resources::mcp::servers(&mut *world.conn().await, &[server])
        .await
        .unwrap()
        .remove(0);
    assert_eq!(row.owner_id, Some(c.viewer.id()));
    assert_eq!(row.home_id, Some(department.base.id));
    assert_eq!(row.org_id, Some(c.acme));
    assert_eq!(row.machine_id, Some(machine));
    assert_eq!(row.spec["imported"]["by"], json!(c.viewer.id()));
    let owners:Vec<Id> = sqlx::query_scalar("SELECT holder_id FROM permission_entries WHERE object_kind='mcp_server' AND object_id=$1 AND permission='access.own' AND removed_at IS NULL AND holder_kind='user'").bind(server).fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(owners, vec![c.viewer.id()]);
    assert_ne!(
        row.owner_id,
        Some(c.admin.id()),
        "delegated import does not transfer the resource to the machine owner"
    );
}

#[tokio::test]
async fn imported_mcp_needs_both_exact_parent_manage_classes_and_keeps_write_only_receipts() {
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = registered_environment(&world, &c).await;
    found(&world, env, "one").await;
    for rules in [
        vec![rule(
            OPERATION,
            "environment",
            env,
            "self",
            &["environment"],
            &["manage"],
        )],
        vec![rule(
            OPERATION,
            "environment",
            env,
            "self",
            &["mcp_server"],
            &["manage"],
        )],
        vec![rule(
            OPERATION,
            "environment",
            env,
            "self",
            &["environment", "mcp_server"],
            &["read"],
        )],
        vec![rule(
            "EnvironmentMcpServerCreate",
            "environment",
            env,
            "self",
            &["environment", "mcp_server"],
            &["manage"],
        )],
        vec![rule(
            OPERATION,
            "mcp_server",
            env,
            "self",
            &["environment", "mcp_server"],
            &["manage"],
        )],
    ] {
        let key = create_scopes::key(&world, c.admin.id(), rules).await;
        let reply = done(start(&world, env, key, "refused-import")).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
    }
    let key = create_scopes::key(&world, c.stranger.id(), manage(env)).await;
    assert_eq!(
        done(start(&world, env, key, "stranger-import"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='mcp.import'",
    )
    .bind(machine)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);

    let key = create_scopes::key(&world, c.admin.id(), manage(env)).await;
    let request = start(&world, env, key, "write-only-import");
    let (command, server) = accept(&world, machine, env, &request).await;
    finish(&world, command).await;
    let reply = done(request).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        json!({"projection":"receipt","server_id":server,"environment_id":env})
    );
    assert_eq!(marks(&world, machine, server).await, (1, 1, 1, 0));
    let audited_enable: bool = sqlx::query_scalar("SELECT details @> jsonb_build_object('environment',$2::uuid,'enabled',true,'via','web') FROM audit_log WHERE action='mcp.imported' AND object_id=$1")
        .bind(server).bind(env).fetch_one(&world.db.pool).await.unwrap();
    assert!(
        audited_enable,
        "the one accepted import records that it also switched the server on here"
    );
    let row = orch_db::queries::resources::mcp::servers(&mut *world.conn().await, &[server])
        .await
        .unwrap()
        .remove(0);
    assert!(row.imported);
    assert_eq!(row.environment_id, None);
    assert_eq!(row.machine_id, Some(machine));
    assert_eq!(row.owner_id, Some(c.admin.id()));
    assert_eq!(row.home_id, Some(c.acme));
    assert_eq!(row.name, "write-only-import");
    assert_eq!(row.spec["imported"]["source"], "codex");

    let key = create_scopes::key(&world, c.admin.id(), full(env, c.acme, machine)).await;
    let request = start(&world, env, key, "full-import");
    let (command, server) = accept(&world, machine, env, &request).await;
    finish(&world, command).await;
    let reply = done(request).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let projected: EnvironmentMcpCreateResponse = serde_json::from_slice(&reply.raw).unwrap();
    let projected = projected.into_full().unwrap();
    let ordinary = world
        .app
        .call(McpServerGet, &[&server])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(projected, ordinary);
    assert_eq!(marks(&world, machine, server).await, (2, 1, 1, 0));
}

#[tokio::test]
async fn imported_mcp_rechecks_the_accepted_copy_source_before_filing_and_owns_only_its_inverse() {
    for change in ["unchanged", "scope", "offering", "expired"] {
        let world = World::new().await;
        let c = company(&world).await;
        let (env, machine) = registered_environment(&world, &c).await;
        found(&world, env, "one").await;
        let key = create_scopes::key(&world, c.admin.id(), manage(env)).await;
        let request = start(&world, env, key.clone(), "after-copy-import");
        let (command, server) = accept(&world, machine, env, &request).await;
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
                    .bind(orch_api_core::middleware::authentication::token_hash(&key))
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "offering" => found(&world, env, "two").await,
            "expired" => world.advance(chrono::Duration::hours(2)),
            _ => (),
        }
        finish(&world, command).await;
        let reply = done(request).await;
        if change == "unchanged" {
            assert_eq!(reply.status, StatusCode::CREATED);
            assert_eq!(marks(&world, machine, server).await, (1, 1, 1, 0));
        } else {
            assert!(
                matches!(
                    reply.status,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ),
                "{change}: {}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(marks(&world, machine, server).await, (1, 0, 0, 1));
            let stored = sqlx::query_as::<_,(Json,Json)>("SELECT body,login_authority FROM commands WHERE machine_id=$1 AND kind='mcp.forget'")
                .bind(machine).fetch_one(&world.db.pool).await.unwrap();
            assert_eq!(stored.0, json!({"server_id":server}));
            assert_eq!(stored.1["kind"], "mcp_forget");
        }
    }
}

#[tokio::test]
async fn imported_mcp_checks_the_actual_filing_sql_wait_before_commit_and_never_replays_the_copy() {
    for expire in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let (env, machine) = registered_environment(&world, &c).await;
        found(&world, env, "one").await;
        let key = create_scopes::key(&world, c.admin.id(), manage(env)).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE mcp_servers IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = start(&world, env, key, "waiting-import");
        let (command, server) = accept(&world, machine, env, &request).await;
        finish(&world, command).await;
        wait_insert(&world, pid, &request).await;
        assert_eq!(marks(&world, machine, server).await, (1, 0, 0, 0));
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = done(request).await;
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::CREATED
            }
        );
        assert_eq!(
            marks(&world, machine, server).await,
            if expire { (1, 0, 0, 1) } else { (1, 1, 1, 0) }
        );
    }
}

#[tokio::test]
async fn imported_mcp_keeps_the_committed_identity_receipt_after_final_read_loss_without_copy_or_filing_replay()
 {
    for expire in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let (env, machine) = registered_environment(&world, &c).await;
        found(&world, env, "one").await;
        let key = create_scopes::key(&world, c.admin.id(), full(env, c.acme, machine)).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = start(&world, env, key, "published-import");
        let (command, server) = accept(&world, machine, env, &request).await;
        finish(&world, command).await;
        create_scopes::activity_wait(&world, pid, &request).await;
        assert_eq!(marks(&world, machine, server).await, (1, 1, 1, 0));
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM mcp_servers WHERE id=$1")
            .bind(server)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = done(request).await;
        assert_eq!(reply.status, StatusCode::CREATED);
        let projected: EnvironmentMcpCreateResponse = serde_json::from_slice(&reply.raw).unwrap();
        if expire {
            assert_eq!(projected.into_full().unwrap_err().server_id, server);
        } else {
            assert_eq!(projected.into_full().unwrap().id, server);
        }
        let current: String = sqlx::query_scalar("SELECT xmin::text FROM mcp_servers WHERE id=$1")
            .bind(server)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(current, xmin);
        assert_eq!(marks(&world, machine, server).await, (1, 1, 1, 0));
    }
}

#[tokio::test]
async fn completed_native_import_replays_current_server_without_another_copy_or_old_management() {
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = registered_environment(&world, &c).await;
    found(&world, env, "one").await;
    let writer = create_scopes::key(&world, c.admin.id(), manage(env)).await;
    let input = body("once-imported-server");
    let act = orch_types::new_id();
    let app = world.app.clone();
    let token = writer.clone();
    let native_input = input.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&token)
            .header(
                orch_api_contract::common::IDEMPOTENCY_HEADER,
                &act.to_string(),
            )
            .body(&native_input);
        call.send_bytes().await
    });
    let (command, server) = accept(&world, machine, env, &request).await;
    finish(&world, command).await;
    let first = done(request).await;
    assert_eq!(first.status, StatusCode::CREATED);
    let accepted: Json = serde_json::from_slice(&first.raw).unwrap();
    assert_eq!(
        accepted,
        json!({"projection":"receipt","server_id":server,"environment_id":env})
    );
    assert_eq!(marks(&world, machine, server).await, (1, 1, 1, 0));
    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(c.admin.id())
            .bind(act.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let mut native_outcome = record["outcome"].clone();
    let command_receipt = native_outcome["data"]
        .as_object_mut()
        .unwrap()
        .remove("native_command")
        .expect("actual import queue reference");
    assert_eq!(command_receipt["id"], json!(command));
    assert!(command_receipt["fingerprint"].is_string());
    assert!(command_receipt.get("body").is_none());
    assert_eq!(
        native_outcome,
        json!({"kind":"environment_mcp_import","id":server,"data":{"environment_id":env}})
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
        .bind(orch_api_core::middleware::authentication::token_hash(
            &writer,
        ))
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE mcp_servers SET name='current-imported-server',revision=revision+1 WHERE id=$1",
    )
    .bind(server)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let reader = create_scopes::key(
        &world,
        c.admin.id(),
        vec![rule(
            OPERATION,
            "mcp_server",
            server,
            "self",
            &["mcp_server"],
            &["read"],
        )],
    )
    .await;
    let replay = world
        .app
        .call(EnvironmentMcpImportCreate, &[&env])
        .token(&reader)
        .header(
            orch_api_contract::common::IDEMPOTENCY_HEADER,
            &act.to_string(),
        )
        .body(&input)
        .send_bytes()
        .await;
    assert_eq!(
        replay.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&replay.raw)
    );
    let full: Json = serde_json::from_slice(&replay.raw).unwrap();
    assert!(full.get("projection").is_none(), "{full}");
    assert_eq!(full["id"], json!(server));
    assert_eq!(full["name"], "current-imported-server");
    let unrelated = create_scopes::key(
        &world,
        c.admin.id(),
        vec![rule(
            "McpServerGet",
            "mcp_server",
            server,
            "self",
            &["mcp_server"],
            &["read"],
        )],
    )
    .await;
    let no_read = world
        .app
        .call(EnvironmentMcpImportCreate, &[&env])
        .token(&unrelated)
        .header(
            orch_api_contract::common::IDEMPOTENCY_HEADER,
            &act.to_string(),
        )
        .body(&input)
        .send_bytes()
        .await;
    assert_eq!(no_read.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Json>(&no_read.raw).unwrap(),
        accepted
    );
    assert_eq!(
        world
            .app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&reader)
            .header(
                orch_api_contract::common::IDEMPOTENCY_HEADER,
                &orch_types::new_id().to_string()
            )
            .body(&input)
            .send_bytes()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(marks(&world, machine, server).await, (1, 1, 1, 0));
}

#[tokio::test]
async fn an_import_header_survives_cancellation_after_its_actual_queue_without_issuing_again() {
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = registered_environment(&world, &c).await;
    found(&world, env, "one").await;
    let writer = create_scopes::key(&world, c.admin.id(), manage(env)).await;
    let act = orch_types::new_id();
    let input = body("cancelled-waiter");
    let app = world.app.clone();
    let first_body = input.clone();
    let first_key = writer.clone();
    let request = tokio::spawn(async move {
        let request = app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&first_key)
            .header(
                orch_api_contract::common::IDEMPOTENCY_HEADER,
                &act.to_string(),
            )
            .body(&first_body);
        request.send_bytes().await
    });
    let (command, server) = accept(&world, machine, env, &request).await;
    request.abort();
    assert!(matches!(request.await, Err(error) if error.is_cancelled()));
    let before: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(c.admin.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(before.0["outcome"]["id"], json!(server));
    assert_eq!(
        before.0["outcome"]["data"]["native_command"]["id"],
        json!(command)
    );
    assert_eq!(
        before.0["outcome"]["data"]["native_command"]["kind"],
        "mcp.import"
    );
    assert!(before.0.to_string().find("cancelled-waiter").is_none());
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
        .bind(orch_api_core::middleware::authentication::token_hash(
            &writer,
        ))
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let reader = create_scopes::key(
        &world,
        c.admin.id(),
        vec![rule(
            "EnvironmentMcpGet",
            "environment",
            env,
            "self",
            &["environment"],
            &["read"],
        )],
    )
    .await;
    let replay = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let request = world
            .app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&reader)
            .header(
                orch_api_contract::common::IDEMPOTENCY_HEADER,
                &act.to_string(),
            )
            .body(&input);
        request.send_bytes().await
    })
    .await
    .expect("a completed queue reference never waits for or issues another command");
    assert_eq!(
        replay.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&replay.raw)
    );
    assert_eq!(
        serde_json::from_slice::<Json>(&replay.raw).unwrap(),
        json!({"projection":"receipt","server_id":server,"environment_id":env})
    );
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(c.admin.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(after, before);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='mcp.import'),(SELECT count(*) FROM mcp_servers WHERE id=$2)")
        .bind(machine).bind(server).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(counts, (1, 0));
    let stored = link::command(&mut *world.conn().await, command)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, "accepted");
    assert!(stored.result.is_none());
}
