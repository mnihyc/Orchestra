//! A person's card lists only current direct shares both people can still open, before paging.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod reader_diagnostics;
mod support;

use desk::Desk;
use orch_api_contract::access::{NodeRoleCreate, PermissionPageUpdate};
use orch_api_contract::common::{ErrorCode, PageQuery};
use orch_api_contract::users::{UserShare, UserSharesGet};
use orch_types::Id;
use serde_json::json;

fn share_key(item: &UserShare) -> (&'static str, Id) {
    match item {
        UserShare::Node { node } => ("node", node.id),
        UserShare::Document { document } => ("document", document.id),
        UserShare::DocumentContainer { folder } => ("document_container", folder.id),
    }
}

async fn give(desk: &Desk, object: &str, id: Id, to: &places::Who) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&object, &id])
        .token(&desk.ada.token)
        .raw_body(
            &json!({"name":[{"holders":[{"kind":"user","id":to.id()}],"choice":"full_view"}]}),
        )
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn the_card_reads_real_roles_and_pages_once_and_opens_a_readable_document() {
    reader_diagnostics::trace("person_card_roles_and_pages", async {
    let desk = Desk::new().await;
    let bob = desk.member("bob", "viewer").await;
    give(&desk, "nodes", desk.topic, &bob).await;
    // Several permissions on one object still make one card row.
    desk.world.app.call(NodeRoleCreate, &[&desk.topic]).token(&desk.ada.token)
        .raw_body(&json!({"subject":{"kind":"user","id":bob.id()},"role":"viewer"}))
        .send().await.ok();
    let mut body = desk::body("A private plan");
    body.access = Some(orch_api_contract::documents::NewDocumentAccess::Private);
    let doc = desk.make_body(&desk.ada, body).await.ok();
    give(&desk,"documents",doc.base.id,&bob).await;
    let page = desk.world.app.call(UserSharesGet, &[&bob.id()]).token(&desk.ada.token)
        .query(&PageQuery::default()).send().await.ok();
    // Membership gives Bob a real direct Viewer role from Ada on the root org too.
    let mut expected = vec![("node", desk.org), ("node", desk.topic), ("document", doc.base.id)];
    expected.sort_unstable();
    assert_eq!(page.items.iter().map(share_key).collect::<Vec<_>>(), expected,
        "the direct org role, deduplicated topic role/page and private document are the complete ordered set");
    assert_eq!(page.items.len(),3);
    assert!(page.next_cursor.is_none());
    assert!(page.items.iter().any(|item| matches!(item, UserShare::Node { node }
        if node.id == desk.org && node.kind == orch_api_contract::nodes::NodeKind::Org
            && node.name.as_deref() == Some("Acme") && !node.redacted)));
    assert!(page.items.iter().any(|item| matches!(item, UserShare::Node { node } if node.id == desk.topic && node.name.as_deref() == Some("Plans"))));
    assert!(page.items.iter().any(|item| matches!(item, UserShare::Document { document } if document.id == doc.base.id && document.title == "A private plan")));
    let reverse = desk.world.app.call(UserSharesGet, &[&desk.ada.id()]).token(&bob.token)
        .query(&PageQuery::default()).send().await.ok();
    let ids = |items: &[UserShare]| items.iter().map(|item| match item {
        UserShare::Node {node} => node.id, UserShare::Document {document} => document.id,
        UserShare::DocumentContainer {folder} => folder.id,
    }).collect::<Vec<_>>();
    assert_eq!(ids(&reverse.items),ids(&page.items),"either person sees the same shared objects, through their own readable references");
    let stranger = desk.stranger("stranger").await;
    let page = desk.world.app.call(UserSharesGet, &[&bob.id()]).token(&stranger.token)
        .query(&PageQuery::default()).send().await.ok();
    assert!(page.items.is_empty(),"no enumeration of another pair's shares");
    assert!(page.next_cursor.is_none());
    }).await;
}

#[tokio::test]
async fn expired_and_currently_unreadable_grants_never_enter_the_cards_pages_or_counts() {
    reader_diagnostics::trace("person_card_expiry_and_paging", async {
    let desk = Desk::new().await;
    let bob = desk.member("bob", "viewer").await;
    let mut topics = Vec::new();
    for number in 0..4 {
        let topic = desk.another_topic(&format!("Shared {number}")).await;
        give(&desk,"nodes",topic,&bob).await;
        topics.push(topic);
    }
    sqlx::query("UPDATE permission_entries SET expires_at=$1 WHERE object_kind='node' AND object_id=ANY($2) AND holder_kind='user' AND holder_id=$3")
        .bind(desk.world.now()).bind(&topics[..2]).bind(bob.id()).execute(&desk.world.db.pool).await.unwrap();
    let first = desk.world.app.call(UserSharesGet, &[&bob.id()]).token(&desk.ada.token)
        .query(&PageQuery{limit:Some(1),cursor:None}).send().await.ok();
    assert_eq!(first.items.len(),1);
    assert!(first.next_cursor.is_some());
    let second = desk.world.app.call(UserSharesGet, &[&bob.id()]).token(&desk.ada.token)
        .query(&PageQuery{limit:Some(1),cursor:first.next_cursor.clone()}).send().await.ok();
    assert_eq!(second.items.len(),1);
    assert!(second.next_cursor.is_some(), "the other unexpired topic or direct root role still has its own page");
    assert_ne!(first.items,second.items);
    let third = desk.world.app.call(UserSharesGet, &[&bob.id()]).token(&desk.ada.token)
        .query(&PageQuery{limit:Some(1),cursor:second.next_cursor.clone()}).send().await.ok();
    assert_eq!(third.items.len(),1);
    assert!(third.next_cursor.is_none());
    let items: Vec<_> = first.items.iter().chain(&second.items).chain(&third.items).collect();
    let keys: Vec<_> = items.iter().map(|item| share_key(item)).collect();
    let mut expected = vec![("node", desk.org), ("node", topics[2]), ("node", topics[3])];
    expected.sort_unstable();
    assert_eq!(keys, expected, "exact paging union includes the real root role and only the two unexpired direct topic grants");
    assert_eq!(keys.iter().copied().collect::<std::collections::HashSet<_>>().len(), 3,
        "the one-row pages never repeat a direct role or permission-page share");
    for item in items {
        let UserShare::Node { node } = item else { panic!("only the exact three node shares belong to this fixture"); };
        let name = if node.id == desk.org { "Acme" } else if node.id == topics[2] { "Shared 2" } else { "Shared 3" };
        let kind = if node.id == desk.org { orch_api_contract::nodes::NodeKind::Org }
            else { orch_api_contract::nodes::NodeKind::Topic };
        assert_eq!(node.kind, kind);
        assert_eq!(node.name.as_deref(), Some(name));
        assert!(!node.redacted);
    }
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1").bind(bob.id()).execute(&desk.world.db.pool).await.unwrap();
    let hidden = desk.world.app.call(UserSharesGet, &[&bob.id()]).token(&desk.ada.token)
        .query(&PageQuery{limit:Some(1),cursor:None}).send().await.ok();
    assert!(hidden.items.is_empty());
    assert!(hidden.next_cursor.is_none());
    let invalid = desk.world.app.call(UserSharesGet, &[&bob.id()]).token(&desk.ada.token)
        .query(&PageQuery{limit:None,cursor:Some("provider|not-a-share".into())}).send().await.problem();
    assert_eq!(invalid.code,ErrorCode::RequestInvalid);
    }).await;
}

#[tokio::test]
async fn a_direct_folder_share_is_discoverable_by_both_people_without_giving_its_place()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use orch_api_contract::document_tree::{
        DocumentContainerCreate, DocumentContainerCreateBody, DocumentContainerGet,
        DocumentTreeItem,
    };
    use orch_api_contract::nodes::{NodeChildList, NodeChildListQuery};
    let desk = Desk::new().await;
    let bob = desk.stranger("folder-reader").await;
    let created = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: None,
            name: "Shared research".into(),
            client_id: "direct-folder".into(),
        })
        .send()
        .await
        .ok();
    let id = match created.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => id,
        _ => return Err("folder receipt absent".into()),
    };
    give(&desk, "document-containers", id, &bob).await;
    for (reader, person) in [(&desk.ada, &bob), (&bob, &desk.ada)] {
        let page = desk
            .world
            .app
            .call(UserSharesGet, &[&person.id()])
            .token(&reader.token)
            .query(&PageQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(page.items.len(), 1);
        assert!(
            matches!(&page.items[0],UserShare::DocumentContainer {folder}
            if folder.id==id && folder.name.as_deref()==Some("Shared research") && !folder.redacted)
        );
        assert!(page.next_cursor.is_none());
    }
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&id])
            .token(&bob.token)
            .send()
            .await
            .status,
        200
    );
    assert_eq!(
        desk.world
            .app
            .call(NodeChildList, &[&desk.topic])
            .token(&bob.token)
            .query(&NodeChildListQuery::default())
            .send()
            .await
            .status,
        403
    );
    sqlx::query("UPDATE permission_entries SET expires_at=$2 WHERE object_kind='document_container' AND object_id=$1
        AND holder_kind='user' AND holder_id=$3 AND removed_at IS NULL")
        .bind(id).bind(desk.world.now()).bind(bob.id()).execute(&desk.world.db.pool).await?;
    let gone = desk
        .world
        .app
        .call(UserSharesGet, &[&bob.id()])
        .token(&desk.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert!(gone.items.is_empty() && gone.next_cursor.is_none());
    Ok(())
}
