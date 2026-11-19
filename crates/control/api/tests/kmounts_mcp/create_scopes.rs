//! Actual owned MCP creation, supplied secret storage, scoped resource readback and waits.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::new_id;

const OPERATION: &str = "EnvironmentMcpServerCreate";

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
fn manage(env: Id) -> Json {
    rule(
        OPERATION,
        "environment",
        env,
        "self",
        &["environment", "mcp_server"],
        &["manage"],
    )
}
fn full(env: Id, home: Id) -> Vec<Json> {
    vec![
        manage(env),
        rule(
            OPERATION,
            "environment",
            env,
            "self",
            &["environment", "mcp_server"],
            &["read"],
        ),
        rule(
            OPERATION,
            "org",
            home,
            "subtree",
            &["org", "project", "topic", "thread", "group", "machine"],
            &["read"],
        ),
    ]
}
async fn key(world: &World, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Owned MCP creation fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}
fn body(name: &str) -> EnvironmentMcpServerCreateBody {
    EnvironmentMcpServerCreateBody {
        name: name.to_owned(),
        description: Some("New supplied tool".to_owned()),
        spec: json!({"transport":"stdio","command":["fixture-tool"],"secret_env":["TOKEN"]}),
        secrets: Some(
            [(
                "TOKEN".to_owned(),
                "newly-supplied-mcp-secret-marker".to_owned(),
            )]
            .into(),
        ),
        enable: Some(true),
    }
}
async fn create(
    world: &World,
    token: &str,
    env: Id,
    body: &EnvironmentMcpServerCreateBody,
) -> RawReply {
    world
        .app
        .call(EnvironmentMcpServerCreate, &[&env])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
async fn count(world: &World, env: Id, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM mcp_servers WHERE environment_id=$1 AND name=$2")
        .bind(env)
        .bind(name)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn activity_wait(world: &World, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{
        assert!(!request.is_finished(),"the actual committed MCP response must reach activity append");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
            .bind(pid).fetch_one(&world.db.pool).await.unwrap();
        if blocked{break;}tokio::task::yield_now().await;
    }}).await.expect("the actual MCP creation final activity INSERT wait");
}

#[tokio::test]
async fn owned_mcp_creation_requires_both_actual_parent_manage_classes_and_returns_a_write_only_receipt()
 {
    install_master_key(MasterKey::new([7; 32]));
    let world = World::new().await;
    let c = company(&world).await;
    let (env, _) = environment(&world, &c, true).await;
    for policy in [
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
            "EnvironmentMcpImportCreate",
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
        let token = key(&world, c.admin.id(), policy).await;
        assert_eq!(
            create(&world, &token, env, &body("denied-tool"))
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(count(&world, env, "denied-tool").await, 0);
    }
    let token = key(
        &world,
        c.admin.id(),
        vec![
            manage(env),
            rule(
                "McpServerGet",
                "environment",
                env,
                "self",
                &["mcp_server"],
                &["read"],
            ),
        ],
    )
    .await;
    let reply = create(&world, &token, env, &body("receipt-tool")).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let receipt: EnvironmentMcpCreateReceipt = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(receipt.environment_id, env);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        json!({"projection":"receipt","server_id":receipt.server_id,"environment_id":env})
    );
    assert_eq!(count(&world, env, "receipt-tool").await, 1);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("newly-supplied-mcp-secret-marker"));
    let sealed: Vec<u8> = sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 AND name='TOKEN'",
    )
    .bind(receipt.server_id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        MasterKey::new([7; 32])
            .open(&sealed, &binding("mcp_server", receipt.server_id, "TOKEN"))
            .unwrap(),
        b"newly-supplied-mcp-secret-marker"
    );
    let denied = key(&world, c.stranger.id(), vec![manage(env)]).await;
    assert_eq!(
        create(&world, &denied, env, &body("stranger-tool"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(count(&world, env, "stranger-tool").await, 0);
}

#[tokio::test]
async fn owned_mcp_created_readback_keeps_the_native_individual_resource_and_independent_parent_content_scope()
 {
    install_master_key(MasterKey::new([7; 32]));
    let world = World::new().await;
    let c = company(&world).await;
    let (env, _) = environment(&world, &c, true).await;
    let token = key(&world, c.admin.id(), full(env, c.acme)).await;
    let reply = create(&world, &token, env, &body("full-tool")).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let shown: EnvironmentMcpCreateResponse = serde_json::from_slice(&reply.raw).unwrap();
    let shown = shown.into_full().unwrap();
    let native = world
        .app
        .call(McpServerGet, &[&shown.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(shown, native);
    assert_eq!(shown.environment.as_ref().unwrap().id, env);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("newly-supplied-mcp-secret-marker"));
    let partial = key(
        &world,
        c.admin.id(),
        vec![
            manage(env),
            rule(
                OPERATION,
                "environment",
                env,
                "self",
                &["mcp_server"],
                &["read"],
            ),
        ],
    )
    .await;
    let reply = create(&world, &partial, env, &body("partial-tool")).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    assert!(
        matches!(
            serde_json::from_slice::<EnvironmentMcpCreateResponse>(&reply.raw).unwrap(),
            EnvironmentMcpCreateResponse::Receipt(_)
        ),
        "independent existing parent Environment content is not lent by MCP Read or completed Manage"
    );
    assert_eq!(count(&world, env, "partial-tool").await, 1);
}

#[tokio::test]
async fn owned_mcp_creation_refreshes_the_actual_key_after_its_native_parent_lock_wait() {
    for expire in [false, true] {
        install_master_key(MasterKey::new([7; 32]));
        let world = World::new().await;
        let c = company(&world).await;
        let (env, _) = environment(&world, &c, true).await;
        let token = key(&world, c.admin.id(), vec![manage(env)]).await;
        assert_eq!(
            create(&world, &token, env, &body("before-wait-tool"))
                .await
                .status,
            StatusCode::CREATED
        );
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(env)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let secret = token.clone();
        let wanted = body("waiting-tool");
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentMcpServerCreate, &[&env]);
            call.token(&secret).body(&wanted).send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async{loop{
            assert!(!request.is_finished(),"the native MCP parent must be serialized before insert");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM environments WHERE id = $1 FOR UPDATE%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked{break;}tokio::task::yield_now().await;
        }}).await.expect("the actual native owned-server parent lock wait");
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::CREATED
            }
        );
        assert_eq!(
            count(&world, env, "waiting-tool").await,
            if expire { 0 } else { 1 }
        );
    }
}

#[tokio::test]
async fn owned_mcp_creation_publication_keeps_the_actual_committed_id_receipt_and_never_replays_secrets_or_insert()
 {
    for expire in [false, true] {
        install_master_key(MasterKey::new([7; 32]));
        let world = World::new().await;
        let c = company(&world).await;
        let (env, _) = environment(&world, &c, true).await;
        let token = key(&world, c.admin.id(), full(env, c.acme)).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let secret = token.clone();
        let wanted = body("publication-tool");
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentMcpServerCreate, &[&env]);
            call.token(&secret).body(&wanted).send_bytes().await
        });
        activity_wait(&world, pid, &request).await;
        let (id,xmin):(Id,String)=sqlx::query_as("SELECT id,xmin::text FROM mcp_servers WHERE environment_id=$1 AND name='publication-tool'")
            .bind(env).fetch_one(&world.db.pool).await.unwrap();
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mcp.created'",
        )
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert!(
            audits > 0,
            "the native creation and audit are already committed"
        );
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply.status, StatusCode::CREATED);
        if expire {
            assert_eq!(
                serde_json::from_slice::<Json>(&reply.raw).unwrap(),
                json!({"projection":"receipt","server_id":id,"environment_id":env})
            );
        } else {
            assert!(matches!(
                serde_json::from_slice::<EnvironmentMcpCreateResponse>(&reply.raw).unwrap(),
                EnvironmentMcpCreateResponse::Full(_)
            ));
        }
        assert!(!String::from_utf8_lossy(&reply.raw).contains("newly-supplied-mcp-secret-marker"));
        assert_eq!(count(&world, env, "publication-tool").await, 1);
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM mcp_servers WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        let after_audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mcp.created'",
        )
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(after, xmin);
        assert_eq!(after_audits, audits);
    }
}
