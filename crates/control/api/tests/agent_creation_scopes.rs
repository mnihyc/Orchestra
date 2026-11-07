//! Native organization profile creation through typed prepared effects and completed Read.
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
use orch_api_contract::resources::{OrgAgentCreate, OrgAgentCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

fn rule(operation: &str, home: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":"org","id":home,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
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
            name: "Native prepared profile creation",
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
fn body(name: &str) -> OrgAgentCreateBody {
    OrgAgentCreateBody {
        name: name.into(),
        harness_kind: "fake".into(),
        profile: json!({"config":{"mode":"native"},"env":{"MODE":"one"}}),
    }
}
async fn create(world: &World, token: &str, home: Id, name: &str) -> RawReply {
    let request = world
        .app
        .call(OrgAgentCreate, &[&home])
        .token(token)
        .body(&body(name));
    request.send_bytes().await
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
async fn saved(world: &World, name: &str) -> Option<(Id, Id, Id, String)> {
    sqlx::query_as("SELECT id,home_id,owner_id,xmin::text FROM agents WHERE name=$1")
        .bind(name)
        .fetch_optional(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn profile_creation_keeps_manage_only_effect_and_does_not_borrow_another_operations_read() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let token = key(
        &world,
        c.boss.id(),
        vec![
            rule("OrgAgentCreate", c.acme, "self", &["manage"]),
            rule("AgentGet", c.acme, "subtree", &["read"]),
        ],
    )
    .await;
    let reply = accepted(create(&world, &token, c.acme, "Write-only native profile").await);
    let row = saved(&world, "Write-only native profile").await.unwrap();
    assert_eq!(reply, json!({"projection":"receipt","agent_id":row.0}));
    assert_eq!((row.1, row.2), (c.acme, c.boss.id()));
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='agent.created'",
    )
    .bind(row.0)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audit, 1);
    assert!(reply.get("profile").is_none());
    assert!(reply.get("home").is_none());
}

#[tokio::test]
async fn profile_creation_own_subtree_read_returns_native_data_and_keeps_ordinary_settings_authority()
 {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let full = key(
        &world,
        c.boss.id(),
        vec![rule(
            "OrgAgentCreate",
            c.acme,
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    let answer = accepted(create(&world, &full, c.acme, "Readable native profile").await);
    let row = saved(&world, "Readable native profile").await.unwrap();
    assert_eq!(answer["id"], json!(row.0));
    assert_eq!(answer["name"], "Readable native profile");
    assert_eq!(answer["profile"], body("").profile);
    assert_eq!(answer["home"]["id"], json!(c.acme));
    let denied = key(
        &world,
        c.viewer.id(),
        vec![rule(
            "OrgAgentCreate",
            c.acme,
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &denied, c.acme, "No ordinary creator")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&world, "No ordinary creator").await.is_none());
    let wrong = key(
        &world,
        c.boss.id(),
        vec![
            json!({"target":{"kind":"project","id":c.acme,"reach":"self"},
        "allow":[{"operation":"OrgAgentCreate","actions":["manage","read"]}]}),
        ],
    )
    .await;
    assert_eq!(
        create(&world, &wrong, c.acme, "Wrong native kind")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&world, "Wrong native kind").await.is_none());
    let ordinary = accepted(create(&world, &c.boss.token, c.acme, "Ordinary native profile").await);
    assert_eq!(ordinary["name"], "Ordinary native profile");
    assert_eq!(ordinary["profile"], body("").profile);
}

#[tokio::test]
async fn profile_creation_rechecks_expiry_after_the_actual_home_footprint_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let c = resources::company(&world).await;
        let token = key(
            &world,
            c.boss.id(),
            vec![rule("OrgAgentCreate", c.acme, "self", &["manage"])],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(c.acme)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let org = c.acme;
        let request = tokio::spawn(async move {
            let request = app
                .call(OrgAgentCreate, &[&org])
                .token(&token)
                .body(&body("Actual footprint wait profile"));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async{
            loop {
                assert!(!request.is_finished(),"the actual creation-home wait was not reached");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM nodes%' AND query LIKE '%FOR SHARE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked{break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the real native home locking query must wait");
        assert!(
            saved(&world, "Actual footprint wait profile")
                .await
                .is_none(),
            "preparation must be effect-free"
        );
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(
                saved(&world, "Actual footprint wait profile")
                    .await
                    .is_none()
            );
        } else {
            let row = saved(&world, "Actual footprint wait profile")
                .await
                .unwrap();
            assert_eq!(
                accepted(reply),
                json!({"projection":"receipt","agent_id":row.0})
            );
        }
    }
}

#[tokio::test]
async fn profile_creation_legacy_settings_writer_keeps_its_generated_native_result() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let secret = new_token(API_TOKEN_PREFIX);
    let permissions = vec!["settings.manage".to_owned()];
    orch_authz::TokenGrant::from_stored(&permissions, Some(c.acme)).unwrap();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: c.boss.id(),
            name: "Legacy native profile creator",
            token_hash: &token_hash(&secret),
            scopes: &permissions,
            node_id: Some(c.acme),
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    let answer = accepted(create(&world, &secret, c.acme, "Legacy generated profile").await);
    let row = saved(&world, "Legacy generated profile").await.unwrap();
    assert_eq!(answer["id"], json!(row.0));
    assert_eq!(answer["profile"], body("").profile);
    assert_eq!(answer["name"], "Legacy generated profile");
}

#[tokio::test]
async fn profile_creation_publication_keeps_committed_identity_without_replaying_after_activity_wait()
 {
    for expire in [false, true] {
        let world = World::new().await;
        let c = resources::company(&world).await;
        let token = key(
            &world,
            c.boss.id(),
            vec![rule(
                "OrgAgentCreate",
                c.acme,
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
        let org = c.acme;
        let request = tokio::spawn(async move {
            let request = app
                .call(OrgAgentCreate, &[&org])
                .token(&token)
                .body(&body("Committed held profile"));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async{
            loop {
                assert!(!request.is_finished(),"the committed response did not reach activity bookkeeping");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked{break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual completed creation response must wait at its activity append");
        let before = saved(&world, "Committed held profile").await.unwrap();
        assert_eq!((before.1, before.2), (org, c.boss.id()));
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
        if expire {
            assert_eq!(reply, json!({"projection":"receipt","agent_id":before.0}));
        } else {
            assert_eq!(reply["id"], json!(before.0));
            assert_eq!(reply["profile"], body("").profile);
        }
        assert_eq!(
            saved(&world, "Committed held profile").await.unwrap(),
            before,
            "publication cannot replay creation or rewrite its tuple"
        );
        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='agent.created'",
        )
        .bind(before.0)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(audit, 1);
    }
}
