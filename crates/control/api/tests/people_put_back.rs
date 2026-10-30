//! Selective inverses are ordinary Edit operations fenced against their live snapshot, with repeatable receipts and old anchors kept.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, paragraph, put, put_after, rewrite};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentDiscussionCreate, DocumentDiscussionCreateBody, DocumentOp, DocumentOpsCreate,
    DocumentOpsCreateBody,
};
use orch_types::new_id;

#[tokio::test]
async fn an_editor_puts_one_block_back_preserving_a_colleagues_later_sibling_and_a_repeat_keeps_its_receipt()
 {
    let desk = Desk::new().await;
    let editor = desk.member("editor", "interactor").await;
    let doc = desk.make(&desk.ada, "Selective").await;
    let original = desk
        .paragraphs(&desk.ada, doc.base.id, &["Before", "Other work"])
        .await;
    let changed = desk
        .write(&editor, doc.base.id, vec![rewrite(&original[0], "After")])
        .await
        .ok();
    desk.write(
        &desk.ada,
        doc.base.id,
        vec![rewrite(&original[1], "A colleague's newer sibling")],
    )
    .await
    .ok();
    let live = desk.sync(&editor, doc.base.id).await;
    let inverse = DocumentOpsCreateBody {
        client_id: new_id().to_string(),
        expected_seq: Some(live.seq),
        ops: vec![rewrite(&changed.blocks[0], "Before")],
    };
    let answer = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&doc.base.id])
        .token(&editor.token)
        .body(&inverse)
        .send()
        .await
        .ok();
    assert_eq!(
        desk.words(&editor, doc.base.id).await,
        ["Before", "A colleague's newer sibling"]
    );
    desk.write(
        &desk.ada,
        doc.base.id,
        vec![put_after(paragraph("Later still"), original[1].id)],
    )
    .await
    .ok();
    let repeated = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&doc.base.id])
        .token(&editor.token)
        .body(&inverse)
        .send()
        .await
        .ok();
    assert_eq!(
        repeated.seq, answer.seq,
        "a retry is answered before the now-old snapshot fence"
    );
    assert_eq!(
        desk.words(&editor, doc.base.id).await,
        ["Before", "A colleague's newer sibling", "Later still"]
    );
}

#[tokio::test]
async fn a_remove_insert_inverse_that_loses_a_race_refuses_the_whole_batch() {
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Race").await;
    let original = desk
        .paragraphs(&desk.ada, doc.base.id, &["Selected", "Sibling"])
        .await;
    let live = desk.sync(&desk.ada, doc.base.id).await;
    let replacement = paragraph("Must not appear");
    let inverse = DocumentOpsCreateBody {
        client_id: new_id().to_string(),
        expected_seq: Some(live.seq),
        ops: vec![DocumentOp::Remove { id: original[0].id }, put(replacement)],
    };
    desk.write(
        &desk.ada,
        doc.base.id,
        vec![rewrite(
            &original[0],
            "Selected changed after the inverse was prepared",
        )],
    )
    .await
    .ok();
    let refused = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&doc.base.id])
        .token(&desk.ada.token)
        .body(&inverse)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        desk.words(&desk.ada, doc.base.id).await,
        ["Selected changed after the inverse was prepared", "Sibling"]
    );
}

#[tokio::test]
async fn returning_a_removed_subtree_uses_fresh_ids_and_does_not_move_a_historical_discussion() {
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Anchors").await;
    let mut parent = paragraph("Parent");
    let child = paragraph("Child");
    let old_parent = parent.id;
    let old_child = child.id;
    parent.children = Some(vec![child]);
    desk.write(&desk.ada, doc.base.id, vec![put(parent)])
        .await
        .ok();
    let discussion = desk
        .world
        .app
        .call(DocumentDiscussionCreate, &[&doc.base.id])
        .token(&desk.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: old_child,
            quote: None,
            content: desk::text("Old discussion"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    desk.write(
        &desk.ada,
        doc.base.id,
        vec![DocumentOp::Remove { id: old_parent }],
    )
    .await
    .ok();
    let live = desk.sync(&desk.ada, doc.base.id).await;
    let mut copy = paragraph("Parent");
    let new_parent = copy.id;
    let child = paragraph("Child");
    let new_child = child.id;
    copy.children = Some(vec![child]);
    let inverse = DocumentOpsCreateBody {
        client_id: new_id().to_string(),
        expected_seq: Some(live.seq),
        ops: vec![put(copy)],
    };
    desk.world
        .app
        .call(DocumentOpsCreate, &[&doc.base.id])
        .token(&desk.ada.token)
        .body(&inverse)
        .send()
        .await
        .ok();
    let now = desk.sync(&desk.ada, doc.base.id).await;
    assert!(
        now.blocks
            .iter()
            .filter(|block| !block.removed)
            .all(|block| [new_parent, new_child].contains(&block.id))
    );
    let old = now
        .discussions
        .iter()
        .find(|one| one.id == discussion.id)
        .unwrap();
    assert_ne!(old.block_id, Some(new_child));
    assert!(!old.anchored);
    let anchor: Option<orch_types::Id> =
        sqlx::query_scalar("SELECT block_id FROM document_discussions WHERE id=$1")
            .bind(discussion.id)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        anchor,
        Some(old_child),
        "the retained historical row keeps its original anchor"
    );
}
