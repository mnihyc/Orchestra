//! Creating a token for a service identity requires a browser sign-in.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{
    OrgServiceAccountCreate, OrgServiceAccountCreateBody, ServiceAccountTokenCreate,
};
use places::{make_org, person};
use support::World;

fn named(name: &str) -> ApiTokenCreateBody {
    ApiTokenCreateBody {
        name: name.to_owned(),
        scopes: Some(vec!["*".to_owned()]),
        node_id: None,
        expires_at: None,
        proof: orch_api_contract::auth::SensitiveProof {
            password: Some(support::PASSWORD.to_owned()),
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn a_token_cannot_create_a_token_for_a_service_identity() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let service = world
        .app
        .call(OrgServiceAccountCreate, &[&org.base.id])
        .token(&owner.token)
        .body(&OrgServiceAccountCreateBody {
            username: "build-service".to_owned(),
            display_name: "Build service".to_owned(),
        })
        .send()
        .await
        .ok();
    let browser = world
        .app
        .call(ServiceAccountTokenCreate, &[&service.user.id])
        .token(&owner.token)
        .body(&named("From the browser"))
        .send()
        .await
        .ok();
    assert_eq!(browser.info.name, "From the browser");
    let caller_token = world
        .app
        .call(MeTokenCreate, &[])
        .token(&owner.token)
        .body(&ApiTokenCreateBody {
            scopes: Some(vec!["members.manage".to_owned()]),
            ..named("People management")
        })
        .send()
        .await
        .ok()
        .token
        .expect("the first answer carries the secret");
    let reply = world
        .app
        .call(ServiceAccountTokenCreate, &[&service.user.id])
        .token(&caller_token)
        .body(&named("From a token"))
        .send()
        .await;
    assert_eq!(reply.status, 403, "a token cannot create another token");
    assert_eq!(reply.problem().code, ErrorCode::AuthSessionRequired);
}
