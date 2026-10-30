//! Current single-entry place metadata uses native person authority and scoped structural wakes.

mod desk;
#[path = "document_entry_metadata/diagnostics.rs"]
mod diagnostics;
mod places;
mod reader_diagnostics;
mod realtime_support;
mod rig;
mod support;

use desk::Desk;
use orch_api_contract::{
    access::{AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate},
    common::ErrorCode,
    document_tree::*,
    documents::{DocumentGet, DocumentSyncGet, DocumentSyncGetQuery},
    nodes::{DeletedInspectionQuery, Node, NodeGet, NodeUpdate, NodeUpdateBody},
};
use orch_api_core::{routes::resources::sealing, testing::Reply};
use orch_db::{Db, secrets::MasterKey};
use orch_types::{Id, new_id};
use places::Who;
use serde_json::json;
use std::time::Duration;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn keys() {
    sealing::install_master_key(MasterKey::new([42; 32]));
}

fn operation(items: Vec<DocumentTreeItem>, to: DocumentDirectory) -> DocumentTreeOperationBody {
    DocumentTreeOperationBody {
        command: DocumentTreeCommand::Move,
        items,
        to: Some(to),
        name: None,
        client_id: new_id().to_string(),
        version: None,
        deletion_plan: None,
    }
}

async fn act(desk: &Desk, by: &Who, body: &DocumentTreeOperationBody) -> DocumentTreeReceipt {
    desk.world
        .app
        .call(DocumentTreeOperate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
        .ok()
}

async fn folder(desk: &Desk, parent: Option<Id>, name: &str) -> Proof<DocumentTreeReceipt> {
    Ok(desk
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
        .ok())
}

fn container(receipt: &DocumentTreeReceipt) -> Proof<Id> {
    match receipt.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => Ok(id),
        _ => Err("folder receipt lacks its typed identity".into()),
    }
}

async fn entry(desk: &Desk, by: &Who, id: Id, kind: DocumentTreeKind) -> Reply<DocumentTreeRead> {
    desk.world
        .app
        .call(DocumentTreeEntryGet, &[&id])
        .token(&by.token)
        .query(&DocumentTreeEntryGetQuery {
            kind,
            inspect_deleted: None,
        })
        .send()
        .await
}

async fn give(desk: &Desk, kind: &str, id: Id, by: &Who, choice: AccessChoice) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&kind, &id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: by.id() }],
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

fn redacted(read: &DocumentTreeRead) -> Proof {
    let place = read.place.as_ref().ok_or("single-entry containing place")?;
    assert!(place.node.redacted && place.node.name.is_none());
    assert!(
        place
            .path
            .iter()
            .all(|node| node.redacted && node.name.is_none())
    );
    assert_eq!(place.path.last(), Some(&place.node));
    Ok(())
}

async fn metadata(client: &mut realtime_support::Client, feed: &str) -> Proof<DocumentTreeRead> {
    loop {
        let frame = client.frame().await;
        assert!(
            !matches!(frame["op"].as_str(), Some("closed" | "error")),
            "metadata subscription failed"
        );
        if frame["op"] == "event" && frame["feed"] == feed {
            return Ok(serde_json::from_value(frame["data"]["entry"].clone())?);
        }
    }
}

#[tokio::test]
async fn own_moves_return_current_place_for_single_documents_and_folders_but_not_multi_entry_receipts()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Single").await;
    let before = entry(&desk, &desk.ada, doc.base.id, DocumentTreeKind::Document)
        .await
        .ok();
    let place = before.place.as_ref().ok_or("root-docs place")?;
    assert_eq!(place.node, doc.base.node);
    assert_eq!(place.path, doc.path);
    assert_eq!(
        before.entries[0].entry.parent,
        DocumentDirectory::NodeDocs(desk.topic)
    );
    let made = folder(&desk, None, "Folder").await?;
    let parent = container(&made)?;
    assert_eq!(
        made.view
            .place
            .as_ref()
            .ok_or("single-folder receipt place")?,
        place
    );
    let shown = desk
        .world
        .app
        .call(DocumentContainerGet, &[&parent])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let context = shown.directory.as_ref().ok_or("folder context")?;
    assert_eq!(
        shown.place,
        Some(DocumentTreePlace {
            node: context.node.clone(),
            path: context.path.clone()
        })
    );
    assert!(shown.entries[0].document.is_none());
    let moved = act(
        &desk,
        &desk.ada,
        &operation(
            vec![DocumentTreeItem::Document(doc.base.id)],
            DocumentDirectory::Container(parent),
        ),
    )
    .await;
    assert_eq!(
        moved.view.entries[0].entry.parent,
        DocumentDirectory::Container(parent)
    );
    assert_eq!(moved.view.place.as_ref(), Some(place));
    assert!(moved.view.entries[0].entry.revision > before.entries[0].entry.revision);
    assert!(moved.view.ancestors.iter().any(|row| row.id == parent));
    let elsewhere = desk.another_topic("Elsewhere").await;
    let moved = act(
        &desk,
        &desk.ada,
        &operation(
            vec![DocumentTreeItem::Document(doc.base.id)],
            DocumentDirectory::NodeDocs(elsewhere),
        ),
    )
    .await;
    let current = desk
        .world
        .app
        .call(DocumentGet, &[&doc.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        moved.view.place,
        Some(DocumentTreePlace {
            node: current.base.node.clone(),
            path: current.path.clone()
        })
    );
    assert_eq!(
        current.location.parent,
        DocumentDirectory::NodeDocs(elsewhere)
    );
    let other = desk.make(&desk.ada, "Other").await.base.id;
    let many = act(
        &desk,
        &desk.ada,
        &operation(
            vec![
                DocumentTreeItem::Document(doc.base.id),
                DocumentTreeItem::Document(other),
            ],
            DocumentDirectory::Container(parent),
        ),
    )
    .await;
    assert_eq!(many.view.entries.len(), 2);
    assert!(
        many.view.place.is_none(),
        "multi-entry answers must not invent one selected place"
    );
    Ok(())
}

#[tokio::test]
async fn leaf_grants_get_redacted_place_without_parent_node_view_and_keep_actual_edit_manage_standings()
-> Proof {
    reader_diagnostics::trace("entry_leaf_grants", async {
        diagnostics::phase("desk");
        keys();
        let desk = Desk::new().await;
        diagnostics::phase("reader");
        let reader = desk.stranger("entryreader").await;
        diagnostics::phase("project.private");
        desk.world
            .app
            .call(PermissionPageUpdate, &[&"nodes", &desk.project])
            .token(&desk.ada.token)
            .raw_body(&json!({"whole":"private"}))
            .send()
            .await
            .ok();
        diagnostics::phase("container.create");
        let hidden = container(&folder(&desk, None, "Hidden folder name").await?)?;
        diagnostics::phase("container.private");
        desk.world
            .app
            .call(PermissionPageUpdate, &[&"document-containers", &hidden])
            .token(&desk.ada.token)
            .raw_body(&json!({"whole":"private"}))
            .send()
            .await
            .ok();
        diagnostics::phase("document.create");
        let doc = desk.make(&desk.ada, "Shared leaf").await.base.id;
        diagnostics::phase("document.move");
        act(
            &desk,
            &desk.ada,
            &operation(
                vec![DocumentTreeItem::Document(doc)],
                DocumentDirectory::Container(hidden),
            ),
        )
        .await;
        for (choice, edit, manage) in [
            (AccessChoice::FullView, false, false),
            (AccessChoice::FullInteract, true, false),
            (AccessChoice::FullManage, true, true),
        ] {
            diagnostics::phase("leaf.grant");
            give(&desk, "documents", doc, &reader, choice).await;
            diagnostics::phase("leaf.metadata");
            let read = entry(&desk, &reader, doc, DocumentTreeKind::Document)
                .await
                .ok();
            redacted(&read)?;
            assert_eq!(
                read.entries[0].entry.parent,
                DocumentDirectory::Container(hidden)
            );
            assert!(
                read.ancestors
                    .iter()
                    .find(|row| row.id == hidden)
                    .is_some_and(|row| row.title.is_none() && row.basename.is_none())
            );
            let standing = &read.entries[0].standing;
            assert!(standing.read);
            assert_eq!(standing.write, edit);
            assert_eq!(standing.manage, manage);
            diagnostics::phase("leaf.document");
            let full = desk
                .world
                .app
                .call(DocumentGet, &[&doc])
                .token(&reader.token)
                .send()
                .await
                .ok();
            assert_eq!(
                read.place,
                Some(DocumentTreePlace {
                    node: full.base.node,
                    path: full.path
                })
            );
            assert_eq!(
                standing.write,
                full.access
                    .permissions
                    .iter()
                    .any(|name| name == "doc.edit")
            );
            assert_eq!(
                standing.manage,
                full.access
                    .permissions
                    .iter()
                    .any(|name| name == "doc.manage")
            );
            diagnostics::phase("topic.denial");
            assert_eq!(
                desk.world
                    .app
                    .call(NodeGet, &[&desk.topic])
                    .token(&reader.token)
                    .send()
                    .await
                    .problem()
                    .code,
                ErrorCode::AuthzDenied
            );
        }
        diagnostics::phase("folder.grant");
        give(
            &desk,
            "document-containers",
            hidden,
            &reader,
            AccessChoice::FullView,
        )
        .await;
        diagnostics::phase("folder.metadata");
        let folder = entry(&desk, &reader, hidden, DocumentTreeKind::Container)
            .await
            .ok();
        redacted(&folder)?;
        assert!(folder.entries[0].standing.read);
        assert!(!folder.entries[0].standing.write && !folder.entries[0].standing.manage);
        assert!(
            !folder
                .directory
                .as_ref()
                .ok_or("shared folder context")?
                .can_create
        );
        Ok(())
    })
    .await
    .0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_and_foreign_live_moves_replace_place_and_folder_location_for_each_actual_reader()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let reader = desk.stranger("livereader").await;
    let parent = container(&folder(&desk, None, "Destination").await?)?;
    let elsewhere = desk.another_topic("Another area").await;
    let doc = desk.make(&desk.ada, "Live leaf").await.base.id;
    give(&desk, "documents", doc, &reader, AccessChoice::FullView).await;
    let initial = entry(&desk, &desk.ada, doc, DocumentTreeKind::Document)
        .await
        .ok();
    let feed = initial.feed.ok_or("entry feed")?;
    let db = Db::from_pool(desk.world.db.pool.clone());
    let server = realtime_support::Server::over(
        &db,
        &desk.world.app.app.clock,
        realtime_support::quick(&desk.world.app.app.config),
    )
    .await;
    let mut owner = realtime_support::Client::signed_in(server.addr, &desk.ada.token).await;
    let mut foreign = realtime_support::Client::signed_in(server.addr, &reader.token).await;
    owner.sub(1, &feed, None).await;
    owner.expect("subscribed").await;
    foreign.sub(2, &feed, None).await;
    foreign.expect("subscribed").await;
    assert_eq!(
        metadata(&mut owner, &feed)
            .await?
            .place
            .as_ref()
            .ok_or("owner place")?
            .node
            .id,
        desk.topic
    );
    redacted(&metadata(&mut foreign, &feed).await?)?;
    let answer = act(
        &desk,
        &desk.ada,
        &operation(
            vec![DocumentTreeItem::Document(doc)],
            DocumentDirectory::Container(parent),
        ),
    )
    .await;
    let mine = metadata(&mut owner, &feed).await?;
    assert_eq!(mine.place, answer.view.place);
    assert_eq!(
        mine.entries[0].entry.parent,
        DocumentDirectory::Container(parent)
    );
    let theirs = metadata(&mut foreign, &feed).await?;
    redacted(&theirs)?;
    assert_eq!(
        theirs.entries[0].entry.parent,
        DocumentDirectory::Container(parent)
    );
    assert!(
        theirs
            .ancestors
            .iter()
            .find(|row| row.id == parent)
            .is_some_and(|row| row.title.is_none())
    );
    act(
        &desk,
        &desk.ada,
        &operation(
            vec![DocumentTreeItem::Document(doc)],
            DocumentDirectory::NodeDocs(elsewhere),
        ),
    )
    .await;
    let mine = metadata(&mut owner, &feed).await?;
    let theirs = metadata(&mut foreign, &feed).await?;
    assert_eq!(mine.place.as_ref().ok_or("moved place")?.node.id, elsewhere);
    assert_eq!(
        theirs.place.as_ref().ok_or("foreign moved place")?.node.id,
        elsewhere
    );
    assert_eq!(
        theirs.entries[0].entry.parent,
        DocumentDirectory::NodeDocs(elsewhere)
    );
    assert!(
        theirs.entries[0].standing.read
            && !theirs.entries[0].standing.write
            && !theirs.entries[0].standing.manage
    );
    redacted(&theirs)?;
    assert_eq!(
        desk.world
            .app
            .call(NodeGet, &[&elsewhere])
            .token(&reader.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert!(foreign.is_quiet_for(Duration::from_millis(1200)).await);
    drop((owner, foreign));
    server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ancestor_only_folder_and_hierarchy_renames_deliver_metadata_with_unchanged_document_body_and_seq()
-> Proof {
    keys();
    let desk = Desk::new().await;
    // Creation changes the authorization epoch. Prepare this unrelated node before subscribing
    // so the final quiet check isolates a structural notice, not a required authority resync.
    let unrelated = desk.another_topic("Unrelated area").await;
    let parent = container(&folder(&desk, None, "Initial folder").await?)?;
    let doc = desk.make(&desk.ada, "Body stays").await.base.id;
    // An empty description reads as the first line of the first paragraph, and travels with the title;
    // nothing after it does.
    desk.paragraphs(
        &desk.ada,
        doc,
        &["The first line describes it.", "The body bytes stay here."],
    )
    .await;
    act(
        &desk,
        &desk.ada,
        &operation(
            vec![DocumentTreeItem::Document(doc)],
            DocumentDirectory::Container(parent),
        ),
    )
    .await;
    let before = desk.sync(&desk.ada, doc).await;
    let initial = entry(&desk, &desk.ada, doc, DocumentTreeKind::Document)
        .await
        .ok();
    let seq = initial.entries[0].entry.revision;
    let feed = initial.feed.clone().ok_or("entry feed")?;
    let db = Db::from_pool(desk.world.db.pool.clone());
    let server = realtime_support::Server::over(
        &db,
        &desk.world.app.app.clock,
        realtime_support::quick(&desk.world.app.app.config),
    )
    .await;
    let mut client = realtime_support::Client::signed_in(server.addr, &desk.ada.token).await;
    client.sub(1, &feed, None).await;
    client.expect("subscribed").await;
    metadata(&mut client, &feed).await?;
    let mut rename = operation(
        vec![DocumentTreeItem::Container(parent)],
        DocumentDirectory::NodeDocs(desk.topic),
    );
    rename.name = Some("Current folder".to_owned());
    act(&desk, &desk.ada, &rename).await;
    let folder_changed = metadata(&mut client, &feed).await?;
    assert_eq!(folder_changed.entries[0].entry.revision, seq);
    assert_eq!(folder_changed.place, initial.place);
    assert!(
        folder_changed
            .ancestors
            .iter()
            .any(|row| row.id == parent && row.title.as_deref() == Some("Current folder"))
    );
    let node = desk
        .world
        .app
        .call(NodeGet, &[&desk.project])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let Node::Project(project) = node else {
        return Err("native project node".into());
    };
    desk.world
        .app
        .call(NodeUpdate, &[&desk.project])
        .token(&desk.ada.token)
        .body(&NodeUpdateBody {
            name: Some("Current project".to_owned()),
            description: None,
            color: None,
            custom_color: None,
            readme_document_id: None,
            expected_revision: project.base.revision,
        })
        .send()
        .await
        .ok();
    let hierarchy_changed = metadata(&mut client, &feed).await?;
    assert_eq!(hierarchy_changed.entries[0].entry.revision, seq);
    assert_eq!(hierarchy_changed.ancestors, folder_changed.ancestors);
    assert_eq!(
        hierarchy_changed.entries[0].document, folder_changed.entries[0].document,
        "a parent's name change is independent of the document's ordinary reference"
    );
    assert!(
        hierarchy_changed
            .place
            .as_ref()
            .ok_or("current hierarchy place")?
            .path
            .iter()
            .any(|node| node.id == desk.project && node.name.as_deref() == Some("Current project"))
    );
    assert!(!serde_json::to_string(&hierarchy_changed)?.contains("The body bytes stay here."));
    let after = desk
        .world
        .app
        .call(DocumentSyncGet, &[&doc])
        .token(&desk.ada.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(after.document.base.seq, before.document.base.seq);
    assert_eq!(after.blocks, before.blocks);
    let mut conn = desk.world.conn().await;
    orch_db::queries::hierarchy::stamp(&mut conn, unrelated).await?;
    drop(conn);
    // Exactly the old first-message/timeout oracle and budget; retain the first unexpected frame
    // for bounded structural diagnosis instead of discarding it in the boolean helper.
    let unexpected = client.hear_within(Duration::from_millis(1200)).await;
    assert!(
        unexpected.is_none(),
        "unexpected metadata notification: {}",
        diagnostics::unexpected(
            unexpected.as_ref(),
            &feed,
            &serde_json::to_value(&hierarchy_changed)?
        )
    );
    drop(client);
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn retained_entry_place_keeps_existing_inspection_authority_and_never_adds_live_content()
-> Proof {
    keys();
    let desk = Desk::new().await;
    let reader = desk.stranger("retainedreader").await;
    let parent = container(&folder(&desk, None, "Retained folder").await?)?;
    let doc = desk.make(&desk.ada, "Retained leaf").await.base.id;
    act(
        &desk,
        &desk.ada,
        &operation(
            vec![DocumentTreeItem::Document(doc)],
            DocumentDirectory::Container(parent),
        ),
    )
    .await;
    give(&desk, "documents", doc, &reader, AccessChoice::FullView).await;
    let mut remove = operation(
        vec![DocumentTreeItem::Container(parent)],
        DocumentDirectory::NodeDocs(desk.topic),
    );
    remove.command = DocumentTreeCommand::Delete;
    remove.to = None;
    remove.deletion_plan = Some(
        desk.world
            .app
            .call(DocumentTreeDeletePreview, &[])
            .token(&desk.ada.token)
            .body(&remove)
            .send()
            .await
            .ok()
            .stamp,
    );
    let removed = act(&desk, &desk.ada, &remove).await;
    assert!(removed.view.place.is_none());
    assert_eq!(
        entry(&desk, &desk.ada, doc, DocumentTreeKind::Document)
            .await
            .status,
        404
    );
    let folder = desk
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
        folder
            .place
            .as_ref()
            .ok_or("retained folder place")?
            .node
            .id,
        desk.topic
    );
    let leaf = desk
        .world
        .app
        .call(DocumentTreeEntryGet, &[&doc])
        .token(&desk.ada.token)
        .query(&DocumentTreeEntryGetQuery {
            kind: DocumentTreeKind::Document,
            inspect_deleted: Some(true),
        })
        .send()
        .await
        .ok();
    assert_eq!(leaf.place, folder.place);
    assert!(
        leaf.entries[0]
            .inspection
            .as_ref()
            .is_some_and(|value| value.readonly)
    );
    assert!(leaf.entries[0].document.is_none());
    assert_eq!(
        desk.world
            .app
            .call(DocumentTreeEntryGet, &[&doc])
            .token(&reader.token)
            .query(&DocumentTreeEntryGetQuery {
                kind: DocumentTreeKind::Document,
                inspect_deleted: Some(true)
            })
            .send()
            .await
            .status,
        404
    );
    Ok(())
}
