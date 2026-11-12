//! Operation-scoped chat writes return typed receipts without history or reply excerpts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::{Room, text};
use orch_api_contract::chats::{
    ChatMessageCreate, ChatMessageCreateBody, ChatMessageCreateResponse, ChatMessageList,
    ChatMessageListQuery, ChatMessagePinSet, ChatMessageReactionAdd, ChatMessageUpdate,
    ChatMessageUpdateBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_core::testing::Reply;
use orch_types::{Id, new_id};
use serde_json::{Value, json};

/// Real stored credentials, never a fabricated response flag. One hour fits ordinary key lifetime
/// limits while the fixture clock remains fixed, and rule mode carries no legacy scope fallback.
async fn chat_operation_key(room: &Room, chat: Id, allow: Value) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};

    let scope: KeyScope = serde_json::from_value(json!({
        "rules": [{
            "target": {"kind":"chat", "id":chat, "reach":"self"},
            "allow": allow
        }]
    }))
    .unwrap();
    assert!(scope.validate().is_empty());
    let stored = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = room.world.now();
    let mut conn = room.world.conn().await;
    create_api_token_scoped(
        &mut conn,
        &NewApiToken {
            id: new_id(),
            user_id: room.ada.id(),
            name: "Chat receipt control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&stored),
    )
    .await
    .unwrap();
    secret
}

fn receipt_of(reply: Reply<ChatMessageCreateResponse>, chat: Id, status: u16) -> (Id, i64) {
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(reply.status.as_u16(), status);
    let ChatMessageCreateResponse::Receipt(receipt) = reply.ok() else {
        panic!("a write-only chat operation returned existing content");
    };
    assert_eq!(receipt.chat_id, chat);
    assert_eq!(
        raw,
        json!({
            "projection":"receipt", "chat_id":chat,
            "message_id":receipt.message_id, "seq":receipt.seq
        }),
        "no message parts, excerpt, author names, reactions or pin can enter the receipt"
    );
    (receipt.message_id, receipt.seq)
}

fn post(words: &str) -> ChatMessageCreateBody {
    ChatMessageCreateBody {
        parts: vec![text(words)],
        reply_to: None,
        agent_review: None,
        client_id: new_id().to_string(),
    }
}

#[tokio::test]
async fn a_write_only_chat_key_keeps_fresh_and_replayed_responses_free_of_later_content() {
    let room = Room::new().await;
    assert!(room.chat(&room.ada, room.chat).await.can.send);
    let key = chat_operation_key(
        &room,
        room.chat,
        json!([{ "operation":"ChatMessageCreate", "actions":["write"] }]),
    )
    .await;
    let body = post("The supplied message");
    let first = room
        .world
        .app
        .call(ChatMessageCreate, &[&room.chat])
        .token(&key)
        .body(&body)
        .send()
        .await;
    let (message, first_seq) = receipt_of(first, room.chat, 201);
    let seen = room.sync(&room.ada, room.chat).await;
    assert_eq!(seen.messages.len(), 1);
    assert_eq!(seen.messages[0].id, message);
    assert_eq!(Room::words(&seen.messages[0]), "The supplied message");

    let edited = room
        .world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &message])
        .token(&room.ada.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("A later private edit")],
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(Room::words(&edited), "A later private edit");
    let emoji = "%F0%9F%91%8D";
    room.world
        .app
        .call(ChatMessageReactionAdd, &[&room.chat, &message, &emoji])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    room.world
        .app
        .call(ChatMessagePinSet, &[&room.chat, &message])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let later = room.sync(&room.ada, room.chat).await;
    assert!(later.seq > first_seq);
    assert!(!later.messages[0].reactions.is_empty());
    assert!(later.messages[0].pinned.is_some());

    let repeated = room
        .world
        .app
        .call(ChatMessageCreate, &[&room.chat])
        .token(&key)
        .body(&body)
        .send()
        .await;
    let (same_message, replay_seq) = receipt_of(repeated, room.chat, 200);
    assert_eq!(same_message, message);
    assert_eq!(replay_seq, later.seq);
    let final_state = room.sync(&room.ada, room.chat).await;
    assert_eq!(
        final_state.seq, later.seq,
        "the replay adds no second message or edit"
    );
    assert_eq!(final_state.messages.len(), 1);
    assert_eq!(
        Room::words(&final_state.messages[0]),
        "A later private edit"
    );
    let no_history = room
        .world
        .app
        .call(ChatMessageList, &[&room.chat])
        .token(&key)
        .query(&ChatMessageListQuery {
            after: Some(0),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(no_history.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn chat_response_content_needs_the_post_operation_read_facet_not_a_list_grant() {
    let room = Room::new().await;
    let unrelated = chat_operation_key(
        &room,
        room.chat,
        json!([
            {"operation":"ChatMessageCreate", "actions":["write"]},
            {"operation":"ChatMessageList", "actions":["read"]}
        ]),
    )
    .await;
    let own_read = chat_operation_key(
        &room,
        room.chat,
        json!([{ "operation":"ChatMessageCreate", "actions":["read","write"] }]),
    )
    .await;
    let restricted = room
        .world
        .app
        .call(ChatMessageCreate, &[&room.chat])
        .token(&unrelated)
        .body(&post("Receipt despite unrelated Read"))
        .send()
        .await;
    receipt_of(restricted, room.chat, 201);
    let full = room
        .world
        .app
        .call(ChatMessageCreate, &[&room.chat])
        .token(&own_read)
        .body(&post("Readable operation result"))
        .send()
        .await
        .ok()
        .into_full()
        .expect("the operation's Read facet and ordinary membership permit the full result");
    assert_eq!(Room::words(&full.message), "Readable operation result");
    assert_eq!(full.message.chat_id, room.chat);
    assert!(full.message.author.is_some());
}

#[tokio::test]
async fn a_simple_chat_reply_receipt_does_not_disclose_the_message_it_answers() {
    let room = Room::new().await;
    let prior = room
        .say(
            &room.bea,
            room.chat,
            "Another person's private quoted words",
        )
        .await;
    let key = chat_operation_key(
        &room,
        room.chat,
        json!([{ "operation":"ChatMessageCreate", "actions":["write"] }]),
    )
    .await;
    let mut body = post("Acknowledged");
    // This is the existing same-chat reply relation, not an ItemRef copy, forward or agent target.
    body.reply_to = Some(prior.id);
    let answer = room
        .world
        .app
        .call(ChatMessageCreate, &[&room.chat])
        .token(&key)
        .body(&body)
        .send()
        .await;
    let (message, _) = receipt_of(answer, room.chat, 201);
    let seen = room.sync(&room.ada, room.chat).await;
    let written = seen.messages.iter().find(|row| row.id == message).unwrap();
    let quoted = written.reply_to.as_ref().unwrap();
    assert_eq!(quoted.id, prior.id);
    assert_eq!(quoted.excerpt, "Another person's private quoted words");
    assert!(quoted.author.is_some());
    assert_eq!(Room::words(written), "Acknowledged");
}
