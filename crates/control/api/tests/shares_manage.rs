//! Sharing documents with agents to manage over HTTP: given by somebody who holds `doc.manage` and refused to somebody who does not, with what is missing; listed, changed and taken back like any share; the page that says beforehand whether its reader may offer it; and the tree of an agent's reach as a person is shown it, with the documents the agent may manage and the ones it may bring back.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::access::{
    AccessChoice, AgentLevel, HolderInput, PageChange, PageChangeName, PermissionPageGet,
    PermissionPageUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentDelete, DocumentRestore, NodeDocumentCreate, NodeDocumentCreateBody,
};
use orch_api_contract::share::{
    AgentShareList, AgentShareListQuery, ShareAgentCreate, ShareAgentCreateReply, ShareAgentDelete,
    ShareAgentUpdate, ShareAgentUpdateBody, ShareDocumentsLevel, ShareParts, ShareTo,
    ShareToThread, ShareWhat, ShareWhatDocument, ShareWhatNode, ShareWithAgents, SharedEntry,
    SharedEntryKind, ThreadSharedTreeGet, ThreadSharedTreeGetQuery,
};
use orch_api_contract::threads::{ShareMode, Thread};
use orch_types::Id;
use places::Who;
use rig::Scene;

fn manage() -> Option<ShareParts> {
    Some(ShareParts {
        documents: Some(ShareDocumentsLevel::Manage),
        ..ShareParts::default()
    })
}

fn share_of(what: ShareWhat, parts: Option<ShareParts>, to: Id, client: &str) -> ShareWithAgents {
    ShareWithAgents {
        read_first: None,
        what,
        inside: None,
        parts,
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: to }]),
        comment: None,
        expires_at: None,
        client_id: client.to_owned(),
        download: None,
    }
}

async fn create(
    s: &Scene,
    by: &Who,
    request: &ShareWithAgents,
) -> orch_api_core::testing::Reply<ShareAgentCreateReply> {
    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&by.token)
        .body(request)
        .send()
        .await
}

/// A scene whose agents take the tools, with a thread whose agents are to be given documents to manage.
async fn a_reader() -> (Scene, Thread) {
    let s = Scene::new().await;
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,orch_tools}', '\"mcp\"')")
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let reader = s.thread_by(&s.ada, "Reader").await;
    (s, reader)
}

async fn document_beside(s: &Scene, node: Id, title: &str) -> Id {
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&node])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.to_owned(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id
}

async fn reached_by(s: &Scene, who: &Who, thread: Id, node: Id) -> Vec<SharedEntry> {
    s.rig
        .world
        .app
        .call(ThreadSharedTreeGet, &[&thread])
        .token(&who.token)
        .query(&ThreadSharedTreeGetQuery {
            path: Some(format!("id:{node}")),
            depth: Some(4),
        })
        .send()
        .await
        .ok()
        .entries
}

#[tokio::test]
async fn a_share_to_manage_is_given_by_a_developer_listed_changed_and_taken_back_and_refused_to_a_viewer()
 {
    let (s, reader) = a_reader().await;
    let topic = s.tree.topic.base.id;
    let doc = document_beside(&s, topic, "Plan").await;
    let reader_id = reader.base.id;
    let request = |client: &str| {
        share_of(
            ShareWhat::Document(ShareWhatDocument { document_id: doc }),
            manage(),
            reader_id,
            client,
        )
    };
    // Cai may drive the reader's thread and only reads documents here: she is told what she lacks, and nothing is
    // shared.
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &reader_id])
        .token(&s.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: s.cai.id() }],
                choice: AccessChoice::FullInteract,
                options: None,
                expires_at: None,
                note: None,
                export: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let refused = create(&s, &s.cai, &request("c1")).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(
        places::missing_permissions(&refused)
            .iter()
            .any(|each| each == "doc.manage"),
        "{refused:?}"
    );
    // Dan is a Developer, and Developers manage documents: he gives it, and it says who gave it.
    let made = create(&s, &s.dan, &request("d1")).await;
    assert_eq!(made.status, http::StatusCode::CREATED);
    let share = made.ok().shares.remove(0);
    assert_eq!(share.parts.documents, Some(ShareDocumentsLevel::Manage));
    assert_eq!(
        share.entries.len(),
        4,
        "View, Edit and Manage, and the Export row that travels with them"
    );
    assert_eq!(share.by.id, s.dan.id());
    // It is listed on the document's page.
    let listed = s
        .rig
        .world
        .app
        .call(AgentShareList, &[&"documents", &doc])
        .token(&s.ada.token)
        .query(&AgentShareListQuery { side: None })
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, share.id);
    assert_eq!(listed[0].parts.documents, Some(ShareDocumentsLevel::Manage));
    // Changed to editing, which takes the right to manage out and leaves the rest.
    let edited = s
        .rig
        .world
        .app
        .call(ShareAgentUpdate, &[&share.id])
        .token(&s.dan.token)
        .body(&ShareAgentUpdateBody {
            read_first: None,
            parts: Some(ShareParts {
                documents: Some(ShareDocumentsLevel::Edit),
                ..ShareParts::default()
            }),
            expires_at: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(edited.parts.documents, Some(ShareDocumentsLevel::Edit));
    assert_eq!(
        edited.entries.len(),
        3,
        "View and Edit, and the Export row still"
    );
    // And taken back by whoever manages the page.
    let gone = s
        .rig
        .world
        .app
        .call(ShareAgentDelete, &[&share.id])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(gone.status, http::StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_page_offers_sharing_with_agents_to_manage_to_those_who_hold_the_right_to_manage_and_to_no_others()
 {
    let (s, _) = a_reader().await;
    let topic = s.tree.topic.base.id;
    let page = |who: &Who| {
        let app = &s.rig.world.app;
        let token = who.token.clone();
        async move {
            app.call(PermissionPageGet, &[&"nodes", &topic])
                .token(&token)
                .send()
                .await
                .ok()
        }
    };
    let ada = page(&s.ada).await;
    assert_eq!(
        ada.gives.agents,
        [AgentLevel::View, AgentLevel::Edit, AgentLevel::Manage]
    );
    let dan = page(&s.dan).await;
    assert_eq!(
        dan.gives.agents,
        [AgentLevel::View, AgentLevel::Edit, AgentLevel::Manage],
        "a Developer manages documents as shipped"
    );
    let cai = page(&s.cai).await;
    assert_eq!(cai.gives.agents, [AgentLevel::View], "a Viewer only reads");
}

#[tokio::test]
async fn the_tree_says_which_documents_the_agent_may_manage_and_lists_a_deleted_one_to_whoever_may_bring_it_back()
 {
    let (s, reader) = a_reader().await;
    let (topic, reader_id) = (s.tree.topic.base.id, reader.base.id);
    let doc = document_beside(&s, topic, "Plan").await;
    let other = document_beside(&s, topic, "Notes").await;
    create(
        &s,
        &s.ada,
        &share_of(
            ShareWhat::Node(ShareWhatNode { node_id: topic }),
            manage(),
            reader_id,
            "m1",
        ),
    )
    .await
    .ok();
    let entries = reached_by(&s, &s.ada, reader_id, topic).await;
    let plan = entries.iter().find(|entry| entry.id == doc).unwrap();
    assert!(plan.manageable && plan.readable && plan.writable && !plan.deleted);
    let folder = entries
        .iter()
        .find(|entry| entry.kind == SharedEntryKind::Folder && entry.path.ends_with("/docs/"))
        .unwrap();
    // The docs directory of a place is a virtual area: what is managed are the documents in it, and the
    // area itself is written in, never deleted or moved.
    assert!(
        folder.writable && !folder.manageable,
        "the docs directory of a place shared to manage"
    );
    // A person deletes it: the agent that may bring it back is listed it, marked, and it cannot be read.
    s.rig
        .world
        .app
        .call(DocumentDelete, &[&doc])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let entries = reached_by(&s, &s.ada, reader_id, topic).await;
    let gone = entries.iter().find(|entry| entry.id == doc).unwrap();
    assert!(gone.deleted && gone.manageable && !gone.readable && !gone.writable);
    assert_eq!(gone.name.as_deref(), Some("Plan.md"));
    assert!(
        entries
            .iter()
            .any(|entry| entry.id == other && !entry.deleted),
        "the other one is there"
    );
    // Cai reads documents and cannot bring one back: to her the entry has no name.
    let hers = reached_by(&s, &s.cai, reader_id, topic).await;
    let veiled = hers.iter().find(|entry| entry.id == doc);
    assert!(
        veiled.is_none_or(|entry| entry.redacted && entry.name.is_none()),
        "{veiled:?}"
    );
    // Brought back, it is an entry like the others.
    s.rig
        .world
        .app
        .call(DocumentRestore, &[&doc])
        .token(&s.ada.token)
        .raw_body(&serde_json::json!({}))
        .send()
        .await
        .ok();
    let entries = reached_by(&s, &s.ada, reader_id, topic).await;
    let back = entries.iter().find(|entry| entry.id == doc).unwrap();
    assert!(!back.deleted && back.readable && back.manageable);
}

#[tokio::test]
async fn person_projection_redacts_folder_ancestors_base_paths_and_document_attachment_names()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use orch_api_contract::access::PageChangeWhole;
    use orch_api_contract::document_tree::*;
    use orch_api_contract::documents::{DocumentFileCreate, DocumentFileCreateQuery};
    use orch_api_contract::threads::FILENAME_HEADER;
    let (s, reader) = a_reader().await;
    let node = s.tree.topic.base.id;
    let receipt = s
        .rig
        .world
        .app
        .call(DocumentContainerCreate, &[&node])
        .token(&s.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: None,
            name: "Private research".into(),
            client_id: "folder-parent".into(),
        })
        .send()
        .await
        .ok();
    let parent = match receipt.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => id,
        _ => return Err("folder receipt absent".into()),
    };
    let receipt = s
        .rig
        .world
        .app
        .call(DocumentContainerCreate, &[&node])
        .token(&s.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: Some(parent),
            name: "Notes".into(),
            client_id: "folder-child".into(),
        })
        .send()
        .await
        .ok();
    let child = match receipt.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => id,
        _ => return Err("child receipt absent".into()),
    };
    let document = document_beside(&s, node, "Plan").await;
    s.rig
        .world
        .app
        .call(DocumentTreeOperate, &[])
        .token(&s.ada.token)
        .body(&DocumentTreeOperationBody {
            command: DocumentTreeCommand::Move,
            items: vec![DocumentTreeItem::Document(document)],
            to: Some(DocumentDirectory::Container(child)),
            name: None,
            client_id: "nested-plan".into(),
            version: None,
            deletion_plan: None,
        })
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&s.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, "private-path.txt")
        .bytes(b"current file")
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &parent])
        .token(&s.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::Agents { id: reader.base.id }],
                choice: AccessChoice::FullManage,
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
    let grant = PageChange {
        name: Some(vec![PageChangeName {
            holders: vec![HolderInput::User { id: s.cai.id() }],
            choice: AccessChoice::FullView,
            options: None,
            expires_at: None,
            note: None,
            export: None,
        }]),
        ..Default::default()
    };
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &child])
        .token(&s.ada.token)
        .body(&grant)
        .send()
        .await
        .ok();
    let query = ThreadSharedTreeGetQuery {
        path: Some(format!("id:{child}")),
        depth: Some(4),
    };
    let hidden = s
        .rig
        .world
        .app
        .call(ThreadSharedTreeGet, &[&reader.base.id])
        .token(&s.cai.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        hidden.path,
        format!("id:{child}"),
        "the listing base must not leak the agent's named ancestry"
    );
    let written = serde_json::to_string(&hidden)?;
    assert!(!written.contains("Private research") && !written.contains("private-path.txt"));
    let plan = hidden
        .entries
        .iter()
        .find(|entry| entry.id == document)
        .ok_or("the agent's document entry absent")?;
    assert!(plan.redacted && plan.name.is_none());
    assert!(
        hidden
            .entries
            .iter()
            .filter(|entry| entry.kind == SharedEntryKind::File)
            .all(|entry| entry.redacted && entry.name.is_none())
    );
    let full = s
        .rig
        .world
        .app
        .call(ThreadSharedTreeGet, &[&reader.base.id])
        .token(&s.ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    let written = serde_json::to_string(&full)?;
    assert!(written.contains("Private research") && written.contains("private-path.txt"));
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &parent])
        .token(&s.ada.token)
        .body(&grant)
        .send()
        .await
        .ok();
    let admitted = s
        .rig
        .world
        .app
        .call(ThreadSharedTreeGet, &[&reader.base.id])
        .token(&s.cai.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(admitted.path.contains("Private research"));
    assert!(
        admitted
            .entries
            .iter()
            .any(|entry| entry.name.as_deref() == Some("Plan.md") && !entry.redacted)
    );
    assert!(serde_json::to_string(&admitted)?.contains("private-path.txt"));
    Ok(())
}
