//! Shortcuts over HTTP: a pointer to a document, a chat, a thread or a place, kept in a place or in a person's own
//! list, which follows its target by identity, grants nothing and never touches its target.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, body};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::documents::{
    DocumentDelete, DocumentGet, DocumentMove, DocumentMoveBody, DocumentRestore, DocumentUpdate,
    DocumentUpdateBody, NewDocumentAccess, NodeDocumentCreateBody,
};
use orch_api_contract::nodes::Node;
use orch_api_contract::personal::{
    MeFavoriteList, MeItemFavoriteSet, MeItemFavoriteSetBody, NodeShortcutCreate, NodeShortcutList,
    NodeShortcutListQuery, PersonalTarget, Shortcut, ShortcutDelete, ShortcutMove,
    ShortcutMoveBody, ShortcutTargetState,
};
use orch_api_contract::search::NameKind;
use orch_db::queries::hierarchy;
use orch_types::Id;
use places::{Who, make_project, make_topic};

fn target(kind: NameKind, id: Id) -> PersonalTarget {
    PersonalTarget { kind, id }
}

async fn keep(desk: &Desk, by: &Who, place: Id, what: PersonalTarget) -> Shortcut {
    desk.world
        .app
        .call(NodeShortcutCreate, &[&place])
        .token(&by.token)
        .body(&what)
        .send()
        .await
        .ok()
}

async fn kept_in(desk: &Desk, by: &Who, place: Id) -> Vec<Shortcut> {
    let page: Page<Shortcut> = desk
        .world
        .app
        .call(NodeShortcutList, &[&place])
        .token(&by.token)
        .query(&NodeShortcutListQuery::default())
        .send()
        .await
        .ok();
    page.items
}

async fn own(desk: &Desk, by: &Who) -> Vec<Shortcut> {
    desk.world
        .app
        .call(MeFavoriteList, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
}

fn name(shortcut: &Shortcut) -> Option<&str> {
    shortcut.item.as_ref().and_then(|item| item.name.as_deref())
}

async fn chat_of(desk: &Desk, place: Id) -> Id {
    match places::read(&desk.world, &desk.ada, place).await {
        Node::Topic(topic) => topic.base.chat_id,
        Node::Project(project) => project.base.chat_id,
        Node::Org(org) => org.base.chat_id,
        Node::Thread(thread) => thread.base.chat_id,
    }
}

#[tokio::test]
async fn a_shortcut_kept_in_a_place_names_its_target_and_is_kept_once() {
    let desk = Desk::new().await;
    let elsewhere = desk.another_topic("Elsewhere").await;
    let plan = desk
        .make_at(&desk.ada, elsewhere, body("Launch plan"))
        .await
        .ok();
    let made = keep(
        &desk,
        &desk.ada,
        desk.topic,
        target(NameKind::Document, plan.base.id),
    )
    .await;
    assert_eq!(made.node_id, Some(desk.topic));
    assert_eq!(made.state, ShortcutTargetState::Open);
    assert_eq!(name(&made), Some("Launch plan"));
    let way = &made.item.as_ref().unwrap().path;
    assert_eq!(way.last().map(|step| step.id), Some(elsewhere));
    // Kept again: the same one answers, and the place holds one.
    let again = keep(
        &desk,
        &desk.ada,
        desk.topic,
        target(NameKind::Document, plan.base.id),
    )
    .await;
    assert_eq!(again.id, made.id);
    let listed = kept_in(&desk, &desk.ada, desk.topic).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, made.id);
    // A project, a topic, a chat and a thread can be pointed to as well, and a place's tree row counts them.
    let thread = orch_types::new_id();
    let mut conn = desk.world.conn().await;
    hierarchy::testing::insert_node(
        &mut conn,
        thread,
        "thread",
        Some(elsewhere),
        "Deploy",
        desk.ada.id(),
    )
    .await
    .unwrap();
    orch_authz::pages::start(
        &mut conn,
        orch_access_contract::pages::StoredKind::Node,
        thread,
        desk.ada.id(),
        orch_types::Time::new(desk.world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    let chat = chat_of(&desk, elsewhere).await;
    for what in [
        target(NameKind::Project, desk.project),
        target(NameKind::Topic, elsewhere),
        target(NameKind::Chat, chat),
        target(NameKind::Thread, thread),
    ] {
        let made = keep(&desk, &desk.ada, desk.topic, what.clone()).await;
        assert_eq!(made.state, ShortcutTargetState::Open, "{what:?}");
        assert!(name(&made).is_some(), "{what:?}");
    }
    assert_eq!(kept_in(&desk, &desk.ada, desk.topic).await.len(), 5);
    let mut conn = desk.world.conn().await;
    let rows = hierarchy::tree(&mut conn, desk.org, None, 1000)
        .await
        .unwrap();
    let row = rows.iter().find(|row| row.id == desk.topic).unwrap();
    assert_eq!(row.shortcuts, 5);
    // A thread keeps no shortcuts, and a place keeps none to what is no document, chat, thread or place.
    let refused = desk
        .world
        .app
        .call(NodeShortcutCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&target(NameKind::User, desk.ada.id()))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_shortcut_follows_its_target_through_a_rename_and_a_move() {
    let desk = Desk::new().await;
    let first = desk.another_topic("First").await;
    let second = desk.another_topic("Second").await;
    let notes = desk.make_at(&desk.ada, first, body("Notes")).await.ok();
    let made = keep(
        &desk,
        &desk.ada,
        desk.topic,
        target(NameKind::Document, notes.base.id),
    )
    .await;
    desk.world
        .app
        .call(DocumentUpdate, &[&notes.base.id])
        .token(&desk.ada.token)
        .body(&DocumentUpdateBody {
            title: Some("Meeting notes".into()),
            description: None,
        })
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(DocumentMove, &[&notes.base.id])
        .token(&desk.ada.token)
        .body(&DocumentMoveBody { node_id: second })
        .send()
        .await
        .ok();
    let listed = kept_in(&desk, &desk.ada, desk.topic).await;
    assert_eq!(listed[0].id, made.id);
    assert_eq!(name(&listed[0]), Some("Meeting notes"));
    let way = &listed[0].item.as_ref().unwrap().path;
    assert_eq!(way.last().map(|step| step.id), Some(second));
    // A place moved to another project is followed the same way.
    let other = make_project(&desk.world, &desk.ada, desk.org, "Other")
        .await
        .base
        .id;
    let topic = make_topic(&desk.world, &desk.ada, desk.project, "Wandering")
        .await
        .base
        .id;
    keep(&desk, &desk.ada, desk.topic, target(NameKind::Topic, topic)).await;
    let mut moving = desk.world.db.pool.begin().await.unwrap();
    hierarchy::move_under(&mut moving, topic, other, desk.world.now())
        .await
        .unwrap();
    moving.commit().await.unwrap();
    let listed = kept_in(&desk, &desk.ada, desk.topic).await;
    let wandering = listed.iter().find(|one| one.target.id == topic).unwrap();
    assert_eq!(
        wandering
            .item
            .as_ref()
            .unwrap()
            .path
            .last()
            .map(|step| step.id),
        Some(other)
    );
}

#[tokio::test]
async fn a_shortcut_grants_nothing_and_says_nothing_of_a_target_its_reader_may_not_open() {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let secret = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                access: Some(NewDocumentAccess::Private),
                ..body("Salaries")
            },
        )
        .await
        .ok();
    let made = keep(
        &desk,
        &desk.ada,
        desk.topic,
        target(NameKind::Document, secret.base.id),
    )
    .await;
    assert_eq!(made.state, ShortcutTargetState::Open);
    // Bea sees the place and its shortcut, not what it points to; nor may she open the document through it.
    let seen = kept_in(&desk, &bea, desk.topic).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].state, ShortcutTargetState::Unavailable);
    assert!(seen[0].item.is_none());
    assert!(!serde_json::to_string(&seen).unwrap().contains("Salaries"));
    let read = desk
        .world
        .app
        .call(DocumentGet, &[&secret.base.id])
        .token(&bea.token)
        .send()
        .await;
    assert!(!read.status.is_success());
    // She may not keep a shortcut to what she may not open either.
    let refused = desk
        .world
        .app
        .call(NodeShortcutCreate, &[&desk.topic])
        .token(&bea.token)
        .body(&target(NameKind::Document, secret.base.id))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    // Somebody who may not see the place sees none of its shortcuts.
    let stranger = desk.stranger("sid").await;
    let hidden = desk
        .world
        .app
        .call(NodeShortcutList, &[&desk.topic])
        .token(&stranger.token)
        .query(&NodeShortcutListQuery::default())
        .send()
        .await;
    assert!(!hidden.status.is_success());
    // Nor may Bea remove Ada's shortcut, which she did not put there and whose documents she does not manage.
    let kept = desk
        .world
        .app
        .call(ShortcutDelete, &[&made.id])
        .token(&bea.token)
        .send()
        .await;
    assert!(!kept.status.is_success());
    assert_eq!(kept_in(&desk, &desk.ada, desk.topic).await.len(), 1);
}

#[tokio::test]
async fn a_deleted_target_shows_as_deleted_and_comes_back_with_its_restore() {
    let desk = Desk::new().await;
    let plan = desk.make(&desk.ada, "Plan").await;
    let elsewhere = desk.another_topic("Elsewhere").await;
    keep(
        &desk,
        &desk.ada,
        elsewhere,
        target(NameKind::Document, plan.base.id),
    )
    .await;
    let deleted = desk
        .world
        .app
        .call(DocumentDelete, &[&plan.base.id])
        .token(&desk.ada.token)
        .send()
        .await;
    assert_eq!(deleted.status, 204);
    let listed = kept_in(&desk, &desk.ada, elsewhere).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, ShortcutTargetState::Deleted);
    assert!(listed[0].item.is_none());
    desk.world
        .app
        .call(DocumentRestore, &[&plan.base.id])
        .token(&desk.ada.token)
        .raw_body(&serde_json::json!({}))
        .send()
        .await
        .ok();
    let listed = kept_in(&desk, &desk.ada, elsewhere).await;
    assert_eq!(listed[0].state, ShortcutTargetState::Open);
    assert_eq!(name(&listed[0]), Some("Plan"));
    // A thread in a place that was deleted is deleted to its shortcuts too.
    let topic = make_topic(&desk.world, &desk.ada, desk.project, "Gone soon")
        .await
        .base
        .id;
    keep(&desk, &desk.ada, elsewhere, target(NameKind::Topic, topic)).await;
    sqlx::query("UPDATE nodes SET deleted_at = now() WHERE id = $1")
        .bind(topic)
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let listed = kept_in(&desk, &desk.ada, elsewhere).await;
    let gone = listed.iter().find(|one| one.target.id == topic).unwrap();
    assert_eq!(gone.state, ShortcutTargetState::Deleted);
}

#[tokio::test]
async fn removing_or_moving_a_shortcut_never_touches_its_target() {
    let desk = Desk::new().await;
    let plan = desk.make(&desk.ada, "Plan").await;
    let elsewhere = desk.another_topic("Elsewhere").await;
    let made = keep(
        &desk,
        &desk.ada,
        desk.topic,
        target(NameKind::Document, plan.base.id),
    )
    .await;
    // Moved to another place, it is there and no longer here.
    let moved: Shortcut = desk
        .world
        .app
        .call(ShortcutMove, &[&made.id])
        .token(&desk.ada.token)
        .body(&ShortcutMoveBody {
            node_id: Some(elsewhere),
        })
        .send()
        .await
        .ok();
    assert_eq!(moved.node_id, Some(elsewhere));
    assert!(kept_in(&desk, &desk.ada, desk.topic).await.is_empty());
    assert_eq!(kept_in(&desk, &desk.ada, elsewhere).await.len(), 1);
    // Into the person's own list: it is one of their stars.
    let starred: Shortcut = desk
        .world
        .app
        .call(ShortcutMove, &[&made.id])
        .token(&desk.ada.token)
        .body(&ShortcutMoveBody { node_id: None })
        .send()
        .await
        .ok();
    assert_eq!(starred.node_id, None);
    assert!(kept_in(&desk, &desk.ada, elsewhere).await.is_empty());
    let mine = own(&desk, &desk.ada).await;
    assert!(mine.iter().any(|one| one.target.id == plan.base.id
        && one.state == ShortcutTargetState::Open
        && name(one) == Some("Plan")));
    // Removed, it is gone; the document is as it was.
    let removed = desk
        .world
        .app
        .call(ShortcutDelete, &[&made.id])
        .token(&desk.ada.token)
        .send()
        .await;
    assert_eq!(removed.status, 204);
    assert!(
        own(&desk, &desk.ada)
            .await
            .iter()
            .all(|one| one.id != made.id)
    );
    let read = desk
        .world
        .app
        .call(DocumentGet, &[&plan.base.id])
        .token(&desk.ada.token)
        .send()
        .await;
    assert!(read.status.is_success());
}

#[tokio::test]
async fn a_person_s_own_list_holds_documents_and_chats_beside_places() {
    let desk = Desk::new().await;
    let plan = desk.make(&desk.ada, "Plan").await;
    let chat = chat_of(&desk, desk.topic).await;
    for what in [
        target(NameKind::Project, desk.project),
        target(NameKind::Document, plan.base.id),
        target(NameKind::Chat, chat),
    ] {
        let status = desk
            .world
            .app
            .call(MeItemFavoriteSet, &[&what.kind.as_str(), &what.id])
            .token(&desk.ada.token)
            .body(&MeItemFavoriteSetBody::default())
            .send()
            .await
            .status;
        assert_eq!(status, 204);
    }
    let mine = own(&desk, &desk.ada).await;
    let kinds = mine.iter().map(|one| one.target.kind).collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![NameKind::Project, NameKind::Document, NameKind::Chat]
    );
    assert!(mine.iter().all(|one| one.node_id.is_none()
        && one.state == ShortcutTargetState::Open
        && name(one).is_some()));
}
