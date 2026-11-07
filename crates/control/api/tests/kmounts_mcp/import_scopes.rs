//! Real scoped HTTP import, native queue/ACK, filing waits and committed receipt publication.

use super::*;
use http::StatusCode;
use orch_api_core::{testing::RawReply, thread_access::AuthzAccess};
use orch_db::queries::link;
use orch_engine::commands::CommandAuthority;
use tokio::task::JoinHandle;

const OPERATION: &str = "EnvironmentMcpImportCreate";

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
        app.call(EnvironmentMcpImportCreate, &[&env])
            .token(&token)
            .body(&body)
            .send_bytes()
            .await
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
async fn imported_mcp_needs_both_exact_parent_manage_classes_and_keeps_write_only_receipts() {
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = environment(&world, &c, true).await;
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
        let (env, machine) = environment(&world, &c, true).await;
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
        let (env, machine) = environment(&world, &c, true).await;
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
        let (env, machine) = environment(&world, &c, true).await;
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
