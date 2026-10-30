//! What a message holds and tells over HTTP: the people it names and the Inbox rows that follow, text masked before it is stored, and the images and files it carries.
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
    ChatLevel, ChatMessageDelete, ChatMessageUpdate, ChatMessageUpdateBody, ChatPart, ChatRead,
    ChatReadBody, ChatStateSet, ChatStateSetBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::ImageMediaType;
use orch_api_core::routes::resources::sealing;
use orch_db::audit::{Actor, Origin};
use orch_db::secrets::MasterKey;
use orch_settings::Target;
use orch_settings::secrets::{Owner, Put, put};
use orch_types::Hash;
use serde_json::json;
use std::time::Duration;

async fn follow_at(room: &Room, who: &places::Who, level: ChatLevel) {
    room.world
        .app
        .call(ChatStateSet, &[&room.chat])
        .token(&who.token)
        .body(&ChatStateSetBody {
            level: Some(Some(level)),
            hidden: None,
        })
        .send()
        .await
        .ok();
}

// ---------------------------------------------------------------- mentions and the Inbox

#[tokio::test]
async fn a_mention_names_only_people_who_can_read_the_chat_and_tells_only_them() {
    let room = Room::new().await;
    let message = room
        .say(
            &room.ada,
            room.chat,
            "Hello @bea and @eve, and me, @ada, and @nobody-here",
        )
        .await;
    let named: Vec<String> = message
        .mentions
        .iter()
        .map(|user| user.username.clone())
        .collect();
    assert_eq!(
        named,
        ["bea"],
        "eve cannot read the chat, ada is the author"
    );
    assert!(!message.mentions_all);

    let told = room.inbox(&room.bea, "mention").await;
    assert_eq!(told.len(), 1);
    assert!(
        told[0].0.contains("mentioned you in Login"),
        "{:?}",
        told[0]
    );
    assert!(told[0].1.starts_with("Hello @bea"));
    assert_eq!(told[0].2, Some(room.chat));
    assert!(!told[0].3, "unread");
    assert!(room.inbox(&room.eve, "mention").await.is_empty());
    assert!(room.inbox(&room.ada, "mention").await.is_empty());

    let state = room.chat(&room.bea, room.chat).await.my.unwrap();
    assert_eq!((state.engaged, state.mentions_unread), (true, 1));
    assert_eq!(
        state.effective_level,
        ChatLevel::Mentions,
        "named is taking part"
    );
    assert!(
        room.chat(&room.ada, room.chat).await.my.unwrap().engaged,
        "and so is writing"
    );
}

#[tokio::test]
async fn a_chat_message_notification_is_one_row_for_the_chat_that_each_message_renews_until_it_is_read()
 {
    let room = Room::new().await;
    follow_at(&room, &room.bea, ChatLevel::All).await;
    for words in ["one", "two", "three"] {
        room.say(&room.cai, room.chat, words).await;
    }
    let rows = room.inbox(&room.bea, "chat.message").await;
    assert_eq!(rows.len(), 1, "one row for the chat, however many messages");
    assert!(
        rows[0].1.ends_with("three") && rows[0].1.starts_with("cai"),
        "{:?}",
        rows[0]
    );
    assert!(!rows[0].3);
    assert!(
        room.inbox(&room.ada, "chat.message").await.is_empty(),
        "ada follows nothing"
    );
    assert!(
        room.inbox(&room.cai, "chat.message").await.is_empty(),
        "nobody is told of their own words"
    );

    room.world
        .app
        .call(ChatRead, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatReadBody {
            seq: 3,
            read_revision: 0,
            unread_from: None,
        })
        .send()
        .await
        .ok();
    let read = room.inbox(&room.bea, "chat.message").await;
    assert!(read[0].3, "reading the chat reads its notifications");
    room.say(&room.cai, room.chat, "four").await;
    let renewed = room.inbox(&room.bea, "chat.message").await;
    assert_eq!(renewed.len(), 1);
    assert!(
        !renewed[0].3 && renewed[0].1.ends_with("four"),
        "unread again, newer"
    );
}

#[tokio::test]
async fn a_person_who_follows_at_mentions_hears_of_a_mention_and_not_of_every_message() {
    let room = Room::new().await;
    room.say(&room.bea, room.chat, "I take part").await;
    room.say(&room.cai, room.chat, "some news").await;
    assert!(room.inbox(&room.bea, "chat.message").await.is_empty());
    assert!(room.inbox(&room.bea, "mention").await.is_empty());
    room.say(&room.cai, room.chat, "@bea ping").await;
    assert_eq!(room.inbox(&room.bea, "mention").await.len(), 1);
    assert!(room.inbox(&room.bea, "chat.message").await.is_empty());
}

#[tokio::test]
async fn at_all_names_everyone_who_follows_the_chat_except_those_who_muted_it() {
    let room = Room::new().await;
    follow_at(&room, &room.bea, ChatLevel::All).await;
    room.say(&room.cai, room.chat, "I take part").await;
    room.say(&room.dee, room.chat, "so do I").await;
    follow_at(&room, &room.dee, ChatLevel::Off).await;
    let said = room.say(&room.ada, room.chat, "@all stand-up now").await;
    assert!(said.mentions_all);
    assert_eq!(room.inbox(&room.bea, "mention").await.len(), 1);
    assert_eq!(room.inbox(&room.cai, "mention").await.len(), 1);
    assert!(
        room.inbox(&room.dee, "mention").await.is_empty(),
        "dee muted the chat"
    );
    assert!(
        room.inbox(&room.eve, "mention").await.is_empty(),
        "eve follows nothing"
    );
    assert!(room.inbox(&room.ada, "mention").await.is_empty());
    assert_eq!(
        room.chat(&room.cai, room.chat)
            .await
            .my
            .unwrap()
            .mentions_unread,
        1
    );
}

#[tokio::test]
async fn an_edit_tells_the_people_it_names_newly_once_and_a_deleted_message_tells_nobody_more() {
    let room = Room::new().await;
    let message = room.say(&room.bea, room.chat, "a note").await;
    let edit = |words: &str| {
        let call = room
            .world
            .app
            .call(ChatMessageUpdate, &[&room.chat, &message.id])
            .token(&room.bea.token)
            .body(&ChatMessageUpdateBody {
                parts: vec![text(words)],
            });
        async move { call.send().await.ok() }
    };
    let named = edit("a note for @cai").await;
    assert_eq!(named.mentions[0].username, "cai");
    assert_eq!(room.inbox(&room.cai, "mention").await.len(), 1);
    edit("a note for @cai, edited").await;
    assert_eq!(
        room.inbox(&room.cai, "mention").await.len(),
        1,
        "named before, not named again"
    );
    assert_eq!(
        room.chat(&room.cai, room.chat)
            .await
            .my
            .unwrap()
            .mentions_unread,
        1
    );
    let dropped = edit("a note for nobody").await;
    assert!(dropped.mentions.is_empty());
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &message.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
}

// ---------------------------------------------------------------- masking

#[tokio::test]
async fn text_is_masked_by_the_patterns_of_the_org_and_the_secrets_of_the_topic_before_it_is_stored()
 {
    let room = Room::new().await;
    room.world
        .set_at(
            Target::Node(room.root),
            "redaction.patterns",
            json!([{ "name": "key", "regex": "sk-[a-z0-9]{6,}" }]),
        )
        .await;
    // Room's TestApp already installed the synthetic deployment key; seal this fixture
    // with that same key instead of attempting to replace the process's first key later.
    let key = MasterKey::new([7; 32]);
    sealing::install_master_key(MasterKey::new([7; 32]));
    {
        let mut conn = room.world.conn().await;
        put(
            &mut conn,
            &key,
            &Put {
                actor: Actor::User(room.ada.id()),
                owner: Owner::Node(room.topic),
                name: "DEPLOY_TOKEN",
                value: "hunter2hunter2",
                origin: &Origin::default(),
            },
            room.world.now(),
        )
        .await
        .unwrap();
    }
    let sent = room
        .say(
            &room.bea,
            room.chat,
            "the key sk-abc12345 and the token hunter2hunter2 are safe",
        )
        .await;
    assert_eq!(
        Room::words(&sent),
        "the key [masked] and the token [masked] are safe",
        "masked for everyone, the sender included"
    );
    let stored: String = sqlx::query_scalar(
        "SELECT content::text || coalesce(search_text, '') FROM chat_messages WHERE id = $1",
    )
    .bind(sent.id)
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert!(
        !stored.contains("sk-abc") && !stored.contains("hunter2"),
        "{stored}"
    );

    // The pattern belongs to the whole org; the topic's secret to the topic.
    let project = places::make_project(&room.world, &room.ada, room.root, "Mobile").await;
    let elsewhere = places::make_topic(&room.world, &room.ada, project.base.id, "Build").await;
    let there = chat_world::chat_of(&room.world, &room.ada, elsewhere.base.id).await;
    let other = room
        .say(&room.bea, there, "sk-abc12345 and hunter2hunter2")
        .await;
    assert_eq!(Room::words(&other), "[masked] and hunter2hunter2");
    // An edit is masked as a message is.
    let edited = room
        .world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &sent.id])
        .token(&room.bea.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("again hunter2hunter2")],
        })
        .send()
        .await
        .ok();
    assert_eq!(Room::words(&edited), "again [masked]");
}

// ---------------------------------------------------------------- images and files

#[tokio::test]
async fn a_message_carries_only_files_the_store_holds_and_pins_them_for_as_long_as_it_exists() {
    let room = Room::new().await;
    let (hash, size) = room.store(b"quarterly report").await;
    let file = |name: &str, size: i64, blob: Hash| ChatPart::Attachment {
        blob,
        name: name.to_owned(),
        size,
        media_type: "application/pdf".to_owned(),
    };
    let unknown = room
        .send(
            &room.bea,
            room.chat,
            vec![file("lost.pdf", 3, Hash::of(b"never stored"))],
            "a",
        )
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);
    let wrong = room
        .send(
            &room.bea,
            room.chat,
            vec![file("report.pdf", size + 1, hash)],
            "b",
        )
        .await
        .problem();
    assert_eq!(wrong.extra["errors"][0]["path"], "parts[0].size");
    assert_eq!(room.pins(&hash).await, 0, "nothing was written");

    let sent = room
        .send(
            &room.bea,
            room.chat,
            vec![
                text("here it is"),
                file("report.pdf", size, hash),
                ChatPart::Image {
                    blob: hash,
                    media_type: ImageMediaType::Png,
                    name: "scan.png".to_owned(),
                },
            ],
            "c",
        )
        .await
        .ok()
        .message;
    assert_eq!(sent.parts.len(), 3);
    assert_eq!(
        room.pins(&hash).await,
        1,
        "pinned once for the message, however many parts carry it"
    );
    let search: String = sqlx::query_scalar("SELECT search_text FROM chat_messages WHERE id = $1")
        .bind(sent.id)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert!(search.contains("report.pdf") && search.contains("here it is"));

    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &sent.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(room.pins(&hash).await, 0, "released with the message");
}

#[tokio::test]
async fn an_overlapping_edit_preserves_the_first_edit_and_the_files_other_messages_hold() {
    let room = Room::new().await;
    let (old, old_size) = room.store(b"old attachment").await;
    let (new, new_size) = room.store(b"new attachment").await;
    let attachment = |blob, size| ChatPart::Attachment {
        blob,
        name: "file.txt".to_owned(),
        size,
        media_type: "text/plain".to_owned(),
    };
    let original = vec![attachment(old, old_size)];
    let message = room
        .send(&room.bea, room.chat, original.clone(), "first")
        .await
        .ok()
        .message;
    room.send(&room.bea, room.chat, original, "second")
        .await
        .ok();
    assert_eq!(room.pins(&old).await, 2);

    // Hold the first edit's chat lock and make its new message and file pins visible only on commit.
    let mut first = room.world.db.pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *first)
        .await
        .unwrap();
    let revision: i64 = sqlx::query_scalar(
        "UPDATE chats SET next_seq = next_seq + 1 WHERE id = $1 RETURNING next_seq - 1",
    )
    .bind(room.chat)
    .fetch_one(&mut *first)
    .await
    .unwrap();
    let replacement = vec![attachment(new, new_size)];
    sqlx::query("UPDATE chat_messages SET content = $2, seq_updated = $3 WHERE id = $1")
        .bind(message.id)
        .bind(serde_json::to_value(&replacement).unwrap())
        .bind(revision)
        .execute(&mut *first)
        .await
        .unwrap();
    sqlx::query("UPDATE blobs SET pin_count = pin_count - 1 WHERE hash = $1")
        .bind(old.as_bytes().to_vec())
        .execute(&mut *first)
        .await
        .unwrap();
    sqlx::query("UPDATE blobs SET pin_count = pin_count + 1 WHERE hash = $1")
        .bind(new.as_bytes().to_vec())
        .execute(&mut *first)
        .await
        .unwrap();

    let app = room.world.app.clone();
    let token = room.bea.token.clone();
    let chat = room.chat;
    let message_id = message.id;
    let pending = tokio::spawn(async move {
        // The request is made before the await, so the path's parameters are not held across it.
        let request = app
            .call(ChatMessageUpdate, &[&chat, &message_id])
            .token(&token)
            .body(&ChatMessageUpdateBody {
                parts: vec![text("Second edit")],
            });
        request.send().await
    });
    // The database wait, not elapsed time, establishes that the second edit saw the old message.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(pid)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the second edit waits for the first");
    first.commit().await.unwrap();
    let problem = pending.await.unwrap().problem();
    assert_eq!(problem.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(problem.extra["seq_updated"], revision);
    let sync = room.sync(&room.bea, room.chat).await;
    let saved = sync
        .messages
        .iter()
        .find(|row| row.id == message.id)
        .unwrap();
    assert_eq!(saved.parts, replacement);
    assert_eq!(room.pins(&old).await, 1, "the other message keeps its file");
    assert_eq!(room.pins(&new).await, 1, "the first edit keeps its file");
}

#[tokio::test]
async fn a_file_is_at_most_the_size_the_org_allows() {
    let room = Room::new().await;
    let (hash, size) = room.store(b"0123456789 0123456789").await;
    room.world.set("chat.attachment_max_bytes", json!(10)).await;
    let too_big = room
        .send(
            &room.bea,
            room.chat,
            vec![ChatPart::Attachment {
                blob: hash,
                name: "big.txt".to_owned(),
                size,
                media_type: "text/plain".to_owned(),
            }],
            "big",
        )
        .await
        .problem();
    assert_eq!(too_big.code, ErrorCode::RequestInvalid);
    assert_eq!(too_big.extra["errors"][0]["path"], "parts[0].size");
}
