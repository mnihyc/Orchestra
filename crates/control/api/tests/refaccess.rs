//! Shared reference labels, private cards, one pending request and timed person-only grants over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use chrono::Duration;
use desk::{Desk, paragraph, put};
use orch_api_contract::access::{
    AccessRequestCreate, AccessRequestCreateBody, AccessRequestItem, AccessRequestObject,
    PageChange, PageChangeRow, PermissionPageUpdate,
};
use orch_api_contract::approvals::{
    AccessDecision, ApprovalDecide, ApprovalDecision, ApprovalList, ApprovalListQuery,
    ApprovalState,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockContent, DocumentSyncGet, DocumentSyncGetQuery, DocumentUpdate, DocumentUpdateBody,
    Inline, InlineRefKind, ReferencePreviewGet, ReferencePreviewGetQuery,
};
use orch_types::{Id, Time};
use serde_json::json;

async fn private_document(desk: &Desk, title: &str) -> Id {
    let id = desk.make(&desk.ada, title).await.base.id;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &id])
        .token(&desk.ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "doc.view".into(),
                profile: None,
                inherit: Some(false),
                locked: None,
                add: None,
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    id
}

fn preview(kind: InlineRefKind, id: Id) -> ReferencePreviewGetQuery {
    ReferencePreviewGetQuery {
        kind,
        id,
        block: None,
        turn: None,
        message: None,
    }
}

fn request(id: Id) -> AccessRequestCreateBody {
    AccessRequestCreateBody {
        items: vec![AccessRequestItem {
            label: None,
            object: AccessRequestObject {
                kind: "document".into(),
                id,
            },
            permissions: vec!["doc.view".into()],
            options: None,
            holder: None,
        }],
        reason: String::new(),
        expires_at: None,
        for_thread: None,
    }
}

#[tokio::test]
async fn a_persons_shared_label_survives_a_rename_and_hidden_readers_receive_no_current_metadata() {
    let desk = Desk::new().await;
    let target = private_document(&desk, "First title").await;
    let notes = desk.make(&desk.ada, "Shared notes").await.base.id;
    let mut block = paragraph("");
    block.content = Some(BlockContent::Inline(vec![Inline::Ref {
        display: None,
        kind: InlineRefKind::Document,
        id: target,
        path: None,
        part: None,
        label: None,
        name: None,
    }]));
    desk.write(&desk.ada, notes, vec![put(block)]).await.ok();
    desk.world
        .app
        .call(DocumentUpdate, &[&target])
        .token(&desk.ada.token)
        .body(&DocumentUpdateBody {
            title: Some("Current secret title".into()),
            description: Some("Current secret description".into()),
        })
        .send()
        .await
        .ok();
    let reader = desk.member("reader", "viewer").await;
    let shown = desk
        .world
        .app
        .call(DocumentSyncGet, &[&notes])
        .token(&reader.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    let value = serde_json::to_value(&shown).unwrap();
    let wire = value.to_string();
    assert!(wire.contains("First title"), "{wire}");
    assert!(
        !wire.contains("Current secret title") && !wire.contains("Current secret description"),
        "{wire}"
    );
    let content = shown
        .blocks
        .iter()
        .find_map(|block| match &block.content {
            BlockContent::Inline(runs) => runs
                .iter()
                .find(|inline| matches!(inline,Inline::Ref {id,..} if *id==target)),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(content,Inline::Ref {label:Some(label),name:Some(None),..} if label=="First title")
    );
    let card = desk
        .world
        .app
        .call(ReferencePreviewGet, &[])
        .token(&reader.token)
        .query(&preview(InlineRefKind::Document, target))
        .send()
        .await
        .ok();
    assert_eq!(card.title, "");
    assert_eq!(card.description, "");
    assert!(
        card.place.is_none()
            && card.updated_at.is_none()
            && card.updated_by.is_none()
            && card.part.is_none()
            && card.file.is_none()
    );
    let unavailable = card.unavailable.unwrap();
    assert!(!unavailable.deleted && unavailable.requestable);
    assert_eq!(unavailable.permission.as_deref(), Some("doc.view"));
    assert_eq!(unavailable.object.unwrap().id, target);
}

#[tokio::test]
async fn simultaneous_sends_leave_one_open_request_for_the_person_and_thing() {
    let desk = Desk::new().await;
    let target = private_document(&desk, "Private plan").await;
    let reader = desk.member("reader", "viewer").await;
    let body = request(target);
    let send = || {
        desk.world
            .app
            .call(AccessRequestCreate, &[])
            .token(&reader.token)
            .body(&body)
            .send()
    };
    let (a, b) = tokio::join!(send(), send());
    let (made, duplicate) = if a.body.is_ok() {
        (a.ok(), b.problem())
    } else {
        (b.ok(), a.problem())
    };
    assert_eq!(duplicate.code, ErrorCode::ConflictExists);
    assert_eq!(duplicate.extra["approval_id"], json!(made.approvals[0].id));
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM approvals WHERE requester_user_id=$1 AND object_id=$2 AND state='pending'").bind(reader.id()).bind(target).fetch_one(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(count, 1);
    let card = desk
        .world
        .app
        .call(ReferencePreviewGet, &[])
        .token(&reader.token)
        .query(&preview(InlineRefKind::Document, target))
        .send()
        .await
        .ok();
    assert_eq!(
        card.unavailable.unwrap().pending,
        Some(made.approvals[0].id)
    );
}

#[tokio::test]
async fn every_quick_duration_gives_one_person_one_thing_and_expires_at_the_servers_decision_time()
{
    let desk = Desk::new().await;
    let reader = desk.member("reader", "viewer").await;
    for (duration, days) in [
        ("1d", Some(1)),
        ("7d", Some(7)),
        ("30d", Some(30)),
        ("always", None),
    ] {
        let target = private_document(&desk, &format!("Plan {duration}")).await;
        let other = private_document(&desk, &format!("Other {duration}")).await;
        let made = desk
            .world
            .app
            .call(AccessRequestCreate, &[])
            .token(&reader.token)
            .body(&request(target))
            .send()
            .await
            .ok();
        let id = made.approvals[0].id;
        assert_eq!(
            made.approvals[0].quick_grant_durations.as_ref().unwrap(),
            &["1d", "7d", "30d", "always"]
        );
        let decision = desk
            .world
            .app
            .call(ApprovalDecide, &[&id])
            .token(&desk.ada.token)
            .body(&ApprovalDecision::Access(AccessDecision {
                approve: true,
                permissions: None,
                options: None,
                expires_at: None,
                quick_duration: Some(duration.into()),
                note: None,
            }))
            .send()
            .await
            .ok();
        assert_eq!(decision.state, ApprovalState::Approved);
        let at = decision.decided_at.unwrap();
        let expected = days.map(|days| at.get() + Duration::days(days));
        let entry:(String,Id,Option<chrono::DateTime<chrono::Utc>>)=sqlx::query_as("SELECT holder_kind,holder_id,expires_at FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND permission='doc.view' AND source='request' AND removed_at IS NULL").bind(target).fetch_one(&mut *desk.world.conn().await).await.unwrap();
        assert_eq!(entry, ("user".into(), reader.id(), expected));
        let shown = desk
            .world
            .app
            .call(ReferencePreviewGet, &[])
            .token(&reader.token)
            .query(&preview(InlineRefKind::Document, target))
            .send()
            .await
            .ok();
        assert!(shown.unavailable.is_none());
        assert_eq!(shown.refresh_at, expected.map(Time::new));
        let hidden = desk
            .world
            .app
            .call(ReferencePreviewGet, &[])
            .token(&reader.token)
            .query(&preview(InlineRefKind::Document, other))
            .send()
            .await
            .ok();
        assert!(
            hidden.unavailable.is_some(),
            "a grant on one document opens no sibling"
        );
        let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='access.approved' AND details->>'approval'=$1").bind(id.to_string()).fetch_one(&mut *desk.world.conn().await).await.unwrap();
        assert_eq!(audits, 1);
        let notices:i64=sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1 AND approval_id=$2 AND kind='access.decided'").bind(reader.id()).bind(id).fetch_one(&mut *desk.world.conn().await).await.unwrap();
        assert_eq!(notices, 1);
        desk.world
            .advance(Duration::days(days.unwrap_or(31)) + Duration::milliseconds(1));
        let after = desk
            .world
            .app
            .call(ReferencePreviewGet, &[])
            .token(&reader.token)
            .query(&preview(InlineRefKind::Document, target))
            .send()
            .await
            .ok();
        assert_eq!(after.unavailable.is_some(), days.is_some());
    }
}

#[tokio::test]
async fn only_the_direct_owner_is_told_and_an_eligible_admin_uses_the_other_list() {
    let desk = Desk::new().await;
    let admin = desk.member("manager", "admin").await;
    let reader = desk.member("reader", "viewer").await;
    places::admin_mode(&desk.world, &admin, true).await;
    let target = private_document(&desk, "Private plan").await;
    let made = desk
        .world
        .app
        .call(AccessRequestCreate, &[])
        .token(&reader.token)
        .body(&request(target))
        .send()
        .await
        .ok();
    let id = made.approvals[0].id;
    let people:Vec<Id>=sqlx::query_scalar("SELECT user_id FROM notifications WHERE approval_id=$1 AND kind='access.requested' ORDER BY user_id").bind(id).fetch_all(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(people, [desk.ada.id()]);
    let own = desk
        .world
        .app
        .call(ApprovalList, &[])
        .token(&admin.token)
        .query(&ApprovalListQuery::default())
        .send()
        .await
        .ok();
    assert!(own.items.is_empty());
    let other = desk
        .world
        .app
        .call(ApprovalList, &[])
        .token(&admin.token)
        .query(&ApprovalListQuery {
            scope: Some("others".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        other
            .items
            .iter()
            .map(|approval| approval.id)
            .collect::<Vec<_>>(),
        [id]
    );
    assert!(other.items[0].can_decide);
}
