//! Human document folders use native pages, retained authority and scoped metadata sockets.

mod desk;
mod places;
mod realtime_support;
mod rig;
mod support;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use desk::Desk;
use orch_api_contract::{
    access::{AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate},
    auth::{AdminModeSetBody, MeAdminModeSet},
    common::ErrorCode,
    document_tree::*,
    documents::{
        DeletedFilter, DocumentDiscussionCreate, DocumentDiscussionCreateBody, DocumentGet,
        DocumentSeen, DocumentSeenBody, DocumentSyncGet, DocumentSyncGetQuery, NodeDocumentList,
        NodeDocumentListQuery,
    },
    nodes::{DeletedInspectionQuery, NodeArchive},
    share::{
        ShareAgentCreate, ShareDocumentsLevel, ShareParts, ShareTo, ShareToThread, ShareWhat,
        ShareWhatDocument, ShareWithAgents, ThreadSharedGet,
    },
    threads::ShareMode,
};
use orch_api_core::{routes::resources::sealing, testing::Reply};
use orch_db::{Db, secrets::MasterKey};
use orch_realtime_contract::feeds::Feed;
use orch_settings::Target;
use orch_types::{Id, Json, Time, new_id};
use places::Who;
use serde_json::json;
use std::{collections::HashSet, time::Duration};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn keys() {
    sealing::install_master_key(MasterKey::new([42; 32]));
}

fn checked<T>(reply: Reply<T>, phase: &str) -> Proof<T> {
    match reply.body {
        Ok(value) => Ok(value),
        Err(problem) => {
            // This target uses only synthetic requests. Bound validation evidence to eight short
            // field faults; never print a bearer token, the request/response body or arbitrary extras.
            let fields:Vec<_>=problem.extra.get("errors").and_then(Json::as_array).into_iter().flatten().take(8)
                .map(|field|json!({"path":field["path"].as_str().unwrap_or("").chars().take(160).collect::<String>(),
                    "message":field["message"].as_str().unwrap_or("").chars().take(160).collect::<String>()})).collect();
            let reason = problem
                .extra
                .get("reason")
                .and_then(Json::as_str)
                .filter(|reason| matches!(*reason, "tools_off" | "no_tools" | "nothing_to_read"))
                .unwrap_or("unclassified");
            Err(format!(
                "{phase}: status={} code={:?} reason={reason} fields={fields:?}",
                problem.status, problem.code
            )
            .into())
        }
    }
}

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
    by: &Who,
    body: &DocumentTreeOperationBody,
) -> Reply<DocumentTreeReceipt> {
    desk.world
        .app
        .call(DocumentTreeOperate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

async fn folder(desk: &Desk, parent: Option<Id>, name: &str) -> Proof<Id> {
    let made = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: parent,
            name: name.to_owned(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    match made.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => Ok(id),
        _ => Err("folder receipt has no container".into()),
    }
}

async fn list(
    desk: &Desk,
    by: &Who,
    parent: Option<Id>,
    deleted: bool,
    cursor: Option<String>,
    limit: Option<u16>,
) -> Reply<DocumentTreePage> {
    desk.world
        .app
        .call(DocumentTreeList, &[&desk.topic])
        .token(&by.token)
        .query(&DocumentTreeListQuery {
            parent_container_id: parent,
            deleted: deleted.then_some(DeletedFilter::Only),
            cursor,
            limit,
        })
        .send()
        .await
}

async fn give(desk: &Desk, folder: Id, holder: HolderInput, choice: AccessChoice) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &folder])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![holder],
                choice,
                export: Some(false),
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
}

async fn remove(desk: &Desk, item: DocumentTreeItem) -> DocumentTreeReceipt {
    let mut request = operation(DocumentTreeCommand::Delete, vec![item], None);
    request.deletion_plan = Some(
        desk.world
            .app
            .call(DocumentTreeDeletePreview, &[])
            .token(&desk.ada.token)
            .body(&request)
            .send()
            .await
            .ok()
            .stamp,
    );
    act(desk, &desk.ada, &request).await.ok()
}

#[tokio::test]
async fn direct_folder_navigation_redacts_ancestors_and_creation_matches_the_actual_native_decision()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let visitor = desk.stranger("visitor").await;
    let private = folder(&desk, None, "Hidden parent").await?;
    let shared = folder(&desk, Some(private), "Shared notes").await?;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &private])
        .token(&desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    give(
        &desk,
        shared,
        HolderInput::User { id: visitor.id() },
        AccessChoice::FullView,
    )
    .await;
    let shown = list(&desk, &visitor, Some(shared), false, None, None)
        .await
        .ok();
    assert!(shown.items.is_empty());
    assert!(!shown.directory.can_create);
    assert!(shown.directory.node.redacted && shown.directory.node.name.is_none());
    let hidden = shown
        .directory
        .ancestors
        .iter()
        .find(|ancestor| ancestor.id == private)
        .ok_or("missing structural ancestor")?;
    assert!(hidden.title.is_none() && hidden.basename.is_none());
    assert_eq!(
        list(&desk, &visitor, None, false, None, None)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerDocumentCreate, &[&shared])
            .token(&visitor.token)
            .body(&desk::body("Read alone"))
            .send()
            .await
            .status,
        403
    );
    give(
        &desk,
        shared,
        HolderInput::User { id: visitor.id() },
        AccessChoice::FullInteract,
    )
    .await;
    assert!(
        list(&desk, &visitor, Some(shared), false, None, None)
            .await
            .ok()
            .directory
            .can_create
    );
    let made = desk
        .world
        .app
        .call(DocumentContainerDocumentCreate, &[&shared])
        .token(&visitor.token)
        .body(&desk::body("A real child"))
        .send()
        .await
        .ok();
    assert_eq!(made.location.parent, DocumentDirectory::Container(shared));
    assert!(
        made.location
            .ancestors
            .iter()
            .find(|ancestor| ancestor.id == private)
            .is_some_and(|ancestor| ancestor.title.is_none())
    );
    desk.world
        .set_at(Target::Node(desk.topic), "docs.enabled", json!(false))
        .await;
    let disabled = list(&desk, &visitor, Some(shared), false, None, None)
        .await
        .ok();
    assert!(
        disabled
            .directory
            .folder
            .as_ref()
            .is_some_and(|folder| folder.standing.write)
    );
    assert!(
        !disabled.directory.can_create,
        "a held Create permission does not override docs.enabled"
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerDocumentCreate, &[&shared])
            .token(&visitor.token)
            .body(&desk::body("Disabled"))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::DocsDisabled
    );
    desk.world
        .set_at(Target::Node(desk.topic), "docs.enabled", json!(true))
        .await;
    let archive = places::archive_body(&desk.world, &desk.ada.token, desk.topic, true).await;
    desk.world
        .app
        .call(NodeArchive, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&archive)
        .send()
        .await
        .ok();
    assert!(
        !list(&desk, &visitor, Some(shared), false, None, None)
            .await
            .ok()
            .directory
            .can_create
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerDocumentCreate, &[&shared])
            .token(&visitor.token)
            .body(&desk::body("Archived"))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NodeArchived
    );
    Ok(())
}

#[tokio::test]
async fn retained_folders_and_nested_documents_survive_reload_with_original_promises_and_read_audit()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let viewer = desk.member("viewer", "viewer").await;
    let parent = folder(&desk, None, "Parent").await?;
    let child = folder(&desk, Some(parent), "Nested").await?;
    let doc = desk.make(&desk.ada, "Earlier child").await.base.id;
    act(
        &desk,
        &desk.ada,
        &operation(
            DocumentTreeCommand::Move,
            vec![DocumentTreeItem::Document(doc)],
            Some(DocumentDirectory::Container(child)),
        ),
    )
    .await
    .ok();
    let earlier = remove(&desk, DocumentTreeItem::Document(doc)).await;
    desk.world.advance(chrono::Duration::seconds(2));
    let removed = remove(&desk, DocumentTreeItem::Container(parent)).await;
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&parent])
            .token(&desk.ada.token)
            .send()
            .await
            .status,
        404
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&doc])
            .token(&desk.ada.token)
            .send()
            .await
            .status,
        404
    );
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1",
    )
    .bind(parent)
    .fetch_one(&desk.world.db.pool)
    .await?;
    let inspected = desk
        .world
        .app
        .call(DocumentContainerGet, &[&parent])
        .token(&desk.ada.token)
        .query(&DeletedInspectionQuery {
            inspect_deleted: Some(true),
        })
        .send()
        .await
        .ok();
    let context = inspected
        .directory
        .as_ref()
        .ok_or("retained directory context")?;
    assert!(!context.can_create);
    assert!(
        context
            .inspection
            .as_ref()
            .is_some_and(|inspection| inspection.readonly
                && Some(inspection.purge_at.get()) == removed.restore_until)
    );
    let after:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1 AND object_kind='document_container'")
        .bind(parent).fetch_one(&desk.world.db.pool).await?;
    assert_eq!(
        after,
        before + 1,
        "one explicit metadata inspection uses the extraordinary-read audit"
    );
    let inventory = list(&desk, &desk.ada, None, true, None, None).await.ok();
    let ids: HashSet<_> = inventory.items.iter().map(|row| row.entry.item).collect();
    assert_eq!(
        ids,
        HashSet::from([
            DocumentTreeItem::Container(parent),
            DocumentTreeItem::Container(child),
            DocumentTreeItem::Document(doc)
        ])
    );
    assert!(
        inventory
            .items
            .iter()
            .all(|row| row.document.is_none() && row.participating.is_none()),
        "restoration-authorized metadata is not an ordinary text-document read"
    );
    let old = inventory
        .items
        .iter()
        .find(|row| row.entry.item == DocumentTreeItem::Document(doc))
        .ok_or("nested retained document absent after reload")?;
    assert_eq!(old.entry.deletion, Some(earlier.operation));
    assert_eq!(
        old.inspection
            .as_ref()
            .map(|inspection| inspection.purge_at.get()),
        earlier.restore_until
    );
    let invisible = list(&desk, &viewer, None, true, None, None).await.ok();
    assert!(invisible.items.is_empty());
    assert!(!serde_json::to_string(&invisible)?.contains("Earlier child"));
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&parent])
            .token(&viewer.token)
            .query(&DeletedInspectionQuery {
                inspect_deleted: Some(true)
            })
            .send()
            .await
            .status,
        404
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
    assert!(restored.entries.iter().any(
        |entry| entry.item == DocumentTreeItem::Container(parent) && entry.deleted_at.is_none()
    ));
    assert!(
        list(&desk, &desk.ada, None, true, None, None)
            .await
            .ok()
            .items
            .iter()
            .any(|row| row.entry.item == DocumentTreeItem::Document(doc)),
        "restoring the newer parent does not restore an independently deleted child"
    );
    act(
        &desk,
        &desk.ada,
        &operation(
            DocumentTreeCommand::Restore,
            vec![DocumentTreeItem::Document(doc)],
            None,
        ),
    )
    .await
    .ok();
    let reloaded = desk
        .world
        .app
        .call(DocumentGet, &[&doc])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(reloaded.base.id, doc);
    assert_eq!(
        reloaded.location.parent,
        DocumentDirectory::Container(child)
    );
    Ok(())
}

#[tokio::test]
async fn overdue_retained_metadata_is_not_actual_purge_and_restore_keeps_its_original_deadline()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let parent = folder(&desk, None, "Retained").await?;
    let removed = remove(&desk, DocumentTreeItem::Container(parent)).await;
    let until = removed.restore_until.ok_or("delete promise")?;
    desk.world
        .advance(until - desk.world.now() + chrono::Duration::days(1));
    let inspected = desk
        .world
        .app
        .call(DocumentContainerGet, &[&parent])
        .token(&desk.ada.token)
        .query(&DeletedInspectionQuery {
            inspect_deleted: Some(true),
        })
        .send()
        .await
        .ok();
    assert_eq!(
        inspected
            .directory
            .and_then(|directory| directory.inspection)
            .map(|inspection| inspection.purge_at.get()),
        Some(until)
    );
    let restore = operation(
        DocumentTreeCommand::Restore,
        vec![DocumentTreeItem::Container(parent)],
        None,
    );
    let before = list(&desk, &desk.ada, None, true, None, None).await.ok();
    // The typed domain reports a retained but expired cohort as a conflict. Only actual collection
    // removes it; refusal must not clear the tombstone, change its promise or save a successful act.
    assert_eq!(
        act(&desk, &desk.ada, &restore).await.problem().code,
        ErrorCode::ConflictExists
    );
    let after = list(&desk, &desk.ada, None, true, None, None).await.ok();
    assert_eq!(after.items, before.items);
    assert_eq!(
        after.items[0]
            .inspection
            .as_ref()
            .map(|inspection| inspection.purge_at.get()),
        Some(until)
    );
    let receipt:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM document_tree_operations WHERE actor_kind='person' AND actor_id=$1 AND request_ref=$2)")
        .bind(desk.ada.id()).bind(&restore.client_id).fetch_one(&desk.world.db.pool).await?;
    assert!(
        !receipt,
        "an expired restoration cannot leave a committed success receipt"
    );
    let mut tx = desk.world.app.app.db.begin().await?;
    let purged = orch_db::queries::notifications::upkeep::purge_soft_deleted(
        &mut tx,
        desk.world.now(),
        desk.world.now(),
    )
    .await?;
    tx.commit().await?;
    assert!(
        purged
            .iter()
            .any(|item| item.table == "document_containers" && item.rows == 1)
    );
    assert!(
        list(&desk, &desk.ada, None, true, None, None)
            .await
            .ok()
            .items
            .is_empty()
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&parent])
            .token(&desk.ada.token)
            .query(&DeletedInspectionQuery {
                inspect_deleted: Some(true)
            })
            .send()
            .await
            .status,
        404
    );
    Ok(())
}

async fn private_headers(desk: &Desk, count: usize) -> Proof<Vec<Id>> {
    // Native structural triggers remain enabled. Only empty fixture headers/pages are batched;
    // this test measures authorized read progress, not repeated HTTP creation or content batching.
    let ids: Vec<_> = (0..count).map(|_| new_id()).collect();
    let titles: Vec<_> = (0..count).map(|n| format!("a-hidden-{n:04}")).collect();
    let grants: Vec<_> = (0..count).map(|_| new_id()).collect();
    let mut tx = desk.world.app.app.db.begin().await?;
    sqlx::query(
        "INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title,created_at,updated_at)
        SELECT x.id,$3,$4,$5,$5,x.title,$6,$6 FROM unnest($1::uuid[],$2::text[]) x(id,title)",
    )
    .bind(&ids)
    .bind(&titles)
    .bind(desk.topic)
    .bind(desk.org)
    .bind(desk.ada.id())
    .bind(desk.world.now())
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO permission_rows(object_kind,object_id,page,profile,permission,inherit,updated_by,updated_at)
        SELECT 'document',x,'live','any','doc.view',false,$2,$3 FROM unnest($1::uuid[]) x")
        .bind(&ids).bind(desk.ada.id()).bind(desk.world.now()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO permission_entries(id,object_kind,object_id,page,profile,permission,holder_kind,holder_id,created_by,created_at,source)
        SELECT x.entry,'document',x.object,'live','any','access.own','user',$3,$3,$4,'creation'
        FROM unnest($1::uuid[],$2::uuid[]) x(entry,object)")
        .bind(grants).bind(&ids).bind(desk.ada.id()).bind(desk.world.now()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(ids)
}

#[tokio::test]
async fn sealed_cursor_progresses_past_unreadable_runs_and_cannot_cross_authority_or_membership()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let viewer = desk.member("viewer", "viewer").await;
    let hidden = private_headers(&desk, 511).await?;
    let visible = desk.make(&desk.ada, "z-visible").await.base.id;
    let first = list(&desk, &viewer, None, false, None, Some(20)).await.ok();
    assert!(first.items.is_empty());
    let cursor = first
        .next_cursor
        .clone()
        .ok_or("an empty authorized scan must still continue")?;
    assert!(!cursor.contains("a-hidden"));
    assert!(
        serde_json::from_slice::<Json>(&URL_SAFE_NO_PAD.decode(&cursor)?).is_err(),
        "opaque progress is sealed, not a base64 disclosure of scan metadata"
    );
    let last = list(&desk, &viewer, None, false, Some(cursor.clone()), Some(20))
        .await
        .ok();
    assert_eq!(
        last.items
            .iter()
            .map(|row| row.entry.item)
            .collect::<Vec<_>>(),
        [DocumentTreeItem::Document(visible)]
    );
    assert!(last.next_cursor.is_none());
    assert_eq!(
        list(&desk, &desk.ada, None, false, Some(cursor.clone()), None)
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    assert_eq!(
        list(&desk, &viewer, None, true, Some(cursor.clone()), None)
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    let mut all = Vec::new();
    let mut next = None;
    loop {
        let page = list(&desk, &desk.ada, None, false, next, Some(127))
            .await
            .ok();
        all.extend(page.items.into_iter().map(|row| row.entry.item));
        next = page.next_cursor;
        if next.is_none() {
            break;
        }
        assert!(all.len() <= 512);
    }
    let expected: HashSet<_> = hidden
        .into_iter()
        .chain([visible])
        .map(DocumentTreeItem::Document)
        .collect();
    assert_eq!(all.len(), 512);
    assert_eq!(all.into_iter().collect::<HashSet<_>>(), expected);
    let mut conn = desk.world.conn().await;
    let membership =
        orch_db::queries::documents::containers::scope_seq(&mut conn, desk.topic).await?;
    let epoch = orch_db::queries::access::epoch(&mut conn).await?;
    drop(conn);
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &visible])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: viewer.id() }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: Some(Time::new(desk.world.now() + chrono::Duration::seconds(1))),
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let mut conn = desk.world.conn().await;
    assert_eq!(
        orch_db::queries::documents::containers::scope_seq(&mut conn, desk.topic).await?,
        membership
    );
    assert!(orch_db::queries::access::epoch(&mut conn).await? > epoch);
    drop(conn);
    assert_eq!(
        list(&desk, &viewer, None, false, Some(cursor), None)
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    let timed = list(&desk, &viewer, None, false, None, None).await.ok();
    desk.world.advance(chrono::Duration::seconds(2));
    assert_eq!(
        list(&desk, &viewer, None, false, timed.next_cursor, None)
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    let current = list(&desk, &viewer, None, false, None, None).await.ok();
    folder(&desk, None, "Membership changed").await?;
    let stale = list(&desk, &viewer, None, false, current.next_cursor, None)
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra.get("restart"), Some(&json!(true)));
    Ok(())
}

#[tokio::test]
async fn copy_move_rename_answers_and_editor_reload_use_actual_current_folder_ids() -> Proof {
    keys();
    let desk = Desk::new().await;
    let source = folder(&desk, None, "Source").await?;
    let destination = folder(&desk, None, "Destination").await?;
    let doc = desk.make(&desk.ada, "Plan").await.base.id;
    act(
        &desk,
        &desk.ada,
        &operation(
            DocumentTreeCommand::Move,
            vec![DocumentTreeItem::Document(doc)],
            Some(DocumentDirectory::Container(source)),
        ),
    )
    .await
    .ok();
    let before = desk
        .world
        .app
        .call(DocumentGet, &[&doc])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let mut rename = operation(
        DocumentTreeCommand::Move,
        vec![DocumentTreeItem::Container(source)],
        Some(DocumentDirectory::NodeDocs(desk.topic)),
    );
    rename.name = Some("Renamed".to_owned());
    let answer = act(&desk, &desk.ada, &rename).await.ok();
    assert!(
        answer
            .view
            .entries
            .iter()
            .any(|row| row.entry.item == DocumentTreeItem::Container(source)
                && row.entry.title == "Renamed")
    );
    let after = desk
        .world
        .app
        .call(DocumentGet, &[&doc])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        before.base.seq, after.base.seq,
        "an ancestor-only rename is independent of the text feed sequence"
    );
    assert!(
        after
            .location
            .ancestors
            .iter()
            .any(|ancestor| ancestor.id == source && ancestor.title.as_deref() == Some("Renamed"))
    );
    let copy = operation(
        DocumentTreeCommand::Copy,
        vec![DocumentTreeItem::Container(source)],
        Some(DocumentDirectory::Container(destination)),
    );
    let copied = act(&desk, &desk.ada, &copy).await.ok();
    let new_doc = copied
        .copied
        .iter()
        .find_map(|pair| match (pair.source, pair.destination) {
            (DocumentTreeItem::Document(old), DocumentTreeItem::Document(new)) if old == doc => {
                Some(new)
            }
            _ => None,
        })
        .ok_or("copy map has no new document")?;
    assert_ne!(new_doc, doc);
    let replay = act(&desk, &desk.ada, &copy).await.ok();
    assert_eq!(replay.operation, copied.operation);
    assert_eq!(replay.copied, copied.copied);
    let new = desk
        .world
        .app
        .call(DocumentGet, &[&new_doc])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(
        new.location
            .ancestors
            .iter()
            .any(|ancestor| ancestor.id == destination)
    );
    assert!(
        !new.location
            .ancestors
            .iter()
            .any(|ancestor| ancestor.id == source)
    );
    let elsewhere = desk.another_topic("Elsewhere").await;
    let moved = act(
        &desk,
        &desk.ada,
        &operation(
            DocumentTreeCommand::Move,
            vec![DocumentTreeItem::Container(source)],
            Some(DocumentDirectory::NodeDocs(elsewhere)),
        ),
    )
    .await
    .ok();
    assert!(moved.view.entries.iter().any(
        |row| row.entry.item == DocumentTreeItem::Document(doc) && row.entry.node == elsewhere
    ));
    let reloaded = desk
        .world
        .app
        .call(DocumentSyncGet, &[&doc])
        .token(&desk.ada.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(reloaded.document.base.id, doc);
    assert_eq!(reloaded.document.base.node.id, elsewhere);
    assert_eq!(
        reloaded.document.location.parent,
        DocumentDirectory::Container(source)
    );
    Ok(())
}

async fn data(client: &mut realtime_support::Client, feed: &str) -> Json {
    loop {
        let frame = client.frame().await;
        assert!(
            !matches!(frame["op"].as_str(), Some("closed" | "error")),
            "{frame}"
        );
        if frame["op"] == "event" && frame["feed"] == feed {
            return frame["data"].clone();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_directory_and_entry_sockets_follow_native_changes_and_never_poll_or_refetch_lists()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let parent = folder(&desk, None, "Folder").await?;
    let other = desk.another_topic("Unrelated").await;
    let doc = desk.make(&desk.ada, "Leaf").await.base.id;
    act(
        &desk,
        &desk.ada,
        &operation(
            DocumentTreeCommand::Move,
            vec![DocumentTreeItem::Document(doc)],
            Some(DocumentDirectory::Container(parent)),
        ),
    )
    .await
    .ok();
    let page = list(&desk, &desk.ada, Some(parent), false, None, Some(1))
        .await
        .ok();
    let entry = desk
        .world
        .app
        .call(DocumentTreeEntryGet, &[&doc])
        .token(&desk.ada.token)
        .query(&DocumentTreeEntryGetQuery {
            kind: DocumentTreeKind::Document,
            inspect_deleted: None,
        })
        .send()
        .await
        .ok();
    let entry_feed = entry.feed.ok_or("entry feed")?;
    assert!(page.feed.parse::<Feed>().is_ok());
    let db = Db::from_pool(desk.world.db.pool.clone());
    let server = realtime_support::Server::over(
        &db,
        &desk.world.app.app.clock,
        realtime_support::quick(&desk.world.app.app.config),
    )
    .await;
    let mut one = realtime_support::Client::signed_in(server.addr, &desk.ada.token).await;
    let mut two = realtime_support::Client::signed_in(server.addr, &desk.ada.token).await;
    one.sub(1, &page.feed, None).await;
    one.expect("subscribed").await;
    two.sub(2, &entry_feed, None).await;
    two.expect("subscribed").await;
    assert_eq!(
        data(&mut one, &page.feed).await["directory"]["items"][0]["entry"]["title"],
        "Leaf"
    );
    assert_eq!(
        data(&mut two, &entry_feed).await["entry"]["entries"][0]["entry"]["item"]["id"],
        json!(doc)
    );
    assert!(one.is_quiet_for(Duration::from_millis(1200)).await);
    let mut rename = operation(
        DocumentTreeCommand::Move,
        vec![DocumentTreeItem::Container(parent)],
        Some(DocumentDirectory::NodeDocs(desk.topic)),
    );
    rename.name = Some("Moved ancestor".to_owned());
    act(&desk, &desk.ada, &rename).await.ok();
    let directory = data(&mut one, &page.feed).await;
    assert_eq!(
        directory["directory"]["directory"]["folder"]["entry"]["title"],
        "Moved ancestor"
    );
    let changed = data(&mut two, &entry_feed).await;
    assert!(
        changed["entry"]["ancestors"]
            .as_array()
            .ok_or("entry ancestors")?
            .iter()
            .any(
                |ancestor| ancestor["id"] == json!(parent) && ancestor["title"] == "Moved ancestor"
            )
    );
    desk.world
        .app
        .call(DocumentContainerDocumentCreate, &[&parent])
        .token(&desk.ada.token)
        .body(&desk::body("Another"))
        .send()
        .await
        .ok();
    let changed = data(&mut one, &page.feed).await;
    assert_eq!(
        changed["directory"]["items"][0]["entry"]["title"],
        "Another"
    );
    assert!(changed["directory"]["next_cursor"].is_string());
    // An unrelated place stamp in the same org wakes the view but cannot manufacture a change
    // merely because the next cursor's authenticated-encryption nonce was regenerated.
    let mut conn = desk.world.conn().await;
    orch_db::queries::hierarchy::stamp(&mut conn, other).await?;
    drop(conn);
    assert!(one.is_quiet_for(Duration::from_millis(1200)).await);
    drop((one, two));
    server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_folder_projection_uses_each_persons_actual_permissions_and_closes_on_withdrawal()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let visitor = desk.stranger("visitor").await;
    let outsider = desk.stranger("outsider").await;
    let parent = folder(&desk, None, "Scoped folder").await?;
    give(
        &desk,
        parent,
        HolderInput::User { id: visitor.id() },
        AccessChoice::FullView,
    )
    .await;
    let page = list(&desk, &visitor, Some(parent), false, None, None)
        .await
        .ok();
    let db = Db::from_pool(desk.world.db.pool.clone());
    let server = realtime_support::Server::over(
        &db,
        &desk.world.app.app.clock,
        realtime_support::quick(&desk.world.app.app.config),
    )
    .await;
    let mut client = realtime_support::Client::signed_in(server.addr, &visitor.token).await;
    let mut denied = realtime_support::Client::signed_in(server.addr, &outsider.token).await;
    client.sub(1, &page.feed, None).await;
    client.expect("subscribed").await;
    assert_eq!(
        data(&mut client, &page.feed).await["directory"]["directory"]["can_create"],
        false
    );
    denied.sub(2, &page.feed, None).await;
    let refusal = denied.expect("error").await;
    assert_eq!(refusal["code"], "forbidden");
    assert!(!refusal.to_string().contains("Scoped folder"));
    let ids: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM permission_entries WHERE object_kind='document_container' AND object_id=$1
        AND permission='doc.view' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(parent)
    .bind(visitor.id())
    .fetch_all(&desk.world.db.pool)
    .await?;
    assert!(!ids.is_empty());
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &parent])
        .token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","remove":ids}]}))
        .send()
        .await
        .ok();
    let closed = client.expect("closed").await;
    assert_eq!(closed["reason"], "revoked");
    assert!(client.is_quiet_for(Duration::from_millis(500)).await);
    drop((client, denied));
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn read_first_paths_never_publish_an_agents_readable_folder_name_to_a_person_without_that_folder()
-> Proof {
    use orch_agent_contract::descriptor::{AgentDescriptor, McpSupport, OrchToolsSupport};
    use orch_link_contract::commands::{AgentStartResult, CommandKind, CommandResult};
    keys();
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    let parent = scene
        .rig
        .world
        .app
        .call(DocumentContainerCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: None,
            name: "Hidden folder name".to_owned(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let folder = match parent.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => id,
        _ => return Err("folder".into()),
    };
    checked(
        scene
            .rig
            .world
            .app
            .call(PermissionPageUpdate, &[&"document-containers", &folder])
            .token(&scene.ada.token)
            .raw_body(&json!({"whole":"private"}))
            .send()
            .await,
        "make the synthetic ancestor private",
    )?;
    checked(scene.rig.world.app.call(PermissionPageUpdate,&[&"document-containers",&folder]).token(&scene.ada.token)
        .raw_body(&json!({"name":[{"holders":[{"kind":"agents","id":thread}],"choice":"full_view","export":false}]})).send().await,
        "give the actual thread agent ancestor Read")?;
    let document = scene
        .rig
        .world
        .app
        .call(DocumentContainerDocumentCreate, &[&folder])
        .token(&scene.ada.token)
        .body(&desk::body("Readable leaf"))
        .send()
        .await
        .ok();
    checked(scene.rig.world.app.call(PermissionPageUpdate,&[&"documents",&document.base.id]).token(&scene.ada.token)
        .raw_body(&json!({"name":[{"holders":[{"kind":"user","id":scene.dan.id()}],"choice":"full_view","export":false}]})).send().await,
        "give the actual person leaf Read without ancestor Read")?;
    let request = ShareWithAgents {
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: document.base.id,
        }),
        read_first: Some(true),
        inside: None,
        mode: ShareMode::Live,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..Default::default()
        }),
        to: ShareTo::Threads(vec![ShareToThread { thread_id: thread }]),
        comment: None,
        expires_at: None,
        client_id: new_id().to_string(),
    };
    // The generic rig does not declare own-tool support. This is a native eligibility refusal,
    // not a folder/name-visibility failure; keep it as a negative control before the capable fixture.
    let mut conn = scene.rig.world.conn().await;
    let settings = orch_engine::tools::ToolSettings::load(&mut conn, thread).await?;
    assert!(
        settings.enabled.iter().any(|name| name == "list_shared"),
        "the topic already enables the required reader"
    );
    assert_eq!(
        orch_db::queries::shares::harness_takes_tools(&mut conn, thread).await?,
        Some(false)
    );
    drop(conn);
    let refused = scene
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&scene.ada.token)
        .body(&request)
        .send()
        .await;
    assert_eq!(refused.status, 409);
    let refused = refused
        .body
        .err()
        .ok_or("the generic no-tools fixture unexpectedly accepted a live share")?;
    assert_eq!(refused.status, 409);
    assert_eq!(refused.code, ErrorCode::ShareUnavailable);
    assert_eq!(
        refused.extra.get("reason").and_then(Json::as_str),
        Some("no_tools")
    );
    // Use the MCP-capable synthetic fake variant for this one scenario, with the same declaration
    // at stored discovery and agent.start. The fake takes MCP; changed sets use resume, not live reload.
    let mut descriptor: AgentDescriptor = serde_json::from_value(rig::descriptor())?;
    assert_eq!(descriptor.capabilities.orch_tools, OrchToolsSupport::None);
    descriptor.capabilities.orch_tools = OrchToolsSupport::Mcp;
    descriptor.capabilities.mcp = McpSupport {
        stdio: true,
        http: true,
        reload: false,
        strict: true,
    };
    let announced = descriptor.clone();
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::AgentStart, move |_| {
            Ok(CommandResult::AgentStart(AgentStartResult {
                reused: false,
                harness_version: "1.0.0".to_owned(),
                descriptor: announced.clone(),
            }))
        });
    let changed =
        sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1 AND harness_kind='fake'")
            .bind(scene.on.agent)
            .bind(serde_json::to_value(&descriptor)?)
            .execute(&scene.rig.world.db.pool)
            .await?;
    assert_eq!(
        changed.rows_affected(),
        1,
        "only this scene's synthetic program declaration changes"
    );
    let mut conn = scene.rig.world.conn().await;
    assert_eq!(
        orch_db::queries::shares::harness_takes_tools(&mut conn, thread).await?,
        Some(true)
    );
    drop(conn);
    let made = checked(
        scene
            .rig
            .world
            .app
            .call(ShareAgentCreate, &[])
            .token(&scene.ada.token)
            .body(&request)
            .send()
            .await,
        "share the synthetic leaf for first reading",
    )?;
    assert_eq!(made.shares.len(), 1);
    assert!(
        made.shares[0].read_first,
        "the real live-share producer stores the first-read intent"
    );
    assert_eq!(
        scene
            .rig
            .world
            .app
            .call(DocumentGet, &[&document.base.id])
            .token(&scene.dan.token)
            .send()
            .await
            .status,
        200
    );
    assert_eq!(
        scene
            .rig
            .world
            .app
            .call(DocumentContainerGet, &[&folder])
            .token(&scene.dan.token)
            .send()
            .await
            .status,
        404,
        "the person retains leaf-only Read and receives no ancestor grant"
    );
    let owner = scene
        .rig
        .world
        .app
        .call(ThreadSharedGet, &[&thread])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    assert!(
        owner
            .read_first_paths
            .iter()
            .any(|path| path.id == document.base.id && path.path.contains("Hidden folder name")),
        "the actual agent path must contain the ancestor before person projection is tested"
    );
    let reader = scene
        .rig
        .world
        .app
        .call(ThreadSharedGet, &[&thread])
        .token(&scene.dan.token)
        .send()
        .await
        .ok();
    assert!(
        reader
            .read_first_paths
            .iter()
            .all(|path| path.id != document.base.id)
    );
    assert!(!serde_json::to_string(&reader)?.contains("Hidden folder name"));
    Ok(())
}

#[tokio::test]
async fn live_text_rows_keep_the_native_personal_list_facts_without_fetching_each_document() -> Proof
{
    keys();
    let desk = Desk::new().await;
    let reader = desk.member("reader", "viewer").await;
    let parent = folder(&desk, None, "Navigation only").await?;
    let document = desk.make(&desk.ada, "Current list facts").await.base.id;
    let blocks = desk
        .paragraphs(&desk.ada, document, &["A real block"])
        .await;
    desk.world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&desk.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: blocks[0].id,
            quote: None,
            content: desk::text("An open discussion"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(DocumentSeen, &[&document])
        .token(&reader.token)
        .body(&DocumentSeenBody { seq: 0 })
        .send()
        .await
        .ok();
    let inherited = list(&desk, &reader, None, false, None, None).await.ok();
    let inherited = inherited
        .items
        .iter()
        .find(|row| row.entry.item == DocumentTreeItem::Document(document))
        .ok_or("inherited readable row")?;
    assert_eq!(
        inherited.participating,
        Some(false),
        "broad inherited View is not explicit participation"
    );
    let unfocused = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&reader.token)
        .query(&NodeDocumentListQuery {
            participating: Some(true),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        unfocused
            .items
            .iter()
            .filter_map(|row| row.as_visible())
            .all(|row| row.id != document)
    );
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: reader.id() }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let flat = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&reader.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    let expected = flat
        .items
        .iter()
        .filter_map(|row| row.as_visible())
        .find(|row| row.id == document)
        .ok_or("native flat document row")?;
    let page = list(&desk, &reader, None, false, None, None).await.ok();
    let row = page
        .items
        .iter()
        .find(|row| row.entry.item == DocumentTreeItem::Document(document))
        .ok_or("tree document row")?;
    assert_eq!(row.document.as_ref(), Some(expected));
    assert_eq!(row.participating, Some(true));
    let focused = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&reader.token)
        .query(&NodeDocumentListQuery {
            participating: Some(true),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        focused
            .items
            .iter()
            .filter_map(|row| row.as_visible())
            .any(|row| row.id == document)
    );
    assert_eq!(expected.seen_seq, Some(0));
    assert!(expected.seq > 0);
    assert_eq!(expected.open_discussions, 1);
    assert!(expected.updated_by.is_some());
    assert!(!expected.archived && !expected.deleted);
    let folder = page
        .items
        .iter()
        .find(|row| row.entry.item == DocumentTreeItem::Container(parent))
        .ok_or("folder row")?;
    assert!(folder.document.is_none() && folder.participating.is_none());
    let selected = desk
        .world
        .app
        .call(DocumentTreeEntryGet, &[&document])
        .token(&reader.token)
        .query(&DocumentTreeEntryGetQuery {
            kind: DocumentTreeKind::Document,
            inspect_deleted: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(selected.entries[0].document.as_ref(), Some(expected));
    let archive = places::archive_body(&desk.world, &desk.ada.token, desk.topic, true).await;
    desk.world
        .app
        .call(NodeArchive, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&archive)
        .send()
        .await
        .ok();
    let archived = list(&desk, &reader, None, false, None, None).await.ok();
    assert!(!archived.directory.can_create);
    let row = archived
        .items
        .iter()
        .find(|row| row.entry.item == DocumentTreeItem::Document(document))
        .ok_or("archived readable text")?;
    assert!(
        row.document
            .as_ref()
            .is_some_and(|document| document.archived)
    );
    Ok(())
}

#[tokio::test]
async fn row_facts_keep_native_admin_reach_distinct_from_participation_and_never_bypass_private_view()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let admin = places::admin(&desk.world, "admin").await;
    let ordinary = desk.member("ordinary", "viewer").await;
    let document = desk
        .make(&desk.ada, "Ordinary inherited text")
        .await
        .base
        .id;
    let flat = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&admin.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    let expected = flat
        .items
        .iter()
        .filter_map(|row| row.as_visible())
        .find(|row| row.id == document)
        .ok_or("native admin-readable row")?;
    let page = list(&desk, &admin, None, false, None, None).await.ok();
    let row = page
        .items
        .iter()
        .find(|row| row.entry.item == DocumentTreeItem::Document(document))
        .ok_or("admin-readable tree row")?;
    assert_eq!(row.document.as_ref(), Some(expected));
    assert!(expected.admin_access.is_some());
    assert_eq!(row.participating, Some(false));
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document])
        .token(&desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    // Enabled native admin reach intentionally reads private content and marks that actual source.
    // Compare the whole reference again; do not turn a fixture's ordinary refusal into a policy change.
    let private = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&admin.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    let expected = private
        .items
        .iter()
        .filter_map(|row| row.as_visible())
        .find(|row| row.id == document)
        .ok_or("native private admin row")?;
    let page = list(&desk, &admin, None, false, None, None).await.ok();
    let row = page
        .items
        .iter()
        .find(|row| row.entry.item == DocumentTreeItem::Document(document))
        .ok_or("private admin tree row")?;
    assert_eq!(row.document.as_ref(), Some(expected));
    assert_eq!(row.participating, Some(false));
    assert!(
        expected
            .admin_access
            .as_ref()
            .is_some_and(|access| access.private)
    );
    let page = list(&desk, &ordinary, None, false, None, None).await.ok();
    assert!(
        page.items
            .iter()
            .all(|row| row.entry.item != DocumentTreeItem::Document(document))
    );
    assert!(!serde_json::to_string(&page)?.contains("Ordinary inherited text"));
    assert_eq!(
        desk.world
            .app
            .call(DocumentTreeEntryGet, &[&document])
            .token(&ordinary.token)
            .query(&DocumentTreeEntryGetQuery {
                kind: DocumentTreeKind::Document,
                inspect_deleted: None
            })
            .send()
            .await
            .status,
        404
    );
    desk.world
        .app
        .call(MeAdminModeSet, &[])
        .token(&admin.token)
        .body(&AdminModeSetBody { as_admin: false })
        .send()
        .await
        .ok();
    assert_eq!(
        list(&desk, &admin, None, false, None, None)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentTreeEntryGet, &[&document])
            .token(&admin.token)
            .query(&DocumentTreeEntryGetQuery {
                kind: DocumentTreeKind::Document,
                inspect_deleted: None
            })
            .send()
            .await
            .status,
        404
    );
    Ok(())
}
