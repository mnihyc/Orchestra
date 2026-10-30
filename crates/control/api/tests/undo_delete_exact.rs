//! The exact Undo of a deletion as each case of it ends: what was deleted before it stays deleted, what it takes out of the archive has its ordinary line, and an older group after a newer deletion changes nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{Node, NodeGet};
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

async fn delete(world: &World, by: &Who, node: Id) -> Id {
    let deleted: NodeDeleted = world
        .app
        .call(NodeDelete, &[&node])
        .token(&by.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
        .ok();
    deleted.deletion_id.expect("a deletion says its group")
}

async fn undo(world: &World, by: &Who, node: Id, group: Id) -> orch_api_core::testing::Reply<Node> {
    world
        .app
        .call(NodeRestore, &[&node])
        .token(&by.token)
        .body(&NodeRestoreBody {
            undo_delete: Some(group),
            expected_deletion_id: None,
        })
        .send()
        .await
}

async fn gone(world: &World, by: &Who, node: Id) -> bool {
    let answer = world
        .app
        .call(NodeGet, &[&node])
        .token(&by.token)
        .send()
        .await;
    !answer.status.is_success()
}

async fn lines(world: &World, action: &str, node: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1 AND object_id = $2")
        .bind(action)
        .bind(node)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn an_undo_leaves_what_was_deleted_before_it_deleted_and_writes_the_ordinary_lines_of_what_it_reopens()
 {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web")
        .await
        .base
        .id;
    let open = make_topic(&world, &boss, web, "Open").await.base.id;
    let earlier = make_topic(&world, &boss, web, "Deleted earlier")
        .await
        .base
        .id;
    delete(&world, &boss, earlier).await;
    let group = delete(&world, &boss, web).await;

    let back: Node = undo(&world, &boss, web, group).await.ok();
    assert!(!archived(&back), "the project was open before its deletion");
    assert!(!archived(&read(&world, &boss, open).await));
    assert!(
        gone(&world, &boss, earlier).await,
        "a topic deleted before keeps its own deletion, and the Undo of another does not bring it back"
    );
    assert_eq!(lines(&world, "node.restored", web).await, 1);
    assert_eq!(
        lines(&world, "node.unarchived", web).await,
        1,
        "what the Undo reopens has its line"
    );
    assert_eq!(lines(&world, "node.unarchived", open).await, 1);
}

#[tokio::test]
async fn the_undo_of_an_older_deletion_after_a_newer_one_is_refused_and_changes_nothing() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web")
        .await
        .base
        .id;
    let older = delete(&world, &boss, web).await;
    world
        .app
        .call(NodeRestore, &[&web])
        .token(&boss.token)
        .body(&NodeRestoreBody::default())
        .send()
        .await
        .ok();
    let newer = delete(&world, &boss, web).await;
    assert_ne!(older, newer);
    let refused = undo(&world, &boss, web, older).await.problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        refused.extra["current_deletion_id"],
        serde_json::json!(newer)
    );
    assert!(gone(&world, &boss, web).await, "the newer deletion stands");
    // The newer deletion's own Undo still works.
    let back: Node = undo(&world, &boss, web, newer).await.ok();
    assert!(
        archived(&back),
        "it was archived (by the ordinary Bring it back) before the newer deletion"
    );
}

#[tokio::test]
async fn somebody_who_may_not_bring_it_back_undoes_nothing() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let outsider = person(&world, "outsider").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web")
        .await
        .base
        .id;
    let group = delete(&world, &boss, web).await;
    let refused = undo(&world, &outsider, web, group).await;
    assert!(!refused.status.is_success(), "{:?}", refused.status);
    assert!(gone(&world, &boss, web).await, "nothing came back");
    let back: Node = undo(&world, &boss, web, group).await.ok();
    assert!(!archived(&back));
}
