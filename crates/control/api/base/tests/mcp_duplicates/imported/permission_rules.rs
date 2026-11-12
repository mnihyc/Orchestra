//! An accepted native copy keeps its sparse identity after response access ends.
use super::*;

#[tokio::test]
async fn a_completed_native_copy_keeps_its_receipt_without_restoring_a_closed_key() -> Proof {
    for ended in [false, true] {
        let (s, machine) = scene().await?;
        let scoped = key(
            &s.world,
            s.company.admin.id(),
            vec![
                rule("McpServerDuplicate", "mcp_server", s.source, "self", "read"),
                rule("McpServerDuplicate", "org", s.company.acme, "self", "write"),
                rule("McpServerDuplicate", "machine", machine, "self", "write"),
            ],
        )
        .await?;
        let waiting = start(&s, scoped.secret.clone(), None);
        let (native, authority) = queued(&s, &waiting).await?;
        let created: Id = serde_json::from_value(native.body["server_id"].clone())?;
        let before: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.import'")
                .fetch_one(&s.world.db.pool)
                .await?;
        if ended {
            assert!(
                orch_db::queries::identity::revoke_api_token(
                    &mut *s.world.conn().await,
                    scoped.id,
                    s.company.admin.id(),
                    s.world.now(),
                )
                .await?
            );
        }
        finish(&s, &native, &authority, "accepted-source-copy").await?;
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), waiting).await??;
        assert_eq!(id(&response)?, created);
        let value: Json = serde_json::from_slice(&response.raw)?;
        assert_eq!(value["projection"], "receipt");
        assert!(value.get("spec").is_none() && value.get("imported").is_none());
        let actual = rows::servers(&mut *s.world.conn().await, &[created])
            .await?
            .pop()
            .ok_or("the actual completed copy is absent")?;
        assert_eq!(actual.owner_id, Some(s.company.admin.id()));
        assert_eq!(actual.spec["imported"]["digest"], "accepted-source-copy");
        if ended {
            let refused = start(&s, scoped.secret.clone(), None);
            let refused =
                tokio::time::timeout(std::time::Duration::from_secs(30), refused).await??;
            assert_eq!(refused.status, http::StatusCode::UNAUTHORIZED);
            assert_eq!(
                refused.problem().code,
                orch_api_contract::common::ErrorCode::AuthInvalidToken
            );
        }
        let after: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.import'")
                .fetch_one(&s.world.db.pool)
                .await?;
        assert_eq!(after, before);
    }
    Ok(())
}
