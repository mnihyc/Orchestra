//! Stored updates keep their native subject, sealed supplied values and completed response.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::new_id;

const OP: &str = "McpServerUpdate";

fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
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
            name: "Native MCP change scopes",
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

async fn preset(world: &World, c: &Company, name: &str) -> McpServer {
    {
        let request = world
        .app
        .call(McpServerCreate, &[])
        .token(&c.viewer.token)
        .body(&McpServerCreateBody {
            name: name.to_owned(),
            description: Some("Stored private MCP metadata".to_owned()),
            home_id: Some(c.acme),
            spec: json!({"transport":"stdio","command":["fixture-server"],"secret_env":["TOKEN"]}),
            secrets: Some(
                [(
                    "TOKEN".to_owned(),
                    "initial-mcp-rotation-fixture".to_owned(),
                )]
                .into(),
            ),
            access: Some(NewResourceAccess::OnlyMe),
        });
        request.send().await
    }
    .ok()
    .into_full()
    .unwrap()
}

async fn update(world: &World, token: &str, id: Id, body: &McpServerUpdateBody) -> RawReply {
    let request = world
        .app
        .call(McpServerUpdate, &[&id])
        .token(token)
        .body(body);
    request.send_bytes().await
}

fn value(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn saved(world: &World, id: Id) -> (i64, String, bool, String) {
    sqlx::query_as("SELECT revision,description,enabled,xmin::text FROM mcp_servers WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn sealed(world: &World, id: Id) -> Vec<u8> {
    sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 AND name='TOKEN'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

async fn audits(world: &World, id: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mcp.updated'")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn mcp_update_manage_keeps_the_native_change_and_supplied_secret_without_borrowing_another_read()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let made = preset(&world, &c, "receipt-update").await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![
            rule(OP, "mcp_server", made.id, "self", &["manage"]),
            rule("McpServerGet", "mcp_server", made.id, "self", &["read"]),
        ],
    )
    .await;
    let mut body = change(made.revision);
    body.secrets = Some(
        [(
            "TOKEN".to_owned(),
            "supplied-mcp-rotation-fixture".to_owned(),
        )]
        .into(),
    );
    let result = value(update(&world, &token, made.id, &body).await);
    assert_eq!(
        result,
        json!({"projection":"receipt","server_id":made.id,"revision":made.revision+1})
    );
    let state = saved(&world, made.id).await;
    assert_eq!(state.0, made.revision + 1);
    assert_eq!(state.1, "Changed");
    assert_eq!(audits(&world, made.id).await, 1);
    assert_eq!(
        MasterKey::new([7; 32])
            .open(
                &sealed(&world, made.id).await,
                &binding("mcp_server", made.id, "TOKEN")
            )
            .unwrap(),
        b"supplied-mcp-rotation-fixture"
    );
    let stale = update(&world, &token, made.id, &body).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    let problem: Json = serde_json::from_slice(&stale.raw).unwrap();
    assert!(problem.get("current").is_none());
    assert!(!String::from_utf8_lossy(&stale.raw).contains("fixture-server"));
    assert_eq!(saved(&world, made.id).await, state);
    assert_eq!(audits(&world, made.id).await, 1);
}

#[tokio::test]
async fn mcp_update_own_read_returns_native_content_and_current_stale_metadata_without_repeating_the_effect()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let made = preset(&world, &c, "read-update").await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![
            rule(OP, "mcp_server", made.id, "self", &["manage", "read"]),
            rule(OP, "org", c.acme, "subtree", &["read"]),
        ],
    )
    .await;
    let body = change(made.revision);
    let result = value(update(&world, &token, made.id, &body).await);
    assert_eq!(result["id"], json!(made.id));
    assert_eq!(result["description"], "Changed");
    assert_eq!(result["spec"], made.spec);
    assert_eq!(result["revision"], made.revision + 1);
    assert!(!result.to_string().contains("initial-mcp-rotation-fixture"));
    let before = saved(&world, made.id).await;
    let stale = update(&world, &token, made.id, &body).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    let problem: orch_api_contract::common::Problem = serde_json::from_slice(&stale.raw).unwrap();
    assert_eq!(problem.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(problem.extra["current"]["id"], json!(made.id));
    assert_eq!(problem.extra["current"]["revision"], made.revision + 1);
    assert_eq!(saved(&world, made.id).await, before);
    assert_eq!(audits(&world, made.id).await, 1);
}

#[tokio::test]
async fn mcp_update_keeps_native_management_and_the_actual_owned_environment_business_classes() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = preset(&world, &c, "native-update").await;
    let before = saved(&world, made.id).await;
    for (actor, rules) in [
        (
            c.stranger.id(),
            vec![rule(OP, "mcp_server", made.id, "self", &["manage"])],
        ),
        (
            c.viewer.id(),
            vec![rule(OP, "mcp_server", made.id, "self", &["read"])],
        ),
        (
            c.viewer.id(),
            vec![rule(OP, "environment", made.id, "self", &["manage"])],
        ),
        (
            c.viewer.id(),
            vec![rule(
                "McpServerReimport",
                "mcp_server",
                made.id,
                "self",
                &["manage"],
            )],
        ),
    ] {
        let token = key(&world, actor, rules).await;
        let refused = update(&world, &token, made.id, &change(made.revision)).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&refused.raw).contains("Stored private MCP metadata"));
        assert_eq!(saved(&world, made.id).await, before);
    }
    let (env, _) = environment(&world, &c, true).await;
    let owned = {
        let request = world
            .app
            .call(EnvironmentMcpServerCreate, &[&env])
            .token(&c.admin.token)
            .body(&EnvironmentMcpServerCreateBody {
                name: "owned-change".to_owned(),
                description: None,
                spec: spec(),
                secrets: None,
                enable: None,
            });
        request.send().await
    }
    .ok()
    .into_full()
    .unwrap();
    for family in ["mcp_server", "environment"] {
        let mut row = rule(OP, "environment", env, "self", &["manage"]);
        row["resources"] = json!([family]);
        let token = key(&world, c.admin.id(), vec![row]).await;
        assert_eq!(
            update(&world, &token, owned.id, &change(owned.revision))
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    let alias = key(
        &world,
        c.admin.id(),
        vec![rule(OP, "mcp_server", owned.id, "self", &["manage"])],
    )
    .await;
    assert_eq!(
        update(&world, &alias, owned.id, &change(owned.revision))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let token = key(
        &world,
        c.admin.id(),
        vec![rule(OP, "environment", env, "self", &["manage"])],
    )
    .await;
    assert_eq!(
        value(update(&world, &token, owned.id, &change(owned.revision)).await),
        json!({"projection":"receipt","server_id":owned.id,"revision":owned.revision+1})
    );
}

async fn wait_on(
    world: &World,
    pid: i32,
    needle: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(
                !request.is_finished(),
                "the actual native request must reach its lock wait"
            );
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)",
            )
            .bind(pid)
            .bind(needle)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual native query must wait on the exact held backend");
}

#[tokio::test]
async fn mcp_update_waits_for_the_actual_owned_parent_before_its_first_server_change() {
    for expired in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let (env, _) = environment(&world, &c, true).await;
        let owned = {
            let request = world
                .app
                .call(EnvironmentMcpServerCreate, &[&env])
                .token(&c.admin.token)
                .body(&EnvironmentMcpServerCreateBody {
                    name: "parent-held-change".to_owned(),
                    description: None,
                    spec: spec(),
                    secrets: None,
                    enable: None,
                });
            request.send().await
        }
        .ok()
        .into_full()
        .unwrap();
        let token = key(
            &world,
            c.admin.id(),
            vec![rule(OP, "environment", env, "self", &["manage"])],
        )
        .await;
        let before = saved(&world, owned.id).await;
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
        let id = owned.id;
        let body = change(owned.revision);
        let request = tokio::spawn(async move {
            let request = app.call(McpServerUpdate, &[&id]).token(&token).body(&body);
            request.send_bytes().await
        });
        wait_on(&world, pid, "%FROM environments%FOR UPDATE%", &request).await;
        assert_eq!(saved(&world, owned.id).await, before);
        assert_eq!(audits(&world, owned.id).await, 0);
        if expired {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if expired {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(saved(&world, owned.id).await, before);
            assert_eq!(audits(&world, owned.id).await, 0);
        } else {
            assert_eq!(value(reply)["revision"], owned.revision + 1);
            assert_eq!(audits(&world, owned.id).await, 1);
        }
    }
}

#[tokio::test]
async fn mcp_update_rechecks_current_scope_and_expiry_after_its_actual_server_row_wait() {
    for change_kind in ["unchanged", "expired", "relocated"] {
        let world = World::new().await;
        let c = company(&world).await;
        let made = preset(&world, &c, "held-update").await;
        let token = key(
            &world,
            c.viewer.id(),
            vec![rule(OP, "org", c.acme, "subtree", &["manage"])],
        )
        .await;
        let before = saved(&world, made.id).await;
        let before_secret = sealed(&world, made.id).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM mcp_servers WHERE id=$1 FOR UPDATE")
            .bind(made.id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let id = made.id;
        let body = change(made.revision);
        let request = tokio::spawn(async move {
            let request = app.call(McpServerUpdate, &[&id]).token(&token).body(&body);
            request.send_bytes().await
        });
        wait_on(&world, pid, "%FROM mcp_servers%FOR UPDATE%", &request).await;
        assert_eq!(saved(&world, made.id).await, before);
        if change_kind == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        if change_kind == "relocated" {
            sqlx::query("UPDATE mcp_servers SET org_id=$2,home_id=$2 WHERE id=$1")
                .bind(made.id)
                .bind(c.viewer.person.personal_org)
                .execute(&mut *blocker)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if change_kind == "unchanged" {
            assert_eq!(value(reply)["revision"], made.revision + 1);
            assert_eq!(audits(&world, made.id).await, 1);
        } else {
            assert_eq!(
                reply.status,
                if change_kind == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            let after = saved(&world, made.id).await;
            assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
            assert_eq!(sealed(&world, made.id).await, before_secret);
            assert_eq!(audits(&world, made.id).await, 0);
        }
    }
}

#[tokio::test]
async fn mcp_update_completes_real_preset_name_and_secret_waits_before_its_first_effect() {
    for wait_kind in ["name", "secret"] {
        for expired in [false, true] {
            let world = World::new().await;
            let c = company(&world).await;
            let made = preset(&world, &c, "first-effect-update").await;
            let token = key(
                &world,
                c.viewer.id(),
                vec![rule(OP, "mcp_server", made.id, "self", &["manage"])],
            )
            .await;
            let before = saved(&world, made.id).await;
            let before_secret = sealed(&world, made.id).await;
            let mut blocker = world.db.pool.begin().await.unwrap();
            if wait_kind == "name" {
                orch_db::queries::resources::mcp::lock_preset_name(
                    &mut blocker,
                    Some(c.acme),
                    None,
                    "renamed-checked-update",
                )
                .await
                .unwrap();
            } else {
                sqlx::query("SELECT id FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 AND name='TOKEN' FOR UPDATE")
                    .bind(made.id).fetch_one(&mut *blocker).await.unwrap();
            }
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = world.app.clone();
            let id = made.id;
            let mut body = change(made.revision);
            body.name = Some("renamed-checked-update".to_owned());
            body.secrets =
                Some([("TOKEN".to_owned(), "after-native-lock-fixture".to_owned())].into());
            let request = tokio::spawn(async move {
                let request = app.call(McpServerUpdate, &[&id]).token(&token).body(&body);
                request.send_bytes().await
            });
            wait_on(
                &world,
                pid,
                if wait_kind == "name" {
                    "%pg_advisory_xact_lock%"
                } else {
                    "%FROM secrets%FOR UPDATE%"
                },
                &request,
            )
            .await;
            assert_eq!(saved(&world, made.id).await, before);
            assert_eq!(sealed(&world, made.id).await, before_secret);
            assert_eq!(audits(&world, made.id).await, 0);
            if expired {
                world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap();
            if expired {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                assert_eq!(saved(&world, made.id).await, before);
                assert_eq!(sealed(&world, made.id).await, before_secret);
                assert_eq!(audits(&world, made.id).await, 0);
            } else {
                assert_eq!(value(reply)["revision"], made.revision + 1);
                assert_ne!(sealed(&world, made.id).await, before_secret);
                assert_eq!(audits(&world, made.id).await, 1);
            }
        }
    }
}

#[tokio::test]
async fn mcp_update_publication_preserves_the_committed_revision_and_audit_without_repeating_the_writer()
 {
    for change_kind in ["unchanged", "expired", "read_lost"] {
        let world = World::new().await;
        let c = company(&world).await;
        let made = preset(&world, &c, "publication-update").await;
        let token = key(
            &world,
            c.viewer.id(),
            vec![
                rule(OP, "mcp_server", made.id, "self", &["manage"]),
                rule(OP, "org", c.acme, "subtree", &["read"]),
            ],
        )
        .await;
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
        let id = made.id;
        let body = change(made.revision);
        let request = tokio::spawn(async move {
            let request = app.call(McpServerUpdate, &[&id]).token(&token).body(&body);
            request.send_bytes().await
        });
        wait_on(&world, pid, "%INSERT INTO api_token_activity%", &request).await;
        assert_eq!(saved(&world, made.id).await.0, made.revision + 1);
        assert_eq!(audits(&world, made.id).await, 1);
        if change_kind == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        if change_kind == "read_lost" {
            sqlx::query("UPDATE mcp_servers SET org_id=$2,home_id=$2 WHERE id=$1")
                .bind(made.id)
                .bind(c.viewer.person.personal_org)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        let committed = saved(&world, made.id).await;
        let committed_secret = sealed(&world, made.id).await;
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        let result = value(reply);
        if change_kind == "unchanged" {
            assert_eq!(result["id"], json!(made.id));
            assert_eq!(result["description"], "Changed");
        } else {
            assert_eq!(
                result,
                json!({"projection":"receipt","server_id":made.id,"revision":made.revision+1})
            );
        }
        assert_eq!(saved(&world, made.id).await, committed);
        assert_eq!(sealed(&world, made.id).await, committed_secret);
        assert_eq!(audits(&world, made.id).await, 1);
    }
}
