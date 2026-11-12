//! Shared lifetime class only: real stored HTTP credentials and an actual native
//! blob-preparation wait. This is not a MessageUpdate HTTP or ordinary-ACL claim.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

use super::*;

use crate::middleware::authentication::ACCESS_TOKEN_PREFIX;

use crate::middleware::authentication::API_TOKEN_PREFIX;

use crate::middleware::authentication::authenticate_on;

use crate::middleware::authentication::new_token;

use crate::middleware::authentication::token_hash;

use crate::registry::Registry;

use orch_api_contract::chats::ChatMessageUpdate;

use orch_api_contract::routes::Endpoint;

use orch_db::queries::identity;

use orch_db::queries::storage;

use orch_db::queries::uploads;

use orch_types::Clock;

use orch_types::Hash;

use orch_types::new_id;
#[cfg(any(test, feature = "test-support"))]
async fn original_raw_and_accepted_key_deadlines_survive_a_real_native_preparation_wait() {
    // Session tests the retained raw deadline. The key's current row is extended
    // after authentication: only its ORIGINAL accepted minimum ends at one second.
    for key in [false, true] {
        let db = orch_testkit::database::TestDb::fresh().await;
        let clock = orch_testkit::clock::manual();
        let app = crate::testing::TestApp::new(
            orch_db::Db::from_pool(db.pool.clone()),
            clock.clone(),
            Registry::new(),
        );
        let user = new_id();
        let credential = new_id();
        let secret = new_token(if key {
            API_TOKEN_PREFIX
        } else {
            ACCESS_TOKEN_PREFIX
        });
        let mut conn = db.pool.acquire().await.unwrap();
        identity::testing::insert_user(&mut conn, user, "native-deadline-owner")
            .await
            .unwrap();
        if key {
            identity::create_api_token_scoped(
                &mut conn,
                &identity::NewApiToken {
                    id: credential,
                    user_id: user,
                    name: "Native preparation deadline",
                    token_hash: &token_hash(&secret),
                    scopes: &["*".to_owned()],
                    node_id: None,
                    at: clock.now(),
                    expires_at: Some(clock.now() + chrono::Duration::seconds(1)),
                },
                None,
            )
            .await
            .unwrap();
        } else {
            identity::testing::insert_session(
                &mut conn,
                credential,
                user,
                &token_hash(&secret),
                clock.now() + chrono::Duration::seconds(1),
            )
            .await
            .unwrap();
        }
        drop(conn);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {secret}").parse().unwrap(),
        );
        // Use canonical authentication of the real stored bearer, never Caller::session.
        let surface = if key {
            orch_api_contract::surface::HttpSurface::Key
        } else {
            orch_api_contract::surface::HttpSurface::Browser
        };
        let caller = authenticate_on(surface, &app.app, &headers, clock.now())
            .await
            .unwrap();
        if key {
            sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                .bind(credential)
                .bind(clock.now() + chrono::Duration::hours(1))
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let mut ctx = Ctx::for_tool(app.app.clone(), caller, ChatMessageUpdate::ROUTE, new_id());
        ctx.surface = Some(surface);
        ctx.headers = headers;
        let bytes = b"native preparation deadline class";
        let hash = Hash::of(bytes);
        sqlx::query("INSERT INTO blobs(hash,size) VALUES($1,$2)")
            .bind(hash.as_bytes().to_vec())
            .bind(i64::try_from(bytes.len()).unwrap())
            .execute(&db.pool)
            .await
            .unwrap();
        // A still-live Guard at the same boundary commits the ordinary storage mutation.
        let mut positive = app.app.db.begin().await.unwrap();
        let live = check(&ctx, &mut positive).await.unwrap();
        live.check_deadline(&ctx).unwrap();
        assert_eq!(storage::pin_blobs(&mut positive, &[hash]).await.unwrap(), 1);
        positive.commit().await.unwrap();
        let before: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(hash.as_bytes().to_vec())
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(before, 1);

        let mut tx = app.app.db.begin().await.unwrap();
        let guard = check(&ctx, &mut tx).await.unwrap();
        guard.check_deadline(&ctx).unwrap();
        let mut held = db.pool.begin().await.unwrap();
        sqlx::query("SELECT hash FROM blobs WHERE hash=$1 FOR UPDATE")
            .bind(hash.as_bytes().to_vec())
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let pending = tokio::spawn(async move {
            // The actual helper used by native message/draft preparation blocks here.
            uploads::lock_blobs(&mut tx, &[hash]).await.unwrap();
            let allowed = guard.check_deadline(&ctx);
            if allowed.is_ok() {
                storage::pin_blobs(&mut tx, &[hash]).await.unwrap();
                tx.commit().await.unwrap();
            } else {
                tx.rollback().await.unwrap();
            }
            allowed
        });
        // Existing ten-second rendezvous budget, below the deployed15s SQL limit.
        let wait = std::time::Duration::from_secs(10);
        let reached = tokio::time::timeout(wait, async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock' AND lower(query) LIKE '%from blobs%')")
                    .bind(blocker).fetch_one(&db.pool).await.unwrap();
                if blocked { break true; }
                if pending.is_finished() { break false; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap_or(false);
        if !reached {
            pending.abort();
            let _ = pending.await;
            held.rollback().await.unwrap();
            panic!("actual native blob preparation must reach the known lock");
        }
        clock.advance(chrono::Duration::seconds(2));
        held.rollback().await.unwrap();
        let mut pending = pending;
        let answer = tokio::time::timeout(wait, &mut pending).await;
        if answer.is_err() {
            pending.abort();
            let _ = pending.await;
        }
        let error = answer
            .expect("released native preparation completes")
            .unwrap()
            .expect_err("expired original authority must refuse before its first mutation");
        assert_eq!(error.code(), ErrorCode::AuthInvalidToken);
        let after: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(hash.as_bytes().to_vec())
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            after, before,
            "the expired request commits no storage effect"
        );
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_original_raw_and_accepted_key_deadlines_survive_a_real_native_preparation_wait()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::original_raw_and_accepted_key_deadlines_survive_a_real_native_preparation_wait(),
    )
}
