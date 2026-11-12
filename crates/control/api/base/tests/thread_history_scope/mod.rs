//! The same real key and HTTP fixtures serve metadata-page and in-flight read controls.
#![allow(
    dead_code,
    reason = "each control module uses the shared fixtures it needs"
)]

use crate::support::World;

use orch_access_contract::key_scope::KeyScope;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::threads::ThreadHistoryListQuery;

use orch_api_core::middleware::authentication::API_TOKEN_PREFIX;

use orch_api_core::middleware::authentication::new_token;

use orch_api_core::middleware::authentication::token_hash;

use orch_api_core::testing::RawReply;

use orch_db::queries::identity::NewApiToken;

use orch_db::queries::identity::create_api_token_scoped;

use orch_types::Id;

use orch_types::new_id;

use serde_json::Value;

use serde_json::json;
pub type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Key {
    pub id: Id,
    pub secret: String,
}

pub fn rule(operation: &str, kind: &str, id: Id, reach: &str, action: &str) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":reach},
        "allow":[{"operation":operation,"actions":[action]}]})
}

fn scope(rules: Vec<Value>) -> Proof<Value> {
    let selected: KeyScope = serde_json::from_value(json!({"rules":rules}))?;
    assert!(selected.validate().is_empty());
    Ok(serde_json::to_value(selected)?)
}

pub async fn key(world: &World, user: Id, rules: Vec<Value>) -> Proof<Key> {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = world.now();
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Historical metadata control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            // Use the existing one-hour scene lifetime; the clock remains unchanged.
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&scope(rules)?),
    )
    .await?;
    Ok(Key { id, secret })
}

pub async fn narrow(world: &World, key: &Key, rules: Vec<Value>) -> Proof {
    let done = sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(scope(rules)?)
        .execute(&world.db.pool)
        .await?;
    assert_eq!(done.rows_affected(), 1);
    Ok(())
}

pub async fn request<E>(
    world: &World,
    endpoint: E,
    thread: Id,
    token: &str,
    query: &ThreadHistoryListQuery,
) -> RawReply
where
    E: Endpoint<Query = ThreadHistoryListQuery, Body = ()>,
{
    world
        .app
        .call(endpoint, &[&thread])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}

pub fn ok(reply: &RawReply) -> Proof<Value> {
    assert_eq!(
        reply.status,
        http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}

pub fn denied(reply: &RawReply) {
    assert!(!reply.status.is_success());
    let problem = reply.problem();
    assert_eq!(
        problem.code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    let body = String::from_utf8_lossy(&reply.raw);
    for content in [
        "Historical question",
        "Historical answer",
        "projection_revision",
        "env_generation",
    ] {
        assert!(
            !body.contains(content),
            "a refused metadata page disclosed {content}"
        );
    }
}
