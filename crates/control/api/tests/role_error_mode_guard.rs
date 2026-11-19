//! A current mode conversion cannot retain an earlier credential's rich refusal projection.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use axum::body::{Body, to_bytes};
use bytes::Bytes;
use futures::stream;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::NodeRoleCreate;
use orch_api_contract::common::{ErrorCode, MAX_JSON_BYTES, Problem};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_db::queries::{identity, transaction_guard};
use orch_types::new_id;
use places::{join, make_org, make_project, person};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use support::World;
use tower::ServiceExt;

#[tokio::test]
async fn a_role_error_after_a_real_guard_wait_uses_current_modern_disclosure_after_legacy_authentication()
 {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let giver = person(&world, "giver").await;
    let recipient = person(&world, "recipient").await;
    let org = make_org(&world, &owner, "Role office", "role-office").await;
    join(
        &world,
        org.base.id,
        org.base.id,
        &giver,
        "developer",
        &owner,
    )
    .await;
    join(
        &world,
        org.base.id,
        org.base.id,
        &recipient,
        "viewer",
        &owner,
    )
    .await;
    let protected_name = "Existing role-place content absent from the request";
    let project = make_project(&world, &owner, org.base.id, protected_name).await;
    let body = json!({"subject":{"kind":"user","id":recipient.id()},"role":"admin"});

    // Establish the ordinary product's real rich error before asserting anything
    // about a key. If this premise fails, it is an unproved projection lead.
    let ordinary = world
        .app
        .call(NodeRoleCreate, &[&project.base.id])
        .token(&giver.token)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(ordinary.status, StatusCode::FORBIDDEN);
    assert_eq!(ordinary.problem().code, ErrorCode::AuthzDenied);
    assert!(
        String::from_utf8_lossy(&ordinary.raw).contains(protected_name),
        "the ordinary refusal must really contain the existing protected name"
    );
    assert!(!body.to_string().contains(protected_name));

    let secret = new_token(API_TOKEN_PREFIX);
    let token_id = new_id();
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: token_id,
            user_id: giver.id(),
            name: "Mode conversion refusal control",
            token_hash: &token_hash(&secret),
            scopes: &["*".into()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    let modern: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"project","id":project.base.id,"reach":"self"},
        "allow":[{"operation":"NodeRoleCreate","actions":["manage"]}]
    }]}))
    .unwrap();
    assert!(modern.validate().is_empty());
    let modern = serde_json::to_value(modern).unwrap();

    let mut blocker = world.db.pool.begin().await.unwrap();
    transaction_guard::lock_person(&mut blocker, giver.id())
        .await
        .unwrap()
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let body_polled = Arc::new(AtomicBool::new(false));
    let converted = Arc::new(AtomicBool::new(false));
    let recorded_body = Arc::clone(&body_polled);
    let recorded_conversion = Arc::clone(&converted);
    let pool = world.db.pool.clone();
    let actor = giver.id();
    let request=Request::builder().method("POST").uri(format!("/nodes/{}/roles",project.base.id))
        .header("authorization",format!("Bearer {secret}")).header("content-type","application/json")
        .body(Body::from_stream(stream::once(async move {
            // This stream is polled after ordinary Bearer authentication. Verify
            // that premise, then commit conversion before the action can SHARE-
            // lock its token. No token mutation is attempted while that guard waits.
            let authenticated:bool=sqlx::query_scalar("SELECT last_used_at IS NOT NULL AND operation_scope IS NULL FROM api_tokens WHERE id=$1 AND user_id=$2")
                .bind(token_id).bind(actor).fetch_one(&pool).await.unwrap();
            assert!(authenticated,"the same key must be authenticated in legacy mode before body conversion");
            sqlx::query("UPDATE api_tokens SET scopes=ARRAY[]::text[],operation_scope=$3 WHERE id=$1 AND user_id=$2")
                .bind(token_id).bind(actor).bind(modern).execute(&pool).await.unwrap();
            recorded_conversion.store(true,Ordering::SeqCst);
            recorded_body.store(true,Ordering::SeqCst);
            Ok::<_,std::io::Error>(Bytes::from(serde_json::to_vec(&body).unwrap()))
        }))).unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(world.app.app.clone());
    let mut action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    let patience = std::time::Duration::from_secs(30);
    let reached=tokio::time::timeout(patience,async {
        loop {
            if action.is_finished() {return false;}
            // Match the real guard statement's prefix (inside PostgreSQL's
            // bounded query text), rather than any unrelated lock wait.
            let actual_guard:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'WITH watched AS MATERIALIZED (%' AND query LIKE '%orch_authz_saw%')")
                .bind(blocker_pid).fetch_one(&world.db.pool).await.unwrap();
            if actual_guard {return true;}
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or(false);
    blocker.commit().await.unwrap();
    let response = match tokio::time::timeout(patience, &mut action).await {
        Ok(response) => response.unwrap(),
        Err(error) => {
            action.abort();
            let _ = action.await;
            Err::<axum::response::Response, _>(error)
                .expect("the released real action guard completes")
        }
    };
    let status = response.status();
    let wire = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    assert!(
        body_polled.load(Ordering::SeqCst) && converted.load(Ordering::SeqCst),
        "the post-authentication mode conversion must actually run"
    );
    assert!(
        reached,
        "the current-mode request must actually wait at its action identity guard before the row is released"
    );
    assert_eq!(status, ordinary.status);
    let problem: Problem = serde_json::from_slice(&wire).unwrap();
    assert_eq!(problem.code, ordinary.problem().code);
    assert!(
        !String::from_utf8_lossy(&wire).contains(protected_name),
        "current Manage-only rules must not release the ordinary rich error's existing content"
    );
    let grants:i64=sqlx::query_scalar("SELECT count(*) FROM node_roles WHERE node_id=$1 AND subject_kind='user' AND subject_id=$2 AND removed_at IS NULL")
        .bind(project.base.id).bind(recipient.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        grants, 0,
        "the ordinary maximum-role rule must remain unchanged"
    );
}
