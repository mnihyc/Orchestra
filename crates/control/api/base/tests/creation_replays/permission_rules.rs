//! A native creation record never replaces current request authentication.
use super::*;

#[tokio::test]
async fn completed_creation_replays_require_current_raw_identity_and_an_active_account() -> Proof {
    for ended in ["expired", "revoked", "rotated", "suspended"] {
        let w = Desk::new().await;
        let actor = key(
            &w.world,
            w.ada.id(),
            vec![
                rule("NodeDocumentCreate", "org", w.org, "subtree", "write"),
                rule("NodeDocumentCreate", "org", w.org, "subtree", "read"),
            ],
        )
        .await?;
        let replay_key = new_id().to_string();
        let input = body("Current authentication protects this creation");
        let first = make(&w, &actor.secret, w.topic, &replay_key, &input).await;
        let created = id(&value(&first, 201));
        let repeated = make(&w, &actor.secret, w.topic, &replay_key, &input).await;
        assert_eq!(id(&value(&repeated, 201)), created);
        assert_eq!(documents(&w).await, 1);
        let original: Json =
            sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
                .bind(w.ada.id())
                .bind(&replay_key)
                .fetch_one(&w.world.db.pool)
                .await?;
        let native = orch_db::queries::documents::get(&mut *w.world.conn().await, created)
            .await?
            .ok_or("the created document is absent")?;
        match ended {
            "expired" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(w.world.now() - chrono::Duration::seconds(1))
                    .execute(&w.world.db.pool)
                    .await?;
            }
            "revoked" => {
                assert!(
                    orch_db::queries::identity::revoke_api_token(
                        &mut *w.world.conn().await,
                        actor.id,
                        w.ada.id(),
                        w.world.now()
                    )
                    .await?
                );
            }
            "rotated" => {
                sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(orch_api_core::middleware::authentication::token_hash(
                        "different-synthetic-bearer",
                    ))
                    .execute(&w.world.db.pool)
                    .await?;
            }
            "suspended" => {
                sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
                    .bind(w.ada.id())
                    .execute(&w.world.db.pool)
                    .await?;
            }
            _ => unreachable!(),
        }
        let refused = make(&w, &actor.secret, w.topic, &replay_key, &input).await;
        assert_eq!(refused.status, http::StatusCode::UNAUTHORIZED);
        assert_eq!(
            refused.problem().code,
            if ended == "suspended" {
                ErrorCode::AuthSuspended
            } else {
                ErrorCode::AuthInvalidToken
            }
        );
        assert!(!String::from_utf8_lossy(&refused.raw).contains("Current authentication protects"));
        assert_eq!(documents(&w).await, 1);
        let after: Json =
            sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
                .bind(w.ada.id())
                .bind(&replay_key)
                .fetch_one(&w.world.db.pool)
                .await?;
        assert_eq!(after, original);
        assert_eq!(
            orch_db::queries::documents::get(&mut *w.world.conn().await, created)
                .await?
                .as_ref(),
            Some(&native)
        );
    }
    Ok(())
}
