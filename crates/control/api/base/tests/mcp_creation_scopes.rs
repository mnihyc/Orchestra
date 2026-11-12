//! MCP preset E rows preserve org/system authority, write-only secrets and native publication.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::resources::{McpServerCreate, McpServerCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::routes::resources::sealing::{binding, install_master_key};
use orch_api_core::testing::RawReply;
use orch_db::queries::{
    identity::{self, NewApiToken},
    resources::mcp as rows,
};
use orch_db::secrets::MasterKey;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

fn rule(operation: &str, kind: &str, id: Option<Id>, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native MCP preset creation",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
fn body(home: Option<Id>, name: &str) -> McpServerCreateBody {
    McpServerCreateBody {
        home_id: home,
        name: name.into(),
        description: Some("Native protected preset metadata".into()),
        spec: json!({"transport":"http","url":"https://tools.example.test/mcp",
            "authentication":{"kind":"bearer","secret":"MCP_TOKEN"},"secret_env":["MCP_TOKEN"],"tools_allow":["search"]}),
        secrets: Some([("MCP_TOKEN".into(), "fixture-private-token-value".into())].into()),
        access: Some(NewResourceAccess::OnlyMe),
    }
}
async fn create(world: &World, token: &str, body: &McpServerCreateBody) -> RawReply {
    world
        .app
        .call(McpServerCreate, &[])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn saved(world: &World, name: &str) -> Option<(Id, Option<Id>, Id, String)> {
    sqlx::query_as("SELECT id,home_id,owner_id,xmin::text FROM mcp_servers WHERE name=$1")
        .bind(name)
        .fetch_optional(&world.db.pool)
        .await
        .unwrap()
}
async fn sealed(world: &World, id: Id) -> Vec<u8> {
    sqlx::query_scalar("SELECT sealed FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 AND name='MCP_TOKEN'")
        .bind(id).fetch_one(&world.db.pool).await.unwrap()
}
fn setup_sealing() {
    install_master_key(MasterKey::new([7; 32]));
}

#[tokio::test]
async fn preset_creation_manage_only_keeps_native_sealing_without_borrowing_another_read() {
    setup_sealing();
    let world = World::new().await;
    let c = resources::company(&world).await;
    let token = key(
        &world,
        c.boss.id(),
        vec![
            rule("McpServerCreate", "org", Some(c.acme), "self", &["manage"]),
            rule("McpServerGet", "org", Some(c.acme), "subtree", &["read"]),
        ],
    )
    .await;
    let reply = accepted(create(&world, &token, &body(Some(c.acme), "manage-only")).await);
    let row = saved(&world, "manage-only").await.unwrap();
    assert_eq!(reply, json!({"projection":"receipt","server_id":row.0}));
    assert_eq!((row.1, row.2), (Some(c.acme), c.boss.id()));
    let ciphertext = sealed(&world, row.0).await;
    assert_ne!(ciphertext, b"fixture-private-token-value");
    assert_eq!(
        MasterKey::new([7; 32])
            .open(&ciphertext, &binding("mcp_server", row.0, "MCP_TOKEN"))
            .unwrap(),
        b"fixture-private-token-value"
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mcp.created'",
    )
    .bind(row.0)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn preset_creation_own_subtree_read_preserves_the_native_member_policy_and_kind() {
    setup_sealing();
    let world = World::new().await;
    let c = resources::company(&world).await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![rule(
            "McpServerCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    let full = accepted(create(&world, &token, &body(Some(c.acme), "member-preset")).await);
    assert!(full.get("projection").is_none());
    assert_eq!(full["name"], "member-preset");
    assert!(
        full.to_string()
            .contains("Native protected preset metadata")
    );
    assert!(!full.to_string().contains("fixture-private-token-value"));
    let outside = key(
        &world,
        c.stranger.id(),
        vec![rule(
            "McpServerCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &outside, &body(Some(c.acme), "outside-member"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&world, "outside-member").await.is_none());
    let wrong = key(
        &world,
        c.boss.id(),
        vec![rule(
            "McpServerCreate",
            "mcp_server",
            Some(c.acme),
            "self",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &wrong, &body(Some(c.acme), "wrong-kind"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&world, "wrong-kind").await.is_none());
}

#[tokio::test]
async fn preset_system_creation_needs_current_admin_standing_and_its_actual_system_scope() {
    setup_sealing();
    let world = World::new().await;
    let c = resources::company(&world).await;
    let system = || {
        rule(
            "McpServerCreate",
            "system",
            None,
            "self",
            &["manage", "read"],
        )
    };
    let unprivileged = key(&world, c.boss.id(), vec![system()]).await;
    assert_eq!(
        create(&world, &unprivileged, &body(None, "no-system-standing"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(c.boss.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let org = key(
        &world,
        c.boss.id(),
        vec![rule(
            "McpServerCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &org, &body(None, "org-is-not-system"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let unrelated = key(
        &world,
        c.boss.id(),
        vec![rule("McpServerGet", "system", None, "self", &["read"])],
    )
    .await;
    assert_eq!(
        create(&world, &unrelated, &body(None, "unrelated-system-op"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let current = key(&world, c.boss.id(), vec![system()]).await;
    let reply = accepted(create(&world, &current, &body(None, "real-system-preset")).await);
    let row = saved(&world, "real-system-preset").await.unwrap();
    assert_eq!(row.1, None);
    assert_eq!(
        reply,
        json!({"projection":"receipt","server_id":row.0}),
        "System is not a fictitious ancestor of a new stored preset"
    );
    assert!(saved(&world, "org-is-not-system").await.is_none());
    assert!(saved(&world, "unrelated-system-op").await.is_none());
}

#[tokio::test]
async fn preset_creation_rechecks_expiry_after_the_actual_native_name_footprint() {
    for expire in [false, true] {
        setup_sealing();
        let world = World::new().await;
        let c = resources::company(&world).await;
        let token = key(
            &world,
            c.boss.id(),
            vec![rule(
                "McpServerCreate",
                "org",
                Some(c.acme),
                "self",
                &["manage"],
            )],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        rows::lock_preset_name(&mut blocker, Some(c.acme), None, "waiting-preset")
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let home = c.acme;
        let request = tokio::spawn(async move {
            let request = app
                .call(McpServerCreate, &[])
                .token(&token)
                .body(&body(Some(home), "waiting-preset"));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the native name serialization was not reached");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT pg_advisory_xact_lock(hashtextextended(%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the real native preset-name namespace must wait");
        assert!(saved(&world, "waiting-preset").await.is_none());
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(saved(&world, "waiting-preset").await.is_none());
        } else {
            let row = saved(&world, "waiting-preset").await.unwrap();
            assert_eq!(
                accepted(reply),
                json!({"projection":"receipt","server_id":row.0})
            );
        }
    }
}

#[tokio::test]
async fn preset_creation_publication_keeps_committed_identity_ciphertext_and_writer_xmin() {
    for expire in [false, true] {
        setup_sealing();
        let world = World::new().await;
        let c = resources::company(&world).await;
        let token = key(
            &world,
            c.boss.id(),
            vec![rule(
                "McpServerCreate",
                "org",
                Some(c.acme),
                "subtree",
                &["manage", "read"],
            )],
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
        let home = c.acme;
        let request = tokio::spawn(async move {
            let request = app
                .call(McpServerCreate, &[])
                .token(&token)
                .body(&body(Some(home), "held-preset"));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the completed native preset did not reach bookkeeping");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual completed native result must wait at activity append");
        let before = saved(&world, "held-preset").await.unwrap();
        let ciphertext = sealed(&world, before.0).await;
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
        if expire {
            assert_eq!(reply, json!({"projection":"receipt","server_id":before.0}));
        } else {
            assert!(reply.get("projection").is_none());
            assert_eq!(reply["name"], "held-preset");
        }
        assert_eq!(saved(&world, "held-preset").await.unwrap(), before);
        assert_eq!(
            sealed(&world, before.0).await,
            ciphertext,
            "the writer must not seal a second secret value"
        );
        assert!(!reply.to_string().contains("fixture-private-token-value"));
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mcp.created'",
        )
        .bind(before.0)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }
}

#[tokio::test]
async fn accepted_preset_creation_replays_current_native_content_with_read_only_and_never_recreates()
 {
    let world = World::new().await;
    let c = resources::company(&world).await;
    setup_sealing();
    let writer = key(
        &world,
        c.boss.id(),
        vec![rule(
            "McpServerCreate",
            "org",
            Some(c.acme),
            "self",
            &["manage"],
        )],
    )
    .await;
    let input = body(Some(c.acme), "once-accepted-native-preset");
    let act = new_id();
    let first = world
        .app
        .call(McpServerCreate, &[])
        .token(&writer)
        .header(
            orch_api_contract::common::IDEMPOTENCY_HEADER,
            &act.to_string(),
        )
        .body(&input)
        .send_bytes()
        .await;
    let first = accepted(first);
    let id: Id = serde_json::from_value(first["server_id"].clone()).unwrap();
    assert_eq!(first, json!({"projection":"receipt","server_id":id}));
    let ciphertext = sealed(&world, id).await;
    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(c.boss.id())
            .bind(act.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        record["outcome"],
        json!({"kind":"mcp_server","id":id,"data":null})
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
        .bind(token_hash(&writer))
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE mcp_servers SET name='current-native-preset',revision=revision+1 WHERE id=$1",
    )
    .bind(id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let reader = key(
        &world,
        c.boss.id(),
        vec![rule(
            "McpServerCreate",
            "mcp_server",
            Some(id),
            "self",
            &["read"],
        )],
    )
    .await;
    let full = accepted(
        world
            .app
            .call(McpServerCreate, &[])
            .token(&reader)
            .header(
                orch_api_contract::common::IDEMPOTENCY_HEADER,
                &act.to_string(),
            )
            .body(&input)
            .send_bytes()
            .await,
    );
    assert!(full.get("projection").is_none(), "{full}");
    assert_eq!(full["id"], json!(id));
    assert_eq!(full["name"], "current-native-preset");
    assert_eq!(sealed(&world, id).await, ciphertext);
    assert!(!full.to_string().contains("fixture-private-token-value"));
    let unrelated = key(
        &world,
        c.boss.id(),
        vec![rule(
            "McpServerGet",
            "mcp_server",
            Some(id),
            "self",
            &["read"],
        )],
    )
    .await;
    let receipt = accepted(
        world
            .app
            .call(McpServerCreate, &[])
            .token(&unrelated)
            .header(
                orch_api_contract::common::IDEMPOTENCY_HEADER,
                &act.to_string(),
            )
            .body(&input)
            .send_bytes()
            .await,
    );
    assert_eq!(receipt, first);
    let refused = world
        .app
        .call(McpServerCreate, &[])
        .token(&reader)
        .header(
            orch_api_contract::common::IDEMPOTENCY_HEADER,
            &new_id().to_string(),
        )
        .body(&input)
        .send_bytes()
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_servers WHERE id=$1 OR name=$2")
        .bind(id)
        .bind(&input.name)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mcp.created'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}
