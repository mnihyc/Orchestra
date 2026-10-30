//! A token kept to one place cannot give a role outside that place.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::NodeRoleCreate;
use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate};
use orch_api_contract::common::ErrorCode;
use places::{join, make_org, make_project, person};
use serde_json::json;
use support::World;

#[tokio::test]
async fn a_token_kept_to_one_project_cannot_give_a_role_in_another() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let recipient = person(&world, "recipient").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    join(
        &world,
        org.base.id,
        org.base.id,
        &recipient,
        "viewer",
        &owner,
    )
    .await;
    let inside = make_project(&world, &owner, org.base.id, "Inside").await;
    let outside = make_project(&world, &owner, org.base.id, "Outside").await;
    let token = world
        .app
        .call(MeTokenCreate, &[])
        .token(&owner.token)
        .body(&ApiTokenCreateBody {
            name: "Inside only".to_owned(),
            scopes: Some(vec!["*".to_owned()]),
            node_id: Some(inside.base.id),
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
        .expect("the first answer carries the secret");
    let held = world
        .app
        .call(NodeRoleCreate, &[&inside.base.id])
        .token(&token)
        .raw_body(
            &json!({ "subject": { "kind": "user", "id": recipient.id() },
            "role": "viewer" }),
        )
        .send()
        .await
        .ok();
    assert_eq!(held.node.id, inside.base.id);
    let problem = world
        .app
        .call(NodeRoleCreate, &[&outside.base.id])
        .token(&token)
        .raw_body(
            &json!({ "subject": { "kind": "user", "id": recipient.id() },
            "role": "viewer" }),
        )
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
}
