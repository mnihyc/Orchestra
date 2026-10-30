//! The ordinary edit_document tool copies current readable material into a fresh, independently owned document.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::json;
use shared::{Places, a_world_whose_agent_takes_tools, call, call_as, entry, ok, refused, share};

#[tokio::test]
async fn create_from_keeps_current_blocks_with_fresh_ids_and_retries_without_a_second_document() {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Copier").await;
    let source_place = world.topic("Sources").await;
    let destination = world.topic("Copies").await;
    let source = world
        .document_beside(
            source_place,
            "Source",
            &["Current source words", "Keep this sibling"],
        )
        .await;
    share(&world, "document", source, "live", "doc.view", "any", agent).await;
    share(
        &world,
        "node",
        destination,
        "live",
        "doc.edit",
        "any",
        agent,
    )
    .await;
    let listing = ok(call(
        &world,
        agent,
        OrchTool::ListShared,
        json!({"path":format!("id:{destination}")}),
    )
    .await);
    let path = format!(
        "{}Independent.md",
        entry(&listing, "docs")["path"].as_str().unwrap()
    );
    let args = json!({"command":"create", "path":path, "from":format!("id:{source}")});
    let made = ok(call_as(
        &world,
        agent,
        OrchTool::EditDocument,
        args.clone(),
        "copy-call",
    )
    .await);
    let id: Id = serde_json::from_value(made["id"].clone()).unwrap();
    assert_ne!(id, source);
    let original = world.blocks_of(source).await;
    let copied = world.blocks_of(id).await;
    assert_eq!(
        original.iter().map(|(_, text)| text).collect::<Vec<_>>(),
        copied.iter().map(|(_, text)| text).collect::<Vec<_>>()
    );
    assert!(
        copied
            .iter()
            .all(|(id, _)| original.iter().all(|(old, _)| id != old))
    );
    let (owner, by): (Id, Option<Id>) =
        sqlx::query_as("SELECT owner_id,created_by_thread FROM documents WHERE id=$1")
            .bind(id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!((owner, by), (world.ada, Some(agent)));
    world
        .change_block(source, "Current source words", "Changed later")
        .await;
    let again = ok(call_as(&world, agent, OrchTool::EditDocument, args, "copy-call").await);
    assert_eq!(again["id"], made["id"]);
    assert_eq!(world.blocks_of(id).await, copied);
    assert_eq!(world.count("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.created_by_agent'", id).await, 1);
    let both = call(
        &world,
        agent,
        OrchTool::EditDocument,
        json!({"command":"create","path":path,"from":format!("id:{source}"),"markdown":"other"}),
    )
    .await;
    assert_eq!(refused(&both).0, "invalid");
}

#[tokio::test]
async fn create_from_refuses_a_source_after_its_only_share_is_withdrawn() {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Copier").await;
    let destination = world.topic("Copies").await;
    let source = world
        .document_beside(world.root, "Unshared", &["No implicit source right"])
        .await;
    let source_entry = share(&world, "document", source, "live", "doc.view", "any", agent).await;
    share(
        &world,
        "node",
        destination,
        "live",
        "doc.edit",
        "any",
        agent,
    )
    .await;
    sqlx::query("UPDATE permission_entries SET removed_at=now() WHERE id=$1")
        .bind(source_entry)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let listing = ok(call(
        &world,
        agent,
        OrchTool::ListShared,
        json!({"path":format!("id:{destination}")}),
    )
    .await);
    let path = format!(
        "{}Refused.md",
        entry(&listing, "docs")["path"].as_str().unwrap()
    );
    let denied = call(
        &world,
        agent,
        OrchTool::EditDocument,
        json!({"command":"create","path":path,"from":format!("id:{source}")}),
    )
    .await;
    assert!(denied.error.is_some());
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM documents WHERE node_id=$1",
                destination
            )
            .await,
        0
    );
}
