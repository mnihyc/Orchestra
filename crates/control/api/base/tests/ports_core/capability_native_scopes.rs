//! Actual native capability grants under exact scoped and ordinary callers.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::Json;

const OP: &str = "EnvironmentPortCapabilityGrant";

fn rule(kind: &str, id: Id, operation: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, user: Id, rules: Vec<Json>, legacy: bool) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    if !legacy {
        assert!(scope.validate().is_empty());
    }
    let secret = new_token(API_TOKEN_PREFIX);
    let value = serde_json::to_value(scope).unwrap();
    let legacy_scopes = if legacy {
        vec!["ports.manage".to_owned()]
    } else {
        Vec::new()
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native port capability control",
            token_hash: &token_hash(&secret),
            scopes: &legacy_scopes,
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        (!legacy).then_some(&value),
    )
    .await
    .unwrap();
    secret
}

async fn grant(world: &World, environment: Id, token: &str) -> RawReply {
    let request = world
        .app
        .call(
            EnvironmentPortCapabilityGrant,
            &[&environment, &"port_place"],
        )
        .token(token);
    request.send_bytes().await
}

async fn granted(world: &World, environment: Id) -> bool {
    sqlx::query_scalar(
        "SELECT 'port_place'=ANY(granted_capabilities) FROM environments WHERE id=$1",
    )
    .bind(environment)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn capability_grant_manage_only_keeps_the_native_effect_and_only_its_own_read_returns_policy()
{
    for read in [false, true] {
        let (world, manager, _, env, _, _) = setup().await;
        let mut rules = vec![rule("environment", env, OP, &["manage"])];
        rules.push(rule(
            "environment",
            env,
            if read { OP } else { "EnvironmentPortGet" },
            &["read"],
        ));
        let token = key(&world, manager.id(), rules, false).await;
        let response = grant(&world, env, &token).await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&response.raw)
        );
        let body: Json = serde_json::from_slice(&response.raw).unwrap();
        assert!(granted(&world, env).await);
        if read {
            assert_eq!(body["key"], "port_place");
            assert_eq!(body["granted"], true);
            assert!(body.get("label").is_some() && body.get("approver_scope").is_some());
            assert!(body.get("projection").is_none());
        } else {
            assert_eq!(
                body,
                json!({"projection":"receipt","environment_id":env,"key":"port_place","granted":true})
            );
            // Native replay is still successful and cannot disclose stored grant attribution.
            let replay = grant(&world, env, &token).await;
            assert_eq!(serde_json::from_slice::<Json>(&replay.raw).unwrap(), body);
        }
        let browser = grant(&world, env, &manager.token).await;
        assert_eq!(browser.status, StatusCode::OK);
        assert!(
            serde_json::from_slice::<Json>(&browser.raw)
                .unwrap()
                .get("label")
                .is_some()
        );
    }
}

#[tokio::test]
async fn capability_grant_scope_does_not_lend_another_operation_target_action_or_native_ports_manage()
 {
    let (world, manager, colleague, env, project, _) = setup().await;
    for rules in [
        vec![rule("environment", env, "EnvironmentPortGet", &["read"])],
        vec![rule("environment", env, OP, &["read"])],
        vec![rule("environment", new_id(), OP, &["manage"])],
        vec![rule("project", project, OP, &["manage"])],
    ] {
        let token = key(&world, manager.id(), rules, false).await;
        assert_eq!(
            grant(&world, env, &token).await.status,
            StatusCode::FORBIDDEN
        );
        assert!(!granted(&world, env).await);
    }
    let no_native = key(
        &world,
        colleague.id(),
        vec![rule("environment", env, OP, &["manage", "read"])],
        false,
    )
    .await;
    let denied = grant(&world, env, &no_native).await;
    assert!(matches!(
        denied.status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    assert!(!granted(&world, env).await);
    let legacy = key(&world, manager.id(), Vec::new(), true).await;
    let response = grant(&world, env, &legacy).await;
    assert_eq!(response.status, StatusCode::OK);
    let body: Json = serde_json::from_slice(&response.raw).unwrap();
    assert!(body.get("label").is_some() && body.get("projection").is_none());
}

async fn blocked(
    world: &World,
    pid: i32,
    request: &tokio::task::JoinHandle<RawReply>,
    query: &str,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!request.is_finished(),"the actual native SQL wait was not reached");
            let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE $2)")
                .bind(pid).bind(query).fetch_one(&world.db.pool).await.unwrap();
            if found {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the native wait must be observed, not assumed from elapsed time");
}

#[tokio::test]
async fn capability_grant_refreshes_the_key_after_the_real_environment_lock_before_its_first_update()
 {
    for expire in [false, true] {
        let (world, manager, _, env, _, _) = setup().await;
        let token = key(
            &world,
            manager.id(),
            vec![rule("environment", env, OP, &["manage"])],
            false,
        )
        .await;
        let mut lock = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(env)
            .execute(&mut *lock)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let app = world.app.clone();
        let action = tokio::spawn(async move {
            let request = app
                .call(EnvironmentPortCapabilityGrant, &[&env, &"port_place"])
                .token(&token);
            request.send_bytes().await
        });
        blocked(
            &world,
            pid,
            &action,
            "%SELECT id FROM environments%FOR UPDATE%",
        )
        .await;
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        lock.commit().await.unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), action)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            response.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(granted(&world, env).await, !expire);
    }
}

#[tokio::test]
async fn completed_capability_grant_keeps_its_receipt_after_activity_wait_without_writer_replay() {
    for expire in [false, true] {
        let (world, manager, _, env, _, _) = setup().await;
        let token = key(
            &world,
            manager.id(),
            vec![rule("environment", env, OP, &["manage", "read"])],
            false,
        )
        .await;
        let mut lock = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let app = world.app.clone();
        let action = tokio::spawn(async move {
            let request = app
                .call(EnvironmentPortCapabilityGrant, &[&env, &"port_place"])
                .token(&token);
            request.send_bytes().await
        });
        blocked(&world, pid, &action, "%INSERT INTO api_token_activity%").await;
        assert!(granted(&world, env).await);
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        lock.commit().await.unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), action)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status, StatusCode::OK);
        let value: Json = serde_json::from_slice(&response.raw).unwrap();
        if expire {
            assert_eq!(
                value,
                json!({"projection":"receipt","environment_id":env,"key":"port_place","granted":true})
            );
        } else {
            assert!(value.get("label").is_some());
        }
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            after, xmin,
            "readback/publication must not rerun the completed grant"
        );
    }
}
