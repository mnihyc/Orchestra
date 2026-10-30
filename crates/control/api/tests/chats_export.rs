//! A chat taken away as a file over HTTP: a node's chat by those who may read it and hold its Export row, a direct or a group chat by those who are in it and nobody else, in its written form or as records, over a range of its messages, and with its files as an archive.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use bytes::Bytes;
use chat_world::{Room, text};
use orch_api_contract::access::{
    AccessChoice, Holder, HolderInput, PageChange, PageChangeAdd, PageChangeName, PageChangeRow,
    PermissionPage, PermissionPageGet, PermissionPageUpdate,
};
use orch_api_contract::chats::{ChatMessage, ChatMessageKind, ChatPart};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::exports::{ChatExportFiles, ChatExportGet, ChatExportGetQuery};
use orch_api_contract::threads::ExportFormat;
use orch_api_core::testing::RawReply;
use orch_types::Id;
use places::Who;
use std::io::Read;

async fn export(room: &Room, by: &Who, chat: Id, query: ChatExportGetQuery) -> RawReply {
    room.world
        .app
        .call(ChatExportGet, &[&chat])
        .token(&by.token)
        .query(&query)
        .send_bytes()
        .await
}

fn format(format: ExportFormat) -> ChatExportGetQuery {
    ChatExportGetQuery {
        format: Some(format),
        ..ChatExportGetQuery::default()
    }
}

fn problem(reply: &RawReply) -> orch_api_contract::common::Problem {
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn audited(room: &Room) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'chat.exported'")
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap()
}

/// Names `who` on the topic with a choice; whether they are named in the Export rows as well is `export`.
async fn name(room: &Room, who: &Who, export: Option<bool>) {
    room.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &room.topic])
        .token(&room.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: who.id() }],
                choice: AccessChoice::FullView,
                export,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn a_nodes_chat_is_taken_away_in_both_forms_over_a_range_and_each_time_is_noted() {
    let room = Room::new().await;
    for words in ["first", "second", "third"] {
        room.say(&room.ada, room.chat, words).await;
    }

    let written = export(&room, &room.ada, room.chat, ChatExportGetQuery::default()).await;
    assert_eq!(written.status, 200);
    assert_eq!(
        written.headers["content-type"],
        "text/markdown; charset=utf-8"
    );
    assert_eq!(
        written.headers["content-disposition"],
        "attachment; filename=\"Login-chat.md\""
    );
    let text_of = String::from_utf8(written.raw.to_vec()).unwrap();
    for words in ["first", "second", "third"] {
        assert!(text_of.contains(words), "{text_of}");
    }

    let records = export(&room, &room.ada, room.chat, format(ExportFormat::Json)).await;
    assert_eq!(records.status, 200);
    assert_eq!(records.headers["content-type"], "application/json");
    assert_eq!(
        records.headers["content-disposition"],
        "attachment; filename=\"Login-chat.json\""
    );
    let messages: Vec<ChatMessage> = serde_json::from_slice(&records.raw).unwrap();
    assert_eq!(
        messages.iter().map(Room::words).collect::<Vec<_>>(),
        ["first", "second", "third"]
    );

    // A range of the messages, by their numbers.
    let range = export(
        &room,
        &room.ada,
        room.chat,
        ChatExportGetQuery {
            format: Some(ExportFormat::Json),
            from: Some(2),
            to: Some(3),
            files: None,
        },
    )
    .await;
    let messages: Vec<ChatMessage> = serde_json::from_slice(&range.raw).unwrap();
    assert_eq!(
        messages.iter().map(Room::words).collect::<Vec<_>>(),
        ["second", "third"]
    );
    assert_eq!(
        audited(&room).await,
        3,
        "every time a chat is taken away is noted"
    );

    // A viewer holds the Export row as the role ships it.
    let by_viewer = export(&room, &room.bea, room.chat, ChatExportGetQuery::default()).await;
    assert_eq!(by_viewer.status, 200);
    assert_eq!(
        by_viewer.raw, written.raw,
        "the same words for everyone who may"
    );

    // A stranger may not even read it.
    let refused = export(&room, &room.eve, room.chat, ChatExportGetQuery::default()).await;
    assert_eq!(refused.status, 403);
    let refused = problem(&refused);
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(places::missing_permissions(&refused), ["chat.view"]);
    assert_eq!(audited(&room).await, 4, "and a refusal is no export");
}

fn entry_of(page: &PermissionPage, permission: &str, user: Id) -> Option<Id> {
    page.rows
        .iter()
        .find(|row| row.permission == permission)?
        .entries
        .iter()
        .find(|entry| matches!(&entry.holder, Holder::User { user: held } if held.id == user))
        .map(|entry| entry.id)
}

#[tokio::test]
async fn taking_a_nodes_chat_away_asks_for_its_export_row_and_reading_it_is_left_as_it_was() {
    let room = Room::new().await;
    room.say(&room.ada, room.chat, "to keep").await;
    // Fay is let read the topic and is not let take it away; Eve is named with the choice as it comes.
    name(&room, &room.fay, Some(false)).await;
    name(&room, &room.eve, None).await;

    let reads = room.chat(&room.fay, room.chat).await;
    assert_eq!(reads.base.id, room.chat, "Fay reads the chat");
    assert!(
        !reads.can.export,
        "and the chat says that she does not take it away"
    );
    assert!(room.chat(&room.ada, room.chat).await.can.export);
    assert_eq!(room.sync(&room.fay, room.chat).await.messages.len(), 1);
    let by_fay = export(&room, &room.fay, room.chat, ChatExportGetQuery::default()).await;
    assert_eq!(by_fay.status, 403);
    let refused = problem(&by_fay);
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(places::missing_permissions(&refused), ["chat.export"]);
    assert_eq!(audited(&room).await, 0);

    let by_eve = export(&room, &room.eve, room.chat, ChatExportGetQuery::default()).await;
    assert_eq!(
        by_eve.status, 200,
        "named with the choice, and the giver held the row"
    );
    assert!(
        String::from_utf8(by_eve.raw.to_vec())
            .unwrap()
            .contains("to keep")
    );
    assert!(room.chat(&room.eve, room.chat).await.can.export);

    // Given the row afterwards, Fay takes it away; taken out again, she does not, and still reads.
    let page: PermissionPage = room
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &room.topic])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    assert!(entry_of(&page, "chat.export", room.fay.id()).is_none());
    assert!(entry_of(&page, "chat.export", room.eve.id()).is_some());
    room.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &room.topic])
        .token(&room.ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "chat.export".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::User { id: room.fay.id() },
                    options: None,
                    expires_at: None,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        export(&room, &room.fay, room.chat, ChatExportGetQuery::default())
            .await
            .status,
        200
    );
    assert!(room.chat(&room.fay, room.chat).await.can.export);
    let page: PermissionPage = room
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &room.topic])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let entry = entry_of(&page, "chat.export", room.fay.id()).unwrap();
    room.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &room.topic])
        .token(&room.ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "chat.export".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: None,
                update: None,
                remove: Some(vec![entry]),
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        export(&room, &room.fay, room.chat, ChatExportGetQuery::default())
            .await
            .status,
        403
    );
    let reads = room.chat(&room.fay, room.chat).await;
    assert_eq!(reads.base.id, room.chat, "still reads it");
    assert!(!reads.can.export);
}

#[tokio::test]
async fn a_direct_or_a_group_chat_is_taken_away_by_those_in_it_and_by_nobody_else() {
    let room = Room::new().await;
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, direct, "private matter").await;
    room.say(&room.cai, direct, "noted").await;
    for who in [&room.bea, &room.cai] {
        let taken = export(&room, who, direct, ChatExportGetQuery::default()).await;
        assert_eq!(taken.status, 200, "{}", who.person.username);
        let words = String::from_utf8(taken.raw.to_vec()).unwrap();
        assert!(
            words.contains("private matter") && words.contains("noted"),
            "{words}"
        );
    }
    // Somebody who is not in it does not know that it is there; an admin who may read it is not in it either.
    let by_gus = export(&room, &room.gus, direct, ChatExportGetQuery::default()).await;
    assert_eq!(by_gus.status, 404);
    let by_admin = export(&room, &room.dee, direct, ChatExportGetQuery::default()).await;
    assert_eq!(
        by_admin.status, 403,
        "reading what an admin may is not taking it away"
    );
    assert_eq!(problem(&by_admin).code, ErrorCode::AuthzDenied);
    assert_eq!(audited(&room).await, 2);
    // The chat says the same to each: those in it take it away, the admin who only reads it does not.
    for who in [&room.bea, &room.cai] {
        assert!(
            room.chat(who, direct).await.can.export,
            "{}",
            who.person.username
        );
    }
    assert!(
        !room.chat(&room.dee, direct).await.can.export,
        "reading is not taking away"
    );

    // A group, in the other form.
    let group = room
        .group(&room.ada, &[&room.bea, &room.cai], Some("Release"))
        .await
        .base
        .id;
    room.say(&room.ada, group, "ship it").await;
    let records = export(&room, &room.cai, group, format(ExportFormat::Json)).await;
    assert_eq!(records.status, 200);
    let messages: Vec<ChatMessage> = serde_json::from_slice(&records.raw).unwrap();
    let said: Vec<String> = messages
        .iter()
        .filter(|message| message.kind == ChatMessageKind::Message)
        .map(Room::words)
        .collect();
    assert_eq!(
        said,
        ["ship it"],
        "the lines the system wrote about the group are records too"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.kind == ChatMessageKind::System)
    );
    assert_eq!(
        records.headers["content-disposition"],
        "attachment; filename=\"Release-chat.json\""
    );
}

#[tokio::test]
async fn a_chat_with_its_files_is_an_archive_of_the_text_and_the_files_it_names() {
    let room = Room::new().await;
    let blobs = &room.world.app.app.blobs;
    let content = Bytes::from_static(b"the notes of the meeting");
    let meta = blobs.put_bytes(content.clone()).await.unwrap();
    {
        let mut conn = room.world.conn().await;
        blobs
            .register(&mut conn, meta, room.world.now())
            .await
            .unwrap();
        orch_db::queries::uploads::remember(&mut conn, room.ada.id(), &meta.hash, room.world.now())
            .await
            .unwrap();
    }
    room.send(
        &room.ada,
        room.chat,
        vec![
            text("see the notes"),
            ChatPart::Attachment {
                blob: meta.hash,
                name: "meeting notes.txt".to_owned(),
                size: meta.size,
                media_type: "text/plain".to_owned(),
            },
        ],
        "with-a-file",
    )
    .await
    .ok();
    let alone = export(&room, &room.ada, room.chat, ChatExportGetQuery::default()).await;
    assert_eq!(
        alone.headers["content-type"],
        "text/markdown; charset=utf-8"
    );

    let with = export(
        &room,
        &room.ada,
        room.chat,
        ChatExportGetQuery {
            files: Some(ChatExportFiles::Include),
            ..ChatExportGetQuery::default()
        },
    )
    .await;
    assert_eq!(with.status, 200, "{}", String::from_utf8_lossy(&with.raw));
    assert_eq!(with.headers["content-type"], "application/zip");
    assert_eq!(
        with.headers["content-disposition"],
        "attachment; filename=\"Login-chat.zip\""
    );
    assert_eq!(
        with.headers["content-length"].to_str().unwrap(),
        with.raw.len().to_string()
    );
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(with.raw.to_vec())).unwrap();
    let names: Vec<String> = (0..zip.len())
        .map(|at| zip.by_index(at).unwrap().name().unwrap().into_owned())
        .collect();
    assert!(names.contains(&"chat.md".to_owned()), "{names:?}");
    let file = names
        .iter()
        .find(|name| name.starts_with("chat.files/") && name.ends_with("meeting notes.txt"))
        .unwrap_or_else(|| panic!("the file is beside the chat: {names:?}"))
        .clone();
    let mut bytes = Vec::new();
    zip.by_name(&file).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, content);
    let mut chat = String::new();
    zip.by_name("chat.md")
        .unwrap()
        .read_to_string(&mut chat)
        .unwrap();
    assert_eq!(
        chat.as_bytes(),
        &alone.raw[..],
        "the chat in the archive is the chat taken away alone"
    );
}
