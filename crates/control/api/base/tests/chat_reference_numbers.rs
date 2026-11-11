//! The message wire carries the exact ordinal used by written references and their preview.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod chat_world;
mod places;
mod support;
mod thread_history_scope;

use chat_world::{Room, text};
use orch_api_contract::{
    chats::{
        ChatMessageDelete, ChatMessageList, ChatMessageListQuery, ChatMessagePinSet,
        ChatMessageUpdate, ChatMessageUpdateBody, ChatSyncGet, ChatSyncGetQuery,
    },
    documents::{InlineRefKind, ReferencePreviewGet, ReferencePreviewGetQuery},
};
use serde_json::json;
use thread_history_scope::Proof;

#[tokio::test]
async fn message_ordinals_survive_change_sequence_gaps_pages_edits_and_deleted_rows() {
    let room = Room::new().await;
    let first = room.say(&room.ada, room.chat, "First message").await;
    assert!(first.reference_number.is_some_and(|number| number > 0));
    let pinned = room
        .world
        .app
        .call(ChatMessagePinSet, &[&room.chat, &first.id])
        .token(&room.ada.token)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(pinned.reference_number, first.reference_number);
    let edited = room
        .world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &first.id])
        .token(&room.ada.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("First, edited")],
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(edited.reference_number, first.reference_number);
    let second = room.say(&room.ada, room.chat, "Second message").await;
    let number = second
        .reference_number
        .expect("the stored message has an ordinal");
    assert_eq!(number, first.reference_number.unwrap() + 1);
    assert!(
        second.seq_created > number,
        "pin and edit issue changes, not message numbers"
    );
    let page = room
        .world
        .app
        .call(ChatMessageList, &[&room.chat])
        .token(&room.bea.token)
        .query(&ChatMessageListQuery {
            before: Some(second.seq_created + 1),
            limit: Some(1),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].id, second.id);
    assert_eq!(
        page.messages[0].reference_number,
        Some(number),
        "a page offset is not a reference number"
    );
    let deleted = room
        .world
        .app
        .call(ChatMessageDelete, &[&room.chat, &first.id])
        .token(&room.ada.token)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(deleted.deleted);
    assert_eq!(deleted.reference_number, first.reference_number);
    let synced = room
        .world
        .app
        .call(ChatSyncGet, &[&room.chat])
        .token(&room.bea.token)
        .query(&ChatSyncGetQuery::default())
        .send()
        .await
        .ok();
    let current = synced
        .messages
        .iter()
        .find(|message| message.id == second.id)
        .unwrap();
    assert_eq!(current.reference_number, Some(number));
    let card = room
        .world
        .app
        .call(ReferencePreviewGet, &[])
        .token(&room.bea.token)
        .query(&ReferencePreviewGetQuery {
            kind: InlineRefKind::Chat,
            id: room.chat,
            block: None,
            turn: None,
            message: Some(number),
        })
        .send()
        .await
        .ok();
    let part = card.part.expect("the exact copied reference resolves");
    assert_eq!(
        (part.number, part.anchor, part.text.as_str()),
        (number, Some(second.id), "Second message")
    );
    let mapping = orch_db::queries::shares::chat_numbers(
        &mut *room.world.conn().await,
        room.chat,
        &[first.id, second.id],
    )
    .await
    .unwrap();
    assert_eq!(mapping[&second.id], number);
    assert_eq!(Some(mapping[&first.id]), first.reference_number);
    let hidden = room
        .world
        .app
        .call(ChatMessageList, &[&room.chat])
        .token(&room.eve.token)
        .query(&ChatMessageListQuery {
            after: Some(0),
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert!(
        !hidden.status.is_success(),
        "the ordinal does not open a chat reading door"
    );
}

#[tokio::test]
async fn a_scoped_message_reader_gets_native_ordinals_only_under_the_current_chat_operation()
-> Proof {
    let room = Room::new().await;
    let message = room
        .say(&room.ada, room.chat, "An admitted message ordinal")
        .await;
    for (operation, admitted) in [("ChatGet", false), ("ChatMessageList", true)] {
        let key = thread_history_scope::key(
            &room.world,
            room.bea.id(),
            vec![json!({
                "target":{"kind":"chat","id":room.chat,"reach":"self"},
                "allow":[{"operation":operation,"actions":["read"]}]
            })],
        )
        .await?;
        let reply = room
            .world
            .app
            .call(ChatMessageList, &[&room.chat])
            .token(&key.secret)
            .query(&ChatMessageListQuery {
                after: Some(0),
                ..Default::default()
            })
            .send_bytes()
            .await;
        if admitted {
            assert_eq!(
                reply.status.as_u16(),
                200,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            let page: orch_api_contract::chats::ChatMessageListReply =
                serde_json::from_slice(&reply.raw)?;
            let found = page
                .messages
                .iter()
                .find(|row| row.id == message.id)
                .ok_or("admitted message missing")?;
            assert_eq!(found.reference_number, message.reference_number);
        } else {
            assert_eq!(reply.status.as_u16(), 403);
        }
    }
    Ok(())
}
