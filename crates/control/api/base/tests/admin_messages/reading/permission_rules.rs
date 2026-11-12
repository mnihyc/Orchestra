//! Each actual message reader closes before delivering data when its key closes.
use super::*;
use futures::StreamExt as _;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn message_and_sent_list_bodies_keep_their_current_read_until_the_first_frame() -> Proof {
    for operation in ["MessageGet", "OrgMessageList", "AdminMessageList"] {
        for closed in [false, true] {
            let world = World::new().await;
            let c = admin_support::cast(&world).await;
            let system = operation == "AdminMessageList";
            let message = if system {
                match world
                    .app
                    .call(AdminMessageCreate, &[])
                    .token(&c.root.token)
                    .body(&body())
                    .send()
                    .await
                    .ok()
                {
                    AdminMessageCreateResponse::Full(message) => *message,
                    AdminMessageCreateResponse::Receipt(_) => {
                        return Err("the native sender has no full message".into());
                    }
                }
            } else {
                full_org_send(&world, &c.boss, c.acme).await
            };
            let owner = if system { c.root.id() } else { c.boss.id() };
            let rule = if system {
                system_read(operation)
            } else {
                org_rule(operation, c.acme, &["read"])
            };
            let reader = key(&world, owner, vec![rule]).await;
            let query = PageQuery {
                limit: Some(1),
                cursor: None,
            };
            let response = match operation {
                "MessageGet" => {
                    world
                        .app
                        .call(MessageGet, &[&message.id])
                        .token(&reader)
                        .send_response()
                        .await
                }
                "OrgMessageList" => {
                    world
                        .app
                        .call(OrgMessageList, &[&c.acme])
                        .token(&reader)
                        .query(&query)
                        .send_response()
                        .await
                }
                "AdminMessageList" => {
                    world
                        .app
                        .call(AdminMessageList, &[])
                        .token(&reader)
                        .query(&query)
                        .send_response()
                        .await
                }
                _ => unreachable!(),
            };
            assert_eq!(response.status(), StatusCode::OK);
            let audience = recipients(&world, message.id).await;
            if closed {
                let changed =
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
                        .bind(token_hash(&reader))
                        .bind(json!({"rules":[]}))
                        .execute(&world.db.pool)
                        .await?;
                assert_eq!(changed.rows_affected(), 1);
                let mut frames = response.into_body().into_data_stream();
                let first = tokio::time::timeout(std::time::Duration::from_secs(30), frames.next())
                    .await?
                    .ok_or("the closed body did not report its ended read")?;
                assert!(
                    first.is_err(),
                    "no original content frame is delivered after its Read closes"
                );
            } else {
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
                let value: Json = serde_json::from_slice(&bytes)?;
                if operation == "MessageGet" {
                    assert_eq!(value["id"], json!(message.id));
                    assert_eq!(value["body"], message.body);
                } else {
                    assert_eq!(value["items"][0]["id"], json!(message.id));
                    assert!(value["items"][0].get("body").is_none());
                }
            }
            assert_eq!(count(&world).await, 1);
            assert_eq!(recipients(&world, message.id).await, audience);
        }
    }
    Ok(())
}
