//! Original credential lifetime remains a bound at the native target-account lock.
use super::*;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

#[tokio::test]
async fn session_revocation_rechecks_the_original_key_after_the_native_person_row_wait() -> Proof {
    for expired in [false, true] {
        let world = World::new().await;
        let c = admin_support::cast(&world).await;
        let owner = c.ada.id();
        let (target, bearer) = session(&world, owner).await;
        // Keep the target valid across the clock cut that ends the one-hour revoker.
        let target_until = world.now() + chrono::Duration::hours(4);
        sqlx::query("UPDATE sessions SET expires_at=$2 WHERE id=$1")
            .bind(target)
            .bind(target_until)
            .execute(&world.db.pool)
            .await?;
        sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE session_id=$1")
            .bind(target)
            .bind(target_until)
            .execute(&world.db.pool)
            .await?;
        let (_, reader) = key(
            &world,
            c.root.id(),
            Some(vec![rule(
                "session",
                target,
                "AdminUserSessionDelete",
                &["manage"],
            )]),
        )
        .await;
        let mut blocker = world.db.pool.begin().await?;
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(owner)
            .fetch_one(&mut *blocker)
            .await?;
        let app = world.app.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(AdminUserSessionDelete, &[&owner, &target])
                .token(&reader);
            request.send_bytes().await
        });
        tokio::time::timeout(WAIT, async {
            loop {
                assert!(!pending.is_finished(), "the native target-account lock must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query='SELECT * FROM users WHERE id = $1 FOR UPDATE')")
                    .fetch_one(&world.db.pool).await?;
                if waiting { break Ok::<(), sqlx::Error>(()); }
                tokio::task::yield_now().await;
            }
        }).await??;
        if expired {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(WAIT, pending).await??;
        let target_after = world.app.call(MeGet, &[]).token(&bearer).send_bytes().await;
        let audits = admin_support::lines_of(&world, "auth.session_revoked").await;
        if expired {
            assert_eq!(reply.status, http::StatusCode::UNAUTHORIZED);
            assert_eq!(reply.problem().code, ErrorCode::AuthInvalidToken);
            assert_eq!(target_after.status, http::StatusCode::OK);
            assert!(audits.is_empty());
        } else {
            assert_eq!(reply.status, http::StatusCode::NO_CONTENT);
            assert_eq!(target_after.status, http::StatusCode::UNAUTHORIZED);
            assert_eq!(audits.len(), 1);
            assert_eq!(audits[0].object_id, Some(owner));
            assert_eq!(audits[0].details["sessions"], json!([target]));
        }
        if !expired {
            let original = world
                .app
                .call(MeGet, &[])
                .token(&c.ada.token)
                .send_bytes()
                .await;
            assert_eq!(original.status, http::StatusCode::OK);
        }
    }
    Ok(())
}
