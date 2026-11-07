//! Entry codes retain the issuing credential through actual HTTP redemption.

use super::*;
use axum::body::Body;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use tower::ServiceExt;

async fn token(
    world: &World,
    user: Id,
    environment: Id,
    operation: &str,
    actions: &[&str],
) -> (Id, String) {
    let scope:KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"environment","id":environment,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native browser port entry",
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
    (id, secret)
}

async fn entry(
    world: &World,
    env: Id,
    token: &str,
) -> orch_api_core::testing::Reply<EnvironmentPortEnterReply> {
    world
        .app
        .call(EnvironmentPortEnter, &[&env, &"8080"])
        .token(token)
        .body(&EnvironmentPortEnterBody { opening_id: None })
        .send()
        .await
}

async fn redeem(world: &World, url: &str) -> axum::response::Response {
    let address = url::Url::parse(url).unwrap();
    let app = world.app.app.clone();
    let router = axum::Router::new()
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            orch_api_core::ports_browser::dispatch,
        ))
        .with_state(app);
    router
        .oneshot(
            Request::builder()
                .uri(format!("{}?{}", address.path(), address.query().unwrap()))
                .header("host", address.host_str().unwrap())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn entry_requires_its_exact_read_write_operation_and_preserves_the_native_port_door() {
    let (world, manager, colleague, env, _, _) = setup().await;
    sqlx::query("UPDATE environments SET status='running',facts=COALESCE(facts,'{}'::jsonb)||jsonb_build_object('ports',jsonb_build_object('transports',jsonb_build_array('tcp'))) WHERE id=$1")
        .bind(env).execute(&world.db.pool).await.unwrap();
    for (operation, actions, target) in [
        ("EnvironmentPortEnter", vec!["read"], env),
        ("EnvironmentPortEnter", vec!["write"], env),
        ("EnvironmentPortUseCreate", vec!["read", "write"], env),
        ("EnvironmentPortEnter", vec!["read", "write"], new_id()),
    ] {
        let (_, key) = token(&world, manager.id(), target, operation, &actions).await;
        assert_eq!(entry(&world, env, &key).await.status, StatusCode::FORBIDDEN);
    }
    let (_, outsider) = token(
        &world,
        colleague.id(),
        env,
        "EnvironmentPortEnter",
        &["read", "write"],
    )
    .await;
    assert!(matches!(
        entry(&world, env, &outsider).await.status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    let (_, key) = token(
        &world,
        manager.id(),
        env,
        "EnvironmentPortEnter",
        &["read", "write"],
    )
    .await;
    let result = entry(&world, env, &key).await.ok();
    let response = redeem(&world, &result.url).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().contains_key(http::header::SET_COOKIE));
    assert_eq!(
        redeem(&world, &result.url).await.status(),
        StatusCode::FORBIDDEN,
        "the entry code is single-use"
    );
    let browser = entry(&world, env, &manager.token).await.ok();
    assert_eq!(
        redeem(&world, &browser.url).await.status(),
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn entry_redemption_rechecks_the_original_key_and_raw_session_instead_of_its_person_id() {
    for revoke in [false, true] {
        let (world, manager, _, env, _, _) = setup().await;
        sqlx::query("UPDATE environments SET status='running',facts=COALESCE(facts,'{}'::jsonb)||jsonb_build_object('ports',jsonb_build_object('transports',jsonb_build_array('tcp'))) WHERE id=$1")
            .bind(env).execute(&world.db.pool).await.unwrap();
        let (id, key) = token(
            &world,
            manager.id(),
            env,
            "EnvironmentPortEnter",
            &["read", "write"],
        )
        .await;
        let issued = entry(&world, env, &key).await.ok();
        if revoke {
            assert!(
                identity::revoke_api_token(&mut *world.conn().await, id, manager.id(), world.now())
                    .await
                    .unwrap()
            );
        }
        let response = redeem(&world, &issued.url).await;
        assert_eq!(
            response.status(),
            if revoke {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::SEE_OTHER
            }
        );
        if revoke {
            assert!(!response.headers().contains_key(http::header::SET_COOKIE));
        }
    }
    let (world, manager, _, env, _, _) = setup().await;
    sqlx::query("UPDATE environments SET status='running',facts=COALESCE(facts,'{}'::jsonb)||jsonb_build_object('ports',jsonb_build_object('transports',jsonb_build_array('tcp'))) WHERE id=$1")
        .bind(env).execute(&world.db.pool).await.unwrap();
    let issued = entry(&world, env, &manager.token).await.ok();
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(manager.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        redeem(&world, &issued.url).await.status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn entry_creation_refreshes_the_key_after_its_actual_environment_row_wait() {
    for expire in [false, true] {
        let (world, manager, _, env, _, _) = setup().await;
        sqlx::query("UPDATE environments SET status='running',facts=COALESCE(facts,'{}'::jsonb)||jsonb_build_object('ports',jsonb_build_object('transports',jsonb_build_array('tcp'))) WHERE id=$1")
            .bind(env).execute(&world.db.pool).await.unwrap();
        let (_, key) = token(
            &world,
            manager.id(),
            env,
            "EnvironmentPortEnter",
            &["read", "write"],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(env)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let action = tokio::spawn(async move {
            let call = app
                .call(EnvironmentPortEnter, &[&env, &"8080"])
                .token(&key)
                .body(&EnvironmentPortEnterBody { opening_id: None });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                assert!(!action.is_finished(),"the native environment lock must be reached");
                let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%SELECT id FROM environments%FOR UPDATE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if found {break;}
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
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
    }
}
