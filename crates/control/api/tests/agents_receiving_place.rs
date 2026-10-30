//! Creating a live agent share must check the receiving place as well as the source page.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::common::ErrorCode;
use places::{make_org, make_project, person};
use serde_json::json;
use support::World;

#[tokio::test]
async fn a_page_change_cannot_share_into_a_place_the_sharer_cannot_drive() {
    let world = World::new().await;
    let giver = person(&world, "giver").await;
    let other = person(&world, "other").await;
    let source_org = make_org(&world, &giver, "Source", "source").await;
    let source = make_project(&world, &giver, source_org.base.id, "Source work").await;
    let target_org = make_org(&world, &other, "Receiver", "receiver").await;
    let receiver = make_project(&world, &other, target_org.base.id, "Private work").await;
    let problem = world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &source.base.id])
        .token(&giver.token)
        .raw_body(&json!({ "rows": [{ "permission": "doc.view", "add": [{
            "holder": { "kind": "agents", "id": receiver.base.id }
        }] }] }))
        .send()
        .await
        .problem();
    assert!(matches!(
        problem.code,
        ErrorCode::AuthzDenied | ErrorCode::RequestInvalid
    ));
}
