//! API tokens cannot change passwords or manage browser sessions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::auth::{
    ApiTokenCreateBody, MePasswordChange, MePasswordChangeBody, MeSessionDelete, MeSessionList,
    MeTokenCreate,
};
use orch_api_contract::common::ErrorCode;
use places::{Who, person};
use support::{PASSWORD, World};

async fn token(world: &World, who: &Who) -> String {
    world
        .app
        .call(MeTokenCreate, &[])
        .token(&who.token)
        .body(&ApiTokenCreateBody {
            name: "Automation".to_owned(),
            scopes: Some(vec!["*".to_owned()]),
            node_id: None,
            expires_at: None,
            proof: orch_api_contract::auth::SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .expect("the first answer carries the secret")
}

#[tokio::test]
async fn an_api_token_cannot_revoke_its_users_browser_session() {
    let world = World::new().await;
    let who = person(&world, "reader").await;
    let token = token(&world, &who).await;
    let session = world
        .app
        .call(MeSessionList, &[])
        .token(&who.token)
        .send()
        .await
        .ok()
        .into_iter()
        .find(|session| session.current)
        .unwrap()
        .id;
    let problem = world
        .app
        .call(MeSessionDelete, &[&session])
        .token(&token)
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthSessionRequired);
    assert!(
        world
            .app
            .call(MeSessionList, &[])
            .token(&who.token)
            .send()
            .await
            .ok()
            .iter()
            .any(|live| live.id == session)
    );
}

#[tokio::test]
async fn an_api_token_cannot_change_its_users_password() {
    let world = World::new().await;
    let who = person(&world, "reader").await;
    let token = token(&world, &who).await;
    let problem = world
        .app
        .call(MePasswordChange, &[])
        .token(&token)
        .body(&MePasswordChangeBody {
            current_password: PASSWORD.to_owned(),
            new_password: "another long password".to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthSessionRequired);
}

#[tokio::test]
async fn an_api_token_may_list_sign_ins_without_being_a_browser_sign_in() {
    let world = World::new().await;
    let who = person(&world, "reader").await;
    let token = token(&world, &who).await;
    let listed = world
        .app
        .call(MeSessionList, &[])
        .token(&token)
        .send()
        .await
        .ok();
    assert!(!listed.is_empty());
    assert!(listed.iter().all(|sign_in| !sign_in.current));
}
