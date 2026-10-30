//! A document's empty-query mention picker lists current readers only, including an explicitly named outside reader.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
};
use orch_api_contract::chats::ChatMentionListQuery;
use orch_api_contract::documents::{DocumentMentionList, NewDocumentAccess};

#[tokio::test]
async fn a_private_pages_picker_obeys_current_view_and_has_no_hidden_people_or_query_minimum() {
    let desk = Desk::new().await;
    let outside = desk.stranger("outside").await;
    let hidden = desk.stranger("hidden").await;
    let mut body = desk::body("Private");
    body.access = Some(NewDocumentAccess::Private);
    let doc = desk.make_body(&desk.ada, body).await.ok();
    let query = ChatMentionListQuery {
        q: None,
        limit: None,
        cursor: None,
    };
    let initial = desk
        .world
        .app
        .call(DocumentMentionList, &[&doc.base.id])
        .token(&desk.ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        initial
            .items
            .iter()
            .map(|item| item.user.id)
            .collect::<Vec<_>>(),
        [desk.ada.id()]
    );
    let denied = desk
        .world
        .app
        .call(DocumentMentionList, &[&doc.base.id])
        .token(&outside.token)
        .query(&query)
        .send()
        .await;
    assert!(!denied.status.is_success());
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: outside.id() }],
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
    let shared = desk
        .world
        .app
        .call(DocumentMentionList, &[&doc.base.id])
        .token(&outside.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(
        shared
            .items
            .iter()
            .any(|item| item.user.id == outside.id() && item.can_view)
    );
    assert!(
        shared
            .items
            .iter()
            .any(|item| item.user.id == desk.ada.id() && item.can_view)
    );
    assert!(shared.items.iter().all(|item| item.user.id != hidden.id()));
}

#[tokio::test]
async fn a_sharer_sees_readers_before_people_who_need_access() {
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let waiting = desk.member("cai", "viewer").await;
    let mut body = desk::body("Private");
    body.access = Some(NewDocumentAccess::Private);
    let doc = desk.make_body(&desk.ada, body).await.ok();
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
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
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let page = desk
        .world
        .app
        .call(DocumentMentionList, &[&doc.base.id])
        .token(&desk.ada.token)
        .query(&ChatMentionListQuery {
            q: None,
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    let granted = page
        .items
        .iter()
        .position(|item| item.user.id == reader.id() && item.can_view)
        .unwrap();
    let missing = page
        .items
        .iter()
        .position(|item| item.user.id == waiting.id() && !item.can_view)
        .unwrap();
    assert!(granted < missing);
}
