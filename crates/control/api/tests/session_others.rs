//! Ending other browsers is one personal act that retains this browser, other people and API tokens.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_api_contract::auth::{
    ApiTokenCreateBody, AuthRefresh, MeGet, MeSessionList, MeSessionOthersDelete, MeTokenCreate,
    SensitiveProof,
};
use orch_api_contract::common::ErrorCode;
use support::{PASSWORD, World, csrf};

#[tokio::test]
async fn sign_out_everywhere_else_keeps_exactly_this_session_other_people_and_personal_tokens() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let bob = world.person("bob").await;
    let current = world.sign_in(&ada).await;
    let first = world.sign_in(&ada).await;
    let second = world.sign_in(&ada).await;
    let bobs = world.sign_in(&bob).await;
    let token = world
        .app
        .call(MeTokenCreate, &[])
        .token(current.token())
        .body(&ApiTokenCreateBody {
            name: "automation".to_owned(),
            scopes: Some(vec!["*".to_owned()]),
            node_id: None,
            expires_at: None,
            proof: SensitiveProof {
                password: Some(PASSWORD.to_owned()),
                ..SensitiveProof::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .unwrap();
    let epoch: i64 = sqlx::query_scalar("SELECT epoch FROM authz_state WHERE id")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let kept = world
        .app
        .call(MeSessionOthersDelete, &[])
        .token(current.token())
        .send()
        .await
        .ok();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].current);
    for ended in [&first, &second] {
        assert_eq!(
            world
                .app
                .call(MeGet, &[])
                .token(ended.token())
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthInvalidToken
        );
        let (header, value) = csrf();
        assert_eq!(
            world
                .app
                .call(AuthRefresh, &[])
                .header("cookie", &ended.browser())
                .header(header, value)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthInvalidToken
        );
    }
    for good in [current.token(), bobs.token(), token.as_str()] {
        assert_eq!(
            world.app.call(MeGet, &[]).token(good).send().await.status,
            200
        );
    }
    assert_eq!(
        world
            .app
            .call(MeSessionList, &[])
            .token(bobs.token())
            .send()
            .await
            .ok()
            .len(),
        1
    );
    let after: i64 = sqlx::query_scalar("SELECT epoch FROM authz_state WHERE id")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        epoch, after,
        "a person's sessions do not invalidate everyone's authorization"
    );
    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action='auth.other_sessions_revoked' AND object_id=$1",
    )
    .bind(ada.id)
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sessions_ended"], 2);
    let again = world
        .app
        .call(MeSessionOthersDelete, &[])
        .token(current.token())
        .send()
        .await
        .ok();
    assert_eq!(again, kept, "repeating the act keeps this same browser");
}

#[tokio::test]
async fn an_api_token_cannot_revoke_its_persons_other_browsers() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let other = world.sign_in(&ada).await;
    let token = world
        .app
        .call(MeTokenCreate, &[])
        .token(signed.token())
        .body(&ApiTokenCreateBody {
            name: "automation".to_owned(),
            scopes: Some(vec!["*".to_owned()]),
            node_id: None,
            expires_at: None,
            proof: SensitiveProof {
                password: Some(PASSWORD.to_owned()),
                ..SensitiveProof::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .unwrap();
    assert_eq!(
        world
            .app
            .call(MeSessionOthersDelete, &[])
            .token(&token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSessionRequired
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(other.token())
            .send()
            .await
            .status,
        200
    );
}
