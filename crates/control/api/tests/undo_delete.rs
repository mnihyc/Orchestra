//! The Undo of a deletion: the deletion says its group, and undoing that group puts back how each thing stood,
//! open things open and archived things archived; another group is refused with nothing brought back, and the
//! ordinary Bring it back still brings things back archived.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{Node, NodeArchive};
use orch_api_contract::ownership::{
    NodeDelete, NodeDeleteBody, NodeDeleted, NodeRestore, NodeRestoreBody,
};
use orch_types::Id;
use places::{Who, make_org, make_project, make_topic, person, read};
use support::World;

fn archived(node: &Node) -> bool {
    match node {
        Node::Org(org) => org.base.archived,
        Node::Project(project) => project.base.archived,
        Node::Topic(topic) => topic.base.archived,
        Node::Thread(thread) => thread.base.archived,
    }
}

async fn delete(world: &World, by: &Who, node: Id) -> NodeDeleted {
    world
        .app
        .call(NodeDelete, &[&node])
        .token(&by.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn undoing_a_deletion_brings_back_what_was_open_open_and_what_was_archived_archived() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let open = make_topic(&world, &boss, web.base.id, "Open").await;
    let shelved = make_topic(&world, &boss, web.base.id, "Shelved").await;
    world
        .app
        .call(NodeArchive, &[&shelved.base.id])
        .token(&boss.token)
        .body(&places::archive_body(&world, &boss.token, shelved.base.id, true).await)
        .send()
        .await
        .ok();

    let group = delete(&world, &boss, web.base.id)
        .await
        .deletion_id
        .expect("a deletion says its group");
    // Another group is no Undo of this deletion: refused, and nothing comes back.
    let refused = world
        .app
        .call(NodeRestore, &[&web.base.id])
        .token(&boss.token)
        .body(&NodeRestoreBody {
            undo_delete: Some(orch_types::new_id()),
            expected_deletion_id: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        refused.extra["current_deletion_id"],
        serde_json::json!(group)
    );

    let back = world
        .app
        .call(NodeRestore, &[&web.base.id])
        .token(&boss.token)
        .body(&NodeRestoreBody {
            undo_delete: Some(group),
            expected_deletion_id: None,
        })
        .send()
        .await
        .ok();
    assert!(
        !archived(&back),
        "the project was open before it was deleted"
    );
    assert!(!archived(&read(&world, &boss, open.base.id).await));
    assert!(archived(&read(&world, &boss, shelved.base.id).await));

    // Brought back the ordinary way, a deletion leaves it archived, as Recently deleted says.
    delete(&world, &boss, web.base.id).await;
    let again = world
        .app
        .call(NodeRestore, &[&web.base.id])
        .token(&boss.token)
        .body(&NodeRestoreBody::default())
        .send()
        .await
        .ok();
    assert!(archived(&again));
}
