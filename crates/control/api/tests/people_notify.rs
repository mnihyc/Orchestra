//! Explicit notices keep recipient authority, anchors, replay and grants together.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
};
use orch_api_contract::common::IDEMPOTENCY_HEADER;
use orch_api_contract::documents::{DocumentGet, NewDocumentAccess};
use orch_api_contract::notifications::{PersonNoticeAnchor, PersonNotifyBody, PersonNotifyCreate};
use orch_types::{Id, new_id};
use serde_json::json;

async fn count(desk: &Desk, person: Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND kind='person.notify'",
    )
    .bind(person)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap()
}

fn grant(person: Id, notify: bool) -> PageChange {
    PageChange {
        name: Some(vec![PageChangeName {
            holders: vec![HolderInput::User { id: person }],
            choice: AccessChoice::FullView,
            export: Some(false),
            options: None,
            expires_at: None,
            note: None,
        }]),
        notify: Some(notify),
        ..PageChange::default()
    }
}

#[tokio::test]
async fn an_explicit_notice_is_one_per_reader_and_a_replay_creates_nothing_more() {
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let outsider = desk.stranger("sid").await;
    let doc = desk.make(&desk.ada, "Guide").await;
    let block = desk
        .paragraphs(&desk.ada, doc.base.id, &["Please look"])
        .await
        .remove(0);
    let body = PersonNotifyBody {
        user_ids: vec![reader.id(), reader.id()],
        anchor: Some(PersonNoticeAnchor::Block(block.id)),
        note: Some("Please review".into()),
    };
    for _ in 0..2 {
        desk.world
            .app
            .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
            .token(&desk.ada.token)
            .header(IDEMPOTENCY_HEADER, "notice-1")
            .body(&body)
            .send()
            .await
            .ok();
    }
    assert_eq!(count(&desk, reader.id()).await, 1);
    let notice: (String, String) = sqlx::query_as("SELECT body,data->>'block_id' FROM notifications WHERE user_id=$1 AND kind='person.notify'")
        .bind(reader.id()).fetch_one(&desk.world.db.pool).await.unwrap();
    assert_eq!(notice, ("Please review".into(), block.id.to_string()));
    let denied = PersonNotifyBody {
        user_ids: vec![reader.id(), outsider.id()],
        anchor: None,
        note: None,
    };
    assert!(
        !desk
            .world
            .app
            .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
            .token(&desk.ada.token)
            .body(&denied)
            .send()
            .await
            .status
            .is_success()
    );
    assert_eq!(count(&desk, reader.id()).await, 1);
    assert_eq!(count(&desk, outsider.id()).await, 0);
    assert!(
        !desk
            .world
            .app
            .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
            .token(&desk.ada.token)
            .header(IDEMPOTENCY_HEADER, "notice-1")
            .body(&denied)
            .send()
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn access_and_the_requested_notice_commit_together() {
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let mut body = desk::body("Private");
    body.access = Some(NewDocumentAccess::Private);
    let doc = desk.make_body(&desk.ada, body).await.ok();
    let block = desk
        .paragraphs(&desk.ada, doc.base.id, &["Read here"])
        .await
        .remove(0);
    let invalid = PageChange {
        notify_anchor: Some(PersonNoticeAnchor::Block(new_id())),
        ..grant(reader.id(), true)
    };
    let denied = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&invalid)
        .send()
        .await;
    assert!(!denied.status.is_success());
    assert_eq!(count(&desk, reader.id()).await, 0);
    assert!(
        !desk
            .world
            .app
            .call(DocumentGet, &[&doc.base.id])
            .token(&reader.token)
            .send()
            .await
            .status
            .is_success()
    );
    let accepted = PageChange {
        notify_anchor: Some(PersonNoticeAnchor::Block(block.id)),
        ..grant(reader.id(), true)
    };
    for _ in 0..2 {
        desk.world
            .app
            .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
            .token(&desk.ada.token)
            .header(IDEMPOTENCY_HEADER, "grant-1")
            .body(&accepted)
            .send()
            .await
            .ok();
    }
    assert_eq!(count(&desk, reader.id()).await, 1);
    assert!(
        desk.world
            .app
            .call(DocumentGet, &[&doc.base.id])
            .token(&reader.token)
            .send()
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn the_person_notification_rate_can_be_changed_or_switched_off() {
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let doc = desk.make(&desk.ada, "Guide").await;
    desk.world
        .set("notifications.person_rate_per_hour", json!(1))
        .await;
    let body = PersonNotifyBody {
        user_ids: vec![reader.id()],
        anchor: None,
        note: None,
    };
    desk.world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let blocked = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(blocked.status.as_u16(), 429);
    assert!(blocked.headers.contains_key("retry-after"));
    assert_eq!(count(&desk, reader.id()).await, 1);
    desk.world
        .set("notifications.person_rate_per_hour", json!(0))
        .await;
    desk.world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(count(&desk, reader.id()).await, 2);
}

#[tokio::test]
async fn the_optional_note_uses_the_resolved_character_limit() {
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let doc = desk.make(&desk.ada, "Guide").await;
    desk.world
        .set("notifications.person_note_chars", json!(3))
        .await;
    let body = |note: &str| PersonNotifyBody {
        user_ids: vec![reader.id()],
        anchor: None,
        note: Some(note.to_owned()),
    };
    desk.world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&body("好😀x"))
        .send()
        .await
        .ok();
    let too_long = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&body("好😀xy"))
        .send()
        .await;
    assert!(!too_long.status.is_success());
    let multiline = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&body("a\nb"))
        .send()
        .await;
    assert!(!multiline.status.is_success());
    let separator = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&body("a\u{2028}b"))
        .send()
        .await;
    assert!(!separator.status.is_success());
    assert_eq!(count(&desk, reader.id()).await, 1);
}

#[tokio::test]
async fn a_group_member_grant_and_notice_are_one_replayable_act() {
    use chat_world::Room;
    use orch_api_contract::chats::ChatGet;
    use orch_api_contract::notifications::{
        ChatPersonGrant, ChatPersonGrantBody, PersonNotifyContextGet,
    };
    let room = Room::new().await;
    let chat = room
        .group(&room.ada, &[&room.bea, &room.cai], Some("Team"))
        .await
        .base
        .id;
    let message = room.say(&room.ada, chat, "Please read").await;
    let context = room
        .world
        .app
        .call(PersonNotifyContextGet, &[&"chats", &chat, &room.gus.id()])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(context.can_view, Some(false));
    assert_eq!(context.gives, [AccessChoice::FullInteract]);
    let body = |anchor| ChatPersonGrantBody {
        choice: AccessChoice::FullInteract,
        notify: true,
        anchor: Some(PersonNoticeAnchor::Message(anchor)),
    };
    let invalid = room
        .world
        .app
        .call(ChatPersonGrant, &[&chat, &room.gus.id()])
        .token(&room.ada.token)
        .body(&body(new_id()))
        .send()
        .await;
    assert!(!invalid.status.is_success());
    assert!(
        !room
            .world
            .app
            .call(ChatGet, &[&chat])
            .token(&room.gus.token)
            .send()
            .await
            .status
            .is_success()
    );
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND kind='person.notify'",
    )
    .bind(room.gus.id())
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert_eq!(before, 0);
    for _ in 0..2 {
        room.world
            .app
            .call(ChatPersonGrant, &[&chat, &room.gus.id()])
            .token(&room.ada.token)
            .header(IDEMPOTENCY_HEADER, "chat-grant")
            .body(&body(message.id))
            .send()
            .await
            .ok();
    }
    assert!(
        room.world
            .app
            .call(ChatGet, &[&chat])
            .token(&room.gus.token)
            .send()
            .await
            .status
            .is_success()
    );
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT data->>'message_id' FROM notifications WHERE user_id=$1 AND kind='person.notify'",
    )
    .bind(room.gus.id())
    .fetch_all(&room.world.db.pool)
    .await
    .unwrap();
    assert_eq!(rows, [message.id.to_string()]);
}

#[tokio::test]
async fn a_node_chat_grant_does_not_open_the_place_or_its_documents() {
    use chat_world::Room;
    use orch_api_contract::access::PageChangeWhole;
    use orch_api_contract::chats::ChatGet;
    use orch_api_contract::nodes::NodeGet;
    use orch_api_contract::notifications::{ChatPersonGrant, ChatPersonGrantBody};
    let room = Room::new().await;
    room.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &room.topic])
        .token(&room.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert!(
        !room
            .world
            .app
            .call(ChatGet, &[&room.chat])
            .token(&room.gus.token)
            .send()
            .await
            .status
            .is_success()
    );
    room.world
        .app
        .call(ChatPersonGrant, &[&room.chat, &room.gus.id()])
        .token(&room.ada.token)
        .body(&ChatPersonGrantBody {
            choice: AccessChoice::FullView,
            notify: false,
            anchor: None,
        })
        .send()
        .await
        .ok();
    assert!(
        room.world
            .app
            .call(ChatGet, &[&room.chat])
            .token(&room.gus.token)
            .send()
            .await
            .status
            .is_success()
    );
    assert!(
        !room
            .world
            .app
            .call(NodeGet, &[&room.topic])
            .token(&room.gus.token)
            .send()
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn a_disabled_notice_refuses_the_whole_act_without_telling_the_sender_why() {
    use orch_api_contract::notifications::PersonNotifyContextGet;
    let desk = Desk::new().await;
    let one = desk.member("bea", "viewer").await;
    let two = desk.member("cai", "viewer").await;
    let doc = desk.make(&desk.ada, "Guide").await;
    sqlx::query("INSERT INTO notification_prefs(user_id,disabled_kinds) VALUES($1,'[\"person.notify\"]'::jsonb) ON CONFLICT(user_id) DO UPDATE SET disabled_kinds=EXCLUDED.disabled_kinds")
        .bind(two.id()).execute(&desk.world.db.pool).await.unwrap();
    let context = desk
        .world
        .app
        .call(
            PersonNotifyContextGet,
            &[&"documents", &doc.base.id, &two.id()],
        )
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(!context.can_notify);
    let body = PersonNotifyBody {
        user_ids: vec![one.id(), two.id()],
        anchor: None,
        note: None,
    };
    let denied = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await;
    assert!(!denied.status.is_success());
    assert_eq!(count(&desk, one.id()).await, 0);
    assert_eq!(count(&desk, two.id()).await, 0);
}

#[tokio::test]
async fn a_notice_in_one_root_does_not_consume_another_roots_allowance() {
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let first = desk.make(&desk.ada, "First").await;
    let other = places::make_org(&desk.world, &desk.ada, "Other", "other")
        .await
        .base
        .id;
    places::join(&desk.world, other, other, &reader, "viewer", &desk.ada).await;
    let project = places::make_project(&desk.world, &desk.ada, other, "Work")
        .await
        .base
        .id;
    let topic = places::make_topic(&desk.world, &desk.ada, project, "Notes")
        .await
        .base
        .id;
    let second = desk
        .make_at(&desk.ada, topic, desk::body("Second"))
        .await
        .ok();
    desk.world
        .set("notifications.person_rate_per_hour", json!(1))
        .await;
    let body = PersonNotifyBody {
        user_ids: vec![reader.id()],
        anchor: None,
        note: None,
    };
    for id in [first.base.id, second.base.id] {
        desk.world
            .app
            .call(PersonNotifyCreate, &[&"documents", &id])
            .token(&desk.ada.token)
            .body(&body)
            .send()
            .await
            .ok();
    }
    assert_eq!(count(&desk, reader.id()).await, 2);
    let blocked = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &first.base.id])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(blocked.status.as_u16(), 429);
}

#[tokio::test]
async fn an_opted_out_notice_rolls_back_the_associated_grant() {
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let mut body = desk::body("Private");
    body.access = Some(NewDocumentAccess::Private);
    let doc = desk.make_body(&desk.ada, body).await.ok();
    sqlx::query("INSERT INTO notification_prefs(user_id,disabled_kinds) VALUES($1,'[\"person.notify\"]'::jsonb) ON CONFLICT(user_id) DO UPDATE SET disabled_kinds=EXCLUDED.disabled_kinds")
        .bind(reader.id()).execute(&desk.world.db.pool).await.unwrap();
    let denied = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&grant(reader.id(), true))
        .send()
        .await;
    assert!(!denied.status.is_success());
    assert!(
        !desk
            .world
            .app
            .call(DocumentGet, &[&doc.base.id])
            .token(&reader.token)
            .send()
            .await
            .status
            .is_success()
    );
    assert_eq!(count(&desk, reader.id()).await, 0);
}

#[tokio::test]
async fn a_mention_in_the_initial_document_blocks_does_not_send_a_notice() {
    use orch_api_contract::documents::{BlockContent, BlockType, Inline, NewBlock};
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let mut body = desk::body("New page");
    body.blocks = Some(vec![NewBlock {
        id: new_id(),
        block_type: BlockType::Paragraph,
        props: None,
        content: Some(BlockContent::Inline(vec![Inline::Mention {
            user_id: reader.id(),
            user: None,
        }])),
        children: None,
    }]);
    desk.make_body(&desk.ada, body).await.ok();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND kind='doc.mention'",
    )
    .bind(reader.id())
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn concurrent_retries_of_one_notice_create_one_inbox_item() {
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let doc = desk.make(&desk.ada, "Guide").await;
    let body = PersonNotifyBody {
        user_ids: vec![reader.id()],
        anchor: None,
        note: None,
    };
    let send = || {
        desk.world
            .app
            .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
            .token(&desk.ada.token)
            .header(IDEMPOTENCY_HEADER, "same-concurrent-act")
            .body(&body)
            .send()
    };
    let (first, second) = tokio::join!(send(), send());
    first.ok();
    second.ok();
    assert_eq!(count(&desk, reader.id()).await, 1);
}

#[tokio::test]
async fn a_deleted_document_no_longer_exposes_its_notice_in_the_inbox() {
    use orch_api_contract::documents::DocumentDelete;
    use orch_api_contract::notifications::{NotificationList, NotificationListQuery};
    let desk = Desk::new().await;
    let reader = desk.member("bea", "viewer").await;
    let doc = desk.make(&desk.ada, "Private title").await;
    let notice = PersonNotifyBody {
        user_ids: vec![reader.id()],
        anchor: None,
        note: Some("Private note".into()),
    };
    desk.world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .header(IDEMPOTENCY_HEADER, "deleted-notice")
        .body(&notice)
        .send()
        .await
        .ok();
    let query = NotificationListQuery {
        kind: Some("person.notify".into()),
        ..NotificationListQuery::default()
    };
    let before = desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&reader.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(before.items.len(), 1);
    assert_eq!(before.items[0].body, "Private note");
    desk.world
        .app
        .call(DocumentDelete, &[&doc.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let after = desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&reader.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(after.items.is_empty());
    let replay = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .header(IDEMPOTENCY_HEADER, "deleted-notice")
        .body(&notice)
        .send()
        .await;
    assert!(!replay.status.is_success());
    assert_eq!(count(&desk, reader.id()).await, 1);
}

#[tokio::test]
async fn an_audience_larger_than_the_rate_is_refused_without_a_false_retry_time() {
    let desk = Desk::new().await;
    let one = desk.member("bea", "viewer").await;
    let two = desk.member("cai", "viewer").await;
    let doc = desk.make(&desk.ada, "Guide").await;
    desk.world
        .set("notifications.person_rate_per_hour", json!(1))
        .await;
    let reply = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PersonNotifyBody {
            user_ids: vec![one.id(), two.id()],
            anchor: None,
            note: None,
        })
        .send()
        .await;
    assert_eq!(reply.status.as_u16(), 429);
    assert!(!reply.headers.contains_key("retry-after"));
    assert_eq!(count(&desk, one.id()).await, 0);
    assert_eq!(count(&desk, two.id()).await, 0);
}

#[tokio::test]
async fn a_service_identity_is_not_offered_as_a_person_to_notify() {
    use orch_api_contract::chats::ChatMentionListQuery;
    use orch_api_contract::directory::{OrgServiceAccountCreate, OrgServiceAccountCreateBody};
    use orch_api_contract::documents::DocumentMentionList;
    let desk = Desk::new().await;
    let service = desk
        .world
        .app
        .call(OrgServiceAccountCreate, &[&desk.org])
        .token(&desk.ada.token)
        .body(&OrgServiceAccountCreateBody {
            username: "ci-robot".into(),
            display_name: "CI robot".into(),
        })
        .send()
        .await
        .ok();
    let mut conn = desk.world.conn().await;
    let directory =
        orch_db::queries::identity::search_users(&mut conn, desk.ada.id(), "ci-robot", None, 50)
            .await
            .unwrap();
    assert!(directory.iter().any(|person| person.id == service.user.id));
    drop(conn);
    let mut body = desk::body("Private");
    body.access = Some(NewDocumentAccess::Private);
    let doc = desk.make_body(&desk.ada, body).await.ok();
    let candidates = desk
        .world
        .app
        .call(DocumentMentionList, &[&doc.base.id])
        .token(&desk.ada.token)
        .query(&ChatMentionListQuery {
            q: Some("ci-robot".into()),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(
        candidates
            .items
            .iter()
            .all(|candidate| candidate.user.id != service.user.id)
    );
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&grant(service.user.id, false))
        .send()
        .await
        .ok();
    let after = desk
        .world
        .app
        .call(DocumentMentionList, &[&doc.base.id])
        .token(&desk.ada.token)
        .query(&ChatMentionListQuery {
            q: Some("ci-robot".into()),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(
        after
            .items
            .iter()
            .all(|candidate| candidate.user.id != service.user.id)
    );
}
