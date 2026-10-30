//! A page manager without content access sees only a private document's identity, with independent name paging.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, body};
use orch_api_contract::access::{PermissionPageGet, PermissionPageUpdate};
use orch_api_contract::common::{ErrorCode, Listed};
use orch_api_contract::documents::{
    DocumentGet, NewDocumentAccess, NodeDocumentList, NodeDocumentListQuery,
};
use serde_json::json;

async fn name_page_manager(
    desk: &Desk,
    maker: &places::Who,
    manager: &places::Who,
    document: orch_types::Id,
) {
    let page = desk
        .world
        .app
        .call(PermissionPageGet, &[&"documents", &document])
        .token(&maker.token)
        .send()
        .await
        .ok();
    desk.world.app.call(PermissionPageUpdate, &[&"documents", &document]).token(&maker.token)
        .raw_body(&json!({"expected_revision":page.revision,"rows":[{"permission":"access.manage","add":[{"holder":{"kind":"user","id":manager.id()}}]}]}))
        .send().await.ok();
}

#[tokio::test]
async fn a_page_manager_lists_a_private_document_without_its_content_and_an_ordinary_reader_discovers_nothing()
 {
    let desk = Desk::new().await;
    let maker = desk.member("maker", "developer").await;
    let reader = desk.member("reader", "viewer").await;
    let manager = desk.member("pagekeeper", "viewer").await;
    let mut request = body("Sensitive handbook");
    request.access = Some(NewDocumentAccess::Private);
    let document = desk.make_body(&maker, request).await.ok();
    desk.paragraphs(&maker, document.base.id, &["secret body"])
        .await;
    name_page_manager(&desk, &maker, &manager, document.base.id).await;
    let page = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&manager.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next_cursor, None);
    let private = page.items[0].as_hidden().unwrap();
    assert_eq!(private.id, document.base.id);
    assert_eq!(private.parent_id, Some(desk.topic));
    assert_eq!(private.kind, "document");
    assert_eq!(private.name, "Sensitive handbook");
    assert_eq!(private.owner.as_ref().unwrap().id, maker.id());
    let wire = serde_json::to_value(&page.items[0]).unwrap();
    let mut keys = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "content_hidden",
            "id",
            "kind",
            "name",
            "owner",
            "parent_id",
            "private"
        ]
    );
    assert_eq!(wire["content_hidden"], true);
    assert_eq!(wire["private"], true);
    let refused = desk
        .world
        .app
        .call(DocumentGet, &[&document.base.id])
        .token(&manager.token)
        .send()
        .await
        .problem();
    assert!(matches!(
        refused.code,
        ErrorCode::AuthzDenied | ErrorCode::NotFound
    ));
    desk.world
        .app
        .call(PermissionPageGet, &[&"documents", &document.base.id])
        .token(&manager.token)
        .send()
        .await
        .ok();
    let unseen = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&reader.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    assert!(unseen.items.is_empty());
    assert_eq!(unseen.next_cursor, None);
    let own = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&maker.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(own.items[0].as_visible().unwrap().id, document.base.id);
}

#[tokio::test]
async fn private_document_pages_are_by_name_not_activity_and_never_repeat_the_readable_rows() {
    let desk = Desk::new().await;
    let maker = desk.member("maker", "developer").await;
    let manager = desk.member("pagekeeper", "developer").await;
    let mut made = Vec::new();
    for name in ["Zulu", "Alpha", "Middle"] {
        let mut request = body(name);
        request.access = Some(NewDocumentAccess::Private);
        made.push(desk.make_body(&maker, request).await.ok());
        name_page_manager(&desk, &maker, &manager, made.last().unwrap().base.id).await;
        desk.world.advance(chrono::Duration::minutes(1));
    }
    desk.paragraphs(
        &maker,
        made[0].base.id,
        &["the most recent private activity"],
    )
    .await;
    let readable = desk.make(&manager, "Visible guide").await;
    let mut cursor = None;
    let mut names = Vec::new();
    let mut readable_ids = Vec::new();
    loop {
        let page = desk
            .world
            .app
            .call(NodeDocumentList, &[&desk.topic])
            .token(&manager.token)
            .query(&NodeDocumentListQuery {
                limit: Some(2),
                cursor,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(page.items.len() <= 2);
        for item in page.items {
            match item {
                Listed::Visible(row) => readable_ids.push(row.content.id),
                Listed::Hidden(row) => names.push(row.name),
            }
        }
        cursor = page.next_cursor;
        if let Some(cursor) = cursor.as_ref() {
            assert!(cursor.starts_with("private:"));
            assert!(
                cursor
                    .strip_prefix("private:")
                    .unwrap()
                    .parse::<u64>()
                    .is_ok(),
                "the private cursor carries no name or activity"
            );
        } else {
            break;
        }
    }
    assert_eq!(names, ["Alpha", "Middle", "Zulu"]);
    assert_eq!(readable_ids, [readable.base.id]);
}

#[tokio::test]
async fn private_documents_below_a_place_are_listed_only_when_descendants_are_asked_for() {
    let desk = Desk::new().await;
    let maker = desk.member("maker", "developer").await;
    let manager = desk.member("pagekeeper", "viewer").await;
    let mut request = body("A topic's private document");
    request.access = Some(NewDocumentAccess::Private);
    let made = desk.make_body(&maker, request).await.ok();
    name_page_manager(&desk, &maker, &manager, made.base.id).await;
    let here = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.project])
        .token(&manager.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    assert!(here.items.is_empty());
    let below = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.project])
        .token(&manager.token)
        .query(&NodeDocumentListQuery {
            under: Some(true),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        serde_json::to_value(&below.items[0]).unwrap()["id"],
        json!(made.base.id)
    );
    assert_eq!(
        below.items[0].as_hidden().unwrap().parent_id,
        Some(desk.topic)
    );
}

#[tokio::test]
async fn a_full_readable_page_continues_into_private_names_without_repeating_its_document() {
    let desk = Desk::new().await;
    let maker = desk.member("maker", "developer").await;
    let manager = desk.member("pagekeeper", "developer").await;
    let mut request = body("Private guide");
    request.access = Some(NewDocumentAccess::Private);
    let private = desk.make_body(&maker, request).await.ok();
    name_page_manager(&desk, &maker, &manager, private.base.id).await;
    let readable = desk.make(&manager, "Readable guide").await;
    let first = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&manager.token)
        .query(&NodeDocumentListQuery {
            limit: Some(1),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(first.items[0].as_visible().unwrap().id, readable.base.id);
    let next = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&manager.token)
        .query(&NodeDocumentListQuery {
            limit: Some(1),
            cursor: first.next_cursor,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(next.items[0].as_hidden().unwrap().id, private.base.id);
    assert_eq!(next.next_cursor, None);
}
