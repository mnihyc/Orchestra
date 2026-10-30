//! A document keeps its maker after the Owners row names somebody else first.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::documents::{
    Author, DocumentGet, NodeDocumentCreate, NodeDocumentCreateBody,
};
use places::{join, make_org, person};
use serde_json::json;
use support::World;

#[tokio::test]
async fn changing_the_first_owner_does_not_rewrite_the_person_who_made_the_document() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Acme", "acme").await;
    join(&world, org.base.id, org.base.id, &bea, "viewer", &ada).await;
    let made = world
        .app
        .call(NodeDocumentCreate, &[&org.base.id])
        .token(&ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Guide".into(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(PermissionPageUpdate, &[&"documents", &made.base.id])
        .token(&ada.token)
        .raw_body(
            &json!({"owners": {"add": [{"kind": "user", "id": bea.id()}], "first": bea.id()}}),
        )
        .send()
        .await
        .ok();
    let now = world
        .app
        .call(DocumentGet, &[&made.base.id])
        .token(&bea.token)
        .send()
        .await
        .ok();
    assert_eq!(now.base.owner.id, bea.id());
    let Author::User(creator) = now.base.created_by else {
        panic!("a person made the document");
    };
    assert_eq!(creator.id, ada.id());
    assert_eq!(now.created_at, made.created_at);
}
