//! The chat beside every node over HTTP: posting, replying, editing, deleting with masking, reacting, pinning, reading by place and by sequence, mentions, unread state, the bell, drafts, and what each is refused for.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::{Room, missing_permissions, text};
use orch_api_contract::chats::{
    ChatCanReason, ChatDraftDelete, ChatDraftGet, ChatDraftSet, ChatDraftSetBody, ChatKind,
    ChatLevel, ChatMessageCreate, ChatMessageCreateBody, ChatMessageDelete, ChatMessageList,
    ChatMessageListQuery, ChatMessagePinDelete, ChatMessagePinSet, ChatMessageReactionAdd,
    ChatMessageReactionRemove, ChatMessageUpdate, ChatMessageUpdateBody, ChatPart, ChatRead,
    ChatReadBody, ChatStateSet, ChatStateSetBody, ChatSyncGet, ChatSyncGetQuery,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::Author;
use orch_api_contract::nodes::{NodeArchive, NodeGet};
use orch_api_contract::personal::MeChatStateList;
use orch_db::queries::access::PageKey;
use orch_db::queries::access::testing::add_entry;
use orch_settings::Target;
use orch_types::Id;
use serde_json::json;

fn author_name(message: &orch_api_contract::chats::ChatMessage) -> String {
    match &message.author {
        Some(Author::User(user)) => user.username.clone(),
        other => panic!("not a person: {other:?}"),
    }
}

// ---------------------------------------------------------------- posting and reading

#[tokio::test]
async fn a_viewer_posts_in_the_chat_of_a_topic_and_whoever_can_view_it_reads_what_was_said() {
    let room = Room::new().await;
    let sent = room
        .send(&room.bea, room.chat, vec![text("Hello team")], "first")
        .await;
    assert_eq!(sent.status, 201);
    let reply = sent.ok();
    assert_eq!(Room::words(&reply.message), "Hello team");
    assert_eq!(author_name(&reply.message), "bea");
    assert!(reply.seq >= reply.message.seq_created);

    let seen = room.sync(&room.cai, room.chat).await;
    assert_eq!(seen.messages.len(), 1);
    assert_eq!(seen.messages[0].id, reply.message.id);
    assert!(seen.complete && !seen.has_older);
    assert_eq!(seen.chat.base.kind, ChatKind::Node);
    assert_eq!(
        seen.chat
            .base
            .node
            .as_ref()
            .and_then(|node| node.name.as_deref()),
        Some("Login")
    );
    assert!(seen.chat.members.is_none() && seen.chat.guests.is_empty());
    assert_eq!(seen.chat.last_message_seq, reply.message.seq_created);
    assert!(seen.chat.last_message_at.is_some());
    assert!(
        seen.chat.can.send && seen.chat.can.moderate,
        "a developer holds chat.manage as shipped"
    );
    let bea_sees = room.chat(&room.bea, room.chat).await;
    assert!(bea_sees.can.send && !bea_sees.can.moderate);
}

#[tokio::test]
async fn somebody_who_holds_nothing_on_a_node_is_refused_its_chat_with_what_they_lack() {
    let room = Room::new().await;
    for refused in [
        room.world
            .app
            .call(orch_api_contract::chats::ChatGet, &[&room.chat])
            .token(&room.eve.token)
            .send()
            .await
            .problem(),
        room.send(&room.eve, room.chat, vec![text("let me in")], "c")
            .await
            .problem(),
    ] {
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        assert_eq!(refused.extra["required"], "chat.view");
        assert_eq!(missing_permissions(&refused), ["chat.view"]);
    }
    assert!(room.sync(&room.ada, room.chat).await.messages.is_empty());
}

#[tokio::test]
async fn sending_again_with_the_same_client_id_returns_the_stored_message_and_writes_nothing() {
    let room = Room::new().await;
    let first = room
        .send(&room.bea, room.chat, vec![text("once")], "same")
        .await;
    assert_eq!(first.status, 201);
    let first = first.ok();
    let again = room
        .send(&room.bea, room.chat, vec![text("once more")], "same")
        .await;
    assert_eq!(again.status, 200);
    let again = again.ok();
    assert_eq!(again.message.id, first.message.id);
    assert_eq!(
        Room::words(&again.message),
        "once",
        "what was stored, not what was sent now"
    );
    assert_eq!(room.sync(&room.bea, room.chat).await.messages.len(), 1);
    // Another author's value is theirs.
    let other = room
        .send(&room.cai, room.chat, vec![text("mine")], "same")
        .await;
    assert_eq!(other.status, 201);
}

#[tokio::test]
async fn a_message_has_between_one_part_and_thirty_two_and_at_most_eight_thousand_characters_of_text()
 {
    let room = Room::new().await;
    let refuse = |parts: Vec<ChatPart>, client: &'static str| {
        let room = &room;
        async move {
            let problem = room
                .send(&room.bea, room.chat, parts, client)
                .await
                .problem();
            assert_eq!(problem.code, ErrorCode::RequestInvalid, "{client}");
            problem
        }
    };
    refuse(vec![], "none").await;
    refuse(vec![text("   \n")], "blank").await;
    refuse(vec![text("x"); 33], "many").await;
    refuse(vec![text(&"x".repeat(8001))], "long").await;
    let empty_id = room.send(&room.bea, room.chat, vec![text("hi")], "").await;
    assert_eq!(empty_id.problem().code, ErrorCode::RequestInvalid);
    let copy = serde_json::from_value::<ChatPart>(json!({
        "type": "forwarded",
        "from": { "kind": "thread", "thread": { "id": room.topic, "kind": "thread", "name": "x", "redacted": false } },
        "source_id": room.topic, "author": null, "at": "2026-10-08T17:04:05.123Z",
        "text": "Bea said so", "images": [], "attachments": []
    }))
    .unwrap();
    let forged = refuse(vec![copy], "forged").await;
    assert_eq!(forged.extra["errors"][0]["path"], "parts[0]");
    // The limits themselves are allowed.
    room.send(&room.bea, room.chat, vec![text(&"x".repeat(8000))], "text")
        .await
        .ok();
    room.send(&room.bea, room.chat, vec![text("p"); 32], "parts")
        .await
        .ok();
}

#[tokio::test]
async fn a_reply_names_the_message_it_answers_and_shows_the_start_of_it() {
    let room = Room::new().await;
    let asked = room
        .say(
            &room.ada,
            room.chat,
            &format!("Can you check this? {}", "x".repeat(300)),
        )
        .await;
    let answer = room
        .world
        .app
        .call(ChatMessageCreate, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatMessageCreateBody {
            parts: vec![text("On it")],
            agent_review: None,
            reply_to: Some(asked.id),
            client_id: "reply".to_owned(),
        })
        .send()
        .await
        .ok()
        .message;
    let reply = answer.reply_to.unwrap();
    assert_eq!(reply.id, asked.id);
    assert_eq!(reply.excerpt.chars().count(), 120);
    assert!(reply.excerpt.starts_with("Can you check this?"));
    assert!(!reply.deleted);
    assert!(matches!(reply.author, Some(Author::User(user)) if user.username == "ada"));
    let elsewhere = room
        .world
        .app
        .call(ChatMessageCreate, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatMessageCreateBody {
            parts: vec![text("to nobody")],
            agent_review: None,
            reply_to: Some(orch_types::new_id()),
            client_id: "lost".to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!(elsewhere.code, ErrorCode::RequestInvalid);
}

// ---------------------------------------------------------------- editing and deleting

#[tokio::test]
async fn only_the_author_edits_a_message_and_it_is_marked_as_edited() {
    let room = Room::new().await;
    let message = room.say(&room.bea, room.chat, "teh plan").await;
    let edit = |by: &places::Who, words: &str| {
        let call = room
            .world
            .app
            .call(ChatMessageUpdate, &[&room.chat, &message.id])
            .token(&by.token)
            .body(&ChatMessageUpdateBody {
                parts: vec![text(words)],
            });
        async move { call.send().await }
    };
    for stranger in [&room.cai, &room.ada, &room.dee] {
        let refused = edit(stranger, "the plan").await.problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
    }
    let edited = edit(&room.bea, "the plan").await.ok();
    assert_eq!(Room::words(&edited), "the plan");
    assert!(edited.edited_at.is_some());
    assert!(edited.seq_updated > message.seq_updated);
    assert_eq!(edited.seq_created, message.seq_created);
    // The same words again change nothing.
    let same = edit(&room.bea, "the plan").await.ok();
    assert_eq!(same.seq_updated, edited.seq_updated);
    assert_eq!(
        edit(&room.bea, "").await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn deleting_a_message_leaves_a_row_that_shows_nothing_of_what_it_said() {
    let room = Room::new().await;
    let secret = room
        .say(&room.bea, room.chat, "the wifi password is hunter2 @cai")
        .await;
    let reply = room
        .world
        .app
        .call(ChatMessageCreate, &[&room.chat])
        .token(&room.cai.token)
        .body(&ChatMessageCreateBody {
            parts: vec![text("noted")],
            agent_review: None,
            reply_to: Some(secret.id),
            client_id: "r".to_owned(),
        })
        .send()
        .await
        .ok()
        .message;
    let emoji = "%F0%9F%91%8D";
    room.world
        .app
        .call(ChatMessageReactionAdd, &[&room.chat, &secret.id, &emoji])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    room.world
        .app
        .call(ChatMessagePinSet, &[&room.chat, &secret.id])
        .token(&room.cai.token)
        .send()
        .await
        .ok();

    let gone = room
        .world
        .app
        .call(ChatMessageDelete, &[&room.chat, &secret.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert!(gone.deleted);
    assert!(gone.parts.is_empty() && gone.reactions.is_empty() && gone.mentions.is_empty());
    assert!(gone.pinned.is_none() && !gone.mentions_all);
    assert!(gone.seq_updated > secret.seq_updated);

    let seen = room.sync(&room.ada, room.chat).await;
    let row = seen.messages.iter().find(|m| m.id == secret.id).unwrap();
    assert!(
        row.deleted && row.parts.is_empty(),
        "the row stays, emptied"
    );
    let answer = seen.messages.iter().find(|m| m.id == reply.id).unwrap();
    let quoted = answer.reply_to.as_ref().unwrap();
    assert!(
        quoted.deleted && quoted.excerpt.is_empty(),
        "a reply shows nothing of it either"
    );
    let stored: (serde_json::Value, Option<String>) =
        sqlx::query_as("SELECT content, search_text FROM chat_messages WHERE id = $1")
            .bind(secret.id)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert_eq!(stored, (json!([]), None), "nothing of the text is kept");
    // Deleting it again is the same answer.
    let again = room
        .world
        .app
        .call(ChatMessageDelete, &[&room.chat, &secret.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(again.seq_updated, gone.seq_updated);
    // Nothing can be said to it any more.
    let react = room
        .world
        .app
        .call(ChatMessageReactionAdd, &[&room.chat, &secret.id, &emoji])
        .token(&room.cai.token)
        .send()
        .await
        .problem();
    assert_eq!(react.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn whoever_holds_chat_manage_deletes_the_message_of_another_and_the_audit_says_so() {
    let room = Room::new().await;
    let message = room.say(&room.bea, room.chat, "off topic").await;
    let by_viewer = room
        .world
        .app
        .call(
            ChatMessageDelete,
            &[&room.chat, &room.say(&room.cai, room.chat, "mine").await.id],
        )
        .token(&room.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(by_viewer.code, ErrorCode::AuthzDenied);
    assert_eq!(by_viewer.extra["required"], "chat.manage");

    let deleted = room
        .world
        .app
        .call(ChatMessageDelete, &[&room.chat, &message.id])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    assert!(deleted.deleted);
    let audit = room.audit_of(room.chat).await;
    assert_eq!(audit, ["chat.message_deleted"]);
    // Deleting one's own message leaves no such row.
    let own = room.say(&room.cai, room.chat, "oops").await;
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &own.id])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(room.audit_of(room.chat).await.len(), 1);
}

// ---------------------------------------------------------------- reactions and pins

#[tokio::test]
async fn a_reaction_is_made_once_by_a_person_and_taken_back_by_them_and_a_message_holds_twenty_kinds()
 {
    let room = Room::new().await;
    let message = room.say(&room.ada, room.chat, "ship it").await;
    let up = "%F0%9F%91%8D";
    let react = |by: &places::Who, emoji: &str, add: bool| {
        let path: [&dyn std::fmt::Display; 3] = [&room.chat, &message.id, &emoji];
        let adding = room
            .world
            .app
            .call(ChatMessageReactionAdd, &path)
            .token(&by.token);
        let removing = room
            .world
            .app
            .call(ChatMessageReactionRemove, &path)
            .token(&by.token);
        async move {
            if add {
                adding.send().await
            } else {
                removing.send().await
            }
        }
    };
    let first = react(&room.bea, up, true).await.ok();
    assert_eq!(first.reactions.len(), 1);
    assert_eq!(first.reactions[0].emoji, "\u{1f44d}");
    assert_eq!(first.reactions[0].users, [room.bea.id()]);
    let twice = react(&room.bea, up, true).await.ok();
    assert_eq!(
        twice.seq_updated, first.seq_updated,
        "a reaction already made changes nothing"
    );
    let second = react(&room.cai, up, true).await.ok();
    assert_eq!(
        second.reactions[0].users,
        [room.bea.id(), room.cai.id()],
        "in the order they reacted"
    );
    let gone = react(&room.bea, up, false).await.ok();
    assert_eq!(gone.reactions[0].users, [room.cai.id()]);
    let nothing = react(&room.bea, up, false).await.ok();
    assert_eq!(
        nothing.seq_updated, gone.seq_updated,
        "one that was not made is not taken back"
    );
    let last = react(&room.cai, up, false).await.ok();
    assert!(last.reactions.is_empty());
    // At most twenty different emoji.
    for number in 0..20 {
        react(&room.bea, &format!("e{number}"), true).await.ok();
    }
    let one_too_many = react(&room.bea, "e20", true).await.problem();
    assert_eq!(one_too_many.code, ErrorCode::RequestInvalid);
    // An emoji is one word without spaces.
    let spaced = react(&room.bea, "two%20words", true).await.problem();
    assert_eq!(spaced.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_message_is_pinned_and_unpinned_by_whoever_can_write_and_the_pinned_ones_are_listed() {
    let room = Room::new().await;
    let one = room.say(&room.ada, room.chat, "one").await;
    let two = room.say(&room.ada, room.chat, "two").await;
    let pinned = room
        .world
        .app
        .call(ChatMessagePinSet, &[&room.chat, &two.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(pinned.pinned.as_ref().unwrap().by.username, "bea");
    let list = |query: ChatMessageListQuery| {
        let call = room
            .world
            .app
            .call(ChatMessageList, &[&room.chat])
            .token(&room.cai.token)
            .query(&query);
        async move { call.send().await }
    };
    let pins = list(ChatMessageListQuery {
        pinned: Some(true),
        ..ChatMessageListQuery::default()
    })
    .await
    .ok();
    assert_eq!(
        pins.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        [two.id]
    );
    let again = room
        .world
        .app
        .call(ChatMessagePinSet, &[&room.chat, &two.id])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(
        again.seq_updated, pinned.seq_updated,
        "pinning a pinned message changes nothing"
    );
    let unpinned = room
        .world
        .app
        .call(ChatMessagePinDelete, &[&room.chat, &two.id])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    assert!(unpinned.pinned.is_none());
    assert!(
        list(ChatMessageListQuery {
            pinned: Some(true),
            ..ChatMessageListQuery::default()
        })
        .await
        .ok()
        .messages
        .is_empty()
    );
    let _ = one;
}

// ---------------------------------------------------------------- reading by place and by sequence

#[tokio::test]
async fn messages_are_read_before_after_or_around_a_place_and_exactly_one_place_is_named() {
    let room = Room::new().await;
    let mut said = Vec::new();
    for number in 0..6 {
        said.push(
            room.say(&room.ada, room.chat, &format!("message {number}"))
                .await,
        );
    }
    let list = |query: ChatMessageListQuery| {
        let call = room
            .world
            .app
            .call(ChatMessageList, &[&room.chat])
            .token(&room.bea.token)
            .query(&query);
        async move { call.send().await }
    };
    let words = |reply: &orch_api_contract::chats::ChatMessageListReply| {
        reply.messages.iter().map(Room::words).collect::<Vec<_>>()
    };
    let before = list(ChatMessageListQuery {
        before: Some(said[4].seq_created),
        limit: Some(2),
        ..ChatMessageListQuery::default()
    })
    .await
    .ok();
    assert_eq!(
        words(&before),
        ["message 2", "message 3"],
        "ascending, the newest of those before"
    );
    assert!(before.has_older && before.has_newer);
    let after = list(ChatMessageListQuery {
        after: Some(said[1].seq_created),
        limit: Some(2),
        ..ChatMessageListQuery::default()
    })
    .await
    .ok();
    assert_eq!(words(&after), ["message 2", "message 3"]);
    assert!(after.has_older && after.has_newer);
    let around = list(ChatMessageListQuery {
        around: Some(said[3].id),
        limit: Some(4),
        ..ChatMessageListQuery::default()
    })
    .await
    .ok();
    assert_eq!(
        words(&around),
        [
            "message 1",
            "message 2",
            "message 3",
            "message 4",
            "message 5"
        ]
    );
    let last = list(ChatMessageListQuery {
        after: Some(said[4].seq_created),
        ..ChatMessageListQuery::default()
    })
    .await
    .ok();
    assert_eq!(words(&last), ["message 5"]);
    assert!(!last.has_newer && last.has_older);
    for bad in [
        ChatMessageListQuery::default(),
        ChatMessageListQuery {
            before: Some(3),
            after: Some(1),
            ..ChatMessageListQuery::default()
        },
        ChatMessageListQuery {
            pinned: Some(false),
            ..ChatMessageListQuery::default()
        },
    ] {
        assert_eq!(list(bad).await.problem().code, ErrorCode::RequestInvalid);
    }
    let lost = list(ChatMessageListQuery {
        around: Some(orch_types::new_id()),
        ..ChatMessageListQuery::default()
    })
    .await
    .problem();
    assert_eq!(lost.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_sync_gives_the_latest_messages_and_then_only_what_changed_after_a_sequence() {
    let room = Room::new().await;
    for number in 0..5 {
        room.say(&room.ada, room.chat, &format!("message {number}"))
            .await;
    }
    let sync = |since: Option<i64>, limit: Option<u32>| {
        let call = room
            .world
            .app
            .call(ChatSyncGet, &[&room.chat])
            .token(&room.bea.token)
            .query(&ChatSyncGetQuery { since, limit });
        async move { call.send().await.ok() }
    };
    let cut = sync(None, Some(2)).await;
    assert_eq!(cut.messages.len(), 2);
    assert_eq!(Room::words(&cut.messages[1]), "message 4");
    assert!(cut.has_older && cut.complete && cut.since.is_none());
    let all = sync(None, None).await;
    assert_eq!(all.messages.len(), 5);
    assert!(!all.has_older);
    let quiet = sync(Some(all.seq), None).await;
    assert!(quiet.messages.is_empty() && quiet.complete);
    assert_eq!(quiet.since, Some(all.seq));
    // A reaction is a change of the message it is on.
    let target = all.messages[1].id;
    room.world
        .app
        .call(ChatMessageReactionAdd, &[&room.chat, &target, &"x"])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    let later = sync(Some(all.seq), None).await;
    assert_eq!(
        later.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        [target]
    );
    assert_eq!(later.messages[0].reactions[0].emoji, "x");
    // More than two thousand changes are not sent: the client loads again.
    sqlx::query(
        "INSERT INTO chat_messages (id, chat_id, kind, author_id, content, seq_created, seq_updated)
         SELECT gen_random_uuid(), $1, 'message', $2, '[]'::jsonb, g, g FROM generate_series(1000, 3100) g",
    )
    .bind(room.chat)
    .bind(room.ada.id())
    .execute(&room.world.db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chats SET next_seq = 3200 WHERE id = $1")
        .bind(room.chat)
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    let flood = sync(Some(0), None).await;
    assert!(!flood.complete && flood.messages.is_empty());
}

// ---------------------------------------------------------------- the state of the chat

#[tokio::test]
async fn reading_a_node_chat_follows_nothing_while_taking_part_follows_it_at_the_default() {
    let room = Room::new().await;
    room.say(&room.ada, room.chat, "news").await;
    // Bea reads and does not take part: she has no state, and follows nothing.
    assert!(room.chat(&room.bea, room.chat).await.my.is_none());
    let read = room
        .world
        .app
        .call(ChatRead, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatReadBody {
            seq: 1,
            read_revision: 0,
            unread_from: None,
        })
        .send()
        .await;
    assert_eq!(read.status, 204);
    // Reading does not make her follow it, and whoever reads a chat without following it is shown
    // only that it has messages: she still has no state of her own there.
    assert!(room.chat(&room.bea, room.chat).await.my.is_none());
    // Writing makes her follow it, at her default for the chat of a node.
    room.say(&room.bea, room.chat, "me too").await;
    let writing = room.chat(&room.bea, room.chat).await.my.unwrap();
    assert!(writing.engaged);
    assert_eq!(writing.level, None);
    assert_eq!(writing.effective_level, ChatLevel::Mentions);
}

#[tokio::test]
async fn marking_a_chat_read_moves_the_position_forward_never_back_and_clears_the_mentions() {
    let room = Room::new().await;
    room.say(&room.ada, room.chat, "hello @bea").await;
    room.say(&room.ada, room.chat, "again").await;
    let mark = |seq: i64| {
        let call = room
            .world
            .app
            .call(ChatRead, &[&room.chat])
            .token(&room.bea.token)
            .body(&ChatReadBody {
                seq,
                read_revision: 0,
                unread_from: None,
            });
        async move { call.send().await }
    };
    let before = room.chat(&room.bea, room.chat).await.my.unwrap();
    assert_eq!(before.mentions_unread, 1, "she was named");
    assert!(before.engaged, "and being named is taking part");
    assert_eq!(mark(2).await.status, 204);
    let after = room.chat(&room.bea, room.chat).await.my.unwrap();
    assert_eq!((after.last_read_seq, after.mentions_unread), (2, 0));
    mark(1).await;
    assert_eq!(
        room.chat(&room.bea, room.chat)
            .await
            .my
            .unwrap()
            .last_read_seq,
        2
    );
    mark(9_000).await;
    let clamped = room.chat(&room.bea, room.chat).await;
    assert_eq!(
        clamped.my.unwrap().last_read_seq,
        clamped.seq,
        "a position beyond what the chat has issued is the last one it has"
    );
    assert_eq!(mark(-1).await.problem().code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn the_bell_follows_or_mutes_a_chat_and_null_returns_to_the_default() {
    let room = Room::new().await;
    let set = |body: ChatStateSetBody| {
        let call = room
            .world
            .app
            .call(ChatStateSet, &[&room.chat])
            .token(&room.bea.token)
            .body(&body);
        async move { call.send().await }
    };
    let follows = set(ChatStateSetBody {
        level: Some(Some(ChatLevel::All)),
        hidden: None,
    })
    .await
    .ok();
    assert_eq!(
        (follows.level, follows.effective_level),
        (Some(ChatLevel::All), ChatLevel::All)
    );
    room.say(&room.bea, room.chat, "I write too").await;
    let muted = set(ChatStateSetBody {
        level: Some(Some(ChatLevel::Off)),
        hidden: None,
    })
    .await
    .ok();
    assert_eq!(
        muted.effective_level,
        ChatLevel::Off,
        "what she set wins over having written"
    );
    assert!(muted.engaged);
    let default = set(ChatStateSetBody {
        level: Some(None),
        hidden: None,
    })
    .await
    .ok();
    assert_eq!(
        (default.level, default.effective_level),
        (None, ChatLevel::Mentions)
    );
    let unchanged = set(ChatStateSetBody::default()).await.ok();
    assert_eq!(unchanged.effective_level, ChatLevel::Mentions);
    let hidden = set(ChatStateSetBody {
        level: None,
        hidden: Some(true),
    })
    .await
    .problem();
    assert_eq!(
        hidden.code,
        ErrorCode::RequestInvalid,
        "only a direct or a group chat is taken off a list"
    );
    let states = room
        .world
        .app
        .call(MeChatStateList, &[])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].chat_id, room.chat);
}

#[tokio::test]
async fn a_draft_is_kept_for_its_person_alone_and_goes_when_it_is_discarded() {
    let room = Room::new().await;
    let get = |by: &places::Who| {
        let call = room
            .world
            .app
            .call(ChatDraftGet, &[&room.chat])
            .token(&by.token);
        async move { call.send().await.ok() }
    };
    assert!(get(&room.bea).await.is_none());
    let saved = room
        .world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatDraftSetBody {
            parts: vec![text("half a thought")],
            reply_to: None,
        })
        .send()
        .await
        .ok();
    let kept = get(&room.bea).await.unwrap();
    assert_eq!(kept, saved);
    assert_eq!(Room::words_of(&kept.parts), "half a thought");
    assert!(get(&room.cai).await.is_none(), "nobody else sees it");
    let discarded = room
        .world
        .app
        .call(ChatDraftDelete, &[&room.chat])
        .token(&room.bea.token)
        .send()
        .await;
    assert_eq!(discarded.status, 204);
    assert!(get(&room.bea).await.is_none());
    let toolong = room
        .world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatDraftSetBody {
            parts: vec![text("x"); 33],
            reply_to: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(toolong.code, ErrorCode::RequestInvalid);
}

// ---------------------------------------------------------------- where a node's chat is read-only or off

#[tokio::test]
async fn a_chat_under_an_archived_node_keeps_its_history_and_takes_no_message() {
    let room = Room::new().await;
    let said = room.say(&room.bea, room.chat, "before").await;
    room.world
        .app
        .call(NodeArchive, &[&room.project])
        .token(&room.ada.token)
        .body(&places::archive_body(&room.world, &room.ada.token, room.project, true).await)
        .send()
        .await
        .ok();
    let refused = room
        .send(&room.bea, room.chat, vec![text("after")], "late")
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeArchived);
    assert_eq!(
        refused.extra["archived_node"]["id"],
        room.project.to_string()
    );
    let chat = room.chat(&room.bea, room.chat).await;
    assert!(!chat.can.send && !chat.can.moderate);
    assert_eq!(chat.can.reason, Some(ChatCanReason::Archived));
    let history = room.sync(&room.bea, room.chat).await;
    assert_eq!(history.messages[0].id, said.id);
    for refused in [
        room.world
            .app
            .call(ChatMessageDelete, &[&room.chat, &said.id])
            .token(&room.bea.token)
            .send()
            .await
            .problem(),
        room.world
            .app
            .call(ChatMessagePinSet, &[&room.chat, &said.id])
            .token(&room.bea.token)
            .send()
            .await
            .problem(),
        room.world
            .app
            .call(ChatDraftSet, &[&room.chat])
            .token(&room.bea.token)
            .body(&ChatDraftSetBody {
                parts: vec![text("x")],
                reply_to: None,
            })
            .send()
            .await
            .problem(),
    ] {
        assert_eq!(refused.code, ErrorCode::NodeArchived);
    }
}

#[tokio::test]
async fn where_chats_are_off_the_history_is_read_and_nothing_is_written() {
    let room = Room::new().await;
    room.say(&room.bea, room.chat, "while it was on").await;
    room.world
        .set_at(Target::Node(room.project), "chat.enabled", json!(false))
        .await;
    let refused = room
        .send(&room.bea, room.chat, vec![text("now")], "off")
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ChatNotAllowed);
    assert_eq!(refused.extra["reason"], "disabled");
    let chat = room.chat(&room.bea, room.chat).await;
    assert!(!chat.can.send);
    assert_eq!(chat.can.reason, Some(ChatCanReason::Disabled));
    assert_eq!(room.sync(&room.bea, room.chat).await.messages.len(), 1);
    // Another project's chat is not affected.
    let other = another_topic_chat(&room).await;
    room.say(&room.bea, other, "still on elsewhere").await;
}

async fn another_topic_chat(room: &Room) -> Id {
    room.chat_in_another_topic().await
}

#[tokio::test]
async fn a_chat_row_left_open_to_a_person_alone_is_read_by_that_person_and_by_nobody_else() {
    let room = Room::new().await;
    // Eve is named in the chat rows of the topic alone: "chat only".
    {
        let mut conn = room.world.conn().await;
        for permission in ["chat.view", "chat.interact"] {
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
    let chat = room.chat(&room.eve, room.chat).await;
    assert_eq!(
        chat.base
            .node
            .as_ref()
            .and_then(|node| node.name.as_deref()),
        Some("Login"),
        "she knows the name of the topic"
    );
    assert!(chat.can.send);
    room.say(&room.eve, room.chat, "I can write here").await;
    for node in [room.topic, room.project, room.root] {
        let refused = room
            .world
            .app
            .call(NodeGet, &[&node])
            .token(&room.eve.token)
            .send()
            .await
            .problem();
        assert_eq!(
            refused.code,
            ErrorCode::AuthzDenied,
            "nothing else of the tree"
        );
    }
    let stranger = room
        .world
        .app
        .call(orch_api_contract::chats::ChatGet, &[&room.chat])
        .token(&room.fay.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
}

// ---------------------------------------------------------------- what nobody who cannot read a chat is answered

/// Every request that is about one chat or one message of it, made as `by`, with the status and the
/// code of its answer.
async fn everything_asked(
    room: &Room,
    by: &places::Who,
    chat: Id,
    message: Id,
    person: Id,
) -> Vec<(&'static str, u16, ErrorCode)> {
    use orch_api_contract::chats::{ChatGet, ChatMemberAdd, ChatMemberRemove, ChatUpdate};
    let app = &room.world.app;
    let token = by.token.as_str();
    macro_rules! ask {
        ($name:literal, $endpoint:expr, $params:expr $(, $kind:ident($value:expr))*) => {{
            #[allow(unused_mut, reason = "a request may have a body or a query, or neither")]
            let mut call = app.call($endpoint, $params).token(token);
            $( call = ask!(@set call, $kind($value)); )*
            let reply = call.send_bytes().await;
            let problem = reply.problem();
            ($name, reply.status.as_u16(), problem.code)
        }};
        (@set $call:ident, body($value:expr)) => { $call.raw_body(&$value) };
        (@set $call:ident, query($value:expr)) => { $call.raw_query($value) };
    }
    let text_part = json!([{ "type": "text", "text": "x" }]);
    vec![
        ask!("get", ChatGet, &[&chat]),
        ask!("sync", ChatSyncGet, &[&chat]),
        ask!("list", ChatMessageList, &[&chat], query("pinned=true")),
        ask!(
            "post",
            ChatMessageCreate,
            &[&chat],
            body(json!({ "parts": text_part, "client_id": "c" }))
        ),
        ask!(
            "edit",
            ChatMessageUpdate,
            &[&chat, &message],
            body(json!({ "parts": text_part }))
        ),
        ask!("delete", ChatMessageDelete, &[&chat, &message]),
        ask!("react", ChatMessageReactionAdd, &[&chat, &message, &"x"]),
        ask!(
            "unreact",
            ChatMessageReactionRemove,
            &[&chat, &message, &"x"]
        ),
        ask!("pin", ChatMessagePinSet, &[&chat, &message]),
        ask!("unpin", ChatMessagePinDelete, &[&chat, &message]),
        ask!(
            "read",
            ChatRead,
            &[&chat],
            body(json!({ "seq": 1, "read_revision": 0 }))
        ),
        ask!(
            "state",
            ChatStateSet,
            &[&chat],
            body(json!({ "level": "all" }))
        ),
        ask!("draft", ChatDraftGet, &[&chat]),
        ask!(
            "draft set",
            ChatDraftSet,
            &[&chat],
            body(json!({ "parts": text_part }))
        ),
        ask!("draft delete", ChatDraftDelete, &[&chat]),
        ask!(
            "rename",
            ChatUpdate,
            &[&chat],
            body(json!({ "name": null }))
        ),
        ask!("add", ChatMemberAdd, &[&chat, &person]),
        ask!("remove", ChatMemberRemove, &[&chat, &person]),
    ]
}

#[tokio::test]
async fn no_request_about_a_chat_is_answered_to_somebody_who_cannot_read_it() {
    let room = Room::new().await;
    // The chat of a node: refused as a node is, with what is missing.
    let message = room.say(&room.ada, room.chat, "inside").await;
    for (name, status, code) in
        everything_asked(&room, &room.eve, room.chat, message.id, room.bea.id()).await
    {
        assert_eq!((status, code), (403, ErrorCode::AuthzDenied), "{name}");
    }
    // A direct chat of others does not exist for them, nor for another member of the org.
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    let private = room.say(&room.bea, direct, "between us").await;
    for outsider in [&room.eve, &room.gus] {
        for (name, status, code) in
            everything_asked(&room, outsider, direct, private.id, room.bea.id()).await
        {
            assert_eq!((status, code), (404, ErrorCode::NotFound), "{name}");
        }
    }
    // Nothing of it was changed.
    let seen = room.sync(&room.bea, direct).await;
    assert_eq!(seen.messages.len(), 1);
    assert!(seen.messages[0].reactions.is_empty() && seen.messages[0].pinned.is_none());
}

#[tokio::test]
async fn every_chat_of_an_org_that_a_person_follows_is_marked_read_at_once() {
    let room = Room::new().await;
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    let group = room
        .group(&room.cai, &[&room.bea], Some("Release"))
        .await
        .base
        .id;
    room.world
        .app
        .call(ChatStateSet, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatStateSetBody {
            level: Some(Some(ChatLevel::All)),
            hidden: None,
        })
        .send()
        .await
        .ok();
    let muted = room.chat_in_another_topic().await;
    room.say(&room.cai, direct, "one").await;
    room.say(&room.cai, group, "two").await;
    room.say(&room.ada, room.chat, "three @bea").await;
    room.say(&room.ada, muted, "four").await;
    let unread = |chat: Id| {
        let room = &room;
        async move {
            let chat = room.chat(&room.bea, chat).await;
            chat.last_message_seq > chat.my.map_or(0, |state| state.last_read_seq)
        }
    };
    assert!(unread(direct).await && unread(group).await && unread(room.chat).await);

    let done = room
        .world
        .app
        .call(orch_api_contract::chats::OrgChatsRead, &[&room.root])
        .token(&room.bea.token)
        .send()
        .await;
    assert_eq!(done.status, 204);
    assert!(!unread(direct).await && !unread(group).await && !unread(room.chat).await);
    let mentioned = room.chat(&room.bea, room.chat).await.my.unwrap();
    assert_eq!(mentioned.mentions_unread, 0);
    assert!(
        room.inbox(&room.bea, "chat.message")
            .await
            .iter()
            .all(|row| row.3),
        "their notifications are read too"
    );
    assert!(
        room.chat(&room.bea, muted).await.my.is_none(),
        "a chat she does not follow is left alone"
    );

    let refused = room
        .world
        .app
        .call(orch_api_contract::chats::OrgChatsRead, &[&room.root])
        .token(&room.eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn the_states_of_a_person_are_listed_with_the_direct_and_group_chats_they_are_in_even_without_a_row()
 {
    let room = Room::new().await;
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.ada, room.chat, "@bea hello").await;
    let states = room
        .world
        .app
        .call(MeChatStateList, &[])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    let of = |chat: Id| states.iter().find(|state| state.chat_id == chat).unwrap();
    assert_eq!(states.len(), 2);
    assert_eq!(
        of(direct).effective_level,
        ChatLevel::All,
        "in a direct chat without having written"
    );
    assert!(!of(direct).engaged);
    assert_eq!(of(room.chat).mentions_unread, 1);
    assert!(
        room.world
            .app
            .call(MeChatStateList, &[])
            .token(&room.eve.token)
            .send()
            .await
            .ok()
            .is_empty()
    );
}
