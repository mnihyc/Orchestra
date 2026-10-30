//! Private chat reference cards, membership grants, and timed expiry through the same HTTP approval path.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod chat_world;
mod places;
mod support;
use chat_world::Room;
use chrono::Duration;
use orch_api_contract::approvals::{
    AccessDecision, ApprovalDecide, ApprovalDecision, ApprovalList, ApprovalListQuery,
    ApprovalState,
};
use orch_api_contract::chats::{ChatGet, ChatJoinRequest, ChatJoinRequestBody};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{InlineRefKind, ReferencePreviewGet, ReferencePreviewGetQuery};
use orch_types::{Id, Time};

fn reference(id: Id) -> ReferencePreviewGetQuery {
    ReferencePreviewGetQuery {
        kind: InlineRefKind::Chat,
        id,
        block: None,
        turn: None,
        message: None,
    }
}

#[tokio::test]
async fn a_direct_chat_card_is_unnamed_and_has_no_join_form() {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    let card = room
        .world
        .app
        .call(ReferencePreviewGet, &[])
        .token(&room.gus.token)
        .query(&reference(chat))
        .send()
        .await
        .ok();
    assert!(card.title.is_empty() && card.description.is_empty());
    assert!(card.place.is_none() && card.updated_by.is_none() && card.updated_at.is_none());
    let unavailable = card.unavailable.unwrap();
    assert!(
        !unavailable.deleted
            && !unavailable.requestable
            && unavailable.object.is_none()
            && unavailable.permission.is_none()
    );
    let denied = room
        .world
        .app
        .call(ChatJoinRequest, &[&chat])
        .token(&room.gus.token)
        .body(&ChatJoinRequestBody::default())
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::NotFound);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE object_id=$1")
        .bind(chat)
        .fetch_one(&mut *room.world.conn().await)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn every_quick_grant_joins_the_group_and_expiry_removes_membership_and_greys_the_card() {
    let room = Room::new().await;
    for (duration, days) in [
        ("1d", Some(1)),
        ("7d", Some(7)),
        ("30d", Some(30)),
        ("always", None),
    ] {
        let chat = room
            .group(&room.bea, &[&room.cai], Some("Current private group name"))
            .await
            .base
            .id;
        let card = room
            .world
            .app
            .call(ReferencePreviewGet, &[])
            .token(&room.gus.token)
            .query(&reference(chat))
            .send()
            .await
            .ok();
        let wire = serde_json::to_string(&card).unwrap();
        assert!(
            !wire.contains("Current private group name") && !wire.contains("Acme"),
            "{wire}"
        );
        assert!(
            card.title.is_empty()
                && card.description.is_empty()
                && card.place.is_none()
                && card.updated_at.is_none()
                && card.updated_by.is_none()
                && card.part.is_none()
        );
        let unavailable = card.unavailable.unwrap();
        assert!(unavailable.requestable);
        assert_eq!(unavailable.object.unwrap().kind, "chat");
        assert!(unavailable.permission.is_none());
        let made = room
            .world
            .app
            .call(ChatJoinRequest, &[&chat])
            .token(&room.gus.token)
            .body(&ChatJoinRequestBody {
                label: Some("Shared group label".into()),
                reason: "Working together".into(),
            })
            .send()
            .await
            .ok();
        let id = made.approvals[0].id;
        assert_eq!(made.approvals[0].required_permission, "chat.interact");
        let wire = serde_json::to_string(&made).unwrap();
        assert!(
            !wire.contains("Current private group name") && !wire.contains("Acme"),
            "{wire}"
        );
        assert!(made.approvals[0].node.is_none());
        let told: Vec<Id> = sqlx::query_scalar(
            "SELECT user_id FROM notifications WHERE approval_id=$1 AND kind='access.requested'",
        )
        .bind(id)
        .fetch_all(&mut *room.world.conn().await)
        .await
        .unwrap();
        assert_eq!(told, [room.bea.id()]);
        let other = room
            .world
            .app
            .call(ApprovalList, &[])
            .token(&room.cai.token)
            .query(&ApprovalListQuery {
                scope: Some("others".into()),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(other.items.iter().any(|row| row.id == id && row.can_decide));
        let duplicate = room
            .world
            .app
            .call(ChatJoinRequest, &[&chat])
            .token(&room.gus.token)
            .body(&ChatJoinRequestBody::default())
            .send()
            .await
            .problem();
        assert_eq!(duplicate.code, ErrorCode::ConflictExists);
        let decided = room
            .world
            .app
            .call(ApprovalDecide, &[&id])
            .token(&room.cai.token)
            .body(&ApprovalDecision::Access(AccessDecision {
                approve: true,
                quick_duration: Some(duration.into()),
                permissions: None,
                options: None,
                expires_at: None,
                note: None,
            }))
            .send()
            .await
            .ok();
        assert_eq!(decided.state, ApprovalState::Approved);
        let expected = days.map(|days| decided.decided_at.unwrap().get() + Duration::days(days));
        let member: (
            Option<chrono::DateTime<chrono::Utc>>,
            Option<chrono::DateTime<chrono::Utc>>,
        ) = sqlx::query_as(
            "SELECT left_at,expires_at FROM chat_members WHERE chat_id=$1 AND user_id=$2",
        )
        .bind(chat)
        .bind(room.gus.id())
        .fetch_one(&mut *room.world.conn().await)
        .await
        .unwrap();
        assert_eq!(member, (None, expected));
        let opened = room
            .world
            .app
            .call(ChatGet, &[&chat])
            .token(&room.gus.token)
            .send()
            .await
            .ok();
        assert!(
            opened
                .members
                .unwrap()
                .iter()
                .any(|user| user.id == room.gus.id())
        );
        assert_eq!(
            opened.membership_expiries.get(&room.gus.id()).copied(),
            expected.map(Time::new)
        );
        let live = room
            .world
            .app
            .call(ReferencePreviewGet, &[])
            .token(&room.gus.token)
            .query(&reference(chat))
            .send()
            .await
            .ok();
        assert!(live.unavailable.is_none());
        assert_eq!(live.refresh_at, expected.map(Time::new));
        let pending:i64=sqlx::query_scalar("SELECT count(*) FROM notifications WHERE approval_id=$1 AND read_at IS NULL AND kind='access.requested'").bind(id).fetch_one(&mut *room.world.conn().await).await.unwrap();
        assert_eq!(pending, 0);
        let late = room
            .world
            .app
            .call(ApprovalDecide, &[&id])
            .token(&room.bea.token)
            .body(&ApprovalDecision::Access(AccessDecision {
                approve: false,
                permissions: None,
                options: None,
                expires_at: None,
                quick_duration: None,
                note: None,
            }))
            .send()
            .await
            .problem();
        assert_eq!(late.code, ErrorCode::ApprovalAlreadyDecided);
        if let Some(days) = days {
            room.world
                .advance(Duration::days(days) + Duration::milliseconds(1));
            // Deadline authorization does not depend on the supervisor having reaped it yet.
            let grey = room
                .world
                .app
                .call(ReferencePreviewGet, &[])
                .token(&room.gus.token)
                .query(&reference(chat))
                .send()
                .await
                .ok();
            assert!(grey.unavailable.is_some());
            room.world
                .app
                .app
                .engine
                .expire_chat_memberships()
                .await
                .unwrap();
            let ended: bool = sqlx::query_scalar(
                "SELECT left_at IS NOT NULL FROM chat_members WHERE chat_id=$1 AND user_id=$2",
            )
            .bind(chat)
            .bind(room.gus.id())
            .fetch_one(&mut *room.world.conn().await)
            .await
            .unwrap();
            assert!(ended);
            let audit:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='chat.member_removed' AND details->>'reason'='expired'").bind(chat).fetch_one(&mut *room.world.conn().await).await.unwrap();
            assert_eq!(audit, 1);
        }
        let notices:i64=sqlx::query_scalar("SELECT count(*) FROM notifications WHERE approval_id=$1 AND user_id=$2 AND kind='access.decided'").bind(id).bind(room.gus.id()).fetch_one(&mut *room.world.conn().await).await.unwrap();
        assert_eq!(notices, 1);
    }
}
