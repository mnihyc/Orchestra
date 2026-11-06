//! Actual socket-origin authentication after its real credential-row wait.
//! These controls call the existing socket login constructor; they do not claim writer-byte proof.

#[path = "../../tests/realtime_support/mod.rs"]
mod fixture;

use super::server;
use orch_realtime_contract::version::CloseCode;
use orch_types::Clock;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn raw_deadline_after_identity_wait(key: bool) -> Proof {
    let rig = fixture::Rig::with_time_held().await;
    let (user, browser) = rig.person("socket-origin-reader").await;
    let token = if key {
        rig.api_token(user).await
    } else {
        browser
    };
    let baseline =
        server::epoch_test_connection(&rig.server.app, &rig.server.app.realtime.inner, &token)
            .await
            .map_err(|error| format!("ordinary exact socket login premise: {error:?}"))?;
    assert_eq!(baseline.user, user);
    let id = baseline.credential().id();
    let until = Clock::now(rig.server.app.clock.as_ref()) + chrono::Duration::seconds(2);
    let update = if key {
        "UPDATE api_tokens SET expires_at=$2 WHERE id=$1"
    } else {
        "UPDATE sessions SET expires_at=$2 WHERE id=$1"
    };
    sqlx::query(update)
        .bind(id)
        .bind(until)
        .execute(rig.pool.pool())
        .await?;
    let mut blocker = rig.pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let lock = if key {
        "SELECT id FROM api_tokens WHERE id=$1 FOR NO KEY UPDATE"
    } else {
        "SELECT id FROM sessions WHERE id=$1 FOR NO KEY UPDATE"
    };
    let _: orch_types::Id = sqlx::query_scalar(lock)
        .bind(id)
        .fetch_one(&mut *blocker)
        .await?;
    let app = rig.server.app.clone();
    let pending = tokio::spawn(async move {
        server::epoch_test_connection(&app, &app.realtime.inner, &token).await
    });
    let reached = tokio::time::timeout(fixture::PATIENCE, async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock'
                  AND $1=ANY(pg_blocking_pids(pid)) AND lower(query) LIKE '%orch_authz_saw%')",
            )
            .bind(pid)
            .fetch_one(rig.pool.pool())
            .await?;
            if blocked {
                return Ok::<(), sqlx::Error>(());
            }
            if pending.is_finished() {
                return Err(sqlx::Error::RowNotFound);
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if !matches!(reached, Ok(Ok(()))) {
        blocker.rollback().await?;
        pending.abort();
        let _ = pending.await;
        return Err(
            format!("actual socket identity SHARE-row wait was not reached: {reached:?}").into(),
        );
    }
    // No revocation/authority SQL substitutes for the natural deadline crossing.
    rig.let_time_pass(chrono::Duration::seconds(3));
    blocker.rollback().await?;
    let result = tokio::time::timeout(fixture::PATIENCE, pending).await??;
    assert!(
        matches!(result, Err((code,_)) if code == CloseCode::Unauthenticated.code()),
        "the actual socket credential must be rechecked after its identity wait"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_login_checks_its_actual_session_deadline_after_identity_wait() -> Proof {
    raw_deadline_after_identity_wait(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn key_login_checks_its_actual_key_deadline_after_identity_wait() -> Proof {
    raw_deadline_after_identity_wait(true).await
}
