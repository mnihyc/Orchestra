//! Finite message replies deliver the data checked before their headers. Later reads check the key again.
use super::*;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn message_and_sent_list_reads_finish_after_their_headers_and_refuse_a_new_read_when_the_key_closes()
-> Proof {
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
            let query = CollectionQuery {
                limit: Some(1),
                ..CollectionQuery::default()
            };
            let read = || async {
                match operation {
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
                }
            };
            let response = read().await;
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
                assert_eq!(read().await.status(), StatusCode::FORBIDDEN);
            }
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
            let value: Json = serde_json::from_slice(&bytes)?;
            if operation == "MessageGet" {
                assert_eq!(value["id"], json!(message.id));
                assert_eq!(value["body"], message.body);
            } else {
                assert_eq!(value["items"][0]["id"], json!(message.id));
                assert!(value["items"][0].get("body").is_none());
            }
            assert_eq!(count(&world).await, 1);
            assert_eq!(recipients(&world, message.id).await, audience);
        }
    }
    Ok(())
}
