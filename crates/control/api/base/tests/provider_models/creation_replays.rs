//! Model creation retries use current model data under the same operation's Provider Read.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::IDEMPOTENCY_HEADER;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::new_id;

async fn key(world: &World, user: Id, provider: Id, op: &str, actions: &[&str]) -> String {
    let scope:KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"provider","id":provider,"reach":"self"},"resources":["provider"],"allow":[{"operation":op,"actions":actions}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native model creation replay",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}
#[tokio::test]
async fn an_accepted_model_creation_replays_current_data_or_receipt_without_another_insert() {
    let world = world().await;
    let c = company(&world).await;
    let provider = provider(&world, &c.boss, c.acme, None).await.base.id;
    let body = ProviderModelCreateBody {
        model_name: "replayed-model".into(),
        display_name: Some("Original title".into()),
        parameters: None,
        groups: None,
    };
    let write = key(
        &world,
        c.boss.id(),
        provider,
        "ProviderModelCreate",
        &["manage"],
    )
    .await;
    let act = new_id();
    let first = world
        .app
        .call(ProviderModelCreate, &[&provider])
        .token(&write)
        .header(IDEMPOTENCY_HEADER, &act.to_string())
        .body(&body)
        .send()
        .await;
    assert_eq!(first.status, 200);
    let receipt = match first.ok() {
        orch_api_contract::resources::ProviderModelWriteResponse::Receipt(r) => r,
        other => panic!("a Manage-only key receives no existing model content: {other:?}"),
    };
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
        .bind(token_hash(&write))
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE provider_models SET display_name='Current title' WHERE id=$1")
        .bind(receipt.model_id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let read = key(
        &world,
        c.boss.id(),
        provider,
        "ProviderModelCreate",
        &["read"],
    )
    .await;
    let replay = world
        .app
        .call(ProviderModelCreate, &[&provider])
        .token(&read)
        .header(IDEMPOTENCY_HEADER, &act.to_string())
        .body(&body)
        .send()
        .await;
    assert_eq!(replay.status, 200);
    let model = replay.ok().into_full().unwrap();
    assert_eq!(model.id, receipt.model_id);
    assert_eq!(model.display_name, "Current title");
    let other = key(&world, c.boss.id(), provider, "ProviderGet", &["read"]).await;
    let replay = world
        .app
        .call(ProviderModelCreate, &[&provider])
        .token(&other)
        .header(IDEMPOTENCY_HEADER, &act.to_string())
        .body(&body)
        .send()
        .await;
    assert_eq!(replay.status, 200);
    assert_eq!(
        replay.ok(),
        orch_api_contract::resources::ProviderModelWriteResponse::Receipt(receipt)
    );
    let fresh = world
        .app
        .call(ProviderModelCreate, &[&provider])
        .token(&read)
        .header(IDEMPOTENCY_HEADER, &new_id().to_string())
        .body(&body)
        .send()
        .await;
    assert!(!fresh.status.is_success());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM provider_models WHERE provider_id=$1 AND model_name='replayed-model'",
    )
    .bind(provider)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}
