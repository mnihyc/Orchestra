//! Distinct document folders, native permissions, complete operations and replay over HTTP.

#[path = "document_containers/archive_cases.rs"]
mod archive_cases;
mod desk;
mod places;
mod support;

use desk::{Desk, paragraph};
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PageChangeWhole, PermissionPageGet,
    PermissionPageUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::document_tree::*;
use orch_api_contract::documents::{
    DocumentGet, DocumentMove, DocumentMoveBody, NewDocumentAccess, NodeDocumentCopy,
    NodeDocumentCopyBody,
};
use orch_api_core::testing::Reply;
use orch_db::queries::notifications::upkeep;
use orch_types::{Id, new_id};
use places::Who;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn operation(
    command: DocumentTreeCommand,
    items: Vec<DocumentTreeItem>,
    to: Option<DocumentDirectory>,
) -> DocumentTreeOperationBody {
    DocumentTreeOperationBody {
        command,
        items,
        to,
        name: None,
        client_id: new_id().to_string(),
        version: None,
        deletion_plan: None,
    }
}

async fn act(
    desk: &Desk,
    who: &Who,
    body: &DocumentTreeOperationBody,
) -> Reply<DocumentTreeReceipt> {
    desk.world
        .app
        .call(DocumentTreeOperate, &[])
        .token(&who.token)
        .body(body)
        .send()
        .await
}

async fn folder(desk: &Desk, parent: Option<Id>, name: &str) -> TestResult<Id> {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([42; 32]),
    );
    let receipt = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: parent,
            name: name.into(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    match receipt.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => Ok(id),
        _ => Err("folder creation must return a real container".into()),
    }
}

async fn place(desk: &Desk, document: Id, parent: Id) {
    act(
        desk,
        &desk.ada,
        &operation(
            DocumentTreeCommand::Move,
            vec![DocumentTreeItem::Document(document)],
            Some(DocumentDirectory::Container(parent)),
        ),
    )
    .await
    .ok();
}

async fn preview(desk: &Desk, who: &Who, body: &DocumentTreeOperationBody) -> DocumentTreePlan {
    desk.world
        .app
        .call(DocumentTreeDeletePreview, &[])
        .token(&who.token)
        .body(body)
        .send()
        .await
        .ok()
}

async fn delete(desk: &Desk, item: DocumentTreeItem) -> DocumentTreeReceipt {
    let mut body = operation(DocumentTreeCommand::Delete, vec![item], None);
    body.deletion_plan = Some(preview(desk, &desk.ada, &body).await.stamp);
    act(desk, &desk.ada, &body).await.ok()
}

#[tokio::test]
async fn folders_are_navigable_typed_items_and_moves_preserve_text_identity_and_material()
-> TestResult {
    let desk = Desk::new().await;
    let parent = folder(&desk, None, "Research").await?;
    let child = folder(&desk, Some(parent), "Notes").await?;
    let document = desk.make(&desk.ada, "Plan").await.base.id;
    let blocks = desk
        .paragraphs(&desk.ada, document, &["Keep these words"])
        .await;
    place(&desk, document, child).await;
    let read = desk
        .world
        .app
        .call(DocumentTreeEntryGet, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentTreeEntryGetQuery {
            kind: DocumentTreeKind::Document,
            inspect_deleted: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(read.entries.len(), 1);
    assert_eq!(
        read.entries[0].entry.parent,
        DocumentDirectory::Container(child)
    );
    assert_eq!(read.ancestors.len(), 2);
    let listed = desk
        .world
        .app
        .call(DocumentTreeList, &[&desk.topic])
        .token(&desk.ada.token)
        .query(&DocumentTreeListQuery {
            parent_container_id: Some(child),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(
        listed.items[0].entry.item,
        DocumentTreeItem::Document(document)
    );
    assert_eq!(desk.page(&desk.ada, document).await, blocks);
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&parent])
            .token(&desk.ada.token)
            .send()
            .await
            .status,
        404,
        "a folder has no document editor payload"
    );
    let text_row: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents WHERE id=$1)")
        .bind(parent)
        .fetch_one(&desk.world.db.pool)
        .await?;
    assert!(
        !text_row,
        "folder identities are never ordinary text documents"
    );
    // The old move-to-node route means that node's docs root, even from a nested folder.
    desk.world
        .app
        .call(DocumentMove, &[&document])
        .token(&desk.ada.token)
        .body(&DocumentMoveBody {
            node_id: desk.topic,
        })
        .send()
        .await
        .ok();
    let parent: Option<Id> =
        sqlx::query_scalar("SELECT parent_container_id FROM documents WHERE id=$1")
            .bind(document)
            .fetch_one(&desk.world.db.pool)
            .await?;
    assert_eq!(parent, None);
    assert_eq!(desk.page(&desk.ada, document).await, blocks);
    Ok(())
}

#[tokio::test]
async fn a_direct_folder_grant_lists_only_its_descendants_and_redacts_unreadable_parent_names()
-> TestResult {
    let desk = Desk::new().await;
    let visitor = desk.stranger("visitor").await;
    let parent = folder(&desk, None, "Private research").await?;
    let shared = folder(&desk, Some(parent), "Shared notes").await?;
    let child = folder(&desk, Some(shared), "Details").await?;
    let document = desk.make(&desk.ada, "Visible plan").await.base.id;
    place(&desk, document, child).await;
    let sibling = desk.make(&desk.ada, "Secret sibling").await.base.id;
    place(&desk, sibling, parent).await;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &parent])
        .token(&desk.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &shared])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: visitor.id() }],
                choice: AccessChoice::FullView,
                options: None,
                expires_at: None,
                note: None,
                export: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let listed = desk
        .world
        .app
        .call(DocumentTreeList, &[&desk.topic])
        .token(&visitor.token)
        .query(&DocumentTreeListQuery {
            parent_container_id: Some(shared),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(
        listed.items[0].entry.item,
        DocumentTreeItem::Container(child)
    );
    assert!(listed.items[0].standing.read && !listed.items[0].standing.manage);
    let read = desk
        .world
        .app
        .call(DocumentTreeEntryGet, &[&document])
        .token(&visitor.token)
        .query(&DocumentTreeEntryGetQuery {
            kind: DocumentTreeKind::Document,
            inspect_deleted: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        read.entries[0].entry.item,
        DocumentTreeItem::Document(document)
    );
    let hidden = read
        .ancestors
        .iter()
        .find(|entry| entry.id == parent)
        .ok_or("structural parent absent")?;
    assert!(hidden.basename.is_none() && hidden.title.is_none() && !hidden.standing.read);
    let written = serde_json::to_string(&read)?;
    assert!(!written.contains("Private research") && !written.contains("Secret sibling"));
    assert_eq!(
        desk.world
            .app
            .call(DocumentTreeEntryGet, &[&sibling])
            .token(&visitor.token)
            .query(&DocumentTreeEntryGetQuery {
                kind: DocumentTreeKind::Document,
                inspect_deleted: None
            })
            .send()
            .await
            .status,
        404
    );
    let root = desk
        .world
        .app
        .call(DocumentTreeList, &[&desk.topic])
        .token(&visitor.token)
        .query(&DocumentTreeListQuery::default())
        .send()
        .await;
    assert_eq!(
        root.status, 403,
        "a folder grant is no node or project right"
    );
    let refused = desk
        .world
        .app
        .call(DocumentContainerDocumentCreate, &[&child])
        .token(&visitor.token)
        .body(&desk::body("Read is not Create"))
        .send()
        .await;
    assert_eq!(refused.status, 403);
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &shared])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: visitor.id() }],
                choice: AccessChoice::FullInteract,
                options: None,
                expires_at: None,
                note: None,
                export: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let made = desk
        .world
        .app
        .call(DocumentContainerDocumentCreate, &[&child])
        .token(&visitor.token)
        .body(&desk::body("Created inside"))
        .send()
        .await
        .ok();
    let entry = desk
        .world
        .app
        .call(DocumentTreeEntryGet, &[&made.base.id])
        .token(&visitor.token)
        .query(&DocumentTreeEntryGetQuery {
            kind: DocumentTreeKind::Document,
            inspect_deleted: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        entry.entries[0].entry.parent,
        DocumentDirectory::Container(child)
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentTreeList, &[&desk.topic])
            .token(&visitor.token)
            .query(&DocumentTreeListQuery::default())
            .send()
            .await
            .status,
        403,
        "creating in the folder still grants no surrounding-place access"
    );
    Ok(())
}

#[tokio::test]
async fn a_recursive_refusal_and_hidden_collision_leave_every_member_unchanged() -> TestResult {
    let desk = Desk::new().await;
    let developer = desk.member("dev", "developer").await;
    let parent = folder(&desk, None, "Research").await?;
    let document = desk.make(&desk.ada, "Private plan").await.base.id;
    place(&desk, document, parent).await;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document])
        .token(&desk.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let before: Vec<(Id, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT id,deleted_at FROM documents WHERE node_id=$1 ORDER BY id")
            .bind(desk.topic)
            .fetch_all(&desk.world.db.pool)
            .await?;
    let request = operation(
        DocumentTreeCommand::Delete,
        vec![DocumentTreeItem::Container(parent)],
        None,
    );
    let refused = desk
        .world
        .app
        .call(DocumentTreeDeletePreview, &[])
        .token(&developer.token)
        .body(&request)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(!serde_json::to_string(&refused)?.contains("Private plan"));
    let after: Vec<(Id, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT id,deleted_at FROM documents WHERE node_id=$1 ORDER BY id")
            .bind(desk.topic)
            .fetch_all(&desk.world.db.pool)
            .await?;
    assert_eq!(after, before);
    let collision = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&developer.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: Some(parent),
            name: "Private plan.md".into(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .problem();
    assert_eq!(
        collision.code,
        ErrorCode::ConflictExists,
        "hidden text and folders share one guarded namespace"
    );
    assert!(!serde_json::to_string(&collision)?.contains("Private plan"));
    Ok(())
}

#[tokio::test]
async fn deletion_preview_is_complete_and_restore_preserves_independent_deletions_and_promised_retention()
-> TestResult {
    let desk = Desk::new().await;
    let parent = folder(&desk, None, "Research").await?;
    let old = desk.make(&desk.ada, "Old").await.base.id;
    let current = desk.make(&desk.ada, "Current").await.base.id;
    place(&desk, old, parent).await;
    place(&desk, current, parent).await;
    delete(&desk, DocumentTreeItem::Document(old)).await;
    let mut request = operation(
        DocumentTreeCommand::Delete,
        vec![DocumentTreeItem::Container(parent)],
        None,
    );
    request.deletion_plan = Some(preview(&desk, &desk.ada, &request).await.stamp);
    let later = folder(&desk, Some(parent), "New child").await?;
    assert_eq!(
        act(&desk, &desk.ada, &request).await.problem().code,
        ErrorCode::ConflictStaleRevision
    );
    assert!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&parent])
            .token(&desk.ada.token)
            .send()
            .await
            .status
            == 200
    );
    let carried = desk
        .world
        .app
        .call(DocumentTreeDeletePreview, &[])
        .token(&desk.ada.token)
        .body(&request)
        .send()
        .await
        .problem();
    assert_eq!(
        carried.code,
        ErrorCode::RequestInvalid,
        "a fresh preview refuses a carried confirmation stamp"
    );
    request.deletion_plan = None;
    request.deletion_plan = Some(preview(&desk, &desk.ada, &request).await.stamp);
    assert_eq!(
        request
            .deletion_plan
            .as_ref()
            .map(|plan| plan.entries.len()),
        Some(4)
    );
    let removed = act(&desk, &desk.ada, &request).await.ok();
    assert_eq!(
        removed.entries.len(),
        3,
        "the independently deleted child keeps its original cohort"
    );
    let repeated = act(&desk, &desk.ada, &request).await.ok();
    assert_eq!(repeated.operation, removed.operation);
    desk.world.advance(chrono::Duration::days(2));
    let mut tx = desk.world.db.pool.begin().await?;
    let purged = upkeep::purge_soft_deleted(
        &mut tx,
        desk.world.now() - chrono::Duration::days(1),
        desk.world.now(),
    )
    .await?;
    tx.commit().await?;
    assert!(
        !purged
            .iter()
            .any(|entry| matches!(entry.table, "documents" | "document_containers")),
        "a shorter current purge cutoff cannot shorten the deletion's promised notice"
    );
    let restored = act(
        &desk,
        &desk.ada,
        &operation(
            DocumentTreeCommand::Restore,
            vec![DocumentTreeItem::Container(parent)],
            None,
        ),
    )
    .await
    .ok();
    assert_eq!(restored.entries.len(), 3);
    assert!(
        restored
            .entries
            .iter()
            .all(|entry| entry.deleted_at.is_none())
    );
    assert!(
        restored
            .entries
            .iter()
            .any(|entry| entry.item == DocumentTreeItem::Container(later))
    );
    let still_deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM documents WHERE id=$1")
            .bind(old)
            .fetch_one(&desk.world.db.pool)
            .await?;
    assert!(still_deleted);
    Ok(())
}

#[tokio::test]
async fn ordinary_copy_keeps_raw_title_private_choice_provenance_and_current_authorized_replay()
-> TestResult {
    let desk = Desk::new().await;
    let viewer = desk.member("reader", "viewer").await;
    let source = desk.make(&desk.ada, "Source").await.base.id;
    desk.write(
        &desk.ada,
        source,
        vec![desk::put(paragraph("Copied current text"))],
    )
    .await
    .ok();
    let body = NodeDocumentCopyBody {
        source_document_id: source,
        title: Some("Human / title".into()),
        access: Some(NewDocumentAccess::Private),
        client_id: "copy-once".into(),
    };
    let copy = desk
        .world
        .app
        .call(NodeDocumentCopy, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(copy.base.title, "Human / title");
    assert_eq!(
        desk.words(&desk.ada, copy.base.id).await,
        vec!["Copied current text"]
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&copy.base.id])
            .token(&viewer.token)
            .send()
            .await
            .status,
        403
    );
    let repeat = desk
        .world
        .app
        .call(NodeDocumentCopy, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(repeat.base.id, copy.base.id);
    assert_eq!(repeat.base.seq, copy.base.seq);
    let altered = NodeDocumentCopyBody {
        title: Some("Changed arguments".into()),
        ..body
    };
    assert_eq!(
        desk.world
            .app
            .call(NodeDocumentCopy, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&altered)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ConflictExists
    );
    let sources:Vec<Id>=sqlx::query_scalar("SELECT (details->>'copied_from')::uuid FROM audit_log WHERE action='document.created' AND object_id=$1")
        .bind(copy.base.id).fetch_all(&desk.world.db.pool).await?;
    assert_eq!(sources, vec![source]);
    Ok(())
}

#[tokio::test]
async fn same_parent_move_and_copy_refuse_before_audit_and_maximum_titles_remain_valid()
-> TestResult {
    let desk = Desk::new().await;
    let title = "x".repeat(200);
    let document = desk.make(&desk.ada, &title).await.base.id;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1")
        .bind(document)
        .fetch_one(&desk.world.db.pool)
        .await?;
    let request = operation(
        DocumentTreeCommand::Move,
        vec![DocumentTreeItem::Document(document)],
        Some(DocumentDirectory::NodeDocs(desk.topic)),
    );
    assert_eq!(
        act(&desk, &desk.ada, &request).await.problem().code,
        ErrorCode::RequestInvalid
    );
    let copy = DocumentTreeOperationBody {
        command: DocumentTreeCommand::Copy,
        ..request
    };
    assert_eq!(
        act(&desk, &desk.ada, &copy).await.problem().code,
        ErrorCode::ConflictExists
    );
    let ordinary = NodeDocumentCopyBody {
        source_document_id: document,
        title: None,
        access: None,
        client_id: "same-directory-copy".into(),
    };
    assert_eq!(
        desk.world
            .app
            .call(NodeDocumentCopy, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&ordinary)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ConflictExists,
        "the old API route uses the same exact namespace"
    );
    // A title is a display value: a document made by its title alone beside one of the same name
    // keeps both, the new one under a stable path of its own (its title's, with its identity), and
    // the longest title stays valid with that suffix.
    let beside = desk.make(&desk.ada, &title).await;
    assert_ne!(beside.base.id, document);
    assert_eq!(beside.base.title, title);
    assert_eq!(
        beside.location.basename,
        format!("{title}-{}.md", beside.base.id.simple())
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1")
        .bind(document)
        .fetch_one(&desk.world.db.pool)
        .await?;
    assert_eq!(after, before);
    let page = desk
        .world
        .app
        .call(PermissionPageGet, &[&"documents", &document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        page.owner.as_ref().map(|owner| owner.username.as_str()),
        Some("ada")
    );
    let read = desk
        .world
        .app
        .call(DocumentGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.base.title, title);
    for name in ["Docs", "CHAT.md", "x.FILES", "x.History"] {
        let refused = desk
            .world
            .app
            .call(DocumentContainerCreate, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&DocumentContainerCreateBody {
                parent_container_id: None,
                name: name.into(),
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
    }
    Ok(())
}

#[tokio::test]
async fn folder_creation_replays_the_same_identity_and_refuses_changed_arguments() -> TestResult {
    let desk = Desk::new().await;
    let body = DocumentContainerCreateBody {
        parent_container_id: None,
        name: "Research".into(),
        client_id: "one-folder".into(),
    };
    let first = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let again = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(first.operation, again.operation);
    assert_eq!(first.entries, again.entries);
    let changed = DocumentContainerCreateBody {
        name: "Other arguments".into(),
        ..body
    };
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerCreate, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&changed)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ConflictExists
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_containers WHERE node_id=$1")
            .bind(desk.topic)
            .fetch_one(&desk.world.db.pool)
            .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test]
async fn recipient_replacement_invalidates_a_preview_even_without_an_epoch_or_count_change()
-> TestResult {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let cai = desk.member("cai", "viewer").await;
    let parent = folder(&desk, None, "Research").await?;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &parent])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: bea.id() }],
                choice: AccessChoice::FullView,
                options: None,
                expires_at: None,
                note: None,
                export: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let mut request = operation(
        DocumentTreeCommand::Delete,
        vec![DocumentTreeItem::Container(parent)],
        None,
    );
    let plan = preview(&desk, &desk.ada, &request).await;
    let updated=sqlx::query("UPDATE permission_entries SET holder_id=$3 WHERE object_kind='document_container'
        AND object_id=$1 AND holder_kind='user' AND holder_id=$2 AND permission='doc.view' AND removed_at IS NULL")
        .bind(parent).bind(bea.id()).bind(cai.id()).execute(&desk.world.db.pool).await?;
    assert_eq!(updated.rows_affected(), 1);
    let fresh = preview(&desk, &desk.ada, &request).await;
    assert_eq!(fresh.stamp.entries, plan.stamp.entries);
    assert_eq!(fresh.stamp.authority_epoch, plan.stamp.authority_epoch);
    assert_eq!(fresh.losing, plan.losing);
    assert_ne!(
        fresh.stamp.authority_fingerprint,
        plan.stamp.authority_fingerprint
    );
    request.deletion_plan = Some(plan.stamp);
    assert_eq!(
        act(&desk, &desk.ada, &request).await.problem().code,
        ErrorCode::ConflictStaleRevision
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&parent])
            .token(&desk.ada.token)
            .send()
            .await
            .status,
        200
    );
    Ok(())
}

#[tokio::test]
async fn recursive_purge_collects_children_before_folders_and_releases_attached_bytes_once()
-> TestResult {
    use orch_api_contract::documents::{DocumentFileCreate, DocumentFileCreateQuery};
    use orch_api_contract::threads::FILENAME_HEADER;
    let desk = Desk::new().await;
    let parent = folder(&desk, None, "Research").await?;
    let child = folder(&desk, Some(parent), "Notes").await?;
    let document = desk.make(&desk.ada, "Plan").await.base.id;
    place(&desk, document, child).await;
    let file = desk
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, "notes.txt")
        .bytes(b"purge this copy")
        .send()
        .await
        .ok();
    let blob: Vec<u8> =
        sqlx::query_scalar("SELECT blob FROM document_files WHERE document_id=$1 AND id=$2")
            .bind(document)
            .bind(file.id)
            .fetch_one(&desk.world.db.pool)
            .await?;
    let before: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(&blob)
        .fetch_one(&desk.world.db.pool)
        .await?;
    assert_eq!(before, 1);
    delete(&desk, DocumentTreeItem::Container(parent)).await;
    desk.world.advance(chrono::Duration::days(31));
    let mut tx = desk.world.db.pool.begin().await?;
    let purged = upkeep::purge_soft_deleted(
        &mut tx,
        desk.world.now() - chrono::Duration::days(30),
        desk.world.now(),
    )
    .await?;
    tx.commit().await?;
    assert!(
        purged
            .iter()
            .any(|entry| entry.table == "documents" && entry.rows == 1)
    );
    assert!(
        purged
            .iter()
            .any(|entry| entry.table == "document_containers" && entry.rows == 2)
    );
    let after: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(&blob)
        .fetch_one(&desk.world.db.pool)
        .await?;
    assert_eq!(after, 0);
    let collected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_object_state WHERE object_kind='document_container'
        AND object_id=ANY($1) AND purged_at IS NOT NULL AND last_name IS NOT NULL",
    )
    .bind(vec![parent, child])
    .fetch_one(&desk.world.db.pool)
    .await?;
    assert_eq!(collected, 2);
    Ok(())
}
