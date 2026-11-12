//! Host cascade replay reads its real accepted parent, never repeats the reviewed children.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::IDEMPOTENCY_HEADER;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::{Id, new_id};
use serde_json::json;

async fn reader(world: &World, user: Id, host: Id, op: &str) -> String {
    let scope:KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"host","id":host,"reach":"self"},"resources":["host"],"allow":[{"operation":op,"actions":["read"]}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Current accepted Host deletion Read",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    token
}
#[tokio::test]
async fn a_host_deletion_creation_key_replays_its_parent_without_spent_cascade_authority() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let (host, _, _) = mixed_host(&world, &c).await;
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(host)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let plan = world
        .app
        .call(HostDeletePreflight, &[&host])
        .token(&c.viewer.token)
        .body(&HostDeletePreflightBody {
            environments: HostEnvironmentCascade::Remove,
            erase: Some(false),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(plan.executable);
    let body = HostDeleteCreateBody {
        preflight_id: plan.preflight_id,
        confirm: plan.name,
        notify: false,
        volumes: Vec::new(),
        data_confirmations: Vec::new(),
    };
    let key = new_id();
    let first = world
        .app
        .call(HostDeleteCreate, &[&host])
        .token(&c.viewer.token)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .body(&body)
        .send()
        .await;
    assert_eq!(first.status, 202);
    let job = first.ok().into_full().unwrap();
    let before:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM jobs),(SELECT count(*) FROM audit_log WHERE action='host.deleted' OR action='environment.updated')").fetch_one(&world.db.pool).await.unwrap();
    let saved: serde_json::Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(c.viewer.id())
            .bind(key.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(saved["outcome"]["id"], json!(job.id));
    assert!(saved["outcome"]["data"].is_null());
    sqlx::query("UPDATE sessions SET expires_at=$2 WHERE user_id=$1")
        .bind(c.viewer.id())
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let current = reader(&world, c.viewer.id(), host, "HostDeleteCreate").await;
    let replay = world
        .app
        .call(HostDeleteCreate, &[&host])
        .token(&current)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .body(&body)
        .send()
        .await;
    assert_eq!(replay.status, 202);
    assert_eq!(replay.ok().into_full().unwrap().id, job.id);
    let wrong = reader(&world, c.viewer.id(), host, "HostGet").await;
    let receipt = world
        .app
        .call(HostDeleteCreate, &[&host])
        .token(&wrong)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .body(&body)
        .send()
        .await;
    assert_eq!(receipt.status, 202);
    assert_eq!(
        receipt.ok(),
        HostDeleteCreateResponse::Receipt(HostDeleteCreateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            host_id: host,
            job_id: job.id
        })
    );
    let fresh = world
        .app
        .call(HostDeleteCreate, &[&host])
        .token(&current)
        .header(IDEMPOTENCY_HEADER, &new_id().to_string())
        .body(&body)
        .send()
        .await;
    assert!(!fresh.status.is_success());
    let after:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM jobs),(SELECT count(*) FROM audit_log WHERE action='host.deleted' OR action='environment.updated')").fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(after, before, "no second parent, child or acceptance");
}
