//! Screen aggregates preserve native chat admission, private drafts and each document place's current access.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod desk;
mod places;
mod support;

use chat_world::{Room, text};
use orch_api_contract::chats::{
    ChatDraftGet, ChatDraftSet, ChatDraftSetBody, ChatOpeningGet, ChatSyncGetQuery,
    NodeChatOpeningGet, NodeChatOpeningGetQuery,
};
use orch_api_contract::documents::{
    NodeDocumentList, NodeDocumentListQuery, NodeDocumentPagesGet, NodeDocumentPagesGetQuery,
};
use orch_api_contract::nodes::{NodeKind, OrgTreeGet, OrgTreeGetQuery};
use orch_api_contract::settings::NodeSettingList;
use orch_db::queries::access::{PageKey, testing::add_entry};

#[tokio::test]
async fn opening_a_chat_keeps_its_native_conversation_and_only_the_callers_draft() {
    let room = Room::new().await;
    room.say(&room.ada, room.chat, "Visible conversation").await;
    let app = &room.world.app;
    app.call(ChatDraftSet, &[&room.chat])
        .token(&room.ada.token)
        .body(&ChatDraftSetBody {
            parts: vec![text("Ada's private draft")],
            reply_to: None,
        })
        .send()
        .await
        .ok();
    let opened = app
        .call(ChatOpeningGet, &[&room.chat])
        .token(&room.ada.token)
        .query(&ChatSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(opened.sync, room.sync(&room.ada, room.chat).await);
    assert_eq!(
        opened.draft,
        app.call(ChatDraftGet, &[&room.chat])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
    );
    let other = app
        .call(ChatOpeningGet, &[&room.chat])
        .token(&room.bea.token)
        .query(&ChatSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert!(other.draft.is_none());
    assert_eq!(other.sync.messages, opened.sync.messages);
    assert_eq!(
        app.call(ChatOpeningGet, &[&room.chat])
            .token(&room.eve.token)
            .query(&ChatSyncGetQuery::default())
            .send()
            .await
            .status,
        403
    );
}

#[tokio::test]
async fn a_named_topic_never_admits_its_unreadable_parent_chat() {
    let room = Room::new().await;
    {
        let mut conn = room.world.conn().await;
        for permission in ["topic.view", "chat.view"] {
            add_entry(
                &mut conn,
                &PageKey::new("node", room.topic, "live"),
                "any",
                permission,
                ("user", Some(room.eve.id()), None),
                None,
                room.world.now(),
            )
            .await
            .unwrap();
        }
    }
    let opened = room
        .world
        .app
        .call(NodeChatOpeningGet, &[&room.topic])
        .token(&room.eve.token)
        .query(&NodeChatOpeningGetQuery {
            scope: Some(NodeKind::Project),
            chat_id: Some(chat_world::chat_of(&room.world, &room.ada, room.project).await),
            limit: Some(10),
        })
        .send()
        .await
        .ok();
    assert_eq!(opened.scopes.len(), 1);
    assert_eq!(opened.scopes[0].node.id, room.topic);
    assert_eq!(opened.opening.unwrap().sync.chat.base.id, room.chat);
    assert_eq!(
        room.world
            .app
            .call(NodeChatOpeningGet, &[&room.project])
            .token(&room.eve.token)
            .query(&NodeChatOpeningGetQuery::default())
            .send()
            .await
            .status,
        403
    );
}

#[tokio::test]
async fn batched_document_pages_keep_native_rows_and_do_not_admit_another_place() {
    let desk = desk::Desk::new().await;
    desk.make(&desk.ada, "A visible document").await;
    let outsider = desk.stranger("stranger").await;
    let foreign = places::make_org(&desk.world, &outsider, "Private team", "private-team").await;
    let expected = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&desk.ada.token)
        .query(&NodeDocumentListQuery {
            limit: Some(30),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let pages = desk
        .world
        .app
        .call(NodeDocumentPagesGet, &[&desk.topic])
        .token(&desk.ada.token)
        .query(&NodeDocumentPagesGetQuery {
            node_ids: format!("{},{}", desk.topic, foreign.base.id),
            limit: Some(30),
        })
        .send()
        .await
        .ok();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].page.as_ref(), Some(&expected));
    assert!(pages[0].error.is_none());
    assert!(pages[1].page.is_none());
    assert_eq!(pages[1].error.as_ref().unwrap().status, 403);
    assert_eq!(
        desk.world
            .app
            .call(NodeDocumentPagesGet, &[&desk.topic])
            .query(&NodeDocumentPagesGetQuery {
                node_ids: desk.topic.to_string(),
                limit: None
            })
            .send()
            .await
            .status,
        401
    );
}

#[tokio::test]
async fn the_tree_opening_carries_the_same_authorized_settings_as_the_native_reader() {
    let room = Room::new().await;
    let tree = room
        .world
        .app
        .call(OrgTreeGet, &[&room.root])
        .token(&room.ada.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    let settings = room
        .world
        .app
        .call(NodeSettingList, &[&room.root])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(tree.settings, Some(settings));
    let changed = room
        .world
        .app
        .call(OrgTreeGet, &[&room.root])
        .token(&room.ada.token)
        .query(&OrgTreeGetQuery {
            since: Some(tree.seq),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(changed.settings.is_none());
}
