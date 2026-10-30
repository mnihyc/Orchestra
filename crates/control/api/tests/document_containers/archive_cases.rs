//! Generic Rule routes retain the same archive and retained-restoration rules as native pages.

use super::*;
use orch_api_contract::documents::{DocumentRestore, DocumentRestoreBody};
use orch_api_contract::nodes::NodeArchive;
use orch_types::Hash;

async fn state(desk: &Desk) -> TestResult<Hash> {
    let value:serde_json::Value=sqlx::query_scalar("SELECT jsonb_build_object(
        'folders',COALESCE((SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM document_containers c),'[]'::jsonb),
        'documents',COALESCE((SELECT jsonb_agg(to_jsonb(d) ORDER BY d.id) FROM documents d),'[]'::jsonb),
        'blocks',COALESCE((SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM document_blocks b),'[]'::jsonb),
        'files',COALESCE((SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM document_files f),'[]'::jsonb),
        'versions',COALESCE((SELECT jsonb_agg(to_jsonb(v) ORDER BY v.document_id,v.version) FROM document_versions v),'[]'::jsonb),
        'pages',COALESCE((SELECT jsonb_agg(to_jsonb(r) ORDER BY r.object_kind,r.object_id,r.page,r.profile,r.permission)
            FROM permission_rows r WHERE object_kind IN ('document','document_container')),'[]'::jsonb),
        'grants',COALESCE((SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM permission_entries e
            WHERE object_kind IN ('document','document_container')),'[]'::jsonb),
        'audit',COALESCE((SELECT jsonb_agg(to_jsonb(a) ORDER BY a.at,a.id) FROM audit_log a
            WHERE object_kind IN ('document','document_container')),'[]'::jsonb),
        'receipts',COALESCE((SELECT jsonb_agg(to_jsonb(o) ORDER BY o.actor_kind,o.actor_id,o.request_ref)
            FROM document_tree_operations o),'[]'::jsonb),
        'scopes',COALESCE((SELECT jsonb_agg(to_jsonb(s) ORDER BY s.node_id) FROM document_tree_scopes s),'[]'::jsonb))")
        .fetch_one(&desk.world.db.pool).await?;
    Ok(Hash::of(&serde_json::to_vec(&value)?))
}

#[tokio::test]
async fn generic_live_folder_and_document_moves_and_deletions_cannot_use_the_retained_archive_exception()
-> TestResult {
    let desk = Desk::new().await;
    let parent = folder(&desk, None, "Research").await?;
    let nested = folder(&desk, Some(parent), "Notes").await?;
    let document = desk.make(&desk.ada, "Plan").await.base.id;
    let blocks = desk
        .paragraphs(&desk.ada, document, &["Archive preserves this content."])
        .await;
    place(&desk, document, nested).await;
    let destination = desk.another_topic("Live destination").await;
    let items = [
        DocumentTreeItem::Container(parent),
        DocumentTreeItem::Document(document),
    ];
    let mut deletions = Vec::new();
    for item in items {
        let mut body = operation(DocumentTreeCommand::Delete, vec![item], None);
        body.deletion_plan = Some(preview(&desk, &desk.ada, &body).await.stamp);
        deletions.push(body);
    }
    desk.world
        .app
        .call(NodeArchive, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&places::archive_body(&desk.world, &desk.ada.token, desk.topic, true).await)
        .send()
        .await
        .ok();
    let before = state(&desk).await?;
    for (item, delete) in items.into_iter().zip(deletions) {
        let moving = operation(
            DocumentTreeCommand::Move,
            vec![item],
            Some(DocumentDirectory::NodeDocs(destination)),
        );
        let refused = act(&desk, &desk.ada, &moving).await.problem();
        assert_eq!(refused.code, ErrorCode::NodeArchived);
        let fresh = operation(DocumentTreeCommand::Delete, vec![item], None);
        let refused = desk
            .world
            .app
            .call(DocumentTreeDeletePreview, &[])
            .token(&desk.ada.token)
            .body(&fresh)
            .send()
            .await
            .problem();
        assert_eq!(
            refused.code,
            ErrorCode::NodeArchived,
            "a live archived selection receives no complete deletion plan"
        );
        assert_eq!(
            act(&desk, &desk.ada, &delete).await.problem().code,
            ErrorCode::NodeArchived,
            "a plan obtained before archive cannot turn a live entry into an AsItWas request"
        );
        assert_eq!(
            state(&desk).await?,
            before,
            "no placement, deletion, content, file, history, page, audit, receipt or scope stamp changes"
        );
    }
    assert_eq!(desk.page(&desk.ada, document).await, blocks);
    let mut copying = operation(
        DocumentTreeCommand::Copy,
        vec![DocumentTreeItem::Container(parent)],
        Some(DocumentDirectory::NodeDocs(destination)),
    );
    copying.name = Some("Readable archived copy".into());
    let copied = act(&desk, &desk.ada, &copying).await.ok();
    assert_eq!(
        copied.entries.len(),
        3,
        "copy still needs source Read, not source Manage or Edit"
    );
    assert!(
        copied
            .entries
            .iter()
            .all(|entry| entry.node == destination && entry.deleted_at.is_none())
    );
    let original: Id = sqlx::query_scalar("SELECT node_id FROM documents WHERE id=$1")
        .bind(document)
        .fetch_one(&desk.world.db.pool)
        .await?;
    assert_eq!(original, desk.topic);
    Ok(())
}

#[tokio::test]
async fn actual_retained_items_keep_parent_first_and_can_restore_to_a_live_destination_from_an_archived_anchor()
-> TestResult {
    let desk = Desk::new().await;
    let parent = folder(&desk, None, "Retained research").await?;
    let document = desk.make(&desk.ada, "Original").await.base.id;
    let blocks = desk
        .paragraphs(&desk.ada, document, &["The same retained content."])
        .await;
    place(&desk, document, parent).await;
    let removed = delete(&desk, DocumentTreeItem::Container(parent)).await;
    assert_eq!(removed.entries.len(), 2);
    let refused = desk
        .world
        .app
        .call(DocumentRestore, &[&document])
        .token(&desk.ada.token)
        .body(&DocumentRestoreBody {
            expected_deletion_id: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeRestoreParentFirst);
    let destination = desk.another_topic("Restored documents").await;
    desk.world
        .app
        .call(NodeArchive, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&places::archive_body(&desk.world, &desk.ada.token, desk.topic, true).await)
        .send()
        .await
        .ok();
    let restoring = operation(
        DocumentTreeCommand::Restore,
        vec![DocumentTreeItem::Container(parent)],
        Some(DocumentDirectory::NodeDocs(destination)),
    );
    let restored = act(&desk, &desk.ada, &restoring).await.ok();
    assert_eq!(restored.entries.len(), 2);
    assert!(
        restored
            .entries
            .iter()
            .all(|entry| entry.node == destination && entry.deleted_at.is_none())
    );
    assert!(
        restored
            .entries
            .iter()
            .any(|entry| entry.item == DocumentTreeItem::Document(document))
    );
    assert_eq!(desk.page(&desk.ada, document).await, blocks);
    Ok(())
}
