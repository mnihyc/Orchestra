//! Entry codes retain the issuing credential through actual HTTP redemption.

use super::*;
use crate::support::FullPortEntryReply as _;
use axum::body::Body;
use http::{Request, StatusCode};
use orch_db::queries::identity;
use tower::ServiceExt;

use super::fixtures::token;

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
        .full_port_entry()
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

#[tokio::test]
async fn a_signed_port_entry_header_replays_only_its_accepted_code_identity_after_redemption_and_key_revocation()
 {
    let (world, manager, _, env, _, _) = setup().await;
    sqlx::query("UPDATE environments SET status='running',facts=COALESCE(facts,'{}'::jsonb)||jsonb_build_object('ports',jsonb_build_object('transports',jsonb_build_array('tcp'))) WHERE id=$1")
        .bind(env).execute(&world.db.pool).await.unwrap();
    let (issuer, writer) = token(
        &world,
        manager.id(),
        env,
        "EnvironmentPortEnter",
        &["read", "write"],
    )
    .await;
    let act = new_id();
    let input = EnvironmentPortEnterBody { opening_id: None };
    let call = world
        .app
        .call(EnvironmentPortEnter, &[&env, &"8080"])
        .token(&writer)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let first = call.send_bytes().await;
    assert_eq!(first.status, StatusCode::OK);
    let first: serde_json::Value = serde_json::from_slice(&first.raw).unwrap();
    let url = first["url"].as_str().unwrap();
    let before: (serde_json::Value, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(manager.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    let id: Id = serde_json::from_value(before.0["outcome"]["id"].clone()).unwrap();
    assert_eq!(before.0["outcome"]["kind"], "port_entry");
    assert_eq!(
        before.0["outcome"]["data"],
        json!({"ticket_expires_at":first["expires_at"]})
    );
    assert!(!before.0.to_string().contains(url));
    assert_eq!(redeem(&world, url).await.status(), StatusCode::SEE_OTHER);
    assert_eq!(redeem(&world, url).await.status(), StatusCode::FORBIDDEN);
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(issuer)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    world.advance(chrono::Duration::minutes(2));
    let (_, reader) = token(&world, manager.id(), env, "EnvironmentPortGet", &["read"]).await;
    let call = world
        .app
        .call(EnvironmentPortEnter, &[&env, &"8080"])
        .token(&reader)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let replay = call.send_bytes().await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&replay.raw).unwrap(),
        json!({
            "projection":"receipt","ticket_id":id,"ticket_expires_at":first["expires_at"]
        })
    );
    let after: (serde_json::Value, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(manager.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(after, before);
    assert_eq!(redeem(&world, url).await.status(), StatusCode::FORBIDDEN);
}
