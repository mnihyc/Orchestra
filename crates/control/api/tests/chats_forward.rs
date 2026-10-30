//! Forwarding over HTTP: messages copied into other chats and to people, the copies that do not change, who must be able to read and write, and all destinations or none.
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
    ChatMessage, ChatMessageDelete, ChatMessageUpdate, ChatMessageUpdateBody, ChatPart,
    ChatSyncGet, ChatSyncGetQuery, ForwardCreate, ForwardCreateBody, ForwardDestination,
    ForwardDestinationChat, ForwardDestinationThread, ForwardDestinationUser, ForwardSource,
    ForwardSourceChat,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::PastedText;
use orch_api_contract::threads::{ForwardedAuthor, ForwardedFrom};
use orch_types::{Id, new_id};
use places::Who;

fn chat(id: Id) -> ForwardDestination {
    ForwardDestination::Chat(ForwardDestinationChat { chat_id: id })
}

fn person(id: Id) -> ForwardDestination {
    ForwardDestination::User(ForwardDestinationUser { user_id: id })
}

fn body(
    from: Id,
    messages: &[Id],
    to: Vec<ForwardDestination>,
    comment: Option<&str>,
    client_id: &str,
) -> ForwardCreateBody {
    ForwardCreateBody {
        from: ForwardSource::Chat(ForwardSourceChat {
            chat_id: from,
            message_ids: messages.to_vec(),
        }),
        to,
        comment: comment.map(str::to_owned),
        client_id: client_id.to_owned(),
    }
}

async fn forward(
    room: &Room,
    by: &Who,
    body: &ForwardCreateBody,
) -> orch_api_core::testing::Reply<orch_api_contract::chats::ForwardCreateReply> {
    room.world
        .app
        .call(ForwardCreate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

fn copies(message: &ChatMessage) -> Vec<&orch_api_contract::threads::Forwarded> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Forwarded(copy) => Some(copy.as_ref()),
            _ => None,
        })
        .collect()
}

async fn total_messages(room: &Room, chat: Id) -> usize {
    room.world
        .app
        .call(ChatSyncGet, &[&chat])
        .token(&room.ada.token)
        .query(&ChatSyncGetQuery::default())
        .send()
        .await
        .ok()
        .messages
        .iter()
        .filter(|message| message.kind == orch_api_contract::chats::ChatMessageKind::Message)
        .count()
}

#[tokio::test]
async fn messages_are_forwarded_to_a_chat_and_to_a_person_as_copies_by_the_forwarder() {
    let room = Room::new().await;
    let one = room
        .say(
            &room.bea,
            room.chat,
            "The staging password rotates on Monday",
        )
        .await;
    let two = room
        .say(&room.bea, room.chat, "Ask me for the new one")
        .await;
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;

    let sent = forward(
        &room,
        &room.cai,
        &body(
            room.chat,
            &[two.id, one.id],
            vec![chat(group), person(room.dee.id())],
            Some("FYI"),
            "fwd-1",
        ),
    )
    .await;
    assert_eq!(sent.status, 201);
    let sent = sent.ok();
    assert_eq!(sent.chats.len(), 2);
    assert!(sent.threads.is_empty());
    let into_group = &sent.chats[0];
    assert_eq!(into_group.chat_id, group);
    let message = &into_group.message;
    assert_eq!(
        message.author.as_ref().map(
            |a| matches!(a, orch_api_contract::documents::Author::User(u) if u.username == "cai")
        ),
        Some(true)
    );
    assert_eq!(Room::words(message), "FYI", "the comment goes first");
    assert!(matches!(&message.parts[0], ChatPart::Text { .. }));
    let carried = copies(message);
    assert_eq!(carried.len(), 2);
    assert_eq!(
        carried[0].text, "The staging password rotates on Monday",
        "in their original order"
    );
    assert_eq!(carried[1].text, "Ask me for the new one");
    assert_eq!(carried[0].source_id, one.id);
    assert!(
        matches!(&carried[0].author, Some(ForwardedAuthor::User(user)) if user.username == "bea")
    );
    assert_eq!(carried[0].at, one.created_at);
    assert!(
        matches!(&carried[0].from, ForwardedFrom::Chat { chat } if chat.node.as_ref().and_then(|n| n.name.as_deref()) == Some("Login"))
    );

    // A person is reached by the direct chat with them, opened for the purpose.
    let direct = &sent.chats[1];
    let opened = room.chat(&room.dee, direct.chat_id).await;
    assert_eq!(opened.base.kind, orch_api_contract::chats::ChatKind::Direct);
    assert_eq!(copies(&direct.message).len(), 2);
    assert_eq!(
        room.direct(&room.cai, &room.dee).await.base.id,
        direct.chat_id
    );

    // Those at the destination need no access to where it came from.
    assert_eq!(
        room.sync(&room.gus, group).await.messages.len(),
        2,
        "the line that made the group, and the forward"
    );
    // A copy does not change when its original does.
    room.world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &one.id])
        .token(&room.bea.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("never mind")],
        })
        .send()
        .await
        .ok();
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &two.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    let after = room.sync(&room.gus, group).await;
    let held = after.messages.iter().find(|m| m.id == message.id).unwrap();
    assert_eq!(
        copies(held)[0].text,
        "The staging password rotates on Monday"
    );
    assert_eq!(copies(held)[1].text, "Ask me for the new one");
    // The people of the chat are told as of any message.
    assert_eq!(room.inbox(&room.gus, "chat.message").await.len(), 1);
}

#[tokio::test]
async fn forwarding_the_same_value_again_returns_what_was_stored_and_writes_nothing_more() {
    let room = Room::new().await;
    let said = room.say(&room.bea, room.chat, "once").await;
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;
    let request = body(room.chat, &[said.id], vec![chat(group)], None, "same");
    let first = forward(&room, &room.cai, &request).await;
    assert_eq!(first.status, 201);
    let first = first.ok();
    let again = forward(&room, &room.cai, &request).await;
    assert_eq!(again.status, 200);
    assert_eq!(again.ok().chats[0].message.id, first.chats[0].message.id);
    assert_eq!(total_messages(&room, group).await, 1);
}

#[tokio::test]
async fn a_message_that_holds_copies_carries_them_as_they_are_so_that_the_first_author_and_source_stay()
 {
    let room = Room::new().await;
    let said = room.say(&room.bea, room.chat, "the original").await;
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;
    let first = forward(
        &room,
        &room.cai,
        &body(room.chat, &[said.id], vec![chat(group)], Some("see"), "a"),
    )
    .await
    .ok();
    let relay = first.chats[0].message.id;
    // Gus passes it on to Dee.
    let second = forward(
        &room,
        &room.gus,
        &body(group, &[relay], vec![person(room.dee.id())], None, "b"),
    )
    .await
    .ok();
    let carried = copies(&second.chats[0].message);
    // The copy of Cai's words ("see") and then the original copy, unchanged.
    assert_eq!(carried.len(), 2);
    assert_eq!(carried[0].text, "see");
    assert!(
        matches!(&carried[0].author, Some(ForwardedAuthor::User(user)) if user.username == "cai")
    );
    assert_eq!(carried[1].text, "the original");
    assert!(
        matches!(&carried[1].author, Some(ForwardedAuthor::User(user)) if user.username == "bea")
    );
    assert_eq!(carried[1].source_id, said.id);
    assert!(matches!(&carried[1].from, ForwardedFrom::Chat { chat } if chat.node.is_some()));
}

#[tokio::test]
async fn a_forward_is_all_destinations_or_none_and_the_error_names_the_one_that_refused() {
    let room = Room::new().await;
    let said = room.say(&room.bea, room.chat, "to share").await;
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;
    let theirs = room.direct(&room.bea, &room.dee).await.base.id;
    let before = total_messages(&room, group).await;

    let refused = forward(
        &room,
        &room.cai,
        &body(
            room.chat,
            &[said.id],
            vec![chat(group), chat(theirs)],
            None,
            "x",
        ),
    )
    .await
    .problem();
    assert_eq!(
        refused.code,
        ErrorCode::NotFound,
        "a chat they may not read does not exist for them"
    );
    assert_eq!(
        refused.extra["object"],
        serde_json::json!({ "chat_id": theirs })
    );
    assert_eq!(
        total_messages(&room, group).await,
        before,
        "nothing was written anywhere"
    );

    let outsider = forward(
        &room,
        &room.cai,
        &body(
            room.chat,
            &[said.id],
            vec![chat(group), person(room.eve.id())],
            None,
            "y",
        ),
    )
    .await
    .problem();
    assert_eq!(outsider.code, ErrorCode::ChatNotAllowed);
    assert_eq!(outsider.extra["reason"], "not_member");
    assert_eq!(
        outsider.extra["object"],
        serde_json::json!({ "user_id": room.eve.id() })
    );
    assert_eq!(total_messages(&room, group).await, before);

    // A thread that is not there refuses the whole forward and is named; nothing is written anywhere.
    let ghost = new_id();
    let thread = forward(
        &room,
        &room.cai,
        &body(
            room.chat,
            &[said.id],
            vec![
                chat(group),
                ForwardDestination::Thread(ForwardDestinationThread { thread_id: ghost }),
            ],
            None,
            "z",
        ),
    )
    .await
    .problem();
    assert_eq!(thread.code, ErrorCode::NotFound);
    assert_eq!(
        thread.extra["object"],
        serde_json::json!({ "thread_id": ghost })
    );
    assert_eq!(total_messages(&room, group).await, before);
}

#[tokio::test]
async fn the_forwarder_must_read_the_messages_and_write_in_each_destination() {
    let room = Room::new().await;
    let said = room.say(&room.bea, room.chat, "shared").await;
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;
    // Eve cannot read the chat the messages are in.
    let cannot_read = forward(
        &room,
        &room.eve,
        &body(
            room.chat,
            &[said.id],
            vec![person(room.bea.id())],
            None,
            "a",
        ),
    )
    .await
    .problem();
    assert_eq!(cannot_read.code, ErrorCode::AuthzDenied);
    assert_eq!(cannot_read.extra["required"], "chat.view");
    // Dee reads the group as an admin and may not write in it.
    let inspecting = forward(
        &room,
        &room.dee,
        &body(room.chat, &[said.id], vec![chat(group)], None, "b"),
    )
    .await
    .problem();
    assert_eq!(inspecting.code, ErrorCode::AuthzDenied);
    assert_eq!(
        inspecting.extra["object"],
        serde_json::json!({ "chat_id": group })
    );
    // Chats are off for the topic's project: refused with the reason, naming the destination.
    room.world
        .set_at(
            orch_settings::Target::Node(room.project),
            "chat.enabled",
            serde_json::json!(false),
        )
        .await;
    let into_off = forward(
        &room,
        &room.cai,
        &body(room.chat, &[said.id], vec![chat(room.chat)], None, "d"),
    )
    .await
    .problem();
    assert_eq!(into_off.code, ErrorCode::ChatNotAllowed);
    assert_eq!(into_off.extra["reason"], "disabled");
    assert_eq!(
        into_off.extra["object"],
        serde_json::json!({ "chat_id": room.chat })
    );
}

#[tokio::test]
async fn what_is_forwarded_and_where_are_bounded_and_must_exist() {
    let room = Room::new().await;
    let mut said = Vec::new();
    for number in 0..21 {
        said.push(
            room.say(&room.bea, room.chat, &format!("m{number}"))
                .await
                .id,
        );
    }
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;
    let refuse = |request: ForwardCreateBody| {
        let room = &room;
        async move {
            let problem = forward(room, &room.cai, &request).await.problem();
            assert_eq!(
                problem.code,
                ErrorCode::RequestInvalid,
                "{:?}",
                problem.detail
            );
            problem
        }
    };
    refuse(body(room.chat, &said, vec![chat(group)], None, "many")).await;
    refuse(body(room.chat, &[], vec![chat(group)], None, "none")).await;
    refuse(body(room.chat, &said[..1], vec![], None, "nowhere")).await;
    refuse(body(
        room.chat,
        &said[..1],
        (0..11).map(|_| chat(new_id())).collect(),
        None,
        "everywhere",
    ))
    .await;
    refuse(body(
        room.chat,
        &[new_id()],
        vec![chat(group)],
        None,
        "lost",
    ))
    .await;
    // Twenty and ten are allowed; a message of another chat is not a message of this one; a deleted
    // message says nothing to forward.
    let twenty = forward(
        &room,
        &room.cai,
        &body(room.chat, &said[..20], vec![chat(group)], None, "twenty"),
    )
    .await
    .ok();
    assert_eq!(copies(&twenty.chats[0].message).len(), 20);
    let ten = forward(
        &room,
        &room.cai,
        &body(
            room.chat,
            &said[..10],
            (0..10).map(|_| person(room.gus.id())).collect(),
            None,
            "ten",
        ),
    )
    .await
    .ok();
    assert_eq!(
        ten.chats.len(),
        1,
        "ten destinations that are one chat are one"
    );
    let other = room.say(&room.cai, group, "in the group").await;
    refuse(body(
        room.chat,
        &[other.id],
        vec![chat(group)],
        None,
        "wrong chat",
    ))
    .await;
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &said[0]])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    refuse(body(
        room.chat,
        &said[..1],
        vec![chat(group)],
        None,
        "deleted",
    ))
    .await;
}

#[tokio::test]
async fn a_destination_named_twice_is_written_once() {
    let room = Room::new().await;
    let said = room.say(&room.bea, room.chat, "one copy").await;
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;
    let sent = forward(
        &room,
        &room.cai,
        &body(
            room.chat,
            &[said.id],
            vec![chat(group), chat(group)],
            None,
            "twice",
        ),
    )
    .await
    .ok();
    assert_eq!(sent.chats.len(), 1);
    assert_eq!(total_messages(&room, group).await, 1);
}

#[tokio::test]
async fn a_copy_carries_the_images_and_files_of_the_original_and_holds_them_for_as_long_as_it_exists()
 {
    let room = Room::new().await;
    let (hash, size) = room.store(b"a long report").await;
    let original = room
        .send(
            &room.bea,
            room.chat,
            vec![
                text("the report"),
                ChatPart::Attachment {
                    blob: hash,
                    name: "report.pdf".to_owned(),
                    size,
                    media_type: "application/pdf".to_owned(),
                },
                ChatPart::Image {
                    blob: hash,
                    media_type: orch_api_contract::threads::ImageMediaType::Png,
                    name: "cover.png".to_owned(),
                },
            ],
            "with files",
        )
        .await
        .ok()
        .message;
    assert_eq!(room.pins(&hash).await, 1);
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;
    let sent = forward(
        &room,
        &room.cai,
        &body(room.chat, &[original.id], vec![chat(group)], None, "files"),
    )
    .await
    .ok();
    let copy = copies(&sent.chats[0].message)[0].clone();
    assert_eq!(copy.text, "the report");
    assert_eq!((copy.attachments.len(), copy.images.len()), (1, 1));
    assert_eq!(
        (copy.attachments[0].name.as_str(), copy.attachments[0].size),
        ("report.pdf", size)
    );
    assert_eq!(
        (
            copy.images[0].name.as_str(),
            copy.images[0].media_type.as_str()
        ),
        ("cover.png", "image/png")
    );
    assert_eq!(room.pins(&hash).await, 2, "the copy holds the files too");
    // The original goes; the copy keeps its files.
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &original.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(room.pins(&hash).await, 1);
    room.world
        .app
        .call(ChatMessageDelete, &[&group, &sent.chats[0].message.id])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(room.pins(&hash).await, 0);
}

#[tokio::test]
async fn a_copied_paste_keeps_its_folded_bytes_and_provenance_but_cannot_be_forged() {
    let room = Room::new().await;
    let (blob, size) = room.store(b"folded original words").await;
    let mut paste = PastedText {
        delivery: None,
        r#type: orch_api_contract::threads::PastedTextType::PastedText,
        blob,
        name: "pasted.txt".to_owned(),
        path: String::new(),
        size,
        characters: size,
        ascii_characters: size,
        inline_text: None,
        source: None,
    };
    let original = room
        .send(
            &room.bea,
            room.chat,
            vec![ChatPart::PastedText(paste.clone())],
            "paste-original",
        )
        .await
        .ok()
        .message;
    let group = room
        .group(&room.cai, &[&room.gus], Some("Ops"))
        .await
        .base
        .id;
    let sent = forward(
        &room,
        &room.cai,
        &body(
            room.chat,
            &[original.id],
            vec![chat(group)],
            None,
            "paste-copy",
        ),
    )
    .await
    .ok();
    let copied = sent.chats[0]
        .message
        .parts
        .iter()
        .find_map(|part| match part {
            ChatPart::PastedText(paste) => Some(paste),
            _ => None,
        })
        .expect("a copied paste stays folded");
    assert_eq!(copied.blob, blob);
    assert_eq!(copied.characters, size);
    assert!(copied.inline_text.is_none());
    let source = copied
        .source
        .as_ref()
        .expect("a copied paste keeps its origin");
    assert_eq!(source.source_id, original.id);
    assert!(
        matches!(&source.author, Some(ForwardedAuthor::User(user)) if user.id == room.bea.id())
    );
    assert_eq!(room.pins(&blob).await, 2);
    let forged = room
        .send(
            &room.cai,
            group,
            vec![ChatPart::PastedText(copied.clone())],
            "forged-paste-origin",
        )
        .await
        .problem();
    assert_eq!(forged.code, ErrorCode::RequestInvalid);
    assert_eq!(room.pins(&blob).await, 2);
    paste.inline_text = Some("words not in the uploaded blob".to_owned());
    let forged = room
        .send(
            &room.bea,
            room.chat,
            vec![ChatPart::PastedText(paste)],
            "forged-paste-words",
        )
        .await
        .problem();
    assert_eq!(forged.code, ErrorCode::RequestInvalid);
    assert_eq!(room.pins(&blob).await, 2);
}
